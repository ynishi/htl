//! Every rule name htl prints is a name htl takes back.
//!
//! Five rules are implemented in the project layer rather than in `lint.lua`, and they name
//! themselves in output exactly like the twelve that are — `[htl contract]`, and `rule` in
//! `--format json`. Until the registry moved out of `L.DEFAULT` those names went one way
//! only: `htl check --lint -contract` answered `unknown lint rule: contract` about a name
//! it had just printed.
//!
//! The round trip below is the whole point of the registry, so it is tested as a round
//! trip: provoke the finding, read the name out of the JSON, and hand that same string back
//! three ways — `--lint`, a key of `[lint.rules]` in `htl.toml`, and `-- htl: allow(...)`
//! on the line the finding points at.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-rule-names", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// The diagnostics of `htl check <target>` run in `root`, as (rule, file, line).
fn diagnostics(root: &Path, target: &str, args: &[&str]) -> Vec<(String, String, u64)> {
    let out = Command::new(env!("CARGO_BIN_EXE_htl"))
        .args(["check", target, "--format", "json", "--no-cache"])
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| {
        panic!(
            "stdout is one JSON document ({e}): {text}\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    v["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .map(|d| {
            (
                d["rule"].as_str().unwrap_or_default().to_string(),
                d["file"].as_str().unwrap_or_default().to_string(),
                d["line"].as_u64().unwrap_or(0),
            )
        })
        .collect()
}

fn under(root: &Path, target: &str, rule: &str, args: &[&str]) -> Vec<(String, String, u64)> {
    diagnostics(root, target, args)
        .into_iter()
        .filter(|(r, _, _)| r == rule)
        .collect()
}

/// The ways a name comes back, run against a project that reports `rule` once.
///
/// `at_the_site` is the third of them, and one rule does not have it: see
/// `an_allow_comment_cannot_be_written_on_a_marker_line`.
fn round_trip(root: &Path, target: &str, rule: &str, at_the_site: bool) {
    let found = under(root, target, rule, &[]);
    assert_eq!(found.len(), 1, "{rule} reported once: {found:?}");

    // 1. The name off the command line.
    let off = under(root, target, rule, &["--lint", &format!("-{rule}")]);
    assert!(off.is_empty(), "--lint -{rule} left it: {off:?}");
    // Still on when the spec names something else, so the flag turned off this rule
    // rather than every rule of its kind.
    let other = under(root, target, rule, &["--lint", "-no-global"]);
    assert_eq!(other.len(), 1, "-no-global silenced {rule}: {other:?}");

    // 2. The name as a key of `[lint.rules]`, at `allow`.
    let cfg = root.join("htl.toml");
    let before = std::fs::read_to_string(&cfg).unwrap_or_default();
    write(
        &cfg,
        &format!("{before}\n[lint.rules]\n{rule:?} = \"allow\"\n"),
    );
    let off = under(root, target, rule, &[]);
    assert!(off.is_empty(), "[lint.rules] allow left {rule}: {off:?}");
    write(&cfg, &before);
    if !at_the_site {
        return;
    }

    // 3. The name in an allow comment on the line the finding points at.
    let found = under(root, target, rule, &[]);
    assert_eq!(found.len(), 1, "back on for 3: {found:?}");
    let (_, file, line) = found[0].clone();
    let at = root.join(&file);
    let src = std::fs::read_to_string(&at).unwrap_or_else(|e| panic!("reading {file}: {e}"));
    let mut lines: Vec<String> = src.lines().map(str::to_string).collect();
    let i = line as usize - 1;
    lines[i] = format!("{}  -- htl: allow({rule})", lines[i]);
    write(&at, &(lines.join("\n") + "\n"));
    let off = under(root, target, rule, &[]);
    assert!(
        off.is_empty(),
        "-- htl: allow({rule}) on {file}:{line} left it: {off:?}"
    );
    write(&at, &src);
}

// ---------------------------------------------------------------- the five fixtures

const MANIFEST: &str = "[package]\nname = \"p\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";
const DECL: &str = "local record xlib\n   connect: function(string): boolean\nend\nreturn xlib\n";

/// A contract nothing under its directory satisfies, and — with a crate around it and no
/// `contract_resolvers(` in its sources — a contract the host does not enforce.
fn contract_project(name: &str, with_crate: bool) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), "[[contract]]\ndir = \"mods\"\n");
    write(
        &root.join("src/defs.tl"),
        "local record defs\n   record Mod   ---@contract\n      name: string   ---@required\n   \
         end\nend\nreturn defs\n",
    );
    write(&root.join("mods/bad.tl"), "return { name = 1 }\n");
    if with_crate {
        write(&root.join("Cargo.toml"), MANIFEST);
        write(&root.join("src/lib.rs"), "fn nothing() {}\n");
    }
    root
}

#[test]
fn require_cycle_comes_back() {
    let root = scratch("cycle");
    write(
        &root.join("src/a.tl"),
        "local b = require(\"b\")\nreturn { b = b }\n",
    );
    write(
        &root.join("src/b.tl"),
        "local a = require(\"a\")\nreturn { a = a }\n",
    );
    write(&root.join("htl.toml"), "[check]\npaths = [\"src\"]\n");
    round_trip(&root, "src", "require-cycle", true);
}

