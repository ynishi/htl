//! `search.lua` (#318): the `require`-resolution functions the checker prelude and a
//! split runtime state both need, in one chunk, loaded by both instead of copied into
//! each.

use htl_core::Htl;
use htl_core::mlua::{Lua, Table, Value};

mod common;

/// `search.lua`'s own contract: no `tl`, no checker prelude, nothing but `package` and
/// `load` (what `with_checker_lua`'s doc promises every program state has) plus base-
/// library functions and `string` / `table`, which every such state has always had on top
/// of that — `Lua::new()` is the safe, standard-libraries-only constructor, and does not
/// open `debug` either, which `required_from_teal` (inside `S.searcher`) copes without.
#[test]
fn the_search_chunk_loads_into_a_bare_state() {
    let lua = Lua::new();
    let s: Table = lua
        .load(include_str!("../src/search.lua"))
        .set_name("=htl-search")
        .eval()
        .expect("search.lua must load into a bare, safe state");

    let mut names: Vec<String> = s.pairs::<String, Value>().map(|p| p.unwrap().0).collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "add_path",
            "drop_cwd_path",
            "install_searcher",
            "reset_path",
            "searcher",
            "templates",
            "type_only_module",
        ],
        "search.lua's table does not have the entries it is supposed to export"
    );
}

/// A directory holding `knl.d.tl` and nothing implementing it — the fixture
/// `declaration_callers.rs` uses for the same question asked of each state alone; this
/// asks it of both at once and compares.
fn declared_only(name: &str) -> common::TempDir {
    let dir = common::tempdir("htl-core-search-chunk", name);
    std::fs::write(
        dir.join("knl.d.tl"),
        "local record knl\n   id: string\nend\nreturn knl\n",
    )
    .unwrap();
    dir
}

/// `require`s the declaration from Teal and hands back the message raised when the
/// stand-in is indexed — the text both states are held to agreeing on.
const PROBE: &str = r#"
    local ok, m = pcall(require, "knl")
    assert(ok, "a Teal require was declined: " .. tostring(m))
    local ok2, err = pcall(function() return m.id end)
    assert(not ok2, "indexing the stand-in did not raise")
    return tostring(err)
"#;

#[test]
fn both_states_answer_a_declaration_only_require_with_the_same_text() {
    // One directory for both: the message names the declaration's path, so two states
    // asked about two different directories would differ for a reason that has nothing
    // to do with this question.
    let dir = declared_only("shared");

    let checker = Htl::new().unwrap();
    checker.add_path(&dir).unwrap();
    checker.install_searcher().unwrap();
    let checker_text: String = checker
        .lua()
        .load(PROBE)
        .set_name("@caller.tl")
        .eval()
        .unwrap();

    let split_checker = Htl::new().unwrap();
    let program = Htl::with_checker(&split_checker).unwrap();
    program.add_path(&dir).unwrap();
    program.install_searcher().unwrap();
    let split_text: String = program
        .lua()
        .load(PROBE)
        .set_name("@caller.tl")
        .eval()
        .unwrap();

    assert_eq!(
        checker_text, split_text,
        "the checker and a split runtime state answered a declaration-only require \
         differently"
    );
}

/// A directory holding `htl/search.tl`, a module a project is free to write — `htl.search`
/// names no module htl itself ships or reserves, the way `tl`, `htl.lint`, `htl.fmt` and
/// `htl.test` do.
fn project_with_its_own_search_module(name: &str) -> common::TempDir {
    let dir = common::tempdir("htl-core-search-chunk", name);
    std::fs::create_dir_all(dir.join("htl")).unwrap();
    std::fs::write(
        dir.join("htl").join("search.tl"),
        "local record M\n   who: string\nend\nlocal m: M = { who = \"project\" }\nreturn m\n",
    )
    .unwrap();
    dir
}

/// #318's blocking review finding: `htl.search` must never be a name `package.preload`
/// answers, in either state, or a project's own module of that name is shadowed by the
/// internal one. Before the fix, a shared state's `require("htl.search")` got the
/// internal table (no `who` field, so `.who` is `nil`) instead of this project's module;
/// a split state never preloaded it, so the two states disagreed on what the name meant.
/// This runs the shared-state half of that disagreement, the way
/// `declaration_callers.rs`'s `a_state_with_its_own_checker_splits_a_declarations_require…`
/// runs a `.tl` through `Htl::new()` directly.
#[test]
fn a_project_module_named_htl_search_is_the_projects_own() {
    let dir = project_with_its_own_search_module("own-search-module");
    let h = Htl::new().unwrap();
    h.add_path(&dir).unwrap();
    h.install_searcher().unwrap();
    let who: String = h
        .lua()
        .load("return require('htl.search').who")
        .set_name("@caller.tl")
        .eval()
        .unwrap();
    assert_eq!(
        who, "project",
        "a project's own htl.search module was shadowed by the internal search chunk"
    );
}
