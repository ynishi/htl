//! `Htl::strict_strings`: arithmetic on a string is an error, not a conversion.
//!
//! Lua 5.4 keeps the string-to-number coercion for arithmetic in the string metatable, so
//! `"10" + 1` is `11` until those metamethods are gone. The checker never lets checked
//! Teal write that expression, on a `string` or on an `any`; it happens past a cast
//! (`(v as integer) + 1` on a `"10"` from `std.json.decode` or `arg`), in a function
//! `load` built, in Lua a host handed to `exec` — what the checker did not see — and this
//! is the guard for that edge (#247). What Lua does elsewhere (concatenation, comparison,
//! `tonumber`) is deliberately left alone, and the tests here say so as much as they say
//! what is stopped.

use htl_core::Htl;
use std::path::Path;

mod common;

/// What a chunk returns, or the error text: the two things the tests compare.
fn eval(h: &Htl, src: &str) -> Result<String, String> {
    h.lua()
        .load(src)
        .eval::<htl_core::mlua::Value>()
        .map(|v| format!("{v:?}"))
        .map_err(|e| e.to_string())
}

/// Before the call Lua converts; after it the same expression is the error Lua raises for
/// arithmetic on a value that has no metamethod for it, and the message names the
/// operand. All eight operators, since each is its own metamethod and a guard that drops
/// seven of them is not a guard.
#[test]
fn arithmetic_on_a_string_is_an_error_once_strict() {
    let h = Htl::new().unwrap();
    assert_eq!(eval(&h, r#"return "10" + 1"#).unwrap(), "Integer(11)");

    h.strict_strings().unwrap();
    for expr in [
        r#""10" + 1"#,
        r#""10" - 1"#,
        r#""10" * 2"#,
        r#""10" / 2"#,
        r#""10" % 3"#,
        r#""10" ^ 2"#,
        r#"-"10""#,
        r#""10" // 3"#,
        r#"1 + "10""#,
    ] {
        let err = eval(&h, &format!("return {expr}")).unwrap_err();
        assert!(
            err.contains("attempt to perform arithmetic on a string value"),
            "{expr}: {err}"
        );
    }
}

/// The three conversions Lua does outside the string metatable are not this guard's, and
/// a reader of its doc should find them still working: concatenation of a number, the
/// comparison that was never a conversion, and the two functions that convert on request.
/// String methods go through `__index`, which stays.
#[test]
fn what_lua_does_elsewhere_is_left_alone() {
    let h = Htl::new().unwrap();
    h.strict_strings().unwrap();
    assert_eq!(eval(&h, r#"return 10 .. """#).unwrap(), r#"String("10")"#);
    assert_eq!(eval(&h, r#"return "10" < "9""#).unwrap(), "Boolean(true)");
    assert_eq!(eval(&h, r#"return tonumber("10")"#).unwrap(), "Integer(10)");
    assert_eq!(
        eval(&h, r#"return math.tointeger("10")"#).unwrap(),
        "Integer(10)"
    );
    assert_eq!(
        eval(&h, r#"return ("x"):upper()"#).unwrap(),
        r#"String("X")"#
    );
    assert_eq!(eval(&h, r#"return #"abc""#).unwrap(), "Integer(3)");
}

/// A host's `preload` may be run more than once on a state (`replace_bundle` reloads),
/// so a second call has to be a no-op rather than an error over a metamethod already gone.
#[test]
fn a_second_call_is_a_no_op() {
    let h = Htl::new().unwrap();
    h.strict_strings().unwrap();
    h.strict_strings().unwrap();
    let err = eval(&h, r#"return "1" + 1"#).unwrap_err();
    assert!(err.contains("arithmetic on a string value"), "{err}");
}

/// The default state holds the checker as well as the program, so the guard is on the
/// checker too. `tl` has to keep working under it: a check of a file that adds numbers
/// and concatenates strings — everything the guard could have broken — still passes, and
/// the file's own `"10" + 1` is still the checker's error, not a run-time one.
#[test]
fn the_checker_in_the_same_state_still_checks() {
    let dir = common::scratch("htl-core-strict-strings", "checker");
    let ok = dir.join("ok.tl");
    std::fs::write(
        &ok,
        "local n: integer = 1 + 2\nlocal s: string = \"a\" .. 3\nlocal t: {string:integer} = { [\"1\"] = n }\nprint(s, t[\"1\"])\n",
    )
    .unwrap();
    let bad = dir.join("bad.tl");
    std::fs::write(&bad, "local n = \"10\" + 1\nprint(n)\n").unwrap();

    let h = Htl::new().unwrap();
    h.strict_strings().unwrap();
    h.add_path(&dir).unwrap();
    let ci = h.check(Path::new(&ok)).unwrap();
    assert!(ci.errors.is_empty(), "{:?}", ci.errors);
    let ci = h.check(Path::new(&bad)).unwrap();
    assert_eq!(ci.errors.len(), 1, "{:?}", ci.errors);
    assert!(
        ci.errors[0].contains("cannot use operator '+'"),
        "{}",
        ci.errors[0]
    );
}
