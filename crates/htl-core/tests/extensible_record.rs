//! `---@extensible`: a record a table may carry keys beyond the ones it declares.
//!
//! Every Teal record is closed, and htl's other two markers move a different boundary —
//! `---@optional` and `---@required` decide which *declared* fields a literal may leave
//! out. This one is about the key the declaration has never heard of: a mod written
//! against a newer SDK, a save file from a later version, a table a host will grow next
//! release. What the marker buys is that tl's `unknown field <k>` is dropped for exactly
//! those keys; everything else about the record is unchanged, including that the key
//! cannot be *read* through the type.

use htl_core::Htl;
use htl_core::pkg::TealResolver;
use mlua_pkg::Registry;
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-extensible", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Check one file of a two-file project: the declaration in `defs.tl`, the site in
/// `use.tl`. Returns the errors, which is what the marker is about.
fn errors_of(dir: &Path, defs: &str, site: &str) -> Vec<String> {
    write(&dir.join("defs.tl"), defs);
    write(&dir.join("use.tl"), site);
    let h = Htl::new().unwrap();
    h.add_path(dir).unwrap();
    h.check(&dir.join("use.tl")).unwrap().errors
}

const OPEN: &str = "local record defs\n   record Mod   ---@extensible\n      name: string\n      \
                    hp: integer\n   end\nend\nreturn defs\n";
const CLOSED: &str = "local record defs\n   record Mod\n      name: string\n      hp: integer\n   \
                      end\nend\nreturn defs\n";
const SITE: &str = "local defs = require(\"defs\")\nlocal m: defs.Mod = { name = \"m\", hp = 1, \
                    extra = \"from a newer SDK\" }\nreturn m\n";

/// The marker's whole job, in the project's own code.
#[test]
fn an_extensible_record_accepts_a_key_it_does_not_declare() {
    let errors = errors_of(&scratch("open"), OPEN, SITE);
    assert!(errors.is_empty(), "{errors:?}");
}

/// Without it the record is closed, which is every record today.
#[test]
fn an_unmarked_record_still_refuses_the_same_key() {
    let errors = errors_of(&scratch("closed"), CLOSED, SITE);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("unknown field extra"), "{}", errors[0]);
}

/// Both marker forms, like `---@struct` and `---@sealed`: trailing (above) and on the
/// line of its own.
#[test]
fn the_marker_is_read_on_the_line_above_the_declaration() {
    let defs = "local record defs\n   ---@extensible\n   record Mod\n      name: string\n      \
                hp: integer\n   end\nend\nreturn defs\n";
    let errors = errors_of(&scratch("above"), defs, SITE);
    assert!(errors.is_empty(), "{errors:?}");
}

/// The marker buys tolerance where a value is built and nothing else: the keys it lets
/// through are still not part of the type, so reading one is the error it always was.
#[test]
fn reading_an_undeclared_key_is_still_an_error() {
    let site = "local defs = require(\"defs\")\nlocal m: defs.Mod = { name = \"m\", hp = 1, extra \
                = \"x\" }\nprint(m.extra)\nreturn m\n";
    let errors = errors_of(&scratch("read"), OPEN, site);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("invalid key 'extra'"),
        "the read, not the write: {}",
        errors[0]
    );
    assert!(
        errors[0].contains("use.tl:3:"),
        "at the read: {}",
        errors[0]
    );
}

/// A record nested directly under an extensible one sits on the line below the marker,
/// and is not extensible by that. Mark it too if it should be — the same rule
/// `---@sealed` reads by.
#[test]
fn a_record_nested_under_an_extensible_one_is_not_extensible() {
    let defs = "local record defs\n   record Mod   ---@extensible\n      record Inner\n         \
                a: string\n      end\n\n      name: string\n      inner: Inner\n   \
                end\nend\nreturn defs\n";
    let site = "local defs = require(\"defs\")\nlocal i: defs.Mod.Inner = { a = \"a\", extra = 1 \
                }\nreturn i\n";
    let errors = errors_of(&scratch("nested"), defs, site);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("unknown field extra"), "{}", errors[0]);
}

/// The other half of the record is untouched: a *declared* field still has its type.
#[test]
fn a_declared_field_still_has_to_hold_its_type() {
    let site = "local defs = require(\"defs\")\nlocal m: defs.Mod = { name = 2, hp = 1, extra = 3 \
                }\nreturn m\n";
    let errors = errors_of(&scratch("typed"), OPEN, site);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("in record field: name") && errors[0].contains("expected string"),
        "{}",
        errors[0]
    );
}

