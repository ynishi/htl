//! Rust -> Teal type mapping now lives in `htl_core::dts` (shared with `htl dts`).
//! Kept as a thin alias so nothing in this crate needs to know where it moved.

#[allow(unused_imports)]
pub use htl_core::dts::{is_result, teal_type};
