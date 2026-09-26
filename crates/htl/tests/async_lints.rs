//! The four checker rules of the `async` / `await` syntax (step 5 of the async design):
//! `await-missing`, `await-outside-async`, `await-non-async`, `task-escape`. Which callee
//! is async is read off its declaration — the keyword on a Teal `async function`, the
//! trailing `---@async` on a `.d.tl` line — the way `---@nilable` is; the context an
//! `await` sits in is the function's (`async` or not), the top level of the checked file
//! being async and a required module's not.
#![cfg(feature = "async")]

use htl::Htl;
use htl::config::LangConfig;
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("htl-async-lints-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, text).unwrap();
    p
}

fn checker(dir: &Path, on: bool, spec: &str) -> Htl {
    let h = Htl::new().unwrap();
    h.set_lang(&LangConfig { async_: Some(on) }).unwrap();
    if !spec.is_empty() {
        h.configure_lints(spec).unwrap();
    }
    h.install_task_lib().unwrap();
    h.add_path(dir).unwrap();
    h.add_path(&dir.join("types")).unwrap();
    h
}

const HTTP: &str = "\
local record http
   get: function(path: string): string ---@async
   sync: function(path: string): string
end
return http
";

/// Line 10: an async host method called without `await`. Line 11: `await` on a sync one.
/// Line 12: the task `x` captured by a closure. Lines 17-19: `await` and `async local` in a
/// function that is not async. Line 24: the task `q` returned bare. Everything else is
/// what the syntax is for and says nothing.
const MAIN: &str = "\
local http = require(\"http\")

local async function work(n: integer): integer
   return n
end

local async function pair(a: string, b: string): string, string
   async local x = http.get(a)
   local y = http.get(b)
   local z = await http.sync(b)
   local f = function(): string return x:await() end
   return await x, y .. z .. f()
end

local function plain(): integer
   local v = await work(1)
   async local w = work(2)
   return v + await w
end

local async function leak(): any
   async local q = work(3)
   return q
end

print(await pair(\"/a\", \"/b\"), plain())
print(await leak())
";

fn of_rule<'a>(lints: &'a [String], rule: &str) -> Vec<&'a String> {
    let tag = format!("[htl {rule}]");
    lints.iter().filter(|l| l.contains(&tag)).collect()
}

fn at<'a>(lints: &'a [String], pos: &str) -> Vec<&'a String> {
    lints.iter().filter(|l| l.contains(pos)).collect()
}

#[test]
fn each_rule_reports_its_case_at_the_line_and_column() {
    let dir = scratch("rules");
    write(&dir, "types/http.d.tl", HTTP);
    let main = write(&dir, "main.tl", MAIN);
    let ci = checker(&dir, true, "").check(&main).unwrap();
    assert!(ci.errors.is_empty(), "{:?}", ci.errors);
    let l = &ci.lints;
    // await-missing: the call, at the callee's first token.
    let m = of_rule(l, "await-missing");
    assert_eq!(m.len(), 1, "{l:?}");
    assert!(
        m[0].contains(
            "main.tl:9:14: call of an async function without await: http.get may suspend"
        ),
        "{}",
        m[0]
    );
    // await-non-async: at the keyword.
    let n = of_rule(l, "await-non-async");
    assert_eq!(n.len(), 1, "{l:?}");
    assert!(n[0].contains("main.tl:10:14: await on a call of a function that is not async: http.sync cannot suspend"), "{}", n[0]);
    // task-escape: captured at the use inside the closure, returned bare at the name.
    let e = of_rule(l, "task-escape");
    assert_eq!(e.len(), 2, "{l:?}");
    assert!(
        e[0].contains("main.tl:11:40: task 'x' is captured by a function"),
        "{}",
        e[0]
    );
    assert!(
        e[1].contains("main.tl:23:11: task 'q' is returned"),
        "{}",
        e[1]
    );
    // await-outside-async: the keyword of each, in the sync function.
    let o = of_rule(l, "await-outside-async");
    assert_eq!(o.len(), 3, "{l:?}");
    assert!(
        o[0].contains("main.tl:16:14: await in a function that is not async"),
        "{}",
        o[0]
    );
    assert!(
        o[1].contains("main.tl:17:4: async local in a function that is not async"),
        "{}",
        o[1]
    );
    assert!(
        o[2].contains("main.tl:18:15: await in a function that is not async"),
        "{}",
        o[2]
    );
    // The expression of an `async local` is the task's body: `http.get(a)` on line 8 and
    // `work(2)` / `work(3)` are not `await-missing`; `return await x` is not `task-escape`;
    // the entry's top-level awaits are fine.
    assert!(
        at(l, ":8:").is_empty()
            && at(l, ":22:").is_empty()
            && at(l, ":26:").is_empty()
            && at(l, ":27:").is_empty(),
        "{l:?}"
    );
    assert_eq!(l.len(), 7, "{l:?}");
}

