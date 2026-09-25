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
//!
//! A test run is the same arrangement over [`crate::testing`]: [`test()`] carries the
//! per-file isolation, the filter, fail-fast, the seed, the store and the coverage hooks,
//! and hands each [`FileReport`] to the caller as it lands. `htl test` prints them;
//! [`crate::testing::run_tests`] collects them, which is how a host runs a project's Teal
//! tests from `cargo test` rather than by shelling out to the binary.

use crate::cache::{self, DependencyJson, FixJson};
use crate::config::HtlConfig;
use crate::diagnostic::{Diagnostic, Severity};
use crate::testing::{FileReport, RunOptions, TestSession};
use crate::{CheckInfo, Htl};
use anyhow::Result;
use serde::Serialize;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

// ------------------------------------------------------------------ output

/// Where a diagnostic goes once the run has decided to say it.
///
/// It is as it should read: a dependency's path has already been rewritten against the
/// working directory, and the once-per-run rule has already dropped what should not be said
/// again. An implementation prints it (its `Display` is the text form), collects it, or
/// forwards it; it does not decide any of the above.
pub trait Output {
    /// Take one diagnostic the run has decided to say. Its `fix` is what `htl fix` would
    /// apply; its `required_by` and `origin` are set when the finding is in a module this
    /// project required rather than in the project, which is what lets a caller file it
    /// differently without reading the path.
    fn diagnostic(&mut self, d: &Diagnostic);
}

/// An [`Output`] that keeps every diagnostic as a value. What a caller with no terminal
/// to print to — a `build.rs`, a test, an editor — wants.
#[derive(Debug, Default)]
pub struct Collect {
    /// Everything said so far, in the order the run said it.
    pub diagnostics: Vec<Diagnostic>,
}

impl Collect {
    /// Take what has been collected, leaving the sink empty and reusable for the next run.
    pub fn take(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.diagnostics)
    }
}

impl Output for Collect {
    fn diagnostic(&mut self, d: &Diagnostic) {
        self.diagnostics.push(d.clone());
    }
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
    /// The levels this run judges by, when its caller judges at all
    /// ([`judge_by`](Self::judge_by)).
    levels: Option<crate::lint::Selection>,
    /// Findings that reached the output under a rule at `deny`.
    denied: usize,
}

impl<O: Output> Sink<O> {
    /// A sink over `out`, judging nothing and having said nothing yet. A run that wants
    /// `deny` to reach its exit code calls [`judge_by`](Self::judge_by) as well.
    pub fn new(out: O) -> Self {
        Self {
            out,
            recorded: Vec::new(),
            walked: Default::default(),
            reported: Default::default(),
            dependency_errors: 0,
            levels: None,
            denied: 0,
        }
    }

    /// The levels a run judges its findings by: what makes `deny` mean something at the
    /// exit code.
    ///
    /// Counted here because this is the one place a diagnostic becomes output, so a
    /// finding replayed from the store is judged by the same reading as a fresh one, and a
    /// dependency error said twice is counted once. A caller that does not judge — `htl
    /// test`, whose verdict is its tests — leaves this unset and gets a `denied` of zero.
    pub fn judge_by(&mut self, sel: &crate::lint::Selection) {
        self.levels = Some(sel.clone());
    }

