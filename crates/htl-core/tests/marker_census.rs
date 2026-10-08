//! The marker census (`CheckInfo::markers`): what `htl adopt` (#304) counts to say how
//! much of a project already writes one of htl's nine markers, before it offers to add
//! one more broadly. Step 1 is the counting; nothing here reads `applicable` yet.

use htl_core::{Htl, MarkerSite};
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-marker-census", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn markers_of(dir: &Path, file: &str) -> Vec<MarkerSite> {
    let h = Htl::new().unwrap();
    h.add_path(dir).unwrap();
    let ci = h.check(&dir.join(file)).unwrap();
    assert!(ci.ok(), "unexpected type errors: {:?}", ci.errors);
    ci.markers
}

fn site(marker: &str, kind: &str, line: usize, name: &str) -> MarkerSite {
    MarkerSite {
        marker: marker.to_string(),
        kind: kind.to_string(),
        line,
        name: name.to_string(),
    }
}

/// One of each: a record carrying `struct`/`sealed`/`extensible`/`contract`, a field
/// carrying `optional`/`required`, a function carrying `nilable`/`async`, and a record
/// field whose own type is a function carrying `noyield`. `R1`/`R2`/`R5` and `f1`/`f2`
/// are otherwise unused, which the checker says as a warning and not as an error — `ok()`
/// answers about `errors` alone.
#[test]
fn a_file_carrying_each_of_the_nine_markers_once_is_counted_exactly() {
    let dir = scratch("nine");
    write(
        &dir.join("nine.tl"),
        "local record R1   ---@contract\n   a: string   ---@required\nend\n\n\
         local record R2   ---@struct\n   b: string   ---@optional\nend\n\n\
         local record R3   ---@sealed\nend\n\n\
         local record R4   ---@extensible\nend\n\n\
         ---@nilable\nlocal function f1(): string\n   return \"x\"\nend\n\n\
         ---@async\nlocal function f2(): string\n   return \"y\"\nend\n\n\
         local record R5\n   cb: function(string)   ---@noyield(f)\nend\n\n\
         return {}\n",
    );
    let markers = markers_of(&dir, "nine.tl");
    assert_eq!(
        markers,
        vec![
            site("contract", "record", 1, "R1"),
            site("required", "field", 2, "R1.a"),
            site("struct", "record", 5, "R2"),
            site("optional", "field", 6, "R2.b"),
            site("sealed", "record", 9, "R3"),
            site("extensible", "record", 12, "R4"),
            site("nilable", "function", 16, "f1"),
            site("async", "function", 21, "f2"),
            site("noyield", "field", 26, "R5.cb"),
        ],
        "{markers:#?}"
    );
}

/// Two markers on the one line above the declaration (`has_marker`'s own position rule:
/// the line itself, or the line above when that line is nothing but markers) give two
/// entries at the record's own line.
#[test]
fn two_markers_on_the_line_above_give_two_entries_at_the_same_line() {
    let dir = scratch("two-above");
    write(
        &dir.join("two.tl"),
        "---@struct ---@sealed\nlocal record Foo\nend\nreturn Foo\n",
    );
    let markers = markers_of(&dir, "two.tl");
    assert_eq!(
        markers,
        vec![
            site("sealed", "record", 2, "Foo"),
            site("struct", "record", 2, "Foo"),
        ],
        "{markers:#?}"
    );
}

/// A record field whose own type is a record is a declaration too, the way
/// `record_meta_walk` (lint.lua, `class-record`) descends for a metamethod; its name is
/// qualified by the record that holds it.
#[test]
fn a_nested_record_is_counted_under_its_qualified_name() {
    let dir = scratch("nested");
    write(
        &dir.join("nested.tl"),
        "local record Outer\n   ---@struct\n   record Inner\n      x: string\n   end\nend\nreturn Outer\n",
    );
    let markers = markers_of(&dir, "nested.tl");
    assert_eq!(
        markers,
        vec![site("struct", "record", 3, "Outer.Inner")],
        "{markers:#?}"
    );
}

/// `Outer` declares its own field `x`, and nested `Inner` declares a field of the same
/// name, marked `---@optional`. Reading each field's own position off the type the checker
/// built, rather than scanning `name: type` lines in source order, keeps `Outer.x`'s marker
/// lookup on its own line even though `Inner.x`'s line follows it and shares the name: a
/// plain text scan, unguarded against a nested body, would find `Inner.x` after `Outer.x`
/// and read `Outer.x`'s marker from `Inner.x`'s line instead, crediting it with a marker
/// `Outer.x` never carries. Only `Inner.x`'s own entry is counted.
#[test]
fn a_nested_records_field_is_not_credited_to_the_outer_field_of_the_same_name() {
    let dir = scratch("collide");
    write(
        &dir.join("collide.tl"),
        "local record Outer\n   x: integer\n   record Inner\n      x: integer   ---@optional\n   end\nend\nreturn Outer\n",
    );
    let markers = markers_of(&dir, "collide.tl");
    assert_eq!(
        markers,
        vec![site("optional", "field", 4, "Outer.Inner.x")],
        "{markers:#?}"
    );
}

