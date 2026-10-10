//! `htl pin <release|main|path:<checkout>> [dir] [--no-update]` through the real binary:
//! what each pin rewrites in a project `htl new` already wrote — `Cargo.toml`'s `htl` /
//! `htl-mq` / `htl-std` lines, `mise.toml`, `[toolchain] htl` in `htl.toml` — what a line
//! this does not recognise is refused with, and the one case that actually runs `cargo
//! update`.

use std::path::Path;
use std::process::Command;

mod common;

fn tempdir(name: &str) -> common::TempDir {
    common::tempdir("htl-cli-pin", name)
}

/// The exit code (`None` only if the process was killed by a signal), stdout, stderr.
fn htl(args: &[&str], cwd: &Path) -> (Option<i32>, String, String) {
    let out = Command::new(common::htl_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The `htl` line of a project's manifest, so that a failure prints the line rather than
/// the file.
fn htl_line(manifest: &Path) -> String {
    dep_line(manifest, "htl")
}

/// The same for any dependency of this repository's: `htl`, or the `htl-mq` a window
/// project gets under the same pin.
fn dep_line(manifest: &Path, name: &str) -> String {
    let cargo = std::fs::read_to_string(manifest).unwrap();
    cargo
        .lines()
        .find(|l| l.starts_with(&format!("{name} = ")))
        .unwrap_or_else(|| panic!("no {name} dependency in:\n{cargo}"))
        .to_string()
}

/// `name`'s line, without assuming the scaffold's own canonical spacing around `=` or
/// quoting of the key — unlike [`dep_line`]. `htl pin` keeps a rewritten value's own
/// original spacing (it replaces the value, not the key), so a line it moved from a
/// non-canonical spelling still has that spelling on the left of `=` afterwards; this is
/// for the one test that writes such spellings on purpose.
fn loose_dep_line<'a>(text: &'a str, name: &str) -> &'a str {
    text.lines()
        .find(|l| {
            let t = l.trim_start();
            let t = t.strip_prefix('"').unwrap_or(t);
            let Some(rest) = t.strip_prefix(name) else {
                return false;
            };
            let rest = rest.strip_prefix('"').unwrap_or(rest);
            rest.trim_start().starts_with('=')
        })
        .unwrap_or_else(|| panic!("no {name} dependency in:\n{text}"))
}

/// This repository's root, the way `--htl path:<checkout>` names one: canonicalised, `/`
/// on every platform, no trailing one. The same string `new_htl_path_writes_a_path_pin`
/// (`scaffold_cli.rs`) builds for the same reason.
fn checkout_path() -> String {
    std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .unwrap()
        .display()
        .to_string()
        .replace('\\', "/")
}

/// `htl pin path:<checkout> --no-update` rewrites the `htl` line from the release form
/// `htl new --htl release` writes to the exact path form `htl new --htl
/// path:<checkout>` would, removes `mise.toml` (a checkout is not something mise
/// installs), and leaves the rest of the file exactly as it was. Started from `--htl
/// release` rather than the default: this binary is itself built from this checkout, so
/// the default pin already *is* `path:<checkout>` and the move would be a no-op.
#[test]
fn pin_path_rewrites_a_release_or_checkout_pin_and_removes_mise_toml() {
    let root = tempdir("path");
    let (code, _, stderr) = htl(&["new", "a", "--target", "bin", "--htl", "release"], &root);
    assert_eq!(code, Some(0), "{stderr}");
    let dir = root.join("a");
    let manifest = dir.join("Cargo.toml");
    assert!(
        dir.join("mise.toml").is_file(),
        "a release pin writes mise.toml"
    );
    let before = std::fs::read_to_string(&manifest).unwrap();
    assert_eq!(
        htl_line(&manifest),
        format!("htl = \"{}\"", env!("CARGO_PKG_VERSION"))
    );

    let checkout = checkout_path();
    let (code, _, stderr) = htl(&["pin", &format!("path:{checkout}"), "--no-update"], &dir);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        htl_line(&manifest),
        format!("htl = {{ path = \"{checkout}/crates/htl\" }}")
    );
    assert!(!dir.join("mise.toml").exists(), "{stderr}");

    let after = std::fs::read_to_string(&manifest).unwrap();
    // `htl-std` moves with `htl` too (a `--target bin` sibling, like `htl-mq` under
    // `window`), so its line is excluded from "the rest" alongside `htl`'s own.
    let keep = |l: &&str| !l.starts_with("htl = ") && !l.starts_with("htl-std = ");
    assert_eq!(
        before.lines().filter(keep).collect::<Vec<_>>(),
        after.lines().filter(keep).collect::<Vec<_>>(),
        "before:\n{before}\nafter:\n{after}"
    );
}

