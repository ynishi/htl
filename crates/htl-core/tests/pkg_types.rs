//! A dep publishes its declarations at `types/` in its package root, which is outside the
//! entry directory `require` resolves through — so they are copied into the project's own
//! `types/`, the one that is committed and checks on a fresh clone.

use htl_core::pkg::Project;
use mlua_pkg::lockfile::{LockedPkg, Lockfile};
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-pkgtypes", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

const DECL: &str = "local record mathx\n   twice: function(n: number): number\nend\nreturn mathx\n";

/// A project with one dep installed the way `mlua-pkg install` leaves it: the package in
/// the cache, `vendored/<name>` a symlink to its entry directory, and a lockfile saying
/// which directory that entry was.
fn project_with_dep(name: &str, entry: &str, decls: &[(&str, &str)]) -> PathBuf {
    let root = scratch(name);
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n[deps]\n",
    );
    let pkg_root = root.join(".htl/modules/cache/git/x/mathx/abc");
    write(&pkg_root.join(entry).join("mathx.tl"), "return {}\n");
    for (file, text) in decls {
        write(&pkg_root.join("types").join(file), text);
    }
    let vendored = root.join(".htl/modules/vendored");
    std::fs::create_dir_all(&vendored).unwrap();
    std::os::unix::fs::symlink(pkg_root.join(entry), vendored.join("mathx")).unwrap();
    Lockfile {
        version: 1,
        pkg: vec![LockedPkg {
            name: "mathx".into(),
            source: "git+https://example.invalid/mathx".into(),
            tag: None,
            rev: None,
            branch: None,
            sha: "a".repeat(40),
            entry: PathBuf::from(entry),
        }],
    }
    .write(root.join("mlua-pkg.lock"))
    .unwrap();
    root
}

/// A project with nothing installed: `types/` is a project's either way.
fn bare_project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n[deps]\n",
    );
    root
}

/// A checkout shaped like teal-types: `types/<library>/<module>.d.tl`, with a nested
/// module and a file that is not a declaration at all.
fn teal_types_checkout(name: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("types/luasocket/socket.d.tl"), DECL);
    write(&root.join("types/luasocket/ltn12.d.tl"), DECL);
    write(&root.join("types/luasocket/socket/http.d.tl"), DECL);
    write(&root.join("types/luasocket/README.md"), "how these were written\n");
    write(&root.join("types/lpeg/lpeg.d.tl"), DECL);
    root
}

#[test]
fn a_deps_published_declarations_land_in_types() {
    let root = project_with_dep("published", "src", &[("mathx.d.tl", DECL)]);
    let sync = Project::at(&root).sync_types().unwrap();
    assert_eq!(sync.written.len(), 1, "{sync:?}");
    assert!(sync.taken.is_empty(), "{sync:?}");
    assert!(root.join("types/mathx.d.tl").is_file());
    assert_eq!(
        std::fs::read_to_string(root.join("types/mathx.d.tl.src")).unwrap(),
        format!("mathx {} types/mathx.d.tl\n", "a".repeat(40)),
        "what published it, at which revision, and the path it had there"
    );
}

/// A dep may publish a module of its own below `types/`, and the path below it is the
/// module name: `mathx/vec.d.tl` is `require("mathx.vec")`.
#[test]
fn a_nested_module_a_dep_publishes_keeps_its_path() {
    let root = project_with_dep(
        "nested",
        "src",
        &[("mathx.d.tl", DECL), ("mathx/vec.d.tl", DECL)],
    );
    let sync = Project::at(&root).sync_types().unwrap();
    assert_eq!(sync.written.len(), 2, "{sync:?}");
    assert!(root.join("types/mathx/vec.d.tl").is_file());
}

/// `entry = "."` (a package whose root is what `require` resolves through): the root is
/// the symlink target itself, not its parent.
#[test]
fn a_flat_package_root_is_found_through_its_entry() {
    let root = project_with_dep("flat", ".", &[("mathx.d.tl", DECL)]);
    let sync = Project::at(&root).sync_types().unwrap();
    assert_eq!(sync.written.len(), 1, "{sync:?}");
    assert!(root.join("types/mathx.d.tl").is_file());
}

