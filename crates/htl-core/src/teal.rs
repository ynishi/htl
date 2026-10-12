//! Rust <-> Teal type bridge used by `#[derive(TealRecord)]` and `#[host_module]`.
//!
//! The macros map Rust types to Teal type names *syntactically* at expansion time
//! (`f64 -> number`, `String -> string`, `Vec<T> -> {T}`, ...). This module holds the
//! runtime-side traits the generated code implements, and the functions it calls:
//! [`field_error`] and its kin name a value that does not fit, and [`serde_field`] is what
//! a `#[teal(serde)]` field crosses through (htl's `serde` feature).

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

/// A struct record's fields, written into and read out of a table the caller holds — what
/// `#[serde(flatten)]` is built on. `#[derive(TealRecord)]` implements it for every struct
/// with named fields (not for an enum or an alias), and its own `IntoLua` / `FromLua` are
/// this around a table of their own; a struct that flattens another calls the inner type's
/// pair on *its* table, so the inner fields sit beside its own, as `serde_json` puts them.
///
/// `TealRecord` itself stays two constants: these are only meaningful for a record, and a
/// struct's `DECL` cannot carry another type's fields at compile time (the declaration
/// side reads the flattened struct from source instead, `htl::dts`). Not meant to be
/// implemented by hand: the derive's declaration and this conversion are one reading.
pub trait TealFields: Sized {
    /// Set each field's key in `t`, as the record's `into_lua` does (a skipped field, or
    /// one whose `skip_serializing_if` holds, is left out).
    fn write_fields(self, t: &mlua::Table, lua: &mlua::Lua) -> mlua::Result<()>;
    /// Read each field from `t`, reporting a failure as `path.field` — `path` is the
    /// record the table is declared as: the type's own name from its `from_lua`, the outer
    /// record's when it is flattened (the Teal side has no level for the inner struct, so a
    /// `FieldError` names the key where it sits: `Outer.innerField`).
    fn read_fields(t: &mlua::Table, lua: &mlua::Lua, path: &str) -> mlua::Result<Self>;
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

/// Name the untagged union a value did not belong to, listing the Teal types it is a union
/// of: `Shape: expected one of string, number, Shape_Rect, got boolean`. An untagged
/// enum's variants carry no word to quote; what tells them apart is the Lua type, so that
/// is what is listed and what `got` names.
pub fn union_error(name: &str, members: &[&str], got: &str) -> mlua::Error {
    mlua::Error::external(FieldError {
        path: name.to_string(),
        expected: format!("one of {}", members.join(", ")),
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

/// What `#[teal(serde)]` on a `#[derive(TealRecord)]` field crosses through: mlua's serde
/// (`LuaSerdeExt::to_value_with` / `from_value_with`), with options htl fixes rather than
/// mlua's defaults. The derive calls these; they are public only so that it can.
///
/// It exists under htl's `serde` feature (on with `std`, which is on by default). Without
/// it the module holds the same two functions bounded by a trait nothing implements,
/// `EnableHtlFeatureSerde`, so a field that says `#[teal(serde)]` fails to compile with an
/// error that names the feature instead of one about a missing function.
///
/// The options, and why:
///
/// - writing: `set_array_metatable` on (a sequence carries mlua's array metatable, so an
///   empty one reads back as a sequence); `serialize_none_to_null` off (a `None` is `nil`,
///   so an absent field is an absent key, as a Teal record field is nilable) — on only for
///   a field whose Rust type holds an `Option` inside a sequence (`Vec<Option<T>>`), where
///   `nil` would leave a hole a Lua sequence cannot hold, and the derive asks for it
///   (`none_to_null`); `serialize_unit_to_null` off; `detect_serde_json_arbitrary_precision`
///   on (a `serde_json::Number` crosses as a number). A value that serializes to the null
///   marker at the top (`None` under `none_to_null`) is written as `nil`: the field is
///   absent, not a marker.
/// - reading: `deny_unsupported_types` and `deny_recursive_tables` on (a function or a
///   cycle is an error, not dropped); `encode_empty_tables_as_array` from the field's Rust
///   type (`empty_as_array`, decided by the derive: a sequence type — `Vec`, `VecDeque`,
///   `HashSet`, `BTreeSet`, a slice or an array, through `Option` / `Box` — reads an empty
///   table as an empty sequence, anything else as an empty map). A serde type that holds
///   both a sequence and a map inside cannot be told apart on an empty table — the option
///   is one per conversion, and the derive sees only the field's type as written: that is
///   the residue of mlua's option, and why the outermost type decides.
///   `detect_mixed_tables` stays off: in mlua 0.12 it overrides `encode_empty_tables_as_array`
///   (an empty table is always an array under it), and it only reaches values serde reads
///   untyped (`serde_json::Value`) — a typed `Vec` reads a table's sequence part and ignores
///   every other key. So htl refuses a mixed table itself, before serde reads: any table in
///   the value with a sequence part (`#t > 0`) and a key that is not one of `1..#t` — a
///   string key beside the sequence, or a hole.
///
/// A failure is a [`FieldError`] at the field (`Rec.opts: expected {string: any}, got table
/// (a mixed table at `list`: ...)`), so a record nested in another extends the path as any
/// field's does.
pub mod serde_field {
    #[cfg(not(feature = "serde"))]
    use mlua::{Lua, Value};

    /// Implemented by nothing: the bound the two functions carry when htl is built without
    /// its `serde` feature, so `#[teal(serde)]` fails to compile naming the feature.
    #[cfg(not(feature = "serde"))]
    #[diagnostic::on_unimplemented(
        message = "`#[teal(serde)]` crosses a field through serde, which needs htl's `serde` feature",
        label = "this field says `#[teal(serde)]`",
        note = "enable it: `htl = {{ version = \"..\", features = [\"serde\"] }}` (the default `std` feature turns it on too)"
    )]
    pub trait EnableHtlFeatureSerde {}

    /// Without the `serde` feature: never callable, see [`EnableHtlFeatureSerde`].
    #[cfg(not(feature = "serde"))]
    pub fn to_lua<T: EnableHtlFeatureSerde + ?Sized>(
        _lua: &Lua,
        _value: &T,
        _record: &str,
        _field: &str,
        _none_to_null: bool,
    ) -> mlua::Result<Value> {
        unreachable!("no type implements EnableHtlFeatureSerde")
    }

    /// Without the `serde` feature: never callable, see [`EnableHtlFeatureSerde`].
    #[cfg(not(feature = "serde"))]
    pub fn from_lua<T: EnableHtlFeatureSerde>(
        _lua: &Lua,
        _value: Value,
        _record: &str,
        _field: &str,
        _expected: &str,
        _empty_as_array: bool,
    ) -> mlua::Result<T> {
        unreachable!("no type implements EnableHtlFeatureSerde")
    }

    #[cfg(feature = "serde")]
    pub use imp::{from_lua, to_lua};

    #[cfg(feature = "serde")]
    mod imp {
        use super::super::FieldError;
        use mlua::{ErrorContext, Lua, LuaSerdeExt, Table, Value};

        /// Field `record.field` -> Lua, through its `Serialize`, with htl's options.
        pub fn to_lua<T: serde::Serialize + ?Sized>(
            lua: &Lua,
            value: &T,
            record: &str,
            field: &str,
            none_to_null: bool,
        ) -> mlua::Result<Value> {
            let opts = mlua::serde::ser::Options::new()
                .set_array_metatable(true)
                .serialize_none_to_null(none_to_null)
                .serialize_unit_to_null(false)
                .detect_serde_json_arbitrary_precision(true);
            let v = lua
                .to_value_with(value, opts)
                .context(format!("{record}.{field}"))?;
            Ok(match v {
                Value::LightUserData(ud) if ud.0.is_null() => Value::Nil,
                v => v,
            })
        }

        /// Lua -> field `record.field` (declared `expected`), through its `Deserialize`,
        /// with htl's options; a mixed table anywhere in the value is refused first.
        pub fn from_lua<T: serde::de::DeserializeOwned>(
            lua: &Lua,
            value: Value,
            record: &str,
            field: &str,
            expected: &str,
            empty_as_array: bool,
        ) -> mlua::Result<T> {
            let fail = |got: String| {
                mlua::Error::external(FieldError {
                    path: format!("{record}.{field}"),
                    expected: expected.to_string(),
                    got,
                })
            };
            let got = value.type_name();
            if let Value::Table(t) = &value {
                let mut seen = Vec::new();
                if let Some(why) = mixed(t, "", &mut seen)? {
                    return Err(fail(format!("{got} ({why})")));
                }
            }
            let opts = mlua::serde::de::Options::new()
                .deny_unsupported_types(true)
                .deny_recursive_tables(true)
                .detect_mixed_tables(false)
                .encode_empty_tables_as_array(empty_as_array);
            lua.from_value_with(value, opts).map_err(|e| {
                let msg = match &e {
                    mlua::Error::DeserializeError(m) => m.clone(),
                    other => other.to_string(),
                };
                fail(format!("{got} ({msg})"))
            })
        }

        /// The first table in `t` (itself, then its values, depth first) with a sequence
        /// part and a key outside it, described with the keys that lead to it from the
        /// field; `None` when there is none. `seen` keeps a cycle from recursing (serde's
        /// own `deny_recursive_tables` reports it after).
        fn mixed(t: &Table, at: &str, seen: &mut Vec<usize>) -> mlua::Result<Option<String>> {
            let ptr = t.to_pointer() as usize;
            if seen.contains(&ptr) {
                return Ok(None);
            }
            seen.push(ptr);
            let n = t.raw_len();
            let mut inner = Vec::new();
            let mut in_seq_count = 0;
            for pair in t.clone().pairs::<Value, Value>() {
                let (k, v) = pair?;
                if n > 0 {
                    let in_seq = match k {
                        Value::Integer(i) => i >= 1 && (i as usize) <= n,
                        _ => false,
                    };
                    in_seq_count += usize::from(in_seq);
                    if !in_seq {
                        let key = match &k {
                            Value::String(s) => format!("{:?}", s.to_string_lossy()),
                            Value::Integer(i) => i.to_string(),
                            other => other.type_name().to_string(),
                        };
                        let place = if at.is_empty() {
                            String::new()
                        } else {
                            format!(" at `{at}`")
                        };
                        return Ok(Some(format!(
                            "a mixed table{place}: a sequence of {n} and the key {key}; a Lua \
                             table crosses as a sequence or as a map, not both, and not with holes"
                        )));
                    }
                }
                if let Value::Table(c) = v {
                    let seg = match &k {
                        Value::String(s) => s.to_string_lossy().to_string(),
                        Value::Integer(i) => format!("[{i}]"),
                        other => format!("[{}]", other.type_name()),
                    };
                    let path = if at.is_empty() {
                        seg
                    } else if seg.starts_with('[') {
                        format!("{at}{seg}")
                    } else {
                        format!("{at}.{seg}")
                    };
                    inner.push((c, path));
                }
            }
            if in_seq_count < n {
                let place = if at.is_empty() {
                    String::new()
                } else {
                    format!(" at `{at}`")
                };
                return Ok(Some(format!(
                    "a sequence with holes{place}: {in_seq_count} values under the length {n}; a \
                     Lua sequence cannot hold nil"
                )));
            }
            for (c, path) in inner {
                if let Some(why) = mixed(&c, &path, seen)? {
                    return Ok(Some(why));
                }
            }
            Ok(None)
        }
    }
}