/// The way back: `htl pin release --no-update` on a project moved off it writes this
/// CLI's own version and `mise.toml` with it — the file a `main` / `path:` pin has none
/// of.
#[test]
fn pin_release_writes_the_clis_version_and_mise_toml_back() {
    let root = tempdir("release");
    let (code, _, stderr) = htl(&["new", "a", "--target", "bin", "--htl", "main"], &root);
    assert_eq!(code, Some(0), "{stderr}");
    let dir = root.join("a");
    assert!(!dir.join("mise.toml").exists(), "{stderr}");

    let (code, _, stderr) = htl(&["pin", "release", "--no-update"], &dir);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        htl_line(&dir.join("Cargo.toml")),
        format!("htl = \"{}\"", env!("CARGO_PKG_VERSION"))
    );
    let mise = std::fs::read_to_string(dir.join("mise.toml")).unwrap();
    assert!(
        mise.ends_with(&format!(
            "\"cargo:htl-cli\" = \"{}\"\n",
            env!("CARGO_PKG_VERSION")
        )),
        "{mise}"
    );
}

/// `htl pin main` writes the same git pin `htl new --htl main` does: no version left for
/// cargo to reconcile with the branch.
#[test]
fn pin_main_writes_a_git_pin() {
    let root = tempdir("main");
    let (code, _, stderr) = htl(&["new", "a", "--target", "bin"], &root);
    assert_eq!(code, Some(0), "{stderr}");
    let dir = root.join("a");

    let (code, _, stderr) = htl(&["pin", "main", "--no-update"], &dir);
    assert_eq!(code, Some(0), "{stderr}");
    let line = htl_line(&dir.join("Cargo.toml"));
    assert!(line.contains("git = "), "{line}");
    assert!(line.contains("branch = \"main\""), "{line}");
    assert!(!line.contains("version ="), "{line}");
}

/// A window project's `htl-mq` line moves with `htl`'s — they are two crates of one tree,
/// and `htl pin` names one tree for both the way `--htl` does at `new` time. Started
/// from `--htl release` for the same reason as the path test above: the default already
/// matches the target checkout in this environment.
#[test]
fn the_window_targets_htl_mq_line_moves_with_the_htl_line() {
    let root = tempdir("window");
    let (code, _, stderr) = htl(
        &["new", "w", "--target", "window", "--htl", "release"],
        &root,
    );
    assert_eq!(code, Some(0), "{stderr}");
    let dir = root.join("w");
    let manifest = dir.join("Cargo.toml");
    assert_eq!(
        htl_line(&manifest),
        format!("htl = \"{}\"", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(
        dep_line(&manifest, "htl-mq"),
        format!("htl-mq = \"{}\"", env!("CARGO_PKG_VERSION"))
    );
    let checkout = checkout_path();

    let (code, _, stderr) = htl(&["pin", &format!("path:{checkout}"), "--no-update"], &dir);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        htl_line(&manifest),
        format!("htl = {{ path = \"{checkout}/crates/htl\" }}")
    );
    assert_eq!(
        dep_line(&manifest, "htl-mq"),
        format!("htl-mq = {{ path = \"{checkout}/crates/htl-mq\" }}")
    );
}

/// A `--target bin` project's `htl-std` line moves with `htl`'s the same way `htl-mq`'s
/// does under the window target (the test above): `htl pin release` leaves the bare
/// version, `htl pin path:<checkout>` leaves the checkout's `crates/htl-std`. This is the
/// case the `SIBLINGS` loop in `pin.rs` fixes — before it, `htl-std` was left wherever
/// `htl new` wrote it while `htl` moved beneath it, and `cargo update` then failed on a
/// stale `htl-std` requirement.
#[test]
fn the_bin_targets_htl_std_line_moves_with_the_htl_line() {
    let root = tempdir("bin-std");
    let (code, _, stderr) = htl(&["new", "s", "--target", "bin", "--htl", "main"], &root);
    assert_eq!(code, Some(0), "{stderr}");
    let dir = root.join("s");
    let manifest = dir.join("Cargo.toml");
    assert!(
        htl_line(&manifest).contains("branch = \"main\""),
        "{}",
        htl_line(&manifest)
    );
    assert!(
        dep_line(&manifest, "htl-std").contains("branch = \"main\""),
        "{}",
        dep_line(&manifest, "htl-std")
    );

    let (code, _, stderr) = htl(&["pin", "release", "--no-update"], &dir);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        htl_line(&manifest),
        format!("htl = \"{}\"", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(
        dep_line(&manifest, "htl-std"),
        format!("htl-std = \"{}\"", env!("CARGO_PKG_VERSION"))
    );

    let checkout = checkout_path();
    let (code, _, stderr) = htl(&["pin", &format!("path:{checkout}"), "--no-update"], &dir);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        htl_line(&manifest),
        format!("htl = {{ path = \"{checkout}/crates/htl\" }}")
    );
    assert_eq!(
        dep_line(&manifest, "htl-std"),
        format!("htl-std = {{ path = \"{checkout}/crates/htl-std\" }}")
    );
}

/// The `cdylib` target's `features = ["ffi"]` survives every pin kind, the same guarantee
/// `the_cdylib_pin_keeps_its_features_under_every_pin_kind` holds at `new` time — here
/// read back out of the manifest and carried into each rewrite rather than supplied by a
/// target profile.
#[test]
fn the_cdylib_feature_list_survives_every_pin_kind() {
    let root = tempdir("cdylib");
    let (code, _, stderr) = htl(&["new", "c", "--lib", "--target", "cdylib"], &root);
    assert_eq!(code, Some(0), "{stderr}");
    let dir = root.join("c");
    let manifest = dir.join("Cargo.toml");
    assert!(
        htl_line(&manifest).contains("features = [\"ffi\"]"),
        "{}",
        htl_line(&manifest)
    );

    for pin in ["main", "release"] {
        let (code, _, stderr) = htl(&["pin", pin, "--no-update"], &dir);
        assert_eq!(code, Some(0), "{pin}: {stderr}");
        assert!(
            htl_line(&manifest).contains("features = [\"ffi\"]"),
            "{pin}: {}",
            htl_line(&manifest)
        );
    }
    let checkout = checkout_path();
    let (code, _, stderr) = htl(&["pin", &format!("path:{checkout}"), "--no-update"], &dir);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        htl_line(&manifest).contains("features = [\"ffi\"]"),
        "{}",
        htl_line(&manifest)
    );
}

