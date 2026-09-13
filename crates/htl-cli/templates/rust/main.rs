//! The binary: the library holds the host and the module, so this is only the entry
//! script and the arguments it runs with.

use htl::bundle::Bundle;
use htl::{Htl, include_bundle};

// The entry script and what it requires that the library does not already provide (a
// dependency from `mlua-pkg.toml`, say), linked at `cargo build`. `host` and `{{mod}}`
// come from `preload` below, so they are named as the host's and left out. `debug` keeps
// line numbers, so a run-time failure inside the entry reads `main:<line>` — the line of
// `src/main.tl` — and the frames below it name their modules the same way.
const MAIN: &[u8] = include_bundle!("src/main.tl", host = ["host", "{{mod}}"], debug = true);

fn main() -> anyhow::Result<()> {
    let h = Htl::new()?;
    {{mod}}::preload(&h)?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `run_bundle` fills `arg` (`arg[1]`.. as under `htl run`) and passes `...` as well.
    h.run_bundle(&Bundle::decode(MAIN)?, &args)?;
    Ok(())
}
