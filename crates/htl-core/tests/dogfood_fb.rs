//! Feedback from the sgen / tsk dogfooding: self-require on case-insensitive
//! filesystems, run-time-only requires, and user-facing error text.

use htl_core::{Htl, user_message};
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-fb", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// `require("site")` from `Site.tl` on a case-insensitive filesystem finds the requiring
/// file itself; the error must say so instead of a bare "no type information".
#[test]
fn self_require_on_case_insensitive_fs_is_explained() {
    let dir = scratch("selfreq");
    write(
        &dir.join("Site.tl"),
        "local site = require(\"site\")\nlocal c: site.Config = {}\nprint(c)\n",
    );
    // Only meaningful where `site.tl` resolves to `Site.tl`.
    if !dir.join("site.tl").is_file() {
        eprintln!("skipped: case-sensitive filesystem");
        return;
    }
    let h = Htl::new().unwrap();
    h.add_path(&dir).unwrap();
    let ci = h.check(&dir.join("Site.tl")).unwrap();
    assert!(!ci.ok(), "self-require must not type-check");
    assert!(
        ci.errors
            .iter()
            .any(|e| e.contains("the requiring file itself") && e.contains("case-insensitive")),
        "{:?}",
        ci.errors
    );
}

/// A module that exists only at run time (the user's `Tasks.tl`, loaded by a host built
/// long before) is declared by a `.d.tl` in the host's tree: the build checks against the
/// declaration, the run resolves the real file. No dynamic require, no `any`.
#[test]
fn runtime_provided_module_is_declared_by_a_dts() {
    let sdk = scratch("sdk");
    write(
        &sdk.join("tsk.tl"),
        "local record tsk\n   record Tasks\n      name: string\n   end\nend\nreturn tsk\n",
    );
    // Contract for the user's file. Only types; nothing to run.
    write(
        &sdk.join("Tasks.d.tl"),
        "local tsk = require(\"tsk\")\nlocal Tasks: tsk.Tasks\nreturn Tasks\n",
    );
    write(
        &sdk.join("main.tl"),
        "local tasks = require(\"Tasks\")\nprint(\"task: \" .. tasks.name)\n",
    );

    // Build time (host's tree only): the literal require type-checks against Tasks.d.tl.
    let h = Htl::new().unwrap();
    h.add_path(&sdk).unwrap();
    let (code, ci) = h.gen_lua(&sdk.join("main.tl")).unwrap();
    assert!(ci.ok(), "{:?}", ci.errors);
    let code = code.unwrap();

    // Run time (user's project): the real Tasks.tl is served by a resolver rooted there.
    let project = scratch("project");
    write(&project.join("Tasks.tl"), "return { name = \"build\" }\n");
    let rt = Htl::new().unwrap();
    let mut reg = mlua_pkg::Registry::new();
    reg.add(
        htl_core::pkg::TealResolver::new(&project)
            .unwrap()
            .expect_type("tsk.Tasks"),
    );
    reg.install(rt.lua()).unwrap();
    rt.add_path(&sdk).unwrap(); // tsk.tl for expect_type's checker
    rt.exec(&code, "=main.tl", &[]).unwrap();
}

/// A host function's `Err` reaches the user as its own text, without Lua's traceback.
/// `t.expect(f())` where `f` returns two values: Teal reports the arity at `expect`, which
/// hides that the multi-value call in last position is what expanded. Name it.
#[test]
fn multi_value_call_in_last_argument_is_explained() {
    let dir = scratch("arity");
    write(
        &dir.join("m_test.tl"),
        "local t = require(\"htl.test\")\n\
         local function can_cast(mana: integer): boolean, string\n   return mana >= 3, \"\"\nend\n\
         t.describe(\"x\", function()\n   t.it(\"y\", function()\n      t.expect(can_cast(1)):to_equal(false)\n   end)\nend)\n",
    );
    let h = Htl::new().unwrap();
    h.install_test_lib().unwrap();
    h.add_path(&dir).unwrap();
    let ci = h.check(&dir.join("m_test.tl")).unwrap();
    let e = ci
        .errors
        .iter()
        .find(|e| e.contains("wrong number of arguments"))
        .expect("arity error");
    assert!(e.contains("m_test.tl:7:"), "{e}");
    assert!(
        e.contains("can_cast(...) is a call in last position"),
        "{e}"
    );
    assert!(
        e.contains("local a, b = can_cast(...)") && e.contains("(can_cast(...))"),
        "{e}"
    );
    assert!(
        !ci.errors.iter().any(|e| e.contains("unresolved generic")),
        "the follow-on generic error is a consequence of the same call: {:?}",
        ci.errors
    );

    // A plain arity mistake gets no such hint.
    write(
        &dir.join("plain.tl"),
        "local function one(a: integer): integer\n   return a\nend\nprint(one(1, 2))\n",
    );
    let ci = h.check(&dir.join("plain.tl")).unwrap();
    let e = ci
        .errors
        .iter()
        .find(|e| e.contains("wrong number of arguments"))
        .expect("arity error");
    assert!(!e.contains("last position"), "{e}");
}

