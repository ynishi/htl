//! `enum-cast` and `enum-table`: the two ends of the string -> enum boundary.
//!
//! A Teal enum is a string at run time and `as` is erased with the types, so a word from
//! a store or a person enters the enum with nothing looking at it (`enum-cast`). The
//! lookup table that replaces the cast is total only while it lists every value of the
//! enum, and nothing kept it level with one (`enum-table`).

use htl_core::Htl;
use htl_core::fix::{FixOptions, fix_file};
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-enum-boundary", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

const DEFS: &str = "local record defs\n   enum State\n      \"open\"\n      \"assigned\"\n      \"closed\"\n\
      \"missed\"\n      \"escalated\"\n      \"withdrawn\"\n   end\n\n   record Row\n      state: string\n   end\nend\n\
      return defs\n";

/// A project with `defs.State` (six values) and `defs.Row` (a row read back as strings).
fn project(name: &str) -> PathBuf {
    let dir = scratch(name);
    write(&dir.join("defs.tl"), DEFS);
    dir
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

/// Lints of a file that is expected to have type errors as well: a word that is not a
/// value of the enum is both the checker's business (in value position) and this rule's.
fn lints_lax(dir: &Path, file: &str) -> Vec<String> {
    checker(dir, "").check(&dir.join(file)).unwrap().lints
}

fn of_rule<'a>(lints: &'a [String], rule: &str) -> Vec<&'a String> {
    let tag = format!("[htl {rule}]");
    lints.iter().filter(|l| l.contains(&tag)).collect()
}

// ---------------------------------------------------------------- enum-cast

#[test]
fn cast_of_a_string_is_reported() {
    let dir = project("cast");
    write(
        &dir.join("store.tl"),
        "local defs = require(\"defs\")\n\n\
         local function stored(h: defs.Row): defs.State\n   return h.state as defs.State\nend\n\n\
         return { stored = stored }\n",
    );
    let lints = lints_of(&dir, "store.tl");
    let hits = of_rule(&lints, "enum-cast");
    assert_eq!(hits.len(), 1, "{lints:?}");
    let m = hits[0];
    assert!(m.contains("store.tl:4:"), "reported at the cast: {m}");
    assert!(
        m.contains("`as defs.State` is not checked at run time"),
        "the type as written, not the checker's bare name: {m}"
    );
    assert!(
        m.contains("{string: defs.State}"),
        "the message names the way out: {m}"
    );
}

/// Not a boundary: the value is already the enum (here narrowed from `E | nil`), and a
/// string literal is checked by the literal itself.
#[test]
fn cast_of_an_enum_or_a_literal_is_not_reported() {
    let dir = project("quiet");
    write(
        &dir.join("q.tl"),
        "local defs = require(\"defs\")\n\n\
         local function pick(x: defs.State | nil): defs.State\n   return x as defs.State\nend\n\n\
         local function first(): defs.State\n   return \"open\" as defs.State\nend\n\n\
         local function second(): defs.State\n   return (\"closed\") as defs.State\nend\n\n\
         return { pick = pick, first = first, second = second }\n",
    );
    let lints = lints_of(&dir, "q.tl");
    assert!(of_rule(&lints, "enum-cast").is_empty(), "{lints:?}");
}

#[test]
fn allow_silences_one_cast_site() {
    let dir = project("allow");
    write(
        &dir.join("two.tl"),
        "local defs = require(\"defs\")\n\n\
         local function a(h: defs.Row): defs.State\n   return h.state as defs.State   -- htl: allow(enum-cast)\nend\n\n\
         local function b(h: defs.Row): defs.State\n   return h.state as defs.State\nend\n\n\
         return { a = a, b = b }\n",
    );
    let lints = lints_of(&dir, "two.tl");
    let hits = of_rule(&lints, "enum-cast");
    assert_eq!(
        hits.len(),
        1,
        "only the site without the comment: {lints:?}"
    );
    assert!(hits[0].contains("two.tl:8:"), "{}", hits[0]);
}

#[test]
fn the_rule_can_be_turned_off() {
    let dir = project("off");
    write(
        &dir.join("store.tl"),
        "local defs = require(\"defs\")\n\n\
         local function stored(h: defs.Row): defs.State\n   return h.state as defs.State\nend\n\n\
         return { stored = stored }\n",
    );
    let ci = checker(&dir, "-enum-cast")
        .check(&dir.join("store.tl"))
        .unwrap();
    assert!(of_rule(&ci.lints, "enum-cast").is_empty(), "{:?}", ci.lints);
}

#[test]
fn both_rules_are_listed_and_on_by_default() {
    let rules = Htl::new().unwrap().lint_rules().unwrap();
    assert!(rules.iter().any(|r| r == "enum-cast"), "{rules:?}");
    assert!(rules.iter().any(|r| r == "enum-table"), "{rules:?}");
}

// ---------------------------------------------------------------- enum-table

