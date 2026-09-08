//! A dependency's type errors through the real binary: `htl check` reports them as
//! errors with the dependency's path and the file that required it, once per run, from
//! the store on a replay, in `--format json` with `required_by` and `origin`; `htl fix`
//! reports them and leaves the dependency alone.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-deperr", name)
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

fn check_json(root: &Path, args: &[&str]) -> serde_json::Value {
    let mut all = vec!["check"];
    all.extend_from_slice(args);
    all.extend_from_slice(&["--format", "json"]);
    let (_, stdout, _) = htl(&all, root);
    serde_json::from_str(&stdout).expect("stdout is one JSON document")
}

/// One type error, at line 4.
const BROKEN_MATHX: &str = "local record mathx\nend\nfunction mathx.twice(n: number): number\n   local s: number = \"no\"\n   return n * 2 + s\nend\nreturn mathx\n";
const FIXED_MATHX: &str = "local record mathx\nend\nfunction mathx.twice(n: number): number\n   return n * 2\nend\nreturn mathx\n";

/// An mlua-pkg project with `mathx` installed under `.htl/modules` — broken at line 4 —
/// and two modules that require it.
fn project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), "[check]\n");
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n[deps]\n",
    );
    write(
        &root.join(".htl/modules/vendored/mathx/init.tl"),
        BROKEN_MATHX,
    );
    write(
        &root.join("src/area.tl"),
        "local mathx = require(\"mathx\")\nlocal record area\nend\n\
         function area.of(n: number): number\n   return mathx.twice(n)\nend\nreturn area\n",
    );
    write(
        &root.join("src/geometry.tl"),
        "local mathx = require(\"mathx\")\nlocal record geometry\nend\n\
         function geometry.twice(n: number): number\n   return mathx.twice(n)\nend\nreturn geometry\n",
    );
    root
}

#[test]
fn a_broken_installed_dependency_fails_the_check_once_naming_the_requirer() {
    let root = project("installed");
    let (ok, _, stderr) = htl(&["check", "src"], &root);
    assert!(!ok, "a dependency's type error fails the check: {stderr}");
    assert_eq!(
        stderr.matches("mathx/init.tl:4:").count(),
        1,
        "the dependency's own path and line, once for the run: {stderr}"
    );
    assert_eq!(
        stderr.matches("(required by src/").count(),
        1,
        "named against the first file that required it: {stderr}"
    );
    assert!(
        stderr.contains("1 error(s)"),
        "counted as an error in the totals: {stderr}"
    );
    assert!(
        !stderr.contains("fixable"),
        "a dependency is never offered to htl fix: {stderr}"
    );

    // `htl run` refuses the module at its first require, as it did before: the two agree.
    write(
        &root.join("src/main.tl"),
        "local geometry = require(\"geometry\")\nprint(geometry.twice(2))\n",
    );
    let (ran, _, run_err) = htl(&["run", "src/main.tl"], &root);
    assert!(!ran, "run must fail on the same module: {run_err}");
    assert!(run_err.contains("mathx/init.tl:4:"), "{run_err}");
}