/// `[toolchain] htl` in `htl.toml`, when the project has one, moves with the pin: set to
/// the release's number under `release`, removed (table and all, since it is the only key
/// in it) under a tree with no release to require. A project with no such key is left
/// byte-identical either way.
#[test]
fn a_toolchain_key_is_rewritten_when_present_and_left_absent_when_not() {
    let root = tempdir("toolchain");
    let (code, _, stderr) = htl(&["new", "a", "--target", "bin"], &root);
    assert_eq!(code, Some(0), "{stderr}");
    let dir = root.join("a");
    let htl_toml = dir.join("htl.toml");
    let config = std::fs::read_to_string(&htl_toml).unwrap();
    std::fs::write(&htl_toml, format!("{config}\n[toolchain]\nhtl = \"0.8\"\n")).unwrap();

    let (code, _, stderr) = htl(&["pin", "release", "--no-update"], &dir);
    assert_eq!(code, Some(0), "{stderr}");
    let after = std::fs::read_to_string(&htl_toml).unwrap();
    assert!(
        after.contains(&format!("htl = \"{}\"", env!("CARGO_PKG_VERSION"))),
        "{after}"
    );
    assert!(!after.contains("htl = \"0.8\""), "{after}");

    let checkout = checkout_path();
    let (code, _, stderr) = htl(&["pin", &format!("path:{checkout}"), "--no-update"], &dir);
    assert_eq!(code, Some(0), "{stderr}");
    let after = std::fs::read_to_string(&htl_toml).unwrap();
    assert!(!after.contains("[toolchain]"), "{after}");
    assert!(
        !after.contains(&format!("htl = \"{}\"", env!("CARGO_PKG_VERSION"))),
        "{after}"
    );

    // A project without the key: byte-identical after either pin.
    let (code, _, stderr) = htl(&["new", "b", "--target", "bin"], &root);
    assert_eq!(code, Some(0), "{stderr}");
    let dir_b = root.join("b");
    let htl_toml_b = dir_b.join("htl.toml");
    let before_b = std::fs::read_to_string(&htl_toml_b).unwrap();

    let (code, _, stderr) = htl(&["pin", "main", "--no-update"], &dir_b);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(std::fs::read_to_string(&htl_toml_b).unwrap(), before_b);

    let (code, _, stderr) = htl(&["pin", "release", "--no-update"], &dir_b);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(std::fs::read_to_string(&htl_toml_b).unwrap(), before_b);
}

