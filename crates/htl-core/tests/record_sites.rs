//! The census of record sites (`CheckInfo::record_sites`): every table literal typed as a
//! declared record, and every `as` cast to one -- marked `---@struct` or `---@sealed` or
//! neither, whole or not -- what `unmarked_structs` and `unmarked_sealeds` group by
//! declaration (#304) and `htl adopt`'s `---@struct` row reads the same way. The marker
//! census, `CheckInfo::markers`, has its own test file, `marker_census.rs`; this is its
//! sibling for the construction- and cast-site half.

use htl_core::{Htl, RecordSite};
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-record-sites", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn sites_of(dir: &Path, file: &str) -> Vec<RecordSite> {
    let h = Htl::new().unwrap();
    h.add_path(dir).unwrap();
    let ci = h.check(&dir.join(file)).unwrap();
    assert!(ci.ok(), "unexpected type errors: {:?}", ci.errors);
    ci.record_sites
}

#[allow(clippy::too_many_arguments)]
fn site(
    record_file: &str,
    record_line: usize,
    record_name: &str,
    marked: bool,
    line: usize,
    col: usize,
    kind: &str,
    sealed: bool,
    complete: bool,
) -> RecordSite {
    RecordSite {
        record_file: record_file.to_string(),
        record_line,
        record_name: record_name.to_string(),
        marked,
        line,
        col,
        kind: kind.to_string(),
        sealed,
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
    let dir = scratch("four");
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
            site(&f, 1, "Whole", false, 16, 18, "literal", false, true),
            site(&f, 6, "Short", false, 17, 18, "literal", false, false),
            site(&f, 12, "Marked", true, 19, 20, "literal", false, true),
        ],
        "{sites:#?}"
    );
}

/// Empty `declared` -- a record whose body adds no data field at all -- is `complete =
/// false` regardless of what the literal sets: nothing to set is not evidence that this
/// literal, or any other, sets everything.
#[test]
fn a_record_with_no_data_field_is_never_complete() {
    let dir = scratch("empty");
    write(
        &dir.join("empty.tl"),
        "local record Empty\nend\n\nlocal e: Empty = {}\nprint(e)\nreturn {}\n",
    );
    let sites = sites_of(&dir, "empty.tl");
    let f = dir.join("empty.tl").to_string_lossy().into_owned();
    assert_eq!(
        sites,
        vec![site(&f, 1, "Empty", false, 4, 18, "literal", false, false)]
    );
}

/// A key the literal sets that the record does not declare -- a stray, in
/// `struct-fields`' own vocabulary -- does not make the site incomplete: that is a
/// question for `struct-fields` once the record is marked, not for this census. An
/// `---@extensible` record is how a stray key reaches the checker at all without an
/// `unknown field` type error getting there first.
#[test]
fn a_stray_key_does_not_make_a_site_incomplete() {
    let dir = scratch("stray");
    write(
        &dir.join("stray.tl"),
        "local record R   ---@extensible\n   a: string\nend\n\n\
         local r: R = { a = \"x\", extra = 1 }\nprint(r)\nreturn {}\n",
    );
    let sites = sites_of(&dir, "stray.tl");
    let f = dir.join("stray.tl").to_string_lossy().into_owned();
    assert_eq!(
        sites,
        vec![site(&f, 1, "R", false, 5, 14, "literal", false, true)]
    );
}

/// No table literal anywhere: an empty census, like the marker one.
#[test]
fn a_record_with_no_construction_site_gives_an_empty_census() {
    let dir = scratch("none");
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
    let dir = scratch("generic");
    write(
        &dir.join("box.tl"),
        "local record Box<T>\n   v: T\n   n: integer\nend\n\n\
         local b: Box<string> = { v = \"s\", n = 1 }\nprint(b)\nreturn {}\n",
    );
    let sites = sites_of(&dir, "box.tl");
    let f = dir.join("box.tl").to_string_lossy().into_owned();
    assert_eq!(
        sites,
        vec![site(&f, 1, "Box", false, 6, 24, "literal", false, true)]
    );
}

/// As above, with the generic declared in a required module and the literal built
/// through a module-qualified type (`geom.Box<string>`): `record_file` names `geom.tl`,
/// not the file holding the literal, and the bare-name fix still finds the site -- the
/// qualifying module name is no more a part of `t.str` than the generic's own arguments
/// are, but the declaration line this matches against is read from `geom.tl`'s own
/// lines (`source_lines(cache, t.file)`), where the bare name is all that is ever there.
#[test]
fn a_module_qualified_generic_site_is_named_by_its_bare_name() {
    let dir = scratch("qualified-generic");
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
    assert_eq!(
        sites,
        vec![site(&f, 2, "Box", false, 3, 29, "literal", false, true)]
    );
}

/// `.d.tl` gives an empty census, like `markers`: no literal is built in a declaration
/// file either.
#[test]
fn a_dtl_file_gives_an_empty_census() {
    let dir = scratch("dtl");
    write(
        &dir.join("lib.d.tl"),
        "global record R\n   a: string\nend\n",
    );
    assert!(sites_of(&dir, "lib.d.tl").is_empty());
}

