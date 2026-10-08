//! `htl adopt` through the real binary: the table with no marker anywhere, the table with
//! some, `--detail`, the replay, `--help`, the one file kind the census excludes, and the
//! `---@struct` row's `applicable` (#304).

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-adopt", name)
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

/// A project with no marker anywhere.
fn unmarked_project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    write(&root.join("src/main.tl"), "print(1)\n");
    root
}

/// Two records carrying `---@struct` in two different files, and a function carrying
/// `---@nilable` beside the first record: `Foo` at `src/a.tl:3`, `find` at `src/a.tl:8`,
/// `Bar` at `src/b.tl:10` — the exact positions the tests below assert the plain and
/// `--detail` stderr against. The lines above each declaration are comments only so each
/// one lands at the line its name is read against below.
fn two_markers_project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    write(
        &root.join("src/a.tl"),
        "-- a\n-- a\nlocal record Foo   ---@struct\n   x: string\nend\n\n\
         ---@nilable\nlocal function find(): string\n   return \"x\"\nend\n\nreturn {}\n",
    );
    write(
        &root.join("src/b.tl"),
        "-- b\n-- b\n-- b\n-- b\n-- b\n-- b\n-- b\n-- b\n-- b\n\
         local record Bar   ---@struct\n   y: string\nend\n\nreturn {}\n",
    );
    root
}

/// As [`two_markers_project`], with an unmarked `Point` (one field, `v`) added right
/// after `find`: declared at `src/a.tl:11`, built whole by `p1` at `src/a.tl:14`, and
/// built from `second_site` by `p2` at `src/a.tl:15` — a candidate for `---@struct`
/// (#304) when `second_site` sets `v` too, and not one when it does not (`"{}"`,
/// one site short). `Foo` (`src/a.tl:3`), `find` (`src/a.tl:8`) and `Bar`
/// (`src/b.tl:10`) keep the positions [`two_markers_project`]'s own tests assert against.
fn candidate_project(name: &str, second_site: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    write(
        &root.join("src/a.tl"),
        &format!(
            "-- a\n-- a\nlocal record Foo   ---@struct\n   x: string\nend\n\n\
             ---@nilable\nlocal function find(): string\n   return \"x\"\nend\n\
             local record Point\n   v: string\nend\n\
             local p1: Point = {{ v = \"a\" }}\nlocal p2: Point = {second_site}\n\
             print(p1, p2)\n\nreturn {{}}\n"
        ),
    );
    write(
        &root.join("src/b.tl"),
        "-- b\n-- b\n-- b\n-- b\n-- b\n-- b\n-- b\n-- b\n-- b\n\
         local record Bar   ---@struct\n   y: string\nend\n\nreturn {}\n",
    );
    root
}

/// Two records in two different files, each a candidate for `---@struct`: `Alpha`
/// (`src/a.tl:3`, built once, in `a.tl`) and `Beta` (`src/b.tl:2`, built twice -- once
/// in `b.tl` itself, once from `a.tl`, which requires `b.tl`). `b.tl`'s own require is
/// `b`'s own file reading itself; `a.tl`'s is a requirer's resolution of the same file,
/// which need not spell it identically -- and reaches `checked` here through the cache
/// `adopt` reads candidates back through (#304), not a fresh `CheckInfo`.
fn two_candidates_project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    write(
        &root.join("src/a.tl"),
        "local b = require(\"b\")\n\n\
         local record Alpha\n   x: string\nend\n\n\
         local a1: Alpha = { x = \"a\" }\n\
         local b1: b.Beta = { y = \"b\" }\n\
         print(a1, b1)\n\nreturn { Alpha = Alpha }\n",
    );
    write(
        &root.join("src/b.tl"),
        "local record b\n   record Beta\n      y: string\n   end\nend\n\n\
         local b2: b.Beta = { y = \"c\" }\nprint(b2)\n\nreturn b\n",
    );
    root
}

