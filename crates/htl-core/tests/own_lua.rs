//! A split state whose program half the host built.
//!
//! `Htl::with_checker` makes the program state itself, and makes it with every standard
//! library open. A host that wants less than that — no `os`, no `io`, a memory limit —
//! builds the `Lua` and hands it to `Htl::with_checker_lua`. The checker is not on that
//! state, so it keeps what it needs; the program gets what the host chose. Nothing here
//! is an htl limit: every bound is mlua's, set by the host, and these tests show the
//! route works through `Htl` rather than around it.

use htl_core::Htl;
use htl_core::mlua::{Lua, LuaOptions, StdLib};
use std::path::Path;

mod common;

fn write(dir: &Path, name: &str, src: &str) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, src).unwrap();
    p
}

/// A program state without `os` and `io`, built by the host. The checker's own state is
/// not touched, and a module the checker type-checked still resolves through the split
/// searcher.
#[test]
fn a_program_state_the_host_built_opens_what_the_host_chose() {
    let dir = common::scratch("htl-core-own-lua", "no-os-io");
    write(
        &dir,
        "m.tl",
        "local M = {}\nfunction M.two(): integer return 2 end\nreturn M\n",
    );

    let checker = Htl::new().unwrap();
    checker.add_path(&dir).unwrap();

    // SAFETY: the state is ours and loads nothing but the Lua below.
    let lua = unsafe {
        Lua::unsafe_new_with(
            StdLib::ALL_SAFE ^ StdLib::OS ^ StdLib::IO,
            LuaOptions::default(),
        )
    };
    let program = Htl::with_checker_lua(&checker, lua).unwrap();
    program.install_searcher().unwrap();

    program
        .exec(
            r#"
            assert(os == nil, "os is open in the program state")
            assert(io == nil, "io is open in the program state")
            local m = require("m")
            assert(m.two() == 2, "the checked module did not resolve")
            "#,
            "=own-lua",
            &[],
        )
        .unwrap();

    // The checker was never asked to give anything up.
    checker
        .exec(
            "assert(os ~= nil and io ~= nil, 'the checker lost a library')",
            "=checker",
            &[],
        )
        .unwrap();
}

/// mlua's memory limit, set by the host on the state it built, reaches a program run
/// through `Htl`: the allocation fails as mlua's own error, and the state runs the next
/// program normally.
#[test]
fn a_memory_limit_the_host_set_stops_a_program_and_the_state_goes_on() {
    let checker = Htl::new().unwrap();
    // SAFETY: as above.
    let lua = unsafe { Lua::unsafe_new_with(StdLib::ALL_SAFE, LuaOptions::default()) };
    lua.set_memory_limit(8 << 20).unwrap();
    let program = Htl::with_checker_lua(&checker, lua).unwrap();

    let err = program
        .exec("local s = string.rep('x', 1 << 26)", "=big", &[])
        .unwrap_err();
    let lua_err = err
        .downcast_ref::<htl_core::mlua::Error>()
        .unwrap_or_else(|| panic!("not an mlua error: {err:#}"));
    assert!(
        matches!(lua_err, htl_core::mlua::Error::MemoryError(_)),
        "expected MemoryError, got {lua_err:?}"
    );

    program
        .exec("local s = string.rep('x', 1 << 10)", "=small", &[])
        .unwrap();
}
