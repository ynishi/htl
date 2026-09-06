//! htl CLI. Also built as `cargo-htl` so `cargo htl <verb>` works.
//!
//! - `htl check <paths...>`   type-check files or directories
//! - `htl gen <file.tl>`      emit readable Lua (escape hatch)
//! - `htl run <file.tl|.hb>`  type-check then execute (strict: type errors abort)
//! - `htl build <dir>`        compile a tree of `.tl` into one stripped-bytecode bundle

mod cache;
mod report;
mod scaffold;

/// Output format of `check` / `test`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
enum Format {
    /// Human-readable lines on stderr
    Text,
    /// One JSON document on stdout (see README, "Machine-readable output")
    Json,
}

/// How `htl check` grains its cache. Separate from whether it caches at all, which is
/// `--no-cache`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
enum CacheModeArg {
    /// One entry per module: an edit costs that module and what depends on it
    PerModule,
    /// One entry per run: any edit re-checks everything, a run with no edits reads one file
    WholeRun,
}

#[derive(Subcommand)]
enum CacheCmd {
    /// Delete this project's stored check results
    Clear {
        /// A path inside the project; the store is found beside its htl.toml
        path: Option<PathBuf>,
    },
}

impl From<CacheModeArg> for cache::Mode {
    fn from(m: CacheModeArg) -> Self {
        match m {
            CacheModeArg::PerModule => cache::Mode::PerModule,
            CacheModeArg::WholeRun => cache::Mode::WholeRun,
        }
    }
}

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use htl::bundle::Bundle;
use htl::{CheckInfo, Htl};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "htl", version, about = "Teal, hidden: run / check / build .tl on mlua")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Type-check .tl files or directories (htl lints are reported as `lint:`)
    Check {
        paths: Vec<PathBuf>,
        /// Treat warnings and lints as errors
        #[arg(long)]
        strict: bool,
        /// Lint rules on top of the defaults, e.g. `+no-any,-shadow-local`
        #[arg(long)]
        lint: Option<String>,
        /// List lint rules and exit
        #[arg(long)]
        list_lints: bool,
        /// Output format
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
        /// Check even if an identical run is cached, and do not store this one
        #[arg(long)]
        no_cache: bool,
        /// How the cache is grained; overrides `[cache] mode` in htl.toml
        #[arg(long, value_enum)]
        cache_mode: Option<CacheModeArg>,
    },
    /// Run tests: `*_test.tl` and `tests/**/*.tl`, one isolated state per file
    Test {
        paths: Vec<PathBuf>,
        /// Only run tests whose "suite > name" contains this substring
        #[arg(long)]
        filter: Option<String>,
        /// Assertion library module consulted for the verdict (must expose `run(filter)`)
        #[arg(long, default_value = htl::testing::DEFAULT_LIB)]
        lib: String,
        /// Lint rules on top of the defaults, e.g. `+no-any`
        #[arg(long)]
        lint: Option<String>,
        /// Stop at the first failure (within a file, and across files)
        #[arg(long)]
        fail_fast: bool,
        /// Print every test with its time (default: only failures and files)
        #[arg(short, long)]
        verbose: bool,
        /// Only failures (with details), errors, and the summary line; no per-file `ok` lines
        #[arg(short, long, conflicts_with = "verbose")]
        quiet: bool,
        /// Also print tests slower than this many milliseconds
        #[arg(long, value_name = "MS")]
        slow: Option<f64>,
        /// Rewrite snapshots (`to_match_snapshot`) that differ instead of failing
        #[arg(long)]
        update: bool,
        /// Report executed statements per `.tl` module the tests reached (slower: a line hook)
        #[arg(long)]
        coverage: bool,
        /// With --coverage, also list the unexecuted line ranges of each module
        #[arg(long, requires = "coverage")]
        coverage_lines: bool,
        /// Output format
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
    },
    /// Apply the fixes diagnostics carry (safe ones by default; see README, "Fixing")
    Fix {
        paths: Vec<PathBuf>,
        /// Only fixes of these rules (e.g. `forward-ref,explicit-number`)
        #[arg(long, value_delimiter = ',')]
        rule: Vec<String>,
        /// Also apply `unsafe` fixes (may change what the program does)
        #[arg(long = "unsafe")]
        unsafe_fixes: bool,
        /// Compute and report, write nothing
        #[arg(long)]
        dry_run: bool,
        /// Like --dry-run, and print a unified diff per file that would change
        #[arg(long)]
        diff: bool,
        /// Fix files that git reports as modified or staged
        #[arg(long)]
        allow_dirty: bool,
        /// Fix files that are not inside a git repository
        #[arg(long)]
        allow_no_vcs: bool,
        /// Exit 1 when any file was changed (for CI)
        #[arg(long)]
        exit_non_zero_on_fix: bool,
        /// Output format
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
    },
    /// Write the `.d.tl` files declared by `#[host_module(dts = ..)]` / `#[teal(dts = ..)]`
    /// in a Rust crate, without building it (check / run / test / build do this automatically)
    Dts {
        /// Crate root or any path inside it (default: current directory)
        dir: Option<PathBuf>,
    },
    /// Create a new Teal project directory
    New {
        name: String,
        /// Library only (no src/main.tl)
        #[arg(long)]
        lib: bool,
        /// Also emit a Rust host (Cargo.toml + src/main.rs with include_tl!)
        #[arg(long)]
        embed: bool,
    },
    /// Fill in the scaffold files that are missing in an existing directory
    Init {
        dir: Option<PathBuf>,
        #[arg(long)]
        lib: bool,
        #[arg(long)]
        embed: bool,
    },
    /// Package management: passthrough to `mlua-pkg` (install / add / update / clean),
    /// run at the nearest `mlua-pkg.toml` project root
    Pkg {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Manage the check cache (see README, "Caching")
    Cache {
        #[command(subcommand)]
        cmd: CacheCmd,
    },
    /// Format .tl files in place (indentation / blank lines / trailing whitespace)
    Fmt {
        paths: Vec<PathBuf>,
        /// Do not write; exit 1 if any file would change
        #[arg(long)]
        check: bool,
        /// Indent width in spaces (default: `[fmt] indent` in htl.toml, else 3)
        #[arg(long)]
        indent: Option<usize>,
    },
    /// Emit readable Lua for one .tl file (escape hatch)
    Gen {
        file: PathBuf,
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Run a .tl script or a .hb bundle
    Run {
        file: PathBuf,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Compile a directory of .tl into a stripped-bytecode bundle
    Build {
        /// Entry `.tl` file: it and everything it requires are bundled (a directory
        /// bundles every `.tl` under it, the older snapshot form)
        entry: PathBuf,
        #[arg(short, long, default_value = "app.hb")]
        out: PathBuf,
        /// Entry module name when `entry` is a directory
        #[arg(short, long, default_value = "main")]
        main: String,
        /// Keep debug info (line numbers, local names) in the bytecode
        #[arg(long)]
        debug: bool,
        /// Store generated Lua source instead of bytecode (loads on any Lua build)
        #[arg(long)]
        source: bool,
        /// Modules to bundle that only a dynamic require reaches (also `[build] extra`)
        #[arg(long, value_delimiter = ',')]
        extra: Vec<String>,
        /// Modules the host provides (also `[build] host`; `.d.tl`-only modules are implied)
        #[arg(long, value_delimiter = ',')]
        host: Vec<String>,
    },
}

pub fn run() -> ExitCode {
    // Invoked as `cargo htl ...` -> argv = ["cargo-htl", "htl", ...]; drop the "htl".
    let mut argv: Vec<String> = std::env::args().collect();
    let is_cargo = Path::new(&argv[0])
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s == "cargo-htl")
        .unwrap_or(false);
    if is_cargo && argv.get(1).map(String::as_str) == Some("htl") {
        argv.remove(1);
    }
    match real_main(Cli::parse_from(argv)) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("htl: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn real_main(cli: Cli) -> Result<ExitCode> {
    match cli.cmd {
        Cmd::Check { paths, strict, lint, list_lints, format, no_cache, cache_mode } => cmd_check(
            &paths,
            strict,
            lint.as_deref(),
            list_lints,
            format == Format::Json,
            !no_cache,
            cache_mode.map(Into::into),
        ),
        Cmd::Fmt { paths, check, indent } => cmd_fmt(&paths, check, indent),
        Cmd::Test { paths, filter, lib, lint, fail_fast, verbose, quiet, slow, update, coverage, coverage_lines, format } => {
            cmd_test(
                &paths,
                filter.as_deref(),
                &lib,
                lint.as_deref(),
                TestFlags { fail_fast, verbose, quiet, slow, update, coverage, coverage_lines, json: format == Format::Json },
            )
        }
        Cmd::Pkg { args } => cmd_pkg(&args),
        Cmd::Cache { cmd } => match cmd {
            CacheCmd::Clear { path } => cmd_cache_clear(path.as_deref()),
        },
        Cmd::Dts { dir } => cmd_dts(dir.as_deref()),
        Cmd::New { name, lib, embed } => cmd_new(&name, lib, embed),
        Cmd::Init { dir, lib, embed } => cmd_init(dir.as_deref(), lib, embed),
        Cmd::Gen { file, out } => cmd_gen(&file, out.as_deref()),
        Cmd::Run { file, args } => cmd_run(&file, &args),
        Cmd::Fix { paths, rule, unsafe_fixes, dry_run, diff, allow_dirty, allow_no_vcs, exit_non_zero_on_fix, format } => cmd_fix(
            &paths,
            FixFlags {
                rule,
                unsafe_fixes,
                dry_run: dry_run || diff,
                diff,
                allow_dirty,
                allow_no_vcs,
                exit_non_zero_on_fix,
                json: format == Format::Json,
            },
        ),
        Cmd::Build { entry, out, main, debug, source, extra, host } => {
            cmd_build(&entry, &out, &main, htl::link::LinkOptions { debug, source, extra, host })
        }
    }
}

fn print_checkinfo(c: &CheckInfo) {
    for w in &c.warnings {
        eprintln!("warning: {w}");
    }
    for l in &c.lints {
        eprintln!("lint: {l}");
    }
    for e in &c.errors {
        eprintln!("error: {e}");
    }
}

/// If `start` is inside a Rust crate, (re)generate the `.d.tl` files its
/// `#[host_module]` / `#[derive(TealRecord)]` declare, so the checker sees Rust-side
/// modules before any `cargo build`. Quiet unless something was written.
fn auto_dts(start: &Path) -> Result<()> {
    let Some(root) = htl::dts::find_cargo_package_root(start) else { return Ok(()) };
    let results = htl::dts::generate_crate(&root).map_err(|e| anyhow::anyhow!("htl dts: {e}"))?;
    for (target, written) in results {
        if written {
            eprintln!("dts: wrote {}", target.strip_prefix(&root).unwrap_or(&target).display());
        }
    }
    Ok(())
}

fn cmd_dts(dir: Option<&Path>) -> Result<ExitCode> {
    let start = match dir {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let Some(root) = htl::dts::find_cargo_package_root(&start) else {
        bail!("no Cargo.toml with a [package] section found at or above {}", start.display());
    };
    let results = htl::dts::generate_crate(&root).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut written = 0usize;
    for (target, w) in &results {
        let rel = target.strip_prefix(&root).unwrap_or(target).display();
        eprintln!("  {} {}", if *w { "wrote    " } else { "unchanged" }, rel);
        written += usize::from(*w);
    }
    eprintln!("htl dts: {} declaration(s) from {}, {} written", results.len(), root.display(), written);
    Ok(ExitCode::SUCCESS)
}

/// If `start` is inside an `mlua-pkg.toml` project, expose its vendored deps to the
/// checker / strict searcher. Returns the project when found.
fn apply_project(h: &Htl, start: &Path) -> Result<Option<htl::pkg::Project>> {
    let Some(p) = htl::pkg::Project::find(start) else { return Ok(None) };
    h.apply_project(&p)?;
    Ok(Some(p))
}

fn report_scaffold(dir: &Path, written: &[PathBuf]) {
    for p in written {
        let rel = p.strip_prefix(dir).unwrap_or(p);
        eprintln!("  created {}", rel.display());
    }
    eprintln!("htl: {} file(s) written under {}", written.len(), dir.display());
}

fn cmd_new(name: &str, lib: bool, embed: bool) -> Result<ExitCode> {
    let dir = PathBuf::from(name);
    let pkg_name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid project name: {name}"))?
        .to_string();
    let written = scaffold::scaffold(&dir, &pkg_name, &scaffold::Options { lib, embed }, true)?;
    report_scaffold(&dir, &written);
    eprintln!("next: cd {} && htl test", dir.display());
    Ok(ExitCode::SUCCESS)
}

fn cmd_init(dir: Option<&Path>, lib: bool, embed: bool) -> Result<ExitCode> {
    let dir = match dir {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let abs = std::fs::canonicalize(&dir).unwrap_or(dir.clone());
    let name = abs
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("cannot derive a project name from {}", abs.display()))?
        .to_string();
    let written = scaffold::scaffold(&dir, &name, &scaffold::Options { lib, embed }, false)?;
    if written.is_empty() {
        eprintln!("htl init: nothing to do, all scaffold files already exist");
    } else {
        report_scaffold(&dir, &written);
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_pkg(args: &[String]) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let root = htl::pkg::Project::find(&cwd).map(|p| p.root).unwrap_or(cwd);
    let status = std::process::Command::new("mlua-pkg")
        .args(args)
        .current_dir(&root)
        .status();
    match status {
        Ok(s) => Ok(if s.success() { ExitCode::SUCCESS } else { ExitCode::FAILURE }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("`mlua-pkg` binary not found on PATH (install with `cargo install mlua-pkg`)")
        }
        Err(e) => Err(e.into()),
    }
}

struct TestFlags {
    fail_fast: bool,
    verbose: bool,
    quiet: bool,
    slow: Option<f64>,
    update: bool,
    coverage: bool,
    coverage_lines: bool,
    json: bool,
}

/// Coverage over the run: every `.tl` the test files' checks depended on (so a module
/// no test reached shows 0%), with the executed statements from the line hooks.
fn coverage_report(
    checker: &Htl,
    test_files: &[PathBuf],
    hits: &std::collections::HashMap<PathBuf, std::collections::BTreeSet<usize>>,
    deps: &std::collections::BTreeSet<PathBuf>,
) -> Result<report::CoverageReport> {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let tests: std::collections::HashSet<PathBuf> = test_files.iter().map(|p| canon(p)).collect();
    let mut sources: std::collections::BTreeSet<PathBuf> = deps.iter().map(|p| canon(p)).collect();
    sources.extend(hits.keys().cloned());
    let cwd = std::env::current_dir().unwrap_or_default();
    let (mut tot_exec, mut tot_all) = (0usize, 0usize);
    // (module, executed, all, unexecuted ranges)
    type Row = (String, usize, usize, Vec<(usize, usize)>);
    let mut rows: Vec<Row> = Vec::new();
    for src in &sources {
        let name = src.to_string_lossy();
        if !name.ends_with(".tl") || name.ends_with(".d.tl") || tests.contains(src) || name.contains("/htl-lib-") {
            continue;
        }
        let ranges = checker.executable_ranges(src)?;
        if ranges.is_empty() {
            continue;
        }
        let empty = std::collections::BTreeSet::new();
        let ran = hits.get(src).unwrap_or(&empty);
        let mut missed = Vec::new();
        let mut executed = 0usize;
        for &(a, b) in &ranges {
            if ran.range(a..=b).next().is_some() {
                executed += 1;
            } else {
                missed.push((a, b));
            }
        }
        tot_exec += executed;
        tot_all += ranges.len();
        let shown = src.strip_prefix(&cwd).unwrap_or(src).to_string_lossy().into_owned();
        rows.push((shown, executed, ranges.len(), missed));
    }
    Ok(report::CoverageReport {
        modules: rows
            .into_iter()
            .map(|(path, executed, total, unexecuted)| report::CoverageModule { path, executed, total, unexecuted })
            .collect(),
        executed: tot_exec,
        total: tot_all,
    })
}

fn print_coverage(cov: &report::CoverageReport, with_lines: bool) {
    let width = cov.modules.iter().map(|m| m.path.len()).max().unwrap_or(4).max(5);
    for m in &cov.modules {
        eprintln!(
            "coverage: {:<width$}  {:>5}/{:<5} {:5.1}%",
            m.path,
            m.executed,
            m.total,
            100.0 * m.executed as f64 / m.total as f64
        );
        if with_lines && !m.unexecuted.is_empty() {
            let spans: Vec<String> = m
                .unexecuted
                .iter()
                .map(|(a, b)| if a == b { a.to_string() } else { format!("{a}-{b}") })
                .collect();
            eprintln!("          unexecuted: {}", spans.join(", "));
        }
    }
    if cov.total > 0 {
        eprintln!(
            "coverage: {:<width$}  {:>5}/{:<5} {:5.1}%  (statements; code run inside coroutines is not seen)",
            "total",
            cov.executed,
            cov.total,
            100.0 * cov.executed as f64 / cov.total as f64
        );
    } else {
        eprintln!("coverage: no .tl module reached");
    }
}

fn cmd_test(paths: &[PathBuf], filter: Option<&str>, lib: &str, lint: Option<&str>, flags: TestFlags) -> Result<ExitCode> {
    let paths = if paths.is_empty() { vec![PathBuf::from(".")] } else { paths.to_vec() };
    let opts = htl::testing::RunOptions {
        fail_fast: flags.fail_fast,
        update_snapshots: flags.update,
        coverage: flags.coverage,
    };
    let mut cov_hits: std::collections::HashMap<PathBuf, std::collections::BTreeSet<usize>> = Default::default();
    let mut cov_deps: std::collections::BTreeSet<PathBuf> = Default::default();
    if let Some(first) = paths.first() {
        auto_dts(first)?;
    }
    let file_spec = load_config(&paths[0])?.map(|(_, _, c)| c.lint_spec()).unwrap_or_default();
    let spec = htl::config::join_specs([file_spec.as_str(), lint.unwrap_or("")]);
    let lint = if spec.is_empty() { None } else { Some(spec.as_str()) };
    let files = htl::testing::discover_tests(&paths)?;
    if files.is_empty() {
        eprintln!("htl test: no test files found (looked for *_test.tl and tests/**/*.tl)");
        return Ok(ExitCode::FAILURE);
    }
    let (mut passed, mut failed, mut bad_files, mut ran_files) = (0usize, 0usize, 0usize, 0usize);
    let started = std::time::Instant::now();
    // One checker for the run; each file still gets a fresh program state.
    let session = htl::testing::TestSession::new(lint, lib, filter, opts)?;
    let mut sink = report::Sink::new(flags.json);
    let mut json_files: Vec<report::TestFile> = Vec::new();
    for f in &files {
        let rep = session.run_file(f)?;
        if flags.coverage {
            for (source, lines) in &rep.coverage {
                // Lua names a file chunk "@<path>"; bundles and preloads ("=name") have no file.
                let Some(path) = source.strip_prefix('@') else { continue };
                let key = std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
                cov_hits.entry(key).or_default().extend(lines.iter().copied());
            }
            cov_deps.extend(rep.check.deps.iter().cloned());
        }
        ran_files += 1;
        sink.checkinfo(&rep.check);
        if flags.json {
            json_files.push(report::TestFile::from_report(&rep, sink.take()));
        }
        let tag = if rep.ok() { "ok  " } else { "FAIL" };
        let detail = if !rep.check.ok() {
            "type check failed".to_string()
        } else if let Some(e) = &rep.error {
            format!("error: {e}")
        } else if rep.file_level {
            "ran to completion (no test library used)".to_string()
        } else {
            format!("{} passed, {} failed", rep.passed, rep.failed)
        };
        // Quiet: a passing file is silence; failures, errors and slow tests still show.
        // JSON: nothing on stderr, the document carries it all.
        let show_file = !flags.json && (!flags.quiet || !rep.ok());
        if show_file {
            eprintln!("{tag} {}  ({detail}, {:.0} ms)", f.display(), rep.duration_ms);
        }
        for tr in &rep.tests {
            let slow = flags.slow.is_some_and(|ms| tr.ms >= ms);
            if !flags.json && (flags.verbose || slow) {
                if flags.quiet && !show_file {
                    // The file line was skipped: name the file with the slow test.
                    eprintln!("slow {}  {}  ({:.1} ms)", f.display(), tr.name, tr.ms);
                    continue;
                }
                let mark = if tr.ok { "ok  " } else { "FAIL" };
                let note = if slow && !flags.verbose { "  [slow]" } else { "" };
                eprintln!("      {mark} {}  ({:.1} ms){note}", tr.name, tr.ms);
            }
        }
        if !flags.json {
            for m in &rep.failures {
                eprintln!("      - {m}");
            }
            // A snapshot written or rewritten is a change on disk: always say so, even in -q.
            for p in &rep.snapshots_written {
                eprintln!("snapshot written: {p}");
            }
            for p in &rep.snapshots_updated {
                eprintln!("snapshot updated: {p}");
            }
        }
        passed += rep.passed;
        failed += rep.failed;
        if !rep.ok() {
            bad_files += 1;
            if flags.fail_fast {
                break;
            }
        }
    }
    let coverage = if flags.coverage {
        Some(coverage_report(session.checker(), &files, &cov_hits, &cov_deps)?)
    } else {
        None
    };
    let skipped = files.len() - ran_files;
    let duration_ms = started.elapsed().as_secs_f64() * 1000.0;
    if flags.json {
        report::emit(&report::TestReport {
            files: json_files,
            summary: report::TestSummary {
                files: files.len(),
                files_run: ran_files,
                passed,
                failed,
                files_with_errors: bad_files,
                duration_ms,
                ok: bad_files == 0,
            },
            coverage,
        })?;
    } else {
        if let Some(cov) = &coverage {
            print_coverage(cov, flags.coverage_lines);
        }
        eprintln!(
            "htl test: {} file(s), {} passed, {} failed, {} file(s) with errors{} ({:.0} ms)",
            ran_files,
            passed,
            failed,
            bad_files,
            if skipped > 0 { format!(", {skipped} file(s) not run (--fail-fast)") } else { String::new() },
            duration_ms
        );
    }
    Ok(if bad_files == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

/// Nearest `htl.toml` above the first path: `(dir holding it, path, config)`.
fn load_config(first: &Path) -> Result<Option<(PathBuf, PathBuf, htl::config::HtlConfig)>> {
    Ok(htl::config::HtlConfig::find(first)?.map(|(p, c)| (htl::parent_dir(&p), p, c)))
}

fn cmd_fmt(paths: &[PathBuf], check: bool, indent: Option<usize>) -> Result<ExitCode> {
    let paths = if paths.is_empty() { vec![PathBuf::from(".")] } else { paths.to_vec() };
    let cfg = load_config(&paths[0])?;
    let indent = indent
        .or_else(|| cfg.as_ref().and_then(|(_, _, c)| c.fmt.indent))
        .unwrap_or(3);
    let h = Htl::new()?;
    let files = htl::collect_tl(&paths)?;
    let (mut changed, mut failed) = (0usize, 0usize);
    for f in &files {
        let before = fs::read_to_string(f).with_context(|| format!("reading {}", f.display()))?;
        let after = match h.format_file(f, indent) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: {e}");
                failed += 1;
                continue;
            }
        };
        if after != before {
            changed += 1;
            if check {
                eprintln!("would reformat: {}", f.display());
            } else {
                fs::write(f, after).with_context(|| format!("writing {}", f.display()))?;
                eprintln!("reformatted: {}", f.display());
            }
        }
    }
    eprintln!(
        "htl fmt: {} file(s), {} {}, {} failed",
        files.len(),
        changed,
        if check { "would change" } else { "reformatted" },
        failed
    );
    let fail = failed > 0 || (check && changed > 0);
    Ok(if fail { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

struct FixFlags {
    rule: Vec<String>,
    unsafe_fixes: bool,
    dry_run: bool,
    diff: bool,
    allow_dirty: bool,
    allow_no_vcs: bool,
    exit_non_zero_on_fix: bool,
    json: bool,
}

fn cmd_fix(paths: &[PathBuf], flags: FixFlags) -> Result<ExitCode> {
    use htl::fix::{FixOptions, fix_file, git_dirty, unified_diff};
    let paths = if paths.is_empty() { vec![PathBuf::from(".")] } else { paths.to_vec() };
    let h = Htl::new()?;
    let cfg = load_config(&paths[0])?;
    let file_spec = cfg.as_ref().map(|(_, _, c)| c.lint_spec()).unwrap_or_default();
    if !file_spec.is_empty() {
        h.configure_lints(&file_spec)?;
    }
    if let Some(first) = paths.first() {
        auto_dts(first)?;
        apply_project(&h, first)?;
    }
    if let Some((root, _, c)) = &cfg {
        h.apply_config(root, c)?;
    }
    h.install_test_lib()?;
    let opts = FixOptions {
        unsafe_fixes: flags.unsafe_fixes,
        promoted: cfg.as_ref().map(|(_, _, c)| c.fix.unsafe_.clone()).unwrap_or_default(),
        disabled: cfg.as_ref().map(|(_, _, c)| c.fix.disable.clone()).unwrap_or_default(),
        only: flags.rule.clone(),
        dry_run: flags.dry_run,
    };
    let files = htl::collect_tl(&paths)?;

    // The working tree is the undo: refuse to rewrite what git could not give back.
    if !flags.dry_run {
        let mut dirty = Vec::new();
        let mut no_vcs = Vec::new();
        for f in &files {
            match git_dirty(f)? {
                Some(true) => dirty.push(f.clone()),
                Some(false) => {}
                None => no_vcs.push(f.clone()),
            }
        }
        if !dirty.is_empty() && !flags.allow_dirty {
            bail!(
                "htl fix rewrites files in place and relies on git to undo it; these have uncommitted changes:\n  {}\ncommit or stash them, or pass --allow-dirty",
                dirty.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n  ")
            );
        }
        if !no_vcs.is_empty() && !flags.allow_no_vcs {
            bail!(
                "htl fix rewrites files in place and relies on git to undo it; these are not in a git repository:\n  {}\npass --allow-no-vcs to proceed without that safety net",
                no_vcs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n  ")
            );
        }
    }

    let mut sink = report::Sink::new(flags.json);
    let (mut applied, mut skipped, mut json_files) = (Vec::new(), Vec::new(), Vec::new());
    let (mut changed, mut deferred, mut reverted, mut errors_remaining) = (0usize, 0usize, 0usize, 0usize);
    for f in &files {
        h.add_layout_paths(f)?;
        let before = if flags.diff { std::fs::read_to_string(f).ok() } else { None };
        let out = fix_file(&h, f, &opts)?;
        if out.contents.is_some() {
            changed += 1;
        }
        deferred += out.deferred;
        if out.reverted.is_some() {
            reverted += 1;
        }
        errors_remaining += out.check.errors.len();
        if !flags.json {
            for a in &out.applied {
                eprintln!(
                    "fixed: {}:{}: {} ({}{})",
                    f.display(),
                    a.line,
                    a.rule,
                    a.applicability.as_str(),
                    if flags.dry_run { ", not written" } else { "" }
                );
            }
            for s in &out.skipped {
                eprintln!("skipped: {}:{}: {} — {}", f.display(), s.line, s.rule, s.reason);
            }
            if let Some(r) = &out.reverted {
                eprintln!("reverted: {}: {r}", f.display());
            }
            if let Some(o) = &out.oscillation {
                eprintln!("stopped: {}: fixes of {o} undo each other", f.display());
            }
            if out.deferred > 0 {
                eprintln!("deferred: {}: {} edit(s) overlapped applied ones; run htl fix again", f.display(), out.deferred);
            }
            if flags.diff
                && let (Some(b), Some(a)) = (&before, &out.contents)
            {
                print!("{}", unified_diff(&f.display().to_string(), b, a));
            }
        }
        sink.checkinfo(&out.check);
        if flags.json {
            applied.extend(out.applied.iter().map(|a| report::FixApplied {
                file: f.display().to_string(),
                line: a.line,
                rule: a.rule.clone(),
                applicability: a.applicability.as_str(),
                pass: a.pass,
            }));
            skipped.extend(out.skipped.iter().map(|s| report::FixSkipped {
                file: f.display().to_string(),
                line: s.line,
                rule: s.rule.clone(),
                reason: s.reason.clone(),
            }));
            json_files.push(report::FixFile {
                path: f.display().to_string(),
                changed: out.contents.is_some(),
                deferred: out.deferred,
                reverted: out.reverted.clone(),
                oscillation: out.oscillation.clone(),
                diagnostics: sink.take(),
            });
        }
    }
    let n_applied = if flags.json { applied.len() } else { 0 };
    let fail = errors_remaining > 0 || (flags.exit_non_zero_on_fix && changed > 0);
    if flags.json {
        let n_skipped = skipped.len();
        report::emit(&report::FixReport {
            dry_run: flags.dry_run,
            applied,
            skipped,
            files: json_files,
            summary: report::FixSummary {
                files: files.len(),
                files_changed: changed,
                applied: n_applied,
                skipped: n_skipped,
                deferred,
                reverted,
                errors_remaining,
                ok: !fail,
            },
        })?;
    } else {
        eprintln!(
            "htl fix: {} file(s), {} changed{}, {} error(s) remaining{}",
            files.len(),
            changed,
            if flags.dry_run { " (dry run, nothing written)" } else { "" },
            errors_remaining,
            if deferred > 0 { format!(", {deferred} deferred") } else { String::new() }
        );
    }
    Ok(if fail { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

/// Type-check a tree, replaying the modules whose inputs have not moved.
///
/// The order here is load-bearing. Generating `.d.tl` and collecting the file list come
/// first, because both produce inputs the keys are built from. Every module is looked up
/// before any is checked, so that a run where nothing moved never builds a checker at all —
/// that costs about 13.5 ms (`crates/htl-core/benches/check.rs`), which is most of what a
/// fully replayed run has to save on a small project.
///
/// A module that misses re-checks its dependencies as part of its own check, since the
/// checker's store starts empty. So an edit costs the edited module, whatever depends on
/// it, and whatever those pull in — less than the project, more than the minimum.
fn cmd_check(
    paths: &[PathBuf],
    strict: bool,
    lint: Option<&str>,
    list_lints: bool,
    json: bool,
    use_cache: bool,
    cache_mode: Option<cache::Mode>,
) -> Result<ExitCode> {
    let mut sink = report::Sink::new(json);
    let paths = if paths.is_empty() { vec![PathBuf::from(".")] } else { paths.to_vec() };
    // Listing the rules reports nothing about the project and shares nothing with a run.
    if list_lints {
        let h = Htl::new()?;
        for r in h.lint_rules()? {
            println!("{r}");
        }
        return Ok(ExitCode::SUCCESS);
    }
    // htl.toml first, then --lint, so the flag wins; `strict` from the file unless flagged.
    let cfg = load_config(&paths[0])?;
    let strict = strict || cfg.as_ref().and_then(|(_, _, c)| c.lint.strict).unwrap_or(false);
    // `.d.tl` written from Rust source is an input to the check, so it is regenerated
    // before anything hashes the tree: a hit computed over stale declarations would be a
    // hit on a different question from the one being asked.
    if let Some(first) = paths.first() {
        auto_dts(first)?;
    }
    let files = htl::collect_tl(&paths)?;

    // The store lives at the project root, so invocations from different directories in
    // one project share it; what separates them is the key, which carries the working
    // directory and each path as written.
    let root = cfg
        .as_ref()
        .map(|(r, _, _)| r.clone())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    // The flag wins over the config, and the config over the default. Whether to cache at
    // all is the separate `--no-cache`.
    let mode = cache_mode
        .or_else(|| {
            cfg.as_ref().and_then(|(_, _, c)| c.cache.mode.as_deref()).map(|m| {
                cache::Mode::parse(m)
                    .unwrap_or_else(|| {
                        eprintln!("htl check: unknown [cache] mode {m:?}, using per-module");
                        cache::Mode::PerModule
                    })
            })
        })
        .unwrap_or_default();
    let store = cache::Cache::open(&root, use_cache, mode);

    // The lint selection is part of what a module reports, so it is part of every key.
    let file_spec = cfg.as_ref().map(|(_, _, c)| c.lint_spec()).unwrap_or_default();
    let spec = htl::config::join_specs([file_spec.as_str(), lint.unwrap_or("")]);

    // Look every module up before checking any of them, so that a run where nothing moved
    // never builds a checker at all.
    let keys: Vec<cache::Key> = files.iter().map(|f| cache::module_key(f, Some(&spec))).collect();
    let run_key = cache::run_key(&files, Some(&spec));
    let hits: Vec<Option<cache::Module>> = match &store {
        Some(c) => c.lookup_all(&keys, &run_key, files.len()),
        None => vec![None; files.len()],
    };
    let to_check = hits.iter().filter(|h| h.is_none()).count();

    let h = if to_check > 0 { Some(build_checker(&cfg, &paths, &spec)?) } else { None };
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
                let m = check_one(h, &mut sink, f, &cfg)?;
                // Per-module entries are written as each one is checked; a whole-run entry
                // cannot be written until the walk is done, so it happens below.
                if let Some(c) = &store
                    && c.mode() == cache::Mode::PerModule
                {
                    c.store_module(key, f, &cfg_inputs, &search_dirs(f, &root, &cfg), &m);
                }
                m
            }
        };
        n_err += m.errors;
        n_warn += m.warnings;
        n_lint += m.lints;
        infos.push((f.clone(), requires_only(&m)));
        modules.push(m);
    }
    // One entry for the walk. Nothing to write when everything replayed: the entry that was
    // read is the entry that would be written.
    if let Some(c) = &store
        && c.mode() == cache::Mode::WholeRun
        && to_check > 0
    {
        let dirs: Vec<PathBuf> = files.iter().flat_map(|f| search_dirs(f, &root, &cfg)).collect();
        c.store_run(&run_key, &files, &cfg_inputs, &dirs, &modules);
    }
    // Project-level: cycles in the require graph of the files just checked.
    for cyc in htl::require_cycles(&infos) {
        sink.diag("lint", &cyc);
        n_lint += 1;
    }
    // A contract the host never enforces is documentation, not a guarantee.
    if let Some((_, cfg_path, cfg)) = &cfg {
        let cargo_root = htl::dts::find_cargo_package_root(&paths[0]);
        for l in htl::contract_enforcement_lints(cfg, cfg_path, cargo_root.as_deref()) {
            sink.diag("lint", &l);
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

    let replayed = files.len() - to_check;
    let fail = report_check(&mut sink, json, files.len(), (n_err, n_warn, n_lint), strict, replayed)?;
    Ok(if fail { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

/// Delete this project's cache store.
///
/// The store sits beside `htl.toml`, which is not necessarily where anyone is standing:
/// `htl check src` run from anywhere in a repo writes to the project root. That is the
/// reason this exists rather than leaving people to work out which directory to remove.
///
/// Only `.htl/cache` goes. `.htl/` itself is left in place for whatever else may come to
/// live there, and the path is built here rather than taken from an argument, so there is
/// no spelling of the command that removes anything else.
fn cmd_cache_clear(path: Option<&Path>) -> Result<ExitCode> {
    let start = path.unwrap_or(Path::new("."));
    let root = load_config(start)?
        .map(|(r, _, _)| r)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let dir = root.join(".htl").join("cache");
    if !dir.is_dir() {
        eprintln!("htl cache: nothing stored at {}", dir.display());
        return Ok(ExitCode::SUCCESS);
    }
    let n = fs::read_dir(&dir)
        .map(|rd| rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "json")).count())
        .unwrap_or(0);
    fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
    eprintln!("htl cache: removed {n} {} from {}", if n == 1 { "entry" } else { "entries" }, dir.display());
    Ok(ExitCode::SUCCESS)
}

/// A checker set up the way a check of these paths needs it: the lint selection, the
/// package project, the config's search paths, and the test library.
///
/// Only built when something actually has to be checked — a run that replays every module
/// should not pay the ~13.5 ms this costs.
fn build_checker(
    cfg: &Option<(PathBuf, PathBuf, htl::config::HtlConfig)>,
    paths: &[PathBuf],
    spec: &str,
) -> Result<Htl> {
    let h = Htl::new()?;
    if !spec.is_empty() {
        h.configure_lints(spec)?;
    }
    if let Some(first) = paths.first() {
        apply_project(&h, first)?;
    }
    if let Some((root, _, c)) = cfg {
        h.apply_config(root, c)?;
    }
    // `*_test.tl` under the checked tree require("htl.test"): make its types visible.
    h.install_test_lib()?;
    Ok(h)
}

/// Check one file and collect everything it reported, its contract lints included.
fn check_one(
    h: &Htl,
    sink: &mut report::Sink,
    f: &Path,
    cfg: &Option<(PathBuf, PathBuf, htl::config::HtlConfig)>,
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
    // `[[contract]]`: static expect_type / require_fields for files under each dir.
    if let Some((root, _, cfg)) = cfg
        && c.ok()
    {
        for l in htl::contract_lints(h, root, cfg, f)? {
            sink.diag("lint", &l);
            lints += 1;
        }
    }
    h.set_search_path(&saved)?;
    Ok(cache::Module {
        // Everything this file put into the sink, and nothing from the files before it:
        // the previous iteration took its own.
        diagnostics: sink.take_recorded(),
        errors: c.errors.len(),
        warnings: c.warnings.len(),
        lints,
        deps: c.deps.iter().map(|p| cache::normal(p)).collect(),
        requires: c
            .requires
            .iter()
            .map(|r| cache::RequireJson {
                module: r.module.clone(),
                path: r.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
                line: r.line,
                col: r.col,
            })
            .collect(),
    })
}

/// A `CheckInfo` carrying only what the project-level lints read.
///
/// `require_cycles` runs over every file in the walk, replayed ones included — a cycle that
/// closes through a module nobody edited is still a cycle — so a replayed module has to
/// produce something that lint can read. Its diagnostics are already printed by then, and
/// nothing downstream looks at the other fields.
fn requires_only(m: &cache::Module) -> CheckInfo {
    CheckInfo {
        deps: m.deps.iter().map(PathBuf::from).collect(),
        requires: m
            .requires
            .iter()
            .map(|r| htl::RequireSite {
                module: r.module.clone(),
                path: r.path.as_ref().map(PathBuf::from),
                line: r.line,
                col: r.col,
            })
            .collect(),
        ..Default::default()
    }
}

/// Directories a `require` could resolve in, listed whether or not they exist yet.
///
/// The ones that do not exist matter most: a `types/` created after an entry was written
/// changes what a module name resolves to while every file the entry recorded still hashes
/// the same. Recording only the directories that happened to exist is the hole ccache
/// documents in its direct mode, and an empty directory hashes differently from one holding
/// a module, so listing it now is what closes it.
fn search_dirs(
    file: &Path,
    root: &Path,
    cfg: &Option<(PathBuf, PathBuf, htl::config::HtlConfig)>,
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = file.parent().map(Path::to_path_buf).into_iter().collect();
    out.push(root.to_path_buf());
    out.push(root.join("src"));
    out.push(root.join("types"));
    if let Some((r, _, c)) = cfg {
        out.extend(c.search_paths(r));
    }
    out
}

/// The one place a check reports its totals, so a replayed module and a checked one cannot
/// drift apart in how they are summarized. Returns whether the run counts as a failure.
fn report_check(
    sink: &mut report::Sink,
    json: bool,
    files: usize,
    counts: (usize, usize, usize),
    strict: bool,
    replayed: usize,
) -> Result<bool> {
    let (errors, warnings, lints) = counts;
    let fail = errors > 0 || (strict && (warnings > 0 || lints > 0));
    let all_cached = replayed == files && files > 0;
    if json {
        report::emit(&report::CheckReport {
            files,
            diagnostics: sink.take(),
            summary: report::CheckSummary {
                errors,
                warnings,
                lints,
                strict,
                ok: !fail,
                cached: all_cached,
                replayed,
            },
        })?;
    } else {
        // Say nothing when nothing was replayed; say how much when it was only some.
        let cached = if all_cached {
            " [cached]".to_string()
        } else if replayed > 0 {
            format!(" [{replayed}/{files} cached]")
        } else {
            String::new()
        };
        eprintln!(
            "htl check: {files} file(s), {errors} error(s), {warnings} warning(s), {lints} lint(s){}{cached}",
            if strict { " [strict]" } else { "" }
        );
    }
    Ok(fail)
}

fn cmd_gen(file: &Path, out: Option<&Path>) -> Result<ExitCode> {
    let h = Htl::new()?;
    h.add_layout_paths(file)?;
    auto_dts(file)?;
    apply_project(&h, file)?;
    let (code, c) = h.gen_lua(file)?;
    print_checkinfo(&c);
    let Some(mut code) = code else { return Ok(ExitCode::FAILURE) };
    if !code.ends_with('\n') {
        code.push('\n');
    }
    match out {
        Some(p) => fs::write(p, code).with_context(|| format!("writing {}", p.display()))?,
        None => print!("{code}"),
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_run(file: &Path, args: &[String]) -> Result<ExitCode> {
    let bytes = fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let h = Htl::new()?;
    auto_dts(file)?;
    apply_project(&h, file)?;
    h.install_test_lib()?;
    if Bundle::is_bundle(&bytes) {
        let b = Bundle::decode(&bytes)?;
        return Ok(match h.run_bundle(&b, args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{}", htl::user_message(&e));
                ExitCode::FAILURE
            }
        });
    }
    // Check first so lints/warnings are visible before the script runs.
    h.add_layout_paths(file)?;
    h.install_searcher()?;
    h.set_arg(&file.to_string_lossy(), args)?;
    let (code, c) = h.gen_lua(file)?;
    print_checkinfo(&c);
    let Some(code) = code else { return Ok(ExitCode::FAILURE) };
    match h.exec(&code, &format!("@{}", file.display()), args) {
        Ok(()) => Ok(ExitCode::SUCCESS),
        Err(e) => {
            // Innermost cause, no Lua traceback (`--trace` would be the place to show it).
            eprintln!("{}", htl::user_message(&e));
            Ok(ExitCode::FAILURE)
        }
    }
}

fn cmd_build(entry: &Path, out: &Path, main: &str, mut opts: htl::link::LinkOptions) -> Result<ExitCode> {
    let h = Htl::new()?;
    auto_dts(entry)?;
    apply_project(&h, entry)?;
    if let Some((root, _, cfg)) = load_config(entry)? {
        h.apply_config(&root, &cfg)?;
        opts.extra.extend(cfg.build.extra.iter().cloned());
        opts.host.extend(cfg.build.host.iter().cloned());
    }
    if entry.is_dir() {
        return cmd_build_dir(&h, entry, out, main, &opts);
    }
    h.add_layout_paths(entry)?;
    let linked = htl::link::link(&h, entry, &opts)?;
    for (_, c) in &linked.checks {
        print_checkinfo(c);
    }
    let n_err = linked.errors.len();
    for e in linked.errors.iter().filter(|e| e.contains("is not on the search path")) {
        eprintln!("error: {e}");
    }
    if n_err > 0 {
        eprintln!("htl build: {n_err} error(s), bundle not written");
        return Ok(ExitCode::FAILURE);
    }
    let buf = linked.bundle()?.encode();
    fs::write(out, &buf).with_context(|| format!("writing {}", out.display()))?;
    let typed = linked.modules.iter().filter(|m| m.typed).count();
    eprintln!(
        "htl build: {} module(s) ({typed} typed, {} lua) -> {} ({} bytes{}{})",
        linked.modules.len(),
        linked.modules.len() - typed,
        out.display(),
        buf.len(),
        if opts.source { ", source" } else { ", bytecode" },
        if opts.debug { " with debug info" } else { "" }
    );
    if !linked.host_modules.is_empty() {
        eprintln!("htl build: host must provide: {}", linked.host_modules.join(", "));
    }
    Ok(ExitCode::SUCCESS)
}

/// The older form: every `.tl` under a directory, module names from their paths.
fn cmd_build_dir(h: &Htl, dir: &Path, out: &Path, entry: &str, opts: &htl::link::LinkOptions) -> Result<ExitCode> {
    h.add_path(dir)?;
    let files = htl::collect_tl(&[dir.to_path_buf()])?;
    let mut b = Bundle { entry: entry.to_string(), htl_version: env!("CARGO_PKG_VERSION").into(), ..Default::default() };
    let mut n_err = 0usize;
    for f in &files {
        let name = htl::module_name(dir, f)?;
        let (code, c) = h.gen_lua(f)?;
        print_checkinfo(&c);
        n_err += c.errors.len();
        let Some(code) = code else { continue };
        let (kind, payload) = if opts.source {
            (htl::bundle::Kind::Source, code.into_bytes())
        } else {
            (htl::bundle::Kind::Bytecode, h.compile_with(&name, &code, !opts.debug)?)
        };
        b.modules.push(htl::bundle::Module { name, kind, payload });
    }
    if n_err > 0 {
        eprintln!("htl build: {n_err} error(s), bundle not written");
        return Ok(ExitCode::FAILURE);
    }
    if b.module(entry).is_none() {
        bail!("entry module '{entry}' not found among {} module(s)", b.modules.len());
    }
    if !opts.source {
        b.fingerprint = h.fingerprint()?;
    }
    let buf = b.encode();
    fs::write(out, &buf).with_context(|| format!("writing {}", out.display()))?;
    eprintln!("htl build: {} module(s) -> {} ({} bytes)", b.modules.len(), out.display(), buf.len());
    Ok(ExitCode::SUCCESS)
}
