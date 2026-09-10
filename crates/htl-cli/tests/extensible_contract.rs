//! `---@extensible` at a contract directory: the case the marker exists for.
//!
//! A `[[contract]]` directory is where a project's declaration and the modules written
//! against it move at different speeds. `---@required` handles one direction of that skew
//! — a mod written before a field existed still checks. This is the other one: a mod
//! written against a newer SDK sets a key the declaration has not heard of yet, and
//! without the marker `htl check` refuses it the same way it refuses a mod that is short.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-extensible", name)
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

fn output(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr)
}

/// A project with a contract directory and a mod that sets every field the contract asks
/// for and one key beside them. `marker` is what the record carries.
fn project(name: &str, marker: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), "[[contract]]\ndir = \"mods\"\n");
    write(
        &root.join("src/defs.tl"),
        &format!(
            "local record defs\n   record Monster\n      id: string\n   end\n\n   record Mod   \
             {marker}\n      name: string         ---@required\n      monsters: {{Monster}}   \
             ---@required\n      factions: {{string}}\n   end\nend\nreturn defs\n"
        ),
    );
    write(
        &root.join("mods/three.tl"),
        "local defs = require(\"defs\")\n\nlocal m: defs.Mod = {\n   name = \"three\",\n   \
         monsters = {},\n   extra = \"not declared\",\n}\nreturn m\n",
    );
    root
}

/// The issue's own transcript: the mod is refused for carrying more than the declaration
/// knows about, and refused as an error, so no allow comment and no `[lint]` setting
/// reaches it.
#[test]
fn an_unmarked_contract_record_refuses_the_extra_key() {
    let root = project("closed", "---@contract");
    let out = htl(&root, &["check", ".", "--no-cache"]);
    let text = output(&out);
    assert!(!out.status.success(), "{text}");
    assert!(
        text.contains("mods/three.tl:6:4:") && text.contains("unknown field extra"),
        "{text}"
    );
}

/// With the marker the same project checks. `---@extensible` reads on the same line as
/// `---@contract`, which stops reading its own arguments where the next marker begins.
#[test]
fn an_extensible_contract_record_accepts_it() {
    let root = project("open", "---@contract ---@extensible");
    let out = htl(&root, &["check", ".", "--no-cache"]);
    let text = output(&out);
    assert!(out.status.success(), "{text}");
    assert!(!text.contains("unknown field"), "{text}");
    // The contract is still a contract: the directory was inherited from htl.toml and the
    // declaration published, marker and all, so a mod author checking against the
    // published `.d.tl` gets the same tolerance the declaring project has.
    let published = std::fs::read_to_string(root.join("types/defs.d.tl")).unwrap();
    assert!(
        published.contains("---@contract(\"mods\") ---@extensible"),
        "{published}"
    );
}

/// The marker does not touch which *declared* fields a mod must set: `---@required` is
/// still `---@required`, and a mod that is short is still reported.
#[test]
fn a_required_field_is_still_required() {
    let root = project("short", "---@contract ---@extensible");
    write(
        &root.join("mods/short.tl"),
        "local defs = require(\"defs\")\n\nlocal m: defs.Mod = {\n   name = \"short\",\n   extra \
         = \"not declared\",\n}\nreturn m\n",
    );
    let text = output(&htl(&root, &["check", ".", "--no-cache"]));
    assert!(
        text.contains("mods/short.tl") && text.contains("monsters"),
        "{text}"
    );
}
