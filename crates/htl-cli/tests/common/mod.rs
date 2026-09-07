//! Scratch directories for the integration tests, in one place.
//!
//! Every test file used to carry its own copy of this, separated only by a timestamp —
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
