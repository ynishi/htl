//! `std.*`: mlua-batteries inside the binary, installed the way `htl.test` is.
//!
//! mlua-batteries registers its modules under a prefix the host chooses and ships a `.d.tl`
//! for each of them with the prefix left out — the crate's position is that `std` is the
//! host's namespace to assemble, so nothing it ships claims it. htl is that host: with the
//! `std` feature, [`Htl::install_std`] preloads every module the feature set built as
//! `std.<name>` (`require("std.json")`, `require("std.string")`, and `require("std")` for
//! the namespace table) and puts their declarations under [`lib_dir`] where the checker
//! searches, so a `.tl` that requires one is typed in `htl check`, `htl test` and
//! `include_tl!` without the project holding a copy.
//!
//! What is in it is the crate's default feature set and not `full`: json, env, path, time,
//! string, validate, pretty, argparse. Those add serde_json to the build and nothing else.
//! A module that reaches the file system, the network or a runtime (fs / http / llm /
//! task) is a decision about what a script may do, and that is a host's to make in its
//! own `Cargo.toml` with its own prefix — not something a toolchain turns on for every
//! project it runs.
//!
//! The declarations are the crate's, written verbatim; the only generated file is
//! `std/init.d.tl`, the record behind `require("std")`, which the crate renders for the
//! prefix. Both are written with [`write_if_changed`] rather than the crate's own
//! `dts::write_to`, which rewrites unconditionally: this directory is read by every
//! command on every run, and a file whose mtime moves on each of them is a file every
//! cache above it has to re-read.

use crate::{Htl, lib_dir, write_if_changed};
use anyhow::{Context, Result};
use std::path::PathBuf;

/// The `require` prefix: `std.json`, `std.string`, ... and `std` itself.
pub const PREFIX: &str = "std";

/// The crate the modules come from, as cargo names it. `dep_dts` reads it: a Rust host
/// that has `std` on has this crate in its graph, and the declarations it names in
/// `[package.metadata.htl] dts` are the ones already under [`lib_dir`] — materialising
/// them a second time under `types/mlua-batteries/` would put the same modules on the
/// path twice, under a prefix nothing preloads.
pub const CRATE: &str = "mlua-batteries";

/// The directory the `std` declarations are written to: `<lib_dir>/std/`.
fn std_dir() -> PathBuf {
    lib_dir().join(PREFIX)
}

/// Write `std/<module>.d.tl` for every module the build carries, and `std/init.d.tl`,
/// under [`lib_dir`]. Returns the directory the checker should search.
fn write_declarations() -> Result<PathBuf> {
    let dir = std_dir();
    let write = |name: &str, text: &str| {
        write_if_changed(&dir.join(format!("{name}.d.tl")), text)
            .with_context(|| format!("writing bundled declarations under {}", dir.display()))
    };
    for e in mlua_batteries::dts::entries() {
        write(e.name, e.source)?;
    }
    write("init", &mlua_batteries::dts::init_source(PREFIX))?;
    Ok(lib_dir())
}

/// Re-wrap each `std.<module>` loader so that a failing call raises a plain string.
///
/// A Rust function that returns `Err` raises, on the Lua side, a userdata whose
/// `tostring` is the error text followed by `stack traceback:` and the frames — mlua
/// builds that for every function it creates, and it is the right thing for the host,
/// which gets the cause and the trace in one value. It is the wrong thing for the script
/// that `pcall`s the function: `err` is not a string, `"failed: " .. err` is a type error,
/// and `tostring(err)` prints five lines where a Lua `error("...")` prints one. So each
/// function of a `std` module is called through `pcall`, and a failure is raised again as
/// `error(<text before the traceback>, 0)`: what `pcall` receives is then the same kind of
/// value it gets from Lua code, `"json.decode: EOF while parsing an object at line 1 column
/// 1"`. The cut is the one [`strip_traceback`](crate::strip_traceback) makes on the Rust
/// side. Level 0 because the text already names the function; a `main.tl:12:` prefix would
/// point at this shim.
///
/// The loaders are wrapped, not the tables: a module is built on its first `require`, and
/// the namespace loader builds itself out of `require` too, so the four lines below reach
/// every function through the one place all of them pass. Values that are not functions
/// (`json.null`) are left as they are.
const WRAP_LOADERS: &str = r#"
local prefix, names = ...
local preload = package.preload
local function plain(err)
   local msg = tostring(err)
   local at = msg:find("\nstack traceback:", 1, true)
   if at then msg = msg:sub(1, at - 1) end
   return msg
end
local function wrap(f)
   return function(...)
      local r = table.pack(pcall(f, ...))
      if r[1] then return table.unpack(r, 2, r.n) end
      error(plain(r[2]), 0)
   end
end
for _, name in ipairs(names) do
   local key = prefix .. "." .. name
   local loader = preload[key]
   preload[key] = function(...)
      local m = loader(...)
      for k, v in pairs(m) do
         if type(v) == "function" then m[k] = wrap(v) end
      end
      return m
   end
