//! `htl unused` through the real binary: the three shapes it reports on, and the four it
//! has to stay quiet about.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-unused", name)
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

/// An entry script, the module it requires, a test, and a module nothing reaches.
fn project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    write(
        &root.join("src/main.tl"),
        "local util = require(\"util\")\nprint(util.twice(2))\n",
    );
    write(
        &root.join("src/util.tl"),
        "local record util\nend\nfunction util.twice(n: integer): integer\n   return n * 2\nend\nreturn util\n",
    );
    write(
        &root.join("src/orphan.tl"),
        "local record orphan\nend\nfunction orphan.hi(): string\n   return \"hi\"\nend\nreturn orphan\n",
    );
    write(
        &root.join("tests/util_test.tl"),
        "local t = require(\"htl.test\")\nlocal util = require(\"util\")\n\
         t.it(\"twice\", function() t.expect(util.twice(2)):to_equal(4) end)\n",
    );
    root
}

#[test]
fn a_module_no_entry_reaches_is_reported_and_a_require_clears_it() {
    let root = project("module");
    let (ok, _, err) = htl(&["unused"], &root);
    assert!(ok, "unused exits 0 by default: {err}");
    assert!(err.contains("module: src/orphan.tl (orphan)"), "{err}");
    assert!(
        err.contains("htl unused: 1 module, 0 dependencies"),
        "{err}"
    );

    // The same project with one `require` added: the report is empty.
    write(
        &root.join("src/main.tl"),
        "local util = require(\"util\")\nlocal orphan = require(\"orphan\")\n\
         print(util.twice(2), orphan.hi())\n",
    );
    let (ok, _, err) = htl(&["unused"], &root);
    assert!(ok, "{err}");
    assert!(!err.contains("module:"), "nothing left to report: {err}");
    assert!(
        err.contains("htl unused: 0 modules, 0 dependencies"),
        "{err}"
    );
}

#[test]
fn a_module_reached_only_from_a_test_is_not_reported() {
    let root = project("test-only");
    write(
        &root.join("tests/orphan_test.tl"),
        "local t = require(\"htl.test\")\nlocal orphan = require(\"orphan\")\n\
         t.it(\"hi\", function() t.expect(orphan.hi()):to_equal(\"hi\") end)\n",
    );
    let (ok, _, err) = htl(&["unused"], &root);
    assert!(ok, "{err}");
    assert!(!err.contains("orphan"), "tests are entries: {err}");
}

#[test]
fn a_module_reached_only_from_a_test_is_not_reported_when_only_src_is_asked_about() {
    // The walk is the project's even when the report is narrowed: `htl unused src` must
    // not turn "reached from tests/" into a finding.
    let root = project("test-only-src");
    write(
        &root.join("tests/orphan_test.tl"),
        "local t = require(\"htl.test\")\nlocal orphan = require(\"orphan\")\n\
         t.it(\"hi\", function() t.expect(orphan.hi()):to_equal(\"hi\") end)\n",
    );
    let (ok, _, err) = htl(&["unused", "src"], &root);
    assert!(ok, "{err}");
    assert!(!err.contains("orphan"), "{err}");
    // Narrowed all the same: the test files are entries, not candidates.
    assert!(err.contains("[3 considered"), "{err}");
}

#[test]
fn a_module_only_a_dynamic_require_reaches_is_quiet_when_build_extra_says_so() {
    let root = project("extra");
    let (_, _, err) = htl(&["unused"], &root);
    assert!(err.contains("module: src/orphan.tl"), "{err}");

    write(
        &root.join("htl.toml"),
        "[lint]\nstrict = false\n\n[build]\nextra = [\"orphan\"]\n",
    );
    let (ok, _, err) = htl(&["unused"], &root);
    assert!(ok, "{err}");
    assert!(!err.contains("module:"), "listed under extra: {err}");

    // And with the line taken out again, it is reported once more.
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    let (_, _, err) = htl(&["unused"], &root);
    assert!(err.contains("module: src/orphan.tl"), "{err}");
}

#[test]
fn a_module_under_a_contract_dir_is_an_entry() {
    let root = project("contract");
    write(
        &root.join("htl.toml"),
        "[lint]\nstrict = false\n\n[[contract]]\ndir = \"mods\"\n",
    );
    write(
        &root.join("src/defs.tl"),
        "local record defs\n   ---@contract\n   record Mod\n      ---@required\n      name: string\n   end\nend\nreturn defs\n",
    );
    write(
        &root.join("mods/goblin.tl"),
        "local defs = require(\"defs\")\nlocal m: defs.Mod = { name = \"goblin\" }\nreturn m\n",
    );
    let (_, _, err) = htl(&["unused"], &root);
    assert!(
        !err.contains("mods/goblin.tl"),
        "a contract module is loaded by name at run time: {err}"
    );
    // And it reaches what it requires: `defs` is not orphaned by having no other caller.
    assert!(!err.contains("src/defs.tl"), "{err}");
}

#[test]
fn the_entry_a_rust_host_embeds_is_an_entry() {
    // A project whose `main` is in Rust has no src/main.tl: the entry is named by
    // `include_bundle!`, and nothing else says which file it is.
    let root = project("host");
    std::fs::remove_file(root.join("src/main.tl")).unwrap();
    write(
        &root.join("Cargo.toml"),
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        &root.join("src/boot.tl"),
        "local util = require(\"util\")\nlocal record boot\nend\n\
         function boot.start(): integer\n   return util.twice(1)\nend\nreturn boot\n",
    );
    write(
        &root.join("src/lib.rs"),
        "const BUNDLE: &[u8] = htl::include_bundle!(\"src/boot.tl\", host = [\"host\"]);\n",
    );
    let (ok, _, err) = htl(&["unused"], &root);
    assert!(ok, "{err}");
    assert!(!err.contains("src/boot.tl"), "{err}");
    assert!(!err.contains("src/util.tl"), "reached through boot: {err}");
    assert!(err.contains("module: src/orphan.tl"), "{err}");
}

