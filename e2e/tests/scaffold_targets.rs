//! Every target `--target` offers, scaffolded into a temporary directory outside this
//! repository, pointed back at this checkout so it is *this* htl that is embedded, then
//! built, tested and run — including, for the C ABI target, the reference callers in C and
//! Python that load the library it builds.
//!
//! The snapshot tests in `htl-cli` pin what the scaffold writes byte for byte; only this
//! says the bytes compile and work.
//!
//! # Where this came from, and what changed
//!
//! This was `_scaffold-hosts`, 87 lines of bash in the justfile, and it is the same work in
//! the same order against the same assertions. Three things are different.
//!
//! The first is that a failure names a file and a line. The second is that the three
//! targets are three tests: the script stopped at the first one that broke, and a red run said
//! nothing about the other two. The third is the reason the move was worth making at all —
//! that script carried a documented instance of a gate that silently passed. Its pin check
//! was once spelled `! grep …`, and bash exempts a command whose status is inverted with
//! `!` from `set -e`, so it reported nothing however the manifest looked. The Rust shape of
//! that bug is a checker that hands back a value nobody looks at, so [`unpatched_pin`] is a
//! pure function with two tests of its own below, and the only caller of it panics.
//!
//! # Two callers, one implementation
//!
//! The bash took five positional arguments because it had two callers. This takes five
//! environment variables for the same reason, each defaulting to this checkout:
//!
//! | Variable | Default |
//! | --- | --- |
//! | `HTL_TEST_BIN` | `cargo build -p htl-cli --bin htl`, whatever path that reports |
//! | `HTL_PATCH_HTL` | `crates/htl` |
//! | `HTL_PATCH_CORE` | `crates/htl-core` |
//! | `HTL_PATCH_MACROS` | `crates/htl-macros` |
//! | `HTL_E2E_TARGET` | `<cargo's target directory>/e2e-scaffold` |
//!
//! `just e2e` sets none of them. `just e2e-scaffold-packaged` sets all five, at a CLI
//! installed out of a `.crate` tarball and at the three extracted trees beside it, and that
//! is the whole of the difference between the two gates — as it was when it was the
//! difference between two argument lists.
//!
//! The binary is *located*, never assembled out of pieces. The recipe this replaces built
//! the path as `"${CARGO_TARGET_DIR:-$root/target}/debug/htl"`, which is wrong under a
//! `[build] target-dir` in `.cargo/config.toml` and wrong under any `--target`: it would
//! have named a file that does not exist, or — worse — a stale one left by an earlier
//! layout. Cargo is asked instead, and answers with the path it just wrote.
//!
//! # Nested cargo
//!
//! Everything here spawns cargo from inside a process cargo spawned, and a child inherits
//! the parent's environment. [`cargo`] is the one place a child is constructed, and it
//! removes every `CARGO_*` variable except two. What that drops and why it keeps those two
//! is written there.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};

// ---------------------------------------------------------------------------------------
// Where things are
// ---------------------------------------------------------------------------------------

/// The root of this workspace: this crate sits directly under it.
fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