/// A module's top level is loaded by `require`, which cannot yield: reported on the check
/// of the file that requires it, at the module's line. The same text checked directly is
/// read as a possible entry and its top level says nothing.
#[test]
fn a_required_modules_top_level_await_is_reported_by_the_requirer_and_not_by_its_own_check() {
    let dir = scratch("module");
    write(&dir, "types/http.d.tl", HTTP);
    let module = write(
        &dir,
        "mod.tl",
        "local http = require(\"http\")\nlocal mod = {}\nlocal x = await http.get(\"/top\")\nasync local t = http.get(\"/t\")\nfunction mod.f(): string\n   return x .. tostring(t:done())\nend\nreturn mod\n",
    );
    let main = write(
        &dir,
        "main.tl",
        "local mod = require(\"mod\")\nprint(mod.f())\n",
    );
    let h = checker(&dir, true, "");
    let ci = h.check(&main).unwrap();
    assert!(ci.errors.is_empty(), "{:?}", ci.errors);
    let o = of_rule(&ci.lints, "await-outside-async");
    assert_eq!(o.len(), 2, "{:?}", ci.lints);
    assert!(
        o[0].contains("mod.tl:3:11: await at the top level of a module: 'require' cannot yield"),
        "{}",
        o[0]
    );
    assert!(
        o[1].contains("mod.tl:4:1: async local at the top level of a module"),
        "{}",
        o[1]
    );
    // Directly: the top level is a possible entry.
    let own = h.check(&module).unwrap();
    assert!(own.errors.is_empty(), "{:?}", own.errors);
    assert!(
        of_rule(&own.lints, "await-outside-async").is_empty(),
        "{:?}",
        own.lints
    );
}

/// A Teal `async function` in another module is async at its call: the keyword is on the
/// declaring line, which the resolver reads the way it reads `---@async`.
#[test]
fn an_async_function_of_a_required_module_needs_await_at_its_call() {
    let dir = scratch("teal-callee");
    write(
        &dir,
        "lib.tl",
        "local lib = {}\nasync function lib.fetch(p: string): string\n   return p\nend\nreturn lib\n",
    );
    let main = write(
        &dir,
        "main.tl",
        "local lib = require(\"lib\")\nlocal a = lib.fetch(\"/a\")\nlocal b = await lib.fetch(\"/b\")\nprint(a, b)\n",
    );
    let ci = checker(&dir, true, "").check(&main).unwrap();
    assert!(ci.errors.is_empty(), "{:?}", ci.errors);
    let m = of_rule(&ci.lints, "await-missing");
    assert_eq!(m.len(), 1, "{:?}", ci.lints);
    assert!(
        m[0].contains(
            "main.tl:2:11: call of an async function without await: lib.fetch may suspend"
        ),
        "{}",
        m[0]
    );
    assert!(
        of_rule(&ci.lints, "await-non-async").is_empty(),
        "{:?}",
        ci.lints
    );
}