    /// Findings this run said under a rule at `deny`.
    pub fn denied(&self) -> usize {
        self.denied
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

    /// Say a finding: one the checker handed over in its parts, or one this layer made.
    pub fn diagnostic(&mut self, d: &Diagnostic) {
        self.say(d.clone(), None);
    }

    /// Same order as the text output has always used: warnings, lints, errors.
    pub fn checkinfo(&mut self, c: &CheckInfo) {
        for d in c
            .warning_items
            .iter()
            .chain(&c.lint_items)
            .chain(&c.error_items)
        {
            self.diagnostic(d);
        }
    }

    /// The errors in what a file required, as errors, each against the file that required
    /// it. `origin_of` says where the dependency lives (`dependency` / `external` / the
    /// project's own).
    ///
    /// Every one is recorded, so the file's cache entry carries them all and a later run
    /// that replays only this file still hears about its dependency. Which of them are
    /// said is decided at output time, once per run — see this type's `emit`.
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
            self.say(e.diagnostic.clone(), Some(dep));
        }
    }

    /// Dependency errors this run reported (after the once-per-run rule), for the totals.
    pub fn dependency_error_count(&self) -> usize {
        self.dependency_errors
    }

    /// Record `d` for the store, then say it.
    fn say(&mut self, d: Diagnostic, dependency: Option<DependencyJson>) {
        self.recorded.push(cache::Recorded {
            severity: d.severity.as_str().to_string(),
            item: cache::ItemJson::from_diagnostic(&d),
            fix: d.fix.as_ref().map(FixJson::from_fix),
            dependency: dependency.clone(),
        });
        self.emit(d, dependency.as_ref());
    }

    /// The single place a diagnostic becomes output, whether it was just produced or
    /// recovered from the cache.
    ///
    /// A dependency's error is said once per run: not at all when the dependency is one
    /// of the files being checked (it reports its own), and not again after the first
    /// file that required it. Deciding here, rather than where the diagnostic was made,
    /// is what makes a replayed entry and a fresh check agree — both come through this.
    fn emit(&mut self, mut d: Diagnostic, dependency: Option<&DependencyJson>) {
        if let Some(dep) = dependency {
            let file = canonical(Path::new(&dep.file));
            if self.walked.contains(&file) || !self.reported.insert(file) {
                return;
            }
            self.dependency_errors += 1;
            // A dependency's path is the one that does not read like the rest of the
            // report: it came from the resolver rather than from the command line. The
            // cache keeps what the checker said and this writes it for the reader, so an
            // entry replayed from another directory still reads against that one.
            if !d.file.is_empty() {
                d.file = display_path(Path::new(&d.file));
            }
            d.required_by = Some(dep.required_by.clone());
            d.origin = dep.origin.clone();
        }
        // A type error fails the run whatever any level says — it is htl being unable to
        // stand behind the code, not an opinion about it. Everything else carries the name
        // of the rule that said it, and that name has a level.
        if let Some(levels) = &self.levels
            && crate::verdict::is_denied(&d, levels)
        {
            self.denied += 1;
        }
        self.out.diagnostic(&d);
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
            self.emit(r.item.to_diagnostic(severity, fix), r.dependency.as_ref());
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

/// Relative to the directory the command ran in when it is under it, normalised absolute
/// when it is not — a dependency outside the project reads better that way than as a stack
/// of `..`.
///
/// Public because it is how *a* path reads in this tool's output, not how a diagnostic's
/// does: [`crate::unused`] reports files the walk found rather than diagnostics, and the
/// two must spell one file the same way.
pub fn display_path(p: &Path) -> String {
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
/// [`std::fs::canonicalize`] would resolve symlinks too, and `.htl/modules/entries/<dep>`
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

/// The `htl.toml` of the project above `first`, in the shape everything below expects;
/// `None` when that project has none (an `mlua-pkg.toml` alone) or there is no project.
///
/// Found by [`model::Project::find_root`](crate::model::Project::find_root), the one walk
/// up for a manifest, so a config whose directory is not the project's root — an
/// `mlua-pkg.toml` above it, or beside some other `htl.toml` — is refused here as it is
/// wherever the model is built.
pub fn config_of(first: &Path) -> Result<Config> {
    Ok(crate::model::Project::find_root(first)?
        .and_then(|r| r.config_file.map(|f| (r.root, f, r.config))))
}

/// The [model](crate::model) of the project `config` was loaded for, or of the mlua-pkg
/// project above `first` when there is no `htl.toml`. `None` when there is neither.
///
/// Built from the config the caller already holds rather than found again, so a command
/// reads `htl.toml` once and the model and the config cannot describe two different
/// files. The mlua-pkg manifest is read at the config's root; one somewhere else is not
/// consulted, since a project has one root.
pub fn model_of(config: &Config, first: &Path) -> Result<Option<crate::model::Project>> {
    match config {
        Some((root, _, cfg)) => crate::model::Project::load(root, cfg.clone()).map(Some),
        None => crate::model::Project::discover(first),
    }
}

/// The directories a walk for `purpose` does not enter: the project model's answer
/// ([`crate::model::Project::not_walked`]). Outside a project there is no dependency to
/// leave out, and nothing is skipped beyond the walker's own rule
/// ([`crate::is_skipped_dir`]).
///
/// `paths` is unused: which directories are a dependency's is the model's to say, not
/// something a walk works out again from where it starts.
pub fn not_walked(
    model: Option<&crate::model::Project>,
    paths: &[PathBuf],
    purpose: crate::model::Purpose,
) -> Vec<PathBuf> {
    let _ = paths;
    model.map(|m| m.not_walked(purpose)).unwrap_or_default()
}

/// `files` split into those a module of the project holds and those none does, in walk
/// order. A module holds a file one of its roots names ([`Project::locate`]) and, for a
/// module other than the project's own, any file in its home. A project is its modules, and the project
/// root is not one unless it is the source root: a `.tl` directly under it, or in a
/// directory no module owns, cannot be required by any name the project gives it, so a
/// walk does not check it as the project's or count it as a module. The caller says which
/// were left out ([`outside_modules_note`]).
///
/// A file named in `paths` is kept whatever holds it: that question was asked outright.
/// Outside a project there are no modules, and every file is kept.
///
/// [`Project::locate`]: crate::model::Project::locate
pub fn held_by_modules(
    model: Option<&crate::model::Project>,
    paths: &[PathBuf],
    files: Vec<PathBuf>,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let Some(model) = model else {
        return (files, Vec::new());
    };
    let named: Vec<PathBuf> = paths
        .iter()
        .filter(|p| p.is_file())
        .map(|p| crate::model::canon(p))
        .collect();
    files.into_iter().partition(|f| {
        // Named by a module's root, or inside another module's home (a patched copy's
        // `tests/`, which no root of the copy names but the copy owns). What is left is
        // the project's own home outside every root it has: the root itself, and a
        // directory the layout does not name.
        model.locate(f).is_some()
            || model
                .home_of(f)
                .is_some_and(|m| m.owner != crate::model::Owner::Own)
            || named.contains(&crate::model::canon(f))
    })
}

/// The one line a walk says about the files [`held_by_modules`] left out, or `None` when it
/// left none: which, and where the project's modules are.
pub fn outside_modules_note(
    model: Option<&crate::model::Project>,
    outside: &[PathBuf],
    what: &str,
) -> Option<String> {
    if outside.is_empty() {
        return None;
    }
    let source = model.map_or("src", |m| m.config.layout.source.as_str());
    Some(format!(
        "{} file(s) belong to no module of the project and were not {what}: {}; move them \
         under {source}/",
        outside.len(),
        outside
            .iter()
            .map(|f| f.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// `store`, probing with `model`'s answers when there is a model
/// ([`Cache::with_answers`](cache::Cache::with_answers)): an entry then says what each name
/// its module required resolves to, which is the question its probes were for.
pub fn with_model(
    store: Option<cache::Cache>,
    model: Option<&crate::model::Project>,
) -> Option<cache::Cache> {
    let store = store?;
    let Some(model) = model else {
        return Some(store);
    };
    let resolver = std::sync::Arc::new(crate::model::Resolver::new(model));
    Some(
        store.with_answers(std::sync::Arc::new(move |requirer, name| {
            resolver.fingerprint(requirer, name)
        })),
    )
}

/// The directory whose `.htl/` holds the store: the project's root, wherever the command
/// ran from — where its installed dependencies live too (`.htl/modules`), so a project has
/// one `.htl/`. Outside any project, the working directory: a person asking for a check
/// of a loose file keeps a store where they stand.
pub fn store_root(model: Option<&crate::model::Project>) -> PathBuf {
    model
        .map(|m| m.root.clone())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// Why a run must not keep a store under `root`, when it must not.
///
/// A person standing in a project and asking for a check never trips this: the store goes
/// at the project root ([`store_root`]), or in the working directory. A macro expands
/// wherever cargo compiles the crate, which is not always somewhere a store belongs — the
/// crate may be in no project at all (`htl init` / `htl new` write `htl.toml` and gitignore
/// `.htl/`), or it may be building in build scratch, which is [`cache::scratch_root`]'s to
/// define and to explain: this is the store's half of a rule that covers the whole `.htl/`,
/// the entry links included.
pub fn store_refusal(root: &Path, in_project: bool) -> Option<String> {
    if !in_project {
        return Some("no project".to_string());
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

/// Directories a `require` from `file` could resolve in, for an entry's probes when the
/// store has no project model ([`cache::search_dirs`]).
pub fn search_dirs(file: &Path) -> Vec<PathBuf> {
    cache::search_dirs(file)
}

// ------------------------------------------------------------------ the checker

/// A checker set up the way a check of a project needs it: the lint selection, the
/// directories of the project's [model](crate::model) as its sources see them, and the
/// test library.
///
/// `model` is `None` outside a project, where nothing is put on the path and a file's
/// `require`s resolve beside it ([`check_one`]).
///
/// Only built when something actually has to be checked — a run that replays every module
/// should not pay the ~13.5 ms this costs.
pub fn checker(model: Option<&crate::model::Project>, sel: &crate::lint::Selection) -> Result<Htl> {
    let h = Htl::new()?;
    h.select_lints(sel)?;
    if let Some(m) = model {
        h.apply_model(m, crate::model::View::Source)?;
    }
    // `*_test.tl` under the checked tree require("htl.test"): make its types visible.
    h.install_test_lib()?;
    // And any file may require("std.json"): the same, for the modules the binary carries.
    #[cfg(feature = "std")]
    h.install_std()?;
    Ok(h)
}

/// Where a file a check pulled in lives, for the `origin` a dependency diagnostic carries.
///
/// Decided by the module of the project model whose home holds the file
/// ([`home_of`](crate::model::Project::home_of)), by its owner
/// ([`Owner::origin`](crate::model::Owner::origin)) — the same module `htl resolve` names
/// the file's origin from — and reported, never enforced: every file with errors is
/// reported whatever this says. A consumer that counts a dependency's errors apart from
/// the project's reads this field rather than parsing paths. Outside a project every file
/// is the caller's own.
pub struct Origins {
    model: Option<crate::model::Project>,
}

impl Origins {
    /// Origins as `model` has them.
    pub fn new(model: Option<&crate::model::Project>) -> Self {
        Self {
            model: model.cloned(),
        }
    }

    /// Which origin `file` has, or `None` for the project's own — the word a dependency
    /// diagnostic carries as its `origin`.
    pub fn of(&self, file: &Path) -> Option<&'static str> {
        self.model.as_ref()?.home_of(file)?.owner.origin()
    }
}

/// What a walk settled before it checked anything, and every file of it is checked
/// against: the config, the contracts resolved from it, where a file a check pulls in
/// lives, the module names the host registers, and the rules this run reports under.
///
/// One value rather than five parameters, because they are one decision — a walk is not
/// free to answer any of them differently from one file to the next.
pub struct Walk<'a> {
    /// `htl.toml` and where it was found, or `None` for a run outside a project.
    pub cfg: &'a Config,
    /// The project's [model](crate::model), or `None` for a run outside a project: which
    /// module each file belongs to, and so which roots its `require`s may read.
    pub model: Option<&'a crate::model::Project>,
    /// The `[[contract]]` directories, already resolved: what a module under one of them
    /// is held to.
    pub contracts: &'a [crate::contract::Resolved],
    /// Where a file the check pulls in lives, for the `origin` its diagnostics carry.
    pub origins: &'a Origins,
    /// Module names the Rust host registers, which resolve at run time and so must not be
    /// reported as missing.
    pub host_modules: &'a [String],
    /// The rules and levels this run reports under, resolved from the config and the
    /// command line before the first file.
    pub lints: &'a crate::lint::Lints,
}

/// What every file of a run is checked against, resolved once before the first: the lint
/// selection, the project's contracts, and the Rust crate around it with the module names
/// its host registers.
///
/// `htl check` and `htl fix` build one each, so a file is held to the same rules whichever
/// of the two looks at it ([`file_findings`], [`project_findings`]).
pub struct Scope {
    /// `htl.toml`'s lint spec joined with the caller's, the caller's last so that it wins.
    /// Part of every cache key: a module checked under one selection is not replayed under
    /// another.
    pub spec: String,
    /// That spec, resolved: which rules report and at what level. An unknown name is
    /// refused when the scope is built, before anything is checked.
    pub lints: crate::lint::Lints,
    /// The `---@contract` markers, read once for the run rather than once per file: they
    /// are a property of the project, and every file under a contract dir asks the same
    /// question of them.
    pub contracts: Vec<crate::contract::Resolved>,
    /// Markers that could not be turned into a contract.
    pub contract_problems: Vec<Diagnostic>,
    /// The Rust crate around the project: the model's own `host_crate` when there is a
    /// model, which found it when it was loaded; otherwise the one around the first path.
    /// It is where the host modules below come from, and where `contract-unenforced`
    /// looks for enforcement.
    pub cargo_root: Option<PathBuf>,
    /// The module names the host registers in `package.preload` with a `#[host_module]`:
    /// `host-module-shadowed` asks the same question of every require. A project's model
    /// read them from the crate around its root when it was loaded
    /// ([`Provider::HostModule`](crate::model::Provider::HostModule)), and its resolver
    /// already makes a file under one of them an error at the require
    /// ([`Resolution::HostShadowed`](crate::model::Resolution::HostShadowed)), so the lint
    /// finds nothing to add there; a file in no project has no model, and scans the crate
    /// around the first path. No crate means no host.
    pub host_modules: Vec<String>,
}

impl Scope {
    /// The scope of a run over `cfg` and `model`, started at `start` (the first path named,
    /// or the working directory), with the caller's lint selection `lint` on top of the
    /// config's.
    pub fn new(
        cfg: &Config,
        model: Option<&crate::model::Project>,
        start: &Path,
        lint: Option<&str>,
    ) -> Result<Self> {
        // The project's contracts and the markers it could not make one of: the model's,
        // read when it was loaded. A run with no `htl.toml` has no model and no contracts.
        let (contracts, contract_problems) = match model {
            Some(m) => (
                m.contracts.iter().map(|c| c.terms.clone()).collect(),
                m.problems.clone(),
            ),
            None => (Vec::new(), Vec::new()),
        };
        let file_spec = cfg
            .as_ref()
            .map(|(_, _, c)| c.lint_spec())
            .unwrap_or_default();
        let spec = crate::config::join_specs([file_spec.as_str(), lint.unwrap_or("")]);
        // One resolution of that spec for the run. The checker is configured from it, so
        // the rules `lint.lua` runs and the rules this layer asks are the same answer to
        // the same question.
        let lints = crate::lint::Lints::parse(&spec)?;
        let cargo_root = match model {
            Some(m) => m.host_crate.clone(),
            None => crate::dts::find_cargo_package_root(start),
        };
        let host_modules: Vec<String> = match model {
            Some(m) => m
                .provided()
                .filter(|(_, p)| *p == crate::model::Provider::HostModule)
                .map(|(n, _)| n.to_string())
                .collect(),
            None => cargo_root
                .as_deref()
                .map(crate::dts::host_module_names)
                .unwrap_or_default(),
        };
        Ok(Self {
            spec,
            lints,
            contracts,
            contract_problems,
            cargo_root,
            host_modules,
        })
    }

    /// What [`check_one`] and [`file_findings`] read, for a run over `cfg` and `model`
    /// whose dependencies' origins are `origins`.
    pub fn walk<'a>(
        &'a self,
        cfg: &'a Config,
        model: Option<&'a crate::model::Project>,
        origins: &'a Origins,
    ) -> Walk<'a> {
        Walk {
            cfg,
            model,
            contracts: &self.contracts,
            origins,
            host_modules: &self.host_modules,
            lints: &self.lints,
        }
    }

    /// What [`project_findings`] reads, for a run over `cfg` and `model`.
    pub fn whole<'a>(
        &'a self,
        cfg: &'a Config,
        model: Option<&'a crate::model::Project>,
    ) -> Whole<'a> {
        Whole {
            config: cfg,
            model,
            lints: &self.lints,
            contracts: &self.contracts,
            contract_problems: &self.contract_problems,
            cargo_root: self.cargo_root.as_deref(),
            publish: true,
        }
    }
}

/// Check one file and collect everything it reported, its contract lints included, and
/// the errors of what it required after them.
///
/// The checker was built with the walk's rule selection, so the file's own lints arrive
/// already filtered; what is decided here is the rules this layer asks itself — and the
/// `-- htl: allow(...)` comments those answer to.
pub fn check_one<O: Output>(
    h: &Htl,
    sink: &mut Sink<O>,
    f: &Path,
    w: &Walk<'_>,
) -> Result<cache::Module> {
    // What this file may read beyond the sources' view, and the contract lints, both
    // prepend to the search path, and without putting it back the Nth file would be
    // checked against the directories of the first N-1 as well — so a `require` would
    // resolve against whatever happened to be walked earlier, and a file's diagnostics
    // would depend on its position in the walk (#21). `TestSession::run_file` does the
    // same for `htl test`. An error below ends the process, so the restore is not on that
    // path.
    let saved = h.search_path()?;
    file_view(h, w.model, f)?;
    let c = h.check(f)?;
    let lints_said = file_findings(h, sink, f, &c, w)?;
    h.set_search_path(&saved)?;
    Ok(cache::Module {
        // Everything this file put into the sink, and nothing from the files before it:
        // the previous iteration took its own.
        diagnostics: sink.take_recorded(),
        errors: c.errors.len(),
        warnings: c.warnings.len(),
        lints: lints_said,
        deps: c.deps.iter().map(|p| cache::normal(p)).collect(),
        requires: cache::requires_json(&c),
        // `htl check` has no use for generated Lua, nor for reading a `CheckInfo` back —
        // it replays the diagnostics above straight into the sink. `htl test` fills both in.
        code: None,
        check: None,
    })
}

/// Say, to `sink`, everything a file reports once the checker has checked it as `c`: its
/// own diagnostics, the lints this layer asks of it (a declaration two files provide, a
/// file under a name the host registers, a contract it does not satisfy), and the errors
/// of what it required. Returns how many lints that was.
///
/// Asked while the search path `f` was checked under is still in place, since the
/// declaration lints resolve against it. [`check_one`] is a check plus this; `htl fix`
/// calls it on the check it ends with, so a file it leaves says what `htl check` would.
pub fn file_findings<O: Output>(
    h: &Htl,
    sink: &mut Sink<O>,
    f: &Path,
    c: &CheckInfo,
    w: &Walk<'_>,
) -> Result<usize> {
    let (cfg, contracts, origins, host_modules, lints) =
        (w.cfg, w.contracts, w.origins, w.host_modules, w.lints);
    sink.checkinfo(c);
    let mut lints_said = c.lints.len();
    // Two declarations of one module on the path: one was read, the other silently was
    // not. And a require of a name the host registers that landed on a file instead.
    // Asked here, while the path this file was checked under is still in place — and only
    // when the run reports at least one of the two, since one walk answers both.
    if lints.on("duplicate-declaration") || lints.on("host-module-shadowed") {
        for l in lints.keep(crate::declaration_conflict_lints(h, f, c, host_modules)?) {
            sink.diagnostic(&l);
            lints_said += 1;
        }
    }
    // `---@contract`: the type and required fields for files under each contract dir.
    if let Some((root, _, cfg)) = cfg
        && c.ok()
        && lints.on("contract")
    {
        for l in lints.keep(crate::contract_lints(h, root, cfg, contracts, f)?) {
            sink.diagnostic(&l);
            lints_said += 1;
        }
    }
    // What this file required and found broken: `htl run` would refuse the module at its
    // first `require`, so the check says so first. Recorded into this file's entry like
    // its own diagnostics, so a replay carries them and an edit to the dependency — which
    // is among `deps` — invalidates the entry.
    sink.dependency_errors(c, &|p| origins.of(p));
    Ok(lints_said)
}

/// Widen what a checker set up for the sources' view may read to what `f` itself may.
///
/// In a project nothing: the model's resolver decides from the requiring file, and a test
/// reads the test root because it is under it. Outside a project there is no model to
/// ask, and a file given on its own resolves its `require`s in its own directory — the one
/// place a single file names by itself — and not in the working directory.
pub fn file_view(h: &Htl, model: Option<&crate::model::Project>, f: &Path) -> Result<()> {
    match model {
        Some(_) => Ok(()),
        None => {
            // Beside the file, not beside wherever the command ran.
            h.drop_cwd_search_path()?;
            h.add_path(&crate::parent_dir(f))
        }
    }
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
    /// Where the generated modules are written.
    pub store: &'a cache::Cache,
    /// The state the test file was checked in, which is what knows the modules it reached
    /// and can generate each of them without resolving the graph a second time.
    pub session: &'a crate::testing::TestSession,
    /// `htl.toml`'s own path, recorded as an input of every entry: a config change invalidates
    /// what was generated under it.
    pub cfg_inputs: &'a [PathBuf],
    /// The project's [model](crate::model): the directories the test file was checked
    /// against, put back for the harvest.
    pub model: Option<&'a crate::model::Project>,
    /// The `--lint` spec the run was given, part of an entry's key: a module generated
    /// under one selection must not be replayed under another.
    pub lint: Option<&'a str>,
    /// The store's own switches — whether to read, whether to write, whether to explain.
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
        model,
        lint,
        opts,
        done,
    } = h;
    let (lint, opts) = (*lint, *opts);
    let done = &mut *done.borrow_mut();
    // The run put the search path back before returning, so the project's directories are
    // no longer on it and every `require` would resolve to nothing — which is silent: the
    // names come back with no path, `resolved_requires` drops them, and the closure stops
    // one level in. Put the test file's view back for the duration: the model's
    // directories as a test sees them, or, outside a project, its own directory.
    let saved = session.checker().search_path().ok();
    let _ = match model {
        Some(m) => session.checker().apply_model(m, crate::model::View::Test),
        None => file_view(session.checker(), None, test_file),
    };

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
            &search_dirs(&path),
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
    /// The project the files belong to ([`model_of`]), built by the caller from where the
    /// run started: which module each file is, what it may read, where the store lives.
    /// Handed in rather than found again from the files, so the run and its caller cannot
    /// be about two projects. `None` outside any project.
    pub model: Option<&'a crate::model::Project>,
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
    /// Warnings the run said — the Teal compiler's own kinds, under the rule names htl
    /// gives them.
    pub warnings: usize,
    /// Lints, including the project-level ones: require cycles, contract problems, and a
    /// contract no host enforces.
    pub lints: usize,
    /// How many of the warnings and lints above were said under a rule the project set to
    /// `deny`. A count of levels rather than of kinds, which is why it overlaps the two
    /// counts before it instead of adding to them: it is the part of what the run said
    /// that the run fails on.
    pub denied: usize,
    /// How many of `files` were replayed from the store rather than checked.
    pub replayed: usize,
    /// What each file required, as the checker resolved it: the require graph, in the
    /// order the walk produced it.
    ///
    /// It is built either way — the `require-cycle` lint below is the graph read for
    /// cycles — and handed back because the complementary question is asked of the same
    /// edges: what does no entry reach ([`crate::unused`])? A replayed file carries its
    /// requires in its entry, so the graph is whole whether the run checked or replayed.
    pub requires: Vec<(PathBuf, Vec<cache::RequireJson>)>,
}

impl Report {
    /// What the run said, counted: the input to [`crate::verdict::verdict`].
    pub fn findings(&self) -> crate::verdict::Findings {
        crate::verdict::Findings {
            errors: self.errors,
            warnings: self.warnings,
            lints: self.lints,
            denied: self.denied,
        }
    }

    /// Whether the run counts as a failure under `strict`: [`crate::verdict::verdict`] of
    /// its [`findings`](Self::findings), which says what that means.
    pub fn failed(&self, strict: bool) -> bool {
        crate::verdict::verdict(
            &self.findings(),
            &crate::verdict::Policy {
                strict,
                capped: false,
            },
        )
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
        model,
        lint,
        cache: cache_opts,
    } = opts;
    let (cfg, cache_opts, model) = (*cfg, *cache_opts, *model);
    let files = files.to_vec();

    // The Rust crate around the walk, for the host modules it registers: found from the
    // first path, and nothing named at all is the working directory.
    let start = paths
        .first()
        .map(PathBuf::as_path)
        .unwrap_or(Path::new("."));
    let origins = Origins::new(model);
    // The store lives at the project root, so invocations from different directories in
    // one project share it; what separates them is the key, which carries the working
    // directory and each path as written.
    let root = store_root(model);
    let store = with_model(store(&root, cache_opts, None, "htl check"), model);
    // A dependency error is said once per run, and not on behalf of a file the walk
    // checks itself. The rule applies to replayed entries as much as to fresh checks.
    sink.walking(&files);

    // What every file of the run is checked against, resolved once before the first.
    let scope = Scope::new(cfg, model, start, *lint)?;
    let (spec, lints, host_modules) = (&scope.spec, &scope.lints, &scope.host_modules);
    let walk = scope.walk(cfg, model, &origins);
    // The same resolution decides the verdict: a finding under a rule this project set to
    // `deny` fails the run, and the sink counts those as it says them.
    sink.judge_by(lints.selection());

    // Look every module up before checking any of them, so that a run where nothing moved
    // never builds a checker at all. The host module names go into the key for the same
    // reason the lint selection does: they are part of what a module reports, so adding a
    // `#[host_module]` beside a Teal file of that name has to make the entry a miss.
    let key_spec = if host_modules.is_empty() {
        spec.clone()
    } else {
        format!("{spec}\u{1}host={}", host_modules.join(","))
    };
    let keys: Vec<cache::Key> = files
        .iter()
        .map(|f| cache::module_key(f, Some(&key_spec)))
        .collect();
    let run_key = cache::run_key(&files, Some(&key_spec));
    let hits: Vec<Option<cache::Module>> = match &store {
        Some(c) => c.lookup_all(&keys, &run_key, files.len()),
        None => vec![None; files.len()],
    };
    let to_check = hits.iter().filter(|h| h.is_none()).count();

    let h = if to_check > 0 {
        Some(checker(model, lints.selection())?)
    } else {
        None
    };
    let cfg_inputs: Vec<PathBuf> = cfg.iter().map(|(_, p, _)| p.clone()).collect();

    let (mut n_err, mut n_warn, mut n_lint) = (0usize, 0usize, 0usize);
    let mut infos: Vec<(PathBuf, CheckInfo)> = Vec::with_capacity(files.len());
    let mut requires: Vec<(PathBuf, Vec<cache::RequireJson>)> = Vec::with_capacity(files.len());
    let mut modules: Vec<cache::Module> = Vec::with_capacity(files.len());
    for ((f, key), hit) in files.iter().zip(&keys).zip(hits) {
        let m = match hit {
            Some(m) => {
                sink.replay(&m.diagnostics)?;
                m
            }
            None => {
                let h = h.as_ref().expect("a module missed, so a checker was built");
                let m = check_one(h, sink, f, &walk)?;
                // Per-module entries are written as each one is checked; a whole-run entry
                // cannot be written until the walk is done, so it happens below.
                if let Some(c) = &store
                    && c.mode() == cache::Mode::PerModule
                {
                    c.store_module(key, f, &cfg_inputs, &search_dirs(f), &m);
                }
                m
            }
        };
        n_err += m.errors;
        n_warn += m.warnings;
        n_lint += m.lints;
        requires.push((f.clone(), m.requires.clone()));
        infos.push((f.clone(), m.requires_only()));
        modules.push(m);
    }
    // One entry for the walk. Nothing to write when everything replayed: the entry that was
    // read is the entry that would be written.
    if let Some(c) = &store
        && c.mode() == cache::Mode::WholeRun
        && to_check > 0
    {
        let dirs: Vec<PathBuf> = files.iter().flat_map(|f| search_dirs(f)).collect();
        c.store_run(&run_key, &files, &cfg_inputs, &dirs, &modules);
    }
    // What the project says about itself as a whole, once the files have been checked.
    let whole = project_findings(sink, &scope.whole(cfg, model), &infos);
    n_err += whole.errors;
    n_lint += whole.lints;
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
        denied: sink.denied(),
        replayed,
        requires,
    })
}