/// A cargo to run, with the parent's cargo environment taken back off it.
///
/// `cargo test -p e2e` exports a good deal into this process, and every one of those
/// variables would otherwise reach the three builds below and mean something there. Cargo's
/// own `cargo-test-support` clears the whole `CARGO_*` space for exactly this reason; there
/// is no crate to borrow that from, so it is done here, by the same rule and with two
/// deliberate exceptions.
///
/// What is dropped, and what it would have done:
///
/// - **`CARGO_MAKEFLAGS`** — the jobserver. It names file descriptors held by the outer
///   cargo, which the grandchild builds do not inherit; passing it on gets a "jobserver
///   unavailable" warning at best and a wrong parallelism at worst. This is the one that
///   matters.
/// - **`CARGO_MANIFEST_DIR`, `CARGO_MANIFEST_PATH`, `CARGO_PKG_*`, `CARGO_CRATE_NAME`,
///   `CARGO_BIN_EXE_*`, `CARGO_PRIMARY_PACKAGE`** — this crate's identity, which build
///   scripts in the scaffolded projects' dependency graphs would read as their own.
/// - **`CARGO_ENCODED_RUSTFLAGS`** — set when cargo invokes a tool, and it *outranks*
///   `RUSTFLAGS`; leaving it would silently override the flags below.
///
/// What is kept, on purpose:
///
/// - **`CARGO_HOME`** — the registry cache and the downloaded sources. Clearing it would
///   make the child re-download the index.
/// - **`CARGO_TARGET_DIR`** — someone who has redirected their target directory meant it,
///   and the binary build below should land in the same place the outer build did rather
///   than compiling htl-cli a second time somewhere else. The three scaffolded projects
///   never see it: they are given `--target-dir` explicitly, which wins over it.
///
/// `RUSTFLAGS` is also kept, and is not a `CARGO_*` variable so it is kept by default. CI
/// sets `RUSTFLAGS: -D warnings` at the workflow level, so the scaffolded projects are
/// already built with it today; dropping it here would quietly make this gate ask less than
/// the one it replaces. `RUSTUP_TOOLCHAIN` is kept for the same reason — the child must be
/// the toolchain the matrix chose, not the default one.
///
/// `CARGO` itself (no underscore, so untouched by the rule) is what is *run*, when it is
/// set: it is the absolute path to the cargo currently executing, which is how the child
/// ends up on the same toolchain even where `RUSTUP_TOOLCHAIN` is not in play.
fn cargo() -> Command {
    let mut cmd =
        Command::new(std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")));
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        let keep = name == "CARGO_HOME" || name == "CARGO_TARGET_DIR";
        if name.starts_with("CARGO_") && !keep {
            cmd.env_remove(&key);
        }
    }
    cmd
}

/// The `htl` that writes the projects: `HTL_TEST_BIN` when it is set, otherwise the one
/// cargo builds and then names.
///
/// The fallback is what keeps the default honest — a `HTL_TEST_BIN` exported into a shell
/// and forgotten is a gate reporting on a binary that stopped matching the source, and the
/// only defence against that is that nothing has to set it.
fn htl_bin() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        if let Some(given) = std::env::var_os("HTL_TEST_BIN") {
            return PathBuf::from(given);
        }
        let out = cargo()
            .args([
                "build",
                "-p",
                "htl-cli",
                "--bin",
                "htl",
                "--message-format=json",
            ])
            .current_dir(workspace_root())
            .stderr(Stdio::inherit())
            .output()
            .expect("cargo build -p htl-cli could not be started");
        assert!(
            out.status.success(),
            "cargo build -p htl-cli: {}",
            out.status
        );
        // One JSON object per line. The interesting one is the `compiler-artifact` for the
        // `htl` bin target, and `executable` on it is the path cargo wrote — which is the
        // whole reason for asking rather than for guessing.
        let stdout = String::from_utf8_lossy(&out.stdout);
        let mut found = stdout.lines().filter_map(|line| {
            let msg: serde_json::Value = serde_json::from_str(line).ok()?;
            if msg["reason"] != "compiler-artifact" || msg["target"]["name"] != "htl" {
                return None;
            }
            Some(PathBuf::from(msg["executable"].as_str()?))
        });
        // The last such line, not the first: a target rebuilt in one run is reported once,
        // and if anything else ever answers to the name `htl` it is the artefact cargo
        // finished with that this should drive.
        found
            .next_back()
            .expect("cargo built htl-cli but reported no executable for the htl bin target")
    })
}

