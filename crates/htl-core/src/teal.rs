//! Rust <-> Teal type bridge used by `#[derive(TealRecord)]` and `#[host_module]`.
//!
//! The macros map Rust types to Teal type names *syntactically* at expansion time
//! (`f64 -> number`, `String -> string`, `Vec<T> -> {T}`, ...). This module holds the
//! runtime-side traits the generated code implements.

/// A Rust struct mirrored as a Teal `record` (plain table with named fields).
pub trait TealRecord {
    /// Teal record name (also the module name of its `.d.tl`).
    const NAME: &'static str;
    /// Full `.d.tl` text: `local record NAME ... end  return NAME`.
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
    if let Some(inner) = cause.downcast_ref::<FieldError>() {
        // A record inside a record. The head of the inner path is this field's own
        // record name, which the outer path is about to state; drop it and keep going.
        let rest = inner
            .path
            .split_once('.')
            .map(|(_, r)| r)
            .unwrap_or(&inner.path);
        return mlua::Error::external(FieldError {
            path: format!("{record}.{field}.{rest}"),
            expected: inner.expected.clone(),
            got: inner.got.clone(),
        });
    }
    if !matches!(cause, mlua::Error::FromLuaConversionError { .. }) {
        return mlua::ErrorContext::context(cause, format!("{record}.{field}"));
    }
    mlua::Error::external(FieldError {
        path: format!("{record}.{field}"),
        expected: expected.to_string(),
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
