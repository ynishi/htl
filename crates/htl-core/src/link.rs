//! Linking: the `require` closure of one entry file, as a [`Bundle`].
//!
//! Starting at the entry, every `require("<literal>")` is followed (only string
//! literals: a `require(expr)` cannot be resolved statically, list its targets under
//! `extra`). `.tl` modules are type-checked and generated; plain `.lua` modules (a
//! vendored dependency, say) are taken as they are. A name that resolves only to a
//! `.d.tl` declaration is recorded as host-provided, as is anything listed in `host`.
//! Any other unresolved `require` is an error: the point of a bundle is that "module
//! not found" happens here, not on the first `require` at the customer's machine.
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
    /// Keep debug info (line numbers, local names) in bytecode. Off = stripped.
    pub debug: bool,
    /// Store generated Lua source instead of bytecode (portable across Lua builds).
    pub source: bool,
    /// Modules to include even if no literal `require` reaches them.
    pub extra: Vec<String>,
    /// Modules the host provides at run time (besides those declared only by a `.d.tl`).
    pub host: Vec<String>,
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

    /// Directories a `require` from `file` could resolve in, for the entry's probes.
    fn probe_dirs(&self, file: &Path) -> Vec<PathBuf> {
        let cfg = self.config.map(|(file, c)| (crate::parent_dir(file), c));
        cache::search_dirs(
            file,
            self.root,
            cfg.as_ref().map(|(dir, c)| (dir.as_path(), *c)),
        )
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
/// [`errors`](Self::errors) and [`bundle`](Self::bundle).
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
    pub errors: Vec<String>,
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
}

impl Linked {
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
    /// for them, e.g. `.d.tl`s): what a build script or macro should watch for changes.
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
    let entry_name = entry
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| "main".into());
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
            Target::Missing => out.errors.push(format!(
                "extra module '{name}' not found on the search path"
            )),
        }
    }

    while let Some((name, path)) = queue.pop_front() {
        let typed = path.extension().is_none_or(|e| e != "lua");
        let (code, requires) = if typed {
            let Generated {
                code,
                check: ci,
                cached,
            } = generate(h, &path, store)?;
            if cached {
                out.cached += 1;
            }
            out.errors.extend(ci.errors.iter().cloned());
            out.lints.extend(ci.lints.iter().cloned());
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
            if queued.contains(&r.module) || host.contains(&r.module) {
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
                Target::Missing if host_declared.contains(&r.module) => {
                    host.insert(r.module.clone());
                }
                Target::Missing => out.errors.push(unresolved(&path, r)),
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

fn unresolved(from: &Path, r: &RequireSite) -> String {
    format!(
        "{}:{}:{}: require(\"{}\") is not on the search path: nothing to bundle. If the host \
         provides it, declare it in a `{}.d.tl` or list it under `[build] host` in htl.toml; \
         if it is reached only through a dynamic require, list it under `[build] extra`",
        from.display(),
        r.line,
        r.col,
        r.module,
        r.module.replace('.', "/")
    )
}

enum Target {
    /// A file to bundle (`.tl` typed, or a plain `.lua`).
    File(PathBuf),
    /// Declared only (`.d.tl` with no `.lua` behind it): the host provides it.
    Host,
    Missing,
}

/// What a `require(name)` points at for the linker. `found` is the checker's own
/// resolution when already known (a require site); otherwise it is looked up.
fn classify(h: &Htl, name: &str, found: Option<&Path>) -> Result<Target> {
    let (found, lua) = match found {
        Some(p) => (Some(p.to_path_buf()), None),
        None => h.resolve_module(name)?,
    };
    let Some(p) = found else {
        return Ok(Target::Missing);
    };
    if !is_decl(&p) {
        return Ok(Target::File(p));
    }
    // A declaration: is there a `.lua` implementation on the path behind it (a vendored
    // dependency typed by a `.d.tl`)? Then that is what gets bundled.
    let lua = match lua {
        Some(l) => Some(l),
        None => h.resolve_module(name)?.1,
    };
    Ok(match lua {
        Some(l) => Target::File(l),
        None => Target::Host,
    })
}
