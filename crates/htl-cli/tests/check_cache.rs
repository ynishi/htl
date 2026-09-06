//! The run cache (`src/cache.rs`) through the real binary.
//!
//! Two properties, and the second is the one that needs the tests. A cached run has to be
//! indistinguishable from the run it stands in for, apart from saying that it was cached
//! — and it has to stop being used the moment anything that fed it moves. A cache that
//! never invalidates passes an "output is identical" test perfectly while being wrong, so
//! most of what is below makes something change and insists on a miss.

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "htl-cli-cache-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn htl(args: &[&str], cwd: &Path) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_htl")).args(args).current_dir(cwd).output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A project that produces diagnostics: replaying an empty report proves nothing, so the
/// fixture carries a type error and a lint alongside a module that is fine.
fn project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("htl.toml"), "[check]\n");
    write(
        &root.join("src/util.tl"),
        "local record util\nend\nfunction util.twice(n: integer): integer\n   return n * 2\nend\nreturn util\n",
    );
    write(&root.join("src/bad.tl"), "local t: {string: {integer}} = {}\nlocal n: string = t[\"a\"][1]\nprint(n)\n");
    root
}

fn check(root: &Path) -> serde_json::Value {
    let (_, stdout, _) = htl(&["check", "src", "--format", "json"], root);
    serde_json::from_str(&stdout).expect("stdout is one JSON document")
}

fn was_cached(v: &serde_json::Value) -> bool {
    v["summary"]["cached"].as_bool().expect("summary carries `cached`")
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
    assert_eq!(v1, v2, "a cached run reports what the run it replaces reported");
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
        s.lines().filter(|l| !l.starts_with("htl check:")).collect::<Vec<_>>().join("\n")
    };
    assert!(!diagnostics(&err1).is_empty(), "the fixture must print diagnostics");
    assert_eq!(diagnostics(&err1), diagnostics(&err2), "the diagnostics a replay prints are the stored ones");
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
    assert!(!was_cached(&check(&root)), "an edited module must be checked again");
}

#[test]
fn a_module_appearing_on_the_search_path_misses() {
    // The case a cache keyed only on the files it read gets wrong, and the one ccache
    // documents as its direct-mode hole: nothing already recorded changed, and `types/`
    // did not exist when the entry was written, but a `require` would now resolve there.
    let root = project("appear");
    assert!(!was_cached(&check(&root)));
    assert!(was_cached(&check(&root)));
    write(&root.join("types/extra.d.tl"), "local record extra\n   version: string\nend\nreturn extra\n");
    assert!(!was_cached(&check(&root)), "a new module where the checker searches must be checked again");
}

#[test]
fn changing_the_config_misses() {
    let root = project("config");
    assert!(!was_cached(&check(&root)));
    assert!(was_cached(&check(&root)));
    write(&root.join("htl.toml"), "[lint]\ndisable = [\"nil-index\"]\n");
    assert!(!was_cached(&check(&root)), "the config shapes the report and is part of the inputs");
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
    assert!(was_cached(&v), "--strict changes the verdict, not the diagnostics: {v}");

    let (_, stdout, _) = htl(&["check", "src", "--format", "json", "--lint", "+no-any"], &root);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(!was_cached(&v), "a different lint selection does change them: {v}");
}

#[test]
fn a_truncated_entry_is_a_miss_rather_than_a_crash() {
    let root = project("corrupt");
    assert!(!was_cached(&check(&root)));
    let entry = std::fs::read_dir(root.join(".htl/cache")).unwrap().next().unwrap().unwrap().path();
    std::fs::write(&entry, "{\"stamp\":{\"format\":1,\"htl\":\"0.1.0\"").unwrap();
    let v = check(&root);
    assert!(!was_cached(&v), "a half-written entry must be ignored");
    assert!(!v["diagnostics"].as_array().unwrap().is_empty(), "and the run must happen normally");
}

#[test]
fn no_cache_neither_writes_nor_reads() {
    let root = project("off");
    htl(&["check", "src", "--format", "json", "--no-cache"], &root);
    assert!(!root.join(".htl").exists(), "--no-cache must not create a store");

    assert!(!was_cached(&check(&root)));
    assert!(was_cached(&check(&root)));
    let (_, stdout, _) = htl(&["check", "src", "--format", "json", "--no-cache"], &root);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(!was_cached(&v), "--no-cache must not read an entry that is sitting there");
}

fn replayed(v: &serde_json::Value) -> u64 {
    v["summary"]["replayed"].as_u64().expect("summary carries `replayed`")
}

/// Three modules, one of which nothing depends on. Editing the leaf must cost the leaf and
/// its requirer, and leave the third alone — the whole point of per-module entries.
#[test]
fn editing_one_module_rechecks_it_and_its_dependents_only() {
    let root = scratch("deps");
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

    assert_eq!(replayed(&check(&root)), 0, "the first run has nothing to replay");
    assert_eq!(replayed(&check(&root)), 3, "the second run replays all three");

    write(
        &root.join("src/leaf.tl"),
        "local record leaf\nend\nfunction leaf.f(): integer\n   return 2\nend\nreturn leaf\n",
    );
    assert_eq!(
        replayed(&check(&root)),
        1,
        "the leaf and the module requiring it are checked again; the third is not"
    );
}

/// The project-level cycle lint runs over every file's requires, and a replayed file has to
/// contribute its own — a cycle closing through a module nobody edited is still a cycle.
/// This is the lint most likely to quietly vanish once modules stop being checked.
#[test]
fn the_cycle_lint_still_fires_when_every_file_was_replayed() {
    let root = scratch("cycle");
    write(&root.join("htl.toml"), "[check]\n");
    write(&root.join("src/a.tl"), "local b = require(\"b\")\nlocal record a\nend\nreturn a\n");
    write(&root.join("src/b.tl"), "local a = require(\"a\")\nlocal record b\nend\nreturn b\n");

    let cycles = |v: &serde_json::Value| {
        v["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["rule"].as_str() == Some("require-cycle"))
            .count()
    };

    let first = check(&root);
    assert!(cycles(&first) > 0, "the fixture must produce a cycle lint or this proves nothing: {first}");

    let second = check(&root);
    assert!(was_cached(&second), "nothing moved, so both modules replay");
    assert_eq!(cycles(&first), cycles(&second), "the cycle lint survives a fully replayed run");
}

/// Output order follows the walk, not the split between what was checked and what was
/// replayed — otherwise a run's diagnostics would shuffle depending on what happened to be
/// in the store.
#[test]
fn diagnostics_come_out_in_file_order_whatever_was_cached() {
    let root = scratch("order");
    write(&root.join("htl.toml"), "[check]\n");
    // Two modules that each report an error, named so the walk visits aaa before zzz.
    write(&root.join("src/aaa.tl"), "local n: string = 1\nprint(n)\n");
    write(&root.join("src/zzz.tl"), "local s: integer = \"x\"\nprint(s)\n");

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
    write(&root.join("src/zzz.tl"), "local s: integer = \"y\"\nprint(s)\n");
    let mixed = check(&root);
    assert_eq!(replayed(&mixed), 1, "aaa replays, zzz is checked");

    assert_eq!(files(&cold), files(&all_cached), "a fully replayed run keeps file order");
    assert_eq!(files(&cold), files(&mixed), "and so does a mixed one");
}
