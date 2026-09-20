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

/// The same dependency, published as an mlua-pkg project itself — which is what a
/// package that has dependencies of its own looks like, and what `patch` copies whole.
fn remote_with_manifest(name: &str) -> (String, String) {
    let dir = scratch(name);
    write(
        &dir.join("mlua-pkg.toml"),
        "[package]\nname = \"mathx\"\nversion = \"0.1.0\"\nentry = \"src\"\n",
    );
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

    let done = Project::at(&root).patch("mathx", false).unwrap();
    assert!(done.report.created, "{done:?}");
    assert_eq!(done.report.base, sha);
    assert_eq!(done.report.patch_dir, root.join("patches/mathx"));

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
    assert_eq!(
        done.dropped,
        vec![".git"],
        "which is the one dot-entry this repository has, and the caller is told"
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
    assert!(
        !again.report.created,
        "the second one is a refresh: {again:?}"
    );
    assert_eq!(again.report.base, sha);

    // An edit that is not committed stops it again, and says which file it is.
    let edited = "return { twice = function(n: number): number return n + n end }\n";
    write(&root.join("patches/mathx/src/mathx.tl"), edited);
    let err = p.patch("mathx", false).unwrap_err().to_string();
    assert!(err.contains("patches/mathx/src/mathx.tl"), "{err}");

    let forced = p.patch("mathx", true).unwrap();
    assert!(!forced.report.created, "{forced:?}");
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

/// The copy carries the dependency's own `mlua-pkg.toml`, and that file does not make
/// `patches/mathx` a project. Whoever the root is gets the `.htl/` — the store, the
/// installed deps, the entry links — and a second one inside the project's tree is what
/// #267 saw: `patches/<dep>/.htl/modules/entries` in the tarball cargo was verifying.
#[test]
fn the_dependencys_own_manifest_does_not_make_the_copy_a_project() {
    let (url, sha) = remote_with_manifest("remote-own-manifest");
    let root = project("own-manifest", &url, &sha);
    Project::at(&root).patch("mathx", false).unwrap();
    assert!(
        root.join("patches/mathx/mlua-pkg.toml").is_file(),
        "the whole package root is copied, manifest included"
    );

    let inside = root.join("patches/mathx/src/mathx.tl");
    let found = Project::find(&inside).expect("a project above the copy");
    assert_eq!(
        found.root,
        std::fs::canonicalize(&root).unwrap(),
        "the project is the one that declared the patch_dir"
    );

    htl_core::Htl::new().unwrap().apply_project(&found).unwrap();
    assert!(
        !root.join("patches/mathx/.htl").exists(),
        "so nothing puts a second .htl/ inside the copy"
    );
    assert!(
        root.join(".htl/modules/entries").is_dir(),
        "and the project's own is where it always was"
    );
}

/// Where `require` reads the copy from, and what decides it.
///
/// The lockfile is the answer whenever there is one: it is the install's own record, and
/// an `entry` the consumer overrode is already folded into it. A project whose lockfile
/// is not committed — or the copy `cargo package` verifies when it is not — has the
/// dependency's own `mlua-pkg.toml` instead, which `htl pkg patch` copied along with the
/// sources, and then mlua-pkg's fallback chain. Three sources for one fact, so the
/// directory htl searches and the directory an install would have linked cannot differ.
#[test]
fn the_entry_the_copy_is_required_from_comes_from_the_lockfile_then_the_copy() {
    let root = installed_patch("entry-locked", &"a".repeat(40), None);
    // The copy says `lib` and the lockfile says `src`. The lockfile wins: it is what an
    // install resolved, and `entries/mathx` points there.
    write(
        &root.join("patches/mathx/mlua-pkg.toml"),
        "[package]\nname = \"mathx\"\nversion = \"0.1.0\"\nentry = \"lib\"\n",
    );
    assert_eq!(
        Project::at(&root).patches[0].entry,
        root.join("patches/mathx/src")
    );

    // Without one — a clone that does not commit `mlua-pkg.lock` — the copy answers for
    // itself, whether or not the directory it names has been written yet.
    std::fs::remove_file(root.join("mlua-pkg.lock")).unwrap();
    assert_eq!(
        Project::at(&root).patches[0].entry,
        root.join("patches/mathx/lib"),
        "the copy's own [package].entry, which patch copied in with the sources"
    );

    // And with neither, mlua-pkg's chain: `src/`, then `lua/`, then the root.
    std::fs::remove_file(root.join("patches/mathx/mlua-pkg.toml")).unwrap();
    assert_eq!(
        Project::at(&root).patches[0].entry,
        root.join("patches/mathx/src")
    );
    std::fs::rename(
        root.join("patches/mathx/src"),
        root.join("patches/mathx/lua"),
    )
    .unwrap();
    assert_eq!(
        Project::at(&root).patches[0].entry,
        root.join("patches/mathx/lua")
    );
}

/// What goes on the search path is not the entry but the directory that holds it under
/// the dependency's name — the link `entries/<name>` in the form a path can express. A
/// package whose entry is named after it has one, and every name then resolves to the
/// file the link resolves it to; a flat package has none anywhere in the copy, and gets
/// the entry itself, which answers for the package's own module and not for what is
/// below it.
#[test]
fn the_directory_on_the_path_is_the_one_holding_the_entry_under_the_dependencys_name() {
    let root = installed_patch("search-named", &"a".repeat(40), None);
    write(
        &root.join("patches/mathx/mlua-pkg.toml"),
        "[package]\nname = \"mathx\"\nversion = \"0.1.0\"\nentry = \"src/mathx\"\n",
    );
    std::fs::remove_file(root.join("mlua-pkg.lock")).unwrap();
    let p = Project::at(&root);
    assert_eq!(p.patches[0].entry, root.join("patches/mathx/src/mathx"));
    assert_eq!(
        p.patch_search_dirs(),
        vec![root.join("patches/mathx/src")],
        "so `mathx.sub` reads src/mathx/sub.tl, as entries/mathx -> src/mathx does"
    );

    let flat = installed_patch("search-flat", &"a".repeat(40), None);
    assert_eq!(
        Project::at(&flat).patch_search_dirs(),
        vec![flat.join("patches/mathx/src")],
        "a flat package has nothing named after it, and the entry is what is left"
    );

    // The copy root is named after the dependency, so a package whose entry is the root
    // has one after all: `patches/`.
    let rooted = installed_patch("search-root", &"a".repeat(40), None);
    write(
        &rooted.join("patches/mathx/mlua-pkg.toml"),
        "[package]\nname = \"mathx\"\nversion = \"0.1.0\"\nentry = \".\"\n",
    );
    std::fs::remove_file(rooted.join("mlua-pkg.lock")).unwrap();
    assert_eq!(
        Project::at(&rooted).patch_search_dirs(),
        vec![rooted.join("patches")]
    );
}
