//! `async fn` in a `#[host_module]` impl.
//!
//! A host that talks to the network is async at the bottom, because the libraries under
//! it are, and the functions it wants to hand to Lua are the ones that do the talking.
//! Without this such a host writes its `UserData` by hand and loses the `.d.tl` the macro
//! would have written, which is the half of `#[host_module]` that has no substitute.
//!
//! Three things are worth pinning down, and each has a test: the declaration does not
//! change (an async function is an ordinary call from Lua — it yields internally and
//! hands back the same values), sync and async methods share one impl block, and the
//! executor is the caller's (mlua yields to it and provides none of its own).

#![cfg(feature = "async")]

use htl::mlua::UserData;
use htl::{Htl, host_module};

pub struct Api {
    calls: u32,
}

#[host_module(name = "api")]
impl Api {
    /// Sync, in the same block as the async ones.
    pub fn seen(&self) -> u32 {
        self.calls
    }

    /// The shape a network call has: `&self`, a parameter, a value back.
    pub async fn fetch(&self, path: String) -> String {
        tokio::task::yield_now().await;
        format!("got {path}")
    }

    /// `&mut self`: the receiver is borrowed across every await, so nothing else may take
    /// it exclusively while the future is live. mlua says that in the type it hands over.
    pub async fn record(&mut self) -> u32 {
        tokio::task::yield_now().await;
        self.calls += 1;
        self.calls
    }

    /// No receiver, and a `Result`: the error handling is the same as the sync path's.
    pub async fn parse(s: String) -> Result<i64, std::num::ParseIntError> {
        tokio::task::yield_now().await;
        s.parse::<i64>()
    }
}

fn host() -> Htl {
    let h = Htl::new().unwrap();
    Api { calls: 0 }.htl_preload(&h).unwrap();
    h
}

/// The `.d.tl` an async method produces is the one the same signature produces without
/// `async`. Teal has no notion of a coroutine-only call, and the values are the same.
#[test]
fn the_declaration_says_nothing_about_async() {
    let decl = <Api as htl::teal::HostModule>::DECL;
    assert!(
        decl.contains("seen: function(self: api): integer"),
        "{decl}"
    );
    assert!(
        decl.contains("fetch: function(self: api, path: string): string"),
        "the async method reads like the sync one: {decl}"
    );
    assert!(
        decl.contains("record: function(self: api): integer"),
        "{decl}"
    );
    assert!(
        decl.contains("parse: function(s: string): integer"),
        "{decl}"
    );
    assert!(!decl.contains("async"), "nothing leaks into Teal: {decl}");
}

/// Sync and async in one impl block, with no annotation saying which it contains: the
/// sync ones are still callable the ordinary way.
#[tokio::test]
async fn a_sync_method_beside_async_ones_is_unchanged() {
    let h = host();
    let seen: u32 = h
        .lua()
        .load("local api = require('api') return api:seen()")
        .eval()
        .unwrap();
    assert_eq!(seen, 0, "no coroutine needed for the sync half");
}

/// The async ones run inside a coroutine, driven by the caller's executor.
#[tokio::test]
async fn async_methods_run_through_call_async() {
    let h = host();
    let f: htl::mlua::Function = h
        .lua()
        .load("return function() local api = require('api') return api:fetch('/ip') end")
        .eval()
        .unwrap();
    let got: String = f.call_async(()).await.unwrap();
    assert_eq!(got, "got /ip");

    // `&mut self` across an await, twice: the second call sees the first one's write.
    let bump: htl::mlua::Function = h
        .lua()
        .load("return function() local api = require('api') return api:record() end")
        .eval()
        .unwrap();
    assert_eq!(bump.call_async::<u32>(()).await.unwrap(), 1);
    assert_eq!(bump.call_async::<u32>(()).await.unwrap(), 2);
}

/// A `Result` from an async fn raises on the Lua side exactly as the sync path does.
#[tokio::test]
async fn a_result_from_an_async_fn_raises() {
    let h = host();
    let f: htl::mlua::Function = h
        .lua()
        .load("return function(s) local api = require('api') return api.parse(s) end")
        .eval()
        .unwrap();
    assert_eq!(f.call_async::<i64>("41").await.unwrap(), 41);
    let e = f.call_async::<i64>("not a number").await.unwrap_err();
    assert!(e.to_string().contains("invalid digit"), "{e}");
}

/// Called from outside a coroutine there is nothing to suspend, and Lua says so. The
/// message is mlua's; what matters is that it is an error rather than a hang.
#[tokio::test]
async fn calling_an_async_method_outside_a_coroutine_is_an_error() {
    let h = host();
    let e = h
        .lua()
        .load("local api = require('api') return api:fetch('/ip')")
        .eval::<String>()
        .unwrap_err()
        .to_string();
    assert!(e.contains("yield"), "{e}");
}

/// The generated `UserData` impl is still one: nothing about async changes the shape of
/// what `htl_preload` registers.
#[test]
fn the_generated_userdata_is_unchanged() {
    fn assert_userdata<T: UserData>() {}
    assert_userdata::<Api>();
}
