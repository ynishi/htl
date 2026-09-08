//! htl — Holistic Typed Lua.
//!
//! Umbrella crate: everything from [`htl_core`] plus, with the default `macros`
//! feature, the proc macros from `htl-macros`. Generated code from the macros refers
//! to `::htl::...`, so depend on this crate (not on `htl-core` directly) when you use
//! `include_tl!` / `include_tl_bytes!` / `#[derive(TealRecord)]` / `#[host_module]`.
//!
//! ```rust,ignore
//! use htl::{Htl, TealRecord, host_module, include_tl, include_tl_bytes};
//! ```
//!
//! With the `ffi` feature there is a fourth way out, for a caller that is not written in
//! Rust: [`ffi`] is the C ABI runtime — the handle, the error slot, the panic guard —
//! and [`macro@c_export`] writes one `extern "C"` wrapper per method and the C header,
//! from the same `impl` block `#[host_module]` reads. Off by default, so a host with no
//! C caller compiles as it did before.

pub use htl_core::*;

#[cfg(feature = "macros")]
pub use htl_macros::{TealRecord, host_module, include_bundle, include_tl, include_tl_bytes};

#[cfg(all(feature = "macros", feature = "ffi"))]
pub use htl_macros::c_export;