/// What a check reads to say what the project says about itself as a whole: the
/// findings no single file carries ([`project_findings`]).
pub struct Whole<'a> {
    /// `htl.toml`, as the run loaded it.
    pub config: &'a Config,
    /// The project, when there is one: its `[imports]` and which modules claim a name.
    pub model: Option<&'a crate::model::Project>,
    /// The run's lint selection: which of the project-level rules are on.
    pub lints: &'a crate::lint::Lints,
    /// The run's contracts ([`crate::contract::resolve`]) and the markers that could not
    /// be turned into one.
    pub contracts: &'a [crate::contract::Resolved],
    /// See `contracts`.
    pub contract_problems: &'a [Diagnostic],
    /// The Rust crate around the project, where `contract-unenforced` looks for the host's
    /// enforcement.
    pub cargo_root: Option<&'a Path>,
    /// Whether the contracts' types are written where they are published. A dry run
    /// works out the same problems and writes nothing ([`crate::contract::publish_to`]).
    pub publish: bool,
}

/// How many errors and lints [`project_findings`] said.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WholeCounts {
    /// A name with more than one owner, an `[imports]` entry naming no dependency.
    pub errors: usize,
    /// `require-cycle`, the `contract` problems of markers and publishing,
    /// `contract-unenforced`.
    pub lints: usize,
}

