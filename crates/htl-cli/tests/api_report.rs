//! `htl api`: the project's public Teal surface as one file.
//!
//! What is asked of the report is that it is *stable* — two runs over one tree write the
//! same bytes, and a change to the source moves the lines that changed and no others —
//! because that is the whole of its value. A report that reorders itself makes every diff
//! unreadable, and a reviewer stops reading it.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-api", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn htl(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_htl"))
        .args(args)
        .current_dir(root)
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// A project with all three surfaces: an `mlua-pkg.toml` entry a consumer requires, a
/// Rust host that publishes a `.d.tl`, and a `---@contract` type outside authors write
/// modules against. Nothing here is built — the Rust source is read, not compiled.
fn project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nentry = \"src/demo\"\n",
    );
    write(&root.join("htl.toml"), "[[contract]]\ndir = \"mods\"\n");
    write(
        &root.join("src/demo/init.tl"),
        "local record demo\n   record Point\n      x: number\n      y: number\n   end\nend\n\n\
         function demo.origin(): demo.Point\n   return { x = 0, y = 0 }\nend\n\nreturn demo\n",
    );
    write(
        &root.join("src/defs.tl"),
        "local record defs\n   record Mod   ---@contract\n      name: string   ---@required\n   \
         end\nend\n\nreturn defs\n",
    );
    write(
        &root.join("Cargo.toml"),
        "[package]\nname = \"demo-host\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(
        &root.join("src/lib.rs"),
        "pub struct Host;\n\n\
         #[host_module(name = \"host\", dts = \"types/host.d.tl\")]\n\
         impl Host {\n    pub fn add(&self, a: i64, b: i64) -> i64 { a + b }\n}\n",
    );
    root
}

/// Every surface is named, and each is named as the thing a consumer types.
#[test]
fn names_the_three_surfaces() {
    let root = project("surfaces");
    let out = htl(&root, &["api"]);
    let text = stdout(&out);
    assert!(
        text.contains("package demo  require(\"demo\")  src/demo/init.tl"),
        "{text}"
    );
    assert!(text.contains("host module host  types/host.d.tl"), "{text}");
    assert!(
        text.contains("contract defs.Mod  types/defs.d.tl"),
        "{text}"
    );
    // The members, not only the headers: the report is what a diff is read from.
    assert!(text.contains("origin: function(): demo.Point"), "{text}");
    assert!(
        text.contains("add: function(self: host, a: integer, b: integer): integer"),
        "{text}"
    );
    assert!(text.contains("name: string   ---@required"), "{text}");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A dependency's declarations describe somebody else's crate, so they are not part of
/// this project's surface — even though they sit in the same `types/` directory.
#[test]
fn a_dependencys_declarations_are_not_this_projects_surface() {
    let root = project("dependency");
    write(
        &root.join("types/othercrate/mq.d.tl"),
        "local record mq\n   send: function(s: string)\nend\n\nreturn mq\n",
    );
    let text = stdout(&htl(&root, &["api"]));
    assert!(!text.contains("mq"), "{text}");
}

/// Two runs with no source change write the same bytes.
#[test]
fn two_runs_agree() {
    let root = project("determinism");
    let first = stdout(&htl(&root, &["api"]));
    let second = stdout(&htl(&root, &["api"]));
    assert_eq!(first, second);
    assert!(!first.is_empty());
}

/// A renamed field, an added function and a changed type each move the lines they should
/// and no others.
#[test]
fn a_change_moves_its_own_lines() {
    let root = project("diff");
    let before = stdout(&htl(&root, &["api"]));

    write(
        &root.join("src/demo/init.tl"),
        "local record demo\n   record Point\n      x: number\n      z: number\n   end\nend\n\n\
         function demo.origin(): demo.Point\n   return { x = 0, z = 0 }\nend\n\n\
         function demo.count(): integer\n   return 1\nend\n\nreturn demo\n",
    );
    let after = stdout(&htl(&root, &["api"]));

    let gone: Vec<&str> = before.lines().filter(|l| !after.contains(*l)).collect();
    let new: Vec<&str> = after.lines().filter(|l| !before.contains(*l)).collect();
    assert_eq!(gone, vec!["        y: number"], "{before}");
    assert_eq!(
        new,
        vec!["        z: number", "     count: function(): integer"],
        "{after}"
    );
}

/// A changed parameter type moves the one line that declares it.
#[test]
fn a_changed_type_moves_one_line() {
    let root = project("retype");
    let before = stdout(&htl(&root, &["api"]));
    write(
        &root.join("src/lib.rs"),
        "pub struct Host;\n\n\
         #[host_module(name = \"host\", dts = \"types/host.d.tl\")]\n\
         impl Host {\n    pub fn add(&self, a: i64, b: f64) -> i64 { a + b as i64 }\n}\n",
    );
    let after = stdout(&htl(&root, &["api"]));
    let gone: Vec<&str> = before.lines().filter(|l| !after.contains(*l)).collect();
    assert_eq!(
        gone,
        vec!["     add: function(self: host, a: integer, b: integer): integer"],
        "{before}"
    );
    assert!(
        after.contains("add: function(self: host, a: integer, b: number): integer"),
        "{after}"
    );
}

/// A project with nothing public says that, rather than writing an empty file that could
/// as easily mean the command broke.
#[test]
fn no_surface_says_so() {
    let root = scratch("empty");
    write(&root.join("htl.toml"), "[lint]\n");
    write(&root.join("src/main.tl"), "print(\"hi\")\n");
    let out = htl(&root, &["api"]);
    let text = stdout(&out);
    assert!(
        text.contains("nothing: this project publishes no"),
        "{text}"
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `--out` writes the file, and says what it did on stderr — the report itself stays the
/// only thing on stdout.
#[test]
fn out_writes_the_file() {
    let root = project("out");
    let out = htl(&root, &["api", "--out", "api.txt"]);
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
    let text = std::fs::read_to_string(root.join("api.txt")).unwrap();
    assert!(text.starts_with("# htl api 1\n"), "{text}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("3 entries written to api.txt"), "{err}");
    // A second run over an unchanged tree leaves the file alone.
    let again = htl(&root, &["api", "--out", "api.txt"]);
    assert!(
        String::from_utf8_lossy(&again.stderr).contains("unchanged in api.txt"),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
}

#[test]
fn json_carries_the_same_entries() {
    let root = project("json");
    let text = stdout(&htl(&root, &["api", "--format", "json"]));
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 1);
    let kinds: Vec<&str> = v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["package", "host-module", "contract"]);
}

/// A contract that cannot be published is part of the surface this report could not
/// read: it is said, and the exit code carries it.
#[test]
fn an_unreadable_surface_is_not_silent() {
    let root = scratch("unpublishable");
    write(&root.join("htl.toml"), "[[contract]]\ndir = \"mods\"\n");
    write(
        &root.join("src/defs.tl"),
        "local record defs\n   record Mod   ---@contract\n      name: string   ---@required\n   \
         end\nend\n\nfunction other.helper(n: integer): integer\n   return n\nend\n\nreturn defs\n",
    );
    let out = htl(&root, &["api"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("nothing declares a record other"), "{err}");
    assert!(!out.status.success(), "{err}");
}
