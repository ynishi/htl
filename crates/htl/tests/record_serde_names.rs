//! `#[derive(TealRecord)]` beside serde's derives: the names `#[serde(..)]` gives the fields
//! and the variants are the ones the `.d.tl` declares and the Lua table carries, so the
//! keys of `serde_json::to_value(x)` and of `x.into_lua()` are the same set.

use std::collections::BTreeSet;

use htl::mlua::{self, FromLua, IntoLua, Value};
use htl::teal::TealRecord as _;
use htl::{Htl, TealRecord};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, TealRecord, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Call {
    #[serde(rename = "callId")]
    call_id: String,
    retry_count: u32,
    #[serde(alias = "msg")]
    message: String,
}

#[derive(Serialize, Deserialize, TealRecord, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
enum State {
    Open,
    InReview,
}

/// Both rules on one enum: `#[teal(rename_all)]` wins over `#[serde(rename_all)]`. This is
/// the precedence rule — `teal` over `serde` at each level, as `#[teal(name)]` over
/// `#[serde(rename)]` on a variant — and the Teal side then crosses as `INREVIEW` (serde's
/// `UPPERCASE` upper-cases the variant name whole; `SCREAMING_SNAKE_CASE` would give
/// `IN_REVIEW`) while serde writes `in_review`, which is what the host asked for by
/// writing both.
#[derive(Serialize, Deserialize, TealRecord, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
#[teal(rename_all = "UPPERCASE")]
enum Loud {
    Open,
    InReview,
}

/// A name that is not a Teal identifier: declared with Teal's `["..."]` key syntax.
#[derive(Serialize, Deserialize, TealRecord, Debug, Clone, PartialEq)]
#[serde(rename_all = "kebab-case")]
struct Kebab {
    call_id: String,
    end: u32,
}

/// `rename_all_fields` on an enum renames the fields of its struct variants. `tag = "kind"`
/// makes serde write the `kind` form htl declares (without it serde writes the enum
/// externally tagged, and the derive refuses it).
#[derive(Serialize, Deserialize, TealRecord, Debug, Clone, PartialEq)]
#[serde(tag = "kind", rename_all_fields = "camelCase")]
enum Ev {
    Hit { call_id: String },
    Stop,
}

/// A word that does not change the shape is passed through to serde.
#[derive(Serialize, Deserialize, TealRecord, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Strict {
    call_id: String,
}

fn call() -> Call {
    Call {
        call_id: "a1".into(),
        retry_count: 2,
        message: "hi".into(),
    }
}

fn convert<T: FromLua>(h: &Htl, expr: &str) -> mlua::Result<T> {
    let v: Value = h.lua().load(expr).eval()?;
    T::from_lua(v, h.lua())
}

/// The keys of the JSON object serde writes for `x`.
fn json_keys<T: Serialize>(x: &T) -> BTreeSet<String> {
    match serde_json::to_value(x).unwrap() {
        serde_json::Value::Object(m) => m.keys().cloned().collect(),
        other => panic!("not an object: {other}"),
    }
}

/// The string keys of the Lua table `x.into_lua()` makes.
fn lua_keys<T: IntoLua>(h: &Htl, x: T) -> BTreeSet<String> {
    let Value::Table(t) = x.into_lua(h.lua()).unwrap() else {
        panic!("not a table")
    };
    t.pairs::<String, Value>().map(|kv| kv.unwrap().0).collect()
}

#[test]
fn the_fields_are_declared_as_serde_names_them() {
    assert_eq!(
        Call::DECL,
        "local record Call\n   callId: string\n   retryCount: integer\n   message: string\nend\n\nreturn Call\n"
    );
}

#[test]
fn the_lua_table_carries_the_keys_serde_json_writes() {
    let h = Htl::new().unwrap();
    let c = call();
    let json = json_keys(&c);
    assert_eq!(
        json,
        BTreeSet::from(["callId".into(), "retryCount".into(), "message".into()])
    );
    assert_eq!(lua_keys(&h, c.clone()), json);
    let back = Call::from_lua(c.clone().into_lua(h.lua()).unwrap(), h.lua()).unwrap();
    assert_eq!(back, c);
}

