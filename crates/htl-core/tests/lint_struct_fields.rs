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
