//! A comment-only line of a `.tl` survives `htl gen` at its own line; a trailing comment
//! and a block comment do not.
//!
//! The generator writes from the AST, which keeps no comments, so every comment used to
//! come out as an empty line — the `---` doc above a record and its functions included,
//! which in this repository *is* the doc. The producer now copies each line comment back
//! into the empty line the generator left for it. Nothing is inserted or removed, so the
//! line numbers a run-time error reports are the `.tl`'s, and a module whose comments now
//! reach the Lua runs and tests as it did.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-gen-comment-lines", name)
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
    let root = scratch("doc");
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
    let root = scratch("long-string");
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
    let root = scratch("test");
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
    let root = scratch("run");
    write(
        &root.join("boom.tl"),
        "--- raises on purpose\nlocal function boom(): nil\n   -- a comment above the raise\n\
         \x20  -- and one more\n   error(\"here\")\nend\nboom()\n",
    );
    let (ok, out, err) = htl(&["run", "boom.tl"], &root);
    assert!(!ok, "the script raises:\n{out}{err}");
    assert!(err.contains("boom.tl:5: here"), "reported at line 5: {err}");
}
