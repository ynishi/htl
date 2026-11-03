//! `htl.trait` in a host that installed nothing: `Htl::new()` and a chunk that requires
//! it. The module is pure Lua with no host behind it, so it is part of every state rather
//! than something an embedder opts into the way `htl.task` is.

use htl::Htl;

const CHUNK: &str = "local trait = require('htl.trait')\n\
     local defaults = { describe = function() return 'default' end, area = function() return 0 end }\n\
     local Circle = { area = function() return 3 end }\n\
     trait.impl(defaults, Circle)\n\
     return Circle.describe() .. ':' .. Circle.area()";

/// Acceptance 4: a host that only calls `Htl::new()` runs a chunk that requires
/// `htl.trait`, and `impl` copies what the record lacks and keeps what it has.
#[test]
fn a_state_from_new_has_the_module() {
    let h = Htl::new().unwrap();
    let got: String = h.lua().load(CHUNK).eval().unwrap();
    assert_eq!(got, "default:3");
}

/// The program state of a split host (`with_checker_lua`, what an embedder with a state of
/// its own calls) has it too, with the checker's `Htl::new` the only other call.
#[test]
fn a_program_state_made_from_a_checker_has_the_module() {
    let checker = Htl::new().unwrap();
    let h = Htl::with_checker_lua(&checker, htl::mlua::Lua::new()).unwrap();
    let got: String = h.lua().load(CHUNK).eval().unwrap();
    assert_eq!(got, "default:3");
}
