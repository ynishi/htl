//! `sealed-record`: a record marked `---@sealed` is built where it is declared and
//! nowhere else, and `---@sealed(gate.judge)` narrows that to the functions it names.
//!
//! The record means "this went through the check"; Teal has no private constructor to say
//! it with, so the marker says it and the rule holds the boundary — over the two ways one
//! is made, a table constructor and an `as` cast.

use htl_core::Htl;
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-sealed", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn checker(dir: &Path, spec: &str) -> Htl {
    let h = Htl::new().unwrap();
    if !spec.is_empty() {
        h.configure_lints(spec).unwrap();
    }
    h.add_path(dir).unwrap();
    h
}

fn lints_of(dir: &Path, file: &str) -> Vec<String> {
    let ci = checker(dir, "").check(&dir.join(file)).unwrap();
    assert!(ci.ok(), "unexpected type errors: {:?}", ci.errors);
    ci.lints
}

fn of_rule<'a>(lints: &'a [String], rule: &str) -> Vec<&'a String> {
    let tag = format!("[htl {rule}]");
    lints.iter().filter(|l| l.contains(&tag)).collect()
}

/// `gate` declares `Judged` (sealed to the file), `Draft` (sealed to `gate.open`), and a
/// `Note` nested inside `Judged` that carries no marker of its own. `gate.judge` and
/// `gate.open` build them; `gate.other` builds a `Draft` it is not allowed to.
const GATE: &str = "local record gate\n\
     \x20  record Judged      ---@sealed\n\
     \x20     verdict: string\n\
     \x20     at: integer\n\n\
     \x20     record Note\n\
     \x20        text: string\n\
     \x20     end\n\
     \x20  end\n\n\
     \x20  ---@sealed(gate.open)\n\
     \x20  record Draft\n\
     \x20     who: string\n\
     \x20  end\n\
     end\n\n\
     function gate.judge(v: string): gate.Judged\n\
     \x20  return { verdict = v, at = 1 }\n\
     end\n\n\
     function gate.open(w: string): gate.Draft\n\
     \x20  return { who = w }\n\
     end\n\n\
     function gate.note(t: string): gate.Judged.Note\n\
     \x20  return { text = t }\n\
     end\n";

/// The declaring file, with `extra` appended before the module's `return`.
fn write_gate(dir: &Path, extra: &str) {
    write(
        &dir.join("gate.tl"),
        &format!("{GATE}{extra}\nreturn gate\n"),
    );
}

fn project(name: &str) -> PathBuf {
    let dir = scratch(name);
    write_gate(&dir, "");
    dir
}

#[test]
fn a_constructor_in_another_file_is_reported_naming_the_record_and_the_file() {
    let dir = project("elsewhere");
    write(
        &dir.join("use.tl"),
        "local gate = require(\"gate\")\n\n\
         local function mint(): gate.Judged\n   return { verdict = \"yes\", at = 2 }\nend\n\n\
         return { mint = mint }\n",
    );
    let lints = lints_of(&dir, "use.tl");
    let hits = of_rule(&lints, "sealed-record");
    assert_eq!(hits.len(), 1, "{lints:?}");
    assert!(
        hits[0].contains("use.tl:4:11"),
        "at the literal: {}",
        hits[0]
    );
    assert!(
        hits[0].contains("`gate.Judged` is sealed: built only in gate.tl"),
        "the record as its declaration names it, and the declaring file: {}",
        hits[0]
    );
}

#[test]
fn an_as_cast_in_another_file_is_reported_too() {
    let dir = project("cast");
    write(
        &dir.join("use.tl"),
        "local gate = require(\"gate\")\n\n\
         local function mint(t: any): gate.Judged\n   return t as gate.Judged\nend\n\n\
         return { mint = mint }\n",
    );
    let lints = lints_of(&dir, "use.tl");
    let hits = of_rule(&lints, "sealed-record");
    assert_eq!(
        hits.len(),
        1,
        "the cast is the other way to make one: {hits:?}"
    );
    assert!(hits[0].contains("use.tl:4:13"), "at the cast: {}", hits[0]);
    assert!(
        hits[0].contains("`gate.Judged` is sealed: built only in gate.tl"),
        "{}",
        hits[0]
    );
}

/// The declaring file is where the record is built, so nothing in it is reported — for
/// `Judged`, which names no function. (`gate.other` below is the marker that does.)
#[test]
fn the_declaring_file_is_quiet_for_both_shapes() {
    let dir = project("declaring");
    write_gate(
        &dir,
        "\nlocal a: gate.Judged = { verdict = \"y\", at = 1 }\n\
         local b = a as gate.Judged\n\
         local _ = b\n",
    );
    let lints = lints_of(&dir, "gate.tl");
    assert!(
        of_rule(&lints, "sealed-record").is_empty(),
        "the marker is about the boundary, not the owner: {lints:?}"
    );
}

