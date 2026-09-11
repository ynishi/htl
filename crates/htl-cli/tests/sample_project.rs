//! The sample `.tl` fixtures, driven through the real binary.
//!
//! These cases were 20-odd lines of bash inside `just e2e`, run against `examples/tl/`. The
//! files they read now live under `tests/fixtures/` beside the tests that read them, and the
//! `grep -q` that stood for each assertion is an `assert!` that prints the run it judged —
//! so a red case names a file, a line and what it saw, rather than reporting that a grep
//! exited 1.
//!
//! Everything the CLI prints for a person goes to **stderr**; stdout is kept for
//! `--format json`. So each run below is captured whole rather than by stream. That
//! distinction cost a failed run when the recipe was first written, and it is the reason
//! `run()` returns one string instead of two.
//!
//! The two `failing/` fixtures had no caller at all before this file: they were checked in
//! to be run by hand and nothing ever ran them.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-sample", name)
}

/// A fixture's absolute path. The tests run from a scratch directory rather than from the
/// checkout, so every path handed to the binary has to be absolute.
fn fixture(rel: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

/// The three modules of the sample: a library, a program requiring it, and its test suite.
/// `main.tl` says `require("util")`, so the set is also a resolution case.
fn sample() -> Vec<String> {
    ["sample/util.tl", "sample/main.tl", "sample/util_test.tl"]
        .iter()
        .map(|f| fixture(f))
        .collect()
}

/// Run the binary in `cwd` and return whether it succeeded together with everything it
/// wrote — stdout and then stderr, joined into one string. The order between the two is not
/// the order a terminal would have shown, because they are collected separately; what the
/// assertions below ask is which words appeared, never which stream carried them.
fn run(args: &[&str], cwd: &Path) -> (bool, String) {
    let out = Command::new(common::htl_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

fn run_with(args: &[&str], extra: &[String], cwd: &Path) -> (bool, String) {
    let mut all: Vec<&str> = args.to_vec();
    all.extend(extra.iter().map(String::as_str));
    run(&all, cwd)
}

/// The whole sample checks clean. A fixture that reported anything would make every case
/// below ambiguous, so this is the one that has to stay quiet.
#[test]
fn the_sample_modules_check_clean() {
    let dir = scratch("check");
    let (ok, out) = run_with(&["check"], &sample(), &dir);
    assert!(ok, "the sample must check clean:\n{out}");
    assert!(
        out.contains("3 file(s), 0 error(s), 0 warning(s), 0 lint(s)"),
        "all three modules were checked and nothing was reported:\n{out}"
    );
}

/// `htl test` on the suite that is meant to pass. The counts are asserted rather than just
/// the exit code: a suite whose cases stopped being collected would exit zero too.
#[test]
fn the_sample_suite_passes() {
    let dir = scratch("test");
    let (ok, out) = run(&["test", &fixture("sample/util_test.tl")], &dir);
    assert!(ok, "the sample suite must pass:\n{out}");
    assert!(
        out.contains("4 passed, 0 failed"),
        "all four cases ran and passed:\n{out}"
    );
}

/// The run cache, over the same three files twice: once against a store that does not exist
/// yet, once against the one the first run left.
///
/// Two claims per run, and only the second is the point. `--explain-cache` prints the
/// store's own counters, and the check prints its summary, which ends in `[cached]` only
/// when every file in the walk was replayed rather than checked. A store that was *written*
/// and a run that was *answered out of it* are different facts, and a case asserting only
/// the first would pass while the cache saved nothing.
///
/// A store lives beside the `htl.toml` a project has; a bare set of files has none, so the
/// CLI falls back to the working directory. That is why this runs from a scratch directory
/// — it is what makes "cold" mean cold on a machine that has run this suite before, and it
/// leaves nothing behind in the checkout.
#[test]
fn the_second_check_is_answered_out_of_the_store() {
    let dir = scratch("cache");
    let files = sample();

    let (ok, cold) = run_with(&["check", "--explain-cache"], &files, &dir);
    assert!(ok, "{cold}");
    assert!(
        cold.contains("htl cache: 0 hit, 0 missed, 3 written,"),
        "a cold store answers nothing and records all three modules:\n{cold}"
    );
    assert!(
        !cold.contains("[cached]"),
        "the first run has nothing to replay:\n{cold}"
    );

    let (ok, warm) = run_with(&["check", "--explain-cache"], &files, &dir);
    assert!(ok, "{warm}");
    assert!(
        warm.contains("htl cache: 3 hit, 0 missed, 0 written,"),
        "the second run reads all three back and writes nothing:\n{warm}"
    );
    assert!(
        warm.lines().any(|l| l.ends_with(" [cached]")),
        "and its summary says the whole walk was replayed:\n{warm}"
    );
}

/// The store from the outside, which is the other half of the same fact: three modules were
/// checked, so three entries are what a second run would have to read.
#[test]
fn cache_status_reports_what_the_check_wrote() {
    let dir = scratch("status");
    let (ok, check) = run_with(&["check"], &sample(), &dir);
    assert!(ok, "{check}");

    let (ok, status) = run(&["cache", "status"], &dir);
    assert!(ok, "{status}");
    assert!(
        status.contains("htl cache: 3 entries,"),
        "one entry per module checked:\n{status}"
    );
}

/// `failing/bad_test.tl` fails at run time: its assertions are well typed and two of them
/// are wrong. Both failures have to be named — a runner that stopped at the first would
/// still exit non-zero — and the file it stopped over has to be counted as one with errors.
#[test]
fn a_failing_suite_names_every_failure_and_exits_non_zero() {
    let dir = scratch("failing");
    let (ok, out) = run(&["test", &fixture("failing/bad_test.tl")], &dir);
    assert!(!ok, "a suite with failing cases must not exit zero:\n{out}");
    assert!(
        out.contains("fails on purpose: expected 3, got 2"),
        "the wrong equality is named with both values:\n{out}"
    );
    assert!(
        out.contains(
            "expected an error but none came: expected an error, but the function returned"
        ),
        "the assertion that expected a raise is named too:\n{out}"
    );
    assert!(
        out.contains("1 passed, 2 failed"),
        "the case that passes still passes; both failures are counted:\n{out}"
    );
}

/// `failing/typed_test.tl` never gets as far as running: `Expect<integer>:to_equal` is handed
/// a string, and that is wrong before anything executes. The counts are what proves the
/// refusal — nothing passed *and* nothing failed, because no case ran.
#[test]
fn a_suite_that_does_not_type_check_is_refused_before_it_runs() {
    let dir = scratch("typed");
    let (ok, out) = run(&["test", &fixture("failing/typed_test.tl")], &dir);
    assert!(!ok, "a suite that does not type check must fail:\n{out}");
    assert!(
        out.contains("got string \"2\", expected integer"),
        "the type error is reported as a type error:\n{out}"
    );
    assert!(
        out.contains("type check failed"),
        "and the file is marked as one that never ran:\n{out}"
    );
    assert!(
        out.contains("0 passed, 0 failed"),
        "no case ran, so there is nothing to count either way:\n{out}"
    );
}
