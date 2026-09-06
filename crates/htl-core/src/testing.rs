//! `htl test`: discovery, one isolated Lua state per test file, compile, report.
//!
//! The runner owns nothing about assertions. A test file `require`s a library; the
//! bundled default is `htl.test` (`describe` / `it` / `expect`, typed via `test.d.tl`).
//! Any library exposing `run(filter) -> { passed, failed, failures }` under the
//! module name the runner is told about plugs in the same way. A file that uses no
//! such library is judged at file level: it passes if it runs to completion.

use crate::{CheckInfo, Htl, parent_dir, write_if_changed};
use anyhow::{Context, Result};
use mlua::{Function, Table, Value};
use std::path::{Path, PathBuf};

const TEST_LUA: &str = include_str!("../lua/test.lua");
const TEST_DTL: &str = include_str!("../lua/test.d.tl");

/// Module name of the bundled assertion library.
pub const DEFAULT_LIB: &str = "htl.test";

/// Directory holding the bundled `.d.tl` files so the checker can see them
/// (`<tmp>/htl-lib-<version>/`). Written on demand, only when content changes.
pub fn lib_dir() -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("htl-lib-{}", env!("CARGO_PKG_VERSION")));
    write_if_changed(&dir.join("htl").join("test.d.tl"), TEST_DTL)
        .with_context(|| format!("writing bundled declarations under {}", dir.display()))?;
    Ok(dir)
}

impl Htl {
    /// Make `require("htl.test")` work at runtime and its types visible to the checker.
    pub fn install_test_lib(&self) -> Result<()> {
        self.preload(DEFAULT_LIB, TEST_LUA)?;
        self.add_path(&lib_dir()?)?;
        Ok(())
    }
}

/// Outcome of one test file.
#[derive(Debug, Default)]
pub struct FileReport {
    pub path: PathBuf,
    pub check: CheckInfo,
    /// Runtime error outside any test (e.g. the file itself raised).
    pub error: Option<String>,
    pub passed: usize,
    pub failed: usize,
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
    pub name: String,
    pub ok: bool,
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
}

/// Where a test file's snapshots live: `<dir>/__snapshots__/<file stem>/`.
pub fn snapshot_dir(test_file: &Path) -> PathBuf {
    let stem = test_file.file_stem().and_then(|s| s.to_str()).unwrap_or("test");
    parent_dir(test_file).join("__snapshots__").join(stem)
}

impl FileReport {
    pub fn ok(&self) -> bool {
        self.check.ok() && self.error.is_none() && self.failed == 0
    }
}

/// `*_test.tl` anywhere, plus every `.tl` under a directory named `tests`.
/// Explicit file paths are always included. Does not enter [`crate::SKIP_DIRS`],
/// dot-directories or the project's mlua-pkg dir (dependencies' tests are theirs).
pub fn discover_tests(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_file() {
            out.push(p.clone());
            continue;
        }
        let extra = crate::project_skip_dirs(p);
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
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            let in_tests_dir = path
                .strip_prefix(p)
                .ok()
                .map(|rel| rel.components().any(|c| c.as_os_str() == "tests"))
                .unwrap_or(false);
            if name.ends_with("_test.tl") || in_tests_dir {
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

/// One checker for a whole run: every test file gets its own fresh program state
/// (globals, `package.loaded`, module state), but modules are type-checked and
/// generated once and served to every file from the checker's store.
pub struct TestSession {
    checker: Htl,
    lib: String,
    filter: Option<String>,
    opts: RunOptions,
}

impl TestSession {
    pub fn new(lint_spec: Option<&str>, lib: &str, filter: Option<&str>, opts: RunOptions) -> Result<Self> {
        let checker = Htl::new()?;
        if let Some(spec) = lint_spec {
            checker.configure_lints(spec)?;
        }
        Ok(Self { checker, lib: lib.to_string(), filter: filter.map(String::from), opts })
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
}

fn run_in(
    h: &Htl,
    path: &Path,
    r: RunIn<'_>,
    out_code: &mut Option<String>,
) -> Result<FileReport> {
    let RunIn { filter, lib, opts, generated, preload } = r;
    let mut rep = FileReport { path: path.to_path_buf(), ..Default::default() };
    let profile = std::env::var_os("HTL_PROFILE").is_some();
    let mut t0 = std::time::Instant::now();
    let phase = |label: &str, t0: &mut std::time::Instant| {
        if profile {
            eprintln!("profile: {label:<8} {:7.1} ms  {}", t0.elapsed().as_secs_f64() * 1000.0, path.display());
        }
        *t0 = std::time::Instant::now();
    };
    phase("state", &mut t0);
    h.install_test_lib()?;
    let dir = parent_dir(path);
    h.add_path(&dir)?;
    #[cfg(feature = "pkg")]
    if let Some(p) = crate::pkg::Project::find(path) {
        h.apply_project(&p)?;
    }
    if let Some((cfg_path, cfg)) = crate::config::HtlConfig::find(path)? {
        h.apply_config(&parent_dir(&cfg_path), &cfg)?;
    }
    // tests/foo_test.tl commonly requires modules from the project root or src/.
    if dir.file_name().is_some_and(|n| n == "tests")
        && let Some(root) = dir.parent()
    {
        h.add_path(root)?;
        h.add_path(&root.join("src"))?;
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
    if opts.coverage {
        h.coverage_start()?;
    }
    if let Err(e) = h.exec(&code, &format!("@{}", path.display()), &[]) {
        rep.error = Some(crate::user_message(&e));
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
                cfg.set("snapshot_dir", snapshot_dir(path).to_string_lossy().as_ref())?;
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
            let report: Table = run.call((filter, lua_opts))?;
            for (key, into) in [
                ("snapshots_written", &mut rep.snapshots_written),
                ("snapshots_updated", &mut rep.snapshots_updated),
            ] {
                if let Ok(list) = report.get::<Table>(key) {
                    *into = list.sequence_values::<String>().collect::<mlua::Result<_>>()?;
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
