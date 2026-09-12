//! `htlx-available`: a loop whose whole body is a function htl-x already has.
//!
//! Not a correctness rule. The loop is right and so is the call, and the only thing that
//! makes the advice worth saying is that the project already depends on htl-x — which is
//! why every check here says whether it does, and why the first test is the one where it
//! does not.
//!
//! The two shapes the rule was written from are real: `summaries()` and `set()` as a
//! command-line tool wrote them before their author found `list.map` and `list.to_set` by
//! reading.

use htl_core::Htl;
use std::path::{Path, PathBuf};

mod common;

const RULE: &str = "htlx-available";

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-htlx-available", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A project directory and a `main.tl` in it. `htlx` is not put on the path: what the rule
/// reads is the dependency list the state was told about ([`Htl::set_deps`]), and a file
/// that does not `require` the module type-checks without it.
fn project(name: &str, body: &str) -> PathBuf {
    let dir = scratch(name);
    write(&dir.join("main.tl"), body);
    dir
}

/// `deps` is what `Htl::apply_project` hands over from the lockfile — the names, so that a
/// test can say "this project has htlx" without installing one.
fn lints_with(dir: &Path, spec: &str, deps: &[&str]) -> Vec<String> {
    let h = Htl::new().unwrap();
    h.configure_lints(spec).unwrap();
    h.add_path(dir).unwrap();
    let owned: Vec<String> = deps.iter().map(|s| s.to_string()).collect();
    h.set_deps(&owned).unwrap();
    let ci = h.check(&dir.join("main.tl")).unwrap();
    assert!(ci.ok(), "unexpected type errors: {:?}", ci.errors);
    ci.lints
}

fn of_rule<'a>(lints: &'a [String], rule: &str) -> Vec<&'a String> {
    let tag = format!("[htl {rule}]");
    lints.iter().filter(|l| l.contains(&tag)).collect()
}

/// With the dependency, asked for by name.
fn lints_of(dir: &Path) -> Vec<String> {
    lints_with(dir, &format!("+{RULE}"), &["htlx"])
}

const MAP: &str = "local function f(s: string): string return s end\n\
     local function summaries(rows: {string}): {string}\n\
     \x20  local out: {string} = {}\n\
     \x20  for i = 1, #rows do\n\
     \x20     out[i] = f(rows[i])\n\
     \x20  end\n\
     \x20  return out\n\
     end\n\
     return summaries\n";

const TO_SET: &str = "local function set(names: {string}): {string:boolean}\n\
     \x20  local out: {string:boolean} = {}\n\
     \x20  for i = 1, #names do\n\
     \x20     out[names[i]] = true\n\
     \x20  end\n\
     \x20  return out\n\
     end\n\
     return set\n";

const FILTER: &str = "local function p(s: string): boolean return s ~= \"\" end\n\
     local function opens(rows: {string}): {string}\n\
     \x20  local out: {string} = {}\n\
     \x20  for i = 1, #rows do\n\
     \x20     if p(rows[i]) then\n\
     \x20        out[#out + 1] = rows[i]\n\
     \x20     end\n\
     \x20  end\n\
     \x20  return out\n\
     end\n\
     return opens\n";

/// The first condition, and the one that makes this advice rather than an opinion: a
/// project that does not depend on htl-x hears nothing, whatever its loops look like.
#[test]
fn a_project_without_the_dependency_hears_nothing() {
    let dir = project("no-dep", MAP);
    let lints = lints_with(&dir, &format!("+{RULE}"), &[]);
    assert!(of_rule(&lints, RULE).is_empty(), "{lints:?}");
    // And with an unrelated dependency, which is the same answer for the same reason.
    let lints = lints_with(&dir, &format!("+{RULE}"), &["lshape"]);
    assert!(of_rule(&lints, RULE).is_empty(), "{lints:?}");
}

/// The three shapes, each naming the call that replaces it. The call is in the message
/// because the name of the rule is not the advice — `list.map(rows, f)` is.
#[test]
fn the_three_shapes_name_their_call() {
    for (name, body, call) in [
        ("map", MAP, "list.map(rows, f)"),
        ("to-set", TO_SET, "list.to_set(names)"),
        ("filter", FILTER, "list.filter(rows, p)"),
    ] {
        let dir = project(name, body);
        let lints = lints_of(&dir);
        let hits = of_rule(&lints, RULE);
        assert_eq!(hits.len(), 1, "{name}: {lints:?}");
        assert!(hits[0].contains(call), "{name}: {}", hits[0]);
    }
}

/// `out[#out + 1] = f(t[i])` is the other way a loop writes the next element, and the same
/// call.
#[test]
fn the_push_form_of_map_is_the_same_call() {
    let dir = project(
        "map-push",
        "local function f(s: string): string return s end\n\
         local function go(rows: {string}): {string}\n\
         \x20  local out: {string} = {}\n\
         \x20  for i = 1, #rows do\n\
         \x20     out[#out + 1] = f(rows[i])\n\
         \x20  end\n\
         \x20  return out\n\
         end\n\
         return go\n",
    );
    let hits = of_rule(&lints_of(&dir), RULE).len();
    assert_eq!(hits, 1);
}