#[test]
fn candidates_across_two_files_are_ordered_by_their_own_declaring_file() {
    let root = two_candidates_project("ordered");
    let (ok, stdout, stderr) = htl(&["adopt", "--format", "json"], &root);
    assert!(ok, "{stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is one JSON document");
    assert_eq!(
        v["features"][0]["candidates"],
        serde_json::json!([
            { "file": "src/a.tl", "line": 3, "name": "Alpha", "sites": 1 },
            { "file": "src/b.tl", "line": 2, "name": "Beta", "sites": 2 },
        ]),
        "{v}"
    );
    assert_eq!(v["features"][0]["applicable"], 2, "{v}");
}

/// `-- htl: allow(unmarked-struct)` on `Point`'s own declaration line leaves it out of
/// both `applicable` and `candidates`, the same way the comment silences the lint's own
/// finding (#304) -- `unmarked_structs` reads no source of its own, so `adopt`
/// has to apply the check itself, through `lint::line_is_allowed`.
#[test]
fn an_allow_comment_on_the_declaration_is_not_a_candidate() {
    let root = scratch("allowed");
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    write(
        &root.join("src/a.tl"),
        "local record Point   -- htl: allow(unmarked-struct)\n   v: string\nend\n\n\
         local p1: Point = { v = \"a\" }\nlocal p2: Point = { v = \"b\" }\n\
         print(p1, p2)\n\nreturn {}\n",
    );
    let (ok, stdout, stderr) = htl(&["adopt", "--format", "json"], &root);
    assert!(ok, "{stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is one JSON document");
    assert_eq!(v["features"][0]["applicable"], 0, "{v}");
    assert_eq!(v["features"][0]["candidates"], serde_json::json!([]), "{v}");
    assert_eq!(v["summary"]["applicable"], 0);
}

/// As above, with `unmarked-struct` named among several in one list-form comment
/// (`-- htl: allow(no-any, unmarked-struct)`) rather than alone -- `collect_allows`
/// splits on commas, so the second name has to be read as its own, not as part of the
/// first or swallowed by the parentheses matching only the whole list as one word.
#[test]
fn an_allow_comment_naming_several_rules_is_not_a_candidate_either() {
    let root = scratch("allowed-list");
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    write(
        &root.join("src/a.tl"),
        "local record Point   -- htl: allow(no-any, unmarked-struct)\n   v: string\nend\n\n\
         local p1: Point = { v = \"a\" }\nlocal p2: Point = { v = \"b\" }\n\
         print(p1, p2)\n\nreturn {}\n",
    );
    let (ok, stdout, stderr) = htl(&["adopt", "--format", "json"], &root);
    assert!(ok, "{stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is one JSON document");
    assert_eq!(v["features"][0]["applicable"], 0, "{v}");
    assert_eq!(v["features"][0]["candidates"], serde_json::json!([]), "{v}");
    assert_eq!(v["summary"]["applicable"], 0);
}

#[test]
fn a_project_with_no_marker_reports_every_feature_unused() {
    let root = unmarked_project("none");
    let (ok, out, err) = htl(&["adopt"], &root);
    assert!(ok, "a report exits 0: {err}");
    assert!(out.is_empty(), "text mode is silent on stdout: {out}");
    for marker in [
        "struct",
        "optional",
        "sealed",
        "extensible",
        "nilable",
        "contract",
        "required",
        "async",
        "noyield",
    ] {
        let row = format!("---@{marker}");
        assert!(err.contains(&row), "missing row {row}: {err}");
    }
    assert!(
        err.contains("htl adopt: 0 of 9 features used, no markers"),
        "{err}"
    );
}

/// The whole stderr, not a substring: both rows and the summary line are deterministic
/// for this fixture, so there is nothing to pick out piecemeal. `---@struct`'s
/// `applicable` is `0`, not `-`: `unmarked-struct` has run over this fixture and found no
/// candidate (`Foo` and `Bar` are both marked already), which is a count, not an absence
/// of one -- unlike every other row here, which still has no lint behind it at all.
const TWO_MARKERS_PLAIN_STDERR: &str = "\
feature          used  applicable
---@struct          2           0
---@optional        0           -
---@sealed          0           -
---@extensible      0           -
---@nilable         1           -
---@contract        0           -
---@required        0           -
---@async           0           -
---@noyield         0           -
htl adopt: 2 of 9 features used, 3 markers in 2 files
";

