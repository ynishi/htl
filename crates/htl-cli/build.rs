//! Where this binary was built from decides what `htl new` pins by default.
//!
//! The scaffold writes for the `htl` this CLI links — the same version, from the same
//! tree — so the only question is where that tree is for the project being written. A
//! published crate is at its version on crates.io; a checkout of this repository is at
//! its path. This script answers from `CARGO_MANIFEST_DIR`, once, at build time, and
//! `scaffold::HtlPin::default` reads the answer:
//!
//! | built from | `HTL_PIN_DEFAULT` | the project's `htl` line |
//! |---|---|---|
//! | the crates.io tarball (`cargo install htl-cli`), or any tree that is not this workspace | `release` | `"<this version>"` |
//! | cargo's git checkout (`cargo install --git`) | `main` | `{ git = …, branch = "main" }` |
//! | a checkout of this repository (`cargo run`, `cargo install --path`) | `path` | `{ path = "<checkout>/crates/htl" }` |
//!
//! `HTL_PIN_DEFAULT=release|main|path` in the environment at build time overrides the
//! guess. A binary built from a checkout of a tag and shipped as a release — cargo-dist —
//! is built in a checkout and is a release, and that is what the override is for.

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=HTL_PIN_DEFAULT");
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    // `crates/htl-cli` under the workspace root: the root is two up, and it is this
    // repository when the library crate is beside this one. A crates.io tarball is
    // extracted on its own, so the sibling is absent there; the packaged gate extracts
    // the four tarballs side by side, which is `../htl-<ver>` and not `../../crates/htl`.
    let root = manifest.join("..").join("..");
    let in_workspace = root.join("crates").join("htl").join("Cargo.toml").exists();
    let text = manifest.to_string_lossy().replace('\\', "/");
    let kind = match env::var("HTL_PIN_DEFAULT") {
        Ok(k) if ["release", "main", "path"].contains(&k.as_str()) => k,
        Ok(k) => panic!("HTL_PIN_DEFAULT is `{k}`; it takes release, main or path"),
        Err(_) if text.contains("/git/checkouts/") => "main".to_string(),
        Err(_) if in_workspace => "path".to_string(),
        Err(_) => "release".to_string(),
    };
    let checkout = if kind == "path" {
        // Canonical, so the pin does not carry the `..` segments — and forward slashes,
        // because the line it goes into is TOML.
        std::fs::canonicalize(&root)
            .unwrap_or(root)
            .to_string_lossy()
            .replace('\\', "/")
    } else {
        String::new()
    };
    println!("cargo:rustc-env=HTL_PIN_DEFAULT={kind}");
    println!("cargo:rustc-env=HTL_PIN_CHECKOUT={checkout}");
}
