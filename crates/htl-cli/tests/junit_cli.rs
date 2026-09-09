//! What `htl test --junit <file>` writes, through the real binary.
//!
//! The report is read by a CI, not a person, so what is asserted here is that it says the
//! same thing the summary line does — the same cases, the same failures, the same
//! durations — and that a message full of markup survives as data.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-junit", name)
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

/// Two passing tests and one failing one, in two files.
fn passing_and_failing() -> PathBuf {
    let root = scratch("mixed");
    write(
        &root.join("tests/math_test.tl"),
        "local t = require(\"htl.test\")\n\
         t.describe(\"add\", function()\n\
         \x20  t.it(\"adds\", function() t.expect(1 + 1):to_equal(2) end)\n\
         \x20  t.it(\"still adds\", function() t.expect(2 + 2):to_equal(4) end)\n\
         end)\n",
    );
    write(
        &root.join("tests/bad_test.tl"),
        "local t = require(\"htl.test\")\n\
         t.it(\"is wrong\", function() t.expect(1):to_equal(2) end)\n",
    );
    root
}

/// The `n` of `<... name="x" ...>` style attributes, as a plain substring count.
fn count(xml: &str, needle: &str) -> usize {
    xml.matches(needle).count()
}

/// The value of `attr` on the first element opened by `open` (`"<testsuites"`, say).
fn attr_of(xml: &str, open: &str, attr: &str) -> String {
    let el = &xml[xml.find(open).expect("the element is there")..];
    let el = &el[..el.find('>').unwrap()];
    let key = format!("{attr}=\"");
    let at = el.find(&key).unwrap_or_else(|| panic!("{attr} in {el}")) + key.len();
    el[at..][..el[at..].find('"').unwrap()].to_string()
}

#[test]
fn the_report_totals_are_the_summary_line() {
    let root = passing_and_failing();
    let out = root.join("junit.xml");
    let (ok, _stdout, stderr) = htl(&["test", "tests", "--junit", out.to_str().unwrap()], &root);
    assert!(!ok, "one test fails, so the run does: {stderr}");
    let xml = std::fs::read_to_string(&out).unwrap();

    assert!(stderr.contains("2 passed, 1 failed"), "summary: {stderr}");
    assert_eq!(attr_of(&xml, "<testsuites", "tests"), "3", "{xml}");
    assert_eq!(attr_of(&xml, "<testsuites", "failures"), "1", "{xml}");
    assert_eq!(attr_of(&xml, "<testsuites", "errors"), "0", "{xml}");
    assert_eq!(count(&xml, "<testsuite "), 2, "one suite per file: {xml}");
    assert_eq!(count(&xml, "<testcase "), 3, "one case per test: {xml}");
    assert_eq!(count(&xml, "<failure "), 1, "{xml}");
    assert!(
        xml.contains("classname=\"tests/math_test.tl\" name=\"add &gt; adds\""),
        "the file is the classname and the composed name is the name: {xml}"
    );
    assert!(
        xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites "),
        "{xml}"
    );
    assert!(xml.trim_end().ends_with("</testsuites>"), "{xml}");
}

#[test]
fn the_file_duration_the_text_prints_is_the_suite_time() {
    let root = passing_and_failing();
    let out = root.join("junit.xml");
    let (_ok, _stdout, stderr) = htl(&["test", "tests", "--junit", out.to_str().unwrap()], &root);
    let xml = std::fs::read_to_string(&out).unwrap();
    // `ok   tests/math_test.tl  (2 passed, 0 failed, 7 ms)` against `time="0.007"`.
    let line = stderr
        .lines()
        .find(|l| l.contains("tests/math_test.tl"))
        .unwrap_or_else(|| panic!("the file's line: {stderr}"));
    let ms: f64 = line
        .rsplit_once(", ")
        .unwrap()
        .1
        .trim_end_matches(" ms)")
        .parse()
        .unwrap_or_else(|e| panic!("{e} in {line}"));
    let suite = &xml[xml.find("name=\"tests/math_test.tl\"").unwrap()..];
    let secs: f64 = {
        let at = suite.find("time=\"").unwrap() + 6;
        suite[at..][..suite[at..].find('"').unwrap()]
            .parse()
            .unwrap()
    };
    assert!(
        (secs * 1000.0 - ms).abs() <= 1.0,
        "the suite's time is the file's duration to the millisecond: {secs}s vs {ms}ms\n{xml}"
    );
}

#[test]
fn a_file_that_does_not_type_check_is_a_suite_with_an_error() {
    let root = scratch("badcheck");
    write(
        &root.join("tests/broken_test.tl"),
        "local t = require(\"htl.test\")\nlocal x: integer = \"not a number\"\n\
         t.it(\"never runs\", function() t.expect(x):to_equal(1) end)\n",
    );
    let out = root.join("junit.xml");
    let (ok, _stdout, stderr) = htl(&["test", "tests", "--junit", out.to_str().unwrap()], &root);
    assert!(!ok, "a file that does not check fails the run: {stderr}");
    let xml = std::fs::read_to_string(&out).unwrap();
    assert!(
        xml.contains("<error message=\"type check failed\" type=\"check\">"),
        "the suite carries an error, not a failure: {xml}"
    );
    assert_eq!(
        count(&xml, "<failure "),
        0,
        "not an assertion failure: {xml}"
    );
    assert_eq!(
        count(&xml, "<testcase "),
        0,
        "its cases could not run: {xml}"
    );
    assert_eq!(attr_of(&xml, "<testsuite ", "errors"), "1", "{xml}");
    assert_eq!(attr_of(&xml, "<testsuite ", "tests"), "0", "{xml}");
    assert!(
        xml.contains("broken_test.tl:2"),
        "the diagnostics are the error's body: {xml}"
    );
}

