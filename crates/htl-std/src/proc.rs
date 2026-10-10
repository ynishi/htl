//! `std.proc`: run a child process to completion, asynchronously, from Teal.
//!
//! One function, [`Proc::run`]: `argv[1]` is the program and the rest its arguments —
//! there is no shell, so `proc.run({"sh", "-c", "echo hi"})` is how a caller gets one.
//! Its stdout and stderr are each captured whole, read concurrently so a child that
//! fills both pipes (one tokio task per pipe, draining it to EOF) cannot deadlock the
//! other against [`Child::wait`](tokio::process::Child::wait); there is no way to see a
//! byte of either before the process has exited. `run` does not stream: both outputs
//! are held whole in memory and returned once the child is done. A streaming `spawn`
//! that returned `task.RecvChannel`s would be a separate function with a different
//! shape. `tests/proc.rs`'s own module doc, "State space", is the one place the full
//! cross of what can happen here — whether the direct child has exited, whether a
//! descendant still holds either pipe, whether (and where) a deadline or a cancel hits
//! — is enumerated against the test that covers each reachable combination; it is not
//! repeated here.
//!
//! # `sh -c "..."` and other children that do not `exec`
//!
//! `argv = {"sh", "-c", "sleep 5"}` — the shape this module's own doc recommends for
//! anything beyond a bare program and its arguments — does not make `sleep` the child:
//! `sh` is the child, and `sleep` is *its* child, a grandchild of this process, holding
//! its own copy of the stdout and stderr pipes open for as long as it runs. A
//! `timeout_ms` or a cancel that only reached `sh` (`tokio::process::Command`'s own
//! `kill_on_drop`, or a plain `Child::kill`, reach the direct child alone) would kill
//! `sh` and leave `sleep` running, still holding the write end of each pipe — so this
//! would wait out the grandchild's own full five seconds despite a 100 ms `timeout_ms`,
//! reading its drain tasks to the end, the entire reason `timeout_ms` and cancellation
//! exist defeated by the exact shape they are meant to bound, were the kill not reaching
//! the whole group rather than one process (below). `argv = {"sh", "-c", "sleep 5 &"}` is
//! the same shape with `sh` exiting at once of its own accord: `wait()` on the direct
//! child returns almost immediately, well inside any `timeout_ms`, with the backgrounded
//! `sleep` left running and still holding both pipes open on its own — which is exactly
//! why `timeout_ms` bounds the drain that follows `wait()`, not only `wait()` itself (see
//! [`Proc::run`]'s own doc). Without a `timeout_ms` at all, such a call simply returns
//! only once every holder of the pipes is gone, backgrounded `sleep` included — which is
//! what asking for no limit means; this module does not invent one where the caller did
//! not ask for it.
//!
//! On Unix, `run` puts the child in its own new process group
//! (`tokio::process::Command::process_group(0)`, the group id equal to the child's own
//! pid) before spawning it; every descendant that does not itself ask to leave that
//! group (a plain fork, as `sh` running `sleep` without `exec`ing it is) stays in it.
//! Killing stops there with `libc::killpg` rather than [`Child::kill`], which reaches
//! only the one process whose handle `tokio::process::Command` returned. On every other
//! platform `run` still only has [`Child::kill`] / `kill_on_drop` to reach with, so a
//! descendant that outlives the direct child is not killed there; see "Cancellation"
//! below for exactly what each platform's kill reaches.
//!
//! Even the process group has a gap a descendant can step outside of on purpose
//! (`setsid`, a double fork, anything that leaves the group deliberately), which is why
//! the pipe drain the group kill is meant to make fast is bounded rather than trusted —
//! see [`Proc::run`]'s own doc, "Draining after a kill".
//!
//! # Signals (Unix)
//!
//! `process_group(0)` has a second effect beyond giving `killpg` something to reach:
//! taking the child out of the terminal's own foreground process group also takes it out
//! of the terminal's own signal delivery, so Ctrl-C (`SIGINT`), Ctrl-\ (`SIGQUIT`) and
//! Ctrl-Z (`SIGTSTP`) typed at a terminal no longer reach it directly the way they would
//! a plain foreground child — only this process's own handling of them, and what that
//! handling does in turn, still can. Under `htl run`, a first Ctrl-C is exactly that: it
//! becomes `token.cancel()`, mlua-isle's `cancellable` drops the suspended future at its
//! next chance to, and [`KillProcessGroupOnDrop`]'s drop (below) kills the group — the
//! child still dies, just relayed through this process rather than struck directly by the
//! terminal. A second Ctrl-C (`htl run`'s own escalation to `std::process::exit(130)`)
//! does not unwind the stack, so no `Drop` runs at all; a `SIGKILL` of this process itself
//! has the same effect. Either way the guard never fires, and the child's whole process
//! group outlives this process. A host linking `htl` through the C ABI, rather than going
//! through `htl run`, is responsible for noticing the same thing on its own exit path —
//! and, if it uses `opts.stdin`, for ignoring `SIGPIPE` itself (`signal(SIGPIPE, SIG_IGN)`
//! or equivalent): Rust's own runtime does that once, for a Rust binary's `fn main`, and
//! that protection does not extend to a host process that was never one — writing to a
//! child's stdin after it has already closed its read end raises exactly that signal.
//!
//! # Cancellation
//!
//! [`Fs`](crate::Fs)'s crate doc describes a future dropped out from under an `await`
//! that was there — a program cancel, or an `async local` task never awaited before its
//! scope ends — and says that for a file, dropping the Rust future does not reach the
//! operation already running on tokio's blocking pool: there is nothing to cancel it
//! with, so it runs to completion and the result is discarded unread. A child process is
//! the opposite case, and exactly what each platform's kill reaches is a fact about that
//! platform, not a choice this crate gets to round up:
//!
//! - **Unix**: the dropped future's locals are torn down in the usual order, which runs
//!   a guard ([`KillProcessGroupOnDrop`]) built right after the child is spawned, before
//!   `tokio::process::Child`'s own `kill_on_drop` guard (declared first, torn down
//!   last). That guard's drop sends `SIGKILL` to the whole process group with
//!   `libc::killpg` — see "`sh -c "..."` and other children that do not `exec`" above
//!   for why the direct child alone is not enough. So a cancelled `proc.run` on Unix
//!   kills the child *and every descendant that has not left its process group* —
//!   `kill_on_drop`'s own kill of the direct child still fires too, redundantly.
//! - **Everywhere else**: only `kill_on_drop`'s kill of the direct child fires. A
//!   descendant that child itself started may outlive it.
//!
//! The two tasks this crate spawns to drain stdout and stderr are not part of the future
//! that gets dropped, so a cancel does not reach into them directly either way — they
//! are left running, to notice the pipe close once the kill (whichever of the above it
//! was) lands, and exit on their own; their output, by then uncollected, is discarded
//! along with the future that would have awaited it.