/// Say, to `sink`, what the project says about itself as a whole: cycles in the require
/// graph of `infos` (the files just checked), contract markers that could not become a
/// contract or be published, `[imports]` entries naming no dependency, names two modules
/// implement, and contracts no host enforces.
///
/// None of these belongs to one file, so no file's check carries them; a command that
/// judges the project has to ask for them as well as for its files'. Publishing the
/// contracts' types is part of it: it is where the publishing problems come from.
pub fn project_findings<O: Output>(
    sink: &mut Sink<O>,
    w: &Whole<'_>,
    infos: &[(PathBuf, CheckInfo)],
) -> WholeCounts {
    let mut out = WholeCounts::default();
    // Project-level: cycles in the require graph of the files just checked.
    if w.lints.on("require-cycle") {
        for cyc in w.lints.keep(crate::require_cycles(infos)) {
            sink.diagnostic(&cyc);
            out.lints += 1;
        }
    }
    // A marker that could not be turned into a contract, and a contract that could not be
    // published: reported once for the run, and before the enforcement question, which
    // cannot be asked about a contract there is no agreement on.
    let publish_problems = match w.config {
        Some((r, _, _)) => crate::contract::publish_to(r, w.contracts, w.publish).1,
        None => Vec::new(),
    };
    // Both report under `contract`, so both go through the selection. Publishing itself is
    // not gated on it: writing a contract's type where the config says to put it is work
    // the command was asked to do, and only what it has to say about it is a finding.
    let problems: Vec<Diagnostic> = w
        .contract_problems
        .iter()
        .chain(&publish_problems)
        .cloned()
        .collect();
    for p in w.lints.keep(problems) {
        sink.diagnostic(&p);
        out.lints += 1;
    }
    // A name two modules implement: the project's own `src/mathx.tl` and a dependency
    // `mathx`, say. Which one a `require` gets was decided by the order of the search path
    // and said nowhere, so it is an error, reported at each file that claims the name.
    if let Some(m) = w.model {
        // An `[imports]` entry pointing at a dependency the project does not have: said
        // at `htl.toml`, which is the line to fix.
        let at = w
            .config
            .as_ref()
            .map(|(_, p, _)| display_path(p))
            .unwrap_or_else(|| crate::config::CONFIG_NAME.to_string());
        for p in m.import_problems() {
            sink.diagnostic(&Diagnostic::new(Severity::Error, at.clone(), 1, 1, p, None));
            out.errors += 1;
        }
        for c in m.conflicts(crate::model::View::Source) {
            let owners: Vec<String> = c
                .claims
                .iter()
                .map(|cl| format!("{} ({})", cl.module.describe(), display_path(&cl.file)))
                .collect();
            for cl in &c.claims {
                sink.diagnostic(&Diagnostic::new(
                    Severity::Error,
                    display_path(&cl.file),
                    1,
                    1,
                    format!(
                        "module name '{}' has more than one owner: {}. A name belongs to one \
                         module; rename one of them",
                        c.name,
                        owners.join(", ")
                    ),
                    None,
                ));
                out.errors += 1;
            }
        }
    }
    // A contract the host never enforces is documentation, not a guarantee. The scan reads
    // every Rust source of the crate, so a run with the rule off does not start it.
    if let Some((_, cfg_path, _)) = w.config
        && w.lints.on("contract-unenforced")
    {
        for l in w.lints.keep(crate::contract_enforcement_lints(
            cfg_path,
            w.contracts,
            w.cargo_root,
        )) {
            sink.diagnostic(&l);
            out.lints += 1;
        }
    }
    out
}

