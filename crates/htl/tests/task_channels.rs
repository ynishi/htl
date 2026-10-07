//! `htl.task`'s channels, timers and `select` (mlua-isle 0.9's), typed by `task.d.tl`, and
//! the host channels a `#[host_module]` hands to Teal (`htl::task::RecvChannel` /
//! `SendChannel` / `Request`): what the checker accepts and refuses, that a host feeds and
//! drains a Teal loop through them, that a generated declaration imports `htl.task` for a
//! channel, and that `await-missing` reports the waits written without `await`. The
//! library's own run-time cases, the joined state of a task case included, are in
//! `task_lib.rs`.

#![cfg(feature = "async")]

use htl::config::LangConfig;
use htl::mlua_isle::runtime::CancelToken;
use htl::task::{RecvChannel, Request, SendChannel};
use htl::teal::HostModule as _;
use htl::{Htl, TealRecord, host_module, include_tl};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- the host

#[derive(TealRecord, Debug, Clone, PartialEq)]
pub struct Event {
    pub n: i64,
}

#[derive(TealRecord, Debug, Clone, PartialEq)]
pub struct Report {
    pub text: String,
}

/// A daemon's host side: events and requests go to Teal, reports come back. The channels
/// are made on the program's state before it runs and handed out by the methods.
pub struct Daemon {
    events: RecvChannel<Event>,
    calls: RecvChannel<Request<String, i64>>,
    reports: SendChannel<Report>,
}

#[host_module(
    name = "daemon",
    dts = "tests/fixtures/task_channels/daemon.d.tl",
    records = [Event, Report]
)]
impl Daemon {
    pub fn events(&self) -> RecvChannel<Event> {
        self.events.clone()
    }
    pub fn calls(&self) -> RecvChannel<Request<String, i64>> {
        self.calls.clone()
    }
    pub fn reports(&self) -> SendChannel<Report> {
        self.reports.clone()
    }
    /// A channel Teal hands back: `true` when it is the one `reports` handed out.
    pub fn is_reports(&self, ch: SendChannel<Report>) -> bool {
        ch.table() == self.reports.table()
    }
}

/// Checked against the declaration above at `cargo build`: this file compiling is the
/// pass case of the generated `.d.tl`.
const MAIN: &str = include_tl!("tests/fixtures/task_channels/main.tl");

// ---------------------------------------------------------------- helpers

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("htl-task-channels-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, text).unwrap();
    p
}

fn checker(dir: &Path, lang_async: bool) -> Htl {
    let h = Htl::new().unwrap();
    h.set_lang(&LangConfig {
        async_: Some(lang_async),
    })
    .unwrap();
    h.install_task_lib().unwrap();
    h.add_path(dir).unwrap();
    h
}

/// The errors of checking `src` (written as `main.tl` in a fresh directory, beside the
/// generated `daemon.d.tl`).
fn errors_of(name: &str, src: &str) -> Vec<String> {
    let dir = scratch(name);
    write(&dir, "daemon.d.tl", Daemon::DECL);
    let main = write(&dir, "main.tl", src);
    checker(&dir, false).check(&main).unwrap().errors
}

fn run(h: &Htl, src: &str) -> anyhow::Result<()> {
    h.run_blocking(src, "=test", &[], &CancelToken::new())
}

fn global<T: htl::mlua::FromLua>(h: &Htl, name: &str) -> T {
    h.lua().globals().get(name).unwrap()
}

fn program_state() -> (Htl, Htl) {
    let checker = Htl::new().unwrap();
    let h = Htl::with_checker(&checker).unwrap();
    h.install_task_lib().unwrap();
    (checker, h)
}

// ---------------------------------------------------------------- 1. types

