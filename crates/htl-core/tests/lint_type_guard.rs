//! `type-guard`: `type(v) == "<tag>"` on a variable typed `any`, or a union with one
//! member the tag selects, is reported with the `is` test Teal does narrow under, and
//! `htl fix` writes it. The fix is safe: `is` compiles to the same `type()` call and a cast
//! compiles to nothing.

use htl_core::Htl;
use htl_core::fix::{FixOptions, fix_file};
use std::path::Path;

mod common;

fn tempdir(name: &str) -> common::TempDir {
    common::tempdir("htl-core-type-guard", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn checker(dir: &Path, spec: &str) -> Htl {
    let h = Htl::new().unwrap();
    if !spec.is_empty() {
        h.configure_lints(spec).unwrap();
    }
    h.add_path(dir).unwrap();
    h
}

fn of_rule(lints: &[String]) -> Vec<&String> {
    lints
        .iter()
        .filter(|l| l.contains("[htl type-guard]"))
        .collect()
}

/// `htl fix --rule type-guard`, repeated until a run changes nothing: a run stops after a
/// pass with nothing deferred, so a guard the first rewrite exposed (`b` below is typed
/// only once `meta` narrows) is the next run's.
fn fix_until_still(dir: &Path, file: &str) -> usize {
    let mut applied = 0;
    for _ in 0..4 {
        let h = checker(dir, "");
        let opts = FixOptions {
            only: vec!["type-guard".into()],
            ..Default::default()
        };
        let out = fix_file(&h, &dir.join(file), &opts).unwrap();
        assert!(out.reverted.is_none(), "{:?}", out.reverted);
        if out.applied.is_empty() {
            return applied;
        }
        applied += out.applied.len();
    }
    panic!("htl fix did not settle");
}

fn gen_of(dir: &Path, file: &str) -> String {
    let (code, ci) = checker(dir, "").gen_lua(&dir.join(file)).unwrap();
    code.unwrap_or_else(|| panic!("gen refused {file}: {:?}", ci.errors))
}

/// The snippet the rule exists for: decoded data checked field by field.
const NARROW: &str = "local function beat_of(meta: any): string\n\
                      \x20   if type(meta) == \"table\" then\n\
                      \x20       local b = meta.beat\n\
                      \x20       if type(b) == \"string\" then\n\
                      \x20           return b\n\
                      \x20       end\n\
                      \x20   end\n\
                      \x20   return nil\n\
                      end\n\
                      return beat_of\n";

// 1
#[test]
fn a_table_guard_on_any_is_reported_beside_the_index_error() {
    let dir = tempdir("narrow");
    write(&dir.join("narrow.tl"), NARROW);
    let ci = checker(&dir, "").check(&dir.join("narrow.tl")).unwrap();
    let hits = of_rule(&ci.lints);
    assert_eq!(hits.len(), 1, "{:?}", ci.lints);
    assert!(hits[0].contains("narrow.tl:2:8:"), "{}", hits[0]);
    assert!(hits[0].contains("`meta is {string:any}`"), "{}", hits[0]);
    assert!(
        ci.errors
            .iter()
            .any(|e| e.contains("narrow.tl:3:24:") && e.contains("cannot index")),
        "{:?}",
        ci.errors
    );
}

// 2
#[test]
fn after_the_fix_the_snippet_checks_and_generates_the_guard_it_was_written_with() {
    let dir = tempdir("narrow-fix");
    write(&dir.join("narrow.tl"), NARROW);
    assert_eq!(fix_until_still(&dir, "narrow.tl"), 2);
    let text = std::fs::read_to_string(dir.join("narrow.tl")).unwrap();
    assert!(
        text.contains("    if meta is {string:any} then\n"),
        "{text}"
    );
    assert!(text.contains("        if b is string then\n"), "{text}");
    // `--strict` promotes warnings to failures; with none of either the run passes it.
    let ci = checker(&dir, "").check(&dir.join("narrow.tl")).unwrap();
    assert!(ci.errors.is_empty(), "{:?}", ci.errors);
    assert!(ci.warnings.is_empty(), "{:?}", ci.warnings);
    assert!(of_rule(&ci.lints).is_empty(), "{:?}", ci.lints);
    // The snippet before the fix has a type error, so there is no `htl gen` of it to
    // compare with. What it means is the Lua below -- the guards as written -- and that is
    // what the fixed file generates.
    assert_eq!(
        gen_of(&dir, "narrow.tl"),
        "local function beat_of(meta)\n\
         \x20  if type(meta) == \"table\" then\n\
         \x20     local b = meta.beat\n\
         \x20     if type(b) == \"string\" then\n\
         \x20        return b\n\
         \x20     end\n\
         \x20  end\n\
         \x20  return nil\n\
         end\n\
         return beat_of\n"
    );
}

// 3
#[test]
fn the_fix_drops_the_cast_the_guard_made_redundant() {
    let dir = tempdir("cast");
    write(
        &dir.join("cast.tl"),
        "local function value(repo: any): string\n\
         \x20  if type(repo) == \"table\" then\n\
         \x20     local v = (repo as {string:any}).value\n\
         \x20     if v is string then return v end\n\
         \x20  end\n\
         \x20  return nil\n\
         end\n\
         local function both(repo: any): boolean\n\
         \x20  return type(repo) == \"table\" and type((repo as {string:any}).value) == \"string\"\n\
         end\n\
         return { value = value, both = both }\n",
    );
    let before = gen_of(&dir, "cast.tl");
    let ci = checker(&dir, "").check(&dir.join("cast.tl")).unwrap();
    let hits = of_rule(&ci.lints);
    assert_eq!(hits.len(), 2, "{:?}", ci.lints);
    assert!(hits.iter().all(|l| l.contains("drop the cast")), "{hits:?}");
    assert_eq!(fix_until_still(&dir, "cast.tl"), 2);
    let text = std::fs::read_to_string(dir.join("cast.tl")).unwrap();
    assert!(text.contains("   if repo is {string:any} then\n"), "{text}");
    assert!(text.contains("      local v = repo.value\n"), "{text}");
    assert!(
        text.contains("   return repo is {string:any} and type(repo.value) == \"string\"\n"),
        "{text}"
    );
    // A cast compiles to nothing, but its parentheses are a `paren` node and survive into
    // the Lua: `(repo).value` before, `repo.value` after. The same expression; that
    // spelling is the only difference.
    let after = gen_of(&dir, "cast.tl");
    assert_ne!(before, after);
    assert_eq!(before.replace("(repo).value", "repo.value"), after);
}

/// A cast is kept where the narrowing does not reach: past an assignment to the variable.
#[test]
fn a_cast_after_the_variable_is_reassigned_stays() {
    let dir = tempdir("reassign");
    write(
        &dir.join("r.tl"),
        "local function f(x: any): any\n\
         \x20  if type(x) == \"table\" then\n\
         \x20     x = {}\n\
         \x20     return (x as {string:any}).k\n\
         \x20  end\n\
         end\n\
         return f\n",
    );
    fix_until_still(&dir, "r.tl");
    let text = std::fs::read_to_string(dir.join("r.tl")).unwrap();
    assert!(text.contains("   if x is {string:any} then\n"), "{text}");
    assert!(text.contains("(x as {string:any}).k"), "{text}");
}

// 4
#[test]
fn a_union_with_one_member_the_tag_selects_gets_that_member() {
    let dir = tempdir("union");
    write(
        &dir.join("u.tl"),
        "local record R\n   a: string\nend\n\
         local function f(x: string | number): string\n\
         \x20  if type(x) == \"string\" then return \"s\" end\n\
         \x20  return \"n\"\n\
         end\n\
         local function g(x: R | string): string\n\
         \x20  if type(x) == \"table\" then return \"t\" end\n\
         \x20  return \"s\"\n\
         end\n\
         return { f = f, g = g }\n",
    );
    let before = gen_of(&dir, "u.tl");
    let ci = checker(&dir, "").check(&dir.join("u.tl")).unwrap();
    let hits = of_rule(&ci.lints);
    assert_eq!(hits.len(), 2, "{:?}", ci.lints);
    assert!(
        hits[0].contains("u.tl:5:7:") && hits[0].contains("`x is string`"),
        "{}",
        hits[0]
    );
    assert!(
        hits[1].contains("u.tl:9:7:") && hits[1].contains("`x is R`"),
        "{}",
        hits[1]
    );
    assert_eq!(fix_until_still(&dir, "u.tl"), 2);
    assert_eq!(gen_of(&dir, "u.tl"), before);
}

// 5
#[test]
fn fields_functions_typed_tables_and_ambiguous_unions_are_quiet() {
    let dir = tempdir("quiet");
    write(
        &dir.join("q.tl"),
        "local record R\n   a: string\nend\n\
         local function h(x: R, m: {string:any}, ev: {string:any}, fn: any, n: integer | string): string\n\
         \x20  if type(x) == \"table\" and type(m) == \"table\" then return \"a\" end\n\
         \x20  if type(ev.data) == \"table\" then return \"b\" end\n\
         \x20  if type(fn) == \"function\" then return \"c\" end\n\
         \x20  if type(n) == \"number\" then return \"d\" end\n\
         \x20  return \"e\"\n\
         end\n\
         return h\n",
    );
    let ci = checker(&dir, "").check(&dir.join("q.tl")).unwrap();
    assert!(ci.errors.is_empty(), "{:?}", ci.errors);
    assert!(of_rule(&ci.lints).is_empty(), "{:?}", ci.lints);
}

#[test]
fn allow_comment_and_spec_silence_it() {
    let dir = tempdir("allow");
    write(
        &dir.join("a.tl"),
        "local function f(x: any): boolean\n\
         \x20  return type(x) == \"table\" -- htl: allow(type-guard)\n\
         end\n\
         local function g(x: any): boolean\n\
         \x20  return type(x) == \"string\"\n\
         end\n\
         return { f = f, g = g }\n",
    );
    let ci = checker(&dir, "").check(&dir.join("a.tl")).unwrap();
    let hits = of_rule(&ci.lints);
    assert_eq!(hits.len(), 1, "{:?}", ci.lints);
    assert!(hits[0].contains("a.tl:5:"), "{}", hits[0]);
    let ci = checker(&dir, "-type-guard")
        .check(&dir.join("a.tl"))
        .unwrap();
    assert!(of_rule(&ci.lints).is_empty(), "{:?}", ci.lints);
}

// 6
#[test]
fn the_rule_is_registered_listed_and_explained() {
    let rule = htl_core::lint::explained("type-guard").expect("registered");
    assert!(rule.is_lint());
    assert_eq!(rule.default, htl_core::lint::Level::Warn);
    assert!(
        rule.explain.contains("never gets a record"),
        "{}",
        rule.explain
    );
    assert!(htl_core::lint::rule_names().contains(&"type-guard"));
}
