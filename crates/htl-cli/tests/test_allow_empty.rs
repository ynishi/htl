//! `htl test` finding no test file: refused by default, passed with `--allow-empty`, and
//! reported either way — a summary line on a terminal, one document on stdout under
//! `--format json` — so that only the exit code says which.
//!
//! A test file is found by what it loads, not by its name, so the default refusal is the
//! guard against a wrong directory or `--lib` that finds nothing. These cases hold it in
//! place and check the flag does not also loosen a run that has files.

use std::path::Path;
use std::process::{Command, Output};

mod common;

const NOTICE: &str =
    "htl test: no test files found (looked for .tl files that require(\"htl.test\"))";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A library with a module and no `.tl` that loads `htl.test`.
fn untested(name: &str) -> common::TempDir {
    let root = common::tempdir("htl-cli-allow-empty", name);
    write(&root.join("htl.toml"), "[check]\n");
    write(
        &root.join("src/adder.tl"),
        "local record adder\nend\nfunction adder.add(a: integer, b: integer): integer\n   return a + b\nend\nreturn adder\n",
    );
    root
}

fn htl(args: &[&str], cwd: &Path) -> Output {
    Command::new(common::htl_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The document on stdout, asserting there is exactly one and that its summary counts
/// nothing.
fn empty_document(out: &Output) -> serde_json::Value {
    let v: serde_json::Value = serde_json::from_str(&stdout(out)).unwrap_or_else(|e| {
        panic!(
            "stdout is one JSON document ({e}):\n{}\n--- stderr ---\n{}",
            stdout(out),
            stderr(out)
        )
    });
    let s = &v["summary"];
    for key in [
        "files",
        "files_run",
        "passed",
        "failed",
        "files_with_errors",
    ] {
        assert_eq!(s[key], 0, "summary.{key} of an empty run: {v}");
    }
    assert_eq!(v["files"], serde_json::json!([]), "no file reports: {v}");
    v
}

#[test]
fn finding_no_test_file_still_fails_by_default() {
    let root = untested("default");
    let out = htl(&["test", "."], &root);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(
        stderr(&out).trim_end(),
        NOTICE,
        "the notice alone, as before the flag existed"
    );
    assert_eq!(stdout(&out), "", "text mode keeps stdout empty");
}

#[test]
fn allow_empty_passes_a_run_that_found_nothing_and_prints_its_summary() {
    let root = untested("text");
    let out = htl(&["test", "--allow-empty", "."], &root);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(0), "{err}");
    let notice = err.find(NOTICE).expect("the notice is still printed");
    let summary = err
        .find("htl test: 0 file(s), 0 passed, 0 failed, 0 file(s) with errors (")
        .unwrap_or_else(|| panic!("the summary a real run prints:\n{err}"));
    assert!(notice < summary, "notice first, then the summary:\n{err}");
    assert!(
        !err.contains("seed"),
        "no file drew from a seed, so none is offered to repeat:\n{err}"
    );
    assert_eq!(stdout(&out), "", "text mode keeps stdout empty");
}

#[test]
fn json_prints_one_empty_document_whether_or_not_empty_is_allowed() {
    let root = untested("json");
    let allowed = htl(&["test", "--allow-empty", "--format", "json", "."], &root);
    assert_eq!(allowed.status.code(), Some(0), "{}", stderr(&allowed));
    let a = empty_document(&allowed);
    assert_eq!(a["summary"]["ok"], true);

    let refused = htl(&["test", "--format", "json", "."], &root);
    assert_eq!(refused.status.code(), Some(1), "{}", stderr(&refused));
    let mut r = empty_document(&refused);
    assert_eq!(
        r["summary"]["ok"], false,
        "`ok` agrees with the exit code, so a reader of the document is refused too"
    );
    r["summary"]["ok"] = serde_json::json!(true);
    assert_eq!(
        a, r,
        "otherwise the same document; only the verdict depends on the flag"
    );
    assert!(stderr(&refused).contains(NOTICE), "{}", stderr(&refused));
}

#[test]
fn allow_empty_does_not_pass_a_failing_test() {
    let root = untested("failing");
    write(
        &root.join("tests/adder_test.tl"),
        "local t = require(\"htl.test\")\nlocal adder = require(\"adder\")\n\
         t.it(\"adds\", function() t.expect(adder.add(1, 2)):to_equal(4) end)\n",
    );
    let out = htl(&["test", "--allow-empty", "."], &root);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("FAIL tests/adder_test.tl"), "{err}");
    assert!(
        err.contains("htl test: 1 file(s), 0 passed, 1 failed, 1 file(s) with errors"),
        "{err}"
    );
    assert!(!err.contains(NOTICE), "{err}");
}
