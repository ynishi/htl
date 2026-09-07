//! `htl pkg patch <dep>`: a dependency's source taken into `patches/<dep>/`, where the
//! project owns it. What is asserted here is the part htl decides — the directory, the
//! `patch_dir` line written into a manifest a person wrote, and the refusal to overwrite a
//! copy git has not been told about. The copy itself and the `patch_base` bookkeeping are
//! mlua-pkg's, and are exercised through it rather than reimplemented.

use htl_core::pkg::Project;
use mlua_pkg::lockfile::{LockedPkg, Lockfile};
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-pkgpatch", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// git, with an identity of its own: the scratch repositories are not the person's, and a
/// machine with no `user.email` configured must still be able to run these.
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

/// A dependency as a repository on disk: a source under `src/`, the declaration it
/// publishes under `types/`, one commit. Returns what to pin it by.
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

/// A project depending on it, with a comment of its own in the manifest.
fn project(name: &str, url: &str, sha: &str) -> PathBuf {
    let root = scratch(name);
    write(
        &root.join("mlua-pkg.toml"),
        &format!(
            "# the arithmetic the report rests on\n\
             [package]\nname = \"p\"\nversion = \"0.1.0\"\n\n\
             [deps.mathx]\ngit = \"{url}\"\nrev = \"{sha}\"\n"
        ),
    );
    write(&root.join("src/main.tl"), "print(1)\n");
    root
}

#[test]
fn patch_takes_the_package_root_into_the_tree_and_records_where_it_came_from() {
    let (url, sha) = remote("remote-take");
    let root = project("take", &url, &sha);

    let report = Project::at(&root).patch("mathx", false).unwrap();
    assert!(report.created, "{report:?}");
    assert_eq!(report.base, sha);
    assert_eq!(report.patch_dir, root.join("patches/mathx"));

    assert!(root.join("patches/mathx/src/mathx.tl").is_file());
    assert!(
        root.join("patches/mathx/types/mathx.d.tl").is_file(),
        "the whole package root is copied, so what the dep publishes comes with it"
    );
    assert!(
        !root.join("patches/mathx/.git").exists(),
        "but not the repository it was checked out of: git would read that as an embedded \
         repository and commit a gitlink instead of the files"
    );

    let manifest = std::fs::read_to_string(root.join("mlua-pkg.toml")).unwrap();
    assert!(
        manifest.contains("patch_dir = \"patches/mathx\""),
        "the manifest says which dependency the directory stands in for: {manifest}"
    );
    assert!(
        manifest.contains("# the arithmetic the report rests on"),
        "and the file a person wrote survives the edit: {manifest}"
    );

    let lock = Lockfile::read(root.join("mlua-pkg.lock")).unwrap();
    assert_eq!(
        lock.pkg[0].patch_base.as_deref(),
        Some(sha.as_str()),
        "the revision the copy was taken from is the lockfile's to remember"
    );
    assert_eq!(lock.pkg[0].patch_dir, Some(PathBuf::from("patches/mathx")));

    assert_eq!(
        Project::at(&root)
            .patches
            .iter()
            .map(|p| (p.name.as_str(), p.dir.clone()))
            .collect::<Vec<_>>(),
        vec![("mathx", root.join("patches/mathx"))],
        "and the project reads it back as a patched dependency, by name"
    );
}

#[test]
fn a_name_the_manifest_does_not_declare_is_refused_before_anything_is_written() {
    let (url, sha) = remote("remote-unknown");
    let root = project("unknown", &url, &sha);
    let before = std::fs::read_to_string(root.join("mlua-pkg.toml")).unwrap();

    let err = Project::at(&root)
        .patch("mathy", false)
        .unwrap_err()
        .to_string();
    assert!(err.contains("mathy"), "{err}");
    assert_eq!(
        std::fs::read_to_string(root.join("mlua-pkg.toml")).unwrap(),
        before,
        "a name that is not a dependency leaves the manifest alone"
    );
    assert!(!root.join("patches").exists());
}

