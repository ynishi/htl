//! The executor a program that awaits runs on: `Htl::run_async` / `run_blocking` drive
//! the entry chunk as a root coroutine of mlua-isle's `Vm::run`, under a cancel token,
//! with the grace and preemption of `[async]`. Step 2 of the async design: a host's
//! `async fn` becomes callable from a program without the host driving the coroutine
//! itself, and a cancel reaches the program the way `htl run` delivers Ctrl-C. The
//! `Interrupt` under the executor (acceptance 9 of the issue) is in `c_export.rs`, with
//! the host that has one.

#![cfg(feature = "async")]

use htl::config::AsyncConfig;
use htl::mlua_isle::runtime::CancelToken;
use htl::{Htl, host_module};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Set when the future of a host call is dropped: the only way to see the executor let
/// go of it, as against the call returning.
struct DropGuard(Arc<AtomicBool>);

impl Drop for DropGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

pub struct Api {
    dropped: Arc<AtomicBool>,
}

#[host_module(name = "api")]
impl Api {
    /// The shape a network call has: sleeps `ms`, then hands `path` back.
    pub async fn get(&self, path: String, ms: u64) -> String {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        path
    }

    /// Never resolves on its own: only a cancel (or a drop) ends it.
    pub async fn forever(&self) -> String {
        std::future::pending::<()>().await;
        "never".to_string()
    }

    /// `get` with a guard on its future, so a test can tell "dropped" from "returned".
    pub async fn watched(&self, ms: u64) -> String {
        let _guard = DropGuard(self.dropped.clone());
        tokio::time::sleep(Duration::from_millis(ms)).await;
        "done".to_string()
    }
}

fn host() -> (Htl, Htl, Arc<AtomicBool>) {
    let checker = Htl::new().unwrap();
    let h = Htl::with_checker(&checker).unwrap();
    let dropped = Arc::new(AtomicBool::new(false));
    Api {
        dropped: dropped.clone(),
    }
    .htl_preload(&h)
    .unwrap();
    (checker, h, dropped)
}

/// `run_blocking` on `src`, with `token` cancelled after `cancel_after` on a thread of
/// its own — the way Ctrl-C reaches `htl run` — or never. Returns the result and how long
/// the run took.
fn run(h: &Htl, src: &str, cancel_after: Option<Duration>) -> (anyhow::Result<()>, Duration) {
    let token = CancelToken::new();
    if let Some(after) = cancel_after {
        let t = token.clone();
        std::thread::spawn(move || {
            std::thread::sleep(after);
            t.cancel();
        });
    }
    let started = Instant::now();
    let out = h.run_blocking(src, "=test", &[], &token);
    (out, started.elapsed())
}

/// (a) A host `async fn` called from a chunk on the executor: the call suspends the root
/// and resumes it with the value, where `exec` would fail to yield.
#[test]
fn an_async_host_method_is_called_sequentially_from_a_chunk() {
    let (_c, h, _) = host();
    let (out, took) = run(
        &h,
        "local api = require('api') \
         GOT = api:get('/a', 300) .. ' ' .. api:get('/b', 10)",
        None,
    );
    out.unwrap();
    let got: String = h.lua().globals().get("GOT").unwrap();
    assert_eq!(got, "/a /b");
    assert!(took >= Duration::from_millis(300), "it waited: {took:?}");
}

/// (b) Grace 0: a cancel while the program awaits a host future drops that future at
/// once, and the run resolves to the cancel.
#[test]
fn a_cancel_with_no_grace_drops_the_pending_host_future_at_once() {
    let (_c, h, dropped) = host();
    h.configure_async(&AsyncConfig {
        grace_ms: Some(0),
        preempt: None,
    })
    .unwrap();
    let (out, took) = run(
        &h,
        "local api = require('api') api:watched(1000)",
        Some(Duration::from_millis(100)),
    );
    let e = out.unwrap_err();
    assert!(htl::is_cancelled(&e), "{e:#}");
    assert!(
        dropped.load(Ordering::SeqCst),
        "the host future was dropped"
    );
    assert!(
        took < Duration::from_millis(400),
        "not the full second: {took:?}"
    );
}

/// (c) Grace 500 ms: the macro wraps the method in `cancellable`, so the cancel returns
/// at the await within a few ms, not when the grace runs out.
#[test]
fn a_wrapped_host_method_returns_the_cancel_at_once_under_a_grace() {
    let (_c, h, dropped) = host();
    h.configure_async(&AsyncConfig {
        grace_ms: Some(500),
        preempt: None,
    })
    .unwrap();
    let (out, took) = run(
        &h,
        "local api = require('api') api:watched(1000)",
        Some(Duration::from_millis(100)),
    );
    assert!(htl::is_cancelled(&out.unwrap_err()));
    assert!(dropped.load(Ordering::SeqCst));
    assert!(
        took < Duration::from_millis(350),
        "returned at the cancel, not at 100 + 500 ms: {took:?}"
    );
}

