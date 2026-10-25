//! A Teal module's functions, called from async Rust: one VM thread, many program states on
//! it, and a deadline on every call (`htl::run`, feature `async`).
//!
//! A project that keeps its logic in Teal and its infrastructure in Rust has the Rust side
//! call into Teal: `require` a module, call a function of the record it returns, read the
//! result back as a Rust value. A Lua state is not `Send`, and a function a script defines
//! may loop for as long as it likes, so neither the state nor the call can sit on a task of
//! the host's own runtime. This module is that boundary, built once:
//!
//! - **One thread.** [`start`] spawns a thread with a current-thread tokio runtime and a
//!   `LocalSet` (what the executor [`Htl::call_async`] runs on needs), and one checker
//!   ([`Htl::new`]) on it. The rest of the program holds a [`Runner`]: cheap to clone,
//!   `Send + Sync`, every method `async` and answered over a oneshot. Requests wait on a
//!   bounded channel ([`RunnerConfig::queue`]), so a caller that outruns the VM waits
//!   instead of piling up work.
//! - **One program state per instance.** [`Runner::instance`] makes a fresh state with
//!   [`Htl::with_checker_lua`] on a safe `Lua::new()` (no `debug`, no C modules) and runs
//!   `require(module)` in it through the strict searcher ([`Htl::install_searcher`]), so a
//!   module with a type error does not load. Every state shares the one checker: a module
//!   is checked once per runner, not once per instance. Module-level state (a `local` the
//!   module's functions close over) is the instance's own, so two instances of one module
//!   share nothing but the checker's store of checked modules. What the checker resolves
//!   and settles for one instance is that instance's own, too: the directories
//!   `extra_paths` adds are on its program alone, so a module name they resolve is not
//!   resolved that way for another instance (`Htl::with_checker_lua` says what a program
//!   holds).
//! - **No Lua value leaves the thread.** The module's value stays on the VM thread and an
//!   [`Instance`] only names it. [`Instance::call`] sends the arguments as Rust values; the
//!   conversion into Lua, the call and the conversion of the result back happen on the VM
//!   thread, and only the Rust result crosses back.
//! - **A deadline on every call**, and on the `require` of [`Runner::instance`]
//!   ([`RunnerConfig::deadline`]). One watchdog thread per runner keeps the deadlines of
//!   what the VM is running and cancels the [`CancelToken`] of the one that is past
//!   it; the Lua debug hook the executor installs sees the token from another thread, so it
//!   stops a function in a CPU loop and a module body inside `require` alike, and neither
//!   the state nor the executor has to yield for it (no `preempt`). A cut-off call has
//!   nothing to clean up, so it gets no grace (`grace_ms = 0`,
//!   [`config::AsyncConfig`](crate::config::AsyncConfig)).
//! - **A caller that goes away cancels what it asked for.** Every request carries a token
//!   of its own, and the future [`Instance::call`] and [`Runner::instance`] return cancels
//!   it when it is dropped: a request still waiting is skipped when the VM reaches it, and
//!   one that is running is stopped by the same hook. Dropping the future of a `call` is
//!   how a host gives up on it (`tokio::time::timeout`, a `select!`, an aborted task).
//!
//! # When the deadline starts
//!
//! When the VM thread starts executing the request, not when the caller made it. The time a
//! request waits in the queue behind others is not counted ([`RunnerConfig::deadline`]).
//! For [`Runner::instance`] the deadline covers the `require` (the module's body) and not
//! the check that comes before it, which is the checker's own work on its own state.
//!
//! # What an instance reports, and as what
//!
//! [`Runner::instance`] tells the three ways a module fails to load apart, by looking
//! before it loads: it asks the checker, in the instance's program, where `module`
//! resolves, so that a missing module is [`CallError::NotFound`] (also a module that only a
//! `.d.tl` declares, with nothing behind it to run), a `.tl` with errors is
//! [`CallError::Check`] with its [`Diagnostic`]s, and only a `require` that then fails is
//! [`CallError::Runtime`]: the module's body raised. A module that is plain `.lua` is not
//! checked and goes straight to `require`. The check is stored, so the `require` after it
//! does not check again.
//!
//! A module that is loaded fine and `require`s another module *at run time* (a name built
//! from a variable, a `require` in a function called later) meets the strict searcher
//! there, which raises a string: that module's type error or absence arrives as
//! [`CallError::Runtime`], with the checker's text in the traceback. A typed answer for it
//! is left for later.
//!
//! # Closing, shutting down
//!
//! An instance is closed with [`Instance::close`], explicitly, because dropping it must not
//! do I/O: dropped without it, its state stays on the VM thread until the runner stops.
//! [`RunnerDriver::shutdown`] stops the VM thread (after the request it is on) and the
//! watchdog and joins both. A driver dropped without it leaves the threads to end on their
//! own: the VM thread once every [`Runner`] and [`Instance`] is gone, the watchdog with it.
//!
//! # Known limitations
//!
//! - The deadline rests on the Lua instruction-count hook, so it does not stop a native
//!   call (a long host function, a blocking read). A script that catches errors with
//!   `pcall` can catch the cancel too and go on, ignoring the deadline.
//! - The VM serves one request at a time. A request that runs long holds up those behind
//!   it, at worst for the queue's length times the deadline; the wait is not counted
//!   against their deadlines. A host that needs calls in parallel runs more than one
//!   [`Runner`].
//!
//! ```rust,ignore
//! use htl::run::{self, RunnerConfig};
//! use std::time::Duration;
//!
//! let (runner, driver) = run::start(RunnerConfig {
//!     search_paths: vec!["src".into()],
//!     deadline: Duration::from_secs(5),
//!     queue: 64,
//! })
//! .await?;
//! let app = runner.instance("app", &[]).await?;
//! let n: i64 = app.call("add", (2, 3)).await?;
//! app.close().await?;
//! driver.shutdown().await?;
//! ```

