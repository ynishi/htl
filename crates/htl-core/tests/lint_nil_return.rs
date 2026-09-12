//! `nil-return`: a function declared `---@nilable` may hand back nothing, and the one use
//! of its result that cannot be right is indexing the call directly.
//!
//! Teal types the result `T` whatever the declaration says — every type there accepts nil —
//! so the marker beside the declaration is the whole of what the checker has to go on, and
//! it travels with the declaration: a `.d.tl` in `types/`, one a crate ships, and one in
//! the project's own source all reach the rule the same way.

use htl_core::Htl;
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-nil-return", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// `types/` as well as the directory itself: that is where a project's declarations live,
/// and it is where a crate's are materialised, so it is the path the marker is read from in
/// practice. `Htl::add_path` is what a project layer does for it.
fn checker(dir: &Path, spec: &str) -> Htl {
    let h = Htl::new().unwrap();
    if !spec.is_empty() {
        h.configure_lints(spec).unwrap();
    }
    h.add_path(dir).unwrap();
    let types = dir.join("types");
    if types.is_dir() {
        h.add_path(&types).unwrap();
    }
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

/// `parent` is marked trailing, `box` on a line of its own; `next_one` is the line under a
/// marked one and `trim` carries nothing. The record `Box` is there so a field access on a
/// call has something to land on.
const DECL: &str = "local record p\n\
     \x20  record Box\n\
     \x20     n: integer\n\
     \x20  end\n\
     \x20  parent: function(string): string   ---@nilable\n\
     \x20  next_one: function(string): string\n\
     \x20  ---@nilable\n\
     \x20  box: function(string): Box\n\
     \x20  tbl: function(string): {string}   ---@nilable\n\
     \x20  fn: function(string): function(): string   ---@nilable\n\
     \x20  trim: function(string): string\n\
     end\n\
     return p\n";

fn project(name: &str) -> PathBuf {
    let dir = scratch(name);
    write(&dir.join("types/p.d.tl"), DECL);
    dir
}

fn main_tl(dir: &Path, body: &str) {
    write(
        &dir.join("main.tl"),
        &format!("local p = require(\"p\")\n{body}"),
    );
}

/// Every way a call's result is used without binding it: a method call, a field access, an
/// index and a call. All four are the shape `nil-index` reports over an index, and each is
/// reported once, at the chain rather than at the call.
#[test]
fn the_four_ways_of_chaining_on_a_nilable_call_are_reported() {
    let dir = project("chain");
    main_tl(
        &dir,
        "print(p.parent(\"a\"):upper())\n\
         print(p.box(\"a\").n)\n\
         print(p.tbl(\"a\")[1])\n\
         print(p.fn(\"a\")())\n",
    );
    let lints = lints_of(&dir, "main.tl");
    let found = of_rule(&lints, "nil-return");
    assert_eq!(found.len(), 4, "{lints:?}");
    assert!(
        found[0].contains("main.tl:2:20")
            && found[0].contains("p.parent is marked ---@nilable")
            && found[0].contains("bind it to a local and nil-check first"),
        "{}",
        found[0]
    );
    assert!(found[1].contains("p.box is marked"), "{}", found[1]);
    assert!(found[2].contains("p.tbl is marked"), "{}", found[2]);
    assert!(found[3].contains("p.fn is marked"), "{}", found[3]);
}

/// What the marker asks for, and the two other shapes that are not a chain: a call whose
/// result nobody looks at, and one handed to another function.
#[test]
fn a_bound_call_is_silent_and_so_is_one_nobody_indexes() {
    let dir = project("bound");
    main_tl(
        &dir,
        "local d = p.parent(\"a\")\n\
         if d then print(d:upper()) end\n\
         p.parent(\"a\")\n\
         print(p.parent(\"a\"))\n",
    );
    assert!(of_rule(&lints_of(&dir, "main.tl"), "nil-return").is_empty());
}

/// An unmarked declaration means "unknown", not "nilable" — the rule says nothing about a
/// function nobody marked, which is every function in every project until someone writes
/// the marker.
#[test]
fn a_function_without_the_marker_is_silent() {
    let dir = project("unmarked");
    main_tl(&dir, "print(p.trim(\"a\"):upper())\n");
    assert!(of_rule(&lints_of(&dir, "main.tl"), "nil-return").is_empty());
}

/// #209's position rule, over this marker: trailing belongs to the declaration it trails,
/// and the line above counts only when it is nothing but markers. `next_one` is declared
/// under `parent   ---@nilable`, and `box` under a marker of its own.
#[test]
fn a_trailing_marker_does_not_reach_the_declaration_below_it() {
    let dir = project("position");
    main_tl(
        &dir,
        "print(p.next_one(\"a\"):upper())\n\
         print(p.box(\"a\").n)\n",
    );
    let lints = lints_of(&dir, "main.tl");
    let found = of_rule(&lints, "nil-return");
    assert_eq!(found.len(), 1, "{lints:?}");
    assert!(found[0].contains("p.box is marked"), "{}", found[0]);
}

/// The marker is read from whichever file declares the function, so one in the project's
/// own source works like one in a `.d.tl`. The name in the message is the one the call
/// site writes.
#[test]
fn a_marked_function_in_the_checked_file_is_reported_by_its_own_name() {
    let dir = scratch("own-file");
    write(
        &dir.join("main.tl"),
        "---@nilable\n\
         local function find(k: string): string\n\
         \x20  if k == \"\" then return nil end\n\
         \x20  return k\n\
         end\n\n\
         local function keep(k: string): string\n\
         \x20  return k\n\
         end\n\n\
         print(find(\"a\"):upper())\n\
         print(keep(\"a\"):upper())\n",
    );
    let lints = lints_of(&dir, "main.tl");
    let found = of_rule(&lints, "nil-return");
    assert_eq!(found.len(), 1, "{lints:?}");
    assert!(found[0].contains("main.tl:11:16"), "{}", found[0]);
    assert!(
        found[0].contains("find is marked ---@nilable"),
        "{}",
        found[0]
    );
}

/// The marker on a record is not this rule's: it means nothing there, and reading it that
/// loosely would report every call of every field of the record.
#[test]
fn the_marker_on_a_record_reports_nothing() {
    let dir = scratch("record-marker");
    write(
        &dir.join("types/r.d.tl"),
        "local record r   ---@nilable\n\
         \x20  get: function(string): string\n\
         end\n\
         return r\n",
    );
    write(
        &dir.join("main.tl"),
        "local r = require(\"r\")\nprint(r.get(\"a\"):upper())\n",
    );
    assert!(of_rule(&lints_of(&dir, "main.tl"), "nil-return").is_empty());
}

#[test]
fn allow_silences_one_site() {
    let dir = project("allow");
    main_tl(
        &dir,
        "print(p.parent(\"a\"):upper())  -- htl: allow(nil-return)\n\
         print(p.box(\"a\").n)\n",
    );
    let lints = lints_of(&dir, "main.tl");
    let found = of_rule(&lints, "nil-return");
    assert_eq!(found.len(), 1, "{lints:?}");
    assert!(found[0].contains("p.box"), "{}", found[0]);
}

#[test]
fn the_rule_can_be_turned_off() {
    let dir = project("off");
    main_tl(&dir, "print(p.parent(\"a\"):upper())\n");
    let h = checker(&dir, "-nil-return");
    let ci = h.check(&dir.join("main.tl")).unwrap();
    assert!(
        of_rule(&ci.lints, "nil-return").is_empty(),
        "{:?}",
        ci.lints
    );
}

/// It is on by default at `warn`, so a project that says nothing gets it, and `strict`
/// promotes it like every other warning.
#[test]
fn the_rule_is_listed_and_warns_by_default() {
    let levels = htl_core::lint::rule_defaults();
    let (_, level) = levels
        .iter()
        .find(|(name, _)| *name == "nil-return")
        .unwrap_or_else(|| panic!("nil-return is not in the registry: {levels:?}"));
    assert_eq!(*level, htl_core::lint::Level::Warn);
}
