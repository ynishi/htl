//! `std.fs`, end to end: the generated declaration, and a handful of Teal programs run
//! through [`htl::Htl::gen_lua`] + [`htl::Htl::run_blocking`] (the shape `htl test`
//! itself runs a file with), each asserting its own outcome the way `pair.tl` and
//! `async_syntax.rs`'s `PROGRAM` do — a test body here is Teal plus the assertions it
//! makes about what it just did, and the Rust side only checks that the program ran to
//! completion.

use htl::Htl;
use htl::config::LangConfig;
use htl::mlua_isle::runtime::CancelToken;
use htl::teal::HostModule;
use htl_std::Fs;
use std::path::{Path, PathBuf};

mod common;

fn tempdir(name: &str) -> common::TempDir {
    common::tempdir("htl-std", name)
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, text).unwrap();
    p
}

/// `std.fs` installed and on the checker's path: [`htl_std::install`] wires
/// `require("std.fs")` at run time, and `add_path` points the checker at the crate's own
/// `dts/` (this crate's `dts/std/fs.d.tl` under it answers to `std.fs`, the same
/// `dir/<a>/<b>.d.tl` -> `a.b` convention `Htl::add_path`'s own doc states and
/// `host_dotted_name.rs` exercises directly) — the wiring a project's model does through
/// `[package.metadata.htl] dts` when one is in play, done by hand here because there is
/// none.
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
    let h = host();
    let main = write(dir, "main.tl", tl_src);
    let (code, ci) = h.gen_lua(&main).unwrap();
    assert!(ci.errors.is_empty(), "{:?}", ci.errors);
    h.run_blocking(
        &code.expect("generated"),
        "@main.tl",
        &[],
        &CancelToken::new(),
    )
    .unwrap();
}

#[test]
fn write_then_read_round_trips_the_text() {
    let dir = tempdir("round-trip");
    let file = dir.join("round.txt").display().to_string();
    run(
        &dir,
        &format!(
            "local fs = require(\"std.fs\")\n\
             local path = \"{file}\"\n\
             await fs.write(path, \"hello std.fs\")\n\
             local got = await fs.read(path)\n\
             assert(got == \"hello std.fs\", got)\n"
        ),
    );
}

/// `fs.read` of a path that was never written raises, and the message a script's own
/// `pcall` sees names both the function and the path — not a traceback, since nothing
/// here asks for `errors = \"return\"` either (see the crate doc's "Errors"). The
/// protected call is wrapped in `async function(): string .. end` rather than a plain
/// one: Lua 5.4's `pcall` is itself yieldable, but a bare `function()` is not an async
/// context, and `await` inside one is `await-outside-async` at the checker.
#[test]
fn read_of_a_missing_file_raises_and_pcall_sees_the_path() {
    let dir = tempdir("missing");
    let missing_path = dir.join("missing.txt");
    let missing = missing_path.display().to_string();
    // The OS message `std.fs.read`'s own error has to carry, read the same way
    // `tokio::fs::read_to_string` gets it underneath — computed here rather than typed
    // as a literal so the assertion holds whatever OS the gate runs on.
    let cause = std::fs::read_to_string(&missing_path)
        .unwrap_err()
        .to_string();
    let cause_lua = cause.replace('\\', "\\\\").replace('"', "\\\"");
    run(
        &dir,
        &format!(
            "local fs = require(\"std.fs\")\n\
             local path = \"{missing}\"\n\
             local cause = \"{cause_lua}\"\n\
             local ok, err = pcall(async function(): string return await fs.read(path) end)\n\
             assert(not ok, \"expected read of a missing file to raise\")\n\
             local msg = tostring(err)\n\
             assert(string.find(msg, \"std.fs.read\", 1, true) ~= nil, msg)\n\
             assert(string.find(msg, path, 1, true) ~= nil, msg)\n\
             assert(string.find(msg, cause, 1, true) ~= nil, msg)\n"
        ),
    );
}

