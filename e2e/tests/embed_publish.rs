//! Publishing a crate that embeds Teal with a dependency: `cargo package`, with nothing
//! written by hand to make it work.
//!
//! The acceptance of #266, end to end. A project `htl new --embed` writes, with the
//! scaffold's own `htlx` dependency installed and then taken into the tree by `htl pkg
//! patch`, is committed and packaged. Nothing is added to `htl.toml` and nothing is
//! excluded from `Cargo.toml`: the copy under `patches/` is committed, `mlua-pkg.toml`
//! names it, and that pair is what the `require` in the entry module resolves through
//! inside `target/package/<crate>/`, where `.htl/` has never existed and cargo forbids
//! writing one (#267). Before the change this failed as the report did, with `module not
//! found: 'htlx.list'` out of the proc macro.
//!
//! It is a separate file from `scaffold_targets.rs` for the same reason
//! `htlx_consumer.rs` is: the dependency is fetched, so this is one of the two cases
//! under `just e2e` that need the network, and a machine without it gets a red `htl pkg
//! install` naming the URL rather than a skip.
//!
//! **The pin is rewritten before packaging, and that is not a weakening.** A CLI built
//! from this checkout writes `htl = { path = <checkout> }`, and cargo refuses to package
//! a path dependency that names no version — so the line becomes the version this
//! workspace is at, and the three htl crates are patched at the trees under test through
//! `--config`, off to one side of the manifest. That is what `e2e-scaffold-packaged`
//! does for the same reason, and cargo applies it to the verification build as well: the
//! copy under `target/package/` compiles against the htl in this working tree.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// The root of this workspace: this crate sits directly under it.
fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

/// The `htl` that writes and patches the project: `HTL_TEST_BIN` when it is set (the
/// packaged gate points it at an installed CLI), otherwise the one cargo just built, at
/// the path cargo reports for it. The same function `htlx_consumer.rs` uses, and it says
/// why each `CARGO_*` variable is dropped or kept.
fn htl_bin() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        if let Some(given) = std::env::var_os("HTL_TEST_BIN") {
            return PathBuf::from(given);
        }
        let mut cargo =
            Command::new(std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")));
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy();
            let keep = name == "CARGO_HOME" || name == "CARGO_TARGET_DIR";
            if name.starts_with("CARGO_") && !keep {
                cargo.env_remove(&key);
            }
        }
        let out = cargo
            .args([
                "build",
                "-p",
                "htl-cli",
                "--bin",
                "htl",
                "--message-format=json",
            ])
            .current_dir(workspace_root())
            .stderr(Stdio::inherit())
            .output()
            .expect("cargo build -p htl-cli could not be started");
        assert!(
            out.status.success(),
            "cargo build -p htl-cli: {}",
            out.status
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        stdout
            .lines()
            .filter_map(|line| {
                let msg: serde_json::Value = serde_json::from_str(line).ok()?;
                if msg["reason"] != "compiler-artifact" || msg["target"]["name"] != "htl" {
                    return None;
                }
                Some(PathBuf::from(msg["executable"].as_str()?))
            })
            .next_back()
            .expect("cargo built htl-cli but reported no executable for the htl bin target")
    })
}

/// A fresh directory under the system temp dir, outside the checkout so the CLI runs
/// against the project's configuration and not this repository's.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("htl-e2e-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    println!("scaffolding into {}", dir.display());
    dir
}

/// `htl <args>` in `cwd`, both streams captured and printed, failing the test naming the
/// command if it did not succeed.
fn htl(args: &[&str], cwd: &Path) -> String {
    let out = Command::new(htl_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|e| panic!("htl {args:?}: could not be started: {e}"));
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    println!("--- htl {} ---\n{text}", args.join(" "));
    assert!(out.status.success(), "htl {args:?}: {}\n{text}", out.status);
    text
}

/// git with an identity of its own: the scratch repository is not the person's, and a
/// machine with no `user.email` configured must still be able to run this. A repository
/// is what makes `cargo package` list the tracked files — and so what keeps the
/// gitignored `.htl/` out of the tarball, which is the situation under test.
fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
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
}

/// Where the three htl crates are for the project being packaged: the extracted tarballs
/// when the packaged gate named them, this checkout otherwise.
fn patch_args() -> Vec<String> {
    let mut args = Vec::new();
    for (key, var, dir) in [
        ("htl", "HTL_PATCH_HTL", "crates/htl"),
        ("htl-core", "HTL_PATCH_CORE", "crates/htl-core"),
        ("htl-macros", "HTL_PATCH_MACROS", "crates/htl-macros"),
    ] {
        let path = std::env::var_os(var)
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace_root().join(dir));
        args.push("--config".to_string());
        args.push(format!("patch.crates-io.{key}.path='{}'", path.display()));
    }
    args
}

