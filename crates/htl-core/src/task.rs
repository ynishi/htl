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
//!
//! # Channels between the host and Teal
//!
//! A program that runs its main loop in Teal — a daemon taking hook requests, webhook
//! events and timer ticks in one `select` — gets its events from the host over a channel.
//! [`RecvChannel<T>`] is the end Teal receives from, made beside the [`Sender<T>`] the
//! host keeps; [`SendChannel<T>`] is the end Teal sends into, made beside the
//! [`Receiver<T>`] the host drains. Both are mlua-isle's host channels
//! ([`mlua_isle::runtime::channel`] / [`channel_to_host`](mlua_isle::runtime::channel_to_host))
//! with the element type kept, which is what the declaration needs: a `#[host_module]`
//! method returning `RecvChannel<Event>` is declared `function(..): task.RecvChannel<Event>`,
//! and the generated `.d.tl` imports `htl.task` for it (`htl::dts`), so a handler that
//! takes the wrong type, or a `send` on that channel, is a check error at `cargo build`.
//! A channel of [`Request<Req, Resp>`] carries values Teal answers ([`Sender::request`]).
//!
//! ```rust,ignore
//! use htl::task::{RecvChannel, Request, SendChannel};
//!
//! pub struct Daemon {
//!     events: RecvChannel<Event>,
//!     reports: SendChannel<Report>,
//! }
//!
//! #[host_module(name = "daemon", dts = "scripts/daemon.d.tl", records = [Event, Report])]
//! impl Daemon {
//!     pub fn events(&self) -> RecvChannel<Event> { self.events.clone() }
//!     pub fn reports(&self) -> SendChannel<Report> { self.reports.clone() }
//! }
//!
//! let (tx, events) = RecvChannel::<Event>::new(h.lua(), 64)?;     // tx: any thread or task
//! let (reports, mut rx) = SendChannel::<Report>::new(h.lua(), 64)?;
//! Daemon { events, reports }.htl_preload(&h)?;
//! ```
//!
//! The channels are made on the program's state, before it runs, and handed to the module
//! the methods read them from: a method has no `Lua` to make one with. Making one creates
//! the state's `task` library if the program has not required `htl.task` yet, so the
//! channel and a later `require("htl.task")` share it. The contracts — capacity, closing,
//! conversion errors, cancellation, several receivers — are mlua-isle's, in its `runtime`
//! module docs ("Host channels and requests", "Channels to the host").
//!
//! # A host asks Teal
//!
//! A host that keeps its entry points in Rust and moves a decision into the Teal loop asks
//! the loop a question and waits for the answer. [`Ask<Q, A>`] is one kind of question: the
//! channel of [`Request<Q, A>`] Teal receives from, with the kind's deadline. [`Ask::new`]
//! makes it on the program's state before it runs and hands back two ends, the way
//! [`RecvChannel::new`] does: the [`Asker`] (`Send + Clone`, the [`Sender`] and the
//! deadline), which a handler on another runtime or thread — an HTTP route, a WebSocket
//! session, a tool call — holds and awaits [`Asker::ask`] on; and the [`Ask`], Teal's end,
//! which the host struct holds and whose `#[host_module]` getter returns [`Ask::channel`],
//! declared `task.RecvChannel<task.Request<Q, A>>` as any channel of requests is.
//!
//! The channel closes for Teal as an event channel does: when the last `Asker` is dropped,
//! the loop's `recv` gets `nil, false` once the queued requests are taken. Dropping the
//! askers is how the host says it will ask no more; the `Ask` in the state does not keep
//! the channel open.
//!
//! ```rust,ignore
//! use htl::task::{Ask, AskError};
//! use std::time::Duration;
//!
//! pub struct Hook {
//!     decide: Ask<Call, Decision>,
//! }
//!
//! #[host_module(name = "hook", dts = "scripts/hook.d.tl", records = [Call, Decision])]
//! impl Hook {
//!     pub fn decide(&self) -> RecvChannel<Request<Call, Decision>> { self.decide.channel() }
//! }
//!
//! let (asker, decide) = Ask::<Call, Decision>::new(h.lua(), 16, Duration::from_millis(500))?;
//! Hook { decide }.htl_preload(&h)?;               // asker: to the handler's runtime
//!
//! // in the handler, the one place a failure becomes the safe side:
//! let d = asker.ask(call).await.unwrap_or(Decision::Deny);
//! ```
//!
//! ```lua
//! local reqs = hook:decide()
//! await task.select({
//!    reqs:on(function(req: task.Request<hook.Call, hook.Decision>, ok: boolean): boolean
//!       if ok then req:reply(decide(req.value)) end
//!       return ok
//!    end),
//!    -- the loop's other cases
//! })
//! ```
//!
//! **The deadline is per kind, and htl has no default.** [`Ask::new`] takes it and
//! [`Asker::ask_within`] overrides it for one call: a library default would be a number
//! nobody can justify, and a kind without a deadline is a caller that can wait forever.
//!
//! An ask ends one of five ways. `Ok(a)`: the loop replied within the deadline.
//! [`AskError::Timeout`]: it did not; the request is dropped, and a `req:reply` the loop
//! makes afterwards returns `false` — a late answer is discarded, not delivered to the next
//! ask. [`AskError::Closed`]: the loop is gone — Teal called `close` on the channel, or the
//! program's state was dropped (a loop that returned while the state lives, with the `Ask`
//! in it, does not close the channel, and an ask then ends in `Timeout`). [`AskError::NoReply`]: the loop took
//! the request and closed it (`local req <close> = ..`) or let it be collected without
//! answering. [`AskError::Rejected`]: the loop refused, `req:reject(reason)`, and the host
//! gets the reason as Teal gave it; like a reply, a reject after the deadline returns
//! `false` to Teal.
//!
//! The reason is a `string`: the [`Request<Q, A>`] an [`Ask`] carries is mlua-isle's
//! `Request<Q, A, E>` with its default `E = String`, and htl offers no typed reason — a
//! refusal is told apart for the host's log, and text is what a log takes.
//!
//! The host maps `Err(_)` to its safe default in one place, not per failure: the four
//! failures are told apart for its log, not for its decision.
//!
//! Only a host whose Teal runs on the executor can ask: the loop has to wait on the
//! request channel. The window and cdylib hosts cannot — nothing in their run is a root
//! the executor runs (the window's `main.tl` is read through a plain `eval` and its
//! callbacks are called by macroquad's own loop; a cdylib's caller calls in synchronously
//! through `#[c_export]`). A target whose loop can run a root would carry it.
use crate::{Htl, write_if_changed};
use anyhow::{Context, Result};
use mlua::{FromLua, IntoLua, Lua, Value};
use std::marker::PhantomData;
use std::path::PathBuf;
use std::time::Duration;