/// Where the scaffolded projects put their artefacts.
///
/// Beside the workspace's rather than in it: the scaffold sets
/// `[profile.dev.build-override]`, and sharing one target directory would rebuild the proc
/// macro dependencies on every switch between this and an ordinary `cargo build`.
///
/// Cargo is asked where its target directory is rather than told, for the same reason the
/// binary above is located rather than assembled: `CARGO_TARGET_DIR` is only one of the
/// ways it gets set, and `[build] target-dir` is another.
fn cargo_target_dir() -> &'static Path {
    static TARGET: OnceLock<PathBuf> = OnceLock::new();
    TARGET.get_or_init(|| {
        if let Some(given) = std::env::var_os("HTL_E2E_TARGET") {
            return PathBuf::from(given);
        }
        let out = cargo()
            .args(["metadata", "--no-deps", "--format-version", "1"])
            .current_dir(workspace_root())
            .stderr(Stdio::inherit())
            .output()
            .expect("cargo metadata could not be started");
        assert!(out.status.success(), "cargo metadata: {}", out.status);
        let meta: serde_json::Value =
            serde_json::from_slice(&out.stdout).expect("cargo metadata did not answer in JSON");
        let dir = meta["target_directory"]
            .as_str()
            .expect("cargo metadata named no target_directory");
        PathBuf::from(dir).join("e2e-scaffold")
    })
}

/// One of the three trees the generated projects are built against: the variable when it is
/// set, this checkout otherwise.
fn patch_path(var: &str, crate_dir: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join(crate_dir))
}

/// The arguments every cargo command inside a scaffolded project takes.
///
/// The patch is handed to cargo through `--config`, off to one side, so that a generated
/// `Cargo.toml` stays byte for byte the one a user gets — which is what
/// [`assert_unpatched_pin`] then checks.
fn scaffold_args() -> Vec<String> {
    let patch =
        |key: &str, path: PathBuf| format!("patch.crates-io.{key}.path='{}'", path.display());
    vec![
        "--config".into(),
        patch("htl", patch_path("HTL_PATCH_HTL", "crates/htl")),
        "--config".into(),
        patch("htl-core", patch_path("HTL_PATCH_CORE", "crates/htl-core")),
        "--config".into(),
        patch(
            "htl-macros",
            patch_path("HTL_PATCH_MACROS", "crates/htl-macros"),
        ),
        "--target-dir".into(),
        cargo_target_dir().display().to_string(),
    ]
}

// ---------------------------------------------------------------------------------------
// Running things
// ---------------------------------------------------------------------------------------

/// The three tests share one target directory, so only one of them can be compiling at a
/// time whatever the harness does — cargo's own lock on that directory would serialise them
/// anyway, printing "Blocking waiting for file lock" at whoever lost. Taking it here makes
/// that explicit and keeps the output in one piece per target.
///
/// Giving each target a directory of its own is the alternative, and it costs a full build
/// of mlua's vendored Lua per target to buy a parallelism the lock would not allow.
fn one_at_a_time() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    // A test that panicked while holding it has failed already; poisoning the rest on top
    // of that would replace two real failures with one confusing one.
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A fresh directory under the system temp dir. Outside the checkout on purpose:
/// scaffolding inside the repository would leave a Cargo package in it and run the CLI
/// against the repository's own configuration.
///
/// It is removed at the end of a test that passed, and left where it is by one that did
/// not — the tree a red gate was looking at is most of what there is to go on.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("htl-e2e-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    println!("scaffolding into {}", dir.display());
    dir
}

/// Run to completion with the output going where this process's went, and fail the test
/// naming the command if it did not succeed.
///
/// Inherited rather than captured because these are the multi-minute builds: their progress
/// is the only sign the test is alive, and no assertion below reads what they printed.
fn must_run(cmd: &mut Command, what: &str) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("{what}: could not be started: {e}"));
    assert!(status.success(), "{what}: {status}");
}

/// Run to completion capturing stdout, leaving stderr where it was, and return what was
/// captured. Printed as well as returned, so that the run a failing assertion judged is in
/// the report beside it.
fn must_capture(cmd: &mut Command, what: &str) -> String {
    let out = cmd
        .stderr(Stdio::inherit())
        .output()
        .unwrap_or_else(|e| panic!("{what}: could not be started: {e}"));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    println!("--- {what} ---\n{text}");
    assert!(out.status.success(), "{what}: {}\n{text}", out.status);
    text
}

