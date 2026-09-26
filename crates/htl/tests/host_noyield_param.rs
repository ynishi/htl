//! A `#[host_module]` method taking a Lua function: which of those parameters the `.d.tl`
//! names in `---@noyield(..)`, and that the methods still register and run once the macro
//! has stripped the `#[teal(yields)]` / `#[teal(noyield)]` it read.
//!
//! A sync fn can run its callback only with `Function::call`, from C, so its `Function`
//! parameters are named; an `async fn` is expected to `call_async`, so it names none; the
//! attribute on a parameter turns either default around.

#![cfg(feature = "async")]

use htl::mlua::Function;
use htl::teal::HostModule as _;
use htl::{Htl, host_module};
use std::sync::Mutex;

pub struct Api {
    stored: Mutex<Option<Function>>,
}

#[host_module(name = "api")]
impl Api {
    pub fn each(&self, f: Function) -> String {
        f.call::<String>("a").unwrap()
    }
    pub fn walk(&self, f: Option<Function>) {
        if let Some(f) = f {
            f.call::<()>(()).unwrap();
        }
    }
    pub fn split(&self, on_ok: Function, on_err: Function) -> String {
        let a: String = on_ok.call(()).unwrap();
        let b: String = on_err.call(()).unwrap();
        format!("{a}{b}")
    }
    pub async fn each_async(&self, f: Function) -> String {
        f.call_async::<String>("b").await.unwrap()
    }
    /// Stored and called later by the host: `f` is left out of the marker.
    pub fn on(&self, #[teal(yields)] f: Function) {
        *self.stored.lock().unwrap() = Some(f);
    }
    /// Calls its callback with the sync `call` on purpose: `f` is named beside `---@async`.
    pub async fn each_sync_call(&self, #[teal(noyield)] f: Function) -> String {
        f.call::<String>("c").unwrap()
    }
    pub fn map(&self, xs: Vec<i64>, f: Function) -> Vec<i64> {
        xs.into_iter().map(|x| f.call::<i64>(x).unwrap()).collect()
    }
}

fn host() -> (Htl, Api) {
    let h = Htl::new().unwrap();
    let api = Api {
        stored: Mutex::new(None),
    };
    (h, api)
}

/// Every shape, one line each, exactly as the checker reads it.
#[test]
fn each_function_parameter_is_declared_by_the_method_s_shape() {
    for line in [
        "   each: function(self: api, f: function): string ---@noyield(f)\n",
        "   walk: function(self: api, f?: function) ---@noyield(f)\n",
        "   split: function(self: api, on_ok: function, on_err: function): string ---@noyield(on_ok, on_err)\n",
        "   each_async: function(self: api, f: function): string ---@async\n",
        "   on: function(self: api, f: function)\n",
        "   each_sync_call: function(self: api, f: function): string ---@async ---@noyield(f)\n",
        "   map: function(self: api, xs: {integer}, f: function): {integer} ---@noyield(f)\n",
    ] {
        assert!(Api::DECL.contains(line), "{line:?} not in\n{}", Api::DECL);
    }
    assert_eq!(Api::DECL.matches("---@noyield").count(), 5, "{}", Api::DECL);
}

/// The attribute is gone from the emitted impl (this file compiles), and the sync methods
/// with a callback are registered and run.
#[test]
fn the_sync_methods_are_registered_and_callable() {
    let (h, api) = host();
    api.htl_preload(&h).unwrap();
    let got: String = h
        .lua()
        .load(
            "local api = require('api')
             local seen = 0
             api:walk(function() seen = seen + 1 end)
             api:walk()
             api:on(function() end)
             local m = api:map({1, 2}, function(x) return x * 10 end)
             return api:each(function(s) return s .. '!' end)
                .. api:split(function() return 'o' end, function() return 'e' end)
                .. seen .. m[1] .. m[2]",
        )
        .eval()
        .unwrap();
    assert_eq!(got, "a!oe11020");
}

/// The async ones, driven by `call_async` as `htl run` drives a program.
#[tokio::test]
async fn the_async_methods_run_through_call_async() {
    let (h, api) = host();
    api.htl_preload(&h).unwrap();
    let f: Function = h
        .lua()
        .load(
            "return function()
               local api = require('api')
               return api:each_async(function(s) return s .. '?' end)
                 .. api:each_sync_call(function(s) return s .. '.' end)
             end",
        )
        .eval()
        .unwrap();
    let got: String = f.call_async(()).await.unwrap();
    assert_eq!(got, "b?c.");
}

/// `on` stored its callback: the host holds a `Function` it may call when it chooses.
#[test]
fn the_stored_callback_is_held_by_the_host() {
    let (h, api) = host();
    let api = h.lua().create_userdata(api).unwrap();
    h.lua().globals().set("api", &api).unwrap();
    h.lua()
        .load("api:on(function() return 'later' end)")
        .exec()
        .unwrap();
    let stored = api
        .borrow::<Api>()
        .unwrap()
        .stored
        .lock()
        .unwrap()
        .clone()
        .unwrap();
    assert_eq!(stored.call::<String>(()).unwrap(), "later");
}
