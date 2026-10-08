//! The acceptance of #429: a `#[host_module]` in one file and the `include_tl!` that
//! checks a `.tl` against its declaration in another build in either order of the
//! crate's `mod` declarations, because `include_tl!` regenerates every `#[host_module]`'s
//! `.d.tl` from the Rust source before it checks (`regenerate_crate`, in
//! `crates/htl-macros/src/lib.rs`) — the same scan `htl dts` runs, so a build never reads
//! a declaration an edit to the host already made stale.
//!
//! `htl new` always writes `#[host_module]` and the `include_tl!`/`include_bundle!` that
//! checks against it in the same file (`src/lib.rs`), so there is no `--target` that
//! scaffolds the two-file layout the issue reported, and this writes the project by
//! hand instead of going through the CLI: `src/hostio.rs` (the host module) and
//! `src/scripts.rs` (the `include_tl!`, of `src/use_io.tl`), with `src/lib.rs` declaring
//! `mod scripts;` *before* `mod hostio;` — the order that read yesterday's `.d.tl` before
//! the fix, because rustc expands a crate's proc macros in the order its `mod`
//! declarations bring each file in. The one thing a scaffolded project would have added
//! is the dependency on this checkout, so the `Cargo.toml` below writes that line itself
//! (`crates/htl`, by path, the way `htl new` pins a checkout CLI's own build).
//!
//! It is a separate file from `scaffold_targets.rs` for the same reason `embed_publish.rs`
//! and `htlx_consumer.rs` are: it is not about a target `htl new --target` offers, and
//! duplicating the handful of helpers it needs is cheaper to read than a shared module
//! would be. [`cargo`] is explained there; this repeats only what it does, not why.
//!
//! The dependency is `crates/htl` of *this checkout*, always, by path — not
//! `HTL_PATCH_HTL` / `HTL_PATCH_CORE` / `HTL_PATCH_MACROS`, which `embed_publish.rs` and
//! `scaffold_targets.rs` read so the packaged gate can point them at extracted tarballs
//! instead. This test is about the macro's behaviour, not about anything packaging can
//! change (there is no `#[host_module]`-ordering difference between a checkout's
//! `htl-macros` and a published one), so it does not need the packaged gate's redirection
//! and does not offer it; `just e2e-scaffold-packaged` runs the checkout's macro here
//! exactly as `just e2e` does.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// The root of this workspace: this crate sits directly under it.
fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

/// A cargo with the parent's `CARGO_*` environment stripped except `CARGO_HOME` and
/// `CARGO_TARGET_DIR`; `scaffold_targets.rs`'s `cargo()` says why each is dropped or kept.
fn cargo() -> Command {
    let mut cmd =
        Command::new(std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")));
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        let keep = name == "CARGO_HOME" || name == "CARGO_TARGET_DIR";
        if name.starts_with("CARGO_") && !keep {
            cmd.env_remove(&key);
        }
    }
    cmd
}

/// Where the scaffolded projects put their artefacts: the same directory
/// `scaffold_targets.rs` and the other e2e files share, asked of cargo rather than
/// assembled, for the same reason.
fn cargo_target_dir() -> &'static Path {
    static TARGET: OnceLock<PathBuf> = OnceLock::new();
    TARGET.get_or_init(|| {
        if let Some(given) = std::env::var_os("HTL_E2E_TARGET") {
            return PathBuf::from(given);
        }
        let out = cargo()
            .args(["metadata", "--no-deps", "--format-version", "1"])
            .current_dir(workspace_root())
            .stderr(Stdio::inherit())
            .output()
            .expect("cargo metadata could not be started");
        assert!(out.status.success(), "cargo metadata: {}", out.status);
        let meta: serde_json::Value =
            serde_json::from_slice(&out.stdout).expect("cargo metadata did not answer in JSON");
        let dir = meta["target_directory"]
            .as_str()
            .expect("cargo metadata named no target_directory");
        PathBuf::from(dir).join("e2e-scaffold")
    })
}

/// A fresh directory under the system temp dir, outside the checkout so the CLI runs
/// against the project's own configuration and not this repository's.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("htl-e2e-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    println!("scaffolding into {}", dir.display());
    dir
}

/// `cargo build` in `project`, with the shared target directory on it, failing the test
/// naming the command and showing both streams if it did not succeed.
fn cargo_build(project: &Path, what: &str) {
    let out = cargo()
        .arg("build")
        .arg("--target-dir")
        .arg(cargo_target_dir())
        .current_dir(project)
        .output()
        .unwrap_or_else(|e| panic!("{what}: could not be started: {e}"));
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    println!("--- {what} ---\n{text}");
    assert!(out.status.success(), "{what}: {}\n{text}", out.status);
}