/// `htl new` in the scratch directory, which is where the generated project lands. The
/// binary is invoked from there rather than from the checkout so that nothing above it in
/// the filesystem belongs to this repository.
fn htl_new(scratch: &Path, project: &str, target: &[&str]) -> PathBuf {
    let dir = scratch.join(project);
    must_run(
        Command::new(htl_bin())
            .arg("new")
            .arg(&dir)
            .args(target)
            .current_dir(scratch),
        &format!("htl new {project} {}", target.join(" ")),
    );
    dir
}

/// `cargo <args>` inside a scaffolded project, with the patch and the target directory on
/// it.
fn cargo_in(project: &Path, args: &[&str]) -> Command {
    let mut cmd = cargo();
    cmd.args(args).args(scaffold_args()).current_dir(project);
    cmd
}

// ---------------------------------------------------------------------------------------
// The manifest assertion
// ---------------------------------------------------------------------------------------

/// What each generated project holds, and must keep holding whoever built it: a pin on the
/// released htl, with nothing in the manifest redirecting it.
///
/// A pure function returning the reason, so that the two tests below can watch it say no.
/// That is not ceremony: the bash this replaces spelled the same two checks as `! grep …`,
/// which `set -e` exempts, and so it reported nothing however the manifest looked. An
/// assertion that cannot fail is worse than no assertion, because it is also a claim that
/// something is being checked.
fn unpatched_pin(manifest: &str) -> Result<(), &'static str> {
    if !manifest.lines().any(|line| line.starts_with("htl = ")) {
        return Err("names no htl to build against");
    }
    if manifest.contains("patch.crates-io") {
        return Err("redirects its own pin, so this is not the manifest a user gets");
    }
    Ok(())
}

/// [`unpatched_pin`] on a project's `Cargo.toml`, with the manifest in the failure so that
/// a red run says what it was looking at.
fn assert_unpatched_pin(project: &Path) {
    let path = project.join("Cargo.toml");
    let manifest = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    if let Err(why) = unpatched_pin(&manifest) {
        panic!("{}: {why}\n--- Cargo.toml ---\n{manifest}", path.display());
    }
}

/// A pin in the shape the `cdylib` target writes, so that the two cases below are refusing
/// something a passing manifest otherwise resembles. The two `bin` cases write the same
/// line without the features table (`htl = "0.4"`); what [`unpatched_pin`] reads is the
/// `htl = ` that starts it, which both forms have.
const PINNED: &str = "[dependencies]\nhtl = { version = \"0.4\", features = [\"ffi\"] }\n";

#[test]
fn a_manifest_naming_no_htl_is_refused() {
    assert_eq!(
        unpatched_pin("[dependencies]\nanyhow = \"1\"\n"),
        Err("names no htl to build against")
    );
    assert_eq!(unpatched_pin(PINNED), Ok(()));
}

#[test]
fn a_manifest_redirecting_its_own_pin_is_refused() {
    let patched = format!("{PINNED}\n[patch.crates-io]\nhtl = {{ path = \"../..\" }}\n");
    assert!(unpatched_pin(&patched).is_err());
}

// ---------------------------------------------------------------------------------------
// The three targets
// ---------------------------------------------------------------------------------------

/// The default target: a binary, its Teal module, and a test suite, built and then run with
/// an argument.
#[test]
fn the_bin_target_builds_tests_and_greets() {
    let _lock = one_at_a_time();
    let scratch = scratch("rustsample");
    let project = htl_new(&scratch, "rustsample", &["--target", "bin"]);

    assert_unpatched_pin(&project);
    must_run(&mut cargo_in(&project, &["test"]), "cargo test");

    let out = must_capture(
        cargo_in(&project, &["run", "-q"]).arg("--").arg("Ada"),
        "cargo run -- Ada",
    );
    assert!(
        out.lines().any(|line| line == "hello from Rust, Ada"),
        "the binary greets the argument it was given:\n{out}"
    );

    fs::remove_dir_all(&scratch).ok();
}

