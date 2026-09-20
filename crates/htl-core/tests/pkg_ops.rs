//! `htl pkg` through mlua-pkg's library rather than its binary: what install, add, update
//! and clean do to a project, and where they put things.
//!
//! The dependency is a git repository in a scratch directory, so these run offline and the
//! revisions are real ones rather than fixtures.

use htl_core::pkg::Project;
use mlua_pkg::ops::{AddSpec, CleanReport, Placement, UpdateOpts, UpdateOutcome};
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-pkgops", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args([
            "-c",
            "user.email=htl@example.invalid",
            "-c",
            "user.name=htl",
        ])
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git on PATH");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

const SOURCE: &str = "return { twice = function(n: number): number return n * 2 end }\n";
const DECL: &str = "local record mathx\n   twice: function(n: number): number\nend\nreturn mathx\n";

/// A dependency as a repository on disk: `src/` is the entry, `types/` is what it
/// publishes. Returns what to pin it by.
fn remote(name: &str) -> (String, String) {
    let dir = scratch(name);
    write(&dir.join("src/mathx.tl"), SOURCE);
    write(&dir.join("types/mathx.d.tl"), DECL);
    git(&dir, &["init", "-q"]);
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-qm", "mathx"]);
    let sha = git(&dir, &["rev-parse", "HEAD"]);
    (format!("file://{}", dir.display()), sha)
}

/// The same dependency as a repository really is: the package (both manifests, the
/// README, the licence texts, a dotfile of its own below the root) with the repository's
/// housekeeping beside it — a CI workflow, ignore rules, a stray `.DS_Store`.
fn remote_with_housekeeping(name: &str) -> (String, String) {
    let dir = scratch(name);
    write(&dir.join("src/mathx.tl"), SOURCE);
    write(&dir.join("src/.luacheckrc"), "std = \"lua54\"\n");
    write(&dir.join("types/mathx.d.tl"), DECL);
    write(&dir.join("README.md"), "# mathx\n");
    write(&dir.join("LICENSE-MIT"), "MIT\n");
    write(&dir.join("LICENSE-APACHE"), "Apache-2.0\n");
    write(&dir.join("htl.toml"), "[check]\nstrict = true\n");
    write(
        &dir.join("mlua-pkg.toml"),
        "[package]\nname = \"mathx\"\nversion = \"0.1.0\"\nentry = \"src\"\n",
    );
    write(
        &dir.join(".github/workflows/ci.yml"),
        "name: ci\non: [push]\n",
    );
    write(&dir.join(".gitignore"), "/target\n*.log\n");
    write(&dir.join(".DS_Store"), "\0\0\n");
    git(&dir, &["init", "-q"]);
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "mathx"]);
    let sha = git(&dir, &["rev-parse", "HEAD"]);
    (format!("file://{}", dir.display()), sha)
}

fn project(name: &str, url: &str, sha: &str) -> PathBuf {
    let root = scratch(name);
    write(
        &root.join("mlua-pkg.toml"),
        &format!(
            "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n\
             [deps.mathx]\ngit = \"{url}\"\nrev = \"{sha}\"\n"
        ),
    );
    root
}

/// Where installed packages go is htl's decision, written down in one place. The library
/// takes the directory it is given and reads neither the environment nor `target/`.
#[test]
fn install_places_packages_under_the_directory_htl_names() {
    let (url, sha) = remote("remote-install");
    let root = project("install", &url, &sha);
    // A `target/` in the project would send mlua-pkg's own binary to `target/mlua-pkgs`.
    std::fs::create_dir_all(root.join("target")).unwrap();

    let report = Project::at(&root).install().unwrap();
    assert_eq!(report.packages.len(), 1, "{report:?}");
    assert_eq!(report.direct, 1);
    assert_eq!(report.transitive, 0);

    let pkg = &report.packages[0];
    assert_eq!(pkg.name, "mathx");
    assert_eq!(pkg.sha, sha);
    assert_eq!(pkg.entry, PathBuf::from("src"));
    assert!(!pkg.patched);
    assert!(matches!(pkg.placement, Placement::Symlink(_)), "{pkg:?}");

    assert!(root.join(".htl/modules/vendored/mathx").exists());
    assert!(!root.join(".mlua-pkgs").exists(), "not the library default");
    assert!(
        !root.join("target/mlua-pkgs").exists(),
        "and not where a `target/` beside the manifest would send the binary"
    );
    assert!(root.join("mlua-pkg.lock").is_file());
    assert_eq!(
        std::fs::read_to_string(pkg.require_dir().join("mathx.tl")).unwrap(),
        SOURCE,
        "the entry below the package root is what require resolves through"
    );
}

