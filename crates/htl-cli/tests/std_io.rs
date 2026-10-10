//! `htl-std`'s `std.fs` / `std.proc` through the CLI, with no Rust host of the project's
//! own: registered once at this binary's entry (`register_libraries` in `src/lib.rs`), so
//! `htl check` / `run` / `test` / `resolve` / `build` of a pure-Teal project see them the
//! same way they see `std.json` and the rest of `std.*`.
//!
//! `sh`, `sleep`, `true` and `pgrep` are unix, so the whole file is `#![cfg(unix)]`, the
//! way `crates/htl-std/tests/proc.rs` is.

#![cfg(unix)]

mod common;

use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

fn tempdir(name: &str) -> common::TempDir {
    common::tempdir("htl-cli-std-io", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn htl(args: &[&str], cwd: &Path) -> (bool, String, String) {
    let out: Output = Command::new(common::htl_bin())
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

/// A pure-Teal project: `[lang] async = true` and nothing else — no `Cargo.toml`, no
/// `#[host_module]` of its own. `std.fs` and `std.proc` reach it only through what this
/// binary registered.
fn project(name: &str) -> common::TempDir {
    let root = tempdir(name);
    write(&root.join("htl.toml"), "[lang]\nasync = true\n");
    root
}

/// The issue #474 Acceptance 1 program: a task started with `std.proc.run` (a shell that
/// sleeps 300 ms before printing) and a 100 ms `task.after` timer, both put in the *same*
/// `select` (`timer:on`, `t:on` — `task.d.tl`'s `Task:on`), so the two genuinely race. A `proc.run` that blocked the caller for 300 ms
/// instead of returning a task at once would mean `t` is already finished by the time
/// `select` runs — its case ready immediately — and the timer, only just started, would
/// lose the race it cannot even enter yet; printing `first` would then show the task's
/// label before the timer's. With a true concurrent spawn the timer (100 ms) is what
/// `select` takes, `first` is `"timer"`, and only afterwards is `t` awaited for its own
/// result. `exit.stdout` is the child's actual output, not the case's return value, so
/// the whole of stdout is `timer\nlate\n` only if the race went the concurrent way.
const SELECT_PROGRAM: &str = "\
local proc = require(\"std.proc\")
local task = require(\"htl.task\")

async local t = proc.run({\"sh\", \"-c\", \"sleep 0.3; echo late\"})
local timer = task.after(100)
local first = await task.select({
   timer:on(function(): string
      return \"timer\"
   end),
   t:on(function(_ok: boolean, _exit: proc.Exit): string
      return \"late\"
   end),
})
print(first)
local exit = await t
io.write(exit.stdout)
";

/// `local fs = require("std.fs")` and `local proc = require("std.proc")`, each called
/// with `await`, check clean: no error, no warning, no lint — the declarations
/// `htl_std::install`'s `install_declarations` (through `register_installer` and
/// `Htl::install_registered`) put in front of the checker type both calls, and
/// `await-missing` has nothing to say about either.
#[test]
fn check_accepts_awaited_fs_and_proc_calls() {
    let root = project("accepts");
    write(
        &root.join("src/main.tl"),
        "local fs = require(\"std.fs\")\n\
         local proc = require(\"std.proc\")\n\
         \n\
         local async function main(): string, integer\n   \
            local text = await fs.read(\"input.txt\")\n   \
            local exit = await proc.run({\"true\"})\n   \
            return text, exit.code\n\
         end\n\
         \n\
         local text, code = await main()\n\
         print(text, code)\n",
    );
    let (ok, _, err) = htl(&["check", "src", "--no-cache"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("0 error(s), 0 warning(s), 0 lint(s)"), "{err}");
}

/// The same file with `await` dropped from `fs.read`: `await-missing`, at the exact line
/// and column of `fs.read`'s leftmost token (`lint.lua`'s `callee_name` /
/// `leftmost` — the same position a plain host module's call is reported at,
/// `tests/async_syntax_cli.rs`'s own `http.get` case).
#[test]
fn check_reports_await_missing_at_the_exact_position_when_await_is_dropped_from_fs_read() {
    let root = project("missing-await");
    write(
        &root.join("src/main.tl"),
        "local fs = require(\"std.fs\")\n\
         local proc = require(\"std.proc\")\n\
         \n\
         local async function main(): string, integer\n   \
            local text = fs.read(\"input.txt\")\n   \
            local exit = await proc.run({\"true\"})\n   \
            return text, exit.code\n\
         end\n\
         \n\
         local text, code = await main()\n\
         print(text, code)\n",
    );
    let (ok, _, err) = htl(&["check", "src", "--no-cache"], &root);
    assert!(!ok, "{err}");
    assert!(
        err.contains(
            "src/main.tl:5:17: call of an async function without await: fs.read may suspend; \
             write await fs.read(..) so the suspension is visible where it happens \
             [htl await-missing]"
        ),
        "{err}"
    );
}

/// Issue #474 Acceptance 1, run for real: `SELECT_PROGRAM`'s stdout is `timer\nlate\n`
/// only when the timer and the task genuinely race (see that constant's own doc).
#[test]
fn run_select_takes_the_timer_first_then_awaits_the_tasks_stdout() {
    let root = project("select");
    write(&root.join("src/main.tl"), SELECT_PROGRAM);
    let (ok, out, err) = htl(&["run", "src/main.tl"], &root);
    assert!(ok, "{err}");
    assert_eq!(out, "timer\nlate\n", "{err}");
}

/// Issue #474 Acceptance 4: the same race, but the process this time outlives the
/// 100 ms timer by a wide margin (`sleep 30.0003`, an argument unique enough that
/// nothing else on this machine is running it) and the root returns the moment the
/// timer's case is taken, never joining `t`. The task's scope ends with it unawaited,
/// so it is cancelled and waited for there (`std.proc`'s own doc, "Cancellation"): on
/// unix that kills the whole process group with `killpg`. `htl run` exits 0 within the
/// `[async] grace_ms` default (1000 ms) plus whatever this binary takes to start, a
/// bound this asserts generously as 2 s, and the child does not outlive it.
#[test]
fn a_cancelled_run_through_select_exits_quickly_and_the_child_does_not_outlive_it() {
    let root = project("cancel-select");
    write(
        &root.join("src/main.tl"),
        "local proc = require(\"std.proc\")\n\
         local task = require(\"htl.task\")\n\
         \n\
         async local t = proc.run({\"sleep\", \"30.0003\"})\n\
         local timer = task.after(100)\n\
         local first = await task.select({\n   \
            timer:on(function(): string\n      \
               return \"timer\"\n   \
            end),\n   \
            t:on(function(_ok: boolean, _exit: proc.Exit): string\n      \
               return \"late\"\n   \
            end),\n\
         })\n\
         print(first)\n",
    );
    let t0 = Instant::now();
    let (ok, out, err) = htl(&["run", "src/main.tl"], &root);
    let took = t0.elapsed();
    assert!(ok, "{err}");
    assert_eq!(out, "timer\n", "{err}");
    assert!(took < Duration::from_secs(2), "took {took:?}: {err}");
    assert_gone_within("^sleep 30\\.0003$", Duration::from_secs(2));
}

/// A plain (no `select`) cancellation: a `std.proc.run` with no deadline, started as an
/// `async local` and never awaited anywhere. When the root returns (a 100 ms timer is
/// all it waits on), the task's scope ends without a join and it is cancelled the same
/// way the `select` variant above is.
#[test]
fn a_plain_cancelled_run_exits_quickly_and_the_child_does_not_outlive_it() {
    let root = project("cancel");
    write(
        &root.join("src/main.tl"),
        "local proc = require(\"std.proc\")\n\
         local task = require(\"htl.task\")\n\
         \n\
         async local sleeper = proc.run({\"sleep\", \"30.0004\"})\n\
         await task.after(100):wait()\n",
    );
    let t0 = Instant::now();
    let (ok, _, err) = htl(&["run", "src/main.tl"], &root);
    let took = t0.elapsed();
    assert!(ok, "{err}");
    assert!(took < Duration::from_secs(2), "took {took:?}: {err}");
    assert_gone_within("^sleep 30\\.0004$", Duration::from_secs(2));
}

/// `htl test .` of a `*_test.tl` that writes a file with `std.fs` and reads it back
/// inside an `async function` body (`t.it`'s `body` parameter is a plain `function()`;
/// an `async function` literal checks against it, and only there does `await` suspend
/// rather than tripping `await-outside-async`).
#[test]
fn test_round_trips_a_file_through_std_fs() {
    let root = project("test");
    write(
        &root.join("src/fs_test.tl"),
        "local t = require(\"htl.test\")\n\
         local fs = require(\"std.fs\")\n\
         \n\
         t.describe(\"std.fs\", function()\n   \
            t.it(\"round trips a write and a read\", async function()\n      \
               local path = \"roundtrip.txt\"\n      \
               await fs.write(path, \"hello\")\n      \
               local back = await fs.read(path)\n      \
               t.expect(back):to_equal(\"hello\")\n   \
            end)\n\
         end)\n",
    );
    let (ok, _, err) = htl(&["test", "."], &root);
    assert!(ok, "{err}");
    assert!(err.contains("1 passed, 0 failed"), "{err}");
}

/// `htl resolve std.fs` / `std.proc`: both answer "provided by the host", and the
/// `typed by` path is the registered installer's own temp directory
/// (`install_declarations`'s `htl-lib-<version>-htl-std-<key>` naming,
/// `crates/htl-core/src/registry.rs`), not a project-side declaration — this project
/// writes none.
#[test]
fn resolve_says_the_host_provides_std_fs_and_std_proc() {
    let root = project("resolve");
    write(&root.join("src/main.tl"), "return 0\n");
    for module in ["std.fs", "std.proc"] {
        let (ok, out, err) = htl(&["resolve", module], &root);
        assert!(ok, "{err}");
        assert!(
            out.contains(&format!("htl resolve {module}: provided by the host")),
            "{out}"
        );
        assert!(
            out.contains("-htl-std-"),
            "typed by the registered installer's own directory: {out}"
        );
    }
}

/// `htl build` of `SELECT_PROGRAM`: the linker's `--host` listing — read from the
/// project model's `provided()` (`crates/htl-cli/src/lib.rs`'s `cmd_build` calls
/// `m.provided()`; the registered names reach it through
/// `crates/htl-core/src/model.rs`'s `providers`, which folds in
/// `registry::registered_provides()`) — names `std.proc` among what the host provides,
/// with no change needed in `cmd_build` itself.
#[test]
fn build_lists_std_proc_among_the_host_provided_modules() {
    let root = project("build");
    write(&root.join("src/main.tl"), SELECT_PROGRAM);
    let (ok, _, err) = htl(&["build", "src/main.tl", "-o", "out.hb"], &root);
    assert!(ok, "{err}");
    assert!(
        err.contains("host must provide: htl.task, std.proc"),
        "{err}"
    );
}

/// Polls `pgrep -f "<pattern>"` for up to `within`, succeeding the moment no process
/// matches. `pgrep` exits 1 for "no match" (success for this assertion) and 0 for "at
/// least one match" (keep polling, or fail once `within` has elapsed); any other exit
/// (2: usage error, a pattern `pgrep` itself rejects) is a hard failure immediately, not
/// a "still running" — `.output()` is `.expect()`ed so a `pgrep` that cannot even be
/// spawned panics too, rather than this reading that the same as "the process is gone".
fn assert_gone_within(pattern: &str, within: Duration) {
    let deadline = Instant::now() + within;
    loop {
        let out = Command::new("pgrep")
            .args(["-f", pattern])
            .output()
            .expect("pgrep");
        match out.status.code() {
            Some(1) => return,
            Some(0) => {
                if Instant::now() >= deadline {
                    panic!("a process matching {pattern:?} is still running after {within:?}");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            other => panic!(
                "pgrep -f {pattern:?} exited {other:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            ),
        }
    }
}