use crate::config::AsyncConfig;
use crate::{Diagnostic, Htl, Severity, developer_message, is_cancelled};
use mlua::{FromLuaMulti, Function, IntoLuaMulti, Lua, MultiValue, Value};
use mlua_isle::runtime::CancelToken;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::future::{Future, poll_fn};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::task::Poll;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// How a runner is set up.
#[derive(Debug, Clone)]
pub struct RunnerConfig {
    /// Directories `require` resolves from in every instance (mounted as with
    /// [`Htl::add_path`]). The working directory is not one of them unless it is listed:
    /// an instance starts from Lua's default path without its `./` entries
    /// ([`Htl::drop_cwd_search_path`]).
    pub search_paths: Vec<PathBuf>,
    /// The longest one call (and one `instance` load) may run, counted from the moment the
    /// VM thread starts on it: the time it waited in the queue is not part of it.
    pub deadline: Duration,
    /// Requests that may wait for the VM before a caller has to.
    pub queue: usize,
}

/// Why a request to the runner failed.
///
/// Not `Clone` and not `PartialEq`: it carries the errors that caused it. Match on it.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    /// The VM thread, its checker or an instance's program state could not be made.
    #[error("the Teal VM could not start: {source}")]
    Start {
        /// What failed.
        #[source]
        source: Arc<dyn std::error::Error + Send + Sync>,
    },
    /// The module resolves to a `.tl` that does not type-check, so it was not loaded (the
    /// searcher is strict).
    #[error("`{module}` does not type-check:\n{}", join_lines(.diagnostics))]
    Check {
        /// The module asked for.
        module: String,
        /// The type errors, as `htl check` would report them.
        diagnostics: Vec<Diagnostic>,
    },
    /// The module is on no search path of the instance, or only a `.d.tl` declares it and
    /// nothing behind that runs.
    #[error("no module `{module}` on the search path")]
    NotFound {
        /// The module asked for.
        module: String,
    },
    /// The call (or the load) ran past the deadline and was cut off.
    #[error("{what} ran past its {after:?} deadline and was cut off")]
    Timeout {
        /// What was running: `module.field`, or `require("module")`.
        what: String,
        /// The deadline.
        after: Duration,
    },
    /// The Teal code raised: in a call, or in the body of the module while `require` loaded
    /// it. Also what a run-time `require` of a module that fails to check comes back as, and
    /// what the checker raising while it resolves or checks the module for `instance` comes
    /// back as (the VM is running; the module could not be looked at).
    #[error("{what} failed: {traceback}")]
    Runtime {
        /// `module.field`, `require("module")`, or `check("module")`.
        what: String,
        /// The error with the `stack traceback:` block it carries
        /// ([`developer_message`]).
        traceback: String,
        /// The error itself.
        #[source]
        source: Arc<dyn std::error::Error + Send + Sync>,
    },
    /// The arguments did not convert into Lua, or the values returned are not what the
    /// caller's type expects. The checker rules most of this out; `as` and `any` let it
    /// through.
    #[error("{what}: a value of the wrong shape: {source}")]
    Shape {
        /// `module.field`.
        what: String,
        /// What the conversion said.
        #[source]
        source: mlua::Error,
    },
    /// The field is not a function (or is absent, or the module's value is not a table).
    #[error("the module has no function `{field}`")]
    NotAFunction {
        /// The field asked for.
        field: String,
    },
    /// The VM thread has stopped (shut down, or it panicked), or the instance was closed.
    #[error("the Teal VM has stopped, or the instance was closed")]
    Gone,
}

