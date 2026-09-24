//! A file's diagnostics must not depend on what was checked before it.
//!
//! `htl check` adds each file's directory to the search path as it walks. Without putting
//! the path back afterwards, the Nth file resolves `require` against the directories of
//! the first N-1 too — so the same file reports differently depending on where it fell in
//! the walk, and the direction is the dangerous one: an error that should be reported
//! disappears because something checked earlier happened to provide the module.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-order", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn diagnostics(args: &[&str], cwd: &Path) -> Vec<String> {
    let out = Command::new(common::htl_bin())
        .arg("check")
        .args(args)
        .arg("--format")
        .arg("json")
        .arg("--no-cache")
        .current_dir(cwd)
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("stdout is one JSON document");
    v["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .map(|d| {
            // The checker echoes the path as it was given, so `htl check .` says
            // `./b/main.tl` where `htl check b/main.tl` says `b/main.tl`. That spelling is
            // not what these tests are about.
            let file = d["file"].as_str().unwrap_or("").trim_start_matches("./");
            format!(
                "{file}:{}: {}",
                d["line"],
                d["message"].as_str().unwrap_or("")
            )
        })
        .collect()
}

/// Two directories where one provides a module the other requires but does not have. The
/// only reason `b` could ever resolve `util` is `a` being on the path.
fn two_dirs() -> PathBuf {
    let root = scratch("two");
    write(
        &root.join("a/util.tl"),
        "local record util\nend\nfunction util.f(): integer\n   return 1\nend\nreturn util\n",
    );
    write(
        &root.join("b/main.tl"),
        "local util = require(\"util\")\nlocal x: integer = util.f()\nprint(x)\n",
    );
    root
}

#[test]
fn a_file_reports_the_same_wherever_it_falls_in_the_walk() {
    let root = two_dirs();
    let alone = diagnostics(&["b/main.tl"], &root);
    let after = diagnostics(&["a/util.tl", "b/main.tl"], &root);
    let before = diagnostics(&["b/main.tl", "a/util.tl"], &root);

    assert!(
        alone.iter().any(|d| d.contains("module not found")),
        "b/ has no util.tl, so checking it alone must say so: {alone:?}"
    );
    assert_eq!(
        alone, after,
        "checking a/ first must not make b/'s missing module resolve"
    );
    assert_eq!(alone, before, "nor must checking it second");
}

#[test]
fn walking_a_tree_agrees_with_checking_its_files_one_at_a_time() {
    let root = two_dirs();
    let whole = diagnostics(&["."], &root);
    let mut apart = diagnostics(&["a/util.tl"], &root);
    apart.extend(diagnostics(&["b/main.tl"], &root));
    assert_eq!(
        whole, apart,
        "a walk is the files it visits, in the state each would be checked in"
    );
}

/// A file under the project's test root reads the project's sources, so that `htl check
/// tests` sees what `htl test` sees. Restoring the path per file must not take that away.
///
/// The `htl.toml` is what makes the directory a project: without one there is no source
/// root to read, and a file resolves its `require`s beside it and nowhere else.
#[test]
fn a_test_file_still_sees_the_project_root_and_src() {
    let root = scratch("tests-layout");
    write(&root.join("htl.toml"), "");
    write(
        &root.join("src/util.tl"),
        "local record util\nend\nfunction util.f(): integer\n   return 1\nend\nreturn util\n",
    );
    write(
        &root.join("tests/util_test.tl"),
        "local util = require(\"util\")\nlocal x: integer = util.f()\nprint(x)\n",
    );
    let d = diagnostics(&["."], &root);
    assert!(
        d.is_empty(),
        "a file under tests/ resolves modules from src/: {d:?}"
    );
}

/// A module that declares a `global` is the second channel of order dependence, and an
/// independent one: the search path is the same for every file here, and the answer still
/// moved. The checked-module store served the declaration's checked result to every
/// environment after the first, and replaying a result is not walking a file — only the
/// walk registers a global into the environment it happens in.
///
/// The shape is a project's: the host's names in one declaration file under `types/`, and
/// modules under `lib/` that require it.
fn globals_project() -> PathBuf {
    let root = scratch("globals");
    write(&root.join("htl.toml"), "[check]\npaths = [\"lib\"]\n");
    write(
        &root.join("types/host_globals.d.tl"),
        "global record std\n   record Json\n      encode: function(any): string\n      decode: function(string): any\n   end\n   json: Json\nend\nglobal VERSION: string\nglobal host_log: function(string)\n",
    );
    for n in 1..=2 {
        write(
            &root.join(format!("lib/m{n}/init.tl")),
            &format!(
                "require(\"host_globals\")\n\nlocal record m{n}\nend\n\nfunction m{n}.run(): string\n   host_log(VERSION)\n   return std.json.encode({{ a = {n} }})\nend\n\nreturn m{n}\n"
            ),
        );
    }
    root
}

