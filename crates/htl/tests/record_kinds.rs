//! `#[derive(TealRecord)]` on the shapes beyond a named-field struct: a unit enum, a
//! newtype, and a data-carrying enum. Each crosses both ways, and what does not fit is
//! named the way a record's field is.

use htl::mlua::{FromLua, IntoLua, Value};
use htl::teal::TealRecord as _;
use htl::{Htl, TealRecord};

#[derive(TealRecord, Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Fast,
    Careful,
}

#[derive(TealRecord, Debug, Clone, PartialEq)]
pub struct Label(pub String);

#[derive(TealRecord, Debug, Clone, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(TealRecord, Debug, Clone, PartialEq)]
pub enum Shape {
    Dot,
    Circle(f64),
    Rect { w: f64, h: f64 },
    At(Point),
}

/// A record holding the other kinds: their errors extend the record's path.
#[derive(TealRecord, Debug, Clone, PartialEq)]
pub struct Run {
    pub mode: Mode,
    pub label: Label,
    pub shape: Shape,
}

/// Struct-variant fields named like the generated code's own locals: the derive binds
/// them under prefixed names, so they neither shadow the table nor the Lua state.
#[derive(TealRecord, Debug, Clone, PartialEq)]
pub enum Shadow {
    Bind { t: f64, lua: String, v: i64 },
}

/// Newtypes over the other kinds: the alias is what the Teal side declared, so the
/// message is rooted at the alias, whatever the inner type reports.
#[derive(TealRecord, Debug, Clone, PartialEq)]
pub struct Wrap(pub Mode);

#[derive(TealRecord, Debug, Clone, PartialEq)]
pub struct Outer(pub Label);

#[derive(TealRecord, Debug, Clone, PartialEq)]
pub struct Boxed(pub Point);

/// The Teal spelling of the variants, `#[teal(rename_all)]` with one `#[teal(name)]`
/// override: a host enum replacing a Teal one keeps the words the project already holds.
#[derive(TealRecord, Debug, Clone, Copy, PartialEq)]
#[teal(rename_all = "snake_case")]
pub enum State {
    Open,
    InReview,
    #[teal(name = "done")]
    Closed,
}

/// The same on a data-carrying enum: the `kind` tag carries the renamed word, the
/// variant records keep their Rust names.
#[derive(TealRecord, Debug, Clone, PartialEq)]
#[teal(rename_all = "snake_case")]
pub enum Step {
    Idle,
    InReview(f64),
    NeedsWork { why: String },
}

fn convert<T: FromLua>(h: &Htl, expr: &str) -> htl::mlua::Result<T> {
    let v: Value = h.lua().load(expr).eval()?;
    T::from_lua(v, h.lua())
}

/// Convert to Lua and read the value back through a Lua expression.
fn round_trip<T: IntoLua + FromLua>(h: &Htl, v: T) -> T {
    let lv = v.into_lua(h.lua()).unwrap();
    T::from_lua(lv, h.lua()).unwrap()
}

// ---------------------------------------------------------------- declarations

#[test]
fn the_declarations_are_the_teal_forms() {
    assert_eq!(
        Mode::DECL,
        "local enum Mode\n   \"Fast\"\n   \"Careful\"\nend\n\nreturn Mode\n"
    );
    assert_eq!(Label::DECL, "local type Label = string\n\nreturn Label\n");
    assert!(
        Shape::DECL.contains("local record Shape_Circle\n   where self.kind == \"Circle\"\n   kind: string\n   value: number\nend\n"),
        "{}",
        Shape::DECL
    );
    assert!(
        Shape::DECL
            .contains("local type Shape = Shape_Dot | Shape_Circle | Shape_Rect | Shape_At\n"),
        "{}",
        Shape::DECL
    );
    assert_eq!(Run::NAME, "Run");
}

// ---------------------------------------------------------------- unit enum

