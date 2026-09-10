//! Declarations a dependency crate ships, materialised into the project's `types/`.
//!
//! The fixture under `tests/fixtures/dep_dts/` is the pair this is about: `dep` names
//! `dts/dep.d.tl` in `[package.metadata.htl]`, `consumer` depends on it by path. Both are
//! copied into a scratch directory first — `htl dts` writes into `consumer/types/`, and a
//! test that wrote into the checkout would pass once.
//!
//! Nothing here builds either crate. `cargo metadata` resolves the graph, the fixture has
//! no dependency outside itself, so the whole thing runs offline.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod common;

const DECL: &str =
    "local record dep\n   greet: function(self: dep, name: string): string\nend\n\nreturn dep\n";

fn htl(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_htl"))
        .args(args)
        .current_dir(root)
        .output()
        .unwrap()
}

fn err(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Copy the fixture pair into a fresh scratch directory, `Cargo.toml.in` becoming the
/// `Cargo.toml` cargo reads. Returns the consumer, which is where the commands are run.
fn project(name: &str) -> PathBuf {
    let root = common::scratch("htl-cli-dep-dts", name);
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dep_dts");
    copy_tree(&from, &root);
    root.join("consumer")
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let name = e.file_name().to_string_lossy().into_owned();
        let src = e.path();
        if src.is_dir() {
            copy_tree(&src, &to.join(name));
            continue;
        }
        let name = name.strip_suffix(".in").unwrap_or(&name).to_string();
        std::fs::copy(&src, to.join(name)).unwrap();
    }
}

/// The first acceptance: written, reported, and a no-op the second time.
#[test]
fn a_shipped_declaration_is_written_reported_and_then_unchanged() {
    let root = project("materialise");
    let out = htl(&root, &["dts"]);
    assert!(out.status.success(), "{}", err(&out));
    assert!(
        err(&out).contains("wrote     types/dep/dep.d.tl"),
        "{}",
        err(&out)
    );
    assert_eq!(
        std::fs::read_to_string(root.join("types/dep/dep.d.tl")).unwrap(),
        DECL
    );

    let out = htl(&root, &["dts"]);
    assert!(out.status.success(), "{}", err(&out));
    assert!(
        err(&out).contains("unchanged types/dep/dep.d.tl"),
        "{}",
        err(&out)
    );
    assert!(!err(&out).contains("wrote"), "{}", err(&out));
}

/// The note is what tells this directory from one a person laid out (`types/socket/`,
/// where the path below `types/` is the module name). It records what the copy came from.
#[test]
fn a_note_beside_the_copy_records_which_crate_it_came_from() {
    let root = project("note");
    assert!(htl(&root, &["dts"]).status.success());
    let note = std::fs::read_to_string(root.join("types/dep/.htl-dts")).unwrap();
    assert!(note.contains("crate = \"dep\""), "{note}");
    assert!(note.contains("version = \"0.1.0\""), "{note}");
    assert!(note.contains("files = [\"dep.d.tl\"]"), "{note}");
}

/// The second acceptance, and the whole point: the script checks against a declaration
/// nobody in this project wrote. `check` materialises it on its own, so this passes on a
/// checkout where `htl dts` has never been run.
#[test]
fn a_script_requiring_the_module_checks_with_no_hand_written_declaration() {
    let root = project("check");
    let out = htl(&root, &["check", "src/main.tl", "--no-cache"]);
    assert!(out.status.success(), "{}", err(&out));
    assert!(root.join("types/dep/dep.d.tl").is_file());
    assert!(!root.join("types/dep.d.tl").exists());

    // Against the declaration, not around it: the wrong argument type is an error.
    write(
        &root.join("src/main.tl"),
        "local dep = require(\"dep\")\n\nreturn dep:greet(42)\n",
    );
    let out = htl(&root, &["check", "src/main.tl", "--no-cache"]);
    assert!(!out.status.success(), "{}", err(&out));
    assert!(
        err(&out).contains("got integer, expected string"),
        "{}",
        err(&out)
    );
}