#[test]
fn a_replay_carries_the_dependency_error_and_an_edit_to_the_dependency_changes_it() {
    let root = project("replay");
    let first = check_json(&root, &["src"]);
    let second = check_json(&root, &["src"]);
    assert!(!first["summary"]["cached"].as_bool().unwrap());
    assert!(
        second["summary"]["cached"].as_bool().unwrap(),
        "the second run must come from the store: {second}"
    );
    for v in [&first, &second] {
        assert_eq!(v["summary"]["errors"], 1, "{v}");
        assert_eq!(v["summary"]["ok"], false, "{v}");
        let deps: Vec<&serde_json::Value> = v["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d.get("required_by").is_some())
            .collect();
        assert_eq!(deps.len(), 1, "one dependency diagnostic: {v}");
        let d = deps[0];
        assert_eq!(d["severity"], "error");
        assert_eq!(d["origin"], "dependency", "{d}");
        assert_eq!(d["line"], 4);
        assert!(
            d["file"].as_str().unwrap().ends_with("mathx/init.tl"),
            "{d}"
        );
        assert!(
            d["required_by"].as_str().unwrap().starts_with("src/"),
            "{d}"
        );
        assert!(d.get("fix").is_none(), "no fix is offered: {d}");
    }
    assert_eq!(
        first["diagnostics"], second["diagnostics"],
        "a replay says what the run said"
    );

    // Move the error: the requirers' entries list the dependency by hash, so they miss.
    write(
        &root.join(".htl/modules/vendored/mathx/init.tl"),
        "local record mathx\nend\nfunction mathx.twice(n: number): number\n   return n * 2\nend\n\
         local bad: string = 1\nprint(bad)\nreturn mathx\n",
    );
    let third = check_json(&root, &["src"]);
    assert!(!third["summary"]["cached"].as_bool().unwrap(), "{third}");
    let d = third["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d.get("required_by").is_some())
        .expect("the new error is reported");
    assert_eq!(d["line"], 6, "the error moved with the edit: {d}");

    // Repaired: clean, and stays clean from the store.
    write(
        &root.join(".htl/modules/vendored/mathx/init.tl"),
        FIXED_MATHX,
    );
    let (ok, _, stderr) = htl(&["check", "src"], &root);
    assert!(ok, "{stderr}");
    let (ok, _, stderr) = htl(&["check", "src"], &root);
    assert!(ok && stderr.contains("[cached]"), "{stderr}");
}

#[test]
fn a_module_under_check_paths_is_external() {
    let root = scratch("external");
    write(&root.join("htl.toml"), "[check]\npaths = [\"mods\"]\n");
    write(&root.join("mods/ext.tl"), BROKEN_MATHX);
    write(
        &root.join("src/main.tl"),
        "local ext = require(\"ext\")\nprint(ext.twice(2))\n",
    );
    let v = check_json(&root, &["src"]);
    let d = v["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d.get("required_by").is_some())
        .expect("reported");
    assert_eq!(d["origin"], "external", "{d}");
    assert!(d["file"].as_str().unwrap().ends_with("mods/ext.tl"), "{d}");
    assert_eq!(v["summary"]["ok"], false);
}

/// A file of the project's own is a dependency only when the walk does not check it
/// itself: then it is reported with no origin. When the walk does, the file reports its
/// own error and nothing says it a second time on the requirer's behalf.
#[test]
fn a_project_file_outside_the_walk_is_reported_once_either_way() {
    let root = scratch("own");
    write(&root.join("htl.toml"), "[check]\n");
    write(&root.join("src/util.tl"), BROKEN_MATHX);
    write(
        &root.join("src/main.tl"),
        "local util = require(\"util\")\nprint(util.twice(2))\n",
    );
    let v = check_json(&root, &["src/main.tl"]);
    let d = v["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d.get("required_by").is_some())
        .expect("util is not in the walk, so main reports it");
    assert!(d.get("origin").is_none(), "the project's own: {d}");
    assert_eq!(v["summary"]["errors"], 1);

    let (ok, _, stderr) = htl(&["check", "src"], &root);
    assert!(!ok);
    assert_eq!(
        stderr.matches("util.tl:4:").count(),
        1,
        "util's error is its own, said once: {stderr}"
    );
    assert!(
        !stderr.contains("required by"),
        "not repeated on main's behalf: {stderr}"
    );
    assert!(stderr.contains("1 error(s)"), "{stderr}");
}

#[test]
fn htl_fix_reports_the_dependency_and_leaves_it_alone() {
    let root = project("fix");
    let dep = root.join(".htl/modules/vendored/mathx/init.tl");
    let before = std::fs::read_to_string(&dep).unwrap();
    let (ok, _, stderr) = htl(&["fix", "src", "--allow-no-vcs"], &root);
    assert!(!ok, "a dependency's error remains: {stderr}");
    assert_eq!(stderr.matches("mathx/init.tl:4:").count(), 1, "{stderr}");
    assert!(stderr.contains("(required by src/"), "{stderr}");
    assert!(stderr.contains("1 error(s) remaining"), "{stderr}");
    assert!(
        !stderr.contains("fixed:"),
        "nothing to fix in src: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(&dep).unwrap(),
        before,
        "htl fix never writes under .htl/"
    );
}

/// One report, one kind of path. A dependency's file comes from the resolver, which
/// searches in absolute paths, so without folding it a run would print the machine's
/// directory layout beside `src/area.tl`.
#[test]
fn a_dependencys_path_reads_against_the_directory_the_command_ran_in() {
    let root = project("relative");
    let (ok, _, stderr) = htl(&["check", "src"], &root);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains("error: .htl/modules/vendored/mathx/init.tl:4:"),
        "the dependency reads from the project, as the README writes it: {stderr}"
    );
    assert!(
        !stderr.contains("error: /"),
        "and no diagnostic carries an absolute path: {stderr}"
    );

    let v = check_json(&root, &["src"]);
    let d = v["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d.get("required_by").is_some())
        .expect("the dependency's error");
    assert_eq!(
        d["file"], ".htl/modules/vendored/mathx/init.tl",
        "json says the same as the text: {d}"
    );
    assert!(
        d["required_by"].as_str().unwrap().starts_with("src/"),
        "{d}"
    );

    // Started from the project root instead: the dependency reads the same way either
    // time, because it is written against the directory rather than against the argument.
    let (_, _, stderr) = htl(&["check", "."], &root);
    assert!(
        stderr.contains("error: .htl/modules/vendored/mathx/init.tl:4:"),
        "{stderr}"
    );
}

/// A `[check] paths` entry is joined onto the config's directory and keeps the `..` it was
/// written with. The report folds it rather than printing a path that walks back out of
/// the directory it starts from.
#[test]
fn an_external_dependencys_path_is_folded_rather_than_kept_with_dot_dot() {
    let root = scratch("external");
    let proj = root.join("proj");
    write(&proj.join("htl.toml"), "[check]\npaths = [\"../shared\"]\n");
    write(&root.join("shared/mathx.tl"), BROKEN_MATHX);
    write(
        &proj.join("src/area.tl"),
        "local mathx = require(\"mathx\")\nlocal record area\nend\n\
         function area.of(n: number): number\n   return mathx.twice(n)\nend\nreturn area\n",
    );
    let (ok, _, stderr) = htl(&["check", "src"], &proj);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains("shared/mathx.tl:4:"),
        "the external dependency is reported: {stderr}"
    );
    assert!(
        !stderr.contains(".."),
        "and not through the `..` it was resolved by: {stderr}"
    );
}
