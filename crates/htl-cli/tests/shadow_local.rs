//! `shadow-local` says the one thing about shadowing the compiler cannot.
//!
//! Teal warns on a declaration over a name already declared (`tl:redeclaration`), and htl's
//! `shadow-local` used to warn on the same situation at the same line and column, so a
//! project with both on — which is the default — read two lines for one mistake. The lint
//! was the smaller of the two: everything it found, Teal found, and Teal also finds two
//! declarations in the *same* scope, which the lint's outward walk cannot see.
//!
//! What the lint has that Teal has not is a fact about where the shadowed name came from.
//! When the outer local was bound to a `require`, the lint names the module and says the
//! consequence — inside this scope that module is unreachable. Teal reports the shadowing
//! without knowing the name was a module at all.
//!
//! So the lint keeps that message and nothing else. The three cases below are the whole of
//! the division: ordinary shadowing is Teal's, a required module's name is htl's, two
//! declarations in one scope stay Teal's as they always were.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-shadow-local", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A module to require, and a file that shadows its name three ways:
///
/// ```text
///  1  local dep = require("dep")
///  6     local outer = 2            an ordinary local ...
///  8        local outer = 3         ... shadowed in an inner scope
/// 15  function m.at(dep: string)    a parameter over the required module
/// 19  local same = 1                two declarations ...
/// 20  local same = 2                ... in one scope
/// ```
const SRC: &str = "local dep = require(\"dep\")\nlocal record m\nend\n\n\
                   function m.go(s: string): integer\n   local outer = 2\n   do\n      \
                   local outer = 3\n      print(outer)\n   end\n   print(s, dep.note())\n   \
                   return outer\nend\n\nfunction m.at(dep: string): string\n   return dep\n\
                   end\n\nlocal same = 1\nlocal same = 2\nprint(same)\n\nreturn m\n";

fn project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), "[check]\npaths = [\"src\"]\n");
    write(
        &root.join("src/dep.tl"),
        "local record dep\nend\nfunction dep.note(): string\n   return \"n\"\nend\nreturn dep\n",
    );
    write(&root.join("src/a.tl"), SRC);
    root
}

/// Every diagnostic as `(rule, line, message)`.
fn diagnostics(root: &Path, args: &[&str]) -> Vec<(String, u64, String)> {
    let out = Command::new(env!("CARGO_BIN_EXE_htl"))
        .args(["check", "src", "--format", "json", "--no-cache"])
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let v: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("stdout is one JSON document ({e}): {text}"));
    v["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .map(|d| {
            (
                d["rule"].as_str().unwrap_or_default().to_string(),
                d["line"].as_u64().unwrap_or(0),
                d["message"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The diagnostics reported at one line.
fn at_line(root: &Path, line: u64, args: &[&str]) -> Vec<(String, u64, String)> {
    diagnostics(root, args)
        .into_iter()
        .filter(|(_, l, _)| *l == line)
        .collect()
}

/// The diagnostics reported under one rule.
fn under(root: &Path, rule: &str, args: &[&str]) -> Vec<(String, u64, String)> {
    diagnostics(root, args)
        .into_iter()
        .filter(|(r, _, _)| r == rule)
        .collect()
}

#[test]
fn an_ordinary_shadowed_local_is_said_once_and_by_the_compiler() {
    let root = project("ordinary");
    let found = at_line(&root, 8, &[]);
    assert_eq!(found.len(), 1, "one mistake, one finding: {found:?}");
    assert_eq!(found[0].0, "tl:redeclaration", "{found:?}");
    // Teal's version is the better one for this case: it carries the origin's column as
    // well as its line, which the lint's message never did.
    assert!(
        found[0].2.contains("previous declaration of 'outer'") && found[0].2.contains("6:10"),
        "{found:?}"
    );
}

#[test]
fn a_local_over_a_required_module_is_named_by_the_lint() {
    let root = project("module");
    let said = under(&root, "shadow-local", &[]);
    assert_eq!(said.len(), 1, "the module case, and only it: {said:?}");
    assert_eq!(said[0].1, 15, "{said:?}");
    assert_eq!(
        said[0].2,
        "local 'dep' shadows the module 'dep' required at line 1; \
         inside this scope the module is unreachable, rename the local",
        "the module and the consequence, which is what Teal cannot say"
    );
    // Teal reports this line too, and that is the one place the two still meet: the pair
    // is a shadowing plus a fact about it, not the same sentence twice.
    let both = at_line(&root, 15, &[]);
    assert_eq!(both.len(), 2, "{both:?}");
    assert!(
        both.iter().any(|(r, _, _)| r == "tl:redeclaration"),
        "{both:?}"
    );
}

#[test]
fn two_declarations_in_one_scope_are_the_compilers_as_they_always_were() {
    let root = project("same-scope");
    let found = at_line(&root, 20, &[]);
    let rules: Vec<&str> = found.iter().map(|(r, _, _)| r.as_str()).collect();
    assert_eq!(
        rules,
        ["tl:redeclaration"],
        "the lint walks outward from the enclosing scope and never saw this: {found:?}"
    );
}

/// Both names are still a project's to write, so a project that wants neither says so
/// twice, and one that wants only the compiler's wording turns the lint off.
#[test]
fn both_rules_are_still_addressed_by_name() {
    let root = project("by-name");

    let off = under(&root, "shadow-local", &["--lint", "-shadow-local"]);
    assert!(off.is_empty(), "--lint -shadow-local left it: {off:?}");
    // And it left the compiler's warnings alone.
    assert_eq!(
        under(&root, "tl:redeclaration", &["--lint", "-shadow-local"]).len(),
        3,
        "one rule off, not the other"
    );

    let off = under(&root, "tl:redeclaration", &["--lint", "-tl:redeclaration"]);
    assert!(off.is_empty(), "--lint -tl:redeclaration left it: {off:?}");
    assert_eq!(
        under(&root, "shadow-local", &["--lint", "-tl:redeclaration"]).len(),
        1,
        "the lint is what is left when the compiler's warning is off"
    );

    // The same two names as keys of `[lint.rules]`.
    write(
        &root.join("htl.toml"),
        "[check]\npaths = [\"src\"]\n\n[lint.rules]\n\
         \"shadow-local\" = \"allow\"\n\"tl:redeclaration\" = \"allow\"\n",
    );
    let left = diagnostics(&root, &[]);
    assert!(
        left.iter()
            .all(|(r, _, _)| r != "shadow-local" && r != "tl:redeclaration"),
        "[lint.rules] allow left one of them: {left:?}"
    );
}
