//! `htl new --embed` / `--target <name>` / `--htl <req>` through the real binary: what the
//! Rust host it writes declares and does, what each pin puts in the manifest, how a target
//! or a release that does not exist — or a target that does not fit `--lib` — is refused,
//! what `htl build` does with the `[build] target` the scaffold recorded, and the
//! `--format` help of the commands whose text form is a report.

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

/// The `htl` line of a project's manifest, so that a failure prints the line rather than
/// the file.
fn htl_line(manifest: &Path) -> String {
    let cargo = std::fs::read_to_string(manifest).unwrap();
    cargo
        .lines()
        .find(|l| l.starts_with("htl = "))
        .unwrap_or_else(|| panic!("no htl dependency in:\n{cargo}"))
        .to_string()
}

/// With no `--htl`, a project pins the release the scaffold defaults to — a number written
/// down in `scaffold.rs`, not this crate's version. The two are equal today and the reason
/// they are separate is that they move at different moments: this one moves when a release
/// that understands what the scaffold writes is on crates.io.
#[test]
fn new_pins_the_default_release() {
    let root = scratch("dep");
    let (ok, _, stderr) = htl(&["new", "a", "--target", "bin"], &root);
    assert!(ok, "{stderr}");
    assert_eq!(htl_line(&root.join("a/Cargo.toml")), "htl = \"0.4\"");
}

/// `--htl main` is the dogfood pin: the project builds against the repository rather than
/// against anything published, and there is no version key left for cargo to reconcile
/// with the branch.
#[test]
fn new_htl_main_writes_a_git_pin() {
    let root = scratch("main-pin");
    let (ok, _, stderr) = htl(&["new", "b", "--target", "bin", "--htl", "main"], &root);
    assert!(ok, "{stderr}");
    let line = htl_line(&root.join("b/Cargo.toml"));
    assert!(line.contains("git = "), "{line}");
    assert!(line.contains("branch = \"main\""), "{line}");
    assert!(!line.contains("version ="), "{line}");
}

/// `--htl path:<dir>` names the checkout's *root*, and the dependency it writes is the
/// `crates/htl` inside it — so what the user types is the directory they cloned, not a
/// path into its layout.
#[test]
fn new_htl_path_writes_a_path_pin() {
    let root = scratch("path-pin");
    let (ok, _, stderr) = htl(
        &["new", "c", "--target", "bin", "--htl", "path:../co"],
        &root,
    );
    assert!(ok, "{stderr}");
    assert_eq!(
        htl_line(&root.join("c/Cargo.toml")),
        "htl = { path = \"../co/crates/htl\" }"
    );
}

/// A release the scaffold has no opinion about is refused with the ones it has, and — like
/// an unknown target — before the directory exists, so a typo leaves nothing behind.
#[test]
fn an_unsupported_htl_is_refused_before_writing() {
    let root = scratch("bad-pin");
    let (ok, _, stderr) = htl(&["new", "d", "--htl", "0.3"], &root);
    assert!(!ok, "{stderr}");
    assert!(stderr.contains("unsupported htl `0.3`"), "{stderr}");
    assert!(stderr.contains("0.4"), "the supported set:\n{stderr}");
    assert!(!root.join("d").exists(), "{stderr}");
}

/// The C ABI target needs `features = ["ffi"]` on its `htl`, and the pin decides the rest
/// of that line — so the two are assembled in one place rather than once per pin kind.
/// This is the case that would break first if they were not.
#[test]
fn the_cdylib_pin_keeps_its_features_under_every_pin_kind() {
    let root = scratch("cdylib-pin");
    let (ok, _, stderr) = htl(
        &["new", "e", "--lib", "--target", "cdylib", "--htl", "main"],
        &root,
    );
    assert!(ok, "{stderr}");
    let line = htl_line(&root.join("e/Cargo.toml"));
    assert!(line.contains("branch = \"main\""), "{line}");
    assert!(line.contains("features = [\"ffi\"]"), "{line}");
}

