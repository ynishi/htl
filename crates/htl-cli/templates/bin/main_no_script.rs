//! Rust host: the Teal sources are type-checked at `cargo build` and embedded.

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

// Teal sources, checked at build. Keep these after `#[host_module]` (same file, source order)
// so the declaration exists when they are checked.
const LIB: &[u8] = htl::include_tl_bytes!("src/{{mod}}/init.tl");

fn main() -> anyhow::Result<()> {
    let h = Htl::new()?;
    Host.htl_preload(&h)?;
    h.preload_bytes("{{mod}}", LIB)?;
    let g: htl::mlua::Table = h.lua().load("return require('{{mod}}').greet('rust')").eval()?;
    println!("{}", g.get::<String>("text")?);
    Ok(())
}
