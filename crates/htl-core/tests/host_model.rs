//! A host described as a project model (#320): the checker set up with
//! `Htl::apply_model` and the run served by `TealResolver::from_project`, both from one
//! `Project::for_host`, answer every name the same way.
//!
//! Set up the old way — `add_package_path` / `apply_config` for the checker and a
//! `TealResolver` per directory for the run — the two disagreed: the checker's `?/?`
//! template read `pkgs/a/b/a/b.tl` as `a.b` where the resolver did not, and `apply_config`
//! put `types/` and `types/htl-mq/` on the path as two roots, so `types/htl-mq/mq.d.tl`
//! checked as `mq` and as `htl-mq.mq` while `TealResolver::new("types")` served only the
//! second. Each case below asks both sides.

#![cfg(all(feature = "pkg", feature = "dts"))]

use htl_core::Htl;
use htl_core::model::{HostDir, Project, View};
use htl_core::pkg::TealResolver;
use mlua_pkg::Registry;
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-host-model", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A host whose checker and run both come from `project`.
fn host(project: &Project) -> Htl {
    let h = Htl::new().unwrap();
    h.apply_model(project, View::Source).unwrap();
    let mut reg = Registry::new();
    reg.add(TealResolver::from_project(project).unwrap());
    reg.install(h.lua()).unwrap();
    h
}

/// The checker's errors for `file`.
fn check(h: &Htl, file: &Path) -> Vec<String> {
    h.check(file).unwrap().errors
}

/// The value of `require(name).n`, or the error text.
fn n_of(h: &Htl, name: &str) -> Result<i64, String> {
    h.lua()
        .load(format!("return require('{name}').n"))
        .eval::<i64>()
        .map_err(|e| e.to_string())
}

/// Whether `require(name)` succeeds at run time; the error text when it does not.
fn loads(h: &Htl, name: &str) -> Result<(), String> {
    h.lua()
        .load(format!("require('{name}')"))
        .exec()
        .map_err(|e| e.to_string())
}

#[test]
fn a_deeper_file_in_a_package_is_not_the_package_name_to_either_side() {
    // P1: `pkgs/a/b/a/b.tl` is `a.b.a.b` — package `a`, file `b/a/b.tl` — and not `a.b`.
    let root = scratch("p1");
    write(&root.join("pkgs/a/b/a/b.tl"), "return { n = 1 }\n");
    write(
        &root.join("scripts/main.tl"),
        "local ab = require(\"a.b\")\nreturn { n = ab.n as integer }\n",
    );
    write(
        &root.join("scripts/deep.tl"),
        "local ab = require(\"a.b.a.b\")\nreturn { n = ab.n + 1 }\n",
    );
    let project = Project::for_host(
        &root,
        &[
            HostDir::Modules("scripts".into()),
            HostDir::Packages("pkgs".into()),
        ],
    );
    let h = host(&project);

    let errors = check(&h, &root.join("scripts/main.tl"));
    assert!(
        errors.iter().any(|e| e.contains("module not found: 'a.b'")),
        "{errors:?}"
    );
    let run = loads(&h, "a.b").unwrap_err();
    assert!(run.contains("not found"), "{run}");
    let main = loads(&h, "main").unwrap_err();
    assert!(main.contains("module not found: 'a.b'"), "{main}");

    assert_eq!(
        check(&h, &root.join("scripts/deep.tl")),
        Vec::<String>::new()
    );
    assert_eq!(n_of(&h, "a.b.a.b"), Ok(1));
    assert_eq!(n_of(&h, "deep"), Ok(2));
}

#[test]
fn a_crates_declaration_is_the_name_the_crate_wrote_it_under_on_both_sides() {
    // P2 and P3: `types/htl-mq/mq.d.tl` is `mq`, and `htl-mq.mq` is nothing.
    let root = scratch("p2");
    write(&root.join("types/htl-mq/.htl-dts"), "");
    write(
        &root.join("types/htl-mq/mq.d.tl"),
        "local record mq\n   n: integer\nend\nreturn mq\n",
    );
    write(
        &root.join("scripts/a.tl"),
        "local mq = require(\"mq\")\nreturn { n = mq.n }\n",
    );
    write(
        &root.join("scripts/b.tl"),
        "local mq = require(\"htl-mq.mq\")\nreturn { n = mq.n }\n",
    );
    let project = Project::for_host(
        &root,
        &[
            HostDir::Modules("scripts".into()),
            HostDir::Declarations("types".into()),
        ],
    );
    let h = host(&project);

    assert_eq!(check(&h, &root.join("scripts/a.tl")), Vec::<String>::new());
    assert_eq!(loads(&h, "mq"), Ok(()));

    let errors = check(&h, &root.join("scripts/b.tl"));
    assert!(
        errors
            .iter()
            .any(|e| e.contains("module not found: 'htl-mq.mq'")),
        "{errors:?}"
    );
    let run = loads(&h, "htl-mq.mq").unwrap_err();
    assert!(run.contains("not found"), "{run}");
}

