//! A `.d.tl` is a declaration, not a module: `TealResolver` steps aside for it so the
//! rest of the mlua-pkg chain is asked, and something in that chain has to implement the
//! name — a declaration nobody implements is a `require` that fails, naming the file.
//!
//! The four cases of #295's evidence table, plus the ends of the rule: nothing behind the
//! declaration at all, a `.tl` that fails its check (which is `Some(Err)` and must still
//! not fall through), and the import to write when a declaration's *types* are all that
//! is wanted, which needs no implementation because it generates no `require`.

use htl_core::Htl;
use htl_core::pkg::TealResolver;
use mlua_pkg::Registry;
use mlua_pkg::resolvers::{FsResolver, MemoryResolver, NativeResolver};
use std::path::{Path, PathBuf};

mod common;

/// Types only — what a `.d.tl` is for.
const DECL: &str = "local record foo\n   t: function(): string\nend\nreturn foo\n";
/// The implementation the chain has to reach.
const IMPL: &str = "return { t = function() return 'REAL' end }\n";

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-declarations", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn call_t(h: &Htl) -> mlua::Result<String> {
    h.lua().load("return require('foo').t()").eval()
}

/// A: the declaration root comes first and the implementation is embedded, with no file
/// behind it anywhere. Only stepping aside reaches it.
#[test]
fn a_declaration_root_first_then_an_embedded_module() {
    let root1 = scratch("a");
    write(&root1.join("foo.d.tl"), DECL);

    let h = Htl::new().unwrap();
    let mut reg = Registry::new();
    reg.add(TealResolver::new(&root1).unwrap());
    reg.add(FsResolver::new(&root1).unwrap());
    reg.add(MemoryResolver::new().add("foo", IMPL));
    reg.install(h.lua()).unwrap();

    assert_eq!(call_t(&h).unwrap(), "REAL");
}

/// B: two filesystem tiers. The declaration is in the first, the `.lua` in the second —
/// precisely what the old comment claimed it stepped aside for and did not.
#[test]
fn a_declaration_in_one_tier_and_the_implementation_in_the_next() {
    let root1 = scratch("b1");
    let root2 = scratch("b2");
    write(&root1.join("foo.d.tl"), DECL);
    write(&root2.join("foo.lua"), IMPL);

    let h = Htl::new().unwrap();
    let mut reg = Registry::new();
    reg.add(TealResolver::new(&root1).unwrap());
    reg.add(FsResolver::new(&root1).unwrap());
    reg.add(TealResolver::new(&root2).unwrap());
    reg.add(FsResolver::new(&root2).unwrap());
    reg.install(h.lua()).unwrap();

    assert_eq!(call_t(&h).unwrap(), "REAL");
}

/// C: `package.preload`. The Registry's hook is searcher 1 and Lua's preload searcher is
/// 2, so stepping aside is what hands the name over to it.
#[test]
fn a_declaration_steps_aside_for_a_preloaded_implementation() {
    let root1 = scratch("c");
    write(&root1.join("foo.d.tl"), DECL);

    let h = Htl::new().unwrap();
    h.lua()
        .load("package.preload['foo'] = function() return { t = function() return 'REAL' end } end")
        .exec()
        .unwrap();
    let mut reg = Registry::new();
    reg.add(TealResolver::new(&root1).unwrap());
    reg.add(FsResolver::new(&root1).unwrap());
    reg.install(h.lua()).unwrap();

    assert_eq!(call_t(&h).unwrap(), "REAL");
}

/// D: the `.lua` sibling in the resolver's own root — the one case the old code did
/// handle, now handled by the `FsResolver` behind it rather than by a filesystem probe.
#[test]
fn a_declaration_steps_aside_for_a_lua_sibling() {
    let root1 = scratch("d");
    write(&root1.join("foo.d.tl"), DECL);
    write(&root1.join("foo.lua"), IMPL);

    let h = Htl::new().unwrap();
    let mut reg = Registry::new();
    reg.add(TealResolver::new(&root1).unwrap());
    reg.add(FsResolver::new(&root1).unwrap());
    reg.install(h.lua()).unwrap();

    assert_eq!(call_t(&h).unwrap(), "REAL");
}

