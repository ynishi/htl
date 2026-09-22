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
//! The first is that a failure names a file and a line. The second is that the four
//! targets are four tests: the script stopped at the first one that broke, and a red run said
//! nothing about the rest. The third is the reason the move was worth making at all —
//! that script carried a documented instance of a gate that silently passed. Its pin check
//! was once spelled `! grep …`, and bash exempts a command whose status is inverted with
//! `!` from `set -e`, so it reported nothing however the manifest looked. The Rust shape of
//! that bug is a checker that hands back a value nobody looks at, so [`unpatched_pin`] is a
//! pure function with two tests of its own below, and the only caller of it panics.
//!
//! # Two callers, one implementation
//!
//! The bash took five positional arguments because it had two callers. This takes six
//! environment variables for the same reason:
//!
//! | Variable | Unset |
//! | --- | --- |
//! | `HTL_TEST_BIN` | `cargo build -p htl-cli --bin htl`, whatever path that reports |
//! | `HTL_PATCH_HTL` | no `[patch]`: the project builds against what its own manifest pins |
//! | `HTL_PATCH_CORE` | likewise |
//! | `HTL_PATCH_MACROS` | likewise |
//! | `HTL_PATCH_MQ` | likewise |
//! | `HTL_E2E_TARGET` | `<cargo's target directory>/e2e-scaffold` |
//!
//! `just e2e` sets none of them: the CLI it builds is this checkout's, and a CLI built
//! from a checkout pins that checkout (`crates/htl-cli/build.rs`), so the project it
//! writes builds against this tree with nothing redirected — the manifest a contributor
//! gets from `cargo run -- new`. `just e2e-scaffold-packaged` sets all six, at a CLI
//! installed out of a `.crate` tarball and at the four extracted trees beside it: that
//! CLI pins its version, which is not on crates.io while the gate runs, and the patch is
//! what points the version at the tarballs. The patch variables — three, then — used to
//! default to this checkout, back when a checkout CLI pinned a release by number and the
//! patch was what made `e2e` build against the checkout at all.
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
/// variables would otherwise reach the four builds below and mean something there. Cargo's
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
///   than compiling htl-cli a second time somewhere else. The four scaffolded projects
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

/// The crates a caller can redirect, and the variable that redirects each. One list, read
/// by [`write_patch_config`]; the module doc above says what sets them and why.
const PATCHES: [(&str, &str); 4] = [
    ("htl", "HTL_PATCH_HTL"),
    ("htl-core", "HTL_PATCH_CORE"),
    ("htl-macros", "HTL_PATCH_MACROS"),
    ("htl-mq", "HTL_PATCH_MQ"),
];

/// The patch a caller asked for, written into the project as `.cargo/config.toml` — and so
/// seen by *every* cargo that runs there, not only by the ones this file spawns.
///
/// It was `--config` on those command lines until the window target arrived, and the
/// difference is that this is the first target where the binary under test spawns a cargo
/// of its own: `htl check` materialises a dependency's declaration and asks `cargo
/// metadata` where that dependency lives. Our command line is nowhere near that process.
/// Under the packaged gate the project pins `htl-mq = "<version>"`, which is not on
/// crates.io while the gate runs, so the resolve failed and both Teal files reported `mq`
/// as a module not found: a patch that never arrived, said as a module that is not there.
///
/// Still off to one side of the manifest, which is the property that mattered about
/// `--config`: the generated `Cargo.toml` stays byte for byte the one a user gets, and
/// [`assert_unpatched_pin`] reads that file and no other. With no `HTL_PATCH_*` set
/// nothing is written at all, which is every run of `just e2e` — there the CLI is this
/// checkout's and pins this checkout, so there is nothing to redirect.
///
/// The three targets that do not draw get the htl-mq line as well, and cargo says the
/// patch went unused. That warning is the one `e2e-scaffold-packaged` already accepts for
/// htl-macros when it installs the CLI, for the same reason: patching a crate nobody has
/// published is cheap, and the alternative to patching it is resolving it.
fn write_patch_config(project: &Path) {
    let mut patched = String::new();
    for (krate, var) in PATCHES {
        if let Some(path) = std::env::var_os(var) {
            patched.push_str(&format!(
                "{krate} = {{ path = '{}' }}\n",
                PathBuf::from(path).display()
            ));
        }
    }
    if patched.is_empty() {
        return;
    }
    let dir = project.join(".cargo");
    fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    let path = dir.join("config.toml");
    fs::write(&path, format!("[patch.crates-io]\n{patched}"))
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

/// The arguments every cargo command inside a scaffolded project takes. The patch is not
/// among them — see [`write_patch_config`] — and what is left is the one thing a config
/// file should not carry: a target directory shared with the other tests, which is a
/// property of this harness rather than of the project.
fn scaffold_args() -> Vec<String> {
    vec![
        "--target-dir".to_string(),
        cargo_target_dir().display().to_string(),
    ]
}

// ---------------------------------------------------------------------------------------
// Running things
// ---------------------------------------------------------------------------------------

/// The four tests share one target directory, so only one of them can be compiling at a
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
///
/// The patch goes in before the caller does anything with what was written, so that the
/// first cargo to run in the project sees it whoever started it — a test here, or the CLI
/// itself. A caller that had to remember to ask for it is a caller that forgets.
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
    write_patch_config(&dir);
    dir
}

