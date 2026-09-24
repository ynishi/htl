//! The binary: the library holds `fx` and the engine, so this only opens the window and
//! hands the entry script's game table to htl-mq's loop.

use htl::bundle::Bundle;
use htl::mlua::Table;
use htl::{Htl, include_bundle};

// `mq` is htl-mq's: its declaration is `types/htl-mq/mq.d.tl`, which `htl check` writes.
// `fx` is this crate's `#[host_module]`, which the build knows without being told.
const MAIN: &[u8] = include_bundle!("src/main.tl", host = ["mq", "{{mod}}"], debug = true);

fn main() -> anyhow::Result<()> {
    let h = Htl::new()?;
    {{mod}}::preload(&h)?;
    h.install_bundle(&Bundle::decode(MAIN)?)?;
    let game: Table = h.lua().load("return require('main')").eval()?;
    // `HTL_MQ_FRAMES=60 HTL_MQ_SHOT=out.png cargo run` stops after sixty frames and writes
    // the last one, for a run nobody is watching.
    htl_mq::run(h, game, htl_mq::conf("{{name}}", 800, 600))
}
