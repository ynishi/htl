//! `htl build` and the run cache, through the real binary: the second build replays, says
//! so the way `htl check` does, and writes the same bundle; `--no-cache` stays silent; and
//! the `gen` entries `htl test` writes are the ones `htl build` reads, and the reverse.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-build-cache", name)
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

/// main -> util, with a test file for util so `htl test` has something to store.
fn project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), "[check]\n");
    write(
        &root.join("src/main.tl"),
        "local util = require(\"util\")\nprint(util.twice(21))\n",
    );
    write(
        &root.join("src/util.tl"),
        "local record util\nend\nfunction util.twice(n: integer): integer\n   return n * 2\nend\nreturn util\n",
    );
    write(
        &root.join("src/util_test.tl"),
        "local t = require(\"htl.test\")\nlocal util = require(\"util\")\nt.it(\"doubles\", function() t.expect(util.twice(2)):to_equal(4) end)\n",
    );
    root
}

fn build(root: &Path, extra: &[&str]) -> (bool, String, Vec<u8>) {
    let mut args = vec!["build", "src/main.tl", "-o", "app.hb"];
    args.extend_from_slice(extra);
    let (ok, _, stderr) = htl(&args, root);
    let bytes = std::fs::read(root.join("app.hb")).unwrap_or_default();
    (ok, stderr, bytes)
}

#[test]
fn the_second_build_replays_and_writes_the_same_bundle() {
    let root = project("replay");
    let (ok, first, bytes) = build(&root, &[]);
    assert!(ok, "{first}");
    assert!(!first.contains("cached"), "cold: {first}");
    let (ok, second, again) = build(&root, &[]);
    assert!(ok, "{second}");
    assert!(second.contains("[cached]"), "warm: {second}");
    assert_eq!(
        bytes, again,
        "the bundle is the same whether replayed or generated"
    );

    let (ok, third, _) = build(&root, &["--no-cache"]);
    assert!(ok, "{third}");
    assert!(
        !third.contains("cached"),
        "--no-cache neither reads nor says: {third}"
    );
}

#[test]
fn an_edit_replays_only_what_did_not_read_it() {
    let root = project("edit");
    build(&root, &[]);
    write(
        &root.join("src/main.tl"),
        "local util = require(\"util\")\nprint(util.twice(42))\n",
    );
    let (ok, stderr, _) = build(&root, &[]);
    assert!(ok, "{stderr}");
    assert!(stderr.contains("[1/2 cached]"), "{stderr}");
    write(
        &root.join("src/util.tl"),
        "local record util\nend\nfunction util.twice(n: integer): integer\n   return n + n\nend\nreturn util\n",
    );
    let (ok, stderr, _) = build(&root, &[]);
    assert!(ok, "{stderr}");
    assert!(!stderr.contains("cached"), "{stderr}");
}

#[test]
fn a_test_run_feeds_the_build_and_the_build_feeds_the_test_run() {
    let root = project("shared");
    let (ok, _, stderr) = htl(&["test", "--explain-cache"], &root);
    assert!(ok, "{stderr}");
    // `htl test` stored util's generated form while harvesting the test file's closure.
    let (ok, stderr, _) = build(&root, &[]);
    assert!(ok, "{stderr}");
    assert!(
        stderr.contains("[1/2 cached]"),
        "util from the test run's entry, main generated: {stderr}"
    );

    // And the other way. The test file itself has never run, so it is generated; util's
    // entry, which the build wrote, still holds, so the harvest finds it (one hit) and
    // stores the rest of the closure around it — and the run after that replays the test
    // file with util preloaded from that same entry.
    let root = project("shared-back");
    build(&root, &[]);
    let (ok, _, stderr) = htl(&["test", "--explain-cache"], &root);
    assert!(ok, "{stderr}");
    assert!(
        stderr.contains("htl cache: 1 hit,"),
        "util's entry came from the build: {stderr}"
    );
    let (ok, _, stderr) = htl(&["test", "--explain-cache"], &root);
    assert!(ok, "{stderr}");
    let preloading = stderr
        .lines()
        .find(|l| l.starts_with("htl cache: preloading"))
        .unwrap_or_else(|| panic!("the test file replays with preloads: {stderr}"));
    assert!(preloading.contains("util"), "{preloading}");
}