// ------------------------------------------------------------------ coverage

/// A function of a module no statement of which ran.
#[derive(Serialize, Debug, Clone)]
pub struct NeverRan {
    /// As the source writes it: `f`, `M.f`, `M:f`.
    pub name: String,
    /// Where its body starts, so the report points at the function rather than at the
    /// module.
    pub line: usize,
}

/// What a run covered of one module.
#[derive(Serialize, Debug, Clone)]
pub struct CoverageModule {
    /// As the report prints it — relative to the project root when there is one.
    pub path: String,
    /// Statements at least one test ran.
    pub executed: usize,
    /// Statements the module has. `executed` over this is the percentage.
    pub total: usize,
    /// Unexecuted statements as `[first_line, last_line]` ranges.
    pub unexecuted: Vec<(usize, usize)>,
    /// Functions nothing in the run entered. A percentage says how much of a module
    /// was missed; this says what was missed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub never_ran: Vec<NeverRan>,
    /// The module's file, absolute. Not part of the JSON (`path` is the reported
    /// spelling); the lcov writer resolves it against the project root instead.
    #[serde(skip)]
    pub source: PathBuf,
    /// Every statement as `(first line, ran)`, in source order. What `executed` /
    /// `total` count, kept for the lcov `DA` records.
    #[serde(skip)]
    pub statements: Vec<(usize, bool)>,
    /// Every function with a body as `(name, line, entered)`, in source order;
    /// `never_ran` is the `entered == false` subset.
    #[serde(skip)]
    pub functions: Vec<(String, usize, bool)>,
}