/// `[toolchain.htl]` written as a sub-table — rather than `[toolchain] htl = "..."`,
/// the shape this reads — is refused with a message that names the file and the key,
/// not a bare "htl is not a plain value" that names neither.
#[test]
fn a_toolchain_htl_table_names_the_file_in_its_refusal() {
    let root = tempdir("toolchain-table");
    let (code, _, stderr) = htl(&["new", "a", "--target", "bin", "--htl", "main"], &root);
    assert_eq!(code, Some(0), "{stderr}");
    let dir = root.join("a");
    let htl_toml = dir.join("htl.toml");
    let config = std::fs::read_to_string(&htl_toml).unwrap();
    std::fs::write(
        &htl_toml,
        format!("{config}\n[toolchain.htl]\nfoo = \"bar\"\n"),
    )
    .unwrap();

    let (code, _, stderr) = htl(&["pin", "release", "--no-update"], &dir);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("htl.toml"), "{stderr}");
    assert!(stderr.contains("[toolchain] htl"), "{stderr}");
    assert!(stderr.contains("not a plain value"), "{stderr}");
}

/// A comment above the `htl` line and a trailing one on it both survive `pin main` —
/// `set_value_keeping_decor` carries the entry's own decor onto the replacement rather
/// than letting a fresh `Item` (or `TableLike::insert`'s key reformat) drop it. The same
/// holds for `[toolchain] htl` under `pin release`.
#[test]
fn pin_preserves_comments_around_the_rewritten_lines() {
    let root = tempdir("comments");
    let (code, _, stderr) = htl(&["new", "a", "--target", "bin", "--htl", "release"], &root);
    assert_eq!(code, Some(0), "{stderr}");
    let dir = root.join("a");

    let manifest = dir.join("Cargo.toml");
    let cargo = std::fs::read_to_string(&manifest).unwrap();
    let line = cargo
        .lines()
        .find(|l| l.starts_with("htl = "))
        .unwrap()
        .to_string();
    let decorated = cargo.replacen(&line, &format!("# the engine\n{line} # trailing"), 1);
    std::fs::write(&manifest, &decorated).unwrap();

    let (code, _, stderr) = htl(&["pin", "main", "--no-update"], &dir);
    assert_eq!(code, Some(0), "{stderr}");

    let after_cargo = std::fs::read_to_string(&manifest).unwrap();
    assert!(after_cargo.contains("# the engine\n"), "{after_cargo}");
    let new_line = after_cargo
        .lines()
        .find(|l| l.starts_with("htl = "))
        .unwrap();
    assert!(new_line.ends_with("# trailing"), "{new_line}");
    assert!(new_line.contains("git = "), "{new_line}");

    // `htl.toml`: inject `[toolchain] htl` *after* the `pin main` above, with its own
    // comments — this pin (release) *sets* the key rather than removing it (the way
    // `pin main` / `pin path:` would, which is why the key is injected only now: a
    // comment attached to a key that pin is about to delete has nothing left to attach
    // to once it is gone, and is not expected to survive that case).
    let htl_toml = dir.join("htl.toml");
    let config = std::fs::read_to_string(&htl_toml).unwrap();
    std::fs::write(
        &htl_toml,
        format!("{config}\n[toolchain]\n# pinned deliberately\nhtl = \"0.8\" # see issue\n"),
    )
    .unwrap();

    let (code, _, stderr) = htl(&["pin", "release", "--no-update"], &dir);
    assert_eq!(code, Some(0), "{stderr}");
    let after_toml = std::fs::read_to_string(&htl_toml).unwrap();
    assert!(
        after_toml.contains("# pinned deliberately\n"),
        "{after_toml}"
    );
    let toolchain_line = after_toml
        .lines()
        .find(|l| l.trim_start().starts_with("htl = "))
        .unwrap();
    assert!(
        toolchain_line.contains(env!("CARGO_PKG_VERSION")),
        "{toolchain_line}"
    );
    assert!(toolchain_line.ends_with("# see issue"), "{toolchain_line}");
}

