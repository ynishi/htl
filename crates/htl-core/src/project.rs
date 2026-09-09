//! A check over a project: many files, a run cache, and the decisions that go with them.
//!
//! [`Htl::check`](crate::Htl::check) answers about one file. Everything above that — which
//! files to check, which of them can be replayed from the store instead, what a replayed
//! run says, what to keep for the next one — was private to `htl-cli`, so a library
//! consumer could reach the checker but not the layer that makes it usable on a project
//! (#107).
//!
//! It lives here now, and both readers of the run cache come through it: `htl check`, and
//! the `include_tl!` / `include_bundle!` expansions, which used to open the store
//! themselves and answer the same "may this build keep one?" question with their own code.
//! [`store`] is that answer, once.
//!
//! What the caller keeps is what is genuinely the caller's: how a diagnostic reads on a
//! terminal, what an exit code is, whether to print a summary at all. A [`Sink`] decides
//! *which* diagnostics a run says — a dependency's error is said once per run, and never
//! on behalf of a file the walk checks itself — and hands each one to an [`Output`], which
//! is the caller's.

use crate::cache::{self, DependencyJson, FixJson};
use crate::config::HtlConfig;
use crate::diagnostic::{Diagnostic, Severity};
use crate::{CheckInfo, Fix, Htl};
use anyhow::Result;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

// ------------------------------------------------------------------ output

/// Where a diagnostic goes once the run has decided to say it.
///
/// The text is as it should read: a dependency's path has already been rewritten against
/// the working directory, and the once-per-run rule has already dropped what should not
/// be said again. An implementation prints it, collects it, or forwards it; it does not
/// decide any of the above.
pub trait Output {
    fn diagnostic(
        &mut self,
        severity: Severity,
        text: &str,
        fix: Option<&Fix>,
        dependency: Option<&DependencyJson>,
    );
}

/// An [`Output`] that keeps every diagnostic as a value. What a caller with no terminal
/// to print to — a `build.rs`, a test, an editor — wants.
#[derive(Debug, Default)]
pub struct Collect {
    pub diagnostics: Vec<Diagnostic>,
}

impl Collect {
    pub fn take(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.diagnostics)
    }
}

impl Output for Collect {
    fn diagnostic(
        &mut self,
        severity: Severity,
        text: &str,
        fix: Option<&Fix>,
        dependency: Option<&DependencyJson>,
    ) {
        self.diagnostics
            .push(diagnostic_of(severity, text, fix, dependency));
    }
}

/// One diagnostic as a value, from the parts a run hands over.
///
/// The one place the three sources are put together: the text the checker wrote (which
/// [`Diagnostic::parse`] takes apart), the fix that travels beside it, and — for an error
/// in a required module — where that module was required from and what kind of place it
/// lives in.
pub fn diagnostic_of(
    severity: Severity,
    text: &str,
    fix: Option<&Fix>,
    dependency: Option<&DependencyJson>,
) -> Diagnostic {
    let mut d = Diagnostic::parse(severity, text);
    d.fix = fix.cloned();
    if let Some(dep) = dependency {
        d.required_by = Some(dep.required_by.clone());
        d.origin = dep.origin.clone();
    }
    d
}

// ------------------------------------------------------------------ sink

/// Which diagnostics a run says, and what it stores of them.
///
/// Everything handed over is recorded verbatim, so one run can be stored and a later one
/// replayed through this same code ([`replay`](Self::replay)). Replaying through the
/// deciding path, rather than through a reconstruction of what it decided, is what makes
/// "a cached run says what the original said" a property of the code instead of a promise
/// in a comment.
pub struct Sink<O: Output> {
    out: O,
    recorded: Vec<cache::Recorded>,
    /// The files this run checks in their own right, canonical. A dependency error in one
    /// of them is that file's to report, and is not said a second time on behalf of a file
    /// that required it.
    walked: HashSet<PathBuf>,
    /// Dependencies whose errors this run has already said, canonical. A module thirty
    /// files require is reported once, against the first of them.
    reported: HashSet<PathBuf>,
    /// How many dependency errors reached the output — what the totals and the exit code
    /// count, as opposed to how many were handed over (the entries store every one).
    dependency_errors: usize,
}