/// What a whole run covered: the modules it entered, and the totals over them.
#[derive(Serialize, Debug, Clone, Default)]
pub struct CoverageReport {
    /// One per module the run loaded. A module nothing required is not here at all — it
    /// has no coverage to report, which is [`crate::unused`]'s question rather than this
    /// one.
    pub modules: Vec<CoverageModule>,
    /// Statements run, summed over the modules.
    pub executed: usize,
    /// Statements those modules have, summed. The pair is the percentage a summary line
    /// prints.
    pub total: usize,
}

impl CoverageReport {
    /// The run as an lcov tracefile, one record per module in the report's order.
    ///
    /// `DA` is one entry per line a statement starts on, with a count of `1` or `0`:
    /// the hook records whether a line ran, not how often, and a number it does not
    /// have is not invented. Two statements starting on one line share the entry, so
    /// `LF` / `LH` equal the table's `total` / `executed` except on such lines. `FN` /
    /// `FNDA` are the classic two-field forms every consumer reads; there is no branch
    /// data, so no `BRDA`. `SF` is relative to `root` (the project root, so the file
    /// resolves against the repository wherever CI ran the command), absolute when the
    /// module is outside it.
    pub fn lcov(&self, root: &Path) -> String {
        use std::collections::BTreeMap;
        use std::fmt::Write as _;
        let mut out = String::new();
        for m in &self.modules {
            let sf = m
                .source
                .strip_prefix(root)
                .unwrap_or(&m.source)
                .to_string_lossy();
            out.push_str("TN:\n");
            let _ = writeln!(out, "SF:{sf}");
            for (name, line, _) in &m.functions {
                let _ = writeln!(out, "FN:{line},{name}");
            }
            for (name, _, entered) in &m.functions {
                let _ = writeln!(out, "FNDA:{},{name}", u8::from(*entered));
            }
            let _ = writeln!(out, "FNF:{}", m.functions.len());
            let _ = writeln!(
                out,
                "FNH:{}",
                m.functions.iter().filter(|(_, _, e)| *e).count()
            );
            let mut lines: BTreeMap<usize, bool> = BTreeMap::new();
            for &(line, ran) in &m.statements {
                *lines.entry(line).or_default() |= ran;
            }
            for (line, ran) in &lines {
                let _ = writeln!(out, "DA:{line},{}", u8::from(*ran));
            }
            let _ = writeln!(out, "LF:{}", lines.len());
            let _ = writeln!(out, "LH:{}", lines.values().filter(|r| **r).count());
            out.push_str("end_of_record\n");
        }
        out
    }
}

