//! Every path a report carries is spelled one way, whoever made the finding: relative to
//! the working directory, with no `./`, and absolute only outside it. The checker's own
//! errors, the project's findings about itself (a contract marker, a require cycle) and a
//! dependency's `required_by` used to come in three spellings in one report.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-one-path", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn htl(args: &[&str], cwd: &Path) -> (String, String) {
    let out = Command::new(common::htl_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A project with a type error of its own, a require cycle, and a contract marker left at
/// the root: a finding from the checker, one from the project layer, one about a marker.
fn project() -> PathBuf {
    let root = scratch("mixed");
    write(&root.join("htl.toml"), "[[contract]]\ndir = \"mods\"\n");
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"game\"\nversion = \"0.1.0\"\n",
    );
    write(
        &root.join("src/bad.tl"),
        "local x: integer = \"no\"\nprint(x)\n",
    );
    write(
        &root.join("src/a.tl"),
        "local b = require(\"b\")\nreturn { b = b }\n",
    );
    write(
        &root.join("src/b.tl"),
        "local a = require(\"a\")\nreturn { a = a }\n",
    );
    write(
        &root.join("defs.tl"),
        "local record defs\n   record Mod ---@contract\n      name: string\n   end\nend\nreturn defs\n",
    );
    root
}

#[test]
fn a_report_spells_every_path_one_way_in_text_and_in_json() {
    let root = project();
    let (_, err) = htl(&["check", "--no-cache", "."], &root);
    for line in err.lines().filter(|l| {
        l.starts_with("error: ") || l.starts_with("warning: ") || l.starts_with("lint: ")
    }) {
        let path = line.split_once(": ").unwrap().1;
        assert!(
            !path.starts_with("./") && !path.starts_with('/'),
            "every file in the project reads relative, with no ./: {line}"
        );
    }
    assert!(err.contains("error: src/bad.tl:1:"), "{err}");
    assert!(err.contains("lint: src/a.tl:1:"), "{err}");
    assert!(err.contains("lint: defs.tl:2:1:"), "{err}");

    let (out, _) = htl(&["check", "--no-cache", "--format", "json", "."], &root);
    let v: serde_json::Value = serde_json::from_str(&out).expect(&out);
    let files: Vec<&str> = v["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["file"].as_str().unwrap())
        .collect();
    assert!(files.contains(&"src/bad.tl"), "{files:?}");
    assert!(files.contains(&"src/a.tl"), "{files:?}");
    assert!(files.contains(&"defs.tl"), "{files:?}");
}

/// From a subdirectory, the same report reads against that directory: a file under it
/// relative to it, and one outside it absolute. `../src/bad.tl` is `bad.tl` here, not
/// `src/bad.tl`, which names a file that does not exist from where the command ran.
#[test]
fn from_a_subdirectory_paths_read_against_it() {
    let root = project();
    let (_, err) = htl(&["check", "--no-cache", ".."], &root.join("src"));
    assert!(err.contains("error: bad.tl:1:"), "{err}");
    assert!(err.contains("lint: a.tl:1:"), "{err}");
    let defs = std::fs::canonicalize(root.join("defs.tl")).unwrap();
    assert!(
        err.contains(&format!("lint: {}:2:1:", defs.display())),
        "outside the working directory: absolute: {err}"
    );
    assert!(
        err.contains(&format!("were not checked: {};", defs.display())),
        "{err}"
    );
}

/// `htl build` says the linker's own errors through the same sink as the checks': a
/// `require` nothing answers reads `src/main.tl:…` like the check's error beside it, not
/// the path as the command line gave it (`./src/main.tl`), and an `extra` module that is
/// not there is said once, with no position.
#[test]
fn build_spells_the_linkers_errors_as_the_checks() {
    let root = scratch("build");
    write(&root.join("htl.toml"), "[build]\nextra = [\"ghost\"]\n");
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"game\"\nversion = \"0.1.0\"\n",
    );
    write(
        &root.join("src/main.tl"),
        "local gone = require(\"nothere\")\nprint(gone)\n",
    );
    let (_, err) = htl(
        &["build", "--no-cache", "./src/main.tl", "-o", "out.hb"],
        &root,
    );
    assert!(
        err.lines().any(|l| l.starts_with("error: src/main.tl:1:")
            && l.contains("require(\"nothere\") is not on the search path")),
        "{err}"
    );
    assert!(!err.contains("./src/main.tl"), "{err}");
    assert_eq!(
        err.matches("error: extra module 'ghost' not found on the search path")
            .count(),
        1,
        "{err}"
    );
}

/// A junit report's `check` error is the checker's errors through the one formatter, the
/// file spelled as `htl check` spells it from the same directory.
#[test]
fn junit_spells_a_failed_check_as_the_report_does() {
    let root = scratch("junit");
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"game\"\nversion = \"0.1.0\"\n",
    );
    write(
        &root.join("tests/x_test.tl"),
        "local t = require(\"htl.test\")\nlocal x: integer = \"no\"\nt.it(\"a\", function() t.expect(x):to_equal(1) end)\n",
    );
    htl(&["test", "--junit", "report.xml", "."], &root);
    let out = std::fs::read_to_string(root.join("report.xml")).unwrap();
    let body = out.split("type=\"check\">").nth(1).expect(&out);
    assert!(body.starts_with("tests/x_test.tl:2:"), "{out}");
    assert!(!out.contains("./tests/x_test.tl:"), "{out}");
}