impl<O: Output> Sink<O> {
    pub fn new(out: O) -> Self {
        Self {
            out,
            recorded: Vec::new(),
            walked: Default::default(),
            reported: Default::default(),
            dependency_errors: 0,
        }
    }

    /// The output this sink writes to, for a caller that has to read back what it collected.
    pub fn out(&mut self) -> &mut O {
        &mut self.out
    }

    /// The files the run checks itself. Their errors are reported as their own, so a
    /// dependency error pointing at one of them is dropped here rather than said twice.
    pub fn walking(&mut self, files: &[PathBuf]) {
        self.walked = files.iter().map(|f| canonical(f)).collect();
    }

    pub fn diag(&mut self, severity: Severity, text: &str) {
        self.diag_with_fix(severity, text, None);
    }

    /// Same order as the text output has always used: warnings, lints, errors.
    pub fn checkinfo(&mut self, c: &CheckInfo) {
        for w in &c.warnings {
            self.diag(Severity::Warning, w);
        }
        for (i, l) in c.lints.iter().enumerate() {
            self.diag_with_fix(
                Severity::Lint,
                l,
                c.lint_fixes.get(i).and_then(|f| f.as_ref()),
            );
        }
        for (i, e) in c.errors.iter().enumerate() {
            self.diag_with_fix(
                Severity::Error,
                e,
                c.error_fixes.get(i).and_then(|f| f.as_ref()),
            );
        }
    }

    /// The errors in what a file required, as errors, each against the file that required
    /// it. `origin_of` says where the dependency lives (`dependency` / `external` / the
    /// project's own).
    ///
    /// Every one is recorded, so the file's cache entry carries them all and a later run
    /// that replays only this file still hears about its dependency. Which of them are
    /// said is decided at output time, once per run — see [`emit`](Self::emit).
    ///
    /// No fix rides along even when the checker found one: a fix under `.htl/` is
    /// overwritten at the next install, and one under a `[check] paths` directory is
    /// outside the project. `htl fix` never writes there, so the output does not say it can.
    pub fn dependency_errors(
        &mut self,
        c: &CheckInfo,
        origin_of: &dyn Fn(&Path) -> Option<&'static str>,
    ) {
        for e in &c.dependency_errors {
            let dep = DependencyJson {
                file: e.file.display().to_string(),
                required_by: e.required_by.display().to_string(),
                origin: origin_of(&e.file).map(str::to_string),
            };
            self.recorded.push(cache::Recorded {
                severity: Severity::Error.as_str().to_string(),
                text: e.text.clone(),
                fix: None,
                dependency: Some(dep.clone()),
            });
            self.emit(Severity::Error, &e.text, None, Some(&dep));
        }
    }

    /// Dependency errors this run reported (after the once-per-run rule), for the totals.
    pub fn dependency_error_count(&self) -> usize {
        self.dependency_errors
    }

    fn diag_with_fix(&mut self, severity: Severity, text: &str, fix: Option<&Fix>) {
        self.recorded.push(cache::Recorded {
            severity: severity.as_str().to_string(),
            text: text.to_string(),
            fix: fix.map(FixJson::from_fix),
            dependency: None,
        });
        self.emit(severity, text, fix, None);
    }

    /// The single place a diagnostic becomes output, whether it was just produced or
    /// recovered from the cache.
    ///
    /// A dependency's error is said once per run: not at all when the dependency is one
    /// of the files being checked (it reports its own), and not again after the first
    /// file that required it. Deciding here, rather than where the diagnostic was made,
    /// is what makes a replayed entry and a fresh check agree — both come through this.
    fn emit(
        &mut self,
        severity: Severity,
        text: &str,
        fix: Option<&Fix>,
        dependency: Option<&DependencyJson>,
    ) {
        if let Some(d) = dependency {
            let file = canonical(Path::new(&d.file));
            if self.walked.contains(&file) || !self.reported.insert(file) {
                return;
            }
            self.dependency_errors += 1;
        }
        // A dependency's path is the one that does not read like the rest of the report:
        // it came from the resolver rather than from the command line. The cache keeps what
        // the checker said and this writes it for the reader, so an entry replayed from
        // another directory still reads against that one.
        let text = match dependency {
            Some(_) => shown(text),
            None => Cow::Borrowed(text),
        };
        self.out
            .diagnostic(severity, text.as_ref(), fix, dependency);
    }