/// A relative `path:` is resolved against the current directory the way the shell reads
/// it — not against the project's own directory, which `dir` can point anywhere away
/// from — so the manifest ends up with an absolute, correct path rather than a relative
/// one that only happens to work when `dir` and the shell's cwd coincide.
#[test]
fn a_relative_path_is_resolved_against_the_current_directory() {
    let root = tempdir("relative");
    let sub = root.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    // A stub checkout: `htl pin` only checks that `crates/htl/Cargo.toml` is there.
    let checkout = sub.join("checkout");
    std::fs::create_dir_all(checkout.join("crates/htl")).unwrap();
    std::fs::write(checkout.join("crates/htl/Cargo.toml"), "[package]\n").unwrap();

    let (code, _, stderr) = htl(&["new", "demo", "--target", "bin"], &sub);
    assert_eq!(code, Some(0), "{stderr}");

    // Run from `sub` (not from `sub/demo`), naming the project with `dir` and the
    // checkout with a path relative to `sub`.
    let (code, _, stderr) = htl(&["pin", "path:checkout", "demo", "--no-update"], &sub);
    assert_eq!(code, Some(0), "{stderr}");

    let expected = std::fs::canonicalize(&checkout)
        .unwrap()
        .display()
        .to_string();
    assert_eq!(
        htl_line(&sub.join("demo/Cargo.toml")),
        format!("htl = {{ path = \"{expected}/crates/htl\" }}")
    );
}

/// A `path:` that does not look like an htl checkout — missing entirely, or there but
/// with no `crates/htl/Cargo.toml` under it — is refused before anything is touched,
/// rather than failing later inside `cargo update` with a message about a lockfile.
#[test]
fn a_path_with_no_checkout_under_it_is_refused() {
    let root = tempdir("no-checkout");
    let (code, _, stderr) = htl(&["new", "a", "--target", "bin"], &root);
    assert_eq!(code, Some(0), "{stderr}");
    let dir = root.join("a");
    let manifest = dir.join("Cargo.toml");
    let before = std::fs::read_to_string(&manifest).unwrap();

    let (code, _, stderr) = htl(&["pin", "path:does-not-exist-xyz", "--no-update"], &dir);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("is not an htl checkout"), "{stderr}");
    assert!(stderr.contains("no crates/htl/Cargo.toml"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&manifest).unwrap(), before);

    // A directory that exists but is not a checkout either.
    std::fs::create_dir_all(root.join("empty-dir")).unwrap();
    let (code, _, stderr) = htl(&["pin", "path:../empty-dir", "--no-update"], &dir);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("is not an htl checkout"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&manifest).unwrap(), before);
}

/// A number is refused with the scaffold's own message — this is [`HtlPin::parse`]
/// shared with `htl new` — and nothing is written.
#[test]
fn a_number_is_refused_with_the_scaffolds_message() {
    let root = tempdir("number");
    let (code, _, stderr) = htl(&["new", "a", "--target", "bin"], &root);
    assert_eq!(code, Some(0), "{stderr}");
    let dir = root.join("a");
    let manifest = dir.join("Cargo.toml");
    let before = std::fs::read_to_string(&manifest).unwrap();

    let (code, _, stderr) = htl(&["pin", "0.7.0", "--no-update"], &dir);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("unsupported htl `0.7.0`"), "{stderr}");
    assert!(
        stderr.contains("cargo install htl-cli --version 0.7.0"),
        "{stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(&manifest).unwrap(),
        before,
        "{stderr}"
    );
}

/// A `Cargo.toml` whose `htl` line is not one the scaffold wrote — an extra key inline, or
/// the line spelled as a `[dependencies.htl]` table — is refused with the
/// `[patch.crates-io]` block rather than guessed at, and nothing changes: not the
/// manifest, not `mise.toml`.
#[test]
fn an_unrecognised_htl_line_is_refused_with_the_patch_block() {
    let root = tempdir("unrecognised");
    let (code, _, stderr) = htl(&["new", "a", "--target", "bin"], &root);
    assert_eq!(code, Some(0), "{stderr}");
    let dir = root.join("a");
    let manifest = dir.join("Cargo.toml");
    let mise = dir.join("mise.toml");
    let original = std::fs::read_to_string(&manifest).unwrap();
    let original_line = htl_line(&manifest);

    // An extra key inline (`optional`) this command's three recognised shapes do not have.
    let rewritten = original.replacen(
        &original_line,
        "htl = { version = \"0.13.0\", optional = true }",
        1,
    );
    std::fs::write(&manifest, &rewritten).unwrap();
    let mise_before = std::fs::read_to_string(&mise).ok();

    let (code, _, stderr) = htl(&["pin", "release", "--no-update"], &dir);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("[patch.crates-io]"), "{stderr}");
    assert!(stderr.contains("htl-macros = { path ="), "{stderr}");
    assert_eq!(
        std::fs::read_to_string(&manifest).unwrap(),
        rewritten,
        "{stderr}"
    );
    assert_eq!(std::fs::read_to_string(&mise).ok(), mise_before, "{stderr}");

    // The other unrecognised shape: a `[dependencies.htl]` table rather than a line.
    let table =
        original.replacen(&original_line, "", 1) + "\n[dependencies.htl]\nversion = \"0.13.0\"\n";
    std::fs::write(&manifest, &table).unwrap();

    let (code, _, stderr) = htl(&["pin", "release", "--no-update"], &dir);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("[patch.crates-io]"), "{stderr}");
    assert!(stderr.contains("htl-macros = { path ="), "{stderr}");
    assert_eq!(
        std::fs::read_to_string(&manifest).unwrap(),
        table,
        "{stderr}"
    );
}

