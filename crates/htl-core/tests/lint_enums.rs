//! enum-exhaustive must see enums nested in records and enums from required modules,
//! not only top-level `local enum` declarations in the same file.

use htl_core::Htl;
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "htl-core-lint-{name}-{}-{}",
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

fn lints_of(dir: &Path, file: &str) -> Vec<String> {
    let h = Htl::new().unwrap();
    h.add_path(dir).unwrap();
    let ci = h.check(&dir.join(file)).unwrap();
    assert!(ci.ok(), "unexpected type errors: {:?}", ci.errors);
    ci.lints
}

#[test]
fn enum_nested_in_record_is_covered() {
    let dir = scratch("nested");
    write(
        &dir.join("game.tl"),
        "local record game\n   enum Command\n      \"move\"\n      \"wait\"\n      \"quit\"\n   end\nend\n\n\
         function game.step(c: game.Command)\n   if c == \"move\" then\n      print(1)\n   elseif c == \"wait\" then\n      print(2)\n   end\nend\n\n\
         return game\n",
    );
    let lints = lints_of(&dir, "game.tl");
    assert!(
        lints.iter().any(|l| l.contains("enum-exhaustive") && l.contains("quit")),
        "expected enum-exhaustive for nested enum, got {lints:?}"
    );
}

#[test]
fn enum_from_required_module_is_covered() {
    let dir = scratch("required");
    write(
        &dir.join("defs.tl"),
        "local record defs\n   enum Behavior\n      \"chase\"\n      \"wander\"\n      \"flee\"\n   end\nend\nreturn defs\n",
    );
    write(
        &dir.join("ai.tl"),
        "local defs = require(\"defs\")\n\nlocal function act(b: defs.Behavior)\n   if b == \"chase\" then\n      print(1)\n   elseif b == \"wander\" then\n      print(2)\n   end\nend\n\nact(\"chase\")\n",
    );
    let lints = lints_of(&dir, "ai.tl");
    assert!(
        lints.iter().any(|l| l.contains("enum-exhaustive") && l.contains("flee")),
        "expected enum-exhaustive for required-module enum, got {lints:?}"
    );
}

#[test]
fn exhaustive_chain_stays_quiet() {
    let dir = scratch("quiet");
    write(
        &dir.join("defs.tl"),
        "local record defs\n   enum Behavior\n      \"chase\"\n      \"flee\"\n   end\nend\nreturn defs\n",
    );
    write(
        &dir.join("ai.tl"),
        "local defs = require(\"defs\")\n\nlocal function act(b: defs.Behavior)\n   if b == \"chase\" then\n      print(1)\n   elseif b == \"flee\" then\n      print(2)\n   end\nend\n\nact(\"chase\")\n",
    );
    let lints = lints_of(&dir, "ai.tl");
    assert!(!lints.iter().any(|l| l.contains("enum-exhaustive")), "false positive: {lints:?}");
}