#[test]
fn a_unit_enum_crosses_as_its_variant_name() {
    let h = Htl::new().unwrap();
    let v = Mode::Careful.into_lua(h.lua()).unwrap();
    assert_eq!(
        v.as_string()
            .map(|s| s.to_string_lossy().to_string())
            .as_deref(),
        Some("Careful")
    );
    assert_eq!(convert::<Mode>(&h, "return 'Fast'").unwrap(), Mode::Fast);
    assert_eq!(round_trip(&h, Mode::Careful), Mode::Careful);
}

#[test]
fn a_string_that_is_no_variant_lists_the_variants() {
    let h = Htl::new().unwrap();
    let e = convert::<Mode>(&h, "return 'fst'").unwrap_err();
    assert_eq!(
        e.to_string(),
        "Mode: expected one of \"Fast\", \"Careful\", got \"fst\""
    );
}

#[test]
fn a_non_string_for_a_unit_enum_says_what_arrived() {
    let h = Htl::new().unwrap();
    let e = convert::<Mode>(&h, "return 3").unwrap_err();
    assert_eq!(
        e.to_string(),
        "Mode: expected one of \"Fast\", \"Careful\", got integer"
    );
}

// ---------------------------------------------------------------- renamed variants

#[test]
fn a_renamed_enum_declares_and_crosses_as_the_renamed_words() {
    assert_eq!(
        State::DECL,
        "local enum State\n   \"open\"\n   \"in_review\"\n   \"done\"\nend\n\nreturn State\n"
    );
    let h = Htl::new().unwrap();
    let v = State::InReview.into_lua(h.lua()).unwrap();
    assert_eq!(
        v.as_string()
            .map(|s| s.to_string_lossy().to_string())
            .as_deref(),
        Some("in_review")
    );
    // The override wins over the rule, both ways across.
    assert_eq!(
        State::Closed
            .into_lua(h.lua())
            .unwrap()
            .as_string()
            .map(|s| s.to_string_lossy().to_string())
            .as_deref(),
        Some("done")
    );
    assert_eq!(convert::<State>(&h, "return 'open'").unwrap(), State::Open);
    assert_eq!(
        convert::<State>(&h, "return 'done'").unwrap(),
        State::Closed
    );
    assert_eq!(round_trip(&h, State::InReview), State::InReview);
}

/// The Rust spelling is not a word the boundary knows: it is refused like any other
/// string that is no variant, and the list is what the declaration says.
#[test]
fn the_rust_spelling_of_a_renamed_variant_is_refused() {
    let h = Htl::new().unwrap();
    let e = convert::<State>(&h, "return 'Open'").unwrap_err();
    assert_eq!(
        e.to_string(),
        "State: expected one of \"open\", \"in_review\", \"done\", got \"Open\""
    );
    let e = convert::<State>(&h, "return 'Closed'").unwrap_err();
    assert_eq!(
        e.to_string(),
        "State: expected one of \"open\", \"in_review\", \"done\", got \"Closed\""
    );
}

#[test]
fn a_renamed_data_enum_tags_with_the_renamed_word() {
    assert!(
        Step::DECL.contains(
            "local record Step_NeedsWork\n   where self.kind == \"needs_work\"\n   kind: string\n   why: string\nend\n"
        ),
        "{}",
        Step::DECL
    );
    let h = Htl::new().unwrap();
    let t = Step::InReview(2.5).into_lua(h.lua()).unwrap();
    let t = t.as_table().unwrap();
    assert_eq!(t.get::<String>("kind").unwrap(), "in_review");
    assert_eq!(t.get::<f64>("value").unwrap(), 2.5);
    assert_eq!(
        convert::<Step>(&h, "return { kind = 'idle' }").unwrap(),
        Step::Idle
    );
    assert_eq!(round_trip(&h, Step::InReview(1.0)), Step::InReview(1.0));
    // A failure inside a variant is reported under the record the value was filling,
    // which keeps its Rust name (`record Step_NeedsWork`).
    let e = convert::<Step>(&h, "return { kind = 'needs_work' }").unwrap_err();
    assert_eq!(
        e.to_string(),
        "Step.NeedsWork.why: expected string, got nil"
    );
    let e = convert::<Step>(&h, "return { kind = 'InReview' }").unwrap_err();
    assert_eq!(
        e.to_string(),
        "Step: expected one of \"idle\", \"in_review\", \"needs_work\", got \"InReview\""
    );
}

