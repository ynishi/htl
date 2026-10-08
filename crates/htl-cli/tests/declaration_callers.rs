//! What a `require` of a name only a `.d.tl` declares gets under `htl run` / `htl test`,
//! by the language of the chunk that asked (#402).
//!
//! A `require` written in Teal gets a table that explains itself when indexed: the checker
//! typed the file against the declaration, and a module that calls its host in one function
//! has to load, host or not, for the rest of it to run and be tested. A `require` from plain
//! Lua gets what a `Registry` gives every caller: the `require` fails naming the
//! declaration, so `pcall(require, name)` — Lua's one probe for an optional dependency — is
//! `false`. The cases below are the two sides of that line and the two scaffolds that stand
//! on the Teal side of it.
//!
//! Every project is written by `htl new` into a temp directory, so what is under test is
//! the binary a user runs on a project it wrote.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

/// Run the binary in `cwd`; whether it succeeded, and stdout then stderr in one string.
fn run(args: &[&str], cwd: &Path) -> (bool, String) {
    let out = Command::new(common::htl_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// `htl new <name> <flags>` in a fresh temp directory; the project's root, with the
/// `[patch]` the packaged gate asks for ([`common::write_patch_config`]) when it asks.
/// Returns the temp directory's own guard alongside the project path: `project` is a
/// subdirectory of it, not the directory itself, so a caller that dropped the guard early
/// would see the project it just wrote vanish before the first command ran against it.
fn new_project(test: &str, name: &str, flags: &[&str]) -> (common::TempDir, PathBuf) {
    let root = common::tempdir("htl-cli-decl-callers", test);
    let mut args = vec!["new", name];
    args.extend_from_slice(flags);
    let (ok, text) = run(&args, &root);
    assert!(ok, "htl new {flags:?}:\n{text}");
    let project = root.join(name);
    common::write_patch_config(&project);
    (root, project)
}

/// A library project with the Problem's layout from #402: `knl_types` declared in
/// `types/` and implemented nowhere — the host provides it — and the plain Lua modules that
/// ask for it, each typed by a declaration of its own so a Teal test can require them.
fn lua_callers(test: &str) -> (common::TempDir, PathBuf) {
    let (dir, p) = new_project(test, "app", &["--lib"]);
    write(
        &p.join("types/knl_types.d.tl"),
        "local record knl_types\n   SessionId: string\nend\nreturn knl_types\n",
    );
    // The idiom every Lua library uses for an optional dependency.
    write(
        &p.join("src/opt.lua"),
        "local ok, m = pcall(require, \"knl_types\")\n\
         local M = { present = ok }\n\
         if ok then\n\
         \x20  local ok2, err = pcall(function() return m.SessionId end)\n\
         \x20  M.first_index_ok, M.first_index_err = ok2, err\n\
         end\n\
         return M\n",
    );
    write(
        &p.join("types/opt.d.tl"),
        "local record opt\n   present: boolean\n   first_index_ok: boolean\n   \
         first_index_err: string\nend\nreturn opt\n",
    );
    write(
        &p.join("src/direct.lua"),
        "local m = require(\"knl_types\")\nreturn { m = m }\n",
    );
    write(
        &p.join("types/direct.d.tl"),
        "local record direct\n   m: any\nend\nreturn direct\n",
    );
    (dir, p)
}

/// `pcall(require, name)` in a `.lua` is `false` for a name only a declaration answers,
/// under `htl test` as in a host: the module takes its fallback branch instead of the
/// "present" one whose first use raised.
#[test]
fn a_pcall_require_from_plain_lua_does_not_find_a_declared_only_module() {
    let (_dir, p) = lua_callers("pcall");
    write(
        &p.join("tests/opt_lua_test.tl"),
        "local t = require(\"htl.test\")\n\
         local opt = require(\"opt\")\n\
         t.describe(\"a tolerant require from plain Lua\", function()\n\
         \x20  t.it(\"sees no module\", function()\n\
         \x20     t.expect(opt.present):to_equal(false)\n\
         \x20     t.expect(opt.first_index_err):to_equal(nil)\n\
         \x20  end)\n\
         end)\n",
    );
    let (ok, text) = run(&["test", "."], &p);
    assert!(ok, "htl test:\n{text}");
    assert!(text.contains("ok   tests/opt_lua_test.tl"), "{text}");
    assert!(!text.contains("declaration-only here"), "{text}");
}

/// A plain `require` from Lua fails at the `require`, naming the declaration and the two
/// ways out — the `Registry`'s text — rather than handing back a table that fails later.
#[test]
fn a_require_from_plain_lua_fails_at_the_require_naming_the_declaration() {
    let (_dir, p) = lua_callers("direct");
    write(
        &p.join("tests/direct_test.tl"),
        "local t = require(\"htl.test\")\n\
         local d = require(\"direct\")\n\
         t.describe(\"direct\", function()\n\
         \x20  t.it(\"loaded\", function() t.expect(d.m ~= nil):to_equal(true) end)\n\
         end)\n",
    );
    let (ok, text) = run(&["test", "tests/direct_test.tl"], &p);
    assert!(
        !ok,
        "a require of a declared-only module from Lua passed:\n{text}"
    );
    assert!(
        text.contains("src/direct.lua:1:"),
        "not at the require:\n{text}"
    );
    assert!(text.contains("module 'knl_types' not found"), "{text}");
    assert!(text.contains("knl_types.d.tl"), "{text}");
    assert!(text.contains("nothing implements it"), "{text}");
    assert!(
        text.contains("local type knl_types = require(\"knl_types\")"),
        "{text}"
    );
    assert!(!text.contains("declaration-only here"), "{text}");
}

/// The split is by the language of the chunk, not by `pcall`: the same probe written in
/// Teal still gets the stand-in, whose first index names what is missing.
#[test]
fn a_pcall_require_from_teal_still_gets_the_stand_in() {
    let (_dir, p) = lua_callers("teal-pcall");
    write(
        &p.join("tests/teal_pcall_test.tl"),
        "local t = require(\"htl.test\")\n\
         t.describe(\"a tolerant require from Teal\", function()\n\
         \x20  t.it(\"gets the stand-in\", function()\n\
         \x20     local ok, m = pcall(require, \"knl_types\")\n\
         \x20     t.expect(ok):to_equal(true)\n\
         \x20     local ok2, err = pcall(function(): any return (m as {string:any}).SessionId end)\n\
         \x20     t.expect(ok2):to_equal(false)\n\
         \x20     t.expect(tostring(err):find(\"declaration-only here\", 1, true) ~= nil):to_equal(true)\n\
         \x20  end)\n\
         end)\n",
    );
    let (ok, text) = run(&["test", "tests/teal_pcall_test.tl"], &p);
    assert!(ok, "htl test:\n{text}");
    assert!(text.contains("1 passed, 0 failed"), "{text}");
}

/// The order of two files does not change what the `.lua` sees (#408): a Teal test that
/// requires the declared name as a value first, and only then the Lua module that probes it,
/// still leaves the probe `false`. The Teal file gets the same stand-in on both of its
/// `require`s, and the stand-in is not in `package.loaded` raw, where `require` would have
/// answered the `.lua` with it.
#[test]
fn a_pcall_require_from_plain_lua_after_a_teal_require_still_does_not_find_the_module() {
    let (_dir, p) = lua_callers("teal-first");
    write(
        &p.join("tests/order_test.tl"),
        "local t = require(\"htl.test\")\n\
         local k = require(\"knl_types\")\n\
         local opt = require(\"opt\")\n\
         t.describe(\"a Teal value require, then plain Lua\", function()\n\
         \x20  t.it(\"leaves the Lua probe false\", function()\n\
         \x20     t.expect(opt.present):to_equal(false)\n\
         \x20  end)\n\
         \x20  t.it(\"gives Teal the same table twice\", function()\n\
         \x20     t.expect(type(k)):to_equal(\"table\")\n\
         \x20     t.expect(rawequal(require(\"knl_types\"), k)):to_equal(true)\n\
         \x20  end)\n\
         \x20  t.it(\"keeps the stand-in out of package.loaded\", function()\n\
         \x20     t.expect(rawget(package.loaded as {string:any}, \"knl_types\") == nil):to_equal(true)\n\
         \x20  end)\n\
         end)\n",
    );
    let (ok, text) = run(&["test", "tests/order_test.tl"], &p);
    assert!(ok, "htl test:\n{text}");
    assert!(text.contains("3 passed, 0 failed"), "{text}");
}

/// The window scaffold's engine requires `mq` as a value and never calls it outside
/// `render`, and its test runs under `htl test` with no window: that `require` is Teal's,
/// so it still resolves to the declaration.
#[test]
fn the_window_scaffolds_engine_test_still_passes_without_a_window() {
    let (_dir, p) = new_project("window", "win", &["--target", "window"]);
    let (ok, text) = run(&["check", "."], &p);
    assert!(ok, "htl check:\n{text}");
    assert!(p.join("types/htl-mq/mq.d.tl").is_file(), "{text}");
    let (ok, text) = run(&["test", "."], &p);
    assert!(ok, "htl test:\n{text}");
    assert!(
        text.contains("htl test: 1 file(s), 2 passed, 0 failed, 0 file(s) with errors"),
        "{text}"
    );
}

/// The embed scaffold's entry requires its host module and calls it: run without the host,
/// it fails at the call — after the line before it printed — not at the `require`.
#[test]
fn a_teal_entry_still_fails_at_the_call_to_its_host_not_the_require() {
    let (_dir, p) = new_project("embed", "emb", &["--embed"]);
    let (ok, text) = run(&["run", "src/main.tl"], &p);
    assert!(!ok, "htl run without the host passed:\n{text}");
    assert!(text.contains("hello, teal"), "{text}");
    assert!(
        text.contains("runtime error: src/main.tl:7: module 'host' is declaration-only here ("),
        "{text}"
    );
    assert!(
        text.contains("src/host.d.tl): 'greet' has no implementation on this path."),
        "{text}"
    );
}
