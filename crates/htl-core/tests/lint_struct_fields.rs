//! `struct-fields`: a record marked `---@struct` is built whole, and the marker lives
//! where the record is declared rather than where it is built.
//!
//! The fix is a suggestion and stays one: a record field has a type and no honest zero,
//! and `field = nil` type-checks, so anything this inserted as a value would satisfy the
//! lint and ship. What it inserts refuses to check until someone replaces it.

use htl_core::Htl;
use htl_core::fix::{FixOptions, fix_file};
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-struct", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn lints_of(dir: &Path, file: &str) -> Vec<String> {
    let h = Htl::new().unwrap();
    h.add_path(dir).unwrap();
    let ci = h.check(&dir.join(file)).unwrap();
    assert!(ci.ok(), "unexpected type errors: {:?}", ci.errors);
    ci.lints
}

/// `MonsterDef` is a struct with two optional fields; `Loose` carries no marker.
fn defs(dir: &Path) {
    write(
        &dir.join("defs.tl"),
        "local record defs\n   ---@struct\n   record MonsterDef\n      id: string\n      hp: integer\n\
         \n      inflicts: string   ---@optional\n      ---@optional\n      home: string\n   end\n\n\
         \n   record Loose\n      a: string\n      b: string\n   end\nend\nreturn defs\n",
    );
}

/// A struct with a short field and a long one, for the two near-miss bounds.
fn spelling_defs(dir: &Path) {
    write(
        &dir.join("defs.tl"),
        "local record defs\n   ---@struct\n   record MonsterDef\n      id: string\n\
         \n      color: string\n      hp: integer\n      description: string\n   end\nend\nreturn defs\n",
    );
}

/// Type errors are expected here and are the point: a key the record does not declare is
/// `unknown field`, and this rule is the other half of the same mistake. So the lints are
/// read without requiring the file to check.
fn spelled(dir: &Path, literal: &str) -> Vec<String> {
    write(
        &dir.join("mod.tl"),
        &format!(
            "local defs = require(\"defs\")\nlocal m: defs.MonsterDef = {literal}\nreturn m\n"
        ),
    );
    let h = Htl::new().unwrap();
    h.add_path(dir).unwrap();
    h.check(&dir.join("mod.tl")).unwrap().lints
}