#[test]
fn exists_is_file_and_is_dir_answer_without_raising() {
    let dir = tempdir("exists");
    let file = dir.join("present.txt").display().to_string();
    let subdir = dir.display().to_string();
    let missing = dir.join("nope.txt").display().to_string();
    run(
        &dir,
        &format!(
            "local fs = require(\"std.fs\")\n\
             local file = \"{file}\"\n\
             local dir = \"{subdir}\"\n\
             local missing = \"{missing}\"\n\
             await fs.write(file, \"x\")\n\
             assert(await fs.exists(file) == true, \"file should exist\")\n\
             assert(await fs.is_file(file) == true, \"file should be a file\")\n\
             assert(await fs.is_dir(file) == false, \"file is not a directory\")\n\
             assert(await fs.exists(dir) == true, \"dir should exist\")\n\
             assert(await fs.is_dir(dir) == true, \"dir should be a directory\")\n\
             assert(await fs.is_file(dir) == false, \"dir is not a file\")\n\
             assert(await fs.exists(missing) == false, \"missing should not exist\")\n\
             assert(await fs.is_file(missing) == false, \"missing is not a file\")\n\
             assert(await fs.is_dir(missing) == false, \"missing is not a directory\")\n"
        ),
    );
}

/// `walk` below a tree `mkdir` created with parents, every file sorted by its full path
/// regardless of the order the directory happens to hold them in on disk.
#[test]
fn mkdir_creates_parents_and_walk_lists_files_sorted() {
    let dir = tempdir("walk");
    let root = dir.join("tree").display().to_string();
    run(
        &dir,
        &format!(
            "local fs = require(\"std.fs\")\n\
             local root = \"{root}\"\n\
             await fs.mkdir(root .. \"/a/b\")\n\
             await fs.write(root .. \"/zero.txt\", \"0\")\n\
             await fs.write(root .. \"/a/one.txt\", \"1\")\n\
             await fs.write(root .. \"/a/b/two.txt\", \"2\")\n\
             local files = await fs.walk(root)\n\
             assert(#files == 3, tostring(#files))\n\
             assert(files[1] == root .. \"/a/b/two.txt\", files[1])\n\
             assert(files[2] == root .. \"/a/one.txt\", files[2])\n\
             assert(files[3] == root .. \"/zero.txt\", files[3])\n"
        ),
    );
}

/// `walk` of a path that is a file, not a directory, raises rather than walking it:
/// `walkdir` would otherwise yield exactly that one entry and `walk` would answer with a
/// one-element list holding `path` itself, which reads as a directory holding one file
/// rather than "wrong kind of path".
#[test]
fn walk_of_a_file_raises_not_a_directory() {
    let dir = tempdir("walk-file");
    let file = dir.join("not-a-dir.txt").display().to_string();
    run(
        &dir,
        &format!(
            "local fs = require(\"std.fs\")\n\
             local path = \"{file}\"\n\
             await fs.write(path, \"x\")\n\
             local ok, err = pcall(async function(): {{string}} return await fs.walk(path) end)\n\
             assert(not ok, \"expected walk of a file to raise\")\n\
             local msg = tostring(err)\n\
             assert(string.find(msg, \"std.fs.walk\", 1, true) ~= nil, msg)\n\
             assert(string.find(msg, path, 1, true) ~= nil, msg)\n\
             assert(string.find(msg, \"not a directory\", 1, true) ~= nil, msg)\n"
        ),
    );
}

#[test]
fn remove_deletes_a_directory_recursively() {
    let dir = tempdir("remove");
    let victim = dir.join("victim").display().to_string();
    run(
        &dir,
        &format!(
            "local fs = require(\"std.fs\")\n\
             local victim = \"{victim}\"\n\
             await fs.mkdir(victim .. \"/inner\")\n\
             await fs.write(victim .. \"/inner/f.txt\", \"x\")\n\
             assert(await fs.is_dir(victim) == true, \"victim should exist first\")\n\
             await fs.remove(victim)\n\
             assert(await fs.exists(victim) == false, \"victim should be gone\")\n"
        ),
    );
}

