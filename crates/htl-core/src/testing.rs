//! `htl test`: discovery, one isolated Lua state per test file, compile, report.
//!
//! The runner owns nothing about assertions. A test file `require`s a library; the
//! bundled default is `htl.test` (`describe` / `it` / `expect`, typed via `test.d.tl`).
//! Any library exposing `run(filter, opts) -> { passed, failed, failures, tests?,
//! snapshots_written?, snapshots_updated? }` (and optionally `configure({ snapshot_dir,
//! update, mkdir })`, called before the file runs) under the module name `--lib` names
//! plugs in the same way, bringing its own `.d.tl`; `test.lua`'s header is the contract in
//! full. A file that uses no such library is judged at file level: it passes if it runs
//! to completion.
//!
//! # Writing a test
//!
//! ```lua
//! local t = require("htl.test")            -- typed via test.d.tl
//! t.describe("util.add", function()
//!    t.it("adds", function()
//!       t.expect(util.add({x=1,y=2}, {x=10,y=20})):to_equal({x=11,y=22})
//!    end)
//! end)
//! ```
//!
//! `expect(x)` is generic, so `t.expect(1 + 1):to_equal("2")` is a *type* error and the
//! file is refused before it runs; the matchers are `test.d.tl`'s `Expect<T>`, and a
//! matcher that is not one of them is a type error with the list appended. A function
//! returning two values is asserted with `t.expect_all(f()):to_equal(false, "no door")`.
//! `t.rng()` is the run's seeded stream (`rng()`, `rng(m)`, `rng(m, n)`; `math.random` is
//! the same stream), and `to_match_snapshot("name")` compares with a `.snap` under
//! [`snapshot_dir`].
//!
//! # Running
//!
//! `htl test [paths] [--filter substr] [--lib MOD] [--lint rule=level] [--fail-fast] [-v |
//! -q] [--slow MS] [--update] [--seed N] [--coverage [--coverage-lines]] [--lcov FILE]
//! [--junit FILE] [--format json] [--no-cache] [--explain-cache]`. Every run ends with
//! `htl test: seed 8014255196 (repeat with --seed 8014255196)`; `--coverage` prints, per
//! `.tl` module of the project's own, how many of its statements ran, and under a module
//! the functions nothing entered:
//!
//! ```text
//! coverage: src/combat.tl      124/181   68.4%
//!           never ran: resolve_counter (61), flee_path (130)
//! ```
//!
//! `--coverage-lines` adds the unexecuted line ranges; `--lcov` writes the same run as an
//! lcov tracefile ([`crate::project::CoverageReport::lcov`]) and `--junit` as a JUnit XML
//! report; `HTL_PROFILE=1` prints per-phase and per-file timings to stderr.
use crate::{CheckInfo, Htl, parent_dir, write_if_changed};
use anyhow::{Context, Result};
use mlua::{Function, Table, Value};
use std::path::{Path, PathBuf};

const TEST_LUA: &str = include_str!("../lua/test.lua");
const TEST_DTL: &str = include_str!("../lua/test.d.tl");

/// Module name of the bundled assertion library.
pub const DEFAULT_LIB: &str = "htl.test";

/// What this library writes under [`crate::lib_dir`]: its declaration, and the path it
/// takes there. A list of one, and a list rather than the constant because
/// [`crate::lib_dir`] hashes it into the directory's name — the name and the contents come
/// from the same place, so the first cannot describe files the binary does not write.
pub(crate) fn declarations() -> Vec<(String, String)> {
    vec![("htl/test.d.tl".to_string(), TEST_DTL.to_string())]
}

/// [`crate::lib_dir`] with this library's declaration in it: `htl/test.d.tl`, written on
/// demand, only when its content changes.
pub fn lib_dir() -> Result<PathBuf> {
    let dir = crate::lib_dir();
    for (path, source) in declarations() {
        write_if_changed(&dir.join(path), &source)
            .with_context(|| format!("writing bundled declarations under {}", dir.display()))?;
    }
    Ok(dir)
}

