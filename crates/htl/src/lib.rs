//! htl — Holistic Typed Lua.
//!
//! Umbrella crate: everything from [`htl_core`] plus, with the default `macros`
//! feature, the proc macros from `htl-macros`. Generated code from the macros refers
//! to `::htl::...`, so depend on this crate (not on `htl-core` directly) when you use
//! `include_tl!` / `include_tl_bytes!` / `#[derive(TealRecord)]` / `#[host_module]`.
//!
//! # Embedding
//!
//! ```rust,ignore
//! use htl::{Htl, TealRecord, host_module, include_tl, include_tl_bytes};
//!
//! #[derive(TealRecord, Clone)]           // Teal record <-> plain table (IntoLua / FromLua)
//! pub struct Point { pub x: f64, pub y: f64 }
//!
//! pub struct Host { started: std::time::Instant }
//!
//! #[host_module(name = "host", dts = "scripts/host.d.tl", records = [Point])]
//! impl Host {
//!     pub fn uptime_ms(&self) -> u64 { self.started.elapsed().as_millis() as u64 }
//!     pub fn scale(&self, p: Point, k: f64) -> Point { Point { x: p.x * k, y: p.y * k } }
//!     pub fn greet(name: &str) -> String { format!("hello, {name}") }            // static
//!     pub fn parse(s: &str) -> Result<i64, std::num::ParseIntError> { s.parse() } // Err -> Lua error
//! }
//!
//! const MAIN: &str = include_tl!("scripts/main.tl");              // checked at cargo build
//! const UTIL: &[u8] = include_tl_bytes!("scripts/util.tl");       // same, as stripped bytecode
//!
//! fn main() -> anyhow::Result<()> {
//!     let h = Htl::new()?;
//!     Host { started: std::time::Instant::now() }.htl_preload(&h)?;
//!     h.preload_bytes("util", UTIL)?;
//!     h.exec(MAIN, "@scripts/main.tl", &[])?;         // frames read scripts/main.tl:<line>
//!     Ok(())
//! }
//! ```
//!
//! `include_tl!` checks the file with the search path `htl check` gives it
//! ([`project::file_view`]); a crate whose Teal lives in `scripts/` says `[layout] source
//! = "scripts"`. A script that reads `arg[1]` needs [`Htl::set_arg`] before
//! [`Htl::exec`]. What crosses between Rust and Teal, and as what, is [`dts`]; what
//! holds the runtime to the declaration is [`teal::Strict`] and [`Htl::strict_strings`];
//! a host running Teal it did not write builds the `Lua` itself and hands it to
//! [`Htl::with_checker_lua`]. A crate that registers a module for other people's
//! projects ships the declaration ([`dep_dts`]); a crate whose Teal has dependencies
//! ships them as a patched copy ([`Htl::apply_project`]). A host that serves modules at
//! run time describes its directories once ([`model::Project::for_host`]) and derives the
//! checker and the run from that ([`pkg::TealResolver`]).
//!
//! The macros run the checker inside the proc macro, under `[profile.dev.build-override]`
//! — `htl new --embed` writes `opt-level = 3` into that section, and a host that predates
//! it adds the two lines by hand (one rebuild of the macro's dependencies, then every
//! build after); see `htl_macros`.
//!
//! # Features
//!
//! With the `ffi` feature there is a fourth way out, for a caller that is not written in
//! Rust: the C ABI runtime — the handle, the error slot, the panic guard — and an
//! attribute that writes one `extern "C"` wrapper per method and the C header, from the
//! same `impl` block `#[host_module]` reads. Off by default, so a host with no C caller
//! compiles as it did before. A host that turns it on adds `crate-type = ["rlib",
//! "cdylib"]` (plus `"staticlib"` for Unity on iOS); `cargo build` then writes the
//! header, and the library exports the `<prefix>_*` functions and nothing else. The two
//! are named and linked in the paragraph this page shows when the feature is on; without
//! it there is nothing on this page to link to, and a link to an item that is not
//! compiled is a broken one.
#![cfg_attr(
    feature = "ffi",
    doc = "
That way out is [`ffi`] and [`macro@c_export`]."
)]
//!
//! The `async` feature is off by default for the same reason: it turns on mlua's own
//! `async`, and a host with no async method should be built as it was without it.
//!
//! # Against an unpublished htl
//!
//! A consumer building against a checkout of htl before a change is published patches
//! all three crates in its `[patch.crates-io]`, not this one alone: `htl` re-exports
//! `htl-core`, and the proc macros in `htl-macros` run `htl-core` at expansion time, so
//! patching only `htl` builds two versions of the same code into one graph. Cargo keeps
//! the version `Cargo.lock` already resolved until `cargo update -p htl -p htl-core -p
//! htl-macros` is run once; the same command, with the block deleted, goes back.
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