/// A `Cargo.toml` with no `htl` dependency at all — a crate `htl new` never wrote, not a
/// shape this does not recognise — gets its own message rather than the
/// `[patch.crates-io]` block, which would do nothing for a crate that was never pinned to
/// htl in the first place.
#[test]
fn no_htl_dependency_is_refused_without_the_patch_block() {
    let root = tempdir("no-htl");
    let dir = root.join("plain");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"plain\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [dependencies]\nanyhow = \"1\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("src/lib.rs"), "").unwrap();
    let before = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();

    let (code, _, stderr) = htl(&["pin", "release", "--no-update"], &dir);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        stderr.contains("Cargo.toml has no htl dependency"),
        "{stderr}"
    );
    assert!(stderr.contains("nothing to move"), "{stderr}");
    assert!(!stderr.contains("[patch.crates-io]"), "{stderr}");
    assert_eq!(
        std::fs::read_to_string(dir.join("Cargo.toml")).unwrap(),
        before
    );
}

/// An `htl = ...` line that exists only in a different table — `[dev-dependencies]`, or
/// a `[patch.crates-io]` block above `[dependencies]` — must not be confused with the
/// real one: reading the old value off the parsed `[dependencies]` item (rather than
/// searching the raw text for the first line that starts with `htl = `) is what this
/// pins down. Deliberately still passes an unrelated `htl` line through untouched.
#[test]
fn an_htl_line_outside_dependencies_does_not_confuse_the_real_one() {
    let root = tempdir("other-table");
    let (code, _, stderr) = htl(&["new", "a", "--target", "bin", "--htl", "main"], &root);
    assert_eq!(code, Some(0), "{stderr}");
    let dir = root.join("a");
    let manifest = dir.join("Cargo.toml");
    let before = std::fs::read_to_string(&manifest).unwrap();

    // A `[patch.crates-io]` block *above* `[dependencies]`, naming the pin this command
    // is about to move the real dependency *to* — the exact case that would be taken as
    // the old value (and read as "already done, nothing to do") by a search that just
    // finds the first line in the file starting with `htl = `.
    let checkout = checkout_path();
    let patch_block =
        format!("[patch.crates-io]\nhtl = {{ path = \"{checkout}/crates/htl\" }}\n\n");
    let with_patch = before.replacen("[dependencies]", &format!("{patch_block}[dependencies]"), 1);
    assert_ne!(
        with_patch, before,
        "no [dependencies] header to insert the patch block before"
    );
    std::fs::write(&manifest, &with_patch).unwrap();

    let (code, _, stderr) = htl(&["pin", &format!("path:{checkout}"), "--no-update"], &dir);
    assert_eq!(code, Some(0), "{stderr}");
    let after = std::fs::read_to_string(&manifest).unwrap();

    // The git pin is gone — the real `[dependencies]` entry moved — and the path form
    // now appears twice: the `[patch.crates-io]` line (untouched) and the rewritten
    // `[dependencies]` line. A version confused by the line above `[dependencies]`
    // would have left the git pin in place (reading the patch line as already-correct)
    // and the path form only once.
    assert!(!after.contains("branch = \"main\""), "{after}");
    let path_form = format!("htl = {{ path = \"{checkout}/crates/htl\" }}");
    assert_eq!(after.matches(&path_form).count(), 2, "{after}");
    assert!(
        after.contains(&format!("[patch.crates-io]\n{path_form}")),
        "{after}"
    );

    // And the summary reported the real move (git -> path), not a no-op.
    assert!(stderr.contains("git = "), "{stderr}");
    assert!(
        stderr.contains(&format!("-> {{ path = \"{checkout}/crates/htl\" }}")),
        "{stderr}"
    );
}

