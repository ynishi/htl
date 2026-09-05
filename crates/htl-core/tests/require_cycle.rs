//! `require_cycles`: project-level detection of cycles in the require graph.

use htl_core::{CheckInfo, Htl, require_cycles};
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "htl-core-cycle-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn check_all(dir: &Path, names: &[&str]) -> Vec<(PathBuf, CheckInfo)> {
    let h = Htl::new().unwrap();
    h.add_path(dir).unwrap();
    names
        .iter()
        .map(|n| {
            let p = dir.join(n);
            let ci = h.check(&p).unwrap();
            (p, ci)
        })
        .collect()
}

#[test]
fn two_file_cycle_is_reported_once() {
    let dir = scratch("ab");
    write(&dir.join("a.tl"), "local b = require(\"b\")\nlocal record a\nend\nfunction a.f(): integer return 1 end\nprint(b)\nreturn a\n");
    write(&dir.join("b.tl"), "local a = require(\"a\")\nlocal record b\nend\nprint(a)\nreturn b\n");
    let infos = check_all(&dir, &["a.tl", "b.tl"]);
    let cycles = require_cycles(&infos);
    assert_eq!(cycles.len(), 1, "{cycles:?}");
    let c = &cycles[0];
    assert!(c.contains("require cycle: a.tl -> b.tl -> a.tl"), "{c}");
    assert!(c.contains("[htl require-cycle]"), "{c}");
    assert!(c.contains("a.tl:1:"), "anchored at a.tl's require: {c}");
}

#[test]
fn dag_has_no_cycles() {
    let dir = scratch("dag");
    write(&dir.join("defs.tl"), "local record defs\n   record P\n      x: number\n   end\nend\nreturn defs\n");
    write(&dir.join("grid.tl"), "local defs = require(\"defs\")\nlocal record grid\nend\nfunction grid.at(p: defs.P): number return p.x end\nreturn grid\n");
    write(&dir.join("world.tl"), "local defs = require(\"defs\")\nlocal grid = require(\"grid\")\nlocal record world\nend\nfunction world.go(p: defs.P): number return grid.at(p) end\nreturn world\n");
    let infos = check_all(&dir, &["defs.tl", "grid.tl", "world.tl"]);
    assert!(require_cycles(&infos).is_empty());
}

#[test]
fn three_file_cycle_names_the_loop() {
    let dir = scratch("abc");
    write(&dir.join("a.tl"), "local b = require(\"b\")\nprint(b)\nreturn {}\n");
    write(&dir.join("b.tl"), "local c = require(\"c\")\nprint(c)\nreturn {}\n");
    write(&dir.join("c.tl"), "local a = require(\"a\")\nprint(a)\nreturn {}\n");
    let infos = check_all(&dir, &["a.tl", "b.tl", "c.tl"]);
    let cycles = require_cycles(&infos);
    assert_eq!(cycles.len(), 1, "{cycles:?}");
    assert!(cycles[0].contains("a.tl -> b.tl -> c.tl -> a.tl"), "{}", cycles[0]);
}