/// Coverage over the run: every `.tl` the test files' checks depended on (so a module
/// no test reached shows 0%), with the executed statements from the line hooks.
///
/// With the project's [model](crate::model), only the project's own sources are
/// reported: what coverage measures is how much of the code the project is answerable
/// for its tests exercise. A dependency's modules, a patched one's included, are reached
/// and not owned — the same line `htl fmt` and `htl unused` draw
/// ([`Purpose::Own`](crate::model::Purpose::Own)) — and a helper under the test root is
/// test code. Without a model every `.tl` reached is reported, test files and htl's
/// library aside.
pub fn coverage_report(
    checker: &Htl,
    test_files: &[PathBuf],
    hits: &HashMap<PathBuf, BTreeSet<usize>>,
    deps: &BTreeSet<PathBuf>,
    model: Option<&crate::model::Project>,
) -> Result<CoverageReport> {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let tests: HashSet<PathBuf> = test_files.iter().map(|p| canon(p)).collect();
    let mut sources: BTreeSet<PathBuf> = deps.iter().map(|p| canon(p)).collect();
    sources.extend(hits.keys().cloned());
    let cwd = std::env::current_dir().unwrap_or_default();
    let (mut tot_exec, mut tot_all) = (0usize, 0usize);
    let mut rows: Vec<CoverageModule> = Vec::new();
    for src in &sources {
        let name = src.to_string_lossy();
        if !name.ends_with(".tl")
            || name.ends_with(".d.tl")
            || tests.contains(src)
            || name.contains("/htl-lib-")
        {
            continue;
        }
        let owns = |m: &crate::model::Project| {
            m.locate(src).is_some_and(|p| {
                p.module.owner == crate::model::Owner::Own && p.role == crate::model::Role::Source
            })
        };
        if model.is_some_and(|m| !owns(m)) {
            continue;
        }
        let (ranges, funcs) = checker.coverage_spans(src)?;
        if ranges.is_empty() {
            continue;
        }
        let empty = BTreeSet::new();
        let ran = hits.get(src).unwrap_or(&empty);
        let mut missed = Vec::new();
        let mut statements = Vec::with_capacity(ranges.len());
        let mut executed = 0usize;
        for &(a, b) in &ranges {
            let hit = ran.range(a..=b).next().is_some();
            statements.push((a, hit));
            if hit {
                executed += 1;
            } else {
                missed.push((a, b));
            }
        }
        // The body only. Defining a function runs its `function` line and its `end`
        // line, so both are silent about whether anything ever entered it.
        let functions: Vec<(String, usize, bool)> = funcs
            .into_iter()
            .map(|f| {
                let entered = ran.range(f.line + 1..=f.last - 1).next().is_some();
                (f.name, f.line, entered)
            })
            .collect();
        let never_ran = functions
            .iter()
            .filter(|(_, _, entered)| !entered)
            .map(|(name, line, _)| NeverRan {
                name: name.clone(),
                line: *line,
            })
            .collect();
        tot_exec += executed;
        tot_all += ranges.len();
        let shown = src
            .strip_prefix(&cwd)
            .unwrap_or(src)
            .to_string_lossy()
            .into_owned();
        rows.push(CoverageModule {
            path: shown,
            executed,
            total: ranges.len(),
            unexecuted: missed,
            never_ran,
            source: src.clone(),
            statements,
            functions,
        });
    }
    Ok(CoverageReport {
        modules: rows,
        executed: tot_exec,
        total: tot_all,
    })
}

// ------------------------------------------------------------------ the test run

/// What a test run needs beyond the files themselves.
pub struct TestOptions<'a> {
    /// `htl.toml`, already loaded ([`config_of`]) — the caller reads it for its own
    /// decisions, and reading it twice would be reading it twice.
    pub config: &'a Config,
    /// The project the run belongs to ([`model_of`]), built by the caller: what each file
    /// may read, and the root the store lives at. `None` outside any project.
    pub model: Option<&'a crate::model::Project>,
    /// A lint selection from the caller, merged after the file's own so that it wins.
    pub lint: Option<&'a str>,
    /// Module name of the assertion library to ask for the verdict
    /// ([`crate::testing::DEFAULT_LIB`] unless the caller says otherwise).
    pub lib: &'a str,
    /// Run only the tests whose name contains this.
    pub filter: Option<&'a str>,
    /// What each file's run is given: fail-fast, snapshot updating, coverage, the seed.
    ///
    /// `fail_fast` is read twice over — the library stops at the first failing test in a
    /// file, and the run stops at the first failing file. `seed: None` means *draw one for
    /// this run*, which is not what it means to [`crate::testing::run_test_file`] (there it
    /// leaves the state's own seeding alone): a run has a seed, and [`TestReport::seed`]
    /// says which, so that `--seed` repeats it.
    pub run: RunOptions,
    /// The run cache's switches ([`cache_options`]). A test run's entries are always
    /// per-module: a whole-run entry over test files would mean one edit anywhere
    /// re-checks every suite, which is the trade a check offers because a check is one
    /// answer. A test run is many.
    pub cache: cache::Options,
}