/// As the test above, with the nested record declared `type Inner = record ... end`
/// instead of a plain `record Inner ... end`: the checker lists `Q.Inner`'s own type the
/// same way either spelling, and `Q`'s own `x` is still not overwritten by `Inner`'s `x`.
#[test]
fn a_nested_type_alias_records_field_is_not_credited_to_the_outer_field_of_the_same_name() {
    let dir = scratch("collide-alias");
    write(
        &dir.join("collide_alias.tl"),
        "local record Q\n   x: integer\n   type Inner = record\n      x: integer   ---@optional\n   end\nend\nreturn Q\n",
    );
    let markers = markers_of(&dir, "collide_alias.tl");
    assert_eq!(
        markers,
        vec![site("optional", "field", 4, "Q.Inner.x")],
        "{markers:#?}"
    );
}

/// A record with a one-line nested `record Empty end`, a one-line `enum Color "red"
/// "blue" end`, and a field literally named `record` (`record` is an ordinary identifier
/// outside a type position, not a reserved word) -- a source scan that tracked nesting by
/// matching `record` / `enum` / `interface` lines against `end` lines would lose every
/// field after the first of these: a one-liner opens and closes a nested body on the same
/// line, so its depth counter would only ever see the open, and a field named `record`
/// would open a nesting level that never closes. Reading each field's own position off the
/// type the checker built, rather than scanning the body's text, has no such depth to
/// lose: only `z`, the
/// last field, carries a marker, and it is still found.
#[test]
fn fields_after_a_one_line_nested_record_enum_or_keyword_named_field_are_still_counted() {
    let dir = scratch("one-liners");
    write(
        &dir.join("one_liners.tl"),
        "local record R\n   record Empty end\n   enum Color \"red\" \"blue\" end\n   record: integer\n   z: integer   ---@optional\nend\nreturn R\n",
    );
    let markers = markers_of(&dir, "one_liners.tl");
    assert_eq!(
        markers,
        vec![site("optional", "field", 5, "R.z")],
        "{markers:#?}"
    );
}

/// A nested generic record (`record Box<T>`, `---@struct`) and a generic function-typed
/// field (`cb: function<T>(T)`, `---@nilable`) declared in the same body: the generic
/// unwrap `record_marker_sites` already does for the record itself (`local record G<T>`)
/// has to run on a field's type too, or both go uncounted. The function field is also the
/// one shape where the field's own type (`ft`) and its unwrapped inner type disagree on
/// `y` -- `parse_function_type` (tl.lua) stamps the `"generic"` wrapper it builds around an
/// inline `function<T>(...)` type from whatever token follows the signature (here, `N`'s
/// closing `end`, the comment being the last thing on `cb`'s own line), not from where the
/// field starts; the unwrapped `function` node's own `y` is stamped at the front of the
/// parse and is the field's line. Reading the unwrapped position is what finds `---@nilable`
/// here at all.
#[test]
fn a_nested_generic_record_and_a_generic_function_field_are_counted() {
    let dir = scratch("generic-nested");
    write(
        &dir.join("generic_nested.tl"),
        "local record N\n   record Box<T>   ---@struct\n      v: T\n   end\n   cb: function<T>(T)   ---@nilable\nend\nreturn N\n",
    );
    let markers = markers_of(&dir, "generic_nested.tl");
    assert_eq!(
        markers,
        vec![
            site("struct", "record", 2, "N.Box"),
            site("nilable", "field", 5, "N.cb"),
        ],
        "{markers:#?}"
    );
}

/// `function M.find()` / `function M:f()` is a `record_function` -- counted once by the
/// AST walk, `kind = "function"` -- and Teal also lists it in `M`'s own `fields`, with no
/// `name:` line of its own in the record body. The function-typed-field branch of
/// `record_marker_sites` skips it there: `is_record_function` (tl.lua) marks the field
/// Teal added for it, and the branch reads that flag instead of crediting the same marker
/// on the same declaration a second time, as `kind = "field"`.
#[test]
fn a_module_function_is_counted_once_as_a_function() {
    let dir = scratch("module-fn");
    write(
        &dir.join("module_fn.tl"),
        "local record M\nend\n\n\
         ---@nilable\nfunction M.find(): string\n   return \"x\"\nend\n\n\
         ---@noyield(g)\nfunction M:run(): string\n   return \"y\"\nend\n\n\
         return M\n",
    );
    let markers = markers_of(&dir, "module_fn.tl");
    assert_eq!(
        markers,
        vec![
            site("nilable", "function", 5, "M.find"),
            site("noyield", "function", 10, "M:run"),
        ],
        "{markers:#?}"
    );
}