pub use mlua_isle::runtime::{Receiver, Request, RequestError, Sender};

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
/// mlua-isle's functions forwarded as they are: their objects carry no method of htl's,
/// and a host channel built on the Rust side (`RecvChannel` / `SendChannel`) is the same
/// object a `task.channel` is.
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

/// The end of a host channel that Teal receives from: `task.RecvChannel<T>` in a
/// declaration. Made with [`RecvChannel::new`], beside the [`Sender`] the host feeds it
/// with; see the [module docs](self) for how a `#[host_module]` hands it out.
///
/// A handle to one Lua object: cloning it hands out the same channel, and it is the
/// program state's, so it stays on the thread that runs the program (the [`Sender`] is
/// what moves). Each value is converted with `T: IntoLua` when Teal takes it.
pub struct RecvChannel<T> {
    ch: mlua::Table,
    _t: PhantomData<fn() -> T>,
}

/// The end of a channel that Teal sends into and the host receives from:
/// `task.SendChannel<T>` in a declaration. Made with [`SendChannel::new`], beside the
/// [`Receiver`] the host drains.
///
/// A handle to one Lua object, as [`RecvChannel`] is. Each value Teal sends is converted
/// with `T: FromLua` as it is queued; one that does not convert raises in the Teal sender
/// and is not sent.
pub struct SendChannel<T> {
    ch: mlua::Table,
    _t: PhantomData<fn(T)>,
}