/// Every function of the library with the types `task.d.tl` gives it: channels in both
/// directions and both forms of select, timers, a ticker, a task's case.
const TYPED: &str = "\
local task = require(\"htl.task\")
local daemon = require(\"daemon\")

local ch: task.Channel<integer> = task.channel(4)
local t = task.spawn(function(): integer return 1 end)
local tk = task.ticker(10)
local got: string = task.select({
   ch:on(function(v: integer, ok: boolean): string return ok and tostring(v) or \"closed\" end),
   ch:on_send(2, function(sent: boolean): string return tostring(sent) end),
   task.after(5):on(function(): string return \"timer\" end),
   tk:on(function(ms: number, _ok: boolean): string return tostring(ms) end),
   t:on(function(ok: boolean, n: integer): string return tostring(ok) .. n end),
}, { biased = true, default = function(): string return \"none\" end })
local i, a, b = task.select_raw({ ch:arm_recv(), ch:arm_send(3), task.after(1):arm(), tk:arm_recv(), t:arm() }, { default = true })
local n: integer = i
tk:stop()
task.after(1):wait()
local v, ok = ch:recv()
local w, ready, open = ch:try_recv()
ch:send(1)
local sent: boolean = ch:try_send(1)
local events: task.RecvChannel<daemon.Event> = daemon:events()
local ev, more = events:recv()
local reports: task.SendChannel<daemon.Report> = daemon:reports()
reports:send({ text = \"x\" })
local as_recv: task.RecvChannel<integer> = ch
local as_send: task.SendChannel<integer> = ch
local same: boolean = daemon:is_reports(reports)
local calls = daemon:calls()
local req = calls:recv()
local answered: boolean = req:reply(#req.value)
print(got, n, a, b, v, ok, w, ready, open, sent, ev.n, more, as_recv:len(), as_send:cap(), same, answered, req:replied())
";

#[test]
fn the_library_checks_with_its_declared_types() {
    let errs = errors_of("typed", TYPED);
    assert!(errs.is_empty(), "{errs:#?}");
}

#[test]
fn a_handler_whose_parameter_is_not_the_channels_element_type_is_refused() {
    let errs = errors_of(
        "handler",
        "local task = require(\"htl.task\")\n\
         local ch: task.Channel<integer> = task.channel(1)\n\
         print(task.select({ ch:on(function(v: string, _ok: boolean): string return v end) }))\n",
    );
    assert_eq!(errs.len(), 1, "{errs:#?}");
    assert!(
        errs[0].contains("main.tl:3:") && errs[0].contains("got string, expected integer"),
        "{errs:#?}"
    );
}

#[test]
fn a_host_event_handler_of_the_wrong_record_is_refused() {
    let errs = errors_of(
        "host-handler",
        "local task = require(\"htl.task\")\n\
         local daemon = require(\"daemon\")\n\
         print(task.select({ daemon:events():on(function(r: daemon.Report, _ok: boolean): string return r.text end) }))\n",
    );
    assert_eq!(errs.len(), 1, "{errs:#?}");
    assert!(
        errs[0].contains("main.tl:3:") && errs[0].contains("daemon.Report is not a Event"),
        "{errs:#?}"
    );
}

#[test]
fn send_on_a_receive_only_channel_is_refused() {
    let errs = errors_of(
        "send-recv",
        "local daemon = require(\"daemon\")\n\
         daemon:events():send({ n = 1 })\n",
    );
    assert_eq!(errs.len(), 1, "{errs:#?}");
    assert!(
        errs[0].contains("main.tl:2:") && errs[0].contains("invalid key 'send'"),
        "{errs:#?}"
    );
}

#[test]
fn recv_on_a_send_only_channel_is_refused() {
    let errs = errors_of(
        "recv-send",
        "local daemon = require(\"daemon\")\n\
         print(daemon:reports():recv())\n",
    );
    assert_eq!(errs.len(), 1, "{errs:#?}");
    assert!(
        errs[0].contains("main.tl:2:") && errs[0].contains("invalid key 'recv'"),
        "{errs:#?}"
    );
}

#[test]
fn cases_of_different_result_types_in_one_select_are_refused() {
    let errs = errors_of(
        "mixed",
        "local task = require(\"htl.task\")\n\
         local ch: task.Channel<integer> = task.channel(1)\n\
         local a = ch:on(function(v: integer, _ok: boolean): string return tostring(v) end)\n\
         local b = task.after(10):on(function(): integer return 1 end)\n\
         local s = task.select({ a, b })\n\
         print(s)\n",
    );
    assert_eq!(errs.len(), 1, "{errs:#?}");
    assert!(
        errs[0].contains("main.tl:5:") && errs[0].contains("got integer, expected string"),
        "{errs:#?}"
    );
}

#[test]
fn a_tasks_case_handler_is_typed_from_the_task() {
    let errs = errors_of(
        "task-case",
        "local task = require(\"htl.task\")\n\
         local t = task.spawn(function(): integer return 1 end)\n\
         print(task.select({ t:on(function(_ok: boolean, s: string): string return s end) }))\n",
    );
    assert_eq!(errs.len(), 1, "{errs:#?}");
    assert!(
        errs[0].contains("main.tl:3:") && errs[0].contains("got string, expected integer"),
        "{errs:#?}"
    );
}

// ---------------------------------------------------------------- 2. run time

/// The host's main loop of `main.tl`, as `include_tl!` generated it: events and a request
/// from Rust, a report back, each converted by its Rust type.
#[test]
fn a_host_feeds_and_drains_the_teal_loop_through_typed_channels() {
    let (_c, h) = program_state();
    let (tx, events) = RecvChannel::<Event>::new(h.lua(), 8).unwrap();
    let (call_tx, calls) = RecvChannel::<Request<String, i64>>::new(h.lua(), 1).unwrap();
    let (reports, mut rx) = SendChannel::<Report>::new(h.lua(), 4).unwrap();
    Daemon {
        events,
        calls,
        reports,
    }
    .htl_preload(&h)
    .unwrap();
    for n in [1, 2, 3] {
        tx.try_send(Event { n }).unwrap();
    }
    drop(tx);
    // The request is answered while the Teal loop runs, from a thread of its own.
    let asker = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let answer = rt.block_on(call_tx.request("hello".to_string()));
        drop(call_tx);
        answer
    });
    run(&h, MAIN).unwrap();
    assert_eq!(asker.join().unwrap().unwrap(), 5);
    assert_eq!(
        rx.try_recv().unwrap(),
        Report {
            text: "sum 6, answered 1".to_string()
        }
    );
    assert!(rx.try_recv().is_err(), "closed after the one report");
}

/// A channel Teal hands back comes in as the same object; a table that is not a channel
/// is refused at the boundary.
#[test]
fn a_channel_handed_back_is_the_same_channel_and_a_plain_table_is_refused() {
    let (_c, h) = program_state();
    let (_tx, events) = RecvChannel::<Event>::new(h.lua(), 1).unwrap();
    let (_tx2, calls) = RecvChannel::<Request<String, i64>>::new(h.lua(), 1).unwrap();
    let (reports, _rx) = SendChannel::<Report>::new(h.lua(), 1).unwrap();
    Daemon {
        events,
        calls,
        reports,
    }
    .htl_preload(&h)
    .unwrap();
    run(
        &h,
        "local d = require('daemon') \
         SAME = d:is_reports(d:reports()) \
         local ok, err = pcall(d.is_reports, d, {}) \
         OK = ok ERR = tostring(err)",
    )
    .unwrap();
    assert!(global::<bool>(&h, "SAME"));
    assert!(!global::<bool>(&h, "OK"));
    assert!(
        global::<String>(&h, "ERR").contains("expected a channel of htl.task"),
        "{}",
        global::<String>(&h, "ERR")
    );
}

/// `RecvChannel::new` before the program required `htl.task`: the library is created for
/// the channel, and the program's `require` gets the same one (the channel is a
/// `task.channel` object a `select` takes).
#[test]
fn a_channel_made_before_the_first_require_shares_the_library() {
    let (_c, h) = program_state();
    let (tx, events) = RecvChannel::<i64>::new(h.lua(), 2).unwrap();
    h.lua().globals().set("events", events).unwrap();
    tx.try_send(41).unwrap();
    run(
        &h,
        "local task = require('htl.task') \
         GOT = task.select({ events:on(function(v) return v + 1 end) })",
    )
    .unwrap();
    assert_eq!(global::<i64>(&h, "GOT"), 42);
}

// ---------------------------------------------------------------- 3. the declaration

/// What the macro wrote (and `include_tl!` checked `main.tl` against above): `htl.task`
/// imported as `task`, each channel typed by its Rust element type.
#[test]
fn the_generated_declaration_imports_htl_task_and_types_the_channels() {
    let decl = Daemon::DECL;
    assert!(
        decl.starts_with("local type task = require(\"htl.task\")\n\nlocal record daemon\n"),
        "{decl}"
    );
    for line in [
        "   events: function(self: daemon): task.RecvChannel<Event>\n",
        "   calls: function(self: daemon): task.RecvChannel<task.Request<string, integer>>\n",
        "   reports: function(self: daemon): task.SendChannel<Report>\n",
        "   is_reports: function(self: daemon, ch: task.SendChannel<Report>): boolean\n",
    ] {
        assert!(decl.contains(line), "{line:?} not in\n{decl}");
    }
    let written = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/task_channels/daemon.d.tl"
    ))
    .unwrap();
    assert_eq!(written, decl);
}

/// The element type misused against the generated declaration, as `include_tl!` would
/// read it: a report sent where an event is expected, and an event read as a report.
#[test]
fn misusing_a_host_channels_element_type_is_a_check_error() {
    let errs = errors_of(
        "misuse",
        "local daemon = require(\"daemon\")\n\
         daemon:reports():send({ n = 1 })\n\
         local ev = daemon:events():recv()\n\
         local s: string = ev.text\n\
         print(s)\n",
    );
    assert_eq!(errs.len(), 2, "{errs:#?}");
    assert!(errs[0].contains("main.tl:2:"), "{errs:#?}");
    assert!(
        errs[1].contains("main.tl:4:") && errs[1].contains("invalid key 'text'"),
        "{errs:#?}"
    );
}

// ---------------------------------------------------------------- 4. await-missing

/// Under `[lang] async`, each wait written without `await` is reported, and the same
/// calls with it are not.
#[test]
fn await_missing_reports_the_waits_written_without_await() {
    let dir = scratch("await-missing");
    write(&dir, "daemon.d.tl", Daemon::DECL);
    let main = write(
        &dir,
        "main.tl",
        "local task = require(\"htl.task\")\n\
         local ch: task.Channel<integer> = task.channel(1)\n\
         ch:send(1)\n\
         local v = ch:recv()\n\
         task.after(1):wait()\n\
         local s = task.select({ ch:on(function(x: integer, _ok: boolean): integer return x end) })\n\
         local i = task.select_raw({ ch:arm_recv() })\n\
         local tk = task.ticker(5)\n\
         tk:recv()\n\
         await ch:send(1)\n\
         local w = await ch:recv()\n\
         await task.after(1):wait()\n\
         local s2 = await task.select({ ch:on(function(x: integer, _ok: boolean): integer return x end) })\n\
         local i2 = await task.select_raw({ ch:arm_recv() })\n\
         await tk:recv()\n\
         local ok = ch:try_send(1)\n\
         print(v, s, i, w, s2, i2, ok, ch:try_recv())\n",
    );
    let ci = checker(&dir, true).check(&main).unwrap();
    assert!(ci.errors.is_empty(), "{:#?}", ci.errors);
    let missing: Vec<&String> = ci
        .lints
        .iter()
        .filter(|l| l.contains("[htl await-missing]"))
        .collect();
    let lines: Vec<&str> = missing
        .iter()
        .map(|l| {
            let at = l.find("main.tl:").unwrap() + "main.tl:".len();
            l[at..].split(':').next().unwrap()
        })
        .collect();
    assert_eq!(lines, ["3", "4", "5", "6", "7", "9"], "{missing:#?}");
    assert!(missing[0].contains("ch:send may suspend"), "{}", missing[0]);
    assert!(missing[1].contains("ch:recv may suspend"), "{}", missing[1]);
    assert!(
        missing[3].contains("task.select may suspend"),
        "{}",
        missing[3]
    );
    assert!(
        missing[4].contains("task.select_raw may suspend"),
        "{}",
        missing[4]
    );
    assert!(
        !ci.lints.iter().any(|l| l.contains("await-non-async")),
        "{:#?}",
        ci.lints
    );
}