impl Htl {
    /// Make `require("htl.test")` work at runtime and its types visible to the checker.
    pub fn install_test_lib(&self) -> Result<()> {
        // A label, not a path: the library ships inside the binary, so `htl/test.tl` is
        // not a file anyone could open. Its frames read `htl.test:404` and stop there.
        self.preload_at(DEFAULT_LIB, &format!("={DEFAULT_LIB}"), TEST_LUA)?;
        self.add_path(&lib_dir()?)?;
        Ok(())
    }
}

/// Outcome of one test file.
#[derive(Debug, Default, Clone)]
pub struct FileReport {
    /// The test file this is about, as it was discovered or named on the command line.
    pub path: PathBuf,
    /// What checking it found. A file that does not type-check still gets a report —
    /// with this carrying the errors and no tests below it — rather than being dropped,
    /// so a run says which files it could not get to.
    pub check: CheckInfo,
    /// Runtime error outside any test (e.g. the file itself raised).
    pub error: Option<String>,
    /// Tests the library reported as passing.
    pub passed: usize,
    /// Tests it reported as failing. `ok` is false while this is non-zero, and a run with
    /// any such file exits 1.
    pub failed: usize,
    /// One message per failure, already formatted by the library — the runner owns no
    /// assertion and so has nothing of its own to say about why one failed.
    pub failures: Vec<String>,
    /// `true` when no test library was used and the verdict is file-level.
    pub file_level: bool,
    /// Per-test outcomes when the library reports them (`htl.test` does).
    pub tests: Vec<TestResult>,
    /// Wall time for the whole file: check, load, and every test.
    pub duration_ms: f64,
    /// Snapshot files created on this run (first `to_match_snapshot` of a name).
    pub snapshots_written: Vec<String>,
    /// Snapshot files rewritten because `update_snapshots` was set.
    pub snapshots_updated: Vec<String>,
    /// With `coverage`: `(chunk source as Lua names it, executed lines)`.
    pub coverage: Vec<(String, Vec<usize>)>,
}

/// One test's outcome, as reported by the assertion library.
#[derive(Debug, Clone, Default)]
pub struct TestResult {
    /// The name the library reported. `htl.test` joins its `describe` and `it` text with
    /// ` > `, and that whole string is what `--filter` matches against.
    pub name: String,
    /// Whether it passed. A file's [`failures`](FileReport::failures) carry why the false
    /// ones did; this is the per-test verdict a reporter lists.
    pub ok: bool,
    /// Wall time for this test alone, against
    /// [`FileReport::duration_ms`](FileReport::duration_ms) for the file around it.
    pub ms: f64,
}

/// Runner options passed through to the library's `run(filter, opts)`.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Stop at the first failing test in a file.
    pub fail_fast: bool,
    /// Rewrite snapshots that differ instead of failing (`htl test --update`).
    pub update_snapshots: bool,
    /// Record executed lines per chunk while the file runs (`htl test --coverage`).
    pub coverage: bool,
    /// Seed for the run (`htl test --seed`). Each file draws from a stream derived from
    /// this and its own path, so one file's values do not depend on which other files ran
    /// or in what order: `--filter` reproduces what the full run did, and a failure can be
    /// looked at again on its own. The runner prints the seed of every run, not only a
    /// failing one: the seed of a run that passed is what reproduces it when a failure two
    /// commits later is compared against it. `None` leaves the state's own seeding alone.
    pub seed: Option<u64>,
    /// The project's `[async]` ([`crate::config::AsyncConfig`]), applied to each file's
    /// state before it runs: the grace a cancelled program gets and whether a CPU-bound
    /// task is preempted. [`crate::project::test`] fills it from the `htl.toml` it was
    /// given; a caller building a [`TestSession`] by hand sets it, or keeps the default.
    pub async_: crate::config::AsyncConfig,
    /// The project's `[lang]` ([`crate::config::LangConfig`]): whether `async` / `await` are
    /// keywords of the files this run checks. Applied to the session's checker when it is
    /// built; filled by [`crate::project::test`] from the `htl.toml`, as `async_` is.
    pub lang: crate::config::LangConfig,
}