#[test]
fn a_message_full_of_markup_survives_as_data() {
    let root = scratch("escaping");
    // `<`, `&`, both quotes, and a raised error, which carries a traceback.
    write(
        &root.join("tests/nasty_test.tl"),
        "local t = require(\"htl.test\")\n\
         t.it(\"raises\", function() error(\"<x> & \\\"y\\\" and 'z'\") end)\n",
    );
    let out = root.join("junit.xml");
    let (ok, _stdout, stderr) = htl(&["test", "tests", "--junit", out.to_str().unwrap()], &root);
    assert!(!ok, "{stderr}");
    let xml = std::fs::read_to_string(&out).unwrap();
    assert!(
        xml.contains("&lt;x&gt; &amp; &quot;y&quot; and &apos;z&apos;"),
        "every one of them is escaped in the attribute: {xml}"
    );
    assert!(
        xml.contains("&lt;x&gt; &amp; \"y\" and 'z'"),
        "and in the body, where only markup has to be: {xml}"
    );
    assert!(
        xml.contains("stack traceback:"),
        "the traceback the text output prints is in the report: {xml}"
    );
    let at = xml.find("message=\"").unwrap() + 9;
    let value = &xml[at..][..xml[at..].find('"').unwrap()];
    assert!(
        !value.contains('\n') && !value.contains("traceback"),
        "the attribute is the headline; the frames are the body's: {value}"
    );
}

#[test]
fn a_file_with_no_tests_is_a_suite_with_no_cases() {
    let root = scratch("notests");
    write(&root.join("tests/plain_test.tl"), "local x = 1 + 1\n");
    let out = root.join("junit.xml");
    let (ok, _stdout, stderr) = htl(&["test", "tests", "--junit", out.to_str().unwrap()], &root);
    assert!(ok, "it ran to completion: {stderr}");
    let xml = std::fs::read_to_string(&out).unwrap();
    assert_eq!(
        count(&xml, "<testsuite "),
        1,
        "it ran, so it is there: {xml}"
    );
    assert_eq!(
        count(&xml, "<testcase "),
        0,
        "with nothing to report: {xml}"
    );
    assert_eq!(attr_of(&xml, "<testsuites", "skipped"), "0", "{xml}");
    assert_eq!(count(&xml, "<skipped"), 0, "nothing is ever skipped: {xml}");
}

#[test]
fn the_report_describes_the_filtered_run_and_leaves_json_alone() {
    let root = passing_and_failing();
    let plain = root.join("plain.json");
    let with = root.join("with.json");
    let out = root.join("junit.xml");

    let (_ok, before, _e) = htl(&["test", "tests", "--format", "json", "--seed", "7"], &root);
    std::fs::write(&plain, &before).unwrap();
    let (_ok, after, _e) = htl(
        &[
            "test",
            "tests",
            "--format",
            "json",
            "--seed",
            "7",
            "--junit",
            out.to_str().unwrap(),
        ],
        &root,
    );
    std::fs::write(&with, &after).unwrap();
    let strip = |s: &str| {
        s.split(|c: char| c.is_ascii_digit() || c == '.')
            .collect::<String>()
    };
    assert_eq!(
        strip(&before),
        strip(&after),
        "the document on stdout is what it was, but for durations and the seed"
    );
    assert!(out.exists(), "and the report was written beside it");

    let (_ok, _stdout, stderr) = htl(
        &[
            "test",
            "tests",
            "--filter",
            "still adds",
            "--seed",
            "7",
            "--junit",
            out.to_str().unwrap(),
        ],
        &root,
    );
    let xml = std::fs::read_to_string(&out).unwrap();
    assert!(stderr.contains("1 passed, 0 failed"), "{stderr}");
    assert_eq!(attr_of(&xml, "<testsuites", "tests"), "1", "{xml}");
    assert_eq!(count(&xml, "<testcase "), 1, "the run that happened: {xml}");
    assert!(xml.contains("name=\"add &gt; still adds\""), "{xml}");
    assert_eq!(
        count(&xml, "<skipped"),
        0,
        "an excluded test is absent, not skipped: {xml}"
    );
}

#[test]
fn two_runs_over_an_unchanged_project_differ_only_in_durations() {
    let root = passing_and_failing();
    let first = root.join("first.xml");
    let second = root.join("second.xml");
    htl(
        &["test", "tests", "--junit", first.to_str().unwrap()],
        &root,
    );
    htl(
        &["test", "tests", "--junit", second.to_str().unwrap()],
        &root,
    );
    let blank_times = |p: &Path| {
        let s = std::fs::read_to_string(p).unwrap();
        let (mut out, mut rest) = (String::new(), s.as_str());
        while let Some(at) = rest.find("time=\"") {
            out.push_str(&rest[..at + 6]);
            rest = &rest[at + 6..];
            rest = &rest[rest.find('"').unwrap()..];
        }
        out.push_str(rest);
        out
    };
    assert_eq!(
        blank_times(&first),
        blank_times(&second),
        "the same run reported the same way"
    );
    assert_ne!(
        std::fs::read_to_string(&first).unwrap(),
        blank_times(&first),
        "and the durations were there to blank out"
    );
}
