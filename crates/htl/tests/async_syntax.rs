//! `async` / `await` under `[lang] async` (step 4 of the async design): the two words are
//! keywords the checker rewrites — `local async function f` marks `f`, `async local x = e`
//! is checked as `require("htl.task").of(e)` and generated as `.spawn(function() return e
//! end)`, `await x` of an `async local` is `x:await()`, `await f()` marks the call — and
//! the generated Lua keeps every line where the Teal had it. Off, the words are names.
#![cfg(feature = "async")]

use htl::Htl;
use htl::config::LangConfig;
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("htl-async-syntax-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, text).unwrap();
    p
}

/// A checker reading the project's Teal with the two keywords on (or off).
fn checker(dir: &Path, on: bool) -> Htl {
    let h = Htl::new().unwrap();
    h.set_lang(&LangConfig { async_: Some(on) }).unwrap();
    h.install_task_lib().unwrap();
    h.add_path(dir).unwrap();
    h
}

const PROGRAM: &str = "\
local async function work(n: integer): integer
   local s = 0
   for i = 1, n do s = s + i end
   return s
end

local async function both(): integer, integer
   async local a = work(10)
   async local b = work(100)
   return await a, await b
end

local async function more(): integer
   async local c = work(3)
   local x = await work(2) + 1
   return x + await c
end

print(await both(), await more())
";

/// The generated Lua, line by line beside the Teal: the rewrites are on the lines the
/// keywords were on, and no line is added or removed.
#[test]
fn gen_keeps_every_line_and_rewrites_the_four_forms_in_place() {
    let dir = scratch("gen");
    let file = write(&dir, "main.tl", PROGRAM);
    let h = checker(&dir, true);
    let (code, c) = h.gen_lua(&file).unwrap();
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let code = code.expect("generated");
    let want = "\
local function work(n)
   local s = 0
   for i = 1, n do s = s + i end
   return s
end

local function both()
   local a <close> = require(\"htl.task\").spawn(function() return work(10) end)
   local b <close> = require(\"htl.task\").spawn(function() return work(100) end)
   return a:await(), b:await()
end

local function more()
   local c <close> = require(\"htl.task\").spawn(function() return work(3) end)
   local x = work(2) + 1
   return x + c:await()
end

print(both(), more())
";
    assert_eq!(code.lines().count(), PROGRAM.lines().count());
    for (n, (got, exp)) in code.lines().zip(want.lines()).enumerate() {
        assert_eq!(got, exp, "line {}", n + 1);
    }
    assert_eq!(code, want);
}

/// The value of an `async local` is typed from its expression: a `Task<integer>` awaited
/// into a `string` is refused at the Teal position.
#[test]
fn an_awaited_task_has_the_type_of_its_expression() {
    let dir = scratch("check");
    let file = write(
        &dir,
        "main.tl",
        "local async function work(n: integer): integer return n end
local async function f(): string
   async local a = work(10)
   local s: string = await a
   return s
end
print(await f())
",
    );
    let h = checker(&dir, true);
    let c = h.check(&file).unwrap();
    assert_eq!(c.errors.len(), 1, "{:?}", c.errors);
    assert!(
        c.errors[0].contains("main.tl:4:") && c.errors[0].contains("got integer, expected string"),
        "{}",
        c.errors[0]
    );
}

/// With the keywords on, the two words are still names where a name is the only reading:
/// a field, a method, a key, a function so named, a variable so named.
#[test]
fn the_words_stay_names_after_a_dot_or_before_a_definition() {
    let dir = scratch("names");
    let file = write(
        &dir,
        "main.tl",
        "local record R
   await: integer
   async: string
end
local r: R = { await = 1, async = \"x\" }
local t = { await = 2 }
local function await(n: integer): integer return n end
local async = 3
local M = {}
function M.await(): integer return 7 end
local function go(): integer return r.await + t.await + await(4) + async + M.await() end
print(go())
",
    );
    let h = checker(&dir, true);
    let c = h.check(&file).unwrap();
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let (code, _) = h.gen_lua(&file).unwrap();
    assert!(
        code.unwrap()
            .contains("r.await + t.await + await(4) + async + M.await()")
    );
}

/// Off (the default), a file using the words as names checks and generates as it always
/// did, and a file using them as keywords is a syntax error, as it is for Teal.
#[test]
fn off_the_words_are_names_and_the_keywords_are_syntax_errors() {
    let dir = scratch("off");
    let names = write(
        &dir,
        "names.tl",
        "local async = 1
local await = 2
local function f(n: integer): integer return n + async + await end
print(f(3))
",
    );
    let h = checker(&dir, false);
    let c = h.check(&names).unwrap();
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let (code, _) = h.gen_lua(&names).unwrap();
    assert!(code.unwrap().contains("n + async + await"));
    let keywords = write(&dir, "kw.tl", PROGRAM);
    let c = h.check(&keywords).unwrap();
    assert!(!c.errors.is_empty(), "the keywords are not Teal");
}

/// One name per `async local`: a task holds one value.
#[test]
fn an_async_local_with_two_names_is_refused_with_the_rule() {
    let dir = scratch("two");
    let file = write(
        &dir,
        "main.tl",
        "local function pair(): integer, integer return 1, 2 end
local async function f(): integer
   async local a, b = pair()
   return 1
end
print(await f())
",
    );
    let h = checker(&dir, true);
    let c = h.check(&file).unwrap();
    assert!(
        c.errors
            .iter()
            .any(|e| e.contains("main.tl:3:4:")
                && e.contains("'async local' declares one name, not 2")),
        "{:?}",
        c.errors
    );
}

/// `async` before a record function and before an anonymous function marks those too, and
/// the generated Lua is the function without the word.
#[test]
fn async_marks_record_and_anonymous_functions() {
    let dir = scratch("forms");
    let file = write(
        &dir,
        "main.tl",
        "local task = require(\"htl.task\")
local record M
end
async function M.f(n: integer): integer return n end
local h = task.spawn(async function(): integer return 9 end)
print(await M.f(1), h:await())
",
    );
    let h = checker(&dir, true);
    let (code, c) = h.gen_lua(&file).unwrap();
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let code = code.unwrap();
    assert!(code.contains("function M.f(n) return n end"), "{code}");
    assert!(
        code.contains("task.spawn(function() return 9 end)"),
        "{code}"
    );
    assert!(code.contains("print(M.f(1), h:await())"), "{code}");
}

/// A runtime error inside an awaited call names the Teal line, since every line of the
/// generated Lua is the Teal's.
#[test]
fn a_runtime_error_in_an_awaited_call_names_the_teal_line() {
    let dir = scratch("boom");
    let file = write(
        &dir,
        "main.tl",
        "local async function work(n: integer): integer
   if n > 1 then
      error(\"boom \" .. tostring(n))
   end
   return n
end
print(await work(5))
",
    );
    let h = checker(&dir, true);
    let (code, c) = h.gen_lua(&file).unwrap();
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let err = h
        .run_blocking(
            &code.unwrap(),
            "@main.tl",
            &[],
            &htl::mlua_isle::runtime::CancelToken::new(),
        )
        .unwrap_err();
    let text = format!("{err:#}");
    assert!(text.contains("main.tl:3: boom 5"), "{text}");
}
