//! `global_redeclarations`: the project-level lint for one global name declared at two
//! sites.
//!
//! The checker keeps the first declaration of a global it walks and says nothing about a
//! second one of the same type (`add_global` in the vendored compiler returns without a
//! word when the types agree), so two `.d.tl` that both declare `global VERSION: string`
//! were read as one and nobody was told. A different type is reported the same way: the
//! checker's own error for it is raised only where one environment walks both, which
//! depends on the order the files were checked in, and the lint does not.
//!
//! A `.d.tl` is never one of the files a directory check walks, and a run whose files all
//! replay from the cache builds no checker at all; so the sites a check carries are those
//! of every module in its require closure, not only its own, and the lint reads them off
//! the `CheckInfo`s whatever produced them.

use htl_core::{CheckInfo, Htl, global_redeclarations};
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-global-redecl", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Check `lib/<m>/init.tl` for each `m`, in that order, in one checker whose path is
/// `lib` and `types`.
fn check_all(root: &Path, order: &[&str]) -> Vec<(PathBuf, CheckInfo)> {
    let h = Htl::new().unwrap();
    h.add_path(&root.join("lib")).unwrap();
    h.add_path(&root.join("types")).unwrap();
    order
        .iter()
        .map(|m| {
            let p = root.join(format!("lib/{m}/init.tl"));
            let ci = h.check(&p).unwrap();
            (p, ci)
        })
        .collect()
}

fn lints(infos: &[(PathBuf, CheckInfo)]) -> Vec<String> {
    global_redeclarations(infos)
        .iter()
        .map(ToString::to_string)
        .collect()
}

/// `ma` requires `a`, `mb` requires `b`, and each reads `VERSION`.
fn two_declarations(name: &str, a: &str, b: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("types/a.d.tl"), a);
    write(&root.join("types/b.d.tl"), b);
    for (m, d) in [("ma", "a"), ("mb", "b")] {
        write(
            &root.join(format!("lib/{m}/init.tl")),
            &format!(
                "require(\"{d}\")\nlocal record {m}\nend\nfunction {m}.v(): string return tostring(VERSION) end\nreturn {m}\n"
            ),
        );
    }
    root
}

#[test]
fn two_declaration_files_of_one_global_are_reported_once_naming_both() {
    let root = two_declarations(
        "same",
        "global VERSION: string\n",
        "-- a second copy\nglobal VERSION: string\n",
    );
    for order in [["ma", "mb"], ["mb", "ma"]] {
        let infos = check_all(&root, &order);
        for (_, ci) in &infos {
            assert!(ci.errors.is_empty(), "{order:?}: {:?}", ci.errors);
        }
        let found = lints(&infos);
        assert_eq!(found.len(), 1, "{order:?}: {found:?}");
        let l = &found[0];
        assert!(l.contains("[htl global-redeclaration]"), "{l}");
        assert!(l.contains("VERSION"), "{l}");
        // The second site, in path order, is where it is reported; the first is named.
        assert!(l.contains("b.d.tl:2:8"), "reported at the later site: {l}");
        assert!(l.contains("a.d.tl:1:8"), "names the earlier site: {l}");
    }
}

fn checker_errors(infos: &[(PathBuf, CheckInfo)]) -> Vec<String> {
    infos
        .iter()
        .flat_map(|(_, ci)| {
            ci.errors
                .iter()
                .cloned()
                .chain(ci.dependency_errors.iter().map(|e| e.text.clone()))
        })
        .collect()
}

