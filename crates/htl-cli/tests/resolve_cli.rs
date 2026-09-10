//! `htl resolve <module>` through the real binary: the chain a name resolves through,
//! what is read, what that hides, and where each file came from.
//!
//! The crate case runs on the same fixture `dep_dts.rs` uses — `dep` ships a declaration
//! in its manifest, `consumer` depends on it — so the `types/dep/` this reports on is one
//! `htl dts` materialised rather than one written here to look like it.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-resolve", name)
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

const DECL: &str = "local record mq\n   send: function(self: mq, s: string)\nend\nreturn mq\n";

/// The row for `path`, with its padding collapsed: the columns are as wide as the widest
/// file in the table, which is not what any of these tests is about.
fn row(out: &str, path: &str) -> String {
    out.lines()
        .find(|l| l.split_whitespace().nth(1) == Some(path))
        .unwrap_or_else(|| panic!("no row for {path} in:\n{out}"))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A project with one module and nothing to shadow it.
fn plain(name: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    write(
        &root.join("src/util.tl"),
        "local record util\nend\nfunction util.twice(n: integer): integer\n   return n * 2\nend\nreturn util\n",
    );
    root
}

#[test]
fn one_candidate_is_reported_as_the_file_that_is_read() {
    let root = plain("one");
    let (ok, out, err) = htl(&["resolve", "util"], &root);
    assert!(ok, "a name that resolves exits 0: {err}");
    assert!(out.contains("htl resolve util: src/util.tl"), "{out}");
    assert_eq!(row(&out, "src/util.tl"), "1 src/util.tl source read");
    // The directories are half the answer, so they are printed whatever was found.
    assert!(out.contains("searched, in order: ., src"), "{out}");
}

/// Three declarations of one name, and the order is the whole reason one of them is in
/// effect. The rows say which, rather than leaving it to be worked out from a lint.
#[test]
fn several_candidates_name_the_one_that_is_read_and_what_it_shadows() {
    let root = scratch("several");
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    write(&root.join("src/mq.d.tl"), DECL);
    write(&root.join("types/mq.d.tl"), DECL);
    let (ok, out, err) = htl(&["resolve", "mq"], &root);
    assert!(ok, "{err}");
    assert!(out.contains("htl resolve mq: src/mq.d.tl"), "{out}");
    assert_eq!(row(&out, "src/mq.d.tl"), "1 src/mq.d.tl declaration read");
    assert_eq!(
        row(&out, "types/mq.d.tl"),
        "2 types/mq.d.tl declaration shadowed by 1"
    );
}

/// Source beats declaration wherever the two sit, which is a rule about kinds and not
/// about directories: `types/` comes after `src/` on the path, and a `.tl` there still
/// wins. A report ordered by directory would name the wrong winner.
#[test]
fn a_source_further_along_the_path_is_still_the_one_read() {
    let root = scratch("source-wins");
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    write(&root.join("src/mq.d.tl"), DECL);
    write(
        &root.join("types/mq.tl"),
        "local record mq\n   send: function(self: mq, s: string)\nend\nreturn mq\n",
    );
    let (ok, out, err) = htl(&["resolve", "mq"], &root);
    assert!(ok, "{err}");
    assert!(out.contains("htl resolve mq: types/mq.tl"), "{out}");
    assert_eq!(row(&out, "types/mq.tl"), "1 types/mq.tl source read");
    assert_eq!(
        row(&out, "src/mq.d.tl"),
        "2 src/mq.d.tl declaration shadowed by 1"
    );
}

/// A `.lua` under a declaration is not shadowed by it: the check reads the `.d.tl` and
/// the run loads the `.lua`. Reporting that as hidden would be the wrong answer.
#[test]
fn the_lua_a_declaration_types_is_reported_as_what_the_run_loads() {
    let root = scratch("lua");
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    write(&root.join("types/mq.d.tl"), DECL);
    write(
        &root.join("src/mq.lua"),
        "local mq = {}\nfunction mq:send(s) end\nreturn mq\n",
    );
    let (ok, out, err) = htl(&["resolve", "mq"], &root);
    assert!(ok, "{err}");
    assert!(out.contains("htl resolve mq: types/mq.d.tl"), "{out}");
    assert_eq!(
        row(&out, "src/mq.lua"),
        "2 src/mq.lua lua runtime, typed by 1"
    );
}

/// An installed dependency is a kind of its own: the file is under `.htl/modules`, the
/// project did not write it, and the row says which dependency it belongs to.
#[test]
fn a_module_installed_under_htl_modules_names_the_dependency() {
    let root = scratch("dependency");
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nentry = \"src\"\n",
    );
    write(&root.join(".htl/modules/vendored/mq/init.d.tl"), DECL);
    let (ok, out, err) = htl(&["resolve", "mq"], &root);
    assert!(ok, "{err}");
    assert_eq!(
        row(&out, ".htl/modules/vendored/mq/init.d.tl"),
        "1 .htl/modules/vendored/mq/init.d.tl declaration read (installed from mq)"
    );
}