#[test]
fn a_global_from_a_required_module_reaches_every_file_whatever_the_order() {
    let root = globals_project();
    let forward = diagnostics(&["--strict", "lib/m1", "lib/m2"], &root);
    let backward = diagnostics(&["--strict", "lib/m2", "lib/m1"], &root);
    let whole = diagnostics(&["--strict", "lib"], &root);

    assert!(
        forward.is_empty(),
        "both modules require the file that declares std, VERSION and host_log: {forward:?}"
    );
    assert_eq!(
        forward, backward,
        "the order the files are given in decides nothing"
    );
    assert_eq!(forward, whole, "a walk is the files it visits");
}

/// What a name means does not depend on where the command ran. A module that happens to
/// sit in the working directory is not one of the project's, and a project that requires
/// it without having it is told so from anywhere.
#[test]
fn the_working_directory_is_not_searched() {
    let root = scratch("cwd-not-searched");
    let project = root.join("project");
    write(&project.join("htl.toml"), "");
    write(
        &project.join("src/main.tl"),
        "local util = require(\"util\")\nprint(util)\n",
    );
    // Beside the command, not in the project.
    write(&root.join("util.tl"), "return {}\n");
    let d = diagnostics(&["project"], &root);
    assert!(
        d.iter().any(|l| l.contains("module not found: 'util'")),
        "the util.tl in the working directory is not the project's: {d:?}"
    );
}

/// A name the project and a dependency both implement is an error, said at each file and
/// naming both owners — not settled by which of the two the search path met first.
#[test]
fn a_name_the_project_and_a_dependency_both_implement_is_an_error() {
    let root = scratch("own-vs-dep");
    write(&root.join("htl.toml"), "");
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"game\"\nversion = \"0.1.0\"\n",
    );
    write(
        &root.join(".htl/modules/entries/mathx/init.tl"),
        "return {}\n",
    );
    write(&root.join("src/mathx.tl"), "return {}\n");
    write(
        &root.join("src/main.tl"),
        "local m = require(\"mathx\")\nprint(m)\n",
    );
    let d = diagnostics(&["."], &root);
    let about: Vec<&String> = d
        .iter()
        .filter(|l| l.contains("module name 'mathx' has more than one owner"))
        .collect();
    assert_eq!(about.len(), 2, "one at each file: {d:?}");
    assert!(
        about[0].contains("this project (src/mathx.tl)")
            && about[0].contains("dependency mathx (.htl/modules/entries/mathx/init.tl)"),
        "{about:?}"
    );
}

/// A dependency sees its own modules and what it depends on, not the project using it: a
/// `require` in a dependency that lands on the project's own `src/util.tl` is an error at
/// that call, while one that lands on another dependency is not.
#[test]
fn a_dependency_does_not_see_the_projects_own_modules() {
    let root = scratch("dep-view");
    write(&root.join("htl.toml"), "");
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"game\"\nversion = \"0.1.0\"\n",
    );
    write(
        &root.join(".htl/modules/entries/mathx/init.tl"),
        "local u = require(\"util\")\nlocal o = require(\"other\")\nprint(u, o)\nreturn {}\n",
    );
    write(
        &root.join(".htl/modules/entries/other/init.tl"),
        "return {}\n",
    );
    write(&root.join("src/util.tl"), "return {}\n");
    write(
        &root.join("src/main.tl"),
        "local m = require(\"mathx\")\nprint(m)\n",
    );
    let d = diagnostics(&["."], &root);
    let reach: Vec<&String> = d
        .iter()
        .filter(|l| l.contains("in dependency mathx reaches"))
        .collect();
    assert_eq!(
        reach.len(),
        1,
        "the util require, and not the other one: {d:?}"
    );
    assert!(reach[0].contains("require(\"util\")"), "{reach:?}");
    assert!(
        reach[0].contains(".htl/modules/entries/mathx/init.tl:1:"),
        "said at the dependency's call: {reach:?}"
    );
}

