//! What `htl test --coverage` and `--coverage-lines` print, through the real binary.
//!
//! A percentage says how much of a module was missed; the `never ran:` line says what.

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "htl-cli-cov-{name}-{}-{}",
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