// ---------------------------------------------------------------- newtype

#[test]
fn a_newtype_crosses_as_its_inner_value() {
    let h = Htl::new().unwrap();
    let v = Label("hi".into()).into_lua(h.lua()).unwrap();
    assert_eq!(
        v.as_string()
            .map(|s| s.to_string_lossy().to_string())
            .as_deref(),
        Some("hi")
    );
    assert_eq!(
        convert::<Label>(&h, "return 'there'").unwrap(),
        Label("there".into())
    );
}

#[test]
fn a_newtype_that_does_not_fit_is_named_with_its_inner_type() {
    let h = Htl::new().unwrap();
    let e = convert::<Label>(&h, "return {}").unwrap_err();
    assert_eq!(e.to_string(), "Label: expected string, got table");
}

// ---------------------------------------------------------------- data enum

#[test]
fn every_variant_shape_round_trips() {
    let h = Htl::new().unwrap();
    for s in [
        Shape::Dot,
        Shape::Circle(2.5),
        Shape::Rect { w: 1.0, h: 2.0 },
        Shape::At(Point { x: 3.0, y: 4.0 }),
    ] {
        assert_eq!(round_trip(&h, s.clone()), s);
    }
}

#[test]
fn a_data_enum_is_a_table_tagged_with_kind() {
    let h = Htl::new().unwrap();
    let t = Shape::Circle(2.5).into_lua(h.lua()).unwrap();
    let t = t.as_table().unwrap();
    assert_eq!(t.get::<String>("kind").unwrap(), "Circle");
    assert_eq!(t.get::<f64>("value").unwrap(), 2.5);
    let t = Shape::Rect { w: 1.0, h: 2.0 }.into_lua(h.lua()).unwrap();
    let t = t.as_table().unwrap();
    assert_eq!(t.get::<String>("kind").unwrap(), "Rect");
    assert_eq!(t.get::<f64>("h").unwrap(), 2.0);
    assert_eq!(
        convert::<Shape>(&h, "return { kind = 'Dot' }").unwrap(),
        Shape::Dot
    );
}

#[test]
fn a_variant_missing_its_payload_is_named_by_path() {
    let h = Htl::new().unwrap();
    let e = convert::<Shape>(&h, "return { kind = 'Circle' }").unwrap_err();
    assert_eq!(
        e.to_string(),
        "Shape.Circle.value: expected number, got nil"
    );
    let e = convert::<Shape>(&h, "return { kind = 'Rect', w = 1 }").unwrap_err();
    assert_eq!(e.to_string(), "Shape.Rect.h: expected number, got nil");
    let e = convert::<Shape>(&h, "return { kind = 'At', value = { x = 1 } }").unwrap_err();
    assert_eq!(e.to_string(), "Shape.At.value.y: expected number, got nil");
}

#[test]
fn an_unknown_kind_or_no_table_lists_the_variants() {
    let h = Htl::new().unwrap();
    let e = convert::<Shape>(&h, "return { kind = 'Square' }").unwrap_err();
    assert_eq!(
        e.to_string(),
        "Shape: expected one of \"Dot\", \"Circle\", \"Rect\", \"At\", got \"Square\""
    );
    let e = convert::<Shape>(&h, "return { value = 1 }").unwrap_err();
    assert_eq!(
        e.to_string(),
        "Shape: expected one of \"Dot\", \"Circle\", \"Rect\", \"At\", got nil"
    );
    let e = convert::<Shape>(&h, "return 'Dot'").unwrap_err();
    assert_eq!(
        e.to_string(),
        "Shape: expected one of \"Dot\", \"Circle\", \"Rect\", \"At\", got string"
    );
}