/// `src/hostio.rs`, naming `method` as the one method the host module declares.
fn hostio_rs(method: &str) -> String {
    format!(
        "use htl::host_module;\n\
         \n\
         pub struct HostIo;\n\
         \n\
         #[host_module(name = \"hostio\", dts = \"src/hostio.d.tl\")]\n\
         impl HostIo {{\n\
         \x20   pub fn {method}(&self) -> i64 {{\n\
         \x20       1\n\
         \x20   }}\n\
         }}\n"
    )
}

/// `src/use_io.tl`, calling `method` on the `hostio` host module the way the issue's
/// report did.
fn use_io_tl(method: &str) -> String {
    format!("local io_ = require(\"hostio\")\nio_:{method}()\n")
}

/// The layout `#429` reported: `#[host_module]` in `src/hostio.rs`, the `include_tl!` of
/// `src/use_io.tl` in `src/scripts.rs`, and `src/lib.rs` declaring `mod scripts;` *before*
/// `mod hostio;` — the host module declared after the module that checks against it.
///
/// The discriminator throughout is simply whether `cargo build` succeeds — not any
/// particular file's content afterward. Without the fix, that order makes the very
/// *first* `cargo build` of this layout fail, whether or not a rename is involved: a
/// fresh checkout has no `src/hostio.d.tl` yet, `scripts.rs`'s `include_tl!` expands
/// first (the `mod` order), and it reads nothing where a declaration should be. #429's
/// own report encountered this as a rename — a project that had built once, with the
/// method renamed in both files, failing on the next build — so this test exercises both:
/// a first build with the files already agreeing (which also fails without the fix, cold,
/// with no rename in sight), and then the rename `#429` reported, built once more.
#[test]
fn a_host_module_declared_after_the_include_tl_that_checks_against_it_builds_in_one_pass() {
    let scratch = scratch("hostorder");
    let project = scratch.join("hostorder");
    fs::create_dir_all(project.join("src")).unwrap_or_else(|e| panic!("{e}"));

    fs::write(
        project.join("Cargo.toml"),
        format!(
            "[package]\n\
             name = \"hostorder\"\n\
             version = \"0.1.0\"\n\
             edition = \"2024\"\n\
             publish = false\n\
             \n\
             [dependencies]\n\
             htl = {{ path = \"{}\" }}\n\
             \n\
             [profile.dev.build-override]\n\
             opt-level = 3\n",
            workspace_root().join("crates/htl").display()
        ),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    // `mod scripts;` first: the `include_tl!` that checks against `hostio`'s declaration
    // is declared before the `#[host_module]` that writes it, which is the order `#429`
    // reported reading a stale `.d.tl` under.
    fs::write(
        project.join("src/lib.rs"),
        "pub mod scripts;\npub mod hostio;\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    fs::write(project.join("src/hostio.rs"), hostio_rs("second")).unwrap_or_else(|e| panic!("{e}"));
    fs::write(
        project.join("src/scripts.rs"),
        "pub const USE_IO: &str = htl::include_tl!(\"src/use_io.tl\");\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    fs::write(project.join("src/use_io.tl"), use_io_tl("second")).unwrap_or_else(|e| panic!("{e}"));

    // The first build of this layout, cold — no `src/hostio.d.tl` exists yet. Without the
    // fix this already fails (`scripts.rs`'s `include_tl!` expands before `hostio.rs`'s
    // `#[host_module]` writes anything, the `mod` order above), with no rename involved:
    // the `cargo build` succeeding here is itself part of the acceptance, not a setup step
    // whose only job is to let the rename below be "the" change under test.
    cargo_build(&project, "cargo build (first build, cold, no rename)");

    // The rename `#429`'s report made: the host method in `hostio.rs`, and the call site
    // in `use_io.tl`, both in one pass with no build in between. Without the fix this
    // build fails the same way the first one would have; with it, it passes.
    fs::write(project.join("src/hostio.rs"), hostio_rs("third")).unwrap_or_else(|e| panic!("{e}"));
    fs::write(project.join("src/use_io.tl"), use_io_tl("third")).unwrap_or_else(|e| panic!("{e}"));
    cargo_build(&project, "cargo build (after the rename, one pass)");

    // A basic sanity check, not a discriminator: this would read `third:` whether or not
    // the fix exists, because a build that gets this far at all means `hostio.rs`'s own
    // `#[host_module]` expansion ran and wrote its current declaration — it always does,
    // last, once the crate compiles. What the fix actually changes is captured above, in
    // whether `cargo build` succeeds in one pass at all.
    let decl = fs::read_to_string(project.join("src/hostio.d.tl"))
        .unwrap_or_else(|e| panic!("src/hostio.d.tl: {e}"));
    assert!(
        decl.contains("third:"),
        "the project compiled, so its own #[host_module] should have written this:\n{decl}"
    );

    fs::remove_dir_all(&scratch).ok();
}