/// The same target without a binary: the library still builds and its test still goes
/// through preload, and there is no entry point for `cargo run` to find.
#[test]
fn the_bin_library_target_has_no_entry_point_and_still_tests() {
    let _lock = one_at_a_time();
    let scratch = scratch("libsample");
    let project = htl_new(&scratch, "libsample", &["--target", "bin", "--lib"]);

    assert_no_entry_point(&project);
    assert_unpatched_pin(&project);
    must_run(&mut cargo_in(&project, &["test"]), "cargo test");

    fs::remove_dir_all(&scratch).ok();
}

/// The C ABI target, whose callers are the part nothing else here compiles: the library is
/// built, the header the macro writes is checked, and the two reference callers under
/// `examples/` are run against the artefact — the Python one wherever python3 is, the C one
/// only where there is a compiler and a make.
#[test]
fn the_cdylib_target_builds_a_library_its_c_and_python_callers_can_load() {
    let _lock = one_at_a_time();
    let scratch = scratch("ffisample");
    let project = htl_new(&scratch, "ffisample", &["--target", "cdylib", "--lib"]);
    let target_dir = cargo_target_dir();

    assert_no_entry_point(&project);
    // The same pin, with the feature the profile needs on it — `htl = { version = "0.4",
    // features = ["ffi"] }` is still one `htl =` line naming the release, and still nothing
    // patches it here.
    assert_unpatched_pin(&project);
    must_run(&mut cargo_in(&project, &["test"]), "cargo test");
    must_run(&mut cargo_in(&project, &["build"]), "cargo build");

    // Written by #[c_export] at build time, not by the scaffold: it is not there until the
    // library is built, and then it declares what the callers below call.
    let header = project.join("include/ffisample.h");
    let declared =
        fs::read_to_string(&header).unwrap_or_else(|e| panic!("{}: {e}", header.display()));
    assert!(
        declared.contains("ffisample_handle *ffisample_open(const char *options_json);"),
        "the header declares the entry point the callers open:\n{declared}"
    );
    let archive = target_dir.join("debug/libffisample.a");
    assert!(
        archive.is_file(),
        "the static library is where cargo was told to put it: {}",
        archive.display()
    );

    if which("python3") {
        // run.py finds the build through CARGO_TARGET_DIR, the same way the recipe told it
        // to. It is a ctypes host and spawns no cargo of its own.
        let out = must_capture(
            Command::new("python3")
                .arg("examples/python/run.py")
                .env("CARGO_TARGET_DIR", target_dir)
                .current_dir(&project),
            "python3 examples/python/run.py",
        );
        assert!(
            out.contains("greet          -> the Python host: hello, Ada"),
            "the Teal module answers through the C ABI:\n{out}"
        );
        assert!(out.contains("schema v1"), "and reports its schema:\n{out}");
        // The Lua error the Teal module raises, as a status rather than as a crash.
        assert!(
            out.contains("greet(\"\")      -> NULL, status 4"),
            "a raise comes back as a status, not as a crash:\n{out}"
        );
    } else {
        println!("no python3: skipping examples/python");
    }

    if which("cc") && which("make") {
        let out = must_capture(
            Command::new("make")
                .args(["-s", "-C", "examples/c", "run"])
                .arg(format!("LIBDIR={}", target_dir.join("debug").display()))
                .current_dir(&project),
            "make -C examples/c run",
        );
        assert!(
            out.contains(r#"{"greeted":1,"greeter":"the C host","v":1}"#),
            "the C caller gets its state back as JSON:\n{out}"
        );
        assert!(
            out.contains("reset again    -> status 1"),
            "and a second reset is reported rather than repeated:\n{out}"
        );
    } else {
        println!("no C compiler: skipping examples/c");
    }

    fs::remove_dir_all(&scratch).ok();
}

/// `--lib` means no binary, and the scaffold writes neither half of one.
fn assert_no_entry_point(project: &Path) {
    for absent in ["src/main.rs", "src/main.tl"] {
        let path = project.join(absent);
        assert!(
            !path.exists(),
            "--lib writes no entry point, but {} is there",
            path.display()
        );
    }
}

/// Whether a program is on PATH. The two callers below are skipped rather than failed where
/// their toolchain is absent, which is what the recipe did and why CI is the place those
/// two cases are known to have run.
fn which(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
