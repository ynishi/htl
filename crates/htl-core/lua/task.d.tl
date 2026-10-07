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
-- `htl.task: task already awaited`, and so does an `await` after a `select` took the
-- task's case (`t:on(f)`), which counts as the one join.
--
-- `t:await()` is the form to write. The function `task.await(t)` is the same call, but
-- the checker reads its `T` from what the caller expects before it reads the task, so
-- `print(task.await(a))` is refused (`Task<integer>` where `any` asked for `Task<any>`)
-- where `print(a:await())` checks; the function form is fine where a type is expected,
-- `local x: integer = task.await(a)`.
--
-- Channels, timers and select. A loop that waits on several sources at once — values
-- from other tasks, events a host pushes, a timeout, a periodic tick — takes whichever
-- is ready first with `select`:
--
--    local jobs: task.Channel<string> = task.channel(16)
--    local tick = task.ticker(1000)
--    while true do
--       local stop = await task.select({
--          jobs:on(function(job: string, ok: boolean): boolean
--             if ok then handle(job) end
--             return not ok           -- closed and empty: leave the loop
--          end),
--          tick:on(function(_ms: number, _ok: boolean): boolean return false end),
--          task.after(5000):on(function(): boolean return true end),
--       })
--       if stop then break end
--    end
--
-- A case (`ch:on(f)`, `ch:on_send(v, f)`, `t:on(f)`, `h:on(f)`) is a handler with the
-- value it receives typed from the source, and every case of one `select` returns the
-- same `R`: the checker refuses a handler whose parameter is not the channel's element
-- type, and a case list that mixes result types. `select` returns what the chosen
-- handler returned. The handlers run as coroutines, so they may `await`. `select_raw`
-- takes `arm` cases instead and hands back the index of the one chosen and its values,
-- untyped (`any`), for a loop that would rather branch on an index.
--
-- `select`, `select_raw` and the waiting methods (`send`, `recv`, `wait`)
-- suspend, and are declared `---@async`: under `[lang] async`, `await-missing` reports a
-- call of one written without `await`. `try_send` / `try_recv` and the case builders do
-- not wait.
--
-- The module is preloaded by the binary that runs the program on the executor; Lua that
-- `htl gen` wrote from a file requiring it runs only on such a host.
local record task
   -- A case for `select`, built by a source's `on` / `on_send`; `R` is what its handler
   -- returns. Nothing to read on it: its only use is the list handed to `select`.
   record Case<R>
   end

   -- A case for `select_raw`, built by a source's `arm` / `arm_recv` / `arm_send`.
   record Arm
   end

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
      -- The case of the task finishing: `f(ok, value)` with what `join` would return
      -- (`false, err` when it raised or was cancelled). A select that takes it has joined
      -- the task: a later `await` raises `htl.task: task already awaited`.
      on: function<R>(self, f: function(boolean, T): R): Case<R>
      -- The same case for `select_raw`: its values are `ok, value`.
      arm: function(self): Arm
   end

   -- The receiving end of a channel: what a host's `RecvChannel<T>` is, and what a local
   -- `Channel<T>` is too.
   interface RecvChannel<T>
      -- Take the value at the front, waiting while the channel is empty: `v, true`, or
      -- `nil, false` once the channel is closed and empty.
      recv: function(self): T, boolean ---@async
      -- `recv` without waiting: `v, true, true` (a value), `nil, false, true` (closed and
      -- empty), `nil, false, false` (empty, still open).
      try_recv: function(self): T, boolean, boolean
      -- Close the channel. Idempotent; the values already in it can still be received.
      close: function(self)
      closed: function(self): boolean
      -- How many values it holds, and how many it can.
      len: function(self): integer
      cap: function(self): integer
      -- The receive case: `f(v, ok)` with what `recv` would return.
      on: function<R>(self, f: function(T, boolean): R): Case<R>
      -- The receive case for `select_raw`: its values are `v, ok`.
      arm_recv: function(self): Arm
   end

   -- The sending end of a channel: what a host's `SendChannel<T>` is, and what a local
   -- `Channel<T>` is too.
   interface SendChannel<T>
      -- Push `v`, waiting while the channel is full (a rendezvous channel, `cap` 0: until
      -- a receiver has taken it). Raises once the channel is closed.
      send: function(self, v: T) ---@async
      -- `send` without waiting: `false` when full. Raises once the channel is closed.
      try_send: function(self, v: T): boolean
      close: function(self)
      closed: function(self): boolean
      len: function(self): integer
      cap: function(self): integer
      -- The send case: `f(sent)`, `false` when the channel was closed instead.
      on_send: function<R>(self, v: T, f: function(boolean): R): Case<R>
      -- The send case for `select_raw`: its value is `sent`.
      arm_send: function(self, v: T): Arm
   end

   -- A local channel (`task.channel`), both ends at once. It belongs to no scope:
   -- cancelling a task does not close it.
   record Channel<T> is RecvChannel<T>, SendChannel<T>
   end

   -- A one-shot timer (`task.after`): ready from its deadline on, and ready for good.
   record Timer
      -- Wait until the timer is ready.
      wait: function(self) ---@async
      -- The timer's case: `f()` once it is ready.
      on: function<R>(self, f: function(): R): Case<R>
      arm: function(self): Arm
   end

   -- A periodic tick (`task.ticker`): a receive-only channel of capacity 1 whose values
   -- are each tick's time in milliseconds since the ticker started, the newest unread one
   -- kept. It stops, and its channel closes, with the scope that created it or at `stop`.
   record Ticker
      recv: function(self): number, boolean ---@async
      try_recv: function(self): number, boolean, boolean
      close: function(self)
      closed: function(self): boolean
      len: function(self): integer
      cap: function(self): integer
      on: function<R>(self, f: function(number, boolean): R): Case<R>
      arm_recv: function(self): Arm
      -- Stop the ticker and close its channel. Idempotent.
      stop: function(self)
   end

   -- `select`'s options. `biased`: check the cases in order instead of round robin.
   -- `default`: when no case is ready, call it and return what it returns instead of
   -- waiting.
   record SelectOpts<R>
      biased: boolean
      default: function(): R
   end

   -- `select_raw`'s options. `default = true`: when no case is ready, return 0 at once.
   record RawOpts
      biased: boolean
      default: boolean
   end

   -- A request a host sent (`htl::task::Request<Req, Resp>`) over a `RecvChannel`, for
   -- Teal to answer once. `reply` returns `false` when the host stopped waiting, and
   -- raises on a second answer; closing an unanswered one (`local req <close> = ..`)
   -- tells the host there will be no answer.
   record Request<Req, Resp>
      value: Req
      reply: function(self, v: Resp): boolean
      replied: function(self): boolean
      metamethod __close: function(self)
   end

   -- Start `f(...)` as a task of the current coroutine. Only on the executor: under a
   -- plain `Htl::exec` it raises `task.spawn: not inside a coroutine request or task`.
   spawn: function<T>(f: function(any...): (T), any...): Task<T>

   -- `t:await()` as a function (see the header for when the checker prefers the method).
   await: function<T>(t: Task<T>): T

   -- A task that already holds `v`: `await` returns it at once. It is what the checker
   -- reads `async local x = e` as (`[lang] async`), so that `x: Task<T>` is typed from `e`;
   -- the Lua that runs spawns `e` as a task instead. It has no select case.
   of: function<T>(v: T): Task<T>

   -- Whether `err` is a cancellation: what a cancel raises inside a task (caught with
   -- `pcall`, or handed to a `__close` handler), or `CANCELLED`. `false` for anything else.
   is_cancelled: function(err: any): boolean

   -- What `join` returns as `err`, and `await` raises, for a cancelled task.
   CANCELLED: any

   -- A local channel holding up to `cap` values; `cap` 0 is a rendezvous channel, where a
   -- send waits for a receiver. The element type comes from where it goes:
   -- `local ch: task.Channel<string> = task.channel(8)`.
   channel: function<T>(cap: integer): Channel<T>

   -- A timer ready `ms` milliseconds from now (`ms <= 0`: at once).
   after: function(ms: number): Timer

   -- A ticker every `ms` milliseconds (`ms > 0`). Only on the executor, like `spawn`.
   ticker: function(ms: number): Ticker

   -- Wait until one case is ready, take it, call its handler and return what it returns.
   select: function<R>(cases: {Case<R>}, opts?: SelectOpts<R>): R ---@async

   -- `select` without handlers: the index of the chosen case (1-based, 0 for `default`)
   -- and its values, `any` because they depend on the kind of case.
   select_raw: function(arms: {Arm}, opts?: RawOpts): integer, any, any ---@async
end

return task