/// Several valid, differently-spaced spellings of the same line all move correctly,
/// where searching the raw text for the literal substring `htl = ` would panic on the
/// first two and silently miss the third.
#[test]
fn differently_spaced_htl_lines_all_move() {
    for prefix in ["htl=\"", "htl   = \"", "\"htl\" = \""] {
        let root = tempdir("spacing");
        let (code, _, stderr) = htl(&["new", "a", "--target", "bin", "--htl", "release"], &root);
        assert_eq!(code, Some(0), "{prefix}: {stderr}");
        let dir = root.join("a");
        let manifest = dir.join("Cargo.toml");
        let original = std::fs::read_to_string(&manifest).unwrap();
        let version = env!("CARGO_PKG_VERSION");
        let canonical = format!("htl = \"{version}\"");
        let respaced = format!("{prefix}{version}\"");
        let rewritten = original.replacen(&canonical, &respaced, 1);
        assert_ne!(
            rewritten, original,
            "{prefix}: the canonical line was not found to replace"
        );
        std::fs::write(&manifest, &rewritten).unwrap();

        let (code, _, stderr) = htl(&["pin", "main", "--no-update"], &dir);
        assert_eq!(code, Some(0), "{prefix}: {stderr}");
        let after = std::fs::read_to_string(&manifest).unwrap();
        let line = loose_dep_line(&after, "htl");
        assert!(line.contains("git = "), "{prefix}: {line}");
        assert!(line.contains("branch = \"main\""), "{prefix}: {line}");
    }
}

/// The per-file summary is printed before `cargo update` is spawned, so a failure there
/// still has the lines it refers to ("the files above") above it rather than printed
/// after a line that claims they already are.
#[test]
fn the_summary_is_printed_before_cargo_update_runs() {
    let root = tempdir("cargo-fail");
    let (code, _, stderr) = htl(&["new", "a", "--target", "bin"], &root);
    assert_eq!(code, Some(0), "{stderr}");
    let dir = root.join("a");
    let checkout = checkout_path();

    let out = Command::new(common::htl_bin())
        .args(["pin", &format!("path:{checkout}")])
        .current_dir(&dir)
        .env("CARGO", "/bin/false")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(2), "{stderr}");

    let cargo_line = stderr
        .find("htl pin: Cargo.toml")
        .unwrap_or_else(|| panic!("{stderr}"));
    let failed_line = stderr
        .find("cargo update failed")
        .unwrap_or_else(|| panic!("{stderr}"));
    assert!(cargo_line < failed_line, "{stderr}");
}

/// An unparsable `htl.toml` is caught — and the whole command fails — before `Cargo.toml`
/// or `mise.toml` are written: every file is read and parsed before anything is written,
/// so a project is never left rewritten halfway when a later file turns out unreadable.
#[test]
fn an_unparsable_htl_toml_leaves_everything_else_untouched() {
    let root = tempdir("atomic");
    let (code, _, stderr) = htl(&["new", "a", "--target", "bin", "--htl", "main"], &root);
    assert_eq!(code, Some(0), "{stderr}");
    let dir = root.join("a");
    let manifest = dir.join("Cargo.toml");
    let mise = dir.join("mise.toml");
    let htl_toml = dir.join("htl.toml");
    let cargo_before = std::fs::read_to_string(&manifest).unwrap();
    assert!(!mise.exists());

    std::fs::write(&htl_toml, "this is not [ valid toml").unwrap();

    let (code, _, stderr) = htl(&["pin", "release", "--no-update"], &dir);
    assert_ne!(code, Some(0), "{stderr}");
    assert_eq!(std::fs::read_to_string(&manifest).unwrap(), cargo_before);
    assert!(
        !mise.exists(),
        "mise.toml was written despite htl.toml failing to parse"
    );
}

