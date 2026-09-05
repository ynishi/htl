//! The checked-module store shared by fresh envs in one state: results are reused, but
//! a module name is only seeded when it resolves to the same file under the current
//! search path, so two directories with a same-named module never see each other's.

use htl_core::Htl;
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "htl-core-store-{name}-{}-{}",
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

#[test]
fn same_named_modules_in_two_dirs_stay_apart_across_checks() {
    let a = scratch("a");
    let b = scratch("b");
    write(&a.join("util.tl"), "local record util\nend\nfunction util.f(): integer\n   return 1\nend\nreturn util\n");
    write(&b.join("util.tl"), "local record util\nend\nfunction util.f(): string\n   return \"s\"\nend\nreturn util\n");
    write(&a.join("main.tl"), "local util = require(\"util\")\nlocal n: integer = util.f()\nprint(n)\n");
    write(&b.join("main.tl"), "local util = require(\"util\")\nlocal s: string = util.f()\nprint(s)\n");

    let h = Htl::new().unwrap();
    // The CLI does this per file: reset, then the file's own dir.
    h.reset_search_path().unwrap();
    h.add_path(&a).unwrap();
    let ca = h.check(&a.join("main.tl")).unwrap();
    assert!(ca.ok(), "{:?}", ca.errors);

    h.reset_search_path().unwrap();
    h.add_path(&b).unwrap();
    let cb = h.check(&b.join("main.tl")).unwrap();
    assert!(cb.ok(), "b must see its own util (string), not a's from the store: {:?}", cb.errors);

    // And back: the store entry for `util` now points at b's file; a resolves to its own.
    h.reset_search_path().unwrap();
    h.add_path(&a).unwrap();
    let ca2 = h.check(&a.join("main.tl")).unwrap();
    assert!(ca2.ok(), "{:?}", ca2.errors);
}

#[test]
fn seeded_check_reports_the_same_as_a_cold_one() {
    let dir = scratch("same");
    write(&dir.join("dep.tl"), "local record dep\nend\nfunction dep.f(): integer\n   return \"wrong\"\nend\nreturn dep\n");
    write(&dir.join("user1.tl"), "local dep = require(\"dep\")\nlocal x: integer = dep.f()\nprint(x)\n");
    write(&dir.join("user2.tl"), "local dep = require(\"dep\")\nlocal y: string = dep.f()\nprint(y)\n");

    let h = Htl::new().unwrap();
    h.add_path(&dir).unwrap();
    // First check populates the store with `dep` (which has its own type error).
    let c1 = h.check(&dir.join("user1.tl")).unwrap();
    assert!(c1.ok(), "dep's internal error belongs to dep, not its requirer: {:?}", c1.errors);
    // Second check is seeded: must see the same `dep` type and report its own mismatch.
    let c2 = h.check(&dir.join("user2.tl")).unwrap();
    assert_eq!(c2.errors.len(), 1, "{:?}", c2.errors);
    assert!(c2.errors[0].contains("got integer, expected string"), "{}", c2.errors[0]);
    // Checking dep itself still reports dep's error.
    let cd = h.check(&dir.join("dep.tl")).unwrap();
    assert!(cd.errors.iter().any(|e| e.contains("expected integer")), "{:?}", cd.errors);
    // Dependencies are still tracked for the seeded check.
    assert!(c2.deps.iter().any(|d| d.ends_with("dep.tl")), "{:?}", c2.deps);
}
