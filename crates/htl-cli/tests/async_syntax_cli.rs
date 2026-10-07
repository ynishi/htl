//! `async` / `await` through the command, under `[lang] async = true` in `htl.toml`: the
//! program checks clean, runs on the executor, formats to itself, and generates one line
//! of Lua per line of Teal. Without the setting the two words are names (the default).

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-async-syntax", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn htl(args: &[&str], cwd: &Path) -> (bool, String, String) {
    let out = Command::new(common::htl_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
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

print(await both())
";

fn project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), "[lang]\nasync = true\n");
    write(&root.join("main.tl"), PROGRAM);
    root
}

/// `htl check` is clean, `htl run` prints what the two tasks computed, `htl fmt --check`
/// has nothing to change, and `htl gen` writes as many lines as it read.
#[test]
fn a_program_with_the_keywords_checks_runs_formats_and_generates_line_for_line() {
    let root = project("run");
    let (ok, _, err) = htl(&["check", "main.tl"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("0 error(s)"), "{err}");
    let (ok, out, err) = htl(&["run", "main.tl"], &root);
    assert!(ok, "{err}");
    assert_eq!(out, "55\t5050\n");
    let (ok, _, err) = htl(&["fmt", "--check", "main.tl"], &root);
    assert!(ok, "the file is already in its formatted shape: {err}");
    assert!(err.contains("0 would change"), "{err}");
    let (ok, out, err) = htl(&["gen", "main.tl"], &root);
    assert!(ok, "{err}");
    assert_eq!(out.lines().count(), PROGRAM.lines().count(), "{out}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines[7],
        "   local a <close> = require(\"htl.task\").spawn(function() return work(10) end)"
    );
    assert_eq!(lines[9], "   return a:await(), b:await()");
    assert_eq!(lines[12], "print(both())");
}

/// The type of an `async local` is its expression's: awaiting it into the wrong type is
/// refused at the Teal line and column.
#[test]
fn awaiting_a_task_into_the_wrong_type_is_a_check_error_at_the_teal_position() {
    let root = scratch("wrong");
    write(&root.join("htl.toml"), "[lang]\nasync = true\n");
    write(
        &root.join("main.tl"),
        "local async function work(n: integer): integer return n end
local async function f(): string
   async local a = work(10)
   local s: string = await a
   return s
end
print(await f())
",
    );
    let (ok, _, err) = htl(&["check", "main.tl"], &root);
    assert!(!ok);
    assert!(
        err.contains("main.tl:4:") && err.contains("got integer, expected string"),
        "{err}"
    );
}

/// Without `[lang] async`, `async` and `await` are the names they are in Teal.
#[test]
fn without_the_setting_the_words_are_names() {
    let root = scratch("off");
    write(&root.join("htl.toml"), "[fmt]\nindent = 3\n");
    write(
        &root.join("main.tl"),
        "local async = 1
local await = 2
local function f(n: integer): integer return n + async + await end
print(f(3))
",
    );
    let (ok, out, err) = htl(&["run", "main.tl"], &root);
    assert!(ok, "{err}");
    assert_eq!(out, "6\n");
    let (ok, _, err) = htl(&["check", "main.tl"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("0 error(s)"), "{err}");
}

const HTTP: &str = "\
local record http
   get: function(path: string): string ---@async
   sync: function(path: string): string
end
return http
";

/// One of each rule: line 9 an async call without `await`, line 10 `await` on a sync
/// call, line 11 the task captured, line 15 `await` in a function that is not async,
/// line 19 the task returned bare.
const RULES: &str = "\
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
   return await work(1)
end
local async function leak(): any
   async local q = work(3)
   return q
end
print(await pair(\"/a\", \"/b\"), plain(), await leak())
";

/// The four rules print under their names, three of them at `deny`, so the check fails;
/// the second run, replayed from the cache, prints the same lines; `--list-lints` names
/// them with their levels.
#[test]
fn the_four_rules_are_reported_by_name_and_replayed_from_the_cache() {
    let root = scratch("rules");
    write(&root.join("htl.toml"), "[lang]\nasync = true\n");
    write(&root.join("types/http.d.tl"), HTTP);
    write(&root.join("src/main.tl"), RULES);
    let (ok, _, err) = htl(&["check", "src"], &root);
    assert!(!ok, "three rules are deny: {err}");
    for want in [
        "src/main.tl:9:14: call of an async function without await: http.get may suspend; write await http.get(..) so the suspension is visible where it happens [htl await-missing]",
        "src/main.tl:10:14: await on a call of a function that is not async: http.sync cannot suspend; drop the await, or declare the function 'async function' (a host method: ---@async on its declaration) [htl await-non-async]",
        "src/main.tl:11:40: task 'x' is captured by a function: it is cancelled when the scope that declared it ends, so the capture would read a cancelled task; await it in that scope and capture the value [htl task-escape]",
        "src/main.tl:15:11: await in a function that is not async: nothing can suspend here; declare the enclosing function 'async function', or move the await into one [htl await-outside-async]",
        "src/main.tl:19:11: task 'q' is returned: it is cancelled when the scope that declared it ends, so the caller would get a cancelled task; return await q instead [htl task-escape]",
    ] {
        assert!(err.contains(want), "missing {want:?} in {err}");
    }
    assert!(err.contains("5 lint(s), 4 at deny"), "{err}");
    let lines_of = |err: &str| -> Vec<String> {
        err.lines()
            .filter(|l| l.starts_with("lint:"))
            .map(str::to_string)
            .collect()
    };
    let first = lines_of(&err);
    let (ok2, _, err2) = htl(&["check", "src"], &root);
    assert!(!ok2, "{err2}");
    assert!(err2.contains("[cached]"), "the second run replays: {err2}");
    assert_eq!(
        lines_of(&err2),
        first,
        "the replayed lints are the same lines"
    );
    let (ok, out, _) = htl(&["check", "--list-lints"], &root);
    assert!(ok);
    for (rule, level) in [
        ("await-missing", "deny"),
        ("await-outside-async", "deny"),
        ("await-non-async", "warn"),
        ("task-escape", "deny"),
    ] {
        let line = out
            .lines()
            .find(|l| l.trim_start().starts_with(rule))
            .unwrap_or_else(|| panic!("{rule} not listed: {out}"));
        assert!(line.trim_end().ends_with(level), "{line}");
    }
}

const IF_THEN_BODY_PROGRAM: &str = "\
local task = require(\"htl.task\")
local async function f(): integer
   await task.after(1):wait()
   return 1
end
local x = 1
if x == 1 then
   local v = await f()
   print(v)
end
";

/// The issue's reproduction: an `await` in an `if`'s `then` body used to be refused as if
/// nothing followed it (`if_blocks`, where Teal keeps an `if`'s branches, was invisible to
/// the walk that resolves `await`'s operand). It checks clean and runs.
#[test]
fn await_in_an_if_then_body_checks_and_runs() {
    let root = scratch("if-then-body");
    write(
        &root.join("htl.toml"),
        "[layout]\nsource = \"src\"\n\n[lang]\nasync = true\n",
    );
    write(&root.join("src/main.tl"), IF_THEN_BODY_PROGRAM);
    let (ok, _, err) = htl(&["check", "src"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("0 error(s)"), "{err}");
    let (ok, out, err) = htl(&["run", "src/main.tl"], &root);
    assert!(ok, "{err}");
    assert_eq!(out, "1\n");
}

const IF_EVERYWHERE_PROGRAM: &str = "\
local task = require(\"htl.task\")

local async function ready(): boolean
   return true
end

local async function f(): integer
   return 1
end

-- An `if` body inside an async function, and an awaited method call in an `if` body.
local async function h(): integer
   if true then
      await task.after(1):wait()
      return 2
   end
   return 0
end

-- An `if` body inside an async function, a plain `await` of a call.
local async function g(n: integer): integer
   if n == 1 then
      return await f()
   end
   return 0
end

local parts: {string} = {}
local x = 1

-- The condition.
if await ready() then
   table.insert(parts, \"cond\")
end

-- The `then` body, and (below) the `elseif` and `else` bodies of the same chain.
if x == 1 then
   local v = await f()
   table.insert(parts, \"then:\" .. tostring(v))
elseif x == 2 then
   table.insert(parts, \"elseif-unreached\")
else
   table.insert(parts, \"else-unreached\")
end

-- The `elseif` body.
if x == 2 then
   table.insert(parts, \"then-unreached\")
elseif x == 1 then
   local v = await f()
   table.insert(parts, \"elseif:\" .. tostring(v))
else
   table.insert(parts, \"else-unreached2\")
end

-- The `else` body.
if x == 2 then
   table.insert(parts, \"then-unreached2\")
else
   local v = await f()
   table.insert(parts, \"else:\" .. tostring(v))
end

local gv = await g(1)
table.insert(parts, \"nested:\" .. tostring(gv))

local hv = await h()
table.insert(parts, \"method:\" .. tostring(hv))

print(table.concat(parts, \",\"))
";

/// One case for each row of the issue's table: `await` in an `if`'s condition, `then`
/// body, `elseif` body and `else` body; an `if` body inside an async function; and an
/// awaited method call in an `if` body. Each one checks clean, the run shows the branch
/// that executed, and `gen` writes line for line.
///
/// `fmt --check` is not asserted here: `htl fmt` currently re-indents a statement-level
/// `await <call>` (such as `await task.after(1):wait()` below) one level shallower than
/// it should be, regardless of whether it is inside an `if` — a separate, pre-existing
/// defect, not something this fix changes.
#[test]
fn await_works_in_every_part_of_an_if_and_inside_an_async_function() {
    let root = scratch("if-everywhere");
    write(&root.join("htl.toml"), "[lang]\nasync = true\n");
    write(&root.join("main.tl"), IF_EVERYWHERE_PROGRAM);
    let (ok, _, err) = htl(&["check", "main.tl"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("0 error(s)"), "{err}");
    let (ok, out, err) = htl(&["run", "main.tl"], &root);
    assert!(ok, "{err}");
    assert_eq!(out, "cond,then:1,elseif:1,else:1,nested:1,method:2\n");
    let (ok, out, err) = htl(&["gen", "main.tl"], &root);
    assert!(ok, "{err}");
    assert_eq!(
        out.lines().count(),
        IF_EVERYWHERE_PROGRAM.lines().count(),
        "{out}"
    );
}

/// An `async local` awaited in an `if`'s condition: `t`'s `variable` node sits in the
/// `if_block`'s `exp` and is rewritten to `t:await()` there; before the fix nothing under
/// an `if` was indexed, so the operand was not found.
#[test]
fn an_async_local_awaited_in_an_if_condition_checks_and_runs() {
    let root = scratch("async-local-if-condition");
    write(&root.join("htl.toml"), "[lang]\nasync = true\n");
    write(
        &root.join("main.tl"),
        "local async function work(): boolean
   return true
end
async local t = work()
if await t then
   print(\"a\")
end
",
    );
    let (ok, _, err) = htl(&["check", "main.tl"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("0 error(s)"), "{err}");
    let (ok, out, err) = htl(&["run", "main.tl"], &root);
    assert!(ok, "{err}");
    assert_eq!(out, "a\n");
}

/// An `async local` declared inside an `if` body and awaited in that same body: the scope
/// walk (`mark_task_vars`) that marks a `variable` node as naming a task has the same
/// `if_blocks` blind spot the operand walk did, so it has to see into the `if` too for the
/// task to be recognised as one at all, rather than rejected as "not an async local".
#[test]
fn an_async_local_declared_and_awaited_inside_an_if_body_checks_and_runs() {
    let root = scratch("async-local-in-if-body");
    write(&root.join("htl.toml"), "[lang]\nasync = true\n");
    write(
        &root.join("main.tl"),
        "local async function work(): boolean
   return true
end
local x = 1
if x == 1 then
   async local t = work()
   print(await t)
end
",
    );
    let (ok, _, err) = htl(&["check", "main.tl"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("0 error(s)"), "{err}");
    let (ok, out, err) = htl(&["run", "main.tl"], &root);
    assert!(ok, "{err}");
    assert_eq!(out, "true\n");
}

/// An `async function` declared with `local`, entirely inside an `if` body: `by_pos` has
/// to find the `local_function` node there for the `async` keyword to attach to it at
/// all, and the call awaiting it is itself inside the same `if` body.
#[test]
fn an_async_local_function_declared_inside_an_if_body_checks_and_runs() {
    let root = scratch("async-local-function-in-if-body");
    write(&root.join("htl.toml"), "[lang]\nasync = true\n");
    write(
        &root.join("main.tl"),
        "local x = 1
if x == 1 then
   local async function work(): boolean
      return true
   end
   print(await work())
end
",
    );
    let (ok, _, err) = htl(&["check", "main.tl"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("0 error(s)"), "{err}");
    let (ok, out, err) = htl(&["run", "main.tl"], &root);
    assert!(ok, "{err}");
    assert_eq!(out, "true\n");
}

const NOT_A_CALL_MESSAGE: &str = "syntax error: 'await' needs a call after it: await f(x)";
const NOT_AN_ASYNC_LOCAL_MESSAGE: &str = "'await' on 'f', which is not an async local: an async \
function is awaited at its call (await f(x)), a task at the name an 'async local' gave it";
const APPLIES_TO_A_CALL_MESSAGE: &str = "syntax error: 'await' applies to a call: await f(x)";

/// `await` on something that is not a call reports one of three messages, depending on
/// what is there: for a unary minus, the token after `await` is not one the operand walk
/// starts from (a name, a parenthesis, a float or string literal), so nothing is found and
/// the generic message is given ("needs a call after it"); a plain local is a name, but
/// not one an `async local` gave a task to ("which is not an async local"); a float
/// literal is something, but still not a call ("applies to a call"). Each one is the same
/// inside an `if` as it is at the top level: the fix that lets the walk see `if` bodies
/// must not also make it accept an operand it would otherwise reject.
#[test]
fn await_on_a_non_call_inside_an_if_reports_the_same_error_as_outside_one() {
    let root = scratch("not-a-call");
    write(&root.join("htl.toml"), "[lang]\nasync = true\n");

    write(
        &root.join("top.tl"),
        "local v = await -1
print(v)
",
    );
    let (ok, _, err) = htl(&["check", "top.tl"], &root);
    assert!(!ok);
    assert!(err.contains(NOT_A_CALL_MESSAGE), "{err}");

    write(
        &root.join("in_if.tl"),
        "local x = 1
if x == 1 then
   local v = await -1
   print(v)
end
",
    );
    let (ok, _, err) = htl(&["check", "in_if.tl"], &root);
    assert!(!ok);
    assert!(err.contains(NOT_A_CALL_MESSAGE), "{err}");

    write(
        &root.join("top_local.tl"),
        "local f = 1
local v = await f
print(v)
",
    );
    let (ok, _, err) = htl(&["check", "top_local.tl"], &root);
    assert!(!ok);
    assert!(err.contains(NOT_AN_ASYNC_LOCAL_MESSAGE), "{err}");

    write(
        &root.join("in_if_local.tl"),
        "local f = 1
local x = 1
if x == 1 then
   local v = await f
   print(v)
end
",
    );
    let (ok, _, err) = htl(&["check", "in_if_local.tl"], &root);
    assert!(!ok);
    assert!(
        err.contains(NOT_AN_ASYNC_LOCAL_MESSAGE),
        "the message inside the `if` must be the one `await f` gets at the top level: {err}"
    );

    write(
        &root.join("top_number.tl"),
        "local v = await 1.5
print(v)
",
    );
    let (ok, _, err) = htl(&["check", "top_number.tl"], &root);
    assert!(!ok);
    assert!(err.contains(APPLIES_TO_A_CALL_MESSAGE), "{err}");

    write(
        &root.join("in_if_number.tl"),
        "local x = 1
if x == 1 then
   local v = await 1.5
   print(v)
end
",
    );
    let (ok, _, err) = htl(&["check", "in_if_number.tl"], &root);
    assert!(!ok);
    assert!(
        err.contains(APPLIES_TO_A_CALL_MESSAGE),
        "the message inside the `if` must be the one `await 1.5` gets at the top level: {err}"
    );
}