/// The same project with two `[deps]`, one of them required. Both are `target_dir`
/// copies, which resolve from the tree without an install.
fn project_with_deps(name: &str) -> PathBuf {
    let root = project(name);
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nentry = \"src\"\n\n[deps]\n\
         mathx = { git = \"https://example.invalid/mathx\", tag = \"v0.1\", target_dir = \"lua/mathx\" }\n\
         strx = { git = \"https://example.invalid/strx\", tag = \"v0.1\", target_dir = \"lua/strx\" }\n",
    );
    write(
        &root.join("lua/mathx/init.tl"),
        "local record mathx\nend\nfunction mathx.add(a: integer, b: integer): integer\n   return a + b\nend\nreturn mathx\n",
    );
    write(
        &root.join("lua/strx/init.tl"),
        "local record strx\nend\nfunction strx.shout(s: string): string\n   return s .. \"!\"\nend\nreturn strx\n",
    );
    root
}

#[test]
fn a_dependency_nothing_requires_is_reported_and_a_require_clears_it() {
    let root = project_with_deps("deps");
    let (ok, _, err) = htl(&["unused"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("dependency: mathx"), "{err}");
    assert!(err.contains("dependency: strx"), "{err}");
    // The dependencies' own sources are not candidates: the copy is not the project's.
    assert!(!err.contains("lua/mathx"), "{err}");

    write(
        &root.join("src/util.tl"),
        "local mathx = require(\"mathx\")\nlocal record util\nend\n\
         function util.twice(n: integer): integer\n   return mathx.add(n, n)\nend\nreturn util\n",
    );
    let (_, _, err) = htl(&["unused"], &root);
    assert!(!err.contains("dependency: mathx"), "{err}");
    assert!(err.contains("dependency: strx"), "still nobody's: {err}");
}

#[test]
fn a_dependency_only_an_unreached_module_requires_is_still_unused() {
    let root = project_with_deps("deps-unreached");
    // src/orphan.tl is not reached, so what it requires is not required.
    write(
        &root.join("src/orphan.tl"),
        "local strx = require(\"strx\")\nlocal record orphan\nend\n\
         function orphan.hi(): string\n   return strx.shout(\"hi\")\nend\nreturn orphan\n",
    );
    let (_, _, err) = htl(&["unused"], &root);
    assert!(err.contains("dependency: strx"), "{err}");
    assert!(err.contains("module: src/orphan.tl"), "{err}");
}

#[test]
fn a_project_with_no_entry_gets_a_message_rather_than_a_list() {
    let root = scratch("no-entry");
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    write(
        &root.join("src/util.tl"),
        "local record util\nend\nfunction util.twice(n: integer): integer\n   return n * 2\nend\nreturn util\n",
    );
    write(
        &root.join("src/other.tl"),
        "local record other\nend\nfunction other.hi(): string\n   return \"hi\"\nend\nreturn other\n",
    );
    let (ok, _, err) = htl(&["unused"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("nothing to start from"), "{err}");
    assert!(!err.contains("module:"), "no list at all: {err}");

    // Even with the flag: there is nothing to fail on.
    let (ok, _, _) = htl(&["unused", "--exit-non-zero-on-unused"], &root);
    assert!(ok);
}

#[test]
fn json_carries_the_kinds_the_text_form_names() {
    let root = project_with_deps("json");
    let (ok, stdout, stderr) = htl(&["unused", "--format", "json"], &root);
    assert!(ok, "{stderr}");
    assert!(
        stderr.trim().is_empty(),
        "json mode keeps stderr silent: {stderr}"
    );
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is one JSON document");
    let modules = v["modules"].as_array().unwrap();
    assert_eq!(modules.len(), 1, "{v}");
    assert_eq!(modules[0]["path"], "src/orphan.tl");
    assert_eq!(modules[0]["module"], "orphan");
    let deps: Vec<&str> = v["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(deps, ["mathx", "strx"]);
    let kinds: Vec<&str> = v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"main"), "{v}");
    assert!(kinds.contains(&"test"), "{v}");
    assert_eq!(v["summary"]["modules"], 1);
    assert_eq!(v["summary"]["dependencies"], 2);
    assert_eq!(v["summary"]["no_entry"], false);
    assert_eq!(v["summary"]["ok"], false);
}

#[test]
fn a_clean_project_exits_zero_and_says_so_and_the_flag_fails_on_a_finding() {
    let root = project("clean");
    std::fs::remove_file(root.join("src/orphan.tl")).unwrap();
    let (ok, _, err) = htl(&["unused", "--exit-non-zero-on-unused"], &root);
    assert!(ok, "{err}");
    assert!(
        err.contains("htl unused: 0 modules, 0 dependencies"),
        "{err}"
    );

    let (ok, stdout, _) = htl(&["unused", "--format", "json"], &root);
    assert!(ok);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["summary"]["ok"], true);

    // The same project with the module back, and the flag on.
    write(
        &root.join("src/orphan.tl"),
        "local record orphan\nend\nfunction orphan.hi(): string\n   return \"hi\"\nend\nreturn orphan\n",
    );
    let (ok, _, err) = htl(&["unused", "--exit-non-zero-on-unused"], &root);
    assert!(!ok, "the flag is what makes a finding fail: {err}");
    // Without it, the same finding exits 0.
    let (ok, _, _) = htl(&["unused"], &root);
    assert!(ok);
}
