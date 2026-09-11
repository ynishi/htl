//! One reporting path: `gen`, `run`, `build` and the directory form of `build` render a
//! finding the way `check` does, because they now hand it to the same layer.
//!
//! These four used to loop `eprintln!` over the three vectors of a `CheckInfo`, so the
//! same lint in the same file read one way under `htl check` and another under
//! `htl build` — no fix hint, and none of the once-per-run rules the layer holds. The
//! tests below hold the two halves of that: the five commands agree line for line on
//! what a finding looks like, and `htl build` still does not fail on a lint.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-reporting-path", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// stderr and the exit code. Every one of these commands puts its text report on stderr
/// and keeps stdout for what it produces (generated Lua, the program's own output).
fn htl(args: &[&str], cwd: &Path) -> (String, i32) {
    let out = Command::new(common::htl_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap(),
    )
}

/// The diagnostic lines of a report, without the summary each command ends with — that
/// line is the command's own (`htl check: 1 file(s), ...` / `htl build: ... -> app.hb`)
/// and is not what a diagnostic looks like.
fn diagnostics(stderr: &str) -> Vec<String> {
    stderr
        .lines()
        .filter(|l| {
            l.starts_with("error: ") || l.starts_with("warning: ") || l.starts_with("lint: ")
        })
        .map(str::to_string)
        .collect()
}

/// One file with exactly one of each: a warning (a local nothing reads), a lint (a cast
/// to an enum, which nothing checks at run time) and an error (a string where a number
/// was declared).
fn one_of_each(name: &str) -> PathBuf {
    let dir = scratch(name);
    write(
        &dir.join("src/main.tl"),
        "local record M\n   enum State\n      \"open\"\n      \"closed\"\n   end\nend\n\n\
         local function state_of(s: string): M.State\n   return s as M.State\nend\n\n\
         local function stray()\n   local never_read: string = \"x\"\nend\n\n\
         local bad: number = \"not a number\"\n\n\
         return { state_of = state_of, stray = stray, bad = bad }\n",
    );
    dir
}

/// A project whose only finding is a lint, and one that carries a fix — so it is also
/// what says whether the fix hint reached these commands.
fn lints_only(name: &str) -> PathBuf {
    let dir = scratch(name);
    write(
        &dir.join("src/main.tl"),
        "global counter: integer = 0\n\n\
         local function bump(): integer\n   counter = counter + 1\n   return counter\nend\n\n\
         return { bump = bump }\n",
    );
    dir
}

/// The property the refactor exists for: five commands, one rendering. A finding reads
/// the same whichever verb reached it — including the order, which is warnings, then
/// lints, then errors.
#[test]
fn every_command_renders_a_finding_the_same_way() {
    let dir = one_of_each("same");
    let (check, _) = htl(&["check", "src/main.tl", "--no-cache"], &dir);
    let (generated, _) = htl(&["gen", "src/main.tl"], &dir);
    let (run, _) = htl(&["run", "src/main.tl"], &dir);
    let (build, _) = htl(
        &["build", "src/main.tl", "-o", "app.hb", "--no-cache"],
        &dir,
    );
    let (build_dir, _) = htl(&["build", "src", "-o", "dir.hb", "--main", "main"], &dir);

    let want = diagnostics(&check);
    assert_eq!(want.len(), 3, "{check}");
    assert!(want[0].starts_with("warning: "), "{check}");
    assert!(want[1].starts_with("lint: "), "{check}");
    assert!(want[2].starts_with("error: "), "{check}");

    assert_eq!(diagnostics(&generated), want, "gen\n{generated}");
    assert_eq!(diagnostics(&run), want, "run\n{run}");
    assert_eq!(diagnostics(&build), want, "build\n{build}");
    assert_eq!(diagnostics(&build_dir), want, "build <dir>\n{build_dir}");
}

/// The one difference this change makes to what these four print: a finding that carries
/// a fix now says so, because saying it is what the layer does. Before, `htl fix` was
/// discoverable from `htl check` and from nowhere else.
#[test]
fn a_fixable_lint_says_so_on_every_command() {
    let dir = lints_only("fixable");
    let hint = "(fixable: htl fix --unsafe)";
    for args in [
        vec!["check", "src/main.tl", "--no-cache"],
        vec!["gen", "src/main.tl"],
        vec!["run", "src/main.tl"],
        vec!["build", "src/main.tl", "-o", "app.hb", "--no-cache"],
        vec!["build", "src", "-o", "dir.hb", "--main", "main"],
    ] {
        let (stderr, _) = htl(&args, &dir);
        let lints = diagnostics(&stderr);
        assert_eq!(lints.len(), 1, "{args:?}\n{stderr}");
        assert!(lints[0].ends_with(hint), "{args:?}\n{stderr}");
    }
}

/// `htl build` prints lints and does not fail on them — the decision `link.rs` records
/// when it says a lint is not an error there. Moving the renderer must not move that:
/// the bundle is written and the exit code is zero.
#[test]
fn build_exits_zero_when_the_only_findings_are_lints() {
    let dir = lints_only("build-lints");
    let (stderr, code) = htl(
        &["build", "src/main.tl", "-o", "app.hb", "--no-cache"],
        &dir,
    );
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(diagnostics(&stderr).len(), 1, "{stderr}");
    assert!(dir.join("app.hb").is_file(), "{stderr}");

    let (stderr, code) = htl(&["build", "src", "-o", "dir.hb", "--main", "main"], &dir);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(diagnostics(&stderr).len(), 1, "{stderr}");
    assert!(dir.join("dir.hb").is_file(), "{stderr}");
}

/// The other half of the exit codes: an error still fails all four, and `htl build`
/// still refuses to write the bundle.
#[test]
fn an_error_still_fails_every_command() {
    let dir = one_of_each("exit-error");
    for args in [
        vec!["gen", "src/main.tl"],
        vec!["run", "src/main.tl"],
        vec!["build", "src/main.tl", "-o", "app.hb", "--no-cache"],
        vec!["build", "src", "-o", "dir.hb", "--main", "main"],
    ] {
        let (stderr, code) = htl(&args, &dir);
        assert_eq!(code, 1, "{args:?}\n{stderr}");
    }
    assert!(!dir.join("app.hb").exists());
    assert!(!dir.join("dir.hb").exists());
}