/// mlua-isle's host channels need the state's `task` library to exist (they are its
/// `Channel` objects); `require("htl.task")` would create it later, and creating it now
/// is the same table.
fn with_task_lib(lua: &Lua) -> Result<()> {
    let vm = crate::vm(lua)?;
    vm.task_lib().map_err(crate::isle_error)?;
    Ok(())
}

impl<T: IntoLua + Send + 'static> RecvChannel<T> {
    /// A host channel holding up to `cap` values (`cap >= 1`): the [`Sender`] for the
    /// host, `Send + Clone`, and the end Teal receives from. Called on the program's
    /// state ([`Htl::lua`]), on its thread. The channel closes for Teal, after the
    /// queued values, when every `Sender` is dropped.
    ///
    /// # Errors
    ///
    /// `cap` 0 (host channels have no rendezvous form) or larger than tokio's bounded
    /// channel allows, and a failure to create the state's `task` library.
    pub fn new(lua: &Lua, cap: usize) -> Result<(Sender<T>, RecvChannel<T>)> {
        with_task_lib(lua)?;
        let (tx, ch) = mlua_isle::runtime::channel::<T>(lua, cap).map_err(crate::isle_error)?;
        Ok((tx, RecvChannel::wrap(ch.into_table())))
    }
}

impl<T: FromLua + Send + 'static> SendChannel<T> {
    /// A channel to the host holding up to `cap` values (`cap >= 1`): the end Teal sends
    /// into, and the [`Receiver`] for the host, `Send`. Called on the program's state
    /// ([`Htl::lua`]), on its thread. Dropping the `Receiver` closes the channel: Teal's
    /// sends raise from then on.
    ///
    /// # Errors
    ///
    /// As [`RecvChannel::new`].
    pub fn new(lua: &Lua, cap: usize) -> Result<(SendChannel<T>, Receiver<T>)> {
        with_task_lib(lua)?;
        let (ch, rx) =
            mlua_isle::runtime::channel_to_host::<T>(lua, cap).map_err(crate::isle_error)?;
        Ok((SendChannel::wrap(ch.into_table()), rx))
    }
}

macro_rules! channel_end {
    ($name:ident) => {
        impl<T> $name<T> {
            fn wrap(ch: mlua::Table) -> Self {
                $name {
                    ch,
                    _t: PhantomData,
                }
            }

            /// The Lua object: the `task` library's `Channel`.
            pub fn table(&self) -> &mlua::Table {
                &self.ch
            }
        }

        impl<T> Clone for $name<T> {
            fn clone(&self) -> Self {
                $name::wrap(self.ch.clone())
            }
        }

        impl<T> std::fmt::Debug for $name<T> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_tuple(stringify!($name)).field(&self.ch).finish()
            }
        }

        impl<T> IntoLua for $name<T> {
            fn into_lua(self, _: &Lua) -> mlua::Result<Value> {
                Ok(Value::Table(self.ch))
            }
        }

        /// A channel Teal hands back to the host, as a method parameter. What is checked
        /// is that it is a channel of the `task` library; its direction and element type
        /// are the declaration's to hold, as for any other parameter.
        impl<T> FromLua for $name<T> {
            fn from_lua(value: Value, _: &Lua) -> mlua::Result<Self> {
                let refused = |from: &'static str| mlua::Error::FromLuaConversionError {
                    from,
                    to: stringify!($name).to_string(),
                    message: Some("expected a channel of htl.task".to_string()),
                };
                let Value::Table(t) = value else {
                    return Err(refused(value.type_name()));
                };
                // A `task` library `Channel` holds its host userdata as `_c`.
                if !matches!(t.raw_get::<Value>("_c")?, Value::UserData(_)) {
                    return Err(refused("table"));
                }
                Ok($name::wrap(t))
            }
        }
    };
}

