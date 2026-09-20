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
//! | a checkout of a `v<version>` tag in GitHub Actions — the cargo-dist build that ships as a release | `release` | `"<this version>"` |
//! | cargo's git checkout (`cargo install --git`) | `main` | `{ git = …, branch = "main" }` |
//! | a checkout of this repository (`cargo run`, `cargo install --path`) | `path` | `{ path = "<checkout>/crates/htl" }` |
//!
//! `HTL_PIN_DEFAULT=release|main|path` in the environment at build time overrides the
//! guess. The tag row is the one guess that reads something other than the path: the
//! binaries a release ships are built in a checkout of the tag (`release.yml`, which dist
//! generates and is not edited by hand), and a checkout of a tag whose version is on
//! crates.io is a release. `GITHUB_REF` is `refs/tags/<tag>` in that job and a pull
//! request or branch ref everywhere else in CI, where a checkout build is a checkout build.

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=HTL_PIN_DEFAULT");
    println!("cargo:rerun-if-env-changed=GITHUB_REF");
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    // `crates/htl-cli` under the workspace root: the root is two up, and it is this
    // repository when the library crate is beside this one. A crates.io tarball is
    // extracted on its own, so the sibling is absent there; the packaged gate extracts
    // the four tarballs side by side, which is `../htl-<ver>` and not `../../crates/htl`.
    let root = manifest.join("..").join("..");
    let in_workspace = root.join("crates").join("htl").join("Cargo.toml").exists();
    let text = manifest.to_string_lossy().replace('\\', "/");
    let at_a_tag = env::var("GITHUB_REF").is_ok_and(|r| r.starts_with("refs/tags/"));
    let kind = match env::var("HTL_PIN_DEFAULT") {
        Ok(k) if ["release", "main", "path"].contains(&k.as_str()) => k,
        Ok(k) => panic!("HTL_PIN_DEFAULT is `{k}`; it takes release, main or path"),
        Err(_) if at_a_tag => "release".to_string(),
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
