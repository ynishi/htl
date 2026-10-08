//! `unmarked-struct` and `unmarked-sealed` read every site of a record across the files
//! a run checked (`infos`), and answer a question about *all* of them ("built short
//! nowhere else", "built and cast only here"). A walk narrower than the project can see
//! only some of a record's sites, so answering from what it saw would be a guess about
//! the ones it did not -- the two rules gate on `project::walk_is_the_whole_project`
//! instead, and say nothing on a narrower walk rather than risk one. The gate is a
//! *subset* test (the project's own module set has to be all there, not exactly there):
//! a patched dependency and a file named outright outside every module root both add
//! files `htl check` walks beyond the project's own set, and neither is reason to call
//! the walk narrower -- the dedicated tests below (`a_patched_dependencys_*`,
//! `a_file_outside_every_module_root_*`) are what would have caught the gate comparing
//! by equality instead.
//!
//! Through the real binary, since the gate reads the project's model (`Scope`'s
//! `held_by_modules` / `collect_tl_skipping`, the way `htl adopt` and `htl unused` walk
//! before narrowing), which needs a project discovered from the path given (an
//! `htl.toml` or, for the patched-dependency cases, an `mlua-pkg.toml` alone) --
//! `project::check`'s own Rust-level tests run with `model: None` and so never exercise
//! this gate at all (it answers `true` unconditionally there, by design: with no model
//! there is no wider module set for the files given to fall short of).

use std::path::Path;
use std::process::Command;

mod common;