/// A function-typed field written in the record's own body (`cb: function(string)`, not
/// `function M.f()`) has a `name:` line of its own: it is a field, and only this one, not
/// a `record_function` the AST walk also counts.
#[test]
fn a_function_typed_field_declared_in_the_body_is_a_field() {
    let dir = scratch("field-fn");
    write(
        &dir.join("field_fn.tl"),
        "local record M\n   cb: function(string)   ---@nilable\nend\nreturn M\n",
    );
    let markers = markers_of(&dir, "field_fn.tl");
    assert_eq!(
        markers,
        vec![site("nilable", "field", 2, "M.cb")],
        "{markers:#?}"
    );
}

/// A generic record (`local record G<T>`) wraps the record in a "generic" type whose own
/// `typename` is `"generic"`, not `"record"` -- `record_marker_sites` unwraps it the same
/// way `struct_spec` does, so a generic record's own markers are still counted.
#[test]
fn a_generic_record_carrying_a_marker_is_counted() {
    let dir = scratch("generic");
    write(
        &dir.join("generic.tl"),
        "local record G<T>   ---@struct\n   v: T\nend\nreturn {}\n",
    );
    let markers = markers_of(&dir, "generic.tl");
    assert_eq!(
        markers,
        vec![site("struct", "record", 1, "G")],
        "{markers:#?}"
    );
}

/// Two declarations on the one line a marker covers tie on `(y, marker)` --
/// `table.sort`'s own instability is otherwise free to put them in either order from one
/// run to the next. Written `b` before `a` in the source so a sort that stopped at
/// `(y, marker)` would still show `b` first; ordering by `name` next keeps `a` first
/// regardless.
#[test]
fn two_declarations_on_one_line_are_ordered_by_name() {
    let dir = scratch("tie");
    write(
        &dir.join("tie.tl"),
        "---@nilable\nlocal function b() end local function a() end\nreturn {}\n",
    );
    let markers = markers_of(&dir, "tie.tl");
    assert_eq!(
        markers,
        vec![
            site("nilable", "function", 2, "a"),
            site("nilable", "function", 2, "b"),
        ],
        "{markers:#?}"
    );
}

/// `---@noyield` on a function *declaration* (`function M.f()`), not only on a function-
/// typed field (the nine-marker test above covers the field case): the nine-marker test's
/// own fixture puts `noyield` on `R5.cb`, so this is the sibling that puts it on a plain
/// declaration instead.
#[test]
fn a_noyield_marker_on_a_function_declaration_is_counted() {
    let dir = scratch("noyield-fn");
    write(
        &dir.join("noyield_fn.tl"),
        "---@noyield(gate)\nlocal function f(): string\n   return \"x\"\nend\nreturn {}\n",
    );
    let markers = markers_of(&dir, "noyield_fn.tl");
    assert_eq!(
        markers,
        vec![site("noyield", "function", 2, "f")],
        "{markers:#?}"
    );
}

/// No marker anywhere, so nothing to count.
#[test]
fn an_unmarked_file_gives_an_empty_census() {
    let dir = scratch("unmarked");
    write(&dir.join("plain.tl"), "local record Foo\nend\nreturn Foo\n");
    assert!(markers_of(&dir, "plain.tl").is_empty());
}

/// `htl dts` writes `---@async` / `---@noyield` into a declaration itself, so a `.d.tl`'s
/// own markers are not the project's authors' writing: the census excludes it.
#[test]
fn a_dtl_file_gives_an_empty_census_even_with_markers_written() {
    let dir = scratch("dtl");
    write(
        &dir.join("lib.d.tl"),
        "---@async\nlocal function f(): string\n   return \"x\"\nend\nreturn f\n",
    );
    assert!(markers_of(&dir, "lib.d.tl").is_empty());
}

/// Two lines above the declaration, with a blank line between: `marker_on` reads only the
/// line itself or the one line directly above, so this is not read as the declaration's.
#[test]
fn a_marker_two_lines_above_its_declaration_is_not_counted() {
    let dir = scratch("too-far");
    write(
        &dir.join("far.tl"),
        "---@struct\n\nlocal record Foo\nend\nreturn Foo\n",
    );
    assert!(markers_of(&dir, "far.tl").is_empty());
}

