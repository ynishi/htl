//! `#[derive(TealRecord)]` beside serde's derives on a data enum, for serde's three tagged
//! representations — `tag`, `tag` + `content`, `untagged`: the `.d.tl` is a union told
//! apart the way serde tells the variants apart, and the Lua value of every variant carries
//! the keys (or, untagged, the Lua type) of what `serde_json::to_value` writes for it.

use std::collections::BTreeSet;

use htl::mlua::{self, FromLua, IntoLua, Value};
use htl::teal::TealRecord as _;
use htl::{Htl, TealRecord};
use serde::{Deserialize, Serialize};

/// The payload of `Internal::Moved`: under `tag` alone serde writes its fields beside the
/// tag, under its own names.
#[derive(Serialize, Deserialize, TealRecord, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Move {
    from_x: f64,
    to_x: f64,
}

/// Internally tagged: `{"type": "Moved", "fromX": .., "toX": ..}`.
#[derive(Serialize, Deserialize, TealRecord, Debug, Clone, PartialEq)]
#[serde(tag = "type")]
enum Internal {
    Idle,
    Moved(Move),
    Resized { w: f64, h: f64 },
}

/// Adjacently tagged: `{"t": "Circle", "c": 1.5}`, `{"t": "Rect", "c": {"w": .., "h": ..}}`.
#[derive(Serialize, Deserialize, TealRecord, Debug, Clone, PartialEq)]
#[serde(tag = "t", content = "c")]
enum Adjacent {
    Dot,
    Circle(f64),
    Rect { w: f64, h: f64 },
}

/// Untagged: the payload alone, told apart by its Lua type.
#[derive(Serialize, Deserialize, TealRecord, Debug, Clone, PartialEq)]
#[serde(untagged)]
enum Untagged {
    Name(String),
    Size(f64),
    Rect { w: f64, h: f64 },
}

fn convert<T: FromLua>(h: &Htl, expr: &str) -> mlua::Result<T> {
    let v: Value = h.lua().load(expr).eval()?;
    T::from_lua(v, h.lua())
}

fn round_trip<T: IntoLua + FromLua>(h: &Htl, v: T) -> T {
    let lv = v.into_lua(h.lua()).unwrap();
    T::from_lua(lv, h.lua()).unwrap()
}

/// The keys of a JSON object.
fn json_keys(v: &serde_json::Value) -> BTreeSet<String> {
    match v {
        serde_json::Value::Object(m) => m.keys().cloned().collect(),
        other => panic!("not an object: {other}"),
    }
}

/// The string keys of a Lua table.
fn lua_keys(v: &Value) -> BTreeSet<String> {
    let Value::Table(t) = v else {
        panic!("not a table: {v:?}")
    };
    t.pairs::<String, Value>().map(|kv| kv.unwrap().0).collect()
}

/// The keys serde writes for `x` equal the keys of the table `into_lua` makes, at the top
/// and, where the JSON nests an object under `nested`, one level down; the tag's value
/// is the same word on both sides.
fn same_keys<T: Serialize + IntoLua + Clone>(
    h: &Htl,
    x: &T,
    tag: &str,
    nested: Option<&str>,
) -> BTreeSet<String> {
    let json = serde_json::to_value(x).unwrap();
    let lua = x.clone().into_lua(h.lua()).unwrap();
    let keys = json_keys(&json);
    assert_eq!(lua_keys(&lua), keys);
    let Value::Table(t) = &lua else {
        unreachable!()
    };
    assert_eq!(
        t.get::<String>(tag).unwrap(),
        json[tag].as_str().unwrap(),
        "the tag's word"
    );
    if let Some(n) = nested {
        assert_eq!(lua_keys(&t.get::<Value>(n).unwrap()), json_keys(&json[n]));
    }
    keys
}

fn keys(ks: &[&str]) -> BTreeSet<String> {
    ks.iter().map(|k| k.to_string()).collect()
}

/// The macro's `DECL`; `htl_core::dts`'s test
/// `htl_dts_nests_the_serde_tagged_enums_as_the_derive_declares_them` asserts the same
/// records on `htl dts`'s road.
#[test]
fn the_declarations_test_the_tag_serde_writes() {
    assert_eq!(
        Internal::DECL,
        "local record Internal_Idle\n   where self.type == \"Idle\"\n   type: string\nend\n\
         local record Internal_Moved\n   where self.type == \"Moved\"\n   type: string\n   \
         fromX: number\n   toX: number\nend\n\
         local record Internal_Resized\n   where self.type == \"Resized\"\n   type: string\n   \
         w: number\n   h: number\nend\n\
         local type Internal = Internal_Idle | Internal_Moved | Internal_Resized\n\n\
         return Internal\n"
    );
    assert_eq!(
        Adjacent::DECL,
        "local record Adjacent_Dot\n   where self.t == \"Dot\"\n   t: string\nend\n\
         local record Adjacent_Circle\n   where self.t == \"Circle\"\n   t: string\n   \
         c: number\nend\n\
         local record Adjacent_Rect_Content\n   w: number\n   h: number\nend\n\
         local record Adjacent_Rect\n   where self.t == \"Rect\"\n   t: string\n   \
         c: Adjacent_Rect_Content\nend\n\
         local type Adjacent = Adjacent_Dot | Adjacent_Circle | Adjacent_Rect\n\n\
         return Adjacent\n"
    );
    assert_eq!(
        Untagged::DECL,
        "local record Untagged_Rect\n   w: number\n   h: number\nend\n\
         local type Untagged = string | number | Untagged_Rect\n\n\
         return Untagged\n"
    );
}

