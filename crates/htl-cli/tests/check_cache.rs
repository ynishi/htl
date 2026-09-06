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

    // Everything but the flag that says where it came from.
    v1["summary"]["cached"] = false.into();
    v2["summary"]["cached"] = false.into();
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

#[test]
fn a_flag_that_changes_the_report_gets_its_own_entry() {
    let root = project("flags");
    assert!(!was_cached(&check(&root)));
    let (_, stdout, _) = htl(&["check", "src", "--format", "json", "--strict"], &root);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(!was_cached(&v), "--strict asks a different question and cannot reuse the answer");
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
