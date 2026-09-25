//! Linking: the `require` closure of one entry file, as a [`Bundle`].
//!
//! Starting at the entry, every `require("<literal>")` is followed (only string
//! literals: a `require(expr)` cannot be resolved statically, list its targets under
//! `extra`). `.tl` modules are type-checked and generated; plain `.lua` modules (a
//! vendored dependency, say) are taken as they are. A name that runs from no file of the
//! project — one the host provides, one only a `.d.tl` declares — is recorded as
//! host-provided, as is anything listed in `host`. Any other unresolved `require` is an
//! error: the point of a bundle is that "module not found" happens here, not on the
//! first `require` at the customer's machine.
//!
//! # The host's names
//!
//! Which names the host provides is the project model's to say
//! (`model::Project::provided`: `#[host_module]`s in the crate around the project,
//! `[build] host`, `std.*`), and the callers that have a model — `htl build`,
//! `include_bundle!` — put all of them in [`LinkOptions::host`]. A name there is not
//! walked. When a file of the model implements it as well, the model refuses the name
//! (its resolver's `HostShadowed`): the check reports that at a checked file's
//! `require`, and the linker reports it at a plain `.lua`'s, which nothing checks — the
//! same split as for a name two files implement. The file is never bundled: every run
//! with the host loads the host's module, which `package.preload` answers first.
//!
//! # Declared names
//!
//! A name the model has only a declaration for — a `.d.tl` and no `.tl` or `.lua`, like
//! `types/socket/http.d.tl` for a LuaSocket installed on the machine — is provided by the
//! environment at run time, and is left out of the bundle the same way. That, too, is the
//! model's answer (`model::Provider::Declared`, from its resolver's `provides`), which the
//! linker asks through [`Htl::model_provides`] for a name that resolves to a declaration.
//! It does not decide it from a lookup of its own, as it did before the model answered
//! it (#321): only for a file in no project, or a name the model has nothing under, is a
//! declaration with no `.lua` found behind it on the search path taken to be the host's —
//! there is no model to ask. The outcome is the same either way; what changed is which of
//! the two says so.
//!
//! # The store
//!
//! Generating a module is the expensive part of linking — the Teal check behind it costs
//! about a second per few thousand lines, against milliseconds for Lua's own compiler to
//! turn the result into bytecode — and it is the part the run cache already answers for.
//! [`link_with`] takes a [`LinkStore`]: for every typed module it asks the store for the
//! module's `gen` entry (the one `htl test` writes and replays), and an entry whose inputs
//! and probes still hold stands in for the check, its generated Lua for `gen_lua`. A miss
//! generates as before and writes the entry, so `htl build`, `htl test` and the
//! `include_bundle!` / `include_tl!` macros feed one another (#100).
//!
//! Bytecode is never stored: compiling it is cheap, and an entry that carried it would
//! have to be keyed on the strip and debug flags and on the Lua the bytecode is for, for
//! no saving. What a bundle contains — module order, fingerprint, host modules — is the
//! same whether a module was generated or replayed.

use crate::bundle::{Bundle, Kind, Module};
use crate::cache::{self, Cache};
use crate::{CheckInfo, Htl, RequireSite};
use anyhow::{Context, Result};
use std::collections::{BTreeSet, HashSet, VecDeque};
use std::path::{Path, PathBuf};

/// What a link is asked for beyond the entry file: how modules are stored, and which ones
/// the walk would otherwise miss or must not take.
///
/// Owned and `Default`, and it crosses the proc-macro boundary as a value — the borrowed
/// half of a link's inputs is [`LinkStore`].
#[derive(Debug, Clone, Default)]
pub struct LinkOptions {
    /// Keep debug info (line numbers, local names) in bytecode. Off = stripped, and
    /// stripping takes the traceback with it: every frame of a run-time failure reads `?`,
    /// with no line. On, a frame reads `depth:8` — the module the bundle knows, since a
    /// bundle holds modules rather than files.
    pub debug: bool,
    /// Store generated Lua source instead of bytecode (portable across Lua builds):
    /// larger and readable, bound to no Lua build, and named the same way as `debug`.
    pub source: bool,
    /// Modules to include even if no literal `require` reaches them.
    pub extra: Vec<String>,
    /// Modules the host provides at run time, besides those declared only by a `.d.tl`
    /// (which the model answers for — see the module doc).
    /// A name here is left out of the bundle and not walked, whether or not a file on
    /// the search path could answer it — a library that bundles its own module names it
    /// here in the binary's bundle so the two do not carry it twice.
    ///
    /// A caller with the project's model puts every name the model says the host provides
    /// here (`model::Project::provided`), as `htl build` and `include_bundle!` do; what
    /// else it lists is added to those. A file of the model under one of them is an error
    /// of the model's, not something this list can override (see the module doc).
    pub host: Vec<String>,
    /// The module name the entry is served under in the bundle. `None` derives it from
    /// the file alone: its stem, or its directory's name for an `init.tl`.
    ///
    /// A caller that has the project's model (`model::Project::locate`) passes the name it
    /// gives the file, which is the one a `require` elsewhere in the project writes —
    /// `src/app/main.tl` is `app.main` there, and a bundle serving it as `main` would
    /// answer a name nothing asks for. The file alone cannot say which directory the
    /// name starts from, so the default is right only for an entry at the top of the
    /// source root.
    pub entry_name: Option<String>,
}

