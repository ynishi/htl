//! Temp directories for the end-to-end tests, shared with `htl-core`'s integration
//! tests rather than duplicated: `TempDir` and `tempdir` are defined once, in
//! `crates/htl-core/tests/common/mod.rs`, and brought in here by `#[path]` so this crate
//! and that one compile the same ~20 lines instead of each keeping its own copy. See that
//! file's doc comment for what `TempDir` promises (in particular: a panicking test's
//! directory is left on disk, any other test's is removed).

#[allow(dead_code)]
#[path = "../../../crates/htl-core/tests/common/mod.rs"]
mod core_common;

pub use core_common::TempDir;

/// A fresh directory under the system temp dir, named for the test that asked. See
/// `core_common::tempdir` (the one place this is implemented) for what `prefix` and
/// `name` do and what the returned [`TempDir`] promises.
pub fn tempdir(prefix: &str, name: &str) -> TempDir {
    core_common::tempdir(prefix, name)
}