/// The diagnostics, one to a line.
fn join_lines(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// An error as the shared, type-erased source a [`CallError`] keeps.
fn source_of(e: anyhow::Error) -> Arc<dyn std::error::Error + Send + Sync> {
    let boxed: Box<dyn std::error::Error + Send + Sync> = e.into();
    Arc::from(boxed)
}

fn start_error(e: impl Into<anyhow::Error>) -> CallError {
    CallError::Start {
        source: source_of(e.into()),
    }
}

type Reply<T> = oneshot::Sender<Result<T, CallError>>;
/// The arguments of a call, made into Lua on the VM thread.
type Args = Box<dyn FnOnce(&Lua) -> mlua::Result<MultiValue> + Send>;
/// What a call returned (or why it failed), read into the caller's type on the VM thread
/// and sent back.
type Finish = Box<dyn FnOnce(&Lua, Result<MultiValue, CallError>) + Send>;

enum Request {
    Instance {
        module: String,
        extra: Vec<PathBuf>,
        token: CancelToken,
        reply: Reply<u64>,
    },
    Call {
        id: u64,
        field: &'static str,
        what: String,
        args: Args,
        finish: Finish,
        token: CancelToken,
    },
    Close {
        id: u64,
        reply: Reply<()>,
    },
}

/// Cancels a request's token when the future that asked for it is dropped. An atomic flag
/// is all it sets, so it does no I/O. On a request that has finished the cancel is a
/// no-op: a token belongs to one request.
struct CancelOnDrop(CancelToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

async fn ask<T>(
    tx: &mpsc::Sender<Request>,
    make: impl FnOnce(Reply<T>) -> Request,
) -> Result<T, CallError> {
    let (reply, rx) = oneshot::channel();
    tx.send(make(reply)).await.map_err(|_| CallError::Gone)?;
    rx.await.map_err(|_| CallError::Gone)?
}

/// The handle every task holds. `Send + Sync`; clone it freely.
#[derive(Debug, Clone)]
pub struct Runner {
    tx: mpsc::Sender<Request>,
}

impl Runner {
    /// A fresh program state (sharing the runner's checker, with a program of its own in
    /// it), `extra_paths` added to its search path, `module` resolved and checked in that
    /// program, and `require(module)` run in it under the deadline. The module's value stays
    /// on the VM thread; the `Instance` names it.
    ///
    /// What a module that cannot be loaded is reported as is in the module doc:
    /// [`CallError::NotFound`], [`CallError::Check`], or [`CallError::Runtime`] for a body
    /// that raised; [`CallError::Timeout`] for one that did not end.
    ///
    /// Dropping the returned future cancels the request, as it does a call's.
    pub async fn instance(
        &self,
        module: &str,
        extra_paths: &[PathBuf],
    ) -> Result<Instance, CallError> {
        let token = CancelToken::new();
        let _cancel = CancelOnDrop(token.clone());
        let id = ask(&self.tx, |reply| Request::Instance {
            module: module.to_string(),
            extra: extra_paths.to_vec(),
            token,
            reply,
        })
        .await?;
        Ok(Instance {
            id,
            module: module.into(),
            tx: self.tx.clone(),
        })
    }
}

/// A module loaded in a program state of its own ([`Runner::instance`]). `Send + Sync`; a
/// clone names the same state.
#[derive(Debug, Clone)]
pub struct Instance {
    id: u64,
    module: Arc<str>,
    tx: mpsc::Sender<Request>,
}

impl Instance {
    /// Call the function in field `field` of the module's value with `args`, under the
    /// deadline, and convert what it returns.
    ///
    /// The function is called with `.`, not `:`: `args` are all its arguments. A tuple is
    /// several arguments and several results (`(a, b)`, `()` for none); an `Option` is a
    /// value that may be nil.
    ///
    /// Dropping the returned future cancels the call: one still queued is never started,
    /// one running is stopped at its next hook check, and the VM goes on to the next
    /// request. A clone of a closed instance is answered [`CallError::Gone`].
    pub async fn call<A, R>(&self, field: &'static str, args: A) -> Result<R, CallError>
    where
        A: IntoLuaMulti + Send + 'static,
        R: FromLuaMulti + Send + 'static,
    {
        let token = CancelToken::new();
        let _cancel = CancelOnDrop(token.clone());
        let what = format!("{}.{field}", self.module);
        let shape = what.clone();
        let (reply, rx) = oneshot::channel::<Result<R, CallError>>();
        let request = Request::Call {
            id: self.id,
            field,
            what,
            args: Box::new(move |lua| args.into_lua_multi(lua)),
            finish: Box::new(move |lua, out| {
                let out = out.and_then(|values| {
                    R::from_lua_multi(values, lua).map_err(|source| CallError::Shape {
                        what: shape,
                        source,
                    })
                });
                let _ = reply.send(out);
            }),
            token,
        };
        self.tx.send(request).await.map_err(|_| CallError::Gone)?;
        rx.await.map_err(|_| CallError::Gone)?
    }

    /// Drop the instance's state on the VM thread. (Drop does no I/O.) A clone of a closed
    /// instance is answered [`CallError::Gone`]; closing it again is not an error.
    pub async fn close(self) -> Result<(), CallError> {
        ask(&self.tx, |reply| Request::Close { id: self.id, reply }).await
    }
}

/// Owns the VM thread and the watchdog thread. Dropped without
/// [`shutdown`](Self::shutdown), the VM thread ends once every [`Runner`] and [`Instance`]
/// is gone, and the watchdog with it.
#[derive(Debug)]
pub struct RunnerDriver {
    thread: std::thread::JoinHandle<()>,
    watchdog: std::thread::JoinHandle<()>,
    deadlines: Arc<Deadlines>,
    stop: Option<oneshot::Sender<()>>,
}

impl RunnerDriver {
    /// Stop the VM (after the request it is on) and the watchdog, and wait for both threads
    /// to end. Every instance's state is dropped with the VM, and a request still waiting
    /// is answered [`CallError::Gone`].
    pub async fn shutdown(mut self) -> Result<(), CallError> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let Self {
            thread,
            watchdog,
            deadlines,
            ..
        } = self;
        // The joins block, so they run on a thread of their own and the answer comes back
        // over a oneshot: nothing here needs the caller's executor to be tokio's.
        let (done, joined) = oneshot::channel();
        std::thread::Builder::new()
            .name("htl-run-join".into())
            .spawn(move || {
                // The VM first: the watchdog is what cuts off the request it may still be on.
                let vm = thread.join();
                deadlines.stop();
                let dog = watchdog.join();
                // Gone before the answer is: nothing of the runner outlives the shutdown.
                drop(deadlines);
                let _ = done.send(vm.is_ok() && dog.is_ok());
            })
            .map_err(start_error)?;
        joined
            .await
            .map_err(|_| CallError::Gone)?
            .then_some(())
            .ok_or(CallError::Gone)
    }

    /// A handle that outlives neither thread: it can no longer be upgraded once both have
    /// ended and the driver is gone.
    #[cfg(test)]
    fn probe(&self) -> std::sync::Weak<Deadlines> {
        Arc::downgrade(&self.deadlines)
    }
}

/// Start the VM thread and the watchdog, and wait until the checker is ready.
pub async fn start(cfg: RunnerConfig) -> Result<(Runner, RunnerDriver), CallError> {
    let (tx, rx) = mpsc::channel(cfg.queue.max(1));
    let (ready_tx, ready_rx) = oneshot::channel();
    let (stop_tx, stop_rx) = oneshot::channel();
    let deadlines = Arc::new(Deadlines::default());
    let watchdog = {
        let deadlines = deadlines.clone();
        std::thread::Builder::new()
            .name("htl-run-watchdog".into())
            .spawn(move || deadlines.watch_over())
            .map_err(start_error)?
    };
    let thread = {
        let deadlines = deadlines.clone();
        std::thread::Builder::new()
            .name("htl-run".into())
            .spawn(move || vm_thread(cfg, deadlines, rx, ready_tx, stop_rx))
    };
    let thread = match thread {
        Ok(thread) => thread,
        Err(e) => {
            deadlines.stop();
            return Err(start_error(e));
        }
    };
    // Whichever way the VM thread failed to come up, it has stopped the watchdog (it holds
    // a `StopOnDrop`); a thread that is gone without having answered is `Gone`.
    ready_rx.await.map_err(|_| CallError::Gone)??;
    Ok((
        Runner { tx },
        RunnerDriver {
            thread,
            watchdog,
            deadlines,
            stop: Some(stop_tx),
        },
    ))
}

/// Stops the watchdog when the VM thread is done, however it ends.
struct StopOnDrop(Arc<Deadlines>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.stop();
    }
}

fn vm_thread(
    cfg: RunnerConfig,
    deadlines: Arc<Deadlines>,
    mut rx: mpsc::Receiver<Request>,
    ready: oneshot::Sender<Result<(), CallError>>,
    stop: oneshot::Receiver<()>,
) {
    let _stop_watchdog = StopOnDrop(deadlines.clone());
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let _ = ready.send(Err(start_error(e)));
            return;
        }
    };
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async move {
        let checker = match Htl::new() {
            Ok(h) => h,
            Err(e) => {
                let _ = ready.send(Err(start_error(e)));
                return;
            }
        };
        let _ = ready.send(Ok(()));
        let mut vm = Vm {
            slots: HashMap::new(),
            next: 0,
            search_paths: cfg.search_paths,
            deadline: cfg.deadline,
            deadlines,
            checker,
        };
        let mut stop = Some(stop);
        while let Some(request) = next(&mut rx, &mut stop).await {
            vm.serve(request).await;
        }
    });
}

