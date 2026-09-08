//! A `#[host_module]` method taking an `Option<T>`: declared `name?: T` on the Teal side,
//! and callable from Lua with the argument left out, passed as nil, or passed as a value.

use htl::teal::HostModule as _;
use htl::{Htl, host_module};

pub struct Api;

#[host_module(name = "api")]
impl Api {
    pub fn find(&self, name: &str, scope: Option<String>) -> String {
        match scope {
            Some(s) => format!("{s}/{name}"),
            None => format!("-/{name}"),
        }
    }
    /// A required parameter after the optional one: Teal refuses `?` there, so the
    /// declaration keeps the plain type.
    pub fn at(&self, scope: Option<String>, n: i64) -> String {
        format!("{}#{n}", scope.unwrap_or_else(|| "-".into()))
    }
}

fn eval(src: &str) -> String {
    let h = Htl::new().unwrap();
    Api.htl_preload(&h).unwrap();
    h.lua().load(src).eval().unwrap()
}

#[test]
fn an_option_parameter_is_declared_with_a_question_mark() {
    assert!(
        Api::DECL.contains("find: function(self: api, name: string, scope?: string): string"),
        "{}",
        Api::DECL
    );
    assert!(
        Api::DECL.contains("at: function(self: api, scope: string, n: integer): string"),
        "{}",
        Api::DECL
    );
}

#[test]
fn the_argument_may_be_left_out() {
    assert_eq!(
        eval("local api = require('api'); return api:find('x')"),
        "-/x"
    );
}

#[test]
fn nil_reaches_the_host_as_none() {
    assert_eq!(
        eval("local api = require('api'); return api:find('x', nil)"),
        "-/x"
    );
}

#[test]
fn a_value_reaches_the_host_as_some() {
    assert_eq!(
        eval("local api = require('api'); return api:find('x', 'y')"),
        "y/x"
    );
}

#[test]
fn a_required_parameter_after_it_is_still_passed_positionally() {
    assert_eq!(
        eval("local api = require('api'); return api:at(nil, 3)"),
        "-#3"
    );
    assert_eq!(
        eval("local api = require('api'); return api:at('s', 3)"),
        "s#3"
    );
}
