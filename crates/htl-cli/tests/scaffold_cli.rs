//! `htl new --embed` / `--host <name>` through the real binary: what the Rust host it
//! writes declares and does, how a host that does not exist or does not fit `--lib` is
//! refused, and the `--format` help of the commands whose text form is a report.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-scaffold", name)
}

fn htl(args: &[&str], cwd: &Path) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_htl"))
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

/// `"0.2"` for a 0.2.x htl, `"1"` for 1.x: what cargo's caret rule reads as the
/// running release and its compatible updates. Computed here a second time so the
/// template cannot drift from the crate version without this failing.
fn expected_dep() -> String {
    let v = env!("CARGO_PKG_VERSION");
    let mut it = v.split('.');
    let major = it.next().unwrap();
    if major == "0" {
        format!("0.{}", it.next().unwrap())
    } else {
        major.to_string()
    }
}

#[test]
fn embed_scaffold_depends_on_the_htl_that_wrote_it() {
    let root = scratch("dep");
    let (ok, _, stderr) = htl(&["new", "sample", "--embed"], &root);
    assert!(ok, "{stderr}");
    let cargo = std::fs::read_to_string(root.join("sample/Cargo.toml")).unwrap();
    let want = format!("htl = \"{}\"", expected_dep());
    assert!(cargo.contains(&want), "want {want} in:\n{cargo}");
}

/// The checker runs inside the proc macros, which the dev profile would otherwise build
/// at `opt-level = 0`: the scaffold says so and sets the override.
#[test]
fn embed_scaffold_optimises_the_proc_macro_build() {
    let root = scratch("opt");
    let (ok, _, stderr) = htl(&["new", "sample", "--embed"], &root);
    assert!(ok, "{stderr}");
    let cargo = std::fs::read_to_string(root.join("sample/Cargo.toml")).unwrap();
    assert!(
        cargo.contains("[profile.dev.build-override]\nopt-level = 3\n"),
        "want the build-override section in:\n{cargo}"
    );
    assert!(
        cargo.contains("# The Teal checker runs inside htl's proc macros"),
        "the section says why:\n{cargo}"
    );
}

#[test]
fn embed_scaffold_fills_arg_before_running_main() {
    let root = scratch("arg");
    let (ok, _, stderr) = htl(&["new", "sample", "--embed"], &root);
    assert!(ok, "{stderr}");
    let main_rs = std::fs::read_to_string(root.join("sample/src/main.rs")).unwrap();
    let main_tl = std::fs::read_to_string(root.join("sample/src/main.tl")).unwrap();
    assert!(
        main_tl.contains("arg[1]"),
        "the script reads arg:\n{main_tl}"
    );
    let set = main_rs
        .find("h.set_arg(\"main.tl\", &args)?")
        .expect("set_arg call");
    let exec = main_rs
        .find("h.exec(MAIN, \"=main.tl\", &args)?")
        .expect("exec call");
    assert!(set < exec, "set_arg comes before exec:\n{main_rs}");
}

/// `--lib` is "no entry script", and the binary exists only to run one: without a script
/// there is nothing for `src/main.rs` to do, so it is not written at all. What the project
/// keeps is the library — the host module, the embedded Teal, and `preload` — which is
/// what another crate embeds it for.
#[test]
fn lib_embed_scaffold_writes_the_library_and_no_binary() {
    let root = scratch("lib");
    let (ok, _, stderr) = htl(&["new", "sample", "--embed", "--lib"], &root);
    assert!(ok, "{stderr}");
    assert!(!root.join("sample/src/main.rs").exists());
    assert!(!root.join("sample/src/main.tl").exists());
    let lib_rs = std::fs::read_to_string(root.join("sample/src/lib.rs")).unwrap();
    assert!(lib_rs.contains("#[host_module"), "{lib_rs}");
    assert!(lib_rs.contains("pub fn preload("), "{lib_rs}");
}

