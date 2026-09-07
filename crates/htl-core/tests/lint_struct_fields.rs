//! `struct-fields`: a record marked `---@struct` is built whole, and the marker lives
//! where the record is declared rather than where it is built.

use htl_core::Htl;
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "htl-core-struct-{name}-{}-{}",
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
        &format!("local defs = require(\"defs\")\nlocal m: defs.MonsterDef = {literal}\nreturn m\n"),
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
    assert!(lints[0].contains("mod.tl:2:28"), "at the literal: {}", lints[0]);
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