/// A Lua string need not be UTF-8. One that is not is no variant, and is reported by
/// type since there is nothing readable to quote — for a unit enum and for `kind`.
#[test]
fn a_non_utf8_string_is_refused_by_type() {
    let h = Htl::new().unwrap();
    let e = convert::<Mode>(&h, "return '\\255'").unwrap_err();
    assert_eq!(
        e.to_string(),
        "Mode: expected one of \"Fast\", \"Careful\", got string"
    );
    let e = convert::<Shape>(&h, "return { kind = '\\255' }").unwrap_err();
    assert_eq!(
        e.to_string(),
        "Shape: expected one of \"Dot\", \"Circle\", \"Rect\", \"At\", got string"
    );
}

#[test]
fn fields_named_t_and_lua_round_trip() {
    let h = Htl::new().unwrap();
    let s = Shadow::Bind {
        t: 1.5,
        lua: "x".into(),
        v: 7,
    };
    assert_eq!(round_trip(&h, s.clone()), s);
    let e = convert::<Shadow>(&h, "return { kind = 'Bind', t = 1, lua = 'x' }").unwrap_err();
    assert_eq!(e.to_string(), "Shadow.Bind.v: expected integer, got nil");
}

// ---------------------------------------------------------------- newtype over the rest

#[test]
fn a_newtype_over_an_enum_reports_as_the_alias() {
    let h = Htl::new().unwrap();
    assert_eq!(round_trip(&h, Wrap(Mode::Fast)), Wrap(Mode::Fast));
    let e = convert::<Wrap>(&h, "return 'x'").unwrap_err();
    assert_eq!(
        e.to_string(),
        "Wrap: expected one of \"Fast\", \"Careful\", got \"x\""
    );
}

#[test]
fn a_newtype_over_a_newtype_reports_as_the_outer_alias() {
    let h = Htl::new().unwrap();
    assert_eq!(
        round_trip(&h, Outer(Label("a".into()))),
        Outer(Label("a".into()))
    );
    let e = convert::<Outer>(&h, "return {}").unwrap_err();
    assert_eq!(e.to_string(), "Outer: expected string, got table");
}

#[test]
fn a_newtype_over_a_record_reports_the_field_under_the_alias() {
    let h = Htl::new().unwrap();
    assert_eq!(
        round_trip(&h, Boxed(Point { x: 1.0, y: 2.0 })),
        Boxed(Point { x: 1.0, y: 2.0 })
    );
    let e = convert::<Boxed>(&h, "return { x = 1 }").unwrap_err();
    assert_eq!(e.to_string(), "Boxed.y: expected number, got nil");
}

// ---------------------------------------------------------------- inside a record

#[test]
fn inside_a_record_the_path_is_the_record_and_field() {
    let h = Htl::new().unwrap();
    let e = convert::<Run>(
        &h,
        "return { mode = 'slow', label = 'x', shape = { kind = 'Dot' } }",
    )
    .unwrap_err();
    assert_eq!(
        e.to_string(),
        "Run.mode: expected one of \"Fast\", \"Careful\", got \"slow\""
    );
    let e = convert::<Run>(
        &h,
        "return { mode = 'Fast', label = {}, shape = { kind = 'Dot' } }",
    )
    .unwrap_err();
    assert_eq!(e.to_string(), "Run.label: expected string, got table");
    let e = convert::<Run>(
        &h,
        "return { mode = 'Fast', label = 'x', shape = { kind = 'Circle' } }",
    )
    .unwrap_err();
    assert_eq!(
        e.to_string(),
        "Run.shape.Circle.value: expected number, got nil"
    );
    let got: Run = convert(
        &h,
        "return { mode = 'Fast', label = 'x', shape = { kind = 'Rect', w = 1, h = 2 } }",
    )
    .unwrap();
    assert_eq!(
        got,
        Run {
            mode: Mode::Fast,
            label: Label("x".into()),
            shape: Shape::Rect { w: 1.0, h: 2.0 },
        }
    );
}