/// `[build] target` is written only when the pinned htl can read it. Under the default
/// release the key does not exist yet, so the project that has a target gets the same
/// `htl.toml` as one that has none; under `main` or a checkout it is recorded. The `main`
/// case also runs `htl check` on the project it wrote, which is the assertion that matters:
/// the file this scaffold produced is one an htl that carries the key accepts.
#[test]
fn new_records_the_target_when_the_pin_reads_it() {
    let root = scratch("build-target");
    let config = |name: &str| std::fs::read_to_string(root.join(name).join("htl.toml")).unwrap();

    let (ok, _, stderr) = htl(&["new", "a", "--target", "bin"], &root);
    assert!(ok, "{stderr}");
    assert!(!config("a").contains("target ="), "{}", config("a"));

    let (ok, _, stderr) = htl(&["new", "b", "--target", "bin", "--htl", "main"], &root);
    assert!(ok, "{stderr}");
    assert!(config("b").contains("target = \"bin\""), "{}", config("b"));

    let (ok, _, stderr) = htl(
        &[
            "new",
            "c",
            "--lib",
            "--target",
            "cdylib",
            "--htl",
            "path:../co",
        ],
        &root,
    );
    assert!(ok, "{stderr}");
    assert!(
        config("c").contains("target = \"cdylib\""),
        "{}",
        config("c")
    );

    // No target, so nothing to record however new the pin is.
    let (ok, _, stderr) = htl(&["new", "d", "--htl", "main"], &root);
    assert!(ok, "{stderr}");
    assert!(!config("d").contains("target ="), "{}", config("d"));

    // This binary's own config reads the key it just wrote.
    let (ok, _, stderr) = htl(&["check", "."], &root.join("b"));
    assert!(ok, "htl check on the project with the key:\n{stderr}");
}

/// The other end of the key the test above writes: `htl build` reads it. A `.hb` bundle is
/// the `hb` target, so a project that recorded another one is told which target it is and
/// which command builds it, and no bundle is written — the record is a decision rather
/// than a note the project keeps about itself.
#[test]
fn build_refuses_a_project_whose_target_is_not_hb() {
    let root = scratch("build-not-hb");
    // Only a pin that reads the key makes the scaffold write it, so the refusal below is
    // reached through what `htl new` produced rather than through a hand-written file.
    let (ok, _, stderr) = htl(&["new", "b", "--target", "bin", "--htl", "main"], &root);
    assert!(ok, "{stderr}");
    let dir = root.join("b");
    let config = std::fs::read_to_string(dir.join("htl.toml")).unwrap();
    assert!(config.contains("target = \"bin\""), "{config}");

    let (ok, _, stderr) = htl(&["build", "src/main.tl", "-o", "app.hb"], &dir);
    assert!(!ok, "{stderr}");
    assert!(stderr.contains("target is `bin`"), "{stderr}");
    assert!(
        stderr.contains("the OS, as a binary"),
        "who runs that output:\n{stderr}"
    );
    assert!(
        stderr.contains("cargo build"),
        "and what builds it:\n{stderr}"
    );
    assert!(!dir.join("app.hb").exists(), "{stderr}");
}