/// The run cache as the linker uses it: the store, plus what the store needs to key and
/// to validate an entry and that only the caller knows.
///
/// A borrowed view rather than part of [`LinkOptions`], which is owned and `Default` and
/// crosses the proc-macro boundary as a value; the store lives for one command or one
/// macro expansion, the options may not.
#[derive(Clone, Copy)]
pub struct LinkStore<'a> {
    /// The store itself: where a typed module's generated Lua and its check are looked up
    /// before the checker is asked, and written back after.
    pub cache: &'a Cache,
    /// The lint selection in force, as [`cache::gen_key`] takes it: an entry generated
    /// under different lints reports different lints, and must not be reused.
    pub lint: Option<&'a str>,
    /// The project root, for the directories a `require` could resolve in.
    pub root: &'a Path,
    /// The project's `htl.toml` (its path) and what it says: its `[check] paths` are
    /// probed, and the file itself is an input of every entry, since its lint selection is.
    pub config: Option<(&'a Path, &'a crate::config::HtlConfig)>,
}

impl LinkStore<'_> {
    /// Files an entry depends on besides the module and what it required.
    fn extra_inputs(&self) -> Vec<PathBuf> {
        self.config
            .map(|(file, _)| vec![file.to_path_buf()])
            .unwrap_or_default()
    }

    /// Directories a `require` from `file` could resolve in, for the entry's probes when
    /// the store has no project model ([`cache::search_dirs`]).
    fn probe_dirs(&self, file: &Path) -> Vec<PathBuf> {
        cache::search_dirs(file)
    }
}

/// One linked module: where it came from and how it was stored.
#[derive(Debug, Clone)]
pub struct LinkedModule {
    /// The module name a `require` reaches it by, which is the name it takes in the
    /// bundle — not its path.
    pub name: String,
    /// The file it was read from, for a report that wants to name something openable.
    pub path: PathBuf,
    /// `true` for a `.tl` that was checked and generated, `false` for a `.lua` taken as
    /// it was. Only the typed ones have a store entry, so this is what
    /// [`Linked::cached`] counts against.
    pub typed: bool,
}

/// The result of a link: the bundle, and everything a reporter wants to say about how it
/// was arrived at.
///
/// The bundle is private because an incomplete one must not escape — see
/// [`errors`](Self::errors) and [`bundle`](Self::bundle). [`link`] itself returns `Ok`
/// with the errors inside, so a caller (a `build.rs`, the macros) can show the whole list
/// rather than the first one; the bundle is handed out only when the list is empty.
#[derive(Debug, Default)]
pub struct Linked {
    bundle: Bundle,
    /// Every module the walk took, in the order it took them.
    pub modules: Vec<LinkedModule>,
    /// Names the host is expected to provide at run time, so the bundle records them as
    /// its own requirements rather than carrying code for them.
    pub host_modules: Vec<String>,
    /// Type errors and unresolved requires. A module with a type error is *absent* from
    /// the bundle, so the bundle is only handed out ([`bundle`](Self::bundle)) when this
    /// is empty: a program missing a module dies at its first `require`, far from here.
    ///
    /// Each is a [`Diagnostic`](crate::Diagnostic)'s text with its path spelled as
    /// `htl check` spells it ([`spelled`](crate::Diagnostic::spelled)), so the error a
    /// failed `include_bundle!` or `into_bundle` shows names a file the way the report does.
    pub errors: Vec<String>,
    /// The part of [`errors`](Self::errors) the linker found itself — a `require` nothing
    /// answers, an `extra` module that is not there, a name two files claim — as values.
    /// The rest of `errors` is the modules' own checks, which [`checks`](Self::checks)
    /// carries. A reporter says these after the checks' diagnostics; it does not have to
    /// tell the two apart by their text.
    pub link_errors: Vec<crate::Diagnostic>,
    /// Lints from every module, which do not stop a bundle: whether they stop the *run*
    /// is the caller's, and the caller is what knows about `strict`.
    pub lints: Vec<String>,
    /// The full check of each module, for a reader that wants more than the flattened
    /// [`errors`](Self::errors) and [`lints`](Self::lints) — the requires, the
    /// dependencies, the per-file verdict.
    pub checks: Vec<(PathBuf, CheckInfo)>,
    /// How many typed modules came from the store rather than the checker. Zero without
    /// a store. The total to say it against is the typed count of [`modules`](Self::modules).
    pub cached: usize,
    /// The checks' errors a linker error says instead: a `require` the checker could not
    /// find is its `module not found`, and the linker's error at the same place says the
    /// same thing and where to declare the module. One `require` is said once, by the
    /// linker; [`errors`](Self::errors) carries that line in the check's place, and
    /// [`reported`](Self::reported) leaves the check's out.
    superseded: Vec<(PathBuf, usize, usize)>,
}