use htl::mlua::{Lua, LuaString};
use htl::{TealRecord, host_module};
use std::collections::HashMap;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

/// What a caller may set beyond `argv` itself. Every field is optional and `nil` is the
/// default for each: no `cwd` change, no extra environment, no stdin (the child's stdin
/// is `/dev/null`, not inherited — see [`Proc::run`]), no timeout.
#[derive(TealRecord, Debug, Clone, Default)]
pub struct Options {
    /// The child's working directory. `nil` leaves it at the caller's own.
    pub cwd: Option<String>,
    /// Environment variables added to the child's *inherited* environment — this is not
    /// `env_clear`, so a variable left out here is still whatever the host process had,
    /// and one named here overrides it.
    pub env: Option<HashMap<String, String>>,
    /// Text written to the child's stdin, then the pipe is closed — a child that reads
    /// until EOF sees exactly this. `nil` means no stdin pipe at all: the child's stdin
    /// is `/dev/null`, so a program that tries to read from it sees EOF immediately
    /// rather than blocking on a terminal that is not there.
    pub stdin: Option<String>,
    /// Milliseconds to let the child run before it is killed. `nil` means no limit.
    pub timeout_ms: Option<u64>,
}

/// What [`Proc::run`] answers with once the child has exited or been killed.
#[derive(TealRecord, Debug, Clone)]
pub struct Exit {
    /// The process's exit code. `nil` when it was killed by a signal, and `nil`
    /// whenever [`Exit::timed_out`] is `true` — a killed child reports why through
    /// `timed_out` and `signal`, not through a code it never chose.
    pub code: Option<i64>,
    /// The signal that killed the *direct* child, on Unix; `nil` on every other
    /// platform, and `nil` on Unix too for a direct child that simply exited. A
    /// `timeout_ms` expiry (or a cancel) kills with `SIGKILL`, but that kill reaches
    /// the whole process group at once (the crate doc's "Cancellation") — so on a
    /// timed-out run this is `9` only when the direct child itself was the one still
    /// running and killed; when the direct child had already exited on its own and
    /// only a descendant was still holding a pipe open (`{"sh", "-c", "sleep 5 &"}`),
    /// the direct child's own exit status is what this reports, which did not come
    /// from a signal at all, so this is `nil`.
    pub signal: Option<i64>,
    /// Everything the child wrote to stdout, byte for byte — not required to be valid
    /// UTF-8, the same promise [`Fs::read_binary`](crate::Fs::read_binary) makes for a
    /// file's bytes, and for the same reason: a `String` here would have to raise or
    /// silently reinterpret non-UTF-8 output, and silently reinterpreting it is exactly
    /// what [`Fs::walk`](crate::Fs::walk) refuses to do to a path name. `LuaString`
    /// (which `#[derive(TealRecord)]` maps to `string`, the same as any other field of
    /// this type) sidesteps the question: a Lua string holds any byte sequence, so the
    /// bytes cross unchanged either way, and nothing here has to choose between raising
    /// and lying about what the child wrote. A `timeout_ms` kill (or a cancel) still
    /// returns whatever had already reached this process by then — see [`Proc::run`]'s
    /// "Draining after a kill".
    pub stdout: LuaString,
    /// Everything the child wrote to stderr, byte for byte. See [`Exit::stdout`].
    pub stderr: LuaString,
    /// `true` if the call hit its `timeout_ms` deadline — the direct child itself
    /// still running, or (the direct child already having exited on its own) a
    /// descendant still holding its stdout or stderr open — and the process group was
    /// killed to get here at all. `false` for everything else, `timeout_ms` unset
    /// included.
    pub timed_out: bool,
}

