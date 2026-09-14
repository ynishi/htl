//! Rust <-> Teal type bridge used by `#[derive(TealRecord)]` and `#[host_module]`.
//!
//! The macros map Rust types to Teal type names *syntactically* at expansion time
//! (`f64 -> number`, `String -> string`, `Vec<T> -> {T}`, ...). This module holds the
//! runtime-side traits the generated code implements.

/// A Rust type mirrored as a Teal declaration: a struct as a `record` (plain table with
/// named fields), a newtype as a `type` alias, an enum as an `enum` of its variant names
/// or, when a variant carries data, as a union of `where`-discriminated records. The
/// lowering is in `htl_core::dts`.
pub trait TealRecord {
    /// Teal type name (also the module name of its `.d.tl`).
    const NAME: &'static str;
    /// Full `.d.tl` text: `local record NAME ... end  return NAME`, or the `enum` /
    /// `type` form.
    const DECL: &'static str;
}

/// Why a value coming back from Lua did not fit a `#[derive(TealRecord)]` struct.
///
/// `FromLua` is the only place the two sides of a record are compared — the `.d.tl` is
/// generated from Rust, so what a host *offers* is checked at build time, while what it
/// *receives* is checked when a value crosses. This carries enough to act on: which
/// record, which field, the Teal type declared for it, and what arrived instead.
///
/// ```text
/// Outcome.cause: expected string, got nil
/// Outcome.depth: expected integer, got string
/// ```
///
/// A record inside a record extends the path rather than nesting the message, so the
/// innermost field is what the reader sees: `Recording.outcome.cause`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    /// `Record.field`, one segment per level of nesting.
    pub path: String,
    /// The Teal type the record declares for the field.
    pub expected: String,
    /// The Lua type that arrived: `nil`, `string`, `table`, ...
    pub got: String,
}

impl std::fmt::Display for FieldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: expected {}, got {}",
            self.path, self.expected, self.got
        )
    }
}

impl std::error::Error for FieldError {}

/// Name the record and the field a conversion failed on.
///
/// Called by the `FromLua` that `#[derive(TealRecord)]` generates, once per field that
/// fails. `cause` is what the field's own conversion returned, and is only replaced when
/// it says nothing the caller does not already know — a plain type mismatch. Anything
/// else (a host function's own error, a borrow failure) is kept and given the field as
/// context, because it carries more than this can reconstruct.
pub fn field_error(
    record: &str,
    field: &str,
    expected: &str,
    got: &str,
    cause: mlua::Error,
) -> mlua::Error {
    located(format!("{record}.{field}"), expected, got, cause)
}

/// Name the type a whole-value conversion failed on: a newtype (`type N = T` on the
/// Teal side) whose inner conversion refused what arrived. The alias is what the Teal
/// side declared, so it is the name the reader looks for: a `FieldError` from inside
/// (the inner type is a record, an enum, or another alias) is re-rooted at the alias —
/// `Label.y: expected number, got nil`, `Wrap: expected one of ..` — and a plain
/// mismatch becomes `N: expected T, got <type>`.
pub fn value_error(name: &str, expected: &str, got: &str, cause: mlua::Error) -> mlua::Error {
    located(name.to_string(), expected, got, cause)
}

/// The one policy behind `field_error` and `value_error`, given the location the caller
/// stands at (`Record.field`, `Enum.Variant.field`, or a bare type name):
///
/// - a `FieldError` from inside is re-rooted here: the head of its path is the inner
///   type's own name, which this location already states, so it is dropped and the rest
///   (if any — an enum or alias reports with a bare name) appended;
/// - a plain type mismatch becomes a `FieldError` at this location;
/// - anything else (a host function's own error, a borrow failure) keeps its message
///   with the location as context, because it carries more than this can reconstruct.
fn located(path: String, expected: &str, got: &str, cause: mlua::Error) -> mlua::Error {
    if let Some(inner) = cause.downcast_ref::<FieldError>() {
        let path = match inner.path.split_once('.') {
            Some((_, rest)) => format!("{path}.{rest}"),
            None => path,
        };
        return mlua::Error::external(FieldError {
            path,
            expected: inner.expected.clone(),
            got: inner.got.clone(),
        });
    }
    if !matches!(cause, mlua::Error::FromLuaConversionError { .. }) {
        return mlua::ErrorContext::context(cause, path);
    }
    mlua::Error::external(FieldError {
        path,
        expected: expected.to_string(),
        got: got.to_string(),
    })
}

/// Name the enum a value did not belong to, listing what it accepts:
/// `Mode: expected one of "Fast", "Careful", got "fst"`. `got` is the offending string
/// in quotes, or the Lua type name when it was not a string (or, for a data-carrying
/// enum, not a table). Reported as a `FieldError` so an enum inside a record extends the
/// record's path the way a nested record does.
pub fn enum_error(name: &str, variants: &[&str], got: &str) -> mlua::Error {
    let list: Vec<String> = variants.iter().map(|v| format!("\"{v}\"")).collect();
    mlua::Error::external(FieldError {
        path: name.to_string(),
        expected: format!("one of {}", list.join(", ")),
        got: got.to_string(),
    })
}