/// The seed one file gets, from the run's seed and its path.
///
/// Derived rather than taken from a shared stream, so the answer to "what did this file
/// draw" does not depend on the company it kept. SplitMix64 over an FNV-1a of the path:
/// the point is that neighbouring paths land far apart, not that it is unguessable.
pub fn file_seed(run_seed: u64, path: &Path) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in path.to_string_lossy().as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let mut z = run_seed.wrapping_add(h).wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Where a test file's snapshots live: `<dir>/__snapshots__/<file stem>/`, in the test
/// file's own directory — `tests/__snapshots__/session_test/first_floor.snap` for
/// `tests/session_test.tl`, and beside the module for a test kept under `src/`. The file
/// is `<name>.snap` with every run of characters outside letters, digits, `-`, `.` and
/// `_` in the name turned into one `_` (`test.lua`, `to_match_snapshot`).
pub fn snapshot_dir(test_file: &Path) -> PathBuf {
    let stem = test_file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("test");
    parent_dir(test_file).join("__snapshots__").join(stem)
}

impl FileReport {
    /// Whether this file is a pass: it checked, it did not raise outside a test, and no
    /// test failed. The three are separate fields because a reporter says which of them
    /// went wrong, and one answer is what an exit code needs.
    pub fn ok(&self) -> bool {
        self.check.ok() && self.error.is_none() && self.failed == 0
    }
}

/// Every test file under `paths`, with the default test library ([`DEFAULT_LIB`]); see
/// [`discover_tests_for`].
pub fn discover_tests(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    discover_tests_skipping(paths, &[])
}

/// [`discover_tests`], not entering `skip` either — directories named by path rather than
/// by name — the project model's `Project::not_walked`: a patched dependency's tests are
/// its own suite, not the project's.
pub fn discover_tests_skipping(paths: &[PathBuf], skip: &[PathBuf]) -> Result<Vec<PathBuf>> {
    discover_tests_for(paths, skip, DEFAULT_LIB)
}

