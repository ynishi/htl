//! A patched dependency through the real binary: which verbs read `patches/<dep>/` and
//! which leave it alone.
//!
//! The copy is the project's code — `htl check` reads it and names the dependency it
//! stands for — but it is also a diff against the revision it came from, so `htl fmt` does
//! not rewrite it and `htl test` does not run the dependency's suite as the project's.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-pkgpatch", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
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

fn git(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "user.email=htl@example.invalid",
            "-c",
            "user.name=htl",
        ])
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git on PATH");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A project that already holds a patched dependency, written out rather than fetched: the
/// manifest names the directory, and the copy has a source, a test of its own and a file
/// that is not formatted the way `htl fmt` would write it.
fn patched_project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n[deps.mathx]\n\
         git = \"https://example.invalid/mathx\"\nrev = \"abc\"\npatch_dir = \"patches/mathx\"\n",
    );
    write(&root.join("src/main.tl"), "local x: number = 1\nprint(x)\n");
    write(
        &root.join("patches/mathx/src/mathx.tl"),
        "local mathx = {}\n        function mathx.twice(n: number): number\n   return n * 2\nend\nreturn mathx\n",
    );
    write(
        &root.join("patches/mathx/tests/mathx_test.tl"),
        "local t = require(\"htl.test\")\nt.it(\"the dep's own suite\", function()\n   t.expect(1):to_equal(2)\nend)\n",
    );
    root
}

#[test]
fn check_reads_the_patched_copy_and_names_the_dependency() {
    let root = patched_project("check");
    let (ok, _, err) = htl(&["check"], &root);
    assert!(ok, "{err}");
    assert!(
        err.contains("patched patches/mathx (mathx)"),
        "the run says which dependency the directory stands in for: {err}"
    );
    assert!(
        err.contains("3 file(s)"),
        "the copy's sources are checked with the project's own: {err}"
    );
}

#[test]
fn fmt_leaves_the_patched_copy_alone() {
    let root = patched_project("fmt");
    let (ok, _, err) = htl(&["fmt", "--check"], &root);
    assert!(
        ok,
        "the copy is indented the way its author left it, and that is not this project's \
         to change: {err}"
    );
    assert!(err.contains("1 file(s)"), "only the project's own: {err}");
}

/// The copy's `*_test.tl` is the dependency's suite. A project whose own `tests/` is empty
/// has no tests, and says so — rather than reporting a library's failing case as its own.
#[test]
fn test_finds_nothing_when_the_only_suite_is_the_dependencys() {
    let root = patched_project("test");
    let (_, _, err) = htl(&["test"], &root);
    assert!(err.contains("no test files found"), "{err}");
    assert!(
        !err.contains("the dep's own suite"),
        "the dependency's failing case was not run: {err}"
    );
}

/// `htl pkg patch <dep>` end to end: a dependency on disk, taken into the tree.
#[test]
fn patch_reports_where_the_copy_is() {
    let dep = scratch("remote");
    write(&dep.join("src/mathx.tl"), "return {}\n");
    git(&dep, &["init", "-q"]);
    git(&dep, &["add", "."]);
    git(&dep, &["commit", "-qm", "mathx"]);
    let sha = git(&dep, &["rev-parse", "HEAD"]);

    let root = scratch("take");
    write(
        &root.join("mlua-pkg.toml"),
        &format!(
            "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n[deps.mathx]\n\
             git = \"file://{}\"\nrev = \"{sha}\"\n",
            dep.display()
        ),
    );

    let (ok, _, err) = htl(&["pkg", "patch", "mathx"], &root);
    assert!(ok, "{err}");
    assert!(
        err.contains(&format!("patched patches/mathx (mathx at {})", &sha[..7])),
        "{err}"
    );
    assert!(root.join("patches/mathx/src/mathx.tl").is_file(), "{err}");
    assert!(
        std::fs::read_to_string(root.join("mlua-pkg.toml"))
            .unwrap()
            .contains("patch_dir = \"patches/mathx\""),
        "{err}"
    );
}
