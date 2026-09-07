//! Two declarations of one module, both reachable.
//!
//! Teal resolves a module by walking `package.path` and taking the first hit, and htl
//! keeps that: a `.tl` source is tried across the whole path before any `.d.tl`, but the
//! tie between two declarations is decided by position alone. Position is not something
//! anyone wrote down, so the `duplicate-declaration` lint says which one was read and
//! which one was not.

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "htl-cli-decl-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Every diagnostic `htl check <target>` printed, run from `root`, as
/// `<rule or kind>: <message>` — the rule is its own field in the JSON, split out of the
/// message, so the message alone never names it.
fn messages(root: &Path, target: &str) -> Vec<String> {
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
        .iter()
        .map(|d| {
            let rule = d["rule"]
                .as_str()
                .or_else(|| d["kind"].as_str())
                .unwrap_or("");
            format!("{rule}: {}", d["message"].as_str().unwrap_or(""))
        })
        .collect()
}

const DECL: &str = "local record xlib\n   connect: function(string): boolean\nend\nreturn xlib\n";
const USE: &str = "local xlib = require(\"xlib\")\nprint(xlib.connect(\"h\"))\n";

/// A host publishes `xlib.d.tl` into the project, and the project also keeps a
/// hand-written one under `types/`. Both are on the path; one is read.
#[test]
fn two_declarations_of_one_module_are_reported() {
    let root = scratch("two-decls");
    write(&root.join("htl.toml"), "[check]\npaths = [\"sdk\"]\n");
    write(&root.join("types/xlib.d.tl"), DECL);
    write(&root.join("sdk/xlib.d.tl"), DECL);
    write(&root.join("src/use.tl"), USE);

    let msgs = messages(&root, "src/use.tl");
    let found: Vec<&String> = msgs
        .iter()
        .filter(|m| m.contains("duplicate-declaration"))
        .collect();
    assert_eq!(found.len(), 1, "one report for one module: {msgs:?}");
    let m = found[0];
    assert!(m.contains("types/xlib.d.tl"), "names the one read: {m}");
    assert!(m.contains("sdk/xlib.d.tl"), "names the shadowed one: {m}");
    assert!(m.contains("is read"), "says which is which: {m}");
}

/// A source beats every declaration wherever the two sit, so the position that decides
/// between declarations never comes up: nothing to report.
#[test]
fn a_source_alongside_a_declaration_is_not_a_collision() {
    let root = scratch("source-wins");
    write(&root.join("htl.toml"), "[check]\npaths = [\"sdk\"]\n");
    write(
        &root.join("types/xlib.d.tl"),
        "local record xlib\n   connect: function(string): boolean\nend\nreturn xlib\n",
    );
    write(
        &root.join("sdk/xlib.tl"),
        "local record xlib\nend\nfunction xlib.connect(_: string): boolean\n   return true\nend\nreturn xlib\n",
    );
    write(&root.join("src/use.tl"), USE);

    let msgs = messages(&root, "src/use.tl");
    assert!(
        !msgs.iter().any(|m| m.contains("duplicate-declaration")),
        "a source is not a second declaration: {msgs:?}"
    );
}

/// One declaration, reachable once. The lint has nothing to say, and says nothing.
#[test]
fn a_single_declaration_is_silent() {
    let root = scratch("single");
    write(&root.join("htl.toml"), "[lint]\n");
    write(&root.join("types/xlib.d.tl"), DECL);
    write(&root.join("src/use.tl"), USE);

    let msgs = messages(&root, "src/use.tl");
    assert!(
        !msgs.iter().any(|m| m.contains("duplicate-declaration")),
        "nothing to report: {msgs:?}"
    );
}