/// The test files under `paths`: every `.tl` that `require`s the test library `lib`.
///
/// **A test is a file that loads the test library**, wherever it is and whatever it is
/// called. That is what makes a file one — `describe` and `it` come from the library, so a
/// file that does not load it has no tests to run — and it is the question the runner asks
/// after a file has executed, answered here before instead of after. A file under `tests/`
/// that does not load it is a helper: its tests `require` it, and it is not run on its own.
/// Neither the directory nor the name is part of the rule: `tests/` is where a project
/// keeps tests and the helpers only tests may reach, and `*_test.tl` beside a source file
/// is a convention that reads well, not a rule. What the rule does ask is that tests stay
/// in a file of their own rather than in the module: a module that loads the test library
/// loads it wherever the module is required, including in the program that ships it.
///
/// Which names a file requires is read from its syntax, not from a type check: every
/// `.tl` in the tree is asked, and parsing is cheap where checking is not. A file that does
/// not parse cannot say, and is a test when its text names `lib` at all, so that a broken
/// test file is reported as the failure it is rather than passed over as a helper.
///
/// Explicit file paths are always included. Does not enter [`crate::SKIP_DIRS`],
/// dot-directories or the project's mlua-pkg dir (dependencies' tests are theirs), nor
/// `skip`.
pub fn discover_tests_for(paths: &[PathBuf], skip: &[PathBuf], lib: &str) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    // Built on the first file that needs asking: a run over explicit files never does.
    let mut parser: Option<Htl> = None;
    for p in paths {
        if p.is_file() {
            out.push(p.clone());
            continue;
        }
        let extra = skip.to_vec();
        let root = p.clone();
        let walker = walkdir::WalkDir::new(p)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(move |e| e.path() == root || !crate::is_skipped_dir(e.path(), &extra));
        for e in walker {
            let e = e?;
            let path = e.path();
            if !crate::is_tl_source(path) {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(path) else {
                continue;
            };
            let h = match &parser {
                Some(h) => h,
                None => parser.insert(Htl::new()?),
            };
            let loads_lib = match h.tl_require_names(&src, path)? {
                Some(names) => names.iter().any(|n| n == lib),
                None => src.contains(lib),
            };
            if loads_lib {
                out.push(path.to_path_buf());
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Run one test file in a fresh state. `lib` is the assertion library module to
/// consult for `run(filter)` after the file executed (default: `htl.test`).
pub fn run_test_file(
    path: &Path,
    filter: Option<&str>,
    lib: &str,
    lint_spec: Option<&str>,
    opts: &RunOptions,
) -> Result<FileReport> {
    TestSession::new(lint_spec, lib, filter, opts.clone())?.run_file(path)
}

/// A whole project's tests, run from Rust.
///
/// [`run_test_file`] runs one file and [`crate::project::test`] runs a project; this is
/// the second of those with the parts a command line supplies filled in with defaults, so
/// that a host embedding Teal can put its scripts' tests in `cargo test` instead of
/// shelling out to `htl test`:
///
/// ```rust,ignore
/// #[test]
/// fn teal_tests_pass() {
///     let rep = htl::testing::run_tests(&["scripts".into()], &htl::testing::Suite::default())
///         .expect("the run itself");
///     assert!(rep.ok(), "{:#?}", rep.failures());
/// }
/// ```
///
/// Every field is what `htl test` takes a flag for, and [`Default`] is what the command
/// does with no flags — except the store, which is off here: a run under `cargo test`
/// starts wherever cargo put it, and a host that wants the cache asks for it.
///
/// A whole project is more than one file, so this is the project layer's
/// ([`crate::project::test`]) and asks for its features; the umbrella `htl` crate a host
/// depends on has both.
#[cfg(all(feature = "pkg", feature = "dts"))]
#[derive(Debug, Clone, Default)]
pub struct Suite {
    /// Run only the tests whose name contains this (`htl test --filter`).
    pub filter: Option<String>,
    /// Module name of the assertion library, when it is not [`DEFAULT_LIB`].
    pub lib: Option<String>,
    /// A lint selection, merged after `htl.toml`'s so that it wins (`htl test --lint`).
    pub lint: Option<String>,
    /// Fail-fast, snapshot updating, coverage and the seed. `seed: None` draws one for the
    /// run and [`SuiteReport::seed`] says which, so a failure can be repeated.
    pub run: RunOptions,
    /// Keep a run cache under the project root, as `htl test` does. Off by default.
    pub cache: bool,
}

/// What a [`run_tests`] run found: every file's report, and the counts over them.
#[cfg(all(feature = "pkg", feature = "dts"))]
#[derive(Debug)]
pub struct SuiteReport {
    /// One per file that ran, in the order they ran.
    pub files: Vec<FileReport>,
    /// Every diagnostic the checks produced, over the whole run, as values.
    pub diagnostics: Vec<crate::Diagnostic>,
    /// Tests that passed, summed over the files that ran — tests, not files, so a run of
    /// one file with forty assertions is forty here and one in [`files`](Self::files).
    pub passed: usize,
    /// Tests that failed, summed the same way. A file that failed to check contributes
    /// nothing to either count and shows up in
    /// [`files_with_errors`](Self::files_with_errors) instead, which is why that is the
    /// field [`ok`](Self::ok) reads.
    pub failed: usize,
    /// Files that failed to check, raised, or had a failing test.
    pub files_with_errors: usize,
    /// Files discovered but never run, because `fail_fast` stopped the run.
    pub skipped: usize,
    /// The seed every file's stream was derived from.
    pub seed: u64,
    /// Wall time for the run: discovery, every file's
    /// [`duration_ms`](FileReport::duration_ms), and the coverage report when one was
    /// asked for. Larger than the files' sum rather than equal to it.
    pub duration_ms: f64,
    /// With `run.coverage`: what the line hooks saw.
    pub coverage: Option<crate::project::CoverageReport>,
}

#[cfg(all(feature = "pkg", feature = "dts"))]
impl SuiteReport {
    /// Whether every file that ran was ok. What an assertion in a `#[test]` reads.
    pub fn ok(&self) -> bool {
        self.files_with_errors == 0
    }

    /// What to put in the assertion message: each failing file with what went wrong —
    /// a check that failed, a file that raised, or the tests that did not pass.
    pub fn failures(&self) -> Vec<String> {
        let mut out = Vec::new();
        for f in self.files.iter().filter(|f| !f.ok()) {
            let at = crate::diagnostic::display_path(&f.path);
            if !f.check.ok() {
                out.extend(
                    f.check
                        .error_items
                        .iter()
                        .map(|d| format!("{at}: {}", d.clone().spelled())),
                );
            }
            if let Some(e) = &f.error {
                out.push(format!("{at}: {e}"));
            }
            out.extend(f.failures.iter().map(|m| format!("{at}: {m}")));
        }
        out
    }
}

/// Run the tests under `paths` — a project root, a directory, or the files themselves —
/// and hand back what happened.
///
/// Discovery is [`discover_tests_for`]'s: every `.tl` that loads the suite's test library,
/// minus a patched dependency's own suite. `htl.toml` is found from the
/// first path, as the command does. Nothing is printed and nothing decides an exit code —
/// [`SuiteReport`] is the whole answer, and asserting on it is the caller's.
#[cfg(all(feature = "pkg", feature = "dts"))]
pub fn run_tests(paths: &[PathBuf], suite: &Suite) -> Result<SuiteReport> {
    use crate::project;
    let paths: Vec<PathBuf> = if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths.to_vec()
    };
    let lib = suite.lib.as_deref().unwrap_or(DEFAULT_LIB);
    let cfg = project::config_of(&paths[0])?;
    let model = project::model_of(&cfg, &paths[0])?;
    let skip = project::not_walked(model.as_ref(), &paths, crate::model::Purpose::Test);
    let files = discover_tests_for(&paths, &skip, lib)?;
    let opts = project::TestOptions {
        config: &cfg,
        model: model.as_ref(),
        lint: suite.lint.as_deref(),
        lib: suite.lib.as_deref().unwrap_or(DEFAULT_LIB),
        filter: suite.filter.as_deref(),
        run: suite.run.clone(),
        cache: project::cache_options(
            suite.cache,
            Some(crate::cache::Mode::PerModule),
            &cfg,
            false,
        ),
    };
    let mut sink = project::Sink::new(project::Collect::default());
    let mut reports: Vec<FileReport> = Vec::new();
    let mut diagnostics: Vec<crate::Diagnostic> = Vec::new();
    let rep = project::test(&mut sink, &files, &opts, &mut |r, sink| {
        diagnostics.extend(sink.out().take());
        reports.push(r.clone());
    })?;
    Ok(SuiteReport {
        files: reports,
        diagnostics,
        passed: rep.passed,
        failed: rep.failed,
        files_with_errors: rep.files_with_errors,
        skipped: rep.skipped(),
        seed: rep.seed,
        duration_ms: rep.duration_ms,
        coverage: rep.coverage,
    })
}

/// One checker for a whole run: every test file gets its own fresh program state
/// (globals, `package.loaded`, module state), but modules are type-checked and
/// generated once and served to every file from the checker's store.
pub struct TestSession {
    checker: Htl,
    lib: String,
    filter: Option<String>,
    opts: RunOptions,
    /// The project every file belongs to, when the caller said ([`for_project`](Self::for_project)).
    /// `None` finds each file's own project as the file is run.
    #[cfg(all(feature = "pkg", feature = "dts"))]
    project: Option<crate::model::Project>,
}

impl TestSession {
    /// Build the one checker a run shares. `lint_spec` is the same `--lint` string the
    /// CLI takes and is applied once here; `lib` is the module a test file requires for
    /// its assertions ([`DEFAULT_LIB`] unless the caller has its own); `filter` and
    /// `opts` are handed to that library's `run` for every file.
    pub fn new(
        lint_spec: Option<&str>,
        lib: &str,
        filter: Option<&str>,
        opts: RunOptions,
    ) -> Result<Self> {
        let checker = Htl::new()?;
        if let Some(spec) = lint_spec {
            checker.configure_lints(spec)?;
        }
        checker.set_lang(&opts.lang)?;
        Ok(Self {
            checker,
            lib: lib.to_string(),
            filter: filter.map(String::from),
            opts,
            #[cfg(all(feature = "pkg", feature = "dts"))]
            project: None,
        })
    }

    /// Run every file of this session as a file of `project`, whose
    /// [model](crate::model) the caller has already built.
    ///
    /// A file runs with its project's directories on its search path as a test sees them
    /// ([`View::Test`](crate::model::View::Test)): the project's sources and declarations,
    /// its test root, its dependencies. Without this, each file's project is found from the
    /// file as it runs ([`model::Project::discover`](crate::model::Project::discover)), which
    /// is the same answer at the cost of reading the project once per file; a file with no
    /// project above it resolves its `require`s in its own directory.
    #[cfg(all(feature = "pkg", feature = "dts"))]
    pub fn for_project(mut self, project: crate::model::Project) -> Self {
        self.project = Some(project);
        self
    }

    /// The session's checker (for [`Htl::executable_ranges`] on the sources a run touched).
    pub fn checker(&self) -> &Htl {
        &self.checker
    }

    /// Run one file in a fresh program state borrowing the session's checker. The
    /// checker's search path is restored afterwards so files do not see each other's
    /// directories.
    pub fn run_file(&self, path: &Path) -> Result<FileReport> {
        self.run_file_with(path, None, &[]).map(|(rep, _)| rep)
    }

    /// Run one file, reusing Lua the caller generated earlier.
    ///
    /// `generated` is `(the Lua, what checking it reported)`. Everything before the codegen
    /// still happens — the searcher, the search path, the project and the config all have to
    /// be in place before the code can execute — and everything after it happens as usual.
    /// Only the check and the codegen are skipped.
    ///
    /// The run itself is never reused, and this signature cannot express reusing it: a test
    /// has to run to say whether it passes.
    ///
    /// Returns the report and, when this call generated the Lua rather than being handed it,
    /// that Lua — so a caller keeping a cache has something to keep.
    /// `preload` is `(module name, its generated Lua, the file it came from)` for modules
    /// this file will require. Each one goes in front of the searcher, so requiring it does
    /// not check and generate it during the run. A module not in the list still loads the
    /// usual way; the list is an optimisation, never a restriction on what can be required.
    pub fn run_file_with(
        &self,
        path: &Path,
        generated: Option<(&str, &CheckInfo)>,
        preload: &[(String, String, PathBuf)],
    ) -> Result<(FileReport, Option<String>)> {
        let started = std::time::Instant::now();
        let saved = self.checker.search_path()?;
        let h = Htl::with_checker(&self.checker)?;
        #[cfg(all(feature = "pkg", feature = "dts"))]
        let found;
        #[cfg(all(feature = "pkg", feature = "dts"))]
        let project = match &self.project {
            Some(p) => Some(p),
            None => {
                found = crate::model::Project::discover(path)?;
                found.as_ref()
            }
        };
        let mut code = None;
        let out = run_in(
            &h,
            path,
            RunIn {
                filter: self.filter.as_deref(),
                lib: &self.lib,
                opts: &self.opts,
                generated,
                preload,
                #[cfg(all(feature = "pkg", feature = "dts"))]
                project,
            },
            &mut code,
        );
        self.checker.set_search_path(&saved)?;
        let mut rep = out?;
        rep.duration_ms = started.elapsed().as_secs_f64() * 1000.0;
        Ok((rep, code))
    }
}

/// What one file's run needs from its session, and what the caller already has for it.
struct RunIn<'a> {
    filter: Option<&'a str>,
    lib: &'a str,
    opts: &'a RunOptions,
    /// Lua and diagnostics from an earlier run, when the caller kept them.
    generated: Option<(&'a str, &'a CheckInfo)>,
    /// Modules to put in front of the searcher before the file executes.
    preload: &'a [(String, String, PathBuf)],
    /// The project the file belongs to, when the session has one.
    #[cfg(all(feature = "pkg", feature = "dts"))]
    project: Option<&'a crate::model::Project>,
}

fn run_in(h: &Htl, path: &Path, r: RunIn<'_>, out_code: &mut Option<String>) -> Result<FileReport> {
    let RunIn {
        filter,
        lib,
        opts,
        generated,
        preload,
        #[cfg(all(feature = "pkg", feature = "dts"))]
        project,
    } = r;
    let mut rep = FileReport {
        path: path.to_path_buf(),
        ..Default::default()
    };
    let profile = std::env::var_os("HTL_PROFILE").is_some();
    let mut t0 = std::time::Instant::now();
    let phase = |label: &str, t0: &mut std::time::Instant| {
        if profile {
            eprintln!(
                "profile: {label:<8} {:7.1} ms  {}",
                t0.elapsed().as_secs_f64() * 1000.0,
                path.display()
            );
        }
        *t0 = std::time::Instant::now();
    };
    phase("state", &mut t0);
    h.configure_async(&opts.async_)?;
    h.install_test_lib()?;
    #[cfg(feature = "std")]
    h.install_std()?;
    // A test on the executor may spawn tasks: the library that does, on the same terms.
    #[cfg(feature = "async")]
    h.install_task_lib()?;
    // What the file may `require`: the project's directories as a test sees them, from
    // its model. A file that belongs to no project reads its own directory, the one place
    // it names by itself. A build without the model (`pkg` or `dts` off) has no
    // dependencies to reach and reads the `htl.toml` directories the config lists.
    #[cfg(all(feature = "pkg", feature = "dts"))]
    match project {
        Some(m) => h.apply_model(m, crate::model::View::Test)?,
        None => {
            h.drop_cwd_search_path()?;
            h.add_path(&parent_dir(path))?
        }
    }
    #[cfg(not(all(feature = "pkg", feature = "dts")))]
    {
        h.add_path(&parent_dir(path))?;
        if let Some((cfg_path, cfg)) = crate::config::HtlConfig::find(path)? {
            h.apply_config(&parent_dir(&cfg_path), &cfg)?;
        }
    }
    h.install_searcher()?;
    // Before the searcher gets a chance to be asked. Position 1 beats position 2.
    for (name, code, from) in preload {
        h.preload_generated(name, code, from)?;
    }
    h.set_arg(&path.to_string_lossy(), &[])?;
    phase("setup", &mut t0);

    let (code, check) = match generated {
        Some((code, check)) => (Some(code.to_string()), check.clone()),
        None => {
            let (code, check) = h.gen_lua(path)?;
            // Hand the caller what was generated, before any of the early returns below: a
            // file whose tests fail still generated the Lua that failed, and the next run
            // should not have to generate it again to find that out.
            *out_code = code.clone();
            (code, check)
        }
    };
    phase("gen_lua", &mut t0);
    rep.check = check;
    let Some(code) = code else { return Ok(rep) };
    // Before the file runs, so a module it requires draws from the same stream. Seeding
    // the state's own generator rather than handing out a private one means a test that
    // already calls `math.random` becomes reproducible without being rewritten.
    if let Some(run_seed) = opts.seed {
        let math: Table = h.lua().globals().get("math")?;
        let randomseed: Function = math.get("randomseed")?;
        randomseed.call::<()>(file_seed(run_seed, path) as i64)?;
    }
    if opts.coverage {
        h.coverage_start()?;
    }
    // The chunk is named as a report names the file, so a runtime error's position and
    // its traceback read like the check's (`tests/a_test.tl:2:`, not `./tests/...`).
    let chunk = format!("@{}", crate::diagnostic::display_path(path));
    // On the executor: the file's chunk is a root coroutine, so a test that calls a host's
    // `async fn` suspends and resumes instead of failing to yield. The verdict below runs
    // the test bodies, so it is a root of its own. Neither root is ever cancelled here:
    // `htl test` has no timeout, and `--slow` only marks.
    #[cfg(feature = "async")]
    let token = crate::mlua_isle::runtime::CancelToken::new();
    #[cfg(feature = "async")]
    let ran = h.run_blocking(&code, &chunk, &[], &token);
    #[cfg(not(feature = "async"))]
    let ran = h.exec(&code, &chunk, &[]);
    if let Err(e) = ran {
        // With the frames: a file that raised while loading is a development failure, and
        // the per-test failures beside it have carried a traceback all along.
        rep.error = Some(crate::developer_message(&e));
        if opts.coverage {
            rep.coverage = h.coverage_stop()?;
        }
        return Ok(rep);
    }
    phase("exec", &mut t0);

    // Did the file load the assertion library? Then ask it for the verdict.
    let package: Table = h.lua().globals().get("package")?;
    let loaded: Table = package.get("loaded")?;
    match loaded.get::<Value>(lib)? {
        Value::Table(t) => {
            // Snapshots: tell the library where this file's live and whether to
            // rewrite them. Lua cannot create a directory, so it borrows one.
            if let Ok(configure) = t.get::<Function>("configure") {
                let cfg = h.lua().create_table()?;
                cfg.set(
                    "snapshot_dir",
                    snapshot_dir(path).to_string_lossy().as_ref(),
                )?;
                cfg.set("update", opts.update_snapshots)?;
                cfg.set(
                    "mkdir",
                    h.lua().create_function(|_, dir: String| {
                        std::fs::create_dir_all(&dir).map_err(mlua::Error::external)
                    })?,
                )?;
                configure.call::<()>(cfg)?;
            }
            let run: Function = t.get("run")?;
            let lua_opts = h.lua().create_table()?;
            lua_opts.set("fail_fast", opts.fail_fast)?;
            #[cfg(feature = "async")]
            let report: Table = {
                let mut out = h.call_blocking(run, (filter, lua_opts), &token)?;
                match out.pop_front() {
                    Some(Value::Table(t)) => t,
                    other => {
                        anyhow::bail!("the test library's run returned {other:?}, not a table")
                    }
                }
            };
            #[cfg(not(feature = "async"))]
            let report: Table = run.call((filter, lua_opts))?;
            for (key, into) in [
                ("snapshots_written", &mut rep.snapshots_written),
                ("snapshots_updated", &mut rep.snapshots_updated),
            ] {
                if let Ok(list) = report.get::<Table>(key) {
                    *into = list
                        .sequence_values::<String>()
                        .collect::<mlua::Result<_>>()?;
                }
            }
            phase("run", &mut t0);
            rep.passed = report.get::<Option<usize>>("passed")?.unwrap_or(0);
            rep.failed = report.get::<Option<usize>>("failed")?.unwrap_or(0);
            if let Ok(f) = report.get::<Table>("failures") {
                rep.failures = f.sequence_values::<String>().collect::<mlua::Result<_>>()?;
            }
            if let Ok(tests) = report.get::<Table>("tests") {
                for tr in tests.sequence_values::<Table>() {
                    let tr = tr?;
                    rep.tests.push(TestResult {
                        name: tr.get::<Option<String>>("name")?.unwrap_or_default(),
                        ok: tr.get::<Option<bool>>("ok")?.unwrap_or(false),
                        ms: tr.get::<Option<f64>>("ms")?.unwrap_or(0.0),
                    });
                }
            }
        }
        _ => {
            rep.file_level = true;
            rep.passed = 1;
        }
    }
    if opts.coverage {
        rep.coverage = h.coverage_stop()?;
    }
    Ok(rep)
}
