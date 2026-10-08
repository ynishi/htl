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
    assert_eq!(n, 35, "the listing names the whole lint surface");
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A run with findings names the flag once per rule, after the findings and before the
/// summary; a clean run and a json run do not.
#[test]
fn a_run_with_findings_names_the_flag_once_per_rule_before_the_summary() {
    let root = common::scratch("htl-explain", "hint");
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    // Two rules: a lint (`no-global`) and one of the compiler's kinds (`tl:unused`), the
    // global twice so that "once per rule" is tested rather than "once per finding".
    write(
        &root.join("src/main.tl"),
        "global g1 = 1\nglobal g2 = 2\nlocal unused = 3\nprint(g1 + g2)\n",
    );
    let (ok, _, _, stderr) = htl(&["check", "."], &root);
    assert!(ok, "warn is not a failure: {stderr}");
    let lines: Vec<&str> = stderr.lines().collect();
    let hints: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| l.starts_with("htl check --explain "))
        .collect();
    assert_eq!(
        hints,
        [
            "htl check --explain no-global",
            "htl check --explain tl:unused"
        ],
        "{stderr}"
    );
    let first_hint = lines
        .iter()
        .position(|l| l.starts_with("htl check --explain "))
        .unwrap();
    let last_finding = lines
        .iter()
        .rposition(|l| l.starts_with("lint: ") || l.starts_with("warning: "))
        .unwrap();
    let summary = lines
        .iter()
        .position(|l| l.starts_with("htl check: "))
        .unwrap();
    assert!(
        last_finding < first_hint && first_hint < summary,
        "{stderr}"
    );
    assert_eq!(
        summary,
        lines.len() - 1,
        "the summary is the last line: {stderr}"
    );

    // The same project through `--format json`: no hint, the document is the output.
    let (_, _, stdout, stderr) = htl(&["check", ".", "--format", "json"], &root);
    assert!(!stderr.contains("--explain"), "{stderr}");
    assert!(stdout.starts_with("{"), "{stdout}");

    // A clean run names nothing.
    write(&root.join("src/main.tl"), "print(1)\n");
    let (ok, _, _, stderr) = htl(&["check", "."], &root);
    assert!(ok, "{stderr}");
    assert!(!stderr.contains("--explain"), "{stderr}");
}
