//! `htl new --embed` / `--target <name>` through the real binary: what the Rust host it
//! writes declares and does, how a target that does not exist or does not fit `--lib` is
//! refused, how the flag's old spelling is answered, and the `--format` help of the
//! commands whose text form is a report.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-scaffold", name)
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
        .find("h.exec(MAIN, \"@src/main.tl\", &args)?")
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

/// `--embed` is spelled `--target rust` from #189 on, and the two write the same tree. The
/// snapshot tests pin the tree; this pins that the shorthand still reaches it.
#[test]
fn embed_and_target_rust_write_the_same_thing() {
    let root = scratch("same");
    let (ok, _, stderr) = htl(&["new", "by-flag", "--embed"], &root);
    assert!(ok, "{stderr}");
    let (ok, _, stderr) = htl(&["new", "by-name", "--target", "rust"], &root);
    assert!(ok, "{stderr}");
    for f in ["src/lib.rs", "src/main.rs", "src/main.tl"] {
        let a = std::fs::read_to_string(root.join("by-flag").join(f)).unwrap();
        let b = std::fs::read_to_string(root.join("by-name").join(f)).unwrap();
        // The module identifier differs with the package name; the rest must not.
        assert_eq!(a.replace("by_flag", "M"), b.replace("by_name", "M"), "{f}");
    }
}

/// The C ABI target is a library and only a library: a `cdylib` has no entry point, so
/// `--target ffi` without `--lib` is refused with the flag that fixes it, before the
/// directory exists.
#[test]
fn the_ffi_target_needs_lib_and_says_so_before_writing_anything() {
    let root = scratch("ffi-needs-lib");
    let (ok, _, stderr) = htl(&["new", "sample", "--target", "ffi"], &root);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains("the `ffi` target writes no entry script, so it needs --lib"),
        "{stderr}"
    );
    assert!(!root.join("sample").exists(), "{stderr}");
}

/// What `--target ffi` writes that `--target rust` does not: the two extra crate types the
/// C caller links against, the feature the generated wrappers need, and the attribute
/// that writes the header. The snapshot pins every byte; this says what the bytes are
/// *for*, so a reader of the test knows what would break.
#[test]
fn the_ffi_scaffold_is_a_c_library_with_the_export_attribute() {
    let root = scratch("ffi");
    let (ok, _, stderr) = htl(&["new", "sample", "--lib", "--target", "ffi"], &root);
    assert!(ok, "{stderr}");
    let dir = root.join("sample");

    let cargo = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    assert!(
        cargo.contains("crate-type = [\"rlib\", \"cdylib\", \"staticlib\"]"),
        "{cargo}"
    );
    assert!(cargo.contains("features = [\"ffi\"]"), "{cargo}");

    let lib_rs = std::fs::read_to_string(dir.join("src/lib.rs")).unwrap();
    assert!(
        lib_rs.contains("#[c_export(prefix = \"sample\", header = \"include/sample.h\")]"),
        "{lib_rs}"
    );
    // The other boundary is still here: the scripts get a host module of their own.
    assert!(lib_rs.contains("#[host_module"), "{lib_rs}");

    // A library, so no binary and no entry script — and the callers that load it.
    assert!(!dir.join("src/main.rs").exists());
    assert!(!dir.join("src/main.tl").exists());
    assert!(dir.join("examples/c/main.c").exists());
    assert!(dir.join("examples/c/Makefile").exists());
    assert!(dir.join("examples/python/run.py").exists());

    // The Python caller's one fatal mistake, asserted rather than described: a
    // `c_char_p` return copies the string and loses the pointer, so every call leaks.
    let py = std::fs::read_to_string(dir.join("examples/python/run.py")).unwrap();
    assert!(py.contains("p = ctypes.c_void_p"), "{py}");
    assert!(
        py.contains("lib.sample_greet.argtypes, lib.sample_greet.restype = [p, cs], p"),
        "a `char *` return is declared c_void_p, not c_char_p:\n{py}"
    );
    assert!(py.contains("ctypes.cast(p, ctypes.c_char_p)"), "{py}");
}