impl Linked {
    /// The check of `path` as a report says it: `check` without the errors a linker error
    /// at the same place says instead ([`link_errors`](Self::link_errors)). The checks in
    /// [`checks`](Self::checks) are left as the checker made them.
    pub fn reported(&self, path: &Path, check: &CheckInfo) -> CheckInfo {
        let taken = |d: &crate::Diagnostic| {
            self.superseded
                .iter()
                .any(|(p, l, c)| p == path && *l == d.line && *c == d.col)
        };
        let mut out = check.clone();
        if !check.error_items.iter().any(taken) {
            return out;
        }
        // `errors`, `error_fixes` and `error_items` are parallel: the i-th of each is one
        // error, so they are filtered together.
        out.errors.clear();
        out.error_fixes.clear();
        out.error_items.clear();
        for (i, d) in check.error_items.iter().enumerate() {
            if taken(d) {
                continue;
            }
            out.errors.push(check.errors[i].clone());
            out.error_fixes
                .push(check.error_fixes.get(i).cloned().flatten());
            out.error_items.push(d.clone());
        }
        out
    }

    /// `true` when every module linked cleanly (lints are not errors here).
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// The bundle, or every error that makes it incomplete.
    pub fn bundle(&self) -> Result<&Bundle> {
        if self.errors.is_empty() {
            Ok(&self.bundle)
        } else {
            Err(self.error())
        }
    }

    /// The bundle by value, on the same condition as [`bundle`](Self::bundle): for a
    /// caller that writes it out and is done with the report around it.
    pub fn into_bundle(self) -> Result<Bundle> {
        if self.errors.is_empty() {
            Ok(self.bundle)
        } else {
            Err(self.error())
        }
    }

    fn error(&self) -> anyhow::Error {
        anyhow::anyhow!(
            "link failed with {} error(s):\n  {}",
            self.errors.len(),
            self.errors.join("\n  ")
        )
    }

    /// Every file the bundle was built from (entry, modules, and what the checker read
    /// for them, e.g. `.d.tl`s): what a build script or macro should watch for changes,
    /// one `cargo:rerun-if-changed=<file>` each. Files, not the directory: cargo compares
    /// the mtime of the path it is given, and editing a file inside a directory does not
    /// change the directory's.
    pub fn inputs(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self.modules.iter().map(|m| m.path.clone()).collect();
        for (_, ci) in &self.checks {
            out.extend(ci.deps.iter().cloned());
        }
        out.sort();
        out.dedup();
        out
    }
}

/// Link `entry` (a `.tl` file) and everything it requires. The checker's search path
/// must already cover the project (`add_path` / `apply_project` / `apply_config`).
pub fn link(h: &Htl, entry: &Path, opts: &LinkOptions) -> Result<Linked> {
    link_with(h, entry, opts, None)
}

