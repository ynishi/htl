//! Temp directories for the integration tests, in one place.
//!
//! Every test file used to carry its own copy of this, separated only by a timestamp —
//! and the clock advances in microsecond steps, so two tests in one binary that ask for
//! the same name land in the same directory and read each other's fixture. A counter
//! settles it: two calls in one process never agree, whatever the clock does.
//!
//! [`tempdir`] hands out a [`TempDir`] rather than a bare path, so the directory is
//! removed when the test that asked for it is done with it — see that type's doc for
//! what "done with it" means for a test that panics.

use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NTH: AtomicU64 = AtomicU64::new(0);

/// A temp directory that removes itself (recursively) when it goes out of scope.
///
/// A test that panics keeps its directory: `Drop` checks [`std::thread::panicking`], and
/// if the thread is unwinding it leaves the directory on disk so the fixtures and output
/// a failure left behind can still be read after the test harness has reported it. A test
/// that fails by returning `Err` does not panic — `std::thread::panicking()` sees nothing
/// unusual — so its directory is removed exactly like a passing test's; a `-> Result<()>`
/// test that fails on a `?` is such a test (`batteries.rs`'s
/// `a_script_using_std_checks_and_runs`, for one).
///
/// A passing test's directory is removed when its `TempDir` drops: `std::fs::remove_dir_all`
/// runs, and its error, if any, is ignored — there is nothing left for a `Drop` to do about
/// a directory another process already removed or still has open, and `Drop` has no
/// `Result` to report it with anyway.
///
/// Bind the guard for as long as the directory is needed: `let dir = tempdir(..);
/// dir.join(..)` keeps it alive, but `tempdir(..).join(..)` drops the guard (and removes
/// the directory) at the end of that statement, before the path it returns is ever used.
///
/// `TempDir` derefs to [`Path`] (and implements [`AsRef<Path>`]), so it is used wherever a
/// `&Path` is wanted. A call site that needs an owned `PathBuf` asks for one explicitly
/// with `root.to_path_buf()`; `TempDir` is deliberately not `Clone`, so two guards never
/// race to remove the same directory.
#[derive(Debug)]
pub struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

impl Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

/// A fresh directory under the system temp dir, named for the test that asked. `prefix`
/// separates one test binary from another (they are separate processes, so the pid does
/// too, but the name is what a leftover directory is read by). The returned [`TempDir`]
/// removes the directory when it drops; see that type's doc for the panic case.
pub fn tempdir(prefix: &str, name: &str) -> TempDir {
    let dir = std::env::temp_dir().join(format!(
        "{prefix}-{name}-{}-{}",
        std::process::id(),
        NTH.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    TempDir(dir)
}
