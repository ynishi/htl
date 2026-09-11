//! What `htl test --coverage` and `--coverage-lines` print, through the real binary.
//!
//! A percentage says how much of a module was missed; the `never ran:` line says what.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-cov", name)
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

/// `helper` is called through `hit`; `resolve_counter` is not called at all; `tiny` is
/// not called either but is written on one line, where the body has no span of its own.
fn project() -> PathBuf {
    let root = scratch("neverran");
    write(
        &root.join("src/combat.tl"),
        "local record combat\nend\n\n\
         local function helper(n: integer): integer\n   return n - 1\nend\n\n\
         function combat.hit(n: integer): integer\n   return helper(n) + 1\nend\n\n\
         function combat.resolve_counter(n: integer): integer\n   local x = n * 2\n   return x\nend\n\n\
         function combat.tiny(): integer return 0 end\n\n\
         return combat\n",
    );
    write(
        &root.join("tests/combat_test.tl"),
        "local t = require(\"htl.test\")\nlocal combat = require(\"combat\")\n\
         t.it(\"hit\", function() t.expect(combat.hit(2)):to_equal(2) end)\n",
    );
    root
}

#[test]
fn coverage_names_the_functions_that_never_ran() {
    let root = project();
    let (ok, _stdout, stderr) = htl(&["test", "tests", "--coverage"], &root);
    assert!(ok, "the suite passes: {stderr}");
    assert!(
        stderr.contains("never ran: combat.resolve_counter (12)"),
        "the uncalled function is named, at its own line: {stderr}"
    );
    assert!(
        !stderr.contains("combat.hit"),
        "a function that ran is not listed: {stderr}"
    );
    assert!(
        !stderr.contains("helper"),
        "a function reached through another one ran: {stderr}"
    );
    assert!(
        !stderr.contains("combat.tiny"),
        "a one-line function has no body span and is left out: {stderr}"
    );
    assert!(
        !stderr.contains("unexecuted:"),
        "the line ranges still need --coverage-lines: {stderr}"
    );
}

/// The `N/M` a text table row reports for `path`.
fn table_row(stderr: &str, path: &str) -> (usize, usize) {
    let line = stderr
        .lines()
        .find(|l| l.starts_with("coverage:") && l.contains(path))
        .unwrap_or_else(|| panic!("no table row for {path}: {stderr}"));
    let frac = line
        .split_whitespace()
        .find(|w| w.contains('/') && w.chars().all(|c| c.is_ascii_digit() || c == '/'))
        .unwrap();
    let (a, b) = frac.split_once('/').unwrap();
    (a.parse().unwrap(), b.parse().unwrap())
}

/// The `KEY:` value of the one record in an lcov file.
fn field<'a>(lcov: &'a str, key: &str) -> &'a str {
    let prefix = format!("{key}:");
    lcov.lines()
        .find_map(|l| l.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("no {key} record: {lcov}"))
}

#[test]
fn lcov_is_the_table_in_tracefile_form() {
    let root = project();
    let (ok, _stdout, stderr) = htl(&["test", "tests", "--lcov", "cov.info"], &root);
    assert!(ok, "the suite passes: {stderr}");
    assert!(
        stderr.contains("never ran: combat.resolve_counter (12)"),
        "--lcov implies --coverage, so the table is still printed: {stderr}"
    );
    assert!(stderr.contains("lcov written to cov.info"), "{stderr}");
    let lcov = std::fs::read_to_string(root.join("cov.info")).unwrap();
    assert_eq!(lcov.matches("end_of_record\n").count(), 1, "{lcov}");
    assert_eq!(field(&lcov, "SF"), "src/combat.tl", "{lcov}");
    // Functions: the two that ran, the one that did not; `tiny` has no body span.
    assert!(lcov.contains("FN:12,combat.resolve_counter\n"), "{lcov}");
    assert!(lcov.contains("FNDA:0,combat.resolve_counter\n"), "{lcov}");
    assert!(lcov.contains("FNDA:1,combat.hit\n"), "{lcov}");
    assert!(lcov.contains("FNDA:1,helper\n"), "{lcov}");
    assert!(!lcov.contains("combat.tiny"), "{lcov}");
    assert_eq!(field(&lcov, "FNF"), "3", "{lcov}");
    assert_eq!(field(&lcov, "FNH"), "2", "{lcov}");
    // Lines: a 0/1 count per line a statement starts on.
    assert!(
        lcov.contains("DA:13,0\n"),
        "the body of resolve_counter: {lcov}"
    );
    assert!(lcov.contains("DA:9,1\n"), "the body of hit: {lcov}");
    assert!(!lcov.contains("BRDA:"), "no branch data is claimed: {lcov}");
    // The table counts statements, the tracefile counts lines: line 17 holds two
    // (`tiny`'s declaration and its `return 0`) and is one `DA`, so LF is one short of
    // the table's total and LH one short of its executed. Everything else agrees.
    let (executed, total) = table_row(&stderr, "combat.tl");
    assert_eq!((executed, total), (8, 10), "{stderr}");
    assert_eq!(
        lcov.matches("DA:17,").count(),
        1,
        "one entry for the line: {lcov}"
    );
    assert_eq!(
        field(&lcov, "LF"),
        (total - 1).to_string(),
        "{lcov}\n{stderr}"
    );
    assert_eq!(
        field(&lcov, "LH"),
        (executed - 1).to_string(),
        "{lcov}\n{stderr}"
    );
    // The order lcov's own tools write: FN/FNDA before DA, the counts after each.
    let (fn_at, da_at, lf_at) = (
        lcov.find("FN:").unwrap(),
        lcov.find("DA:").unwrap(),
        lcov.find("LF:").unwrap(),
    );
    assert!(fn_at < da_at && da_at < lf_at, "{lcov}");
}

#[test]
fn lcov_paths_are_relative_to_the_project_root_not_the_cwd() {
    let root = project();
    write(&root.join("htl.toml"), "");
    let (ok, _stdout, stderr) = htl(&["test", "tests", "--lcov", "from-root.info"], &root);
    assert!(ok, "{stderr}");
    let (ok, _stdout, stderr) = htl(
        &["test", ".", "--lcov", "../from-tests.info"],
        &root.join("tests"),
    );
    assert!(ok, "{stderr}");
    let a = std::fs::read_to_string(root.join("from-root.info")).unwrap();
    let b = std::fs::read_to_string(root.join("from-tests.info")).unwrap();
    assert_eq!(field(&a, "SF"), "src/combat.tl", "{a}");
    assert_eq!(a, b, "the same file from the root and from a subdirectory");
}

#[test]
fn coverage_lines_keeps_its_ranges_under_the_names() {
    let root = project();
    let (ok, _stdout, stderr) = htl(&["test", "tests", "--coverage", "--coverage-lines"], &root);
    assert!(ok, "the suite passes: {stderr}");
    let names = stderr
        .find("never ran:")
        .unwrap_or_else(|| panic!("no names: {stderr}"));
    let ranges = stderr
        .find("unexecuted:")
        .unwrap_or_else(|| panic!("no ranges: {stderr}"));
    assert!(
        names < ranges,
        "the names come first, the line numbers under them: {stderr}"
    );
    assert!(
        stderr.contains("never ran: combat.resolve_counter (12)"),
        "{stderr}"
    );
}