/// An `as` cast to a declared record is its own site, `kind = "cast"`, and a cast sets
/// no field at all so it is never `complete`.
#[test]
fn an_as_cast_is_a_cast_site() {
    let dir = scratch("cast");
    write(
        &dir.join("cast.tl"),
        "local record R\n   x: integer\nend\n\n\
         local v: any = nil\nlocal j = v as R\nprint(j)\nreturn {}\n",
    );
    let sites = sites_of(&dir, "cast.tl");
    let f = dir.join("cast.tl").to_string_lossy().into_owned();
    assert_eq!(
        sites,
        vec![site(&f, 1, "R", false, 6, 13, "cast", false, false)]
    );
}

/// `{ x = 1 } as R`: the literal keeps its own inferred type (`as` is erased, not a
/// hint that retypes it) and is not a site at all, the way an un-cast inferred literal
/// is not one -- the `as` node is the one and only site, a cast of `R`.
#[test]
fn a_cast_literal_gives_one_cast_site_and_no_literal_site() {
    let dir = scratch("cast-literal");
    write(
        &dir.join("cast.tl"),
        "local record R\n   x: integer\nend\n\n\
         local r = { x = 1 } as R\nprint(r)\nreturn {}\n",
    );
    let sites = sites_of(&dir, "cast.tl");
    let f = dir.join("cast.tl").to_string_lossy().into_owned();
    assert_eq!(
        sites,
        vec![site(&f, 1, "R", false, 5, 21, "cast", false, false)]
    );
}

/// A literal of a record marked `---@sealed` (no function list) is `sealed = true`.
#[test]
fn a_sealed_records_literal_is_sealed() {
    let dir = scratch("sealed");
    write(
        &dir.join("sealed.tl"),
        "local record R   ---@sealed\n   x: integer\nend\n\n\
         local r: R = { x = 1 }\nprint(r)\nreturn {}\n",
    );
    let sites = sites_of(&dir, "sealed.tl");
    let f = dir.join("sealed.tl").to_string_lossy().into_owned();
    assert_eq!(
        sites,
        vec![site(&f, 1, "R", false, 5, 14, "literal", true, true)]
    );
}

/// As above, `---@sealed(gate.judge)`: `sealed` reads the marker's truthiness, not its
/// function list, so a named marker is `sealed = true` too.
#[test]
fn a_sealed_record_with_a_function_list_is_sealed_too() {
    let dir = scratch("sealed-named");
    write(
        &dir.join("sealed.tl"),
        "local record R   ---@sealed(gate.judge)\n   x: integer\nend\n\n\
         local r: R = { x = 1 }\nprint(r)\nreturn {}\n",
    );
    let sites = sites_of(&dir, "sealed.tl");
    let f = dir.join("sealed.tl").to_string_lossy().into_owned();
    assert_eq!(
        sites,
        vec![site(&f, 1, "R", false, 5, 14, "literal", true, true)]
    );
}

/// One file, every cast shape together: a cast as a function argument (`f(x as
/// gate.Judged)`); a cast nested in a literal, through a `local type` alias (`{ j = x
/// as J }`, `local type J = gate.Judged`) -- `deref` follows the alias through to the
/// record it names, the same as it would a direct spelling; a cast to a generic
/// instantiation (`x as gate.Box<string>`, named by its bare name `Box`); a cast in a
/// `return`. Beside them, three shapes that give no site at all: `as any` (no
/// `.fields`), `as (gate.Other | string)` (a union, no `.fields` either), and `y is
/// gate.Judged` (`is`, not `as`, so the cast branch never matches it). One literal,
/// `{ v = 1 }` typed `gate.Judged`, for a fifth site and to show the two kinds still
/// sort together by position.
#[test]
fn every_cast_shape_in_one_file() {
    let dir = scratch("combo");
    write(
        &dir.join("combo.tl"),
        "local record gate\n   record Judged\n      v: integer\n   end\n\n   record Box<T>\n      item: T\n   end\n\n   record Other\n      w: integer\n   end\nend\n\nlocal type J = gate.Judged\n\nlocal function f(j: gate.Judged)\n   print(j)\nend\n\nlocal x: any = nil\nf(x as gate.Judged)\n\nlocal lit = { j = x as J }\nprint(lit)\n\nlocal b = x as gate.Box<string>\n\nlocal function make(): gate.Judged\n   return x as gate.Judged\nend\nprint(make())\n\nlocal a = x as any\nlocal u = x as (gate.Other | string)\nprint(a, u)\n\nlocal y: gate.Judged = { v = 1 }\nif y is gate.Judged then print(y) end\n\nreturn gate\n",
    );
    let sites = sites_of(&dir, "combo.tl");
    let f = dir.join("combo.tl").to_string_lossy().into_owned();
    assert_eq!(
        sites,
        vec![
            // `f(x as gate.Judged)`: the cast as a function argument.
            site(&f, 2, "Judged", false, 22, 5, "cast", false, false),
            // `{ j = x as J }`: the cast nested in a literal, through alias `J`.
            site(&f, 2, "Judged", false, 24, 21, "cast", false, false),
            // `x as gate.Box<string>`: the cast to a generic instantiation.
            site(&f, 6, "Box", false, 27, 13, "cast", false, false),
            // `return x as gate.Judged`: the cast in a return.
            site(&f, 2, "Judged", false, 30, 13, "cast", false, false),
            // `local y: gate.Judged = { v = 1 }`: the one literal, complete.
            site(&f, 2, "Judged", false, 38, 24, "literal", false, true),
        ],
        "{sites:#?}"
    );
}
