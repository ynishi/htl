//! `htl check --statistics`: the per-rule tally in place of the findings — ruff's shape,
//! the count first, `[*]`/`[-]`/`[ ]` for how much of a rule's findings a fix covers.

use std::path::Path;
use std::process::Command;

mod common;

fn tempdir(name: &str) -> common::TempDir {
    common::tempdir("htl-cli-check-statistics", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn htl(args: &[&str], cwd: &Path) -> (i32, String, String) {
    let out = Command::new(common::htl_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    (
        out.status.code().expect("the run exited, not signalled"),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Five rules, one row each: `nil-index` (two sites, no fix — `[ ]`), the Teal compiler's
/// `tl:unused` (one unused local, no fix — `[ ]`), `type-guard` (one guard, fix always
/// `safe` — `[*]`), `no-global` (one declaration, fix always `unsafe` — `[-]`), and the
/// type error itself — every error the checker reports is classed `tl:error` or
/// `forward-ref` before it reaches [`htl_cli::report::Out`] (`prelude.lua`'s
/// `collect_errors`), so this one is a `tl:error` row rather than an unclassified one —
/// see `an_error_without_a_rule_is_tallied_under_error` in `report.rs` for that case.
fn project(name: &str) -> common::TempDir {
    let root = tempdir(name);
    write(&root.join("htl.toml"), "");
    // Two `nil-index` sites (index chained on an index), one of them a type error: the
    // second assigns the same shape to an `integer` local instead, so it reports the lint
    // without a second error.
    write(
        &root.join("src/bad.tl"),
        "local t: {string: {integer}} = {}\n\
         local n: string = t[\"a\"][1]\n\
         local k: integer = t[\"b\"][1]\n\
         print(n, k)\n",
    );
    // `tl:unused`: a local nothing reads.
    write(
        &root.join("src/warn.tl"),
        "local record warn\nend\n\
         function warn.f(): integer\n\
         \x20  local unused_here = 1\n\
         \x20  return 1\n\
         end\n\
         return warn\n",
    );
    // `type-guard`: `type(x) == \"string\"` on an `any` narrows nothing Teal reads, and
    // `htl fix` writes `x is string` for it (safe). `no-global`: a `global` declaration,
    // fixed by turning the keyword into `local` (unsafe — the name stops being visible to
    // other chunks).
    write(
        &root.join("src/extra.tl"),
        "local function g(x: any): boolean\n\
         \x20  return type(x) == \"string\"\n\
         end\n\
         global flag: boolean = true\n\
         print(g(flag))\n",
    );
    root
}

/// The table [`project`]'s run prints, count descending then rule ascending, the rule
/// column as wide as `type-guard` (the longest name in it).
const TABLE: &str = "    2  nil-index   [ ]\n\
                      \x20   1  no-global   [-]\n\
                      \x20   1  tl:error    [ ]\n\
                      \x20   1  tl:unused   [ ]\n\
                      \x20   1  type-guard  [*]\n";

/// The `--explain` hints `report_check` names after the table, in `rules_said()`'s order
/// (name order over the tally's keys): `tl:error` is a fix class, not a lint, so
/// `htl::lint::explained` has nothing for it and it names no hint.
const HINTS: &str = "htl check --explain nil-index\n\
                      htl check --explain no-global\n\
                      htl check --explain tl:unused\n\
                      htl check --explain type-guard\n";

#[test]
fn statistics_prints_one_row_per_rule_with_findings_count_first() {
    let root = project("table");
    let (_, _, plain_err) = htl(&["check", "src", "--no-cache"], &root);
    let (code, stdout, stats_err) = htl(&["check", "src", "--no-cache", "--statistics"], &root);
    assert_ne!(code, 0, "the type error still fails the run: {stats_err}");
    assert!(
        stdout.trim().is_empty(),
        "text mode prints nothing on stdout: {stdout}"
    );
    // The table, then the same `--explain` hints and summary line a plain run prints —
    // pinned whole, so the table is shown to replace the findings rather than sit beside
    // them, with nothing else slipped in either side of it.
    let plain_summary = plain_err.lines().last().unwrap();
    let expected = format!("{TABLE}{HINTS}{plain_summary}\n");
    assert_eq!(
        stats_err, expected,
        "plain run (for its summary): {plain_err}"
    );
}

#[test]
fn rules_without_findings_are_not_listed() {
    let root = project("absent");
    let (_, _, stats_err) = htl(&["check", "src", "--no-cache", "--statistics"], &root);
    // `no-any` is `allow` by default and nothing in the fixture asks for it: the fixture
    // never trips it, so it never becomes a key of the tally, and is never a row.
    assert!(
        !stats_err.contains("no-any"),
        "a rule with no finding is not a row: {stats_err}"
    );
}

#[test]
fn the_exit_code_is_unchanged_by_statistics() {
    let root = project("exit-code");
    let (plain_code, _, plain_err) = htl(&["check", "src", "--no-cache", "--strict"], &root);
    let (stats_code, _, stats_err) = htl(
        &["check", "src", "--no-cache", "--strict", "--statistics"],
        &root,
    );
    assert_eq!(
        plain_code, stats_code,
        "plain: {plain_err}\nstatistics: {stats_err}"
    );
}

#[test]
fn json_carries_statistics_and_no_diagnostics() {
    let root = project("json");
    let (plain_code, plain_out, plain_err) =
        htl(&["check", "src", "--no-cache", "--format", "json"], &root);
    let (stats_code, stats_out, stats_err) = htl(
        &[
            "check",
            "src",
            "--no-cache",
            "--format",
            "json",
            "--statistics",
        ],
        &root,
    );
    assert!(plain_err.trim().is_empty(), "{plain_err}");
    assert!(
        stats_err.trim().is_empty(),
        "json mode keeps stderr silent even with --statistics: {stats_err}"
    );
    assert_eq!(plain_code, stats_code);
    let plain: serde_json::Value = serde_json::from_str(&plain_out).unwrap();
    let stats: serde_json::Value = serde_json::from_str(&stats_out).unwrap();
    assert_eq!(
        stats["statistics"],
        serde_json::json!([
            {"rule": "nil-index", "count": 2, "fixable": 0, "unsafe_fixable": 0},
            {"rule": "no-global", "count": 1, "fixable": 0, "unsafe_fixable": 1},
            {"rule": "tl:error", "count": 1, "fixable": 0, "unsafe_fixable": 0},
            {"rule": "tl:unused", "count": 1, "fixable": 0, "unsafe_fixable": 0},
            {"rule": "type-guard", "count": 1, "fixable": 1, "unsafe_fixable": 0},
        ]),
        "{stats_out}"
    );
    assert_eq!(stats["diagnostics"], serde_json::json!([]), "{stats_out}");
    assert_eq!(
        stats["summary"], plain["summary"],
        "the summary is unaffected by the flag:\nplain: {plain_out}\nstatistics: {stats_out}"
    );
}

#[test]
fn a_run_without_the_flag_has_no_statistics_field() {
    let root = project("absent-field");
    let (_, stdout, stderr) = htl(&["check", "src", "--no-cache", "--format", "json"], &root);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(
        !v.as_object().unwrap().contains_key("statistics"),
        "a plain run carries no statistics key at all: {stdout} {stderr}"
    );
}
