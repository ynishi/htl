//! Source collection must not walk into installed packages, build output or tool state:
//! `htl check .` / `htl fmt .` / `htl test` operate on the project's own files.

use htl_core::testing::discover_tests;
use htl_core::{collect_tl, is_skipped_dir};
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-skip", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn rel(root: &Path, files: &[PathBuf]) -> Vec<String> {
    let mut v: Vec<String> = files
        .iter()
        .map(|f| {
            f.strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    v.sort();
    v
}

fn project() -> PathBuf {
    let root = scratch("proj");
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n[deps]\n",
    );
    write(&root.join("src/main.tl"), "print(1)\n");
    write(&root.join("src/util.tl"), "return {}\n");
    write(&root.join("tests/util_test.tl"), "print(2)\n");
    // dependency material that `htl pkg install` produces under the project
    write(
        &root.join(".htl/modules/cache/git/x/vec2/abc/src/vec2.tl"),
        "return {}\n",
    );
    write(
        &root.join(".htl/modules/cache/git/x/vec2/abc/tests/vec2_test.tl"),
        "print(3)\n",
    );
    write(
        &root.join(".htl/modules/vendored/vec2/vec2.tl"),
        "return {}\n",
    );
    // and what the `mlua-pkg` binary leaves behind when it is run on its own
    write(
        &root.join(".mlua-pkgs/vendored/vec1/vec1.tl"),
        "return {}\n",
    );
    // build output and other tool state
    write(&root.join("target/debug/gen.tl"), "print(4)\n");
    write(&root.join(".hidden/x_test.tl"), "print(5)\n");
    write(&root.join("node_modules/m/m.tl"), "return {}\n");
    root
}

#[test]
fn collect_tl_skips_packages_build_output_and_dot_dirs() {
    let root = project();
    let files = collect_tl(std::slice::from_ref(&root)).unwrap();
    assert_eq!(
        rel(&root, &files),
        vec!["src/main.tl", "src/util.tl", "tests/util_test.tl"]
    );
}

#[test]
fn discover_tests_skips_dependency_tests() {
    let root = project();
    let files = discover_tests(std::slice::from_ref(&root)).unwrap();
    assert_eq!(rel(&root, &files), vec!["tests/util_test.tl"]);
}

#[test]
fn an_explicit_root_inside_a_skipped_dir_is_still_walked() {
    let root = project();
    let inner = root.join(".htl/modules/vendored/vec2");
    let files = collect_tl(std::slice::from_ref(&inner)).unwrap();
    assert_eq!(rel(&inner, &files), vec!["vec2.tl"]);
    // `.htl/` by the dot-directory rule, `.mlua-pkgs/` by name for a project that ran the
    // `mlua-pkg` binary itself.
    assert!(is_skipped_dir(&root.join(".htl"), &[]));
    assert!(is_skipped_dir(&root.join(".mlua-pkgs"), &[]));
    assert!(!is_skipped_dir(&root.join("src"), &[]));
}

/// A pkgs dir named by path is skipped even under a plain name: `extra` says what a name
/// cannot.
#[test]
fn project_pkgs_dir_is_skipped_by_path() {
    let root = project();
    let custom = root.join("deps-here");
    write(&custom.join("vendored/vec2/vec2.tl"), "return {}\n");
    let extra = vec![custom.clone()];
    assert!(is_skipped_dir(&custom, &extra));
    assert!(!is_skipped_dir(&root.join("src"), &extra));
}
