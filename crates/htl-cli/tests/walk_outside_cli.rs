//! A walk over a project visits the files its modules hold. A `.tl` directly under the
//! project root — which is no module's root unless it is the source root — is not checked
//! as the project's or counted as a module, and the run says so once, with where it goes.
//! A project that has no source directory at all and keeps its `.tl` at the root is laid
//! out flat without saying so: every walk refuses it, naming `[layout] source = "."` and
//! the source directory as the two ways out.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-walk-outside", name)
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

/// A project with one source file and a `.tl` at its root that would not type-check.
fn project(name: &str, toml: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), toml);
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"game\"\nversion = \"0.1.0\"\n",
    );
    write(&root.join("src/main.tl"), "print(1)\n");
    write(
        &root.join("stray.tl"),
        "local x: integer = \"no\"\nprint(x)\n",
    );
    root
}

#[test]
fn a_file_at_the_root_is_not_the_projects_and_the_run_says_so() {
    let root = project("root-file", "");
    let (ok, err) = htl(&["check", "--no-cache", "."], &root);
    assert!(
        ok,
        "the stray file's type error is not the project's: {err}"
    );
    assert!(
        err.contains(
            "htl check: 1 file(s) belong to no module of the project and were not checked: \
             stray.tl; move them under src/"
        ),
        "{err}"
    );
    assert!(
        err.contains(
            "move them under src/, or set [layout] source = \".\" if the project root is where \
             its modules are"
        ),
        "{err}"
    );
    assert!(err.contains("htl check: 1 file(s), 0 error(s)"), "{err}");

    let (_, err) = htl(&["unused"], &root);
    assert!(
        !err.contains("stray.tl"),
        "not a module, so not an unused one: {err}"
    );

    let (ok, err) = htl(&["fix", "--allow-no-vcs", "."], &root);
    assert!(ok, "{err}");
    assert!(err.contains("were not fixed: stray.tl"), "{err}");
}

/// Named on its own, the file is the question asked outright, and is checked.
#[test]
fn a_file_named_on_the_command_line_is_checked() {
    let root = project("named", "");
    let (ok, err) = htl(&["check", "--no-cache", "stray.tl"], &root);
    assert!(!ok, "{err}");
    assert!(err.contains("stray.tl:1:"), "{err}");
    assert!(!err.contains("belong to no module"), "{err}");
}

/// A flat project's root is its source root, so the same file is one of its modules.
#[test]
fn a_flat_projects_root_file_is_its_own() {
    let root = project("flat", "[layout]\nsource = \".\"\n");
    let (ok, err) = htl(&["check", "--no-cache", "."], &root);
    assert!(!ok, "the file is the project's, and so is its error: {err}");
    assert!(!err.contains("belong to no module"), "{err}");
}

/// A project with no `src/` and its code at the root: flat, but not declared so.
fn undeclared_flat(name: &str, toml: Option<&str>) -> PathBuf {
    let root = scratch(name);
    if let Some(toml) = toml {
        write(&root.join("htl.toml"), toml);
    }
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"flat\"\nversion = \"0.1.0\"\n",
    );
    write(
        &root.join("lib.tl"),
        "local record lib\nend\nfunction lib.add(a: integer, b: integer): integer\n   return a + b\nend\nreturn lib\n",
    );
    write(
        &root.join("main.tl"),
        "local lib = require(\"lib\")\nlocal s: string = lib.add(1, 2)\nprint(s)\n",
    );
    write(
        &root.join("lib_test.tl"),
        "local t = require(\"htl.test\")\nlocal lib = require(\"lib\")\n\
         t.it(\"adds\", function() t.expect(lib.add(1, 2)):to_equal(3) end)\n",
    );
    root
}

fn exit_code(args: &[&str], cwd: &Path) -> (Option<i32>, String) {
    let out = Command::new(common::htl_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// With no `src/` and no `[layout] source`, the root's `.tl` are where the project keeps
/// its code: every walk refuses to skip them and says how to state the layout — whether
/// `htl.toml` is absent or says nothing about it.
#[test]
fn an_undeclared_flat_project_is_refused_with_both_ways_out() {
    for (name, toml) in [("flat-no-toml", None), ("flat-empty-toml", Some(""))] {
        let root = undeclared_flat(name, toml);
        for cmd in [
            &["check", "--no-cache", "."][..],
            &["fix", "--allow-no-vcs", "."],
            &["unused"],
        ] {
            let (code, err) = exit_code(cmd, &root);
            assert_eq!(code, Some(2), "{cmd:?}: {err}");
            assert!(
                err.contains(
                    "htl: 3 file(s) at the project root belong to no module (lib.tl, \
                     lib_test.tl, main.tl): add [layout] source = \".\" to htl.toml, or move \
                     them under src/"
                ),
                "{cmd:?}: {err}"
            );
        }
        let (code, err) = exit_code(&["test", "."], &root);
        assert_eq!(code, Some(2), "{err}");
        assert!(
            err.contains("htl: 1 file(s) at the project root belong to no module (lib_test.tl)"),
            "{err}"
        );
        assert!(!err.contains("module not found"), "no test ran: {err}");
    }
}

/// Stating the layout is the whole of the fix: the same files are the project's modules.
#[test]
fn the_same_project_declared_flat_is_checked_and_tested() {
    let root = undeclared_flat("flat-declared", Some("[layout]\nsource = \".\"\n"));
    let (code, err) = exit_code(&["check", "--no-cache", "."], &root);
    assert_eq!(code, Some(1), "main.tl's own error is found: {err}");
    assert!(err.contains("error: main.tl:2:"), "{err}");
    let (code, err) = exit_code(&["test", "."], &root);
    assert_eq!(code, Some(0), "{err}");
    assert!(err.contains("ok   lib_test.tl"), "{err}");
}

/// A file named outright is checked even in a project the walk refuses.
#[test]
fn a_named_file_in_an_undeclared_flat_project_is_checked() {
    let root = undeclared_flat("flat-named", None);
    let (code, err) = exit_code(&["check", "--no-cache", "main.tl"], &root);
    assert_eq!(code, Some(1), "{err}");
    assert!(err.contains("error: main.tl:"), "{err}");
    assert!(!err.contains("at the project root"), "{err}");
}

/// No `src/` and nothing at the root is not a flat project: nothing to refuse.
#[test]
fn no_source_directory_and_nothing_at_the_root_is_not_refused() {
    let root = scratch("empty-project");
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"empty\"\nversion = \"0.1.0\"\n",
    );
    let (code, err) = exit_code(&["check", "--no-cache", "."], &root);
    assert_eq!(code, Some(0), "{err}");
}