    /// Say a run recovered from the cache.
    ///
    /// An entry carrying a severity this build does not know is refused rather than
    /// guessed at; the caller then runs the check, which is the right answer for a store
    /// written by something else.
    pub fn replay(&mut self, recorded: &[cache::Recorded]) -> Result<()> {
        // Validate the whole entry before saying any of it. Text output goes out as it
        // is produced, so a replay that gave up halfway would leave those lines on the
        // terminal and the caller — which falls back to running the check — would print
        // them a second time.
        let severities = recorded
            .iter()
            .map(|r| match Severity::parse(&r.severity) {
                Some(s) => Ok(s),
                None => anyhow::bail!("cache entry has an unknown severity: {}", r.severity),
            })
            .collect::<Result<Vec<Severity>>>()?;
        for (severity, r) in severities.into_iter().zip(recorded) {
            let fix = r.fix.as_ref().map(FixJson::to_fix);
            self.emit(severity, &r.text, fix.as_ref(), r.dependency.as_ref());
        }
        Ok(())
    }

    /// What this run said, in the form the cache stores it.
    pub fn take_recorded(&mut self) -> Vec<cache::Recorded> {
        std::mem::take(&mut self.recorded)
    }
}

/// One spelling of a file for the once-per-run rule: the checker names a dependency by
/// the search-path template that found it, the walk names a file as it was given.
fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// A dependency's diagnostic with its file written against the directory the command ran
/// in, the way the rest of the report already reads. Everything else is left alone.
///
/// A dependency's path comes from the resolver, which searches in absolute paths, and a
/// `[check] paths` entry keeps the `..` it was joined through — so a report would otherwise
/// carry `src/area.tl` and `/home/me/proj/../ext/extmod.tl` side by side, and
/// `--format json` would give two `file` spellings for what may be one directory. The walk
/// and `required_by` carry what the command line said, which is theirs to keep.
fn shown(text: &str) -> Cow<'_, str> {
    // The same reading of the text [`Diagnostic::parse`] makes, and the same one place
    // making it: a text with no position keeps every character it has.
    let Some((file, _, _, _)) = crate::diagnostic::position(text) else {
        return Cow::Borrowed(text);
    };
    let shown = display_path(Path::new(file));
    if shown == file {
        return Cow::Borrowed(text);
    }
    Cow::Owned(format!("{shown}{}", &text[file.len()..]))
}

/// Relative to the directory the command ran in when it is under it, normalised absolute
/// when it is not — a dependency outside the project reads better that way than as a stack
/// of `..`.
fn display_path(p: &Path) -> String {
    let norm = lexical(p);
    match std::env::current_dir()
        .ok()
        .and_then(|cwd| norm.strip_prefix(cwd).ok().map(Path::to_path_buf))
    {
        Some(rel) => rel.display().to_string(),
        None => norm.display().to_string(),
    }
}

/// Fold `.` and `..` without touching the filesystem.
///
/// [`std::fs::canonicalize`] would resolve symlinks too, and `.htl/modules/vendored/<dep>`
/// is one: following it names mlua-pkg's cache directory rather than the dependency, which
/// is the opposite of what a report wants to say.
fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c.as_os_str()),
        }
    }
    out
}

// ------------------------------------------------------------------ the store

/// `htl.toml` as a caller loaded it: where the file is, its inputs, and what it says.
///
/// The tuple `htl-cli` has always passed around — `(root, config file, config)` — named
/// here so the signatures below read.
pub type Config = Option<(PathBuf, PathBuf, HtlConfig)>;

/// Why a run must not keep a store under `root`, when it must not.
///
/// A person standing in a project and asking for a check never trips this: the store goes
/// beside `htl.toml`, or in the working directory. A macro expands wherever cargo compiles
/// the crate, which is not always somewhere a store belongs — the crate may not have opted
/// into the layout at all (`htl init` / `htl new` write `htl.toml` and gitignore `.htl/`),
/// or it may be building from the copy `cargo publish` verifies under `target/package/`,
/// where a new file aborts the publish, or from a registry checkout, which nothing should
/// write to.
pub fn store_refusal(root: &Path, has_config: bool) -> Option<String> {
    if !has_config {
        return Some("no htl.toml".to_string());
    }
    cache::scratch_root(root).map(str::to_string)
}

