//! `unmarked_structs` and the `unmarked-struct` project-level lint: a record declared
//! among the files a run checked, built whole at every one of its construction sites,
//! and carrying no `---@struct` (#304).
//!
//! Driven through `project::check` rather than a bare `CheckInfo`, the way
//! `lint_global_redeclaration.rs` and `require_cycle.rs` call `global_redeclarations`
//! and `require_cycles` directly — this rule's own project-level test has no such
//! precedent to follow (`marker_census.rs`'s replay test is the nearest one, for a
//! census rather than a lint): a `--lint` spec turns the rule on, a `Collect` sink
//! gathers what the run said, and the fixtures are real multi-file projects.

use htl_core::project;
use htl_core::{Diagnostic, cache};
use std::path::{Path, PathBuf};

mod common;

fn tempdir(name: &str) -> common::TempDir {
    common::tempdir("htl-core-unmarked-struct", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Check every one of `files` (each relative to `dir`) in one run, `lint` as the
/// `--lint` spec (`None` leaves every rule at its default). Returns everything the run
/// said and the `Report` it produced.
fn check(dir: &Path, files: &[&str], lint: Option<&str>) -> (Vec<Diagnostic>, project::Report) {
    let paths = [dir.to_path_buf()];
    let config = None;
    let file_paths: Vec<PathBuf> = files.iter().map(|f| dir.join(f)).collect();
    let opts = project::Options {
        paths: &paths,
        config: &config,
        model: None,
        lint,
        cache: cache::Options::default(),
    };
    let mut sink = project::Sink::new(project::Collect::default());
    let report = project::check(&mut sink, &file_paths, &opts).expect("the run checks");
    (sink.out().diagnostics.clone(), report)
}

fn unmarked(diags: &[Diagnostic]) -> Vec<Diagnostic> {
    diags
        .iter()
        .filter(|d| d.rule.as_deref() == Some("unmarked-struct"))
        .cloned()
        .collect()
}

/// `Point` declared in `geom.tl`, unmarked, built whole by a typed local there and by a
/// typed argument in `use.tl`.
fn geom_and_use(dir: &Path, use_literal: &str) {
    write(
        &dir.join("src/geom.tl"),
        "local record geom\n   record Point\n      x: integer\n      y: integer\n   end\nend\n\n\
         local p: geom.Point = { x = 1, y = 2 }\nprint(p)\n\nreturn geom\n",
    );
    write(
        &dir.join("src/use.tl"),
        &format!(
            "local geom = require(\"geom\")\n\nlocal function show(p: geom.Point)\n   print(p.x, p.y)\nend\n\nshow({use_literal})\n\nreturn {{}}\n"
        ),
    );
}

#[test]
fn a_record_built_whole_at_every_site_is_reported_at_its_declaration() {
    let dir = tempdir("whole");
    geom_and_use(&dir, "{ x = 3, y = 4 }");
    let (diags, _) = check(
        &dir,
        &["src/geom.tl", "src/use.tl"],
        Some("unmarked-struct=warn"),
    );
    for d in &diags {
        assert_ne!(d.severity, htl_core::Severity::Error, "{diags:?}");
    }
    let found = unmarked(&diags);
    assert_eq!(found.len(), 1, "{diags:?}");
    let d = &found[0];
    assert_eq!(d.file, dir.join("src/geom.tl").display().to_string());
    assert_eq!(d.line, 2, "the declaration's own line: {d:?}");
    assert_eq!(d.col, 1);
    assert_eq!(d.rule.as_deref(), Some("unmarked-struct"));
    assert_eq!(
        d.message,
        "Point is built whole at all 2 construction sites and carries no ---@struct \
         (mark it, and struct-fields holds every site to it)"
    );
}

/// `use.tl` builds `{ x = 3 }` only: one site short, so the record is not reported --
/// and `struct-fields` is silent too, the record being unmarked.
#[test]
fn a_record_built_short_at_one_site_is_not_reported() {
    let dir = tempdir("short");
    geom_and_use(&dir, "{ x = 3 }");
    let (diags, _) = check(
        &dir,
        &["src/geom.tl", "src/use.tl"],
        Some("unmarked-struct=warn"),
    );
    for d in &diags {
        assert_ne!(d.severity, htl_core::Severity::Error, "{diags:?}");
    }
    assert!(unmarked(&diags).is_empty(), "{diags:?}");
    assert!(
        !diags
            .iter()
            .any(|d| d.rule.as_deref() == Some("struct-fields")),
        "unmarked, so struct-fields has nothing to say either: {diags:?}"
    );
}

/// Same fixture, `Point` marked `---@struct`: every site is still whole, so neither rule
/// says anything.
#[test]
fn a_marked_record_is_not_reported() {
    let dir = tempdir("marked");
    write(
        &dir.join("src/geom.tl"),
        "local record geom\n   ---@struct\n   record Point\n      x: integer\n      y: integer\n   end\nend\n\n\
         local p: geom.Point = { x = 1, y = 2 }\nprint(p)\n\nreturn geom\n",
    );
    write(
        &dir.join("src/use.tl"),
        "local geom = require(\"geom\")\n\nlocal function show(p: geom.Point)\n   print(p.x, p.y)\nend\n\nshow({ x = 3, y = 4 })\n\nreturn {}\n",
    );
    let (diags, _) = check(
        &dir,
        &["src/geom.tl", "src/use.tl"],
        Some("unmarked-struct=warn"),
    );
    assert!(unmarked(&diags).is_empty(), "{diags:?}");
}

/// `Point` declared and unmarked, nobody builds it: nothing to report either way.
#[test]
fn a_record_with_no_construction_site_is_not_reported() {
    let dir = tempdir("none");
    write(
        &dir.join("src/geom.tl"),
        "local record geom\n   record Point\n      x: integer\n      y: integer\n   end\nend\n\nreturn geom\n",
    );
    let (diags, _) = check(&dir, &["src/geom.tl"], Some("unmarked-struct=warn"));
    assert!(unmarked(&diags).is_empty(), "{diags:?}");
}

/// `local t = { a = 1 }` with no type anywhere: the checker infers a record type for
/// `t` itself, which is not a declared one -- no site, so nothing to report.
#[test]
fn an_inferred_literal_is_not_a_declared_record() {
    let dir = tempdir("inferred");
    write(
        &dir.join("src/plain.tl"),
        "local t = { a = 1 }\nprint(t)\nreturn {}\n",
    );
    let (diags, _) = check(&dir, &["src/plain.tl"], Some("unmarked-struct=warn"));
    assert!(unmarked(&diags).is_empty(), "{diags:?}");
}

/// `Thing` declared in `types/thing.d.tl`, required by `use.tl` and built whole there --
/// but `thing.d.tl` is not one of the files this run checks, so it is not the project's
/// to mark.
#[test]
fn a_record_declared_outside_the_walk_is_not_reported() {
    let dir = tempdir("outside");
    write(
        &dir.join("src/thing.d.tl"),
        "local record Thing\n   x: integer\n   y: integer\nend\nreturn Thing\n",
    );
    write(
        &dir.join("src/use.tl"),
        "local Thing = require(\"thing\")\n\nlocal function make(): Thing\n   return { x = 1, y = 2 }\nend\n\nprint(make())\n\nreturn {}\n",
    );
    let (diags, _) = check(&dir, &["src/use.tl"], Some("unmarked-struct=warn"));
    assert!(unmarked(&diags).is_empty(), "{diags:?}");
}

/// No `--lint` spec at all: `unmarked-struct` is `allow` by default, so the candidate
/// case 1 builds is not reported.
#[test]
fn the_rule_is_off_by_default() {
    let dir = tempdir("default");
    geom_and_use(&dir, "{ x = 3, y = 4 }");
    let (diags, _) = check(&dir, &["src/geom.tl", "src/use.tl"], None);
    assert!(unmarked(&diags).is_empty(), "{diags:?}");
}

/// The same project, checked twice with the store enabled: the second run replays every
/// file rather than checking it, and reports the same finding -- what `requires_only`
/// carrying `struct_sites` buys, the way it already does for `markers`.
#[test]
fn sites_in_a_replayed_module_still_count() {
    let dir = tempdir("replay");
    geom_and_use(&dir, "{ x = 3, y = 4 }");
    let files = ["src/geom.tl", "src/use.tl"];

    let (first, rep1) = check(&dir, &files, Some("unmarked-struct=warn"));
    assert_eq!(rep1.replayed, 0, "nothing in the store yet: {rep1:?}");

    let (second, rep2) = check(&dir, &files, Some("unmarked-struct=warn"));
    assert_eq!(
        rep2.replayed,
        rep2.files.len(),
        "the store had both files: {rep2:?}"
    );

    let (f1, f2) = (unmarked(&first), unmarked(&second));
    assert_eq!(f1.len(), 1, "{first:?}");
    assert_eq!(f1, f2, "the replayed run reports the same finding");
}

/// A generic record (`local record Box<T>`), built whole at two sites with two
/// different type arguments: both resolve to the one declaration, and the finding
/// names it by its bare name (`Box`), not the generic spelling either site's own type
/// carries (`Box<string>` / `Box<integer>`).
#[test]
fn a_generic_record_is_reported_at_its_declaration_by_its_bare_name() {
    let dir = tempdir("generic");
    write(
        &dir.join("src/box.tl"),
        "local record Box<T>\n   v: T\n   n: integer\nend\n\n\
         local b1: Box<string> = { v = \"s\", n = 1 }\n\
         local b2: Box<integer> = { v = 1, n = 2 }\n\
         print(b1, b2)\n\nreturn {}\n",
    );
    let (diags, _) = check(&dir, &["src/box.tl"], Some("unmarked-struct=warn"));
    let found = unmarked(&diags);
    assert_eq!(found.len(), 1, "{diags:?}");
    let d = &found[0];
    assert_eq!(d.file, dir.join("src/box.tl").display().to_string());
    assert_eq!(d.line, 1, "the declaration's own line: {d:?}");
    assert_eq!(
        d.message,
        "Box is built whole at all 2 construction sites and carries no ---@struct \
         (mark it, and struct-fields holds every site to it)"
    );
}

/// `-- htl: allow(unmarked-struct)` on `Point`'s own declaration line: the same comment
/// that silences any other project-layer finding silences this one too, through
/// `Lints::keep` -- case 1's fixture, with the comment added.
#[test]
fn an_allow_comment_on_the_declaration_silences_the_finding() {
    let dir = tempdir("allowed");
    write(
        &dir.join("src/geom.tl"),
        "local record geom\n   record Point   -- htl: allow(unmarked-struct)\n      x: integer\n      y: integer\n   end\nend\n\n\
         local p: geom.Point = { x = 1, y = 2 }\nprint(p)\n\nreturn geom\n",
    );
    write(
        &dir.join("src/use.tl"),
        "local geom = require(\"geom\")\n\nlocal function show(p: geom.Point)\n   print(p.x, p.y)\nend\n\nshow({ x = 3, y = 4 })\n\nreturn {}\n",
    );
    let (diags, _) = check(
        &dir,
        &["src/geom.tl", "src/use.tl"],
        Some("unmarked-struct=warn"),
    );
    assert!(unmarked(&diags).is_empty(), "{diags:?}");
}

/// As above, `unmarked-struct` named second in a list-form comment
/// (`-- htl: allow(no-any, unmarked-struct)`) rather than alone.
#[test]
fn an_allow_comment_naming_several_rules_silences_the_finding_too() {
    let dir = tempdir("allowed-list");
    write(
        &dir.join("src/geom.tl"),
        "local record geom\n   record Point   -- htl: allow(no-any, unmarked-struct)\n      x: integer\n      y: integer\n   end\nend\n\n\
         local p: geom.Point = { x = 1, y = 2 }\nprint(p)\n\nreturn geom\n",
    );
    write(
        &dir.join("src/use.tl"),
        "local geom = require(\"geom\")\n\nlocal function show(p: geom.Point)\n   print(p.x, p.y)\nend\n\nshow({ x = 3, y = 4 })\n\nreturn {}\n",
    );
    let (diags, _) = check(
        &dir,
        &["src/geom.tl", "src/use.tl"],
        Some("unmarked-struct=warn"),
    );
    assert!(unmarked(&diags).is_empty(), "{diags:?}");
}

/// Two unmarked, whole-built records in two different files: `Alpha`, declared and
/// built only in `a.tl`, and `Beta`, declared in `b.tl` and built there *and* from
/// `a.tl` (`a.tl` requires `b.tl`, not the other way round, so there is no cycle). The
/// walk checks `a.tl` first, so the first `StructSite` this run sees for `Beta` is the
/// one built in `a.tl` -- spelled the way `a.tl`'s own require resolved `b.tl`, which
/// need not be the string `b.tl`'s own check names itself with. The result still groups
/// both of `Beta`'s sites under `b.tl`'s own spelling, which is what keeps the order
/// `(file, line)` sorts by one record's two files' worth of spellings would otherwise
/// scramble: `Alpha` (`a.tl`) before `Beta` (`b.tl`), not interleaved by whichever
/// spelling sorted where.
#[test]
fn several_candidates_across_two_files_are_ordered_by_their_own_declaring_file() {
    let dir = tempdir("several");
    write(
        &dir.join("src/a.tl"),
        "local b = require(\"b\")\n\n\
         local record Alpha\n   x: integer\nend\n\n\
         local a1: Alpha = { x = 1 }\n\
         local b1: b.Beta = { y = 2 }\n\
         print(a1, b1)\n\nreturn { Alpha = Alpha }\n",
    );
    write(
        &dir.join("src/b.tl"),
        "local record b\n   record Beta\n      y: integer\n   end\nend\n\n\
         local b2: b.Beta = { y = 3 }\nprint(b2)\n\nreturn b\n",
    );
    let (diags, _) = check(
        &dir,
        &["src/a.tl", "src/b.tl"],
        Some("unmarked-struct=warn"),
    );
    let found = unmarked(&diags);
    assert_eq!(found.len(), 2, "{diags:?}");
    assert_eq!(found[0].file, dir.join("src/a.tl").display().to_string());
    assert!(found[0].message.starts_with("Alpha "), "{found:?}");
    assert_eq!(found[1].file, dir.join("src/b.tl").display().to_string());
    assert!(found[1].message.starts_with("Beta "), "{found:?}");
}