/// The default project is the `hb` target, whether it says so or not: `htl new` records no
/// key, and a project that records `hb` by hand gets the same bundle. Both halves matter —
/// the refusal above must not be what every build does.
#[test]
fn build_bundles_an_hb_project() {
    let root = scratch("build-hb");
    let (ok, _, stderr) = htl(&["new", "a"], &root);
    assert!(ok, "{stderr}");
    let dir = root.join("a");
    assert!(
        !std::fs::read_to_string(dir.join("htl.toml"))
            .unwrap()
            .contains("target ="),
        "the default project records nothing"
    );

    let (ok, _, stderr) = htl(&["build", "src/main.tl", "-o", "app.hb"], &dir);
    assert!(ok, "{stderr}");
    assert!(dir.join("app.hb").exists(), "{stderr}");

    // The same project, now saying what it already was.
    std::fs::remove_file(dir.join("app.hb")).unwrap();
    let config = std::fs::read_to_string(dir.join("htl.toml")).unwrap();
    std::fs::write(
        dir.join("htl.toml"),
        format!("{config}\n[build]\ntarget = \"hb\"\n"),
    )
    .unwrap();
    let (ok, _, stderr) = htl(&["build", "src/main.tl", "-o", "app.hb"], &dir);
    assert!(ok, "an explicit `hb` builds too:\n{stderr}");
    assert!(dir.join("app.hb").exists(), "{stderr}");
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

/// `--embed` is spelled `--target bin` from #195 on, and the two write the same tree. The
/// snapshot tests pin the tree; this pins that the shorthand still reaches it.
#[test]
fn embed_and_target_bin_write_the_same_thing() {
    let root = scratch("same");
    let (ok, _, stderr) = htl(&["new", "by-flag", "--embed"], &root);
    assert!(ok, "{stderr}");
    let (ok, _, stderr) = htl(&["new", "by-name", "--target", "bin"], &root);
    assert!(ok, "{stderr}");
    for f in ["src/lib.rs", "src/main.rs", "src/main.tl"] {
        let a = std::fs::read_to_string(root.join("by-flag").join(f)).unwrap();
        let b = std::fs::read_to_string(root.join("by-name").join(f)).unwrap();
        // The module identifier differs with the package name; the rest must not.
        assert_eq!(a.replace("by_flag", "M"), b.replace("by_name", "M"), "{f}");
    }
}

/// The C ABI target is a library and only a library: a `cdylib` has no entry point, so
/// `--target cdylib` without `--lib` is refused with the flag that fixes it, before the
/// directory exists.
#[test]
fn the_cdylib_target_needs_lib_and_says_so_before_writing_anything() {
    let root = scratch("cdylib-needs-lib");
    let (ok, _, stderr) = htl(&["new", "sample", "--target", "cdylib"], &root);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains("the `cdylib` target writes no entry script, so it needs --lib"),
        "{stderr}"
    );
    assert!(!root.join("sample").exists(), "{stderr}");
}

/// What `--target cdylib` writes that `--target bin` does not: the two extra crate types
/// the C caller links against, the feature the generated wrappers need, and the attribute
/// that writes the header. The snapshot pins every byte; this says what the bytes are
/// *for*, so a reader of the test knows what would break.
#[test]
fn the_cdylib_scaffold_is_a_c_library_with_the_export_attribute() {
    let root = scratch("cdylib");
    let (ok, _, stderr) = htl(&["new", "sample", "--lib", "--target", "cdylib"], &root);
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
/// is a target the flag offers. `--host` is on neither page: it is `htl build`'s flag, where
/// it names the modules the host provides — the other meaning of the word. The two senses
/// do not share a help page, which is the collision #189 removed.
#[test]
fn target_help_lists_every_registered_target_and_never_the_old_flag() {
    let root = scratch("target-help");
    for cmd in [&["new", "--help"][..], &["init", "--help"][..]] {
        let (ok, stdout, _) = htl(cmd, &root);
        assert!(ok);
        assert!(
            stdout.contains("[possible values: bin, cdylib]"),
            "{cmd:?}:\n{stdout}"
        );
        assert!(!stdout.contains("--host"), "{cmd:?}:\n{stdout}");
    }
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
    assert!(stderr.contains("bin"), "the registered names:\n{stderr}");
    assert!(stderr.contains("cdylib"), "the registered names:\n{stderr}");
    assert!(!root.join("sample").exists(), "{stderr}");
}

/// The old spellings are gone rather than aliased: a target is named for the artefact it
/// produces from #195 on, and someone whose fingers still write the pre-release names is
/// answered with the ones that replaced them rather than with a half-written directory.
#[test]
fn an_old_target_name_is_refused_with_the_new_ones() {
    let root = scratch("old-names");
    for old in ["rust", "ffi"] {
        let (ok, _, stderr) = htl(&["new", "x", "--target", old], &root);
        assert!(!ok, "`{old}` should not be a target:\n{stderr}");
        assert!(stderr.contains("bin"), "{old}:\n{stderr}");
        assert!(stderr.contains("cdylib"), "{old}:\n{stderr}");
        assert!(!root.join("x").exists(), "{old}:\n{stderr}");
    }
}

/// `htl init --target bin` on a project that predates the Rust side fills it in and
/// says which files it left alone, so nothing is skipped in silence.
#[test]
fn init_with_a_target_reports_what_it_kept() {
    let root = scratch("init-target");
    let (ok, _, stderr) = htl(&["new", "sample"], &root);
    assert!(ok, "{stderr}");
    let dir = root.join("sample");
    let (ok, _, stderr) = htl(&["init", "--target", "bin"], &dir);
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
