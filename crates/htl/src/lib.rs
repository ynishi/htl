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
//! Rust: the C ABI runtime — the handle, the error slot, the panic guard — and an
//! attribute that writes one `extern "C"` wrapper per method and the C header, from the
//! same `impl` block `#[host_module]` reads. Off by default, so a host with no C caller
//! compiles as it did before. The two are named and linked in the paragraph this page
//! shows when the feature is on; without it there is nothing on this page to link to, and
//! a link to an item that is not compiled is a broken one.
#![cfg_attr(
    feature = "ffi",
    doc = "
That way out is [`ffi`] and [`macro@c_export`]."
)]
//!
//! Nearly everything documented here is defined in [`htl_core`] and re-exported by the
//! line below, so that is where those pages are written and where a gap in them is filled.
//! The `deny` under this paragraph holds only what this crate defines itself: `missing_docs`
//! fires in the crate that *defines* an item, not in the one that re-exports it, so a clean
//! run here says nothing about `htl_core`'s half of the same public surface. That half is
//! being closed file by file, and the `deny` goes there when it reaches zero (#224).
#![deny(missing_docs)]

pub use htl_core::*;

#[cfg(feature = "macros")]
pub use htl_macros::{TealRecord, host_module, include_bundle, include_tl, include_tl_bytes};

#[cfg(all(feature = "macros", feature = "ffi"))]
pub use htl_macros::c_export;
