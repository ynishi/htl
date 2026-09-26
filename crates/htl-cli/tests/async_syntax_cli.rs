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
