//! What a cold `htl check` actually spends its time on.
//!
//! The incremental cache (issue #3) can only ever remove two things: the cost of
//! standing up a checker at all, and the cost of type-checking modules whose inputs
//! did not change. This benchmark measures both separately, so that a claim about
//! what the cache saves rests on a number rather than on an estimate.
//!
//! - `checker/new` is the fixed cost: `Htl::new()` compiles the vendored `tl.lua`,
//!   evaluates the prelude and builds the base environment. A run that hits the cache
//!   for every module can skip this entirely; a run that misses even once cannot.
//! - `check/cold/<n>` is a whole run from nothing: build a checker, then check `n`
//!   modules the way `htl check` does. This is the number a fully-cached run removes.
//! - `check/warm-checker/<n>` runs the same modules through a checker that already
//!   checked them once, so its store is populated. It answers a different question
//!   from the cache's: how much the *in-process* reuse already saves within one run.
//! - `check/module-size/<fns>` holds the module count fixed and grows each module
//!   instead. A project is not 48 eight-line modules, and if checking cost scales with
//!   source size rather than with file count, that is where a real project's time goes
//!   and where the cache's value is decided.
//!
//! The fixture is shaped like a real project rather than like a chain: one leaf module
//! everything uses, a few mid-layer modules, and a wide outer layer that depends on
//! both. A deep chain would measure the checker's recursion, which is not what a
//! project looks like and not what the cache is for.
//!
//! Run one group:
//!
//! ```bash
//! cargo bench -p htl-core --bench check -- check/module-size
//! ```

mod common;

use common::{config, scratch, write};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use htl_core::Htl;
use std::hint::black_box;
use std::path::{Path, PathBuf};

/// Module counts to measure. Two points, so the fixed cost and the per-module cost
/// can be told apart; more points would cost bench time without changing the answer.
const SIZES: [usize; 2] = [12, 48];

/// How many mid-layer modules the outer layer fans in to.
const CORES: usize = 4;

/// Functions per module for the file-count benchmarks: a small module.
const SMALL: usize = 3;

/// Functions per module for `check/module-size`, at ~5 lines of Teal each: roughly a
/// 15-, 100- and 300-line module. A hand-written `.tl` module sits nearer the last two
/// than the first.
const FN_COUNTS: [usize; 3] = [3, 20, 60];

/// Module count held fixed while module size varies.
const SIZE_BENCH_MODULES: usize = 48;

/// `fns` typed functions on `record`, about five lines each, so a module can be grown
/// without changing the shape of the dependency graph. Every one of them goes through
/// `util`, so the checker resolves and applies an imported type per function rather
/// than checking a self-contained body.
fn filler(record: &str, fns: usize) -> String {
    let mut s = String::new();
    for k in 0..fns {
        s.push_str(&format!(
            "function {record}.f{k}(n: integer, s: string): {{string:integer}}\n   local t: {{string:integer}} = {{}}\n   t[s] = util.id(n) + {k}\n   return t\nend\n"
        ));
    }
    s
}

/// Write an `n`-module project with `fns` filler functions per module, and return its
/// directory and its files in the order `htl check` would walk them (leaf first, so the
/// store is populated the way a real run populates it).
///
/// Shape: `util` is required by everything; `core_0..core_3` require `util`; the rest
/// are `feat_i`, each requiring `util` and one `core`.
fn project(n: usize, fns: usize) -> (PathBuf, Vec<PathBuf>) {
    assert!(
        n > CORES + 1,
        "n must leave room for the leaf and the cores"
    );
    let dir = scratch(&format!("proj{n}x{fns}"));
    let mut files = Vec::with_capacity(n);

    write(
        &dir.join("util.tl"),
        "local record util\n\
         end\n\
         function util.id(n: integer): integer\n   return n\nend\n\
         function util.name(s: string): string\n   return s .. \"\"\nend\n\
         function util.pair(n: integer, s: string): {string:integer}\n   return { [s] = n }\nend\n\
         return util\n",
    );
    files.push(dir.join("util.tl"));

    for i in 0..CORES {
        let name = format!("core_{i}");
        write(
            &dir.join(format!("{name}.tl")),
            &format!(
                "local util = require(\"util\")\n\
                 local record {name}\n\
                 end\n\
                 function {name}.step(n: integer): integer\n   return util.id(n) + {i}\nend\n\
                 function {name}.label(s: string): string\n   return util.name(s)\nend\n\
                 {}\
                 return {name}\n",
                filler(&name, fns)
            ),
        );
        files.push(dir.join(format!("{name}.tl")));
    }

    for i in 0..(n - CORES - 1) {
        let name = format!("feat_{i}");
        let c = i % CORES;
        write(
            &dir.join(format!("{name}.tl")),
            &format!(
                "local util = require(\"util\")\n\
                 local core = require(\"core_{c}\")\n\
                 local record {name}\n\
                 end\n\
                 function {name}.run(n: integer): integer\n   return core.step(util.id(n))\nend\n\
                 function {name}.tag(s: string): {{string:integer}}\n   return util.pair({i}, core.label(s))\nend\n\
                 {}\
                 return {name}\n",
                filler(&name, fns)
            ),
        );
        files.push(dir.join(format!("{name}.tl")));
    }

    assert_eq!(files.len(), n);
    (dir, files)
}

/// Check every file with a checker built for this run, the way one `htl check`
/// invocation does. Asserts the fixture is clean, so a bench that silently started
/// measuring error paths fails instead of reporting a number.
fn check_all(h: &Htl, dir: &Path, files: &[PathBuf]) {
    h.add_path(dir).unwrap();
    for f in files {
        let info = h.check(f).unwrap();
        assert!(
            info.ok(),
            "bench fixture must type-check: {:?}",
            info.errors
        );
        black_box(&info);
    }
}

/// The fixed cost every run pays before it checks anything.
fn bench_checker_new(c: &mut Criterion) {
    c.bench_function("checker/new", |b| {
        b.iter(|| black_box(Htl::new().unwrap()));
    });
}

/// A whole cold run: build a checker, check `n` modules. This is what a fully-cached
/// run removes.
fn bench_check_cold(c: &mut Criterion) {
    let mut g = c.benchmark_group("check/cold");
    for n in SIZES {
        let (dir, files) = project(n, SMALL);
        g.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let h = Htl::new().unwrap();
                check_all(&h, &dir, &files);
            });
        });
    }
    g.finish();
}

/// The same modules through a checker whose store they already populated.
fn bench_check_warm_checker(c: &mut Criterion) {
    let mut g = c.benchmark_group("check/warm-checker");
    for n in SIZES {
        let (dir, files) = project(n, SMALL);
        let h = Htl::new().unwrap();
        g.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| check_all(&h, &dir, &files));
        });
    }
    g.finish();
}

/// Fixed module count, growing modules: does checking cost follow file count or source
/// size?
fn bench_module_size(c: &mut Criterion) {
    let mut g = c.benchmark_group("check/module-size");
    for fns in FN_COUNTS {
        let (dir, files) = project(SIZE_BENCH_MODULES, fns);
        g.bench_with_input(BenchmarkId::from_parameter(fns), &fns, |b, _| {
            b.iter(|| {
                let h = Htl::new().unwrap();
                check_all(&h, &dir, &files);
            });
        });
    }
    g.finish();
}

criterion_group! {
    name = benches;
    config = config();
    targets = bench_checker_new, bench_check_cold, bench_check_warm_checker, bench_module_size
}
criterion_main!(benches);