/// As [`TWO_MARKERS_PLAIN_STDERR`], with `--detail`'s three lines (feature order: `Foo` and
/// `Bar` under `---@struct`, file then line; `find` under `---@nilable`) inserted between
/// the table and the summary. No candidate line: this fixture has none.
const TWO_MARKERS_DETAIL_STDERR: &str = "\
feature          used  applicable
---@struct          2           0
---@optional        0           -
---@sealed          0           -
---@extensible      0           -
---@nilable         1           -
---@contract        0           -
---@required        0           -
---@async           0           -
---@noyield         0           -
  ---@struct      src/a.tl:3   Foo
  ---@struct      src/b.tl:10  Bar
  ---@nilable     src/a.tl:8   find
htl adopt: 2 of 9 features used, 3 markers in 2 files
";

#[test]
fn a_project_with_two_structs_and_a_nilable_function_counts_each_marker() {
    let root = two_markers_project("two");
    let (ok, _, err) = htl(&["adopt"], &root);
    assert!(ok, "{err}");
    assert_eq!(err, TWO_MARKERS_PLAIN_STDERR);

    let (ok, stdout, stderr) = htl(&["adopt", "--format", "json"], &root);
    assert!(ok, "{stderr}");
    assert!(
        stderr.trim().is_empty(),
        "json mode keeps stderr silent: {stderr}"
    );
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is one JSON document");
    let features = v["features"].as_array().unwrap();
    assert_eq!(features[0]["marker"], "struct");
    assert_eq!(features[0]["used"], 2);
    assert_eq!(features[0]["applicable"], 0, "{v}");
    assert_eq!(features[0]["candidates"], serde_json::json!([]), "{v}");
    assert_eq!(v["summary"]["used"], 2);
    assert_eq!(v["summary"]["markers"], 3);
    assert_eq!(v["summary"]["applicable"], 0);
}

#[test]
fn detail_lists_every_marked_declaration_and_the_plain_form_does_not() {
    let root = two_markers_project("detail");
    let (ok, _, err) = htl(&["adopt", "--detail"], &root);
    assert!(ok, "{err}");
    assert_eq!(err, TWO_MARKERS_DETAIL_STDERR);

    let (ok, _, plain) = htl(&["adopt"], &root);
    assert!(ok, "{plain}");
    assert_eq!(plain, TWO_MARKERS_PLAIN_STDERR);
}

/// `htl adopt <path>` walks the whole project either way (the check behind the census has
/// to resolve and replay as it does for the project as a whole), but the counts are of the
/// files under the path given: `---@sealed` in `src/x/a.tl` and `---@struct` in
/// `src/y/b.tl`, asked about `src/x` alone, report the first and not the second.
#[test]
fn paths_narrow_the_counts_to_the_files_under_them() {
    let root = scratch("narrow");
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    write(
        &root.join("src/x/a.tl"),
        "local record S   ---@sealed\nend\nreturn S\n",
    );
    write(
        &root.join("src/y/b.tl"),
        "local record T   ---@struct\n   v: string\nend\nreturn T\n",
    );
    let (ok, _, err) = htl(&["adopt", "src/x"], &root);
    assert!(ok, "{err}");
    let sealed_row = err
        .lines()
        .find(|l| l.trim_start().starts_with("---@sealed"))
        .unwrap_or_else(|| panic!("no ---@sealed row: {err}"));
    assert!(
        sealed_row.split_whitespace().nth(1) == Some("1"),
        "sealed row: {sealed_row}"
    );
    let struct_row = err
        .lines()
        .find(|l| l.trim_start().starts_with("---@struct"))
        .unwrap_or_else(|| panic!("no ---@struct row: {err}"));
    assert!(
        struct_row.split_whitespace().nth(1) == Some("0"),
        "struct row: {struct_row}"
    );
    assert!(
        err.contains("htl adopt: 1 of 9 features used, 1 marker in 1 file"),
        "{err}"
    );
}