fn tempdir(name: &str) -> common::TempDir {
    common::tempdir("htl-cli-narrow-walk", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn check(root: &Path, targets: &[&str], lint: &str) -> (bool, String, String) {
    let out = Command::new(common::htl_bin())
        .arg("check")
        .args(targets)
        .args(["--lint", lint, "--no-cache"])
        .current_dir(root)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `gate.Only` (unmarked), declared and built once in `src/gate.tl`; cast once from
/// `src/main.tl`, a site the rule has to miss for a walk that never checks that file.
fn sealed_project(name: &str) -> common::TempDir {
    let root = tempdir(name);
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    write(
        &root.join("src/gate.tl"),
        "local record gate\n   record Only\n      v: integer\n   end\nend\n\n\
         local o: gate.Only = { v = 1 }\nprint(o)\n\nreturn gate\n",
    );
    write(
        &root.join("src/main.tl"),
        "local gate = require(\"gate\")\n\n\
         local w: any = nil\nlocal x = w as gate.Only\nprint(x)\n",
    );
    root
}

/// `gate.Only` (unmarked), built whole once in `src/gate.tl`; built short a second time
/// (no `v` set) from `src/main.tl`, a site the rule has to miss the same way.
fn struct_project(name: &str) -> common::TempDir {
    let root = tempdir(name);
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    write(
        &root.join("src/gate.tl"),
        "local record gate\n   record Only\n      v: integer\n   end\nend\n\n\
         local o: gate.Only = { v = 1 }\nprint(o)\n\nreturn gate\n",
    );
    write(
        &root.join("src/main.tl"),
        "local gate = require(\"gate\")\n\n\
         local p: gate.Only = {}\nprint(p)\n",
    );
    root
}

/// `htl check --lint unmarked-sealed src/gate.tl`: `gate.tl` alone, `main.tl` never
/// checked. Before the gate, this reported `Only` (its one visible site is in its own
/// file) -- wrong, since `main.tl` casts it too. After, it reports nothing: the walk is
/// narrower than the project (`main.tl` exists and was not asked for), so the rule
/// cannot know whether `Only` is built or cast anywhere else and says nothing rather
/// than guess from the one site it saw.
#[test]
fn a_narrow_walk_does_not_report_a_record_the_unwalked_files_use() {
    let root = sealed_project("sealed-narrow");
    let (ok, _, err) = check(&root, &["src/gate.tl"], "unmarked-sealed");
    assert!(ok, "{err}");
    assert!(
        !err.contains("unmarked-sealed"),
        "a walk narrower than the project reports nothing for this rule: {err}"
    );
}

/// As above, checking both files (the whole project): still nothing, because `Only`
/// really is cast from `main.tl` too -- the narrow walk's silence above is not merely
/// "ask again later", the whole-project answer agrees with it.
#[test]
fn the_same_project_walked_whole_still_reports_nothing() {
    let root = sealed_project("sealed-whole");
    let (ok, _, err) = check(&root, &["src"], "unmarked-sealed");
    assert!(ok, "{err}");
    assert!(!err.contains("unmarked-sealed"), "{err}");
}

/// The symmetric blind spot for `unmarked-struct`: a site in an unwalked file could be
/// short. `htl check --lint unmarked-struct src/gate.tl` sees only the one whole site
/// in `gate.tl` and, before the gate, called `Only` a candidate; `main.tl`'s short build
/// is the site that would have said otherwise, and a walk that never checks it cannot
/// know that.
#[test]
fn a_narrow_walk_does_not_report_a_struct_candidate_the_unwalked_files_use() {
    let root = struct_project("struct-narrow");
    let (ok, _, err) = check(&root, &["src/gate.tl"], "unmarked-struct");
    assert!(ok, "{err}");
    assert!(
        !err.contains("unmarked-struct"),
        "a walk narrower than the project reports nothing for this rule: {err}"
    );
}

/// A single-file project where `src/gate.tl` *is* the whole project (no `main.tl`, no
/// other module): the gate does not suppress a genuine candidate just because the walk
/// happens to be one file. Pins the positive case the narrow-walk tests above are a
/// negative of, in directory form (`.`, not the file named outright) -- the shape
/// `the_same_project_walked_whole_still_reports_nothing` above does not cover on its
/// own: that test's whole walk agrees with its narrow one by reporting nothing either
/// way, which passes even if the gate always answered `false` and silenced both. This
/// one only passes if the gate answers `true` for a directory walk that really is the
/// whole project.
#[test]
fn a_directory_walk_that_is_the_whole_project_still_reports_a_genuine_candidate() {
    let root = tempdir("sealed-whole-one-file");
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    write(
        &root.join("src/gate.tl"),
        "local record gate\n   record Only\n      v: integer\n   end\nend\n\n\
         local o: gate.Only = { v = 1 }\nprint(o)\n\nreturn gate\n",
    );
    let (ok, _, err) = check(&root, &["."], "unmarked-sealed");
    assert!(ok, "{err}");
    assert!(
        err.contains(
            "Only is built and cast only in its own file (1 site) and carries no ---@sealed"
        ),
        "a directory walk that is the whole project still answers: {err}"
    );
}

/// A patched dependency's own files (`patches/mathx/`) are entered by `htl check`
/// (`Purpose::Check`) but not by the project-set walk the gate builds (`Purpose::Own`,
/// the same purpose `htl adopt` and `htl unused` use): `htl check .` on this project
/// checks more files than the project's own module set names, which the gate has to
/// accept (`declared_in.is_subset(&checked)`, not equality) or it would call the
/// canonical whole-project invocation "narrower" and silence both rules on it.
/// `gate.Only` lives in `src/gate.tl`, the project's own one source file, built and cast
/// only there; `mathx.tl` declares its own record, `Dep`, which the dedicated
/// candidacy tests below check is never a candidate itself -- the extra file's sites
/// still count against a project record (`Only`'s own census includes none here, so
/// this fixture does not exercise that half), but the extra file's own declarations do
/// not become candidates merely because `htl check` read them.
fn patched_sealed_project(name: &str) -> common::TempDir {
    let root = tempdir(name);
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n[deps.mathx]\n\
         git = \"https://example.invalid/mathx\"\nrev = \"abc\"\npatch_dir = \"patches/mathx\"\n",
    );
    write(
        &root.join("src/gate.tl"),
        "local record gate\n   record Only\n      v: integer\n   end\nend\n\n\
         local o: gate.Only = { v = 1 }\nlocal q = o as gate.Only\nprint(o, q)\n\nreturn gate\n",
    );
    write(
        &root.join("patches/mathx/src/mathx.tl"),
        "local mathx = {}\nfunction mathx.twice(n: number): number\n   return n * 2\nend\n\n\
         local record Dep\n   w: integer\nend\n\
         local d: Dep = { w = 1 }\nprint(d)\n\n\
         return mathx\n",
    );
    root
}

/// `htl check --lint unmarked-sealed .`: the walk includes `patches/mathx/src/mathx.tl`
/// as well as `src/gate.tl`, more files than the project's own module set names, and the
/// extra file holds no site of `Only` at all -- so the record is still a candidate, and
/// a gate comparing by equality would have silenced it here, on exactly the invocation
/// (checking the whole project) the explain's sentence says decides the rule.
#[test]
fn a_patched_dependencys_extra_files_do_not_narrow_the_walk() {
    let root = patched_sealed_project("sealed-patched");
    let (ok, _, err) = check(&root, &["."], "unmarked-sealed");
    assert!(ok, "{err}");
    assert!(
        err.contains(
            "Only is built and cast only in its own file (2 sites) and carries no ---@sealed"
        ),
        "a patched dependency's extra files do not narrow the walk: {err}"
    );
}

/// As above, named instead of walked as a directory: `htl check src/gate.tl
/// patches/mathx/src/mathx.tl` -- `held_by_modules` keeps a file named outright
/// whatever holds it, so the dependency file is in `checked` the same way walking `.`
/// put it there, and the gate has to agree with the directory form.
#[test]
fn a_patched_dependencys_file_named_outright_does_not_narrow_the_walk_either() {
    let root = patched_sealed_project("sealed-patched-named");
    let (ok, _, err) = check(
        &root,
        &["src/gate.tl", "patches/mathx/src/mathx.tl"],
        "unmarked-sealed",
    );
    assert!(ok, "{err}");
    assert!(
        err.contains(
            "Only is built and cast only in its own file (2 sites) and carries no ---@sealed"
        ),
        "{err}"
    );
}

/// `patches/mathx/src/mathx.tl` declares its own record, `Dep`, built whole once, in
/// the one file that declares it -- a candidate for *both* rules by every condition
/// except the fourth: `Dep` is in `checked` (`htl check` reads its site) but not in
/// `declared_in` (the project's own `Purpose::Own` set never enters a patched
/// dependency), so condition 4 excludes it from either rule's report -- the same list
/// `htl adopt` reports over its own walk, which never sees `Dep` at all. `gate.Only`,
/// a genuine candidate, still reports: the extra file changes nothing about the
/// project's own records (the earlier tests above), it only fails to supply one of its
/// own.
#[test]
fn a_patched_dependencys_own_record_is_not_a_candidate() {
    let root = patched_sealed_project("sealed-patched-own-record");
    let (ok, _, err) = check(&root, &["."], "unmarked-struct=warn,unmarked-sealed=warn");
    assert!(ok, "{err}");
    assert!(!err.contains("Dep is"), "{err}");
    assert!(
        err.contains(
            "Only is built and cast only in its own file (2 sites) and carries no ---@sealed"
        ),
        "{err}"
    );
}

/// A file outside every module root, named outright on the command line: `htl check
/// src/gate.tl stray.tl`. `stray.tl` sits at the project root, not under `src/`, so it
/// is no module's and the project's own set is `{src/gate.tl}` alone -- still a subset
/// of `{src/gate.tl, stray.tl}`, so the gate still answers `true` and the rule still
/// reports, the same reason a patched dependency's extra files do not narrow the walk.
#[test]
fn a_file_outside_every_module_root_named_outright_does_not_narrow_the_walk() {
    let root = tempdir("sealed-stray");
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    write(
        &root.join("src/gate.tl"),
        "local record gate\n   record Only\n      v: integer\n   end\nend\n\n\
         local o: gate.Only = { v = 1 }\nprint(o)\n\nreturn gate\n",
    );
    write(&root.join("stray.tl"), "print(\"stray\")\n");
    let (ok, _, err) = check(&root, &["src/gate.tl", "stray.tl"], "unmarked-sealed");
    assert!(ok, "{err}");
    assert!(
        err.contains(
            "Only is built and cast only in its own file (1 site) and carries no ---@sealed"
        ),
        "a file outside every module root, named outright, does not narrow the walk: {err}"
    );
}

/// `stray.tl` declares its own record, `Stray`, built whole once, cast once, both in
/// itself -- a candidate for both rules by every condition but the fourth, the same
/// reason `patches/mathx/src/mathx.tl`'s `Dep` is not one: a file named outright that
/// no module claims is in `checked` (its sites count) but not in `declared_in` (it is
/// not the project's). `gate.Only` still reports.
#[test]
fn a_stray_file_named_outright_is_not_a_candidate_itself() {
    let root = tempdir("sealed-stray-own-record");
    write(&root.join("htl.toml"), "[lint]\nstrict = false\n");
    write(
        &root.join("src/gate.tl"),
        "local record gate\n   record Only\n      v: integer\n   end\nend\n\n\
         local o: gate.Only = { v = 1 }\nprint(o)\n\nreturn gate\n",
    );
    write(
        &root.join("stray.tl"),
        "local record Stray\n   w: integer\nend\n\n\
         local s: Stray = { w = 1 }\nlocal t = s as Stray\nprint(s, t)\n",
    );
    let (ok, _, err) = check(
        &root,
        &["src/gate.tl", "stray.tl"],
        "unmarked-struct=warn,unmarked-sealed=warn",
    );
    assert!(ok, "{err}");
    assert!(!err.contains("Stray is"), "{err}");
    assert!(
        err.contains(
            "Only is built and cast only in its own file (1 site) and carries no ---@sealed"
        ),
        "{err}"
    );
}
