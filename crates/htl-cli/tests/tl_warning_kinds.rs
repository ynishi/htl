//! Teal's warning kinds, as names.
//!
//! htl vendors the Teal compiler and forwards its warnings. Teal tags each one with a kind
//! — `unused`, `redeclaration`, `hint`, and four more — and htl used to drop the tag: every
//! Teal warning arrived as an anonymous `warning:` line whose `rule` was `null` in
//! `--format json`, so a project that wanted one kind quiet had nothing to write anywhere
//! and a JSON consumer had to pattern-match message text to tell two kinds apart.
//!
//! The kind is now carried through as the ` [htl tl:<kind>]` suffix htl's own lints already
//! use, and the seven are registry entries (`htl_core::lint::RULES`), so they are addressed
//! exactly as every other rule is. That is what the tests below hold: the name is printed,
//! the name is in the JSON, and the name comes back — from `--lint`, from `[lint.rules]`
//! and from an allow comment at the site.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-tl-warnings", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A module with one `tl:unused` (a local nothing reads, line 6) and one
/// `tl:redeclaration` (a parameter over the module required on line 1, line 5), and
/// neither an error nor an htl lint of its own beyond the `shadow-local` that sits on the
/// same line as the redeclaration — which is the subject of its own test below.
///
/// The shadowed name is a required module because that is the case where the two still
/// coincide: `shadow-local` was reduced to it (#167), an ordinary local shadowed in an
/// inner scope being `tl:redeclaration` alone now. What is wanted here is a line that both
/// halves of htl report on, so that one printed shape can be seen carrying both.
const SRC: &str = "local dep = require(\"dep\")\nlocal record m\nend\n\n\
                   function m.go(s: string, dep: string): integer\n   local unread_one = 1\n   \
                   print(string.rep(s, 2), dep)\n   return 1\nend\n\nprint(dep.note())\n\
                   return m\n";

fn project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), "[check]\npaths = [\"src\"]\n");
    write(
        &root.join("src/dep.tl"),
        "local record dep\nend\nfunction dep.note(): string\n   return \"n\"\nend\nreturn dep\n",
    );
    write(&root.join("src/a.tl"), SRC);
    root
}

fn run(root: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_htl"))
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    (
        out.status.code().expect("the run exited, not signalled"),
        String::from_utf8_lossy(&out.stderr).into_owned() + &String::from_utf8_lossy(&out.stdout),
    )
}

/// The `(severity, rule, file, line)` of every diagnostic `htl check src --format json`
/// reports. `rule` is the empty string when the field is absent or null, which is what it
/// was for every Teal warning before this change.
fn diagnostics(root: &Path, args: &[&str]) -> Vec<(String, String, String, u64)> {
    let out = Command::new(env!("CARGO_BIN_EXE_htl"))
        .args(["check", "src", "--format", "json", "--no-cache"])
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let v: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("stdout is one JSON document ({e}): {text}"));
    v["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .map(|d| {
            (
                d["severity"].as_str().unwrap_or_default().to_string(),
                d["rule"].as_str().unwrap_or_default().to_string(),
                d["file"].as_str().unwrap_or_default().to_string(),
                d["line"].as_u64().unwrap_or(0),
            )
        })
        .collect()
}

/// The diagnostics reported under one rule.
fn under(root: &Path, rule: &str, args: &[&str]) -> Vec<(String, String, String, u64)> {
    diagnostics(root, args)
        .into_iter()
        .filter(|(_, r, _, _)| r == rule)
        .collect()
}

// ------------------------------------------------------------------ what is printed

#[test]
fn the_kind_is_printed_in_the_shape_the_lints_use() {
    let root = project("printed");
    let (code, said) = run(&root, &["check", "src", "--no-cache"]);
    assert_eq!(code, 0, "a warning does not fail a plain check: {said}");
    assert!(
        said.contains("unused variable unread_one: integer [htl tl:unused]"),
        "{said}"
    );
    assert!(
        said.contains("variable shadows previous declaration of 'dep'")
            && said.contains("[htl tl:redeclaration]"),
        "{said}"
    );
    // The same suffix an htl lint writes, on the same run, so one shape carries both.
    assert!(said.contains("[htl shadow-local]"), "{said}");
}

#[test]
fn json_reports_the_kind_as_the_rule() {
    let root = project("json");
    let found = diagnostics(&root, &[]);
    let warnings: Vec<_> = found.iter().filter(|(s, ..)| s == "warning").collect();
    assert_eq!(warnings.len(), 2, "{found:?}");
    for (_, rule, _, _) in &warnings {
        assert!(
            !rule.is_empty(),
            "a Teal warning with no rule in the JSON: {found:?}"
        );
        assert!(rule.starts_with("tl:"), "{rule} is not in the tl namespace");
    }
    let mut rules: Vec<&str> = warnings.iter().map(|(_, r, ..)| r.as_str()).collect();
    rules.sort_unstable();
    assert_eq!(rules, ["tl:redeclaration", "tl:unused"], "{found:?}");
}

// ------------------------------------------------------------------ turning one off

#[test]
fn a_kind_goes_off_from_the_flag() {
    let root = project("flag");
    assert_eq!(under(&root, "tl:unused", &[]).len(), 1);

    let off = under(&root, "tl:unused", &["--lint", "-tl:unused"]);
    assert!(off.is_empty(), "--lint -tl:unused left it: {off:?}");
    // The other kind is still reported, so the flag turned off one rule and not the
    // whole of Teal's vocabulary.
    let other = under(&root, "tl:redeclaration", &["--lint", "-tl:unused"]);
    assert_eq!(
        other.len(),
        1,
        "-tl:unused silenced another kind: {other:?}"
    );
}