/// The two halves meeting: a patched dependency is what install resolves from, and it says
/// so per package rather than leaving htl to work it out.
#[test]
fn install_resolves_a_patched_dependency_from_its_copy() {
    let (url, sha) = remote("remote-patched");
    let root = project("patched", &url, &sha);
    let p = Project::at(&root);
    p.patch("mathx", false).unwrap();
    write(
        &root.join("patches/mathx/src/mathx.tl"),
        "return { twice = function(n: number): number return n + n end }\n",
    );

    let report = Project::at(&root).install().unwrap();
    let pkg = &report.packages[0];
    assert!(pkg.patched, "{report:?}");
    assert_eq!(
        std::fs::canonicalize(pkg.root()).unwrap(),
        std::fs::canonicalize(root.join("patches/mathx")).unwrap()
    );
    assert!(
        std::fs::read_to_string(pkg.require_dir().join("mathx.tl"))
            .unwrap()
            .contains("n + n"),
        "the project's own edit is what is installed"
    );
}

/// The copy is the package, not the repository it was checked out of: every dot-entry at
/// its root goes, and nothing else does. `.github` and `.gitignore` are the two that cost
/// the project something once committed — an inert workflow the `.github/workflows/*`
/// gates still fire on, and ignore rules from another repository dropping files from this
/// one's commits.
#[test]
fn patch_drops_the_repositorys_dot_entries_and_keeps_the_package() {
    let (url, sha) = remote_with_housekeeping("remote-housekeeping");
    let root = project("housekeeping", &url, &sha);
    let done = Project::at(&root).patch("mathx", false).unwrap();
    assert_eq!(
        done.dropped,
        vec![".DS_Store", ".git", ".github", ".gitignore"],
        "and the caller is told which ones, so the report can say it"
    );

    let dir = root.join("patches/mathx");
    for gone in [".git", ".github", ".gitignore", ".DS_Store"] {
        assert!(
            std::fs::symlink_metadata(dir.join(gone)).is_err(),
            "{gone} is the repository's housekeeping and is not committed here"
        );
    }
    for kept in [
        "src/mathx.tl",
        "types/mathx.d.tl",
        "README.md",
        "LICENSE-MIT",
        "LICENSE-APACHE",
        "htl.toml",
        "mlua-pkg.toml",
        // Below the root a dotfile belongs to the package the way any other file there
        // does, and htl does not know which ones the dependency needs.
        "src/.luacheckrc",
    ] {
        assert!(dir.join(kept).is_file(), "{kept} is the package's own");
    }

    // The issue's acceptance: what is left is still a package install resolves from.
    let report = Project::at(&root).install().unwrap();
    assert!(report.packages[0].patched, "{report:?}");
    assert_eq!(
        std::fs::canonicalize(report.packages[0].root()).unwrap(),
        std::fs::canonicalize(&dir).unwrap()
    );
    assert!(
        report.packages[0].require_dir().join("mathx.tl").is_file(),
        "{report:?}"
    );
}

