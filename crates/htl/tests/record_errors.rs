//! What `#[derive(TealRecord)]`'s `FromLua` says when a table does not fit the struct.
//!
//! `.d.tl` generation covers what a host offers; this direction is only compared when a
//! value crosses, so the message is the whole diagnostic and has to name the field.

use htl::mlua::{FromLua, Value};
use htl::{Htl, TealRecord};

#[derive(TealRecord, Debug, Clone, PartialEq)]
pub struct Outcome {
    pub result: String,
    pub depth: u32,
    pub cause: String,
}

#[derive(TealRecord, Debug, Clone, PartialEq)]
pub struct Recording {
    pub label: String,
    pub outcome: Outcome,
}

/// Evaluate a Lua expression and convert it, the way a host call receives a return value.
fn convert<T: FromLua>(h: &Htl, expr: &str) -> htl::mlua::Result<T> {
    let v: Value = h.lua().load(expr).eval()?;
    T::from_lua(v, h.lua())
}

#[test]
fn a_field_that_is_absent_is_named_with_its_record() {
    let h = Htl::new().unwrap();
    let e = convert::<Outcome>(&h, "return { result = 'win', depth = 3 }").unwrap_err();
    assert_eq!(e.to_string(), "Outcome.cause: expected string, got nil");
}

#[test]
fn a_field_of_the_wrong_type_says_which_type_it_wanted() {
    let h = Htl::new().unwrap();
    let e = convert::<Outcome>(
        &h,
        "return { result = 'win', depth = 'deep', cause = 'quit' }",
    )
    .unwrap_err();
    assert_eq!(e.to_string(), "Outcome.depth: expected integer, got string");
}

#[test]
fn a_record_inside_a_record_extends_the_path() {
    let h = Htl::new().unwrap();
    let e = convert::<Recording>(
        &h,
        "return { label = 'run', outcome = { result = 'win', depth = 3 } }",
    )
    .unwrap_err();
    assert_eq!(
        e.to_string(),
        "Recording.outcome.cause: expected string, got nil"
    );
}

#[test]
fn a_table_that_fits_still_converts() {
    let h = Htl::new().unwrap();
    let got: Recording = convert(
        &h,
        "return { label = 'run', outcome = { result = 'win', depth = 3, cause = 'quit' } }",
    )
    .unwrap();
    assert_eq!(
        got,
        Recording {
            label: "run".into(),
            outcome: Outcome {
                result: "win".into(),
                depth: 3,
                cause: "quit".into(),
            },
        }
    );
}
