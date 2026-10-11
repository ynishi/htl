//! `#[derive(TealRecord)]` beside serde's derives, for the words that decide whether and how
//! a field is there — `skip`, `default`, `skip_serializing_if` with a default, `transparent`
//! and `flatten`: the `.d.tl` declares what `serde_json` writes, the Lua table carries the
//! keys of `serde_json::to_value(x)`, and a key left nil reads as serde reads it missing.

use std::collections::BTreeSet;

use htl::mlua::{self, FromLua, IntoLua, Value};
use htl::teal::TealRecord as _;
use htl::{Htl, TealRecord};
use serde::{Deserialize, Serialize};

/// Flattened into `Call`: its fields join `Call`'s under its own names (`rename`, its own
/// `rename_all`), not `Call`'s rule.
#[derive(Serialize, Deserialize, TealRecord, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Meta {
    #[serde(rename = "trace")]
    trace_id: String,
    span_count: u32,
}

/// Crosses as its one field that is not skipped: `type Label = string`.
#[derive(Serialize, Deserialize, TealRecord, Debug, Clone, PartialEq)]
#[serde(transparent)]
struct Label {
    text: String,
    #[serde(skip)]
    shown: u32,
}

/// All five words at once, under a `rename_all`.
#[derive(Serialize, Deserialize, TealRecord, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Call {
    call_id: String,
    #[serde(skip)]
    secret: String,
    #[serde(default)]
    retry_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(flatten)]
    meta: Meta,
    label: Label,
}

/// The struct's own `default`: a nil field takes the same field of `Opts::default()`, and a
/// field's `default = "path"` beats it.
#[derive(Serialize, Deserialize, TealRecord, Debug, Clone, PartialEq)]
#[serde(default)]
struct Opts {
    depth: u32,
    #[serde(default = "five")]
    width: u32,
    name: String,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            depth: 3,
            width: 1,
            name: "plain".into(),
        }
    }
}

fn five() -> u32 {
    5
}

