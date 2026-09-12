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
    Command::new(common::htl_bin())
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

/// What a person does after editing `Cargo.toml`: write the lockfile the new manifest asks
/// for. The graph is read with `--locked` when there is a lockfile, so a test that changes
/// the dependencies mid-run has to refresh it or be refused — which is the point of
/// `a_lockfile_the_manifest_has_outgrown_is_refused_naming_it` below, and noise everywhere
/// else. Removing it rather than running `cargo update`: with no lockfile the flag is not
/// passed, cargo resolves the path-only graph offline, and there is nothing to go stale.
fn relock(root: &Path) {
    let _ = std::fs::remove_file(root.join("Cargo.lock"));
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

/// A crate that ships one declaration per module names the directory and a `*`, and gets
/// the same files under `types/` as it would have by listing them. The manifest is the
/// fixture's with its one entry rewritten; a second file beside the first shows the pattern
/// took both, in name order.
#[test]
fn a_star_in_the_manifest_ships_every_matching_file() {
    let root = project("glob");
    let dep = root.parent().unwrap().join("dep");
    write(
        &dep.join("Cargo.toml"),
        "[package]\nname = \"dep\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [package.metadata.htl]\ndts = [\"dts/*.d.tl\"]\n",
    );
    write(
        &dep.join("dts/also.d.tl"),
        "local record also\n   n: integer\nend\n\nreturn also\n",
    );
    let out = htl(&root, &["dts"]);
    let msg = err(&out);
    assert!(out.status.success(), "{msg}");
    assert!(msg.contains("wrote     types/dep/also.d.tl"), "{msg}");
    assert!(msg.contains("wrote     types/dep/dep.d.tl"), "{msg}");
    assert_eq!(
        std::fs::read_to_string(root.join("types/dep/dep.d.tl")).unwrap(),
        DECL
    );
    let note = std::fs::read_to_string(root.join("types/dep/.htl-dts")).unwrap();
    assert!(
        note.contains("files = [\"also.d.tl\", \"dep.d.tl\"]"),
        "{note}"
    );
}

/// A pattern that matches nothing is the manifest's mistake, and the report names the
/// pattern — the line to fix — the way it names a listed file that is not there.
#[test]
fn a_star_matching_nothing_fails_naming_the_pattern() {
    let root = project("glob-none");
    let dep = root.parent().unwrap().join("dep");
    write(
        &dep.join("Cargo.toml"),
        "[package]\nname = \"dep\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [package.metadata.htl]\ndts = [\"types/*.d.tl\"]\n",
    );
    let out = htl(&root, &["dts"]);
    let msg = err(&out);
    assert!(!out.status.success(), "{msg}");
    assert!(msg.contains("not written: dep 0.1.0"), "{msg}");
    assert!(msg.contains("types/*.d.tl"), "{msg}");
}

/// A crate whose modules have a namespace says where its paths start, and what is below
/// that root is kept: the file lands at `types/<crate>/mine/thing.d.tl` and the project
/// requires the module the crate registered, `mine.thing`. Without the root the same
/// entry would land as `thing.d.tl` and only `require("thing")` would resolve.
#[test]
fn a_dts_root_keeps_the_namespace_and_require_reads_it() {
    let root = project("dts-root");
    let dep = root.parent().unwrap().join("dep");
    write(
        &dep.join("Cargo.toml"),
        "[package]\nname = \"dep\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [package.metadata.htl]\ndts_root = \"types\"\ndts = [\"types/mine/thing.d.tl\"]\n",
    );
    write(
        &dep.join("types/mine/thing.d.tl"),
        "local record thing\n   go: function(string): string\nend\n\nreturn thing\n",
    );
    let out = htl(&root, &["dts"]);
    let msg = err(&out);
    assert!(out.status.success(), "{msg}");
    assert!(msg.contains("wrote     types/dep/mine/thing.d.tl"), "{msg}");
    assert!(root.join("types/dep/mine/thing.d.tl").is_file(), "{msg}");
    let note = std::fs::read_to_string(root.join("types/dep/.htl-dts")).unwrap();
    assert!(note.contains("files = [\"mine/thing.d.tl\"]"), "{note}");

    write(
        &root.join("src/main.tl"),
        "local thing = require(\"mine.thing\")\n\nreturn thing.go(\"x\")\n",
    );
    let out = htl(&root, &["check", "src/main.tl", "--no-cache"]);
    assert!(out.status.success(), "{}", err(&out));

    // The un-namespaced name is what the file would have been called without the root,
    // and is not a module here.
    write(
        &root.join("src/main.tl"),
        "local thing = require(\"thing\")\n\nreturn thing.go(\"x\")\n",
    );
    let out = htl(&root, &["check", "src/main.tl", "--no-cache"]);
    assert!(!out.status.success(), "{}", err(&out));
    assert!(
        err(&out).contains("module not found: 'thing'"),
        "{}",
        err(&out)
    );
}

/// Two modules of the same name in two namespaces are two files, where before the root
/// they were one target and the second was a duplicate.
#[test]
fn two_namespaces_may_hold_the_same_module_name() {
    let root = project("dts-root-two");
    let dep = root.parent().unwrap().join("dep");
    write(
        &dep.join("Cargo.toml"),
        "[package]\nname = \"dep\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [package.metadata.htl]\ndts_root = \"types\"\n\
         dts = [\"types/a/log.d.tl\", \"types/b/log.d.tl\"]\n",
    );
    let decl = "local record log\n   n: integer\nend\n\nreturn log\n";
    write(&dep.join("types/a/log.d.tl"), decl);
    write(&dep.join("types/b/log.d.tl"), decl);
    let out = htl(&root, &["dts"]);
    let msg = err(&out);
    assert!(out.status.success(), "{msg}");
    assert!(msg.contains("wrote     types/dep/a/log.d.tl"), "{msg}");
    assert!(msg.contains("wrote     types/dep/b/log.d.tl"), "{msg}");
    assert!(!msg.contains("names both"), "{msg}");
}

/// An entry the root does not cover is the manifest's mistake: reported like a file the
/// crate does not ship, naming the crate, the entry and the root, and the command fails.
/// The entries the root does cover are written all the same.
#[test]
fn an_entry_outside_the_dts_root_fails_naming_the_entry_and_the_root() {
    let root = project("dts-root-stray");
    let dep = root.parent().unwrap().join("dep");
    write(
        &dep.join("Cargo.toml"),
        "[package]\nname = \"dep\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [package.metadata.htl]\ndts_root = \"types\"\n\
         dts = [\"types/mine/thing.d.tl\", \"dts/dep.d.tl\"]\n",
    );
    write(
        &dep.join("types/mine/thing.d.tl"),
        "local record thing\n   go: function(string): string\nend\n\nreturn thing\n",
    );
    let out = htl(&root, &["dts"]);
    let msg = err(&out);
    assert!(!out.status.success(), "{msg}");
    assert!(msg.contains("not written: dep 0.1.0"), "{msg}");
    assert!(msg.contains("dts/dep.d.tl"), "{msg}");
    assert!(msg.contains("dts_root \"types\""), "{msg}");
    assert!(msg.contains("wrote     types/dep/mine/thing.d.tl"), "{msg}");
}

/// A file from a layout the crate has since left is under `types/<crate>/` at a path
/// nothing ships any more. It is reported wherever it sits in the tree, and it stays:
/// what a file under `types/` is for is the project's to say.
#[test]
fn a_declaration_from_a_previous_layout_is_reported_left_in_place() {
    let root = project("dts-root-moved");
    assert!(htl(&root, &["dts"]).status.success());
    assert!(root.join("types/dep/dep.d.tl").is_file());
    let dep = root.parent().unwrap().join("dep");
    write(
        &dep.join("Cargo.toml"),
        "[package]\nname = \"dep\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [package.metadata.htl]\ndts_root = \"types\"\ndts = [\"types/mine/dep.d.tl\"]\n",
    );
    write(&dep.join("types/mine/dep.d.tl"), DECL);
    let out = htl(&root, &["dts"]);
    let msg = err(&out);
    assert!(out.status.success(), "{msg}");
    assert!(msg.contains("wrote     types/dep/mine/dep.d.tl"), "{msg}");
    assert!(msg.contains("left in place: types/dep/dep.d.tl"), "{msg}");
    assert!(root.join("types/dep/dep.d.tl").is_file(), "not deleted");
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
    relock(&root);
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

/// The crate behind `std.*` is skipped rather than materialised, and a directory an older
/// htl wrote for it is not evidence that it left the graph — it is still a dependency here,
/// by a path rather than by a registry so that this runs offline. The line says what is
/// true of that copy, and the run succeeds: nothing was asked for and not written.
///
/// The departed dependency in the same project keeps the other message, so one run holds
/// both readings of "absent from the resolved set" apart.
#[test]
fn the_crate_std_carries_is_not_reported_as_a_departed_dependency() {
    let root = project("carried");
    let scratch = root.parent().unwrap().to_path_buf();
    // A path dependency with that crate's name: the skip is by name, and the fixture is
    // the smaller half of what the real one ships.
    let batteries = scratch.join("mlua-batteries");
    write(
        &batteries.join("Cargo.toml"),
        "[package]\nname = \"mlua-batteries\"\nversion = \"0.7.2\"\nedition = \"2024\"\n\n\
         [package.metadata.htl]\ndts = [\"types/mlua_batteries/json.d.tl\"]\n",
    );
    write(&batteries.join("src/lib.rs"), "");
    write(
        &batteries.join("types/mlua_batteries/json.d.tl"),
        "local record json\n   encode: function(any): string\nend\n\nreturn json\n",
    );
    // What an htl without the feature left behind: the copy, and the note that says which
    // crate it came from.
    write(
        &root.join("types/mlua-batteries/json.d.tl"),
        "local record json\n   encode: function(any): string\nend\n\nreturn json\n",
    );
    write(
        &root.join("types/mlua-batteries/.htl-dts"),
        "crate = \"mlua-batteries\"\nversion = \"0.7.2\"\nfiles = [\"json.d.tl\"]\n",
    );
    // The fixture's own `dep`, materialised and then taken out of the graph.
    assert!(htl(&root, &["dts"]).status.success());
    write(
        &root.join("Cargo.toml"),
        "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [dependencies]\nmlua-batteries = { path = \"../mlua-batteries\" }\n",
    );
    relock(&root);

    let out = htl(&root, &["dts"]);
    let msg = err(&out);
    assert!(out.status.success(), "{msg}");
    let carried = msg
        .lines()
        .find(|l| l.contains("types/mlua-batteries/json.d.tl"))
        .unwrap_or_else(|| panic!("no line for the carried crate:\n{msg}"));
    assert!(carried.starts_with("  left in place:"), "{carried}");
    assert!(
        carried.contains("mlua-batteries is on the path as std.* instead"),
        "{carried}"
    );
    assert!(
        carried.contains("nothing preloads this copy's module name"),
        "{carried}"
    );
    assert!(!carried.contains("no longer"), "{carried}");
    // Nothing of it was written: the crate is skipped, not materialised.
    assert!(!msg.contains("types/mlua-batteries/json.d.tl\n"), "{msg}");
    // And the one that really did leave still reads as it did.
    assert!(msg.contains("dep is no longer a dependency"), "{msg}");
}

/// Reading the graph does not write the lockfile. The manifest here names a dependency the
/// lockfile does not have, which is what a moved branch looks like from cargo's side, and
/// the run says so instead of resolving over it. The file is byte-identical afterwards.
#[test]
fn a_lockfile_the_manifest_has_outgrown_is_refused_naming_it() {
    let root = project("stale-lock");
    // A lockfile for the manifest as it stands, written by the first run.
    assert!(htl(&root, &["dts"]).status.success());
    let lock = root.join("Cargo.lock");
    let before = std::fs::read_to_string(&lock).expect("the first run wrote a lockfile");
    assert!(before.contains("name = \"dep\""), "{before}");

    // A second dependency the lockfile has never heard of.
    let other = root.parent().unwrap().join("other");
    write(
        &other.join("Cargo.toml"),
        "[package]\nname = \"other\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(&other.join("src/lib.rs"), "");
    write(
        &root.join("Cargo.toml"),
        "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [dependencies]\ndep = { path = \"../dep\" }\nother = { path = \"../other\" }\n",
    );

    let out = htl(&root, &["dts"]);
    let msg = err(&out);
    assert!(msg.contains("Cargo.lock does not cover"), "{msg}");
    assert!(msg.contains("cargo update"), "{msg}");
    // The reader is told it is the lockfile, not their Teal, and not a flag they passed.
    assert!(!msg.contains("remove the --locked flag"), "{msg}");
    assert_eq!(
        std::fs::read_to_string(&lock).unwrap(),
        before,
        "the lockfile was rewritten"
    );
    // Reported, not fatal: the committed declarations stand and the check goes on.
    assert!(out.status.success(), "{msg}");
}

/// A project that has never run a cargo command has no lockfile, and a refusal there would
/// be a refusal to every scaffolded host until its first build. Nothing to protect, so the
/// graph is read and cargo writes what that build would have written.
#[test]
fn a_project_with_no_lockfile_yet_is_resolved_rather_than_refused() {
    let root = project("no-lock");
    assert!(!root.join("Cargo.lock").exists());
    let out = htl(&root, &["dts"]);
    let msg = err(&out);
    assert!(out.status.success(), "{msg}");
    assert!(msg.contains("wrote     types/dep/dep.d.tl"), "{msg}");
    assert!(!msg.contains("does not cover"), "{msg}");
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