/// The run cache for `root`, or `None` — with a word about why, when asked — when there
/// must not be one.
///
/// Every reader of the store that is not the store itself comes through here: `htl check`,
/// `htl test`, `htl build`, and each macro expansion. `tag` names the caller for the
/// explanation, which is the only thing that differs between them.
pub fn store(
    root: &Path,
    opts: cache::Options,
    refusal: Option<&str>,
    tag: &str,
) -> Option<cache::Cache> {
    if let Some(why) = refusal {
        if opts.explain {
            eprintln!("htl cache: not used by {tag} ({why}: {})", root.display());
        }
        return None;
    }
    cache::Cache::open(root, opts)
}

/// The store's settings for this run: the flag, then the config, then the environment.
///
/// **This is the only place any of them is read.** `cache.rs` takes what this decided, so a
/// run's behaviour is settled in one function rather than wherever each value happens to be
/// wanted — which is what makes it possible to see, from the code, what a given invocation
/// will do.
///
/// `HTL_CACHE_DEBUG` is the same switch as `--explain-cache`, for turning it on without
/// editing a command line. `HTL_CACHE_MAX_ENTRIES` has no flag: its only caller is the test
/// suite, which cannot reach a few hundred entries by honest means.
pub fn cache_options(
    enabled: bool,
    mode: Option<cache::Mode>,
    cfg: &Config,
    explain: bool,
) -> cache::Options {
    let mode = mode
        .or_else(|| {
            cfg.as_ref()
                .and_then(|(_, _, c)| c.cache.mode.as_deref())
                .map(|m| {
                    cache::Mode::parse(m).unwrap_or_else(|| {
                        eprintln!("htl: unknown [cache] mode {m:?}, using per-module");
                        cache::Mode::PerModule
                    })
                })
        })
        .unwrap_or_default();
    // The environment's switches are the store's own (`cache::Options::from_env`, shared
    // with the proc macros); the flag and the config decide over them.
    let env = cache::Options::from_env();
    cache::Options {
        enabled: enabled && env.enabled,
        mode,
        explain: explain || env.explain,
        max_entries: env.max_entries,
    }
}

/// Directories a `require` could resolve in, listed whether or not they exist yet.
///
/// The ones that do not exist matter most: a `types/` created after an entry was written
/// changes what a module name resolves to while every file the entry recorded still hashes
/// the same. Recording only the directories that happened to exist is the hole ccache
/// documents in its direct mode, and an empty directory hashes differently from one holding
/// a module, so listing it now is what closes it.
pub fn search_dirs(file: &Path, root: &Path, cfg: &Config) -> Vec<PathBuf> {
    cache::search_dirs(file, root, cfg.as_ref().map(|(r, _, c)| (r.as_path(), c)))
}

// ------------------------------------------------------------------ the checker

/// A checker set up the way a check of these paths needs it: the lint selection, the
/// package project, the config's search paths, and the test library.
///
/// Only built when something actually has to be checked — a run that replays every module
/// should not pay the ~13.5 ms this costs.
pub fn checker(cfg: &Config, paths: &[PathBuf], spec: &str) -> Result<Htl> {
    let h = Htl::new()?;
    if !spec.is_empty() {
        h.configure_lints(spec)?;
    }
    if let Some(first) = paths.first()
        && let Some(p) = crate::pkg::Project::find(first)
    {
        h.apply_project(&p)?;
    }
    if let Some((root, _, c)) = cfg {
        h.apply_config(root, c)?;
    }
    // `*_test.tl` under the checked tree require("htl.test"): make its types visible.
    h.install_test_lib()?;
    Ok(h)
}

/// Where a file a check pulled in lives, for the `origin` a dependency diagnostic carries.
///
/// Decided by the directory and reported, never enforced: every file with errors is
/// reported whatever this says. `dependency` is the installed-deps directory
/// (`.htl/modules`) and the vendored copies the manifest declares; `external` is a
/// `[check] paths` or contract directory, supplied from outside the project; anything else
/// is the project's own and carries no origin. A consumer that counts a dependency's
/// errors apart from the project's reads this field rather than parsing paths.
pub struct Origins {
    dependency: Vec<PathBuf>,
    external: Vec<PathBuf>,
}

