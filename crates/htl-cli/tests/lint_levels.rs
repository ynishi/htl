//! A level per rule, through the binary: what is said, what fails the run, and what the
//! summary says it did.
//!
//! A lint used to be on or off, and every lint that was on weighed the same: the only
//! escalation was `--strict`, which promoted all of them at once. So a project that wanted
//! to see one rule while it migrated, without failing CI on it, had nothing to write —
//! turning the rule on put it on the same footing as `nil-index`.
//!
//! Now every rule has a level. `allow` is not reported, `warn` is reported and advisory,
//! `deny` is reported and fails `htl check`. The project below trips two rules at once, so
//! each test is a run in which they are set differently and the exit code is the answer.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-lint-levels", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// The exit code and stderr: the verdict, and what it was reached on.
fn htl(args: &[&str], cwd: &Path) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_htl"))
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    (
        out.status.code().expect("the run exited, not signalled"),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// One file with three findings and no error: a `nil-index` lint, an `any` the `no-any`
/// rule would report if it were asked to, and an unused local the Teal compiler warns
/// about under `tl:unused`.
fn project(name: &str, htl_toml: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), htl_toml);
    write(
        &root.join("src/rows.tl"),
        "local record rows\nend\n\n\
         function rows.first(t: {string:{string:string}}): string\n   \
         local unused_here = 1\n   local x: any = t[\"a\"].b\n   \
         return x as string\nend\n\nreturn rows\n",
    );
    root
}

/// The case the problem statement is about: one rule failing the run while another is
/// advice, in one run, from `htl.toml`.
#[test]
fn one_rule_fails_the_run_while_another_is_advice() {
    let root = project(
        "mixed",
        "[lint.rules]\nnil-index = \"deny\"\nno-any = \"warn\"\n",
    );
    let (code, err) = htl(&["check", "src", "--no-cache"], &root);
    assert_eq!(
        code, 1,
        "a finding at deny fails the run with no flag: {err}"
    );
    // Both are reported, and each says which rule said it.
    assert!(err.contains("[htl nil-index]"), "{err}");
    assert!(err.contains("[htl no-any]"), "{err}");
    // Two lints and a Teal warning were said; one of them is what the run failed on.
    assert!(
        err.contains("0 error(s), 1 warning(s), 2 lint(s), 1 at deny"),
        "the summary says what the run is: {err}"
    );

    // The same project with the levels the other way round: `no-any` is now the one that
    // stops the run, so the level and not the rule is what decided it.
    let root = project(
        "mixed-swapped",
        "[lint.rules]\nnil-index = \"warn\"\nno-any = \"deny\"\n",
    );
    let (code, err) = htl(&["check", "src", "--no-cache"], &root);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("1 at deny"), "{err}");
}

/// `warn` alone is advice: reported, counted, and the run passes. This is the level every
/// rule htl reports has by default, so it is also the assertion that a project which
/// writes no configuration is judged as it was before levels existed.
#[test]
fn warn_is_reported_and_does_not_fail_the_run() {
    let root = project("warn", "[lint.rules]\nno-any = \"warn\"\n");
    let (code, err) = htl(&["check", "src", "--no-cache"], &root);
    assert_eq!(code, 0, "nothing is at deny: {err}");
    assert!(
        err.contains("[htl nil-index]") && err.contains("[htl no-any]"),
        "{err}"
    );
    assert!(
        err.contains("0 error(s), 1 warning(s), 2 lint(s)") && !err.contains("at deny"),
        "and the summary does not mention a level nothing is at: {err}"
    );

    let bare = project("default", "[check]\n");
    let (code, err) = htl(&["check", "src", "--no-cache"], &bare);
    assert_eq!(code, 0, "the defaults fail on nothing: {err}");
    assert!(
        err.contains("0 error(s), 1 warning(s), 1 lint(s)"),
        "and `no-any` is not reported at all, being allow by default: {err}"
    );
}

/// `allow` is the level at which a finding is not made: not reported, and so not counted,
/// which is what keeps a run's verdict a verdict on what it said.
#[test]
fn allow_makes_a_finding_disappear() {
    let root = project(
        "allow",
        "[lint.rules]\nnil-index = \"allow\"\n\"tl:unused\" = \"allow\"\n",
    );
    let (code, err) = htl(&["check", "src", "--no-cache"], &root);
    assert_eq!(code, 0, "{err}");
    assert!(!err.contains("nil-index"), "{err}");
    assert!(!err.contains("unused_here"), "{err}");
    assert!(
        err.contains("0 error(s), 0 warning(s), 0 lint(s)"),
        "not counted either: {err}"
    );

    // And a silenced rule does not come back under `strict`: promoting `warn` to `deny`
    // says nothing about `allow`.
    let (code, err) = htl(&["check", "src", "--no-cache", "--strict"], &root);
    assert_eq!(code, 0, "a strict run has nothing to fail on: {err}");
}

