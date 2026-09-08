//! `link_with` and the run cache: a module whose `gen` entry still holds is replayed, an
//! edit invalidates it and everything that read it, and a replayed bundle is the bundle a
//! generated one would have been.

use htl_core::Htl;
use htl_core::cache::{Cache, Mode, Options, module_gen_key};
use htl_core::link::{LinkOptions, LinkStore, link, link_with};
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-link-cache", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// main -> b -> c, all typed.
fn project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(
        &root.join("src/main.tl"),
        "local b = require(\"b\")\nprint(b.two())\n",
    );
    write(
        &root.join("src/b.tl"),
        "local c = require(\"c\")\nlocal record b\nend\nfunction b.two(): integer\n   return c.one() + 1\nend\nreturn b\n",
    );
    write(
        &root.join("src/c.tl"),
        "local record c\nend\nfunction c.one(): integer\n   return 1\nend\nreturn c\n",
    );
    root
}

fn checker(root: &Path) -> Htl {
    let h = Htl::new().unwrap();
    h.add_path(&root.join("src")).unwrap();
    h
}

fn store(root: &Path) -> Cache {
    Cache::open(
        root,
        Options {
            enabled: true,
            mode: Mode::PerModule,
            explain: false,
            max_entries: None,
        },
    )
    .expect("this build can stamp its entries")
}

/// One run: a checker and a store opened for it, the way a command or a macro expansion
/// has them. A `Cache` memoizes the hashes it takes for the life of the run, so reusing
/// one across edits would be a test of the memo rather than of the store.
fn link_cached(root: &Path) -> htl_core::link::Linked {
    let h = checker(root);
    let cache = store(root);
    let s = LinkStore {
        cache: &cache,
        lint: None,
        root,
        config: None,
    };
    link_with(
        &h,
        &root.join("src/main.tl"),
        &LinkOptions::default(),
        Some(s),
    )
    .unwrap()
}

/// Inputs as sets of files, whichever way the paths were spelled.
fn canonical(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = paths
        .into_iter()
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .collect();
    out.sort();
    out.dedup();
    out
}

#[test]
fn the_second_link_replays_every_module_and_produces_the_same_bundle() {
    let root = project("replay");
    let first = link_cached(&root);
    assert_eq!(first.cached, 0, "nothing to replay on a cold store");
    assert_eq!(first.modules.len(), 3);
    let second = link_cached(&root);
    assert_eq!(second.cached, 3, "{:?}", second.modules);
    assert_eq!(
        first.bundle().unwrap().encode(),
        second.bundle().unwrap().encode(),
        "a replayed bundle is byte-for-byte the generated one"
    );
    assert_eq!(
        canonical(second.inputs()),
        canonical(first.inputs()),
        "the tracked inputs are the same files"
    );
}

#[test]
fn an_edit_costs_the_module_and_what_read_it() {
    let root = project("edit");
    link_cached(&root);
    // The entry module: nothing requires it, so the two below it replay.
    write(
        &root.join("src/main.tl"),
        "local b = require(\"b\")\nprint(b.two() + 1)\n",
    );
    let after_top = link_cached(&root);
    assert_eq!(after_top.cached, 2, "{:?}", after_top.modules);
    // The leaf: it and the module that requires it are re-generated. `main` did not
    // require `c` itself and replays — an entry's inputs are the module and what it
    // required, the rule `htl check` has always used (README, "Caching"); `b`'s
    // declared interface is what `main` was checked against, and `b.tl` did not move.
    write(
        &root.join("src/c.tl"),
        "local record c\nend\nfunction c.one(): integer\n   return 2\nend\nreturn c\n",
    );
    let after_leaf = link_cached(&root);
    assert_eq!(after_leaf.cached, 1, "{:?}", after_leaf.modules);
    // And the store holds the new versions: back to replaying everything.
    assert_eq!(link_cached(&root).cached, 3);
}

#[test]
fn a_type_error_is_reported_on_every_link() {
    let root = project("error");
    write(
        &root.join("src/c.tl"),
        "local record c\nend\nfunction c.one(): integer\n   return \"no\"\nend\nreturn c\n",
    );
    let bad = link_cached(&root);
    assert!(!bad.ok(), "{:?}", bad.errors);
    // main and b generated and were stored; c produced no code and was not. The next
    // link replays the two and walks into c again, which fails again: the store never
    // hides an error.
    let again = link_cached(&root);
    assert!(!again.ok(), "{:?}", again.errors);
    assert_eq!(again.cached, 2, "{:?}", again.modules);
    assert_eq!(again.errors, bad.errors);
}

#[test]
fn a_check_entry_without_code_is_not_a_hit() {
    // `htl check` writes entries with no code under its own key; a `gen` entry can also
    // lack code when a build predating the field wrote it. Neither is a replay.
    let root = project("nocode");
    let main = root.join("src/main.tl");
    let m = htl_core::cache::Module {
        diagnostics: Vec::new(),
        errors: 0,
        warnings: 0,
        lints: 0,
        deps: Vec::new(),
        requires: Vec::new(),
        code: None,
        check: None,
    };
    store(&root).store_module(
        &module_gen_key(&main, None),
        &main,
        &[],
        &[root.join("src")],
        &m,
    );
    let linked = link_cached(&root);
    assert_eq!(linked.cached, 0);
    assert_eq!(
        linked.modules.len(),
        3,
        "and the walk still found everything"
    );
    // The miss overwrote it with a real entry.
    assert_eq!(link_cached(&root).cached, 3);
}

#[test]
fn without_a_store_nothing_is_written() {
    let root = project("plain");
    let h = checker(&root);
    let linked = link(&h, &root.join("src/main.tl"), &LinkOptions::default()).unwrap();
    assert_eq!(linked.cached, 0);
    assert!(
        !root.join(".htl").exists(),
        "`link` without a store leaves no `.htl/` behind"
    );
}
