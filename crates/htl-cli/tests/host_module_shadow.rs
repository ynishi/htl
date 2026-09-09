//! A host module name that is also a Teal file on the search path.
//!
//! The host puts its modules in `package.preload`, and Lua consults preload before any
//! path searcher; the checker has no preload and searches the path. So a `src/host.tl`
//! beside `#[host_module(name = "host")]` is checked and never run, while the host module
//! is run and never checked — a project in that state passes every gate and fails at the
//! first call of anything the two do not share. `host-module-shadowed` says so at the
//! require.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-host-shadow", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Every diagnostic `htl check <target>` printed, run from `root`, as its JSON object.
fn diagnostics(root: &Path, target: &str) -> Vec<serde_json::Value> {
    let out = Command::new(env!("CARGO_BIN_EXE_htl"))
        .arg("check")
        .arg(target)
        .arg("--format")
        .arg("json")
        .arg("--no-cache")
        .current_dir(root)
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("stdout is one JSON document");
    v["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .to_vec()
}

fn shadow_reports(root: &Path, target: &str) -> Vec<serde_json::Value> {
    diagnostics(root, target)
        .into_iter()
        .filter(|d| d["rule"].as_str() == Some("host-module-shadowed"))
        .collect()
}

const MANIFEST: &str = "[package]\nname = \"p\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";

/// A `#[host_module]` impl, written the way a scaffolded host writes it. No `dts = ..`:
/// the divergence is between the host and the Teal file, and a declaration would only be
/// a third thing on the path.
fn host_rs(module: &str, ty: &str) -> String {
    format!(
        "pub struct {ty};\n\n#[host_module(name = \"{module}\")]\nimpl {ty} {{\n    \
         pub fn only_in_rust(&self) -> String {{ String::new() }}\n}}\n"
    )
}

const HOST_TL: &str = "local record host\nend\n\
                       function host.only_in_teal(): string\n   return \"\"\nend\n\
                       return host\n";
const MAIN_TL: &str = "local host = require(\"host\")\nprint(host.only_in_teal())\n";

/// The name is a host module and a Teal file both. Reported where the require is, naming
/// the module the host registers and the file the check read.
#[test]
fn a_teal_file_named_after_a_host_module_is_reported() {
    let root = scratch("shadowed");
    write(&root.join("Cargo.toml"), MANIFEST);
    write(&root.join("src/lib.rs"), &host_rs("host", "Host"));
    write(&root.join("src/host.tl"), HOST_TL);
    write(&root.join("src/main.tl"), MAIN_TL);

    let found = shadow_reports(&root, "src/main.tl");
    assert_eq!(found.len(), 1, "one report for one module: {found:?}");
    let d = &found[0];
    let msg = d["message"].as_str().unwrap_or_default();
    assert!(msg.contains("host"), "names the host module: {msg}");
    assert!(msg.contains("src/host.tl"), "names the Teal file: {msg}");
    // At the require, not at the top of the file and not at the host.
    assert_eq!(d["file"].as_str(), Some("src/main.tl"), "{d:?}");
    assert_eq!(d["line"].as_u64(), Some(1), "the require's line: {d:?}");
    assert_eq!(d["severity"].as_str(), Some("lint"), "{d:?}");
}

/// Take the Teal file away and the name resolves to the host alone; take the host away
/// and it resolves to the file alone. Either way there is no divergence to report.
#[test]
fn the_report_needs_both_halves() {
    let without_teal = scratch("host-only");
    write(&without_teal.join("Cargo.toml"), MANIFEST);
    write(&without_teal.join("src/lib.rs"), &host_rs("host", "Host"));
    write(
        &without_teal.join("src/host.d.tl"),
        "local record host\n   only_in_rust: function(): string\nend\nreturn host\n",
    );
    write(
        &without_teal.join("src/main.tl"),
        "local host = require(\"host\")\nprint(host.only_in_rust())\n",
    );
    assert!(
        shadow_reports(&without_teal, "src/main.tl").is_empty(),
        "a declaration is how a host module is typed, not a second implementation"
    );

    let without_host = scratch("teal-only");
    write(&without_host.join("Cargo.toml"), MANIFEST);
    write(&without_host.join("src/lib.rs"), "pub struct Host;\n");
    write(&without_host.join("src/host.tl"), HOST_TL);
    write(&without_host.join("src/main.tl"), MAIN_TL);
    assert!(
        shadow_reports(&without_host, "src/main.tl").is_empty(),
        "no host module of that name: nothing shadows anything"
    );
}

/// A crate whose host modules are named something else. The lint is about one name being
/// two things, not about a project having both a host and Teal files.
#[test]
fn a_teal_file_of_another_name_is_silent() {
    let root = scratch("different-names");
    write(&root.join("Cargo.toml"), MANIFEST);
    write(&root.join("src/lib.rs"), &host_rs("engine", "Engine"));
    write(
        &root.join("src/util.tl"),
        "local record util\nend\nfunction util.f(): string\n   return \"\"\nend\nreturn util\n",
    );
    write(
        &root.join("src/main.tl"),
        "local util = require(\"util\")\nprint(util.f())\n",
    );

    assert!(
        shadow_reports(&root, "src/main.tl").is_empty(),
        "engine is the host module, util is a Teal file"
    );
}

/// A script-only project: no crate around it, so no host, so nothing to scan and nothing
/// to say — including about a Teal module named `host`.
#[test]
fn a_project_with_no_rust_host_is_silent() {
    let root = scratch("no-host");
    write(&root.join("htl.toml"), "[lint]\n");
    write(&root.join("src/host.tl"), HOST_TL);
    write(&root.join("src/main.tl"), MAIN_TL);

    assert!(
        shadow_reports(&root, "src/main.tl").is_empty(),
        "no Cargo.toml above the project: no host modules exist"
    );
}