impl Origins {
    pub fn new(
        start: &Path,
        root: &Path,
        cfg: &Config,
        contracts: &[crate::contract::Resolved],
    ) -> Self {
        let canon = |p: PathBuf| std::fs::canonicalize(&p).unwrap_or(p);
        let mut dependency = Vec::new();
        if let Some(p) = crate::pkg::Project::find(start) {
            dependency.push(canon(p.pkgs_dir.clone()));
            dependency.extend(p.target_dirs.iter().cloned().map(canon));
        }
        let mut external = Vec::new();
        if let Some((r, _, c)) = cfg {
            external.extend(
                c.check
                    .paths
                    .iter()
                    .map(|p| canon(crate::config::resolve_path(r, p))),
            );
            for c in contracts {
                external.extend(c.dirs(root).into_iter().map(canon));
            }
        }
        Self {
            dependency,
            external,
        }
    }

    pub fn of(&self, file: &Path) -> Option<&'static str> {
        let file = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
        if self.dependency.iter().any(|d| file.starts_with(d)) {
            Some("dependency")
        } else if self.external.iter().any(|d| file.starts_with(d)) {
            Some("external")
        } else {
            None
        }
    }
}

/// Check one file and collect everything it reported, its contract lints included, and
/// the errors of what it required after them.
pub fn check_one<O: Output>(
    h: &Htl,
    sink: &mut Sink<O>,
    f: &Path,
    cfg: &Config,
    contracts: &[crate::contract::Resolved],
    origins: &Origins,
) -> Result<cache::Module> {
    // Both `add_layout_paths` and the contract lints prepend to the search path, and
    // without putting it back the Nth file would be checked against the directories of the
    // first N-1 as well — so a `require` would resolve against whatever happened to be
    // walked earlier, and a file's diagnostics would depend on its position in the walk
    // (#21). `TestSession::run_file` does the same for `htl test`. An error below ends the
    // process, so the restore is not on that path.
    let saved = h.search_path()?;
    h.add_layout_paths(f)?;
    let c = h.check(f)?;
    sink.checkinfo(&c);
    let mut lints = c.lints.len();
    // Two declarations of one module on the path: one was read, the other silently was
    // not. Asked here, while the path this file was checked under is still in place.
    for l in crate::declaration_conflict_lints(h, f, &c)? {
        sink.diag(Severity::Lint, &l);
        lints += 1;
    }
    // `---@contract`: the type and required fields for files under each contract dir.
    if let Some((root, _, cfg)) = cfg
        && c.ok()
    {
        for l in crate::contract_lints(h, root, cfg, contracts, f)? {
            sink.diag(Severity::Lint, &l);
            lints += 1;
        }
    }
    // What this file required and found broken: `htl run` would refuse the module at its
    // first `require`, so the check says so first. Recorded into this file's entry like
    // its own diagnostics, so a replay carries them and an edit to the dependency — which
    // is among `deps` — invalidates the entry.
    sink.dependency_errors(&c, &|p| origins.of(p));
    h.set_search_path(&saved)?;
    Ok(cache::Module {
        // Everything this file put into the sink, and nothing from the files before it:
        // the previous iteration took its own.
        diagnostics: sink.take_recorded(),
        errors: c.errors.len(),
        warnings: c.warnings.len(),
        lints,
        deps: c.deps.iter().map(|p| cache::normal(p)).collect(),
        requires: cache::requires_json(&c),
        // `htl check` has no use for generated Lua, nor for reading a `CheckInfo` back —
        // it replays the diagnostics above straight into the sink. `htl test` fills both in.
        code: None,
        check: None,
    })
}

/// The `(name, file)` pairs an entry's requires resolved to.
pub fn resolved_requires(requires: &[cache::RequireJson]) -> Vec<(String, PathBuf)> {
    requires
        .iter()
        .filter_map(|r| {
            r.path
                .as_ref()
                .map(|p| (r.module.clone(), PathBuf::from(p)))
        })
        .collect()
}

