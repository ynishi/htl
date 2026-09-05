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

pub use htl_core::*;

#[cfg(feature = "macros")]
pub use htl_macros::{TealRecord, host_module, include_tl, include_tl_bytes};
