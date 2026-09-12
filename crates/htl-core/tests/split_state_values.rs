//! Values handed to the prelude from a state that is not the prelude's.
//!
//! `Htl::with_checker` makes a fresh Lua for the program and leaves the prelude — and so
//! `h` — in the checker's. A table built from the program state and passed to a function
//! out of the checker's is `Lua instance passed Value created from a different main Lua
//! state`, which is a panic and not an error a caller can handle.
//!
//! `set_deps` was written that way once and `pkg_cli` caught it. `select_lints` and
//! `check_written` had the same shape and no test on a split state, so nothing would have
//! caught them until a caller reached one. These are that test.

use htl_core::Htl;
use htl_core::lint::Selection;
use std::path::Path;

mod common;

/// `d[1]` — a `nil-index` lint, which is `warn` by default and so reported unless the
/// selection turns it off.
const SRC: &str = "local t: {{string:{string}}} = {}\nlocal d = t[\"a\"][1]\nreturn d\n";

fn write(dir: &Path, name: &str, src: &str) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, src).unwrap();
    p
}

/// The selection reaches the checker from a split state, and is the selection that runs.
/// Before the fix this panicked at the first call rather than reporting anything.
#[test]
fn a_selection_set_from_a_split_state_takes_effect() {
    let dir = common::scratch("htl-core-split", "select-lints");
    let file = write(&dir, "m.tl", SRC);

    let checker = Htl::new().unwrap();
    checker.add_path(&dir).unwrap();
    let program = Htl::with_checker(&checker).unwrap();

    program
        .select_lints(&Selection::parse("-nil-index").unwrap())
        .unwrap();
    let off = program.check(&file).unwrap();
    assert!(
        !off.lints.iter().any(|l| l.contains("nil-index")),
        "the rule was turned off from the split state: {:?}",
        off.lints
    );

    program
        .select_lints(&Selection::parse("+nil-index").unwrap())
        .unwrap();
    let on = program.check(&file).unwrap();
    assert!(
        on.lints.iter().any(|l| l.contains("nil-index")),
        "and back on: {:?}",
        on.lints
    );
}

/// A Teal warning kind is silenced by its entry being exactly `false`, not by being absent
/// — so the off names have to cross as well as the on ones. `tl:unused` is the kind a
/// local nobody reads is reported under.
#[test]
fn a_teal_warning_kind_turned_off_stays_off() {
    let dir = common::scratch("htl-core-split", "tl-kind");
    let file = write(
        &dir,
        "m.tl",
        "local function f()\n   local unread = 1\nend\nreturn f\n",
    );

    let h = Htl::new().unwrap();
    h.add_path(&dir).unwrap();

    h.select_lints(&Selection::parse("+tl:unused").unwrap())
        .unwrap();
    let on = h.check(&file).unwrap();
    assert!(
        on.warnings.iter().any(|w| w.contains("unread")),
        "{:?}",
        on.warnings
    );

    h.select_lints(&Selection::parse("-tl:unused").unwrap())
        .unwrap();
    let off = h.check(&file).unwrap();
    assert!(
        !off.warnings.iter().any(|w| w.contains("unread")),
        "an off kind is off, which needs the name to cross rather than be left out: {:?}",
        off.warnings
    );
}

/// `check_written` is the other function that used to build a table in the caller's state.
/// What it answers is not the point here — that it answers at all from a split state is.
#[test]
fn check_written_answers_from_a_split_state() {
    let dir = common::scratch("htl-core-split", "check-written");
    let file = write(&dir, "m.tl", "local x: integer = 1\nreturn x\n");

    let checker = Htl::new().unwrap();
    checker.add_path(&dir).unwrap();
    let program = Htl::with_checker(&checker).unwrap();

    let info = program.check_written(&file).unwrap();
    assert!(info.ok(), "{:?}", info.errors);

    // And it reads what is on disk now, which is what it is for.
    write(&dir, "m.tl", "local x: integer = \"no\"\nreturn x\n");
    let after = program.check_written(&file).unwrap();
    assert!(!after.ok(), "{:?}", after.errors);
}
