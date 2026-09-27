//! A declaration served through a `Registry` in a split state (`Htl::with_checker_lua`).
//!
//! The checker's prelude lives in another `Lua`, so a table built from it and returned
//! into the program state is `Lua instance passed Value created from a different main Lua
//! state` — a panic, not an error a host can catch. Stepping aside for a `.d.tl` means the
//! resolver builds no table at all, and the message a host gets when nothing implements
//! the name comes from a `package.searchers` entry made in the program state with
//! `lua.create_function`, so nothing crosses.

use htl_core::Htl;
use htl_core::mlua::Lua;
use htl_core::pkg::TealResolver;
use mlua_pkg::Registry;
use mlua_pkg::resolvers::MemoryResolver;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};

mod common;

const DECL: &str = "local record foo\n   t: function(): string\nend\nreturn foo\n";
const IMPL: &str = "return { t = function() return 'REAL' end }\n";

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-split-declarations", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Nothing behind the declaration: an error naming the file, and no panic on the way.
/// Before the fix this `require` took the host's thread down instead of failing.
#[test]
fn a_declaration_in_a_split_state_errors_rather_than_panicking() {
    let root = scratch("alone");
    write(&root.join("foo.d.tl"), DECL);

    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let checker = Htl::new().unwrap();
        let program = Htl::with_checker_lua(&checker, Lua::new()).unwrap();
        let mut reg = Registry::new();
        reg.add(TealResolver::new(&root).unwrap());
        reg.install(program.lua()).unwrap();
        program
            .lua()
            .load("return require('foo')")
            .eval::<htl_core::mlua::Value>()
            .map(|_| ())
    }));

    let result = outcome.expect("require of a declaration must not panic the host");
    let err = result.expect_err("nothing implements foo").to_string();
    assert!(err.contains("foo.d.tl"), "names the declaration: {err}");
    assert!(err.contains("nothing implements it"), "{err}");
}

/// #295's case A in a split state: the declaration root first, the module in memory.
#[test]
fn a_declaration_in_a_split_state_steps_aside_for_an_embedded_module() {
    let root = scratch("embedded");
    write(&root.join("foo.d.tl"), DECL);

    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let checker = Htl::new().unwrap();
        let program = Htl::with_checker_lua(&checker, Lua::new()).unwrap();
        let mut reg = Registry::new();
        reg.add(TealResolver::new(&root).unwrap());
        reg.add(MemoryResolver::new().add("foo", IMPL));
        reg.install(program.lua()).unwrap();
        program
            .lua()
            .load("return require('foo').t()")
            .eval::<String>()
    }));

    let result = outcome.expect("require of a declaration must not panic the host");
    assert_eq!(result.unwrap(), "REAL");
}
