//! `htl gen` and `htl run` read the project's `htl.toml`, so they resolve what `htl check`
//! resolves.
//!
//! The two commands used to build their search path from the layout alone — the file's own
//! directory, plus the scaffold's rule for a file under `tests/` — and never called
//! `apply_config`. `htl check` did, so a module living under a `[check] paths` directory or
//! declared under `types/` was found by the checker and missing from the two commands that
//! come after it: a file `htl check` accepted could not be emitted or run, with
//! `module not found` naming a module that is right there on the checker's path.
//!
//! The project below is the smallest shape that separates the two paths. Nothing it
//! requires sits beside the file requiring it, so every case here fails without the config
//! and passes with it, and a regression cannot hide behind the layout rule.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-gen-run-config", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Run the binary in `cwd` and return whether it succeeded together with everything it
/// wrote. `htl gen` prints the generated Lua on stdout and its diagnostics on stderr, and
/// the assertions below ask about both, so the two are captured separately and the
/// combined text is what a failure prints.
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

/// A project whose modules are reachable only through `htl.toml`.
///
/// `lib/` is a `[check] paths` entry rather than one of the directories the layout knows,
/// and it holds both a declaration (`host_types.d.tl`, the shape a host provides) and two
/// modules in `<name>/init.tl` form. `types/` holds a second declaration, which is on the
/// search path because `search_paths` puts it there for every project. `src/entry.tl`
/// requires across the gap: it sits in `src/` and the module it wants is under `lib/`.
fn project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(
        &root.join("htl.toml"),
        "[check]\npaths = [\"lib\", \"types\"]\n",
    );
    write(
        &root.join("lib/host_types.d.tl"),
        "local record host_types\n   record Point\n      x: integer\n      y: integer\n   end\nend\nreturn host_types\n",
    );
    write(
        &root.join("lib/m1/init.tl"),
        "local host_types = require(\"host_types\")\n\nlocal record m1\nend\n\nfunction m1.origin(): host_types.Point\n   return { x = 0, y = 0 }\nend\n\nreturn m1\n",
    );
    write(
        &root.join("types/probe.d.tl"),
        "local record probe\n   record Tag\n      name: string\n   end\nend\nreturn probe\n",
    );
    write(
        &root.join("lib/m2/init.tl"),
        "local probe = require(\"probe\")\n\nlocal record m2\nend\n\nfunction m2.tag(): probe.Tag\n   return { name = \"t\" }\nend\n\nreturn m2\n",
    );
    write(
        &root.join("src/entry.tl"),
        "local m1 = require(\"m1\")\nprint(m1.origin().x)\n",
    );
    root
}

/// The two words a missing search path produces, so a failure says which half broke: the
/// `require` that found nothing, and the type that was therefore unknown.
fn assert_resolved(out: &str, err: &str) {
    let both = format!("{out}{err}");
    assert!(
        !both.contains("module not found"),
        "every require resolves off the configured path:\n{both}"
    );
    assert!(
        !both.contains("unknown type"),
        "a module that resolved also supplies its types:\n{both}"
    );
}

/// The premise of the rest: the checker is the command that was always right about this
/// project, and it stays right.
#[test]
fn check_resolves_a_module_under_a_configured_path() {
    let root = project("check");
    let (ok, out, err) = htl(&["check", "lib/m1/init.tl"], &root);
    assert!(ok, "the checker reads htl.toml:\n{out}{err}");
    assert_resolved(&out, &err);
}

/// `htl gen` on the module whose `require` is answered by a `[check] paths` directory. The
/// emitted Lua is asserted as well as the exit code: a command that resolved nothing and
/// printed nothing would also have to fail, but one that silently dropped the require
/// would not.
#[test]
fn gen_resolves_a_module_under_a_configured_path() {
    let root = project("gen");
    let (ok, out, err) = htl(&["gen", "lib/m1/init.tl"], &root);
    assert!(ok, "gen reads htl.toml as check does:\n{out}{err}");
    assert_resolved(&out, &err);
    assert!(
        out.contains("require(\"host_types\")"),
        "the require survives into the generated Lua:\n{out}"
    );
}

/// The same file, named from inside its own directory. The paths in `htl.toml` are relative
/// to the directory holding it, and the command is run from somewhere else — so this is the
/// case that would pass on a path built by joining the argument to the working directory
/// and fail on one built from the config's root.
#[test]
fn gen_resolves_it_from_the_module_s_own_directory() {
    let root = project("gen-cwd");
    let (ok, out, err) = htl(&["gen", "init.tl"], &root.join("lib/m1"));
    assert!(
        ok,
        "the project is found by walking up from the file:\n{out}{err}"
    );
    assert_resolved(&out, &err);
    assert!(
        out.contains("require(\"host_types\")"),
        "the require survives into the generated Lua:\n{out}"
    );
}

/// A declaration under `types/`, which is on the search path for every project with an
/// `htl.toml` whether or not `[check] paths` mentions it — and therefore on no search path
/// at all for a command that never read the config.
#[test]
fn gen_resolves_a_declaration_under_types() {
    let root = project("gen-types");
    let (ok, out, err) = htl(&["gen", "lib/m2/init.tl"], &root);
    assert!(ok, "types/ is on the path gen uses:\n{out}{err}");
    assert_resolved(&out, &err);
    assert!(
        out.contains("require(\"probe\")"),
        "the require survives into the generated Lua:\n{out}"
    );
}

/// `htl run` on an entry point whose dependency is in a directory only `htl.toml` names.
/// The printed `0` is the assertion that matters: it says the module was not merely found
/// but loaded and called, so the configured directories reached the runtime searcher and
/// not only the checker.
#[test]
fn run_resolves_a_module_under_a_configured_path() {
    let root = project("run");
    let (ok, out, err) = htl(&["run", "src/entry.tl"], &root);
    assert!(ok, "run reads htl.toml as check does:\n{out}{err}");
    assert_resolved(&out, &err);
    assert_eq!(out.trim(), "0", "the entry point ran:\n{out}{err}");
}
