//! What every integration test in this crate needs: a scratch directory, and the binary
//! to run in it.
//!
//! This module is compiled into each test binary that declares `mod common;`, and anything
//! it defines that a given binary does not use is a warning CI turns into an error. So it
//! holds only what all of them want. Both items here qualify: every file that declares the
//! module calls `scratch` and spawns `htl_bin`. A helper wanted by two files stays in those
//! two files — `fmt_snapshot.rs` keeps its own copy of the snapshot block for that reason.
//!
//! The scratch directories used to be a copy per test file, separated only by a timestamp —
//! and the clock advances in microsecond steps, so two tests in one binary that ask for
//! the same name land in the same directory and read each other's fixture. A counter
//! settles it: two calls in one process never agree, whatever the clock does.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NTH: AtomicU64 = AtomicU64::new(0);

/// A fresh directory under the system temp dir, named for the test that asked. `prefix`
/// separates one test binary from another (they are separate processes, so the pid does
/// too, but the name is what a leftover directory is read by).
pub fn scratch(prefix: &str, name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "{prefix}-{name}-{}-{}",
        std::process::id(),
        NTH.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
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
