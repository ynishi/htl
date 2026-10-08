//! `unmarked_sealeds` and the `unmarked-sealed` project-level lint: a record declared
//! among the files a run checked, built and cast only in the file that declares it, and
//! carrying no `---@sealed` (#304).
//!
//! In the shape of `unmarked_struct.rs`: driven through `project::check` rather than a
//! bare `CheckInfo`, a `--lint` spec turns the rule on, a `Collect` sink gathers what the
//! run said, and the fixtures are real multi-file projects.

use htl_core::project;
use htl_core::{Diagnostic, cache};
use std::path::{Path, PathBuf};

mod common;

fn tempdir(name: &str) -> common::TempDir {
    common::tempdir("htl-core-unmarked-sealed", name)
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

fn sealed(diags: &[Diagnostic]) -> Vec<Diagnostic> {
    diags
        .iter()
        .filter(|d| d.rule.as_deref() == Some("unmarked-sealed"))
        .cloned()
        .collect()
}

/// `gate.Judged`, declared in `gate.tl` with `marker` appended right after its own name
/// on the declaration line (`""` for none, `"   ---@sealed"`, a trailing `-- htl:
/// allow(...)` comment), built twice and cast once — all three sites in `gate.tl`
/// itself, none elsewhere.
fn gate_file(dir: &Path, marker: &str) {
    write(
        &dir.join("src/gate.tl"),
        &format!(
            "local record gate\n   record Judged{marker}\n      verdict: string\n      \
             at: integer\n   end\nend\n\n\
             function gate.judge(v: string): gate.Judged\n   return {{ verdict = v, at = 1 }}\nend\n\n\
             function gate.mint(v: string): gate.Judged\n   return {{ verdict = v, at = 2 }}\nend\n\n\
             local x: gate.Judged = gate.judge(\"y\")\n\
             local y = x as gate.Judged\nprint(y)\n\n\
             return gate\n"
        ),
    );
}

#[test]
fn a_record_built_twice_and_cast_once_in_its_own_file_is_reported_at_its_declaration() {
    let dir = tempdir("three-sites");
    gate_file(&dir, "");
    let (diags, _) = check(&dir, &["src/gate.tl"], Some("unmarked-sealed=warn"));
    for d in &diags {
        assert_ne!(d.severity, htl_core::Severity::Error, "{diags:?}");
    }
    let found = sealed(&diags);
    assert_eq!(found.len(), 1, "{diags:?}");
    let d = &found[0];
    assert_eq!(d.file, dir.join("src/gate.tl").display().to_string());
    assert_eq!(d.line, 2, "the declaration's own line: {d:?}");
    assert_eq!(d.col, 1);
    assert_eq!(d.rule.as_deref(), Some("unmarked-sealed"));
    assert_eq!(
        d.message,
        "Judged is built and cast only in its own file (3 sites) and carries no \
         ---@sealed (mark it, and sealed-record holds every other file to it)"
    );
}

/// The same record, plus one literal built in `use.tl`: a site outside the declaring
/// file, so the record is no longer built and cast only where it is declared.
#[test]
fn a_literal_site_in_another_file_is_not_reported() {
    let dir = tempdir("literal-elsewhere");
    gate_file(&dir, "");
    write(
        &dir.join("src/use.tl"),
        "local gate = require(\"gate\")\n\n\
         local z: gate.Judged = { verdict = \"z\", at = 9 }\nprint(z)\n\nreturn {}\n",
    );
    let (diags, _) = check(
        &dir,
        &["src/gate.tl", "src/use.tl"],
        Some("unmarked-sealed=warn"),
    );
    assert!(sealed(&diags).is_empty(), "{diags:?}");
}

/// As above, with an `as` cast in `use.tl` instead of a literal: the other way to make
/// one is the other way to break condition 3 too.
#[test]
fn a_cast_site_in_another_file_is_not_reported_either() {
    let dir = tempdir("cast-elsewhere");
    gate_file(&dir, "");
    write(
        &dir.join("src/use.tl"),
        "local gate = require(\"gate\")\n\n\
         local w: any = nil\nlocal z = w as gate.Judged\nprint(z)\n\nreturn {}\n",
    );
    let (diags, _) = check(
        &dir,
        &["src/gate.tl", "src/use.tl"],
        Some("unmarked-sealed=warn"),
    );
    assert!(sealed(&diags).is_empty(), "{diags:?}");
}

/// Marked `---@sealed`: every site is still only in the declaring file, but the record
/// already carries the marker, so there is nothing to suggest.
#[test]
fn a_sealed_record_is_not_reported() {
    let dir = tempdir("marked");
    gate_file(&dir, "   ---@sealed");
    let (diags, _) = check(&dir, &["src/gate.tl"], Some("unmarked-sealed=warn"));
    assert!(sealed(&diags).is_empty(), "{diags:?}");
}

/// As above, `---@sealed(gate.judge)`: a named marker is still a marker.
#[test]
fn a_sealed_record_with_a_function_list_is_not_reported() {
    let dir = tempdir("marked-named");
    gate_file(&dir, "   ---@sealed(gate.judge)");
    let (diags, _) = check(&dir, &["src/gate.tl"], Some("unmarked-sealed=warn"));
    assert!(sealed(&diags).is_empty(), "{diags:?}");
}

/// `Judged` declared and unmarked, nobody builds or casts it: nothing to report either
/// way.
#[test]
fn a_record_with_no_site_is_not_reported() {
    let dir = tempdir("no-site");
    write(
        &dir.join("src/gate.tl"),
        "local record gate\n   record Judged\n      verdict: string\n      at: integer\n   \
         end\nend\n\nreturn gate\n",
    );
    let (diags, _) = check(&dir, &["src/gate.tl"], Some("unmarked-sealed=warn"));
    assert!(sealed(&diags).is_empty(), "{diags:?}");
}

/// `{ ... } as R` in the declaring file: the literal is never a site (it keeps its own
/// inferred type), but the cast is -- one site, the record's only one, and it is in the
/// declaring file, so the record is still a candidate.
#[test]
fn a_cast_literal_in_the_declaring_file_counts_as_its_one_site() {
    let dir = tempdir("cast-literal");
    write(
        &dir.join("src/gate.tl"),
        "local record gate\n   record Judged\n      verdict: string\n      at: integer\n   \
         end\nend\n\n\
         local y = { verdict = \"z\", at = 9 } as gate.Judged\nprint(y)\n\nreturn gate\n",
    );
    let (diags, _) = check(&dir, &["src/gate.tl"], Some("unmarked-sealed=warn"));
    let found = sealed(&diags);
    assert_eq!(found.len(), 1, "{diags:?}");
    assert_eq!(
        found[0].message,
        "Judged is built and cast only in its own file (1 site) and carries no \
         ---@sealed (mark it, and sealed-record holds every other file to it)"
    );
}

/// No `--lint` spec at all: `unmarked-sealed` is `allow` by default, so the candidate
/// case 1 builds is not reported.
#[test]
fn the_rule_is_off_by_default() {
    let dir = tempdir("default");
    gate_file(&dir, "");
    let (diags, _) = check(&dir, &["src/gate.tl"], None);
    assert!(sealed(&diags).is_empty(), "{diags:?}");
}

/// The same project, checked twice with the store enabled: the second run replays the
/// file rather than checking it, and reports the same finding -- what `requires_only`
/// carrying `record_sites` buys, the way it already does for `markers` and
/// `unmarked-struct`.
#[test]
fn sites_in_a_replayed_module_still_count() {
    let dir = tempdir("replay");
    gate_file(&dir, "");
    let files = ["src/gate.tl"];

    let (first, rep1) = check(&dir, &files, Some("unmarked-sealed=warn"));
    assert_eq!(rep1.replayed, 0, "nothing in the store yet: {rep1:?}");

    let (second, rep2) = check(&dir, &files, Some("unmarked-sealed=warn"));
    assert_eq!(
        rep2.replayed,
        rep2.files.len(),
        "the store had the file: {rep2:?}"
    );

    let (f1, f2) = (sealed(&first), sealed(&second));
    assert_eq!(f1.len(), 1, "{first:?}");
    assert_eq!(f1, f2, "the replayed run reports the same finding");
}

/// `-- htl: allow(unmarked-sealed)` on `Judged`'s own declaration line: the same
/// comment that silences any other project-layer finding silences this one too,
/// through `Lints::keep` -- case 1's fixture, with the comment added.
#[test]
fn an_allow_comment_on_the_declaration_silences_the_finding() {
    let dir = tempdir("allowed");
    gate_file(&dir, "   -- htl: allow(unmarked-sealed)");
    let (diags, _) = check(&dir, &["src/gate.tl"], Some("unmarked-sealed=warn"));
    assert!(sealed(&diags).is_empty(), "{diags:?}");
}

/// `Judged` declared in `src/x.d.tl` -- a dependency's own declaration, as far as this
/// walk is concerned -- required and built whole from `src/use.tl`, which is all this
/// run checks. Condition 3 already excludes it (the one site's container, `use.tl`,
/// does not canonicalise to the declaring file, `x.d.tl`), before condition 4 (the
/// declaring file is not among the walk's own paths either -- a `.d.tl` is never
/// walked at all, and its own census is always empty) gets a chance to -- the two can
/// never disagree for this rule (`unmarked_sealeds`'s own doc says why), so this
/// fixture does not isolate 4 on its own, only confirms the combination still reports
/// nothing. (A `.d.tl` placed under a `types/` directory would need a project model to
/// resolve through `require` at all; `src/`, beside the requirer, is what the no-model
/// search path this harness runs under actually offers, the same placement
/// `unmarked_struct.rs`'s own outside-the-walk fixture uses.)
#[test]
fn a_record_declared_outside_the_walk_is_not_reported() {
    let dir = tempdir("outside");
    write(
        &dir.join("src/x.d.tl"),
        "local record Judged\n   verdict: string\n   at: integer\nend\nreturn Judged\n",
    );
    write(
        &dir.join("src/use.tl"),
        "local Judged = require(\"x\")\n\n\
         local function mint(): Judged\n   return { verdict = \"y\", at = 1 }\nend\n\n\
         print(mint())\n\nreturn {}\n",
    );
    let (diags, _) = check(&dir, &["src/use.tl"], Some("unmarked-sealed=warn"));
    assert!(sealed(&diags).is_empty(), "{diags:?}");
}