/// The predicate `strict` states, in the vocabulary levels gave it: for this run, every
/// `warn` counts as `deny`. The project's `warn` rule is what fails, and it is the only
/// thing that changed between the two runs.
#[test]
fn strict_promotes_every_warn_to_deny() {
    let root = project("strict", "[lint.rules]\nno-any = \"warn\"\n");
    let (code, _) = htl(&["check", "src", "--no-cache"], &root);
    assert_eq!(code, 0);

    let (code, err) = htl(&["check", "src", "--no-cache", "--strict"], &root);
    assert_eq!(code, 1, "the warn-level findings now fail it: {err}");
    assert!(err.contains("[strict]"), "{err}");

    // `[lint] strict` says the same thing as the flag, beside a level map.
    let from_file = project(
        "strict-config",
        "[lint]\nstrict = true\n\n[lint.rules]\nno-any = \"warn\"\n",
    );
    let (code, err) = htl(&["check", "src", "--no-cache"], &from_file);
    assert_eq!(code, 1, "{err}");
}

/// A level on the command line, which is the other half of #147's acceptance: the same
/// rule set two ways, and the flag wins because it comes after the file.
#[test]
fn a_level_comes_from_the_command_line_and_beats_the_file() {
    let root = project("flag", "[lint.rules]\nnil-index = \"allow\"\n");
    let (code, err) = htl(&["check", "src", "--no-cache"], &root);
    assert_eq!(code, 0, "{err}");

    let (code, err) = htl(
        &["check", "src", "--no-cache", "--lint", "nil-index=deny"],
        &root,
    );
    assert_eq!(code, 1, "the flag raised what the file silenced: {err}");
    assert!(err.contains("[htl nil-index]"), "{err}");
    assert!(err.contains("1 at deny"), "{err}");

    // `+rule` / `-rule` is the older spelling of `=warn` / `=allow` and still taken, so a
    // spec already in a CI job keeps its meaning.
    let (code, err) = htl(
        &[
            "check",
            "src",
            "--no-cache",
            "--lint",
            "+nil-index,-tl:unused",
        ],
        &root,
    );
    assert_eq!(code, 0, "+ is warn, which does not fail a run: {err}");
    assert!(
        err.contains("[htl nil-index]"),
        "+ turned it back on: {err}"
    );
    assert!(!err.contains("unused_here"), "- silenced it: {err}");

    // A level that is not one is refused rather than ignored: the alternative is a run
    // that passes because a word was misspelt.
    let (code, err) = htl(
        &["check", "src", "--no-cache", "--lint", "nil-index=error"],
        &root,
    );
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("unknown lint level"), "{err}");
    assert!(err.contains("allow, warn or deny"), "{err}");
}

/// `enable` and `disable` are gone, and a project that still writes one is told so where
/// it wrote it — with the lines to write instead, built from its own names.
#[test]
fn the_old_keys_are_refused_with_the_replacement() {
    let root = project("removed", "[lint]\nenable = [\"no-any\"]\n");
    let (code, err) = htl(&["check", "src", "--no-cache"], &root);
    assert_eq!(code, 2, "a removed key is fatal, not ignored: {err}");
    assert!(err.contains("[lint] enable"), "{err}");
    assert!(err.contains("[lint.rules]"), "{err}");
    assert!(
        err.contains("\"no-any\" = \"warn\""),
        "the line to write instead: {err}"
    );

    let root = project("removed-disable", "[lint]\ndisable = [\"nil-index\"]\n");
    let (code, err) = htl(&["check", "src", "--no-cache"], &root);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("\"nil-index\" = \"allow\""), "{err}");
}

/// The listing is where the levels are readable without provoking a finding: every rule,
/// with the level a project that says nothing gets.
#[test]
fn the_listing_says_each_rules_default_level() {
    let dir = scratch("listing");
    let out = Command::new(env!("CARGO_BIN_EXE_htl"))
        .args(["check", "--list-lints"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    let line = |name: &str| {
        said.lines()
            .find(|l| l.starts_with(name) && l[name.len()..].starts_with(' '))
            .unwrap_or_else(|| panic!("{name} not listed in {said}"))
            .to_string()
    };
    assert!(line("nil-index").ends_with("warn"), "{said}");
    assert!(line("no-any").ends_with("allow"), "{said}");
    assert!(line("tl:hint").ends_with("warn"), "{said}");
}
