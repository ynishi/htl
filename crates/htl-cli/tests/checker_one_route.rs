//! Every command that parses a project's `.tl` builds its checker the way `htl check`
//! does, so a file `htl check` accepts is one `htl gen`, `htl build`, `htl resolve` and
//! `htl run` accept, at the same lint levels.
//!
//! Four commands used to assemble the checker by hand. Each spelled out its own list of
//! install calls, in an order of its own, and none of the lists was `htl check`'s: `htl
//! gen`, `htl build` and `htl resolve` never installed the library behind
//! `require("htl.task")`. That the declaration is written to a directory under the
//! system's temporary directory, and left there, is what hid it: on a machine where
//! `htl check` or `htl run` had already written the file, the three found it on the
//! search path, and on one where neither had run yet they failed on a require the
//! checker was happy with. Now they all go through one function (`checker_for`) that
//! calls the two library functions `htl check` calls, and the cases below hold them to
//! it from outside, one project per case. The first points the binary at an empty
//! temporary directory so that it asks the question a fresh machine asks.

use std::path::Path;
use std::process::Command;

mod common;

fn tempdir(name: &str) -> common::TempDir {
    common::tempdir("htl-cli-checker-one-route", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Run the binary in `cwd`: whether it succeeded, then stdout and stderr. Both are
/// captured apart because `htl gen` prints the Lua on stdout and its diagnostics on
/// stderr, and the assertions ask about the second.
fn htl(args: &[&str], cwd: &Path) -> (bool, String, String) {
    htl_with_tmp(args, cwd, None)
}

/// [`htl`], with the binary's temporary directory moved to `tmp` when one is given: the
/// bundled declarations (`htl/task.d.tl` among them) live under it, and a directory nothing
/// has written to yet is what a command sees on a machine `htl` was just installed on.
fn htl_with_tmp(args: &[&str], cwd: &Path, tmp: Option<&Path>) -> (bool, String, String) {
    let mut cmd = Command::new(common::htl_bin());
    cmd.args(args).current_dir(cwd);
    if let Some(t) = tmp {
        std::fs::create_dir_all(t).unwrap();
        cmd.env("TMPDIR", t);
    }
    let out = cmd.output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A project whose only file needs the types of `htl.task`: nothing beside it provides
/// that name, so it resolves only if the command installed the library the way a check
/// does.
fn task_project(name: &str) -> common::TempDir {
    let root = tempdir(name);
    write(&root.join("htl.toml"), "[check]\n");
    write(
        &root.join("src/main.tl"),
        "local task = require(\"htl.task\")\n\n\
         local function one(): task.Task<string>\n   \
         return task.spawn(function(): string return \"x\" end)\nend\n\n\
         print(one)\n",
    );
    root
}

/// A name `htl check` resolves is one `htl gen`, `htl resolve` and `htl build` resolve —
/// each asked with nothing in the temporary directory, as the first command run on a
/// fresh machine is. Each of the three failed before (`module not found: 'htl.task'`),
/// and passed only when a check or a run had written the declaration first.
#[test]
fn a_require_the_check_resolves_resolves_in_gen_resolve_and_build() {
    let root = task_project("task");
    // Outside the project, so neither the check nor the model sees the declarations a
    // command writes there, and one command's directory is never on another's path.
    let tmp = tempdir("task-tmp");

    let (ok, _, err) = htl_with_tmp(
        &["check", ".", "--no-cache"],
        &root,
        Some(&tmp.join("check")),
    );
    assert!(ok, "the check is the baseline: {err}");

    let (ok, out, err) = htl_with_tmp(&["gen", "src/main.tl"], &root, Some(&tmp.join("gen")));
    assert!(ok, "htl gen: {err}");
    assert!(
        out.contains("htl.task"),
        "the generated Lua requires it: {out}"
    );

    let (ok, out, err) = htl_with_tmp(&["resolve", "htl.task"], &root, Some(&tmp.join("resolve")));
    assert!(ok, "htl resolve: {err}");
    assert!(out.contains("htl.task"), "{out}");

    let (ok, _, err) = htl_with_tmp(
        &["build", "src/main.tl", "-o", "out.hb"],
        &root,
        Some(&tmp.join("build")),
    );
    assert!(ok, "htl build: {err}");
    // The line is printed only when the host has something to provide, so its presence
    // says the linker found the declaration and filed the name under the host's modules.
    assert!(
        err.contains("host must provide") && err.contains("htl.task"),
        "{err}"
    );
}

/// One rule, off in `htl.toml`, stays off in every command that checks.
#[test]
fn a_rule_the_project_turned_off_is_not_reported_by_gen_run_or_build() {
    let source = "local function first(t: {string:{string:string}}): string\n   \
                  return t[\"a\"].b\nend\n\nprint(first)\n";

    // The control: with the rule left at its default every command reports it, so the
    // silence below is the configuration's and not a rule that never fired.
    let on = tempdir("rule-on");
    write(&on.join("htl.toml"), "[check]\n");
    write(&on.join("src/main.tl"), source);
    let (_, _, err) = htl(&["check", ".", "--no-cache"], &on);
    assert!(err.contains("nil-index"), "{err}");
    let (_, _, err) = htl(&["gen", "src/main.tl"], &on);
    assert!(
        err.contains("nil-index"),
        "gen reports it by default: {err}"
    );

    let off = tempdir("rule-off");
    write(
        &off.join("htl.toml"),
        "[lint.rules]\nnil-index = \"allow\"\n",
    );
    write(&off.join("src/main.tl"), source);

    let (ok, _, err) = htl(&["check", ".", "--no-cache"], &off);
    assert!(ok, "{err}");
    assert!(!err.contains("nil-index"), "check: {err}");

    let (ok, _, err) = htl(&["gen", "src/main.tl"], &off);
    assert!(ok, "{err}");
    assert!(!err.contains("nil-index"), "gen: {err}");

    let (ok, _, err) = htl(&["run", "src/main.tl"], &off);
    assert!(ok, "{err}");
    assert!(!err.contains("nil-index"), "run: {err}");

    let (ok, _, err) = htl(&["build", "src/main.tl", "-o", "out.hb"], &off);
    assert!(ok, "{err}");
    assert!(!err.contains("nil-index"), "build: {err}");
}