/// The refresh replaces the directory with the pinned upstream. The project's own change
/// survives that through git and nothing else, so a copy git cannot account for is not
/// overwritten — and `--force` is how it is discarded on purpose.
#[test]
fn a_copy_with_uncommitted_changes_is_not_overwritten() {
    let (url, sha) = remote("remote-dirty");
    let root = project("dirty", &url, &sha);
    git(&root, &["init", "-q"]);
    let p = Project::at(&root);
    p.patch("mathx", false).unwrap();

    // Straight after `patch` the copy is untracked, which is uncommitted like any other.
    let err = p.patch("mathx", false).unwrap_err().to_string();
    assert!(err.contains("patches/mathx"), "{err}");
    assert!(err.contains("--force"), "{err}");

    // Committed, git can carry it forward, and the refresh goes ahead.
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "take mathx into the tree"]);
    let again = p.patch("mathx", false).unwrap();
    assert!(!again.created, "the second one is a refresh: {again:?}");
    assert_eq!(again.base, sha);

    // An edit that is not committed stops it again, and says which file it is.
    let edited = "return { twice = function(n: number): number return n + n end }\n";
    write(&root.join("patches/mathx/src/mathx.tl"), edited);
    let err = p.patch("mathx", false).unwrap_err().to_string();
    assert!(err.contains("patches/mathx/src/mathx.tl"), "{err}");

    let forced = p.patch("mathx", true).unwrap();
    assert!(!forced.created, "{forced:?}");
    assert_eq!(
        std::fs::read_to_string(root.join("patches/mathx/src/mathx.tl")).unwrap(),
        SOURCE,
        "--force is what discards the edit"
    );
}

/// A project holding a patch, as an install leaves it: the manifest names the directory,
/// the copy is there, and the lockfile says which revision the pin resolves to and which
/// one the copy was taken from.
fn installed_patch(name: &str, locked: &str, base: Option<&str>) -> PathBuf {
    let root = scratch(name);
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n[deps.mathx]\n\
         git = \"https://example.invalid/mathx\"\npatch_dir = \"patches/mathx\"\n",
    );
    write(&root.join("patches/mathx/src/mathx.tl"), SOURCE);
    Lockfile {
        version: 1,
        pkg: vec![LockedPkg {
            name: "mathx".into(),
            source: "git+https://example.invalid/mathx".into(),
            tag: None,
            rev: None,
            branch: None,
            sha: locked.to_string(),
            entry: PathBuf::from("src"),
            patch_dir: Some(PathBuf::from("patches/mathx")),
            patch_base: base.map(str::to_string),
        }],
    }
    .write(root.join("mlua-pkg.lock"))
    .unwrap();
    root
}

/// A patch is bound to the revision it was taken from. While the pin still resolves to it
/// the copy is what the dependency resolves from; once the pin moves the copy sits in the
/// tree unused, and saying so is the only thing that keeps it from being forgotten there.
#[test]
fn a_patch_is_in_use_while_the_pin_still_resolves_to_its_base() {
    let sha = "a".repeat(40);
    let root = installed_patch("in-use", &sha, Some(&sha));
    let status = Project::at(&root).patch_status();
    assert_eq!(status.len(), 1, "{status:?}");
    assert!(status[0].in_use, "{status:?}");

    let moved = installed_patch("moved", &"b".repeat(40), Some(&sha));
    let status = Project::at(&moved).patch_status();
    assert!(!status[0].in_use, "the pin moved off the base: {status:?}");
    assert_eq!(status[0].base.as_deref(), Some(sha.as_str()));
    assert_eq!(status[0].locked.as_deref(), Some("b".repeat(40).as_str()));

    let unrecorded = installed_patch("unrecorded", &sha, None);
    let status = Project::at(&unrecorded).patch_status();
    assert!(
        !status[0].in_use,
        "and a copy with no recorded base is not one either: {status:?}"
    );
}

/// Outside a repository the question cannot be asked at all, and a refresh would overwrite
/// whatever is in the directory. That is said rather than guessed at.
#[test]
fn a_copy_git_cannot_account_for_is_not_overwritten_either() {
    let (url, sha) = remote("remote-nogit");
    let root = project("nogit", &url, &sha);
    let p = Project::at(&root);
    p.patch("mathx", false).unwrap();

    let err = p.patch("mathx", false).unwrap_err().to_string();
    assert!(err.contains("cannot tell"), "{err}");
    assert!(err.contains("--force"), "{err}");
    assert!(p.patch("mathx", true).is_ok());
}