/// `htl.toml` that exists but cannot even be read (`chmod 000`) must not be treated as
/// "there is no such file" — only [`std::io::ErrorKind::NotFound`] is that. The whole
/// command fails, naming the file, before anything is written.
///
/// Skipped — like `cargo_update_runs_and_the_lock_names_the_checkout` skips on a sandbox
/// with no registry — when the permission bit turns out not to block anything: running
/// as root, or a filesystem/container that reads straight through the mode bits, both
/// happen in CI-like environments this cannot control.
#[test]
fn an_unreadable_htl_toml_fails_before_anything_is_written() {
    #[cfg(not(unix))]
    {
        println!("not unix: skipping the permission-based refusal test");
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let root = tempdir("unreadable");
        let (code, _, stderr) = htl(&["new", "a", "--target", "bin", "--htl", "main"], &root);
        assert_eq!(code, Some(0), "{stderr}");
        let dir = root.join("a");
        let manifest = dir.join("Cargo.toml");
        let cargo_before = std::fs::read_to_string(&manifest).unwrap();
        let htl_toml = dir.join("htl.toml");
        std::fs::write(&htl_toml, "[toolchain]\nhtl = \"0.12.0\"\n").unwrap();

        std::fs::set_permissions(&htl_toml, std::fs::Permissions::from_mode(0o000)).unwrap();
        let mode = std::fs::metadata(&htl_toml).unwrap().permissions().mode() & 0o777;
        let still_readable = mode == 0 && std::fs::read_to_string(&htl_toml).is_ok();
        if mode != 0 || still_readable {
            println!("chmod 000 did not block reading (mode {mode:o}): skipping");
            std::fs::set_permissions(&htl_toml, std::fs::Permissions::from_mode(0o644)).unwrap();
            return;
        }

        let (code, _, stderr) = htl(&["pin", "release", "--no-update"], &dir);
        std::fs::set_permissions(&htl_toml, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert_eq!(code, Some(2), "{stderr}");
        assert!(stderr.contains("htl.toml"), "{stderr}");
        assert_eq!(std::fs::read_to_string(&manifest).unwrap(), cargo_before);
    }
}

/// `mise.toml` existing as a directory (or anything else that is not a plain file) is
/// refused before anything is written, including `Cargo.toml` — which a release pin
/// would otherwise move first, since the manifest is written before `mise.toml` is.
#[test]
fn mise_toml_as_a_directory_is_refused_before_any_write() {
    let root = tempdir("mise-dir");
    let (code, _, stderr) = htl(&["new", "a", "--target", "bin", "--htl", "main"], &root);
    assert_eq!(code, Some(0), "{stderr}");
    let dir = root.join("a");
    let manifest = dir.join("Cargo.toml");
    let before = std::fs::read_to_string(&manifest).unwrap();
    std::fs::create_dir(dir.join("mise.toml")).unwrap();

    let (code, _, stderr) = htl(&["pin", "release", "--no-update"], &dir);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("mise.toml"), "{stderr}");
    assert!(stderr.contains("not a regular file"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&manifest).unwrap(), before);
    assert!(dir.join("mise.toml").is_dir());
}

/// The one case that actually runs `cargo update`, so it is the slow one — and, unlike
/// every other case here, it needs the registry to resolve the rest of the graph against,
/// which a sandbox may not have. Skipped rather than failed in that case, the way the e2e
/// window case is without `xvfb-run`. Started from `--htl release` for the same reason as
/// the other checkout-pinning tests above: the default already matches the checkout here.
#[test]
fn cargo_update_runs_and_the_lock_names_the_checkout() {
    let root = tempdir("cargo-update");
    let (code, _, stderr) = htl(&["new", "a", "--target", "bin", "--htl", "release"], &root);
    assert_eq!(code, Some(0), "{stderr}");
    let dir = root.join("a");
    let checkout = checkout_path();

    let (code, _, stderr) = htl(&["pin", &format!("path:{checkout}")], &dir);
    if code != Some(0) {
        let offline = [
            "failed to get",
            "could not resolve",
            "unable to get packages",
        ]
        .iter()
        .any(|s| stderr.contains(s));
        if offline {
            println!("no registry access: skipping the cargo update run\n{stderr}");
            return;
        }
        panic!("{stderr}");
    }

    let lock = std::fs::read_to_string(dir.join("Cargo.lock")).unwrap();
    let htl_pkg = lock
        .split("[[package]]")
        .find(|p| p.contains("name = \"htl\"\n"))
        .unwrap_or_else(|| panic!("no htl package in:\n{lock}"));
    // A path dependency's lock entry has no `source = "registry+…"` line; this is what
    // tells the two apart short of reading cargo's resolver output.
    assert!(!htl_pkg.contains("source = \"registry+"), "{htl_pkg}");
}

/// No `Cargo.toml` above the directory asked for is refused, naming it — like every other
/// command that walks up looking for a project root.
#[test]
fn no_cargo_toml_is_refused() {
    let root = tempdir("empty");
    let (code, _, stderr) = htl(&["pin", "release", root.to_str().unwrap()], &root);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains(&root.display().to_string()), "{stderr}");
}
