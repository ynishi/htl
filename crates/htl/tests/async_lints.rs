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
    write(&dir, "types/api.d.tl", API);
    let main = write(
        &dir,
        "main.tl",
        "local http = require(\"http\")\nlocal await = 1\nlocal async = 2\nprint(http.get(\"/a\"), await + async)\nlocal t = { \"b\", \"a\" }\ntable.sort(t, function(a: string, b: string): boolean return a < b end)\nprint(string.gsub(\"a\", \"%w\", http.get))\nlocal api = require(\"api\")\nprint(api:each(api.fetch), api.each(api, api.fetch))\nlocal game: api.Game = { update = api.tick, draw = function() end }\nprint(game)\n",
    );
    let ci = checker(&dir, false, "").check(&main).unwrap();
    assert!(ci.errors.is_empty(), "{:?}", ci.errors);
    assert!(
        ci.lints.iter().all(|l| !l.contains("[htl await")
            && !l.contains("[htl task-escape]")
            && !l.contains("[htl async-as-sync-callback]")),
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

const ORDER: &str = "\
local record order
   less: function(a: string, b: string): boolean ---@async
end
return order
";

/// Lines 35-37 and 40-41: an async function handed to `table.sort` or `string.gsub`, which
/// call it from C — a local `async function`, a host method declared `---@async`, an inline
/// `async function(..)`. Line 42: `xpcall`'s handler (its body, also async, is resumed and
/// may yield). Line 43: an async `__tostring` in the metatable's constructor, for a record
/// that declares `metamethod __tostring` (line 31). Line 38 is an inline sync comparator;
/// line 39 is one whose body awaits, which is `await-outside-async` at the `await` and
/// nothing here — as is the named `awaiting_cmp` at line 17. Line 44: `gsub` on a record
/// that is not a string (`d`, a `Doc`), a Teal method that may take an async function.
/// Line 46: `s:gsub` on a string, which is `string.gsub` and is reported like line 40.
const CALLBACKS: &str = "\
local http = require(\"http\")
local order = require(\"order\")
local Doc = require(\"doc\")
local async function cmp(a: string, b: string): boolean
   return a < b
end
local d: Doc = nil
local async function shout(w: string): string
   return w:upper()
end

local function sync_cmp(a: string, b: string): boolean
   return a < b
end

local function awaiting_cmp(a: string, b: string): boolean
   local s = await http.get(a)
   return s < b
end

local async function body(): string
   return \"ok\"
end

local async function handler(e: any): string
   return tostring(e)
end

local record R
   name: string
   metamethod __tostring: function(R): string
end
local async function show(r: R): string return r.name end
local items: {string} = { \"b\", \"a\" }
table.sort(items, cmp)
table.sort(items, order.less)
table.sort(items, async function(a: string, b: string): boolean return a < b end)
table.sort(items, function(a: string, b: string): boolean return a < b end)
table.sort(items, function(a: string, b: string): boolean return (await http.get(a)) < b end)
print(string.gsub(\"a b\", \"%w+\", shout))
print(string.gsub(\"a b\", \"%w+\", http.get))
print(xpcall(body, handler))
local r: R = setmetatable({ name = \"r\" }, { __tostring = show } as metatable<R>)
print(r, d:gsub(\"%w+\", shout))
local s = \"a b\"
print(s:gsub(\"%w+\", shout))
";

const DOC: &str = "\
local record Doc
   gsub: function(self: Doc, pat: string, f: function(string): string): string
end
return Doc
";

#[test]
fn an_async_function_handed_to_a_c_callee_is_reported_at_the_argument() {
    let dir = scratch("callbacks");
    write(&dir, "types/http.d.tl", HTTP);
    write(&dir, "types/order.d.tl", ORDER);
    write(&dir, "types/doc.d.tl", DOC);
    let main = write(&dir, "main.tl", CALLBACKS);
    let ci = checker(&dir, true, "").check(&main).unwrap();
    assert!(ci.errors.is_empty(), "{:?}", ci.errors);
    let l = &ci.lints;
    let c = of_rule(l, "async-as-sync-callback");
    let want = [
        "main.tl:35:19: async function passed to table.sort",
        "main.tl:36:19: async function passed to table.sort",
        "main.tl:37:19: async function passed to table.sort",
        "main.tl:40:33: async function passed to string.gsub",
        "main.tl:41:33: async function passed to string.gsub",
        "main.tl:42:20: async function passed to xpcall",
        "main.tl:43:58: async function bound to __tostring",
        "main.tl:46:21: async function passed to string.gsub",
    ];
    assert_eq!(c.len(), want.len(), "{c:?}");
    for (got, w) in c.iter().zip(want) {
        assert!(got.contains(w), "{got} lacks {w}");
    }
    assert!(
        c[0].contains("attempt to yield across a C-call boundary"),
        "{}",
        c[0]
    );
    assert!(c[5].contains("error in error handling"), "{}", c[5]);
    // The sync comparators, `xpcall`'s body and the record's `gsub` say nothing here; the
    // await inside the sync one is the rule for that.
    for pos in [":38:", ":39:", ":42:14:", ":44:"] {
        assert!(!c.iter().any(|s| s.contains(pos)), "{pos} in {c:?}");
    }
    let o = of_rule(l, "await-outside-async");
    assert_eq!(o.len(), 2, "{l:?}");
    assert!(
        o[0].contains("main.tl:17:14: await in a function that is not async"),
        "{}",
        o[0]
    );
    assert!(
        o[1].contains("main.tl:39:67: await in a function that is not async"),
        "{}",
        o[1]
    );
    // A spec turns it off like any other rule; the default is deny.
    let off = checker(&dir, true, "-async-as-sync-callback")
        .check(&main)
        .unwrap();
    assert!(
        of_rule(&off.lints, "async-as-sync-callback").is_empty(),
        "{:?}",
        off.lints
    );
    assert_eq!(
        htl::lint::RULES
            .iter()
            .find(|r| r.name == "async-as-sync-callback")
            .unwrap()
            .default,
        htl::lint::Level::Deny
    );
}

/// A host's own C boundary, declared on its `.d.tl`: `---@noyield(f)` names the parameters
/// the host calls from C, and a bare `---@noyield` on a record field says the host calls
/// that field of a table it is handed.
const API: &str = "\
local record api
   record Game
      update: function(dt: number): boolean ---@noyield
      draw: function() ---@noyield
   end
   each: function(self: api, f: function): string ---@noyield(f)
   each_async: function(self: api, f: function): string ---@async
   plain: function(self: api, f: function)
   split: function(self: api, on_ok: function, on_err: function) ---@noyield(on_ok, on_err)
   walk: function(self: api, f?: function) ---@noyield(f)
   fetch: function(self: api, path: string): string ---@async
   tick: function(dt: number): boolean ---@async
   mk: function(self: api, f: function): function(a: string, b: function) ---@noyield(f)
   first: function( ---@noyield(f)
      self: api,
      f: function
   )
   vr: function(self: api, ...: function) ---@noyield(...)
   run: function(self: api, game: Game)
   record Sub
      each: function(self: Sub, f: function) ---@noyield(f)
   end
   sub: Sub
   free: function(f: function) ---@noyield(f)
   nself: function(a: api, f: function) ---@noyield(f)
end
return api
";

/// Lines 10-12: an async function handed to `each`, whose declaration marks `f`: a local
/// `async function`, an inline one, and the `.` form, where `api` itself is the first
/// argument and `cb` the second. Line 17: both parameters of `split` are marked. Line 18:
/// `walk`'s optional `f?`. Line 20: an async `update` in a constructor typed `api.Game`,
/// whose field line says the host calls it. Silent: line 13 (`each_async` is async and
/// marks nothing), line 14 (`plain`, no marker), line 15 (a Teal function calling its
/// callback, which may yield), line 16 (a sync callback into `each`), line 21 (`draw` is
/// marked but its value is not async), line 23 (a sync `update`). Lines 26-27: `mk`'s return
/// type sits on `mk`'s line and has parameters of its own, none of them marked, so calling the
/// returned function is silent. Line 28: a declaration written one parameter per line. Line
/// 29: `...` marked, the async value second among the varargs. Line 30: a constructor in
/// argument position, typed by the parameter. Line 32: an alias of `api.each`. Line 33: a
/// method of a nested record. Line 34: a free function called with `.`, its `f` the first
/// argument. Line 35: a first parameter not spelled `self`, called with `:`. Line 36: a
/// string key. Line 37: a named async value (`tick`, declared `---@async`) bound to a field.
const HOST_CALLBACKS: &str = "\
local api = require(\"api\")
local async function cb(): string
   return await api:fetch(\"/a\")
end
local function sync_cb(): string return \"s\" end
local function each(f: function): string
   f()
   return \"t\"
end
print(api:each(cb))
print(api:each(async function(): string return await api:fetch(\"/b\") end))
print(api.each(api, cb))
print(await api:each_async(cb))
api:plain(cb)
print(each(cb))
print(api:each(sync_cb))
api:split(cb, async function() end)
api:walk(cb)
local game: api.Game = {
   update = async function(dt: number): boolean return dt > 0 end,
   draw = function() end,
}
local calm: api.Game = { update = function(dt: number): boolean return dt > 0 end, draw = function() end }
print(game, calm)
local r = api:mk(function() end)
r(\"x\", cb)
api:mk(function() end)(\"y\", cb)
api:first(cb)
api:vr(function() end, cb)
api:run({ update = async function(dt: number): boolean return dt > 0 end, draw = function() end })
local run = api.each
print(run(api, cb))
api.sub:each(cb)
api.free(cb)
api:nself(cb)
local keyed: api.Game = { [\"update\"] = async function(dt: number): boolean return dt > 0 end, draw = function() end }
local named: api.Game = { update = api.tick, draw = function() end }
print(keyed, named)
";

#[test]
fn an_async_function_handed_to_a_host_parameter_marked_noyield_is_reported_at_the_argument() {
    let dir = scratch("noyield");
    write(&dir, "types/api.d.tl", API);
    let main = write(&dir, "main.tl", HOST_CALLBACKS);
    let ci = checker(&dir, true, "").check(&main).unwrap();
    assert!(ci.errors.is_empty(), "{:?}", ci.errors);
    let l = &ci.lints;
    let c = of_rule(l, "async-as-sync-callback");
    let want = [
        "main.tl:10:16: async function passed to api:each: its declaration says 'f' is called from C (---@noyield)",
        "main.tl:11:16: async function passed to api:each: its declaration says 'f' is called from C (---@noyield)",
        "main.tl:12:21: async function passed to api.each: its declaration says 'f' is called from C (---@noyield)",
        "main.tl:17:11: async function passed to api:split: its declaration says 'on_ok' is called from C (---@noyield)",
        "main.tl:17:15: async function passed to api:split: its declaration says 'on_err' is called from C (---@noyield)",
        "main.tl:18:10: async function passed to api:walk: its declaration says 'f' is called from C (---@noyield)",
        "main.tl:20:13: async function bound to update of api.Game: the record's declaration says the field is called from C (---@noyield)",
        "main.tl:28:11: async function passed to api:first: its declaration says 'f' is called from C (---@noyield)",
        "main.tl:29:24: async function passed to api:vr: its declaration says '...' is called from C (---@noyield)",
        "main.tl:30:20: async function bound to update of api.Game: the record's declaration says the field is called from C (---@noyield)",
        "main.tl:32:16: async function passed to run: its declaration says 'f' is called from C (---@noyield)",
        "main.tl:33:14: async function passed to api.sub:each: its declaration says 'f' is called from C (---@noyield)",
        "main.tl:34:10: async function passed to api.free: its declaration says 'f' is called from C (---@noyield)",
        "main.tl:35:11: async function passed to api:nself: its declaration says 'f' is called from C (---@noyield)",
        "main.tl:36:40: async function bound to update of api.Game: the record's declaration says the field is called from C (---@noyield)",
        "main.tl:37:36: async function bound to update of api.Game: the record's declaration says the field is called from C (---@noyield)",
    ];
    assert_eq!(c.len(), want.len(), "{c:?}");
    for (got, w) in c.iter().zip(want) {
        assert!(got.contains(w), "{got} lacks {w}");
    }
    assert!(
        c.iter()
            .all(|s| s.contains("attempt to yield across a C-call boundary")),
        "{c:?}"
    );
    for pos in [
        "main.tl:13:",
        "main.tl:14:",
        "main.tl:15:",
        "main.tl:16:",
        "main.tl:21:",
        "main.tl:23:",
        "main.tl:26:",
        "main.tl:27:",
        "main.tl:29:8:",
    ] {
        assert!(!c.iter().any(|s| s.contains(pos)), "{pos} in {c:?}");
    }
    // Nothing else in the file is a finding: the awaits are in async context and awaited.
    assert_eq!(l.len(), want.len(), "{l:?}");
}