/// `[imports]` settles a name two modules answer to without a rename: the project says
/// which one it means, and reaches the other under a name of its choosing. The
/// dependency's own `require`s keep meaning the dependency's modules.
#[test]
fn imports_say_which_module_a_shared_name_means() {
    let root = scratch("imports");
    write(
        &root.join("htl.toml"),
        "[imports]\nmathx = \"dep:mathx\"\nmathx_local = \"own:mathx\"\n",
    );
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"game\"\nversion = \"0.1.0\"\n",
    );
    write(
        &root.join(".htl/modules/entries/mathx/init.tl"),
        "local vec = require(\"mathx.vec\")\nreturn { twice = function(n: integer): integer return vec.k * n end }\n",
    );
    write(
        &root.join(".htl/modules/entries/mathx/vec.tl"),
        "return { k = 2 }\n",
    );
    write(&root.join("src/mathx.tl"), "return { local_one = 1 }\n");
    // The project's own `mathx.vec` too: the dependency must still read its own.
    write(&root.join("src/mathx/vec.tl"), "return { other = true }\n");
    write(
        &root.join("src/main.tl"),
        "local m = require(\"mathx\")\nlocal l = require(\"mathx_local\")\n\
         local x: integer = m.twice(21)\nlocal y: integer = l.local_one\nprint(x + y)\n",
    );
    let d = diagnostics(&["."], &root);
    assert!(d.is_empty(), "the imports settle both names: {d:?}");
    // And the program runs with the modules the project meant, from source and bundled.
    let run = |args: &[&str]| {
        let out = Command::new(common::htl_bin())
            .args(args)
            .current_dir(&root)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    assert_eq!(run(&["run", "src/main.tl"]), "43");
    run(&["build", "src/main.tl", "-o", "app.hb"]);
    assert_eq!(run(&["run", "app.hb"]), "43");

    write(
        &root.join("htl.toml"),
        "[imports]\nmathx = \"dep:nosuch\"\n",
    );
    let d = diagnostics(&["."], &root);
    assert!(
        d.iter().any(|l| l.contains("no dependency named nosuch")),
        "{d:?}"
    );
}

/// A file answers to the name the project model gives it and to no other. The search
/// path's `<dir>/?/?.lua` template would make `src/util/util.tl` answer to `util`, and
/// `types/` on the path would make a crate's `types/<crate>/mq.d.tl` answer to
/// `<crate>.mq` as well as to `mq`; neither is the file's name.
#[test]
fn a_file_answers_to_its_model_name_only() {
    let root = scratch("model-names");
    write(&root.join("htl.toml"), "");
    write(&root.join("src/util/util.tl"), "return { n = 1 }\n");
    write(
        &root.join("types/shipper").join(".htl-dts"),
        "crate = \"shipper\"\nversion = \"0.1.0\"\nfiles = [\"mq.d.tl\"]\n",
    );
    write(
        &root.join("types/shipper/mq.d.tl"),
        "local record mq\n   n: integer\nend\nreturn mq\n",
    );
    write(
        &root.join("src/main.tl"),
        "local a = require(\"util\")\nlocal b = require(\"shipper.mq\")\n\
         local c = require(\"util.util\")\nlocal d = require(\"mq\")\nprint(a, b, c, d)\n",
    );
    let d = diagnostics(&["."], &root);
    let missing: Vec<&String> = d
        .iter()
        .filter(|l| l.contains("module not found"))
        .collect();
    assert_eq!(missing.len(), 2, "{d:?}");
    assert!(missing.iter().any(|l| l.contains("'util'")), "{d:?}");
    assert!(missing.iter().any(|l| l.contains("'shipper.mq'")), "{d:?}");
}

/// Run time and a bundle resolve with the same model the check did. A dependency's plain
/// `.lua`, typed by a declaration, requires another `.lua` of the same dependency: the
/// run serves both from the dependency, and the bundle carries both.
#[test]
fn run_time_and_the_bundle_resolve_as_the_check_did() {
    let root = scratch("run-resolves");
    write(&root.join("htl.toml"), "");
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"game\"\nversion = \"0.1.0\"\n",
    );
    let dep = root.join(".htl/modules/entries/mathx");
    write(
        &dep.join("raw.lua"),
        "local more = require(\"mathx.more\")\n\
         local alias = pcall(require, \"mathx\")\n\
         return { k = more.k + 1, alias = alias }\n",
    );
    write(&dep.join("more.lua"), "return { k = 6 }\n");
    write(
        &dep.join("raw.d.tl"),
        "local record raw\n   k: integer\n   alias: boolean\nend\nreturn raw\n",
    );
    // What the `?/?` template would have made `mathx`: the project's own `src/mathx/mathx.tl`
    // is `mathx.mathx`, and a run must not read it for `mathx.more` or anything else.
    write(&root.join("src/mathx/mathx.tl"), "return { k = 100 }\n");
    write(
        &root.join("src/main.tl"),
        "local raw = require(\"mathx.raw\")\nprint(raw.k, raw.alias)\n",
    );
    let run = |args: &[&str]| {
        let out = Command::new(common::htl_bin())
            .args(args)
            .current_dir(&root)
            .output()
            .unwrap();
        (
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    let (out, err) = run(&["run", "src/main.tl"]);
    assert_eq!(out, "7\tfalse", "{err}");
    let (_, err) = run(&["build", "src/main.tl", "-o", "app.hb"]);
    assert!(err.contains("-> app.hb"), "{err}");
    let (info, _) = run(&["bundle", "info", "app.hb"]);
    assert!(
        info.contains("mathx.raw") && info.contains("mathx.more"),
        "{info}"
    );
    let (out, err) = run(&["run", "app.hb"]);
    assert_eq!(out, "7\tfalse", "{err}");
}