/// `project::check` twice on the same directory, the store enabled: the second run
/// replays every file from the store rather than checking it, and the census it hands
/// back is the one the first run's check produced — whole, because it is built either
/// way, the same mechanism `Report::requires` already relies on.
#[test]
fn a_replayed_module_carries_the_census_its_check_produced() {
    use htl_core::cache;
    use htl_core::project;

    let dir = scratch("replay");
    let file = dir.join("lib.tl");
    write(
        &file,
        "---@async\nlocal function f(): string\n   return \"x\"\nend\nreturn f\n",
    );
    let paths = [dir.clone()];
    let config = None;
    let opts = |cache: cache::Options| project::Options {
        paths: &paths,
        config: &config,
        model: None,
        lint: None,
        cache,
    };

    let mut sink1 = project::Sink::new(project::Collect::default());
    let rep1 = project::check(
        &mut sink1,
        std::slice::from_ref(&file),
        &opts(cache::Options::default()),
    )
    .expect("first run checks");
    assert_eq!(rep1.replayed, 0, "nothing in the store yet: {rep1:?}");

    let mut sink2 = project::Sink::new(project::Collect::default());
    let rep2 = project::check(
        &mut sink2,
        std::slice::from_ref(&file),
        &opts(cache::Options::default()),
    )
    .expect("second run replays");
    assert_eq!(
        rep2.replayed,
        rep2.files.len(),
        "the store had the one file: {rep2:?}"
    );
    assert_eq!(
        rep2.markers, rep1.markers,
        "the replayed census is the one the check produced"
    );
    assert!(
        rep1.markers.iter().any(|(_, sites)| !sites.is_empty()),
        "the fixture wrote a marker, so there is something to replay: {rep1:?}"
    );
}

/// An interface is not a record -- `record_marker_sites` already keeps a bare `interface`
/// declaration out by its `typename` -- and `expand_interfaces` (tl.lua) copies its field
/// into every record that `is` it, keeping the interface's own `f`/`y`/`x` on the copy.
/// Checking a copy's `(f, y, x)` against `t.interface_list`'s own entries is what keeps
/// `A.ix` and `B.ix` from being credited with a marker neither record wrote.
#[test]
fn an_interface_field_is_not_counted_for_the_records_that_implement_it() {
    let dir = scratch("iface-copy");
    write(
        &dir.join("iface_copy.tl"),
        "local interface I\n   ix: integer   ---@optional\nend\n\n\
         local record A is I\nend\n\n\
         local record B is I\nend\n\n\
         return { A = A, B = B }\n",
    );
    assert!(markers_of(&dir, "iface_copy.tl").is_empty());
}

/// `shape.tl` declares `Named` with `label` on its own line 3; `c.tl` happens to carry an
/// unrelated `---@optional` on *its* line 3 -- a real field of another record, not
/// `shape.Named`'s copy. Reading a copied field's line against the file being checked,
/// rather than the field's own file, would find that coincidence and credit `B.label`; the
/// field's own file (`shape.tl`) is what keeps it out, so only `Other.x` -- the field
/// actually on `c.tl`'s line 3 -- is counted.
#[test]
fn a_field_copied_from_another_files_interface_is_not_read_against_this_files_lines() {
    let dir = scratch("iface-cross-file");
    write(
        &dir.join("shape.tl"),
        "local interface Named\n\n   label: string\nend\nreturn { Named = Named }\n",
    );
    write(
        &dir.join("c.tl"),
        "local shape = require(\"shape\")\n\
         local record Other\n   x: string   ---@optional\nend\n\n\
         local record B is shape.Named\nend\n\n\
         return { Other = Other, B = B }\n",
    );
    let markers = markers_of(&dir, "c.tl");
    assert_eq!(
        markers,
        vec![site("optional", "field", 3, "Other.x")],
        "{markers:#?}"
    );
}

/// `A` carries its own field (`own`) beside the one `I` contributes (`ix`): only the
/// record's own declaration is counted, the interface's copy is not.
#[test]
fn a_records_own_fields_beside_an_interface_are_still_counted() {
    let dir = scratch("iface-own-field");
    write(
        &dir.join("iface_own.tl"),
        "local interface I\n   ix: integer   ---@optional\nend\n\n\
         local record A is I\n   own: integer   ---@optional\nend\n\n\
         return A\n",
    );
    let markers = markers_of(&dir, "iface_own.tl");
    assert_eq!(
        markers,
        vec![site("optional", "field", 6, "A.own")],
        "{markers:#?}"
    );
}