/// Stateless, like [`Fs`](crate::Fs): `require("std.proc")` is called with `.`, not `:`
/// (`proc.run(argv)`), since there is nothing of a `Proc` to hold between calls.
pub struct Proc;

#[host_module(name = "std.proc", dts = "dts/std/proc.d.tl", records = [Options, Exit])]
impl Proc {
    /// Runs `argv[1]` with `argv[2..]` as its arguments — no shell, so a pipeline or a
    /// redirect needs `argv = {"sh", "-c", "..."}` to ask a shell for one (and see the
    /// crate doc's `"sh -c "..."` and other children that do not exec"` for what that
    /// shape means for `timeout_ms` and cancellation). Raises
    /// `std.proc.run: argv is empty` for an empty `argv`, and
    /// `std.proc.run: <argv[1]>: <OS message>` if the program cannot even be spawned
    /// (not found, not executable, and so on — whatever `std::process::Command::spawn`
    /// itself reports).
    ///
    /// stdout and stderr are each captured whole (see the crate doc's first paragraph
    /// for why this is one answer rather than a stream) and stdin is a closed pipe
    /// unless `opts.stdin` gives it something to write, in which case that text is
    /// written and the pipe closed — a program reading until EOF sees exactly that text
    /// and nothing after it. `opts.cwd` and `opts.env` reach `tokio::process::Command`'s
    /// own `current_dir` / `envs` unchanged: `env` *adds* to the inherited environment,
    /// it does not replace it.
    ///
    /// `opts.timeout_ms`, when set, is a deadline over the *whole* call, not only over
    /// the direct child's own exit: `{"sh", "-c", "sleep 5 &"}` has `wait()` on the
    /// direct child (`sh`) return almost at once, well inside any reasonable
    /// `timeout_ms`, while the backgrounded `sleep` it leaves behind keeps both pipes
    /// open on its own — so the clock keeps running underneath the drain that follows
    /// `wait()`, and expiring there is exactly as much a timeout as expiring during
    /// `wait()` itself is: either way the child (on Unix, its whole process group — see
    /// the crate doc's "Cancellation") is killed, and [`Exit::timed_out`] is `true` on
    /// the way back, *even if the direct child had already exited by then* — `timed_out`
    /// means this call hit its deadline, not only "the child itself was killed by it".
    /// [`Exit::code`] is then always `nil`, whatever the killed process's own status
    /// happened to carry, since a timed-out run chose nothing. See the crate doc's
    /// "Cancellation" for what happens to a call that is itself cancelled — by the whole
    /// program, or by an `async local` task left unawaited at its scope's end — rather
    /// than timing out on its own account: the same kill, for the same reason, sent by
    /// a drop instead of by this function's own timeout handling.
    ///
    /// # Draining after a kill
    ///
    /// stdout and stderr are each drained by their own tokio task into a buffer shared
    /// with this function (an `Arc<Mutex<Vec<u8>>>`), rather than a buffer local to the
    /// task and only handed back when it finishes: a run that reaches the end of its
    /// deadline (or has none) simply awaits both tasks, which finishes quickly once
    /// every holder of both pipes — the direct child, and on Unix the rest of its
    /// process group — has exited on its own, but a run that was killed, whether
    /// because `wait()` itself timed out or because the drain that followed it did,
    /// bounds that wait to 200 ms instead of trusting the kill to have closed every
    /// pipe (the dropped-future cancellation case has no "after" to run code in, so it
    /// has no such bound — see "Cancellation" above). The group kill closes both pipes
    /// for anything that stayed inside the group; a descendant that left it on purpose
    /// (`setsid`, a double fork) would otherwise hang this call exactly as if it had
    /// never been killed at all. Once the bound elapses, whatever had already landed in
    /// the shared buffer is what [`Exit::stdout`] / [`Exit::stderr`] carries, and the
    /// still-running drain task (its `JoinHandle` dropped, not aborted) is left to
    /// finish on its own, unseen, same as a cancelled call's own drain tasks.
    pub async fn run(argv: Vec<String>, opts: Option<Options>, lua: &Lua) -> anyhow::Result<Exit> {
        let Some((program, rest)) = argv.split_first() else {
            anyhow::bail!("std.proc.run: argv is empty");
        };
        let opts = opts.unwrap_or_default();

        let mut cmd = Command::new(program);
        cmd.args(rest);
        cmd.kill_on_drop(true);
        // See the crate doc's "Cancellation": on every other platform this is the only
        // kill a drop or a timeout can reach.
        #[cfg(unix)]
        cmd.process_group(0);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.stdin(if opts.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        if let Some(cwd) = &opts.cwd {
            cmd.current_dir(cwd);
        }
        if let Some(env) = &opts.env {
            cmd.envs(env);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("std.proc.run: {program}: {e}"))?;

        // Declared right after `spawn`, ahead of everything below: if this function's
        // future is dropped at any point from here on (a program cancel, or an
        // `async local` task left unawaited), Rust tears down locals in reverse
        // declaration order, so this guard's `Drop` — the group kill — runs before
        // `child`'s own `kill_on_drop` guard does. Either order kills the direct child;
        // only this one reaches the rest of its group.
        #[cfg(unix)]
        let pid = child.id().expect("the child has not been waited on yet") as libc::pid_t;
        #[cfg(unix)]
        let mut group_guard = KillProcessGroupOnDrop::new(pid);

        // Drained on their own tokio tasks, concurrently with each other and with
        // `child.wait()` below: a child that fills both the stdout and the stderr pipe
        // before exiting would otherwise deadlock against whichever of the two this
        // function read first, or against `wait()` itself if neither were read until
        // the child had already exited. Each writes into a buffer this function still
        // holds a handle to, not only a local one handed back when the task finishes —
        // see "Draining after a kill" above for why.
        let stdout_buf = Arc::new(Mutex::new(Vec::new()));
        let stderr_buf = Arc::new(Mutex::new(Vec::new()));
        let stdout = child.stdout.take().expect("stdout is piped above");
        let stderr = child.stderr.take().expect("stderr is piped above");
        // `Option`, not a bare `JoinHandle`: tokio panics ("JoinHandle polled after
        // completion") if the same handle is ever polled again once it has already
        // resolved, and the two phases below do not resolve the two readers in lockstep
        // — one pipe can close well before the other (a child that only redirects one
        // of them away before backgrounding, say), so by the time the first phase's
        // bound expires, one reader may already be done while the other still is not.
        // `join_one` below is the only thing allowed to touch either `Option`, and it
        // takes the handle out the moment it resolves, so nothing downstream can poll
        // the same completed handle a second time.
        let mut stdout_task = Some(tokio::spawn(drain_into(stdout, stdout_buf.clone())));
        let mut stderr_task = Some(tokio::spawn(drain_into(stderr, stderr_buf.clone())));
        if let Some(text) = opts.stdin.clone() {
            let mut stdin = child.stdin.take().expect("stdin is piped above");
            tokio::spawn(async move {
                let _ = stdin.write_all(text.as_bytes()).await;
                // `stdin` is dropped here, closing the pipe, whether the write above
                // succeeded or the child had already stopped reading.
            });
        }

        // `deadline` is the whole call's budget, not only `wait()`'s: a `Copy` pair
        // (`Instant` and `Duration` both are), so matching it below does not consume
        // the binding and it is still there, unchanged, for the drain phase after.
        let deadline = opts
            .timeout_ms
            .map(|ms| (Instant::now(), Duration::from_millis(ms)));

        let (status, mut timed_out) = match deadline {
            Some((_, dur)) => match tokio::time::timeout(dur, child.wait()).await {
                Ok(status) => (status, false),
                Err(_) => {
                    #[cfg(unix)]
                    kill_process_group(pid);
                    let _ = child.start_kill();
                    (child.wait().await, true)
                }
            },
            None => (child.wait().await, false),
        };
        let status = status.map_err(|e| anyhow::anyhow!("std.proc.run: {program}: {e}"))?;

        // The direct child's own exit is not necessarily the whole call's: a
        // descendant it left behind in the group (`{"sh", "-c", "sleep 5 &"}` — `sh`
        // exits at once, the backgrounded `sleep` keeps both pipes open) can still be
        // running. Whatever is left of the deadline still bounds this join, and
        // expiring here is exactly as much a timeout as `wait()` itself timing out
        // above was — the group is killed now, and `timed_out` becomes `true` even
        // though the direct child had already exited on its own. `join_one` (not
        // `&mut stdout_task` directly) is what actually polls each reader, because the
        // two need not resolve together — a child that only redirected one of its two
        // streams away before backgrounding (`sleep 5 2>/dev/null &` leaves stdout
        // held, stderr already closed) lets one `join_one` finish well before the
        // other, and `tokio::join!` still has to wait out the slower one, dropping
        // both when (if) the surrounding timeout fires regardless of which, if any,
        // had already finished.
        if !timed_out {
            match deadline {
                Some((start, dur)) => {
                    let remaining = dur.saturating_sub(start.elapsed());
                    let joined = tokio::time::timeout(remaining, async {
                        tokio::join!(join_one(&mut stdout_task), join_one(&mut stderr_task));
                    })
                    .await;
                    if joined.is_err() {
                        #[cfg(unix)]
                        kill_process_group(pid);
                        let _ = child.start_kill();
                        timed_out = true;
                    }
                }
                None => {
                    tokio::join!(join_one(&mut stdout_task), join_one(&mut stderr_task));
                }
            }
        }

        // A reader still outstanding when the bound above (whichever of the two just
        // ran) expired gets one bounded last chance now — see "Draining after a kill"
        // above. `drain_one` skips a reader that is already `None`: `join_one` set it
        // that way the moment it resolved, possibly already inside the join above, and
        // tokio panics ("JoinHandle polled after completion") if a completed handle is
        // polled again — the bug this `Option` tracking exists to rule out by
        // construction rather than by hoping the two readers happen to finish together.
        if timed_out {
            let drain_bound = Duration::from_millis(200);
            drain_one(&mut stdout_task, drain_bound).await;
            drain_one(&mut stderr_task, drain_bound).await;
        }
        let stdout_bytes = std::mem::take(&mut *stdout_buf.lock().unwrap());
        let stderr_bytes = std::mem::take(&mut *stderr_buf.lock().unwrap());

        // Only now — every reader either joined or given its bounded last chance — is
        // the group's pid no longer this call's to signal. Linux does not reuse a pid
        // as long as something still names it as a process group id, so holding the
        // guard open until here, rather than disarming it right after `wait()` reaped
        // the direct child, cannot land a later `killpg` on an unrelated process; it
        // only means a cancel at any point up to here still reaches the whole group,
        // a lingering descendant included, rather than only whatever `wait()` alone
        // had already stopped seeing.
        #[cfg(unix)]
        group_guard.disarm();

        let (mut code, signal) = exit_status_parts(&status);
        if timed_out {
            code = None;
        }

        Ok(Exit {
            code,
            signal,
            stdout: lua.create_string(stdout_bytes)?,
            stderr: lua.create_string(stderr_bytes)?,
            timed_out,
        })
    }
}

/// Reads `reader` to EOF (or a read error, treated the same as EOF: whatever was
/// already captured stands) one chunk at a time, appending each chunk to `buf` as it
/// arrives rather than accumulating locally and handing the whole thing back only at
/// the end — so a concurrent read of `buf` partway through (`Proc::run`'s own "Draining
/// after a kill") sees whatever has landed so far, not nothing until this returns.
async fn drain_into(mut reader: impl tokio::io::AsyncRead + Unpin, buf: Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.lock().unwrap().extend_from_slice(&chunk[..n]),
        }
    }
}