/// `htl adopt` with no path at all, run from a subdirectory: the paths default to `"."`,
/// the working directory the process was spawned in, not the project root — so a run from
/// `src/x` reports only `src/x`'s marker (`---@sealed` on `S`) and not `src/y`'s
/// (`---@struct` on `T`), the same narrowing the explicit-path test above gets by naming
/// `src/x` on the command line.
#[test]
fn run_from_a_subdirectory_counts_that_subdirectory() {
    let root = scratch("subdir");
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    write(
        &root.join("src/x/a.tl"),
        "local record S   ---@sealed\nend\nreturn S\n",
    );
    write(
        &root.join("src/y/b.tl"),
        "local record T   ---@struct\n   v: string\nend\nreturn T\n",
    );
    let (ok, _, err) = htl(&["adopt"], &root.join("src/x"));
    assert!(ok, "{err}");
    let sealed_row = err
        .lines()
        .find(|l| l.trim_start().starts_with("---@sealed"))
        .unwrap_or_else(|| panic!("no ---@sealed row: {err}"));
    assert!(
        sealed_row.split_whitespace().nth(1) == Some("1"),
        "sealed row: {sealed_row}"
    );
    let struct_row = err
        .lines()
        .find(|l| l.trim_start().starts_with("---@struct"))
        .unwrap_or_else(|| panic!("no ---@struct row: {err}"));
    assert!(
        struct_row.split_whitespace().nth(1) == Some("0"),
        "struct row: {struct_row}"
    );
    assert!(
        err.contains("htl adopt: 1 of 9 features used, 1 marker in 1 file"),
        "{err}"
    );
}

/// A type error (`local v: integer = "s"`) does not empty the census: the file still
/// declares `---@struct` on `R`, and `R` still has a full census entry — only a *syntax*
/// error leaves a file uncensused ([`Summary::check_errors`]'s own doc), and this is not
/// one. The error count still surfaces, as the exact line `print_adopt` prints for it.
#[test]
fn a_type_error_does_not_empty_the_census_and_the_error_line_is_printed() {
    let root = scratch("type-error");
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    write(
        &root.join("src/main.tl"),
        "local v: integer = \"s\"\nlocal record R   ---@struct\n   x: string\nend\nreturn R\n",
    );
    let (ok, _, err) = htl(&["adopt"], &root);
    assert!(
        ok,
        "a report exits 0 even with a type error in the project's check: {err}"
    );
    let struct_row = err
        .lines()
        .find(|l| l.trim_start().starts_with("---@struct"))
        .unwrap_or_else(|| panic!("no ---@struct row: {err}"));
    assert!(
        struct_row.split_whitespace().nth(1) == Some("1"),
        "a type error does not empty the census: {struct_row}"
    );
    assert!(
        err.contains(
            "htl adopt: 1 error(s) in the project's check (htl check says which); a file \
             with a syntax error contributes no markers"
        ),
        "{err}"
    );
}

