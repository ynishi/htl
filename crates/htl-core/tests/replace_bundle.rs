//! `Htl::replace_bundle`: a newer bundle into a state that is already running.
//!
//! The bundles here are built by hand rather than linked from files. What is under test is
//! what `install_bundle` remembers and what `replace_bundle` takes back, and a linker
//! between the test and that would only decide which modules exist — which is the one
//! thing each case wants to say for itself. Source payloads, so no fingerprint is involved.

use htl_core::Htl;
use htl_core::bundle::{Bundle, Kind, Module};

fn module(name: &str, src: &str) -> Module {
    Module {
        name: name.to_string(),
        kind: Kind::Source,
        payload: src.as_bytes().to_vec(),
    }
}

/// `main` returns nothing and is the entry; `rules` is the module that changes between the
/// two bundles, `world` the one that holds state, `gone` the one only A carries.
fn bundle_a() -> Bundle {
    Bundle {
        entry: "main".into(),
        modules: vec![
            module("main", "return 0\n"),
            module("rules", "return 1\n"),
            module("world", "return { n = 0 }\n"),
            module("gone", "return { here = true }\n"),
        ],
        ..Default::default()
    }
}

fn bundle_b() -> Bundle {
    Bundle {
        entry: "main".into(),
        modules: vec![
            module("main", "return 0\n"),
            module("rules", "return 2\n"),
            module("world", "return { n = -1 }\n"),
        ],
        ..Default::default()
    }
}

/// The walk the issue asks for: run A, mutate the state a module holds, replace with B
/// keeping that module, and read both back in the same `Htl`.
#[test]
fn a_replaced_module_is_new_and_a_kept_one_holds_its_state() {
    let h = Htl::new().unwrap();
    h.run_bundle(&bundle_a(), &[]).unwrap();

    let one: i64 = h.lua().load("return require('rules')").eval().unwrap();
    assert_eq!(one, 1);
    h.lua()
        .load("require('world').n = 7")
        .exec()
        .expect("mutate the world A evaluated");

    h.replace_bundle(&bundle_b(), &["world"]).unwrap();

    let two: i64 = h.lua().load("return require('rules')").eval().unwrap();
    assert_eq!(two, 2, "the new bundle's rules, in the same state");
    let n: i64 = h.lua().load("return require('world').n").eval().unwrap();
    assert_eq!(
        n, 7,
        "the kept module is the table that was mutated, not B's fresh one"
    );

    // A module A had and B does not: nothing answers it any more.
    let (preload_gone, loaded_gone): (bool, bool) = h
        .lua()
        .load("return package.preload['gone'] == nil, package.loaded['gone'] == nil")
        .eval()
        .unwrap();
    assert!(preload_gone, "dropped from package.preload");
    assert!(loaded_gone, "dropped from package.loaded");
}

/// The record is of what the bundle installed, so a name the host had first is neither
/// taken back nor replaced.
#[test]
fn a_host_module_is_the_same_function_after_the_replace() {
    let h = Htl::new().unwrap();
    let t = h.lua().create_table().unwrap();
    t.set("base", h.lua().create_function(|_, ()| Ok(21)).unwrap())
        .unwrap();
    h.preload_value("rules", t).unwrap();

    h.install_bundle(&bundle_a()).unwrap();
    let before: i64 = h
        .lua()
        .load("return require('rules').base()")
        .eval()
        .unwrap();
    assert_eq!(before, 21, "the host won over A");

    let r = h.replace_bundle(&bundle_b(), &[]).unwrap();
    let after: i64 = h
        .lua()
        .load("return require('rules').base()")
        .eval()
        .unwrap();
    assert_eq!(after, 21, "and over B");
    assert!(
        !r.dropped.contains(&"rules".to_string()) && !r.added.contains(&"rules".to_string()),
        "never the bundle's to drop or to add: {r:?}"
    );
}

/// The check that refuses a bundle runs before anything is dropped, so the state a host
/// had is still there afterwards — and says the same thing it says on a plain install.
#[test]
fn a_refused_replace_says_what_install_says_and_changes_nothing() {
    let h = Htl::new().unwrap();
    h.run_bundle(&bundle_a(), &[]).unwrap();

    let mut needs_host = bundle_b();
    needs_host.host_modules = vec!["renderer".into()];
    let err = h.replace_bundle(&needs_host, &[]).unwrap_err().to_string();
    assert!(
        err.contains("host-provided module(s) 'renderer'"),
        "the message a plain install gives: {err}"
    );

    let still: i64 = h.lua().load("return require('rules')").eval().unwrap();
    assert_eq!(still, 1, "A is still installed, and still evaluated");
}

/// What the three lists are for, on the walk above.
#[test]
fn the_report_separates_dropped_kept_and_added() {
    let h = Htl::new().unwrap();
    h.run_bundle(&bundle_a(), &[]).unwrap();
    let r = h.replace_bundle(&bundle_b(), &["world"]).unwrap();

    assert_eq!(r.kept, vec!["world".to_string()]);
    assert_eq!(
        r.dropped,
        vec!["main".to_string(), "rules".to_string(), "gone".to_string()],
        "everything A installed except the kept one, in the order A installed them"
    );
    assert_eq!(
        r.added,
        vec!["main".to_string(), "rules".to_string(), "world".to_string()],
        "what B wrote into preload: a module both carry is dropped and added"
    );
}

/// A `keep` name the bundle never installed is not kept, and the short list is the only
/// place that says so.
#[test]
fn keeping_a_name_the_bundle_never_installed_keeps_nothing() {
    let h = Htl::new().unwrap();
    h.run_bundle(&bundle_a(), &[]).unwrap();
    let r = h.replace_bundle(&bundle_b(), &["world", "typo"]).unwrap();
    assert_eq!(
        r.kept,
        vec!["world".to_string()],
        "two asked for, one was this bundle's: {r:?}"
    );
}

/// Installing the same bundle twice writes nothing the second time and must not forget
/// what the first time wrote — otherwise a replace after it would take nothing back.
#[test]
fn a_second_install_does_not_erase_the_record() {
    let h = Htl::new().unwrap();
    h.install_bundle(&bundle_a()).unwrap();
    h.install_bundle(&bundle_a()).unwrap();
    let r = h.replace_bundle(&bundle_b(), &[]).unwrap();
    assert_eq!(
        r.dropped,
        vec![
            "main".to_string(),
            "rules".to_string(),
            "world".to_string(),
            "gone".to_string()
        ],
        "all four, recorded once: {r:?}"
    );
}

/// A replace into a state that never saw a bundle is an install: nothing to take back.
#[test]
fn replacing_nothing_is_installing() {
    let h = Htl::new().unwrap();
    let r = h.replace_bundle(&bundle_b(), &["world"]).unwrap();
    assert!(r.dropped.is_empty() && r.kept.is_empty(), "{r:?}");
    let two: i64 = h.lua().load("return require('rules')").eval().unwrap();
    assert_eq!(two, 2);
}