#[test]
fn a_kind_goes_off_from_the_config() {
    let root = project("config");
    write(
        &root.join("htl.toml"),
        "[check]\npaths = [\"src\"]\n\n[lint.rules]\n\"tl:redeclaration\" = \"allow\"\n",
    );
    let off = under(&root, "tl:redeclaration", &[]);
    assert!(off.is_empty(), "[lint.rules] allow left it: {off:?}");
    assert_eq!(under(&root, "tl:unused", &[]).len(), 1);
}

/// A kind htl turned off is not reported, and so is not counted either: `[lint] strict`
/// judges a run on what it said. A warning that failed a strict run after being silenced
/// would be the worst of both.
#[test]
fn a_kind_turned_off_does_not_fail_a_strict_run() {
    let root = project("strict");
    let (code, said) = run(&root, &["check", "src", "--no-cache", "--strict"]);
    assert_eq!(code, 1, "warnings fail a strict check: {said}");

    let (code, said) = run(
        &root,
        &[
            "check",
            "src",
            "--no-cache",
            "--strict",
            "--lint",
            "-tl:unused,-tl:redeclaration,-shadow-local",
        ],
    );
    assert_eq!(code, 0, "silenced findings still failed the run: {said}");
    assert!(said.contains("0 warning(s)"), "{said}");
}

/// The name written at the site the finding points at, which is how htl's own lints are
/// silenced. `tl:redeclaration` and `shadow-local` land on the same line and column, so
/// the line below is also the case that decided site suppression should reach these at
/// all: a comment there answers the finding the reader is looking at, whichever half of
/// htl raised it.
#[test]
fn a_kind_goes_off_at_the_site() {
    let root = project("site");
    let at = root.join("src/a.tl");
    let src = std::fs::read_to_string(&at).unwrap();
    write(
        &at,
        &src.replace(
            "function m.go(s: string, dep: string): integer",
            "function m.go(s: string, dep: string): integer  -- htl: allow(tl:redeclaration)",
        )
        .replace(
            "   local unread_one = 1",
            "   local unread_one = 1  -- htl: allow(tl:unused)",
        ),
    );
    let found = diagnostics(&root, &[]);
    assert!(
        found.iter().all(|(s, ..)| s != "warning"),
        "an allow comment left a warning: {found:?}"
    );
    // And it silenced only what it named: the lint on the same line is still reported,
    // because that line allowed `tl:redeclaration` and nothing else.
    assert_eq!(under(&root, "shadow-local", &[]).len(), 1, "{found:?}");
}

// ------------------------------------------------------------------ the round trip

/// Take the name out of a diagnostic and type it back — the property the registry exists
/// for, run over a name that comes from the vendored compiler rather than from htl.
#[test]
fn the_name_out_of_a_diagnostic_is_a_name_htl_takes_back() {
    let root = project("round-trip");
    let warning = diagnostics(&root, &[])
        .into_iter()
        .find(|(s, ..)| s == "warning")
        .expect("one Teal warning");
    let (_, rule, file, line) = warning;

    // 1. The command line.
    let off = under(&root, &rule, &["--lint", &format!("-{rule}")]);
    assert!(off.is_empty(), "--lint -{rule} left it: {off:?}");

    // 2. `[lint.rules]`, at `allow`.
    let cfg = root.join("htl.toml");
    let before = std::fs::read_to_string(&cfg).unwrap();
    write(
        &cfg,
        &format!("{before}\n[lint.rules]\n{rule:?} = \"allow\"\n"),
    );
    let off = under(&root, &rule, &[]);
    assert!(off.is_empty(), "[lint.rules] allow left {rule}: {off:?}");
    write(&cfg, &before);

    // 3. An allow comment on the line the diagnostic points at.
    let at = root.join(&file);
    let src = std::fs::read_to_string(&at).unwrap();
    let mut lines: Vec<String> = src.lines().map(str::to_string).collect();
    lines[line as usize - 1] = format!("{}  -- htl: allow({rule})", lines[line as usize - 1]);
    write(&at, &(lines.join("\n") + "\n"));
    let off = under(&root, &rule, &[]);
    assert!(
        off.is_empty(),
        "-- htl: allow({rule}) on {file}:{line} left it: {off:?}"
    );
}

/// A name in the namespace that is not one of the seven is refused as written, like any
/// other unknown rule — rather than turning nothing off and reading like a kind that
/// found nothing.
#[test]
fn a_name_that_is_not_a_kind_is_refused() {
    let root = project("unknown");
    let (code, said) = run(&root, &["check", "src", "--no-cache", "--lint", "-tl:hnt"]);
    assert_eq!(code, 2, "{said}");
    assert!(said.contains("unknown lint rule"), "{said}");
    assert!(said.contains("tl:hnt"), "{said}");
}

/// The prefix is what keeps the two vocabularies apart: `unused` is a word htl already
/// uses for something else (`htl unused` reports modules nothing requires), so the bare
/// kind is not a rule name here.
#[test]
fn the_bare_kind_is_not_a_rule_name() {
    let root = project("bare");
    let (code, said) = run(&root, &["check", "src", "--no-cache", "--lint", "-hint"]);
    assert_eq!(code, 2, "{said}");
    assert!(said.contains("unknown lint rule"), "{said}");
}
