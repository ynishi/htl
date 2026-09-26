-- Type declarations for htl.task (runtime: mlua-isle's task library, plus `await`).
--
-- Concurrent tasks for a program that runs on the executor (`htl run`, `htl test`, a
-- host's `Htl::run_async` / `run_blocking`). A task is a coroutine of its own that the
-- executor drives beside the one that spawned it; a host `async fn` awaited inside one
-- suspends that task and not its siblings, so two calls that each wait 300 ms take
-- 300 ms together.
--
--    local task = require("htl.task")
--    local a = task.spawn(function(): string return http.get("/a") end)
--    local b = task.spawn(function(): string return http.get("/b") end)
--    print(a:await(), b:await())
--
-- Tasks are structured. A task belongs to the coroutine that spawned it: when that
-- coroutine ends — returns, raises, or is cancelled — the tasks it did not join are
-- cancelled and waited for before its own result is delivered, and `local h <close> =
-- task.spawn(..)` does the same at the end of the scope. A cancel reaches a task as a
-- Lua error at its next await (a host `async fn` returns it, `join` returns it); a
-- `<close>` handler or a `pcall` tells it from any other error with `is_cancelled`, and
-- `[async] grace_ms` in `htl.toml` says how long a cancelled task may keep running its
-- cleanup before it is dropped.
--
-- The closure passed to `spawn` declares its return type: `Task<T>` is inferred from
-- `function(): string`, not from an untyped `function() return x end`, which the checker
-- reads as returning nothing. A task holds one value (`Task<T>`, not `Task<T, U>`);
-- return a table to hand back several. A task is awaited once: a second `await` raises
-- `htl.task: task already awaited`.
--
-- `t:await()` is the form to write. The function `task.await(t)` is the same call, but
-- the checker reads its `T` from what the caller expects before it reads the task, so
-- `print(task.await(a))` is refused (`Task<integer>` where `any` asked for `Task<any>`)
-- where `print(a:await())` checks; the function form is fine where a type is expected,
-- `local x: integer = task.await(a)`.
--
-- The module is preloaded by the binary that runs the program on the executor; Lua that
-- `htl gen` wrote from a file requiring it runs only on such a host.
local record task
   record Task<T>
      -- Wait for the task and hand back what it returned; what it raised is re-raised
      -- as the value it was (a table stays a table), and `CANCELLED` for a cancelled
      -- task, so `pcall(t.await, t)` reads as the task's own `pcall` would.
      await: function(self): T
      -- `await` without the re-raise: `true, value` when the task returned; `false, err`
      -- when it raised (`err` is the raised value itself) or was cancelled (`CANCELLED`).
      join: function(self): boolean, T
      -- Ask the task to stop. Does not wait; `join` or the scope's end does.
      cancel: function(self)
      -- Whether the task has finished, one way or the other.
      done: function(self): boolean
      -- `local h <close> = task.spawn(..)`: a task not joined when the scope ends is
      -- cancelled and waited for.
      metamethod __close: function(self)
   end

   -- Start `f(...)` as a task of the current coroutine. Only on the executor: under a
   -- plain `Htl::exec` it raises `task.spawn: not inside a coroutine request or task`.
   spawn: function<T>(f: function(any...): (T), any...): Task<T>

   -- `t:await()` as a function (see the header for when the checker prefers the method).
   await: function<T>(t: Task<T>): T

   -- Whether `err` is a cancellation: what a cancel raises inside a task (caught with
   -- `pcall`, or handed to a `__close` handler), or `CANCELLED`. `false` for anything else.
   is_cancelled: function(err: any): boolean

   -- What `join` returns as `err`, and `await` raises, for a cancelled task.
   CANCELLED: any
end

return task
