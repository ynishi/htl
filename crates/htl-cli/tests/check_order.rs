//! A file's diagnostics must not depend on what was checked before it.
//!
//! `htl check` adds each file's directory to the search path as it walks. Without putting
//! the path back afterwards, the Nth file resolves `require` against the directories of
//! the first N-1 too — so the same file reports differently depending on where it fell in
//! the walk, and the direction is the dangerous one: an error that should be reported
//! disappears because something checked earlier happened to provide the module.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-order", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn diagnostics(args: &[&str], cwd: &Path) -> Vec<String> {
    let out = Command::new(common::htl_bin())
        .arg("check")
        .args(args)
        .arg("--format")
        .arg("json")
        .arg("--no-cache")
        .current_dir(cwd)
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("stdout is one JSON document");
    v["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .map(|d| {
            // The checker echoes the path as it was given, so `htl check .` says
            // `./b/main.tl` where `htl check b/main.tl` says `b/main.tl`. That spelling is
            // not what these tests are about.
            let file = d["file"].as_str().unwrap_or("").trim_start_matches("./");
            format!(
                "{file}:{}: {}",
                d["line"],
                d["message"].as_str().unwrap_or("")
            )
        })
        .collect()
}

/// Two directories where one provides a module the other requires but does not have. The
/// only reason `b` could ever resolve `util` is `a` being on the path.
fn two_dirs() -> PathBuf {
    let root = scratch("two");
    write(
        &root.join("a/util.tl"),
        "local record util\nend\nfunction util.f(): integer\n   return 1\nend\nreturn util\n",
    );
    write(
        &root.join("b/main.tl"),
        "local util = require(\"util\")\nlocal x: integer = util.f()\nprint(x)\n",
    );
    root
}

#[test]
fn a_file_reports_the_same_wherever_it_falls_in_the_walk() {
    let root = two_dirs();
    let alone = diagnostics(&["b/main.tl"], &root);
    let after = diagnostics(&["a/util.tl", "b/main.tl"], &root);
    let before = diagnostics(&["b/main.tl", "a/util.tl"], &root);

    assert!(
        alone.iter().any(|d| d.contains("module not found")),
        "b/ has no util.tl, so checking it alone must say so: {alone:?}"
    );
    assert_eq!(
        alone, after,
        "checking a/ first must not make b/'s missing module resolve"
    );
    assert_eq!(alone, before, "nor must checking it second");
}

#[test]
fn walking_a_tree_agrees_with_checking_its_files_one_at_a_time() {
    let root = two_dirs();
    let whole = diagnostics(&["."], &root);
    let mut apart = diagnostics(&["a/util.tl"], &root);
    apart.extend(diagnostics(&["b/main.tl"], &root));
    assert_eq!(
        whole, apart,
        "a walk is the files it visits, in the state each would be checked in"
    );
}

/// A file under the project's test root reads the project's sources, so that `htl check
/// tests` sees what `htl test` sees. Restoring the path per file must not take that away.
///
/// The `htl.toml` is what makes the directory a project: without one there is no source
/// root to read, and a file resolves its `require`s beside it and nowhere else.
#[test]
fn a_test_file_still_sees_the_project_root_and_src() {
    let root = scratch("tests-layout");
    write(&root.join("htl.toml"), "");
    write(
        &root.join("src/util.tl"),
        "local record util\nend\nfunction util.f(): integer\n   return 1\nend\nreturn util\n",
    );
    write(
        &root.join("tests/util_test.tl"),
        "local util = require(\"util\")\nlocal x: integer = util.f()\nprint(x)\n",
    );
    let d = diagnostics(&["."], &root);
    assert!(
        d.is_empty(),
        "a file under tests/ resolves modules from src/: {d:?}"
    );
}

/// A module that declares a `global` is the second channel of order dependence, and an
/// independent one: the search path is the same for every file here, and the answer still
/// moved. The checked-module store served the declaration's checked result to every
/// environment after the first, and replaying a result is not walking a file — only the
/// walk registers a global into the environment it happens in.
///
/// The shape is a project's: the host's names in one declaration file under `types/`, and
/// modules under `lib/` that require it.
fn globals_project() -> PathBuf {
    let root = scratch("globals");
    write(&root.join("htl.toml"), "[check]\npaths = [\"lib\"]\n");
    write(
        &root.join("types/host_globals.d.tl"),
        "global record std\n   record Json\n      encode: function(any): string\n      decode: function(string): any\n   end\n   json: Json\nend\nglobal VERSION: string\nglobal host_log: function(string)\n",
    );
    for n in 1..=2 {
        write(
            &root.join(format!("lib/m{n}/init.tl")),
            &format!(
                "require(\"host_globals\")\n\nlocal record m{n}\nend\n\nfunction m{n}.run(): string\n   host_log(VERSION)\n   return std.json.encode({{ a = {n} }})\nend\n\nreturn m{n}\n"
            ),
        );
    }
    root
}

#[test]
fn a_global_from_a_required_module_reaches_every_file_whatever_the_order() {
    let root = globals_project();
    let forward = diagnostics(&["--strict", "lib/m1", "lib/m2"], &root);
    let backward = diagnostics(&["--strict", "lib/m2", "lib/m1"], &root);
    let whole = diagnostics(&["--strict", "lib"], &root);

    assert!(
        forward.is_empty(),
        "both modules require the file that declares std, VERSION and host_log: {forward:?}"
    );
    assert_eq!(
        forward, backward,
        "the order the files are given in decides nothing"
    );
    assert_eq!(forward, whole, "a walk is the files it visits");
}
