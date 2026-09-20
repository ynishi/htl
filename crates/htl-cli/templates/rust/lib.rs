//! The project as a library: the Rust host, the Teal module embedded beside it, and
//! `preload`, which hands both to an `Htl`. Everything that grows lives here — a second
//! `#[host_module]`, a Rust test, an `extern "C"` layer, a window loop — and every entry
//! point (the binary, a test, another crate embedding this one) goes through `preload`.

use htl::{Htl, TealRecord, host_module};

/// Crosses the Rust <-> Teal boundary as a plain table (`host.Point` on the Teal side).
#[derive(TealRecord, Clone)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

pub struct Host;

/// Exposed to Teal as `require("host")`. Its declaration is written to `src/host.d.tl`
/// by this macro at build time, and by `htl dts` / `htl check` without building.
#[host_module(name = "host", dts = "src/host.d.tl", records = [Point])]
impl Host {
    pub fn greet(&self, who: &str) -> String {
        format!("hello from Rust, {who}")
    }

    pub fn scale(&self, p: Point, k: f64) -> Point {
        Point { x: p.x * k, y: p.y * k }
    }
}

// The Teal module and everything it requires, linked at `cargo build` and embedded as one
// bundle of stripped bytecode: a dependency from `mlua-pkg.toml` rides along, and a
// `require` that resolves to nothing fails the build here rather than at run time. `host`
// is this crate's; a name declared only by a `.d.tl` (`std.*`) is the host's too. Keep
// this after `#[host_module]` (same file, source order) so the declaration exists when the
// closure is checked.
const BUNDLE: &[u8] = htl::include_bundle!("src/{{mod}}/init.tl", host = ["host"]);

/// Register what this crate provides on a fresh `Htl`: the Rust `host` module, then the
/// Teal module as `require("{{mod}}")`.
pub fn preload(h: &Htl) -> anyhow::Result<()> {
    Host.htl_preload(h)?;
    // `std.*`: json, string, path and the rest, from mlua-batteries; typed in the checker
    // the same way. Remove this line and the project has no native modules but `host`.
    h.install_std()?;
    // Every module in the bundle goes into `package.preload`; a name already there
    // (`host`, `std.*`) stays the host's. Stripped bytecode is small and has neither line
    // numbers nor a chunk name, so a failure inside this module reads `?: in function
    // '{{mod}}.greet'`. `htl run src/{{mod}}/init.tl` and `htl test` run the Teal itself
    // and name file and line.
    h.install_bundle(&htl::bundle::Bundle::decode(BUNDLE)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::preload;
    use htl::Htl;

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
}