fn call(tags: Vec<String>, note: Option<String>) -> Call {
    Call {
        call_id: "a1".into(),
        secret: "s3cret".into(),
        retry_count: 2,
        tags,
        note,
        meta: Meta {
            trace_id: "t9".into(),
            span_count: 4,
        },
        label: Label {
            text: "first".into(),
            shown: 7,
        },
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

fn keys(ks: &[&str]) -> BTreeSet<String> {
    ks.iter().map(|k| k.to_string()).collect()
}

/// The macro's `DECL`. `htl_core::dts`'s test
/// `htl_dts_nests_the_serde_shaped_record_as_the_derive_declares_it` asserts the same text
/// for `Call` and `Label` on `htl dts`'s road (the scan of the same source), and
/// `crates/htl-cli/tests/dts_derive.rs`'s
/// `dts_writes_a_serde_shaped_record_as_the_derive_declares_it` through the CLI: the three
/// hold one text, so the `.d.tl` a build writes and the one `htl dts` writes are the same.
#[test]
fn the_declaration_leaves_out_the_skipped_field_and_lists_the_flattened_ones() {
    assert_eq!(
        Call::DECL,
        "local record Call\n   callId: string\n   retryCount: integer\n   tags: {string}\n   \
         note: string\n   trace: string\n   spanCount: integer\n   label: Label\nend\n\n\
         return Call\n"
    );
    assert_eq!(Label::DECL, "local type Label = string\n\nreturn Label\n");
    assert_eq!(
        Opts::DECL,
        "local record Opts\n   depth: integer\n   width: integer\n   name: string\nend\n\nreturn Opts\n"
    );
}

#[test]
fn the_lua_table_carries_the_keys_serde_json_writes_whichever_way_the_predicate_goes() {
    let h = Htl::new().unwrap();
    // The predicates hold: serde leaves `tags` and `note` out, and so does `into_lua`.
    let bare = call(vec![], None);
    let json = json_keys(&bare);
    assert_eq!(
        json,
        keys(&["callId", "retryCount", "trace", "spanCount", "label"])
    );
    assert_eq!(lua_keys(&h, bare.clone()), json);
    // They do not: both keys are written.
    let full = call(vec!["x".into()], Some("n".into()));
    let json = json_keys(&full);
    assert_eq!(
        json,
        keys(&[
            "callId",
            "retryCount",
            "tags",
            "note",
            "trace",
            "spanCount",
            "label"
        ])
    );
    assert_eq!(lua_keys(&h, full.clone()), json);
}

#[test]
fn a_round_trip_gives_the_value_back_with_the_skipped_fields_defaulted() {
    let h = Htl::new().unwrap();
    for c in [call(vec![], None), call(vec!["x".into()], Some("n".into()))] {
        let back = Call::from_lua(c.clone().into_lua(h.lua()).unwrap(), h.lua()).unwrap();
        // What serde gives back from its own JSON is what Lua gives back from the table.
        let via_json: Call = serde_json::from_value(serde_json::to_value(&c).unwrap()).unwrap();
        assert_eq!(back, via_json);
        let mut want = c.clone();
        want.secret = String::new();
        want.label.shown = 0;
        assert_eq!(back, want);
    }
}

#[test]
fn the_skipped_field_is_not_in_the_table_and_the_flattened_ones_are_at_its_top() {
    let h = Htl::new().unwrap();
    let Value::Table(t) = call(vec![], None).into_lua(h.lua()).unwrap() else {
        panic!("not a table")
    };
    assert!(t.get::<Value>("secret").unwrap().is_nil());
    assert!(t.get::<Value>("meta").unwrap().is_nil());
    assert_eq!(t.get::<String>("trace").unwrap(), "t9");
    assert_eq!(t.get::<u32>("spanCount").unwrap(), 4);
    assert_eq!(t.get::<String>("label").unwrap(), "first");
}

#[test]
fn nil_for_a_default_field_reads_as_the_rust_default() {
    let h = Htl::new().unwrap();
    let src = r#"{ callId = "a1", trace = "t9", spanCount = 4, label = "first" }"#;
    let c: Call = convert(&h, src).unwrap();
    assert_eq!(c.retry_count, 0);
    assert_eq!(c.tags, Vec::<String>::new());
    assert_eq!(c.note, None);
    let json: Call = serde_json::from_str(
        r#"{ "callId": "a1", "trace": "t9", "spanCount": 4, "label": "first" }"#,
    )
    .unwrap();
    assert_eq!(c, json);
    // The struct's `default` fills each nil field from `Opts::default()`; the field's own
    // `default = "five"` wins over it — as serde reads `{}`.
    let o: Opts = convert(&h, "{}").unwrap();
    assert_eq!(
        o,
        Opts {
            depth: 3,
            width: 5,
            name: "plain".into()
        }
    );
    assert_eq!(o, serde_json::from_str::<Opts>("{}").unwrap());
    let o: Opts = convert(&h, r#"{ depth = 9, name = "n" }"#).unwrap();
    assert_eq!((o.depth, o.width, o.name.as_str()), (9, 5, "n"));
}

#[test]
fn a_wrong_flattened_field_is_reported_under_the_outer_record() {
    let h = Htl::new().unwrap();
    let e = convert::<Call>(
        &h,
        r#"{ callId = "a1", trace = {}, spanCount = 4, label = "first" }"#,
    )
    .unwrap_err();
    assert_eq!(e.to_string(), "Call.trace: expected string, got table");
    let e = convert::<Call>(&h, r#"{ callId = "a1", trace = "t9", label = "first" }"#).unwrap_err();
    assert_eq!(e.to_string(), "Call.spanCount: expected integer, got nil");
    // The inner struct on its own still reports under its own name.
    let e = convert::<Meta>(&h, r#"{ trace = "t9" }"#).unwrap_err();
    assert_eq!(e.to_string(), "Meta.spanCount: expected integer, got nil");
}

#[test]
fn a_transparent_struct_crosses_as_its_field() {
    let h = Htl::new().unwrap();
    let v = Label {
        text: "hi".into(),
        shown: 3,
    }
    .into_lua(h.lua())
    .unwrap();
    assert_eq!(String::from_lua(v.clone(), h.lua()).unwrap(), "hi");
    assert!(v.is_string());
    let back = Label::from_lua(v, h.lua()).unwrap();
    assert_eq!(
        back,
        Label {
            text: "hi".into(),
            shown: 0
        }
    );
    assert_eq!(
        serde_json::to_value(&back).unwrap(),
        serde_json::json!("hi")
    );
    let e = convert::<Label>(&h, "{}").unwrap_err();
    assert_eq!(e.to_string(), "Label: expected string, got table");
    // As a field: the field's key holds the bare value, and a wrong one names the path.
    let e = convert::<Call>(
        &h,
        r#"{ callId = "a1", trace = "t9", spanCount = 4, label = {} }"#,
    )
    .unwrap_err();
    assert_eq!(e.to_string(), "Call.label: expected string, got table");
}