/// A cargo invocation in the scratch project, with the `CARGO_*` environment of the test
/// run stripped so the project's own configuration is what cargo reads. `CARGO_HOME` and
/// `CARGO_TARGET_DIR` stay: the first says where the registry is, the second is honoured
/// by the caller, which passes `--target-dir` after the subcommand (it is not a global
/// flag).
fn cargo_in(project: &Path) -> Command {
    let mut cargo =
        Command::new(std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")));
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        let keep = name == "CARGO_HOME" || name == "CARGO_TARGET_DIR";
        if name.starts_with("CARGO_") && !keep {
            cargo.env_remove(&key);
        }
    }
    cargo.current_dir(project);
    cargo
}

/// The entry module, requiring the dependency the way the reporting project did: a
/// `require` the proc macro has to resolve while cargo verifies the tarball.
const ENTRY: &str = r#"local list = require("htlx.list")

local record embedpub
   record Greeting
      who: string
      text: string
   end
end

function embedpub.greet(who: string): embedpub.Greeting
   return { who = who, text = "hello, " .. who }
end

function embedpub.doubled(ns: {integer}): {integer}
   return list.map(ns, function(n: integer): integer return n * 2 end)
end

return embedpub
"#;

#[test]
fn a_crate_with_a_patched_dependency_packages_and_verifies_with_nothing_written_by_hand() {
    let root = scratch("embed-publish");
    htl(&["new", "embedpub", "--embed"], &root);
    let project = root.join("embedpub");

    // The scaffold's own htlx pin, fetched and then taken into the tree. `htl pkg patch`
    // is the only step a publisher adds, and `git add patches/` is the rest of it.
    htl(&["pkg", "install"], &project);
    let patched = htl(&["pkg", "patch", "htlx"], &project);
    assert!(patched.contains("patched patches/htlx"), "{patched}");
    fs::write(project.join("src/embedpub/init.tl"), ENTRY).unwrap();
    htl(&["check", "."], &project);

    // A clone has no `.htl/` — `htl init` gitignores it — and the check is the same
    // check there.
    fs::remove_dir_all(project.join(".htl")).unwrap();
    let checked = htl(&["check", "."], &project);
    assert!(
        checked.contains("0 error(s)"),
        "the copy resolves the dependency with nothing installed:\n{checked}"
    );

    // Cargo will not package a path dependency that names no version; the header of this
    // file says why rewriting it is the same build all the same.
    let manifest = project.join("Cargo.toml");
    let pinned = fs::read_to_string(&manifest)
        .unwrap()
        .lines()
        .map(|l| {
            if l.starts_with("htl = ") {
                format!("htl = \"{}\"", env!("CARGO_PKG_VERSION"))
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        pinned.contains(&format!("htl = \"{}\"", env!("CARGO_PKG_VERSION"))),
        "the scaffold writes a line starting `htl = `, whoever built it:\n{pinned}"
    );
    assert!(
        !pinned.contains("patch.crates-io") && !pinned.contains("exclude"),
        "and nothing in the manifest is there to make packaging work:\n{pinned}"
    );
    fs::write(&manifest, pinned).unwrap();
    assert!(
        !fs::read_to_string(project.join("htl.toml"))
            .unwrap()
            .lines()
            .any(|l| l.trim_start().starts_with("paths")),
        "nor a `[check] paths` line in htl.toml"
    );

    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target"))
        .join("e2e-scaffold");

    // The lockfile the commit carries is resolved against the crates `cargo package` is
    // about to build with — the same `--config` patch — so that packaging does not
    // rewrite it and then refuse its own rewrite as an uncommitted change. `htl check`
    // above already wrote one, against the path the scaffold pinned; that path is the
    // patch's when the crates are this checkout, and is not when the packaged gate points
    // the patch at extracted tarballs, which is where the difference first showed.
    let out = cargo_in(&project)
        .arg("generate-lockfile")
        .args(patch_args())
        .output()
        .expect("cargo generate-lockfile could not be started");
    assert!(
        out.status.success(),
        "cargo generate-lockfile: {}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );

    git(&project, &["init", "-q"]);
    git(&project, &["add", "-A"]);
    git(&project, &["commit", "-qm", "a crate that embeds Teal"]);

    let out = cargo_in(&project)
        .arg("package")
        .args(patch_args())
        .arg("--target-dir")
        .arg(&target)
        .output()
        .expect("cargo package could not be started");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    println!("--- cargo package ---\n{text}");
    assert!(
        out.status.success(),
        "cargo package: {}\n{text}",
        out.status
    );
    assert!(
        text.contains("Verifying embedpub"),
        "the tarball was built and not verified, which is the half that fails:\n{text}"
    );

    // The copy cargo built and checked is a tree htl read and did not write in. It is
    // under the target directory this passed, not the project's own.
    let copy = target.join("package/embedpub-0.1.0");
    assert!(
        copy.join("patches/htlx/src/htlx/list.tl").is_file(),
        "{text}"
    );
    assert!(
        !copy.join(".htl").exists(),
        "nothing was written into the tree cargo verifies"
    );

    fs::remove_dir_all(&root).ok();
}
