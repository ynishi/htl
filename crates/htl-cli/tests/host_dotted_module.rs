//! A dotted `#[host_module(name = "..")]` in a project: the project model resolves
//! `require("ns.inner")` through the same machinery any host module name goes through
//! (`#[host_module]` in the crate around the project), with the declaration at the
//! dotted name's own path under the declaration root (`types/ns/inner.d.tl`, `[layout]
//! types`'s default) — the same shape `host_provided.rs`'s undotted `host` is typed by,
//! at `src/host.d.tl`. See `htl_macros::host_module` for why the declaration's record is
//! `inner` while `require` and this file's path both read the whole `ns.inner`.

use std::path::Path;
use std::process::{Command, Output};

mod common;

fn tempdir(name: &str) -> common::TempDir {
    common::tempdir("htl-cli-host-dotted-module", name)
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

/// The host crate: a `#[host_module]` whose `name` is dotted and whose `dts` points at
/// the declaration root (`types/`), segment for segment with the name.
const LIB_RS: &str = "use htl::host_module;\n\npub struct Inner;\n\n\
                      #[host_module(name = \"ns.inner\", dts = \"types/ns/inner.d.tl\")]\n\
                      impl Inner {\n    pub fn greet(&self, who: &str) -> String {\n        \
                      format!(\"hello {who}\")\n    }\n}\n";

/// `types/ns/inner.d.tl` as the check writes it from `LIB_RS` — written here too, so the
/// test does not depend on whether the check regenerates it (`host_provided.rs`'s
/// `HOST_DTL` does the same for its undotted case).
const INNER_DTL: &str = "local record inner\n   greet: function(self: inner, who: string): string\nend\n\nreturn inner\n";

const MAIN_TL: &str = "local inner = require(\"ns.inner\")\nprint(inner:greet(\"x\"))\n";

/// A project around a host crate whose one `#[host_module]` has a dotted name.
fn dotted_project(name: &str) -> common::TempDir {
    let root = tempdir(name);
    write(&root.join("htl.toml"), "");
    write(&root.join("Cargo.toml"), MANIFEST);
    write(&root.join("src/lib.rs"), LIB_RS);
    write(&root.join("types/ns/inner.d.tl"), INNER_DTL);
    write(&root.join("src/main.tl"), MAIN_TL);
    root
}

/// The declaration at the dotted name's own path types the require: a correct call
/// checks clean, same as `host_provided.rs::a_host_module_typed_by_its_declaration_checks`.
#[test]
fn a_dotted_host_module_typed_by_its_declaration_checks() {
    let root = dotted_project("declared");
    let (ok, diags) = check(&root);
    assert!(ok, "{diags:#?}");
    assert!(diags.is_empty(), "{diags:#?}");
}

/// Misusing the declared return type is a check error read from the dotted name's
/// declaration, the same as any other host module's: `greet` returns `string`, assigned
/// to an `integer`.
#[test]
fn misusing_the_declared_return_type_is_a_check_error() {
    let root = dotted_project("misuse");
    write(
        &root.join("src/main.tl"),
        "local inner = require(\"ns.inner\")\n\
         local n: integer = inner:greet(\"x\")\n\
         print(n)\n",
    );
    let (ok, diags) = check(&root);
    assert!(!ok, "{diags:#?}");
    assert!(
        diags
            .iter()
            .any(|d| message(d).contains("got string, expected integer")),
        "{diags:#?}"
    );
}

/// `htl resolve` names the host crate and the declaration's path under the declaration
/// root — `types/ns/inner.d.tl`, the dotted name's own path — and says the project model
/// answered it, the same report shape an undotted host module gets
/// (`host_provided.rs::resolve_says_the_host_provides_a_name_and_refuses_a_file_under_it`).
#[test]
fn resolve_names_the_host_and_the_dotted_declarations_path() {
    let root = dotted_project("resolve");
    let out = htl(&root, &["resolve", "ns.inner"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(
        stdout.starts_with(
            "htl resolve ns.inner: provided by the host (#[host_module] in Cargo.toml's \
             crate), typed by types/ns/inner.d.tl\n"
        ),
        "{stdout}"
    );
    assert!(stdout.contains("answered by the project model"), "{stdout}");
}