/// The entry script and the binary that runs it are one decision, so a project with a
/// script gets both, and the binary goes through the library rather than around it.
#[test]
fn the_binary_reaches_the_host_through_the_library() {
    let root = scratch("through-lib");
    let (ok, _, stderr) = htl(&["new", "sample", "--embed"], &root);
    assert!(ok, "{stderr}");
    let main_rs = std::fs::read_to_string(root.join("sample/src/main.rs")).unwrap();
    assert!(main_rs.contains("sample::preload(&h)?"), "{main_rs}");
    // The host module itself lives in the library, and only there.
    assert!(!main_rs.contains("#[host_module"), "{main_rs}");
}

/// `--embed` is spelled `--host rust` from #106 on, and the two write the same tree. The
/// snapshot tests pin the tree; this pins that the shorthand still reaches it.
#[test]
fn embed_and_host_rust_write_the_same_thing() {
    let root = scratch("same");
    let (ok, _, stderr) = htl(&["new", "by-flag", "--embed"], &root);
    assert!(ok, "{stderr}");
    let (ok, _, stderr) = htl(&["new", "by-name", "--host", "rust"], &root);
    assert!(ok, "{stderr}");
    for f in ["src/lib.rs", "src/main.rs", "src/main.tl"] {
        let a = std::fs::read_to_string(root.join("by-flag").join(f)).unwrap();
        let b = std::fs::read_to_string(root.join("by-name").join(f)).unwrap();
        // The module identifier differs with the package name; the rest must not.
        assert_eq!(a.replace("by_flag", "M"), b.replace("by_name", "M"), "{f}");
    }
}

/// A typo in `--host` is answered with the names that would have worked. On the command
/// line clap answers it, from the same registry that fills `--help`; `scaffold.rs` keeps
/// its own refusal for callers that do not come through clap. Either way the host is
/// settled before the first write, so a typo leaves no half-written directory behind.
#[test]
fn an_unknown_host_is_refused_before_anything_is_written() {
    let root = scratch("unknown");
    let (ok, _, stderr) = htl(&["new", "sample", "--host", "nope"], &root);
    assert!(!ok, "{stderr}");
    assert!(stderr.contains("rust"), "the registered names:\n{stderr}");
    assert!(!root.join("sample").exists(), "{stderr}");
}

/// `htl init --host rust` on a project that predates the host fills in the Rust side and
/// says which files it left alone, so nothing is skipped in silence.
#[test]
fn init_with_a_host_reports_what_it_kept() {
    let root = scratch("init-host");
    let (ok, _, stderr) = htl(&["new", "sample"], &root);
    assert!(ok, "{stderr}");
    let dir = root.join("sample");
    let (ok, _, stderr) = htl(&["init", "--host", "rust"], &dir);
    assert!(ok, "{stderr}");
    assert!(stderr.contains("created src/lib.rs"), "{stderr}");
    assert!(stderr.contains("created Cargo.toml"), "{stderr}");
    assert!(
        stderr.contains("kept    mlua-pkg.toml (already there)"),
        "{stderr}"
    );
    assert!(dir.join("src/lib.rs").exists());
}

/// Without a host named, `htl init` on a finished project stays the one-liner it was —
/// the kept list is what asking for a host buys, not noise on every re-run.
#[test]
fn init_without_a_host_still_says_nothing_to_do() {
    let root = scratch("init-plain");
    let (ok, _, stderr) = htl(&["new", "sample"], &root);
    assert!(ok, "{stderr}");
    let (ok, _, stderr) = htl(&["init"], &root.join("sample"));
    assert!(ok, "{stderr}");
    assert!(stderr.contains("nothing to do"), "{stderr}");
}

#[test]
fn format_help_does_not_promise_stderr_for_report_commands() {
    let root = scratch("help");
    for cmd in [
        &["bundle", "info", "--help"][..],
        &["cache", "status", "--help"][..],
    ] {
        let (ok, stdout, _) = htl(cmd, &root);
        assert!(ok);
        assert!(
            stdout.contains("Human-readable lines"),
            "{cmd:?}:\n{stdout}"
        );
        assert!(
            !stdout.contains("stderr"),
            "{cmd:?} promises stderr:\n{stdout}"
        );
    }
    // Where the stream matters, the help still sends the reader to the README.
    let (_, stdout, _) = htl(&["check", "--help"], &root);
    assert!(stdout.contains("Machine-readable output"), "{stdout}");
}