/// Awaits `*slot`'s handle if it still holds one, taking it out (`None` from here on)
/// the moment it resolves. A completed [`tokio::task::JoinHandle`] panics
/// ("JoinHandle polled after completion") if it is ever polled again, so this is the
/// only thing in [`Proc::run`] allowed to touch either reader's handle at all — every
/// call site goes through this (or [`drain_one`], which calls this), never `.await`
/// directly on the `Option`'s contents. If `*slot` is already `None` (this reader
/// resolved on an earlier call), this is a future that never resolves, so a
/// `tokio::join!` or `tokio::time::timeout` built from two of these can only ever be
/// won by whichever reader is genuinely still outstanding, never by re-polling the one
/// that already finished.
async fn join_one(slot: &mut Option<tokio::task::JoinHandle<()>>) {
    match slot {
        Some(handle) => {
            let _ = handle.await;
            *slot = None;
        }
        None => std::future::pending().await,
    }
}

/// [`join_one`], bounded to `bound` and skipped outright if `*slot` is already `None` —
/// used only in [`Proc::run`]'s post-kill drain, where one reader having already
/// resolved while the other did not is the exact case a bare bound (without the
/// `is_some` check) would otherwise spend the whole `bound` waiting out for nothing.
async fn drain_one(slot: &mut Option<tokio::task::JoinHandle<()>>, bound: Duration) {
    if slot.is_some() {
        let _ = tokio::time::timeout(bound, join_one(slot)).await;
    }
}

