//! `htl.task`: tasks a program on the executor spawns and awaits. Step 3 of the async
//! design: mlua-isle's task library under a declaration, with `await` over `join`, and
//! the structure the library promises — an unjoined child is cancelled and waited for
//! when its parent ends or its scope closes, a child's error reaches the parent as the
//! value it raised, and a cancel reaches it as one `is_cancelled` recognises.

#![cfg(feature = "async")]

use htl::config::AsyncConfig;
use htl::mlua_isle::runtime::CancelToken;
use htl::{Htl, host_module};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Set when the future of a host call is dropped, as against the call returning.
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
    /// Sleeps `ms`, then hands `path` back: the shape a network call has.
    pub async fn get(&self, path: String, ms: u64) -> String {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        path
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
    h.install_task_lib().unwrap();
    (checker, h, dropped)
}

/// `run_blocking` on `src`, timed; the token is never cancelled. A child starts when its
/// parent next suspends, so the programs below await a short host call after `spawn`
/// (`api:get('', 20)`) before looking at the child: a child cancelled before it ran has
/// no future to drop.
fn run(h: &Htl, src: &str) -> (anyhow::Result<()>, Duration) {
    let token = CancelToken::new();
    let started = Instant::now();
    let out = h.run_blocking(src, "=test", &[], &token);
    (out, started.elapsed())
}

fn global<T: htl::mlua::FromLua>(h: &Htl, name: &str) -> T {
    h.lua().globals().get(name).unwrap()
}

/// (S1) Two children each awaiting a 300 ms host call, both awaited by the parent: the
/// two waits overlap, and each value comes back to the task that asked for it.
#[test]
fn two_children_wait_together_and_each_hands_back_its_value() {
    let (_c, h, _) = host();
    let (out, took) = run(
        &h,
        "local task = require('htl.task') \
         local api = require('api') \
         local a = task.spawn(function() return api:get('/a', 300) end) \
         local b = task.spawn(function() return api:get('/b', 300) end) \
         GOT = task.await(a) .. ' ' .. task.await(b)",
    );
    out.unwrap();
    eprintln!("S1 wall: {took:?}");
    assert_eq!(global::<String>(&h, "GOT"), "/a /b");
    assert!(
        took >= Duration::from_millis(300) && took < Duration::from_millis(450),
        "600 ms of sleep, overlapped: {took:?}"
    );
}

/// (S4b) The root returns with a child still sleeping: the child is cancelled and its
/// host future dropped before `run_blocking` returns, not when the state goes away.
#[test]
fn a_child_left_sleeping_is_dropped_before_the_run_returns() {
    let (_c, h, dropped) = host();
    let (out, took) = run(
        &h,
        "local task = require('htl.task') \
         local api = require('api') \
         task.spawn(function() return api:watched(5000) end) \
         api:get('', 20) \
         DONE = true",
    );
    out.unwrap();
    assert!(global::<bool>(&h, "DONE"));
    assert!(
        dropped.load(Ordering::SeqCst),
        "the child's future was dropped"
    );
    assert!(
        took < Duration::from_millis(200),
        "not waited out: {took:?}"
    );
}

/// (S5) A child raises a table: `await` in the parent raises that table, as it was.
#[test]
fn a_childs_error_reaches_the_parent_as_the_value_it_raised() {
    let (_c, h, _) = host();
    let (out, _) = run(
        &h,
        "local task = require('htl.task') \
         local t = task.spawn(function() error({ code = 42 }) end) \
         local ok, err = pcall(task.await, t) \
         OK = ok \
         CODE = type(err) == 'table' and err.code or nil \
         CANCELLED = task.is_cancelled(err)",
    );
    out.unwrap();
    assert!(!global::<bool>(&h, "OK"));
    assert_eq!(global::<i64>(&h, "CODE"), 42);
    assert!(!global::<bool>(&h, "CANCELLED"));
}

/// A cancelled child: `await` raises a value `is_cancelled` is true for, and the parent
/// goes on.
#[test]
fn awaiting_a_cancelled_child_raises_a_cancellation() {
    let (_c, h, dropped) = host();
    let (out, took) = run(
        &h,
        "local task = require('htl.task') \
         local api = require('api') \
         local t = task.spawn(function() return api:watched(5000) end) \
         api:get('', 20) \
         t:cancel() \
         local ok, err = pcall(task.await, t) \
         OK = ok \
         CANCELLED = task.is_cancelled(err) \
         AFTER = true",
    );
    out.unwrap();
    assert!(!global::<bool>(&h, "OK"));
    assert!(global::<bool>(&h, "CANCELLED"));
    assert!(global::<bool>(&h, "AFTER"));
    assert!(dropped.load(Ordering::SeqCst));
    assert!(took < Duration::from_millis(500), "{took:?}");
}

/// `<close>`: a handle the scope leaves without awaiting is cancelled and waited for at
/// the scope's end, and the parent continues past it.
#[test]
fn a_close_handle_cancels_its_child_when_the_scope_ends() {
    let (_c, h, dropped) = host();
    let (out, took) = run(
        &h,
        "local task = require('htl.task') \
         local api = require('api') \
         do \
            local h <close> = task.spawn(function() return api:watched(5000) end) \
            api:get('', 20) \
            BEFORE = h:done() \
         end \
         AFTER = true",
    );
    out.unwrap();
    assert!(
        !global::<bool>(&h, "BEFORE"),
        "still running inside the scope"
    );
    assert!(global::<bool>(&h, "AFTER"));
    assert!(dropped.load(Ordering::SeqCst), "dropped at the scope's end");
    assert!(took < Duration::from_millis(500), "{took:?}");
}

/// (S6) `spawn` under a plain `exec`: the library loads, and the first `spawn` raises
/// mlua-isle's own error rather than hanging.
#[test]
fn spawn_under_exec_raises_rather_than_hanging() {
    let (_c, h, _) = host();
    let err = h
        .exec(
            "local task = require('htl.task') \
             task.spawn(function() return 1 end)",
            "=test",
            &[],
        )
        .unwrap_err();
    let text = format!("{err:#}");
    assert!(
        text.contains("sync requests cannot spawn"),
        "mlua-isle names the reason: {text}"
    );
}

/// A task is awaited once: the second `await` is htl's own error, not mlua-isle's, and
/// the method and the function are one call.
#[test]
fn a_second_await_of_one_task_is_an_error_that_says_so() {
    let (_c, h, _) = host();
    let (out, _) = run(
        &h,
        "local task = require('htl.task') \
         local t = task.spawn(function() return 7 end) \
         FIRST = t:await() \
         local ok, err = pcall(task.await, t) \
         OK = ok \
         ERR = tostring(err)",
    );
    out.unwrap();
    assert_eq!(global::<i64>(&h, "FIRST"), 7);
    assert!(!global::<bool>(&h, "OK"));
    assert_eq!(
        global::<String>(&h, "ERR"),
        "htl.task: task already awaited"
    );
}

/// The grace applies to the tree: with `grace_ms = 0` a child cancelled by its scope is
/// dropped at once, the same as under a cancel of the root.
#[test]
fn the_projects_grace_applies_to_a_childs_cancel() {
    let (_c, h, dropped) = host();
    h.configure_async(&AsyncConfig {
        grace_ms: Some(0),
        preempt: None,
    })
    .unwrap();
    let (out, took) = run(
        &h,
        "local task = require('htl.task') \
         local api = require('api') \
         task.spawn(function() return api:watched(5000) end) \
         api:get('', 20)",
    );
    out.unwrap();
    assert!(dropped.load(Ordering::SeqCst));
    assert!(took < Duration::from_millis(200), "{took:?}");
}