/// (d) With a grace, the cancel is an error the program unwinds through: a `<close>`
/// handler runs, and may itself await a host call.
#[test]
fn a_close_handler_runs_on_a_cancel_with_a_grace() {
    let (_c, h, _) = host();
    h.configure_async(&AsyncConfig {
        grace_ms: Some(500),
        preempt: None,
    })
    .unwrap();
    let (out, _) = run(
        &h,
        "local api = require('api') \
         local guard <close> = setmetatable({}, { __close = function() \
            CLOSED = true \
            local ok, err = pcall(api.get, api, '/cleanup', 1) \
            CLEANUP = ok and 'awaited' or 'cancelled' \
         end }) \
         api:get('/long', 1000)",
        Some(Duration::from_millis(50)),
    );
    assert!(htl::is_cancelled(&out.unwrap_err()));
    let closed: bool = h.lua().globals().get("CLOSED").unwrap();
    assert!(closed, "the handler ran");
    // A wrapped call inside the cleanup sees the cancelled token and returns the cancel:
    // cleanup that has to wait is a call the macro did not wrap.
    let cleanup: String = h.lua().globals().get("CLEANUP").unwrap();
    assert_eq!(cleanup, "cancelled");
}

/// (f) The grace is a tokio timeout: a runtime without a time driver panics at the first
/// cancel that finds the program suspended in a host call — the moment the runtime starts
/// the grace clock. `run_async`'s doc asks for `enable_all()`; this pins why. (A program
/// in a CPU loop takes the cancel from the hook inside its poll and never reaches the
/// clock, so it is not the case that shows it.)
#[test]
#[should_panic(expected = "timers are disabled")]
fn a_runtime_without_a_time_driver_panics_at_the_first_cancel() {
    let (_c, h, _) = host();
    h.configure_async(&AsyncConfig {
        grace_ms: Some(500),
        preempt: None,
    })
    .unwrap();
    let token = CancelToken::new();
    let t = token.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        t.cancel();
    });
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    let _ = local.block_on(
        &rt,
        h.run_async(
            "local api = require('api') api:forever()",
            "=pending",
            &[],
            &token,
        ),
    );
}

/// (g) When the run resolves on a cancel, nothing the program started is alive: a child
/// task pending in a host call has been dropped before `run_blocking` returns.
#[test]
fn a_child_task_is_gone_when_a_cancelled_run_resolves() {
    let (_c, h, dropped) = host();
    let task = h.hook_owner().unwrap().task_lib().unwrap();
    h.lua().globals().set("task", task).unwrap();
    let (out, _) = run(
        &h,
        "local api = require('api') \
         local child = task.spawn(function() return api:watched(2000) end) \
         api:get('/parent', 2000)",
        Some(Duration::from_millis(100)),
    );
    assert!(htl::is_cancelled(&out.unwrap_err()));
    assert!(
        dropped.load(Ordering::SeqCst),
        "the child's host future was dropped before the run resolved"
    );
}

/// A program that raises comes back as the same error shape `exec` returns: the message
/// and the `stack traceback:` block, without the executor's own frames under the
/// program's.
#[test]
fn a_raised_error_reads_as_it_does_from_exec() {
    let (_c, h, _) = host();
    let src =
        "local function inner() error('boom') end\nlocal function outer() inner() end\nouter()";
    let from_exec = htl::developer_message(&h.exec(src, "=test", &[]).unwrap_err());
    let (out, _) = run(&h, src, None);
    let from_run = htl::developer_message(&out.unwrap_err());
    assert!(
        from_run.starts_with("runtime error: test:1: boom"),
        "{from_run}"
    );
    assert!(
        from_run.contains("test:2: in"),
        "the program's frames: {from_run}"
    );
    assert!(
        !from_run.contains("mlua_isle"),
        "no executor frames: {from_run}"
    );
    assert!(
        !from_run.contains("xpcall"),
        "no executor frames: {from_run}"
    );
    assert_eq!(
        from_run.lines().next(),
        from_exec.lines().next(),
        "same head line\nexec: {from_exec}\nrun: {from_run}"
    );
}

/// A host on its own runtime awaits `run_async` inside a `LocalSet` and keeps its
/// executor; the same program, the same value.
#[tokio::test]
async fn a_host_awaits_run_async_on_its_own_runtime() {
    let (_c, h, _) = host();
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let token = CancelToken::new();
            h.run_async(
                "local api = require('api') GOT = api:get('/x', 5)",
                "=host",
                &[],
                &token,
            )
            .await
            .unwrap();
        })
        .await;
    let got: String = h.lua().globals().get("GOT").unwrap();
    assert_eq!(got, "/x");
}