/// `function world.observe` defined below its first use: Teal reports "invalid key";
/// htl names the later definition and hands over the record declaration line.
#[test]
fn forward_reference_gets_the_declaration_line() {
    let dir = scratch("fwd");
    write(
        &dir.join("world.tl"),
        "local record world\n   record W\n      hp: integer\n   end\nend\n\n\
         function world.tick(w: world.W): boolean\n   world.observe(w, \"tick\")\n   return world:alive(w)\nend\n\n\
         function world.observe(w: world.W,\n                       what: string) -- log it\n   print(w.hp, what)\nend\n\n\
         function world:alive(w: world.W): boolean\n   return w.hp > 0\nend\n\nreturn world\n",
    );
    let h = Htl::new().unwrap();
    h.add_path(&dir).unwrap();
    let ci = h.check(&dir.join("world.tl")).unwrap();
    let observe = ci
        .errors
        .iter()
        .find(|e| e.contains("'observe'"))
        .expect("forward ref error");
    assert!(
        observe.contains("`world.observe` is defined at line 12, after this use"),
        "{observe}"
    );
    assert!(
        observe.contains("`observe: function(w: world.W, what: string)`"),
        "multi-line header joined, comment dropped: {observe}"
    );
    assert!(
        observe.contains("move the definition above line 8"),
        "{observe}"
    );
    let alive = ci
        .errors
        .iter()
        .find(|e| e.contains("'alive'"))
        .expect("forward ref error");
    assert!(
        alive.contains("`alive: function(self: world, w: world.W): boolean`"),
        "method form: {alive}"
    );

    // A genuinely unknown field gets no such hint.
    write(
        &dir.join("typo.tl"),
        "local record m\nend\nfunction m.f()\n   m.g()\nend\nreturn m\n",
    );
    let ci = h.check(&dir.join("typo.tl")).unwrap();
    let e = ci.errors.iter().find(|e| e.contains("'g'")).expect("error");
    assert!(!e.contains("defined at line"), "{e}");
}

/// A parameter named like a required module: the message names the module and the require
/// line, which is the one thing about shadowing the compiler's own warning cannot say.
///
/// The plain `count` over `count` further down is not the lint's to report — it is
/// `tl:redeclaration`, which says the same thing about it and more. This test asserted
/// both messages while the lint had a second, generic branch; that branch is gone, so what
/// it holds now is that the module case survives and the ordinary one is left to Teal.
#[test]
fn shadowing_a_required_module_is_named_as_such() {
    let dir = scratch("shadow-mod");
    write(
        &dir.join("bestiary.tl"),
        "local record bestiary\nend\nfunction bestiary.note()\nend\nreturn bestiary\n",
    );
    write(
        &dir.join("world.tl"),
        "local bestiary = require(\"bestiary\")\nlocal record world\nend\n\
         function world.new(seed: integer, bestiary: {string}): integer\n   return seed + #bestiary\nend\n\
         local count = 0\nfunction world.tick()\n   local count = 1\n   print(count)\nend\n\
         print(bestiary)\nreturn world\n",
    );
    let h = Htl::new().unwrap();
    h.add_path(&dir).unwrap();
    let ci = h.check(&dir.join("world.tl")).unwrap();
    let shadows: Vec<&String> = ci
        .lints
        .iter()
        .filter(|l| l.contains("shadow-local"))
        .collect();
    assert_eq!(shadows.len(), 1, "{shadows:?}");
    assert!(shadows[0].contains("world.tl:4:"), "{}", shadows[0]);
    assert!(
        shadows[0].contains("shadows the module 'bestiary' required at line 1"),
        "{}",
        shadows[0]
    );
    // The local `count` over the file-level `count`, on line 9, is reported once — by the
    // compiler, whose message carries the origin's column as well as its line.
    let plain: Vec<&String> = ci
        .warnings
        .iter()
        .filter(|w| w.contains("world.tl:9:"))
        .collect();
    assert_eq!(plain.len(), 1, "{:?}", ci.warnings);
    assert!(
        plain[0].contains("shadows previous declaration of 'count'"),
        "{}",
        plain[0]
    );
}