/// `states` is the lookup table that replaces the cast, one value short.
const SHORT: &str = "local defs = require(\"defs\")\n\n\
     local states: {string: defs.State} = {\n\
     \x20  open = \"open\",\n\
     \x20  assigned = \"assigned\",\n\
     \x20  closed = \"closed\",\n\
     \x20  missed = \"missed\",\n\
     \x20  escalated = \"escalated\",\n\
     }\n\n\
     local function stored(s: string): defs.State\n   return states[s] or \"open\"\nend\n\n\
     return { stored = stored }\n";

#[test]
fn a_lookup_table_missing_a_value_is_reported() {
    let dir = project("short");
    write(&dir.join("store.tl"), SHORT);
    let lints = lints_of(&dir, "store.tl");
    let hits = of_rule(&lints, "enum-table");
    assert_eq!(hits.len(), 1, "{lints:?}");
    let m = hits[0];
    assert!(m.contains("store.tl:3:"), "at the constructor: {m}");
    assert!(
        m.contains("does not list State value(s): withdrawn"),
        "names the value that is missing: {m}"
    );
    assert!(
        m.contains("{string : defs.State}"),
        "names the type the checker gave it: {m}"
    );
}

/// A word that is not a value of the enum: the checker rejects it in value position and
/// this rule names it as a key, which is where a misspelled entry is dead weight.
#[test]
fn a_word_that_is_not_a_value_is_named() {
    let dir = project("typo");
    write(
        &dir.join("store.tl"),
        "local defs = require(\"defs\")\n\n\
         local states: {string: defs.State} = {\n\
         \x20  open = \"open\",\n\
         \x20  opne = \"opne\",\n\
         }\n\n\
         return { states = states }\n",
    );
    let lints = lints_lax(&dir, "store.tl");
    let hits = of_rule(&lints, "enum-table");
    assert_eq!(hits.len(), 1, "{lints:?}");
    let m = hits[0];
    assert!(m.contains("withdrawn"), "the missing values: {m}");
    assert!(
        m.contains("lists word(s) that are not State value(s): opne"),
        "the word that is not one: {m}"
    );
}

#[test]
fn a_complete_table_and_one_built_by_a_call_are_quiet() {
    let dir = project("full");
    write(
        &dir.join("store.tl"),
        "local defs = require(\"defs\")\n\n\
         local states: {string: defs.State} = {\n\
         \x20  open = \"open\",\n\
         \x20  assigned = \"assigned\",\n\
         \x20  closed = \"closed\",\n\
         \x20  missed = \"missed\",\n\
         \x20  escalated = \"escalated\",\n\
         \x20  withdrawn = \"withdrawn\",\n\
         }\n\n\
         local function build(): {string: defs.State}\n   return states\nend\n\n\
         local other: {string: defs.State} = build()\n\n\
         local cache: {string: defs.State} = {}\n\n\
         return { states = states, other = other, cache = cache }\n",
    );
    let lints = lints_of(&dir, "store.tl");
    assert!(of_rule(&lints, "enum-table").is_empty(), "{lints:?}");
}

/// The enum in key position (`{E: T}`) lists the enum's own words and is checked; it gets
/// no fix, since what an entry maps to cannot be invented. An array of the enum is a
/// selection — one branch's map styles, the behaviours one test walks — and is not
/// checked: on a 22k-line dogfood project every array literal of an enum was one of those.
#[test]
fn enum_keys_are_checked_and_arrays_are_not() {
    let dir = project("keys");
    write(
        &dir.join("k.tl"),
        "local defs = require(\"defs\")\n\n\
         local counts: {defs.State: integer} = {\n\
         \x20  open = 1,\n\
         }\n\n\
         local order: {defs.State} = { \"open\", \"assigned\" }\n\n\
         return { counts = counts, order = order }\n",
    );
    let lints = lints_of(&dir, "k.tl");
    let hits = of_rule(&lints, "enum-table");
    assert_eq!(hits.len(), 1, "the array is not one of them: {lints:?}");
    assert!(
        hits[0].contains("{defs.State : integer}") && hits[0].contains("withdrawn"),
        "{}",
        hits[0]
    );
    assert!(
        hits[0].contains("not something a fix can invent"),
        "the value cannot be made up: {}",
        hits[0]
    );
}

/// A map of the enum that is not the word -> value lookup (no key is a value of the enum)
/// is some other map, and none of this rule's business.
#[test]
fn a_map_whose_keys_are_not_the_enums_words_is_quiet() {
    let dir = project("aliases");
    write(
        &dir.join("a.tl"),
        "local defs = require(\"defs\")\n\n\
         local from_ui: {string: defs.State} = {\n\
         \x20  [\"is-open\"] = \"open\",\n\
         \x20  [\"is-shut\"] = \"closed\",\n\
         }\n\n\
         return { from_ui = from_ui }\n",
    );
    let lints = lints_of(&dir, "a.tl");
    assert!(of_rule(&lints, "enum-table").is_empty(), "{lints:?}");
}