/// The next request, or `None` once the driver says stop or every sender is gone. A driver
/// dropped without a shutdown is no stop.
async fn next(
    rx: &mut mpsc::Receiver<Request>,
    stop: &mut Option<oneshot::Receiver<()>>,
) -> Option<Request> {
    poll_fn(|cx| {
        match stop.as_mut().map(|s| Pin::new(s).poll(cx)) {
            Some(Poll::Ready(Ok(()))) => return Poll::Ready(None),
            Some(Poll::Ready(Err(_))) => *stop = None,
            _ => {}
        }
        rx.poll_recv(cx)
    })
    .await
}

/// The deadlines of what the VM is running, and the one thread that cancels a token when
/// its deadline passes.
///
/// A run registers its token when the VM starts it ([`watch`](Self::watch)) and drops the
/// returned guard when it is done; the watchdog keeps a min-heap of `(deadline, id)` and a
/// map from the id to the token, and sleeps on a condvar until the earliest deadline. An
/// entry the guard has taken out of the map is skipped when it comes to the top of the
/// heap, and the heap is rebuilt when most of it is such entries (a long deadline with
/// many short calls would otherwise grow it until the deadline passed).
#[derive(Default)]
struct Deadlines {
    state: Mutex<DeadlineState>,
    wake: Condvar,
}

#[derive(Default)]
struct DeadlineState {
    next: u64,
    heap: BinaryHeap<Reverse<(Instant, u64)>>,
    live: HashMap<u64, CancelToken>,
    stop: bool,
}

impl std::fmt::Debug for Deadlines {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Deadlines")
    }
}

/// A registered deadline; ends it when dropped.
struct Watched<'a> {
    deadlines: &'a Deadlines,
    id: u64,
}

impl Drop for Watched<'_> {
    fn drop(&mut self) {
        let mut state = self.deadlines.lock();
        state.live.remove(&self.id);
        if state.heap.len() > 2 * state.live.len() + 16 {
            let live = &state.live;
            let kept: Vec<_> = state
                .heap
                .iter()
                .filter(|Reverse((_, id))| live.contains_key(id))
                .copied()
                .collect();
            state.heap = kept.into();
        }
    }
}

impl Deadlines {
    fn lock(&self) -> MutexGuard<'_, DeadlineState> {
        // Nothing the lock guards is left half-written by a panic.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Cancel `token` once `after` has passed, unless the guard is dropped first.
    fn watch(&self, token: &CancelToken, after: Duration) -> Watched<'_> {
        let now = Instant::now();
        // A deadline too far off to add is no deadline: thirty years.
        let at = now
            .checked_add(after)
            .unwrap_or_else(|| now + Duration::from_secs(30 * 365 * 24 * 3600));
        let mut state = self.lock();
        let id = state.next;
        state.next += 1;
        state.live.insert(id, token.clone());
        state.heap.push(Reverse((at, id)));
        drop(state);
        self.wake.notify_one();
        Watched {
            deadlines: self,
            id,
        }
    }

    fn stop(&self) {
        self.lock().stop = true;
        self.wake.notify_one();
    }

    /// The watchdog thread's body: cancel what is due, sleep until what is next, until
    /// [`stop`](Self::stop).
    fn watch_over(&self) {
        let mut state = self.lock();
        while !state.stop {
            let now = Instant::now();
            let mut next = None;
            while let Some(&Reverse((at, id))) = state.heap.peek() {
                if !state.live.contains_key(&id) {
                    state.heap.pop();
                } else if at <= now {
                    state.heap.pop();
                    if let Some(token) = state.live.remove(&id) {
                        token.cancel();
                    }
                } else {
                    next = Some(at - now);
                    break;
                }
            }
            state = match next {
                Some(wait) => {
                    self.wake
                        .wait_timeout(state, wait)
                        .unwrap_or_else(|e| e.into_inner())
                        .0
                }
                None => self.wake.wait(state).unwrap_or_else(|e| e.into_inner()),
            };
        }
    }
}

