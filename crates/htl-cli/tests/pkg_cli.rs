//! `htl pkg` through the real binary, with nothing else on `PATH`.
//!
//! The verbs used to be handed to an `mlua-pkg` process; they are calls into the library
//! htl links now. The test that says so is the one that runs with an empty `PATH`: if a
//! child process were still involved, there would be nothing to spawn.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-pkg", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// The CLI with an empty `PATH`: nothing of the host's is reachable, so anything that
/// works here is htl's own code.
fn htl(args: &[&str], cwd: &Path) -> (bool, String, String) {
    let out = Command::new(common::htl_bin())
        .args(args)
        .current_dir(cwd)
        .env("PATH", "")
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

const DECL: &str = "local record mathx\n   twice: function(n: number): number\nend\nreturn mathx\n";

/// A dependency as a repository on disk, and a project depending on it at that commit.
fn project(name: &str) -> PathBuf {
    let dep = scratch(&format!("{name}-remote"));
    write(
        &dep.join("src/mathx.tl"),
        "return { twice = function(n: number): number return n * 2 end }\n",
    );
    write(&dep.join("types/mathx.d.tl"), DECL);
    git(&dep, &["init", "-q"]);
    git(&dep, &["add", "."]);
    git(&dep, &["commit", "-qm", "mathx"]);
    let sha = git(&dep, &["rev-parse", "HEAD"]);

    let root = scratch(name);
    write(
        &root.join("mlua-pkg.toml"),
        &format!(
            "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n\
             [deps.mathx]\ngit = \"file://{}\"\nrev = \"{sha}\"\n",
            dep.display()
        ),
    );
    root
}

#[test]
fn install_runs_without_mlua_pkg_on_path_and_says_what_it_did() {
    let root = project("install");
    let (ok, _, err) = htl(&["pkg", "install"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("install mathx"), "{err}");
    assert!(err.contains("htl pkg install: 1 package(s)"), "{err}");
    assert!(root.join(".htl/modules/vendored/mathx").exists(), "{err}");
    assert!(
        root.join("types/mathx.d.tl").is_file(),
        "and what the dependency publishes is brought into the project: {err}"
    );
}

/// The dependency's module is at `src/mathx.tl`, so its entry is `src/` and the root
/// `vendored/mathx` points at holds no `mathx.tl`. What `require("mathx")` reads is
/// `entries/mathx`, the link at the entry — from the checker, the runner and the test
/// runner alike. Before the link existed (htl 0.4.0) every one of them said `module not
/// found`.
#[test]
fn an_installed_dependency_is_required_at_its_entry() {
    let root = project("entry");
    write(
        &root.join("src/main.tl"),
        "local mathx = require(\"mathx\")\nprint(mathx.twice(21))\n",
    );
    write(
        &root.join("tests/mathx_test.tl"),
        "local t = require(\"htl.test\")\nlocal mathx = require(\"mathx\")\n\
         t.it(\"doubles\", function() t.expect(mathx.twice(2)):to_equal(4) end)\n",
    );
    let (ok, _, err) = htl(&["pkg", "install"], &root);
    assert!(ok, "{err}");
    let link = root.join(".htl/modules/entries/mathx");
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        Path::new("../vendored/mathx/src"),
        "relative, through the root link, at the entry"
    );

    let (ok, _, err) = htl(&["check", "."], &root);
    assert!(ok, "{err}");
    assert!(err.contains("0 error(s)"), "{err}");
    let (ok, out, err) = htl(&["run", "src/main.tl"], &root);
    assert!(ok, "{err}");
    assert_eq!(out.trim(), "42");
    let (ok, _, err) = htl(&["test"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("1 passed, 0 failed"), "{err}");
}

/// A check before the install records that `mathx` resolved nowhere; the install has to be
/// seen as the change it is, not replayed. The directory the dependency arrives in is one
/// of the entry's probes for that reason.
#[test]
fn a_check_cached_before_the_install_is_not_replayed_after_it() {
    let root = project("entry-cache");
    write(
        &root.join("src/main.tl"),
        "local mathx = require(\"mathx\")\nprint(mathx.twice(21))\n",
    );
    let (ok, _, err) = htl(&["check", "."], &root);
    assert!(!ok, "{err}");
    assert!(err.contains("module not found: 'mathx'"), "{err}");
    let (ok, _, err) = htl(&["pkg", "install"], &root);
    assert!(ok, "{err}");
    let (ok, _, err) = htl(&["check", "."], &root);
    assert!(ok, "{err}");
    assert!(err.contains("0 error(s)"), "{err}");
}

/// The links are htl's, written from the lockfile, so a project installed by an htl that
/// did not write them gets them from the first command that reads the path — no reinstall.
#[test]
fn a_project_installed_without_entry_links_gets_them_on_the_next_check() {
    let root = project("entry-upgrade");
    write(
        &root.join("src/main.tl"),
        "local mathx = require(\"mathx\")\nprint(mathx.twice(21))\n",
    );
    let (ok, _, err) = htl(&["pkg", "install"], &root);
    assert!(ok, "{err}");
    std::fs::remove_dir_all(root.join(".htl/modules/entries")).unwrap();
    let (ok, _, err) = htl(&["check", "."], &root);
    assert!(ok, "{err}");
    assert!(root.join(".htl/modules/entries/mathx").exists(), "{err}");
}

#[test]
fn clean_reports_the_cache_it_swept() {
    let root = project("clean");
    let (ok, _, err) = htl(&["pkg", "clean"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("no lockfile"), "{err}");

    htl(&["pkg", "install"], &root);
    let (ok, _, err) = htl(&["pkg", "clean", "--all"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("removed every cached package"), "{err}");
}

#[test]
fn add_writes_the_entry_and_says_what_to_run_next() {
    let root = project("add");
    let (ok, _, err) = htl(
        &[
            "pkg",
            "add",
            "vec2",
            "https://example.invalid/vec2",
            "--tag",
            "v1.0",
        ],
        &root,
    );
    assert!(ok, "{err}");
    assert!(err.contains("added   vec2"), "{err}");
    assert!(err.contains("htl pkg install"), "{err}");
    let manifest = std::fs::read_to_string(root.join("mlua-pkg.toml")).unwrap();
    assert!(manifest.contains("[deps.vec2]"), "{manifest}");
}

/// A commit pin is nothing to update, and update says which dependency it left alone and
/// why rather than reporting an empty run.
#[test]
fn update_says_what_it_left_alone() {
    let root = project("update");
    let (ok, _, err) = htl(&["pkg", "update", "--dry-run"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("skip    mathx"), "{err}");
    assert!(err.contains("dry run; nothing was written"), "{err}");
    assert!(
        !root.join("mlua-pkg.lock").exists(),
        "and a dry run installs nothing: {err}"
    );
}

/// The library's error for a manifest that is not there does not name the file. htl looked
/// for it, so htl says where.
#[test]
fn a_project_that_is_not_there_is_named() {
    let root = scratch("bare");
    let (ok, _, err) = htl(&["pkg", "install"], &root);
    assert!(!ok, "{err}");
    assert!(err.contains("mlua-pkg.toml"), "{err}");
    assert!(err.contains(&root.display().to_string()), "{err}");
}

#[test]
fn pkg_help_lists_the_verbs() {
    let root = scratch("help");
    let (ok, out, err) = htl(&["pkg", "--help"], &root);
    assert!(ok, "{err}");
    for verb in ["install", "add", "update", "clean", "patch"] {
        assert!(out.contains(verb), "{verb} missing from:\n{out}");
    }
}