/// The levels are the registry's, and a spec moves them like any other rule's.
#[test]
fn a_spec_turns_a_rule_off_or_up() {
    let dir = scratch("levels");
    write(&dir, "types/http.d.tl", HTTP);
    let main = write(&dir, "main.tl", MAIN);
    let off = checker(&dir, true, "-await-non-async,-task-escape")
        .check(&main)
        .unwrap();
    assert!(
        of_rule(&off.lints, "await-non-async").is_empty(),
        "{:?}",
        off.lints
    );
    assert!(
        of_rule(&off.lints, "task-escape").is_empty(),
        "{:?}",
        off.lints
    );
    assert_eq!(
        of_rule(&off.lints, "await-missing").len(),
        1,
        "{:?}",
        off.lints
    );
    assert_eq!(
        htl::lint::RULES
            .iter()
            .find(|r| r.name == "await-non-async")
            .unwrap()
            .default,
        htl::lint::Level::Warn
    );
    assert_eq!(
        htl::lint::RULES
            .iter()
            .find(|r| r.name == "await-missing")
            .unwrap()
            .default,
        htl::lint::Level::Deny
    );
}

/// With the setting off the words are names and no rule applies; a `.d.tl` with the
/// marker is an ordinary declaration then.
#[test]
fn with_the_setting_off_nothing_is_reported() {
    let dir = scratch("off");
    write(&dir, "types/http.d.tl", HTTP);
    let main = write(
        &dir,
        "main.tl",
        "local http = require(\"http\")\nlocal await = 1\nlocal async = 2\nprint(http.get(\"/a\"), await + async)\n",
    );
    let ci = checker(&dir, false, "").check(&main).unwrap();
    assert!(ci.errors.is_empty(), "{:?}", ci.errors);
    assert!(
        ci.lints
            .iter()
            .all(|l| !l.contains("[htl await") && !l.contains("[htl task-escape]")),
        "{:?}",
        ci.lints
    );
}

/// `#[host_module]` writes the marker for an `async fn`, so a `.d.tl` it produced drives
/// the rule the way a hand-written one does: the declaration text is `dts`'s, the same
/// for the macro and `htl dts`.
#[test]
fn the_macro_declares_an_async_fn_with_the_marker() {
    pub struct Api;
    #[htl::host_module(name = "api")]
    impl Api {
        pub async fn fetch(&self, path: String) -> String {
            path
        }
        pub fn sync(&self, path: String) -> String {
            path
        }
    }
    let decl = <Api as htl::teal::HostModule>::DECL;
    assert!(
        decl.contains("fetch: function(self: api, path: string): string ---@async\n"),
        "{decl}"
    );
    assert!(
        decl.contains("sync: function(self: api, path: string): string\n"),
        "{decl}"
    );
    let dir = scratch("macro");
    write(&dir, "types/api.d.tl", decl);
    let main = write(
        &dir,
        "main.tl",
        "local api = require(\"api\")\nlocal a: api = nil\nprint(a:fetch(\"/x\"), await a:sync(\"/y\"))\n",
    );
    let ci = checker(&dir, true, "").check(&main).unwrap();
    assert!(ci.errors.is_empty(), "{:?}", ci.errors);
    let m = of_rule(&ci.lints, "await-missing");
    assert_eq!(m.len(), 1, "{:?}", ci.lints);
    assert!(
        m[0].contains("main.tl:3:7: call of an async function without await: a:fetch may suspend"),
        "{}",
        m[0]
    );
    let n = of_rule(&ci.lints, "await-non-async");
    assert_eq!(n.len(), 1, "{:?}", ci.lints);
    assert!(
        n[0].contains(
            "main.tl:3:22: await on a call of a function that is not async: a:sync cannot suspend"
        ),
        "{}",
        n[0]
    );
}
