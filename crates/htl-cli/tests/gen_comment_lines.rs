//! A comment-only line of a `.tl` survives `htl gen` at its own line, after the last line
//! of code too; a trailing comment and a block comment do not.
//!
//! The generator writes from the AST, which keeps no comments, so every comment used to
//! come out as an empty line — the `---` doc above a record and its functions included,
//! which in this repository *is* the doc. The producer now copies each line comment back
//! into the empty line the generator left for it, and appends the comment lines after the
//! generator's last line at their own numbers. A line that starts inside a long bracket
//! (`--[[`, `[==[`, ...) is not a line comment, whatever it begins with. Nothing before the
//! last line of code is inserted or removed, so the line numbers a run-time error reports
//! are the `.tl`'s, and a module whose comments now reach the Lua runs and tests as it did.

use std::path::Path;
use std::process::Command;

mod common;

fn tempdir(name: &str) -> common::TempDir {
    common::tempdir("htl-cli-gen-comment-lines", name)
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

/// Sixteen lines: a two-line header doc, two doc lines above a function, a comment inside
/// its body, a trailing comment, and a two-line block comment.
const DOC: &str = "\
--- Module header doc, line 1
--- line 2 of the header
local record M
end

--- adds one
--- @param x the number
function M.inc(x: number): number
   -- a comment inside the body
   local y = x + 1 -- trailing comment
   return y
end

--[[ a block
comment ]]
return M
";

/// What `DOC` generates, whole: the five comment-only lines at their numbers, the trailing
/// comment gone from line 10, the block comment's two lines empty.
const DOC_LUA: &str = "\
--- Module header doc, line 1
--- line 2 of the header
local M = {}


--- adds one
--- @param x the number
function M.inc(x)
   -- a comment inside the body
   local y = x + 1
   return y
end



return M
";

#[test]
fn htl_gen_keeps_each_comment_only_line_at_its_line_and_drops_trailing_and_block_comments() {
    let root = tempdir("doc");
    write(&root.join("doc2.tl"), DOC);
    let (ok, out, err) = htl(&["gen", "doc2.tl"], &root);
    assert!(ok, "gen emits a module that checks:\n{out}{err}");
    assert_eq!(out, DOC_LUA);
    assert_eq!(out.lines().count(), 16, "every line keeps its number");
}

/// A `--` line inside a multi-line long string is string content, not a comment: the
/// generator writes those lines verbatim, so the rule has no empty line to copy into.
#[test]
fn a_dash_dash_line_inside_a_long_string_is_left_as_the_string_wrote_it() {
    let root = tempdir("long-string");
    let src = "local s = [[\n-- not a comment\n   -- nor this\n]]\n\n-- a comment\nprint(s)\n";
    write(&root.join("s.tl"), src);
    let (ok, out, err) = htl(&["gen", "s.tl"], &root);
    assert!(ok, "gen emits a module that checks:\n{out}{err}");
    assert_eq!(out, src);
}

/// The module whose comments now reach the Lua is the module a test runs, and it still
/// behaves: `M.inc(1)` is 2.
#[test]
fn a_module_whose_comment_lines_are_kept_still_passes_its_test() {
    let root = tempdir("test");
    write(&root.join("htl.toml"), "[check]\n");
    write(&root.join("src/doc2.tl"), DOC);
    write(
        &root.join("tests/doc2_test.tl"),
        "local t = require(\"htl.test\")\nlocal M = require(\"doc2\")\n\
         t.it(\"adds one\", function() t.expect(M.inc(1)):to_equal(2) end)\n",
    );
    let (ok, out, err) = htl(&["test", "tests"], &root);
    assert!(ok, "the test passes:\n{out}{err}");
    assert!(err.contains("1 passed, 0 failed"), "summary: {err}");
}

/// Copying comment lines back moves nothing: an `error` on line 5, below comment lines, is
/// reported at line 5.
#[test]
fn an_error_below_comment_lines_is_reported_at_its_own_line() {
    let root = tempdir("run");
    write(
        &root.join("boom.tl"),
        "--- raises on purpose\nlocal function boom(): nil\n   -- a comment above the raise\n\
         \x20  -- and one more\n   error(\"here\")\nend\nboom()\n",
    );
    let (ok, out, err) = htl(&["run", "boom.tl"], &root);
    assert!(!ok, "the script raises:\n{out}{err}");
    assert!(err.contains("boom.tl:5: here"), "reported at line 5: {err}");
}

/// Ten lines: a three-line block comment with a `--` line inside it, and a `---` doc line
/// after the module's `return`, the file's last.
const BLOCK_AND_TAIL: &str = "\
local record M
end
--[[ block comment line 1
-- a dash line inside the block
last block line ]]
function M.f(): integer
   return 1
end
return M
--- doc after return, last line of the file
";

/// What `BLOCK_AND_TAIL` generates, whole: ten lines, the block comment's three empty (the
/// `--` line inside it included), the doc after `return M` at line 10.
const BLOCK_AND_TAIL_LUA: &str = "\
local M = {}




function M.f()
   return 1
end
return M
--- doc after return, last line of the file
";

#[test]
fn a_dash_dash_line_inside_a_block_comment_stays_empty_and_the_doc_after_return_is_the_last_line() {
    let root = tempdir("block-and-tail");
    write(&root.join("doc5.tl"), BLOCK_AND_TAIL);
    let (ok, out, err) = htl(&["gen", "doc5.tl"], &root);
    assert!(ok, "gen emits a module that checks:\n{out}{err}");
    assert_eq!(out, BLOCK_AND_TAIL_LUA);
    assert_eq!(out.lines().count(), 10, "every line keeps its number");
}

/// The empty line between `return M` and the doc is kept, so the doc is at its own line;
/// nothing follows the doc.
#[test]
fn a_file_whose_tail_is_doc_after_an_empty_line_ends_with_that_doc() {
    let root = tempdir("tail");
    let src =
        "local record M\nend\nfunction M.f(): integer\n   return 1\nend\nreturn M\n\n--- tail\n";
    write(&root.join("m.tl"), src);
    let (ok, out, err) = htl(&["gen", "m.tl"], &root);
    assert!(ok, "gen emits a module that checks:\n{out}{err}");
    assert_eq!(
        out,
        "local M = {}\n\nfunction M.f()\n   return 1\nend\nreturn M\n\n--- tail\n"
    );
}

/// Empty lines after the last comment line of the file are not appended: the output ends
/// with the doc, not with the empty lines the source had after it.
#[test]
fn empty_lines_after_the_last_trailing_comment_are_dropped() {
    let root = tempdir("tail-blank");
    let src = "local x = 1\nprint(x)\n\n-- tail\n\n\n";
    write(&root.join("t.tl"), src);
    let (ok, out, err) = htl(&["gen", "t.tl"], &root);
    assert!(ok, "gen emits a module that checks:\n{out}{err}");
    assert_eq!(out, "local x = 1\nprint(x)\n\n-- tail\n");
}

/// A levelled long comment closes only at `]==]`: a `]]` inside it closes nothing, and the
/// `--` line inside it stays empty like every other line of the block.
#[test]
fn a_dash_dash_line_inside_a_levelled_long_comment_stays_empty() {
    let root = tempdir("levelled");
    let src =
        "--[==[ levelled\n-- x\n]] still inside\n-- y\n]==]\n-- after\nlocal x = 1\nprint(x)\n";
    write(&root.join("l.tl"), src);
    let (ok, out, err) = htl(&["gen", "l.tl"], &root);
    assert!(ok, "gen emits a module that checks:\n{out}{err}");
    assert_eq!(out, "\n\n\n\n\n-- after\nlocal x = 1\nprint(x)\n");
}

/// A block comment opened and closed on one line leaves the next line a line comment, and
/// a bracket inside a short string opens nothing.
#[test]
fn a_block_comment_closed_on_its_own_line_and_a_bracket_in_a_short_string_open_nothing() {
    let root = tempdir("same-line");
    let src = "local s = \"--[[\" --[[ closed ]] .. \"[[\"\n-- kept\nprint(s)\n";
    write(&root.join("o.tl"), src);
    let (ok, out, err) = htl(&["gen", "o.tl"], &root);
    assert!(ok, "gen emits a module that checks:\n{out}{err}");
    assert_eq!(out.lines().nth(1), Some("-- kept"), "{out}");
}

/// The comment lines after the last line of code are appended at their own numbers, and
/// the lines before are untouched: an `error` on line 12, below doc lines 10–11 and above a
/// trailing doc line, is reported at line 12.
#[test]
fn an_error_between_doc_lines_and_trailing_doc_is_reported_at_its_own_line() {
    let root = tempdir("run-tail");
    write(
        &root.join("boom.tl"),
        "--- a script\nlocal function boom(): nil\n   -- comment\n   print(\"--[[\")\nend\n\n\n\n\
         boom()\n--- doc line 10\n--- doc line 11\nerror(\"here\")\n--- trailing 13\n",
    );
    let (ok, out, err) = htl(&["run", "boom.tl"], &root);
    assert!(!ok, "the script raises:\n{out}{err}");
    assert!(
        err.contains("boom.tl:12: here"),
        "reported at line 12: {err}"
    );
    let (ok, out, err) = htl(&["gen", "boom.tl"], &root);
    assert!(ok, "{out}{err}");
    assert_eq!(out.lines().count(), 13, "{out}");
    assert_eq!(out.lines().last(), Some("--- trailing 13"), "{out}");
}
