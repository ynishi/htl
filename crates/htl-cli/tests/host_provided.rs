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

// ------------------------------------------------------------------ build, resolve, unused
//
// The linker, `htl resolve` and `htl unused` read the host's names from the same model:
// nothing has to restate a `#[host_module]` in `[build] host` or `--host`.

/// The shadowing message for `file`, as the resolver words it for the crate's
/// `#[host_module(name = "host")]`.
fn host_message(file: &str) -> String {
    format!(
        "'host' is provided by the host (#[host_module] in Cargo.toml's crate) and also \
         implemented by {file}: the host's module is what runs, so this file would be \
         checked and never run — rename it, or stop providing the name"
    )
}

/// `htl build` with no `--host` and no `[build] host`: the `#[host_module]` name is a host
/// module of the bundle, not a module in it.
#[test]
fn build_leaves_a_host_module_name_to_the_host_without_it_being_listed() {
    let root = host_project("build");
    let out = htl(&root, &["build", "src/main.tl", "-o", "out.hb"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");

    let info = htl(&root, &["bundle", "info", "out.hb", "--format", "json"]);
    assert!(info.status.success());
    let v: serde_json::Value = serde_json::from_slice(&info.stdout).unwrap();
    assert_eq!(v["host_modules"], serde_json::json!(["host"]), "{v:#}");
    let modules: Vec<&str> = v["modules"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["name"].as_str())
        .collect();
    assert_eq!(modules, vec!["main"], "{v:#}");
}

/// A plain `.lua` the check never reads, requiring a name the host provides that a file
/// of the project implements: the linker refuses it at that `require`, and writes nothing.
#[test]
fn build_refuses_a_plain_lua_require_of_a_shadowed_host_name() {
    let root = host_project("build-shadowed");
    write(&root.join("src/host.lua"), "return {}\n");
    write(
        &root.join("src/helper.d.tl"),
        "local record helper\n   n: integer\nend\nreturn helper\n",
    );
    write(
        &root.join("src/helper.lua"),
        "local host = require(\"host\")\nreturn { n = 1, host = host }\n",
    );
    write(
        &root.join("src/main.tl"),
        "local helper = require(\"helper\")\nprint(helper.n)\n",
    );
    let out = htl(&root, &["build", "src/main.tl", "-o", "out.hb"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    let line = stderr
        .lines()
        .find(|l| l.contains(&host_message("src/host.lua")))
        .unwrap_or_else(|| panic!("the resolver's message: {stderr}"));
    assert!(
        line.contains("src/helper.lua:1:21:"),
        "at helper.lua's require: {line}"
    );
    assert_eq!(
        stderr.matches(&host_message("src/host.lua")).count(),
        1,
        "said once: {stderr}"
    );
    assert!(!root.join("out.hb").exists(), "no bundle written");
}

/// `htl resolve` names the source a provided name comes from, and succeeds; with a file
/// under the name, the header is the error, the file's row is `refused`, and it fails.
#[test]
fn resolve_says_the_host_provides_a_name_and_refuses_a_file_under_it() {
    let root = host_project("resolve");
    let out = htl(&root, &["resolve", "host"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(
        stdout.starts_with(
            "htl resolve host: provided by the host (#[host_module] in Cargo.toml's crate), \
             typed by src/host.d.tl\n"
        ),
        "{stdout}"
    );
    let out = htl(&root, &["resolve", "host", "--format", "json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v["provided_by"].as_str(),
        Some("#[host_module] in Cargo.toml's crate"),
        "{v:#}"
    );
    assert!(v.get("error").is_none(), "{v:#}");
    assert_eq!(v["summary"]["ok"], serde_json::json!(true), "{v:#}");

    write(
        &root.join("src/host.tl"),
        "local record host\nend\nreturn host\n",
    );
    let out = htl(&root, &["resolve", "host"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.starts_with(&format!(
            "htl resolve host: error: {}\n",
            host_message("src/host.tl")
        )),
        "{stdout}"
    );
    let row = |file: &str| {
        stdout
            .lines()
            .find(|l| l.contains(file))
            .unwrap_or_default()
            .to_string()
    };
    assert!(
        row("src/host.tl ").trim_end().ends_with("refused"),
        "{stdout}"
    );
    assert!(
        row("src/host.d.tl").trim_end().ends_with("read"),
        "{stdout}"
    );

    let out = htl(&root, &["resolve", "host", "--format", "json"]);
    assert_eq!(out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v["error"].as_str(),
        Some(host_message("src/host.tl").as_str())
    );
    assert_eq!(v["read"].as_str(), Some("src/host.d.tl"), "{v:#}");
    let status = |path: &str| {
        v["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["path"].as_str() == Some(path))
            .map(|c| c["status"].clone())
    };
    assert_eq!(status("src/host.tl"), Some(serde_json::json!("refused")));
    assert_eq!(status("src/host.d.tl"), Some(serde_json::json!("read")));
    assert_eq!(v["summary"]["ok"], serde_json::json!(false), "{v:#}");
}

/// `htl unused` counts a `#[host_module]` name as the host's, as it does a `[build] host`
/// one: the file under it is the check's error, not a module nothing reaches.
#[test]
fn unused_counts_a_host_module_name_as_the_hosts() {
    let root = host_project("unused");
    write(
        &root.join("src/host.tl"),
        "local record host\nend\nreturn host\n",
    );
    write(&root.join("src/orphan.tl"), "return {}\n");
    let out = htl(&root, &["unused"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(!stderr.contains("src/host.tl"), "{stderr}");
    assert!(stderr.contains("module: src/orphan.tl"), "{stderr}");
}

// ------------------------------------------------ a name the environment provides

/// A LuaSocket-shaped declaration, hand-written for a library installed on the machine.
const HTTP_DTL: &str =
    "local record http\n   request: function(url: string): string\nend\n\nreturn http\n";

/// A project with no host crate and no `[build] host`, whose `src/main.tl` requires
/// `socket.http`, declared by `decl` (a path under the project) and implemented by nothing.
fn declared_project(name: &str, decl: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), "");
    write(&root.join(decl), HTTP_DTL);
    write(
        &root.join("src/main.tl"),
        "local http = require(\"socket.http\")\nprint(http.request(\"x\"))\n",
    );
    root
}

/// The model's answer for a name only a declaration has, through every command that asks
/// it: the check types the `require` from the declaration, the bundle files the name with
/// the host's, and `htl resolve` says the environment provides it and which declaration
/// says so — with no configuration beside the `.d.tl`.
fn assert_provided_by_the_environment(root: &Path, decl: &str) {
    let (ok, diags) = check(root);
    assert!(ok, "{diags:#?}");

    let out = htl(root, &["build", "src/main.tl", "-o", "out.hb"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let info = htl(root, &["bundle", "info", "out.hb", "--format", "json"]);
    assert!(info.status.success());
    let v: serde_json::Value = serde_json::from_slice(&info.stdout).unwrap();
    assert_eq!(
        v["host_modules"],
        serde_json::json!(["socket.http"]),
        "{v:#}"
    );
    let modules: Vec<&str> = v["modules"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["name"].as_str())
        .collect();
    assert_eq!(modules, vec!["main"], "not bundled: {v:#}");

    let out = htl(root, &["resolve", "socket.http"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(
        stdout.starts_with(&format!(
            "htl resolve socket.http: {decl}, provided by the environment (declared by {decl})\n"
        )),
        "{stdout}"
    );
    let out = htl(root, &["resolve", "socket.http", "--format", "json"]);
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v["provided_by"].as_str(),
        Some(format!("declared by {decl}").as_str()),
        "{v:#}"
    );
    assert_eq!(v["read"].as_str(), Some(decl), "{v:#}");
    assert!(v.get("error").is_none(), "{v:#}");
    assert_eq!(v["summary"]["ok"], serde_json::json!(true), "{v:#}");
}

/// A hand-written declaration in the declaration root, for a library installed on the
/// machine: the environment provides the name.
#[test]
fn a_declaration_in_the_types_root_is_provided_by_the_environment() {
    let root = declared_project("declared-types", "types/socket/http.d.tl");
    assert_provided_by_the_environment(&root, "types/socket/http.d.tl");
}

/// The same declaration beside the sources, with no implementation: the same answer.
/// Where the declaration sits is not what makes the name the environment's.
#[test]
fn a_declaration_beside_the_sources_is_provided_by_the_environment() {
    let root = declared_project("declared-src", "src/socket/http.d.tl");
    assert_provided_by_the_environment(&root, "src/socket/http.d.tl");
}