/// Two declarations of different types, both required by one file. Checked first, that
/// file walks both into one environment and the checker raises its own error; checked
/// after the files that require one each, it is handed both from the store and the
/// checker sees nothing. The lint says the same thing in both orders, which is why it
/// does not step aside for the error.
#[test]
fn a_different_type_in_one_environment_is_reported_whatever_the_checker_saw() {
    let root = scratch("typed-one-env");
    write(&root.join("types/a.d.tl"), "global VERSION: string\n");
    write(&root.join("types/b.d.tl"), "global VERSION: integer\n");
    for (m, d) in [("ma", "a"), ("mb", "b")] {
        write(
            &root.join(format!("lib/{m}/init.tl")),
            &format!(
                "require(\"{d}\")\nlocal record {m}\nend\nfunction {m}.v(): string return tostring(VERSION) end\nreturn {m}\n"
            ),
        );
    }
    write(
        &root.join("lib/both/init.tl"),
        "require(\"a\")\nrequire(\"b\")\nlocal record both\nend\nfunction both.v(): string return tostring(VERSION) end\nreturn both\n",
    );
    let walked = check_all(&root, &["both", "ma", "mb"]);
    let all = checker_errors(&walked);
    assert!(
        all.iter()
            .any(|e| e.contains("cannot redeclare global with a different type")),
        "walked first, both declarations meet in one environment: {all:?}"
    );
    let served = check_all(&root, &["ma", "mb", "both"]);
    assert!(
        checker_errors(&served).is_empty(),
        "served both from the store, the checker sees nothing: {:?}",
        checker_errors(&served)
    );
    let (w, s) = (lints(&walked), lints(&served));
    assert_eq!(w.len(), 1, "{w:?}");
    assert_eq!(w, s, "the lint does not depend on the order");
}

/// The same two declarations, each required by a different file: no environment ever
/// holds both, so the checker has nothing to say, and each file is typed against its own
/// idea of the global. That is the lint's case as much as the same-typed one — more so.
#[test]
fn a_different_type_in_two_environments_is_the_lints_to_report() {
    let root = two_declarations(
        "typed-two-envs",
        "global VERSION: string\n",
        "global VERSION: integer\n",
    );
    let infos = check_all(&root, &["ma", "mb"]);
    assert!(
        checker_errors(&infos).is_empty(),
        "no environment saw both: {:?}",
        checker_errors(&infos)
    );
    let found = lints(&infos);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("VERSION"), "{}", found[0]);
}

#[test]
fn one_declaration_required_by_three_modules_is_nothing() {
    let root = scratch("one");
    write(&root.join("types/host.d.tl"), "global VERSION: string\n");
    for m in ["m1", "m2", "m3"] {
        write(
            &root.join(format!("lib/{m}/init.tl")),
            &format!(
                "require(\"host\")\nlocal record {m}\nend\nfunction {m}.v(): string return VERSION end\nreturn {m}\n"
            ),
        );
    }
    let infos = check_all(&root, &["m1", "m2", "m3"]);
    assert!(lints(&infos).is_empty(), "{:?}", lints(&infos));
}

#[test]
fn a_module_declaring_what_a_declaration_file_declares_is_a_second_site() {
    let root = scratch("tl-and-dtl");
    write(&root.join("types/host.d.tl"), "global VERSION: string\n");
    write(
        &root.join("lib/own/init.tl"),
        "global VERSION: string\nlocal record own\nend\nreturn own\n",
    );
    write(
        &root.join("lib/user/init.tl"),
        "require(\"host\")\nrequire(\"own\")\nlocal record user\nend\nfunction user.v(): string return VERSION end\nreturn user\n",
    );
    let infos = check_all(&root, &["own", "user"]);
    let found = lints(&infos);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("VERSION"), "{}", found[0]);
}

/// The sites a check carries are those of its require closure, so the same tree checked
/// twice in two checkers — the second standing in for a replay — reports the same.
#[test]
fn a_second_run_over_the_same_tree_reports_the_same() {
    let root = two_declarations(
        "again",
        "global VERSION: string\n",
        "global VERSION: string\n",
    );
    let first = lints(&check_all(&root, &["ma", "mb"]));
    let second = lints(&check_all(&root, &["ma", "mb"]));
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(first, second);
}