/// `---@struct` decides which declared fields a literal may leave out, and this decides
/// nothing about that. The near-miss message is unchanged, which is the typo path that
/// matters most: a *required* field left short is still named, and the key the literal
/// set instead is still pointed at.
#[test]
fn struct_and_extensible_still_report_a_missing_required_field() {
    let dir = scratch("struct");
    write(
        &dir.join("defs.tl"),
        "local record defs\n   ---@struct\n   record MonsterDef   ---@extensible\n      id: \
         string\n      color: string\n   end\nend\nreturn defs\n",
    );
    write(
        &dir.join("use.tl"),
        "local defs = require(\"defs\")\nlocal m: defs.MonsterDef = { id = \"bat\", colour = \
         \"red\" }\nreturn m\n",
    );
    let h = Htl::new().unwrap();
    h.add_path(&dir).unwrap();
    let ci = h.check(&dir.join("use.tl")).unwrap();
    // The misspelled key is a key the record does not declare, so the marker lets it
    // through the checker; the lint is what still has something to say about it.
    assert!(ci.errors.is_empty(), "{:?}", ci.errors);
    let hits: Vec<&String> = ci
        .lints
        .iter()
        .filter(|l| l.contains("[htl struct-fields]"))
        .collect();
    assert_eq!(hits.len(), 1, "{:?}", ci.lints);
    assert!(
        hits[0].contains("MonsterDef is built without color (the literal sets `colour`)"),
        "unchanged message: {}",
        hits[0]
    );
}

/// The cost, stated as a test so it is not a surprise later: a misspelled *optional*
/// field on an extensible record is silence. Nothing is missing, so `struct-fields` has
/// nothing to say, and the key is one the marker exists to allow. This is what the README
/// says beside the marker, and the answer for a program that wants the keys back is a map
/// field (`extra: {string: any}`).
#[test]
fn a_misspelled_optional_field_is_the_price_and_is_silent() {
    let dir = scratch("cost");
    write(
        &dir.join("defs.tl"),
        "local record defs\n   ---@struct\n   record MonsterDef   ---@extensible\n      id: \
         string\n      color: string   ---@optional\n   end\nend\nreturn defs\n",
    );
    write(
        &dir.join("use.tl"),
        "local defs = require(\"defs\")\nlocal m: defs.MonsterDef = { id = \"bat\", colour = \
         \"red\" }\nreturn m\n",
    );
    let h = Htl::new().unwrap();
    h.add_path(&dir).unwrap();
    let ci = h.check(&dir.join("use.tl")).unwrap();
    assert!(ci.errors.is_empty(), "{:?}", ci.errors);
    assert!(
        !ci.lints.iter().any(|l| l.contains("struct-fields")),
        "nothing is missing, so nothing is said: {:?}",
        ci.lints
    );
}

// ------------------------------------------------------------------ the run boundary

/// The resolver type-checks a module's source before handing the value over, so the
/// record's closedness is a wall at `require` as well as at `htl check`. These two say
/// the wall is where the marker puts it and not somewhere else.
fn resolver_dir(name: &str, defs: &str) -> PathBuf {
    let dir = scratch(name);
    write(&dir.join("defs.tl"), defs);
    write(
        &dir.join("three.tl"),
        "local defs = require(\"defs\")\nlocal m: defs.Mod = { name = \"three\", hp = 3, extra = \
         \"not declared\" }\nreturn m\n",
    );
    dir
}

fn resolved(dir: &Path) -> mlua::Result<i64> {
    let h = Htl::new().unwrap();
    let mut reg = Registry::new();
    reg.add(TealResolver::new(dir).unwrap().expect_type("defs.Mod"));
    reg.install(h.lua()).unwrap();
    h.lua().load("return require('three').hp").eval::<i64>()
}

#[test]
fn a_mod_carrying_an_extra_key_resolves_through_the_resolver() {
    let hp = resolved(&resolver_dir("run-open", OPEN)).unwrap();
    assert_eq!(hp, 3);
}

#[test]
fn the_same_mod_against_an_unmarked_record_fails_at_require() {
    let err = resolved(&resolver_dir("run-closed", CLOSED)).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("unknown field extra"), "{msg}");
}
