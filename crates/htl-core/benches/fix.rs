//! What verifying a fix against the file rather than against the store costs.
//!
//! `htl fix` writes a file and then measures it. It cannot ask the checker's usual way,
//! because the store answers about the version from before the write and a correct fix gets
//! reverted for leaving the error count unchanged (#29). `Htl::check_written` asks with a
//! cold environment instead, which re-checks the modules the file requires — a real cost,
//! paid a handful of times per file, and this is it.
//!
//! - `check/from-store` and `check/written` are the same file, checked both ways. The gap is
//!   the price of the correct answer, and it grows with how much the file requires.
//! - `fix/file` is what a caller actually waits for: `fix_file` end to end, including the
//!   passes and their verification.
//!
//! Run:
//!
//! ```bash
//! cargo bench -p htl-core --bench fix
//! ```

mod common;

use common::{config, scratch, write};
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use htl_core::Htl;
use htl_core::fix::{FixOptions, fix_file};
use std::hint::black_box;
use std::path::PathBuf;

/// How many modules the file being fixed requires. A cold check re-checks all of them, so
/// this is the axis the change is sensitive to.
const DEPS: [usize; 3] = [0, 4, 16];

/// The file that gets fixed: a forward reference, plus a require of each dependency so the
/// cold check has something to re-check.
fn world_source(deps: usize) -> String {
    let mut s = String::new();
    for i in 0..deps {
        s.push_str(&format!("local dep_{i} = require(\"dep_{i}\")\n"));
    }
    s.push_str("local record world\n   greet: function(): integer\nend\n\n");
    s.push_str("function world.greet(): integer\n   return world.helper()");
    for i in 0..deps {
        s.push_str(&format!(" + dep_{i}.value()"));
    }
    s.push_str("\nend\n\nfunction world.helper(): integer\n   return 1\nend\n\nreturn world\n");
    s
}

/// A project whose `world.tl` needs one forward-reference fix, and a second module requiring
/// it — which is what puts `world` in the store and made #29 visible in the first place.
fn project(deps: usize) -> (PathBuf, PathBuf) {
    let dir = scratch(&format!("d{deps}"));
    for i in 0..deps {
        write(
            &dir.join(format!("dep_{i}.tl")),
            &format!(
                "local record dep_{i}\nend\nfunction dep_{i}.value(): integer\n   return {i}\nend\nreturn dep_{i}\n"
            ),
        );
    }
    let world = dir.join("world.tl");
    write(&world, &world_source(deps));
    write(
        &dir.join("main.tl"),
        "local world = require(\"world\")\nprint(world.greet())\n",
    );
    (dir, world)
}

/// Both ways of asking about the same file, with the store warm for it.
fn bench_check_ways(c: &mut Criterion) {
    let mut g = c.benchmark_group("check");
    for deps in DEPS {
        let (dir, world) = project(deps);
        let h = Htl::new().unwrap();
        h.add_path(&dir).unwrap();
        // Put `world` in the store, the way checking anything that requires it does.
        h.check(&dir.join("main.tl")).unwrap();

        g.bench_with_input(BenchmarkId::new("from-store", deps), &deps, |b, _| {
            b.iter(|| black_box(h.check(&world).unwrap()));
        });
        g.bench_with_input(BenchmarkId::new("written", deps), &deps, |b, _| {
            b.iter(|| black_box(h.check_written(&world).unwrap()));
        });
    }
    g.finish();
}

/// What `htl fix` costs on one file, verification included. The fixture is restored before
/// each iteration, outside the measurement, since fixing is not idempotent in the sense that
/// matters here: the second run has nothing to do.
fn bench_fix_file(c: &mut Criterion) {
    let mut g = c.benchmark_group("fix/file");
    for deps in DEPS {
        let (dir, world) = project(deps);
        let h = Htl::new().unwrap();
        h.add_path(&dir).unwrap();
        h.check(&dir.join("main.tl")).unwrap();
        let source = world_source(deps);

        g.bench_with_input(BenchmarkId::from_parameter(deps), &deps, |b, _| {
            b.iter_batched(
                || write(&world, &source),
                |_| {
                    let out = fix_file(&h, &world, &FixOptions::default()).unwrap();
                    assert!(
                        out.reverted.is_none(),
                        "the fix must apply: {:?}",
                        out.reverted
                    );
                    black_box(out)
                },
                // Per iteration, not per batch: a batch would restore the fixture N times and
                // then fix N times, so every run after the first would be fixing a file that
                // is already fixed.
                BatchSize::PerIteration,
            );
        });
    }
    g.finish();
}

criterion_group! {
    name = benches;
    config = config();
    targets = bench_check_ways, bench_fix_file
}
criterion_main!(benches);
