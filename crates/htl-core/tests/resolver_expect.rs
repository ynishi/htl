//! `TealResolver::expect_type`: a mod that never annotates its return value is still
//! rejected at `require` time when it does not satisfy the expected record type.

use htl_core::Htl;
use htl_core::pkg::TealResolver;
use mlua_pkg::Registry;
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "htl-core-expect-{name}-{}-{}",
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

fn setup() -> (PathBuf, Htl) {
    let dir = scratch("mods");
    write(
        &dir.join("defs.tl"),
        "local record defs\n   record Mod\n      name: string\n      hp: integer\n   end\nend\nreturn defs\n",
    );
    // No `local m: defs.Mod` annotation in either mod.
    write(&dir.join("good.tl"), "return { name = \"swarm\", hp = 3 }\n");
    write(&dir.join("bad.tl"), "return { name = \"broken\", hp = \"lots\" }\n");

    let h = Htl::new().unwrap();
    let mut reg = Registry::new();
    reg.add(TealResolver::new(&dir).unwrap().expect_type("defs.Mod"));
    reg.install(h.lua()).unwrap();
    (dir, h)
}

#[test]
fn conforming_mod_loads() {
    let (_dir, h) = setup();
    let hp: i64 = h.lua().load("return require('good').hp").eval().unwrap();
    assert_eq!(hp, 3);
}

#[test]
fn nonconforming_mod_is_rejected_at_require() {
    let (_dir, h) = setup();
    let err = h.lua().load("return require('bad').hp").eval::<i64>().unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("does not satisfy defs.Mod"), "{msg}");
    assert!(msg.contains("hp"), "should point at the offending field: {msg}");
}

#[test]
fn defs_itself_still_resolves() {
    // `defs.tl` is served by the same resolver; it must not be held to `defs.Mod`
    // in a way that breaks (it isn't a Mod) — so expectation applies only to modules
    // other than the type's own module.
    let (_dir, h) = setup();
    let ok: bool = h.lua().load("return require('defs') ~= nil").eval().unwrap();
    assert!(ok);
}
