//! `trait-impl`: a record that `is` an interface marked `---@trait` has to define every
//! method of it or have `trait.impl` copy in the default from the interface's
//! `---@defaults` table, as a statement of the chunk itself.
//!
//! Every case is `htl check` through the binary on files, because the rule's verdict is
//! the exit code (`deny`) and what it knows of a callee and of a table it reads from the
//! checker's position report, which only a checked file has.

use std::path::Path;
use std::process::Command;

mod common;

fn tempdir(name: &str) -> common::TempDir {
    common::tempdir("htl-cli-trait-impl", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// `htl check` of the flat project in `dir`: exit code, and what it said on both streams.
fn check(dir: &Path) -> (i32, String) {
    let out = Command::new(common::htl_bin())
        .args(["check", "."])
        .current_dir(dir)
        .output()
        .unwrap();
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.code().unwrap_or(-1), said)
}

/// A flat project (the files sit beside the manifest) holding `files`.
fn project(name: &str, files: &[(&str, &str)]) -> common::TempDir {
    let dir = tempdir(name);
    write(&dir.join("htl.toml"), "[layout]\nsource = \".\"\n");
    for (file, text) in files {
        write(&dir.join(file), text);
    }
    dir
}

/// The `trait-impl` findings among `said`, one line each.
fn findings(said: &str) -> Vec<&str> {
    said.lines()
        .filter(|l| l.contains("[htl trait-impl]"))
        .collect()
}

/// The 1-based line of the first line of `src` that contains `needle`.
fn line_of(src: &str, needle: &str) -> usize {
    src.lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line with {needle:?} in\n{src}"))
        + 1
}

/// The issue's example, minus the trait marker line so the cases can vary it: `Shape`
/// with a default for `describe` and none for `area`, and `Circle` completing it.
fn example(marker: &str, tail: &str) -> String {
    format!(
        "local trait = require(\"htl.trait\")\n\
         {marker}\n\
         local interface Shape\n\
         \x20  area: function(self): number\n\
         \x20  describe: function(self): string\n\
         end\n\
         ---@defaults\n\
         local Shape_default: Shape = {{\n\
         \x20  describe = function(self: Shape): string\n\
         \x20     return (\"area=%.2f\"):format(self:area())\n\
         \x20  end,\n\
         }}\n\
         local record Circle is Shape\n\
         \x20  r: number\n\
         end\n\
         function Circle:area(): number return math.pi * self.r ^ 2 end\n\
         trait.impl(Shape_default, Circle)\n\
         {tail}"
    )
}

const SQUARE: &str = "local record Square is Shape\n   s: number\nend\n";

#[test]
fn a_complete_record_is_not_reported() {
    let dir = project("complete", &[("main.tl", &example("---@trait", ""))]);
    let (code, said) = check(&dir);
    assert!(findings(&said).is_empty(), "{said}");
    assert_eq!(code, 0, "{said}");
}

// Acceptance 6.
#[test]
fn a_record_that_defines_nothing_is_reported_at_its_declaration_and_fails_the_check() {
    let src = example("---@trait", SQUARE);
    let dir = project("square", &[("main.tl", &src)]);
    let (code, said) = check(&dir);
    let found = findings(&said);
    assert_eq!(found.len(), 1, "{said}");
    let at = line_of(&src, "local record Square");
    assert!(
        found[0].contains(&format!("main.tl:{at}:1:")),
        "{}",
        found[0]
    );
    assert!(found[0].contains("`Square`"), "{}", found[0]);
    assert!(
        found[0].contains("`area` is not defined, and `Shape` has no default for it"),
        "{}",
        found[0]
    );
    assert!(
        !found[0].contains("`Circle`"),
        "Circle is complete: {}",
        found[0]
    );
    assert_eq!(code, 1, "a finding at deny fails the run: {said}");
}

// Acceptance 7.
#[test]
fn a_default_nothing_applies_is_reported() {
    let src = example("---@trait", "").replace("trait.impl(Shape_default, Circle)\n", "");
    let dir = project("unapplied", &[("main.tl", &src)]);
    let (code, said) = check(&dir);
    let found = findings(&said);
    assert_eq!(found.len(), 1, "{said}");
    let at = line_of(&src, "local record Circle");
    assert!(
        found[0].contains(&format!("main.tl:{at}:1:")),
        "{}",
        found[0]
    );
    assert!(found[0].contains("`Circle`"), "{}", found[0]);
    assert!(
        found[0].contains(
            "`describe` is not defined, and no top-level `trait.impl` applies `Shape_default`"
        ),
        "{}",
        found[0]
    );
    assert!(
        !found[0].contains("`area`"),
        "Circle defines area: {}",
        found[0]
    );
    assert_eq!(code, 1, "{said}");
}

// Acceptance 8.
#[test]
fn the_callee_is_recognised_by_its_type_and_not_by_the_name_of_the_local() {
    let aliased = example("---@trait", "")
        .replace(
            "local trait = require(\"htl.trait\")",
            "local t = require(\"htl.trait\")",
        )
        .replace("trait.impl(", "t.impl(");
    let dir = project("alias-module", &[("main.tl", &aliased)]);
    let (code, said) = check(&dir);
    assert!(findings(&said).is_empty(), "{said}");
    assert_eq!(code, 0, "{said}");

    let bare = example("---@trait", "")
        .replace(
            "local trait = require(\"htl.trait\")",
            "local apply = require(\"htl.trait\").impl",
        )
        .replace("trait.impl(", "apply(");
    let dir = project("alias-function", &[("main.tl", &bare)]);
    let (code, said) = check(&dir);
    assert!(findings(&said).is_empty(), "{said}");
    assert_eq!(code, 0, "{said}");

    // A local that is merely called `trait`, and is not the library's, applies nothing.
    let other = example("---@trait", "")
        .replace("local trait = require(\"htl.trait\")\n", "")
        .replace(
            "trait.impl(Shape_default, Circle)\n",
            "local trait = { impl = function(_d: Shape, _t: Shape) end }\ntrait.impl(Shape_default, Circle)\n",
        );
    let dir = project("lookalike", &[("main.tl", &other)]);
    let (code, said) = check(&dir);
    let found = findings(&said);
    assert_eq!(found.len(), 1, "{said}");
    assert!(found[0].contains("`describe`"), "{}", found[0]);
    assert_eq!(code, 1, "{said}");
}

// Acceptance 8, the other half.
#[test]
fn a_call_that_is_not_a_top_level_statement_is_not_counted_and_says_why() {
    let src = example("---@trait", "").replace(
        "trait.impl(Shape_default, Circle)\n",
        "local function register()\n   trait.impl(Shape_default, Circle)\nend\nregister()\n",
    );
    let dir = project("nested-call", &[("main.tl", &src)]);
    let (code, said) = check(&dir);
    let found = findings(&said);
    assert_eq!(found.len(), 1, "{said}");
    let call = line_of(&src, "   trait.impl(");
    assert!(
        found[0].contains(&format!(
            "`trait.impl(Shape_default, Circle)` at line {call} is not counted because it is not a top-level statement"
        )),
        "{}",
        found[0]
    );
    assert_eq!(code, 1, "{said}");
}

#[test]
fn a_method_defined_inside_a_function_is_not_counted_and_says_why() {
    let src = example("---@trait", "").replace(
        "function Circle:area(): number return math.pi * self.r ^ 2 end\n",
        "local function define()\n   function Circle:area(): number return math.pi * self.r ^ 2 end\nend\ndefine()\n",
    );
    let dir = project("nested-definition", &[("main.tl", &src)]);
    let (code, said) = check(&dir);
    let found = findings(&said);
    assert_eq!(found.len(), 1, "{said}");
    assert!(
        found[0].contains("`function Circle:area` at line")
            && found[0].contains("is not counted because it is not a top-level statement"),
        "{}",
        found[0]
    );
    assert_eq!(code, 1, "{said}");
}

#[test]
fn a_method_assigned_at_the_top_level_counts_as_defined() {
    let src = example("---@trait", "").replace(
        "function Circle:area(): number return math.pi * self.r ^ 2 end\n",
        "Circle.area = function(self: Circle): number return self.r end\n",
    );
    let dir = project("assigned", &[("main.tl", &src)]);
    let (code, said) = check(&dir);
    assert!(findings(&said).is_empty(), "{said}");
    assert_eq!(code, 0, "{said}");
}

/// `Shape` and its defaults in a module of their own: a nested interface, and the table
/// exported through a field of the module record.
const SHAPES: &str = "local record shapes\n\
     \x20  ---@trait\n\
     \x20  interface Shape\n\
     \x20     area: function(self): number\n\
     \x20     describe: function(self): string\n\
     \x20  end\n\
     \x20  Shape_default: Shape\n\
     end\n\
     \n\
     ---@defaults\n\
     local Shape_default: shapes.Shape = {\n\
     \x20  describe = function(self: shapes.Shape): string\n\
     \x20     return (\"area=%.2f\"):format(self:area())\n\
     \x20  end,\n\
     }\n\
     shapes.Shape_default = Shape_default\n\
     \n\
     return shapes\n";

fn circle_using_shapes(tail: &str) -> String {
    format!(
        "local trait = require(\"htl.trait\")\n\
         local shapes = require(\"shapes\")\n\
         \n\
         local record Circle is shapes.Shape\n\
         \x20  r: number\n\
         end\n\
         function Circle:area(): number return math.pi * self.r ^ 2 end\n\
         {tail}"
    )
}

// Acceptance 9.
#[test]
fn a_trait_declared_in_another_module_is_checked_the_same() {
    let complete = circle_using_shapes("trait.impl(shapes.Shape_default, Circle)\n");
    let dir = project(
        "module-complete",
        &[("shapes.tl", SHAPES), ("main.tl", &complete)],
    );
    let (code, said) = check(&dir);
    assert!(findings(&said).is_empty(), "{said}");
    assert_eq!(code, 0, "{said}");

    // Item 6: a record that defines nothing.
    let square = format!(
        "{}local record Square is shapes.Shape\n   s: number\nend\n",
        circle_using_shapes("trait.impl(shapes.Shape_default, Circle)\n")
    );
    let dir = project(
        "module-square",
        &[("shapes.tl", SHAPES), ("main.tl", &square)],
    );
    let (code, said) = check(&dir);
    let found = findings(&said);
    assert_eq!(found.len(), 1, "{said}");
    let at = line_of(&square, "local record Square");
    assert!(
        found[0].contains(&format!("main.tl:{at}:1:")),
        "{}",
        found[0]
    );
    assert!(
        found[0].contains("`Square`")
            && found[0].contains("`area` is not defined, and `shapes.Shape` has no default for it"),
        "{}",
        found[0]
    );
    assert_eq!(code, 1, "{said}");

    // Item 7: the call removed.
    let bare = circle_using_shapes("");
    let dir = project(
        "module-unapplied",
        &[("shapes.tl", SHAPES), ("main.tl", &bare)],
    );
    let (code, said) = check(&dir);
    let found = findings(&said);
    assert_eq!(found.len(), 1, "{said}");
    assert!(
        found[0].contains("`Circle`") && found[0].contains("`describe` is not defined"),
        "{}",
        found[0]
    );
    assert_eq!(code, 1, "{said}");
}

/// Two files that each complete a record against the trait module. The second file's check
/// is served the trait module from the store rather than checking it again, and its
/// defaults table is read from the stored check, not from this check's positions.
#[test]
fn a_trait_module_used_by_two_files_gives_both_the_same_defaults() {
    let user = |name: &str| {
        format!(
            "local trait = require(\"htl.trait\")\n\
             local shapes = require(\"shapes\")\n\
             local record {name} is shapes.Shape\n\
             \x20  r: number\n\
             end\n\
             function {name}:area(): number return self.r end\n\
             trait.impl(shapes.Shape_default, {name})\n\
             return {name}\n"
        )
    };
    let dir = project(
        "two-users",
        &[
            ("shapes.tl", SHAPES),
            ("a.tl", &user("A")),
            ("b.tl", &user("B")),
        ],
    );
    let (code, said) = check(&dir);
    assert!(findings(&said).is_empty(), "{said}");
    assert_eq!(code, 0, "{said}");
}

/// A base interface declared as a type alias (`local type Base = interface`) owns the
/// methods it declares, and its defaults table is theirs.
#[test]
fn a_method_of_a_base_declared_as_a_type_alias_takes_the_bases_default() {
    let src = "local trait = require(\"htl.trait\")\n\
         local type Base = interface\n\
         \x20  area: function(self): number\n\
         \x20  describe: function(self): string\n\
         end\n\
         ---@defaults\n\
         local Base_default: Base = {\n\
         \x20  describe = function(_self: Base): string return \"base\" end,\n\
         }\n\
         ---@trait\n\
         local interface Named is Base\n\
         \x20  name: function(self): string\n\
         end\n\
         local record Cat is Named\n\
         \x20  lives: integer\n\
         end\n\
         function Cat:area(): number return 1 end\n\
         function Cat:name(): string return \"cat\" end\n\
         trait.impl(Base_default, Cat)\n\
         return Cat\n";
    let dir = project("alias-base", &[("main.tl", src)]);
    let (code, said) = check(&dir);
    assert!(findings(&said).is_empty(), "{said}");
    assert_eq!(code, 0, "{said}");
}

/// `Named is Shape` adds `name`, optionally with a defaults table of its own.
fn supertrait(shape_marker: &str, named_defaults: bool, record: &str) -> String {
    let defaults = if named_defaults {
        "---@defaults\n\
         local Named_default: Named = {\n\
         \x20  name = function(self: Named): string return \"?\" end,\n\
         }\n"
    } else {
        ""
    };
    format!(
        "local trait = require(\"htl.trait\")\n\
         {shape_marker}\n\
         local interface Shape\n\
         \x20  area: function(self): number\n\
         \x20  describe: function(self): string\n\
         end\n\
         ---@defaults\n\
         local Shape_default: Shape = {{\n\
         \x20  describe = function(self: Shape): string return \"shape\" end,\n\
         }}\n\
         ---@trait\n\
         local interface Named is Shape\n\
         \x20  name: function(self): string\n\
         end\n\
         {defaults}\
         {record}"
    )
}

// Acceptance 10.
#[test]
fn a_method_inherited_through_a_supertrait_is_required_whether_or_not_it_is_marked() {
    // Defines `name`, applies Shape's defaults, but not `area`.
    let record = "local record Cat is Named\n\
         \x20  lives: integer\n\
         end\n\
         function Cat:name(): string return \"cat\" end\n\
         trait.impl(Shape_default, Cat)\n";
    for (case, marker) in [("marked", "---@trait"), ("unmarked", "")] {
        let src = supertrait(marker, false, record);
        let dir = project(&format!("super-{case}"), &[("main.tl", &src)]);
        let (code, said) = check(&dir);
        let found = findings(&said);
        assert_eq!(found.len(), 1, "{case}: {said}");
        let at = line_of(&src, "local record Cat");
        assert!(
            found[0].contains(&format!("main.tl:{at}:1:")),
            "{}",
            found[0]
        );
        assert!(
            found[0].contains("`area` is not defined, and `Shape` has no default for it"),
            "{case}: {}",
            found[0]
        );
        assert!(
            !found[0].contains("`name`") && !found[0].contains("`describe`"),
            "{case}: only area is missing: {}",
            found[0]
        );
        assert_eq!(code, 1, "{case}: {said}");
    }
}

// Acceptance 10, the last sentence.
#[test]
fn each_interfaces_defaults_are_applied_by_their_own_call() {
    // Defines `area`, not `name`; Named has a default for `name`, and only Shape's table
    // is applied.
    let record = "local record Cat is Named\n\
         \x20  lives: integer\n\
         end\n\
         function Cat:area(): number return 1 end\n\
         trait.impl(Shape_default, Cat)\n";
    let src = supertrait("---@trait", true, record);
    let dir = project("super-defaults", &[("main.tl", &src)]);
    let (code, said) = check(&dir);
    let found = findings(&said);
    assert_eq!(found.len(), 1, "{said}");
    assert!(
        found[0].contains(
            "`name` is not defined, and no top-level `trait.impl` applies `Named_default`"
        ),
        "{}",
        found[0]
    );
    let call = line_of(&src, "trait.impl(Shape_default, Cat)");
    assert!(
        found[0].contains(&format!(
            "`trait.impl(Shape_default, Cat)` at line {call} is not counted because `Shape_default` is not the `---@defaults` table of `Named`"
        )),
        "{}",
        found[0]
    );
    assert_eq!(code, 1, "{said}");

    // Both applied: complete.
    let both = record.replace(
        "trait.impl(Shape_default, Cat)\n",
        "trait.impl(Shape_default, Cat)\ntrait.impl(Named_default, Cat)\n",
    );
    let src = supertrait("---@trait", true, &both);
    let dir = project("super-both", &[("main.tl", &src)]);
    let (code, said) = check(&dir);
    assert!(findings(&said).is_empty(), "{said}");
    assert_eq!(code, 0, "{said}");
}

// Acceptance 11.
#[test]
fn a_table_typed_as_the_trait_but_not_marked_does_not_apply_its_defaults() {
    let src = example("---@trait", "").replace(
        "trait.impl(Shape_default, Circle)\n",
        "local Other_default: Shape = {\n   describe = function(self: Shape): string return \"other\" end,\n}\n\
         trait.impl(Other_default, Circle)\n",
    );
    let dir = project("other-table", &[("main.tl", &src)]);
    let (code, said) = check(&dir);
    let found = findings(&said);
    assert_eq!(found.len(), 1, "{said}");
    assert!(found[0].contains("`describe`"), "{}", found[0]);
    assert!(
        found[0].contains(
            "is not counted because `Other_default` is not the `---@defaults` table of `Shape`"
        ),
        "{}",
        found[0]
    );
    assert_eq!(code, 1, "{said}");
}

// Acceptance 12.
#[test]
fn a_key_added_after_the_constructor_is_not_a_default_and_a_second_table_is_refused() {
    let assigned = example("---@trait", "").replace(
        "trait.impl(Shape_default, Circle)\n",
        "trait.impl(Shape_default, Circle)\nShape_default.area = function(self: Shape): number return 0 end\n",
    );
    let dir = project("late-key", &[("main.tl", &assigned)]);
    let (code, said) = check(&dir);
    let found = findings(&said);
    assert_eq!(found.len(), 1, "{said}");
    let at = line_of(&assigned, "Shape_default.area = ");
    assert!(
        found[0].contains(&format!("main.tl:{at}:1:")),
        "{}",
        found[0]
    );
    assert!(
        found[0].contains("the defaults are the keys of the table constructor"),
        "{}",
        found[0]
    );
    assert_eq!(code, 1, "{said}");

    let second = example("---@trait", "").replace(
        "local record Circle is Shape",
        "---@defaults\nlocal Second_default: Shape = {\n   describe = function(self: Shape): string return \"second\" end,\n}\nlocal record Circle is Shape",
    );
    let dir = project("second-table", &[("main.tl", &second)]);
    let (code, said) = check(&dir);
    let found = findings(&said);
    assert_eq!(found.len(), 1, "{said}");
    let at = line_of(&second, "local Second_default");
    assert!(found[0].contains(&format!("main.tl:{at}:")), "{}", found[0]);
    assert!(
        found[0].contains("`Shape` already has a `---@defaults` table, `Shape_default`"),
        "{}",
        found[0]
    );
    assert_eq!(code, 1, "{said}");
}

// Acceptance 13.
#[test]
fn an_interface_without_the_marker_is_never_checked() {
    let src = example("", SQUARE);
    let dir = project("unmarked", &[("main.tl", &src)]);
    let (code, said) = check(&dir);
    assert!(findings(&said).is_empty(), "{said}");
    assert_eq!(code, 0, "{said}");
}

#[test]
fn the_allow_comment_on_the_record_says_the_defaults_are_applied_elsewhere() {
    let src = example("---@trait", "")
        .replace("trait.impl(Shape_default, Circle)\n", "")
        .replace(
            "local record Circle is Shape",
            "local record Circle is Shape -- htl: allow(trait-impl)",
        );
    let dir = project("allowed", &[("main.tl", &src)]);
    let (code, said) = check(&dir);
    assert!(findings(&said).is_empty(), "{said}");
    assert_eq!(code, 0, "{said}");
}

// Acceptance 14.
#[test]
fn the_rule_is_listed_at_deny_and_explained() {
    let dir = tempdir("listing");
    let out = Command::new(common::htl_bin())
        .args(["check", "--list-lints"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let listing = String::from_utf8_lossy(&out.stdout).into_owned();
    let line = listing
        .lines()
        .find(|l| l.split_whitespace().next() == Some("trait-impl"))
        .unwrap_or_else(|| panic!("trait-impl is not listed:\n{listing}"));
    assert_eq!(line.split_whitespace().nth(1), Some("deny"), "{line}");

    let out = Command::new(common::htl_bin())
        .args(["check", "--explain", "trait-impl"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        text.starts_with("trait-impl  (default: deny)\n\n"),
        "{text}"
    );
    for want in [
        "---@trait",
        "---@defaults",
        "trait.impl",
        "top-level",
        "another file",
        "-- htl: allow(trait-impl)",
    ] {
        assert!(text.contains(want), "{want} missing from:\n{text}");
    }
}
