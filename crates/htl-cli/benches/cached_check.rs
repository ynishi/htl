//! What the run cache saves, measured through the binary rather than around it.
//!
//! `htl-core`'s `check` bench measures the work itself: building a checker, and
//! type-checking modules. This one measures what a person or a tool actually waits for,
//! process startup included, with the cache off and on. The gap between the two groups is
//! the claim the cache makes, and keeping it a benchmark rather than a remembered figure
//! is what stops it from quietly going away.
//!
//! The fixture mirrors the one in `htl-core`'s bench — a leaf module everything uses, a
//! few mid-layer modules, a wide outer layer — at a size where a module is about a hundred
//! lines, which is nearer real Teal than a generated stub is. The two are separate crates,
//! so the generator is written twice on purpose rather than exported from the library for
//! a benchmark's sake.
//!
//! ```bash
//! cargo bench -p htl-cli --bench cached_check
//! ```

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const MODULES: usize = 48;
const CORES: usize = 4;
const FNS: usize = 20;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn filler(record: &str, fns: usize) -> String {
    let mut s = String::new();
    for k in 0..fns {
        s.push_str(&format!(
            "function {record}.f{k}(n: integer, s: string): {{string:integer}}\n   local t: {{string:integer}} = {{}}\n   t[s] = util.id(n) + {k}\n   return t\nend\n"
        ));
    }
    s
}

/// One mid-layer module. `salt` changes its body without changing its type, so the
/// one-module-edited benchmark can make it miss on every iteration while everything that
/// does not depend on it still replays.
fn core_source(i: usize, salt: u32) -> String {
    format!(
        "local util = require(\"util\")\n\
         local record core_{i}\n\
         end\n\
         function core_{i}.step(n: integer): integer\n   return util.id(n) + {i} + {salt} - {salt}\nend\n\
         {}\
         return core_{i}\n",
        filler(&format!("core_{i}"), FNS)
    )
}

/// An `n`-module project under a fresh directory.
fn project(n: usize) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "htl-cli-bench-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    write(&root.join("htl.toml"), "[check]\n");
    write(
        &root.join("src/util.tl"),
        "local record util\n\
         end\n\
         function util.id(n: integer): integer\n   return n\nend\n\
         function util.pair(n: integer, s: string): {string:integer}\n   return { [s] = n }\nend\n\
         return util\n",
    );
    for i in 0..CORES {
        write(&root.join(format!("src/core_{i}.tl")), &core_source(i, 0));
    }
    for i in 0..(n - CORES - 1) {
        let name = format!("feat_{i}");
        let c = i % CORES;
        write(
            &root.join(format!("src/{name}.tl")),
            &format!(
                "local util = require(\"util\")\n\
                 local core = require(\"core_{c}\")\n\
                 local record {name}\n\
                 end\n\
                 function {name}.run(n: integer): integer\n   return core.step(util.id(n))\nend\n\
                 {}\
                 return {name}\n",
                filler(&name, FNS)
            ),
        );
    }
    root
}

/// One `htl check`, asserting it succeeded: a benchmark that started measuring an error
/// path should fail rather than report a faster number.
fn check(root: &Path, args: &[&str]) {
    let out = Command::new(env!("CARGO_BIN_EXE_htl"))
        .arg("check")
        .arg("src")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "bench fixture must check clean: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    black_box(&out);
}

fn bench_cli(c: &mut Criterion) {
    let root = project(MODULES);
    let mut g = c.benchmark_group("check/cli");

    g.bench_function("cold", |b| b.iter(|| check(&root, &["--no-cache"])));

    // Warm the store once, so the measured runs are hits rather than the first miss.
    check(&root, &[]);
    g.bench_function("cached", |b| b.iter(|| check(&root, &[])));

    // The case the per-module entries exist for: one mid-layer module edited, so it and the
    // quarter of the outer layer that requires it are checked while the rest replay. The
    // file write is inside the measured region — it is part of what an edit costs, and it
    // is microseconds against the rest.
    let mut salt = 0u32;
    g.bench_function("one-edited", |b| {
        b.iter(|| {
            salt += 1;
            write(&root.join("src/core_0.tl"), &core_source(0, salt));
            check(&root, &[]);
        })
    });

    // The other granularity, for the same two cases. It should match per-module when
    // nothing moved and match a cold run when anything did — which is the whole of the
    // difference between them, and the reason the mode is a choice rather than a default
    // nobody can change.
    let whole = &["--cache-mode", "whole-run"];
    check(&root, whole);
    g.bench_function("cached/whole-run", |b| b.iter(|| check(&root, whole)));
    g.bench_function("one-edited/whole-run", |b| {
        b.iter(|| {
            salt += 1;
            write(&root.join("src/core_0.tl"), &core_source(0, salt));
            check(&root, whole);
        })
    });

    g.finish();
}

criterion_group! {
    name = benches;
    // Each sample forks a process and, in the cold group, type-checks 48 modules. Ten
    // samples is enough to separate a second from a few milliseconds.
    config = Criterion::default()
        .sample_size(10)
        .measurement_time(Duration::from_secs(10))
        .warm_up_time(Duration::from_secs(3));
    targets = bench_cli
}
criterion_main!(benches);
