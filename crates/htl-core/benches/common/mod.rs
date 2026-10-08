//! Shared setup for this crate's benchmarks.
//!
//! Every benchmark here wants a temp project, a way to write files into it, and the same
//! criterion settings. Writing those separately in each file is how they drift: one ends up
//! with a different sample count than another and the numbers stop being comparable.
//!
//! Lives in `benches/common/mod.rs` rather than `benches/common.rs` so cargo does not treat
//! it as a benchmark of its own.

use criterion::Criterion;
use std::path::Path;
use std::time::Duration;

#[allow(dead_code)]
#[path = "../../tests/common/mod.rs"]
mod core_common;

pub use core_common::TempDir;

/// A temp directory that will not collide with a parallel run of another benchmark, and
/// is removed when the returned [`TempDir`] drops; see `tests/common/mod.rs` — the one
/// place this is implemented — for what that promises.
pub fn tempdir(name: &str) -> TempDir {
    core_common::tempdir("htl-bench", name)
}

/// Write a file, creating its directory. Panics: a benchmark that cannot lay out its fixture
/// has nothing to measure, and failing loudly beats reporting a number for the wrong thing.
pub fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// The settings every benchmark in this crate uses.
///
/// Ten samples rather than the default hundred: these are milliseconds, the figures get read
/// to one significant digit, and a hundred samples of a checker run would hold the machine
/// for minutes to sharpen a number nobody reads that closely.
pub fn config() -> Criterion {
    Criterion::default()
        .sample_size(10)
        .measurement_time(Duration::from_secs(10))
        .warm_up_time(Duration::from_secs(3))
}