/// [`link`], replaying from the run cache what it can (see the module doc).
///
/// With `store` = `None` this is `link`. With a store, a typed module whose `gen` entry
/// still holds is taken from it — its generated Lua, and what checking it reported — and
/// counted in [`Linked::cached`]; every other typed module is generated and its entry
/// written. Nothing about the store can fail the link: an unreadable or stale entry is a
/// generate, an unwritable store is a generate next time too.
pub fn link_with(
    h: &Htl,
    entry: &Path,
    opts: &LinkOptions,
    store: Option<LinkStore<'_>>,
) -> Result<Linked> {
    let mut out = Linked::default();
    let entry_name = opts
        .entry_name
        .clone()
        .unwrap_or_else(|| entry_module_name(entry));
    let host_declared: HashSet<String> = opts.host.iter().cloned().collect();
    let mut host: BTreeSet<String> = BTreeSet::new();
    let mut queued: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, PathBuf)> = VecDeque::new();
    queue.push_back((entry_name.clone(), entry.to_path_buf()));
    queued.insert(entry_name.clone());
    for name in &opts.extra {
        match classify(h, name, None)? {
            Target::File(p) => {
                if queued.insert(name.clone()) {
                    queue.push_back((name.clone(), p));
                }
            }
            Target::Host => {
                host.insert(name.clone());
            }
            Target::Missing => out.link_error(positionless(format!(
                "extra module '{name}' not found on the search path"
            ))),
            Target::Ambiguous(why) | Target::Shadowed(why) => {
                out.link_error(positionless(format!("extra module {why}")))
            }
        }
    }

    while let Some((name, path)) = queue.pop_front() {
        let typed = path.extension().is_none_or(|e| e != "lua");
        let mut checked_errors: Option<(usize, Vec<(usize, usize)>)> = None;
        let (code, requires) = if typed {
            let Generated {
                code,
                check: ci,
                cached,
            } = generate(h, &path, store)?;
            if cached {
                out.cached += 1;
            }
            debug_assert_eq!(ci.error_items.len(), ci.errors.len());
            // Where this file's errors start in `errors`, and where each sits: a linker
            // error at one of those places says it instead (`superseded`).
            let first = out.errors.len();
            let at_positions: Vec<(usize, usize)> =
                ci.error_items.iter().map(|d| (d.line, d.col)).collect();
            checked_errors = Some((first, at_positions));
            out.errors.extend(
                ci.error_items
                    .iter()
                    .map(|d| d.clone().spelled().to_string()),
            );
            out.lints.extend(
                ci.lint_items
                    .iter()
                    .map(|d| d.clone().spelled().to_string()),
            );
            let reqs = ci.requires.clone();
            out.checks.push((path.clone(), ci));
            (code, reqs)
        } else {
            let src = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let reqs = h.lua_requires(&src, &path)?;
            (Some(src), reqs)
        };
        for r in &requires {
            if queued.contains(&r.module) {
                continue;
            }
            // A name the caller said the host provides is the host's before the search
            // path is asked: a file that could answer it is not bundled and not walked.
            // If a file of the model implements it all the same, the model refuses the
            // name; a checked file's `require` of it is already an error of the check, a
            // plain `.lua`'s is said here, where it would otherwise pass unseen — at every
            // such `require`, including after a checked file has filed the name under the
            // host's.
            if host_declared.contains(&r.module) || host.contains(&r.module) {
                if !typed && let Some(why) = h.host_shadowing(&r.module)? {
                    out.link_error(at(&path, r, &why));
                    continue;
                }
                host.insert(r.module.clone());
                continue;
            }
            match classify(h, &r.module, r.path.as_deref())? {
                Target::File(p) => {
                    queued.insert(r.module.clone());
                    queue.push_back((r.module.clone(), p));
                }
                Target::Host => {
                    host.insert(r.module.clone());
                }
                Target::Missing => {
                    let d = unresolved(&path, r);
                    // The checker's own error at this `require`, when it has one, is
                    // `module not found`: the same finding, said without where to declare
                    // the module. The linker's line takes its place.
                    let own = checked_errors.as_ref().and_then(|(first, at)| {
                        at.iter()
                            .position(|&(l, c)| l == r.line && c == r.col)
                            .map(|i| first + i)
                    });
                    match own {
                        Some(i) => {
                            let d = d.spelled();
                            out.errors[i] = d.to_string();
                            out.superseded.push((path.clone(), r.line, r.col));
                            out.link_errors.push(d);
                        }
                        None => out.link_error(d),
                    }
                }
                // A checked file's `require` of it is already an error of the check, at
                // the same place; a plain `.lua` is checked by nobody, so it is said here.
                Target::Ambiguous(why) | Target::Shadowed(why) if !typed => {
                    out.link_error(at(&path, r, &why))
                }
                Target::Ambiguous(_) => {}
                // The check said it; the name is still the host's, and nothing is bundled.
                Target::Shadowed(_) => {
                    host.insert(r.module.clone());
                }
            }
        }
        let Some(code) = code else { continue };
        let payload = if opts.source {
            Module {
                name: name.clone(),
                kind: Kind::Source,
                payload: code.into_bytes(),
            }
        } else {
            let bc = h.compile_with(&name, &code, !opts.debug)?;
            Module {
                name: name.clone(),
                kind: Kind::Bytecode,
                payload: bc,
            }
        };
        out.bundle.modules.push(payload);
        out.modules.push(LinkedModule { name, path, typed });
    }

    out.host_modules = host.iter().cloned().collect();
    out.bundle.entry = entry_name;
    out.bundle.htl_version = env!("CARGO_PKG_VERSION").to_string();
    out.bundle.host_modules = out.host_modules.clone();
    if !opts.source {
        out.bundle.fingerprint = h.fingerprint()?;
    }
    Ok(out)
}

