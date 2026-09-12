//! `nil-return-unchecked`: the local `nil-return` tells you to bind, used before anything
//! looks at it.
//!
//! `local d = p.parent(s)` then `d:upper()` is the run-time error `p.parent(s):upper()`
//! would have been, and the other rule cannot see it because the base of the chain is no
//! longer the call. What ends the tracking is any statement that mentions the name in a
//! condition, asserts it, or re-binds it — read loosely on purpose, because the shapes a
//! flow rule gets wrong are the ones where something *did* check and this rule could not
//! tell.

use htl_core::Htl;
use std::path::{Path, PathBuf};

mod common;

const RULE: &str = "nil-return-unchecked";

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-nil-flow", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// The rule is `allow` by default, so every check here asks for it by name. That is the
/// spec — a project that wants a flow rule says so — and it is what `the_rule_is_off_until_
/// a_project_asks_for_it` holds the registry to.
fn checker(dir: &Path, spec: &str) -> Htl {
    let h = Htl::new().unwrap();
    h.configure_lints(spec).unwrap();
    h.add_path(dir).unwrap();
    let types = dir.join("types");
    if types.is_dir() {
        h.add_path(&types).unwrap();
    }
    h
}

fn lints_of(dir: &Path, file: &str) -> Vec<String> {
    let ci = checker(dir, &format!("+{RULE}"))
        .check(&dir.join(file))
        .unwrap();
    assert!(ci.ok(), "unexpected type errors: {:?}", ci.errors);
    ci.lints
}

fn of_rule<'a>(lints: &'a [String], rule: &str) -> Vec<&'a String> {
    let tag = format!("[htl {rule}]");
    lints.iter().filter(|l| l.contains(&tag)).collect()
}

const DECL: &str = "local record p\n\
     \x20  record Box\n\
     \x20     n: integer\n\
     \x20  end\n\
     \x20  parent: function(string): string   ---@nilable\n\
     \x20  box: function(string): Box   ---@nilable\n\
     \x20  trim: function(string): string\n\
     end\n\
     return p\n";

fn project(name: &str) -> PathBuf {
    let dir = scratch(name);
    write(&dir.join("types/p.d.tl"), DECL);
    dir
}

fn main_tl(dir: &Path, body: &str) {
    write(
        &dir.join("main.tl"),
        &format!("local p = require(\"p\")\n{body}"),
    );
}

/// The shape the rule exists for, at the use rather than at the declaration, naming the
/// local and the function it came from.
#[test]
fn a_local_used_before_anything_looks_at_it_is_reported() {
    let dir = project("plain");
    main_tl(&dir, "local d = p.parent(\"a\")\nprint(d:upper())\n");
    let lints = lints_of(&dir, "main.tl");
    let found = of_rule(&lints, RULE);
    assert_eq!(found.len(), 1, "{lints:?}");
    assert!(found[0].contains("main.tl:3:8"), "{}", found[0]);
    assert!(
        found[0].contains("'d' may be nil at runtime")
            && found[0].contains("it comes from p.parent, which is marked ---@nilable")
            && found[0].contains("nothing checks it before this"),
        "{}",
        found[0]
    );
}

/// Every guard the rule takes, each with the use that follows it. A condition that
/// mentions the name, an `assert`, the `and` written into the use itself, and the `or` at
/// the declaration, which settles it before there is anything to guard.
#[test]
fn the_guards_are_silent() {
    for (name, body) in [
        (
            "if",
            "local d = p.parent(\"a\")\nif d then print(d:upper()) end\n",
        ),
        (
            "if-nil-return",
            "local function f()\n\x20  local d: string = p.parent(\"a\")\n\x20  if d == nil then return end\n\x20  print(d:upper())\nend\nf()\n",
        ),
        (
            "if-not",
            "local function f()\n\x20  local d: string = p.parent(\"a\")\n\x20  if not d then return end\n\x20  print(d:upper())\nend\nf()\n",
        ),
        (
            "assert",
            "local d = p.parent(\"a\")\nassert(d)\nprint(d:upper())\n",
        ),
        ("and", "local d = p.parent(\"a\")\nprint(d and d:upper())\n"),
        (
            "or-default",
            "local d = p.parent(\"a\") or \"/\"\nprint(d:upper())\n",
        ),
        (
            "while",
            "local d = p.parent(\"a\")\nwhile d do print(d:upper()) break end\n",
        ),
    ] {
        let dir = project(&format!("guard-{name}"));
        main_tl(&dir, body);
        let lints = lints_of(&dir, "main.tl");
        assert!(of_rule(&lints, RULE).is_empty(), "{name}: {lints:?}");
    }
}

/// `local d = p.parent(s) or error(...)` is the third of the false positives this rule was
/// designed around: the `or` is read where the local is declared, so there is nothing left
/// to check afterwards.
#[test]
fn an_or_error_at_the_declaration_settles_it() {
    let dir = project("or-error");
    main_tl(
        &dir,
        "local d = p.parent(\"a\") or error(\"no parent\")\nprint(d:upper())\n",
    );
    assert!(of_rule(&lints_of(&dir, "main.tl"), RULE).is_empty());
}

/// Two names bound at once is a result, not a value that may be nil, and a field is not a
/// local at all. Neither is this rule's subject.
#[test]
fn a_multiple_binding_and_a_field_are_not_this_rules_subject() {
    let dir = project("not-subject");
    main_tl(
        &dir,
        "local ok, d = pcall(function(): string return p.parent(\"a\") end)\n\
         print(ok, d)\n\
         local t = { d = p.parent(\"b\") }\n\
         print(t.d:upper())\n",
    );
    assert!(of_rule(&lints_of(&dir, "main.tl"), RULE).is_empty());
}

