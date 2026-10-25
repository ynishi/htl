//! The acceptance of #538 that is about a *host crate*: `[rust] exports` in `htl.toml` and
//! `htl::teal_types!();` at the crate root give it a typed client of a Teal module.
//!
//! 1. A call with the arguments Teal declares builds, and run, returns what the Teal
//!    function computed.
//! 2. A call with an argument of the wrong type fails `cargo check` with `E0308`. The
//!    error is read from the compiler's JSON, by its code: rustc's wording differs between
//!    the stable release and the MSRV, and a snapshot of it would break with the version.
//!
//! The project is `crates/htl-core/tests/fixtures/client`, the one whose generated file the
//! tests of `htl rs`, `teal_types!()` and `htl::run` are held to.

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

fn run_cargo(project: &Path, args: &[&str]) -> Output {
    cargo()
        .args(args)
        .arg("--target-dir")
        .arg(cargo_target_dir())
        .current_dir(project)
        .output()
        .unwrap_or_else(|e| panic!("cargo {args:?}: could not be started: {e}"))
}

fn text_of(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// `main.rs` of the host: loads `shop.app` on a runner, calls both functions and prints what
/// they return. `call` is the line that is wrong in the second case.
fn main_rs(call: &str) -> String {
    format!(
        "htl::teal_types!();

#[tokio::main(flavor = \"current_thread\")]
async fn main() {{
    let src = std::path::PathBuf::from(env!(\"CARGO_MANIFEST_DIR\")).join(\"src\");
    let (runner, driver) = htl::run::start(htl::run::RunnerConfig {{
        search_paths: vec![src],
        deadline: std::time::Duration::from_secs(30),
        queue: 8,
    }})
    .await
    .unwrap();
    let app = shop::app::App::load(&runner, &[]).await.unwrap();
    {call}
    let greeted = app.greet(\"Ada\".to_string(), Some(\"Dr. \".to_string())).await.unwrap();
    println!(\"{{}} {{}}\", greeted.ok, greeted.text);
    driver.shutdown().await.unwrap();
}}
"
    )
}

#[test]
fn a_host_calls_a_teal_module_through_the_generated_client_and_a_wrong_argument_fails_to_build() {
    let dir = common::tempdir("htl-e2e", "exports-client");
    let project = dir.join("host");
    let fixture = workspace_root().join("crates/htl-core/tests/fixtures/client");
    fs::create_dir_all(project.join("src/shop")).unwrap();
    for rel in ["htl.toml", "src/shop/types.tl", "src/shop/app.tl"] {
        fs::copy(fixture.join(rel), project.join(rel)).unwrap();
    }
    fs::write(
        project.join("Cargo.toml"),
        format!(
            "[package]\n\
             name = \"host\"\n\
             version = \"0.1.0\"\n\
             edition = \"2024\"\n\
             publish = false\n\
             \n\
             [dependencies]\n\
             htl = {{ path = \"{}\", features = [\"async\"] }}\n\
             tokio = {{ version = \"1\", features = [\"rt\", \"macros\"] }}\n",
            workspace_root().join("crates/htl").display()
        ),
    )
    .unwrap();
    fs::copy(
        workspace_root().join("Cargo.lock"),
        project.join("Cargo.lock"),
    )
    .unwrap();

    // 1. The call Teal declares builds, and returns what the function computed.
    fs::write(
        project.join("src/main.rs"),
        main_rs("let (s, n) = app.split(\"abc\".to_string()).await.unwrap();\n    println!(\"{s} {n}\");"),
    )
    .unwrap();
    let out = run_cargo(&project, &["run", "-q"]);
    assert!(out.status.success(), "cargo run:\n{}", text_of(&out));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "abc 3\ntrue Dr. Ada\n",
        "{}",
        text_of(&out)
    );

    // 2. An integer where Teal says `string` is a type error of the host, not a run-time
    // error of the call: rustc's `E0308`, mismatched types.
    fs::write(
        project.join("src/main.rs"),
        main_rs("let (s, n) = app.split(42).await.unwrap();\n    println!(\"{s} {n}\");"),
    )
    .unwrap();
    let out = run_cargo(&project, &["check", "--message-format=json"]);
    assert!(
        !out.status.success(),
        "expected cargo check to fail:\n{}",
        text_of(&out)
    );
    let codes: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|m| m["reason"] == "compiler-message" && m["message"]["level"] == "error")
        .filter_map(|m| m["message"]["code"]["code"].as_str().map(str::to_string))
        .collect();
    assert_eq!(codes, ["E0308"], "{}", text_of(&out));
}
