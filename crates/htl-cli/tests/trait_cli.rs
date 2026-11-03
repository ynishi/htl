//! `htl.trait` through the real binary: `htl run`, `htl check`, `htl test` and `htl build`
//! on the example the module's declaration shows: `run`, `test` and the first `build` in a
//! directory with no `htl.toml`, `check` and the second `build` inside a project.
//!
//! What each of them needs of the module is different — `run` the runtime in the checker's
//! state, `check` the declaration, `test` a program state made from a checker, `build` the
//! source in the bundle — so each is a test of its own.

use std::path::Path;
use std::process::Command;

mod common;

fn tempdir(name: &str) -> common::TempDir {
    common::tempdir("htl-cli-trait", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
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

/// The declaration of `Shape`, its defaults and `Circle`, up to the point where `Circle`
/// gets them; `tail` is what follows `trait.impl`.
fn program(circle_extra: &str, tail: &str) -> String {
    format!(
        "local trait = require(\"htl.trait\")\n\
         local interface Shape\n   area: function(self): number\n   describe: function(self): string\nend\n\
         local Shape_default: Shape = {{\n   describe = function(self: Shape): string\n      return (\"area=%.2f\"):format(self:area())\n   end,\n}}\n\
         local record Circle is Shape\n   r: number\nend\n\
         function Circle:area(): number return math.pi * self.r ^ 2 end\n\
         {circle_extra}\n\
         trait.impl(Shape_default, Circle)\n\
         {tail}\n\
         local c: Circle = setmetatable({{ r = 1.0 }}, {{ __index = Circle }})\n\
         print(c:describe())\n"
    )
}

/// Acceptance 1: `htl run` prints `area=3.14`.
#[test]
fn run_prints_the_default() {
    let root = tempdir("run");
    write(&root.join("src/main.tl"), &program("", ""));
    let (ok, out, err) = htl(&["run", "src/main.tl"], &root);
    assert!(ok, "{err}");
    assert_eq!(out, "area=3.14\n");
}

/// Acceptance 2: a method the record defines is not overwritten.
#[test]
fn run_keeps_the_records_own_method() {
    let root = tempdir("keeps");
    write(
        &root.join("src/main.tl"),
        &program(
            "function Circle:describe(): string return \"circle\" end",
            "",
        ),
    );
    let (ok, out, err) = htl(&["run", "src/main.tl"], &root);
    assert!(ok, "{err}");
    assert_eq!(out, "circle\n");
}

/// Acceptance 3: a record that is not `is Shape` is refused by `htl check`, with the
/// checker's own words.
#[test]
fn check_refuses_a_record_that_is_not_the_interface() {
    let root = tempdir("refused");
    write(
        &root.join("src/main.tl"),
        &program(
            "local record Other\n   r: number\nend",
            "trait.impl(Shape_default, Other)",
        ),
    );
    write(&root.join("htl.toml"), "");
    let (ok, out, err) = htl(&["check", "."], &root);
    assert!(!ok, "{out}\n{err}");
    assert!(
        format!("{out}{err}").contains("got Other, expected interface Shape"),
        "{out}\n{err}"
    );
}

/// `htl check` of the example itself is clean: the declaration is there for a project with
/// nothing configured.
#[test]
fn check_sees_the_declaration_for_a_bare_project() {
    let root = tempdir("check");
    write(&root.join("src/main.tl"), &program("", ""));
    write(&root.join("htl.toml"), "");
    let (ok, out, err) = htl(&["check", "."], &root);
    assert!(ok, "{out}\n{err}");
    assert!(
        out.contains("0 error(s)") || err.contains("0 error(s)"),
        "{out}\n{err}"
    );
}

/// `htl test`: a test file's program state is made from the test runner's checker, and has
/// the module and its declaration.
#[test]
fn test_runs_a_file_that_requires_it() {
    let root = tempdir("test");
    write(
        &root.join("src/shape_test.tl"),
        &(program("", "").replace(
            "print(c:describe())\n",
            "local t = require(\"htl.test\")\nt.it(\"describe\", function() t.expect(c:describe()):to_equal(\"area=3.14\") end)\n",
        )),
    );
    let (ok, out, err) = htl(&["test", "."], &root);
    assert!(ok, "{out}\n{err}");
}

/// Acceptance 5: `htl build` puts the module in the bundle and does not list it as the
/// host's, and the bundle runs and prints `area=3.14`.
#[test]
fn build_bundles_the_module_and_the_bundle_runs() {
    let root = tempdir("build");
    write(&root.join("src/main.tl"), &program("", ""));
    let (ok, _, err) = htl(&["build", "src/main.tl", "-o", "app.hb"], &root);
    assert!(ok, "build: {err}");
    let (ok, info, err) = htl(&["bundle", "info", "app.hb"], &root);
    assert!(ok, "{err}");
    assert!(info.contains("  modules:  htl.trait, main\n"), "{info}");
    assert!(info.contains("  host:     (none)\n"), "{info}");
    let (ok, out, err) = htl(&["run", "app.hb"], &root);
    assert!(ok, "{err}");
    assert_eq!(out, "area=3.14\n");
}

/// The same inside a project: with an `htl.toml` the linker asks the project model, which
/// files `htl.trait` as built in (htl's own library's declaration, with its source in the
/// binary). That is bundled, not left to the host, and not counted as a `.lua` module.
#[test]
fn build_in_a_project_bundles_the_module() {
    let root = tempdir("build-project");
    write(&root.join("htl.toml"), "");
    write(&root.join("src/main.tl"), &program("", ""));
    let (ok, _, err) = htl(&["build", "src/main.tl", "-o", "app.hb"], &root);
    assert!(ok, "build: {err}");
    assert!(!err.contains("host must provide"), "{err}");
    assert!(
        err.contains("2 module(s) (1 typed, 0 lua, 1 built-in)"),
        "{err}"
    );
    let (ok, info, err) = htl(&["bundle", "info", "app.hb"], &root);
    assert!(ok, "{err}");
    assert!(info.contains("htl.trait"), "{info}");
    assert!(info.contains("  host:     (none)\n"), "{info}");
    let (ok, out, err) = htl(&["run", "app.hb"], &root);
    assert!(ok, "{err}");
    assert_eq!(out, "area=3.14\n");
}

/// `htl resolve` says what `htl.trait` is: htl's own library, bundled by `htl build`. It is
/// not "provided by the environment", which would be the host's to supply.
#[test]
fn resolve_says_the_module_is_htls_own() {
    let root = tempdir("resolve");
    write(&root.join("htl.toml"), "");
    write(&root.join("src/main.tl"), &program("", ""));
    let (ok, out, err) = htl(&["resolve", "htl.trait"], &root);
    assert!(ok, "{out}\n{err}");
    let first = out.lines().next().unwrap_or_default();
    assert!(
        first.starts_with("htl resolve htl.trait: ")
            && first
                .ends_with("/htl/trait.d.tl, provided by htl's own library, bundled by htl build"),
        "{out}"
    );
    assert!(!out.contains("environment"), "{out}");
    let (ok, out, err) = htl(&["resolve", "htl.trait", "--format", "json"], &root);
    assert!(ok, "{out}\n{err}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["provided_by"].as_str(),
        Some("htl's own library, bundled by htl build"),
        "{v:#}"
    );
}

/// A project that types `htl.trait` itself, for a host that provides it and says so in
/// `[build] host`: the name is the host's. htl's source is not bundled in its place, and the
/// summary counts the project's one typed module and nothing built in.
#[test]
fn a_project_own_declaration_for_a_host_module_is_the_hosts() {
    let root = tempdir("own-decl-host");
    write(&root.join("htl.toml"), "[build]\nhost = [\"htl.trait\"]\n");
    write(
        &root.join("types/htl/trait.d.tl"),
        "local record Trait\n   mine: number\nend\nreturn Trait\n",
    );
    write(
        &root.join("src/main.tl"),
        "local t = require(\"htl.trait\")\nprint(t.mine)\n",
    );
    let (ok, _, err) = htl(&["build", "src/main.tl", "-o", "app.hb"], &root);
    assert!(ok, "build: {err}");
    assert!(err.contains("1 module(s) (1 typed, 0 lua)"), "{err}");
    assert!(!err.contains("built-in"), "{err}");
    assert!(err.contains("host must provide: htl.trait"), "{err}");
    let (ok, info, err) = htl(&["bundle", "info", "app.hb"], &root);
    assert!(ok, "{err}");
    assert!(info.contains("  modules:  main\n"), "{info}");
    assert!(info.contains("  host:     htl.trait\n"), "{info}");
}

/// The same project without `[build] host`: its own declaration is the environment's, as
/// for any name only a `.d.tl` holds, and `htl resolve` does not call it htl's own.
#[test]
fn a_project_own_declaration_stays_declared() {
    let root = tempdir("own-decl");
    write(&root.join("htl.toml"), "");
    write(
        &root.join("types/htl/trait.d.tl"),
        "local record Trait\n   mine: number\nend\nreturn Trait\n",
    );
    write(
        &root.join("src/main.tl"),
        "local t = require(\"htl.trait\")\nprint(t.mine)\n",
    );
    let (ok, out, err) = htl(&["resolve", "htl.trait"], &root);
    assert!(ok, "{out}\n{err}");
    assert!(!out.contains("bundled by htl build"), "{out}");
    let (ok, _, err) = htl(&["build", "src/main.tl", "-o", "app.hb"], &root);
    assert!(ok, "build: {err}");
    assert!(!err.contains("built-in"), "{err}");
    let (ok, info, err) = htl(&["bundle", "info", "app.hb"], &root);
    assert!(ok, "{err}");
    assert!(info.contains("  modules:  main\n"), "{info}");
}