/// An unmarked function says nothing, here as in `nil-return`: a declaration nobody marked
/// means "unknown", not "nilable".
#[test]
fn a_local_from_an_unmarked_function_is_silent() {
    let dir = project("unmarked");
    main_tl(&dir, "local d = p.trim(\"  a  \")\nprint(d:upper())\n");
    assert!(of_rule(&lints_of(&dir, "main.tl"), RULE).is_empty());
}

/// One report per local, at the first unguarded use. What happens to a value after it has
/// been mentioned once is the caller's business, and repeating the same finding down the
/// block would say nothing new.
#[test]
fn the_second_use_says_nothing_more() {
    let dir = project("once");
    main_tl(
        &dir,
        "local d = p.parent(\"a\")\nprint(d:upper())\nprint(d:lower())\nprint(d:len())\n",
    );
    let lints = lints_of(&dir, "main.tl");
    let found = of_rule(&lints, RULE);
    assert_eq!(found.len(), 1, "{lints:?}");
    assert!(found[0].contains("main.tl:3:8"), "{}", found[0]);
}

/// Every way the local is the base of a chain: a method call, a field access, an index and
/// a call. Passing it, returning it and printing it are not chains and are not reported —
/// whether nil is allowed there is the receiving declaration's to say.
#[test]
fn the_chain_shapes_are_reported_and_the_others_are_not() {
    let dir = project("shapes");
    main_tl(
        &dir,
        "local a = p.box(\"a\")\nprint(a.n)\n\
         local b = p.parent(\"b\")\nprint(b:upper())\n\
         local c = p.parent(\"c\")\nprint(c)\n\
         local e = p.parent(\"e\")\nreturn e\n",
    );
    let lints = lints_of(&dir, "main.tl");
    let found = of_rule(&lints, RULE);
    assert_eq!(found.len(), 2, "{lints:?}");
    assert!(found[0].contains("'a' may be nil"), "{}", found[0]);
    assert!(found[1].contains("'b' may be nil"), "{}", found[1]);
}

#[test]
fn allow_silences_one_site() {
    let dir = project("allow-comment");
    main_tl(
        &dir,
        "local d = p.parent(\"a\")\n\
         print(d:upper())  -- htl: allow(nil-return-unchecked)\n\
         local e = p.parent(\"b\")\n\
         print(e:upper())\n",
    );
    let lints = lints_of(&dir, "main.tl");
    let found = of_rule(&lints, RULE);
    assert_eq!(found.len(), 1, "{lints:?}");
    assert!(found[0].contains("'e' may be nil"), "{}", found[0]);
}

/// Off until a project asks. `nil-return` is `warn` because indexing a call's result
/// directly is wrong whatever an analysis says; this one is a flow question, and the level
/// is where that difference is written down.
#[test]
fn the_rule_is_off_until_a_project_asks_for_it() {
    let levels = htl_core::lint::rule_defaults();
    let (_, level) = levels
        .iter()
        .find(|(name, _)| *name == RULE)
        .unwrap_or_else(|| panic!("{RULE} is not in the registry: {levels:?}"));
    assert_eq!(*level, htl_core::lint::Level::Allow);

    let dir = project("default-off");
    main_tl(&dir, "local d = p.parent(\"a\")\nprint(d:upper())\n");
    let ci = checker(&dir, "").check(&dir.join("main.tl")).unwrap();
    assert!(of_rule(&ci.lints, RULE).is_empty(), "{:?}", ci.lints);
}

/// Asked for by name at either level, it is reported. Which of the two fails the run is
/// the project layer's to say — `deny` does, `warn` does not — and a check at this level
/// carries the finding either way. The exit codes are measured through the binary.
#[test]
fn the_rule_reports_under_warn_and_under_deny() {
    for level in ["warn", "deny"] {
        let dir = project(&format!("level-{level}"));
        main_tl(&dir, "local d = p.parent(\"a\")\nprint(d:upper())\n");
        let h = checker(&dir, &format!("{RULE}={level}"));
        let ci = h.check(&dir.join("main.tl")).unwrap();
        assert_eq!(of_rule(&ci.lints, RULE).len(), 1, "{level}: {:?}", ci.lints);
    }
}

/// The two shapes this rule gets wrong, held here so that they are a decision and not a
/// surprise: a check written inside a helper, and a check assigned to a second local. In
/// both, something did look at the value and the condition names something else, which is
/// all this rule reads. They are why the default is `allow`, and a project that turns the
/// rule on is saying it would rather see these than miss the use that raises.
#[test]
fn a_check_the_condition_does_not_name_is_reported_anyway() {
    for (name, body) in [
        (
            "helper",
            "local function has(s: string): boolean return #s > 0 end\n\
             local d = p.parent(\"a\")\nif has(\"a\") then print(d:upper()) end\n",
        ),
        (
            "second-local",
            "local d = p.parent(\"a\")\nlocal ok = d ~= nil\nif ok then print(d:upper()) end\n",
        ),
    ] {
        let dir = project(&format!("known-fp-{name}"));
        main_tl(&dir, body);
        let lints = lints_of(&dir, "main.tl");
        assert_eq!(of_rule(&lints, RULE).len(), 1, "{name}: {lints:?}");
    }
}

/// The other half is untouched: a call indexed directly is still `nil-return`, and it is
/// still on by default. The two rules are named apart so that a project may have the
/// certain one without the uncertain one.
#[test]
fn the_direct_chain_is_still_the_other_rules() {
    let dir = project("other-rule");
    main_tl(&dir, "print(p.parent(\"a\"):upper())\n");
    let lints = lints_of(&dir, "main.tl");
    assert!(of_rule(&lints, RULE).is_empty(), "{lints:?}");
    assert_eq!(of_rule(&lints, "nil-return").len(), 1, "{lints:?}");
}
