//! A contract type that cannot be published is said so, on every command that would have
//! published it.
//!
//! The failure is one thing — a `function` in the declaring module that belongs to no
//! record the module declares, so the declaration would come out missing it — but three
//! commands reach it, and each has its own way of not being silent: `htl check` reports a
//! lint (fatal under `strict`), `htl dts` exits non-zero, and `htl run` says it on
//! stderr. What none of them may do is write nothing and report nothing.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-publish", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn htl(root: &Path, args: &[&str]) -> Output {
    Command::new(common::htl_bin())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap()
}

/// A project whose declaring module has a function on a table it never declares a record
/// for: there is nowhere in the declaration for it to go.
fn unpublishable(name: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), "[[contract]]\ndir = \"mods\"\n");
    write(
        &root.join("src/defs.tl"),
        "local record defs\n   record Mod   ---@contract\n      name: string   ---@required\n   \
         end\nend\n\nfunction other.helper(n: integer): integer\n   return n\nend\n\nreturn defs\n",
    );
    write(&root.join("mods/good.tl"), "return { name = \"g\" }\n");
    write(&root.join("src/main.tl"), "print(\"hi\")\n");
    root
}

/// `htl check` reports it as a `contract` lint. The fixture is a type error as well —
/// a function on a table nothing declares is one — which is the shape of this failure:
/// with the transform general enough to place the rest, what it cannot place is source
/// that was already wrong.
#[test]
fn check_reports_it_as_a_contract_lint() {
    let root = unpublishable("check");
    let out = htl(&root, &["check", ".", "--no-cache"]);
    let err =
        String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
    assert!(
        err.contains("nothing declares a record other") && err.contains("[htl contract]"),
        "reported as a contract lint: {err}"
    );
}

#[test]
fn dts_exits_non_zero() {
    let root = unpublishable("dts");
    let out = htl(&root, &["dts"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("nothing declares a record other"), "{err}");
    assert!(
        !out.status.success(),
        "asked to generate and did not: {err}"
    );
    assert!(!root.join("types/defs.d.tl").exists(), "nothing written");
}

/// `run` neither lints nor exits on it — the program is still runnable — so it says it on
/// stderr. The one outcome ruled out is silence.
#[test]
fn run_says_it_on_stderr() {
    let root = unpublishable("run");
    let out = htl(&root, &["run", "src/main.tl"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("nothing declares a record other"), "{err}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("hi"),
        "the program still runs"
    );
}

/// The publishable case, for contrast: written, announced, and silent on a second run.
#[test]
fn a_declaration_only_module_is_published_and_announced() {
    let root = scratch("ok");
    write(&root.join("htl.toml"), "[[contract]]\ndir = \"mods\"\n");
    write(
        &root.join("src/defs.tl"),
        "local record defs\n   record Mod   ---@contract\n      name: string   ---@required\n   \
         end\nend\nreturn defs\n",
    );
    write(&root.join("mods/good.tl"), "return { name = \"g\" }\n");

    let first = htl(&root, &["check", ".", "--no-cache"]);
    assert!(first.status.success());
    assert!(
        String::from_utf8_lossy(&first.stderr).contains("dts: wrote types/defs.d.tl"),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    // The directory is written into the published marker: whoever reads this declaration
    // does not have the htl.toml a bare `---@contract` would inherit from.
    assert_eq!(
        std::fs::read_to_string(root.join("types/defs.d.tl")).unwrap(),
        std::fs::read_to_string(root.join("src/defs.tl"))
            .unwrap()
            .replace("---@contract", "---@contract(\"mods\")")
    );

    let second = htl(&root, &["check", ".", "--no-cache"]);
    assert!(
        !String::from_utf8_lossy(&second.stderr).contains("dts: wrote"),
        "nothing changed, nothing written"
    );
}

/// A project with two contracts, run twice: it says what it wrote once per file, and it
/// has nothing to say about its own declarations. Both were symptoms of the same thing —
/// the published declaration read back as a second claimant of the directory, and the
/// module publishing once per contract instead of once.
#[test]
fn two_contracts_are_published_once_each_and_report_nothing() {
    let root = scratch("two-contracts");
    write(
        &root.join("htl.toml"),
        "[[contract]]\ndir = \"mods_a\"\n\n[[contract]]\ndir = \"mods_b\"\n",
    );
    write(
        &root.join("src/defs.tl"),
        "local record defs\n   record ModA   ---@contract(\"mods_a\")\n      name: string   \
         ---@required\n   end\n   record ModB   ---@contract(\"mods_b\")\n      name: string   \
         ---@required\n   end\nend\nreturn defs\n",
    );
    write(&root.join("mods_a/one.tl"), "return { name = \"a\" }\n");
    write(&root.join("mods_b/one.tl"), "return { name = \"b\" }\n");

    let first = htl(&root, &["check", ".", "--no-cache"]);
    let said = String::from_utf8_lossy(&first.stderr).to_string();
    assert_eq!(
        said.matches("dts: wrote").count(),
        1,
        "one file written, said once: {said}"
    );
    assert!(!said.contains("[htl contract]"), "{said}");

    let second = htl(&root, &["check", ".", "--no-cache"]);
    let said = String::from_utf8_lossy(&second.stderr).to_string();
    assert!(!said.contains("dts: wrote"), "it settled: {said}");
    assert!(
        !said.contains("[htl contract]"),
        "and the declaration it wrote is not a second claimant: {said}"
    );
}
