//! Program states sharing one checker (`Htl::with_checker`), alive at the same time.
//!
//! Each state has a program of its own in the checker — its search path, its model, its
//! `[lang]`, deps and lint selection, its module names — and every call it makes into the
//! checker enters that program first (`H.with_program` in `prelude.lua`). Before that, the
//! checker kept all of it in its globals, and the state made last decided for every state
//! still alive: the test runner hid it by running one file at a time and putting the path
//! back after each.

use htl_core::Htl;
use htl_core::config::LangConfig;
use htl_core::lint::Selection;
use std::path::{Path, PathBuf};

mod common;

fn tempdir(name: &str) -> common::TempDir {
    common::tempdir("htl-core-programs", name)
}

fn write(path: &Path, text: &str) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    path.to_path_buf()
}

/// Two directories, each with a `util` of its own that types `f` differently, and a `main`
/// beside it that holds `f` to its own directory's type.
fn two_utils() -> (common::TempDir, common::TempDir) {
    let a = tempdir("a");
    let b = tempdir("b");
    write(
        &a.join("util.tl"),
        "local record util\nend\nfunction util.f(): integer\n   return 1\nend\nreturn util\n",
    );
    write(
        &b.join("util.tl"),
        "local record util\nend\nfunction util.f(): string\n   return \"b\"\nend\nreturn util\n",
    );
    write(
        &a.join("main.tl"),
        "local util = require(\"util\")\nlocal n: integer = util.f()\nreturn n\n",
    );
    write(
        &b.join("main.tl"),
        "local util = require(\"util\")\nlocal s: string = util.f()\nreturn s\n",
    );
    (a, b)
}

fn run(h: &Htl, src: &str) -> String {
    h.lua().load(src).eval::<String>().unwrap()
}

/// Both states made, both paths added, then every check and every `require` interleaved:
/// each resolves `util` to its own directory's file. The checker made the second state's
/// path and env the only ones, so the first state's check and `require` found `b/util.tl`.
#[test]
fn two_live_states_resolve_a_name_each_to_its_own_file() {
    let (a, b) = two_utils();
    let checker = Htl::new().unwrap();
    let pa = Htl::with_checker(&checker).unwrap();
    let pb = Htl::with_checker(&checker).unwrap();
    pa.add_path(&a).unwrap();
    pb.add_path(&b).unwrap();
    pa.install_searcher().unwrap();
    pb.install_searcher().unwrap();

    let (code, ca) = pa.gen_lua(&a.join("main.tl")).unwrap();
    assert!(
        ca.ok() && code.is_some(),
        "a's main against a's util: {:?}",
        ca.errors
    );
    let (code, cb) = pb.gen_lua(&b.join("main.tl")).unwrap();
    assert!(
        cb.ok() && code.is_some(),
        "b's main against b's util: {:?}",
        cb.errors
    );

    assert_eq!(run(&pa, "return tostring(require(\"util\").f())"), "1");
    assert_eq!(run(&pb, "return tostring(require(\"util\").f())"), "b");

    // And again after the other has run: nothing either did moved the other.
    let ca = pa.check(&a.join("main.tl")).unwrap();
    assert!(ca.ok(), "{:?}", ca.errors);
    let cb = pb.check(&b.join("main.tl")).unwrap();
    assert!(cb.ok(), "{:?}", cb.errors);
    assert_eq!(
        pa.resolve_module("util").unwrap().0.as_deref(),
        Some(a.join("util.tl").as_path())
    );
    assert_eq!(
        pb.resolve_module("util").unwrap().0.as_deref(),
        Some(b.join("util.tl").as_path())
    );

    // The checker's own path has neither: a state's `add_path` is the state's.
    let path = checker.search_path().unwrap();
    assert!(
        !path.contains(&*a.to_string_lossy()) && !path.contains(&*b.to_string_lossy()),
        "{path}"
    );
}

/// An `async function`: a syntax error in plain Teal, a function under `[lang] async`.
const ASYNC_SRC: &str = "local async function f(): integer\n   return 1\nend\nreturn f\n";

/// `d[1]` — a `nil-index` lint, `warn` by default, so reported unless a selection turns it
/// off.
const NIL_INDEX_SRC: &str = "local t: {{string:{string}}} = {}\nlocal d = t[\"a\"][1]\nreturn d\n";

/// `[lang]` and the lint selection set in one state are that state's: the other, alive at
/// the same time, and the checker itself read by their own.
#[test]
fn two_live_states_keep_their_own_lang_and_lints() {
    let dir = tempdir("settings");
    let async_file = write(&dir.join("a.tl"), ASYNC_SRC);
    let lint_file = write(&dir.join("l.tl"), NIL_INDEX_SRC);
    let checker = Htl::new().unwrap();
    let on = Htl::with_checker(&checker).unwrap();
    let off = Htl::with_checker(&checker).unwrap();

    on.set_lang(&LangConfig { async_: Some(true) }).unwrap();
    on.select_lints(&Selection::parse("-nil-index").unwrap())
        .unwrap();

    let c = on.check(&async_file).unwrap();
    assert!(
        c.ok(),
        "async is a keyword where it was set: {:?}",
        c.errors
    );
    let c = off.check(&async_file).unwrap();
    assert!(!c.ok(), "and not in the other state");
    let c = checker.check(&async_file).unwrap();
    assert!(!c.ok(), "nor in the checker's own program");

    let has = |h: &Htl| {
        h.check(&lint_file)
            .unwrap()
            .lints
            .iter()
            .any(|l| l.contains("nil-index"))
    };
    assert!(!has(&on), "turned off where it was turned off");
    assert!(has(&off), "on in the other state");
    assert!(has(&checker), "and in the checker");
}

/// What a setter writes stays in the state's program for the calls after it: the path
/// `add_path` added answers the next `gen_lua`, and the `[lang]` `set_lang` set reads the
/// next file. Each call enters and leaves the program, so this is the leave writing back.
#[test]
fn a_setter_reaches_the_later_calls_of_its_own_state() {
    let (a, _b) = two_utils();
    let dir = tempdir("later");
    let async_file = write(&dir.join("a.tl"), ASYNC_SRC);
    let checker = Htl::new().unwrap();
    let h = Htl::with_checker(&checker).unwrap();

    let before = h.check(&a.join("main.tl")).unwrap();
    assert!(!before.ok(), "no util before its directory is added");
    h.add_path(&a).unwrap();
    let (code, c) = h.gen_lua(&a.join("main.tl")).unwrap();
    assert!(c.ok() && code.is_some(), "{:?}", c.errors);
    assert!(h.search_path().unwrap().contains(&*a.to_string_lossy()));

    assert!(!h.check(&async_file).unwrap().ok());
    h.set_lang(&LangConfig { async_: Some(true) }).unwrap();
    let c = h.check(&async_file).unwrap();
    assert!(c.ok(), "{:?}", c.errors);
}

/// A state starts from the checker's settings as they are when it is made, and from no
/// other state's: a path a sibling added is not on it.
#[test]
fn a_new_state_starts_from_the_checker_and_not_from_a_sibling() {
    let (a, b) = two_utils();
    let checker = Htl::new().unwrap();
    checker.add_path(&a).unwrap();
    let sibling = Htl::with_checker(&checker).unwrap();
    sibling.add_path(&b).unwrap();
    let h = Htl::with_checker(&checker).unwrap();
    let path = h.search_path().unwrap();
    assert!(path.contains(&*a.to_string_lossy()), "{path}");
    assert!(!path.contains(&*b.to_string_lossy()), "{path}");
}
