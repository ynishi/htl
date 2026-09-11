//! The lint selection through the binary: `htl check --list-lints` is the list of rules
//! there are with the level each has by default, and `--lint -<rule>` (the same spec
//! `HTL_LINTS` carries into `include_tl!`) silences one for a run. What a level does to a
//! run's verdict is `lint_levels.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-lint-flags", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn htl(args: &[&str], cwd: &Path) -> (String, String) {
    let out = Command::new(common::htl_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// One file, one cast the checker cannot stand behind.
fn project(name: &str) -> PathBuf {
    let dir = scratch(name);
    write(
        &dir.join("defs.tl"),
        "local record defs\n   enum State\n      \"open\"\n      \"closed\"\n   end\n\
         \n   record Row\n      state: string\n   end\nend\nreturn defs\n",
    );
    write(
        &dir.join("store.tl"),
        "local defs = require(\"defs\")\n\n\
         local function stored(h: defs.Row): defs.State\n   return h.state as defs.State\nend\n\n\
         return { stored = stored }\n",
    );
    dir
}

/// A line of the listing is the rule name, then the level it has by default.
fn listed(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .collect()
}

#[test]
fn list_lints_names_the_enum_boundary_rules() {
    let dir = scratch("list");
    let (stdout, _) = htl(&["check", "--list-lints"], &dir);
    let rules = listed(&stdout);
    assert!(rules.contains(&"enum-cast"), "{stdout}");
    assert!(rules.contains(&"enum-table"), "{stdout}");
}

#[test]
fn list_lints_names_the_sealed_record_rule() {
    let dir = scratch("list-sealed");
    let (stdout, _) = htl(&["check", "--list-lints"], &dir);
    assert!(listed(&stdout).contains(&"sealed-record"), "{stdout}");
}

#[test]
fn a_rule_can_be_turned_off_for_a_run() {
    let dir = project("off");
    let (_, on) = htl(&["check", ".", "--no-cache"], &dir);
    assert!(on.contains("enum-cast"), "{on}");
    let (out, off) = htl(&["check", ".", "--no-cache", "--lint", "-enum-cast"], &dir);
    // The spec has to have been read, not just absent from the output: clap used to take
    // `-enum-cast` for a cluster of short flags and answer `unexpected argument '-e'`,
    // which is a run that says nothing about `enum-cast` for the wrong reason.
    assert!(!off.contains("unexpected argument"), "{off}");
    assert!(
        out.contains("0 lint(s)") || off.contains("0 lint(s)"),
        "{off}"
    );
    assert!(!off.contains("enum-cast"), "{off}");
}