/// A host parameter that takes the Lua kind its Teal type names, and converts nothing.
///
/// mlua's `FromLua` for `i64` accepts a Lua string and coerces it (`"10"` arrives as
/// `10`), and its `FromLua` for `String` accepts a number and formats it. The `.d.tl`
/// that `#[host_module]` writes says `integer` and `string`, and checked Teal cannot pass
/// the other kind — but a value from past a cast or a `load`, which the checker did not
/// see, can, and the host has no way to tell the two apart once mlua has converted.
/// `Strict<T>` is the parameter type that keeps the runtime to the declaration: an
/// integer is `Value::Integer`, or a `Value::Number` with no fraction (as
/// `math.tointeger` reads it); a float is either number kind; `bool` is
/// `Value::Boolean`; `String` is `Value::String`. Anything else is a conversion error
/// naming what arrived — `error converting Lua string to integer` — the same shape mlua
/// reports, so a caller reading errors sees one kind of message.
///
/// ```ignore
/// #[host_module(name = "api")]
/// impl Api {
///     pub fn take(&self, n: Strict<i64>) -> i64 { *n }   // `take: function(self: api, n: integer): integer`
/// }
/// ```
///
/// The declaration is the inner type's — `Strict<i64>` is `integer` in the `.d.tl` — since
/// it already said that; this only holds the runtime to it. Derefs to `T`, and goes back
/// to Lua as `T` would, so a return type may be `Strict<T>` as well, though there it
/// adds nothing.
///
/// The other two layers of the same question are the checker, which holds checked Teal,
/// and [`Htl::strict_strings`](crate::Htl::strict_strings), which holds the program state
/// where arithmetic on a string would otherwise convert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Strict<T>(pub T);

impl<T> Strict<T> {
    /// The value, out of the wrapper.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> std::ops::Deref for Strict<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> std::ops::DerefMut for Strict<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T> From<T> for Strict<T> {
    fn from(v: T) -> Self {
        Strict(v)
    }
}

/// What [`Strict<T>`] accepts for a `T`: the Lua kind, read without conversion. Sealed;
/// the integers, `f32` / `f64`, `bool` and `String`.
pub trait StrictKind: Sized + private::Sealed {
    /// The Teal type the `.d.tl` declares for `T`, for the error message.
    const TEAL: &'static str;
    /// `Some` when `v` is the kind `T` names, `None` for every other kind.
    fn from_value(v: &mlua::Value) -> Option<Self>;
}

mod private {
    pub trait Sealed {}
}

macro_rules! strict_int {
    ($($t:ty),*) => {$(
        impl private::Sealed for $t {}
        impl StrictKind for $t {
            const TEAL: &'static str = "integer";
            fn from_value(v: &mlua::Value) -> Option<Self> {
                match *v {
                    mlua::Value::Integer(i) => <$t>::try_from(i).ok(),
                    // What `math.tointeger` accepts: a float with no fraction. Lua's own
                    // `3.0 == 3`, and a JSON decoder hands back a float for `3.0`.
                    mlua::Value::Number(n) if n.fract() == 0.0 && n.is_finite() => {
                        let i = n as i64;
                        (i as f64 == n).then(|| <$t>::try_from(i).ok()).flatten()
                    }
                    _ => None,
                }
            }
        }
    )*};
}
strict_int!(
    i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize
);

macro_rules! strict_float {
    ($($t:ty),*) => {$(
        impl private::Sealed for $t {}
        impl StrictKind for $t {
            const TEAL: &'static str = "number";
            fn from_value(v: &mlua::Value) -> Option<Self> {
                match *v {
                    mlua::Value::Integer(i) => Some(i as $t),
                    mlua::Value::Number(n) => Some(n as $t),
                    _ => None,
                }
            }
        }
    )*};
}
strict_float!(f32, f64);

impl private::Sealed for bool {}
impl StrictKind for bool {
    const TEAL: &'static str = "boolean";
    fn from_value(v: &mlua::Value) -> Option<Self> {
        match *v {
            mlua::Value::Boolean(b) => Some(b),
            _ => None,
        }
    }
}

impl private::Sealed for String {}
impl StrictKind for String {
    const TEAL: &'static str = "string";
    fn from_value(v: &mlua::Value) -> Option<Self> {
        match v {
            mlua::Value::String(s) => s.to_str().ok().map(|s| s.to_owned()),
            _ => None,
        }
    }
}

impl<T: StrictKind> mlua::FromLua for Strict<T> {
    fn from_lua(value: mlua::Value, _lua: &mlua::Lua) -> mlua::Result<Self> {
        T::from_value(&value)
            .map(Strict)
            .ok_or_else(|| mlua::Error::FromLuaConversionError {
                from: value.type_name(),
                to: T::TEAL.to_string(),
                message: Some(format!(
                    "a Strict parameter takes a Lua {} and converts nothing",
                    T::TEAL
                )),
            })
    }
}

impl<T: mlua::IntoLua> mlua::IntoLua for Strict<T> {
    fn into_lua(self, lua: &mlua::Lua) -> mlua::Result<mlua::Value> {
        self.0.into_lua(lua)
    }
}

/// A Rust type exposed to Teal as a userdata module via `#[host_module]`.
pub trait HostModule {
    /// Module name used in `require("...")`.
    const MODULE: &'static str;
    /// Full `.d.tl` text for the module.
    const DECL: &'static str;
}
