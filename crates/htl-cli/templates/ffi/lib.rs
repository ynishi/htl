//! The project as a library with a C ABI on top: the Rust host the Teal side calls, the
//! module embedded beside it, `preload`, and a `Game` that a caller which is not written
//! in Rust holds through `#[c_export]`.
//!
//! Two boundaries, pointing opposite ways, from the same crate:
//!
//! - `#[host_module]` on `Host` is what the *scripts* call. Its declaration is written to
//!   `src/host.d.tl`, so a mod that requires `host` is type-checked against the Rust
//!   signatures.
//! - `#[c_export]` on `Game` is what the *caller* calls — a Unity script, a Swift app, a
//!   Python REPL. It writes one `extern "C"` wrapper per method and `include/{{mod}}.h`,
//!   from the same breakdown, so a rename here moves the header with it.
//!
//! There is no binary: `--host ffi` is a library, and what a host loads is
//! `target/debug/lib{{mod}}.{so,dylib,dll}` (or `lib{{mod}}.a` to link statically).
//! `examples/c/` and `examples/python/` are two such hosts, each written the way its
//! language has to be written to not leak or corrupt the strings it is handed.

use htl::{Htl, c_export, ffi, host_module};

pub struct Host;

/// Exposed to Teal as `require("host")`. Its declaration is written to `src/host.d.tl`
/// by this macro at build time, and by `htl dts` / `htl check` without building.
#[host_module(name = "host", dts = "src/host.d.tl")]
impl Host {
    pub fn shout(&self, text: &str) -> String {
        text.to_uppercase()
    }
}

// The Teal module, type-checked at `cargo build` and embedded as stripped bytecode. Keep
// this after `#[host_module]` (same file, source order) so the declaration exists when the
// module is checked.
const MODULE: &[u8] = htl::include_tl_bytes!("src/{{mod}}/init.tl");

/// Register what this crate provides on a fresh `Htl`: the Rust `host` module, then the
/// Teal module as `require("{{mod}}")`.
pub fn preload(h: &Htl) -> anyhow::Result<()> {
    Host.htl_preload(h)?;
    h.preload_bytes("{{mod}}", MODULE)?;
    Ok(())
}

/// What `{{mod}}_open` is given, as one JSON object. Everything the library would
/// otherwise read from the environment or the working directory is passed in here — a
/// game engine and a sandboxed app provide neither.
#[derive(serde::Deserialize)]
struct Options {
    /// Who the greetings come from.
    greeter: String,
}

/// What `{{mod}}_state` hands back: a record, so it crosses as JSON with a `"v"` schema
/// version the runtime stamps on it.
#[derive(serde::Serialize)]
pub struct State {
    pub greeter: String,
    pub greeted: i32,
}

/// One of these is what a `{{mod}}_handle *` points at: the Lua state, and whatever the
/// C caller's session is made of. One per thread — see `{{mod}}_threadsafe()`.
pub struct Game {
    h: Htl,
    greeter: String,
    greeted: i32,
}

/// Every method here is one of four shapes, and anything else is a compile error naming
/// the type: `()` is `void`, a `String` or a serde type is `char *` (yours to free with
/// `{{mod}}_free`), a `Result<(), E>` is an `int` status, and an `i32` is an `int` status
/// with the value written through `int *out`.
#[c_export(prefix = "{{mod}}", header = "include/{{mod}}.h")]
impl Game {
    /// The opener. The options as one JSON object, and the flag `{{mod}}_interrupt` sets:
    /// the hook goes on the Lua state while it is being built, which is the one moment
    /// the handle and the state exist together.
    pub fn open(options: &str, interrupt: ffi::Interrupt) -> Result<Self, String> {
        let o: Options = ffi::from_json(options, "options_json")?;
        let h = Htl::new().map_err(|e| e.to_string())?;
        preload(&h).map_err(|e| e.to_string())?;
        interrupt.install(&h).map_err(|e| e.to_string())?;
        Ok(Game {
            h,
            greeter: o.greeter,
            greeted: 0,
        })
    }

    /// `char *`: the text itself. The work happens in Teal, so a mod's `error()` reaches
    /// the caller as `NULL` with the `LUA` status from `{{mod}}_last_status()` — greet
    /// the empty string to see it.
    pub fn greet(&mut self, who: &str) -> Result<String, htl::mlua::Error> {
        let g: htl::mlua::Table = self
            .h
            .lua()
            .load("return require('{{mod}}').greet(...)")
            .call(who)?;
        self.greeted += 1;
        Ok(format!("{}: {}", self.greeter, g.get::<String>("text")?))
    }

    /// `char *`: JSON, with `"v"` on it.
    pub fn state(&self) -> State {
        State {
            greeter: self.greeter.clone(),
            greeted: self.greeted,
        }
    }

    /// `Result<(), E>` -> `int`: a status and nothing else. Refusing a reset that would
    /// do nothing is what gives a caller an `ERR` to handle that is not a Lua error.
    pub fn reset(&mut self) -> Result<(), String> {
        if self.greeted == 0 {
            return Err("nothing to reset: nobody has been greeted yet".into());
        }
        self.greeted = 0;
        Ok(())
    }

    /// `i32` -> `int` with an out-parameter: the value never rides in the status, so a
    /// count of `-1` is a count and not an error.
    pub fn greeted(&self) -> i32 {
        self.greeted
    }
}

#[cfg(test)]
mod tests {
    use super::{Game, preload};
    use htl::{Htl, ffi::CExport};

    /// The Teal side, exercised the way a host does it: through `preload`, so the test
    /// fails if the module stops loading or `greet` changes shape.
    #[test]
    fn greet_comes_back_through_preload() -> anyhow::Result<()> {
        let h = Htl::new()?;
        preload(&h)?;
        let g: htl::mlua::Table = h
            .lua()
            .load("return require('{{mod}}').greet('rust')")
            .eval()?;
        assert_eq!(g.get::<String>("text")?, "hello, rust");
        Ok(())
    }

    /// The C side without a C compiler: the header is a constant in the library, so what
    /// `examples/c/` and `examples/python/` call can be checked here. `cargo test` on
    /// this crate is therefore what catches a rename before a host does.
    #[test]
    fn the_header_declares_what_the_examples_call() {
        assert_eq!(<Game as CExport>::PREFIX, "{{mod}}");
        let h = <Game as CExport>::HEADER;
        for want in [
            "{{mod}}_handle *{{mod}}_open(const char *options_json);",
            "void {{mod}}_close({{mod}}_handle *h);",
            "char *{{mod}}_greet({{mod}}_handle *h, const char *who);",
            "char *{{mod}}_state({{mod}}_handle *h);",
            "int {{mod}}_reset({{mod}}_handle *h);",
            "int {{mod}}_greeted({{mod}}_handle *h, int *out);",
            "void {{mod}}_free(char *s);",
        ] {
            assert!(h.contains(want), "the header is missing `{want}`:\n{h}");
        }
    }
}
