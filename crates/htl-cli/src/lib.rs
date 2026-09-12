//! htl CLI. Also built as `cargo-htl` so `cargo htl <verb>` works.
//!
//! - `htl check <paths...>`   type-check files or directories
//! - `htl gen <file.tl>`      emit readable Lua (escape hatch)
//! - `htl run <file.tl|.hb>`  type-check then execute (strict: type errors abort)
//! - `htl build <dir>`        compile a tree of `.tl` into one stripped-bytecode bundle
//! - `htl bundle info <.hb>`  print what a bundle records, without running it

use htl::cache;
mod junit;
mod report;
mod scaffold;

/// Output format of every command that has `--format`. One enum, so its help must be
/// true of all of them: `check` / `test` / `fix` put their text form on stderr (README,
/// "Machine-readable output" says why), `cache status` / `bundle info` / `resolve` are
/// reports and put it on stdout. Which stream is the README's to say, not this help's.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
enum Format {
    /// Human-readable lines
    Text,
    /// One JSON document on stdout (README, "Machine-readable output":
    /// https://github.com/ynishi/htl#machine-readable-output)
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
enum TypesCmd {
    /// Copy a library's declarations from teal-types into `types/`, and record which
    /// commit they came from
    Add {
        /// The library as teal-types names it (`luasocket`, `lpeg`, …)
        library: String,
        /// Take them from a checkout of teal-types already on disk instead of fetching
        #[arg(long)]
        from: Option<PathBuf>,
        /// Replace declarations that are already in `types/`
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum PkgCmd {
    /// Fetch every dependency `mlua-pkg.toml` declares and write `mlua-pkg.lock`
    Install,
    /// Write a dependency into `mlua-pkg.toml`; `install` is what fetches it
    Add {
        /// The name `require()` will use for it
        name: String,
        /// Remote git URL
        git: String,
        /// Pin to a tag: exact (`v1.0.0`), or a prefix that follows its patches (`v1.0`)
        #[arg(long)]
        tag: Option<String>,
        /// Pin to a commit
        #[arg(long)]
        rev: Option<String>,
        /// Track a branch (a build from it is not reproducible)
        #[arg(long)]
        branch: Option<String>,
        /// The subdirectory `require()` resolves through, when it is not the one the
        /// package declares
        #[arg(long)]
        entry: Option<PathBuf>,
        /// Copy the package into this directory instead of linking it (manifest-relative,
        /// rewritten by every install)
        #[arg(long)]
        target_dir: Option<PathBuf>,
    },
    /// Refresh dependencies, bump the pins that follow releases, then install
    Update {
        /// One dependency; every one when omitted
        name: Option<String>,
        /// Print the plan and write nothing
        #[arg(long)]
        dry_run: bool,
        /// Bump exact tag pins too, to the highest release the remote has
        #[arg(long)]
        force: bool,
    },
    /// Remove cached packages the lockfile no longer refers to
    Clean {
        /// Remove the whole cache rather than the unreferenced entries
        #[arg(long)]
        all: bool,
    },
    /// Take a dependency's source into `patches/<dep>/`: a copy the project owns, edits
    /// and commits, which install resolves the dependency from
    Patch {
        /// The dependency as `mlua-pkg.toml` names it
        dep: String,
        /// Refresh the copy even when it has uncommitted changes (they are discarded)
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum CacheCmd {
    /// Delete this project's stored check results
    Clear {
        /// A path inside the project; the store is found beside its htl.toml
        path: Option<PathBuf>,
    },
    /// Report what the store holds
    Status {
        /// A path inside the project; the store is found beside its htl.toml
        path: Option<PathBuf>,
        /// Output format
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
        /// List every entry rather than counting them by kind
        #[arg(long)]
        entries: bool,
    },
}

#[derive(Subcommand)]
enum BundleCmd {
    /// Print what a bundle records: format, the htl that built it, payload kind, the Lua
    /// its bytecode was compiled for, entry, modules, host-provided names
    Info {
        /// The `.hb` file
        file: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
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
use htl::BuildTarget;
use htl::Htl;
use htl::bundle::Bundle;
// The project layer: which files a check walks, what it replays from the run cache and
// what it keeps there, and which of a run's findings are said. Shared with `include_tl!`,
// which asks the same of the same store.
use htl::project;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "htl",
    version,
    about = "Teal, hidden: run / check / build .tl on mlua",
    long_about = "\
Teal, hidden: run / check / build .tl on mlua.

htl is a toolchain for Teal, and a way to embed one. It type-checks `.tl` sources with \
Teal's own checker, runs and tests them on an embedded mlua state, formats them, and \
links them into a single stripped-bytecode bundle. A Rust host embeds the same engine, \
so the scripts it ships are checked when the host is built rather than when its user \
first runs one.

The guide, with every command and what a project looks like:
  https://github.com/ynishi/htl

The API a Rust host holds — `include_tl!`, `#[host_module]`, the run cache:
  https://docs.rs/htl"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Type-check .tl files or directories (htl lints are reported as `lint:`)
    ///
    /// A module reached through `require` is checked with the file that required it, and
    /// what has not changed is replayed from `.htl/`. README, "Lints":
    /// https://github.com/ynishi/htl#lints-htl-check-include_tl
    #[command(after_long_help = "\
Examples:
  htl check src                  every .tl under src/
  htl check src --strict         every warn counts as deny: warnings and lints fail
  htl check src --lint nil-index=deny,no-any=warn
                                 one rule fails the run, another is advice
  htl check src --lint -tl:hint  a warning kind of the Teal compiler silenced
  htl check --list-lints         every rule with its default level, then exit

Caching: https://github.com/ynishi/htl#caching
")]
    Check {
        paths: Vec<PathBuf>,
        /// Treat every `warn` as `deny`: warnings and lints fail the run
        #[arg(long)]
        strict: bool,
        /// Rule levels over the defaults, e.g. `nil-index=deny,no-any=warn` (`-rule` =
        /// allow, `+rule` = warn)
        // A spec that silences a rule starts with `-`, and without this clap reads
        // `--lint -contract` as a cluster of short flags and answers `unexpected argument
        // '-c'`: the documented way to turn one rule off never reached the spec parser.
        #[arg(long, allow_hyphen_values = true)]
        lint: Option<String>,
        /// List every lint rule with its default level and exit
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
        /// Say why the cache was not used, and what this run did with it
        #[arg(long)]
        explain_cache: bool,
    },
    /// Run tests: `*_test.tl` and `tests/**/*.tl`, one isolated state per file
    ///
    /// README, "Tests": https://github.com/ynishi/htl#tests
    #[command(after_long_help = "\
Examples:
  htl test                       every *_test.tl and tests/**/*.tl
  htl test tests --filter parser only tests whose \"suite > name\" contains it
  htl test --coverage --coverage-lines
                                 which lines of each module the suite never reached
  htl test --junit report.xml    also write the run as a JUnit report, for a CI
")]
    Test {
        paths: Vec<PathBuf>,
        /// Only run tests whose "suite > name" contains this substring
        #[arg(long)]
        filter: Option<String>,
        /// Assertion library module consulted for the verdict (must expose `run(filter)`)
        #[arg(long, default_value = htl::testing::DEFAULT_LIB)]
        lib: String,
        /// Lint rules on top of the defaults, e.g. `+no-any`
        #[arg(long, allow_hyphen_values = true)]
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
        /// Also write the coverage as an lcov tracefile here (implies --coverage)
        #[arg(long, value_name = "FILE")]
        lcov: Option<PathBuf>,
        /// Also write the run as a JUnit XML report here, for a CI that reads test results
        #[arg(long, value_name = "FILE")]
        junit: Option<PathBuf>,
        /// Seed the random stream tests draw from; printed every run, so a failure repeats
        #[arg(long)]
        seed: Option<u64>,
        /// Output format
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
        /// Check and generate every file even if a cached form is available, and store none
        #[arg(long)]
        no_cache: bool,
        /// Say why the cache was not used, and what this run did with it
        #[arg(long)]
        explain_cache: bool,
    },
    /// Apply the fixes diagnostics carry (safe ones by default)
    ///
    /// README, "Fixing": https://github.com/ynishi/htl#fixing-htl-fix
    #[command(after_long_help = "\
Examples:
  htl fix src                    apply the safe fixes to every .tl under src/
  htl fix src --diff --dry-run   what would change, as a unified diff per file
  htl fix src --rule forward-ref only that rule's fixes
  htl fix src --unsafe           also the fixes that may change what the program does
")]
    Fix {
        paths: Vec<PathBuf>,
        /// Only fixes of these rules: any rule `htl check --list-lints` names, or
        /// `forward-ref` / `tl:error`, the classes an error's fix is filed under
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
    /// Write the `.d.tl` files this project declares: those `#[host_module(dts = ..)]` /
    /// `#[teal(dts = ..)]` ask for in a Rust crate, without building it, and the module
    /// each `---@contract` type is declared in (check / run / test / build do both)
    ///
    /// Each declaration is reported as `wrote`, `unchanged`, or `not written`. The exit
    /// code is about that last one and nothing else: non-zero when a declaration this
    /// command was asked to write could not be written. A file already under `types/`
    /// that no dependency ships any more is reported as `left in place` — nothing was
    /// asked for, nothing is deleted, and the exit code does not move.
    Dts {
        /// Crate root or any path inside it (default: current directory)
        dir: Option<PathBuf>,
    },
    /// Create a new Teal project directory
    ///
    /// README, "Layout of a project": https://github.com/ynishi/htl#layout-of-a-project-htl-new
    #[command(after_long_help = "\
Examples:
  htl new hello                    src/main.tl, a module, tests/, mlua-pkg.toml
  htl new hello --lib              a library: no entry script
  htl new hello --embed            the same, plus a Rust host (--target bin)
  htl new hello --target cdylib --lib
                                   a library behind a C ABI, for a caller that is not Rust
  htl new hello --embed --htl main
                                   built against the unreleased htl (a git pin on main)
  htl new hello --embed --htl path:../htl
                                   built against a local checkout of this repository
  htl new hello --no-x             without the htlx dependency the manifest gets by default

Build targets: https://github.com/ynishi/htl#build-targets---target-name
The cdylib target: https://github.com/ynishi/htl#the-cdylib-target---target-cdylib
")]
    New {
        name: String,
        /// Library only (no src/main.tl)
        #[arg(long)]
        lib: bool,
        /// Also emit a Rust host: shorthand for --target bin
        #[arg(long)]
        embed: bool,
        /// What will run this project's output: a target writes Cargo.toml + src/lib.rs
        /// (the #[host_module], the embedded module, preload) and, when there is an entry
        /// script, a thin src/main.rs
        #[arg(long, value_name = "NAME", value_parser = clap::builder::PossibleValuesParser::new(scaffold::target_names()))]
        target: Option<String>,
        /// The htl the project depends on: a supported release (0.4), `main`, or
        /// `path:<checkout>`
        #[arg(long, value_name = "REQ", default_value = scaffold::DEFAULT_HTL)]
        htl: String,
        /// Leave the htlx (htl-x collections) dependency out of mlua-pkg.toml
        #[arg(long)]
        no_x: bool,
    },
    /// Fill in the scaffold files that are missing in an existing directory
    Init {
        dir: Option<PathBuf>,
        #[arg(long)]
        lib: bool,
        /// Shorthand for --target bin
        #[arg(long)]
        embed: bool,
        /// Fill in this target's files, and report the ones that were already there
        #[arg(long, value_name = "NAME", value_parser = clap::builder::PossibleValuesParser::new(scaffold::target_names()))]
        target: Option<String>,
        /// The htl the project depends on: a supported release (0.4), `main`, or
        /// `path:<checkout>`
        #[arg(long, value_name = "REQ", default_value = scaffold::DEFAULT_HTL)]
        htl: String,
        /// Leave the htlx (htl-x collections) dependency out of mlua-pkg.toml
        #[arg(long)]
        no_x: bool,
    },
    /// Package management at the nearest `mlua-pkg.toml` project root: install / add /
    /// update / clean / patch, through mlua-pkg's library rather than its binary
    Pkg {
        #[command(subcommand)]
        cmd: PkgCmd,
    },
    /// Bring declarations for a library into `types/`
    Types {
        #[command(subcommand)]
        cmd: TypesCmd,
    },
    /// Manage the check cache
    ///
    /// README, "Caching": https://github.com/ynishi/htl#caching
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
    ///
    /// A bundle is the `hb` target, so this refuses a project whose `[build] target` is
    /// another one — a `bin` or a `cdylib` is built with `cargo build`.
    ///
    /// README, "Bundles": https://github.com/ynishi/htl#bundles-htl-build
    #[command(after_long_help = "\
Examples:
  htl build src/main.tl -o app.hb
                                 the entry's require closure, as one bundle
  htl build src/main.tl -o app.hb --debug
                                 keep line numbers and local names in the bytecode
  htl build src/main.tl -o app.hb --host mymod
                                 leave a module the host provides out of the bundle
  htl run app.hb                 run what was built

Caching: https://github.com/ynishi/htl#caching
")]
    Build {
        /// Entry `.tl` file: it and everything it requires are bundled, replaying from
        /// the run cache what still holds (a directory bundles every `.tl` under it, the
        /// older snapshot form, which is not cached)
        entry: PathBuf,
        #[arg(short, long, default_value = "app.hb")]
        out: PathBuf,
        /// Entry module name when `entry` is a directory
        #[arg(short, long, default_value = "main")]
        main: String,
        /// Keep debug info (line numbers, local names) in the bytecode
        #[arg(long)]
        debug: bool,
        /// Store generated Lua source instead of bytecode: for a big-endian target, a Lua
        /// with non-default number types, or a bundle that must outlive a Lua upgrade
        #[arg(long)]
        source: bool,
        /// Modules to bundle that only a dynamic require reaches (also `[build] extra`)
        #[arg(long, value_delimiter = ',')]
        extra: Vec<String>,
        /// Modules the host provides (also `[build] host`; `.d.tl`-only modules are implied)
        #[arg(long, value_delimiter = ',')]
        host: Vec<String>,
        /// Generate every module even if its cached form still holds, and do not store
        /// (the directory form is never cached)
        #[arg(long)]
        no_cache: bool,
        /// Say why the cache was not used, and what this run did with it
        #[arg(long)]
        explain_cache: bool,
    },
    /// Report what no entry's `require` closure reaches: modules, and `mlua-pkg.toml`
    /// dependencies
    ///
    /// README, "Unused": https://github.com/ynishi/htl#unused-htl-unused
    #[command(after_long_help = "\
Examples:
  htl unused                     modules and dependencies no entry reaches
  htl unused --exit-non-zero-on-unused
                                 the same report, and a non-zero exit for a CI
  htl unused --format json       the same report as one JSON document
")]
    Unused {
        /// What to report on (default: the working directory). The walk is the project's
        /// either way: a module reached only from `tests/` is reached
        paths: Vec<PathBuf>,
        /// Output format
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
        /// Exit 1 when anything is reported (for CI)
        #[arg(long)]
        exit_non_zero_on_unused: bool,
        /// Build the graph from a fresh check even if a cached one is available, and
        /// store none
        #[arg(long)]
        no_cache: bool,
        /// Say why the cache was not used, and what this run did with it
        #[arg(long)]
        explain_cache: bool,
    },
    /// Which file `require("<module>")` resolves to, and what that hides
    ///
    /// The whole chain in search order: what is read, what is reachable and not read, and
    /// which crate or dependency each one came from. An override on the search path is the
    /// mechanism working, and this is how to see it. README, "Which file a name resolves
    /// to": https://github.com/ynishi/htl#which-file-a-name-resolves-to-htl-resolve
    #[command(after_long_help = "\
Examples:
  htl resolve mq                 the chain for require(\"mq\")
  htl resolve socket.http        a name with dots, as a require spells it
  htl resolve mq --format json   the same rows as one JSON document

Exits 1 when the name resolves to nothing, so a script can ask.

`htl.test` is not on a project's search path: `htl test` preloads it into the state it
runs, and the declarations behind it come from the binary rather than the project.
")]
    Resolve {
        /// The module name a `require` would spell (`mq`, `socket.http`)
        module: String,
        /// A path inside the project (default: the working directory)
        path: Option<PathBuf>,
        /// Output format
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
    },
    /// Read a `.hb` bundle without running it
    ///
    /// README, "Bundles": https://github.com/ynishi/htl#bundles-htl-build
    Bundle {
        #[command(subcommand)]
        cmd: BundleCmd,
    },
}

/// The command line as clap holds it, before any argument is parsed.
///
/// The help is documentation, and the tests that keep it from rotting read it from here
/// rather than from a copy kept beside them: they walk this tree and ask each command,
/// argument and value for the strings it would print.
pub fn command() -> clap::Command {
    <Cli as clap::CommandFactory>::command()
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
        Cmd::Check {
            paths,
            strict,
            lint,
            list_lints,
            format,
            no_cache,
            cache_mode,
            explain_cache,
        } => cmd_check(
            &paths,
            lint.as_deref(),
            CheckFlags {
                strict,
                list_lints,
                json: format == Format::Json,
                use_cache: !no_cache,
                cache_mode: cache_mode.map(Into::into),
                explain: explain_cache,
            },
        ),
        Cmd::Fmt {
            paths,
            check,
            indent,
        } => cmd_fmt(&paths, check, indent),
        Cmd::Test {
            paths,
            filter,
            lib,
            lint,
            fail_fast,
            verbose,
            quiet,
            slow,
            update,
            coverage,
            coverage_lines,
            lcov,
            junit,
            seed,
            format,
            no_cache,
            explain_cache,
        } => cmd_test(
            &paths,
            filter.as_deref(),
            &lib,
            lint.as_deref(),
            TestFlags {
                fail_fast,
                verbose,
                quiet,
                slow,
                update,
                coverage: coverage || lcov.is_some(),
                coverage_lines,
                lcov,
                junit,
                json: format == Format::Json,
                no_cache,
                explain: explain_cache,
                seed,
            },
        ),
        Cmd::Pkg { cmd } => match cmd {
            PkgCmd::Install => cmd_pkg_install(),
            PkgCmd::Add {
                name,
                git,
                tag,
                rev,
                branch,
                entry,
                target_dir,
            } => cmd_pkg_add(htl::pkg::mlua_pkg::ops::AddSpec {
                name,
                git,
                tag,
                rev,
                branch,
                entry,
                target_dir,
            }),
            PkgCmd::Update {
                name,
                dry_run,
                force,
            } => cmd_pkg_update(htl::pkg::mlua_pkg::ops::UpdateOpts {
                name,
                dry_run,
                force,
            }),
            PkgCmd::Clean { all } => cmd_pkg_clean(all),
            PkgCmd::Patch { dep, force } => cmd_pkg_patch(&dep, force),
        },
        Cmd::Types { cmd } => match cmd {
            TypesCmd::Add {
                library,
                from,
                force,
            } => cmd_types_add(&library, from.as_deref(), force),
        },
        Cmd::Cache { cmd } => match cmd {
            CacheCmd::Clear { path } => cmd_cache_clear(path.as_deref()),
            CacheCmd::Status {
                path,
                format,
                entries,
            } => cmd_cache_status(path.as_deref(), format == Format::Json, entries),
        },
        Cmd::Dts { dir } => cmd_dts(dir.as_deref()),
        Cmd::New {
            name,
            lib,
            embed,
            target,
            htl,
            no_x,
        } => cmd_new(&name, lib, embed, target.as_deref(), &htl, no_x),
        Cmd::Init {
            dir,
            lib,
            embed,
            target,
            htl,
            no_x,
        } => cmd_init(dir.as_deref(), lib, embed, target.as_deref(), &htl, no_x),
        Cmd::Gen { file, out } => cmd_gen(&file, out.as_deref()),
        Cmd::Run { file, args } => cmd_run(&file, &args),
        Cmd::Fix {
            paths,
            rule,
            unsafe_fixes,
            dry_run,
            diff,
            allow_dirty,
            allow_no_vcs,
            exit_non_zero_on_fix,
            format,
        } => cmd_fix(
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
        Cmd::Build {
            entry,
            out,
            main,
            debug,
            source,
            extra,
            host,
            no_cache,
            explain_cache,
        } => cmd_build(
            &entry,
            &out,
            &main,
            htl::link::LinkOptions {
                debug,
                source,
                extra,
                host,
            },
            BuildCache {
                use_cache: !no_cache,
                explain: explain_cache,
            },
        ),
        Cmd::Unused {
            paths,
            format,
            exit_non_zero_on_unused,
            no_cache,
            explain_cache,
        } => cmd_unused(
            &paths,
            UnusedFlags {
                json: format == Format::Json,
                fail_on_unused: exit_non_zero_on_unused,
                use_cache: !no_cache,
                explain: explain_cache,
            },
        ),
        Cmd::Resolve {
            module,
            path,
            format,
        } => cmd_resolve(&module, path.as_deref(), format == Format::Json),
        Cmd::Bundle { cmd } => match cmd {
            BundleCmd::Info { file, format } => cmd_bundle_info(&file, format == Format::Json),
        },
    }
}

/// The reporting layer, for the commands that report without a `--format` of their own.
///
/// `htl check` and `htl test` hand their findings to a [`project::Sink`] over a
/// [`report::Out`]; `gen` / `run` / `build` used to loop `eprintln!` over the three
/// vectors themselves, so the same finding read differently depending on which verb was
/// typed. They come through here instead, and there is one renderer again.
///
/// One sink per command run, not per file: what the layer decides once per run — a
/// dependency's error said once, and never on behalf of a file the run checks itself —
/// is then once per *command*, which is what `htl build` over a closure of modules wants.
fn text_sink() -> project::Sink<report::Out> {
    project::Sink::new(report::Out::new(false))
}

/// (Re)generate the `.d.tl` files this project declares: the ones a Rust crate's
/// `#[host_module]` / `#[derive(TealRecord)]` ask for, so the checker sees Rust-side
/// modules before any `cargo build`, and the module each `---@contract` type is declared
/// in, which is what an outside author writes their modules against. Quiet unless
/// something was written.
///
/// A contract that could not be published is said here, on every command that generates:
/// `htl check` reports it again as a lint (which `strict` makes fatal) and `htl dts`
/// exits non-zero on it, but `run` / `test` / `build` have neither, and a declaration
/// that is quietly not written is one an outside author finds missing later.
fn auto_dts(start: &Path) -> Result<()> {
    let mut project = None;
    if let Some((root, _, cfg)) = load_config(start)? {
        let (contracts, _) = htl::contract::resolve(&root, &cfg);
        let (results, problems) = htl::contract::publish(&root, &contracts);
        announce_dts(&results, &root);
        for p in &problems {
            eprintln!("dts: {p}");
        }
        project = Some(root);
    }
    let Some(root) = htl::dts::find_cargo_package_root(start) else {
        return Ok(());
    };
    let results = htl::dts::generate_crate(&root).map_err(|e| anyhow::anyhow!("htl dts: {e}"))?;
    announce_dts(&results, &root);
    let types_root = project.unwrap_or_else(|| root.clone());
    let report = dep_dts(&root, &types_root);
    announce_dts(&report.written, &types_root);
    announce_dep_report(&report, "dts: ");
    Ok(())
}

/// What `htl dts` has to say about the declarations its dependencies ship. Three kinds,
/// kept apart because they mean three different things to the reader and to the exit code
/// — which is the distinction the single `[htl <rule>]` suffix they all used to carry was
/// hiding. None of them is a lint: no rule name, nothing to configure or silence, and
/// nothing that reaches the `Sink`, so `htl check` neither counts one nor carries one in
/// `--format json` — it prints them, prefixed `dts:`, exactly as this command does.
#[derive(Default)]
struct DepReport {
    /// `(target, written)` pairs, in the shape the rest of the report prints.
    written: Vec<(PathBuf, bool)>,
    /// Asked for and not written. This, and only this, is what `htl dts` exits non-zero on.
    not_written: Vec<String>,
    /// Under `types/` and no longer shipped by anything. Reported, never deleted, and it
    /// fails nothing: what the file is for is the project's to say.
    left_in_place: Vec<String>,
    /// The crate graph would not resolve, so nothing was asked for — cargo could not be
    /// run, or the project's lockfile does not cover its manifest and the graph is read
    /// with `--locked`. Either way the committed declarations stand and the check that
    /// follows names the module if one is missing, which is why this fails nothing.
    unresolved: Option<String>,
}

/// Materialise the declarations this project's dependencies ship, under
/// `<types_root>/types/<crate>/`, and report on what happened to each.
///
/// The graph comes from `cargo metadata`, so this costs a subprocess on every command that
/// generates. Nothing is built, and nothing is downloaded for dependencies already fetched.
fn dep_dts(cargo_root: &Path, types_root: &Path) -> DepReport {
    let decls = match htl::dep_dts::resolve(cargo_root) {
        Ok(d) => d,
        Err(e) => {
            return DepReport {
                unresolved: Some(e),
                ..DepReport::default()
            };
        }
    };
    // Orphans before materialising: the note beside a crate's declarations is what says
    // which crate they came from, and the write below rewrites it.
    let left_in_place = htl::dep_dts::orphans(types_root, &decls);
    let (written, not_written) = htl::dep_dts::materialise(types_root, &decls);
    DepReport {
        written,
        not_written,
        left_in_place,
        unresolved: None,
    }
}

/// The three report lists, each said as what it is. `wrote` / `unchanged` name what
/// happened to a declaration, and so do these: a reader who has never heard the words
/// `shipped-declaration` or `orphaned-declaration` can still tell which line failed the
/// command and which one is a file sitting there to decide about.
fn announce_dep_report(report: &DepReport, prefix: &str) {
    for p in &report.not_written {
        eprintln!("{prefix}not written: {p}");
    }
    for p in &report.left_in_place {
        eprintln!("{prefix}left in place: {p}");
    }
    if let Some(e) = &report.unresolved {
        eprintln!("{prefix}{e}");
    }
}

fn announce_dts(results: &[(PathBuf, bool)], root: &Path) {
    for (target, written) in results {
        if *written {
            eprintln!(
                "dts: wrote {}",
                target.strip_prefix(root).unwrap_or(target).display()
            );
        }
    }
}

fn cmd_dts(dir: Option<&Path>) -> Result<ExitCode> {
    let start = match dir {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir()?,
    };
    // The contract types first: a project may publish one without having a Rust host at
    // all, and the `bail!` below would then be wrong about there being nothing to do.
    let mut results = Vec::new();
    let mut failed = false;
    let mut project = None;
    if let Some((croot, _, cfg)) = load_config(&start)? {
        let (contracts, _) = htl::contract::resolve(&croot, &cfg);
        let (published, problems) = htl::contract::publish(&croot, &contracts);
        for p in &problems {
            eprintln!("  {p}");
        }
        // Asked to generate and did not: the command says so in its exit code, or a CI
        // step that regenerates declarations would pass having written nothing.
        failed = !problems.is_empty();
        results.extend(published);
        project = Some(croot);
    }
    let code = |failed: bool| {
        if failed {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        }
    };
    let Some(root) = htl::dts::find_cargo_package_root(&start) else {
        if results.is_empty() && !failed {
            bail!(
                "no Cargo.toml with a [package] section, and no ---@contract type to publish, \
                 at or above {}",
                start.display()
            );
        }
        report_dts(&results, &start);
        return Ok(code(failed));
    };
    results.extend(htl::dts::generate_crate(&root).map_err(|e| anyhow::anyhow!("{e}"))?);
    let types_root = project.unwrap_or_else(|| root.clone());
    let report = dep_dts(&root, &types_root);
    announce_dep_report(&report, "  ");
    // An orphan, and a graph that would not resolve, are reports. A declaration asked for
    // and not written is a failure, the same as a contract that could not be published.
    failed = failed || !report.not_written.is_empty();
    results.extend(report.written);
    report_dts(&results, &root);
    Ok(code(failed))
}

fn report_dts(results: &[(PathBuf, bool)], root: &Path) {
    let mut written = 0usize;
    for (target, w) in results {
        let rel = target.strip_prefix(root).unwrap_or(target).display();
        eprintln!("  {} {}", if *w { "wrote    " } else { "unchanged" }, rel);
        written += usize::from(*w);
    }
    eprintln!(
        "htl dts: {} declaration(s) from {}, {} written",
        results.len(),
        root.display(),
        written
    );
}

/// If `start` is inside an `mlua-pkg.toml` project, expose its vendored deps to the
/// checker / strict searcher. Returns the project when found.
fn apply_project(h: &Htl, start: &Path) -> Result<Option<htl::pkg::Project>> {
    let Some(p) = htl::pkg::Project::find(start) else {
        return Ok(None);
    };
    h.apply_project(&p)?;
    Ok(Some(p))
}

/// What the run wrote, and — when a target was asked for by name — what it left alone. The
/// kept list is what makes `htl init --target bin` on an older project say which of that
/// target's files were already there instead of skipping them in silence.
fn report_scaffold(dir: &Path, written: &[PathBuf], kept: &[PathBuf]) {
    let rel = |p: &PathBuf| p.strip_prefix(dir).unwrap_or(p).display().to_string();
    for p in written {
        eprintln!("  created {}", rel(p));
    }
    for p in kept {
        eprintln!("  kept    {} (already there)", rel(p));
    }
    if kept.is_empty() {
        eprintln!(
            "htl: {} file(s) written under {}",
            written.len(),
            dir.display()
        );
    } else {
        eprintln!(
            "htl: {} file(s) written, {} kept under {}",
            written.len(),
            kept.len(),
            dir.display()
        );
    }
}

fn cmd_new(
    name: &str,
    lib: bool,
    embed: bool,
    target: Option<&str>,
    htl: &str,
    no_x: bool,
) -> Result<ExitCode> {
    let dir = PathBuf::from(name);
    let pkg_name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid project name: {name}"))?
        .to_string();
    // Before the directory is touched: an unknown target, or one that disagrees with
    // --lib, fails here and leaves nothing behind. So does an htl this scaffold does not
    // write for — the pin decides what goes into the files, so it is settled first too.
    let target = scaffold::resolve_target(target, embed, lib)?;
    let htl = scaffold::HtlPin::parse(htl)?;
    let opts = scaffold::Options {
        lib,
        target,
        htl,
        no_x,
    };
    let done = scaffold::scaffold(&dir, &pkg_name, &opts, true)?;
    report_scaffold(&dir, &done.written, &[]);
    // The manifest names a dependency: the fetch comes before the first test, or the
    // first test is a `module not found`.
    let fetch = if opts.writes_htlx() {
        "htl pkg install && "
    } else {
        ""
    };
    eprintln!("next: cd {} && {fetch}htl test", dir.display());
    Ok(ExitCode::SUCCESS)
}

fn cmd_init(
    dir: Option<&Path>,
    lib: bool,
    embed: bool,
    target: Option<&str>,
    htl: &str,
    no_x: bool,
) -> Result<ExitCode> {
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
    let asked_for_a_target = target.is_some() || embed;
    let target = scaffold::resolve_target(target, embed, lib)?;
    let htl = scaffold::HtlPin::parse(htl)?;
    let opts = scaffold::Options {
        lib,
        target,
        htl,
        no_x,
    };
    let done = scaffold::scaffold(&dir, &name, &opts, false)?;
    // A target was named: say what it would have written and found already there. Without
    // one the old one-liner stands, so a plain re-run does not list the whole tree.
    let kept: &[PathBuf] = if asked_for_a_target { &done.kept } else { &[] };
    if done.written.is_empty() && kept.is_empty() {
        eprintln!("htl init: nothing to do, all scaffold files already exist");
    } else {
        report_scaffold(&dir, &done.written, kept);
    }
    Ok(ExitCode::SUCCESS)
}

/// The project `htl pkg` acts on: the nearest one above the working directory.
///
/// mlua-pkg reports a missing manifest as an I/O error that does not name the file, and the
/// path htl looked for is the whole of the answer, so it is checked here.
fn pkg_project() -> Result<htl::pkg::Project> {
    let cwd = std::env::current_dir()?;
    htl::pkg::Project::find(&cwd).with_context(|| {
        format!(
            "no {} above {}: `htl pkg` runs in a project",
            htl::pkg::MANIFEST_NAME,
            cwd.display()
        )
    })
}

/// `htl pkg install`: fetch what the manifest declares, then bring in what the deps publish.
fn cmd_pkg_install() -> Result<ExitCode> {
    let project = pkg_project()?;
    let report = project.install()?;
    report_install(&report, &project);
    // Re-read the project: install wrote the lockfile the two reports below are read from.
    // A dep publishes its declarations at `types/` in its package root, which is not where
    // `require` looks, so they are copied in for the checker to see.
    let project = htl::pkg::Project::at(&project.root);
    report_types_sync(&project.sync_types()?, &project.root);
    report_patch_drift(&project);
    Ok(ExitCode::SUCCESS)
}

/// What install did, in the shape the other reports use. The library prints nothing.
fn report_install(report: &htl::pkg::mlua_pkg::ops::InstallReport, project: &htl::pkg::Project) {
    use htl::pkg::mlua_pkg::ops::Placement;
    for w in &report.warnings {
        // A patch that was not used is reported below in htl's own verbs, where both ways
        // out are named. The library's line points at `mlua-pkg patch --force`, which
        // rebuilds the copy without asking git whether what is in it was committed.
        if w.contains("patch_dir") && w.contains("not used") {
            continue;
        }
        eprintln!("warning: {w}");
    }
    let rel = |p: &Path| {
        p.strip_prefix(&project.root)
            .unwrap_or(p)
            .display()
            .to_string()
    };
    for p in &report.packages {
        let sha: String = p.sha.chars().take(7).collect();
        let where_it_is = match (&p.placement, p.patched) {
            (_, true) => format!("from {}", rel(p.root())),
            (Placement::Copied(dest), _) => format!("copied to {}", rel(dest)),
            (Placement::Symlink(_), _) => p.entry.display().to_string(),
        };
        eprintln!("  install {} {sha} ({where_it_is})", p.name);
    }
    let n = report.packages.len();
    if report.transitive > 0 {
        eprintln!(
            "htl pkg install: {n} package(s), {} of them transitive",
            report.transitive
        );
    } else {
        eprintln!("htl pkg install: {n} package(s)");
    }
}

/// `htl pkg add <name> <git>`: the manifest entry, without fetching anything.
///
/// This is the one verb that may run outside a project: mlua-pkg writes a manifest when
/// there is none, and the directory it writes it in is the working one.
fn cmd_pkg_add(spec: htl::pkg::mlua_pkg::ops::AddSpec) -> Result<ExitCode> {
    use htl::pkg::mlua_pkg::ops::AddOutcome;
    let cwd = std::env::current_dir()?;
    let project = htl::pkg::Project::find(&cwd).unwrap_or_else(|| htl::pkg::Project::at(&cwd));
    let name = spec.name.clone();
    let done = project.add(spec)?;
    let manifest = project
        .manifest
        .strip_prefix(&project.root)
        .unwrap_or(&project.manifest)
        .display();
    if done.report.manifest_created {
        eprintln!("  created {manifest}");
    }
    let verb = match done.report.outcome {
        AddOutcome::Added => "added",
        AddOutcome::Replaced => "updated",
    };
    eprintln!("  {verb}   {name} in {manifest}");
    if let Some(dir) = &done.kept_patch_dir {
        // `add` rewrites the entry, and its spec has no room for a patch. Putting the key
        // back is the difference between the project resolving that dependency from its own
        // copy and resolving it from upstream with the copy left unread in the tree.
        eprintln!(
            "  kept    patch_dir = {} ({name} is patched)",
            dir.display()
        );
    }
    eprintln!("htl pkg add: run `htl pkg install` to fetch it");
    Ok(ExitCode::SUCCESS)
}

/// `htl pkg update [name]`: what each pin does next, then the install that follows.
fn cmd_pkg_update(opts: htl::pkg::mlua_pkg::ops::UpdateOpts) -> Result<ExitCode> {
    use htl::pkg::mlua_pkg::ops::UpdateOutcome;
    let project = pkg_project()?;
    let dry_run = opts.dry_run;
    let report = project.update(opts)?;
    if report.entries.is_empty() {
        eprintln!("htl pkg update: no dependency selected");
        return Ok(ExitCode::SUCCESS);
    }
    for (name, outcome) in &report.entries {
        match outcome {
            UpdateOutcome::TagBumped { old, new } => {
                eprintln!("  update  {name}: tag {old} -> {new}")
            }
            UpdateOutcome::PrefixResolved { pin, resolved } => eprintln!(
                "  update  {name}: prefix '{pin}' is {resolved} (the manifest keeps the prefix)"
            ),
            UpdateOutcome::Refresh => eprintln!("  update  {name}: refresh (branch or unpinned)"),
            UpdateOutcome::Skipped(why) => eprintln!("  skip    {name}: {why}"),
        }
    }
    if dry_run {
        eprintln!("htl pkg update: dry run; nothing was written");
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(install) = &report.install {
        report_install(install, &project);
        let project = htl::pkg::Project::at(&project.root);
        report_types_sync(&project.sync_types()?, &project.root);
        report_patch_drift(&project);
    }
    Ok(ExitCode::SUCCESS)
}

/// `htl pkg clean`: the cache, which is machine-local and rebuilt by the next install.
fn cmd_pkg_clean(all: bool) -> Result<ExitCode> {
    use htl::pkg::mlua_pkg::ops::CleanReport;
    let project = pkg_project()?;
    match project.clean(all)? {
        CleanReport::CacheRemoved => eprintln!("htl pkg clean: removed every cached package"),
        CleanReport::NoLockfile => {
            eprintln!("htl pkg clean: no lockfile, so nothing was ever cached")
        }
        CleanReport::StaleRemoved { removed: 0 } => eprintln!("htl pkg clean: nothing to remove"),
        CleanReport::StaleRemoved { removed } => eprintln!(
            "htl pkg clean: removed {removed} cache entr{} the lockfile no longer refers to",
            if removed == 1 { "y" } else { "ies" }
        ),
    }
    Ok(ExitCode::SUCCESS)
}

/// A patched copy the dependency is no longer resolved from, said after every install
/// until it is refreshed or removed.
///
/// A patch is bound to the revision it was taken from, and an upgrade moves the pin off it.
/// Install does not fail over that — the project builds against the new upstream — so the
/// only thing that keeps the copy from being forgotten in the tree is being told about it
/// each time. Both ways out are named, in htl's verbs: mlua-pkg's own warning points at
/// `mlua-pkg patch --force`, which skips the question htl asks git before overwriting.
fn report_patch_drift(project: &htl::pkg::Project) {
    let short = |s: &str| s.chars().take(7).collect::<String>();
    for s in project.patch_status() {
        if s.in_use {
            continue;
        }
        let rel = s
            .dir
            .strip_prefix(&project.root)
            .unwrap_or(&s.dir)
            .display();
        let why = match (&s.base, &s.locked) {
            _ if !s.dir.is_dir() => "the copy is not there".to_string(),
            (None, _) => "the lockfile records no revision it was taken from".to_string(),
            (Some(b), Some(l)) => {
                format!("taken from {}, {} is now at {}", short(b), s.name, short(l))
            }
            (Some(b), None) => format!("taken from {}, and nothing is locked", short(b)),
        };
        eprintln!("  patch   {rel} is not in use ({why})");
        eprintln!(
            "          carry the change forward: commit it, then `htl pkg patch {}`",
            s.name
        );
        eprintln!("          drop it: remove patch_dir from mlua-pkg.toml and delete {rel}");
    }
}

/// `htl pkg patch <dep>`: the dependency's source, taken into `patches/<dep>/` where the
/// project owns it. Not a passthrough — htl decides the directory, writes `patch_dir` into
/// the manifest and answers the "may this be overwritten" question against git; the copy
/// and the `patch_base` bookkeeping are mlua-pkg's. See `Project::patch`.
fn cmd_pkg_patch(dep: &str, force: bool) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let project = htl::pkg::Project::find(&cwd).context(
        "no mlua-pkg.toml above the current directory: a patch belongs to a project, so this runs in one",
    )?;
    let report = project.patch(dep, force)?;
    let rel = report
        .patch_dir
        .strip_prefix(&project.root)
        .unwrap_or(&report.patch_dir);
    let verb = if report.created { "patched" } else { "rebuilt" };
    let base: String = report.base.chars().take(7).collect();
    eprintln!("  {verb} {} ({dep} at {base})", rel.display());
    eprintln!("htl: it is the project's code now — edit it, commit it, then `htl pkg install`.");
    Ok(ExitCode::SUCCESS)
}

/// What the deps published, in the shape `htl init` reports its scaffold. A name that was
/// already taken is said out loud: nothing was overwritten, and the project is the one
/// that decides which declaration it wants.
fn report_types_sync(sync: &htl::pkg::TypesSync, root: &Path) {
    let rel = |p: &Path| p.strip_prefix(root).unwrap_or(p).display().to_string();
    for (path, dep) in &sync.written {
        eprintln!("  types   {} (from {dep})", rel(path));
    }
    for (path, dep) in &sync.taken {
        eprintln!("  kept    {} ({dep} publishes one too)", rel(path));
    }
}

/// `htl types add <library>`: the declarations a library did not ship, from the collection
/// that has them. The revision is recorded because nothing else in that ecosystem does —
/// see `Project::add_types`.
fn cmd_types_add(library: &str, from: Option<&Path>, force: bool) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let project = htl::pkg::Project::find(&cwd).context(
        "no mlua-pkg.toml above the current directory: `types/` is a project's, so this runs in one",
    )?;
    let sync = match from {
        Some(dir) => project.add_types_from(dir, library, "local", force)?,
        None => project.add_types(library, force)?,
    };
    report_types_sync(&sync, &project.root);
    if sync.written.is_empty() && !sync.taken.is_empty() {
        eprintln!("htl: nothing written; --force replaces what is already there");
    }
    Ok(ExitCode::SUCCESS)
}

/// What `htl unused` was asked for, beyond the paths.
struct UnusedFlags {
    json: bool,
    /// Exit 1 when anything was reported. Off by default: "unused" is a question about
    /// intent, so CI opts in rather than out.
    fail_on_unused: bool,
    use_cache: bool,
    explain: bool,
}

/// What `htl check` was asked for, beyond the paths and the lint selection.
struct CheckFlags {
    strict: bool,
    list_lints: bool,
    json: bool,
    use_cache: bool,
    cache_mode: Option<cache::Mode>,
    explain: bool,
}

struct TestFlags {
    fail_fast: bool,
    verbose: bool,
    quiet: bool,
    slow: Option<f64>,
    update: bool,
    coverage: bool,
    coverage_lines: bool,
    /// Where to write the lcov tracefile, if asked for.
    lcov: Option<PathBuf>,
    /// Where to write the JUnit XML report, if asked for.
    junit: Option<PathBuf>,
    json: bool,
    no_cache: bool,
    explain: bool,
    seed: Option<u64>,
}

fn print_coverage(cov: &report::CoverageReport, with_lines: bool) {
    let width = cov
        .modules
        .iter()
        .map(|m| m.path.len())
        .max()
        .unwrap_or(4)
        .max(5);
    for m in &cov.modules {
        eprintln!(
            "coverage: {:<width$}  {:>5}/{:<5} {:5.1}%",
            m.path,
            m.executed,
            m.total,
            100.0 * m.executed as f64 / m.total as f64
        );
        if !m.never_ran.is_empty() {
            let names: Vec<String> = m
                .never_ran
                .iter()
                .map(|f| format!("{} ({})", f.name, f.line))
                .collect();
            eprintln!("          never ran: {}", names.join(", "));
        }
        if with_lines && !m.unexecuted.is_empty() {
            let spans: Vec<String> = m
                .unexecuted
                .iter()
                .map(|(a, b)| {
                    if a == b {
                        a.to_string()
                    } else {
                        format!("{a}-{b}")
                    }
                })
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

/// One file's report as a JUnit suite.
///
/// The assertion library reports a failure as `"<suite > test>: <message>"` and its tests
/// by the same composed name, so a failing case takes back the message printed under the
/// file by matching that prefix — including the traceback a runtime error carries, which
/// belongs in the report for the same reason it belongs on the terminal: it is the part
/// that says where.
fn junit_suite(rep: &htl::testing::FileReport) -> junit::Suite {
    let mut left: Vec<&str> = rep.failures.iter().map(String::as_str).collect();
    let cases = rep
        .tests
        .iter()
        .map(|t| {
            let failure = (!t.ok)
                .then(|| {
                    let head = format!("{}: ", t.name);
                    let at = left.iter().position(|m| m.starts_with(&head))?;
                    Some(left.remove(at)[head.len()..].to_string())
                })
                .flatten();
            junit::Case {
                name: t.name.clone(),
                ms: t.ms,
                // A failing test whose message could not be matched still failed: say so
                // rather than reporting it as passed.
                failure: failure.or_else(|| (!t.ok).then(|| "failed".to_string())),
            }
        })
        .collect();
    let error = if !rep.check.ok() {
        Some(junit::Error {
            message: "type check failed".to_string(),
            kind: "check",
            body: rep.check.errors.join("\n"),
        })
    } else {
        rep.error.as_ref().map(|e| junit::Error {
            message: e.clone(),
            kind: "error",
            body: e.clone(),
        })
    };
    junit::Suite {
        file: rep.path.display().to_string(),
        duration_ms: rep.duration_ms,
        error,
        cases,
    }
}

fn cmd_test(
    paths: &[PathBuf],
    filter: Option<&str>,
    lib: &str,
    lint: Option<&str>,
    flags: TestFlags,
) -> Result<ExitCode> {
    let paths = if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths.to_vec()
    };
    if let Some(first) = paths.first() {
        auto_dts(first)?;
    }
    // A patched dependency's `*_test.tl` are its suite, not this project's: `htl pkg patch`
    // takes the whole package root, tests included, and running them here would report a
    // library's own failures as the project's.
    let files = htl::testing::discover_tests_skipping(&paths, &project::patched(&paths))?;
    if files.is_empty() {
        eprintln!("htl test: no test files found (looked for *_test.tl and tests/**/*.tl)");
        return Ok(ExitCode::FAILURE);
    }
    let cfg = load_config(&paths[0])?;
    let opts = project::TestOptions {
        config: &cfg,
        lint,
        lib,
        filter,
        run: htl::testing::RunOptions {
            fail_fast: flags.fail_fast,
            update_snapshots: flags.update,
            coverage: flags.coverage,
            // Given, or drawn for the run and printed below, so that it can be given back.
            seed: flags.seed,
        },
        cache: project::cache_options(
            !flags.no_cache,
            Some(cache::Mode::PerModule),
            &cfg,
            flags.explain,
        ),
    };

    let mut sink = project::Sink::new(report::Out::new(flags.json));
    let mut json_files: Vec<report::TestFile> = Vec::new();
    let mut junit_suites: Vec<junit::Suite> = Vec::new();
    // What is left of the run here: how a file reads on a terminal, and what the document
    // says about it. Which files run, in what isolation, from what store — and what they
    // draw — is `project::test`'s.
    let rep = project::test(&mut sink, &files, &opts, &mut |rep, sink| {
        if flags.json {
            json_files.push(report::TestFile::from_report(rep, sink.out().take()));
        }
        if flags.junit.is_some() {
            junit_suites.push(junit_suite(rep));
        }
        let f = &rep.path;
        let tag = if rep.ok() { "ok  " } else { "FAIL" };
        let detail = if !rep.check.ok() {
            "type check failed".to_string()
        } else if let Some(e) = &rep.error {
            // The cause on the file's own line, its frames under it: a traceback inside
            // the parenthesised summary would push `12 ms` past a screen of stack.
            format!("error: {}", e.lines().next().unwrap_or_default())
        } else if rep.file_level {
            "ran to completion (no test library used)".to_string()
        } else {
            format!("{} passed, {} failed", rep.passed, rep.failed)
        };
        // Quiet: a passing file is silence; failures, errors and slow tests still show.
        // JSON: nothing on stderr, the document carries it all.
        let show_file = !flags.json && (!flags.quiet || !rep.ok());
        if show_file {
            eprintln!(
                "{tag} {}  ({detail}, {:.0} ms)",
                f.display(),
                rep.duration_ms
            );
            if let Some(e) = &rep.error {
                for line in e.lines().skip(1) {
                    eprintln!("      {line}");
                }
            }
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
                let note = if slow && !flags.verbose {
                    "  [slow]"
                } else {
                    ""
                };
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
    })?;

    if let Some(out) = &flags.junit {
        // The run's own total, not the sum of the files: it is the number the summary
        // line prints, and the report is there to agree with the summary line.
        std::fs::write(out, junit::document(&junit_suites, rep.duration_ms))
            .with_context(|| format!("writing {}", out.display()))?;
    }
    if let (Some(out), Some(cov)) = (&flags.lcov, &rep.coverage) {
        // Against the project root rather than the working directory: a tracefile is
        // uploaded from wherever CI ran the command and resolved against the repository.
        let root = match &cfg {
            Some((dir, _, _)) => dir.clone(),
            None => std::env::current_dir()?,
        };
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        std::fs::write(out, cov.lcov(&root))
            .with_context(|| format!("writing {}", out.display()))?;
    }
    let ok = rep.ok();
    if flags.json {
        let summary = report::TestSummary {
            files: rep.files.len(),
            files_run: rep.ran,
            passed: rep.passed,
            failed: rep.failed,
            files_with_errors: rep.files_with_errors,
            replayed: rep.replayed,
            duration_ms: rep.duration_ms,
            ok,
            seed: rep.seed,
        };
        report::emit(&report::TestReport {
            files: json_files,
            summary,
            coverage: rep.coverage,
        })?;
    } else {
        if let Some(cov) = &rep.coverage {
            print_coverage(cov, flags.coverage_lines);
            if let Some(out) = &flags.lcov {
                eprintln!("coverage: lcov written to {}", out.display());
            }
        }
        eprintln!(
            "htl test: {} file(s), {} passed, {} failed, {} file(s) with errors{}{} ({:.0} ms)",
            rep.ran,
            rep.passed,
            rep.failed,
            rep.files_with_errors,
            if rep.skipped() > 0 {
                format!(", {} file(s) not run (--fail-fast)", rep.skipped())
            } else {
                String::new()
            },
            // Says the checking was reused, not the run: every one of these files ran.
            if rep.replayed > 0 {
                format!(", {} checked from cache", rep.replayed)
            } else {
                String::new()
            },
            rep.duration_ms
        );
        // Always, not only on failure: the seed of a run that passed is what reproduces
        // the run that passes, and a failure two commits later is compared against it.
        eprintln!(
            "htl test: seed {} (repeat with --seed {})",
            rep.seed, rep.seed
        );
    }
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Name the patched dependencies this walk enters, in the shape the other reports use.
///
/// Only the ones actually below a given root: `htl check src` walks the project's own
/// sources and nothing under `patches/`, and saying otherwise would be a claim about files
/// that were not read.
fn report_patched(paths: &[PathBuf]) {
    let roots: Vec<PathBuf> = paths
        .iter()
        .filter_map(|p| std::fs::canonicalize(p).ok())
        .collect();
    let cwd = std::env::current_dir().unwrap_or_default();
    let Some(project) = paths.first().and_then(|p| htl::pkg::Project::find(p)) else {
        return;
    };
    for p in &project.patches {
        let canon = std::fs::canonicalize(&p.dir).unwrap_or_else(|_| p.dir.clone());
        if !roots.iter().any(|r| canon.starts_with(r)) {
            continue;
        }
        let shown = p.dir.strip_prefix(&cwd).unwrap_or(&p.dir);
        eprintln!("  patched {} ({})", shown.display(), p.name);
    }
}

/// Nearest `htl.toml` above the first path: `(dir holding it, path, config)`.
///
/// Where `[toolchain] htl` is answered, and the only place: every command that reads the
/// config comes through here, and the pin has to be settled before the command reads a
/// source file — which is a property of this function, not something each command could
/// be trusted to repeat. `htl new` / `htl init` do not call it, so a mismatch never stops
/// anyone from creating a project.
///
/// The version compared is this binary's, which is the one the pin is about. The `htl`
/// crate a Rust host builds against is Cargo's business and is pinned in `Cargo.toml`.
fn load_config(first: &Path) -> Result<project::Config> {
    let cfg = project::config_of(first)?;
    if let Some((_, path, c)) = &cfg {
        htl::config::check_toolchain(c, path, env!("CARGO_PKG_VERSION"))?;
    }
    Ok(cfg)
}

fn cmd_fmt(paths: &[PathBuf], check: bool, indent: Option<usize>) -> Result<ExitCode> {
    let paths = if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths.to_vec()
    };
    let cfg = load_config(&paths[0])?;
    let indent = indent
        .or_else(|| cfg.as_ref().and_then(|(_, _, c)| c.fmt.indent))
        .unwrap_or(3);
    let h = Htl::new()?;
    // Not a patched dependency: formatting the copy would turn every one of its files into
    // a diff against the revision it was taken from, and bury the project's own change
    // somewhere inside that.
    let files = htl::collect_tl_skipping(&paths, &project::patched(&paths))?;
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
    Ok(if fail {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
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
    let paths = if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths.to_vec()
    };
    let h = Htl::new()?;
    let cfg = load_config(&paths[0])?;
    let file_spec = cfg
        .as_ref()
        .map(|(_, _, c)| c.lint_spec())
        .unwrap_or_default();
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
    h.install_std()?;
    let opts = FixOptions {
        unsafe_fixes: flags.unsafe_fixes,
        promoted: cfg
            .as_ref()
            .map(|(_, _, c)| c.fix.unsafe_.clone())
            .unwrap_or_default(),
        disabled: cfg
            .as_ref()
            .map(|(_, _, c)| c.fix.disable.clone())
            .unwrap_or_default(),
        only: flags.rule.clone(),
        dry_run: flags.dry_run,
    };
    // Here as well as inside `fix_file`, so a misspelt rule is answered even when the
    // paths hold no `.tl` at all — the request is wrong either way.
    opts.validate()?;
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
                dirty
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join("\n  ")
            );
        }
        if !no_vcs.is_empty() && !flags.allow_no_vcs {
            bail!(
                "htl fix rewrites files in place and relies on git to undo it; these are not in a git repository:\n  {}\npass --allow-no-vcs to proceed without that safety net",
                no_vcs
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join("\n  ")
            );
        }
    }

    let mut sink = project::Sink::new(report::Out::new(flags.json));
    // Dependencies are reported as `htl check` reports them and never rewritten: a fix
    // under `.htl/` goes at the next install, one under `[check] paths` is not this
    // project's. `fix_file` only ever writes the file it was given.
    let fix_root = cfg
        .as_ref()
        .map(|(r, _, _)| r.clone())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let contracts = match &cfg {
        Some((r, _, c)) => htl::contract::resolve(r, c).0,
        None => Vec::new(),
    };
    let origins = project::Origins::new(&paths[0], &fix_root, &cfg, &contracts);
    sink.walking(&files);
    let (mut applied, mut skipped, mut json_files) = (Vec::new(), Vec::new(), Vec::new());
    let (mut changed, mut deferred, mut reverted, mut errors_remaining) =
        (0usize, 0usize, 0usize, 0usize);
    for f in &files {
        h.add_layout_paths(f)?;
        let before = if flags.diff {
            std::fs::read_to_string(f).ok()
        } else {
            None
        };
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
                eprintln!(
                    "skipped: {}:{}: {} — {}",
                    f.display(),
                    s.line,
                    s.rule,
                    s.reason
                );
            }
            if let Some(r) = &out.reverted {
                eprintln!("reverted: {}: {r}", f.display());
            }
            if let Some(o) = &out.oscillation {
                eprintln!("stopped: {}: fixes of {o} undo each other", f.display());
            }
            if out.deferred > 0 {
                eprintln!(
                    "deferred: {}: {} edit(s) overlapped applied ones; run htl fix again",
                    f.display(),
                    out.deferred
                );
            }
            if flags.diff
                && let (Some(b), Some(a)) = (&before, &out.contents)
            {
                print!("{}", unified_diff(&f.display().to_string(), b, a));
            }
            // A suggestion is never written, so the diff is the only place it is shown.
            // Against what was applied, when something was: the two are one edit session.
            if flags.diff
                && let (Some(b), Some(s)) = (
                    out.contents.as_ref().or(before.as_ref()),
                    out.suggested.as_ref(),
                )
            {
                print!(
                    "{}",
                    unified_diff(&format!("{} (suggested)", f.display()), b, s)
                );
            }
        }
        sink.checkinfo(&out.check);
        sink.dependency_errors(&out.check, &|p| origins.of(p));
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
                diagnostics: sink.out().take(),
            });
        }
    }
    // A broken dependency is an error `htl fix` cannot remove; it remains, as `htl check`
    // would count it, so the two exit the same way on the same tree.
    errors_remaining += sink.dependency_error_count();
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
            if flags.dry_run {
                " (dry run, nothing written)"
            } else {
                ""
            },
            errors_remaining,
            if deferred > 0 {
                format!(", {deferred} deferred")
            } else {
                String::new()
            }
        );
    }
    Ok(if fail {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
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
fn cmd_check(paths: &[PathBuf], lint: Option<&str>, flags: CheckFlags) -> Result<ExitCode> {
    let CheckFlags {
        list_lints,
        json,
        use_cache,
        cache_mode,
        explain,
        ..
    } = flags;
    let mut sink = project::Sink::new(report::Out::new(json));
    let paths = if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths.to_vec()
    };
    // Listing the rules reports nothing about the project and shares nothing with a run —
    // and now not even a Lua state: the registry is Rust (`htl_core::lint::RULES`), which
    // is what lets the rules the project layer reports under be listed beside the ones
    // `lint.lua` implements.
    if list_lints {
        // The name and the level a project that says nothing gets. Two columns rather than
        // one because the default is a level now, and the three rules at `allow` are
        // otherwise invisible: a reader would have to turn one on to find out it was off.
        // Still one rule per line, name first, so it reads and greps as it always did.
        let width = htl::lint::rule_names()
            .iter()
            .map(|r| r.len())
            .max()
            .unwrap_or(0);
        for (name, level) in htl::lint::rule_defaults() {
            println!("{name:width$}  {level}");
        }
        return Ok(ExitCode::SUCCESS);
    }
    // htl.toml first, then --lint, so the flag wins; `strict` from the file unless flagged.
    let cfg = load_config(&paths[0])?;
    let strict = flags.strict
        || cfg
            .as_ref()
            .and_then(|(_, _, c)| c.lint.strict)
            .unwrap_or(false);
    // `.d.tl` written from Rust source is an input to the check, so it is regenerated
    // before anything hashes the tree: a hit computed over stale declarations would be a
    // hit on a different question from the one being asked.
    if let Some(first) = paths.first() {
        auto_dts(first)?;
    }
    // A patched dependency is walked with the project's own sources — it is the project's
    // code, and its errors are the project's to fix. Which dependency each directory
    // stands in for is said here, so that an error under it is read as that dependency's
    // without the reader having to know the manifest.
    let files = htl::collect_tl(&paths)?;
    if !json {
        report_patched(&paths);
    }

    // The walk, the store and every decision about them are the library's: `include_tl!`
    // asks the same questions of the same store, and the two answering them separately is
    // what let them disagree. What is left here is the flags, the summary and the exit code.
    let rep = project::check(
        &mut sink,
        &files,
        &project::Options {
            paths: &paths,
            config: &cfg,
            lint,
            cache: project::cache_options(use_cache, cache_mode, &cfg, explain),
        },
    )?;
    let fail = report_check(sink.out(), json, &rep, strict)?;
    Ok(if fail {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// Report what no entry's `require` closure reaches.
///
/// The graph is the check's — `htl check` resolves every `require` already, and the
/// `require-cycle` lint reads the same edges — so this is that walk plus the entries the
/// project declares (`htl::unused`). What is left here is the flags, the printing and the
/// exit code.
fn cmd_unused(paths: &[PathBuf], flags: UnusedFlags) -> Result<ExitCode> {
    let UnusedFlags {
        json,
        fail_on_unused,
        use_cache,
        explain,
    } = flags;
    let paths = if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths.to_vec()
    };
    let cfg = load_config(&paths[0])?;
    // A `.d.tl` written from Rust source is an input to the check the graph comes from,
    // exactly as it is for `htl check`.
    auto_dts(&paths[0])?;
    let rep = htl::unused::unused(&htl::unused::Options {
        paths: &paths,
        config: &cfg,
        cache: project::cache_options(use_cache, None, &cfg, explain),
    })?;
    let s = &rep.summary;
    if json {
        report::emit(&rep)?;
    } else if s.no_entry {
        eprintln!(
            "htl unused: nothing to start from, so nothing is reported. An entry is \
             src/main.tl, a test file, a module under a [[contract]] directory, or a name \
             in [build] extra / host"
        );
    } else {
        if s.check_errors > 0 {
            eprintln!(
                "htl unused: {} error(s) in the check this graph comes from — what those \
                 files require is missing from it (htl check)",
                s.check_errors
            );
        }
        for m in &rep.modules {
            match &m.module {
                Some(n) => eprintln!("module: {} ({n})", m.path),
                None => eprintln!("module: {}", m.path),
            }
        }
        for d in &rep.dependencies {
            eprintln!("dependency: {}", d.name);
        }
        eprintln!(
            "htl unused: {}, {} [{} considered, {} reached, {}]",
            count(s.modules, "module", "modules"),
            count(s.dependencies, "dependency", "dependencies"),
            s.considered,
            s.reached,
            count(s.entries, "entry", "entries"),
        );
    }
    Ok(if fail_on_unused && !s.ok {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// Say which file a module name resolves to, and what that hides.
///
/// The chain comes from the library ([`htl::resolve`]), over a checker set up exactly as a
/// check of this project sets one up — the installed deps, then the config's search paths,
/// in that order, because `add_path` prepends and the order is the whole answer. Asking
/// any other way would report a path nothing else uses.
///
/// A report, so both forms go to stdout (README, "Machine-readable output").
fn cmd_resolve(module: &str, path: Option<&Path>, json: bool) -> Result<ExitCode> {
    let start = path.unwrap_or(Path::new("."));
    let cfg = load_config(start)?;
    // The declarations a check generates before it reads anything are inputs to the
    // answer — `types/<crate>/` is written by `htl dts` — and a report without them would
    // describe a project nobody checks.
    auto_dts(start)?;
    let h = Htl::new()?;
    // First, so that the project's own directories go in front of it as they do under
    // `htl run`: `add_path` prepends, and the report prints the order it searched.
    h.install_std()?;
    let project = apply_project(&h, start)?;
    if let Some((root, _, c)) = &cfg {
        h.apply_config(root, c)?;
    }
    let rep = htl::resolve::resolve(
        &h,
        module,
        cfg.as_ref().map(|(r, _, _)| r.as_path()),
        project.as_ref(),
    )?;
    if json {
        report::emit(&rep)?;
    } else {
        print_resolution(&rep);
    }
    Ok(if rep.summary.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// The text form of a resolution: a table in search order, then the directories that were
/// consulted. The order column is the reason one row is read and the others are not, which
/// is why it is printed rather than left to be looked up.
fn print_resolution(r: &htl::resolve::Resolution) {
    match &r.read {
        Some(read) => println!("htl resolve {}: {read}", r.module),
        None => println!(
            "htl resolve {}: nothing on the search path answers require(\"{}\")",
            r.module, r.module
        ),
    }
    if !r.candidates.is_empty() {
        let w = |f: fn(&htl::resolve::Candidate) -> usize, head: usize| {
            r.candidates.iter().map(f).max().unwrap_or(0).max(head)
        };
        let file = w(|c| c.path.len(), 4);
        let kind = w(|c| c.kind.as_str().len(), 4);
        let read_at = r
            .candidates
            .iter()
            .find(|c| c.status == htl::resolve::Status::Read)
            .map(|c| c.order);
        println!();
        println!("  order  {:file$}  {:kind$}  status", "file", "kind");
        for c in &r.candidates {
            let status = match (c.status, c.shadowed_by, read_at) {
                (htl::resolve::Status::Shadowed, Some(by), _) => format!("shadowed by {by}"),
                // Not hidden by the row that is read: typed by it, and loaded by the run.
                (htl::resolve::Status::Runtime, _, Some(by)) => format!("runtime, typed by {by}"),
                (s, _, _) => s.as_str().to_string(),
            };
            let origin = match &c.origin {
                Some(o) => format!("  ({})", o.describe()),
                None => String::new(),
            };
            println!(
                "  {:<5}  {:file$}  {:kind$}  {status}{origin}",
                c.order,
                c.path,
                c.kind.as_str()
            );
        }
    }
    println!();
    println!("  searched, in order: {}", r.searched.join(", "));
}

/// `1 module` / `2 modules`: a count whose noun agrees with it, for a line short enough
/// that `module(s)` would be the loudest thing on it.
fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
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
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .count()
        })
        .unwrap_or(0);
    fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
    eprintln!(
        "htl cache: removed {n} {} from {}",
        if n == 1 { "entry" } else { "entries" },
        dir.display()
    );
    Ok(ExitCode::SUCCESS)
}

/// The project root a cache command works on: beside `htl.toml`, or the working directory.
fn cache_root(path: Option<&Path>) -> Result<PathBuf> {
    let start = path.unwrap_or(Path::new("."));
    Ok(cache::root_for(start)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))))
}

/// Say what the store holds.
///
/// Answers "what is stored", not "what would be reused": an entry listed here can still miss
/// on the next run because a file it recorded has changed. The two are different questions
/// and conflating them would make this command lie in the more dangerous direction.
fn cmd_cache_status(path: Option<&Path>, json: bool, list_entries: bool) -> Result<ExitCode> {
    let c = cache::describe(&cache_root(path)?);
    if json {
        report::emit(&c)?;
        return Ok(ExitCode::SUCCESS);
    }

    if c.entries.is_empty() {
        println!("htl cache: empty ({})", c.dir);
        return Ok(ExitCode::SUCCESS);
    }
    let mut by_kind: std::collections::BTreeMap<&str, (usize, u64)> = Default::default();
    for e in &c.entries {
        let slot = by_kind.entry(e.kind.as_str()).or_default();
        slot.0 += 1;
        slot.1 += e.bytes;
    }
    println!(
        "htl cache: {} entries, {} at {}",
        c.entries.len(),
        human_bytes(c.bytes),
        c.dir
    );
    for (kind, (n, bytes)) in &by_kind {
        let what = match *kind {
            cache::CHECK => "checked modules",
            cache::GEN => "checked and generated test files (htl test)",
            cache::MODULE => "generated modules (htl test / build, include_bundle!)",
            cache::RUN => "whole runs",
            _ => "entries this build cannot read",
        };
        println!("  {n:>5} {what} ({})", human_bytes(*bytes));
    }
    if let (Some(min), Some(max)) = (
        c.entries.iter().map(|e| e.age_secs).min(),
        c.entries.iter().map(|e| e.age_secs).max(),
    ) {
        println!(
            "  last used between {} and {} ago",
            human_age(min),
            human_age(max)
        );
    }
    if list_entries {
        println!();
        for e in &c.entries {
            println!(
                "  {:<6} {:>8}  {}",
                e.kind,
                human_bytes(e.bytes),
                e.subjects.join(", ")
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn human_bytes(n: u64) -> String {
    if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1u64 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{:.0} KB", n as f64 / (1u64 << 10) as f64)
    } else {
        format!("{n} B")
    }
}

fn human_age(secs: u64) -> String {
    match secs {
        s if s < 90 => format!("{s}s"),
        s if s < 5400 => format!("{}m", s / 60),
        s if s < 172_800 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// The one place a check reports its totals, so a replayed module and a checked one cannot
/// drift apart in how they are summarized. Returns whether the run counts as a failure.
fn report_check(
    out: &mut report::Out,
    json: bool,
    rep: &project::Report,
    strict: bool,
) -> Result<bool> {
    let (files, replayed) = (rep.files.len(), rep.replayed);
    let (errors, warnings, lints) = (rep.errors, rep.warnings, rep.lints);
    let fail = rep.failed(strict);
    let all_cached = rep.all_cached();
    if json {
        report::emit(&report::CheckReport {
            files,
            diagnostics: out.take(),
            summary: report::CheckSummary {
                errors,
                warnings,
                lints,
                denied: rep.denied,
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
        // The three counts say what kinds of thing the run said; `at deny` says how much
        // of that it failed on, and is left out when it is none of it — its being there at
        // all is the news. A run with two warnings and one denied lint reads as
        // `0 error(s), 2 warning(s), 1 lint(s), 1 at deny`.
        let denied = match rep.denied {
            0 => String::new(),
            n => format!(", {n} at deny"),
        };
        eprintln!(
            "htl check: {files} file(s), {errors} error(s), {warnings} warning(s), {lints} lint(s){denied}{}{cached}",
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
    h.install_std()?;
    let (code, c) = h.gen_lua(file)?;
    text_sink().checkinfo(&c);
    let Some(mut code) = code else {
        return Ok(ExitCode::FAILURE);
    };
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
    h.install_std()?;
    if Bundle::is_bundle(&bytes) {
        let b = Bundle::decode(&bytes)?;
        return Ok(match h.run_bundle(&b, args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                // A bundle's frames are as good as its payload: stripped bytecode has no
                // lines to show, and `htl build --debug` is what keeps them.
                eprintln!("{}", htl::developer_message(&e));
                ExitCode::FAILURE
            }
        });
    }
    // Check first so lints/warnings are visible before the script runs.
    h.add_layout_paths(file)?;
    h.install_searcher()?;
    h.set_arg(&file.to_string_lossy(), args)?;
    let (code, c) = h.gen_lua(file)?;
    text_sink().checkinfo(&c);
    let Some(code) = code else {
        return Ok(ExitCode::FAILURE);
    };
    match h.exec(&code, &format!("@{}", file.display()), args) {
        Ok(()) => Ok(ExitCode::SUCCESS),
        Err(e) => {
            // Innermost cause and the frames that reached it. `htl run` is a development
            // command, so the frames are the default; `htl::user_message` is what an
            // embedding host shows people who did not write the Teal.
            eprintln!("{}", htl::developer_message(&e));
            Ok(ExitCode::FAILURE)
        }
    }
}

/// The run cache as `htl build` was asked to use it: `--no-cache` and `--explain-cache`,
/// the two switches that mean something for one closure. There is no `--cache-mode`: a
/// build's entries are per module, as `htl test`'s are, since a build is many modules and
/// an edit to one should cost that one and its dependents rather than the closure.
struct BuildCache {
    use_cache: bool,
    explain: bool,
}

fn cmd_build(
    entry: &Path,
    out: &Path,
    main: &str,
    mut opts: htl::link::LinkOptions,
    cache_flags: BuildCache,
) -> Result<ExitCode> {
    let h = Htl::new()?;
    auto_dts(entry)?;
    apply_project(&h, entry)?;
    // `std.*` resolves to its declaration and nothing else, so the linker files it under
    // the host's modules — which is what it is: the binary that runs the bundle preloads
    // it, as `htl run` does before `run_bundle`.
    h.install_std()?;
    let cfg = load_config(entry)?;
    if let Some((root, _, cfg)) = &cfg {
        h.apply_config(root, cfg)?;
        opts.extra.extend(cfg.build.extra.iter().cloned());
        opts.host.extend(cfg.build.host.iter().cloned());
        // The one thing `[build] target` changes: this command produces the `hb` target and
        // nothing else, so a project that records another is told which command does build
        // it rather than handed a bundle nothing in it was going to load. Here rather than
        // further down because both forms of `htl build` pass through — the directory form
        // returns a few lines below — and because nothing has been written yet.
        if let Some(t) = cfg.build.target.filter(|t| *t != BuildTarget::Hb) {
            eprintln!(
                "htl build: this project's target is `{}` — what runs its output is {} — and `htl build` writes a `.hb` bundle, which is the `hb` target. Build it with `cargo build`; to bundle its scripts anyway, drop `[build] target` from htl.toml.",
                t.name(),
                t.runs_it()
            );
            return Ok(ExitCode::FAILURE);
        }
    }
    if entry.is_dir() {
        if cache_flags.explain {
            eprintln!("htl cache: the directory form of `htl build` is not cached");
        }
        return cmd_build_dir(&h, entry, out, main, &opts);
    }
    h.add_layout_paths(entry)?;
    // The store: `module` entries keyed the way `htl test` and the macros key them — the
    // file the module is and the lint selection `htl.toml` puts in force — so a module any
    // of them generated is one the build replays, and the reverse. Nothing is swept here:
    // a build sees one closure, and only `htl check`, which sees the project, bounds the
    // store (`cache.rs`).
    let root = cache::root_for(entry)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let spec = cfg
        .as_ref()
        .map(|(_, _, c)| c.lint_spec())
        .unwrap_or_default();
    let lint = (!spec.is_empty()).then_some(spec.as_str());
    let cache_opts = project::cache_options(
        cache_flags.use_cache,
        Some(cache::Mode::PerModule),
        &cfg,
        cache_flags.explain,
    );
    let store = project::store(&root, cache_opts, None, "htl build");
    let link_store = store.as_ref().map(|c| htl::link::LinkStore {
        cache: c,
        lint,
        root: &root,
        config: cfg.as_ref().map(|(_, p, c)| (p.as_path(), c)),
    });
    let linked = htl::link::link_with(&h, entry, &opts, link_store)?;
    project::explain_cache(store.as_ref(), cache_opts);
    // One sink for the whole closure: a build is many modules, and what the layer says
    // once per run is said once for the build rather than once per module.
    let mut sink = text_sink();
    for (_, c) in &linked.checks {
        sink.checkinfo(c);
    }
    let n_err = linked.errors.len();
    for e in linked
        .errors
        .iter()
        .filter(|e| e.contains("is not on the search path"))
    {
        eprintln!("error: {e}");
    }
    if n_err > 0 {
        eprintln!("htl build: {n_err} error(s), bundle not written");
        return Ok(ExitCode::FAILURE);
    }
    let buf = linked.bundle()?.encode();
    fs::write(out, &buf).with_context(|| format!("writing {}", out.display()))?;
    let typed = linked.modules.iter().filter(|m| m.typed).count();
    // The same suffix `htl check` prints: everything from the store, some of it, or
    // nothing said when none was.
    let cached = if typed > 0 && linked.cached == typed {
        " [cached]".to_string()
    } else if linked.cached > 0 {
        format!(" [{}/{typed} cached]", linked.cached)
    } else {
        String::new()
    };
    eprintln!(
        "htl build: {} module(s) ({typed} typed, {} lua) -> {} ({} bytes{}{}){cached}",
        linked.modules.len(),
        linked.modules.len() - typed,
        out.display(),
        buf.len(),
        if opts.source {
            ", source"
        } else {
            ", bytecode"
        },
        if opts.debug { " with debug info" } else { "" }
    );
    if !linked.host_modules.is_empty() {
        eprintln!(
            "htl build: host must provide: {}",
            linked.host_modules.join(", ")
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// The older form: every `.tl` under a directory, module names from their paths.
fn cmd_build_dir(
    h: &Htl,
    dir: &Path,
    out: &Path,
    entry: &str,
    opts: &htl::link::LinkOptions,
) -> Result<ExitCode> {
    h.add_path(dir)?;
    let files = htl::collect_tl(&[dir.to_path_buf()])?;
    let mut b = Bundle {
        entry: entry.to_string(),
        htl_version: env!("CARGO_PKG_VERSION").into(),
        ..Default::default()
    };
    let mut n_err = 0usize;
    let mut sink = text_sink();
    for f in &files {
        let name = htl::module_name(dir, f)?;
        let (code, c) = h.gen_lua(f)?;
        sink.checkinfo(&c);
        n_err += c.errors.len();
        let Some(code) = code else { continue };
        let (kind, payload) = if opts.source {
            (htl::bundle::Kind::Source, code.into_bytes())
        } else {
            (
                htl::bundle::Kind::Bytecode,
                h.compile_with(&name, &code, !opts.debug)?,
            )
        };
        b.modules.push(htl::bundle::Module {
            name,
            kind,
            payload,
        });
    }
    if n_err > 0 {
        eprintln!("htl build: {n_err} error(s), bundle not written");
        return Ok(ExitCode::FAILURE);
    }
    if b.module(entry).is_none() {
        bail!(
            "entry module '{entry}' not found among {} module(s)",
            b.modules.len()
        );
    }
    if !opts.source {
        b.fingerprint = h.fingerprint()?;
    }
    let buf = b.encode();
    fs::write(out, &buf).with_context(|| format!("writing {}", out.display()))?;
    eprintln!(
        "htl build: {} module(s) -> {} ({} bytes)",
        b.modules.len(),
        out.display(),
        buf.len()
    );
    Ok(ExitCode::SUCCESS)
}

/// What `htl bundle info` reports: everything the file records and nothing it does not
/// (no Lua state is created, nothing is loaded). `lua` is the header the bytecode was
/// compiled for, in the fields the mismatch message names; `None` when the bundle
/// carries no fingerprint, which `payload` explains (`source`) or `format` does (`1`).
#[derive(serde::Serialize)]
struct BundleInfo {
    file: String,
    format: u8,
    htl_version: Option<String>,
    /// `bytecode`, `source`, or `mixed` when modules disagree.
    payload: &'static str,
    lua: Option<htl::bundle::LuaHeader>,
    entry: String,
    modules: Vec<BundleModuleInfo>,
    host_modules: Vec<String>,
}

#[derive(serde::Serialize)]
struct BundleModuleInfo {
    name: String,
    kind: &'static str,
    bytes: usize,
}

fn cmd_bundle_info(file: &Path, json: bool) -> Result<ExitCode> {
    let bytes = fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let Some(format) = htl::bundle::format_version(&bytes) else {
        bail!("{} is not an htl bundle (bad magic)", file.display());
    };
    let b = Bundle::decode(&bytes)?;
    let kind_name = |k: htl::bundle::Kind| match k {
        htl::bundle::Kind::Bytecode => "bytecode",
        htl::bundle::Kind::Source => "source",
    };
    let payload = match (
        b.modules
            .iter()
            .any(|m| m.kind == htl::bundle::Kind::Bytecode),
        b.modules
            .iter()
            .any(|m| m.kind == htl::bundle::Kind::Source),
    ) {
        (true, true) => "mixed",
        (false, true) => "source",
        // Bytecode, or no modules at all: what the loader would treat it as.
        _ => "bytecode",
    };
    let info = BundleInfo {
        file: file.display().to_string(),
        format,
        htl_version: (!b.htl_version.is_empty()).then(|| b.htl_version.clone()),
        payload,
        lua: htl::bundle::LuaHeader::parse(&b.fingerprint),
        entry: b.entry.clone(),
        modules: b
            .modules
            .iter()
            .map(|m| BundleModuleInfo {
                name: m.name.clone(),
                kind: kind_name(m.kind),
                bytes: m.payload.len(),
            })
            .collect(),
        host_modules: b.host_modules.clone(),
    };
    if json {
        report::emit(&info)?;
        return Ok(ExitCode::SUCCESS);
    }

    println!(
        "{}: htl bundle, format {}, built by {}",
        info.file,
        info.format,
        match &info.htl_version {
            Some(v) => format!("htl {v}"),
            None => "htl (version not recorded)".to_string(),
        }
    );
    println!("  payload:  {}", info.payload);
    let lua = match &info.lua {
        Some(h) => h.to_string(),
        None if !b.fingerprint.is_empty() => {
            format!("unreadable fingerprint ({} bytes)", b.fingerprint.len())
        }
        None if info.format == 1 => {
            "not recorded (a format 1 bundle carries no fingerprint)".into()
        }
        None => "any (source payload, no bytecode to bind)".into(),
    };
    println!("  lua:      {lua}");
    println!("  entry:    {}", info.entry);
    let modules: Vec<String> = info
        .modules
        .iter()
        .map(|m| {
            if info.payload == "mixed" {
                format!("{} ({})", m.name, m.kind)
            } else {
                m.name.clone()
            }
        })
        .collect();
    println!(
        "  modules:  {}",
        if modules.is_empty() {
            "(none)".to_string()
        } else {
            modules.join(", ")
        }
    );
    println!(
        "  host:     {}",
        if info.host_modules.is_empty() {
            "(none)".to_string()
        } else {
            info.host_modules.join(", ")
        }
    );
    Ok(ExitCode::SUCCESS)
}