/// A computed key leaves the word set unknown, so the constructor is not judged.
#[test]
fn a_computed_key_is_not_judged() {
    let dir = project("computed");
    write(
        &dir.join("c.tl"),
        "local defs = require(\"defs\")\n\n\
         local key = \"open\"\n\
         local states: {string: defs.State} = {\n\
         \x20  open = \"open\",\n\
         \x20  [key] = \"closed\",\n\
         }\n\n\
         return { states = states }\n",
    );
    let lints = lints_of(&dir, "c.tl");
    assert!(of_rule(&lints, "enum-table").is_empty(), "{lints:?}");
}

#[test]
fn allow_silences_one_table() {
    let dir = project("allow-table");
    write(
        &dir.join("store.tl"),
        &SHORT.replace(
            "local states: {string: defs.State} = {",
            "local states: {string: defs.State} = {   -- htl: allow(enum-table)",
        ),
    );
    let lints = lints_of(&dir, "store.tl");
    assert!(of_rule(&lints, "enum-table").is_empty(), "{lints:?}");
}

// ---------------------------------------------------------------- htl fix enum-table

fn fix_only(dir: &Path, file: &str, rule: &str) -> htl_core::fix::FileOutcome {
    let h = checker(dir, "");
    let opts = FixOptions {
        only: vec![rule.into()],
        ..Default::default()
    };
    fix_file(&h, &dir.join(file), &opts).unwrap()
}

#[test]
fn the_fix_fills_the_lookup_table_in() {
    let dir = project("fix");
    write(&dir.join("store.tl"), SHORT);
    let out = fix_only(&dir, "store.tl", "enum-table");
    assert_eq!(out.applied.len(), 1, "{:?}", out.skipped);
    let text = std::fs::read_to_string(dir.join("store.tl")).unwrap();
    assert!(
        text.contains("   escalated = \"escalated\",\n   withdrawn = \"withdrawn\",\n}"),
        "the entry goes in indented like the others:\n{text}"
    );
    assert!(
        of_rule(&out.check.lints, "enum-table").is_empty(),
        "the file passes afterwards: {:?}",
        out.check.lints
    );
    assert!(out.check.errors.is_empty(), "{:?}", out.check.errors);
}

/// The last entry without a trailing comma, and the single-line form: the fix reads the
/// layout off the source rather than assuming one.
#[test]
fn the_fix_adds_the_separator_it_needs() {
    let dir = project("fix-comma");
    write(
        &dir.join("multi.tl"),
        "local defs = require(\"defs\")\n\n\
         local states: {string: defs.State} = {\n\
         \x20  open = \"open\",\n\
         \x20  assigned = \"assigned\",\n\
         \x20  closed = \"closed\",\n\
         \x20  missed = \"missed\",\n\
         \x20  escalated = \"escalated\"   -- the last one\n\
         }\n\n\
         return { states = states }\n",
    );
    let out = fix_only(&dir, "multi.tl", "enum-table");
    assert_eq!(out.applied.len(), 1, "{:?}", out.skipped);
    let text = std::fs::read_to_string(dir.join("multi.tl")).unwrap();
    assert!(
        text.contains(
            "   escalated = \"escalated\",   -- the last one\n   withdrawn = \"withdrawn\",\n}"
        ),
        "the comma goes before the comment:\n{text}"
    );

    write(
        &dir.join("one.tl"),
        "local defs = require(\"defs\")\n\n\
         local few: {string: defs.State} = { open = \"open\", assigned = \"assigned\", closed = \"closed\", \
         missed = \"missed\", escalated = \"escalated\" }\n\n\
         return { few = few }\n",
    );
    let out = fix_only(&dir, "one.tl", "enum-table");
    assert_eq!(out.applied.len(), 1, "{:?}", out.skipped);
    let text = std::fs::read_to_string(dir.join("one.tl")).unwrap();
    assert!(
        text.contains("escalated = \"escalated\", withdrawn = \"withdrawn\" }"),
        "stays on the one line it was written on:\n{text}"
    );
    assert!(
        lints_of(&dir, "one.tl").is_empty(),
        "and the file passes afterwards"
    );
}

/// `{E: T}` reports and changes nothing: the value an entry maps to is not something a
/// fix can invent.
#[test]
fn the_fix_leaves_an_enum_keyed_table_alone() {
    let dir = project("fix-keys");
    write(
        &dir.join("k.tl"),
        "local defs = require(\"defs\")\n\n\
         local counts: {defs.State: integer} = {\n\
         \x20  open = 1,\n\
         }\n\n\
         return { counts = counts }\n",
    );
    let before = std::fs::read_to_string(dir.join("k.tl")).unwrap();
    let out = fix_only(&dir, "k.tl", "enum-table");
    assert!(out.applied.is_empty(), "{:?}", out.applied);
    assert!(out.contents.is_none(), "nothing was rewritten");
    assert_eq!(before, std::fs::read_to_string(dir.join("k.tl")).unwrap());
    assert_eq!(
        of_rule(&out.check.lints, "enum-table").len(),
        1,
        "and it is still reported: {:?}",
        out.check.lints
    );
}
