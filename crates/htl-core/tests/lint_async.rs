//! `await-missing` reads whether a function may suspend from its declaration, not from
//! another `async function` that happens to share the declaration's line.

use htl_core::Htl;
use htl_core::config::LangConfig;
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-async-lint", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn lints_for(dir: &Path, file: &str, rule: &str) -> Vec<String> {
    let h = Htl::new().unwrap();
    h.set_lang(&LangConfig { async_: Some(true) }).unwrap();
    h.configure_lints("+await-missing").unwrap();
    h.add_path(dir).unwrap();
    let types = dir.join("types");
    if types.is_dir() {
        h.add_path(&types).unwrap();
    }
    let ci = h.check(&dir.join(file)).unwrap();
    assert!(
        ci.errors.is_empty(),
        "unexpected type errors: {:?}",
        ci.errors
    );
    ci.lints
        .into_iter()
        .filter(|l| l.contains(&format!("[htl {rule}]")))
        .collect()
}

fn lints_of(dir: &Path, file: &str) -> Vec<String> {
    lints_for(dir, file, "await-missing")
}

/// Issue #430's file: the synchronous callback parameter and the local function value
/// must not inherit `async` from a different function declaration on their line.
#[test]
fn sync_function_types_on_async_declaration_lines_are_not_async() {
    let dir = scratch("issue-430");
    write(
        &dir.join("main.tl"),
        r#"local async function same_line(cond: function(): boolean): boolean
   return cond()
end

local async function next_line(
   cond: function(): boolean
): boolean
   return cond()
end

local async function local_on_line(): boolean
   local cb = function(): boolean return true end
   return cb()
end

local async function on_async_line(): boolean local cb = function(): boolean return true end
   return cb()
end

return { same_line = same_line, next_line = next_line, local_on_line = local_on_line, on_async_line = on_async_line }
"#,
    );

    let lints = lints_of(&dir, "main.tl");
    assert!(
        lints.is_empty(),
        "synchronous calls were reported: {lints:?}"
    );
}

/// The same mistaken classification used to let the suggested `await cond()` pass as
/// though the callback could suspend. It is rejected as a needless await instead.
#[test]
fn awaiting_a_sync_callback_is_reported_as_non_async() {
    let dir = scratch("sync-callback-await");
    write(
        &dir.join("main.tl"),
        "local async function same_line(cond: function(): boolean): boolean\n\
         \x20  return await cond()\n\
         end\n",
    );

    let lints = lints_for(&dir, "main.tl", "await-non-async");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert!(
        lints[0].contains("await on a call of a function that is not async: cond cannot suspend")
    );
}

/// A real async declaration remains detectable when the call shares its line and when
/// it appears later in the file.
#[test]
fn async_function_calls_are_reported_on_and_after_the_declaration_line() {
    let dir = scratch("async-decl-lines");
    write(
        &dir.join("main.tl"),
        "local async function f(): integer return f() end\n\
         local async function later(): integer\n\
         \x20  return f()\n\
         end\n",
    );

    let lints = lints_of(&dir, "main.tl");
    assert_eq!(lints.len(), 2, "{lints:?}");
    assert!(lints[0].contains("main.tl:1:"), "{lints:?}");
    assert!(lints[1].contains("main.tl:3:"), "{lints:?}");
}

/// The value's own `async function` marker wins even when another async declaration is
/// earlier on the same line.
#[test]
fn async_function_value_shares_a_line_with_another_async_declaration() {
    let dir = scratch("async-value-line");
    write(
        &dir.join("main.tl"),
        "local async function first(): integer return 1 end; local value = async function(): integer return 2 end\n\
         local async function caller(): integer return value() end\n",
    );

    let lints = lints_of(&dir, "main.tl");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert!(lints[0].contains("main.tl:2:"), "{lints:?}");
    assert!(lints[0].contains("value may suspend"), "{lints:?}");
}

/// Host declarations keep using `---@async`; their marker is independent of the source
/// column used for Teal `async function` declarations.
#[test]
fn async_marker_in_a_declaration_file_is_preserved() {
    let dir = scratch("async-dts-marker");
    write(
        &dir.join("types/api.d.tl"),
        "local record api\n\
         \x20  get: function(): string ---@async\n\
         end\n\
         return api\n",
    );
    write(
        &dir.join("main.tl"),
        "local api = require(\"api\")\n\
         local async function caller(): string\n\
         \x20  return api.get()\n\
         end\n",
    );

    let lints = lints_of(&dir, "main.tl");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert!(lints[0].contains("main.tl:3:"), "{lints:?}");
    assert!(lints[0].contains("api.get may suspend"), "{lints:?}");
}