channel_end!(RecvChannel);
channel_end!(SendChannel);

/// Teal's end of one kind of question: the channel of [`Request<Q, A>`] Teal receives from,
/// and the kind's deadline. Made by [`Ask::new`] beside the [`Asker`] the host asks with;
/// see the [module docs](self#a-host-asks-teal) for the pattern.
///
/// It is the program state's, as the [`RecvChannel`] in it is, and stays on the thread that
/// runs the program: the host struct holds it and its `#[host_module]` getter returns
/// [`channel`](Self::channel). It holds no sender, so it does not keep the channel open:
/// the channel closes for Teal when the last [`Asker`] is dropped.
pub struct Ask<Q, A> {
    teal: RecvChannel<Request<Q, A>>,
    deadline: Duration,
}

impl<Q, A> Ask<Q, A>
where
    Q: IntoLua + Send + 'static,
    A: FromLua + Send + 'static,
{
    /// A kind of question with up to `cap` requests queued for Teal (`cap >= 1`) and
    /// `deadline` as the limit of each [`Asker::ask`]: the [`Asker`] for the host,
    /// `Send + Clone`, and Teal's end — the order [`RecvChannel::new`] returns its two in.
    /// Called on the program's state ([`Htl::lua`]), on its thread, before the program
    /// runs. The channel closes for Teal, after the queued requests, when every `Asker` is
    /// dropped.
    ///
    /// `cap` is backpressure only: an ask waits for room in the queue and then for its
    /// answer, both inside the deadline. There is no default deadline: it is the kind's,
    /// chosen by the host (see the [module docs](self#a-host-asks-teal)).
    ///
    /// # Errors
    ///
    /// As [`RecvChannel::new`].
    pub fn new(lua: &Lua, cap: usize, deadline: Duration) -> Result<(Asker<Q, A>, Self)> {
        let (tx, teal) = RecvChannel::<Request<Q, A>>::new(lua, cap)?;
        Ok((Asker { tx, deadline }, Ask { teal, deadline }))
    }
}

impl<Q, A> Ask<Q, A> {
    /// The end Teal receives the requests from: what the `#[host_module]` getter returns,
    /// declared `task.RecvChannel<task.Request<Q, A>>`. Every call hands out the same
    /// channel.
    pub fn channel(&self) -> RecvChannel<Request<Q, A>> {
        self.teal.clone()
    }

    /// The kind's deadline, the one its [`Asker::ask`] waits for.
    pub fn deadline(&self) -> Duration {
        self.deadline
    }
}

impl<Q, A> std::fmt::Debug for Ask<Q, A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ask")
            .field("channel", &self.teal)
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}

/// The asking end of a kind of question: what a handler on another runtime or thread holds.
/// Made by [`Ask::new`]; `Send + Clone` when `Q` and `A` are `Send`, and a clone asks the
/// same kind with the same deadline.
///
/// The askers are the channel's senders: when the last one is dropped, the channel closes
/// for Teal, and its `recv` gets `nil, false` once the queued requests are taken — the way
/// the host says it will ask no more, as dropping the last [`Sender`] of an event channel
/// is.
pub struct Asker<Q, A> {
    tx: Sender<Request<Q, A>>,
    deadline: Duration,
}

impl<Q, A> Clone for Asker<Q, A> {
    fn clone(&self) -> Self {
        Asker {
            tx: self.tx.clone(),
            deadline: self.deadline,
        }
    }
}

impl<Q, A> std::fmt::Debug for Asker<Q, A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Asker")
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}