#[test]
fn user_message_strips_traceback_and_unwraps_host_errors() {
    let h = Htl::new().unwrap();
    let host = h.lua().create_table().unwrap();
    host.set(
        "pages",
        h.lua()
            .create_function(|_, ()| -> mlua::Result<()> {
                Err(mlua::Error::external(
                    "content/no-date.md: front matter: 'date' is required",
                ))
            })
            .unwrap(),
    )
    .unwrap();
    h.preload_value("host", host).unwrap();

    let err = h
        .exec(
            "local host = require('host')\nhost.pages()\n",
            "=main.lua",
            &[],
        )
        .unwrap_err();
    let raw = err.to_string();
    assert!(
        raw.contains("stack traceback"),
        "precondition: mlua includes a traceback: {raw}"
    );
    let msg = user_message(&err);
    assert_eq!(msg, "content/no-date.md: front matter: 'date' is required");

    // A plain Lua error keeps its `file:line:` prefix but drops the traceback.
    let err = h.exec("error('boom')", "=s.lua", &[]).unwrap_err();
    let msg = user_message(&err);
    assert!(msg.ends_with("s.lua:1: boom"), "{msg}");
    assert!(!msg.contains("traceback"), "{msg}");
}

/// A field named `where` on the first line of a record body is a bare "syntax error"
/// from Teal, with a follow-on error pointing at the *next* field's line. Only the
/// position is at fault -- the field loop has not started yet, and `where` is where a
/// union variant's predicate goes -- so the message names both ways out and the
/// follow-on error goes away with the cause.
#[test]
fn where_as_a_record_bodys_first_field_is_explained() {
    let dir = scratch("where-first");
    let file = dir.join("FindArgs.tl");
    write(
        &file,
        "local record FindArgs\n   where: any\n   pkg: string\nend\nreturn FindArgs\n",
    );
    let h = Htl::new().unwrap();
    let ci = h.check(&file).unwrap();
    assert!(!ci.ok(), "precondition: Teal rejects the field");
    assert_eq!(ci.errors.len(), 1, "follow-on dropped: {:?}", ci.errors);
    let e = &ci.errors[0];
    assert!(e.contains("FindArgs.tl:2:9:"), "{e}");
    assert!(e.contains("'where' opens a union predicate"), "{e}");
    assert!(e.contains("[\"where\"]: <type>"), "{e}");
    assert!(e.contains("put another field first"), "{e}");
    // `htl fix` recognises a file the parser rejected by this word, and the parse did fail.
    assert!(e.contains("syntax error"), "{e}");
}

/// The same body with the field one line down: Teal's field loop has begun, `where` is
/// an ordinary identifier, and nothing is wrong. Saying "`where` is reserved in a record
/// body" would contradict this.
#[test]
fn where_after_the_first_field_type_checks() {
    let dir = scratch("where-second");
    let file = dir.join("FindArgs.tl");
    write(
        &file,
        "local record FindArgs\n   pkg: string\n   where: any\nend\nreturn FindArgs\n",
    );
    let h = Htl::new().unwrap();
    let ci = h.check(&file).unwrap();
    assert!(ci.ok(), "{:?}", ci.errors);
}

/// The spelling the message offers, in the position that fails without it: the field
/// loop's bracketed-string-literal branch takes any name, first line included.
#[test]
fn quoted_where_as_the_first_field_type_checks() {
    let dir = scratch("where-quoted");
    let file = dir.join("FindArgs.tl");
    write(
        &file,
        "local record FindArgs\n   [\"where\"]: any\n   pkg: string\nend\nreturn FindArgs\n",
    );
    let h = Htl::new().unwrap();
    let ci = h.check(&file).unwrap();
    assert!(ci.ok(), "{:?}", ci.errors);
}

/// A different mistake on a first field's line keeps Teal's own text, follow-on error
/// and all: the explanation is proved by re-parsing with `["where"]` in place, so a
/// parse failure that rewrite does not cure never gets it.
#[test]
fn an_unrelated_first_field_syntax_error_is_untouched() {
    let dir = scratch("where-unrelated");
    let file = dir.join("FindArgs.tl");
    write(
        &file,
        "local record FindArgs\n   pkg string\n   n: integer\nend\nreturn FindArgs\n",
    );
    let h = Htl::new().unwrap();
    let ci = h.check(&file).unwrap();
    assert!(!ci.ok(), "precondition: Teal rejects the field");
    assert_eq!(ci.errors.len(), 2, "{:?}", ci.errors);
    assert!(
        ci.errors
            .iter()
            .all(|e| e.contains("expected ':' for an attribute") && !e.contains("union predicate")),
        "{:?}",
        ci.errors
    );
}