end
"#;

impl Htl {
    /// Make `require("std.<module>")` work at runtime and its types visible to the checker.
    ///
    /// Idempotent in effect: preloading again replaces the same `package.preload` entries
    /// with equivalent loaders, and the declarations are rewritten only when they differ.
    pub fn install_std(&self) -> Result<()> {
        mlua_batteries::preload_all(self.lua(), PREFIX)
            .context("registering std.* (mlua-batteries) in package.preload")?;
        let names: Vec<&str> = mlua_batteries::dts::entries()
            .into_iter()
            .map(|e| e.name)
            .collect();
        self.lua()
            .load(WRAP_LOADERS)
            .set_name("=std")
            .call::<()>((PREFIX, names))
            .context("wrapping std.* loaders")?;
        self.add_path(&write_declarations()?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_module_the_build_carries_is_required_as_std() -> Result<()> {
        let h = Htl::new()?;
        h.install_std()?;
        for e in mlua_batteries::dts::entries() {
            let t: mlua::Table = h
                .lua()
                .load(format!("return require('std.{}')", e.name))
                .eval()
                .with_context(|| format!("std.{}", e.name))?;
            assert!(t.len()? > 0 || t.pairs::<String, mlua::Value>().count() > 0);
        }
        let ns: mlua::Table = h.lua().load("return require('std')").eval()?;
        assert!(ns.contains_key("json")?);
        Ok(())
    }

    #[test]
    fn the_declarations_are_on_the_path_and_the_namespace_is_typed() -> Result<()> {
        let h = Htl::new()?;
        h.install_std()?;
        let dir = std_dir();
        assert!(dir.join("json.d.tl").is_file(), "{}", dir.display());
        assert!(dir.join("init.d.tl").is_file(), "{}", dir.display());
        let init = std::fs::read_to_string(dir.join("init.d.tl"))?;
        assert!(init.contains("local record std\n"), "{init}");
        assert!(init.contains("require(\"std.json\")"), "{init}");
        Ok(())
    }

    /// What a script's `pcall` gets from a failing `std` function: a string, one line, the
    /// text mlua would have put before `stack traceback:` — the same kind of value a Lua
    /// `error("...")` gives it. Through the namespace too, since it is the same table.
    #[test]
    fn a_failing_std_call_raises_a_plain_string_without_a_traceback() -> Result<()> {
        let h = Htl::new()?;
        h.install_std()?;
        let (kind, msg): (String, String) = h
            .lua()
            .load(
                "local json = require('std.json')\n\
                 local ok, err = pcall(json.decode, '{')\n\
                 assert(not ok)\n\
                 return type(err), err",
            )
            .eval()?;
        assert_eq!(kind, "string");
        assert_eq!(
            msg,
            "json.decode: EOF while parsing an object at line 1 column 1"
        );
        let via_ns: String = h
            .lua()
            .load(
                "local std = require('std')\n\
                 local ok, err = pcall(std.json.decode, '[1,')\n\
                 assert(not ok)\n\
                 return err",
            )
            .eval()?;
        assert!(!via_ns.contains("traceback"), "{via_ns}");
        assert!(via_ns.starts_with("json.decode: "), "{via_ns}");
        assert_eq!(via_ns.lines().count(), 1, "{via_ns}");
        Ok(())
    }

    /// The shim passes every return value through, `nil` included: `json.decode("null")`
    /// is `nil` and not an error, and a call returning nothing stays a call returning
    /// nothing.
    #[test]
    fn a_succeeding_std_call_returns_what_it_returned() -> Result<()> {
        let h = Htl::new()?;
        h.install_std()?;
        let (n, first, rest): (i64, mlua::Value, String) = h
            .lua()
            .load(
                "local json = require('std.json')\n\
                 local r = table.pack(json.decode('null'))\n\
                 local s = require('std.string')\n\
                 return r.n, r[1], table.concat(s.split('a,b', ','), '+')",
            )
            .eval()?;
        assert_eq!(n, 1);
        assert!(matches!(first, mlua::Value::Nil));
        assert_eq!(rest, "a+b");
        Ok(())
    }

    /// A `.tl` that requires `std.json` checks clean and runs: the same file through the
    /// checker (declaration) and the program state (preload).
    #[test]
    fn a_script_using_std_checks_and_runs() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("htl-std-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let file = dir.join("main.tl");
        std::fs::write(
            &file,
            "local json = require(\"std.json\")\n\
             local s = require(\"std.string\")\n\
             local t: {string:integer} = { a = 1 }\n\
             local out = json.encode(t)\n\
             assert(out == '{\"a\":1}', out)\n\
             assert(s.trim(\"  x \") == \"x\")\n",
        )?;
        let h = Htl::new()?;
        h.install_std()?;
        let (code, info) = h.gen_lua(&file)?;
        assert!(info.ok(), "{info:?}");
        h.exec(&code.expect("generated"), "@main.tl", &[])?;
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }
}