#[test]
fn allow_silences_one_site() {
    let dir = project("allow");
    write(
        &dir.join("use.tl"),
        "local gate = require(\"gate\")\n\n\
         local function one(): gate.Judged\n\
         \x20  return { verdict = \"yes\", at = 2 }   -- htl: allow(sealed-record)\nend\n\n\
         local function two(): gate.Judged\n   return { verdict = \"no\", at = 3 }\nend\n\n\
         return { one = one, two = two }\n",
    );
    let lints = lints_of(&dir, "use.tl");
    let hits = of_rule(&lints, "sealed-record");
    assert_eq!(hits.len(), 1, "only the site without the comment: {hits:?}");
    assert!(hits[0].contains("use.tl:8:"), "{}", hits[0]);
}

#[test]
fn a_named_function_may_build_it_and_another_function_in_the_same_file_may_not() {
    let dir = project("named");
    write_gate(
        &dir,
        "\nfunction gate.other(w: string): gate.Draft\n   return { who = w }\nend\n",
    );
    let lints = lints_of(&dir, "gate.tl");
    let hits = of_rule(&lints, "sealed-record");
    assert_eq!(
        hits.len(),
        1,
        "`gate.open` builds one and is not reported: {hits:?}"
    );
    assert!(
        hits[0].contains("`gate.Draft` is sealed: built only in gate.tl by gate.open"),
        "the function the marker names: {}",
        hits[0]
    );
    assert!(
        hits[0].contains(&format!("gate.tl:{}:", GATE.lines().count() + 3)),
        "at the constructor in gate.other: {}",
        hits[0]
    );
}

#[test]
fn a_named_marker_still_holds_against_other_files() {
    let dir = project("named-elsewhere");
    write(
        &dir.join("use.tl"),
        "local gate = require(\"gate\")\n\n\
         local function open(w: string): gate.Draft\n   return { who = w }\nend\n\n\
         return { open = open }\n",
    );
    let lints = lints_of(&dir, "use.tl");
    let hits = of_rule(&lints, "sealed-record");
    assert_eq!(
        hits.len(),
        1,
        "a function of the same name in another file is not the one named: {hits:?}"
    );
    assert!(hits[0].contains("by gate.open"), "{}", hits[0]);
}

/// The marker on the line above a nested record's declaration is the *enclosing* record's
/// trailing marker, and sealing the nested one by it would be an accident.
#[test]
fn a_record_nested_in_a_sealed_one_is_not_sealed() {
    let dir = project("nested");
    write(
        &dir.join("use.tl"),
        "local gate = require(\"gate\")\n\n\
         local function note(): gate.Judged.Note\n   return { text = \"fine\" }\nend\n\n\
         return { note = note }\n",
    );
    assert!(
        of_rule(&lints_of(&dir, "use.tl"), "sealed-record").is_empty(),
        "mark it too if it should be sealed"
    );
}

/// Two rules, one site: the record is built where it may not be, *and* short of a field.
#[test]
fn struct_fields_and_sealed_record_both_fire_at_one_site() {
    let dir = scratch("both");
    write(
        &dir.join("gate.tl"),
        "local record gate\n   ---@struct\n   record Judged      ---@sealed\n\
         \x20     verdict: string\n      at: integer\n   end\nend\n\nreturn gate\n",
    );
    write(
        &dir.join("use.tl"),
        "local gate = require(\"gate\")\n\n\
         local function mint(): gate.Judged\n   return { verdict = \"yes\" }\nend\n\n\
         return { mint = mint }\n",
    );
    let lints = lints_of(&dir, "use.tl");
    let sealed = of_rule(&lints, "sealed-record");
    let fields = of_rule(&lints, "struct-fields");
    assert_eq!(sealed.len(), 1, "{lints:?}");
    assert_eq!(fields.len(), 1, "{lints:?}");
    assert!(sealed[0].contains("use.tl:4:11"), "{}", sealed[0]);
    assert!(fields[0].contains("use.tl:4:11"), "{}", fields[0]);
    assert!(
        sealed[0].contains("is sealed: built only in"),
        "{}",
        sealed[0]
    );
    assert!(fields[0].contains("is built without at"), "{}", fields[0]);
}

/// On by default and silent by default: a record without the marker is nobody's business.
#[test]
fn a_record_without_the_marker_is_not_reported() {
    let dir = scratch("unmarked");
    write(
        &dir.join("gate.tl"),
        "local record gate\n   record Loose\n      a: string\n   end\nend\n\nreturn gate\n",
    );
    write(
        &dir.join("use.tl"),
        "local gate = require(\"gate\")\nlocal l: gate.Loose = { a = \"x\" }\nreturn l\n",
    );
    assert!(of_rule(&lints_of(&dir, "use.tl"), "sealed-record").is_empty());
}

#[test]
fn the_rule_can_be_turned_off() {
    let dir = project("off");
    write(
        &dir.join("use.tl"),
        "local gate = require(\"gate\")\n\n\
         local function mint(): gate.Judged\n   return { verdict = \"yes\", at = 2 }\nend\n\n\
         return { mint = mint }\n",
    );
    let ci = checker(&dir, "-sealed-record")
        .check(&dir.join("use.tl"))
        .unwrap();
    assert!(
        of_rule(&ci.lints, "sealed-record").is_empty(),
        "{:?}",
        ci.lints
    );
}

#[test]
fn the_rule_is_listed() {
    let rules = Htl::new().unwrap().lint_rules().unwrap();
    assert!(rules.iter().any(|r| r == "sealed-record"), "{rules:?}");
}
