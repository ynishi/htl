//! `.tl` modules resolved at runtime through mlua-pkg's `Registry`.
//!
//! Chain (first match wins):
//!   NativeResolver  "host"        Rust-built table
//!   TealResolver    scripts/*.tl  check + gen on require; `*.d.tl` -> steps aside
//!   FsResolver      scripts/*.lua plain Lua, untouched
//!   MemoryResolver  "embedded"    Lua source held in this file
//!
//! A `.tl` is the only thing TealResolver serves. A `.d.tl` typed the name for the
//! checker and has nothing to run, so the resolver returns `None` and the chain carries on
//! to whatever implements it (`htl::pkg` module doc) — which means every declaration here
//! needs something behind it, and has one:
//!
//!   host.d.tl      -> NativeResolver, in front of the Teal resolver
//!   legacy.d.tl    -> the sibling legacy.lua, through FsResolver behind it
//!   embedded.d.tl  -> MemoryResolver, at the end, with no file anywhere
//!   shape.d.tl     -> nothing, and nothing needs to: it is imported with `local type`,
//!                     which the generator erases, so the name is never required
//!
//! `embedded` is the one to read: nothing on disk implements it, and it resolves because
//! the declaration in `scripts/` does not end the chain. A declaration with nothing behind
//! it is a `require` that fails, naming the file.
//!
//! No `.tl` is embedded — they are read and checked on require, so edit one and re-run. A
//! type error in any required `.tl` fails that `require` (it does not fall through to
//! FsResolver).

use anyhow::Result;
use htl::Htl;
use htl::pkg::TealResolver;
use mlua_pkg::Registry;
use mlua_pkg::resolvers::{FsResolver, MemoryResolver, NativeResolver};
use std::path::PathBuf;

fn main() -> Result<()> {
    let scripts = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts");
    let h = Htl::new()?;

    let mut reg = Registry::new();
    reg.add(NativeResolver::new().add("host", |lua| {
        let t = lua.create_table()?;
        t.set("name", "resolver-example")?;
        t.set("double", lua.create_function(|_, n: f64| Ok(n * 2.0))?)?;
        Ok(htl::mlua::Value::Table(t))
    }));
    reg.add(TealResolver::new(&scripts)?);
    reg.add(FsResolver::new(&scripts)?);
    // Last: a module with no file anywhere, declared by `scripts/embedded.d.tl` and reached
    // only because TealResolver steps aside for a declaration.
    reg.add(MemoryResolver::new().add(
        "embedded",
        "return { hello = function(name) return 'hello, ' .. name .. ' (from Rust memory)' end }",
    ));
    reg.install(h.lua())?;

    let entry = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "main".to_string());
    let require: htl::mlua::Function = h.lua().globals().get("require")?;
    if let Err(e) = require.call::<()>(entry.as_str()) {
        eprintln!("{e}");
        std::process::exit(1);
    }
    Ok(())
}
