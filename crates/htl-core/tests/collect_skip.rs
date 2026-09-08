//! Source collection must not walk into installed packages, build output or tool state:
//! `htl check .` / `htl fmt .` / `htl test` operate on the project's own files.

use htl_core::testing::{discover_tests, discover_tests_skipping};
use htl_core::{collect_tl, collect_tl_skipping, is_skipped_dir, patched_dirs};
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

/// A project holding a dependency it patched: `patch_dir` on the dep, and the copy under
/// `patches/mathx` with a source and a test of the dependency's own.
fn patched_project() -> PathBuf {
    let root = scratch("patched");
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n[deps.mathx]\n\
         git = \"https://example.invalid/mathx\"\nrev = \"abc\"\npatch_dir = \"patches/mathx\"\n",
    );
    write(&root.join("src/main.tl"), "print(1)\n");
    write(&root.join("patches/mathx/src/mathx.tl"), "return {}\n");
    write(
        &root.join("patches/mathx/tests/mathx_test.tl"),
        "print(2)\n",
    );
    root
}

/// The copy is committed, project-owned code, and a type error in it is the project's to
/// fix — so `htl check` reads it like the rest of the tree.
#[test]
fn a_patched_dependency_is_checked_with_the_project() {
    let root = patched_project();
    let files = collect_tl(std::slice::from_ref(&root)).unwrap();
    assert_eq!(
        rel(&root, &files),
        vec![
            "patches/mathx/src/mathx.tl",
            "patches/mathx/tests/mathx_test.tl",
            "src/main.tl"
        ]
    );
}

/// What the copy holds is a diff against the revision it was taken from: reformatting it
/// would turn every file into a diff and bury the change, and its `*_test.tl` are the
/// dependency's suite rather than the project's.
#[test]
fn fmt_and_test_leave_a_patched_dependency_alone() {
    let root = patched_project();
    let skip = patched_dirs(&root);
    assert_eq!(skip.len(), 1, "{skip:?}");
    assert!(skip[0].ends_with("patches/mathx"), "{skip:?}");

    let files = collect_tl_skipping(std::slice::from_ref(&root), &skip).unwrap();
    assert_eq!(rel(&root, &files), vec!["src/main.tl"]);

    let tests = discover_tests_skipping(std::slice::from_ref(&root), &skip).unwrap();
    assert!(tests.is_empty(), "{tests:?}");
    assert_eq!(
        rel(&root, &discover_tests(std::slice::from_ref(&root)).unwrap()),
        vec!["patches/mathx/tests/mathx_test.tl"],
        "and it is the skip that leaves them out, not the walk missing them"
    );
}

/// A dependency with no `patch_dir` contributes nothing to skip: the walkers are unchanged
/// for a project that has patched nothing.
#[test]
fn a_project_with_no_patches_skips_nothing_extra() {
    let root = project();
    assert!(patched_dirs(&root).is_empty());
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

/// A project holding a `target_dir` dep: the copy is committed to the repo under a name the
/// project chose, and `lua/mine.tl` — the project's own — sits beside it under the same
/// parent. Only the manifest says which is which.
fn target_dir_project() -> PathBuf {
    let root = scratch("targetdir");
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n[deps.mathx]\n\
         git = \"https://example.invalid/mathx\"\ntag = \"v1\"\ntarget_dir = \"lua/mathx\"\n",
    );
    write(&root.join("src/main.tl"), "print(1)\n");
    write(&root.join("tests/main_test.tl"), "print(2)\n");
    // the dependency's own source and tests, put there by `mlua-pkg install`
    write(&root.join("lua/mathx/init.tl"), "return {}\n");
    write(&root.join("lua/mathx/mathx_test.tl"), "print(3)\n");
    // and the project's own module, under the same parent
    write(&root.join("lua/mine.tl"), "return {}\n");
    root
}

/// Install rewrites the copy every time it runs, so its errors are not the project's to fix
/// and an edit made there does not survive — `htl check` and `htl fmt` stay out of it.
#[test]
fn a_target_dir_copy_is_not_the_projects_to_check() {
    let root = target_dir_project();
    let files = collect_tl(std::slice::from_ref(&root)).unwrap();
    assert_eq!(
        rel(&root, &files),
        vec!["lua/mine.tl", "src/main.tl", "tests/main_test.tl"],
        "the copy is a dependency's source; what sits beside it is not"
    );
}

/// And the `*_test.tl` in there are the dependency's suite rather than the project's.
#[test]
fn a_target_dir_copys_tests_are_not_the_projects() {
    let root = target_dir_project();
    let files = discover_tests(std::slice::from_ref(&root)).unwrap();
    assert_eq!(rel(&root, &files), vec!["tests/main_test.tl"]);
}

/// A dependency with no `target_dir` contributes nothing: the walkers are unchanged for a
/// project that vendors nothing.
#[test]
fn a_project_with_no_target_dir_copies_skips_nothing_extra() {
    let root = project();
    let skip = htl_core::project_skip_dirs(&root);
    assert_eq!(skip.len(), 1, "just the pkgs dir: {skip:?}");
    assert!(skip[0].ends_with(".htl/modules"), "{skip:?}");
}