#[test]
fn an_alias_is_accepted_on_the_way_in() {
    let h = Htl::new().unwrap();
    let c: Call = convert(&h, r#"{ callId = "a1", retryCount = 2, msg = "hi" }"#).unwrap();
    assert_eq!(c, call());
    // serde reads the same table's JSON the same way.
    let s: Call =
        serde_json::from_str(r#"{ "callId": "a1", "retryCount": 2, "msg": "hi" }"#).unwrap();
    assert_eq!(s, call());
}

#[test]
fn the_rust_name_is_not_a_key() {
    let h = Htl::new().unwrap();
    let e = convert::<Call>(&h, r#"{ call_id = "a1", retryCount = 2, message = "hi" }"#)
        .unwrap_err()
        .to_string();
    assert_eq!(e, "Call.callId: expected string, got nil");
}

#[test]
fn every_rename_rule_names_a_field_as_serde_does() {
    macro_rules! rule {
        ($rule:literal, $ty:ident) => {{
            #[derive(Serialize, TealRecord)]
            #[serde(rename_all = $rule)]
            struct $ty {
                call_id: u8,
                x: u8,
            }
            let h = Htl::new().unwrap();
            let v = $ty { call_id: 1, x: 2 };
            let json = json_keys(&v);
            assert_eq!(lua_keys(&h, v), json, "{}", $rule);
            json.into_iter().collect::<Vec<_>>().join(" ")
        }};
    }
    let got = vec![
        rule!("lowercase", R1),
        rule!("UPPERCASE", R2),
        rule!("PascalCase", R3),
        rule!("camelCase", R4),
        rule!("snake_case", R5),
        rule!("SCREAMING_SNAKE_CASE", R6),
        rule!("kebab-case", R7),
        rule!("SCREAMING-KEBAB-CASE", R8),
    ];
    assert_eq!(
        got,
        vec![
            "call_id x",
            "CALL_ID X",
            "CallId X",
            "callId x",
            "call_id x",
            "CALL_ID X",
            "call-id x",
            "CALL-ID X",
        ]
    );
}

#[test]
fn a_name_that_is_not_a_teal_identifier_is_a_bracketed_key() {
    assert_eq!(
        Kebab::DECL,
        "local record Kebab\n   [\"call-id\"]: string\n   [\"end\"]: integer\nend\n\nreturn Kebab\n"
    );
    let h = Htl::new().unwrap();
    let k = Kebab {
        call_id: "a".into(),
        end: 3,
    };
    assert_eq!(lua_keys(&h, k.clone()), json_keys(&k));
    let back = Kebab::from_lua(k.clone().into_lua(h.lua()).unwrap(), h.lua()).unwrap();
    assert_eq!(back, k);
}

#[test]
fn an_enum_crosses_as_the_word_serde_writes() {
    assert_eq!(
        State::DECL,
        "local enum State\n   \"open\"\n   \"in_review\"\nend\n\nreturn State\n"
    );
    let h = Htl::new().unwrap();
    assert_eq!(
        serde_json::to_value(State::InReview).unwrap(),
        serde_json::json!("in_review")
    );
    let v = State::InReview.into_lua(h.lua()).unwrap();
    assert_eq!(v.to_string().unwrap(), "in_review");
    assert_eq!(
        convert::<State>(&h, r#""in_review""#).unwrap(),
        State::InReview
    );
    let e = convert::<State>(&h, r#""InReview""#)
        .unwrap_err()
        .to_string();
    assert_eq!(
        e,
        "State: expected one of \"open\", \"in_review\", got \"InReview\""
    );
}

#[test]
fn teal_rename_all_wins_over_serde_rename_all() {
    // The precedence rule: `#[teal(rename_all)]` beats `#[serde(rename_all)]`.
    assert_eq!(
        Loud::DECL,
        "local enum Loud\n   \"OPEN\"\n   \"INREVIEW\"\nend\n\nreturn Loud\n"
    );
    let h = Htl::new().unwrap();
    let v = Loud::InReview.into_lua(h.lua()).unwrap();
    assert_eq!(v.to_string().unwrap(), "INREVIEW");
    assert_eq!(
        convert::<Loud>(&h, r#""INREVIEW""#).unwrap(),
        Loud::InReview
    );
}

#[test]
fn rename_all_fields_names_a_struct_variant_s_fields() {
    assert!(
        Ev::DECL.contains("local record Ev_Hit\n   where self.kind == \"Hit\"\n   kind: string\n   callId: string\nend\n"),
        "{}",
        Ev::DECL
    );
    let h = Htl::new().unwrap();
    let e = Ev::Hit {
        call_id: "a".into(),
    };
    let back = Ev::from_lua(e.clone().into_lua(h.lua()).unwrap(), h.lua()).unwrap();
    assert_eq!(back, e);
    let err = convert::<Ev>(&h, r#"{ kind = "Hit", call_id = "a" }"#)
        .unwrap_err()
        .to_string();
    assert_eq!(err, "Ev.Hit.callId: expected string, got nil");
}

#[test]
fn a_word_that_keeps_the_shape_passes_through() {
    assert_eq!(
        Strict::DECL,
        "local record Strict\n   callId: string\nend\n\nreturn Strict\n"
    );
    let h = Htl::new().unwrap();
    let s = Strict {
        call_id: "a".into(),
    };
    assert_eq!(lua_keys(&h, s.clone()), json_keys(&s));
}