/// One typed module, generated or replayed: see [`generate`].
#[derive(Debug)]
pub struct Generated {
    /// The Lua; `None` when checking produced errors (see [`CheckInfo`]).
    pub code: Option<String>,
    /// What checking said — carried whether or not there is code, and the half a replay
    /// needs as much as the Lua: the lints, the requires and the dependencies come out of
    /// here.
    pub check: CheckInfo,
    /// Whether it came from the store rather than the checker.
    pub cached: bool,
}

/// One typed module's generated Lua and what checking it said: from the store when its
/// `gen` entry still holds, else from the checker, and then into the store. What
/// [`link_with`] does per module, and what `include_tl!` does for its one file.
///
/// A hit needs both halves of the entry — the Lua, and the structured check it came with —
/// since the reader wants the lints, the requires and the dependencies out of the second.
/// `htl test` writes both; an entry missing either is a miss rather than a partial replay.
pub fn generate(h: &Htl, path: &Path, store: Option<LinkStore<'_>>) -> Result<Generated> {
    let key = store.map(|s| cache::module_gen_key(path, s.lint));
    if let (Some(s), Some(k)) = (store, &key)
        && let Some(m) = s.cache.lookup(k)
        && let (Some(code), Some(check)) = (&m.code, &m.check)
    {
        return Ok(Generated {
            code: Some(code.clone()),
            check: check.to_check(),
            cached: true,
        });
    }
    let (code, ci) = h.gen_lua(path)?;
    // Only when there is code: a module that failed to check has nothing to bundle, and
    // storing that would replay the failure as if it were a result.
    if let (Some(s), Some(k), Some(code)) = (store, &key, &code) {
        // `gen_lua` can come back without requires for a module the checker already holds
        // — it serves the generated code without walking the file again — and the requires
        // are what the next run's validation and the linker's own walk are built from. Ask
        // the checker separately, but only when the file could have any: a leaf that never
        // says `require` is most of a project, and a check per leaf is the wrong price.
        let stored = if ci.requires.is_empty() && mentions_require(path) {
            match h.check(path) {
                Ok(c) if !c.requires.is_empty() => CheckInfo {
                    requires: c.requires,
                    ..ci.clone()
                },
                _ => ci.clone(),
            }
        } else {
            ci.clone()
        };
        let m = cache::Module::generated(&stored, code.clone());
        s.cache
            .store_module(k, path, &s.extra_inputs(), &s.probe_dirs(path), &m);
    }
    Ok(Generated {
        code,
        check: ci,
        cached: false,
    })
}

/// Whether `path`'s source says `require` anywhere ([`cache::source_mentions_require`]);
/// an unreadable file is taken to, which costs a check rather than a wrong entry.
pub fn mentions_require(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|s| cache::source_mentions_require(&s))
        .unwrap_or(true)
}

fn is_decl(p: &Path) -> bool {
    p.to_string_lossy().ends_with(".d.tl")
}

fn unresolved(from: &Path, r: &RequireSite) -> crate::Diagnostic {
    at(
        from,
        r,
        &format!(
            "require(\"{}\") is not on the search path: nothing to bundle. If the host \
             provides it, declare it in a `{}.d.tl` or list it under `[build] host` in \
             htl.toml; if it is reached only through a dynamic require, list it under \
             `[build] extra`",
            r.module,
            r.module.replace('.', "/")
        ),
    )
}

