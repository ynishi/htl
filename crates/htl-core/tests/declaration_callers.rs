//! `Htl::install_searcher` answers a `require` of a name only a `.d.tl` declares by the
//! language of the chunk that asked (#402): Teal gets the stand-in table, plain Lua gets the
//! `Registry`'s answer — the `require` fails naming the declaration. Both halves of the
//! searcher are held to it: the checker prelude's, for a state that hosts its own checker,
//! and the runtime prelude's, for a program state split from its checker.
//!
//! The requiring chunk's language is read from its source name, so the chunks below are run
//! under `@caller.tl` and `@caller.lua` — the names `htl run` / `htl test` and Lua's own
//! searcher give a `.tl` and a `.lua`.

use htl_core::Htl;
use htl_core::mlua::{Lua, LuaOptions, StdLib};
use std::path::PathBuf;

mod common;

/// A directory holding `knl.d.tl` and nothing implementing it.
fn declared_only(name: &str) -> PathBuf {
    let dir = common::scratch("htl-core-decl-callers", name);
    std::fs::write(
        dir.join("knl.d.tl"),
        "local record knl\n   id: string\nend\nreturn knl\n",
    )
    .unwrap();
    dir
}

/// Asserts, from inside the chunk, what `pcall(require, "knl")` gave.
const PROBE_TEAL: &str = r#"
    local ok, m = pcall(require, "knl")
    assert(ok, "a Teal require was declined: " .. tostring(m))
    local ok2, err = pcall(function() return m.id end)
    assert(not ok2 and tostring(err):find("declaration-only here", 1, true), tostring(err))
"#;

const PROBE_LUA: &str = r#"
    local ok, err = pcall(require, "knl")
    assert(not ok, "a Lua require got a module")
    assert(tostring(err):find("knl.d.tl' declares it and nothing implements it", 1, true),
       tostring(err))
"#;

#[test]
fn a_state_with_its_own_checker_splits_a_declarations_require_by_the_callers_language() {
    let dir = declared_only("own");
    let h = Htl::new().unwrap();
    h.add_path(&dir).unwrap();
    h.install_searcher().unwrap();
    // Lua first: a declined `require` caches nothing, while the stand-in a Teal `require`
    // gets is in `package.loaded` from then on, for every later caller.
    h.exec(PROBE_LUA, "@caller.lua", &[]).unwrap();
    // A chunk under a label of its own is not known to be Teal, and is answered as Lua.
    h.exec(PROBE_LUA, "=host-label", &[]).unwrap();
    h.exec(PROBE_TEAL, "@caller.tl", &[]).unwrap();
}

#[test]
fn a_split_program_state_splits_a_declarations_require_by_the_callers_language() {
    let dir = declared_only("split");
    let checker = Htl::new().unwrap();
    let program = Htl::with_checker(&checker).unwrap();
    program.add_path(&dir).unwrap();
    program.install_searcher().unwrap();
    program.exec(PROBE_LUA, "@caller.lua", &[]).unwrap();
    program.exec(PROBE_TEAL, "@caller.tl", &[]).unwrap();
}

/// A host that left `debug` out cannot see who asked, and every caller gets the table, as
/// before the rule.
#[test]
fn a_program_state_without_debug_keeps_the_stand_in_for_every_caller() {
    let dir = declared_only("no-debug");
    let checker = Htl::new().unwrap();
    // SAFETY: the state is ours and loads nothing but the Lua below.
    let lua = unsafe { Lua::unsafe_new_with(StdLib::ALL_SAFE, LuaOptions::default()) };
    let program = Htl::with_checker_lua(&checker, lua).unwrap();
    program.add_path(&dir).unwrap();
    program.install_searcher().unwrap();
    program
        .exec("assert(debug == nil, 'debug is open')", "=probe", &[])
        .unwrap();
    program.exec(PROBE_TEAL, "@caller.lua", &[]).unwrap();
}
