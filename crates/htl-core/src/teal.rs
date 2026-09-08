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

/// A Rust type exposed to Teal as a userdata module via `#[host_module]`.
pub trait HostModule {
    /// Module name used in `require("...")`.
    const MODULE: &'static str;
    /// Full `.d.tl` text for the module.
    const DECL: &'static str;
}
