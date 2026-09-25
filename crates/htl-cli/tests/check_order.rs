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
/// only reason `b` could ever resolve `util` is `a` being on the path. A flat project, so
/// that the tree can be walked: `a/util.tl` is `a.util`, and `util` is nobody's.
fn two_dirs() -> PathBuf {
    let root = scratch("two");
    write(&root.join("htl.toml"), "[layout]\nsource = \".\"\n");
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

/// A module that declares a `global` and a record, required by a middle module and by two
/// consumers that pass the record through the middle module. The declaring file is walked
/// once per run and its result served to every requirer, so the record is one type in all
/// of them; walked once per requirer, it was one type per requirer and the second consumer
/// could not call the middle module.
fn global_record_project() -> PathBuf {
    let root = scratch("global-record");
    write(
        &root.join("htl.toml"),
        "[layout]\nsource = \".\"\n[lint.rules]\nno-global = \"allow\"\n",
    );
    write(
        &root.join("modx.tl"),
        "global knl: {string:any}\nlocal record SessionX\n   id: function(SessionX): string\nend\n\
         local record M\n   type Session = SessionX\n   open: function(): SessionX\n   use: function(SessionX): string\nend\n\
         function M.open(): SessionX return nil end\nfunction M.use(s: SessionX): string return s:id() end\nreturn M\n",
    );
    write(
        &root.join("midx.tl"),
        "local k = require(\"modx\")\nlocal record S\n   whole: function(k.Session, string): string\nend\n\
         function S.whole(s: k.Session, _who: string): string return s:id() end\nreturn S\n",
    );
    for (m, who) in [("topx", "x"), ("topy", "y")] {
        write(
            &root.join(format!("{m}.tl")),
            &format!(
                "local k = require(\"modx\")\nlocal S = require(\"midx\")\n\
                 local function go(s: k.Session): string return S.whole(s, \"{who}\") end\nprint(go(k.open()))\n"
            ),
        );
    }
    root
}

#[test]
fn a_record_declared_beside_a_global_is_one_type_whatever_the_order() {
    let root = global_record_project();
    let whole = diagnostics(&["--strict", "."], &root);
    let backward = diagnostics(
        &["--strict", "topy.tl", "topx.tl", "midx.tl", "modx.tl"],
        &root,
    );
    let alone = diagnostics(&["--strict", "topx.tl"], &root);
    assert!(
        whole.is_empty(),
        "one walk of modx.tl, one SessionX in every requirer: {whole:?}"
    );
    assert_eq!(
        whole, backward,
        "the order the files are given in decides nothing"
    );
    assert_eq!(
        whole, alone,
        "a consumer alone reports what it reports in the tree"
    );
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

/// A cached test is replayed only while every name it required still means the file it
/// meant. A helper added directly under `tests/` gives `helper` a second implementation
/// for a test in `tests/sub/`, and the next run says so instead of replaying the entry.
#[test]
fn a_cached_test_sees_a_helper_added_under_tests() {
    let root = scratch("cache-helper");
    write(&root.join("htl.toml"), "");
    write(&root.join("src/helper.tl"), "return { v = 1 }\n");
    write(
        &root.join("tests/sub/a_test.tl"),
        "local t = require(\"htl.test\")\nlocal h = require(\"helper\")\n\
         t.it(\"x\", function() t.expect(h.v):to_equal(1) end)\n",
    );
    let test = || {
        Command::new(common::htl_bin())
            .args(["test", "."])
            .current_dir(&root)
            .output()
            .unwrap()
    };
    assert!(
        test().status.success(),
        "the first run passes and is stored"
    );
    write(&root.join("tests/helper.tl"), "return { v = 2 }\n");
    let out = test();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "not replayed from the store: {err}");
    assert!(!err.contains("from cache"), "{err}");
}

/// A name two files implement is reported at the `require` as that, naming both — not as
/// the `module not found` Teal says when its search comes back empty. Within the sources,
/// and in a test's view, where a helper under `tests/` and a module under `src/` answer to
/// the same name.
#[test]
fn a_require_of_a_name_two_files_implement_names_both() {
    let root = scratch("ambiguous-require");
    write(&root.join("htl.toml"), "");
    write(&root.join("src/demo.tl"), "return { n = 1 }\n");
    write(&root.join("src/demo/init.tl"), "return { n = 2 }\n");
    write(
        &root.join("src/main.tl"),
        "local d = require(\"demo\")\nprint(d.n)\n",
    );
    write(&root.join("src/helper.tl"), "return { v = 1 }\n");
    write(&root.join("tests/helper.tl"), "return { v = 2 }\n");
    write(
        &root.join("tests/sub/a_test.tl"),
        "local t = require(\"htl.test\")\nlocal h = require(\"helper\")\n\
         t.it(\"x\", function() t.expect(h.v):to_equal(1) end)\n",
    );
    let d = diagnostics(&["."], &root).join("\n");
    assert!(!d.contains("module not found"), "{d}");
    assert!(
        d.contains("src/main.tl:1: 'demo' is implemented by more than one file")
            && d.contains("(src/demo.tl)")
            && d.contains("(src/demo/init.tl)"),
        "{d}"
    );
    assert!(
        d.contains("tests/sub/a_test.tl:2: 'helper' is implemented by more than one file")
            && d.contains("(src/helper.tl)")
            && d.contains("(tests/helper.tl)"),
        "{d}"
    );
}

/// A `require` no check sees — in a plain `.lua` — of a name two files implement: the run
/// and the bundle refuse it with the same message, rather than loading or bundling one of
/// the two.
#[test]
fn a_plain_lua_require_of_a_name_two_files_implement_is_refused_at_run_time_and_in_a_bundle() {
    let root = scratch("ambiguous-lua");
    write(&root.join("htl.toml"), "");
    write(&root.join("src/demo.tl"), "return { n = 1 }\n");
    write(&root.join("src/demo/init.tl"), "return { n = 2 }\n");
    write(
        &root.join("src/plain.lua"),
        "local d = require(\"demo\")\nreturn d\n",
    );
    write(
        &root.join("src/plain.d.tl"),
        "local record plain\n   n: integer\nend\nreturn plain\n",
    );
    write(
        &root.join("src/main.tl"),
        "local p = require(\"plain\")\nprint(p.n)\n",
    );
    let run = |args: &[&str]| {
        let out = Command::new(common::htl_bin())
            .args(args)
            .current_dir(&root)
            .output()
            .unwrap();
        (
            out.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        )
    };
    let bundle = root.join("out.hb");
    for (ok, out) in [
        run(&["run", "src/main.tl"]),
        run(&["build", "src/main.tl", "-o", bundle.to_str().unwrap()]),
    ] {
        assert!(!ok, "{out}");
        assert!(
            out.contains("'demo' is implemented by more than one file")
                && out.contains("(src/demo.tl)")
                && out.contains("(src/demo/init.tl)"),
            "{out}"
        );
    }
    assert!(!bundle.exists(), "no bundle is written");
}

/// A project keeps one `.htl/`: its store goes at the project's root wherever the command
/// ran from, beside the installed dependencies — for a project described by
/// `mlua-pkg.toml` alone as for one with an `htl.toml`. It used to go beside `htl.toml`,
/// else in the working directory, so checking such a project from `src/` wrote a second
/// `.htl/` there.
#[test]
fn the_store_is_at_the_project_root_wherever_the_command_ran() {
    let root = scratch("store-root");
    write(
        &root.join("mlua-pkg.toml"),
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n",
    );
    write(&root.join("src/a.tl"), "print(1)\n");
    let out = Command::new(common::htl_bin())
        .args(["check", "."])
        .current_dir(root.join("src"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(root.join(".htl/cache").is_dir(), "the store is at the root");
    assert!(!root.join("src/.htl").exists(), "and nowhere else");
}

/// A directory is checked as a project; with no `htl.toml` or `mlua-pkg.toml` in it or
/// above it, `htl check` says so — what it looked for, from where, and what to do — rather
/// than checking every file as a thing on its own under a name no project gave it. A file
/// named on its own is still checked: that question has an answer without a project.
#[test]
fn a_directory_with_no_project_is_refused_and_a_file_is_still_checked() {
    let root = scratch("no-project");
    write(&root.join("a.tl"), "print(1)\n");
    let run = |args: &[&str]| {
        let out = Command::new(common::htl_bin())
            .args(args)
            .current_dir(&root)
            .output()
            .unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    for args in [&["check"][..], &["check", "."][..]] {
        let (ok, err) = run(args);
        assert!(!ok, "{args:?}: {err}");
        assert!(
            err.contains(&format!(
                "no htl.toml or mlua-pkg.toml in {} or any directory above it",
                std::fs::canonicalize(&root).unwrap().display()
            )) && err.contains("htl init"),
            "{args:?}: {err}"
        );
    }
    let (ok, err) = run(&["check", "a.tl"]);
    assert!(ok, "{err}");
}

/// A global declared by a module only the first link of a chain requires, and a file
/// that requires nothing and reads it anyway. Through the CLI: the chain is clean and the
/// loose file is told `unknown variable`, whatever the order and alone.
fn global_chain_project() -> PathBuf {
    let root = scratch("global-chain");
    write(
        &root.join("htl.toml"),
        "[layout]\nsource = \".\"\n[lint.rules]\nno-global = \"allow\"\n",
    );
    write(&root.join("hostg.d.tl"), "global VERSION: string\n");
    write(
        &root.join("modx.tl"),
        "require(\"hostg\")\nlocal record M\n   v: function(): string\nend\n\
         function M.v(): string return VERSION end\nreturn M\n",
    );
    write(
        &root.join("midx.tl"),
        "local k = require(\"modx\")\nlocal record S\n   both: function(): string\nend\n\
         function S.both(): string return k.v() .. VERSION end\nreturn S\n",
    );
    write(
        &root.join("topx.tl"),
        "local s = require(\"midx\")\nprint(s.both() .. VERSION)\n",
    );
    write(&root.join("loose.tl"), "print(VERSION)\n");
    root
}

#[test]
fn a_global_reaches_the_end_of_a_require_chain_and_nowhere_else() {
    let root = global_chain_project();
    let loose = "loose.tl:1: unknown variable: VERSION";
    let whole = diagnostics(&["--strict", "."], &root);
    let backward = diagnostics(
        &["--strict", "loose.tl", "topx.tl", "midx.tl", "modx.tl"],
        &root,
    );
    assert_eq!(
        whole,
        vec![loose.to_string()],
        "the tree: only the loose file"
    );
    assert_eq!(
        whole, backward,
        "the order the files are given in decides nothing"
    );
    let mid_alone = diagnostics(&["--strict", "midx.tl"], &root);
    assert!(mid_alone.is_empty(), "midx alone: {mid_alone:?}");
    let loose_alone = diagnostics(&["--strict", "loose.tl"], &root);
    assert_eq!(loose_alone, vec![loose.to_string()], "loose alone");
}
