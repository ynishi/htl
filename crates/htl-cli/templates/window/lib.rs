//! The project as a library: the `fx` host module (what wants the GPU), the Teal engine
//! embedded beside it, and `preload`, which hands both — and htl-mq's `mq` — to an `Htl`.
//! The window loop is `htl_mq::run`, called from `src/main.rs`; nothing here opens one,
//! which is what keeps the engine testable with no display.

use htl::{Htl, TealRecord, host_module};

/// Channels 0.0..=1.0, the way `mq` spells them. Declared here as well because a record
/// nested in another module's declaration cannot be named from this one; both cross as a
/// plain table, so `{ r = 1, g = 0.6, b = 0.2, a = 1 }` is either.
#[derive(TealRecord, Clone, Copy)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

pub struct Fx;

/// The project's own effects, exposed to Teal as `require("fx")` beside `mq`. Declared
/// into `src/fx.d.tl` by this macro at build time (and by `htl dts` / `htl check`).
#[host_module(name = "fx", dts = "src/fx.d.tl", records = [Color])]
impl Fx {
    /// A soft glow: three circles of falling alpha. The kind of thing that belongs on this
    /// side of the boundary — per-pixel work the engine only asks for.
    pub fn draw_glow(&self, x: f32, y: f32, r: f32, color: Color) {
        // htl-mq's macroquad, not one this crate names: the window was opened with that
        // one, and a second copy would be a second set of process-global state.
        use htl_mq::macroquad::{color::Color as C, shapes::draw_circle};
        for (k, a) in [(2.2, 0.08), (1.6, 0.18), (1.0, 1.0)] {
            draw_circle(x, y, r * k, C::new(color.r, color.g, color.b, color.a * a));
        }
    }
}

// `fx` is this crate's `#[host_module]`, which the build knows without being told; `mq` is
// htl-mq's, registered by another crate, so it is named.
const BUNDLE: &[u8] = htl::include_bundle!("src/{{mod}}/init.tl", host = ["mq"]);

/// Register what this crate provides on a fresh `Htl`: `fx`, htl-mq's `mq`, `std.*`, then
/// the engine as `require("{{mod}}")`.
pub fn preload(h: &Htl) -> anyhow::Result<()> {
    Fx.htl_preload(h)?;
    htl_mq::Mq.htl_preload(h)?;
    h.install_std()?;
    h.install_bundle(&htl::bundle::Bundle::decode(BUNDLE)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::preload;
    use htl::Htl;

    /// The engine, exercised the way the binary reaches it: through `preload`, with no
    /// window — `step` moves a ball and never touches `mq`.
    #[test]
    fn a_ball_moves_through_preload() -> anyhow::Result<()> {
        let h = Htl::new()?;
        preload(&h)?;
        let x: f64 = h
            .lua()
            .load("local e = require('{{mod}}'); local s = e.new_scene(1, 800, 600); e.step(s, 0.5, 800, 600); return s.balls[1].x")
            .eval()?;
        assert!(x != 400.0, "x = {x}");
        Ok(())
    }
}