#[test]
fn a_module_at_the_top_and_a_flat_package_resolve_the_same_in_check_and_run() {
    let root = scratch("flat");
    write(
        &root.join("mods/mathx/mathx.tl"),
        "local sub = require(\"mathx.sub\")\nreturn { n = sub.n * 2 }\n",
    );
    write(&root.join("mods/mathx/sub.tl"), "return { n = 21 }\n");
    write(&root.join("scripts/util.tl"), "return { n = 1 }\n");
    write(
        &root.join("scripts/main.tl"),
        "local mathx = require(\"mathx\")\nlocal util = require(\"util\")\n\
         return { n = mathx.n + util.n }\n",
    );
    let project = Project::for_host(
        &root,
        &[
            HostDir::Modules("scripts".into()),
            HostDir::Packages("mods".into()),
        ],
    );
    let h = host(&project);

    assert_eq!(
        check(&h, &root.join("scripts/main.tl")),
        Vec::<String>::new()
    );
    assert_eq!(
        check(&h, &root.join("mods/mathx/mathx.tl")),
        Vec::<String>::new()
    );
    assert_eq!(n_of(&h, "mathx"), Ok(42));
    assert_eq!(n_of(&h, "mathx.sub"), Ok(21));
    assert_eq!(n_of(&h, "util"), Ok(1));
    assert_eq!(n_of(&h, "main"), Ok(43));
}

#[test]
fn a_name_two_served_directories_implement_is_an_error_to_both() {
    let root = scratch("two");
    write(&root.join("scripts/mathx.tl"), "return { n = 1 }\n");
    write(&root.join("mods/mathx/init.tl"), "return { n = 2 }\n");
    write(
        &root.join("scripts/main.tl"),
        "local m = require(\"mathx\")\nreturn { n = m.n as integer }\n",
    );
    let project = Project::for_host(
        &root,
        &[
            HostDir::Modules("scripts".into()),
            HostDir::Packages("mods".into()),
        ],
    );
    let h = host(&project);

    let errors = check(&h, &root.join("scripts/main.tl"));
    assert!(
        errors
            .iter()
            .any(|e| e.contains("'mathx' is implemented by more than one file")),
        "{errors:?}"
    );
    let run = loads(&h, "mathx").unwrap_err();
    assert!(
        run.contains("module 'mathx' is implemented by more than one file"),
        "{run}"
    );
}

#[test]
fn a_file_added_after_the_host_started_is_found_at_the_next_require() {
    let root = scratch("later");
    write(&root.join("scripts/main.tl"), "return { n = 1 }\n");
    let project = Project::for_host(&root, &[HostDir::Modules("scripts".into())]);
    let h = host(&project);
    assert_eq!(n_of(&h, "main"), Ok(1));
    assert!(loads(&h, "later").is_err());

    // Two files dropped in together, one requiring the other: the run finds the first,
    // and its check reads the directory as it is now, so it finds the second.
    write(&root.join("scripts/helper.tl"), "return { n = 20 }\n");
    write(
        &root.join("scripts/later.tl"),
        "local helper = require(\"helper\")\nreturn { n = helper.n + 1 }\n",
    );
    assert_eq!(n_of(&h, "later"), Ok(21));
}

/// #328: a module the host has had since it started, first required after a file it
/// requires was dropped in. The module is in the table, so nothing rebuilds it before the
/// check; the check's own `require` is what finds the new name outside it.
#[test]
fn a_known_module_requiring_a_file_added_later_checks_against_it() {
    let root = scratch("known");
    write(
        &root.join("scripts/known.tl"),
        "local helper = require(\"helper\")\nreturn { n = helper.n + 1 }\n",
    );
    let project = Project::for_host(&root, &[HostDir::Modules("scripts".into())]);
    let h = host(&project);
    assert!(loads(&h, "helper").is_err());

    write(&root.join("scripts/helper.tl"), "return { n = 40 }\n");
    assert_eq!(n_of(&h, "known"), Ok(41));
}
