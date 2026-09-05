//! `.tl` modules resolved at runtime through mlua-pkg's `Registry`.
//!
//! Chain (first match wins):
//!   NativeResolver  "host"           Rust-built table
//!   TealResolver    scripts/*.tl     check + gen on require; `*.d.tl` -> type-only table
//!   FsResolver      scripts/*.lua    plain Lua, untouched
//!
//! Nothing is embedded: edit a `.tl` and re-run. A type error in any required `.tl`
//! fails that `require` (it does not fall through to FsResolver).

use anyhow::Result;
use htl::Htl;
use htl::pkg::TealResolver;
use mlua_pkg::Registry;
use mlua_pkg::resolvers::{FsResolver, NativeResolver};
use std::path::PathBuf;

fn main() -> Result<()> {
    let scripts = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts");
    let h = Htl::new()?;

    let mut reg = Registry::new();
    reg.add(NativeResolver::new().add("host", |lua| {
        let t = lua.create_table()?;
        t.set("name", "resolver-example")?;
        t.set(
            "double",
            lua.create_function(|_, n: f64| Ok(n * 2.0))?,
        )?;
        Ok(htl::mlua::Value::Table(t))
    }));
    reg.add(TealResolver::new(&scripts)?);
    reg.add(FsResolver::new(&scripts)?);
    reg.install(h.lua())?;

    let entry = std::env::args().nth(1).unwrap_or_else(|| "main".to_string());
    let require: htl::mlua::Function = h.lua().globals().get("require")?;
    if let Err(e) = require.call::<()>(entry.as_str()) {
        eprintln!("{e}");
        std::process::exit(1);
    }
    Ok(())
}