/// What a harvest works with, gathered so the call site reads as one thing.
pub struct Harvest<'a> {
    pub store: &'a cache::Cache,
    pub session: &'a crate::testing::TestSession,
    pub cfg_inputs: &'a [PathBuf],
    pub root: &'a Path,
    pub cfg: &'a Config,
    pub lint: Option<&'a str>,
    pub opts: cache::Options,
    /// Modules already harvested by an earlier file in this run. Test files overlap heavily,
    /// and generating one twice writes the same entry twice.
    pub done: RefCell<HashSet<PathBuf>>,
}

/// Generate and store every module a checked test file reached, transitively.
///
/// Called after a miss, when the checker's store holds everything the check just walked, so
/// each `gen_lua` here generates rather than re-checks. The point is the next run: with these
/// stored, a replayed test file can preload what it requires instead of the searcher checking
/// and generating each module mid-execution.
///
/// Best-effort throughout. A module that fails to generate is one the next run will generate
/// itself, which is what happens today.
pub fn harvest_modules(h: &Harvest<'_>, check: &CheckInfo, test_file: &Path) {
    let Harvest {
        store,
        session,
        cfg_inputs,
        root,
        cfg,
        lint,
        opts,
        done,
    } = h;
    let (lint, opts) = (*lint, *opts);
    let done = &mut *done.borrow_mut();
    // The run put the search path back before returning, so `src/` is no longer on it and
    // every `require` would resolve to nothing — which is silent: the names come back with
    // no path, `resolved_requires` drops them, and the closure stops one level in. Put the
    // file's own layout back for the duration.
    let saved = session.checker().search_path().ok();
    let _ = session.checker().add_layout_paths(test_file);

    let mut queue = resolved_requires(&cache::requires_json(check));
    let (mut stored, mut skipped) = (0usize, 0usize);
    while let Some((_, path)) = queue.pop() {
        // `done` spans the whole run, not this file. Test files share their modules — on a
        // 27-file suite the closures overlapped enough to generate and store 171 times for
        // 55 distinct modules — and generating one twice writes the same entry twice.
        if !done.insert(path.clone()) {
            continue;
        }
        // Nor is there anything to do for one another run already stored and that still
        // holds. Checking that costs a few hashes against a generate.
        if let Some(m) = store.lookup(&cache::module_gen_key(&path, lint))
            && m.code.is_some()
        {
            queue.extend(resolved_requires(&m.requires));
            continue;
        }
        let Ok((Some(code), c)) = session.checker().gen_lua(&path) else {
            skipped += 1;
            continue;
        };
        // `gen_lua` comes back without requires for a module the checker already has in its
        // store — it serves the generated code and does not walk the AST again. The requires
        // are what the next run's closure is built from, so ask for them separately; the
        // check is served from the same store and costs almost nothing.
        // Only when the file could have any (`crate::link::mentions_require`): a check
        // per leaf module is the wrong price for an empty list that is right already.
        let c = if c.requires.is_empty() && crate::link::mentions_require(&path) {
            session.checker().check(&path).unwrap_or(c)
        } else {
            c
        };
        stored += 1;
        let m = cache::Module::generated(&c, code);
        queue.extend(resolved_requires(&m.requires));
        store.store_module(
            &cache::module_gen_key(&path, lint),
            &path,
            cfg_inputs,
            &search_dirs(&path, root, cfg),
            &m,
        );
    }
    if let Some(s) = saved {
        let _ = session.checker().set_search_path(&s);
    }
    if opts.explain {
        eprintln!("htl cache: harvested {stored} modules, {skipped} could not be generated");
    }
}

/// What a replayed test file should have in front of the searcher: every module it requires,
/// transitively, that the store still holds a valid entry for.
///
/// A module the store does not have is simply absent from the list and loads the usual way.
/// Falling back is always correct — it is what happens without any of this — so a partial
/// answer here costs time and never correctness.
pub fn preloads_for(
    store: &cache::Cache,
    entry: &cache::Module,
    lint: Option<&str>,
    opts: cache::Options,
) -> Vec<(String, String, PathBuf)> {
    let mut out = Vec::new();
    let mut queue = resolved_requires(&entry.requires);
    let mut seen: HashSet<PathBuf> = Default::default();
    let mut absent = 0usize;
    while let Some((name, path)) = queue.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        let Some(m) = store.lookup(&cache::module_gen_key(&path, lint)) else {
            absent += 1;
            continue;
        };
        let Some(code) = m.code.clone() else { continue };
        queue.extend(resolved_requires(&m.requires));
        out.push((name, code, path));
    }
    if opts.explain {
        let names: Vec<&str> = out.iter().map(|(n, _, _)| n.as_str()).collect();
        eprintln!(
            "htl cache: preloading {} [{}], {absent} not in the store",
            out.len(),
            names.join(" ")
        );
    }
    out
}

