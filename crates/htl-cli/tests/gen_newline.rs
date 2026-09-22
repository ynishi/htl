//! What `htl gen` writes ends with exactly one newline, whatever the input ended with.
//!
//! Nothing looked at `htl gen`'s output before this file existed: the only other
//! invocations capture stderr and filter it to diagnostics. So the one thing the command
//! produces — Lua text — went unasserted, and the newline the CLI appended to it was a
//! local habit rather than a promise. It is a promise now, made by the generator rather
//! than by this command, and these cases are what says so from the outside.
//!
//! A `.tl` whose source deliberately stops at `return M` is the input that separates the
//! two: the generator emits one output line per input line joined with `\n` and writes no
//! terminator, so an unterminated source used to produce an unterminated string, and only
//! `htl gen` hid it.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-gen-newline", name)
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

/// The module both halves of this file generate, given verbatim so the absence of the
/// final newline is visible at the call site rather than hidden in an escape.
const UNTERMINATED: &str = "local record M\n   x: integer\nend\n\nreturn M";

/// Terminated once, not twice — the two failures the change could produce, named apart so
/// a regression says which one happened.
fn assert_one_terminator(what: &str, text: &str) {
    assert!(text.ends_with('\n'), "{what} is terminated: {text:?}");
    assert!(
        !text.ends_with("\n\n"),
        "{what} is terminated once, not twice: {text:?}"
    );
}

#[test]
fn the_file_written_by_o_ends_with_one_newline() {
    let root = scratch("out-file");
    write(&root.join("nonl.tl"), UNTERMINATED);
    let (ok, out, err) = htl(&["gen", "nonl.tl", "-o", "nonl.lua"], &root);
    assert!(ok, "gen emits a module that checks:\n{out}{err}");
    let lua = std::fs::read_to_string(root.join("nonl.lua")).unwrap();
    assert!(lua.contains("return M"), "the module is in there: {lua:?}");
    assert_one_terminator("the file gen wrote", &lua);
}

/// The stdout form goes through the same string and is the half that was never
/// normalized anywhere but in `cmd_gen`.
#[test]
fn the_text_printed_on_stdout_ends_with_one_newline() {
    let root = scratch("stdout");
    write(&root.join("nonl.tl"), UNTERMINATED);
    let (ok, out, err) = htl(&["gen", "nonl.tl"], &root);
    assert!(ok, "gen emits a module that checks:\n{out}{err}");
    assert!(out.contains("return M"), "the module is in there: {out:?}");
    assert_one_terminator("what gen printed", &out);
}

/// The generator joins one output line per input line, so the newline a source ends with
/// is not one of them: two inputs that differ only in their last byte generate the same
/// Lua. This is what makes the terminator the producer's to add rather than something
/// carried through from the input.
#[test]
fn a_terminated_source_and_an_unterminated_one_generate_the_same_bytes() {
    let root = scratch("same-bytes");
    write(&root.join("nonl.tl"), UNTERMINATED);
    write(&root.join("nl.tl"), &format!("{UNTERMINATED}\n"));
    let (ok_a, without, err_a) = htl(&["gen", "nonl.tl"], &root);
    let (ok_b, with, err_b) = htl(&["gen", "nl.tl"], &root);
    assert!(ok_a && ok_b, "both check:\n{err_a}{err_b}");
    assert_eq!(
        without, with,
        "how the source ended does not reach the generated Lua"
    );
    assert_one_terminator("either output", &without);
}