/// `--target` is one line of `--help` and the registry fills it, so a target that exists
/// is a target the flag offers. The old spelling is on neither line: it is a hidden
/// argument that exists only to be refused, and a hidden argument that showed up in
/// `--help` would put the collision #189 removed back on the page.
#[test]
fn target_help_lists_every_registered_target_and_never_the_old_flag() {
    let root = scratch("target-help");
    for cmd in [&["new", "--help"][..], &["init", "--help"][..]] {
        let (ok, stdout, _) = htl(cmd, &root);
        assert!(ok);
        assert!(
            stdout.contains("[possible values: rust, ffi]"),
            "{cmd:?}:\n{stdout}"
        );
        assert!(!stdout.contains("--host"), "{cmd:?}:\n{stdout}");
    }
}

/// `--host` on `htl new` / `htl init` was 0.4.0's spelling of this flag and does not work
/// any more, but the refusal names the flag that replaced it rather than leaving the
/// reader to guess. It has to be said explicitly: clap's own suggester scores
/// `jaro("host", "target")` at about 0.47 against a 0.7 threshold, so with the argument
/// simply deleted it would offer nothing — and, because `htl new` takes a positional, it
/// would offer `tip: to pass '--host' as a value, use '-- --host'` instead, which is
/// advice for naming a project `--host`.
#[test]
fn the_old_host_flag_is_refused_and_points_at_target() {
    let root = scratch("renamed");
    for cmd in [
        &["new", "sample", "--host", "rust"][..],
        &["init", "--host", "ffi"][..],
    ] {
        let (ok, _, stderr) = htl(cmd, &root);
        assert!(!ok, "{cmd:?}:\n{stderr}");
        assert!(
            stderr.contains("a similar argument exists: '--target'"),
            "{cmd:?}:\n{stderr}"
        );
        // The other half of the word, so a reader is not left thinking `--host` is gone.
        assert!(
            stderr.contains("on `htl build`, `--host` still names the modules the host provides"),
            "{cmd:?}:\n{stderr}"
        );
        // The tip clap would have printed in its place, and the reason for the argument.
        assert!(!stderr.contains("-- --host"), "{cmd:?}:\n{stderr}");
    }
    assert!(!root.join("sample").exists());
}

/// A typo in `--target` is answered with the names that would have worked. On the command
/// line clap answers it, from the same registry that fills `--help`; `scaffold.rs` keeps
/// its own refusal for callers that do not come through clap. Either way the target is
/// settled before the first write, so a typo leaves no half-written directory behind.
#[test]
fn an_unknown_target_is_refused_before_anything_is_written() {
    let root = scratch("unknown");
    let (ok, _, stderr) = htl(&["new", "sample", "--target", "nope"], &root);
    assert!(!ok, "{stderr}");
    assert!(stderr.contains("rust"), "the registered names:\n{stderr}");
    assert!(stderr.contains("ffi"), "the registered names:\n{stderr}");
    assert!(!root.join("sample").exists(), "{stderr}");
}

/// `htl init --target rust` on a project that predates the Rust side fills it in and
/// says which files it left alone, so nothing is skipped in silence.
#[test]
fn init_with_a_target_reports_what_it_kept() {
    let root = scratch("init-target");
    let (ok, _, stderr) = htl(&["new", "sample"], &root);
    assert!(ok, "{stderr}");
    let dir = root.join("sample");
    let (ok, _, stderr) = htl(&["init", "--target", "rust"], &dir);
    assert!(ok, "{stderr}");
    assert!(stderr.contains("created src/lib.rs"), "{stderr}");
    assert!(stderr.contains("created Cargo.toml"), "{stderr}");
    assert!(
        stderr.contains("kept    mlua-pkg.toml (already there)"),
        "{stderr}"
    );
    assert!(dir.join("src/lib.rs").exists());
}

/// Without a target named, `htl init` on a finished project stays the one-liner it was —
/// the kept list is what asking for a target buys, not noise on every re-run.
#[test]
fn init_without_a_target_still_says_nothing_to_do() {
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