#[test]
fn a_name_the_project_already_has_is_kept_and_reported() {
    let root = project_with_dep("taken", "src", &[("mathx.d.tl", DECL)]);
    write(&root.join("types/mathx.d.tl"), "-- by hand\n");
    let sync = Project::at(&root).sync_types().unwrap();
    assert!(sync.written.is_empty(), "{sync:?}");
    assert_eq!(sync.taken.len(), 1, "{sync:?}");
    assert_eq!(
        std::fs::read_to_string(root.join("types/mathx.d.tl")).unwrap(),
        "-- by hand\n",
        "the file that was there is the one that stays"
    );
    assert!(
        !root.join("types/mathx.d.tl.src").exists(),
        "and nothing is recorded for a file that was not written"
    );
}

/// Only declarations: a dep's `types/` may hold a README, and `.tl` sources there are
/// implementations rather than something to publish.
#[test]
fn only_declarations_are_taken() {
    let root = project_with_dep(
        "mixed",
        "src",
        &[
            ("mathx.d.tl", DECL),
            ("README.md", "how these were written\n"),
            ("helper.tl", "return {}\n"),
        ],
    );
    let sync = Project::at(&root).sync_types().unwrap();
    assert_eq!(sync.written.len(), 1, "{sync:?}");
    assert!(!root.join("types/README.md").exists());
    assert!(!root.join("types/helper.tl").exists());
}

#[test]
fn nothing_happens_before_an_install() {
    let root = scratch("uninstalled");
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n[deps]\n",
    );
    let sync = Project::at(&root).sync_types().unwrap();
    assert!(sync.written.is_empty() && sync.taken.is_empty(), "{sync:?}");
    assert!(
        !root.join("types").exists(),
        "an empty types/ is not created on the way"
    );
}

/// `htl types add luasocket`: the library's directory in the collection is dropped, and
/// everything below it keeps the path it had — that path is the module name.
#[test]
fn add_drops_the_library_dir_and_keeps_what_is_below_it() {
    let root = bare_project("add");
    let checkout = teal_types_checkout("collection");
    let sync = Project::at(&root)
        .add_types_from(&checkout, "luasocket", "beef", false)
        .unwrap();
    assert_eq!(sync.written.len(), 3, "{sync:?}");
    assert!(root.join("types/socket.d.tl").is_file());
    assert!(root.join("types/ltn12.d.tl").is_file());
    assert!(
        root.join("types/socket/http.d.tl").is_file(),
        "socket.http is a module of its own; flattening it would rename it"
    );
    assert!(!root.join("types/README.md").exists());
    assert!(!root.join("types/lpeg.d.tl").exists(), "only the library asked for");
    assert_eq!(
        std::fs::read_to_string(root.join("types/socket/http.d.tl.src")).unwrap(),
        "teal-types beef types/luasocket/socket/http.d.tl\n",
        "the note names the collection, the revision, and the path it had there"
    );
}

#[test]
fn a_library_the_collection_does_not_have_names_the_near_ones() {
    let root = bare_project("missing");
    let checkout = teal_types_checkout("collection-near");
    let err = Project::at(&root)
        .add_types_from(&checkout, "socket", "beef", false)
        .unwrap_err()
        .to_string();
    assert!(err.contains("luasocket"), "{err}");
}

#[test]
fn add_keeps_what_is_there_until_it_is_forced() {
    let root = bare_project("forced");
    let checkout = teal_types_checkout("collection-force");
    write(&root.join("types/socket.d.tl"), "-- by hand\n");
    let p = Project::at(&root);

    let kept = p.add_types_from(&checkout, "luasocket", "beef", false).unwrap();
    assert_eq!(kept.taken.len(), 1, "{kept:?}");
    assert_eq!(
        std::fs::read_to_string(root.join("types/socket.d.tl")).unwrap(),
        "-- by hand\n"
    );

    let forced = p.add_types_from(&checkout, "luasocket", "beef", true).unwrap();
    assert_eq!(forced.written.len(), 3, "{forced:?}");
    assert_ne!(
        std::fs::read_to_string(root.join("types/socket.d.tl")).unwrap(),
        "-- by hand\n",
        "--force is what replaces it"
    );
}