/// E: nothing behind the declaration. The failure is at `require`, and it names the
/// declaration file and both ways of giving the name something to resolve to.
#[test]
fn a_declaration_nothing_answers_fails_at_require() {
    let root1 = scratch("e");
    write(&root1.join("foo.d.tl"), DECL);

    let h = Htl::new().unwrap();
    let mut reg = Registry::new();
    reg.add(TealResolver::new(&root1).unwrap());
    reg.install(h.lua()).unwrap();

    let err = h
        .lua()
        .load("return require('foo')")
        .eval::<mlua::Value>()
        .unwrap_err()
        .to_string();
    assert!(err.contains("foo.d.tl"), "names the declaration: {err}");
    assert!(err.contains("nothing implements it"), "{err}");
    assert!(
        err.contains("local type foo = require(\"foo\")"),
        "offers the types-only import: {err}"
    );
}

/// The import to write when a declaration's types are all that is wanted. Teal has a
/// dedicated form for it, and the generator erases the whole statement — so the module is
/// never required at run time and needs no implementation behind it. This is what the
/// example's `shape` should have been, and what the E message points a reader at.
#[test]
fn a_types_only_import_generates_no_require_and_needs_nothing() {
    let root1 = scratch("types-only");
    write(
        &root1.join("shape.d.tl"),
        "local record shape\n   record Point\n      x: number\n      y: number\n   end\nend\nreturn shape\n",
    );
    let use_tl = root1.join("use.tl");
    write(
        &use_tl,
        "local type shape = require(\"shape\")\n\nlocal p: shape.Point = { x = 2, y = 3 }\nreturn p.x + p.y\n",
    );

    // It checks clean against the declaration...
    let h = Htl::new().unwrap();
    h.add_path(&root1).unwrap();
    let (code, ci) = h.gen_lua(&use_tl).unwrap();
    assert!(ci.ok(), "{:?}", ci.errors);
    let code = code.expect("generated");
    assert!(
        !code.contains("require"),
        "the type import must be erased, not emitted: {code}"
    );

    // ...and at run time the name is never asked for, so an empty chain is enough.
    let h = Htl::new().unwrap();
    let mut reg = Registry::new();
    reg.add(TealResolver::new(&root1).unwrap());
    reg.install(h.lua()).unwrap();
    let sum: f64 = h.lua().load("return require('use')").eval().unwrap();
    assert_eq!(sum, 5.0);
}

/// F: the rule that does not move. A `.tl` with a type error is `Some(Err)`, so the
/// chain stops there and the `.lua` behind it is never served.
#[test]
fn a_failing_tl_still_does_not_fall_through() {
    let root1 = scratch("f");
    write(
        &root1.join("foo.tl"),
        "local n: number = 'nope'\nreturn n\n",
    );
    write(&root1.join("foo.lua"), IMPL);

    let h = Htl::new().unwrap();
    let mut reg = Registry::new();
    reg.add(TealResolver::new(&root1).unwrap());
    reg.add(FsResolver::new(&root1).unwrap());
    reg.install(h.lua()).unwrap();

    let err = h
        .lua()
        .load("return require('foo')")
        .eval::<mlua::Value>()
        .unwrap_err()
        .to_string();
    assert!(err.contains("Teal type check failed"), "{err}");
}

/// The README's relaxed rule: a native module no longer has to be registered in front of
/// the Teal resolver, because the declaration that types it no longer ends the chain.
#[test]
fn a_native_module_may_be_registered_after_the_teal_resolver() {
    let root1 = scratch("native");
    write(&root1.join("foo.d.tl"), DECL);

    let h = Htl::new().unwrap();
    let mut reg = Registry::new();
    reg.add(TealResolver::new(&root1).unwrap());
    reg.add(NativeResolver::new().add("foo", |lua| {
        let t = lua.create_table()?;
        t.set("t", lua.create_function(|_, ()| Ok("REAL"))?)?;
        Ok(mlua::Value::Table(t))
    }));
    reg.install(h.lua()).unwrap();

    assert_eq!(call_t(&h).unwrap(), "REAL");
}
