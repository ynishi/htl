//! `htl test` reusing the check and the codegen — and never the run.
//!
//! The distinction is the point. Checking a test file and generating its Lua does not depend
//! on what the tests do, so it is reusable. Whether they pass does, so it is not: every one
//! of these runs actually runs the tests, and the assertions below check that it did.

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "htl-cli-testcache-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn test_run(root: &Path, args: &[&str]) -> serde_json::Value {
    let mut a = vec!["test", ".", "--format", "json"];
    a.extend_from_slice(args);
    let out = Command::new(env!("CARGO_BIN_EXE_htl"))
        .args(&a)
        .current_dir(root)
        .output()
        .unwrap();
    serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("stdout is one JSON document")
}

fn replayed(v: &serde_json::Value) -> u64 {
    v["summary"]["replayed"]
        .as_u64()
        .expect("summary carries `replayed`")
}

fn passed(v: &serde_json::Value) -> u64 {
    v["summary"]["passed"]
        .as_u64()
        .expect("summary carries `passed`")
}

/// A module, a passing test over it, and a second test that does not touch it.
fn project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), "[check]\n");
    write(
        &root.join("src/adder.tl"),
        "local record adder\nend\nfunction adder.add(a: integer, b: integer): integer\n   return a + b\nend\nreturn adder\n",
    );
    write(
        &root.join("tests/adder_test.tl"),
        "local t = require(\"htl.test\")\nlocal adder = require(\"adder\")\n\
         t.it(\"adds\", function() t.expect(adder.add(1, 2)):to_equal(3) end)\n",
    );
    write(
        &root.join("tests/plain_test.tl"),
        "local t = require(\"htl.test\")\n\
         t.it(\"is true\", function() t.expect(true):to_equal(true) end)\n",
    );
    root
}

#[test]
fn a_second_run_reuses_the_checking_and_still_runs_the_tests() {
    let root = project("replay");
    let first = test_run(&root, &[]);
    assert_eq!(replayed(&first), 0, "nothing to reuse yet");
    assert_eq!(passed(&first), 2);

    let second = test_run(&root, &[]);
    assert_eq!(
        replayed(&second),
        2,
        "both files' checks came from the store"
    );
    assert_eq!(passed(&second), 2, "and both files' tests still ran");
    assert_eq!(first["summary"]["failed"], second["summary"]["failed"]);
    assert_eq!(
        first["summary"]["files_with_errors"],
        second["summary"]["files_with_errors"]
    );
}

/// The run is what decides pass or fail, so a test that starts failing has to start failing
/// even though nothing about the checking changed.
#[test]
fn a_replayed_file_still_reports_a_newly_failing_test() {
    let root = project("still-runs");
    assert_eq!(passed(&test_run(&root, &[])), 2);
    assert_eq!(replayed(&test_run(&root, &[])), 2);

    // The module the test asserts on now returns something else. Its own file is unchanged.
    write(
        &root.join("src/adder.tl"),
        "local record adder\nend\nfunction adder.add(a: integer, b: integer): integer\n   return a + b + 1\nend\nreturn adder\n",
    );
    let v = test_run(&root, &[]);
    assert_eq!(
        v["summary"]["failed"], 1,
        "the assertion has to fail now: {v}"
    );
}

#[test]
fn editing_a_test_file_misses_for_that_file() {
    let root = project("edit-test");
    test_run(&root, &[]);
    assert_eq!(replayed(&test_run(&root, &[])), 2);

    write(
        &root.join("tests/plain_test.tl"),
        "local t = require(\"htl.test\")\n\
         t.it(\"is still true\", function() t.expect(1):to_equal(1) end)\n",
    );
    assert_eq!(
        replayed(&test_run(&root, &[])),
        1,
        "only the untouched file replays"
    );
}

#[test]
fn editing_a_module_a_test_requires_misses_for_that_test() {
    let root = project("edit-module");
    test_run(&root, &[]);
    assert_eq!(replayed(&test_run(&root, &[])), 2);

    write(
        &root.join("src/adder.tl"),
        "local record adder\nend\nfunction adder.add(a: integer, b: integer): integer\n   return b + a\nend\nreturn adder\n",
    );
    assert_eq!(
        replayed(&test_run(&root, &[])),
        1,
        "the test requiring it is checked again; the one that does not is not"
    );
}

/// A preloaded module must never be a stale one. The whole store exists to make "generated
/// earlier" and "generated now" the same thing; if it ever stops being true, a test asserts
/// against code that is no longer there.
#[test]
fn a_module_edited_between_runs_is_not_served_from_a_preload() {
    let root = project("stale");
    assert_eq!(passed(&test_run(&root, &[])), 2);
    assert_eq!(replayed(&test_run(&root, &[])), 2);

    // `add` now returns something the assertion does not expect. If a preload served the old
    // generated Lua, the test would keep passing and the cache would be lying.
    write(
        &root.join("src/adder.tl"),
        "local record adder\nend\nfunction adder.add(a: integer, b: integer): integer\n   return 99\nend\nreturn adder\n",
    );
    let v = test_run(&root, &[]);
    assert_eq!(
        v["summary"]["failed"], 1,
        "the edited module must reach the test: {v}"
    );
}

/// The test library is put into `package.preload` by the runner. Generated Lua for a module
/// of the same name is a different thing, and displacing it makes every test quietly stop
/// counting — which is how this was found.
#[test]
fn preloading_never_displaces_what_the_runner_installed() {
    let root = project("no-displace");
    let first = test_run(&root, &[]);
    assert_eq!(passed(&first), 2);
    let second = test_run(&root, &[]);
    assert_eq!(replayed(&second), 2, "this run preloads");
    assert_eq!(
        passed(&second),
        2,
        "and the assertion library still reports its results"
    );
}

#[test]
fn no_cache_neither_reads_nor_writes() {
    let root = project("off");
    test_run(&root, &["--no-cache"]);
    assert!(
        !root.join(".htl").exists(),
        "--no-cache must not create a store"
    );

    test_run(&root, &[]);
    assert_eq!(replayed(&test_run(&root, &[])), 2);
    assert_eq!(
        replayed(&test_run(&root, &["--no-cache"])),
        0,
        "--no-cache must not read entries that are sitting there"
    );
}