/// `cargo <args>` inside a scaffolded project, with the shared target directory on it. The
/// patch is not passed here — it is in the project's own `.cargo/config.toml`, so that the
/// cargo `htl check` spawns is under it too ([`write_patch_config`]).
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
    if !pins(manifest, "htl") {
        return Err("names no htl to build against");
    }
    // The window target has a second pin, `htl-mq`, and it is held to the same rule: the
    // packaged gate patches that one too, so a generated manifest that redirected it would
    // be as wrong and as invisible. It is asked of a manifest that mentions the crate
    // rather than of every manifest, because the other three targets do not draw and do
    // not name it — and what that catches in the one that does is an htl-mq arriving as
    // anything but a dependency line, a `[dependencies.htl-mq]` table or a `[patch]` entry,
    // which the rule below would then not be reading.
    if manifest.contains("htl-mq") && !pins(manifest, "htl-mq") {
        return Err("names htl-mq without a pin to build it against");
    }
    if manifest.contains("patch.crates-io") {
        return Err("redirects its own pin, so this is not the manifest a user gets");
    }
    Ok(())
}

/// Whether `manifest` opens a line with a dependency on `krate`. The start of the line is
/// the whole of the test: `htl = "0.4"`, `htl = { version = "0.4", features = ["ffi"] }`
/// and `htl-mq = { path = "…" }` are all pins, and which of those a given caller gets is
/// the difference between the two gates rather than something to assert here.
fn pins(manifest: &str, krate: &str) -> bool {
    manifest
        .lines()
        .any(|line| line.starts_with(&format!("{krate} = ")))
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

/// The window target's pair, in the shape its manifest writes them.
const PINNED_WINDOW: &str = "[dependencies]\nhtl = \"0.7.0\"\nhtl-mq = \"0.7.0\"\n";

#[test]
fn a_manifest_naming_no_htl_is_refused() {
    assert_eq!(
        unpatched_pin("[dependencies]\nanyhow = \"1\"\n"),
        Err("names no htl to build against")
    );
    assert_eq!(unpatched_pin(PINNED), Ok(()));
    assert_eq!(unpatched_pin(PINNED_WINDOW), Ok(()));
}

#[test]
fn a_window_manifest_naming_no_htl_mq_pin_is_refused() {
    // The crate is named — a `[dependencies.htl-mq]` table is how a manifest mentions it
    // without a line the pin rule reads — and that is the case worth refusing, because a
    // window project without an htl-mq to build against does not compile at all while a
    // window project whose htl-mq went somewhere unread compiles and ships.
    let sectioned = format!("{PINNED}\n[dependencies.htl-mq]\nversion = \"0.7.0\"\n");
    assert_eq!(
        unpatched_pin(&sectioned),
        Err("names htl-mq without a pin to build it against")
    );
}

#[test]
fn a_manifest_redirecting_its_own_pin_is_refused() {
    let patched = format!("{PINNED}\n[patch.crates-io]\nhtl = {{ path = \"../..\" }}\n");
    assert!(unpatched_pin(&patched).is_err());
    // The window pair, with the second of the two redirected rather than the first.
    let mq_patched =
        format!("{PINNED_WINDOW}\n[patch.crates-io]\nhtl-mq = {{ path = \"../..\" }}\n");
    assert!(unpatched_pin(&mq_patched).is_err());
}

// ---------------------------------------------------------------------------------------
// The four targets
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

/// The window target, the one whose host is the OS. Four things happen here that no other
/// target asks for: `htl check` is run *before* anything is compiled, because it is what
/// writes the two declarations — one of them copied out of the dependency crate — and
/// `include_bundle!` links `mq` against that one; the Teal suite runs with no display at
/// all, which is the point of an engine that takes its world as arguments; the Rust test
/// reaches the same engine through `preload`; and then the binary is run for thirty frames
/// under a virtual X server, the only case in this file that needs a screen.
#[test]
fn the_window_target_builds_tests_and_draws_a_frame() {
    let _lock = one_at_a_time();
    let scratch = scratch("winsample");
    let project = htl_new(&scratch, "winsample", &["--target", "window"]);

    // Two pins here, `htl` and `htl-mq`, and [`unpatched_pin`] holds both to one rule.
    assert_unpatched_pin(&project);

    // Not `cargo` yet. `htl check` writes `types/htl-mq/mq.d.tl`, which is htl-mq's own
    // declaration copied in from the crate, and `src/fx.d.tl`, which is generated from
    // this project's `#[host_module]`. `include_bundle!` links `mq` against the first, so
    // `cargo test` below cannot compile until this has run — which is why the scaffold's
    // README and the CLI's `next:` hint both put it ahead of the build.
    must_run(
        Command::new(htl_bin())
            .args(["check", "."])
            .current_dir(&project),
        "htl check .",
    );
    for declared in ["types/htl-mq/mq.d.tl", "src/fx.d.tl"] {
        let path = project.join(declared);
        assert!(
            path.is_file(),
            "htl check writes the declarations, but {} is not there",
            path.display()
        );
    }

    // The engine's own suite. It never calls `render`, and `require("mq")` resolves to the
    // declaration, which declares and does nothing — so this needs no window.
    must_run(
        Command::new(htl_bin())
            .args(["test", "."])
            .current_dir(&project),
        "htl test .",
    );

    // The Rust side: the library's test loads the engine through `preload`, the way the
    // loop does, and this is also where the binary gets compiled.
    must_run(&mut cargo_in(&project, &["test"]), "cargo test");

    if which("xvfb-run") {
        // `HTL_MQ_FRAMES` stops the loop after a count and `HTL_MQ_SHOT` writes the last
        // frame drawn: htl-mq's answer to a run nobody is watching, and the only way this
        // can assert that something was drawn rather than that something compiled. Without
        // the two it opens a window and waits for Escape.
        let mut run = cargo_in(&project, &["run", "-q"]);
        run.env("HTL_MQ_FRAMES", "30").env("HTL_MQ_SHOT", "out.png");
        must_run(&mut under_xvfb(run), "xvfb-run cargo run");

        let shot = project.join("out.png");
        let bytes = fs::read(&shot).unwrap_or_else(|e| panic!("{}: {e}", shot.display()));
        assert!(
            bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            "the frame it wrote is a PNG, but {} starts {:?}",
            shot.display(),
            &bytes[..bytes.len().min(8)]
        );
        // The one line this case prints when it passes: a gate whose whole output is
        // silence is one nobody can tell from a gate that was skipped.
        println!(
            "{}: {} bytes of PNG after 30 frames",
            shot.display(),
            bytes.len()
        );
    } else {
        // Skipped rather than failed, the way the C caller is where there is no compiler.
        // CI's ubuntu-latest image carries xvfb, so that is where this case is known to
        // have run.
        println!("no xvfb-run: skipping the frame run (CI's ubuntu image has it)");
    }

    fs::remove_dir_all(&scratch).ok();
}

/// A command moved onto an `xvfb-run` that will supply it a display. macroquad opens a GL
/// context on an X server, and neither CI nor a headless checkout has one; `-a` takes a
/// free server number instead of colliding with a real session, and `-s` gives the virtual
/// one the size and depth the scaffolded window asks for.
///
/// It takes a built [`Command`] apart rather than being told what to run, because the
/// thing that matters about the command it wraps is not its arguments but its
/// *environment*: [`cargo`] removes the outer cargo's `CARGO_*` variables, and a wrapper
/// that rebuilt the invocation from the program name up would hand the child every one of
/// them back. `get_envs` reports those removals as `None`, and they are re-applied here as
/// removals.
fn under_xvfb(cmd: Command) -> Command {
    let mut wrapped = Command::new("xvfb-run");
    wrapped.args(["-a", "-s", "-screen 0 800x600x24"]);
    wrapped.arg(cmd.get_program()).args(cmd.get_args());
    for (key, value) in cmd.get_envs() {
        match value {
            Some(value) => wrapped.env(key, value),
            None => wrapped.env_remove(key),
        };
    }
    if let Some(dir) = cmd.get_current_dir() {
        wrapped.current_dir(dir);
    }
    wrapped
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

/// Whether a program is on PATH. The cases that ask are skipped rather than failed where
/// their toolchain is absent, which is what the recipe did and why CI is the place those
/// cases are known to have run.
///
/// Two flags, not one. `--version` is what `cc`, `make` and `python3` answer; `xvfb-run`
/// rejects it — "unrecognized option", status 1 — and answers `--help` instead. Asking
/// only the first would have reported a program that is installed as absent, and the case
/// that depends on it does not then go red: it prints that it was skipped and passes, on
/// every machine, for ever. A missing program fails to spawn at all, so both probes
/// answer `Err` and nothing here mistakes one for the other.
fn which(program: &str) -> bool {
    ["--version", "--help"].into_iter().any(|flag| {
        Command::new(program)
            .arg(flag)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}