/// Rule 2 (the range of the `local_type` / `global_type` statement that declares it)
/// holds for `global_type` the same as `local_type`: `verify_end` (tl.lua) stamps `yend`
/// on the `newtype` node for `global record ... end` the same way it does for `local
/// record ... end`.
#[test]
fn a_global_records_field_is_still_counted() {
    let dir = scratch("global-record");
    write(
        &dir.join("global_record.tl"),
        "global record G\n   v: integer   ---@optional\nend\n\nreturn {}\n",
    );
    let markers = markers_of(&dir, "global_record.tl");
    assert_eq!(
        markers,
        vec![site("optional", "field", 2, "G.v")],
        "{markers:#?}"
    );
}

/// `multi:` on one line, its type (and the trailing marker) on the next: `struct_spec`
/// reads a field's marker from its *name*'s line (`field_lines`), not its type's, so a
/// census that counted this from the type's line would claim a marker `struct_spec` could
/// not act on. Not counted at all.
#[test]
fn a_field_whose_type_wraps_to_the_next_line_is_not_counted() {
    let dir = scratch("wrapped-field");
    write(
        &dir.join("wrapped.tl"),
        "local record R\n   multi:\n   integer   ---@optional\nend\n\nreturn R\n",
    );
    assert!(markers_of(&dir, "wrapped.tl").is_empty());
}

/// `["quoted key"]: integer` is a field `field_lines` has no `name:` line for at all --
/// its name pattern matches a bare identifier, not a quoted key -- so `struct_spec` has
/// no line to read a marker from either, and the census does not claim one it cannot see.
/// Only the record's own `---@struct` is counted.
#[test]
fn a_field_the_struct_lint_cannot_read_is_not_counted() {
    let dir = scratch("quoted-key");
    write(
        &dir.join("quoted.tl"),
        "local record Q   ---@struct\n   [\"quoted key\"]: integer   ---@optional\nend\nreturn Q\n",
    );
    let markers = markers_of(&dir, "quoted.tl");
    assert_eq!(
        markers,
        vec![site("struct", "record", 1, "Q")],
        "{markers:#?}"
    );
}

/// `field_lines` scans every `name:` line up to the record's closing `end`, nested bodies
/// included, but keeps only the entries at the scan's own shallowest indent -- a nested
/// body's fields sit deeper than the record's own and are left out, so a field whose name
/// a later nested record's own field of the same name would otherwise shadow is held to
/// its own line regardless of which order the two are declared in. `OnlyId.id`, declared
/// before `Sub.id`, is counted at its own line (2); in `IdAfter`, the same two fields
/// declared in the other order leave `id`'s own line (5) just as reachable.
#[test]
fn a_field_shadowed_by_a_later_nested_fields_name_is_counted() {
    let after = scratch("shadow-nested-after");
    write(
        &after.join("only_id.tl"),
        "local record OnlyId\n   id: integer   ---@optional\n   record Sub\n      id: string\n   end\nend\nreturn OnlyId\n",
    );
    let markers = markers_of(&after, "only_id.tl");
    assert_eq!(
        markers,
        vec![site("optional", "field", 2, "OnlyId.id")],
        "{markers:#?}"
    );

    let before = scratch("shadow-nested-before");
    write(
        &before.join("id_after.tl"),
        "local record IdAfter\n   record Sub\n      id: string\n   end\n   id: integer   ---@optional\nend\nreturn IdAfter\n",
    );
    let markers = markers_of(&before, "id_after.tl");
    assert_eq!(
        markers,
        vec![site("optional", "field", 5, "IdAfter.id")],
        "{markers:#?}"
    );
}

/// `cb:` alone on one line, its function type and `---@nilable` on the next: unlike a
/// *plain* field wrapped the same way (`a_field_whose_type_wraps_to_the_next_line_is_not_
/// counted`, above, dropped because the field-marker gate requires `at[fname] == fy`),
/// a function-shaped field's marker is read through a different branch entirely --
/// `marker_on(lines, fy, marker)` straight off the function type's own position, with no
/// `at` lookup at all -- so `cb` is counted here, at the type's own line, where the
/// plain field is not.
#[test]
fn a_wrapped_function_fields_marker_is_read_at_the_types_line() {
    let dir = scratch("wrapped-function-field");
    write(
        &dir.join("wrapped_fn.tl"),
        "local record W\n   cb:\n   function(): integer   ---@nilable\nend\nreturn W\n",
    );
    let markers = markers_of(&dir, "wrapped_fn.tl");
    assert_eq!(
        markers,
        vec![site("nilable", "field", 3, "W.cb")],
        "{markers:#?}"
    );
}
