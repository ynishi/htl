//! `#[teal(serde)]` on a `#[derive(TealRecord)]` field: a type with `Serialize` /
//! `Deserialize` and no `IntoLua` / `FromLua` crosses through mlua's serde with the options
//! htl fixes — `None` is `nil`, a sequence carries the array metatable, an empty sequence
//! comes back empty, a mixed table is refused at the field — and `#[teal(skip)]` leaves a
//! field out of the declaration and the table and reads it as its `Default`.
#![cfg(feature = "serde")]

use htl::mlua::{self, FromLua, IntoLua, LuaSerdeExt, Value};
use htl::teal::TealRecord as _;
use htl::{Htl, TealRecord};
use serde::{Deserialize, Serialize};

/// A host type with serde and no `IntoLua` / `FromLua`: serde writes it as its decimal
/// string.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct Stamp(pub u64);

impl From<Stamp> for String {
    fn from(s: Stamp) -> String {
        s.0.to_string()
    }
}

impl TryFrom<String> for Stamp {
    type Error = std::num::ParseIntError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse().map(Stamp)
    }
}

/// A serde struct with an `Option` and a sequence inside.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Options {
    pub limit: Option<u32>,
    pub list: Vec<String>,
}

/// `serde(with)`'s module: a `u128` as a decimal string.
mod decimal {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &u128, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u128, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(TealRecord, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rec {
    #[teal(serde, as = "string")]
    pub at: Stamp,
    #[teal(serde, as = "{string}")]
    pub seen: Vec<Stamp>,
    #[teal(serde, as = "{string: any}")]
    pub opts: Options,
    #[teal(serde)]
    pub tags: Vec<String>,
    #[teal(serde, as = "{any}")]
    pub gaps: Vec<Option<u32>>,
    #[teal(serde, as = "string")]
    #[serde(with = "decimal")]
    pub big: u128,
    #[teal(skip)]
    pub cache: Vec<u8>,
}

fn sample() -> Rec {
    Rec {
        at: Stamp(1_700_000_000),
        seen: vec![Stamp(1), Stamp(2)],
        opts: Options {
            limit: None,
            list: vec!["a".into(), "b".into()],
        },
        tags: vec![],
        gaps: vec![Some(1), None, Some(3)],
        big: 340_282_366_920_938_463_463_374_607_431_768_211_455,
        cache: vec![9, 9],
    }
}

#[test]
fn the_declaration_says_as_or_the_mapped_type_and_leaves_a_skipped_field_out() {
    assert_eq!(
        Rec::DECL,
        "local record Rec\n   at: string\n   seen: {string}\n   opts: {string: any}\n   \
         tags: {string}\n   gaps: {any}\n   big: string\nend\n\nreturn Rec\n"
    );
}

#[test]
fn the_fields_cross_as_serde_writes_them_with_htls_options() {
    let h = Htl::new().unwrap();
    let lua = h.lua();
    let t = sample().into_lua(lua).unwrap();
    // mlua's array metatable says `__metatable = false`, so Lua's `getmetatable` answers
    // `false` for it; the metatable itself is read from Rust.
    let tt = t.as_table().unwrap();
    for seq in [
        tt.get::<mlua::Table>("seen").unwrap(),
        tt.get("tags").unwrap(),
        tt.get("gaps").unwrap(),
    ]
    .into_iter()
    .chain([tt.get::<mlua::Table>("opts").unwrap().get("list").unwrap()])
    {
        assert_eq!(seq.metatable(), Some(lua.array_metatable()));
    }
    lua.globals().set("t", t).unwrap();
    let got: String = lua
        .load(
            r#"
            local keys = {}
            for k in pairs(t.opts) do keys[#keys + 1] = k end
            local n = 0
            for _ in ipairs(t.opts.list) do n = n + 1 end
            return table.concat({
                type(t.at), t.at,
                tostring(#t.seen), t.seen[1],
                tostring(t.opts.limit), table.concat(keys, ","),
                tostring(getmetatable(t.opts.list)), tostring(#t.opts.list), tostring(n),
                tostring(#t.tags),
                tostring(#t.gaps), type(t.gaps[2]), tostring(t.gaps[3]),
                t.big, tostring(t.cache),
            }, " ")
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(
        got,
        "string 1700000000 2 1 nil list false 2 2 0 3 userdata 3 \
         340282366920938463463374607431768211455 nil"
    );
}

#[test]
fn a_value_round_trips_and_a_skipped_field_reads_as_its_default() {
    let h = Htl::new().unwrap();
    let v = sample().into_lua(h.lua()).unwrap();
    let back = Rec::from_lua(v, h.lua()).unwrap();
    assert_eq!(
        back,
        Rec {
            cache: vec![],
            ..sample()
        }
    );
}

#[test]
fn an_empty_sequence_comes_back_empty_with_or_without_the_metatable() {
    let h = Htl::new().unwrap();
    let mut r = sample();
    r.opts.list = vec![];
    let v = r.clone().into_lua(h.lua()).unwrap();
    assert_eq!(
        Rec::from_lua(v, h.lua()).unwrap().opts.list,
        Vec::<String>::new()
    );
    // A table Teal wrote, with no metatable: `tags` is a sequence type, so `{}` is one.
    let v: Value = h
        .lua()
        .load(r#"{ at = "5", seen = {}, opts = { list = {} }, tags = {}, gaps = {}, big = "7" }"#)
        .eval()
        .unwrap();
    let back = Rec::from_lua(v, h.lua()).unwrap();
    assert_eq!(back.tags, Vec::<String>::new());
    assert_eq!(back.opts, Options::default());
    assert_eq!((back.at, back.big), (Stamp(5), 7));
}

#[test]
fn a_mixed_table_is_refused_at_the_field() {
    let h = Htl::new().unwrap();
    let read = |src: &str| -> String {
        let v: Value = h.lua().load(src).eval().unwrap();
        Rec::from_lua(v, h.lua()).unwrap_err().to_string()
    };
    assert_eq!(
        read(
            r#"{ at = "5", seen = {}, opts = { list = { "x", "y", a = 3 } }, tags = {}, gaps = {}, big = "7" }"#
        ),
        "Rec.opts: expected {string: any}, got table (a mixed table at `list`: a sequence of \
         2 and the key \"a\"; a Lua table crosses as a sequence or as a map, not both, and not \
         with holes)"
    );
    assert_eq!(
        read(
            r#"{ at = "5", seen = {}, opts = { list = {} }, tags = { "x", k = "y" }, gaps = {}, big = "7" }"#
        ),
        "Rec.tags: expected {string}, got table (a mixed table: a sequence of 1 and the key \
         \"k\"; a Lua table crosses as a sequence or as a map, not both, and not with holes)"
    );
    // What serde itself refuses keeps its words under the same path.
    assert_eq!(
        read(r#"{ at = "x", seen = {}, opts = { list = {} }, tags = {}, gaps = {}, big = "7" }"#),
        "Rec.at: expected string, got string (invalid digit found in string)"
    );
    assert_eq!(
        read(
            r#"{ at = "5", seen = {}, opts = { list = { 1 } }, tags = {}, gaps = {}, big = "7" }"#
        ),
        "Rec.opts: expected {string: any}, got table (invalid type: integer `1`, expected a string)"
    );
}

/// The error is a `FieldError`, so a caller can read the path off it.
#[test]
fn the_refusal_is_a_field_error() {
    let h = Htl::new().unwrap();
    let v: Value = h
        .lua()
        .load(r#"{ at = "5", seen = {}, opts = { list = {} }, tags = {}, gaps = {}, big = {} }"#)
        .eval()
        .unwrap();
    let e = Rec::from_lua(v, h.lua()).unwrap_err();
    let fe = e.downcast_ref::<htl::teal::FieldError>().unwrap();
    assert_eq!(
        (fe.path.as_str(), fe.expected.as_str()),
        ("Rec.big", "string")
    );
}

/// A host type whose `Serialize` refuses: a `#[teal(serde)]` field of it fails on the way
/// out. `Deserialize` is there because the field's reading half is generated beside it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Unwritable(pub u32);

impl Serialize for Unwritable {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("no writer"))
    }
}

/// Flattened into `Call`, then `Call` into `Outer`: the field that fails to write sits two
/// structs below the record the table is declared as.
#[derive(TealRecord, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    #[teal(serde, as = "number")]
    pub opts: Unwritable,
}

#[derive(TealRecord, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Call {
    pub id: String,
    #[serde(flatten)]
    pub meta: Meta,
}

#[derive(TealRecord, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outer {
    #[serde(flatten)]
    pub call: Call,
}

/// A `tag` enum whose newtype variant writes `Meta`'s fields beside the tag.
#[derive(TealRecord, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Ev {
    Idle,
    Moved(Meta),
}

fn meta() -> Meta {
    Meta {
        opts: Unwritable(1),
    }
}

/// Writing names the key where it sits in the table, as reading does
/// (`a_wrong_flattened_field_is_reported_under_the_outer_record` in `record_serde_shape.rs`):
/// the Teal side has no level for the inner struct. The path is the context line, serde's
/// own words follow.
#[test]
fn a_flattened_field_that_will_not_write_is_reported_under_the_outer_record() {
    let h = Htl::new().unwrap();
    let e = Call {
        id: "a1".into(),
        meta: meta(),
    }
    .into_lua(h.lua())
    .unwrap_err();
    assert_eq!(e.to_string(), "Call.opts\nserialize error: no writer");
    // The inner struct on its own still reports under its own name.
    let e = meta().into_lua(h.lua()).unwrap_err();
    assert_eq!(e.to_string(), "Meta.opts\nserialize error: no writer");
}

#[test]
fn the_outermost_record_names_the_field_through_two_levels_of_flatten() {
    let h = Htl::new().unwrap();
    let e = Outer {
        call: Call {
            id: "a1".into(),
            meta: meta(),
        },
    }
    .into_lua(h.lua())
    .unwrap_err();
    assert_eq!(e.to_string(), "Outer.opts\nserialize error: no writer");
}

/// The payload of a `tag` variant is named as `union_from` names it, `Enum.Variant`, so the
/// path to the field is the same whichever way the value crosses.
#[test]
fn a_tag_variants_payload_that_will_not_write_is_reported_under_the_variant() {
    let h = Htl::new().unwrap();
    let e = Ev::Moved(meta()).into_lua(h.lua()).unwrap_err();
    assert_eq!(e.to_string(), "Ev.Moved.opts\nserialize error: no writer");
    // A variant that carries no such field still writes.
    assert!(Ev::Idle.into_lua(h.lua()).is_ok());
}
