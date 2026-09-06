//! `htl test --seed`: the run's stream, through the real binary.
//!
//! A test that draws randomness is only worth writing if a failure can be looked at
//! again, which means the seed has to be printed and accepted back.

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "htl-cli-seed-{name}-{}-{}",
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
    let out = Command::new(env!("CARGO_BIN_EXE_htl"))
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

/// Two files, each printing one draw, so a run's values can be compared with a run of
/// one file on its own.
fn project() -> PathBuf {
    let root = scratch("proj");
    write(
        &root.join("tests/a_test.tl"),
        "local t = require(\"htl.test\")\n\
         t.it(\"a\", function()\n   print(\"a=\" .. tostring(t.rng()(1, 1000000)))\n   t.expect(1):to_equal(1)\nend)\n",
    );
    write(
        &root.join("tests/b_test.tl"),
        "local t = require(\"htl.test\")\n\
         t.it(\"b\", function()\n   print(\"b=\" .. tostring(t.rng()(1, 1000000)))\n   t.expect(1):to_equal(1)\nend)\n",
    );
    root
}

/// The values printed by a run, as `a=…` / `b=…` lines in order.
fn draws(out: &str) -> Vec<String> {
    out.lines()
        .filter(|l| l.starts_with("a=") || l.starts_with("b="))
        .map(|l| l.to_string())
        .collect()
}

#[test]
fn the_same_seed_draws_the_same_values() {
    let root = project();
    let (ok, out1, err1) = htl(&["test", "tests", "--seed", "42"], &root);
    assert!(ok, "{err1}");
    let (_, out2, _) = htl(&["test", "tests", "--seed", "42"], &root);
    let (first, second) = (draws(&out1), draws(&out2));
    assert_eq!(first.len(), 2, "both files drew: {out1}");
    assert_eq!(first, second, "same seed, same values");

    let (_, out3, _) = htl(&["test", "tests", "--seed", "43"], &root);
    assert_ne!(first, draws(&out3), "a different seed draws differently");
}

/// The point of deriving each file's seed from its path: one file on its own draws what
/// it drew in the full run, so a failure can be looked at without the rest.
#[test]
fn one_file_alone_draws_what_it_drew_in_the_run() {
    let root = project();
    let (_, whole, _) = htl(&["test", "tests", "--seed", "7"], &root);
    let (_, alone, _) = htl(&["test", "tests/b_test.tl", "--seed", "7"], &root);
    let b = draws(&whole).into_iter().find(|l| l.starts_with("b="));
    assert_eq!(b, draws(&alone).into_iter().next(), "{whole}\n{alone}");
}

#[test]
fn the_seed_is_printed_every_run_and_differs_when_not_given() {
    let root = project();
    let (_, _, err1) = htl(&["test", "tests"], &root);
    let (_, _, err2) = htl(&["test", "tests"], &root);
    let line = |e: &str| {
        e.lines()
            .find(|l| l.contains("htl test: seed "))
            .map(|l| l.to_string())
            .unwrap_or_default()
    };
    let (l1, l2) = (line(&err1), line(&err2));
    assert!(l1.contains("repeat with --seed"), "{err1}");
    assert_ne!(l1, l2, "an unseeded run picks a new one each time");
}

#[test]
fn json_carries_the_seed() {
    // Its own project: `--format json` puts one document on stdout, and the fixture above
    // prints its draws there.
    let root = scratch("json");
    write(
        &root.join("tests/quiet_test.tl"),
        "local t = require(\"htl.test\")\n\
         t.it(\"quiet\", function()\n   t.expect(t.rng()(1, 10) >= 1):to_equal(true)\nend)\n",
    );
    let (_, stdout, _) = htl(&["test", "tests", "--format", "json", "--seed", "5"], &root);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("one JSON document");
    assert_eq!(v["summary"]["seed"], 5);
}