/// Every shape the rule refuses, and why each one is a loop and not a call.
#[test]
fn the_shapes_that_are_not_the_call_are_silent() {
    for (name, body) in [
        // A second statement: the loop is doing something else as well.
        (
            "two-statements",
            "\x20     out[i] = f(rows[i])\n\x20     print(i)\n",
        ),
        // The index in something other than a subscript.
        (
            "index-used",
            "\x20     out[i] = f(rows[i]) .. tostring(i)\n",
        ),
    ] {
        let dir = project(
            name,
            &format!(
                "local function f(s: string): string return s end\n\
                 local function go(rows: {{string}}): {{string}}\n\
                 \x20  local out: {{string}} = {{}}\n\
                 \x20  for i = 1, #rows do\n\
                 {body}\
                 \x20  end\n\
                 \x20  return out\n\
                 end\n\
                 return go\n"
            ),
        );
        let lints = lints_of(&dir);
        assert!(of_rule(&lints, RULE).is_empty(), "{name}: {lints:?}");
    }
}

/// The element read has to come from the array the loop measures. `for i = 1, #a` writing
/// `f(b[i])` is a loop over two arrays, and `list.map(a, f)` is not what it does.
#[test]
fn a_second_array_is_not_this_loops_call() {
    let dir = project(
        "two-arrays",
        "local function f(s: string): string return s end\n\
         local function go(a: {string}, b: {string}): {string}\n\
         \x20  local out: {string} = {}\n\
         \x20  for i = 1, #a do\n\
         \x20     out[i] = f(b[i])\n\
         \x20  end\n\
         \x20  return out\n\
         end\n\
         return go\n",
    );
    let lints = lints_of(&dir);
    assert!(of_rule(&lints, RULE).is_empty(), "{lints:?}");
}

/// A loop that does not start at 1 is a slice; `list.map` is over the whole array.
#[test]
fn a_loop_that_does_not_start_at_one_is_silent() {
    let dir = project(
        "from-two",
        "local function f(s: string): string return s end\n\
         local function go(rows: {string}): {string}\n\
         \x20  local out: {string} = {}\n\
         \x20  for i = 2, #rows do\n\
         \x20     out[#out + 1] = f(rows[i])\n\
         \x20  end\n\
         \x20  return out\n\
         end\n\
         return go\n",
    );
    let lints = lints_of(&dir);
    assert!(of_rule(&lints, RULE).is_empty(), "{lints:?}");
}

/// The accumulator has to be empty on the line above, which is what makes the call equal
/// to the loop rather than merely similar: `list.map` hands back a new array, and a loop
/// over an `out` that already held something does not.
#[test]
fn an_accumulator_that_is_not_empty_above_the_loop_is_silent() {
    for (name, decl, between) in [
        ("prefilled", "local out: {string} = { \"head\" }\n", ""),
        (
            "apart",
            "local out: {string} = {}\n",
            "\x20  print(\"go\")\n",
        ),
    ] {
        let dir = project(
            name,
            &format!(
                "local function f(s: string): string return s end\n\
                 local function go(rows: {{string}}): {{string}}\n\
                 \x20  {decl}{between}\
                 \x20  for i = 1, #rows do\n\
                 \x20     out[#out + 1] = f(rows[i])\n\
                 \x20  end\n\
                 \x20  return out\n\
                 end\n\
                 return go\n"
            ),
        );
        let lints = lints_of(&dir);
        assert!(of_rule(&lints, RULE).is_empty(), "{name}: {lints:?}");
    }
}

/// Off until a project asks. The two rules before it in the listing are `allow` because
/// what they report may be wrong; this one because what it reports is right.
#[test]
fn the_rule_is_off_until_a_project_asks_for_it() {
    let dir = project("default-off", MAP);
    let lints = lints_with(&dir, "", &["htlx"]);
    assert!(of_rule(&lints, RULE).is_empty(), "{lints:?}");
    assert_eq!(
        htl_core::lint::rule_defaults()
            .into_iter()
            .find(|(n, _)| *n == RULE)
            .map(|(_, l)| l),
        Some(htl_core::lint::Level::Allow)
    );
    for level in ["warn", "deny"] {
        let lints = lints_with(&dir, &format!("{RULE}={level}"), &["htlx"]);
        assert_eq!(of_rule(&lints, RULE).len(), 1, "{level}: {lints:?}");
    }
}

/// One occurrence, silenced where it is.
#[test]
fn allow_silences_one_site() {
    let dir = project(
        "allow",
        "local function f(s: string): string return s end\n\
         local function go(rows: {string}): {string}\n\
         \x20  local out: {string} = {}\n\
         \x20  for i = 1, #rows do  -- htl: allow(htlx-available)\n\
         \x20     out[i] = f(rows[i])\n\
         \x20  end\n\
         \x20  return out\n\
         end\n\
         return go\n",
    );
    let lints = lints_of(&dir);
    assert!(of_rule(&lints, RULE).is_empty(), "{lints:?}");
}

/// The finding carries the rewrite, as a suggestion: the loop replaced by the assignment
/// it was. Never applied — merging it into the declaration above is the edit a person
/// makes, and this rule does not write that line.
#[test]
fn the_finding_carries_the_call_as_a_suggestion() {
    let dir = project("fix", MAP);
    let h = Htl::new().unwrap();
    h.configure_lints(&format!("+{RULE}")).unwrap();
    h.add_path(&dir).unwrap();
    h.set_deps(&["htlx".to_string()]).unwrap();
    let ci = h.check(&dir.join("main.tl")).unwrap();
    let fix = ci
        .lint_fixes
        .iter()
        .flatten()
        .find(|f| !f.edits.is_empty())
        .expect("a fix on the finding");
    assert_eq!(fix.applicability, htl_core::Applicability::Suggest);
    assert_eq!(fix.edits.len(), 1);
    assert_eq!(fix.edits[0].text, "out = list.map(rows, f)");
    // The edit spans the whole loop, `end` included: the three lines of it become one.
    assert_eq!(fix.edits[0].line, 4);
    assert_eq!(fix.edits[0].end_line, 6);
}
