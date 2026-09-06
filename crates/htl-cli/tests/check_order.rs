//! A file's diagnostics must not depend on what was checked before it.
//!
//! `htl check` adds each file's directory to the search path as it walks. Without putting
//! the path back afterwards, the Nth file resolves `require` against the directories of
//! the first N-1 too — so the same file reports differently depending on where it fell in
//! the walk, and the direction is the dangerous one: an error that should be reported
//! disappears because something checked earlier happened to provide the module.

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "htl-cli-order-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn diagnostics(args: &[&str], cwd: &Path) -> Vec<String> {
    let out = Command::new(env!("CARGO_BIN_EXE_htl"))
        .arg("check")
        .args(args)
        .arg("--format")
        .arg("json")
        .arg("--no-cache")
        .current_dir(cwd)
        .output()
        .unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("stdout is one JSON document");
    v["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .map(|d| {
            // The checker echoes the path as it was given, so `htl check .` says
            // `./b/main.tl` where `htl check b/main.tl` says `b/main.tl`. That spelling is
            // not what these tests are about.
            let file = d["file"].as_str().unwrap_or("").trim_start_matches("./");
            format!("{file}:{}: {}", d["line"], d["message"].as_str().unwrap_or(""))
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
    write(&root.join("b/main.tl"), "local util = require(\"util\")\nlocal x: integer = util.f()\nprint(x)\n");
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
    assert_eq!(alone, after, "checking a/ first must not make b/'s missing module resolve");
    assert_eq!(alone, before, "nor must checking it second");
}

#[test]
fn walking_a_tree_agrees_with_checking_its_files_one_at_a_time() {
    let root = two_dirs();
    let whole = diagnostics(&["."], &root);
    let mut apart = diagnostics(&["a/util.tl"], &root);
    apart.extend(diagnostics(&["b/main.tl"], &root));
    assert_eq!(whole, apart, "a walk is the files it visits, in the state each would be checked in");
}

/// `add_layout_paths` deliberately puts the project root and `src/` on the path for a file
/// under `tests/`, so that `htl check tests` sees what `htl test` sees. Restoring the path
/// per file must not take that away.
#[test]
fn a_test_file_still_sees_the_project_root_and_src() {
    let root = scratch("tests-layout");
    write(
        &root.join("src/util.tl"),
        "local record util\nend\nfunction util.f(): integer\n   return 1\nend\nreturn util\n",
    );
    write(&root.join("tests/util_test.tl"), "local util = require(\"util\")\nlocal x: integer = util.f()\nprint(x)\n");
    let d = diagnostics(&["."], &root);
    assert!(d.is_empty(), "a file under tests/ resolves modules from src/: {d:?}");
}
