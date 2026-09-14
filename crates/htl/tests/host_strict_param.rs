//! A `#[host_module]` method taking a `Strict<T>`: declared as `T` on the Teal side, and
//! given only the Lua kind `T` names at run time — mlua's own `FromLua` for `i64` would
//! read `"10"` as `10`, and for `String` would read `10` as `"10"` (#247).

use htl::teal::HostModule as _;
use htl::{Htl, Strict, host_module};

pub struct Api;

#[host_module(name = "api")]
impl Api {
    /// The comparison: mlua converts for a plain `i64`.
    pub fn loose(&self, n: i64) -> i64 {
        n
    }
    pub fn take(&self, n: Strict<i64>) -> i64 {
        *n
    }
    pub fn name(&self, s: Strict<String>) -> String {
        s.into_inner()
    }
    pub fn flag(&self, b: Strict<bool>) -> bool {
        *b
    }
    pub fn ratio(&self, x: Strict<f64>) -> f64 {
        *x
    }
}

fn eval(src: &str) -> Result<String, String> {
    let h = Htl::new().unwrap();
    Api.htl_preload(&h).unwrap();
    h.lua()
        .load(src)
        .eval::<htl::mlua::Value>()
        .map(|v| format!("{v:?}"))
        .map_err(|e| e.to_string())
}

/// The `.d.tl` line is the inner type's: `Strict<i64>` declares `integer` exactly as `i64`
/// does, because the declaration already says what the wrapper holds the runtime to.
#[test]
fn a_strict_parameter_is_declared_as_its_inner_type() {
    for line in [
        "loose: function(self: api, n: integer): integer",
        "take: function(self: api, n: integer): integer",
        "name: function(self: api, s: string): string",
        "flag: function(self: api, b: boolean): boolean",
        "ratio: function(self: api, x: number): number",
    ] {
        assert!(Api::DECL.contains(line), "{line}\n{}", Api::DECL);
    }
}

/// The kind the declaration names goes through, and — for an integer — so does a float
/// with no fraction, as `math.tointeger` would read it.
#[test]
fn the_declared_kind_is_taken() {
    assert_eq!(
        eval("return require('api'):take(10)").unwrap(),
        "Integer(10)"
    );
    assert_eq!(
        eval("return require('api'):take(10.0)").unwrap(),
        "Integer(10)"
    );
    assert_eq!(
        eval("return require('api'):name('ten')").unwrap(),
        r#"String("ten")"#
    );
    assert_eq!(
        eval("return require('api'):flag(true)").unwrap(),
        "Boolean(true)"
    );
    assert_eq!(eval("return require('api'):ratio(3)").unwrap(), "Number(3)");
}

/// The same call on the plain `i64` parameter converts, which is the behaviour the
/// wrapper exists to refuse: `"10"` is an error that names the kind that arrived and the
/// kind the declaration wanted, in mlua's own words.
#[test]
fn the_other_kind_is_refused_where_mlua_would_convert() {
    assert_eq!(
        eval("return require('api'):loose('10')").unwrap(),
        "Integer(10)"
    );
    let err = eval("return require('api'):take('10')").unwrap_err();
    assert!(
        err.contains("error converting Lua string to integer"),
        "{err}"
    );
    assert!(err.contains("converts nothing"), "{err}");

    let err = eval("return require('api'):name(10)").unwrap_err();
    assert!(
        err.contains("error converting Lua number to string")
            || err.contains("error converting Lua integer to string"),
        "{err}"
    );
    let err = eval("return require('api'):flag(1)").unwrap_err();
    assert!(err.contains("to boolean"), "{err}");
    let err = eval("return require('api'):take(10.5)").unwrap_err();
    assert!(
        err.contains("error converting Lua number to integer"),
        "{err}"
    );
    let err = eval("return require('api'):take(nil)").unwrap_err();
    assert!(err.contains("error converting Lua nil to integer"), "{err}");
}