/// `add` replaces the whole entry and its spec has no room for a patch, so the key that
/// binds `patches/<dep>` to the dependency has to be carried across it. Without that the
/// project keeps building — against upstream, with the copy unread in the tree.
#[test]
fn add_keeps_the_patch_a_dependency_already_declares() {
    let (url, sha) = remote("remote-add");
    let root = project("add", &url, &sha);
    Project::at(&root).patch("mathx", false).unwrap();

    let done = Project::at(&root)
        .add(AddSpec {
            name: "mathx".into(),
            git: url.clone(),
            rev: Some(sha.clone()),
            ..AddSpec::default()
        })
        .unwrap();
    assert_eq!(
        done.kept_patch_dir,
        Some(PathBuf::from("patches/mathx")),
        "and it is reported rather than done silently"
    );
    let manifest = std::fs::read_to_string(root.join("mlua-pkg.toml")).unwrap();
    assert!(
        manifest.contains("patch_dir = \"patches/mathx\""),
        "{manifest}"
    );
    assert!(
        Project::at(&root).install().unwrap().packages[0].patched,
        "so the next install still resolves from the copy"
    );
}

/// A dependency the manifest did not have: nothing to carry, and the entry is written.
#[test]
fn add_writes_a_dependency_the_manifest_did_not_have() {
    let (url, sha) = remote("remote-new");
    let root = project("new", &url, &sha);
    let done = Project::at(&root)
        .add(AddSpec {
            name: "vec2".into(),
            git: url.clone(),
            tag: Some("v1.0".into()),
            ..AddSpec::default()
        })
        .unwrap();
    assert!(done.kept_patch_dir.is_none(), "{done:?}");
    let manifest = std::fs::read_to_string(root.join("mlua-pkg.toml")).unwrap();
    assert!(manifest.contains("[deps.vec2]"), "{manifest}");
    assert!(manifest.contains("tag = \"v1.0\""), "{manifest}");
}

/// `add` is the one verb that may run where there is no project yet: it writes the manifest
/// it needs, and says that it had to.
#[test]
fn add_writes_the_manifest_when_there_is_none() {
    let root = scratch("add-bare");
    std::fs::create_dir_all(&root).unwrap();
    let done = Project::at(&root)
        .add(AddSpec::new("mathx", "https://example.invalid/mathx"))
        .unwrap();
    assert!(done.report.manifest_created, "{done:?}");
    let manifest = std::fs::read_to_string(root.join("mlua-pkg.toml")).unwrap();
    assert!(manifest.contains("[deps.mathx]"), "{manifest}");
    assert!(manifest.contains("[package]"), "{manifest}");
}

/// mlua-pkg walks a map, so the same project reports its dependencies in a different order
/// every run. A report a person reads, and diffs against the last one, is sorted.
#[test]
fn update_reports_its_dependencies_in_a_stable_order() {
    let root = scratch("update");
    // Both are pinned to a commit, which update skips before it touches the network — so
    // this asserts the ordering without fetching anything.
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n\
         [deps.zeta]\ngit = \"https://example.invalid/zeta\"\nrev = \"aaaa\"\n\n\
         [deps.alpha]\ngit = \"https://example.invalid/alpha\"\nrev = \"bbbb\"\n",
    );
    let report = Project::at(&root).update(UpdateOpts::default()).unwrap();
    let names: Vec<&str> = report.entries.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, vec!["alpha", "zeta"]);
    assert!(
        report
            .entries
            .iter()
            .all(|(_, o)| matches!(o, UpdateOutcome::Skipped(_))),
        "{report:?}"
    );
    assert!(
        report.install.is_none(),
        "nothing to install when every pin was skipped"
    );
}

#[test]
fn clean_tells_an_empty_cache_from_a_swept_one_and_from_no_lockfile() {
    let (url, sha) = remote("remote-clean");
    let root = project("clean", &url, &sha);
    let p = Project::at(&root);
    assert_eq!(p.clean(false).unwrap(), CleanReport::NoLockfile);

    p.install().unwrap();
    assert_eq!(
        p.clean(false).unwrap(),
        CleanReport::StaleRemoved { removed: 0 },
        "everything in the cache is what the lockfile refers to"
    );

    assert_eq!(p.clean(true).unwrap(), CleanReport::CacheRemoved);
    assert!(!root.join(".htl/modules/cache").exists());
    assert!(
        // The link itself, not what it points at: sweeping the cache is what leaves it
        // dangling, and the next install is what repairs it.
        std::fs::symlink_metadata(root.join(".htl/modules/vendored/mathx")).is_ok(),
        "what install placed is left alone"
    );
}