/// An instance's state: the program state, and the value `require` returned in it.
struct Slot {
    prog: Htl,
    module: Value,
}

struct Vm {
    /// Declared before the checker, so the states that borrow its compiler drop first.
    slots: HashMap<u64, Slot>,
    next: u64,
    search_paths: Vec<PathBuf>,
    deadline: Duration,
    deadlines: Arc<Deadlines>,
    checker: Htl,
}

impl Vm {
    async fn serve(&mut self, request: Request) {
        match request {
            Request::Instance {
                module,
                extra,
                token,
                reply,
            } => {
                // The caller gave up while the request waited.
                if token.is_cancelled() {
                    return;
                }
                let out = self.instance(&module, &extra, &token).await;
                // The caller gave up while the module loaded: nothing will name this slot.
                if let Err(Ok(id)) = reply.send(out) {
                    self.slots.remove(&id);
                }
            }
            Request::Call {
                id,
                field,
                what,
                args,
                finish,
                token,
            } => {
                if token.is_cancelled() {
                    return;
                }
                let Some(slot) = self.slots.get(&id) else {
                    finish(self.checker.lua(), Err(CallError::Gone));
                    return;
                };
                let lua = slot.prog.lua();
                // Reading the field can raise (an `__index` that errors): that is the
                // module failing, not a field that is missing.
                let f = match &slot.module {
                    Value::Table(t) => match t.get::<Value>(field) {
                        Ok(v) => Some(v),
                        Err(e) => {
                            let e = anyhow::Error::from(e);
                            finish(
                                lua,
                                Err(CallError::Runtime {
                                    what,
                                    traceback: developer_message(&e),
                                    source: source_of(e),
                                }),
                            );
                            return;
                        }
                    },
                    _ => None,
                };
                let Some(Value::Function(f)) = f else {
                    let field = field.to_string();
                    finish(lua, Err(CallError::NotAFunction { field }));
                    return;
                };
                let args = match args(lua) {
                    Ok(args) => args,
                    Err(source) => {
                        finish(lua, Err(CallError::Shape { what, source }));
                        return;
                    }
                };
                let out = self.run(&slot.prog, &what, f, args, &token).await;
                finish(lua, out);
            }
            Request::Close { id, reply } => {
                self.slots.remove(&id);
                let _ = reply.send(Ok(()));
            }
        }
    }

    /// Build the instance's program state, say what the module resolves to, and load it.
    ///
    /// Not shared with the test runner (`testing`) or `Htl::run_file`: the test runner puts
    /// a project's model, config and lints in each program, `run_file` runs one file once,
    /// and this keeps the program alive to be called many times with none of those. What
    /// the three have in common is three calls (`with_checker_lua`, `add_path`,
    /// `install_searcher`), each of which already is one method of `Htl`.
    async fn instance(
        &mut self,
        module: &str,
        extra: &[PathBuf],
        token: &CancelToken,
    ) -> Result<u64, CallError> {
        let prog = Htl::with_checker_lua(&self.checker, Lua::new()).map_err(start_error)?;
        // No grace for a cut-off call, and no `preempt`: the hook sees a token cancelled
        // from the watchdog thread by itself.
        prog.configure_async(&AsyncConfig {
            grace_ms: Some(0),
            preempt: None,
        })
        .map_err(start_error)?;
        // What a module name means must not depend on where the host was started.
        prog.drop_cwd_search_path().map_err(start_error)?;
        for dir in self.search_paths.iter().chain(extra) {
            prog.add_path(dir).map_err(start_error)?;
        }
        prog.install_searcher().map_err(start_error)?;
        check_module(&prog, module)?;

        let require: Function = prog
            .lua()
            .globals()
            .get("require")
            .map_err(|e| start_error(anyhow::Error::from(e)))?;
        let what = format!("require(\"{module}\")");
        let values = self
            .run(&prog, &what, require, module.to_string(), token)
            .await?;
        let id = self.next;
        self.next += 1;
        let module = values.into_iter().next().unwrap_or(Value::Nil);
        self.slots.insert(id, Slot { prog, module });
        Ok(id)
    }

    /// `f(args)` on the executor under `token`, which the watchdog cancels at the deadline
    /// (counted from here, when the VM starts on it) and a dropped caller cancels sooner.
    async fn run(
        &self,
        prog: &Htl,
        what: &str,
        f: Function,
        args: impl IntoLuaMulti,
        token: &CancelToken,
    ) -> Result<MultiValue, CallError> {
        let _watched = self.deadlines.watch(token, self.deadline);
        prog.call_async(f, args, token).await.map_err(|e| {
            if is_cancelled(&e) {
                CallError::Timeout {
                    what: what.to_string(),
                    after: self.deadline,
                }
            } else {
                CallError::Runtime {
                    what: what.to_string(),
                    traceback: developer_message(&e),
                    source: source_of(e),
                }
            }
        })
    }
}

