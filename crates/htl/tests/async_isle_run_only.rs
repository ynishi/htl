//! `#431`: `Htl::with_checker_lua` on a state mlua-isle's `AsyncIsle` built, and
//! `htl.task` run through the isle's own executor. The mechanism is the crate doc's
//! Embedding section (`htl`'s `# Embedding`, async-feature paragraph); this asserts the
//! two numbers the issue's acceptance criteria ask for: the bundle runs to completion
//! (criterion 1), and the state's `used_memory()` stays under 100,000 bytes
//! (criterion 2).
#![cfg(feature = "async")]

use htl::Htl;
use htl::bundle::{Bundle, Kind, Module};
use htl::mlua::{Error as LuaError, Lua};
use htl::mlua_isle::AsyncIsle;

/// The bundle's one module: `require`s `htl.task` at its own top level and hands back a
/// function; nothing spawns or awaits until that function is called. See the crate
/// doc's Embedding section for why the ordering matters.
const ENTRY_SRC: &str = "\
local task = require('htl.task')
return { answer = function() return task.await(task.spawn(function() return 42 end)) end }
";

/// Requires the bundle, then calls its function — a separate coroutine turn from the
/// `require`, which is what lets the function spawn and await.
const RUN_SRC: &str = "return require('entry').answer()";

#[tokio::test]
async fn async_isle_runs_a_bundle_through_htl_task() {
    // The bundle is built ahead of time, on a throwaway checker of its own: nothing
    // about compiling it needs the isle's state.
    let build_checker = Htl::new().unwrap();
    let fingerprint = build_checker.fingerprint().unwrap();
    let bytecode = build_checker.compile("entry", ENTRY_SRC).unwrap();
    let bundle = Bundle {
        entry: "entry".to_string(),
        fingerprint,
        modules: vec![Module {
            name: "entry".to_string(),
            kind: Kind::Bytecode,
            payload: bytecode,
        }],
        ..Default::default()
    };

    // `init` gets `&Lua`; `with_checker_lua` wants an owned one. `lua.clone()` gives it
    // that, and the `Htl` built from it — and its throwaway checker — never leave this
    // closure (the crate doc's Embedding section explains why that's what makes it
    // work at all).
    let (isle, driver) = AsyncIsle::spawn(move |lua: &Lua| {
        let checker = Htl::new().map_err(LuaError::external)?;
        let h = Htl::with_checker_lua(&checker, lua.clone()).map_err(LuaError::external)?;
        h.install_task_lib().map_err(LuaError::external)?;
        h.install_bundle(&bundle).map_err(LuaError::external)?;
        Ok(())
    })
    .await
    .unwrap();

    // Criterion 2: under 100,000 bytes (#431 measured `from_lua`'s state at
    // ~3,000,000).
    let isle_used: usize = isle.exec(|lua| Ok(lua.used_memory())).await.unwrap();
    eprintln!("AsyncIsle + Htl::with_checker_lua used_memory(): {isle_used} bytes");
    assert!(
        isle_used < 100_000,
        "the split state on the isle should stay under 100,000 bytes, got {isle_used}"
    );

    // Criterion 1: run the bundle's entry (`task.spawn` + `task.await`) to completion,
    // through the isle's own coroutine executor, and read a value back.
    let value: i64 = isle.coroutine_eval(RUN_SRC).await.unwrap();
    assert_eq!(value, 42, "the bundle's task did not run to completion");

    driver.shutdown().await.unwrap();
}