/// The matchers declared on `Expect` in `htl/test.d.tl`, read the way the test's subject
/// is meant to be read: out of the declaration. Two lists written by hand -- one in the
/// checker, one here -- would agree until somebody added a matcher.
fn declared_matchers(record: &str) -> Vec<String> {
    let src = include_str!("../lua/test.d.tl");
    let body = src
        .split_once(&format!("record {record}<"))
        .expect("record in test.d.tl")
        .1
        .split_once("\n   end")
        .expect("record end")
        .0;
    body.lines()
        .filter_map(|l| l.trim().split_once(": function("))
        .map(|(name, _)| name.to_string())
        .collect()
}

/// `t.expect(#params):to_be(2)` is refused as `invalid key 'to_be' in type
/// Expect<integer>`, which names the key that is wrong and not one that is right. The
/// message carries the set the declaration allows, and keeps Teal's own text in front of
/// it so anything matching on that keeps matching.
#[test]
fn an_unknown_matcher_names_the_matchers() {
    let dir = scratch("matchers");
    let file = dir.join("m_test.tl");
    write(
        &file,
        "local t = require(\"htl.test\")\n\
         t.describe(\"x\", function()\n   t.it(\"y\", function()\n      t.expect(1):to_be(1)\n   end)\nend)\n",
    );
    let h = Htl::new().unwrap();
    h.install_test_lib().unwrap();
    h.add_path(&dir).unwrap();
    let ci = h.check(&file).unwrap();
    assert_eq!(ci.errors.len(), 1, "{:?}", ci.errors);
    let e = &ci.errors[0];
    assert!(
        e.contains("m_test.tl:4:") && e.contains("invalid key 'to_be' in type Expect<integer>;"),
        "Teal's text is the prefix: {e}"
    );
    let declared = declared_matchers("Expect");
    assert!(declared.len() > 10, "precondition: {declared:?}");
    assert!(
        declared.iter().all(|m| e.contains(m.as_str())),
        "every declared matcher is named: {declared:?} in {e}"
    );
    assert!(e.contains("(README, \"Tests\")"), "{e}");
}

/// `expect_all` hands back a different record, and the message answers for that one:
/// `Expect2` declares one matcher, so one is what it lists.
#[test]
fn the_two_value_expect_lists_its_own_matcher() {
    let dir = scratch("matchers2");
    let file = dir.join("m2_test.tl");
    write(
        &file,
        "local t = require(\"htl.test\")\n\
         local function two(): integer, string\n   return 1, \"a\"\nend\n\
         t.describe(\"x\", function()\n   t.it(\"y\", function()\n      t.expect_all(two()):to_be(1, \"a\")\n   end)\nend)\n",
    );
    let h = Htl::new().unwrap();
    h.install_test_lib().unwrap();
    h.add_path(&dir).unwrap();
    let ci = h.check(&file).unwrap();
    assert_eq!(ci.errors.len(), 1, "{:?}", ci.errors);
    let e = &ci.errors[0];
    assert_eq!(declared_matchers("Expect2"), vec!["to_equal".to_string()]);
    assert!(
        e.contains(
            "in type Expect2<integer, string>; the matchers are to_equal (README, \"Tests\")"
        ),
        "{e}"
    );
}

/// A matcher that exists says nothing, and neither does an `invalid key` about a record
/// of the project's own: the list is read out of the file this file's `require("htl.test")`
/// resolved to, so a project with an `Expect` of its own is not told about these.
#[test]
fn a_valid_matcher_and_a_foreign_record_get_no_matcher_list() {
    let dir = scratch("matchers-quiet");
    let h = Htl::new().unwrap();
    h.install_test_lib().unwrap();
    h.add_path(&dir).unwrap();

    let ok = dir.join("ok_test.tl");
    write(
        &ok,
        "local t = require(\"htl.test\")\n\
         t.describe(\"x\", function()\n   t.it(\"y\", function()\n      t.expect(1):to_equal(1)\n   end)\nend)\n",
    );
    let ci = h.check(&ok).unwrap();
    assert!(ci.ok(), "{:?}", ci.errors);

    let own = dir.join("own.tl");
    write(
        &own,
        "local record box\n   record Expect<T>\n      get: function(self): T\n   end\nend\n\
         local e: box.Expect<integer>\nprint(e:to_be(1))\nreturn box\n",
    );
    let ci = h.check(&own).unwrap();
    let e = ci
        .errors
        .iter()
        .find(|e| e.contains("'to_be'"))
        .expect("invalid key error");
    assert!(!e.contains("the matchers are"), "{e}");
}
