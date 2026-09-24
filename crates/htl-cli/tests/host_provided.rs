//! A name the host provides, in a project: typed by its declaration, and never
//! implemented by a file of the project.
//!
//! The host puts its modules in `package.preload`, which Lua consults before any searcher,
//! so a `src/host.tl` or `src/host.lua` under a name the host provides is what the check
//! would read and what a run without the host would load — and never what a run with the
//! host runs. The project model knows which names the host provides (`#[host_module]` in
//! the crate around the project, `[build] host`, `std.*`), and its resolver makes such a
//! file an error at every `require` of the name, in `htl check` and at run time alike. A
//! `.d.tl` of the name is how the host module gets its types, and stays what the check
//! reads.
//!
//! The same state in a file that belongs to no project is `host_module_shadow.rs`'s: no
//! model, so the `host-module-shadowed` lint.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-host-provided", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn htl(root: &Path, args: &[&str]) -> Output {
    Command::new(common::htl_bin())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap()
}

/// `htl check src --format json --no-cache` in `root`: whether it passed, and every
/// diagnostic as its JSON object.
fn check(root: &Path) -> (bool, Vec<serde_json::Value>) {
    let out = htl(root, &["check", "src", "--format", "json", "--no-cache"]);
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("stdout is one JSON document");
    let diags = v["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .to_vec();
    (out.status.success(), diags)
}

fn message(d: &serde_json::Value) -> &str {
    d["message"].as_str().unwrap_or_default()
}

const MANIFEST: &str = "[package]\nname = \"p\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";

/// The host crate, shaped the way `htl new --embed` scaffolds one: a `#[host_module]`
/// registering `host`, whose declaration goes to `src/host.d.tl`. The scan reads the
/// source with syn; the crate is never compiled.
const LIB_RS: &str = "use htl::host_module;\n\npub struct Host;\n\n\
                      #[host_module(name = \"host\", dts = \"src/host.d.tl\")]\n\
                      impl Host {\n    pub fn greet(&self, who: &str) -> String {\n        \
                      format!(\"hello {who}\")\n    }\n}\n";

/// `src/host.d.tl` as the check writes it from `LIB_RS` — written here too, so the test
/// does not depend on whether the check regenerates it.
const HOST_DTL: &str =
    "local record host\n   greet: function(self: host, who: string): string\nend\n\nreturn host\n";

const MAIN_TL: &str = "local host = require(\"host\")\nprint(host:greet(\"x\"))\n";

/// A project around a host crate that registers `host`, typed by its declaration.
fn host_project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), "");
    write(&root.join("Cargo.toml"), MANIFEST);
    write(&root.join("src/lib.rs"), LIB_RS);
    write(&root.join("src/host.d.tl"), HOST_DTL);
    write(&root.join("src/main.tl"), MAIN_TL);
    root
}

/// The error, at the one `require` of the name, and nothing Teal or the lint would have
/// said about the same `require` beside it.
fn assert_shadowed(diags: &[serde_json::Value], name: &str, from: &str, file: &str) {
    let expected = format!(
        "'{name}' is provided by the host ({from}) and also implemented by {file}: the host's \
         module is what runs, so this file would be checked and never run — rename it, or \
         stop providing the name"
    );
    let errors: Vec<&serde_json::Value> = diags.iter().filter(|d| message(d) == expected).collect();
    assert_eq!(errors.len(), 1, "one error at the require: {diags:#?}");
    let d = errors[0];
    assert_eq!(d["severity"].as_str(), Some("error"), "{d:?}");
    assert!(
        d["file"]
            .as_str()
            .unwrap_or_default()
            .ends_with("src/main.tl"),
        "at the requiring file: {d:?}"
    );
    assert_eq!(d["line"].as_u64(), Some(1), "the require's line: {d:?}");
    assert!(
        !diags
            .iter()
            .any(|d| message(d).contains("module not found")),
        "the error is instead of Teal's `module not found`: {diags:#?}"
    );
    assert!(
        !diags
            .iter()
            .any(|d| d["rule"].as_str() == Some("host-module-shadowed")),
        "the error replaces the lint: {diags:#?}"
    );
}

/// The host module and its declaration, and no file of the project under the name: the
/// declaration types the require, and there is nothing to report.
#[test]
fn a_host_module_typed_by_its_declaration_checks() {
    let root = host_project("declared");
    let (ok, diags) = check(&root);
    assert!(ok, "{diags:#?}");
    assert!(diags.is_empty(), "{diags:#?}");
}