/// What a test run found.
///
/// The counts are what an exit code and a summary are made of; each file's own report
/// went to the caller as it finished.
#[derive(Debug)]
pub struct TestReport {
    /// The files the run was given, in the order it ran them.
    pub files: Vec<PathBuf>,
    /// How many of them ran. Fewer than `files` when `fail_fast` stopped the run.
    pub ran: usize,
    /// Tests that passed, summed over the files — tests, not files.
    pub passed: usize,
    /// Tests that failed, summed the same way. A file can contribute to both.
    pub failed: usize,
    /// Files that failed to check, raised, or had a failing test.
    pub files_with_errors: usize,
    /// Files whose check and codegen came from the store. They still ran: only the work
    /// before the run is reusable.
    pub replayed: usize,
    /// The seed every file's stream was derived from, given or drawn.
    pub seed: u64,
    /// Wall time for the whole run, including the checks that were not replayed.
    pub duration_ms: f64,
    /// With `run.coverage`: what the line hooks saw, over the modules the checks reached.
    pub coverage: Option<CoverageReport>,
}

impl TestReport {
    /// Whether the run counts as a success: every file that ran was ok.
    pub fn ok(&self) -> bool {
        self.files_with_errors == 0
    }

    /// Files never run because `fail_fast` stopped the run.
    pub fn skipped(&self) -> usize {
        self.files.len() - self.ran
    }
}

/// Run a project's test files: one fresh program state each, replaying the check and the
/// codegen from the store where it still holds, and keeping what this run generated.
///
/// `each` is handed every file's report as it finishes, with the sink the file's
/// diagnostics have just gone to — that is where a caller prints a line, collects a
/// document, or counts what it likes. `htl test` is this function plus its flags and its
/// printing.
pub fn test<O: Output>(
    sink: &mut Sink<O>,
    files: &[PathBuf],
    opts: &TestOptions<'_>,
    each: &mut dyn FnMut(&FileReport, &mut Sink<O>),
) -> Result<TestReport> {
    let TestOptions {
        config: cfg,
        model,
        lint,
        lib,
        filter,
        run,
        cache: cache_opts,
    } = opts;
    let (cfg, cache_opts, model) = (*cfg, *cache_opts, *model);
    let files = files.to_vec();

    // Given, or drawn once for the whole run and reported. Drawn from the clock rather
    // than from a generator this process also hands to the tests: the seed has to differ
    // between runs, and nothing else about it matters.
    let seed = run.seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    });
    let run = RunOptions {
        seed: Some(seed),
        ..run.clone()
    };
    let coverage_wanted = run.coverage;
    let fail_fast = run.fail_fast;

    // The lint selection is part of what a module reports, so it is part of every key.
    let file_spec = cfg
        .as_ref()
        .map(|(_, _, c)| c.lint_spec())
        .unwrap_or_default();
    let spec = crate::config::join_specs([file_spec.as_str(), lint.unwrap_or("")]);
    let lint = if spec.is_empty() {
        None
    } else {
        Some(spec.as_str())
    };

    let mut cov_hits: HashMap<PathBuf, BTreeSet<usize>> = Default::default();
    let mut cov_deps: BTreeSet<PathBuf> = Default::default();
    let (mut passed, mut failed, mut bad_files, mut ran_files) = (0usize, 0usize, 0usize, 0usize);
    let started = std::time::Instant::now();
    // One checker for the run; each file still gets a fresh program state. The project's
    // model says what every file may read.
    let mut session = TestSession::new(lint, lib, *filter, run)?;
    if let Some(m) = &model {
        session = session.for_project((*m).clone());
    }

    // Checking a test file and generating its Lua is most of what a run costs — the tests
    // themselves are a few percent of it — and none of that work depends on the outcome, so
    // it is reusable in exactly the way a check's is. Running is not: a test has to run to
    // say whether it passes, every time.
    let root = store_root(model);
    let store = with_model(store(&root, cache_opts, None, "htl test"), model);
    let keys: Vec<cache::Key> = files.iter().map(|f| cache::gen_key(f, lint)).collect();
    let cfg_inputs: Vec<PathBuf> = cfg.iter().map(|(_, p, _)| p.clone()).collect();

    let mut replayed = 0usize;
    let harvest = store.as_ref().map(|c| Harvest {
        store: c,
        session: &session,
        cfg_inputs: &cfg_inputs,
        model,
        lint,
        opts: cache_opts,
        done: RefCell::new(Default::default()),
    });
    for (f, key) in files.iter().zip(&keys) {
        // A hit needs both halves: the Lua to run, and what checking it said. An entry
        // missing either is no use, so it is a miss rather than a partial replay.
        let hit = store
            .as_ref()
            .and_then(|c| c.lookup(key))
            .filter(|m| m.code.is_some() && m.check.is_some());
        let rep = match &hit {
            Some(m) => {
                replayed += 1;
                let check = m.check.as_ref().expect("filtered above").to_check();
                let code = m.code.as_deref().expect("filtered above");
                // Without these, every module this file requires is checked and generated
                // while it runs — the work skipping `gen_lua` was supposed to avoid.
                let pre = store
                    .as_ref()
                    .map(|c| preloads_for(c, m, lint, cache_opts))
                    .unwrap_or_default();
                session.run_file_with(f, Some((code, &check)), &pre)?.0
            }
            None => {
                let (rep, code) = session.run_file_with(f, None, &[])?;
                // Only when there is code: a file that failed to check has nothing to run,
                // and storing that would replay an empty run as if it were a result.
                if let (Some(c), Some(code)) = (&store, code) {
                    let m = cache::Module::generated(&rep.check, code);
                    c.store_module(key, f, &cfg_inputs, &search_dirs(f), &m);
                    // And the modules it reached, so the next run can preload them. The
                    // checker's store is warm here, so this generates rather than re-checks.
                    if let Some(h) = &harvest {
                        harvest_modules(h, &rep.check, f);
                    }
                }
                rep
            }
        };
        if coverage_wanted {
            for (source, lines) in &rep.coverage {
                // Lua names a file chunk "@<path>"; a label ("=name") — a bundle entry, the
                // test library — names no file and has no source to attribute lines to.
                let Some(path) = source.strip_prefix('@') else {
                    continue;
                };
                let key = std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
                cov_hits
                    .entry(key)
                    .or_default()
                    .extend(lines.iter().copied());
            }
            cov_deps.extend(rep.check.deps.iter().cloned());
        }
        ran_files += 1;
        sink.checkinfo(&rep.check);
        each(&rep, sink);
        passed += rep.passed;
        failed += rep.failed;
        if !rep.ok() {
            bad_files += 1;
            if fail_fast {
                break;
            }
        }
    }
    let coverage = if coverage_wanted {
        Some(coverage_report(
            session.checker(),
            &files,
            &cov_hits,
            &cov_deps,
            model,
        )?)
    } else {
        None
    };
    explain_cache(store.as_ref(), cache_opts);
    Ok(TestReport {
        files,
        ran: ran_files,
        passed,
        failed,
        files_with_errors: bad_files,
        replayed,
        seed,
        duration_ms: started.elapsed().as_secs_f64() * 1000.0,
        coverage,
    })
}
