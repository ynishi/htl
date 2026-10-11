//! `#[teal(as = "..")]` on a `#[derive(TealRecord)]` field: the declaration says the Teal
//! type given, the value crosses by the field type's own `IntoLua` / `FromLua`, and a read
//! that does not fit is reported at the field with the declared type.

use htl::mlua::{self, FromLua, IntoLua, Lua, Value};
use htl::teal::TealRecord as _;
use htl::{Htl, TealRecord};

/// A host type the type table cannot see into: a timestamp that crosses as its decimal
/// string, through conversions written by hand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stamp(pub u64);

impl IntoLua for Stamp {
    fn into_lua(self, lua: &Lua) -> mlua::Result<Value> {
        Ok(Value::String(lua.create_string(self.0.to_string())?))
    }
}

impl FromLua for Stamp {
    fn from_lua(value: Value, _: &Lua) -> mlua::Result<Self> {
        let not_a_stamp = |from: &'static str| mlua::Error::FromLuaConversionError {
            from,
            to: "Stamp".to_string(),
            message: None,
        };
        match &value {
            Value::String(s) => s
                .to_str()
                .ok()
                .and_then(|s| s.parse().ok())
                .map(Stamp)
                .ok_or_else(|| not_a_stamp("string")),
            other => Err(not_a_stamp(other.type_name())),
        }
    }
}

#[derive(TealRecord, Debug, Clone, PartialEq)]
pub struct Rec {
    #[teal(as = "string")]
    pub stamp: Stamp,
    pub text: String,
}

/// The same word on a data variant's field.
#[derive(TealRecord, Debug, Clone, PartialEq)]
pub enum Ev {
    Tick {
        #[teal(as = "string")]
        at: Stamp,
    },
    Stop,
}

fn convert<T: FromLua>(h: &Htl, expr: &str) -> mlua::Result<T> {
    let v: Value = h.lua().load(expr).eval()?;
    T::from_lua(v, h.lua())
}

#[test]
fn the_field_is_declared_as_the_text_given() {
    assert_eq!(
        Rec::DECL,
        "local record Rec\n   stamp: string\n   text: string\nend\n\nreturn Rec\n"
    );
    assert!(
        Ev::DECL.contains("local record Ev_Tick\n   where self.kind == \"Tick\"\n   kind: string\n   at: string\nend\n"),
        "{}",
        Ev::DECL
    );
}

#[test]
fn the_value_crosses_by_the_field_types_own_conversion() {
    let h = Htl::new().unwrap();
    let r = Rec {
        stamp: Stamp(1_700_000_000),
        text: "hi".into(),
    };
    let lv = r.clone().into_lua(h.lua()).unwrap();
    let t = lv.as_table().unwrap();
    // What Teal reads is what the declaration says: a string.
    assert_eq!(t.get::<String>("stamp").unwrap(), "1700000000");
    assert_eq!(Rec::from_lua(lv, h.lua()).unwrap(), r);
    assert_eq!(
        convert::<Rec>(&h, "return { stamp = '42', text = 'x' }").unwrap(),
        Rec {
            stamp: Stamp(42),
            text: "x".into()
        }
    );
    let ev = Ev::Tick { at: Stamp(7) };
    let lv = ev.clone().into_lua(h.lua()).unwrap();
    assert_eq!(Ev::from_lua(lv, h.lua()).unwrap(), ev);
}

#[test]
fn a_value_that_does_not_fit_is_named_at_the_field_with_the_declared_type() {
    let h = Htl::new().unwrap();
    let e = convert::<Rec>(&h, "return { stamp = 42, text = 'x' }").unwrap_err();
    assert_eq!(e.to_string(), "Rec.stamp: expected string, got integer");
    let e = convert::<Ev>(&h, "return { kind = 'Tick', at = true }").unwrap_err();
    assert_eq!(e.to_string(), "Ev.Tick.at: expected string, got boolean");
}
