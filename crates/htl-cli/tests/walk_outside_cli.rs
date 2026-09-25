//! A walk over a project visits the files its modules hold. A `.tl` directly under the
//! project root — which is no module's root unless it is the source root — is not checked
//! as the project's or counted as a module, and the run says so once, with where it goes.

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
             ./stray.tl; move them under src/"
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
    assert!(err.contains("were not fixed: ./stray.tl"), "{err}");
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