/// `(code, signal)` out of a [`std::process::ExitStatus`]: `code()` is already `nil`
/// (`None`) on Unix for a process a signal killed, so the two never disagree about
/// which one fired — only the platform decides whether `signal` can be anything but
/// `nil` at all.
fn exit_status_parts(status: &ExitStatus) -> (Option<i64>, Option<i64>) {
    let code = status.code().map(i64::from);
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        status.signal().map(i64::from)
    };
    #[cfg(not(unix))]
    let signal = None;
    (code, signal)
}

/// Sends `SIGKILL` to every process in the group `pgid` names (`killpg(2)`, the way
/// `kill -<pgid>` from a shell does) — used both by [`KillProcessGroupOnDrop`] (a
/// dropped future: a program cancel, or an `async local` task left unawaited) and
/// directly by [`Proc::run`] on a `timeout_ms` expiry. See the crate doc's "Unix" bullet
/// under "Cancellation" for why this, rather than `Child::kill`, is what reaches a
/// grandchild like the `sleep` in `{"sh", "-c", "sleep 5"}`.
#[cfg(unix)]
fn kill_process_group(pgid: libc::pid_t) {
    // SAFETY: `killpg` only ever signals a process group id this function was handed by
    // `Proc::run`, which read it off `Child::id()` right after `process_group(0)` made
    // that same child the leader of a brand-new group — never an arbitrary or stale
    // pid. `KillProcessGroupOnDrop::disarm` stops it from being called again once that
    // child has been reaped, past which the id is no longer this process's to signal.
    let _ = unsafe { libc::killpg(pgid, libc::SIGKILL) };
}

