//! The state's debug hook has one owner (`mlua_isle::runtime::Vm`, attached by every
//! constructor), and `Interrupt`, coverage and a host's own callback register with it.
//! Before #350 each of the three set the hook itself, and the last one to do so was the
//! only one left: coverage removed a host's hook for good, and an `Interrupt` and a host
//! hook replaced each other. The `Interrupt` beside the two is in `c_export.rs`, with
//! the host that has one.

use htl::Htl;
use htl::mlua::{HookTriggers, VmState};
use htl::mlua_isle::runtime::Vm;
use std::cell::Cell;
use std::rc::Rc;

const BUSY: &str = "local x = 0 for i = 1, 1000000 do x = x + 1 end";

/// A program state with a host callback counting every thousandth instruction.
fn counted() -> (Htl, Htl, Rc<Cell<u64>>) {
    let checker = Htl::new().unwrap();
    let h = Htl::with_checker(&checker).unwrap();
    let n = Rc::new(Cell::new(0u64));
    let nn = n.clone();
    h.hook_owner()
        .unwrap()
        .add_hook(
            HookTriggers::new().every_nth_instruction(1000),
            move |_, _| {
                nn.set(nn.get() + 1);
                Ok(VmState::Continue)
            },
        )
        .unwrap();
    (checker, h, n)
}

/// The measurement of #350 as a test: the host's callback fires before coverage, while it
/// records (on the main thread and inside a coroutine the script creates), and after it
/// stopped. Before: 2000, 0, 0, 0.
#[test]
fn a_host_callback_keeps_firing_through_coverage() {
    let (_checker, h, n) = counted();
    let fired = |n: &Rc<Cell<u64>>| n.replace(0);

    h.exec(BUSY, "@busy.lua", &[]).unwrap();
    assert!(fired(&n) > 0, "hook alone");

    h.coverage_start().unwrap();
    h.exec(BUSY, "@busy.lua", &[]).unwrap();
    assert!(fired(&n) > 0, "during coverage, main thread");
    h.exec(
        &format!("coroutine.wrap(function() {BUSY} end)()"),
        "@busy.lua",
        &[],
    )
    .unwrap();
    assert!(
        fired(&n) > 0,
        "during coverage, a coroutine the script creates"
    );
    let cov = h.coverage_stop().unwrap();
    assert!(
        cov.iter().any(|(src, _)| src == "@busy.lua"),
        "coverage recorded the chunk as well: {cov:?}"
    );

    h.exec(BUSY, "@busy.lua", &[]).unwrap();
    assert!(fired(&n) > 0, "after coverage_stop");
}

/// Coverage is a callback on the same global hook, so a line run inside a coroutine the
/// program creates is recorded — the "per thread" limitation of the `debug.sethook` line
/// hook it replaced is gone.
#[test]
fn coverage_sees_a_line_run_inside_a_coroutine() {
    let checker = Htl::new().unwrap();
    let h = Htl::with_checker(&checker).unwrap();
    h.coverage_start().unwrap();
    // Line 1 runs on the main thread, line 3 only inside the coroutine.
    h.exec(
        "local co = coroutine.wrap(function()\n\
         \n\
            local inside = 1\n\
         end)\n\
         co()",
        "@co.lua",
        &[],
    )
    .unwrap();
    let cov = h.coverage_stop().unwrap();
    let (_, lines) = cov
        .iter()
        .find(|(src, _)| src == "@co.lua")
        .expect("the chunk ran");
    assert!(lines.contains(&1), "the main thread's line: {lines:?}");
    assert!(lines.contains(&3), "the coroutine's line: {lines:?}");
}

/// Stopping is a matter of `coverage_stop` and nothing else: a second start discards the
/// first recording, and a stop with nothing running is an empty report.
#[test]
fn coverage_restarts_and_stops_cleanly() {
    let checker = Htl::new().unwrap();
    let h = Htl::with_checker(&checker).unwrap();
    assert!(
        h.coverage_stop().unwrap().is_empty(),
        "nothing was recording"
    );
    h.coverage_start().unwrap();
    h.exec("local a = 1", "@first.lua", &[]).unwrap();
    h.coverage_start().unwrap();
    h.exec("local b = 1", "@second.lua", &[]).unwrap();
    let cov = h.coverage_stop().unwrap();
    assert_eq!(cov, vec![("@second.lua".to_string(), vec![1])]);
    assert!(h.coverage_stop().unwrap().is_empty(), "stopped already");
}

/// The shared state (`Htl::new`, where `htl run` runs a program) is attached when a
/// program runs on it, not at construction: the checker's own work on that state is not
/// under a count hook.
#[test]
fn the_shared_state_is_attached_when_a_program_runs() {
    let h = Htl::new().unwrap();
    assert!(Vm::of(h.lua()).is_none(), "nothing has run yet");
    h.exec("local a = 1", "@first.lua", &[]).unwrap();
    assert!(Vm::of(h.lua()).is_some(), "a program ran");
    h.coverage_start().unwrap();
    h.exec("local a = 1", "@shared.lua", &[]).unwrap();
    assert_eq!(
        h.coverage_stop().unwrap(),
        vec![("@shared.lua".to_string(), vec![1])]
    );
}
