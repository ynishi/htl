//! `[toolchain] htl` through the real binary: a project says which `htl` command it
//! expects, and one outside that requirement is refused before it reads anything.
//!
//! The unit of the feature is the refusal, so these run the command rather than the
//! comparison — what a project gets is an exit code and a message, and both are here.
//! The comparison itself, and what a malformed requirement does to parsing, are in
//! `htl-core`'s `tests/config.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-toolchain", name)
}

fn htl(args: &[&str], cwd: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_htl"))
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// This CLI's own version — the one the pin is compared against.
const RUNNING: &str = env!("CARGO_PKG_VERSION");

/// The requirement this release satisfies, in the form the scaffold writes: `"0.3"` for
/// 0.3.x, `"1"` for 1.x. Derived here rather than hard-coded so a bump does not turn
/// these into a release chore.
fn satisfied_req() -> String {
    let mut it = RUNNING.split('.');
    let major = it.next().unwrap();
    if major == "0" {
        format!("0.{}", it.next().unwrap())
    } else {
        major.to_string()
    }
}

/// A requirement no build of htl satisfies: one major above this one.
fn violated_req() -> String {
    let major: u64 = RUNNING.split('.').next().unwrap().parse().unwrap();
    format!("{}", major + 1000)
}

/// A project with one module that checks clean, and whatever `htl.toml` the caller wants.
fn project(name: &str, toml: &str) -> PathBuf {
    let root = scratch(name);
    std::fs::write(root.join("htl.toml"), toml).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src").join("greet.tl"),
        "local greet = {}\n\nfunction greet.hello(who: string): string\n   return \"hi \" .. who\nend\n\nreturn greet\n",
    )
    .unwrap();
    root
}

#[test]
fn a_satisfied_requirement_says_nothing_new() {
    let req = satisfied_req();
    let pinned = project("satisfied", &format!("[toolchain]\nhtl = \"{req}\"\n"));
    let bare = project("satisfied-bare", "[fmt]\nindent = 3\n");

    let (ok, pinned_err) = htl(&["check", ".", "--no-cache"], &pinned);
    assert!(ok, "pinned project should check:\n{pinned_err}");
    let (ok, bare_err) = htl(&["check", ".", "--no-cache"], &bare);
    assert!(ok, "unpinned project should check:\n{bare_err}");

    // Not merely "no error": the same output as the project without the key, so that a
    // satisfied pin costs a reader nothing at all.
    assert_eq!(
        pinned_err, bare_err,
        "a satisfied pin changed the output of htl check"
    );
}

/// Absent key, today's behaviour — including the case that used to be the only one, a
/// project whose config carries no `[toolchain]` table at all.
#[test]
fn no_toolchain_key_runs_whatever_command_is_there() {
    let root = project("absent", "[lint]\nstrict = true\n");
    let (ok, err) = htl(&["check", ".", "--no-cache"], &root);
    assert!(ok, "{err}");
    assert!(
        !err.contains("toolchain"),
        "a project with no pin was told about one:\n{err}"
    );
}

/// The refusal: both versions, the file, and what to do about it.
#[test]
fn a_violated_requirement_is_refused_naming_both_versions_and_the_file() {
    let req = violated_req();
    let root = project("violated", &format!("[toolchain]\nhtl = \"{req}\"\n"));
    let (ok, err) = htl(&["check", ".", "--no-cache"], &root);
    assert!(!ok, "a violated pin was not refused:\n{err}");
    assert!(
        err.contains(RUNNING),
        "the running version is missing:\n{err}"
    );
    assert!(
        err.contains(&format!("[toolchain] htl = \"{req}\"")),
        "the requirement is missing:\n{err}"
    );
    assert!(
        err.contains("htl.toml"),
        "the config file is missing:\n{err}"
    );
    assert!(
        err.contains("cargo install htl-cli"),
        "the message does not say how to get one:\n{err}"
    );
}

/// Before any file is read: the module below is one the checker would report on, and the
/// refusal happens instead of that. A run that reported the Teal error would be one that
/// had already decided the pin did not matter.
#[test]
fn the_refusal_comes_before_the_sources_are_read() {
    let req = violated_req();
    let root = project("violated-early", &format!("[toolchain]\nhtl = \"{req}\"\n"));
    std::fs::write(
        root.join("src").join("broken.tl"),
        "local x: integer = \"not an integer\"\nreturn x\n",
    )
    .unwrap();
    let (ok, err) = htl(&["check", ".", "--no-cache"], &root);
    assert!(!ok, "{err}");
    assert!(
        !err.contains("broken.tl"),
        "the sources were checked despite the pin:\n{err}"
    );
}

/// Every command that reads the config, not only `htl check`. `htl fmt` and `htl test`
/// each load it through the same function, and this is what says so from outside.
#[test]
fn the_other_commands_that_read_the_config_are_refused_too() {
    let req = violated_req();
    let root = project(
        "violated-commands",
        &format!("[toolchain]\nhtl = \"{req}\"\n"),
    );
    for args in [
        vec!["fmt", ".", "--check"],
        vec!["test", "."],
        vec!["dts", "."],
    ] {
        let (ok, err) = htl(&args, &root);
        assert!(!ok, "htl {args:?} ran with a violated pin:\n{err}");
        assert!(
            err.contains("does not satisfy the toolchain"),
            "htl {args:?} failed for another reason:\n{err}"
        );
    }
}

/// A malformed requirement is a config error like any other — reported by the command
/// that read the file, not swallowed into "no pin".
#[test]
fn a_malformed_requirement_is_a_config_error() {
    let root = project("malformed", "[toolchain]\nhtl = \"not a requirement\"\n");
    let (ok, err) = htl(&["check", ".", "--no-cache"], &root);
    assert!(!ok, "a malformed requirement was accepted:\n{err}");
    assert!(
        err.contains("[toolchain] htl = \"not a requirement\"")
            && err.contains("is not a version requirement"),
        "unhelpful message for a malformed requirement:\n{err}"
    );
}

/// The scaffold's answer, end to end: the project `htl new` wrote checks with the command
/// that wrote it.
///
/// It does so *without* `[toolchain]`, and that absence is the assertion. A host project
/// pins the released `htl` crate, whose `HtlConfig` is `deny_unknown_fields`; a key this
/// workspace has and no release carries yet is not ignored there but fatal, inside
/// `include_tl!`, at the project's first `cargo build`. So the scaffold may only write
/// configuration the version it pins can read, and a new key waits a release; the rule and
/// the keys owed by it are on `scaffold::t_htl_toml`. `[toolchain]` itself keeps working
/// for a project that writes it by hand against an htl that knows it — the tests above are
/// that half.
#[test]
fn a_scaffolded_project_checks_with_the_cli_that_wrote_it() {
    let root = scratch("scaffold-roundtrip");
    let (ok, err) = htl(&["new", "sample"], &root);
    assert!(ok, "htl new failed:\n{err}");
    let dir = root.join("sample");

    let written = std::fs::read_to_string(dir.join("htl.toml")).unwrap();
    assert!(
        !written.contains("[toolchain]"),
        "htl new wrote a key the htl it pins cannot read:\n{written}"
    );

    let (ok, err) = htl(&["check", ".", "--no-cache"], &dir);
    assert!(ok, "the scaffolded project did not check:\n{err}");
}