/// A `.tl` of the name beside the declaration: an error at the require, typed from the
/// declaration all the same (no second error for the method the file lacks or has).
#[test]
fn a_teal_file_under_a_host_module_name_is_an_error() {
    let root = host_project("teal");
    write(
        &root.join("src/host.tl"),
        "local record host\nend\nfunction host.only_in_teal(): string\n   return \"\"\nend\n\
         return host\n",
    );
    let (ok, diags) = check(&root);
    assert!(!ok, "{diags:#?}");
    assert_shadowed(
        &diags,
        "host",
        "#[host_module] in Cargo.toml's crate",
        "src/host.tl",
    );
    assert!(
        diags.iter().all(|d| !message(d).contains("greet")),
        "the require is typed from the declaration, which has greet: {diags:#?}"
    );
}

/// A plain `.lua` of the name: the same error in the check, and `htl run` refuses it —
/// through the check before the run, and at run time for a `require` the check could not
/// follow, where the searcher would otherwise have served the file.
#[test]
fn a_lua_file_under_a_host_module_name_is_an_error_in_the_check_and_the_run() {
    let root = host_project("lua");
    write(
        &root.join("src/host.lua"),
        "return { greet = function(_, who) return \"lua \" .. who end }\n",
    );
    let (ok, diags) = check(&root);
    assert!(!ok, "{diags:#?}");
    assert_shadowed(
        &diags,
        "host",
        "#[host_module] in Cargo.toml's crate",
        "src/host.lua",
    );

    let expected = "'host' is provided by the host (#[host_module] in Cargo.toml's crate) \
                    and also implemented by src/host.lua";
    let run = htl(&root, &["run", "src/main.tl"]);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(!run.status.success(), "{stderr}");
    assert!(stderr.contains(expected), "{stderr}");
    assert!(
        !String::from_utf8_lossy(&run.stdout).contains("lua x"),
        "the file never ran"
    );

    // A name the check cannot see: the run-time searcher asks the model and raises.
    write(
        &root.join("src/dynamic.tl"),
        "local name = \"ho\" .. \"st\"\nlocal m = require(name) as {string:any}\nprint(m)\n",
    );
    let run = htl(&root, &["run", "src/dynamic.tl"]);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(!run.status.success(), "{stderr}");
    assert!(stderr.contains(expected), "raised at run time: {stderr}");
}

/// No declaration at all: the error still stands at the require, and is the only thing
/// said there — Teal's `module not found` for the name is what it replaces.
#[test]
fn a_host_module_with_no_declaration_is_reported_once() {
    let root = host_project("undeclared");
    write(
        &root.join("src/lib.rs"),
        &LIB_RS.replace(", dts = \"src/host.d.tl\"", ""),
    );
    std::fs::remove_file(root.join("src/host.d.tl")).unwrap();
    write(
        &root.join("src/host.tl"),
        "local record host\nend\nfunction host.greet(_: host, who: string): string\n   \
         return who\nend\nreturn host\n",
    );
    let (ok, diags) = check(&root);
    assert!(!ok, "{diags:#?}");
    assert_shadowed(
        &diags,
        "host",
        "#[host_module] in Cargo.toml's crate",
        "src/host.tl",
    );
}

/// `[build] host` names a module the host provides as much as a `#[host_module]` does, and
/// the error says which of the two the model read.
#[test]
fn a_file_under_a_build_host_name_is_an_error() {
    let root = scratch("build-host");
    write(&root.join("htl.toml"), "[build]\nhost = [\"game\"]\n");
    write(
        &root.join("src/game.d.tl"),
        "local record game\n   n: integer\nend\nreturn game\n",
    );
    write(
        &root.join("src/main.tl"),
        "local game = require(\"game\")\nprint(game.n)\n",
    );
    let (ok, diags) = check(&root);
    assert!(ok, "the declaration alone: {diags:#?}");

    write(
        &root.join("src/game.tl"),
        "local record game\n   n: integer\nend\ngame.n = 1\nreturn game\n",
    );
    let (ok, diags) = check(&root);
    assert!(!ok, "{diags:#?}");
    assert_shadowed(&diags, "game", "[build] host in htl.toml", "src/game.tl");
}
