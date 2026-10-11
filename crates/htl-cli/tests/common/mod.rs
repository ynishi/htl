//! What every integration test in this crate needs: a temp directory, and the binary
//! to run in it.
//!
//! This module is compiled into each test binary that declares `mod common;`, and anything
//! it defines that a given binary does not use is a warning CI turns into an error. So it
//! holds only what all of them want. The items here qualify: every file that declares the
//! module calls `tempdir` and spawns `htl_bin`. A helper wanted by two files stays in those
//! two files — `fmt_snapshot.rs` keeps its own copy of the snapshot block for that reason.
//!
//! [`write_patch_config`] is the one exception, and it carries `#[allow(dead_code)]` to be
//! one. It is not wanted by every file but by every file that scaffolds a project and runs a
//! cargo in it — today one — and it has to agree with the e2e copy and with the recipe that
//! sets its variables. A copy per such file is a copy that forgets a crate when the list of
//! them grows, and the failure it would hide only shows on a release PR.
//!
//! `tempdir`'s directory guard, [`TempDir`], is implemented once, in
//! `crates/htl-core/tests/common/mod.rs`, and brought in here by `#[path]`: the workspace
//! shares test helpers by linking the one file rather than copying it. A `#[path]` `mod`
//! item names any file on disk, crate boundary or not, so `core_common` below reaches
//! `htl-core`'s file even though this crate's test binaries are compiled separately from
//! `htl-core`'s. See that file's doc for what `TempDir` promises, in particular for a test
//! that panics.

use std::path::{Path, PathBuf};

#[allow(dead_code)]
#[path = "../../../htl-core/tests/common/mod.rs"]
mod core_common;

pub use core_common::TempDir;

/// A fresh directory under the system temp dir, named for the test that asked. `prefix`
/// separates one test binary from another (they are separate processes, so the pid does
/// too, but the name is what a leftover directory is read by). See `core_common::tempdir`
/// — the one place this is implemented — for what the returned [`TempDir`] promises.
pub fn tempdir(prefix: &str, name: &str) -> TempDir {
    core_common::tempdir(prefix, name)
}

/// The `htl` binary these tests drive: `HTL_TEST_BIN` when it is set, otherwise the one
/// cargo built for this test binary.
///
/// The default is the only binary a plain `cargo test` has any reason to run, and it is
/// exactly what `env!("CARGO_BIN_EXE_htl")` gave every call site before this existed. The
/// override is for a gate that has an `htl` from somewhere else — installed out of a
/// packaged tarball, say, where the file set is not the checkout's — and wants this whole
/// suite pointed at it rather than a few cases restated in shell.
///
/// It is a foot-gun by construction: export it, forget it, and the suite reports on a
/// binary that stopped matching the source. Set it for one command, not for a shell.
pub fn htl_bin() -> PathBuf {
    std::env::var_os("HTL_TEST_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_htl")))
}

/// The crates a caller can redirect, and the variable that redirects each: the same five
/// pairs, in the same order, as `PATCHES` in `e2e/tests/scaffold_targets.rs`, which is
/// where the table of what each variable means when unset lives.
#[allow(dead_code)]
const PATCHES: [(&str, &str); 5] = [
    ("htl", "HTL_PATCH_HTL"),
    ("htl-core", "HTL_PATCH_CORE"),
    ("htl-macros", "HTL_PATCH_MACROS"),
    ("htl-mq", "HTL_PATCH_MQ"),
    ("htl-std", "HTL_PATCH_STD"),
];

/// The patch a caller asked for, written into a scaffolded project as `.cargo/config.toml`
/// — and so seen by *every* cargo that runs there, including the ones `htl` spawns itself.
/// Call it right after `htl new`, before the first `htl check`, `htl test` or `htl run`.
///
/// `htl check` materialises a dependency's declaration and asks `cargo metadata` where
/// that dependency lives; the window target is the first whose check does, for
/// `types/htl-mq/mq.d.tl`. Under `just e2e-scaffold-packaged` the CLI under test
/// (`HTL_TEST_BIN`) is installed from a `.crate` tarball and pins its own version,
/// `htl-mq = "<version>"`, which is not on crates.io while the gate runs. The resolve
/// fails, the declaration is never written, and both Teal files of the scaffold report
/// `mq` as a module not found: a patch that never arrived, said as a module that is not
/// there. The recipe sets `HTL_PATCH_HTL`, `HTL_PATCH_CORE`, `HTL_PATCH_MACROS`,
/// `HTL_PATCH_MQ` and `HTL_PATCH_STD` to the extracted tarballs, and this turns the ones
/// set into one `[patch.crates-io]` line each. It is the e2e helper of the same name, for
/// the tests that `cargo test -p htl-cli` runs on every commit. A path only replaces a
/// pin it satisfies: pointed at a tree of another version, cargo passes over the patch
/// and fails the resolve as before (`failed to select a version for the requirement`),
/// which is why the recipe points them at the tarballs of the version being packaged.
///
/// The patch goes to `.cargo/config.toml` rather than to the manifest so the scaffolded
/// `Cargo.toml` stays byte for byte the one a user gets — what the scaffold tests and the
/// e2e pin check read. With no `HTL_PATCH_*` set nothing is written at all, which is every
/// run in a checkout: there the CLI is the checkout's and pins the checkout, so there is
/// nothing to redirect.
///
/// A project that never spawns cargo (`hb`, which has no `Cargo.toml`) or pins git rather
/// than a version (`--htl main`) is not affected by `[patch.crates-io]`, and does not need
/// this.
#[allow(dead_code)]
pub fn write_patch_config(project: &Path) {
    let mut patched = String::new();
    for (krate, var) in PATCHES {
        if let Some(path) = std::env::var_os(var) {
            patched.push_str(&format!(
                "{krate} = {{ path = '{}' }}\n",
                PathBuf::from(path).display()
            ));
        }
    }
    if patched.is_empty() {
        return;
    }
    let dir = project.join(".cargo");
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    let path = dir.join("config.toml");
    std::fs::write(&path, format!("[patch.crates-io]\n{patched}"))
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}
