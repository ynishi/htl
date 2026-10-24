//! The acceptance of #539: `htl::teal_types!();` at the crate root reads `htl.toml`
//! (`[rust] modules` and `[rust.types]`), runs `rsgen::generate` at `cargo build`, and
//! expands to the same `pub mod ... { ... }` items `htl rs` writes, with `include_str!`
//! tracking for `htl.toml` and every `.tl` source file it read.
//!
//! 1. A crate with `[rust] modules = ["ev"]` and `htl::teal_types!();` at the crate root
//!    builds, and `ev::Event` has the same fields as `htl rs` would write.
//! 2. A type error in `src/ev/init.tl` fails `cargo build` at the macro's span, with the
//!    Teal file and line.
//! 3. Editing `src/ev/init.tl` triggers a rebuild without `cargo clean`.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

mod common;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

fn cargo() -> Command {
    let mut cmd =
        Command::new(std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")));
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        let keep =
            name == "CARGO_HOME" || name == "CARGO_TARGET_DIR" || name == "CARGO_NET_OFFLINE";
        if name.starts_with("CARGO_") && !keep {
            cmd.env_remove(&key);
        }
    }
    cmd
}

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

fn tempdir(name: &str) -> common::TempDir {
    let dir = common::tempdir("htl-e2e", name);
    println!("scaffolding into {}", dir.display());
    dir
}

fn run_cargo(project: &Path, subcmd: &str) -> (Output, String) {
    let out = cargo()
        .arg(subcmd)
        .arg("--target-dir")
        .arg(cargo_target_dir())
        .current_dir(project)
        .output()
        .unwrap_or_else(|e| panic!("cargo {subcmd}: could not be started: {e}"));
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out, text)
}

const EV_TL: &str = "local record M
   record Event
      kind: Kind
      at: integer
      note: string
   end
   enum Kind
      \"open\"
      \"close\"
   end
end
return M
";

#[test]
fn a_crate_with_teal_types_at_the_root_builds_and_fails_at_the_macro_span_on_a_type_error() {
    let dir = tempdir("tealtypes");
    let project = dir.join("tealtypes");
    fs::create_dir_all(project.join("src/ev")).unwrap();

    fs::write(
        project.join("Cargo.toml"),
        format!(
            "[package]\n\
             name = \"tealtypes\"\n\
             version = \"0.1.0\"\n\
             edition = \"2024\"\n\
             publish = false\n\
             \n\
             [dependencies]\n\
             htl = {{ path = \"{}\" }}\n",
            workspace_root().join("crates/htl").display()
        ),
    )
    .unwrap();
    fs::copy(
        workspace_root().join("Cargo.lock"),
        project.join("Cargo.lock"),
    )
    .unwrap();
    fs::write(project.join("htl.toml"), "[rust]\nmodules = [\"ev\"]\n").unwrap();
    fs::write(project.join("src/ev/init.tl"), EV_TL).unwrap();
    fs::write(
        project.join("src/lib.rs"),
        "htl::teal_types!();\n\
         \n\
         pub fn check_event() -> ev::Event {\n\
         \x20   ev::Event {\n\
         \x20       kind: Some(ev::Kind::Open),\n\
         \x20       at: Some(3),\n\
         \x20       note: Some(String::from(\"hi\")),\n\
         \x20   }\n\
         }\n",
    )
    .unwrap();

    // Acceptance 1: crate with `[rust] modules = ["ev"]` and `htl::teal_types!();` at the
    // crate root builds, and `ev::Event` has the same fields (`kind`, `at`, `note`) as
    // `htl rs` writes.
    let (out, text) = run_cargo(&project, "build");
    assert!(out.status.success(), "cargo build:\n{text}");

    // Acceptance 2: a type error in `src/ev/init.tl` fails `cargo build` at the macro's
    // span (`src/lib.rs:1:1`), with the Teal file and line (`src/ev/init.tl:6:`).
    fs::write(
        project.join("src/ev/init.tl"),
        format!("{EV_TL}local bad: integer = \"not an integer\"\n"),
    )
    .unwrap();
    let (out, text) = run_cargo(&project, "build");
    assert!(
        !out.status.success(),
        "expected cargo build to fail:\n{text}"
    );
    assert!(
        text.contains("src/lib.rs:1:1") && text.contains("src/ev/init.tl:13:"),
        "expected macro span and Teal file:line:\n{text}"
    );

    // Editing `src/ev/init.tl` triggers a rebuild without `cargo clean` because the
    // expansion emitted `include_str!` for it.
    fs::write(
        project.join("src/ev/init.tl"),
        EV_TL.replace("note: string\n", "note: string\n      by: string\n"),
    )
    .unwrap();
    fs::write(
        project.join("src/lib.rs"),
        "htl::teal_types!();\n\
         \n\
         pub fn check_event() -> ev::Event {\n\
         \x20   ev::Event {\n\
         \x20       kind: Some(ev::Kind::Open),\n\
         \x20       at: Some(3),\n\
         \x20       note: Some(String::from(\"hi\")),\n\
         \x20       by: Some(String::from(\"me\")),\n\
         \x20   }\n\
         }\n",
    )
    .unwrap();
    let (out, text) = run_cargo(&project, "build");
    assert!(
        out.status.success(),
        "cargo build after editing src/ev/init.tl:\n{text}"
    );
}