/// The third acceptance. A project that kept its own copy of a module a crate has since
/// started shipping is told which one is read and which one is not.
#[test]
fn a_hand_written_declaration_beside_the_shipped_one_is_a_duplicate() {
    let root = project("duplicate");
    assert!(htl(&root, &["dts"]).status.success());
    write(&root.join("types/dep.d.tl"), DECL);
    let out = htl(&root, &["check", "src/main.tl", "--no-cache"]);
    let msg = err(&out);
    let line = msg
        .lines()
        .find(|l| l.contains("duplicate-declaration"))
        .unwrap_or_else(|| panic!("no duplicate-declaration in:\n{msg}"));
    assert!(line.contains("types/dep.d.tl is read"), "{line}");
    assert!(line.contains("types/dep/dep.d.tl"), "{line}");
}

/// The fourth acceptance: the dependency goes, the file stays, and the report says so.
/// Deleting it is the project's call — a script may still require the module, and this
/// command has no business removing committed files.
#[test]
fn a_declaration_left_by_a_dependency_that_is_gone_is_reported_not_deleted() {
    let root = project("orphan");
    assert!(htl(&root, &["dts"]).status.success());
    write(
        &root.join("Cargo.toml"),
        "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    let out = htl(&root, &["dts"]);
    let msg = err(&out);
    assert!(msg.contains("left in place: types/dep/dep.d.tl"), "{msg}");
    assert!(msg.contains("dep is no longer a dependency"), "{msg}");
    // Reported, not fatal: the project decides what to do with the file.
    assert!(out.status.success(), "{msg}");
    assert!(root.join("types/dep/dep.d.tl").is_file());
}

/// A crate naming a file it does not ship: the declaration is missing from the project
/// either way, and only the message says whose manifest is wrong.
#[test]
fn a_crate_naming_a_file_it_does_not_ship_fails_naming_the_crate() {
    let root = project("missing");
    let dep = root.parent().unwrap().join("dep");
    std::fs::remove_file(dep.join("dts/dep.d.tl")).unwrap();
    let out = htl(&root, &["dts"]);
    let msg = err(&out);
    assert!(!out.status.success(), "{msg}");
    assert!(msg.contains("not written: dep 0.1.0"), "{msg}");
    assert!(msg.contains("dts/dep.d.tl"), "{msg}");
}

/// Neither report is a lint, so neither wears a lint's `[htl <rule>]` suffix — the suffix
/// that would send a reader to `--list-lints` and `[lint]` for a name that is in neither.
/// Both conditions at once, because the two used to be told apart only by which of them
/// carried which rule name.
#[test]
fn the_report_carries_no_rule_name_for_either_condition() {
    let root = project("no-rule-name");
    assert!(htl(&root, &["dts"]).status.success());
    // A dependency that is gone (its declaration stays under types/), and a declaration
    // the surviving manifest names but does not ship.
    write(
        &root.join("Cargo.toml"),
        "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [dependencies]\nother = { path = \"../other\" }\n",
    );
    let other = root.parent().unwrap().join("other");
    write(
        &other.join("Cargo.toml"),
        "[package]\nname = \"other\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [package.metadata.htl]\ndts = [\"dts/other.d.tl\"]\n",
    );
    write(&other.join("src/lib.rs"), "");
    let out = htl(&root, &["dts"]);
    let msg = err(&out);
    assert!(msg.contains("not written: other 0.1.0"), "{msg}");
    assert!(msg.contains("left in place: types/dep/dep.d.tl"), "{msg}");
    assert!(!msg.contains("[htl "), "{msg}");
    assert!(!msg.contains("shipped-declaration"), "{msg}");
    assert!(!msg.contains("orphaned-declaration"), "{msg}");
    // The exit code is about the one it was asked to write and could not, and says
    // nothing about the one it left alone.
    assert!(!out.status.success(), "{msg}");
}

/// The names are gone from the output, and they were never configurable: nothing sends a
/// reader to `--list-lints` for either of them, and neither is there.
#[test]
fn neither_condition_is_a_configurable_lint_name() {
    let root = project("not-a-lint");
    let out = htl(&root, &["check", "--list-lints"]);
    let listed = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{}", err(&out));
    assert!(!listed.contains("shipped-declaration"), "{listed}");
    assert!(!listed.contains("orphaned-declaration"), "{listed}");
}
