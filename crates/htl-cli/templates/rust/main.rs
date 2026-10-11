//! The binary: the library holds the host and the module, so this is only the entry
//! script and the arguments it runs with, run on the executor so `await`, `select` and
//! `htl.task` (installed in `preload`) resolve the way they do under `htl run`.

use anyhow::Context;
use htl::bundle::Bundle;
use htl::mlua::Variadic;
use htl::mlua_isle::runtime::CancelToken;
use htl::{Htl, include_bundle};
use std::process::ExitCode;

// The entry script and what it requires that the library does not already provide (a
// dependency from `mlua-pkg.toml`, say), linked at `cargo build`. `{{mod}}` comes from
// `preload` below, so it is named as the host's and left out; `host`, the library's host
// module, the build knows without being told. `debug` keeps
// line numbers, so a run-time failure inside the entry reads `main:<line>` — the line of
// `src/main.tl` — and the frames below it name their modules the same way.
const MAIN: &[u8] = include_bundle!("src/main.tl", host = ["{{mod}}"], debug = true);

fn main() -> anyhow::Result<ExitCode> {
    let h = Htl::new()?;
    {{mod}}::preload(&h)?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `load_bundle` fills `arg` (`arg[1]`.. as under `htl run`) and hands back the entry
    // as a function rather than calling it; `call_blocking` is what `htl run app.hb` runs
    // it with — a current-thread tokio runtime with a `LocalSet`, built for this call —
    // which is the executor a task, a channel or a timer needs under it.
    let main = h.load_bundle(&Bundle::decode(MAIN)?, &args)?;
    let va: Variadic<String> = args.iter().cloned().collect();
    let token = CancelToken::new();

    // Ctrl-C from a thread of its own, the way `htl run` wires it (`run_root` in
    // crates/htl-cli/src/lib.rs): a program in a CPU loop never yields to this binary's
    // own runtime, so a task on it would never run, while the cancel hook reads the
    // token from any thread at its next check. The first Ctrl-C cancels the program —
    // it gets the grace of `[async]`'s default (`htl::config::AsyncConfig`) to clean
    // up, so e.g. a `std.proc` child in its own process group is killed rather than left
    // running; the second ends the process outright.
    {
        let token = token.clone();
        std::thread::Builder::new()
            .name("{{mod}}-ctrl-c".into())
            .spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                rt.block_on(async {
                    if tokio::signal::ctrl_c().await.is_ok() {
                        token.cancel();
                    }
                    if tokio::signal::ctrl_c().await.is_ok() {
                        std::process::exit(130);
                    }
                });
            })
            .context("starting the Ctrl-C watcher")?;
    }

    match h.call_blocking(main, va, &token) {
        Ok(_) => Ok(ExitCode::SUCCESS),
        Err(e) if htl::is_cancelled(&e) => Ok(ExitCode::from(130)),
        Err(e) => Err(e),
    }
}