/// Bytes outside UTF-8 (`\0`, `\255`, `\254`) round-trip through `write_binary` /
/// `read_binary` exactly, which `write` / `read` (Rust `String`, UTF-8 only) cannot
/// promise.
#[test]
fn read_binary_and_write_binary_round_trip_non_utf8_bytes() {
    let dir = tempdir("binary");
    let file = dir.join("blob.bin").display().to_string();
    run(
        &dir,
        &format!(
            "local fs = require(\"std.fs\")\n\
             local path = \"{file}\"\n\
             local data = \"\\0\\1\\255\\254abc\"\n\
             await fs.write_binary(path, data)\n\
             local got = await fs.read_binary(path)\n\
             assert(#got == #data, tostring(#got) .. \" ~= \" .. tostring(#data))\n\
             assert(got == data, \"round trip changed the bytes\")\n"
        ),
    );
}

/// The declaration the macro wrote is the one tracked at `dts/std/fs.d.tl` (checked
/// against, not only regenerated by, the build that just ran — the same thing
/// `host_dotted_name.rs`'s own `add_path_reads_the_fixture_the_macro_wrote_and_checks_against_it`
/// does for its fixture), `MODULE` is the whole dotted `require` key, and every function
/// line — there is nothing else in this module — carries the `---@async` marker, since
/// every one of [`Fs`]'s methods is an `async fn`.
#[test]
fn the_generated_declaration_is_tracked_and_every_function_is_async() {
    assert_eq!(Fs::MODULE, "std.fs");
    let decl = Fs::DECL;
    let tracked =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/dts/std/fs.d.tl")).unwrap();
    assert_eq!(tracked, decl, "the tracked file is stale against the build");

    let fn_lines: Vec<&str> = decl.lines().filter(|l| l.contains(": function(")).collect();
    assert_eq!(fn_lines.len(), 10, "{decl}");
    for l in &fn_lines {
        assert!(l.trim_end().ends_with("---@async"), "{l}");
    }
}

/// `Htl::check` resolves `require("std.fs")` through the `add_path` [`host`] adds, types
/// `await fs.read(p)` clean, and reports `await-missing` on the same call with `await`
/// taken out — the two-line proof that the declaration [`Fs`] produces drives the async
/// checker the way a hand-written `.d.tl` does (`async_lints.rs`'s own
/// `each_rule_reports_its_case_at_the_line_and_column` is the same shape for `http.get`).
#[test]
fn checking_requires_await_and_reports_await_missing_without_it() {
    let dir = tempdir("check");
    let h = host();

    let with_await = write(
        &dir,
        "with_await.tl",
        "local fs = require(\"std.fs\")\n\
         local s = await fs.read(\"/tmp/x\")\n\
         print(s)\n",
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
        "local fs = require(\"std.fs\")\n\
         local s = fs.read(\"/tmp/x\")\n\
         print(s)\n",
    );
    let ci = h.check(&without_await).unwrap();
    assert!(ci.errors.is_empty(), "{:?}", ci.errors);
    let missing: Vec<&String> = ci
        .lints
        .iter()
        .filter(|l| l.contains("[htl await-missing]"))
        .collect();
    assert_eq!(missing.len(), 1, "{:?}", ci.lints);
    // Column 11: "local s = " is 10 characters, so `fs` (the callee's first token)
    // starts at the 11th, the way `async_lints.rs`'s own
    // `each_rule_reports_its_case_at_the_line_and_column` pins the column for `http.get`.
    assert!(
        missing[0].contains(
            "without_await.tl:2:11: call of an async function without await: fs.read may suspend"
        ),
        "{}",
        missing[0]
    );
}