/// One edit away, so the key that was written is the answer and "mark it optional" is not.
#[test]
fn a_misspelled_key_is_named_instead_of_the_standing_advice() {
    let dir = scratch("typo");
    spelling_defs(&dir);
    let lints = spelled(
        &dir,
        "{ id = \"x\", colour = \"red\", hp = 1, description = \"d\" }",
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert!(
        lints[0].contains("is built without color (the literal sets `colour`)"),
        "{}",
        lints[0]
    );
    assert!(
        !lints[0].contains("---@optional"),
        "the advice for a typo is not to mark it optional: {}",
        lints[0]
    );
}

/// Two edits, believable in a name long enough that two is still a small share of it.
#[test]
fn a_longer_name_is_matched_two_edits_away() {
    let dir = scratch("typo-long");
    spelling_defs(&dir);
    let lints = spelled(
        &dir,
        "{ id = \"x\", color = \"red\", hp = 1, descrption = \"d\" }",
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert!(
        lints[0].contains("is built without description (the literal sets `descrption`)"),
        "{}",
        lints[0]
    );
}

/// Two letters swapped is one of the commonest ways to mistype a name, and plain
/// Levenshtein charges it two edits — outside the bound for a name this short.
#[test]
fn two_letters_swapped_is_one_edit() {
    let dir = scratch("swap");
    write(
        &dir.join("defs.tl"),
        "local record defs\n   ---@struct\n   record Tag\n      label: string\n      n: integer\n   end\nend\nreturn defs\n",
    );
    write(
        &dir.join("mod.tl"),
        "local defs = require(\"defs\")\nlocal m: defs.Tag = { lable = \"x\", n = 1 }\nreturn m\n",
    );
    let h = Htl::new().unwrap();
    h.add_path(&dir).unwrap();
    let lints = h.check(&dir.join("mod.tl")).unwrap().lints;
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert!(
        lints[0].contains("is built without label (the literal sets `lable`)"),
        "{}",
        lints[0]
    );
}

#[test]
fn a_field_simply_left_out_keeps_the_old_message() {
    let dir = scratch("plain");
    spelling_defs(&dir);
    let lints = spelled(&dir, "{ id = \"x\", hp = 1, description = \"d\" }");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert!(
        lints[0].contains("---@optional"),
        "nothing to suggest, so the advice stands: {}",
        lints[0]
    );
    assert!(!lints[0].contains("the literal sets"), "{}", lints[0]);
}

/// An extra key that is nothing like the missing one is not offered as a suggestion.
#[test]
fn an_unrelated_extra_key_is_not_offered() {
    let dir = scratch("unrelated");
    spelling_defs(&dir);
    let lints = spelled(
        &dir,
        "{ id = \"x\", hp = 1, description = \"d\", weight = 2 }",
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert!(!lints[0].contains("weight"), "{}", lints[0]);
    assert!(!lints[0].contains("the literal sets"), "{}", lints[0]);
}

#[test]
fn a_field_left_out_is_reported_where_the_record_is_built() {
    let dir = scratch("missing");
    defs(&dir);
    write(
        &dir.join("mod.tl"),
        "local defs = require(\"defs\")\nlocal m: defs.MonsterDef = { id = \"rat\" }\nreturn m\n",
    );
    let lints = lints_of(&dir, "mod.tl");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert!(
        lints[0].contains("mod.tl:2:28"),
        "at the literal: {}",
        lints[0]
    );
    assert!(
        lints[0].contains("MonsterDef is built without hp"),
        "{}",
        lints[0]
    );
    assert!(lints[0].contains("[htl struct-fields]"), "{}", lints[0]);
}

#[test]
fn the_optional_fields_may_be_absent_and_a_whole_literal_is_silent() {
    let dir = scratch("complete");
    defs(&dir);
    write(
        &dir.join("mod.tl"),
        "local defs = require(\"defs\")\nlocal m: defs.MonsterDef = { id = \"bat\", hp = 3 }\nreturn m\n",
    );
    assert!(lints_of(&dir, "mod.tl").is_empty());
}

/// A trailing `---@optional` belongs to the field it trails and to no other: the field
/// declared on the next line is still required. The own-line form (`---@optional` alone
/// on the line above) keeps working.
#[test]
fn a_trailing_marker_does_not_reach_the_next_line() {
    let dir = scratch("trailing");
    write(
        &dir.join("defs.tl"),
        "local record defs\n   record Def   ---@struct\n      a: string   ---@optional\n\
         \x20     b: string\n      ---@optional\n      c: string\n   end\nend\nreturn defs\n",
    );
    write(
        &dir.join("mod.tl"),
        "local defs = require(\"defs\")\nlocal m: defs.Def = { c = \"c\" }\nreturn m\n",
    );
    let lints = lints_of(&dir, "mod.tl");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert!(lints[0].contains("Def is built without b"), "{}", lints[0]);
    // `a` (trailing) and `c` (own line) are optional: a literal with only `b` is whole.
    write(
        &dir.join("mod.tl"),
        "local defs = require(\"defs\")\nlocal m: defs.Def = { b = \"b\" }\nreturn m\n",
    );
    assert!(lints_of(&dir, "mod.tl").is_empty());
}

/// The form a mod actually writes: records nested in an array of them.
#[test]
fn an_element_of_an_array_of_the_record_is_held_to_it() {
    let dir = scratch("array");
    defs(&dir);
    write(
        &dir.join("mod.tl"),
        "local defs = require(\"defs\")\nlocal ms: {defs.MonsterDef} = {\n   { id = \"ghost\", hp = 1 },\n   { id = \"shade\" },\n}\nreturn ms\n",
    );
    let lints = lints_of(&dir, "mod.tl");
    assert_eq!(lints.len(), 1, "only the incomplete element: {lints:?}");
    assert!(lints[0].contains("mod.tl:4:4"), "{}", lints[0]);
}

#[test]
fn a_literal_passed_as_a_typed_argument_is_held_to_it() {
    let dir = scratch("argument");
    defs(&dir);
    write(
        &dir.join("mod.tl"),
        "local defs = require(\"defs\")\nlocal function take(m: defs.MonsterDef): string\n   return m.id\nend\nreturn take({ id = \"arg\" })\n",
    );
    let lints = lints_of(&dir, "mod.tl");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert!(lints[0].contains("mod.tl:5:13"), "{}", lints[0]);
}

#[test]
fn a_record_without_the_marker_is_held_to_nothing() {
    let dir = scratch("loose");
    defs(&dir);
    write(
        &dir.join("mod.tl"),
        "local defs = require(\"defs\")\nlocal l: defs.Loose = { a = \"x\" }\nreturn l\n",
    );
    assert!(lints_of(&dir, "mod.tl").is_empty());
}

/// The marker is read from the file that declares the record, so editing that file
/// changes the verdict for every file that builds it — including within one process.
#[test]
fn marking_a_field_optional_afterwards_silences_it() {
    let dir = scratch("reread");
    defs(&dir);
    write(
        &dir.join("mod.tl"),
        "local defs = require(\"defs\")\nlocal m: defs.MonsterDef = { id = \"rat\" }\nreturn m\n",
    );
    assert_eq!(lints_of(&dir, "mod.tl").len(), 1);

    write(
        &dir.join("defs.tl"),
        "local record defs\n   ---@struct\n   record MonsterDef\n      id: string\n      hp: integer   ---@optional\n\
         \n      inflicts: string   ---@optional\n      ---@optional\n      home: string\n   end\n\n\
         \n   record Loose\n      a: string\n      b: string\n   end\nend\nreturn defs\n",
    );
    assert!(
        lints_of(&dir, "mod.tl").is_empty(),
        "the declaring file is read again, not remembered"
    );
}

// ---------------------------------------------------------------- htl fix struct-fields

/// A record whose fields are declared in an order alphabetical order would not produce,
/// with one type that is a name and one that is not.
fn order_defs(dir: &Path) {
    write(
        &dir.join("defs.tl"),
        "local record defs\n   ---@struct\n   record Node\n      zeta: string\n      alpha: integer\n\
         \n      tags: {string}\n      note: string   ---@optional\n   end\nend\nreturn defs\n",
    );
}

/// The fix of the file's one `struct-fields` lint.
fn struct_fix(dir: &Path, file: &str) -> htl_core::Fix {
    let h = Htl::new().unwrap();
    h.add_path(dir).unwrap();
    let ci = h.check(&dir.join(file)).unwrap();
    let at = ci
        .lints
        .iter()
        .position(|l| l.contains("[htl struct-fields]"))
        .unwrap_or_else(|| panic!("no struct-fields lint: {:?}", ci.lints));
    ci.lint_fixes[at]
        .clone()
        .unwrap_or_else(|| panic!("no fix on {}", ci.lints[at]))
}

fn fix_only(dir: &Path, file: &str, opts: FixOptions) -> htl_core::fix::FileOutcome {
    let h = Htl::new().unwrap();
    h.add_path(dir).unwrap();
    fix_file(
        &h,
        &dir.join(file),
        &FixOptions {
            only: vec!["struct-fields".into()],
            ..opts
        },
    )
    .unwrap()
}

/// One edit per missing field, in the order the record declares them — not the order the
/// message lists them in, which is alphabetical.
#[test]
fn the_fix_names_every_missing_field_in_declaration_order() {
    let dir = scratch("fix-order");
    order_defs(&dir);
    write(
        &dir.join("mod.tl"),
        "local defs = require(\"defs\")\nlocal n: defs.Node = {\n   zeta = \"z\",\n}\nreturn n\n",
    );
    let lints = lints_of(&dir, "mod.tl");
    assert!(
        lints[0].contains("is built without alpha, tags"),
        "the message stays alphabetical: {}",
        lints[0]
    );

    let fix = struct_fix(&dir, "mod.tl");
    assert_eq!(fix.applicability, htl_core::Applicability::Suggest);
    let texts: Vec<&str> = fix.edits.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(
        texts,
        vec![
            "   alpha = htl_fixme(\"integer\"),\n",
            "   tags = htl_fixme(\"{string}\"),\n"
        ],
        "declaration order, one edit each, indented like the entry above"
    );
    assert!(
        fix.edits.iter().all(|e| e.line == 4 && e.col == e.end_col),
        "insertions before the closing brace: {:?}",
        fix.edits
    );
}

/// The layout comes off the source: the comma the previous entry lacks goes in before its
/// trailing comment, and a single-line constructor stays on its line.
#[test]
fn the_fix_lands_where_the_entries_already_there_are() {
    let dir = scratch("fix-layout");
    order_defs(&dir);
    write(
        &dir.join("multi.tl"),
        "local defs = require(\"defs\")\nlocal n: defs.Node = {\n   zeta = \"z\"   -- the only one\n}\nreturn n\n",
    );
    let out = fix_only(&dir, "multi.tl", FixOptions::default());
    let text = out.suggested.expect("the suggestion is rendered");
    assert!(
        text.contains(
            "   zeta = \"z\",   -- the only one\n   alpha = htl_fixme(\"integer\"),\n   tags = htl_fixme(\"{string}\"),\n}"
        ),
        "the comma goes before the comment and the entries keep the indentation:\n{text}"
    );

    write(
        &dir.join("one.tl"),
        "local defs = require(\"defs\")\nlocal n: defs.Node = { zeta = \"z\" }\nreturn n\n",
    );
    let out = fix_only(&dir, "one.tl", FixOptions::default());
    let text = out.suggested.expect("the suggestion is rendered");
    assert!(
        text.contains(
            "{ zeta = \"z\", alpha = htl_fixme(\"integer\"), tags = htl_fixme(\"{string}\") }"
        ),
        "stays on the one line it was written on:\n{text}"
    );

    write(
        &dir.join("empty.tl"),
        "local defs = require(\"defs\")\nlocal n: defs.Node = {}\nreturn n\n",
    );
    let out = fix_only(&dir, "empty.tl", FixOptions::default());
    let text = out.suggested.expect("the suggestion is rendered");
    assert!(
        text.contains(
            "= { zeta = htl_fixme(\"string\"), alpha = htl_fixme(\"integer\"), tags = htl_fixme(\"{string}\") }"
        ),
        "an empty constructor is filled between its braces:\n{text}"
    );
}

/// The point of the applicability: nothing is written, by `htl fix` or by `--unsafe`, and
/// the lint is still there afterwards.
#[test]
fn the_suggestion_is_never_applied() {
    let dir = scratch("fix-skipped");
    order_defs(&dir);
    let src = "local defs = require(\"defs\")\nlocal n: defs.Node = { zeta = \"z\" }\nreturn n\n";
    write(&dir.join("mod.tl"), src);

    for opts in [
        FixOptions::default(),
        FixOptions {
            unsafe_fixes: true,
            ..Default::default()
        },
    ] {
        let out = fix_only(&dir, "mod.tl", opts);
        assert!(out.applied.is_empty(), "{:?}", out.applied);
        assert!(out.contents.is_none(), "nothing was rewritten");
        assert_eq!(src, std::fs::read_to_string(dir.join("mod.tl")).unwrap());
        assert_eq!(out.skipped.len(), 1, "{:?}", out.skipped);
        assert_eq!(out.skipped[0].rule, "struct-fields");
        assert!(
            out.skipped[0].reason.contains("suggestion only"),
            "{}",
            out.skipped[0].reason
        );
        assert_eq!(
            out.check
                .lints
                .iter()
                .filter(|l| l.contains("[htl struct-fields]"))
                .count(),
            1,
            "and it is still reported: {:?}",
            out.check.lints
        );
    }
}

/// What the suggestion inserts is refused by the checker, in every form the placeholder
/// takes. A fix that could be left as it is would satisfy the lint and ship the wrong
/// value, which is the one outcome this must not have.
#[test]
fn what_it_inserts_does_not_check() {
    let dir = scratch("fix-refused");
    order_defs(&dir);
    write(
        &dir.join("mod.tl"),
        "local defs = require(\"defs\")\nlocal n: defs.Node = { zeta = \"z\" }\nreturn n\n",
    );
    let out = fix_only(&dir, "mod.tl", FixOptions::default());
    let text = out.suggested.expect("the suggestion is rendered");
    write(&dir.join("mod.tl"), &text);

    let h = Htl::new().unwrap();
    h.add_path(&dir).unwrap();
    let ci = h.check(&dir.join("mod.tl")).unwrap();
    assert_eq!(
        ci.errors.len(),
        2,
        "one per placeholder left in place: {:?}",
        ci.errors
    );
    assert!(
        ci.errors.iter().all(|e| e.contains("htl_fixme")),
        "{:?}",
        ci.errors
    );
    assert!(
        !ci.lints.iter().any(|l| l.contains("[htl struct-fields]")),
        "the lint is answered, but the file does not check: {:?}",
        ci.lints
    );
}

/// A field the message blames on a misspelling is not also inserted: the answer there is
/// to correct the key that is there, not to add a second one beside it.
#[test]
fn a_misspelled_field_is_not_offered_as_an_insertion() {
    let dir = scratch("fix-typo");
    spelling_defs(&dir);
    write(
        &dir.join("mod.tl"),
        "local defs = require(\"defs\")\nlocal m: defs.MonsterDef = { id = \"x\", colour = \"red\", hp = 1, description = \"d\" }\nreturn m\n",
    );
    let h = Htl::new().unwrap();
    h.add_path(&dir).unwrap();
    let ci = h.check(&dir.join("mod.tl")).unwrap();
    let at = ci
        .lints
        .iter()
        .position(|l| l.contains("[htl struct-fields]"))
        .unwrap();
    assert!(ci.lints[at].contains("the literal sets `colour`"));
    assert!(
        ci.lint_fixes[at].is_none(),
        "nothing to insert: {:?}",
        ci.lint_fixes[at]
    );
}
