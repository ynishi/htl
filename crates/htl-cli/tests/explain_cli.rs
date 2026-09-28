//! `htl check --explain <rule>`: a rule's explanation, reachable by the name a finding
//! prints; every rule `--list-lints` names has one.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn htl(args: &[&str], cwd: &Path) -> (bool, i32, String, String) {
    let out = Command::new(common::htl_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    (
        out.status.success(),
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn scratch() -> PathBuf {
    common::scratch("htl-explain", "cwd")
}

#[test]
fn a_rule_is_explained_by_the_name_a_finding_prints() {
    let (ok, code, stdout, stderr) = htl(&["check", "--explain", "union-exhaustive"], &scratch());
    assert!(ok, "{stderr}");
    assert_eq!(code, 0);
    assert!(
        stdout.starts_with("union-exhaustive  (default: warn)\n\n"),
        "{stdout}"
    );
    // The judgment call, which the finding's message has no room for.
    assert!(stdout.contains("do the variants hold"), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn an_unknown_rule_is_refused_on_stderr_with_exit_2() {
    let (ok, code, stdout, stderr) = htl(&["check", "--explain", "no-such-rule"], &scratch());
    assert!(!ok);
    assert_eq!(code, 2);
    assert_eq!(stderr, "unknown lint rule: no-such-rule\n");
    assert!(stdout.is_empty(), "{stdout}");
}

#[test]
fn a_fix_class_is_not_a_lint_and_is_refused() {
    let (_, code, _, stderr) = htl(&["check", "--explain", "forward-ref"], &scratch());
    assert_eq!(code, 2, "{stderr}");
}

#[test]
fn every_rule_the_listing_names_is_explained() {
    let (ok, _, stdout, stderr) = htl(&["check", "--list-lints"], &scratch());
    assert!(ok, "{stderr}");
    let mut n = 0;
    for line in stdout.lines() {
        let Some(name) = line.split_whitespace().next() else {
            continue;
        };
        let (ok, _, out, stderr) = htl(&["check", "--explain", name], &scratch());
        assert!(ok, "{name}: {stderr}");
        assert!(
            out.starts_with(&format!("{name}  (default: ")),
            "{name}: {out}"
        );
        n += 1;
    }
    assert_eq!(n, 33, "the listing names the whole lint surface");
}