impl<Q, A> Asker<Q, A> {
    /// The deadline [`ask`](Self::ask) waits for.
    pub fn deadline(&self) -> Duration {
        self.deadline
    }

    /// Ask `q` and wait, within the kind's deadline, for Teal's answer:
    /// [`ask_within`](Self::ask_within) with the deadline [`Ask::new`] was given.
    ///
    /// # Errors
    ///
    /// As [`ask_within`](Self::ask_within).
    pub async fn ask(&self, q: Q) -> Result<A, AskError> {
        self.ask_within(q, self.deadline).await
    }

    /// Ask `q` and wait at most `deadline` for Teal's answer: the wait for room in the
    /// queue and the wait for the reply together. Must be awaited inside a tokio runtime
    /// with its time driver enabled.
    ///
    /// Dropping the future is the same as a timeout: the request is abandoned, and a
    /// `req:reply` or `req:reject` Teal makes afterwards returns `false`.
    ///
    /// # Errors
    ///
    /// [`AskError::Timeout`] with `deadline` when no answer came in time;
    /// [`AskError::Closed`] when the channel was closed before the request was queued (Teal
    /// closed it, or the program's state was dropped); [`AskError::Rejected`] with the
    /// reason when Teal called `req:reject(reason)`; [`AskError::NoReply`] when Teal
    /// closed or collected the request unanswered, or `q` failed to convert to a Lua value.
    pub async fn ask_within(&self, q: Q, deadline: Duration) -> Result<A, AskError> {
        match tokio::time::timeout(deadline, self.tx.request(q)).await {
            Ok(Ok(a)) => Ok(a),
            Ok(Err(RequestError::Closed(_))) => Err(AskError::Closed),
            Ok(Err(RequestError::NoReply)) => Err(AskError::NoReply),
            Ok(Err(RequestError::Rejected(reason))) => Err(AskError::Rejected(reason)),
            // `RequestError` is `#[non_exhaustive]`: a later mlua-isle may add a way for a
            // request to end unanswered. Until htl names it, it is the request ending
            // without an answer, and `AskError` (non-exhaustive too) can take a variant for
            // it when it comes.
            Ok(Err(_)) => Err(AskError::NoReply),
            Err(_elapsed) => Err(AskError::Timeout(deadline)),
        }
    }
}

/// Why an [`Asker::ask`] has no answer. The transport's failures, told apart for the host's
/// log; the host maps any of them to its safe default in one place.
///
/// Not generic over the question: unlike [`RequestError::Closed`], [`Closed`](Self::Closed)
/// does not hand the value back, since the host still holds what it asked (it is the
/// argument of the call that failed) and an error that carries `Q` cannot be one type
/// across kinds.
///
/// `#[non_exhaustive]` because the transport may learn another way to fail (mlua-isle's
/// `RequestError` is non-exhaustive too). A match on it keeps a `_` arm.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskError {
    /// No answer within the deadline, which it carries. The request is abandoned: Teal's
    /// later `req:reply` or `req:reject` returns `false`.
    Timeout(Duration),
    /// The loop is gone: the channel was closed before the request was queued — Teal
    /// called `close` on it, or the program's state was dropped.
    Closed,
    /// Teal took the request and closed it (`local req <close> = ..`) or let it be
    /// collected without answering; or the question failed to convert to a Lua value, and
    /// Teal never saw it.
    NoReply,
    /// The loop refused: Teal called `req:reject(reason)`. Carries the reason as Teal gave
    /// it.
    Rejected(String),
}

impl std::fmt::Display for AskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AskError::Timeout(d) => write!(f, "Teal did not answer within {d:?}"),
            AskError::Closed => f.write_str("the Teal loop's request channel is closed"),
            AskError::NoReply => f.write_str("Teal closed the request without answering"),
            AskError::Rejected(reason) => write!(f, "Teal rejected the request: {reason}"),
        }
    }
}

impl std::error::Error for AskError {}