/// Say what the run did with the store, when asked. One line, at the end, from the store's
/// own counters.
pub fn explain_cache(store: Option<&cache::Cache>, opts: cache::Options) {
    if let Some(c) = store
        && opts.explain
        && let Some(line) = c.stats().summary(opts.mode)
    {
        eprintln!("{line}");
    }
}

// ------------------------------------------------------------------ the check

/// What a project check needs beyond the files themselves.
pub struct Options<'a> {
    /// The paths as the command line spelled them. The first names the project the walk
    /// belongs to: its mlua-pkg manifest, and the directory a dependency is judged against.
    pub paths: &'a [PathBuf],
    /// `htl.toml`, already loaded — the caller needs it for its own decisions (`strict`)
    /// and reading it twice would be reading it twice.
    pub config: &'a Config,
    /// A lint selection from the caller, merged after the file's own so that it wins.
    pub lint: Option<&'a str>,
    /// The run cache's switches ([`cache_options`]).
    pub cache: cache::Options,
}

/// What a project check found.
///
/// The counts are what an exit code and a summary are made of; the diagnostics themselves
/// went to the [`Output`] as they were decided.
#[derive(Debug, Clone)]
pub struct Report {
    /// The files the walk visited, in the order it visited them.
    pub files: Vec<PathBuf>,
    /// Errors, including those of required modules as the run said them (once each).
    pub errors: usize,
    pub warnings: usize,
    /// Lints, including the project-level ones: require cycles, contract problems, and a
    /// contract no host enforces.
    pub lints: usize,
    /// How many of `files` were replayed from the store rather than checked.
    pub replayed: usize,
}

impl Report {
    /// Whether the run counts as a failure: an error always, a warning or a lint under
    /// `strict`.
    pub fn failed(&self, strict: bool) -> bool {
        self.errors > 0 || (strict && (self.warnings > 0 || self.lints > 0))
    }

    /// Every module came from the store, so no checker was built.
    pub fn all_cached(&self) -> bool {
        self.replayed == self.files.len() && !self.files.is_empty()
    }
}

