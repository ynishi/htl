//! The run cache (`src/cache.rs`) through the real binary.
//!
//! Two properties, and the second is the one that needs the tests. A cached run has to be
//! indistinguishable from the run it stands in for, apart from saying that it was cached
//! — and it has to stop being used the moment anything that fed it moves. A cache that
//! never invalidates passes an "output is identical" test perfectly while being wrong, so
//! most of what is below makes something change and insists on a miss.

use std::path::Path;
use std::process::Command;

mod common;

fn tempdir(name: &str) -> common::TempDir {
    common::tempdir("htl-cli-cache", name)
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

/// A project that produces diagnostics: replaying an empty report proves nothing, so the
/// fixture carries a type error and a lint alongside a module that is fine.
fn project(name: &str) -> common::TempDir {
    let root = tempdir(name);
    write(&root.join("htl.toml"), "[check]\n");
    write(
        &root.join("src/util.tl"),
        "local record util\nend\nfunction util.twice(n: integer): integer\n   return n * 2\nend\nreturn util\n",
    );
    write(
        &root.join("src/bad.tl"),
        "local t: {string: {integer}} = {}\nlocal n: string = t[\"a\"][1]\nprint(n)\n",
    );
    root
}

fn check(root: &Path) -> serde_json::Value {
    let (_, stdout, _) = htl(&["check", "src", "--format", "json"], root);
    serde_json::from_str(&stdout).expect("stdout is one JSON document")
}

fn was_cached(v: &serde_json::Value) -> bool {
    v["summary"]["cached"]
        .as_bool()
        .expect("summary carries `cached`")
}

#[test]
fn a_second_run_reports_exactly_what_the_first_reported() {
    let root = project("replay");
    let (ok1, out1, _) = htl(&["check", "src", "--format", "json"], &root);
    let (ok2, out2, _) = htl(&["check", "src", "--format", "json"], &root);
    let mut v1: serde_json::Value = serde_json::from_str(&out1).unwrap();
    let mut v2: serde_json::Value = serde_json::from_str(&out2).unwrap();

    assert!(!was_cached(&v1), "the first run has nothing to replay");
    assert!(was_cached(&v2), "the second run must come from the store");
    assert_eq!(ok1, ok2, "a replayed run must agree about the exit code");
    assert!(
        !v1["diagnostics"].as_array().unwrap().is_empty(),
        "the fixture must produce diagnostics or this test proves nothing"
    );

    // Everything but the two fields that say where the answer came from.
    for v in [&mut v1, &mut v2] {
        v["summary"]["cached"] = false.into();
        v["summary"]["replayed"] = 0.into();
    }
    assert_eq!(
        v1, v2,
        "a cached run reports what the run it replaces reported"
    );
}

#[test]
fn text_output_differs_only_in_saying_it_was_cached() {
    let root = project("text");
    let (_, _, err1) = htl(&["check", "src"], &root);
    let (_, _, err2) = htl(&["check", "src"], &root);
    assert!(!err1.contains("[cached]"), "{err1}");
    assert!(err2.contains("[cached]"), "{err2}");

    // The summary is the one line that is allowed to differ.
    let diagnostics = |s: &str| {
        s.lines()
            .filter(|l| !l.starts_with("htl check:") && !l.starts_with("htl check --explain "))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(
        !diagnostics(&err1).is_empty(),
        "the fixture must print diagnostics"
    );
    assert_eq!(
        diagnostics(&err1),
        diagnostics(&err2),
        "the diagnostics a replay prints are the stored ones"
    );
}

#[test]
fn editing_a_checked_file_misses() {
    let root = project("edit");
    assert!(!was_cached(&check(&root)));
    assert!(was_cached(&check(&root)));
    write(
        &root.join("src/util.tl"),
        "local record util\nend\nfunction util.twice(n: integer): integer\n   return n + n\nend\nreturn util\n",
    );
    assert!(
        !was_cached(&check(&root)),
        "an edited module must be checked again"
    );
}

/// A project where one module requires another by name, so there is a name whose resolution
/// can be disturbed.
fn requires_helper(name: &str) -> common::TempDir {
    let root = tempdir(name);
    write(&root.join("htl.toml"), "[check]\n");
    write(
        &root.join("src/helper.tl"),
        "local record helper\nend\nfunction helper.f(): integer\n   return 1\nend\nreturn helper\n",
    );
    write(
        &root.join("src/main.tl"),
        "local helper = require(\"helper\")\nlocal x: integer = helper.f()\nprint(x)\n",
    );
    root
}

/// The case a cache keyed only on the files it read gets wrong, and the one ccache documents
/// as its direct-mode hole: nothing already recorded changed, `types/` did not exist when
/// the entry was written, and yet a name this module requires can now be found there too.
///
/// The checker would still pick `src/helper.tl` — `src/` comes first — but whether it does
/// is no longer the same question, so the entry is not trusted.
#[test]
fn a_module_appearing_under_a_required_name_misses() {
    let root = requires_helper("appear");
    assert!(!was_cached(&check(&root)));
    assert!(was_cached(&check(&root)));

    write(
        &root.join("types/helper.d.tl"),
        "local record helper\n   f: function(): integer\nend\nreturn helper\n",
    );
    assert!(
        !was_cached(&check(&root)),
        "a file appearing under a required name must be checked again"
    );
}

/// The other half, and what #24 was about: adding a module nobody requires is a normal thing
/// to do while working, and it used to re-check the project. A probe that hashed every name
/// in the directory could not tell the two cases apart.
#[test]
fn a_module_appearing_under_a_name_nobody_requires_does_not() {
    let root = requires_helper("unrelated");
    assert!(!was_cached(&check(&root)));
    assert_eq!(replayed(&check(&root)), 2);

    write(
        &root.join("src/brand_new.tl"),
        "local record brand_new\nend\nreturn brand_new\n",
    );
    let v = check(&root);
    assert_eq!(v["files"], 3, "the new module is part of the walk now");
    assert_eq!(
        replayed(&v),
        2,
        "the two that were there replay; only the new one is checked"
    );
}

#[test]
fn changing_the_config_misses() {
    let root = project("config");
    assert!(!was_cached(&check(&root)));
    assert!(was_cached(&check(&root)));
    write(
        &root.join("htl.toml"),
        "[lint.rules]\nnil-index = \"allow\"\n",
    );
    assert!(
        !was_cached(&check(&root)),
        "the config shapes the report and is part of the inputs"
    );
}

/// A flag belongs in the key when it changes what a module *reports*, and not when it only
/// changes what the run concludes. `--strict` decides the exit code from diagnostics that
/// are the same either way, so its modules are reusable; `--lint` changes which lints run,
/// so they are not.
#[test]
fn a_flag_is_in_the_key_only_when_it_changes_what_a_module_reports() {
    let root = project("flags");
    assert!(!was_cached(&check(&root)));
    assert!(was_cached(&check(&root)));

    let (_, stdout, _) = htl(&["check", "src", "--format", "json", "--strict"], &root);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(
        was_cached(&v),
        "--strict changes the verdict, not the diagnostics: {v}"
    );

    let (_, stdout, _) = htl(
        &["check", "src", "--format", "json", "--lint", "+no-any"],
        &root,
    );
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(
        !was_cached(&v),
        "a different lint selection does change them: {v}"
    );
}

#[test]
fn a_truncated_entry_is_a_miss_rather_than_a_crash() {
    let root = project("corrupt");
    assert!(!was_cached(&check(&root)));
    let entry = std::fs::read_dir(root.join(".htl/cache"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(&entry, "{\"stamp\":{\"format\":1,\"htl\":\"0.1.0\"").unwrap();
    let v = check(&root);
    assert!(!was_cached(&v), "a half-written entry must be ignored");
    assert!(
        !v["diagnostics"].as_array().unwrap().is_empty(),
        "and the run must happen normally"
    );
}

#[test]
fn no_cache_neither_writes_nor_reads() {
    let root = project("off");
    htl(&["check", "src", "--format", "json", "--no-cache"], &root);
    assert!(
        !root.join(".htl").exists(),
        "--no-cache must not create a store"
    );

    assert!(!was_cached(&check(&root)));
    assert!(was_cached(&check(&root)));
    let (_, stdout, _) = htl(&["check", "src", "--format", "json", "--no-cache"], &root);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(
        !was_cached(&v),
        "--no-cache must not read an entry that is sitting there"
    );
}

fn replayed(v: &serde_json::Value) -> u64 {
    v["summary"]["replayed"]
        .as_u64()
        .expect("summary carries `replayed`")
}

/// Three modules: a leaf, one requiring it, and one requiring nothing. Enough to tell the
/// two granularities apart, since only per-module can leave the third alone.
fn three_modules(name: &str) -> common::TempDir {
    let root = tempdir(name);
    write(&root.join("htl.toml"), "[check]\n");
    write(
        &root.join("src/leaf.tl"),
        "local record leaf\nend\nfunction leaf.f(): integer\n   return 1\nend\nreturn leaf\n",
    );
    write(
        &root.join("src/uses_leaf.tl"),
        "local leaf = require(\"leaf\")\nlocal record uses_leaf\nend\nfunction uses_leaf.g(): integer\n   return leaf.f()\nend\nreturn uses_leaf\n",
    );
    write(
        &root.join("src/alone.tl"),
        "local record alone\nend\nfunction alone.h(): integer\n   return 2\nend\nreturn alone\n",
    );
    root
}

fn edit_leaf(root: &Path, n: i32) {
    write(
        &root.join("src/leaf.tl"),
        &format!(
            "local record leaf\nend\nfunction leaf.f(): integer\n   return {n}\nend\nreturn leaf\n"
        ),
    );
}

fn check_with(root: &Path, args: &[&str]) -> serde_json::Value {
    let mut a = vec!["check", "src", "--format", "json"];
    a.extend_from_slice(args);
    let (_, stdout, _) = htl(&a, root);
    serde_json::from_str(&stdout).expect("stdout is one JSON document")
}

fn entries(root: &Path) -> usize {
    std::fs::read_dir(root.join(".htl/cache"))
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .count()
        })
        .unwrap_or(0)
}

/// Run with the eviction bound forced low. Reaching a few hundred entries honestly would
/// take more invocations than a test should.
fn check_bounded(root: &Path, bound: usize, args: &[&str]) -> serde_json::Value {
    let mut a = vec!["check", "src", "--format", "json"];
    a.extend_from_slice(args);
    let out = Command::new(common::htl_bin())
        .args(&a)
        .env("HTL_CACHE_MAX_ENTRIES", bound.to_string())
        .current_dir(root)
        .output()
        .unwrap();
    serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("stdout is one JSON document")
}

/// Editing the leaf must cost the leaf and its requirer, and leave the third module alone —
/// the whole point of per-module entries.
#[test]
fn editing_one_module_rechecks_it_and_its_dependents_only() {
    let root = three_modules("deps");
    assert_eq!(
        replayed(&check(&root)),
        0,
        "the first run has nothing to replay"
    );
    assert_eq!(
        replayed(&check(&root)),
        3,
        "the second run replays all three"
    );

    edit_leaf(&root, 2);
    assert_eq!(
        replayed(&check(&root)),
        1,
        "the leaf and the module requiring it are checked again; the third is not"
    );
}

/// `--cache-mode whole-run` grains the cache to the walk instead. One entry covers all of
/// it, so an edit anywhere re-checks everything — which is the behaviour someone picks it
/// for, and the same trade #3 shipped before the entries were split.
///
/// This is a different question from `--no-cache`, which is whether to cache at all.
#[test]
fn whole_run_mode_replays_all_of_the_walk_or_none_of_it() {
    let root = three_modules("wholerun");
    let m = &["--cache-mode", "whole-run"];

    assert_eq!(replayed(&check_with(&root, m)), 0, "nothing stored yet");
    assert_eq!(replayed(&check_with(&root, m)), 3, "the walk replays whole");

    edit_leaf(&root, 3);
    assert_eq!(
        replayed(&check_with(&root, m)),
        0,
        "one edit costs the entire walk here"
    );
    assert_eq!(
        replayed(&check_with(&root, m)),
        3,
        "and it is whole again after that run"
    );
}

/// The mode comes from `[cache] mode`, and `--cache-mode` overrides it. The two granularities
/// key their entries separately, so switching between them does not read the other's.
#[test]
fn the_config_picks_the_mode_and_the_flag_overrides_it() {
    let root = three_modules("mode-config");
    write(&root.join("htl.toml"), "[cache]\nmode = \"whole-run\"\n");

    assert_eq!(replayed(&check(&root)), 0);
    assert_eq!(replayed(&check(&root)), 3, "the config selected whole-run");
    edit_leaf(&root, 4);
    assert_eq!(replayed(&check(&root)), 0, "so one edit costs the walk");

    // The flag overrides it, and starts from its own empty set of entries.
    assert_eq!(
        replayed(&check_with(&root, &["--cache-mode", "per-module"])),
        0
    );
    assert_eq!(
        replayed(&check_with(&root, &["--cache-mode", "per-module"])),
        3
    );
    edit_leaf(&root, 5);
    assert_eq!(
        replayed(&check_with(&root, &["--cache-mode", "per-module"])),
        1,
        "and under per-module the same edit leaves the independent module alone"
    );
}

/// The store used to only grow: every distinct invocation left a full set of module entries
/// and nothing took the old ones away.
#[test]
fn the_store_stays_within_its_bound() {
    let root = three_modules("bound");
    // Three modules, so each invocation leaves three entries. A bound of four forces the
    // second shape to evict most of the first.
    assert_eq!(replayed(&check_bounded(&root, 4, &[])), 0);
    assert_eq!(entries(&root), 3);

    check_bounded(&root, 4, &["--lint", "+no-any"]);
    assert!(
        entries(&root) <= 4,
        "the store must not exceed the bound: {}",
        entries(&root)
    );

    check_bounded(&root, 4, &["--lint", "+class-record"]);
    assert!(
        entries(&root) <= 4,
        "nor after another shape: {}",
        entries(&root)
    );
}

/// What a run just wrote or just read has to survive its own sweep, or a project whose module
/// count is near the bound would evict itself on every run and never hit.
#[test]
fn a_sweep_never_drops_what_the_run_itself_used() {
    let root = three_modules("keep");
    // A bound below the module count: the three entries this run needs are all it may keep.
    assert_eq!(replayed(&check_bounded(&root, 1, &[])), 0);
    assert_eq!(
        replayed(&check_bounded(&root, 1, &[])),
        3,
        "its own entries survived"
    );
    assert_eq!(
        replayed(&check_bounded(&root, 1, &[])),
        3,
        "and keep surviving"
    );
}

/// An entry whose module is gone can never be read again.
#[test]
fn entries_for_deleted_modules_are_dropped() {
    let root = three_modules("orphans");
    assert_eq!(replayed(&check_bounded(&root, 3, &[])), 0);
    assert_eq!(entries(&root), 3);

    std::fs::remove_file(root.join("src/alone.tl")).unwrap();
    // Two modules left, so the bound of 3 is not exceeded by them — but the orphan pushes the
    // store to 3 and it is the one with nothing behind it.
    check_bounded(&root, 2, &[]);
    assert!(entries(&root) <= 2, "the orphan went: {}", entries(&root));
    assert_eq!(
        replayed(&check_bounded(&root, 2, &[])),
        2,
        "and the two that remain still replay"
    );
}

/// `htl cache clear` exists because the store is beside `htl.toml` rather than wherever the
/// caller happens to be standing, so "just delete the directory" is advice that needs a
/// lookup first.
#[test]
fn cache_clear_empties_the_store() {
    let root = three_modules("clear");
    assert_eq!(replayed(&check(&root)), 0);
    assert_eq!(replayed(&check(&root)), 3);
    assert!(entries(&root) > 0);

    let (ok, _, err) = htl(&["cache", "clear"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("removed"), "it says what it did: {err}");
    assert_eq!(entries(&root), 0);
    assert_eq!(
        replayed(&check(&root)),
        0,
        "the next run has nothing to replay"
    );
}

/// Run from a subdirectory, where there is no `.htl/` — the point of the command.
#[test]
fn cache_clear_finds_the_store_from_inside_the_project() {
    let root = three_modules("clear-sub");
    check(&root);
    assert!(entries(&root) > 0);

    let (ok, _, err) = htl(&["cache", "clear"], &root.join("src"));
    assert!(ok, "{err}");
    assert_eq!(
        entries(&root),
        0,
        "it cleared the project's store, not one under src/"
    );
}

/// Clearing a project that never cached anything is not an error.
#[test]
fn cache_clear_on_an_empty_project_says_so_and_succeeds() {
    let root = three_modules("clear-empty");
    let (ok, _, err) = htl(&["cache", "clear"], &root);
    assert!(ok, "{err}");
    assert!(err.contains("nothing stored"), "{err}");
}

/// Whether to cache and how to grain it are separate: `--no-cache` wins over any mode.
#[test]
fn no_cache_beats_the_mode() {
    let root = three_modules("both-axes");
    assert_eq!(
        replayed(&check_with(&root, &["--cache-mode", "whole-run"])),
        0
    );
    assert_eq!(
        replayed(&check_with(&root, &["--cache-mode", "whole-run"])),
        3
    );
    assert_eq!(
        replayed(&check_with(
            &root,
            &["--cache-mode", "whole-run", "--no-cache"]
        )),
        0,
        "--no-cache does not read the entry the mode would have used"
    );
}

/// The project-level cycle lint runs over every file's requires, and a replayed file has to
/// contribute its own — a cycle closing through a module nobody edited is still a cycle.
/// This is the lint most likely to quietly vanish once modules stop being checked.
#[test]
fn the_cycle_lint_still_fires_when_every_file_was_replayed() {
    let root = tempdir("cycle");
    write(&root.join("htl.toml"), "[check]\n");
    write(
        &root.join("src/a.tl"),
        "local b = require(\"b\")\nlocal record a\nend\nreturn a\n",
    );
    write(
        &root.join("src/b.tl"),
        "local a = require(\"a\")\nlocal record b\nend\nreturn b\n",
    );

    let cycles = |v: &serde_json::Value| {
        v["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["rule"].as_str() == Some("require-cycle"))
            .count()
    };

    let first = check(&root);
    assert!(
        cycles(&first) > 0,
        "the fixture must produce a cycle lint or this proves nothing: {first}"
    );

    let second = check(&root);
    assert!(was_cached(&second), "nothing moved, so both modules replay");
    assert_eq!(
        cycles(&first),
        cycles(&second),
        "the cycle lint survives a fully replayed run"
    );
}

/// Output order follows the walk, not the split between what was checked and what was
/// replayed — otherwise a run's diagnostics would shuffle depending on what happened to be
/// in the store.
#[test]
fn diagnostics_come_out_in_file_order_whatever_was_cached() {
    let root = tempdir("order");
    write(&root.join("htl.toml"), "[check]\n");
    // Two modules that each report an error, named so the walk visits aaa before zzz.
    write(&root.join("src/aaa.tl"), "local n: string = 1\nprint(n)\n");
    write(
        &root.join("src/zzz.tl"),
        "local s: integer = \"x\"\nprint(s)\n",
    );

    let files = |v: &serde_json::Value| {
        v["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["file"].as_str().unwrap_or("").to_string())
            .collect::<Vec<_>>()
    };

    let cold = check(&root);
    assert_eq!(replayed(&cold), 0);
    let all_cached = check(&root);
    assert_eq!(replayed(&all_cached), 2);

    // Now a mix: zzz is edited, aaa comes from the store.
    write(
        &root.join("src/zzz.tl"),
        "local s: integer = \"y\"\nprint(s)\n",
    );
    let mixed = check(&root);
    assert_eq!(replayed(&mixed), 1, "aaa replays, zzz is checked");

    assert_eq!(
        files(&cold),
        files(&all_cached),
        "a fully replayed run keeps file order"
    );
    assert_eq!(files(&cold), files(&mixed), "and so does a mixed one");
}

const HOST_DTL: &str = "local record host\n   record Std\n      version: string\n   end\nend\nglobal std: host.Std\nreturn host\n";
const MID_TL: &str = "local h = require(\"host\")\nlocal record M\n   type Std = h.Std\n   ver: function(): string\nend\nfunction M.ver(): string return std.version end\nreturn M\n";
const TOP_TL: &str = "local mid = require(\"mid\")\nlocal M = {}\nfunction M.viaGlobal(): string return std.version end\nfunction M.viaType(s: mid.Std): string return s.version end\nreturn M\n";

/// A module typed by a file two requires away: `top` requires `mid`, `mid` requires the
/// declaration `host`, and `top` reads both `host`'s global and the record `mid`
/// re-exports from it, without ever naming `host`. `dtl` is where the declaration goes
/// (`types/host.d.tl` beside `src/`, or `host.d.tl` in a flat tree).
fn two_requires_away(root: &Path, dtl: &str, src: &str) {
    write(&root.join(dtl), HOST_DTL);
    write(&root.join(src).join("mid.tl"), MID_TL);
    write(&root.join(src).join("top.tl"), TOP_TL);
}

/// `(file name, line, col, message)` of every error a JSON report carries, in its order.
fn errors_of(v: &serde_json::Value) -> Vec<(String, u64, u64, String)> {
    v["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .filter(|d| d["severity"] == "error")
        .map(|d| {
            let file = d["file"].as_str().unwrap_or("");
            (
                file.rsplit('/').next().unwrap_or(file).to_string(),
                d["line"].as_u64().unwrap_or(0),
                d["col"].as_u64().unwrap_or(0),
                d["message"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

/// An edit to a file the module never names, but reads through a require of a require,
/// misses: the entry's inputs are the require closure, not the direct requires. Before,
/// `top.tl` hit on its old entry — `mid.tl` unchanged — and the two errors the edit put
/// into it stayed hidden until `--no-cache`.
#[test]
fn editing_a_declaration_two_requires_away_rechecks_the_module_that_reads_it() {
    let root = tempdir("closure");
    write(&root.join("htl.toml"), "[check]\n");
    two_requires_away(&root, "types/host.d.tl", "src");

    let first = check(&root);
    assert!(errors_of(&first).is_empty(), "{first}");

    write(
        &root.join("types/host.d.tl"),
        &HOST_DTL.replace("version: string", "version: number"),
    );
    let msg = "in return value: got number, expected string";
    let want = vec![
        ("mid.tl".to_string(), 6, 36, msg.to_string()),
        ("top.tl".to_string(), 3, 42, msg.to_string()),
        ("top.tl".to_string(), 4, 48, msg.to_string()),
    ];
    let fresh = check_with(&root, &["--no-cache"]);
    assert_eq!(
        errors_of(&fresh),
        want,
        "the fixture's answer without the store"
    );

    let second = check(&root);
    assert_eq!(
        replayed(&second),
        0,
        "both modules read the declaration, so neither replays"
    );
    assert_eq!(errors_of(&second), want);

    let third = check(&root);
    assert_eq!(
        replayed(&third),
        2,
        "and the answer is then stored for both"
    );
    assert_eq!(errors_of(&third), want);
}

/// The same hole as a false lint: moving the declaration down three lines left `top.tl`
/// replaying the site it had seen (line 6) beside the one `mid.tl` saw after the edit
/// (line 9), and `global-redeclaration` read the two as two declarations.
#[test]
fn shifting_a_declaration_two_requires_away_reports_no_redeclaration() {
    let root = tempdir("closure-lines");
    write(&root.join("htl.toml"), "[layout]\nsource = \".\"\n");
    two_requires_away(&root, "host.d.tl", ".");

    let (ok, _, warm) = htl(&["check", "."], &root);
    assert!(ok, "{warm}");
    write(
        &root.join("host.d.tl"),
        &format!("-- one\n-- two\n-- three\n{HOST_DTL}"),
    );

    // Both modules read the declaration now, so neither replays: the run says what a run
    // without the store says, line for line.
    let (ok, _, after) = htl(&["check", "."], &root);
    assert!(ok, "{after}");
    assert!(!after.contains("global-redeclaration"), "{after}");
    let (_, _, fresh) = htl(&["check", "--no-cache", "."], &root);
    assert_eq!(
        after, "htl check: 2 file(s), 0 error(s), 0 warning(s), 0 lint(s)\n",
        "the run after the edit"
    );
    assert_eq!(after, fresh, "and the run without the store");
}

/// The probe is as deep as the inputs. A file appearing under a name `mid` requires changes
/// what `top` reads through `mid` just as an edit would, while every file either entry
/// hashed is unchanged — so `top`, which never names `leaf`, is checked again too, as `mid`
/// is by its own probe. The rest of `a_module_appearing_under_a_required_name_misses`
/// applies: the checker still picks `src/leaf.tl`, but whether it does is no longer the
/// same question.
#[test]
fn a_module_appearing_under_a_name_a_required_module_requires_misses() {
    let root = tempdir("closure-appear");
    write(&root.join("htl.toml"), "[check]\n");
    write(
        &root.join("src/leaf.tl"),
        "local record leaf\nend\nfunction leaf.f(): integer\n   return 1\nend\nreturn leaf\n",
    );
    write(
        &root.join("src/mid.tl"),
        "local leaf = require(\"leaf\")\nlocal record mid\nend\nfunction mid.g(): integer\n   return leaf.f()\nend\nreturn mid\n",
    );
    write(
        &root.join("src/top.tl"),
        "local mid = require(\"mid\")\nlocal x: integer = mid.g()\nprint(x)\n",
    );
    assert_eq!(replayed(&check(&root)), 0);
    assert_eq!(replayed(&check(&root)), 3);

    write(
        &root.join("types/leaf.d.tl"),
        "local record leaf\n   f: function(): integer\nend\nreturn leaf\n",
    );
    assert_eq!(
        replayed(&check(&root)),
        1,
        "only the leaf replays: nothing it read or asked about moved"
    );
}

/// `mid` requires `gone` and re-exports its record; `top` reads a field of it through `mid`
/// and never names `gone`. The fixture of #410.
const GONE_MID_TL: &str = "local g = require(\"gone\")\nlocal record M\n   type G = g.G\n   make: function(): g.G\nend\nfunction M.make(): g.G return { n = 1 } end\nreturn M\n";
const GONE_TOP_TL: &str = "local mid = require(\"mid\")\nlocal M = {}\nfunction M.n(): integer return mid.make().n end\nreturn M\n";

fn gone_tl(field: &str) -> String {
    format!("local record gone\n   record G\n      n: {field}\n   end\nend\nreturn gone\n")
}

fn check_flat(root: &Path, args: &[&str]) -> serde_json::Value {
    let mut a = vec!["check", ".", "--format", "json"];
    a.extend_from_slice(args);
    let (_, stdout, _) = htl(&a, root);
    serde_json::from_str(&stdout).expect("stdout is one JSON document")
}

/// A name a required module required that resolved nowhere is a question the entry above it
/// asks again. Before, `top`'s entry had edges only for the names below it that resolved,
/// so writing `gone.tl` re-checked `mid` (its own require of `gone`) while `top` hashed an
/// unchanged `mid` and replayed the errors it recorded while `gone` was missing — on a tree
/// `--no-cache` passes, and, after `gone` changed, instead of the error that tree has.
#[test]
fn a_module_appearing_under_a_name_a_required_module_could_not_resolve_misses() {
    let root = tempdir("closure-unresolved");
    write(&root.join("htl.toml"), "[layout]\nsource = \".\"\n");
    write(&root.join("mid.tl"), GONE_MID_TL);
    write(&root.join("top.tl"), GONE_TOP_TL);

    let first = check_flat(&root, &[]);
    let unknown = "unknown type g.G".to_string();
    assert_eq!(
        errors_of(&first),
        vec![
            (
                "mid.tl".to_string(),
                1,
                18,
                "module not found: 'gone'".to_string()
            ),
            ("mid.tl".to_string(), 3, 13, unknown.clone()),
            ("mid.tl".to_string(), 4, 22, unknown.clone()),
            ("mid.tl".to_string(), 6, 20, unknown.clone()),
            (
                "top.tl".to_string(),
                3,
                43,
                "cannot index key 'n' in type g.G".to_string()
            ),
            ("mid.tl".to_string(), 4, 22, unknown.clone()),
        ],
        "{first}"
    );

    write(&root.join("gone.tl"), &gone_tl("integer"));
    let second = check_flat(&root, &[]);
    assert_eq!(
        replayed(&second),
        0,
        "`top` asks about `gone` from `mid`, so it does not replay"
    );
    assert!(errors_of(&second).is_empty(), "{second}");

    write(&root.join("gone.tl"), &gone_tl("string"));
    let want = vec![
        (
            "mid.tl".to_string(),
            6,
            37,
            "in record field: n: got integer, expected string".to_string(),
        ),
        (
            "top.tl".to_string(),
            3,
            42,
            "in return value: got string, expected integer".to_string(),
        ),
    ];
    let fresh = check_flat(&root, &["--no-cache"]);
    assert_eq!(
        errors_of(&fresh),
        want,
        "the fixture's answer without the store"
    );
    let third = check_flat(&root, &[]);
    assert_eq!(replayed(&third), 0, "every module read `gone.tl`");
    assert_eq!(errors_of(&third), want);

    let fourth = check_flat(&root, &[]);
    assert_eq!(
        replayed(&fourth),
        3,
        "and the answer is then stored for all three"
    );
    assert_eq!(errors_of(&fourth), want);
}