/// Kills [`Proc::run`]'s whole process group (Unix only — see the crate doc's
/// "Cancellation") if dropped before [`KillProcessGroupOnDrop::disarm`] is called:
/// the cancellation half of that section, covering every descendant that stayed in the
/// group `process_group(0)` put the child in, not only the one child
/// `tokio::process::Command`'s own `kill_on_drop` already reaches. Stays armed past the
/// direct child's own `wait()` — [`Proc::run`] disarms it only once every reader of its
/// pipes has joined or been given its bounded last chance, not the moment `wait()`
/// reaps the direct child — because a child's exit does not mean the group is empty
/// (`{"sh", "-c", "sleep 5 &"}`'s `sh` exits long before the `sleep` it left behind
/// does), and that longer-armed window is safe for the same reason it is necessary: on
/// Linux a pid is not reused for an unrelated process for as long as something still
/// names it as a process group id, so a `killpg` sent any time before every holder of
/// that id is actually gone can only ever reach processes that belong there.
#[cfg(unix)]
struct KillProcessGroupOnDrop(Option<libc::pid_t>);

#[cfg(unix)]
impl KillProcessGroupOnDrop {
    fn new(pgid: libc::pid_t) -> Self {
        Self(Some(pgid))
    }

    /// Stops the drop from killing anything: called once [`Proc::run`] has nothing
    /// left to wait on in this group — every pipe reader has either joined on its own
    /// or been given its bounded last chance to. Past this point the group id may no
    /// longer name anything this call started, and the OS is free to reuse it for an
    /// unrelated process.
    fn disarm(&mut self) {
        self.0 = None;
    }
}

#[cfg(unix)]
impl Drop for KillProcessGroupOnDrop {
    fn drop(&mut self) {
        if let Some(pgid) = self.0.take() {
            kill_process_group(pgid);
        }
    }
}
