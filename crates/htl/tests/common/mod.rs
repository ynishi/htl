//! Temp directories for the integration tests, in one place.
//!
//! [`TempDir`] and [`tempdir`] are implemented once, in
//! `crates/htl-core/tests/common/mod.rs`, and brought in here by `#[path]`: the workspace
//! shares test helpers by linking the one file rather than copying it. This crate's test
//! binaries are compiled separately from `htl-core`'s and `htl-cli`'s, but a `#[path]`
//! `mod` item names any file on disk, crate boundary or not.

#[allow(dead_code)]
#[path = "../../../htl-core/tests/common/mod.rs"]
mod core_common;

pub use core_common::TempDir;

/// A fresh directory under the system temp dir, named for the test that asked. See
/// `core_common::tempdir` — the one place this is implemented — for what `prefix` and
/// `name` do and what the returned [`TempDir`] promises.
pub fn tempdir(prefix: &str, name: &str) -> TempDir {
    core_common::tempdir(prefix, name)
}
