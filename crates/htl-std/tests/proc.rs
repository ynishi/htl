//! `std.proc`, end to end: the generated declaration, and Teal programs run through
//! [`htl::Htl::gen_lua`] + [`htl::Htl::run_blocking`], the same shape `fs.rs`'s own
//! tests use (see that file's module doc for why). Every test here needs `sh`, `cat`,
//! `sleep`, `pwd` or `touch`, so the whole file is `#[cfg(unix)]`.
//!
//! # State space
//!
//! `Proc::run`'s own control flow (`htl_std::proc`) is two phases — `wait()` on the
//! direct child, then a drain of its two pipe readers — each with its own deadline
//! and its own way of being cancelled, and a descendant can outlive the direct child
//! and keep either pipe open on its own. The five axes below are what actually varies
//! the path taken; every reachable combination of them has a test, named in the table,
//! and every cell that is not reachable (or not separately meaningful to test) says why
//! rather than being silently missing.
//!
//! - **direct child**: `exits` (on its own, before any deadline or cancel reaches it)
//!   or `running` (still alive when a deadline fires or a cancel lands)
//! - **stdout after the direct child is done with it**: `closed` or `held` (by a
//!   descendant)
//! - **stderr after the direct child is done with it**: `closed` or `held`
//! - **deadline**: `none`, `set` (present but never reached), `wait` (expires inside
//!   phase one), or `drain` (expires inside phase two, the direct child having already
//!   exited)
//! - **cancel**: `no`, `wait` (the future dropped while still inside phase one), or
//!   `drain` (dropped while inside phase two)
//!
//! | child | stdout | stderr | deadline | cancel | test |
//! |---|---|---|---|---|---|
//! | exits | closed | closed | none | no | `echo_round_trips_stdout_and_exits_zero`, and its siblings `stderr_is_captured_and_the_exit_code_is_the_childs_own`, `stdin_is_piped_through_to_the_child`, `cwd_is_honoured`, `env_is_added_to_the_inherited_environment` — the same cell, each adding one more of `Options`' own fields on top |
//! | exits | closed | closed | set | no | `a_deadline_that_is_never_hit_still_returns_normally` — the `Some` branch of the deadline match, in both phases, taken all the way to its success path rather than only ever its timeout path |
//! | exits | held | held | none | no | `no_deadline_returns_once_a_backgrounded_descendant_closes_both_pipes` |
//! | exits | held | held | drain | no | `timeout_kills_a_backgrounded_descendant_the_direct_child_already_left_behind` |
//! | exits | held | closed | drain | no | `timeout_kills_a_background_descendant_holding_only_stdout` — the exact shape that panicked ("JoinHandle polled after completion") before `join_one` / `drain_one` existed: the stderr reader resolves inside the bounded join while stdout's does not, and the old code polled stderr's already-completed handle a second time in the post-kill drain |
//! | exits | closed | held | drain | no | `timeout_kills_a_background_descendant_holding_only_stderr` — the same bug, the other order |
//! | running | held (by itself) | held (by itself) | wait | no | `timeout_kills_a_long_running_child` — no shell, the direct child is its own only holder |
//! | running | held (by itself + a child) | held | wait | no | `timeout_kills_a_grandchild_under_a_non_exec_shell` — the direct child (`sh`) is itself still blocked in its own `wait`, not yet exited, when the deadline fires |
//! | running | held | held | none | wait | `a_cancelled_task_kills_the_child_and_its_grandchild_before_they_finish` — the future is dropped while still inside phase one's `child.wait()` |
//! | exits | held | held | none | drain | `a_cancelled_task_during_drain_still_kills_the_backgrounded_descendant` — the direct child has already exited (backgrounded its only descendant and had nothing left to do), so the future is dropped from inside phase two's drain instead |
//!
//! Not reachable, or not separately tested:
//!
//! - **deadline `wait` with child `exits`**: contradictory by construction — `wait()`
//!   returning `Ok` before the timer fires *is* "the child exited before the
//!   deadline"; these are the same outcome of one race, not independent axes
//! - **deadline `drain` with child `running`**: equally impossible — phase two is only
//!   ever reached once phase one's `wait()` has already returned `Ok`, i.e. once the
//!   direct child has already exited
//! - **cancel (`wait` or `drain`) together with deadline `wait` or `drain`**: a run
//!   either reaches its own `timed_out = true` by continuing to run, or has its future
//!   dropped before it gets the chance to — a cancelled call never produces an `Exit`
//!   at all, so `timed_out` is never observed on one
//! - **cancel `wait` / `drain` with a `timeout_ms` set** (rather than `none`, as the two
//!   cancel tests above use): reachable — cancelling while the deadline-wrapped
//!   `wait()` / drain is in flight rather than the bare one — but not separately
//!   tested: `KillProcessGroupOnDrop` (`htl_std::proc`) is armed the same way either
//!   way, so there is nothing a dedicated test would add over the two above
//! - **child `running` with stdout/stderr `closed`**: reachable only for a child that
//!   closes its own stdout and stderr itself while continuing to run — none of this
//!   file's shell commands construct that shape, and the kill path does not treat it
//!   any differently from one that still holds them
//! - `spawn_of_a_missing_program_raises_with_its_name_and_the_os_message`,
//!   `an_empty_argv_raises`: fail before a child exists at all (`spawn` itself errors,
//!   or `argv` is rejected before a `Command` is even built) — outside this matrix,
//!   which describes states reached only once a child has actually started

