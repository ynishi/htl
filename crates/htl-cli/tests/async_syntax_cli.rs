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
