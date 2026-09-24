//! `htl build` judges a bundle's closure as `htl check` judges it, and `htl run` / `htl gen`
//! judge on errors alone — one predicate (`htl_core::verdict`), and the difference is one
//! named policy (`Policy::ERRORS_ONLY`), not a command that never asked (#316).

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-build-verdict", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn htl(args: &[&str], cwd: &Path) -> (bool, String, String) {
    let out = Command::new(common::htl_bin())
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

/// A project whose entry has one `nil-index` lint and prints `ran`. `toml` is its
/// `htl.toml`.
fn project(name: &str, toml: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), toml);
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"game\"\nversion = \"0.1.0\"\n",
    );
    write(
        &root.join("src/main.tl"),
        "local names: {string:string} = {}\nif names[\"a\"] then print(names[\"a\"]:upper()) end\nprint(\"ran\")\n",
    );
    root
}

fn build(root: &Path) -> (bool, String) {
    let (ok, _, err) = htl(
        &["build", "--no-cache", "src/main.tl", "-o", "out.hb"],
        root,
    );
    (ok, err)
}

#[test]
fn a_lint_at_warn_is_reported_and_the_bundle_is_written() {
    let root = project("warn", "");
    let (ok, err) = build(&root);
    assert!(ok, "{err}");
    assert!(err.contains("[htl nil-index]"), "{err}");
    assert!(root.join("out.hb").is_file());
}

#[test]
fn a_rule_the_project_turned_off_is_not_reported() {
    let root = project("allow", "[lint.rules]\nnil-index = \"allow\"\n");
    let (ok, err) = build(&root);
    assert!(ok, "{err}");
    assert!(
        !err.contains("nil-index"),
        "the build reports what `htl check` reports, and the project turned this off: {err}"
    );
}

#[test]
fn a_rule_at_deny_stops_the_bundle_as_it_stops_the_check() {
    let root = project("deny", "[lint.rules]\nnil-index = \"deny\"\n");
    let (check_ok, _, _) = htl(&["check", "--no-cache", "."], &root);
    let (ok, err) = build(&root);
    assert!(!check_ok);
    assert!(!ok, "{err}");
    assert!(
        err.contains("htl build: 0 error(s), 1 at deny, bundle not written"),
        "{err}"
    );
    assert!(!root.join("out.hb").exists());
}

#[test]
fn strict_stops_the_bundle() {
    let root = project("strict", "[lint]\nstrict = true\n");
    let (ok, err) = build(&root);
    assert!(!ok, "{err}");
    assert!(err.contains("1 lint(s) under strict"), "{err}");
}

/// `htl run`'s verdict is its program: a lint at `deny` is reported and the script runs.
#[test]
fn run_reports_a_lint_at_deny_and_runs() {
    let root = project("run", "[lint.rules]\nnil-index = \"deny\"\n");
    let (ok, out, err) = htl(&["run", "src/main.tl"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("[htl nil-index]"), "{err}");
    assert!(out.contains("ran"), "{out}");
}