/// Settle, before `require` runs, what a module is: absent ([`CallError::NotFound`]), a
/// `.tl` that does not check ([`CallError::Check`]), or something `require` can load.
///
/// Asked of the checker in the instance's own program (`prog`), so it sees the paths of
/// that instance and no other. `resolve_module` answers with the file `require` would find
/// and with where a plain `.lua` implementation sits:
///
/// - nothing for either: `NotFound`;
/// - only a `.lua` (or a `.lua` as the file found): not Teal, nothing to check, `require`;
/// - a `.d.tl` with a `.lua` behind it: `require` serves the `.lua`;
/// - a `.d.tl` with no `.lua`: a declaration only, nothing to run, `NotFound`. (A `.tl`
///   beside it would have been the file found: `tl.search_module` looks for sources across
///   the whole path before declarations.)
/// - a `.tl`: checked and generated the way the searcher does (no lints), whose result the
///   checker stores, so the `require` after it serves the generated code without checking
///   again. A `.tl` that checks but statically requires one that does not is a `Check`
///   too: the searcher would refuse that dependency in the body's first `require`.
fn check_module(prog: &Htl, module: &str) -> Result<(), CallError> {
    let not_found = || CallError::NotFound {
        module: module.to_string(),
    };
    // The VM is up: the checker raising here is about this module, not a failed start.
    let checker_error = |e: anyhow::Error| CallError::Runtime {
        what: format!("check(\"{module}\")"),
        traceback: developer_message(&e),
        source: source_of(e),
    };
    let (found, lua) = prog.resolve_module(module).map_err(checker_error)?;
    let Some(file) = found else {
        return if lua.is_some() {
            Ok(())
        } else {
            Err(not_found())
        };
    };
    let name = file.to_string_lossy();
    if name.ends_with(".d.tl") {
        return if lua.is_some() {
            Ok(())
        } else {
            Err(not_found())
        };
    }
    if name.ends_with(".lua") {
        return Ok(());
    }
    let (code, info) = prog.gen_lua_unlinted(&file).map_err(checker_error)?;
    if code.is_some() && info.dependency_errors.is_empty() {
        return Ok(());
    }
    let mut diagnostics = info.error_diagnostics();
    if diagnostics.is_empty() {
        // The file's own list is empty: the errors are in a module it requires.
        diagnostics = info
            .dependency_errors
            .iter()
            .map(|e| e.diagnostic.clone())
            .collect();
    }
    if diagnostics.is_empty() {
        diagnostics = info
            .errors
            .iter()
            .map(|e| Diagnostic::parse(Severity::Error, e))
            .collect();
    }
    Err(CallError::Check {
        module: module.to_string(),
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core_common::{TempDir, tempdir};
    use std::path::Path;

    const M: &str = "local record m
   add: function(integer, integer): integer
   split: function(string): string, integer
   bump: function(): integer
   spin: function()
   busy: function(integer)
   fail: function()
   count: integer
end
local n = 0
function m.add(a: integer, b: integer): integer
   return a + b
end
function m.split(s: string): string, integer
   return s:upper(), #s
end
function m.bump(): integer
   n = n + 1
   return n
end
function m.spin()
   while true do end
end
function m.busy(seconds: integer)
   -- Wall time, to the second: os.clock is the CPU time of the whole process, which runs
   -- as fast as the tests beside this one are busy. Takes more than seconds - 1 and at
   -- most seconds.
   local t = os.time()
   while os.time() < t + seconds do end
end
function m.fail()
   error(\"boom\")
end
return m
";

    const BAD: &str = "local x: integer = \"not an integer\"
return { x = x }
";

    const ENDLESS: &str = "local record e
end
while true do end
return e
";

    const ONLY_HERE: &str = "local record only_here
   five: function(): integer
end
function only_here.five(): integer
   return 5
end
return only_here
";

    const PLAIN: &str = "return { add = function(a, b) return a + b end }\n";

    const DECLARED: &str = "local record declared
   f: function()
end
return declared
";

    fn dir(name: &str) -> TempDir {
        let root = tempdir("htl-core-run", name);
        std::fs::write(root.join("m.tl"), M).unwrap();
        std::fs::write(root.join("bad.tl"), BAD).unwrap();
        std::fs::write(root.join("endless.tl"), ENDLESS).unwrap();
        std::fs::write(root.join("plain.lua"), PLAIN).unwrap();
        std::fs::write(root.join("declared.d.tl"), DECLARED).unwrap();
        root
    }

    /// `f` with a runner over `root`, on a runtime of the test's own, and the runner shut
    /// down after it.
    fn with_runner<F, Fut>(root: &TempDir, deadline: Duration, f: F)
    where
        F: FnOnce(Runner) -> Fut,
        Fut: Future<Output = ()>,
    {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let (runner, driver) = start(RunnerConfig {
                search_paths: vec![root.to_path_buf()],
                deadline,
                queue: 4,
            })
            .await
            .unwrap();
            f(runner).await;
            driver.shutdown().await.unwrap();
        });
    }

    #[test]
    fn a_call_returns_a_converted_value() {
        let root = dir("call");
        with_runner(&root, Duration::from_secs(30), |runner| async move {
            let m = runner.instance("m", &[]).await.unwrap();
            assert_eq!(m.call::<_, i64>("add", (2, 3)).await.unwrap(), 5);
            assert_eq!(
                m.call::<_, (String, i64)>("split", ("abc".to_string(),))
                    .await
                    .unwrap(),
                ("ABC".to_string(), 3)
            );
            m.close().await.unwrap();
        });
    }

    #[test]
    fn an_endless_loop_in_a_call_is_cut_at_the_deadline() {
        let root = dir("spin");
        let after = Duration::from_millis(500);
        with_runner(&root, after, |runner| async move {
            let m = runner.instance("m", &[]).await.unwrap();
            let started = Instant::now();
            match m.call::<_, ()>("spin", ()).await {
                Err(CallError::Timeout { what, after: got }) => {
                    assert_eq!(what, "m.spin");
                    assert_eq!(got, after);
                }
                other => panic!("{other:?}"),
            }
            assert!(started.elapsed() >= after, "{:?}", started.elapsed());
            // The instance is still there after a cut-off call.
            assert_eq!(m.call::<_, i64>("add", (1, 1)).await.unwrap(), 2);
        });
    }

    #[test]
    fn an_endless_module_body_is_cut_at_the_deadline_of_instance() {
        let root = dir("endless");
        let after = Duration::from_millis(500);
        with_runner(&root, after, |runner| async move {
            match runner.instance("endless", &[]).await {
                Err(CallError::Timeout { what, after: got }) => {
                    assert_eq!(what, "require(\"endless\")");
                    assert_eq!(got, after);
                }
                other => panic!("{:?}", other.map(|_| ())),
            }
            // The runner serves on.
            let m = runner.instance("m", &[]).await.unwrap();
            assert_eq!(m.call::<_, i64>("add", (1, 1)).await.unwrap(), 2);
        });
    }

    #[test]
    fn two_instances_of_a_module_share_no_state() {
        let root = dir("isolated");
        with_runner(&root, Duration::from_secs(30), |runner| async move {
            let a = runner.instance("m", &[]).await.unwrap();
            let b = runner.instance("m", &[]).await.unwrap();
            assert_eq!(a.call::<_, i64>("bump", ()).await.unwrap(), 1);
            assert_eq!(a.call::<_, i64>("bump", ()).await.unwrap(), 2);
            assert_eq!(b.call::<_, i64>("bump", ()).await.unwrap(), 1);
            assert_eq!(b.call::<_, i64>("bump", ()).await.unwrap(), 2);
        });
    }

    #[test]
    fn an_extra_path_is_seen_by_its_instance_alone() {
        let root = dir("extra");
        let extra = tempdir("htl-core-run", "extra-dir");
        std::fs::write(extra.join("only_here.tl"), ONLY_HERE).unwrap();
        with_runner(&root, Duration::from_secs(30), |runner| async move {
            let with = runner
                .instance("only_here", &[extra.to_path_buf()])
                .await
                .unwrap();
            assert_eq!(with.call::<_, i64>("five", ()).await.unwrap(), 5);
            // Made after the first, and resolved by its own program, not by that one's.
            assert!(matches!(
                runner.instance("only_here", &[]).await.map(|_| ()),
                Err(CallError::NotFound { module }) if module == "only_here"
            ));
            // The first is untouched.
            assert_eq!(with.call::<_, i64>("five", ()).await.unwrap(), 5);
        });
    }

    #[test]
    fn a_teal_error_is_a_runtime_error_with_a_traceback() {
        let root = dir("raise");
        with_runner(&root, Duration::from_secs(30), |runner| async move {
            let m = runner.instance("m", &[]).await.unwrap();
            match m.call::<_, ()>("fail", ()).await {
                Err(CallError::Runtime {
                    what, traceback, ..
                }) => {
                    assert_eq!(what, "m.fail");
                    assert!(traceback.contains("boom"), "{traceback}");
                    assert!(traceback.contains("stack traceback:"), "{traceback}");
                }
                other => panic!("{other:?}"),
            }
        });
    }

    /// Reading the field raising is the module failing, not a missing function.
    #[test]
    fn a_field_whose_read_raises_is_a_runtime_error() {
        let root = dir("guarded");
        std::fs::write(
            root.join("guarded.lua"),
            "return setmetatable({}, { __index = function(_, k) error(\"no field \" .. k) end })\n",
        )
        .unwrap();
        with_runner(&root, Duration::from_secs(30), |runner| async move {
            let g = runner.instance("guarded", &[]).await.unwrap();
            match g.call::<_, ()>("anything", ()).await {
                Err(CallError::Runtime {
                    what, traceback, ..
                }) => {
                    assert_eq!(what, "guarded.anything");
                    assert!(traceback.contains("no field anything"), "{traceback}");
                }
                other => panic!("{other:?}"),
            }
        });
    }

    #[test]
    fn a_field_that_is_no_function_is_not_a_function() {
        let root = dir("missing");
        with_runner(&root, Duration::from_secs(30), |runner| async move {
            let m = runner.instance("m", &[]).await.unwrap();
            for field in ["nope", "count"] {
                assert!(matches!(
                    m.call::<_, ()>(field, ()).await,
                    Err(CallError::NotAFunction { field: got }) if got == field
                ));
            }
            // A result of the wrong type is a shape error.
            match m.call::<_, Vec<i64>>("bump", ()).await {
                Err(CallError::Shape { what, .. }) => assert_eq!(what, "m.bump"),
                other => panic!("{other:?}"),
            }
        });
    }

    #[test]
    fn a_module_with_a_type_error_is_a_check_error() {
        let root = dir("bad");
        with_runner(&root, Duration::from_secs(30), |runner| async move {
            match runner.instance("bad", &[]).await {
                Err(CallError::Check {
                    module,
                    diagnostics,
                }) => {
                    assert_eq!(module, "bad");
                    assert!(!diagnostics.is_empty());
                    assert!(
                        diagnostics.iter().all(|d| d.file.ends_with("bad.tl")),
                        "{diagnostics:?}"
                    );
                }
                other => panic!("{:?}", other.map(|_| ())),
            }
        });
    }

    /// A module that checks but requires one that does not is a check error too, with the
    /// dependency's diagnostics, and not a body that raised.
    #[test]
    fn a_module_requiring_one_with_a_type_error_is_a_check_error() {
        let root = dir("uses-bad");
        std::fs::write(
            root.join("uses_bad.tl"),
            "local bad = require(\"bad\")\nreturn { x = bad.x }\n",
        )
        .unwrap();
        with_runner(&root, Duration::from_secs(30), |runner| async move {
            match runner.instance("uses_bad", &[]).await {
                Err(CallError::Check {
                    module,
                    diagnostics,
                }) => {
                    assert_eq!(module, "uses_bad");
                    assert!(
                        !diagnostics.is_empty()
                            && diagnostics.iter().all(|d| d.file.ends_with("bad.tl")),
                        "{diagnostics:?}"
                    );
                }
                other => panic!("{:?}", other.map(|_| ())),
            }
        });
    }

    #[test]
    fn an_absent_module_is_not_found() {
        let root = dir("absent");
        with_runner(&root, Duration::from_secs(30), |runner| async move {
            assert!(matches!(
                runner.instance("absent", &[]).await.map(|_| ()),
                Err(CallError::NotFound { module }) if module == "absent"
            ));
            // A declaration with nothing behind it has nothing to run.
            assert!(matches!(
                runner.instance("declared", &[]).await.map(|_| ()),
                Err(CallError::NotFound { module }) if module == "declared"
            ));
        });
    }

    /// `cargo test` runs in the package root, where `./?.lua` would find `src/fmt.lua` as
    /// `src.fmt`. An instance does not search the working directory.
    #[test]
    fn the_working_directory_is_not_searched() {
        assert!(
            Path::new("src/fmt.lua").is_file(),
            "the test needs src/fmt.lua in the cwd"
        );
        let root = dir("cwd");
        with_runner(&root, Duration::from_secs(30), |runner| async move {
            assert!(matches!(
                runner.instance("src.fmt", &[]).await.map(|_| ()),
                Err(CallError::NotFound { module }) if module == "src.fmt"
            ));
        });
    }

    #[test]
    fn a_plain_lua_module_loads_without_a_check() {
        let root = dir("plain");
        with_runner(&root, Duration::from_secs(30), |runner| async move {
            let plain = runner.instance("plain", &[]).await.unwrap();
            assert_eq!(plain.call::<_, i64>("add", (4, 5)).await.unwrap(), 9);
        });
    }

    #[test]
    fn a_closed_instance_is_gone_and_closing_twice_is_fine() {
        let root = dir("close");
        with_runner(&root, Duration::from_secs(30), |runner| async move {
            let a = runner.instance("m", &[]).await.unwrap();
            let b = runner.instance("m", &[]).await.unwrap();
            let a2 = a.clone();
            a.close().await.unwrap();
            assert!(matches!(
                a2.call::<_, i64>("bump", ()).await,
                Err(CallError::Gone)
            ));
            a2.close().await.unwrap();
            // Another instance is not affected.
            assert_eq!(b.call::<_, i64>("bump", ()).await.unwrap(), 1);
        });
    }

    #[test]
    fn dropping_a_call_stops_it_and_the_next_request_is_served() {
        let root = dir("drop");
        // A deadline far off: only the drop can end the loop in good time.
        with_runner(&root, Duration::from_secs(60), |runner| async move {
            let m = runner.instance("m", &[]).await.unwrap();
            let gave_up =
                tokio::time::timeout(Duration::from_millis(200), m.call::<_, ()>("spin", ())).await;
            assert!(gave_up.is_err(), "the loop ended by itself");
            let dropped = Instant::now();
            assert_eq!(m.call::<_, i64>("add", (1, 1)).await.unwrap(), 2);
            assert!(dropped.elapsed() < Duration::from_secs(10));
        });
    }

    #[test]
    fn a_request_dropped_in_the_queue_is_never_run() {
        let root = dir("skip");
        with_runner(&root, Duration::from_secs(60), |runner| async move {
            let m = runner.instance("m", &[]).await.unwrap();
            // Holds the VM while the second request is queued and then given up on.
            let first = tokio::spawn({
                let m = m.clone();
                async move { m.call::<_, ()>("busy", (2,)).await }
            });
            tokio::time::sleep(Duration::from_millis(100)).await;
            let gave_up =
                tokio::time::timeout(Duration::from_millis(100), m.call::<_, i64>("bump", ()))
                    .await;
            assert!(gave_up.is_err());
            first.await.unwrap().unwrap();
            // `bump` was skipped, so the counter has not moved.
            assert_eq!(m.call::<_, i64>("bump", ()).await.unwrap(), 1);
        });
    }

    #[test]
    fn time_in_the_queue_is_not_counted_against_the_deadline() {
        let root = dir("queue");
        let after = Duration::from_millis(2500);
        with_runner(&root, after, |runner| async move {
            let m = runner.instance("m", &[]).await.unwrap();
            // Each runs for more than a second and at most two, under the deadline. The
            // third waits behind two of them, more than two seconds, and then runs: from
            // the moment it was asked for it is past the deadline, from the moment it was
            // started it is not.
            let started = Instant::now();
            let calls: Vec<_> = (0..3)
                .map(|_| {
                    let m = m.clone();
                    tokio::spawn(async move { m.call::<_, ()>("busy", (2,)).await })
                })
                .collect();
            for call in calls {
                call.await.unwrap().unwrap();
            }
            assert!(started.elapsed() > after, "{:?}", started.elapsed());
        });
    }

    #[test]
    fn shutdown_joins_the_threads() {
        let root = dir("shutdown");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let (runner, driver) = start(RunnerConfig {
                search_paths: vec![root.to_path_buf()],
                deadline: Duration::from_secs(30),
                queue: 4,
            })
            .await
            .unwrap();
            let m = runner.instance("m", &[]).await.unwrap();
            let probe = driver.probe();
            assert!(probe.upgrade().is_some());
            driver.shutdown().await.unwrap();
            // The VM thread and the watchdog each held the deadlines, and the driver: all
            // gone means both threads ended.
            assert!(probe.upgrade().is_none());
            assert!(matches!(
                m.call::<_, i64>("add", (1, 1)).await,
                Err(CallError::Gone)
            ));
            assert!(matches!(
                runner.instance("m", &[]).await.map(|_| ()),
                Err(CallError::Gone)
            ));
        });
    }
}