#[test]
fn a_replayed_project_prints_the_same_table_and_the_cache_says_so() {
    let root = two_markers_project("replay");
    let (ok1, _, err1) = htl(&["adopt"], &root);
    assert!(ok1, "{err1}");

    let (ok2, _, err2) = htl(&["adopt", "--explain-cache"], &root);
    assert!(ok2, "{err2}");
    assert!(
        err2.contains("htl cache: 2 hit, 0 missed,"),
        "the second run replays both files: {err2}"
    );
    // The cache line is the one addition; the table and summary beneath it are the
    // first run's.
    let rest: String = err2
        .lines()
        .filter(|l| !l.starts_with("htl cache:"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(err1.trim_end(), rest.trim_end());
}

#[test]
fn help_says_which_row_has_applicable() {
    let out = Command::new(common::htl_bin())
        .args(["adopt", "--help"])
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(help.contains("applicable"), "{help}");
    assert!(help.contains("unmarked-struct"), "{help}");
}

#[test]
fn a_dtl_declaration_carrying_a_marker_does_not_move_its_row() {
    let root = unmarked_project("dtl");
    write(
        &root.join("types/foo.d.tl"),
        "---@async\nlocal function f(): string\n   return \"x\"\nend\nreturn f\n",
    );
    let (ok, _, err) = htl(&["adopt"], &root);
    assert!(ok, "{err}");
    assert!(
        err.contains("htl adopt: 0 of 9 features used, no markers"),
        "a declaration's own markers are not the project's authors' writing: {err}"
    );
}

/// As [`TWO_MARKERS_PLAIN_STDERR`], but `Point` (unmarked, built whole at two sites) makes
/// the `---@struct` row's `applicable` `1` instead of `-`; every other row is still `-`,
/// and the summary line is unchanged (a candidate carries no marker, so it adds nothing
/// to `used` or `markers`).
const CANDIDATE_PLAIN_STDERR: &str = "\
feature          used  applicable
---@struct          2           1
---@optional        0           -
---@sealed          0           -
---@extensible      0           -
---@nilable         1           -
---@contract        0           -
---@required        0           -
---@async           0           -
---@noyield         0           -
htl adopt: 2 of 9 features used, 3 markers in 2 files
";

/// As [`CANDIDATE_PLAIN_STDERR`], with `--detail`'s four lines inserted: `Foo` and `Bar`
/// (used sites) before `Point` (the candidate, with its site count) under `---@struct`,
/// then `find` under `---@nilable`.
const CANDIDATE_DETAIL_STDERR: &str = "\
feature          used  applicable
---@struct          2           1
---@optional        0           -
---@sealed          0           -
---@extensible      0           -
---@nilable         1           -
---@contract        0           -
---@required        0           -
---@async           0           -
---@noyield         0           -
  ---@struct      src/a.tl:3   Foo
  ---@struct      src/b.tl:10  Bar
  ---@struct      src/a.tl:11  Point  (applicable: built whole at 2 sites)
  ---@nilable     src/a.tl:8   find
htl adopt: 2 of 9 features used, 3 markers in 2 files
";

#[test]
fn the_struct_row_counts_the_records_unmarked_struct_would_report() {
    let root = candidate_project("row", "{ v = \"b\" }");
    let (ok, _, err) = htl(&["adopt"], &root);
    assert!(ok, "{err}");
    assert_eq!(err, CANDIDATE_PLAIN_STDERR);

    let (ok, stdout, stderr) = htl(&["adopt", "--format", "json"], &root);
    assert!(ok, "{stderr}");
    assert!(
        stderr.trim().is_empty(),
        "json mode keeps stderr silent: {stderr}"
    );
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is one JSON document");
    let features = v["features"].as_array().unwrap();
    assert_eq!(features[0]["marker"], "struct");
    assert_eq!(features[0]["applicable"], 1);
    assert_eq!(
        features[0]["candidates"],
        serde_json::json!([{ "file": "src/a.tl", "line": 11, "name": "Point", "sites": 2 }]),
        "{v}"
    );
    for f in &features[1..] {
        assert!(f["applicable"].is_null(), "{f}");
        assert_eq!(f["candidates"], serde_json::json!([]), "{f}");
    }
    assert_eq!(v["summary"]["applicable"], 1);
}

#[test]
fn detail_lists_candidates_after_the_marked_declarations() {
    let root = candidate_project("detail-candidate", "{ v = \"b\" }");
    let (ok, _, err) = htl(&["adopt", "--detail"], &root);
    assert!(ok, "{err}");
    assert_eq!(err, CANDIDATE_DETAIL_STDERR);
}

#[test]
fn a_record_built_short_at_one_site_is_not_a_candidate() {
    let root = candidate_project("short", "{}");
    let (ok, _, err) = htl(&["adopt"], &root);
    assert!(ok, "{err}");
    let struct_row = err
        .lines()
        .find(|l| l.trim_start().starts_with("---@struct"))
        .unwrap_or_else(|| panic!("no ---@struct row: {err}"));
    assert_eq!(
        struct_row, "---@struct          2           0",
        "{struct_row}"
    );

    let (ok, _, detail_err) = htl(&["adopt", "--detail"], &root);
    assert!(ok, "{detail_err}");
    assert!(
        !detail_err.contains("Point"),
        "a record built short at one site is not a candidate: {detail_err}"
    );

    let (ok, stdout, stderr) = htl(&["adopt", "--format", "json"], &root);
    assert!(ok, "{stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is one JSON document");
    assert_eq!(v["features"][0]["applicable"], 0);
    assert_eq!(v["features"][0]["candidates"], serde_json::json!([]), "{v}");
    assert_eq!(v["summary"]["applicable"], 0);
}
