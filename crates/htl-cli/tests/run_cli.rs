//! `htl run` on the executor: a program is a root coroutine of mlua-isle's `Vm::run`
//! rather than a plain call, so that a host's `async fn` can be called from it. Two
//! things a user of the command sees have to stay as they were — a program that finishes
//! prints what it printed and exits 0, one that raises prints the frames `traceback_cli`
//! pins — and one is new: Ctrl-C cancels the program, and the command says so.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-run", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn htl(args: &[&str], cwd: &Path) -> (std::process::ExitStatus, String, String) {
    let out = Command::new(common::htl_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    (
        out.status,
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A program that finishes: its stdout, in order, and exit 0. Coroutines the program
/// creates itself, and `arg`, work as they did under the plain call.
#[test]
fn a_program_that_finishes_prints_what_it_printed_and_exits_zero() {
    let root = scratch("finishes");
    write(
        &root.join("main.tl"),
        "print(\"one\")\n\
         local co = coroutine.wrap(function(): integer coroutine.yield(2) return 3 end)\n\
         print(\"two\", co())\n\
         print(\"three\", co())\n\
         print(\"args\", #arg, arg[1])\n",
    );
    let (status, out, err) = htl(&["run", "main.tl", "x"], &root);
    assert!(status.success(), "{err}");
    assert_eq!(out, "one\ntwo\t2\nthree\t3\nargs\t1\tx\n");
    assert_eq!(err, "");
}

/// A program that returns `os.exit` codes and a raising one keep their exit status: 1 for
/// a raise, with the error on stderr and nothing about the executor in it.
#[test]
fn a_program_that_raises_exits_one_with_the_error_and_no_executor_frames() {
    let root = scratch("raises");
    write(
        &root.join("main.tl"),
        "local function inner() error(\"boom\") end\ninner()\n",
    );
    let (status, _, err) = htl(&["run", "main.tl"], &root);
    assert_eq!(status.code(), Some(1));
    assert!(err.starts_with("runtime error: main.tl:1: boom"), "{err}");
    assert!(err.contains("main.tl:2: in main chunk"), "{err}");
    assert!(
        !err.contains("xpcall") && !err.contains("mlua_isle"),
        "{err}"
    );
}

/// Ctrl-C (SIGINT) cancels the program: the cancel reaches a CPU loop from the hook, the
/// command prints `htl run: interrupted` and exits 130. That the program's own `<close>`
/// handlers run first, under the grace, is the executor test in the `htl` crate.
#[cfg(unix)]
#[test]
fn ctrl_c_cancels_the_program_and_exits_130() {
    let root = scratch("ctrl-c");
    write(
        &root.join("main.tl"),
        "print(\"started\")\n\
         io.stdout:flush()\n\
         local x = 0\n\
         while true do x = x + 1 end\n",
    );
    let child = Command::new(common::htl_bin())
        .args(["run", "main.tl"])
        .current_dir(&root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Let the check finish and the loop start before the signal, or the signal lands on
    // a process that has not installed its handler yet and takes the default (exit 2).
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let killed = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(130),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert_eq!(stderr.trim_end(), "htl run: interrupted");
    assert_eq!(stdout, "started\n");
}