enum Target {
    /// A file to bundle (`.tl` typed, or a plain `.lua`).
    File(PathBuf),
    /// Runs from no file of the project: the model says the host or the environment
    /// provides it — declared only, by a `.d.tl` with no `.lua` behind it, or named by
    /// one of the host's sources — or, with no model to ask, it resolves to a `.d.tl` and
    /// the search path has no `.lua` behind it.
    Host,
    Missing,
    /// More than one file implements the name: the model's message saying which. Nothing
    /// is bundled for it, since no order picks one.
    Ambiguous(String),
    /// The host provides the name and a file of the model implements it too: the model's
    /// message saying so. Nothing is bundled for it — the host's module is what runs — and
    /// it is an error wherever it is required, as `Ambiguous` is. Reached for a name the
    /// caller did not list in [`LinkOptions::host`] (a caller without the model's list);
    /// a listed one is checked before the walk would classify it.
    Shadowed(String),
}

/// A linker error at a `require`: `file:line:col: why`, the shape a check's error has.
fn at(file: &Path, r: &RequireSite, why: &str) -> crate::Diagnostic {
    crate::Diagnostic::new(
        crate::Severity::Error,
        file.display().to_string(),
        r.line,
        r.col,
        why,
        None,
    )
}

/// A linker error about no place in a file: an `extra` module named in the config.
fn positionless(message: String) -> crate::Diagnostic {
    crate::Diagnostic::new(crate::Severity::Error, "", 0, 0, message, None)
}

impl Linked {
    /// Record an error the linker found itself: as a value, and in the flattened
    /// [`errors`](Self::errors) as its text.
    fn link_error(&mut self, d: crate::Diagnostic) {
        let d = d.spelled();
        self.errors.push(d.to_string());
        self.link_errors.push(d);
    }
}

/// The module name an entry file answers to when the project model does not name it — an
/// entry outside every project: the naming rule ([`crate::naming`]) against the file's own
/// directory, so its stem, except that `<dir>/init.tl` is the module `<dir>` — the name a
/// `require` of it is written as, and so the name a bundle has to serve it under once a
/// host has installed the bundle and a program asks for it. An entry the model names is
/// served under that name ([`LinkOptions::entry_name`]).
fn entry_module_name(entry: &Path) -> String {
    // Named against its own directory — the one place a file outside every project
    // names by itself — which leaves `<dir>/init.tl` nothing to be named by but `<dir>`.
    let file = entry.file_name().map(PathBuf::from).unwrap_or_default();
    let rel = match (file.to_str(), entry.parent().and_then(|d| d.file_name())) {
        (Some("init.tl"), Some(dir)) => Path::new(dir).join(&file),
        _ => file,
    };
    crate::naming::name_of("", &rel).unwrap_or_else(|| "main".into())
}

/// What a `require(name)` points at for the linker. `found` is the checker's own
/// resolution when already known (a require site); otherwise it is looked up.
fn classify(h: &Htl, name: &str, found: Option<&Path>) -> Result<Target> {
    let (found, lua) = match found {
        Some(p) => (Some(p.to_path_buf()), None),
        None => h.resolve_module(name)?,
    };
    // A refused name resolves to its declaration or to nothing, never to its file; either
    // would read as the host's or as missing, so ask the model first.
    if found.as_deref().is_none_or(is_decl)
        && let Some(why) = h.host_shadowing(name)?
    {
        return Ok(Target::Shadowed(why));
    }
    let Some(p) = found else {
        return Ok(match h.ambiguity(name)? {
            Some(why) => Target::Ambiguous(why),
            None => Target::Missing,
        });
    };
    if !is_decl(&p) {
        return Ok(Target::File(p));
    }
    // A declaration. Whether anything of the project runs behind it is the model's to
    // say: a name it has only this declaration for is the environment's, one the host
    // provides is the host's, and neither is bundled.
    let model = h.model_provides(name)?;
    if let crate::Provision::Provided(_) = model {
        return Ok(Target::Host);
    }
    // Otherwise a `.lua` behind the declaration (a vendored dependency typed by a `.d.tl`)
    // is what runs, and what gets bundled: the model's own, when it has the name.
    let lua = match lua {
        Some(l) => Some(l),
        None => h.resolve_module(name)?.1,
    };
    Ok(match (lua, model) {
        (Some(l), _) => Target::File(l),
        // No model to ask, or a name it has nothing under (a `.d.tl` found on the search
        // path, outside every root of the project): a declaration with nothing behind it
        // on the path is taken to be the host's, as it was before the model answered.
        (None, crate::Provision::Unknown) => Target::Host,
        // The model has the name and says a file of the project runs for it, and the
        // lookup found none: the two disagree, which a declared name never makes them
        // do. Filed with the host's as before rather than failing a build over it.
        (None, _) => Target::Host,
    })
}
