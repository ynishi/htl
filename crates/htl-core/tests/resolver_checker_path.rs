//! Where a `TealResolver`'s root lands on the checker's search path.
//!
//! A host that chains one resolver per directory tier states the search order once, with
//! [`Htl::add_search_paths`]. A resolver used to prepend its own root on its first
//! `resolve` whatever the host had said, so the tier that happened to serve a `.tl` first
//! ended up in front and every module checked afterwards was typed against *its*
//! declarations — the same two files gave two different answers depending on which
//! `require` came first. `H.add_path` now leaves a directory that is already on the path
//! where it is, so the host's order stands and the path stops collecting duplicates.
//!
//! The resolver that is the only source of its root is untouched: the root is absent, so
//! it is still prepended and consulted first (`sibling_resolves_without_a_host_order`).

use htl_core::Htl;
use htl_core::pkg::TealResolver;
use mlua_pkg::Registry;
use mlua_pkg::resolvers::FsResolver;
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-checker-path", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Two tiers, each holding a `shared` of its own: the lower one says `v: integer`, the
/// upper one `v: string`. `b` lives in the lower tier and is written against its
/// declaration, `a` in the upper tier against the other. With both tiers on the path the
/// host's order decides which declaration *both* modules are typed against — one of the
/// two is then wrong, and which one must not depend on the order of the `require`s.
fn tiers(name: &str) -> (PathBuf, PathBuf) {
    let base = scratch(name);
    let tier1 = base.join("tier1");
    let tier2 = base.join("tier2");

    write(
        &tier1.join("shared.d.tl"),
        "local record shared\n   v: integer\nend\nreturn shared\n",
    );
    write(&tier1.join("shared.lua"), "return { v = 1 }\n");
    write(
        &tier1.join("b.tl"),
        "local shared = require(\"shared\")\nlocal n: integer = shared.v\nreturn { n = n }\n",
    );

    write(
        &tier2.join("shared.d.tl"),
        "local record shared\n   v: string\nend\nreturn shared\n",
    );
    write(&tier2.join("shared.lua"), "return { v = \"one\" }\n");
    write(
        &tier2.join("a.tl"),
        "local shared = require(\"shared\")\nlocal s: string = shared.v\nreturn { s = s }\n",
    );

    (tier1, tier2)
}

/// The host of the issue: the order stated once, then a resolver per tier.
fn host(tier1: &Path, tier2: &Path) -> Htl {
    let h = Htl::new().unwrap();
    h.add_search_paths(&[tier1.to_path_buf(), tier2.to_path_buf()])
        .unwrap();
    let mut reg = Registry::new();
    reg.add(TealResolver::new(tier1).unwrap());
    reg.add(FsResolver::new(tier1).unwrap());
    reg.add(TealResolver::new(tier2).unwrap());
    reg.add(FsResolver::new(tier2).unwrap());
    reg.install(h.lua()).unwrap();
    h
}

/// `Ok` or the error text of `require(name)`.
fn require(h: &Htl, name: &str) -> Result<(), String> {
    h.lua()
        .load(format!("return require('{name}')"))
        .eval::<mlua::Value>()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The whole `;`-delimited entries of `package.path`, in order.
fn path_entries(h: &Htl) -> Vec<String> {
    h.lua()
        .load("return package.path")
        .eval::<String>()
        .unwrap()
        .split(';')
        .map(str::to_string)
        .collect()
}

#[test]
fn the_host_order_decides_whichever_require_comes_first() {
    let mut outcomes = Vec::new();
    for order in [["a", "b"], ["b", "a"]] {
        let (tier1, tier2) = tiers(&format!("{}-first", order[0]));
        let h = host(&tier1, &tier2);
        let mut seen = Vec::new();
        for name in order {
            seen.push((name, require(&h, name)));
        }
        seen.sort_by_key(|(name, _)| *name);
        outcomes.push(seen);
    }

    // tier1 is what the host put first, so tier1's `shared` types both modules: `b`
    // agrees with it and `a` does not, in either order.
    for seen in &outcomes {
        let a = &seen[0].1;
        let b = &seen[1].1;
        assert!(b.is_ok(), "b is written against tier1's shared: {b:?}");
        let err = a.as_ref().unwrap_err();
        assert!(
            err.contains("got integer, expected string"),
            "a is typed against tier1's shared too: {err}"
        );
    }
    assert_eq!(
        outcomes[0]
            .iter()
            .map(|(n, r)| (*n, r.is_ok()))
            .collect::<Vec<_>>(),
        outcomes[1]
            .iter()
            .map(|(n, r)| (*n, r.is_ok()))
            .collect::<Vec<_>>(),
        "the same files give the same answer in both require orders"
    );
}

#[test]
fn a_resolved_tier_neither_moves_nor_duplicates_the_host_entry() {
    let (tier1, tier2) = tiers("entries");
    let h = host(&tier1, &tier2);
    // Whatever they answer is the previous test's subject; here it is only that both
    // resolvers have run.
    let _ = require(&h, "a");
    let _ = require(&h, "b");

    let entries = path_entries(&h);
    let first = |dir: &Path| format!("{}/?.lua", dir.display());
    let at = |dir: &Path| {
        let want = first(dir);
        entries
            .iter()
            .enumerate()
            .filter(|(_, e)| **e == want)
            .map(|(i, _)| i)
            .collect::<Vec<_>>()
    };
    let one = at(&tier1);
    let two = at(&tier2);
    assert_eq!(one.len(), 1, "tier1 once: {entries:?}");
    assert_eq!(two.len(), 1, "tier2 once: {entries:?}");
    assert!(one[0] < two[0], "in the host's order: {entries:?}");
}

/// The case the resolver's prepend exists for: one resolver over a root nobody else
/// placed. The root is not on the path, so it is put in front and a module resolves its
/// siblings through it.
#[test]
fn sibling_resolves_without_a_host_order() {
    let dir = scratch("sibling");
    write(
        &dir.join("util.tl"),
        "local record util\n   n: integer\nend\nreturn util\n",
    );
    write(
        &dir.join("main.tl"),
        "local util = require(\"util\")\nlocal n: integer = util.n\nreturn { n = n }\n",
    );

    let h = Htl::new().unwrap();
    let mut reg = Registry::new();
    reg.add(TealResolver::new(&dir).unwrap());
    reg.install(h.lua()).unwrap();

    require(&h, "main").unwrap();
}
