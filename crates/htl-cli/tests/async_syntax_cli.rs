//! `async` / `await` through the command, under `[lang] async = true` in `htl.toml`: the
//! program checks clean, runs on the executor, formats to itself, and generates one line
//! of Lua per line of Teal. Without the setting the two words are names (the default).

mod common;

use std::path::Path;
use std::process::Command;

fn tempdir(name: &str) -> common::TempDir {
    common::tempdir("htl-cli-async-syntax", name)
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

fn project(name: &str) -> common::TempDir {
    let root = tempdir(name);
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
    let root = tempdir("wrong");
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
    let root = tempdir("off");
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
    let root = tempdir("rules");
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
    let root = tempdir("if-then-body");
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
/// that executed, `fmt --check` has nothing to change (including the statement-level
/// `await task.after(1):wait()` that opens `h`'s `if` body — #435), and `gen` writes line
/// for line.
#[test]
fn await_works_in_every_part_of_an_if_and_inside_an_async_function() {
    let root = tempdir("if-everywhere");
    write(&root.join("htl.toml"), "[lang]\nasync = true\n");
    write(&root.join("main.tl"), IF_EVERYWHERE_PROGRAM);
    let (ok, _, err) = htl(&["check", "main.tl"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("0 error(s)"), "{err}");
    let (ok, out, err) = htl(&["run", "main.tl"], &root);
    assert!(ok, "{err}");
    assert_eq!(out, "cond,then:1,elseif:1,else:1,nested:1,method:2\n");
    let (ok, _, err) = htl(&["fmt", "--check", "main.tl"], &root);
    assert!(ok, "the file is already in its formatted shape: {err}");
    assert!(err.contains("0 would change"), "{err}");
    let (ok, out, err) = htl(&["gen", "main.tl"], &root);
    assert!(ok, "{err}");
    assert_eq!(
        out.lines().count(),
        IF_EVERYWHERE_PROGRAM.lines().count(),
        "{out}"
    );
}

/// #435: `htl fmt` moved a statement-level `await <call>` one indent level left when it
/// was the first statement of a block — `if`, `while`, directly under an `async
/// function`'s body — because the block's own position came from the call's token, not
/// the keyword's. One case per block kind, each with a plain call and a method call, plus
/// `local v = await f()` in the same three places staying put (acceptance 1-3): a
/// statement-level await moves, an awaited expression does not.
#[test]
fn fmt_check_does_not_move_a_statement_level_await_that_opens_a_block() {
    let root = tempdir("await-opens-block");
    write(&root.join("htl.toml"), "[lang]\nasync = true\n");
    write(
        &root.join("main.tl"),
        "local task = require(\"htl.task\")

local async function plain(): integer
   return 1
end

local async function h(): integer
   while true do
      await task.after(1):wait()
      return 2
   end
   return 0
end

local async function while_plain(): integer
   while true do
      await plain()
      return 2
   end
   return 0
end

local async function if_plain(x: integer): integer
   if x == 1 then
      await plain()
      return 2
   end
   return 0
end

local async function if_method(x: integer): integer
   if x == 1 then
      await task.after(1):wait()
      return 2
   end
   return 0
end

local async function fn_plain(): integer
   await plain()
   return 2
end

local async function fn_method(): integer
   await task.after(1):wait()
   return 2
end

local async function assigned(x: integer): integer
   local v = await plain()
   if x == 1 then
      local w = await plain()
      return v + w
   end
   while true do
      local t = await plain()
      return v + t
   end
   return 0
end
",
    );
    let (ok, _, err) = htl(&["check", "main.tl"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("0 error(s)"), "{err}");
    let (ok, _, err) = htl(&["fmt", "--check", "main.tl"], &root);
    assert!(ok, "the file is already in its formatted shape: {err}");
    assert!(err.contains("0 would change"), "{err}");
}

/// #435, acceptance 4: a diagnostic on an awaited line keeps its position after the fix,
/// except the one the fix means to move. `f` takes one argument; `await f()` is called
/// with none, once as a block's first statement and once as its second, and the "wrong
/// number of arguments" error moves to the `await` keyword's column both times (`start_at`
/// touches the node itself regardless of which statement position it is, and only
/// additionally repairs the block's own position when it is the first — the comment in
/// `prelude.lua` says so). The same call as `local v = await f()` is not a statement —
/// there is no block position to repair, so its error stays at the call's `(`, as does an
/// ordinary, unawaited `p(1)`'s: neither is touched by this fix at all. The
/// `await-non-async` lint (`htl_await_at`) and the argument-type error (the argument's own
/// node) are included too, to show those never moved either way.
#[test]
fn an_argument_count_error_moves_to_the_keyword_only_for_a_statement_level_await() {
    let root = tempdir("await-argument-count-position");
    write(&root.join("htl.toml"), "[lang]\nasync = true\n");
    write(
        &root.join("main.tl"),
        "local async function f(n: integer): integer
   return n
end
local function s(): integer
   return 1
end
local function p(a: integer, b: integer): integer
   return a + b
end
local async function g(x: integer): integer
   if x == 1 then
      await f(\"x\")
      return 1
   end
   if x == 2 then
      await s()
      return 2
   end
   if x == 3 then
      await f()
      await f()
      return 3
   end
   local v = await f()
   return v
end
p(1)

return { g = g }
",
    );
    let (ok, _, err) = htl(&["check", "main.tl"], &root);
    assert!(!ok);
    // `await-non-async`: still at the keyword, via `htl_await_at` — unrelated to this fix.
    assert!(
        err.contains("main.tl:16:7:") && err.contains("[htl await-non-async]"),
        "{err}"
    );
    // The argument type error: still at the argument, never the call — unrelated to this fix.
    assert!(
        err.contains("main.tl:12:15:") && err.contains("expected integer"),
        "{err}"
    );
    // `await f()` as the `if` body's first statement: moved to the keyword's column.
    assert!(
        err.contains("main.tl:20:7: wrong number of arguments (given 0, expects 1)"),
        "{err}"
    );
    // The same body's second statement: also moved, same as the comment in prelude.lua says.
    assert!(
        err.contains("main.tl:21:7: wrong number of arguments (given 0, expects 1)"),
        "{err}"
    );
    // `local v = await f()`: an expression, not a statement — stays at the call's `(`.
    assert!(
        err.contains("main.tl:24:21: wrong number of arguments (given 0, expects 1)"),
        "{err}"
    );
    // An ordinary, unawaited call: never touched by this fix, stays at its own `(`.
    assert!(
        err.contains("main.tl:27:2: wrong number of arguments (given 1, expects 2)"),
        "{err}"
    );
}

/// An `async local` awaited in an `if`'s condition: `t`'s `variable` node sits in the
/// `if_block`'s `exp` and is rewritten to `t:await()` there; before the fix nothing under
/// an `if` was indexed, so the operand was not found.
#[test]
fn an_async_local_awaited_in_an_if_condition_checks_and_runs() {
    let root = tempdir("async-local-if-condition");
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
    let root = tempdir("async-local-in-if-body");
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
    let root = tempdir("async-local-function-in-if-body");
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

/// `await`'s answer for an operand that is not a call -- a plain local, a unary
/// expression, a missing operand -- is the same inside an `if` as it is at the top
/// level: the fix that lets the walk see `if` bodies must not also make it accept an
/// operand it would otherwise reject, or reject one it would otherwise accept.
#[test]
fn await_on_a_non_call_inside_an_if_reports_the_same_error_as_outside_one() {
    let root = tempdir("not-a-call");
    write(&root.join("htl.toml"), "[lang]\nasync = true\n");

    write(
        &root.join("neg_top.tl"),
        "local v = await -1
print(v)
",
    );
    let (ok, _, err) = htl(&["check", "neg_top.tl"], &root);
    assert!(!ok);
    assert!(err.contains(APPLIES_TO_A_CALL_MESSAGE), "{err}");

    write(
        &root.join("neg_if.tl"),
        "local x = 1
if x == 1 then
   local v = await -1
   print(v)
end
",
    );
    let (ok, _, err) = htl(&["check", "neg_if.tl"], &root);
    assert!(!ok);
    assert!(
        err.contains(APPLIES_TO_A_CALL_MESSAGE),
        "the message inside the `if` must be the one `await -1` gets at the top level: {err}"
    );

    write(&root.join("ret.tl"), "return await\n");
    let (ok, _, err) = htl(&["check", "ret.tl"], &root);
    assert!(!ok);
    assert!(err.contains(NOT_A_CALL_MESSAGE), "{err}");

    write(
        &root.join("ifret.tl"),
        "local x = 1
if x == 1 then
   return await
end
",
    );
    let (ok, _, err) = htl(&["check", "ifret.tl"], &root);
    assert!(!ok);
    assert!(
        err.contains(NOT_A_CALL_MESSAGE),
        "the message inside the `if` must be the one `return await` gets at the top level: {err}"
    );

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

/// `await`'s operand walk (`#436`) starts at a name, a parenthesis, a literal (an
/// integer, a float, a string, `true` / `false`, `nil`, a table constructor, `...`) or a
/// unary operator. A literal or a unary operand is refused as "applies to a call", the
/// same as any other operand that is not a call or an async local; a missing operand
/// (nothing an expression starts with follows `await`) is "needs a call after it". One
/// file per row, each asserting the exact message at the literal's position; a last file
/// keeps the forms this walk must accept -- a call chained to a binary operator, a method
/// call, and an `async local` awaited by its name -- checking clean. (A parenthesized
/// call, `await (f)(x)`, is not in that file: `split_async_tokens` treats any `await`
/// directly followed by `(` as the name of a plain function -- `AWAIT_NAME_BEFORE["("]`
/// -- so it is never recognized as the keyword there at all.)
#[test]
fn await_on_a_literal_or_a_unary_operator_applies_to_a_call() {
    let root = tempdir("literal-operand");
    write(&root.join("htl.toml"), "[lang]\nasync = true\n");

    for (file, body) in [
        (
            "integer.tl",
            "local async function f(): integer
   local v = await 1
   return v
end
",
        ),
        (
            "boolean.tl",
            "local async function f(): integer
   local v = await true
   return 1
end
",
        ),
        (
            "nil.tl",
            "local async function f(): integer
   local v = await nil
   return 1
end
",
        ),
        (
            "table.tl",
            "local async function f(): integer
   local v = await {}
   return 1
end
",
        ),
        (
            "float.tl",
            "local async function f(): integer
   local v = await 1.5
   return 1
end
",
        ),
        (
            "string.tl",
            "local async function f(): integer
   local v = await \"s\"
   return 1
end
",
        ),
        (
            "vararg.tl",
            "local async function f(...: integer): integer
   local v = await ...
   return 1
end
",
        ),
        (
            "unary_minus.tl",
            "local async function f(): integer
   local v = await -1
   return v
end
",
        ),
        (
            "unary_not.tl",
            "local async function f(x: boolean): integer
   local v = await not x
   return 1
end
",
        ),
    ] {
        write(&root.join(file), body);
        let (ok, _, err) = htl(&["check", file], &root);
        assert!(!ok, "{file}: {err}");
        assert!(
            err.contains(&format!(
                "{file}:2:14: syntax error: 'await' applies to a call: await f(x)"
            )),
            "{file}: {err}"
        );
    }

    write(
        &root.join("clean.tl"),
        "local task = require(\"htl.task\")

local async function g(): integer
   return 1
end

local async function f(): integer
   local a = await g() + 1
   await task.after(1):wait()
   async local u = g()
   local b = await u
   return a + b
end
print(await f())
",
    );
    let (ok, _, err) = htl(&["check", "clean.tl"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("0 error(s)"), "{err}");
}

const SAME_LINE_DECLARATIONS_PROGRAM: &str = "\
local async function same_line(cond: function(): boolean): boolean
   return cond()
end

local async function next_line(
   cond: function(): boolean
): boolean
   return cond()
end

local async function local_on_line(): boolean
   local cb = function(): boolean return true end
   return cb()
end

local async function on_async_line(): boolean local cb = function(): boolean return true end
   return cb()
end

return { same_line = same_line, next_line = next_line, local_on_line = local_on_line, on_async_line = on_async_line }
";

/// A sync parameter's type (`same_line`, `next_line`) or a sync local's inferred type
/// (`local_on_line`, `on_async_line`) that shares a source line with the enclosing
/// `async function` keyword must not be read as async itself: none of the four calls
/// here is of an async function, so `await-missing` stays silent for all of them. (#430)
#[test]
fn await_missing_is_silent_when_the_callees_type_shares_the_declaration_line() {
    let root = tempdir("same-line-decl");
    write(&root.join("htl.toml"), "[lang]\nasync = true\n");
    write(&root.join("src/main.tl"), SAME_LINE_DECLARATIONS_PROGRAM);
    let (ok, _, err) = htl(&["check", "src"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("0 error(s), 0 warning(s), 0 lint(s)"), "{err}");
}

/// A plain `local async function f` called without `await` is reported whether the
/// call sits on the same source line as `f`'s own declaration (`f_same`, called right
/// after its own `end` on one line) or several lines below it (`f_diff`, called from
/// `caller`). (#430)
#[test]
fn await_missing_still_fires_whether_the_call_shares_the_declaration_line_or_not() {
    let root = tempdir("same-line-and-diff-line-call");
    write(&root.join("htl.toml"), "[lang]\nasync = true\n");
    write(
        &root.join("main.tl"),
        "local async function f_same(): integer return 1 end print(f_same())

local async function f_diff(): integer
   return 1
end

local async function caller(): integer
   return f_diff()
end
print(await caller())
",
    );
    let (ok, _, err) = htl(&["check", "main.tl"], &root);
    assert!(
        !ok,
        "both calls are of async functions without await: {err}"
    );
    for want in [
        "main.tl:1:59: call of an async function without await: f_same may suspend",
        "main.tl:8:11: call of an async function without await: f_diff may suspend",
    ] {
        assert!(err.contains(want), "missing {want:?} in {err}");
    }
}

/// An `async function(...)` value assigned to an unannotated local (`g`), sharing a
/// source line with another, unrelated `async function` declaration (`marker`), is
/// reported when called without `await`: `g`'s own column sits at its own `async`
/// keyword, not at `marker`'s. (The silent direction -- a sync declaration sharing a
/// line with someone else's `async function` -- is `on_async_line`, above.) (#430)
#[test]
fn await_missing_still_fires_for_an_async_value_sharing_a_line_with_another_async_function() {
    let root = tempdir("async-value-shares-line");
    write(&root.join("htl.toml"), "[lang]\nasync = true\n");
    write(
        &root.join("main.tl"),
        "local async function marker(): boolean return true end local g = async function(): boolean return true end

local async function caller(): boolean
   return g()
end
print(await caller())
",
    );
    let (ok, _, err) = htl(&["check", "main.tl"], &root);
    assert!(!ok, "{err}");
    assert!(
        err.contains("main.tl:4:11: call of an async function without await: g may suspend"),
        "{err}"
    );
}

/// `async_at_decl`'s other branch: `t.x` at the type's own `function` keyword with
/// `async` immediately before it, rather than at `local` or `async` itself. `gf`'s node
/// is a `global_function`, which Teal starts at `function` (unlike `local_function`,
/// which starts at `local`), so this is the only spelling that takes this branch on its
/// own. The second part shares one line between a sync declaration (`a`) and an async
/// one (`b`): `a`'s own `function` keyword has no `async` immediately before it, `b`'s
/// does, so the lint must name `b` and not `a`. (#430)
#[test]
fn await_missing_fires_for_global_async_function_and_only_names_the_async_one_sharing_a_line() {
    let root = tempdir("global-and-shared-line");
    write(&root.join("htl.toml"), "[lang]\nasync = true\n");
    write(
        &root.join("main.tl"),
        "global async function gf(): integer
   return 1
end
print(gf())

local function a(): integer return 1 end local async function b(): integer return 2 end
print(a(), b())
",
    );
    let (ok, _, err) = htl(&["check", "main.tl"], &root);
    assert!(!ok, "{err}");
    assert!(
        err.contains("main.tl:4:7: call of an async function without await: gf may suspend"),
        "{err}"
    );
    assert!(
        err.contains("main.tl:7:12: call of an async function without await: b may suspend"),
        "{err}"
    );
    assert!(
        !err.contains("without await: a may suspend"),
        "the sync declaration sharing the line with `b` must not be named: {err}"
    );
}

const DTL_ASYNC_METHOD_HTTP: &str = "\
local record http
   get: function(path: string): string ---@async
end
return http
";

/// A `.d.tl` method marked `---@async` is reported when called without `await`;
/// `marker_on` (the `---@async` reader) is untouched by this fix. (#430)
#[test]
fn await_missing_still_fires_for_a_dtl_method_marked_async() {
    let root = tempdir("dtl-marker-still-fires");
    write(&root.join("htl.toml"), "[lang]\nasync = true\n");
    write(&root.join("types/http.d.tl"), DTL_ASYNC_METHOD_HTTP);
    write(
        &root.join("src/main.tl"),
        "local http = require(\"http\")

local async function f(): string
   return http.get(\"/a\")
end
print(await f())
",
    );
    let (ok, _, err) = htl(&["check", "src"], &root);
    assert!(!ok, "{err}");
    assert!(
        err.contains("call of an async function without await: http.get may suspend"),
        "{err}"
    );
}

/// #457: a statement-level `await` / `async local` whose keyword sits alone on its own
/// line, with the call or the declaration on the next, under `htl fmt`.
///
/// `start_at` (prelude.lua) gives the rewritten node the keyword's column but leaves its
/// `y` at the parser's own — the line of the first token *after* the keyword, i.e. the
/// call's line when the keyword is alone. Two different things in `fmt.lua` went wrong
/// from that one fact, and this test is one case of each:
///
/// - `first` (the keyword as a block's first statement, #457's own repro): the block's
///   span looked itself up at `(call's y, keyword's x)`, a position nothing in the token
///   stream sits at, so the lookup fell back to that same pair instead of finding the
///   block's real opener (`then`) — and a block whose span starts on the *call's* line
///   does not cover the keyword's line at all, which is why `await` moved out of the
///   `if` a whole level to its left and `g(1)` came with it (`htl fmt` rewrote `      await`
///   / `         g(1)` to `   await` / `      g(1)`).
/// - `second` (the keyword as a block's *second* statement): the block's span is the real
///   one here (its first statement, `local a = 1`, is never touched), so `await` itself
///   never moved — only `g(a)` did, losing the one extra level a line that continues a
///   statement whose own line held nothing but the keyword is owed, the same as a line
///   ending in `+` owes the next one a level, except no operator marks this continuation
///   and nothing in `fmt.lua` was watching for it at all.
/// - `asyncnl` (`async local`, not `await`, as a block's first statement): the same
///   mechanism as `first` — `local_declaration` is `start_at`'s other caller — and the
///   same symptom (`async` / `local t = g(x)` moved a level left together).
///
/// A fourth file, `messy`, is the first shape written at the *wrong* depth (`await` one
/// level too shallow): `htl fmt` recomputes depth from the AST rather than trusting
/// existing indentation, so it is reindented to the one correct depth, asserted exactly
/// rather than merely "0 would change" — the three shapes above already cover the
/// idempotent case, and this is the one place the fix has to move text rather than leave
/// it alone.
///
/// A fifth, `callback`, is a keyword immediately followed by a call whose own argument
/// is a multi-line function literal — `await f(function(): integer ... end)`, both as a
/// statement and inside `local v = ...`, and `async local t = f(function(): integer
/// ... end)` — with the keyword sharing its line with the call rather than split from
/// it. A keyword sharing its line with the call is left alone, however many lines the
/// call itself runs on: the call's own spans (the paren span for its argument list, the
/// block span for the callback's body) already count every one of them.
///
/// A sixth, `exprpos`, moves the split keyword out of statement position: `local v =
/// await` / newline / `g(x)`, the expression of a declaration, and `return await` /
/// newline / `g(x)`. The continuation rule is not specific to a statement-level keyword
/// — it fires for a marked node wherever it is — so the call's line gets the same extra
/// level there as it does as a bare statement.
///
/// A seventh pair, `splitcb`, is the split form of `callback`: `await` / newline /
/// `f(function(): integer ... end)` and `async` / newline / `local u = f(function():
/// integer ... end)`, each as an `if` body's first statement. The call's own line and
/// the callback's own body line stack the keyword-continuation level on top of the
/// block level the callback's body already gets, and the callback's closing `end)`
/// drops back to the call's own (continuation-only) level, the same as any block's
/// terminator does.
///
/// `messy2` and `messy3` repeat the `messy` idea (wrong depth corrected, asserted
/// exactly, then a second `--check` confirming the result is now stable) for the other
/// two shapes: `await` / `g(a)` written flat as a block's *second* statement, and
/// `async` / `local t = g(1)` written flat as a block's *first*.
#[test]
fn fmt_reindents_or_leaves_alone_a_split_await_or_async_local_keyword_line() {
    let root = tempdir("await-keyword-line");
    write(&root.join("htl.toml"), "[lang]\nasync = true\n");

    write(
        &root.join("src/first.tl"),
        "local async function g(n: integer): integer
   return n
end
local async function h(x: integer): integer
   if x == 1 then
      await
         g(1)
      return 2
   end
   return 0
end

return { h = h }
",
    );
    write(
        &root.join("src/second.tl"),
        "local async function g(n: integer): integer
   return n
end
local async function h(x: integer): integer
   if x == 1 then
      local a = 1
      await
         g(a)
      return 2
   end
   return 0
end

return { h = h }
",
    );
    write(
        &root.join("src/asyncnl.tl"),
        "local async function g(n: integer): integer
   return n
end
local async function h(x: integer): integer
   if x == 1 then
      async
         local t = g(x)
      return await t
   end
   return 0
end

return { h = h }
",
    );

    write(
        &root.join("src/callback.tl"),
        "local async function f(cb: function(): integer): integer
   return cb()
end
local async function g(x: integer): integer
   await f(function(): integer
      return x
   end)
   local v = await f(function(): integer
      return x
   end)
   async local t = f(function(): integer
      return x
   end)
   return await t
end

return { g = g }
",
    );

    write(
        &root.join("src/exprpos.tl"),
        "local async function g(n: integer): integer
   return n
end
local async function via_decl(x: integer): integer
   local v = await
      g(x)
   return v
end
local async function via_return(x: integer): integer
   return await
      g(x)
end

return { via_decl = via_decl, via_return = via_return }
",
    );

    write(
        &root.join("src/splitcb.tl"),
        "local async function f(cb: function(): integer): integer
   return cb()
end
local async function h4(x: integer): integer
   if x == 1 then
      await
         f(function(): integer
            return 1
         end)
      return 2
   end
   return 0
end
local async function h5(x: integer): integer
   if x == 1 then
      async
         local u = f(function(): integer
            return 4
         end)
      return await u
   end
   return 0
end

return { h4 = h4, h5 = h5 }
",
    );

    let (ok, _, err) = htl(&["check", "src"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("0 error(s)"), "{err}");

    for f in [
        "src/first.tl",
        "src/second.tl",
        "src/asyncnl.tl",
        "src/callback.tl",
        "src/exprpos.tl",
        "src/splitcb.tl",
    ] {
        let (ok, _, err) = htl(&["fmt", "--check", f], &root);
        assert!(ok, "{f} should already be in its formatted shape: {err}");
        assert!(err.contains("0 would change"), "{f}: {err}");
    }

    let messy = root.join("src/messy.tl");
    write(
        &messy,
        "local async function g(n: integer): integer
   return n
end
local async function h(x: integer): integer
   if x == 1 then
   await
      g(1)
      return 2
   end
   return 0
end

return { h = h }
",
    );
    let (ok, _, err) = htl(&["fmt", "--check", "src/messy.tl"], &root);
    assert!(!ok, "a misindented keyword line should be reported: {err}");
    assert!(err.contains("1 would change"), "{err}");
    let (ok, _, err) = htl(&["fmt", "src/messy.tl"], &root);
    assert!(ok, "{err}");
    assert_eq!(
        std::fs::read_to_string(&messy).unwrap(),
        "local async function g(n: integer): integer
   return n
end
local async function h(x: integer): integer
   if x == 1 then
      await
         g(1)
      return 2
   end
   return 0
end

return { h = h }
"
    );
    let (ok, _, err) = htl(&["fmt", "--check", "src/messy.tl"], &root);
    assert!(ok, "the reindented file should now be stable: {err}");
    assert!(err.contains("0 would change"), "{err}");

    // `messy2`: shape 2 (`await` as a block's *second* statement) written at the wrong,
    // flat depth — `await` and `g(a)` both one level too shallow.
    let messy2 = root.join("src/messy2.tl");
    write(
        &messy2,
        "local async function g(n: integer): integer
   return n
end
local async function h(x: integer): integer
   if x == 1 then
      local a = 1
   await
   g(a)
      return 2
   end
   return 0
end

return { h = h }
",
    );
    let (ok, _, err) = htl(&["fmt", "--check", "src/messy2.tl"], &root);
    assert!(!ok, "a misindented keyword line should be reported: {err}");
    assert!(err.contains("1 would change"), "{err}");
    let (ok, _, err) = htl(&["fmt", "src/messy2.tl"], &root);
    assert!(ok, "{err}");
    assert_eq!(
        std::fs::read_to_string(&messy2).unwrap(),
        "local async function g(n: integer): integer
   return n
end
local async function h(x: integer): integer
   if x == 1 then
      local a = 1
      await
         g(a)
      return 2
   end
   return 0
end

return { h = h }
"
    );
    let (ok, _, err) = htl(&["fmt", "--check", "src/messy2.tl"], &root);
    assert!(ok, "the reindented file should now be stable: {err}");
    assert!(err.contains("0 would change"), "{err}");

    // `messy3`: shape 3 (`async local` as a block's *first* statement) written at the
    // wrong, flat depth — `async` and `local t = g(1)` both one level too shallow.
    let messy3 = root.join("src/messy3.tl");
    write(
        &messy3,
        "local async function g(n: integer): integer
   return n
end
local async function h(x: integer): integer
   if x == 1 then
   async
   local t = g(1)
      return await t
   end
   return 0
end

return { h = h }
",
    );
    let (ok, _, err) = htl(&["fmt", "--check", "src/messy3.tl"], &root);
    assert!(!ok, "a misindented keyword line should be reported: {err}");
    assert!(err.contains("1 would change"), "{err}");
    let (ok, _, err) = htl(&["fmt", "src/messy3.tl"], &root);
    assert!(ok, "{err}");
    assert_eq!(
        std::fs::read_to_string(&messy3).unwrap(),
        "local async function g(n: integer): integer
   return n
end
local async function h(x: integer): integer
   if x == 1 then
      async
         local t = g(1)
      return await t
   end
   return 0
end

return { h = h }
"
    );
    let (ok, _, err) = htl(&["fmt", "--check", "src/messy3.tl"], &root);
    assert!(ok, "the reindented file should now be stable: {err}");
    assert!(err.contains("0 would change"), "{err}");
}