#[test]
fn duplicate_declaration_comes_back() {
    let root = scratch("duplicate");
    write(&root.join("htl.toml"), "[check]\npaths = [\"sdk\"]\n");
    write(&root.join("types/xlib.d.tl"), DECL);
    write(&root.join("sdk/xlib.d.tl"), DECL);
    write(
        &root.join("src/use.tl"),
        "local xlib = require(\"xlib\")\nprint(xlib.connect(\"h\"))\n",
    );
    round_trip(&root, "src/use.tl", "duplicate-declaration", true);
}

#[test]
fn host_module_shadowed_comes_back() {
    let root = scratch("shadowed");
    write(&root.join("Cargo.toml"), MANIFEST);
    write(
        &root.join("src/lib.rs"),
        "pub struct Host;\n\n#[host_module(name = \"host\")]\nimpl Host {\n    \
         pub fn only_in_rust(&self) -> String { String::new() }\n}\n",
    );
    write(
        &root.join("src/host.tl"),
        "local record host\nend\nfunction host.only_in_teal(): string\n   return \"\"\nend\n\
         return host\n",
    );
    write(
        &root.join("src/main.tl"),
        "local host = require(\"host\")\nprint(host.only_in_teal())\n",
    );
    round_trip(&root, "src/main.tl", "host-module-shadowed", true);
}

#[test]
fn contract_comes_back() {
    let root = contract_project("contract", false);
    round_trip(&root, "mods/bad.tl", "contract", true);
}

#[test]
fn contract_unenforced_comes_back() {
    let root = contract_project("unenforced", true);
    // Reported for the run rather than for a file; the file named is the one being checked
    // only because a run has to check something. No site suppression — the test below.
    round_trip(&root, "src/defs.tl", "contract-unenforced", false);
}

/// `contract-unenforced` points at the `---@contract` marker, and a marker owns the rest
/// of its line: `-- htl: allow(contract-unenforced)` written there is read as an argument
/// to the marker and reported as a malformed one, which is worse than not being silenced.
/// So that rule is turned off by name (`[lint.rules]`, `--lint`) or answered with
/// `[[contract]] enforced_by`, and it is the one rule of the five with no site switch.
#[test]
fn an_allow_comment_cannot_be_written_on_a_marker_line() {
    let root = contract_project("marker-line", true);
    let defs = root.join("src/defs.tl");
    let src = std::fs::read_to_string(&defs).unwrap();
    write(
        &defs,
        &src.replace(
            "---@contract",
            "---@contract  -- htl: allow(contract-unenforced)",
        ),
    );
    let out = Command::new(env!("CARGO_BIN_EXE_htl"))
        .args(["check", "src/defs.tl", "--no-cache"])
        .current_dir(&root)
        .output()
        .unwrap();
    let said =
        String::from_utf8_lossy(&out.stderr).into_owned() + &String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains("---@contract takes no arguments") && said.contains("[htl contract]"),
        "the comment is read as an argument to the marker: {said}"
    );
}

/// The names are in `--list-lints` as well, which is where a reader looks for them without
/// having to provoke one first. Sixteen was the count while the project layer's five were
/// absent and `lint.lua`'s twelve were all there was; seventeen once they were registered,
/// and twenty-four now that Teal's seven warning kinds are names too.
///
/// Each line is the name and the level a project that says nothing gets, so the listing
/// also answers which rules are `allow` — which a reader used to have to turn a rule on to
/// find out.
#[test]
fn the_listing_accounts_for_every_rule() {
    let dir = scratch("listing");
    let out = Command::new(env!("CARGO_BIN_EXE_htl"))
        .args(["check", "--list-lints"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let lines: Vec<(String, String)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_once("  "))
        .map(|(name, level)| (name.trim().to_string(), level.trim().to_string()))
        .collect();
    let listed: Vec<String> = lines.iter().map(|(name, _)| name.clone()).collect();
    let allow: Vec<&str> = lines
        .iter()
        .filter(|(_, level)| level == "allow")
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(
        allow,
        ["no-any", "explicit-number", "class-record"],
        "the three opinions, and nothing else, is what a project does not get by default"
    );
    assert!(
        lines.iter().all(|(_, l)| l == "allow" || l == "warn"),
        "nothing defaults to deny: {lines:?}"
    );
    for rule in [
        "nil-index",
        "class-record",
        "require-cycle",
        "duplicate-declaration",
        "host-module-shadowed",
        "contract",
        "contract-unenforced",
        "tl:hint",
        "tl:redeclaration",
    ] {
        assert!(listed.iter().any(|l| l == rule), "{rule} not in {listed:?}");
    }
    assert_eq!(listed.len(), 24, "{listed:?}");
}

/// A name that is not a rule is still refused, and says which word it did not know.
#[test]
fn an_unknown_name_is_still_an_error() {
    let dir = scratch("unknown");
    write(&dir.join("src/a.tl"), "return {}\n");
    let out = Command::new(env!("CARGO_BIN_EXE_htl"))
        .args(["check", "src", "--no-cache", "--lint", "-contrct"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("unknown lint rule"), "{err}");
    assert!(err.contains("contrct"), "{err}");
    assert!(!out.status.success(), "{err}");
}