/// A script can ask, which means the exit code has to carry the answer.
#[test]
fn a_name_that_resolves_to_nothing_says_so_and_exits_non_zero() {
    let root = plain("missing");
    let (ok, out, err) = htl(&["resolve", "nope"], &root);
    assert!(
        !ok,
        "a name that resolves to nothing exits non-zero: {out}{err}"
    );
    assert!(
        out.contains("nothing on the search path answers require(\"nope\")"),
        "{out}"
    );
    // What was looked at is the useful half of a report with no rows in it.
    assert!(out.contains("searched, in order: ., src"), "{out}");
}

#[test]
fn the_json_carries_the_same_rows() {
    let root = scratch("json");
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    write(&root.join("src/mq.d.tl"), DECL);
    write(&root.join("types/mq.d.tl"), DECL);
    let (ok, out, err) = htl(&["resolve", "mq", "--format", "json"], &root);
    assert!(ok, "{err}");
    assert!(err.is_empty(), "the document is alone on stdout: {err}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["module"], "mq");
    assert_eq!(v["read"], "src/mq.d.tl");
    assert_eq!(v["summary"]["ok"], true);
    assert_eq!(v["summary"]["candidates"], 2);
    assert_eq!(v["summary"]["shadowed"], 1);
    let c = v["candidates"].as_array().unwrap();
    assert_eq!(c[0]["path"], "src/mq.d.tl");
    assert_eq!(c[0]["kind"], "declaration");
    assert_eq!(c[0]["status"], "read");
    assert_eq!(c[1]["path"], "types/mq.d.tl");
    assert_eq!(c[1]["status"], "shadowed");
    assert_eq!(c[1]["shadowed_by"], 1);
    assert!(
        v["searched"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d == "types"),
        "{out}"
    );

    let (ok, _, _) = htl(&["resolve", "nope", "--format", "json"], &root);
    assert!(!ok, "the exit code is the same in both forms");
}

// ---------------------------------------------------------------- a crate's declaration

/// Copy the `dep_dts` fixture into a scratch directory, `Cargo.toml.in` becoming the
/// `Cargo.toml` cargo reads. Returns the consumer, where the commands run.
fn fixture(name: &str) -> PathBuf {
    let root = scratch(name);
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dep_dts");
    copy_tree(&from, &root);
    root.join("consumer")
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let name = e.file_name().to_string_lossy().into_owned();
        let src = e.path();
        if src.is_dir() {
            copy_tree(&src, &to.join(name));
            continue;
        }
        let name = name.strip_suffix(".in").unwrap_or(&name).to_string();
        std::fs::copy(&src, to.join(name)).unwrap();
    }
}

/// Which crate a declaration came from is the fact a reader of `types/<crate>/` most
/// wants, and the file itself does not say. The note `htl dts` leaves beside it does.
#[test]
fn a_declaration_a_crate_ships_names_the_crate() {
    let root = fixture("crate");
    // `resolve` generates what a check would, so the copy under types/dep/ is there
    // without anyone having run `htl dts` first.
    let hand =
        "local record dep\n   greet: function(self: dep, name: string): string\nend\nreturn dep\n";
    write(&root.join("types/dep.d.tl"), hand);
    let (ok, out, err) = htl(&["resolve", "dep"], &root);
    assert!(ok, "{out}{err}");
    assert!(out.contains("htl resolve dep: types/dep.d.tl"), "{out}");
    assert_eq!(
        row(&out, "types/dep/dep.d.tl"),
        "2 types/dep/dep.d.tl declaration shadowed by 1 (shipped by dep 0.1.0)"
    );

    let (_, out, _) = htl(&["resolve", "dep", "--format", "json"], &root);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let shipped = v["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["path"] == "types/dep/dep.d.tl")
        .expect("the materialised declaration is a row");
    assert_eq!(shipped["origin"]["kind"], "crate");
    assert_eq!(shipped["origin"]["name"], "dep");
    assert_eq!(shipped["origin"]["version"], "0.1.0");
    // The directory it is attributed to is the one on the path in its own right, not the
    // `types/` above it that a `?/?.lua` template also reaches it through.
    assert_eq!(shipped["dir"], "types/dep");
}
