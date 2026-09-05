//! htl CLI. Also built as `cargo-htl` so `cargo htl <verb>` works.
//!
//! - `htl check <paths...>`   type-check files or directories
//! - `htl gen <file.tl>`      emit readable Lua (escape hatch)
//! - `htl run <file.tl|.hb>`  type-check then execute (strict: type errors abort)
//! - `htl build <dir>`        compile a tree of `.tl` into one stripped-bytecode bundle

mod scaffold;

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
        dir: PathBuf,
        #[arg(short, long, default_value = "app.hb")]
        out: PathBuf,
        #[arg(short, long, default_value = "main")]
        entry: String,
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
        Cmd::Check { paths, strict, lint, list_lints } => cmd_check(&paths, strict, lint.as_deref(), list_lints),
        Cmd::Fmt { paths, check, indent } => cmd_fmt(&paths, check, indent),
        Cmd::Test { paths, filter, lib, lint } => cmd_test(&paths, filter.as_deref(), &lib, lint.as_deref()),
        Cmd::Pkg { args } => cmd_pkg(&args),
        Cmd::Dts { dir } => cmd_dts(dir.as_deref()),
        Cmd::New { name, lib, embed } => cmd_new(&name, lib, embed),
        Cmd::Init { dir, lib, embed } => cmd_init(dir.as_deref(), lib, embed),
        Cmd::Gen { file, out } => cmd_gen(&file, out.as_deref()),
        Cmd::Run { file, args } => cmd_run(&file, &args),
        Cmd::Build { dir, out, entry } => cmd_build(&dir, &out, &entry),
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

fn cmd_test(paths: &[PathBuf], filter: Option<&str>, lib: &str, lint: Option<&str>) -> Result<ExitCode> {
    let paths = if paths.is_empty() { vec![PathBuf::from(".")] } else { paths.to_vec() };
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
    let (mut passed, mut failed, mut bad_files) = (0usize, 0usize, 0usize);
    for f in &files {
        let rep = htl::testing::run_test_file(f, filter, lib, lint)?;
        print_checkinfo(&rep.check);
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
        eprintln!("{tag} {}  ({detail})", f.display());
        for m in &rep.failures {
            eprintln!("      - {m}");
        }
        passed += rep.passed;
        failed += rep.failed;
        if !rep.ok() {
            bad_files += 1;
        }
    }
    eprintln!(
        "htl test: {} file(s), {} passed, {} failed, {} file(s) with errors",
        files.len(),
        passed,
        failed,
        bad_files
    );
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

fn cmd_check(paths: &[PathBuf], strict: bool, lint: Option<&str>, list_lints: bool) -> Result<ExitCode> {
    let paths = if paths.is_empty() { vec![PathBuf::from(".")] } else { paths.to_vec() };
    let h = Htl::new()?;
    if list_lints {
        for r in h.lint_rules()? {
            println!("{r}");
        }
        return Ok(ExitCode::SUCCESS);
    }
    // htl.toml first, then --lint, so the flag wins; `strict` from the file unless flagged.
    let cfg = load_config(&paths[0])?;
    let file_spec = cfg.as_ref().map(|(_, _, c)| c.lint_spec()).unwrap_or_default();
    let spec = htl::config::join_specs([file_spec.as_str(), lint.unwrap_or("")]);
    if !spec.is_empty() {
        h.configure_lints(&spec)?;
    }
    let strict = strict || cfg.as_ref().and_then(|(_, _, c)| c.lint.strict).unwrap_or(false);
    if let Some(first) = paths.first() {
        auto_dts(first)?;
        apply_project(&h, first)?;
    }
    if let Some((root, _, c)) = &cfg {
        h.apply_config(root, c)?;
    }
    // `*_test.tl` under the checked tree require("htl.test"): make its types visible.
    h.install_test_lib()?;
    let files = htl::collect_tl(&paths)?;
    let (mut n_err, mut n_warn, mut n_lint) = (0usize, 0usize, 0usize);
    let mut infos: Vec<(PathBuf, CheckInfo)> = Vec::with_capacity(files.len());
    for f in &files {
        h.add_layout_paths(f)?;
        let c = h.check(f)?;
        print_checkinfo(&c);
        n_err += c.errors.len();
        n_warn += c.warnings.len();
        n_lint += c.lints.len();
        // `[[contract]]`: static expect_type / require_fields for files under each dir.
        if let Some((root, _, cfg)) = &cfg
            && c.ok()
        {
            for l in htl::contract_lints(&h, root, cfg, f)? {
                eprintln!("lint: {l}");
                n_lint += 1;
            }
        }
        infos.push((f.clone(), c));
    }
    // Project-level: cycles in the require graph of the files just checked.
    for cyc in htl::require_cycles(&infos) {
        eprintln!("lint: {cyc}");
        n_lint += 1;
    }
    // A contract the host never enforces is documentation, not a guarantee.
    if let Some((_, cfg_path, cfg)) = &cfg {
        let cargo_root = htl::dts::find_cargo_package_root(&paths[0]);
        for l in htl::contract_enforcement_lints(cfg, cfg_path, cargo_root.as_deref()) {
            eprintln!("lint: {l}");
            n_lint += 1;
        }
    }
    eprintln!(
        "htl check: {} file(s), {} error(s), {} warning(s), {} lint(s){}",
        files.len(),
        n_err,
        n_warn,
        n_lint,
        if strict { " [strict]" } else { "" }
    );
    let fail = n_err > 0 || (strict && (n_warn > 0 || n_lint > 0));
    Ok(if fail { ExitCode::FAILURE } else { ExitCode::SUCCESS })
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

fn cmd_build(dir: &Path, out: &Path, entry: &str) -> Result<ExitCode> {
    let h = Htl::new()?;
    h.add_path(dir)?;
    auto_dts(dir)?;
    apply_project(&h, dir)?;
    let files = htl::collect_tl(&[dir.to_path_buf()])?;
    let mut b = Bundle { entry: entry.to_string(), modules: Vec::new() };
    let mut n_err = 0usize;
    for f in &files {
        let name = htl::module_name(dir, f)?;
        let (code, c) = h.gen_lua(f)?;
        print_checkinfo(&c);
        n_err += c.errors.len();
        let Some(code) = code else { continue };
        b.modules.push((name.clone(), h.compile(&name, &code)?));
    }
    if n_err > 0 {
        eprintln!("htl build: {n_err} error(s), bundle not written");
        return Ok(ExitCode::FAILURE);
    }
    if !b.modules.iter().any(|(n, _)| n == entry) {
        bail!("entry module '{entry}' not found among {} module(s)", b.modules.len());
    }
    let buf = b.encode();
    fs::write(out, &buf).with_context(|| format!("writing {}", out.display()))?;
    eprintln!("htl build: {} module(s) -> {} ({} bytes)", b.modules.len(), out.display(), buf.len());
    Ok(ExitCode::SUCCESS)
}