#[test]
fn an_internally_tagged_variant_carries_serde_jsons_keys() {
    let h = Htl::new().unwrap();
    let idle = Internal::Idle;
    let moved = Internal::Moved(Move {
        from_x: 1.0,
        to_x: 2.0,
    });
    let resized = Internal::Resized { w: 3.0, h: 4.0 };
    assert_eq!(same_keys(&h, &idle, "type", None), keys(&["type"]));
    assert_eq!(
        same_keys(&h, &moved, "type", None),
        keys(&["type", "fromX", "toX"])
    );
    assert_eq!(
        same_keys(&h, &resized, "type", None),
        keys(&["type", "w", "h"])
    );
    for v in [idle, moved, resized] {
        assert_eq!(round_trip(&h, v.clone()), v);
    }
}

#[test]
fn an_adjacently_tagged_variant_carries_serde_jsons_keys() {
    let h = Htl::new().unwrap();
    let dot = Adjacent::Dot;
    let circle = Adjacent::Circle(1.5);
    let rect = Adjacent::Rect { w: 3.0, h: 4.0 };
    assert_eq!(same_keys(&h, &dot, "t", None), keys(&["t"]));
    assert_eq!(same_keys(&h, &circle, "t", None), keys(&["t", "c"]));
    assert_eq!(same_keys(&h, &rect, "t", Some("c")), keys(&["t", "c"]));
    for v in [dot, circle, rect] {
        assert_eq!(round_trip(&h, v.clone()), v);
    }
}

#[test]
fn an_untagged_variant_is_its_payload_as_serde_json_writes_it() {
    let h = Htl::new().unwrap();
    let name = Untagged::Name("n".into());
    let size = Untagged::Size(2.5);
    let rect = Untagged::Rect { w: 3.0, h: 4.0 };
    assert!(serde_json::to_value(&name).unwrap().is_string());
    assert!(matches!(
        name.clone().into_lua(h.lua()).unwrap(),
        Value::String(_)
    ));
    assert!(serde_json::to_value(&size).unwrap().is_number());
    assert!(matches!(
        size.clone().into_lua(h.lua()).unwrap(),
        Value::Number(_)
    ));
    let json = serde_json::to_value(&rect).unwrap();
    assert_eq!(
        lua_keys(&rect.clone().into_lua(h.lua()).unwrap()),
        json_keys(&json)
    );
    assert_eq!(json_keys(&json), keys(&["w", "h"]));
    for v in [name, size, rect] {
        assert_eq!(round_trip(&h, v.clone()), v);
    }
    // An integer is a number too, as serde reads `1` for an `f64`.
    assert_eq!(
        convert::<Untagged>(&h, "return 1").unwrap(),
        Untagged::Size(1.0)
    );
}

#[test]
fn a_wrong_or_unknown_tag_lists_the_variants() {
    let h = Htl::new().unwrap();
    let e = convert::<Internal>(&h, "return { type = 'Jumped' }").unwrap_err();
    assert_eq!(
        e.to_string(),
        "Internal: expected one of \"Idle\", \"Moved\", \"Resized\", got \"Jumped\""
    );
    // htl's `kind` is not the tag here.
    let e = convert::<Internal>(&h, "return { kind = 'Idle' }").unwrap_err();
    assert_eq!(
        e.to_string(),
        "Internal: expected one of \"Idle\", \"Moved\", \"Resized\", got nil"
    );
    let e = convert::<Adjacent>(&h, "return { t = 1 }").unwrap_err();
    assert_eq!(
        e.to_string(),
        "Adjacent: expected one of \"Dot\", \"Circle\", \"Rect\", got integer"
    );
    let e = convert::<Untagged>(&h, "return true").unwrap_err();
    assert_eq!(
        e.to_string(),
        "Untagged: expected one of string, number, Untagged_Rect, got boolean"
    );
}

#[test]
fn a_payload_that_does_not_fit_is_named_by_path() {
    let h = Htl::new().unwrap();
    let e = convert::<Internal>(&h, "return { type = 'Moved', fromX = 1 }").unwrap_err();
    assert_eq!(
        e.to_string(),
        "Internal.Moved.toX: expected number, got nil"
    );
    let e = convert::<Adjacent>(&h, "return { t = 'Circle' }").unwrap_err();
    assert_eq!(e.to_string(), "Adjacent.Circle.c: expected number, got nil");
    let e = convert::<Adjacent>(&h, "return { t = 'Rect', c = 1 }").unwrap_err();
    assert_eq!(
        e.to_string(),
        "Adjacent.Rect.c: expected Adjacent_Rect_Content, got integer"
    );
    let e = convert::<Adjacent>(&h, "return { t = 'Rect', c = { w = 1 } }").unwrap_err();
    assert_eq!(e.to_string(), "Adjacent.Rect.c.h: expected number, got nil");
    let e = convert::<Untagged>(&h, "return { w = 1 }").unwrap_err();
    assert_eq!(e.to_string(), "Untagged.Rect.h: expected number, got nil");
}
