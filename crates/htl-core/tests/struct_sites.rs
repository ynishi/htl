//! The construction-site census (`CheckInfo::struct_sites`): every table literal built as
//! a declared record, marked `---@struct` or not, whole or not -- what `unmarked_structs`
//! groups by declaration (#304) and `htl adopt`'s `---@struct` row reads the same way.
//! Step 1 (the marker census, `CheckInfo::markers`) has its own test file,
//! `marker_census.rs`; this is its sibling for the construction-site half.

use htl_core::{Htl, StructSite};
use std::path::Path;

mod common;

fn tempdir(name: &str) -> common::TempDir {
    common::tempdir("htl-core-struct-sites", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn sites_of(dir: &Path, file: &str) -> Vec<StructSite> {
    let h = Htl::new().unwrap();
    h.add_path(dir).unwrap();
    let ci = h.check(&dir.join(file)).unwrap();
    assert!(ci.ok(), "unexpected type errors: {:?}", ci.errors);
    ci.struct_sites
}

fn site(
    record_file: &str,
    record_line: usize,
    record_name: &str,
    marked: bool,
    line: usize,
    col: usize,
    complete: bool,
) -> StructSite {
    StructSite {
        record_file: record_file.to_string(),
        record_line,
        record_name: record_name.to_string(),
        marked,
        line,
        col,
        complete,
    }
}

/// One file with four cases: a record built whole (`Whole`), a record built short
/// (`Short`), a bare table the checker infers a record type for rather than one anyone
/// declared (`t`), and a record already carrying `---@struct` (`Marked`). The inferred
/// one is not in the result at all; the other three are, in position order, each with
/// its own `complete` and `marked`.
#[test]
fn a_whole_a_short_an_untyped_and_a_marked_literal() {
    let dir = tempdir("four");
    write(
        &dir.join("sites.tl"),
        "local record Whole\n   x: integer\n   y: integer\nend\n\n\
         local record Short\n   a: string\n   b: string\nend\n\n\
         ---@struct\nlocal record Marked\n   m: integer\nend\n\n\
         local w: Whole = { x = 1, y = 2 }\n\
         local s: Short = { a = \"x\" }\n\
         local t = { z = 1 }\n\
         local mk: Marked = { m = 3 }\n\n\
         print(w, s, t, mk)\n\nreturn {}\n",
    );
    let sites = sites_of(&dir, "sites.tl");
    let f = dir.join("sites.tl").to_string_lossy().into_owned();
    assert_eq!(
        sites,
        vec![
            site(&f, 1, "Whole", false, 16, 18, true),
            site(&f, 6, "Short", false, 17, 18, false),
            site(&f, 12, "Marked", true, 19, 20, true),
        ],
        "{sites:#?}"
    );
}

/// Empty `declared` -- a record whose body adds no data field at all -- is `complete =
/// false` regardless of what the literal sets: nothing to set is not evidence that this
/// literal, or any other, sets everything.
#[test]
fn a_record_with_no_data_field_is_never_complete() {
    let dir = tempdir("empty");
    write(
        &dir.join("empty.tl"),
        "local record Empty\nend\n\nlocal e: Empty = {}\nprint(e)\nreturn {}\n",
    );
    let sites = sites_of(&dir, "empty.tl");
    let f = dir.join("empty.tl").to_string_lossy().into_owned();
    assert_eq!(sites, vec![site(&f, 1, "Empty", false, 4, 18, false)]);
}

/// A key the literal sets that the record does not declare -- a stray, in
/// `struct-fields`' own vocabulary -- does not make the site incomplete: that is a
/// question for `struct-fields` once the record is marked, not for this census. An
/// `---@extensible` record is how a stray key reaches the checker at all without an
/// `unknown field` type error getting there first.
#[test]
fn a_stray_key_does_not_make_a_site_incomplete() {
    let dir = tempdir("stray");
    write(
        &dir.join("stray.tl"),
        "local record R   ---@extensible\n   a: string\nend\n\n\
         local r: R = { a = \"x\", extra = 1 }\nprint(r)\nreturn {}\n",
    );
    let sites = sites_of(&dir, "stray.tl");
    let f = dir.join("stray.tl").to_string_lossy().into_owned();
    assert_eq!(sites, vec![site(&f, 1, "R", false, 5, 14, true)]);
}

/// No table literal anywhere: an empty census, like the marker one.
#[test]
fn a_record_with_no_construction_site_gives_an_empty_census() {
    let dir = tempdir("none");
    write(
        &dir.join("none.tl"),
        "local record R\n   a: string\nend\nreturn { R = R }\n",
    );
    assert!(sites_of(&dir, "none.tl").is_empty());
}

/// A generic record (`local record Box<T>`): `t.str` carries the generic's own
/// arguments (`"Box<T>"`), which a declaration line only ever spells up to the `<` --
/// `record_name` is the bare name (`"Box"`), and the declaration match that finds the
/// site at all has to look for that bare name rather than the full generic spelling.
#[test]
fn a_generic_record_is_named_by_its_bare_name() {
    let dir = tempdir("generic");
    write(
        &dir.join("box.tl"),
        "local record Box<T>\n   v: T\n   n: integer\nend\n\n\
         local b: Box<string> = { v = \"s\", n = 1 }\nprint(b)\nreturn {}\n",
    );
    let sites = sites_of(&dir, "box.tl");
    let f = dir.join("box.tl").to_string_lossy().into_owned();
    assert_eq!(sites, vec![site(&f, 1, "Box", false, 6, 24, true)]);
}

/// As above, with the generic declared in a required module and the literal built
/// through a module-qualified type (`geom.Box<string>`): `record_file` names `geom.tl`,
/// not the file holding the literal, and the bare-name fix still finds the site -- the
/// qualifying module name is no more a part of `t.str` than the generic's own arguments
/// are, but the declaration line this matches against is read from `geom.tl`'s own
/// lines (`source_lines(cache, t.file)`), where the bare name is all that is ever there.
#[test]
fn a_module_qualified_generic_site_is_named_by_its_bare_name() {
    let dir = tempdir("qualified-generic");
    write(
        &dir.join("geom.tl"),
        "local record geom\n   record Box<T>\n      v: T\n      n: integer\n   end\nend\n\nreturn geom\n",
    );
    write(
        &dir.join("use.tl"),
        "local geom = require(\"geom\")\n\n\
         local b: geom.Box<string> = { v = \"s\", n = 1 }\nprint(b)\nreturn {}\n",
    );
    let sites = sites_of(&dir, "use.tl");
    let f = dir.join("geom.tl").to_string_lossy().into_owned();
    assert_eq!(sites, vec![site(&f, 2, "Box", false, 3, 29, true)]);
}

/// `.d.tl` gives an empty census, like `markers`: no literal is built in a declaration
/// file either.
#[test]
fn a_dtl_file_gives_an_empty_census() {
    let dir = tempdir("dtl");
    write(
        &dir.join("lib.d.tl"),
        "global record R\n   a: string\nend\n",
    );
    assert!(sites_of(&dir, "lib.d.tl").is_empty());
}