/// Check a project: replay what the store still holds, check the rest, and keep the answer.
///
/// The diagnostics go to `sink` as they are decided, in the order a walk produces them;
/// what comes back is what a summary and an exit code are made of. `htl check` is this
/// function plus its flags and its printing.
pub fn check<O: Output>(
    sink: &mut Sink<O>,
    files: &[PathBuf],
    opts: &Options<'_>,
) -> Result<Report> {
    let Options {
        paths,
        config: cfg,
        lint,
        cache: cache_opts,
    } = opts;
    let (cfg, cache_opts) = (*cfg, *cache_opts);
    let files = files.to_vec();

    // The `---@contract` markers, read once for the run rather than once per file: they
    // are a property of the project, and every file under a contract dir asks the same
    // question of them.
    let (contracts, contract_problems) = match cfg {
        Some((r, _, c)) => crate::contract::resolve(r, c),
        None => (Vec::new(), Vec::new()),
    };

    // The store lives at the project root, so invocations from different directories in
    // one project share it; what separates them is the key, which carries the working
    // directory and each path as written.
    let root = cfg
        .as_ref()
        .map(|(r, _, _)| r.clone())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let store = store(&root, cache_opts, None, "htl check");
    // The project the walk belongs to is the first path's. Nothing named at all is the
    // working directory, which is what a command line with no argument already means.
    let start = paths
        .first()
        .map(PathBuf::as_path)
        .unwrap_or(Path::new("."));
    let origins = Origins::new(start, &root, cfg, &contracts);
    // A dependency error is said once per run, and not on behalf of a file the walk
    // checks itself. The rule applies to replayed entries as much as to fresh checks.
    sink.walking(&files);

    // The lint selection is part of what a module reports, so it is part of every key.
    let file_spec = cfg
        .as_ref()
        .map(|(_, _, c)| c.lint_spec())
        .unwrap_or_default();
    let spec = crate::config::join_specs([file_spec.as_str(), lint.unwrap_or("")]);

    // Look every module up before checking any of them, so that a run where nothing moved
    // never builds a checker at all.
    let keys: Vec<cache::Key> = files
        .iter()
        .map(|f| cache::module_key(f, Some(&spec)))
        .collect();
    let run_key = cache::run_key(&files, Some(&spec));
    let hits: Vec<Option<cache::Module>> = match &store {
        Some(c) => c.lookup_all(&keys, &run_key, files.len()),
        None => vec![None; files.len()],
    };
    let to_check = hits.iter().filter(|h| h.is_none()).count();

    let h = if to_check > 0 {
        Some(checker(cfg, paths, &spec)?)
    } else {
        None
    };
    let cfg_inputs: Vec<PathBuf> = cfg.iter().map(|(_, p, _)| p.clone()).collect();

    let (mut n_err, mut n_warn, mut n_lint) = (0usize, 0usize, 0usize);
    let mut infos: Vec<(PathBuf, CheckInfo)> = Vec::with_capacity(files.len());
    let mut modules: Vec<cache::Module> = Vec::with_capacity(files.len());
    for ((f, key), hit) in files.iter().zip(&keys).zip(hits) {
        let m = match hit {
            Some(m) => {
                sink.replay(&m.diagnostics)?;
                m
            }
            None => {
                let h = h.as_ref().expect("a module missed, so a checker was built");
                let m = check_one(h, sink, f, cfg, &contracts, &origins)?;
                // Per-module entries are written as each one is checked; a whole-run entry
                // cannot be written until the walk is done, so it happens below.
                if let Some(c) = &store
                    && c.mode() == cache::Mode::PerModule
                {
                    c.store_module(key, f, &cfg_inputs, &search_dirs(f, &root, cfg), &m);
                }
                m
            }
        };
        n_err += m.errors;
        n_warn += m.warnings;
        n_lint += m.lints;
        infos.push((f.clone(), m.requires_only()));
        modules.push(m);
    }
    // One entry for the walk. Nothing to write when everything replayed: the entry that was
    // read is the entry that would be written.
    if let Some(c) = &store
        && c.mode() == cache::Mode::WholeRun
        && to_check > 0
    {
        let dirs: Vec<PathBuf> = files
            .iter()
            .flat_map(|f| search_dirs(f, &root, cfg))
            .collect();
        c.store_run(&run_key, &files, &cfg_inputs, &dirs, &modules);
    }
    // Project-level: cycles in the require graph of the files just checked.
    for cyc in crate::require_cycles(&infos) {
        sink.diag(Severity::Lint, &cyc);
        n_lint += 1;
    }
    // A marker that could not be turned into a contract, and a contract that could not be
    // published: reported once for the run, and before the enforcement question, which
    // cannot be asked about a contract there is no agreement on.
    let publish_problems = match cfg {
        Some((r, _, _)) => crate::contract::publish(r, &contracts).1,
        None => Vec::new(),
    };
    for p in contract_problems.iter().chain(&publish_problems) {
        sink.diag(Severity::Lint, p);
        n_lint += 1;
    }
    // A contract the host never enforces is documentation, not a guarantee.
    if let Some((_, cfg_path, _)) = cfg {
        let cargo_root = crate::dts::find_cargo_package_root(start);
        for l in crate::contract_enforcement_lints(cfg_path, &contracts, cargo_root.as_deref()) {
            sink.diag(Severity::Lint, &l);
            n_lint += 1;
        }
    }
    // Nothing else removes an entry, and this is the only moment the whole set is in hand.
    if let Some(c) = &store {
        let keep = match c.mode() {
            cache::Mode::PerModule => keys.clone(),
            cache::Mode::WholeRun => vec![run_key.clone()],
        };
        c.sweep(&keep, files.len());
    }
    explain_cache(store.as_ref(), cache_opts);

    // Errors in dependencies, counted as the sink said them: a module's own count says
    // nothing about them, and one required from thirty files was said once.
    n_err += sink.dependency_error_count();
    let replayed = files.len() - to_check;
    Ok(Report {
        files,
        errors: n_err,
        warnings: n_warn,
        lints: n_lint,
        replayed,
    })
}
