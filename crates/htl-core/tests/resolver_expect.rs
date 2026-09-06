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
    write(
        &dir.join("good.tl"),
        "return { name = \"swarm\", hp = 3 }\n",
    );
    write(
        &dir.join("bad.tl"),
        "return { name = \"broken\", hp = \"lots\" }\n",
    );

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
    let err = h
        .lua()
        .load("return require('bad').hp")
        .eval::<i64>()
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("does not satisfy defs.Mod"), "{msg}");
    assert!(
        msg.contains("hp"),
        "should point at the offending field: {msg}"
    );
    assert!(
        msg.contains("hint: annotate the returned table"),
        "should tell how to get field-level errors: {msg}"
    );
}

fn setup_strict() -> (PathBuf, Htl) {
    let dir = scratch("strict");
    write(
        &dir.join("defs.tl"),
        "local record defs\n   record Mod\n      name: string\n      hp: integer\n      monsters: {string}\n   end\nend\nreturn defs\n",
    );
    write(
        &dir.join("full.tl"),
        "return { name = \"swarm\", hp = 3, monsters = { \"bat\" } }\n",
    );
    write(&dir.join("partial.tl"), "return { name = \"quiet\" }\n");
    let h = Htl::new().unwrap();
    let mut reg = Registry::new();
    reg.add(
        TealResolver::new(&dir)
            .unwrap()
            .expect_type("defs.Mod")
            .require_all_fields(),
    );
    reg.install(h.lua()).unwrap();
    (dir, h)
}

/// `require_fields(names)`: the same tree, held to a core rather than to everything.
fn setup_named(names: &[&str]) -> (PathBuf, Htl) {
    let dir = scratch("named");
    write(
        &dir.join("defs.tl"),
        "local record defs\n   record Mod\n      name: string\n      hp: integer\n      monsters: {string}\n   end\nend\nreturn defs\n",
    );
    write(&dir.join("partial.tl"), "return { name = \"quiet\" }\n");
    write(
        &dir.join("core.tl"),
        "return { name = \"swarm\", hp = 3 }\n",
    );
    let h = Htl::new().unwrap();
    let mut reg = Registry::new();
    reg.add(
        TealResolver::new(&dir)
            .unwrap()
            .expect_type("defs.Mod")
            .require_fields(names.iter().copied()),
    );
    reg.install(h.lua()).unwrap();
    (dir, h)
}

/// A field outside the list may be absent: this is what lets the record grow.
#[test]
fn require_fields_named_ignores_the_fields_it_does_not_name() {
    let (_dir, h) = setup_named(&["name", "hp"]);
    let hp: i64 = h.lua().load("return require('core').hp").eval().unwrap();
    assert_eq!(hp, 3, "monsters is declared but not required");
}

/// A field inside the list is still mandatory.
#[test]
fn require_fields_named_rejects_a_missing_one_of_its_own() {
    let (_dir, h) = setup_named(&["name", "hp"]);
    let err = h
        .lua()
        .load("return require('partial').name")
        .eval::<String>()
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("missing required field(s) of defs.Mod: hp"),
        "{err}"
    );
    assert!(
        !err.contains("monsters"),
        "a field outside the list is not reported: {err}"
    );
}

/// A name the record does not declare is the host's mistake, and is said so.
#[test]
fn require_fields_named_rejects_a_field_the_record_lacks() {
    let (_dir, h) = setup_named(&["name", "hitpoints"]);
    let err = h
        .lua()
        .load("return require('core').name")
        .eval::<String>()
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("does not declare: hitpoints"),
        "the unknown name is named: {err}"
    );
}

/// `require_fields`: a mod that type-checks but leaves declared fields nil is rejected.
#[test]
fn require_fields_rejects_missing_fields_at_require() {
    let (_dir, h) = setup_strict();
    let err = h
        .lua()
        .load("return require('partial').name")
        .eval::<String>()
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("missing required field(s) of defs.Mod: hp, monsters"),
        "{err}"
    );
}

#[test]
fn require_fields_accepts_complete_mod() {
    let (_dir, h) = setup_strict();
    let hp: i64 = h.lua().load("return require('full').hp").eval().unwrap();
    assert_eq!(hp, 3);
}

/// Without `require_fields`, the partial mod still passes (Teal fields are nilable).
#[test]
fn without_require_fields_partial_passes() {
    let (_dir, h) = setup();
    write(&_dir.join("partial.tl"), "return { name = \"quiet\" }\n");
    let name: String = h
        .lua()
        .load("return require('partial').name")
        .eval()
        .unwrap();
    assert_eq!(name, "quiet");
}

#[test]
fn defs_itself_still_resolves() {
    // `defs.tl` is served by the same resolver; it must not be held to `defs.Mod`
    // in a way that breaks (it isn't a Mod) — so expectation applies only to modules
    // other than the type's own module.
    let (_dir, h) = setup();
    let ok: bool = h
        .lua()
        .load("return require('defs') ~= nil")
        .eval()
        .unwrap();
    assert!(ok);
}
