//! What `strict` covers, and where it applies.
//!
//! Two things about it were described wrongly for long enough to be carried into a
//! scaffolded `htl.toml`, because nothing held them: `strict` promotes Teal's warnings and
//! not only htl's lints, and it is read by `htl check` alone. The project below has one
//! finding and it is a Teal warning, so a run's verdict is `strict` and nothing else.
//!
//! Both are still what the code does now that a rule has a level, and both assertions are
//! unchanged by it: `strict` promoting every `warn` of the run to `deny` is what "warnings
//! and lints fail the run" always was, and with no rule defaulting to `deny` a project
//! that writes no `[lint.rules]` fails on exactly what it failed on before. What the level
//! model adds — a finding fatal without `strict`, and one advisory beside it — is
//! `lint_levels.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-strict-scope", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// The exit code and stderr: the verdict, and what it was reached on.
fn htl(args: &[&str], cwd: &Path) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_htl"))
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    (
        out.status.code().expect("the run exited, not signalled"),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A module and a test over it, both type-correct and both holding an unused local —
/// a Teal warning, no error and no lint.
fn project(name: &str, htl_toml: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), htl_toml);
    write(
        &root.join("src/greet.tl"),
        "local record greet\nend\n\n\
         function greet.hello(name: string): string\n   local unused_here = 1\n   \
         return \"hello, \" .. name\nend\n\nreturn greet\n",
    );
    write(
        &root.join("tests/greet_test.tl"),
        "local t = require(\"htl.test\")\nlocal greet = require(\"greet\")\n\n\
         local unused_here = 1\n\n\
         t.it(\"greets\", function() \
         t.expect(greet.hello(\"teal\")):to_equal(\"hello, teal\") end)\n",
    );
    root
}

/// The predicate `Report::failed` states, through the binary: a warning is a failure under
/// `strict` and advisory without it, from the flag and from `htl.toml` alike.
#[test]
fn a_teal_warning_fails_the_check_only_under_strict() {
    let root = project("check", "[check]\n");
    let (code, err) = htl(&["check", ".", "--no-cache"], &root);
    assert_eq!(code, 0, "a warning alone is not a failure: {err}");
    assert!(err.contains("unused variable unused_here"), "{err}");
    assert!(err.contains("0 error(s), 2 warning(s), 0 lint(s)"), "{err}");

    let (code, err) = htl(&["check", ".", "--no-cache", "--strict"], &root);
    assert_eq!(code, 1, "the same warning under --strict: {err}");
    assert!(err.contains("[strict]"), "{err}");

    let from_file = project("check-config", "[lint]\nstrict = true\n\n[check]\n");
    let (code, err) = htl(&["check", ".", "--no-cache"], &from_file);
    assert_eq!(
        code, 1,
        "and `strict = true` in htl.toml says the same: {err}"
    );
}

/// `strict` is resolved in `cmd_check` and nowhere else, which is the intent: a test run's
/// verdict is its tests, plus the type errors that stop a file from running at all. The
/// warning is still reported — it is judged by `htl check`, not here.
#[test]
fn htl_test_does_not_read_strict() {
    let root = project("test", "[lint]\nstrict = true\n\n[check]\n");
    let (code, err) = htl(&["test", ".", "--no-cache"], &root);
    assert_eq!(code, 0, "the tests passed, so the run passed: {err}");
    assert!(
        err.contains("unused variable unused_here"),
        "the warning is reported, just not judged: {err}"
    );
    assert!(err.contains("1 passed, 0 failed"), "{err}");
}
