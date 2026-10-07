//! The facts `Htl::new`'s and `with_checker_lua`'s docs state about safe vs. unsafe mlua
//! states, checked here.

use htl_core::Htl;
use htl_core::bundle::{Bundle, Kind, Module};
use htl_core::mlua::{Lua, LuaOptions, StdLib};

/// `Htl::new`'s documented reason for `unsafe_new`: the state keeps `debug`, and a safe
/// constructor refuses `StdLib::DEBUG` outright rather than quietly leaving it out.
#[test]
fn htl_new_keeps_the_debug_library_that_forces_unsafe_new() {
    let h = Htl::new().unwrap();
    let has_debug: bool = h
        .lua()
        .load("return type(debug) == 'table' and type(debug.getinfo) == 'function'")
        .eval()
        .unwrap();
    assert!(
        has_debug,
        "Htl::new() did not open a working `debug` library"
    );

    assert!(
        matches!(
            Lua::new_with(StdLib::ALL_SAFE | StdLib::DEBUG, LuaOptions::default()),
            Err(htl_core::mlua::Error::SafetyError(_))
        ),
        "a safe constructor should refuse StdLib::DEBUG"
    );
}

/// A program state the host built with a *safe* constructor still loads a bundle: the
/// binary-chunk path mlua uses does not check the state's safety flag — only `StdLib`
/// membership and `package`'s C loaders do — and bundles ask for neither.
#[test]
fn a_safe_state_loads_a_bundle() {
    let checker = Htl::new().unwrap();
    // The bytecode module and the checker's fingerprint, the shape `htl build` hands
    // `install_bundle`: stripped bytecode plus the header another state must match.
    let bytecode = checker
        .compile("answer", "return { value = 42 }\n")
        .unwrap();
    let fingerprint = checker.fingerprint().unwrap();

    // No `unsafe`: StdLib::ALL_SAFE excludes only `debug` (and, on luajit, `ffi`), and
    // `with_checker_lua` does not ask for either.
    let lua = Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default()).unwrap();
    let program = Htl::with_checker_lua(&checker, lua).unwrap();

    let bundle = Bundle {
        entry: "answer".to_string(),
        fingerprint,
        modules: vec![Module {
            name: "answer".to_string(),
            kind: Kind::Bytecode,
            payload: bytecode,
        }],
        ..Default::default()
    };
    program.install_bundle(&bundle).unwrap();

    let value: i64 = program
        .lua()
        .load("return require('answer').value")
        .eval()
        .unwrap();
    assert_eq!(value, 42, "the bundle's bytecode module did not run");
}
