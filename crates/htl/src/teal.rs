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

/// A Rust type exposed to Teal as a userdata module via `#[host_module]`.
pub trait HostModule {
    /// Module name used in `require("...")`.
    const MODULE: &'static str;
    /// Full `.d.tl` text for the module.
    const DECL: &'static str;
}
