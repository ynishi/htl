//! `htl.task`: concurrent tasks for a program on the executor, typed via `task.d.tl`.
//!
//! The runtime is mlua-isle's `task` library (`Vm::task_lib`: `spawn`, `join`, `cancel`,
//! `done`, `<close>`, `is_cancelled`, `CANCELLED`, and the channels, timers and `select`
//! of `channel` / `after` / `ticker` / `select` / `select_raw` — the `runtime` module of
//! that crate is the reference for what each does) under a thin layer of htl's: `spawn`
//! hands back a proxy over the handle with one method added, `await`, which is `join`
//! with the value returned and the failure re-raised as the value it was. The
//! declaration, `htl/task.d.tl`, is what a `.tl` file sees; its header is the module's
//! doc, the Teal side of everything here included.
//!
//! Installed the way `htl.test` is ([`Htl::install_test_lib`]): a `package.preload`
//! entry and the `.d.tl` under [`crate::lib_dir`]. Both exist only with the `async`
//! feature, the one that compiles the executor: a binary without it must not declare a
//! module it cannot load (a project checked against such a declaration fails at its
//! first `require`, which is what keying [`crate::lib_dir`] on its contents rules out).
//! The preload entry is a loader, not the table: the table is the VM's, created by
//! mlua-isle on the first `require`, so installing on a state that only checks (the
//! checker's own, `include_tl!`'s) attaches nothing to it.
use crate::{Htl, write_if_changed};
use anyhow::{Context, Result};
use std::path::PathBuf;

const TASK_DTL: &str = include_str!("../lua/task.d.tl");

/// Module name of the bundled task library.
pub const TASK_LIB: &str = "htl.task";

/// htl's layer over mlua-isle's table, run once on the library's first load. `spawn`
/// returns a proxy over the handle rather than the handle: mlua-isle's handle metatable
/// is local to its own chunk, so a method of htl's (`await`) has to live on a table of
/// htl's, and the proxy forwards `join` / `cancel` / `done` / `on` / `arm` to the handle.
/// `await` is `join` with the value returned and the failure re-raised with level 0 — the
/// raised value as it was, no position prepended — so a table a task raised reaches the
/// awaiting task as that table, and a cancel as the value `is_cancelled` recognises. The
/// proxy's `__close` does what the handle's does — cancel, then join (a `__close` may
/// yield in Lua 5.4) — for a task not yet joined.
///
/// Whether a task was joined is the handle's state, not a flag of the proxy's: a `select`
/// that takes the task's case (`t:on(f)` / `t:arm()`) joins the handle inside mlua-isle,
/// where the proxy never sees it, by setting the handle's `_joined` field, which its own
/// `join` sets too. The proxy reads that field, so a second `await` — after an `await`, a
/// `join` or a chosen case — is htl's "task already awaited" and not mlua-isle's "task
/// already joined": the value was already consumed. The field is mlua-isle's, not a
/// documented API; `select_case_then_await` in `crates/htl/tests/task_lib.rs` is
/// what notices if a release renames it.
///
/// Channels, timers and select (`channel`, `after`, `ticker`, `select`, `select_raw`) are
/// mlua-isle's functions forwarded as they are: their objects carry no method of htl's.
const TASK_LUA: &str = r#"
local isle = ...
local pack, unpack, rawget = table.pack, table.unpack, rawget
local Task = {}
Task.__index = Task
local function joined(self)
   return rawget(self.h, "_joined") == true
end
function Task:join()
   return self.h:join()
end
function Task:cancel()
   self.h:cancel()
end
function Task:done()
   return self.h:done()
end
function Task:await()
   if joined(self) then error("htl.task: task already awaited", 2) end
   local r = pack(self.h:join())
   if r[1] then
      return unpack(r, 2, r.n)
   end
   error(r[2], 0)
end
function Task:on(f)
   if joined(self) then error("htl.task: task already awaited", 2) end
   return self.h:on(f)
end
function Task:arm()
   if joined(self) then error("htl.task: task already awaited", 2) end
   return self.h:arm()
end
Task.__close = function(self)
   if not joined(self) then
      self.h:cancel()
      self.h:join()
   end
end
local task = {
   CANCELLED = isle.CANCELLED,
   is_cancelled = isle.is_cancelled,
   channel = isle.channel,
   after = isle.after,
   ticker = isle.ticker,
   select = isle.select,
   select_raw = isle.select_raw,
}
function task.spawn(f, ...)
   return setmetatable({ h = isle.spawn(f, ...) }, Task)
end
function task.await(t)
   return t:await()
end
-- A task that already has its value: what `async local x = e` is checked as
-- (`of<T>(v: T): Task<T>` gives `x` the type of `e`); the generated Lua spawns instead.
-- Its handle keeps `_joined` the way mlua-isle's does. It is no task of mlua-isle's, so
-- it has no select case.
local function no_case()
   error("htl.task: a task made by task.of has no select case; await it", 2)
end
local Done = {
   join = function(self)
      if self._joined then error("task already joined", 2) end
      self._joined = true
      return true, self.v
   end,
   cancel = function() end,
   done = function() return true end,
   on = no_case,
   arm = no_case,
}
Done.__index = Done
function task.of(v)
   return setmetatable({ h = setmetatable({ v = v }, Done) }, Task)
end
return task
"#;

/// What this library writes under [`crate::lib_dir`]: its declaration, and the path it
/// takes there. A list rather than the constant because [`crate::lib_dir`] hashes it into
/// the directory's name, the way [`crate::testing::declarations`] is one.
pub(crate) fn declarations() -> Vec<(String, String)> {
    vec![("htl/task.d.tl".to_string(), TASK_DTL.to_string())]
}

/// [`crate::lib_dir`] with this library's declaration in it: `htl/task.d.tl`, written on
/// demand, only when its content changes.
pub fn lib_dir() -> Result<PathBuf> {
    let dir = crate::lib_dir();
    for (path, source) in declarations() {
        write_if_changed(&dir.join(path), &source)
            .with_context(|| format!("writing bundled declarations under {}", dir.display()))?;
    }
    Ok(dir)
}

impl Htl {
    /// Make `require("htl.task")` work at runtime and its types visible to the checker.
    ///
    /// The preload entry builds the table on the first `require`, from the state's hook
    /// owner ([`Htl::hook_owner`]) — so a state that only checks pays nothing, and a
    /// program that requires it under a plain [`Htl::exec`] gets the table and mlua-isle's
    /// error at its first `spawn` (`sync requests cannot spawn`): only a root the executor
    /// runs ([`Htl::run_async`], [`Htl::run_blocking`], `htl run`, `htl test`) can.
    pub fn install_task_lib(&self) -> Result<()> {
        let loader = self.lua.create_function(|lua, ()| {
            let vm = crate::vm(lua).map_err(mlua::Error::external)?;
            let table = vm.task_lib().map_err(mlua::Error::external)?;
            lua.load(TASK_LUA)
                .set_name(format!("={TASK_LIB}"))
                .call::<mlua::Table>(table)
        })?;
        let package: mlua::Table = self.lua.globals().get("package")?;
        let preload: mlua::Table = package.get("preload")?;
        preload.set(TASK_LIB, loader)?;
        self.add_path(&lib_dir()?)?;
        Ok(())
    }
}
