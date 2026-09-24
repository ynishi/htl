//! `htl fix` and `htl check` judge one tree the same way.
//!
//! Each case is a tree where nothing is left to fix. What `htl fix` finds there is exactly
//! what `htl check` finds: the file's own diagnostics, this layer's lints, the findings
//! about the project as a whole. Both judge them with `htl_core::verdict`, so the two exit
//! the same. Before, fix judged on its own errors alone: a name with two owners, a lint at
//! `deny` and `strict` all passed there and failed `htl check` (#316).

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-fix-verdict", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn htl(args: &[&str], cwd: &Path) -> (bool, String) {
    let out = Command::new(common::htl_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `htl check` and `htl fix` on `root`, and what each said.
fn both(root: &Path) -> ((bool, String), (bool, String)) {
    (
        htl(&["check", "--no-cache", "."], root),
        htl(&["fix", "--allow-no-vcs", "."], root),
    )
}

/// A project whose two files require each other: a `require-cycle` lint, which no fix
/// removes, and nothing else. `lint` is the `htl.toml` around it.
fn cycle(name: &str, lint: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), lint);
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"game\"\nversion = \"0.1.0\"\n",
    );
    write(
        &root.join("src/a.tl"),
        "local b = require(\"b\")\nreturn { b = b }\n",
    );
    write(
        &root.join("src/b.tl"),
        "local a = require(\"a\")\nreturn { a = a }\n",
    );
    root
}

#[test]
fn a_warn_lint_passes_both() {
    let root = cycle("warn", "");
    let ((check_ok, check), (fix_ok, fix)) = both(&root);
    assert!(check_ok, "{check}");
    assert!(fix_ok, "{fix}");
    assert!(
        fix.contains("[htl require-cycle]"),
        "fix says the project-level lint as check does: {fix}"
    );
}

#[test]
fn a_lint_at_deny_fails_both() {
    let root = cycle("deny", "[lint.rules]\nrequire-cycle = \"deny\"\n");
    let ((check_ok, check), (fix_ok, fix)) = both(&root);
    assert!(!check_ok, "{check}");
    assert!(!fix_ok, "the lint is at deny wherever it is judged: {fix}");
    assert!(fix.contains("0 error(s) remaining, 1 at deny"), "{fix}");
}

#[test]
fn strict_fails_both() {
    let root = cycle("strict", "[lint]\nstrict = true\n");
    let ((check_ok, check), (fix_ok, fix)) = both(&root);
    assert!(!check_ok, "{check}");
    assert!(!fix_ok, "{fix}");
    assert!(
        fix.contains("0 warning(s) and 1 lint(s) under strict"),
        "{fix}"
    );
}

/// A name the project and an installed dependency both implement: an error about the
/// project, which belongs to neither file.
#[test]
fn a_name_with_two_owners_fails_both() {
    let root = scratch("two-owners");
    write(&root.join("htl.toml"), "");
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"game\"\nversion = \"0.1.0\"\n",
    );
    write(
        &root.join(".htl/modules/entries/mathx/init.tl"),
        "return {}\n",
    );
    write(&root.join("src/mathx.tl"), "return {}\n");
    let ((check_ok, check), (fix_ok, fix)) = both(&root);
    assert!(!check_ok, "{check}");
    assert!(!fix_ok, "{fix}");
    assert!(
        fix.contains("module name 'mathx' has more than one owner"),
        "{fix}"
    );
}

/// `--format json` carries what the verdict counted, and the project-level findings at
/// the top, since no one file owns them.
#[test]
fn json_carries_the_verdict_and_the_project_findings() {
    let root = cycle("json", "[lint.rules]\nrequire-cycle = \"deny\"\n");
    let out = Command::new(common::htl_bin())
        .args(["fix", "--allow-no-vcs", "--format", "json", "."])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect(&stdout);
    let s = &v["summary"];
    assert_eq!(s["lints"], 1, "{stdout}");
    assert_eq!(s["denied"], 1, "{stdout}");
    assert_eq!(s["ok"], false, "{stdout}");
    let top = v["diagnostics"].as_array().expect(&stdout);
    assert!(
        top.iter().any(|d| d["rule"] == "require-cycle"),
        "the cycle is the project's, said at the top: {stdout}"
    );
}
