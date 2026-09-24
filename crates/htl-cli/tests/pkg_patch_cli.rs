//! A patched dependency through the real binary: which verbs read `patches/<dep>/` and
//! which leave it alone.
//!
//! The copy is the project's code — `htl check` reads it and names the dependency it
//! stands for — but it is also a diff against the revision it came from, so `htl fmt` and
//! `htl fix` do not rewrite it and `htl test` does not run the dependency's suite as the
//! project's.

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
    // One of the project's own and two of the dependency's — added up, the three files the
    // walk visited, and split so that the jump from one to three is accounted for.
    assert!(
        err.contains("htl check: 1 file(s) + 2 in patched dependencies,"),
        "the copy's sources are checked with the project's own, and counted apart: {err}"
    );
}

/// The same two numbers for a consumer: `files` keeps its meaning (everything the walk
/// visited) so that nobody reading it is moved by the new field, and `patched` is the part
/// of it the project did not write.
#[test]
fn json_carries_the_total_and_the_patched_part_of_it() {
    let root = patched_project("check-json");
    let (ok, out, err) = htl(&["check", "--format", "json"], &root);
    assert!(ok, "{err}");
    let v: serde_json::Value = serde_json::from_str(&out).expect(&out);
    assert_eq!(v["files"], 3, "{out}");
    assert_eq!(v["patched"], 2, "{out}");
}

/// The line a project with no patch gets is the line it always got: one count, no `+`.
/// The split is news only where there is something to split.
#[test]
fn a_project_without_a_patch_prints_the_line_it_always_did() {
    let root = scratch("unpatched");
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n",
    );
    write(&root.join("src/main.tl"), "local x: number = 1\nprint(x)\n");
    let (ok, _, err) = htl(&["check"], &root);
    assert!(ok, "{err}");
    assert!(
        err.contains("htl check: 1 file(s), 0 error(s), 0 warning(s), 0 lint(s)"),
        "the summary of an unpatched project changed: {err}"
    );
    assert!(
        !err.contains("patched"),
        "a project with no patch was told about one: {err}"
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

/// `htl fix` rewrites files, so it walks as `htl fmt` does. The copy holds an error that
/// carries a safe fix, and `htl check` offers it — which is what makes the copy left as it
/// was a decision rather than a fix that found nothing to do.
#[test]
fn fix_leaves_the_patched_copy_alone() {
    let root = patched_project("fix");
    let fwd = root.join("patches/mathx/src/fwd.tl");
    let text = "local record R\nend\nfunction R.a(): integer return R.b() end\n\
                function R.b(): integer return 1 end\nreturn R\n";
    write(&fwd, text);
    git(&root, &["init", "-q"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "patched"]);

    let (_, _, err) = htl(&["check"], &root);
    assert!(
        err.contains("patches/mathx/src/fwd.tl") && err.contains("(fixable: htl fix)"),
        "the copy has something to fix: {err}"
    );

    let (ok, _, err) = htl(&["fix"], &root);
    assert!(ok, "{err}");
    assert_eq!(
        std::fs::read_to_string(&fwd).unwrap(),
        text,
        "the copy is a diff against the revision it came from, and a fix nobody wrote is \
         not this project's to add to it: {err}"
    );
    assert!(
        err.contains("htl fix: 1 file(s), 0 changed"),
        "only the project's own: {err}"
    );
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

/// `htl pkg patch <dep>` end to end: a dependency on disk, taken into the tree — the
/// package, and not the repository it was checked out of. The upstream here carries what a
/// real one does beside its source, a CI workflow and an ignore file, and neither is
/// committed into this project.
#[test]
fn patch_reports_where_the_copy_is() {
    let dep = scratch("remote");
    write(&dep.join("src/mathx.tl"), "return {}\n");
    write(&dep.join("README.md"), "# mathx\n");
    write(
        &dep.join(".github/workflows/ci.yml"),
        "name: ci\non: [push]\n",
    );
    write(&dep.join(".gitignore"), "/target\n");
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
    assert!(
        err.contains("dropped .git, .github, .gitignore (the repository's, not the package's)"),
        "the run says what it left out, once, and only because there was something: {err}"
    );
    assert!(root.join("patches/mathx/src/mathx.tl").is_file(), "{err}");
    assert!(root.join("patches/mathx/README.md").is_file(), "{err}");
    for gone in [".git", ".github", ".gitignore"] {
        assert!(
            std::fs::symlink_metadata(root.join("patches/mathx").join(gone)).is_err(),
            "{gone} is under patches/mathx, where the project would commit it: {err}"
        );
    }
    assert!(
        std::fs::read_to_string(root.join("mlua-pkg.toml"))
            .unwrap()
            .contains("patch_dir = \"patches/mathx\""),
        "{err}"
    );

    // And the copy is still what install resolves the dependency from (the acceptance).
    let (ok, _, err) = htl(&["pkg", "install"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("mathx"), "{err}");
    assert!(
        std::fs::canonicalize(root.join(".htl/modules/vendored/mathx")).unwrap()
            == std::fs::canonicalize(root.join("patches/mathx")).unwrap(),
        "{err}"
    );
}

/// A fresh clone of a project with a patched dependency: the manifest, the copy, and no
/// `.htl/` at all — which is every clone, since `htl init` gitignores that directory, and
/// the copy `cargo package` builds as well. The dependency resolves from the copy, with
/// no install, no link and no network, because the manifest names the directory and the
/// directory is committed (#266). Nothing has to be written by hand for it: no
/// `[check] paths`, no `exclude`.
#[test]
fn a_clone_resolves_the_patched_dependency_from_the_copy_with_no_links() {
    let root = patched_project("fresh-clone");
    write(
        &root.join("src/main.tl"),
        "local mathx = require(\"mathx\")\nprint(mathx.twice(21))\n",
    );
    assert!(
        !root.join(".htl").exists(),
        "the shape under test is the one a clone has"
    );

    let (ok, _, err) = htl(&["check", "."], &root);
    assert!(ok, "the require resolves with nothing installed: {err}");

    let (ok, out, err) = htl(&["resolve", "mathx", "--format", "json"], &root);
    assert!(ok, "{out}{err}");
    // The project has no `htl.toml`, so there is no root to print relative to and the
    // paths come out absolute; what this is about is the last three components of them.
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let read_path = v["read"].as_str().unwrap_or_default();
    assert!(
        read_path.ends_with("patches/mathx/src/mathx.tl"),
        "the copy is what the name resolves to: {out}"
    );
    let read = v["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["status"] == "read")
        .expect("the file that is read is a row");
    assert!(
        read["dir"].as_str().unwrap().ends_with("patches/mathx/src"),
        "attributed to the copy's entry, which is on the path in its own right: {out}"
    );
    assert_eq!(read["origin"]["kind"], "patched", "{out}");
    assert_eq!(read["origin"]["name"], "mathx", "{out}");
}
