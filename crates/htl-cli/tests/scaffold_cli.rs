//! `htl new --embed` through the real binary: what the Rust host it writes declares
//! and does, and the `--format` help of the commands whose text form is a report.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-scaffold", name)
}

fn htl(args: &[&str], cwd: &Path) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_htl"))
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `"0.2"` for a 0.2.x htl, `"1"` for 1.x: what cargo's caret rule reads as the
/// running release and its compatible updates. Computed here a second time so the
/// template cannot drift from the crate version without this failing.
fn expected_dep() -> String {
    let v = env!("CARGO_PKG_VERSION");
    let mut it = v.split('.');
    let major = it.next().unwrap();
    if major == "0" {
        format!("0.{}", it.next().unwrap())
    } else {
        major.to_string()
    }
}

#[test]
fn embed_scaffold_depends_on_the_htl_that_wrote_it() {
    let root = scratch("dep");
    let (ok, _, stderr) = htl(&["new", "sample", "--embed"], &root);
    assert!(ok, "{stderr}");
    let cargo = std::fs::read_to_string(root.join("sample/Cargo.toml")).unwrap();
    let want = format!("htl = \"{}\"", expected_dep());
    assert!(cargo.contains(&want), "want {want} in:\n{cargo}");
}

#[test]
fn embed_scaffold_fills_arg_before_running_main() {
    let root = scratch("arg");
    let (ok, _, stderr) = htl(&["new", "sample", "--embed"], &root);
    assert!(ok, "{stderr}");
    let main_rs = std::fs::read_to_string(root.join("sample/src/main.rs")).unwrap();
    let main_tl = std::fs::read_to_string(root.join("sample/src/main.tl")).unwrap();
    assert!(
        main_tl.contains("arg[1]"),
        "the script reads arg:\n{main_tl}"
    );
    let set = main_rs
        .find("h.set_arg(\"main.tl\", &args)?")
        .expect("set_arg call");
    let exec = main_rs
        .find("h.exec(MAIN, \"=main.tl\", &args)?")
        .expect("exec call");
    assert!(set < exec, "set_arg comes before exec:\n{main_rs}");
}

#[test]
fn lib_embed_scaffold_has_no_script_to_pass_args_to() {
    let root = scratch("lib");
    let (ok, _, stderr) = htl(&["new", "sample", "--embed", "--lib"], &root);
    assert!(ok, "{stderr}");
    let main_rs = std::fs::read_to_string(root.join("sample/src/main.rs")).unwrap();
    assert!(!main_rs.contains("set_arg"), "{main_rs}");
    assert!(!root.join("sample/src/main.tl").exists());
}

#[test]
fn format_help_does_not_promise_stderr_for_report_commands() {
    let root = scratch("help");
    for cmd in [
        &["bundle", "info", "--help"][..],
        &["cache", "status", "--help"][..],
    ] {
        let (ok, stdout, _) = htl(cmd, &root);
        assert!(ok);
        assert!(
            stdout.contains("Human-readable lines"),
            "{cmd:?}:\n{stdout}"
        );
        assert!(
            !stdout.contains("stderr"),
            "{cmd:?} promises stderr:\n{stdout}"
        );
    }
    // Where the stream matters, the help still sends the reader to the README.
    let (_, stdout, _) = htl(&["check", "--help"], &root);
    assert!(stdout.contains("Machine-readable output"), "{stdout}");
}
