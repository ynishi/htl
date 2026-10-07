//! `&Lua` as a `#[host_module]` parameter (#427): filled from the closure's own Lua
//! handle rather than from the Lua arguments, so a method can build whatever it needs the
//! state for — a table, a function, a `htl::task::RecvChannel<T>::new(lua, cap)` — inside
//! the call instead of having it built beforehand, stored on the host, and cloned out.
//! Left out of the `.d.tl` entirely, the way `&self` is.
//!
//! Gated on `async` because the channel case (`events`) needs `htl::task`; the sync cases
//! and the async-fn case (`counted`) are kept in the same impl and file for the same
//! reason `host_async.rs` keeps sync and async methods together.

#![cfg(feature = "async")]

use htl::mlua_isle::runtime::CancelToken;
use htl::task::{RecvChannel, Sender};
use htl::teal::HostModule as _;
use htl::{Htl, host_module};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct Host {
    /// The sender half `events()` makes, handed to the test so it can feed the channel
    /// from outside while Teal awaits the receiver — populated only once `events()` has
    /// run, which is why it starts `None`.
    events_tx: Arc<Mutex<Option<Sender<i64>>>>,
}

#[host_module(name = "host")]
impl Host {
    /// The issue's own case: nothing built beforehand, nothing stored on the host.
    pub fn count(&self, lua: &htl::mlua::Lua) -> i64 {
        lua.used_memory() as i64
    }

    /// The channel is built inside the method, with the state the closure already has;
    /// the sender is handed to the test (through the host, since nothing else reaches
    /// out of a `#[host_module]` method) rather than used here, so the test can send
    /// while Teal is genuinely waiting on an empty channel rather than reading a value
    /// already queued.
    pub fn events(&self, lua: &htl::mlua::Lua) -> RecvChannel<i64> {
        let (tx, ch) = RecvChannel::<i64>::new(lua, 1).unwrap();
        *self.events_tx.lock().unwrap() = Some(tx);
        ch
    }

    /// `async fn` taking `&Lua`: the closure hands over an owned `Lua`, and the wrapper
    /// borrows from it for the duration of the future.
    pub async fn counted(&self, lua: &htl::mlua::Lua) -> i64 {
        tokio::task::yield_now().await;
        lua.used_memory() as i64
    }

    /// A `&Lua` parameter after a regular one: its own position in the signature must not
    /// shift `n`'s value or its place in the call.
    pub fn after(&self, n: i64, lua: &htl::mlua::Lua) -> i64 {
        let _ = lua;
        n
    }

    /// The parameter is named `this` — the same name the generated wrapper's own receiver
    /// binding would use if the macro spliced parameter names into its generated code
    /// (it does not: the binding is internal and fixed, `LuaParam::name` is for error
    /// messages only).
    pub fn named_this(&self, this: &htl::mlua::Lua) -> i64 {
        this.used_memory() as i64
    }
}

fn host() -> (Htl, Arc<Mutex<Option<Sender<i64>>>>) {
    let h = Htl::new().unwrap();
    h.install_task_lib().unwrap();
    let events_tx = Arc::new(Mutex::new(None));
    Host {
        events_tx: events_tx.clone(),
    }
    .htl_preload(&h)
    .unwrap();
    (h, events_tx)
}

// ---------------------------------------------------------------- 1. count(&self, lua: &Lua)

#[test]
fn the_declaration_leaves_the_lua_parameter_out() {
    assert!(
        Host::DECL.contains("count: function(self: host): integer\n"),
        "{}",
        Host::DECL
    );
}

#[test]
fn require_host_count_from_teal_returns_an_integer() {
    let (h, _tx) = host();
    let n: i64 = h
        .lua()
        .load("local host = require('host'); return host:count()")
        .eval()
        .unwrap();
    assert!(n > 0, "mlua's used_memory should be > 0, got {n}");
}

// ------------------------------------------------- 2. events(&self, lua) -> RecvChannel<i64>

/// No `lua` parameter on `events` either, same as `count`, and the channel is typed by
/// its element — the same channel mapping any `#[host_module]` return gets (the
/// `host_module` macro doc, *A Lua function as a parameter* section's neighbour).
#[test]
fn the_channel_methods_declaration_also_leaves_lua_out() {
    assert!(
        Host::DECL.contains("events: function(self: host): task.RecvChannel<integer>\n"),
        "{}",
        Host::DECL
    );
}

/// The value is sent only after Teal is already blocked on `ch:recv()`, from a thread of
/// its own (the pattern `task_channels.rs`'s asker thread uses for its `Request`): this is
/// the issue's own case end to end, not a pre-buffered value read back.
#[test]
fn teal_receives_a_value_sent_while_it_is_genuinely_waiting_on_the_channel() {
    let (h, events_tx) = host();
    let sender = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        let tx = events_tx
            .lock()
            .unwrap()
            .take()
            .expect("events() has populated the sender by now");
        tx.try_send(42).unwrap();
    });
    h.run_blocking(
        "local host = require('host') \
         local ch = host:events() \
         local v, ok = ch:recv() \
         GOT = v \
         OK = ok",
        "=test",
        &[],
        &CancelToken::new(),
    )
    .unwrap();
    sender.join().unwrap();
    let got: i64 = h.lua().globals().get("GOT").unwrap();
    let ok: bool = h.lua().globals().get("OK").unwrap();
    assert_eq!(got, 42);
    assert!(ok);
}

// ---------------------------------------------------------------- 3. async fn + &Lua

#[test]
fn the_async_methods_declaration_also_leaves_lua_out() {
    assert!(
        Host::DECL.contains("counted: function(self: host): integer ---@async"),
        "{}",
        Host::DECL
    );
}

#[tokio::test]
async fn an_async_method_taking_lua_runs_through_call_async() {
    let (h, _tx) = host();
    let f: htl::mlua::Function = h
        .lua()
        .load("return function() local host = require('host') return host:counted() end")
        .eval()
        .unwrap();
    let got: i64 = f.call_async(()).await.unwrap();
    assert!(got > 0, "{got}");
}

// ---------------------------------------------------------------- 4. lua after a parameter

#[test]
fn the_declaration_of_a_lua_parameter_after_a_regular_one_keeps_the_regular_one() {
    assert!(
        Host::DECL.contains("after: function(self: host, n: integer): integer\n"),
        "{}",
        Host::DECL
    );
}

#[test]
fn a_lua_parameter_after_a_regular_one_does_not_disturb_its_value() {
    let (h, _tx) = host();
    let n: i64 = h
        .lua()
        .load("local host = require('host'); return host:after(7)")
        .eval()
        .unwrap();
    assert_eq!(n, 7);
}

// ---------------------------------------------------------------- 5. lua parameter named `this`

#[test]
fn a_lua_parameter_named_this_does_not_collide_with_the_receiver() {
    let (h, _tx) = host();
    let n: i64 = h
        .lua()
        .load("local host = require('host'); return host:named_this()")
        .eval()
        .unwrap();
    assert!(n > 0, "{n}");
}