#![cfg(unix)]

use htl::Htl;
use htl::config::LangConfig;
use htl::mlua_isle::runtime::CancelToken;
use htl::teal::HostModule;
use htl_std::Proc;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

mod common;

fn tempdir(name: &str) -> common::TempDir {
    common::tempdir("htl-std-proc", name)
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, text).unwrap();
    p
}

/// `std.proc` (and `std.fs` beside it — [`htl_std::install`] wires both) installed, the
/// task library on (`std.proc`'s `run` is `---@async`, like every `std.fs` function), and
/// the checker pointed at this crate's own `dts/` — the same wiring `fs.rs`'s `host`
/// does, for the same reason: there is no project model in play here to do it instead.
fn host() -> Htl {
    let h = Htl::new().unwrap();
    h.set_lang(&LangConfig { async_: Some(true) }).unwrap();
    h.install_task_lib().unwrap();
    htl_std::install(&h).unwrap();
    h.add_path(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/dts")))
        .unwrap();
    h
}

/// Checks and runs `tl_src` as `main.tl` under `dir`. Panics with the checker's errors,
/// or the runtime error, whichever fires first; a passing test is one whose Teal ran to
/// completion without either.
fn run(dir: &Path, tl_src: &str) {
    run_timed(dir, tl_src);
}

/// [`run`], returning how long [`htl::Htl::run_blocking`] itself took. `gen_lua` (the
/// checker) runs first, outside the timed region: a wall-clock assertion about a
/// `timeout_ms` or a cancellation is an assertion about `run_blocking`, not about how
/// long typechecking a few lines happened to take on this machine.
fn run_timed(dir: &Path, tl_src: &str) -> Duration {
    let h = host();
    let main = write(dir, "main.tl", tl_src);
    let (code, ci) = h.gen_lua(&main).unwrap();
    assert!(ci.errors.is_empty(), "{:?}", ci.errors);
    let code = code.expect("generated");
    let start = Instant::now();
    h.run_blocking(&code, "@main.tl", &[], &CancelToken::new())
        .unwrap();
    start.elapsed()
}

#[test]
fn echo_round_trips_stdout_and_exits_zero() {
    let dir = tempdir("echo");
    run(
        &dir,
        "local proc = require(\"std.proc\")\n\
         local e = await proc.run({\"echo\", \"-n\", \"hello proc\"})\n\
         assert(e.code == 0, tostring(e.code))\n\
         assert(e.stdout == \"hello proc\", e.stdout)\n\
         assert(e.stderr == \"\", e.stderr)\n\
         assert(e.timed_out == false, \"should not have timed out\")\n",
    );
}

/// `sh -c` with no program of its own but an inline script: stderr is captured whole and
/// the exit code is the shell's own `exit 3`, not `sh`'s own exit (0, since `sh` itself
/// ran to completion).
#[test]
fn stderr_is_captured_and_the_exit_code_is_the_childs_own() {
    let dir = tempdir("stderr");
    run(
        &dir,
        "local proc = require(\"std.proc\")\n\
         local e = await proc.run({\"sh\", \"-c\", \"echo err 1>&2; exit 3\"})\n\
         assert(e.code == 3, tostring(e.code))\n\
         assert(e.stderr == \"err\\n\", e.stderr)\n\
         assert(e.stdout == \"\", e.stdout)\n",
    );
}

/// `opts.stdin` is written to the child, then the pipe is closed: `cat` with nothing on
/// its own command line reads until EOF and echoes exactly what it was given.
#[test]
fn stdin_is_piped_through_to_the_child() {
    let dir = tempdir("stdin");
    run(
        &dir,
        "local proc = require(\"std.proc\")\n\
         local e = await proc.run({\"cat\"}, { stdin = \"piped through cat\" })\n\
         assert(e.code == 0, tostring(e.code))\n\
         assert(e.stdout == \"piped through cat\", e.stdout)\n",
    );
}

/// `opts.cwd` reaches the child: `pwd` prints exactly the directory `proc.run` was asked
/// to start it in.
#[test]
fn cwd_is_honoured() {
    let dir = tempdir("cwd");
    let want = dir.display().to_string();
    run(
        &dir,
        &format!(
            "local proc = require(\"std.proc\")\n\
             local cwd = \"{want}\"\n\
             local e = await proc.run({{\"pwd\"}}, {{ cwd = cwd }})\n\
             assert(e.code == 0, tostring(e.code))\n\
             assert(e.stdout == cwd .. \"\\n\", e.stdout)\n"
        ),
    );
}

/// `opts.env` adds to the child's inherited environment rather than replacing it: a
/// variable named only there is still visible to `sh -c 'echo $VAR'`.
#[test]
fn env_is_added_to_the_inherited_environment() {
    let dir = tempdir("env");
    run(
        &dir,
        "local proc = require(\"std.proc\")\n\
         local e = await proc.run(\n\
            {\"sh\", \"-c\", \"echo $HTL_STD_PROC_TEST\"},\n\
            { env = { HTL_STD_PROC_TEST = \"from-teal\" } }\n\
         )\n\
         assert(e.code == 0, tostring(e.code))\n\
         assert(e.stdout == \"from-teal\\n\", e.stdout)\n",
    );
}

/// `timeout_ms` shorter than the child's own `sleep` kills it: `timed_out` is `true`,
/// `code` is `nil` (killed, not exited on its own choosing), and `run_blocking` itself
/// returns well inside the 5 s the child was asked to sleep for — close to the 100 ms
/// timeout, not anywhere near the sleep. `sleep` is `proc.run`'s own direct child here
/// (no shell), so this does not yet exercise the process-group kill —
/// `timeout_kills_a_grandchild_under_a_non_exec_shell` below does.
#[test]
fn timeout_kills_a_long_running_child() {
    let dir = tempdir("timeout");
    let elapsed = run_timed(
        &dir,
        "local proc = require(\"std.proc\")\n\
         local e = await proc.run({\"sleep\", \"5\"}, { timeout_ms = 100 })\n\
         assert(e.timed_out == true, \"expected the sleep to time out\")\n\
         assert(e.code == nil, tostring(e.code))\n",
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "expected run_blocking to return well under 1s, took {elapsed:?}"
    );
}

/// The crate doc's `"sh -c "..." and other children that do not exec"`: `sh` does not
/// `exec` the `sleep` it runs, so `sleep` is a grandchild of this process, holding its
/// own copy of the stdout/stderr pipes. `timeout_ms = 100` has to reach past `sh` to
/// kill it too (`Child::kill` alone would not — only `sh` dies, `sleep` keeps running
/// and keeps the pipes open) for `run_blocking` to return anywhere near the timeout
/// rather than the grandchild's own 5 s sleep.
#[test]
fn timeout_kills_a_grandchild_under_a_non_exec_shell() {
    let dir = tempdir("timeout-grandchild");
    let elapsed = run_timed(
        &dir,
        "local proc = require(\"std.proc\")\n\
         local e = await proc.run({\"sh\", \"-c\", \"sleep 5; true\"}, { timeout_ms = 100 })\n\
         assert(e.timed_out == true, \"expected the shell's sleep to time out\")\n\
         assert(e.code == nil, tostring(e.code))\n",
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "expected run_blocking to return well under 1s even through a non-exec'd shell, \
         took {elapsed:?}"
    );
}

/// The crate doc's `"sh -c "sleep 5 &""` case: the direct child (`sh`) exits almost at
/// once on its own — `&` backgrounds `sleep` and `sh` has nothing left to do — so
/// `wait()` on it resolves quickly, well inside `timeout_ms`, and `timed_out` would
/// wrongly stay `false` if the deadline covered only `wait()`. The backgrounded
/// `sleep` is left running, still holding both pipes open, for its own full 5 s: the
/// deadline has to bound the drain that follows `wait()` too, or this would return only
/// once that `sleep` finally exits on its own, exactly as if there had been no
/// `timeout_ms` at all.
#[test]
fn timeout_kills_a_backgrounded_descendant_the_direct_child_already_left_behind() {
    let dir = tempdir("timeout-background");
    let elapsed = run_timed(
        &dir,
        "local proc = require(\"std.proc\")\n\
         local e = await proc.run({\"sh\", \"-c\", \"sleep 5 &\"}, { timeout_ms = 100 })\n\
         assert(e.timed_out == true, \"expected the backgrounded sleep to time out\")\n\
         assert(e.code == nil, tostring(e.code))\n",
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "expected run_blocking to return well under 1s even though the direct child \
         had already exited on its own, took {elapsed:?}"
    );
}

/// The two readers need not finish together: `2>/dev/null` on the backgrounded `sleep`
/// redirects *its* stderr away before it is left running, so only stdout stays held —
/// stderr closes as soon as the direct child (`sh`) itself exits, well before the
/// deadline, while stdout stays open for the backgrounded `sleep`'s full 5 s. Before
/// the fix this reproduced tokio's "JoinHandle polled after completion" panic (`sh -c
/// "sleep 5 2>/dev/null &"` + `timeout_ms = 100`, reviewer-measured exit 101): the
/// stderr reader resolved inside the bounded join, and the old code then polled its
/// already-completed handle a second time in the post-kill drain. `join_one` /
/// `drain_one` (`Proc::run`'s own source) exist to make that impossible by
/// construction — see `tests/proc.rs`'s own module doc, "State space", row for this
/// test.
#[test]
fn timeout_kills_a_background_descendant_holding_only_stdout() {
    let dir = tempdir("timeout-background-stdout-only");
    let elapsed = run_timed(
        &dir,
        "local proc = require(\"std.proc\")\n\
         local e = await proc.run(\n\
            {\"sh\", \"-c\", \"sleep 5 2>/dev/null &\"}, { timeout_ms = 100 }\n\
         )\n\
         assert(e.timed_out == true, \"expected the backgrounded sleep to time out\")\n\
         assert(e.code == nil, tostring(e.code))\n",
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "expected run_blocking to return well under 1s without panicking, took {elapsed:?}"
    );
}

/// The mirror of [`timeout_kills_a_background_descendant_holding_only_stdout`]: `>/dev/null`
/// redirects the backgrounded `sleep`'s stdout away instead, so stdout closes early and
/// stderr stays held — the other order the two readers can resolve out of step in.
#[test]
fn timeout_kills_a_background_descendant_holding_only_stderr() {
    let dir = tempdir("timeout-background-stderr-only");
    let elapsed = run_timed(
        &dir,
        "local proc = require(\"std.proc\")\n\
         local e = await proc.run(\n\
            {\"sh\", \"-c\", \"sleep 5 >/dev/null &\"}, { timeout_ms = 100 }\n\
         )\n\
         assert(e.timed_out == true, \"expected the backgrounded sleep to time out\")\n\
         assert(e.code == nil, tostring(e.code))\n",
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "expected run_blocking to return well under 1s without panicking, took {elapsed:?}"
    );
}

/// `opts.timeout_ms` present but generous enough that the child finishes long before
/// it: exercises the `Some` branch of the deadline match in both phases (`wait()`
/// itself, then the drain that follows it) all the way through its *success* path —
/// `joined.is_err()` false — rather than only ever seeing that branch's timeout path,
/// which every other `timeout_ms` test here does.
#[test]
fn a_deadline_that_is_never_hit_still_returns_normally() {
    let dir = tempdir("deadline-not-hit");
    let elapsed = run_timed(
        &dir,
        "local proc = require(\"std.proc\")\n\
         local e = await proc.run({\"echo\", \"-n\", \"hi\"}, { timeout_ms = 5000 })\n\
         assert(e.timed_out == false, \"the child exited well within the deadline\")\n\
         assert(e.code == 0, tostring(e.code))\n\
         assert(e.stdout == \"hi\", e.stdout)\n",
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "expected this to return almost immediately rather than waiting out the 5s \
         deadline, took {elapsed:?}"
    );
}

/// No `timeout_ms` at all, and the direct child's own `sh` backgrounds a `sleep` that
/// holds both pipes open rather than running one that holds them itself: the call
/// simply returns once that backgrounded descendant finally closes both on its own —
/// no deadline means no deadline, not "treat it as instant". `echo hi` runs first, in
/// the foreground, so its output is captured intact regardless of what happens after.
#[test]
fn no_deadline_returns_once_a_backgrounded_descendant_closes_both_pipes() {
    let dir = tempdir("no-deadline-background");
    let elapsed = run_timed(
        &dir,
        "local proc = require(\"std.proc\")\n\
         local e = await proc.run({\"sh\", \"-c\", \"echo hi; sleep 0.3 &\"})\n\
         assert(e.timed_out == false, \"no timeout_ms was set\")\n\
         assert(e.code == 0, tostring(e.code))\n\
         assert(e.stdout == \"hi\\n\", e.stdout)\n",
    );
    assert!(
        elapsed >= Duration::from_millis(250),
        "expected the call to wait for the backgrounded sleep (0.3s) to close the pipes \
         rather than return as soon as the direct child exited, took {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "expected this to return once the backgrounded sleep closed the pipes, well \
         under 1s, took {elapsed:?}"
    );
}

/// A task left unawaited when its scope ends is cancelled and waited for
/// (`crates/htl-core/src/task.rs`'s own `TASK_LUA`, `Task.__close`): `leave` starts
/// `proc.run` as an `async local` and returns, without ever awaiting it, after only a
/// 100 ms timer — the same shape `examples/embed`'s own `pair.tl` exercises for
/// `http.get`'s `/slow`, here with a real OS process standing in for the dropped
/// future. The direct child `proc.run` starts is the outer `sh`; it runs an *inner*
/// `sh -c 'sleep 2; touch <marker>'` in the foreground and only then `true` — so the
/// inner `sh` (a grandchild of this test, not the direct child) is the one that
/// actually sleeps and touches the marker. Killing only the direct child (the outer
/// `sh`) would not touch the inner one at all: an orphaned child is not itself killed
/// when its parent dies, so it would run to completion regardless and still produce
/// the marker two seconds later. Only the process-group kill (the crate doc's
/// "Cancellation", Unix bullet) reaches the inner `sh` too, which is what this test is
/// actually proving.
#[test]
fn a_cancelled_task_kills_the_child_and_its_grandchild_before_they_finish() {
    let dir = tempdir("cancel");
    let marker = dir.join("marker-touched").display().to_string();
    let tl_src = format!(
        "local proc = require(\"std.proc\")\n\
         local task = require(\"htl.task\")\n\
         local async function leave()\n\
         \x20  async local t = proc.run(\n\
         \x20     {{\"sh\", \"-c\", \"sh -c 'sleep 2; touch {marker}'; true\"}}\n\
         \x20  )\n\
         \x20  await task.after(100):wait()\n\
         end\n\
         await leave()\n"
    );

    let elapsed = run_timed(&dir, &tl_src);
    assert!(
        elapsed < Duration::from_millis(1500),
        "expected run_blocking to return well under 1.5s once the task was cancelled, \
         took {elapsed:?}"
    );

    // The real spawn happened at some point before `run_timed` returned above, so
    // sleeping a flat 2.5 s now (rather than measuring from a point taken *before* that
    // call, which would undercount by however long `host()` / `gen_lua` took) guarantees
    // at least 2.5 s have passed since the real spawn — comfortably past the 2 s the
    // grandchild's `sleep` would need to reach its own `touch`, however slow the checker
    // or the scheduler were under load.
    std::thread::sleep(Duration::from_millis(2500));
    assert!(
        !Path::new(&marker).exists(),
        "the grandchild should have been killed before it could touch the marker"
    );
}

/// The other half of "cancel during wait" above: here the outer `sh` *backgrounds* the
/// inner one (`&`) instead of running it in the foreground, so the direct child exits
/// almost at once — by the 100 ms mark `proc.run` is no longer inside `child.wait()`
/// at all, it is already blocked inside the unbounded drain that follows it (no
/// `timeout_ms` is set), waiting on the backgrounded inner `sh` to close both pipes on
/// its own. Cancelling `leave`'s task drops `proc.run`'s future from inside that
/// drain, not from inside `wait()` — [`KillProcessGroupOnDrop`] (`htl_std::proc`) does
/// not care which of the two it was suspended in, only that it is still armed, so the
/// group kill still reaches the backgrounded inner `sh` (and the `sleep` / `touch`
/// still ahead of it) before any of them finish.
#[test]
fn a_cancelled_task_during_drain_still_kills_the_backgrounded_descendant() {
    let dir = tempdir("cancel-drain");
    let marker = dir.join("marker-touched").display().to_string();
    let tl_src = format!(
        "local proc = require(\"std.proc\")\n\
         local task = require(\"htl.task\")\n\
         local async function leave()\n\
         \x20  async local t = proc.run(\n\
         \x20     {{\"sh\", \"-c\", \"sh -c 'sleep 2; touch {marker}' &\"}}\n\
         \x20  )\n\
         \x20  await task.after(100):wait()\n\
         end\n\
         await leave()\n"
    );

    let elapsed = run_timed(&dir, &tl_src);
    assert!(
        elapsed < Duration::from_millis(1500),
        "expected run_blocking to return well under 1.5s once the task was cancelled \
         mid-drain, took {elapsed:?}"
    );

    // Same flat 2.5 s margin as the "cancel during wait" test above, for the same
    // reason: guaranteed to be at least 2.5 s past the real spawn, comfortably past the
    // 2 s the backgrounded descendant's `sleep` would need to reach its own `touch`.
    std::thread::sleep(Duration::from_millis(2500));
    assert!(
        !Path::new(&marker).exists(),
        "the backgrounded descendant should have been killed mid-drain before it could \
         touch the marker"
    );
}

/// A program that does not exist raises with the program name (the Lua side's
/// `argv[1]`) and the OS's own message for why `spawn` failed, the same shape
/// `std.fs.read`'s own "a missing path raises and names it" test uses — computed from a
/// real `std::process::Command::spawn()` rather than typed as a literal, so the
/// assertion holds whatever OS the gate runs on.
#[test]
fn spawn_of_a_missing_program_raises_with_its_name_and_the_os_message() {
    let dir = tempdir("missing-program");
    let missing = "/no/such/htl-std-proc-test-binary";
    let cause = std::process::Command::new(missing)
        .spawn()
        .unwrap_err()
        .to_string();
    let cause_lua = cause.replace('\\', "\\\\").replace('"', "\\\"");
    run(
        &dir,
        &format!(
            "local proc = require(\"std.proc\")\n\
             local program = \"{missing}\"\n\
             local cause = \"{cause_lua}\"\n\
             local ok, err = pcall(async function(): proc.Exit return await proc.run({{program}}) end)\n\
             assert(not ok, \"expected a missing program to raise\")\n\
             local msg = tostring(err)\n\
             assert(string.find(msg, \"std.proc.run\", 1, true) ~= nil, msg)\n\
             assert(string.find(msg, program, 1, true) ~= nil, msg)\n\
             assert(string.find(msg, cause, 1, true) ~= nil, msg)\n"
        ),
    );
}

/// An empty `argv` has no program to run at all, so `run` raises before ever touching
/// `std::process::Command`.
#[test]
fn an_empty_argv_raises() {
    let dir = tempdir("empty-argv");
    run(
        &dir,
        "local proc = require(\"std.proc\")\n\
         local ok, err = pcall(async function(): proc.Exit return await proc.run({}) end)\n\
         assert(not ok, \"expected an empty argv to raise\")\n\
         local msg = tostring(err)\n\
         assert(string.find(msg, \"std.proc.run: argv is empty\", 1, true) ~= nil, msg)\n",
    );
}

/// The declaration the macro wrote is the one tracked at `dts/std/proc.d.tl`, `MODULE`
/// is the whole dotted `require` key, and `run` — the one function this module has —
/// carries the `---@async` marker: the same shape `fs.rs`'s own
/// `the_generated_declaration_is_tracked_and_every_function_is_async` checks for `std.fs`.
#[test]
fn the_generated_declaration_is_tracked_and_run_is_async() {
    assert_eq!(Proc::MODULE, "std.proc");
    let decl = Proc::DECL;
    let tracked =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/dts/std/proc.d.tl")).unwrap();
    assert_eq!(tracked, decl, "the tracked file is stale against the build");

    let fn_lines: Vec<&str> = decl.lines().filter(|l| l.contains(": function(")).collect();
    assert_eq!(fn_lines.len(), 1, "{decl}");
    for l in &fn_lines {
        assert!(l.trim_end().ends_with("---@async"), "{l}");
    }
}

/// `Htl::check` resolves `require("std.proc")` through the `add_path` [`host`] adds,
/// types `await proc.run(..)` clean, and reports `await-missing` on the same call with
/// `await` taken out — the same two-line proof `fs.rs`'s own
/// `checking_requires_await_and_reports_await_missing_without_it` runs for `std.fs`.
#[test]
fn checking_requires_await_and_reports_await_missing_without_it() {
    let dir = tempdir("check");
    let h = host();

    let with_await = write(
        &dir,
        "with_await.tl",
        "local proc = require(\"std.proc\")\n\
         local e = await proc.run({\"true\"})\n\
         print(e)\n",
    );
    let ci = h.check(&with_await).unwrap();
    assert!(ci.errors.is_empty(), "{:?}", ci.errors);
    assert!(
        ci.lints.iter().all(|l| !l.contains("[htl await")),
        "{:?}",
        ci.lints
    );

    let without_await = write(
        &dir,
        "without_await.tl",
        "local proc = require(\"std.proc\")\n\
         local e = proc.run({\"true\"})\n\
         print(e)\n",
    );
    let ci = h.check(&without_await).unwrap();
    assert!(ci.errors.is_empty(), "{:?}", ci.errors);
    let missing: Vec<&String> = ci
        .lints
        .iter()
        .filter(|l| l.contains("[htl await-missing]"))
        .collect();
    assert_eq!(missing.len(), 1, "{:?}", ci.lints);
    // Column 11: "local e = " is 10 characters, so `proc` (the callee's first token)
    // starts at the 11th, the way `fs.rs`'s own test pins the column for `fs.read`.
    assert!(
        missing[0].contains(
            "without_await.tl:2:11: call of an async function without await: proc.run may suspend"
        ),
        "{}",
        missing[0]
    );
}
