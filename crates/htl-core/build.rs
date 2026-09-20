//! On Windows, link `advapi32`, which the `git2` that mlua-pkg brings in needs and does
//! not ask for.
//!
//! `mlua-pkg` (the `pkg` feature) depends on `git2 0.18`, whose `libgit2-sys 0.16` calls
//! `OpenProcessToken`, `RegOpenKeyExW`, `CryptGenRandom` and sixteen other functions
//! that live in `advapi32.dll`, but its build script links `winhttp`, `rpcrt4`, `ole32`,
//! `crypt32` and `secur32` only. Older Rust toolchains linked `advapi32` for every crate
//! and hid the omission; current ones do not, and every artifact that carries `htl-core`
//! (the proc macros, the `htl` binary) fails to link on `x86_64-pc-windows-msvc` with
//! nineteen unresolved externals. `libgit2-sys 0.18` fixed its build script; this is the
//! same line until mlua-pkg moves to a `git2` that carries it, at which point this file
//! goes.
//!
//! A `cargo:rustc-link-lib` from any crate in the graph reaches the final link of every
//! artifact that depends on it, which is why the line lives here rather than in each of
//! them.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-lib=advapi32");
    }
}
