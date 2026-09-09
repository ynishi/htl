//! `htl::testing::run_tests`: a project's Teal tests run from Rust, the way a host would
//! put them in `cargo test` instead of shelling out to `htl test`.
//!
//! The entry point is the project layer's, so it asks for the project layer's features
//! ([`htl_core::project`]); the umbrella `htl` crate a host depends on has both.
#![cfg(all(feature = "pkg", feature = "dts"))]

use htl_core::testing::{RunOptions, Suite, run_tests};
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-suite", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A project with a module, a suite that passes over it, and one that does not.
fn project(dir: &Path) {
    write(
        &dir.join("src").join("purse.tl"),
        "local M = {}\n\
         function M.add(a: integer, b: integer): integer\n   return a + b\nend\n\
         return M\n",
    );
    write(
        &dir.join("tests").join("purse_test.tl"),
        "local t = require(\"htl.test\")\n\
         local purse = require(\"purse\")\n\
         t.describe(\"purse\", function()\n\
            t.it(\"adds\", function()\n\
               t.expect(purse.add(2, 3)):to_equal(5)\n\
            end)\n\
            t.it(\"is seeded\", function()\n\
               t.expect(math.random(1, 6) >= 1):to_equal(true)\n\
            end)\n\
         end)\n",
    );
}

#[test]
fn a_host_runs_a_projects_tests_and_reads_the_report() {
    let dir = scratch("passes");
    project(&dir);

    let rep = run_tests(std::slice::from_ref(&dir), &Suite::default()).unwrap();

    assert!(rep.ok(), "{:?}", rep.failures());
    assert_eq!(rep.files.len(), 1, "one test file discovered and run");
    assert_eq!((rep.passed, rep.failed), (2, 0));
    assert_eq!(rep.files_with_errors, 0);
    assert_eq!(rep.skipped, 0);
    assert!(rep.coverage.is_none(), "not asked for");
    // Every file's own report is there to assert on, not just the totals.
    let f = &rep.files[0];
    assert!(f.check.ok(), "{:?}", f.check.errors);
    assert_eq!(
        f.tests.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ["purse > adds", "purse > is seeded"]
    );
}

#[test]
fn a_failing_test_is_a_failing_report_with_the_reason_in_it() {
    let dir = scratch("fails");
    project(&dir);
    write(
        &dir.join("tests").join("broken_test.tl"),
        "local t = require(\"htl.test\")\n\
         local purse = require(\"purse\")\n\
         t.describe(\"purse\", function()\n\
            t.it(\"does not add like that\", function()\n\
               t.expect(purse.add(2, 3)):to_equal(6)\n\
            end)\n\
         end)\n",
    );

    let rep = run_tests(std::slice::from_ref(&dir), &Suite::default()).unwrap();

    assert!(!rep.ok());
    assert_eq!(rep.files_with_errors, 1);
    assert_eq!((rep.passed, rep.failed), (2, 1));
    let why = rep.failures().join("\n");
    assert!(why.contains("broken_test.tl"), "{why}");
    assert!(why.contains('6'), "the expected value is in it: {why}");
}

#[test]
fn the_filter_and_the_seed_are_the_flags_they_are_on_the_command_line() {
    let dir = scratch("filter-seed");
    project(&dir);

    let suite = Suite {
        filter: Some("adds".to_string()),
        run: RunOptions {
            seed: Some(7),
            ..Default::default()
        },
        ..Default::default()
    };
    let rep = run_tests(std::slice::from_ref(&dir), &suite).unwrap();

    assert!(rep.ok(), "{:?}", rep.failures());
    assert_eq!(
        (rep.passed, rep.failed),
        (1, 0),
        "the other test filtered out"
    );
    assert_eq!(rep.seed, 7, "the seed the caller gave, back for repeating");

    // Not given: drawn for the run and reported, so the run can be repeated.
    let drawn = run_tests(&[dir], &Suite::default()).unwrap();
    assert!(drawn.ok(), "{:?}", drawn.failures());
    assert_ne!(drawn.seed, 0, "a seed was drawn and said");
}

#[test]
fn coverage_is_reported_over_the_modules_the_tests_reached() {
    let dir = scratch("coverage");
    project(&dir);

    let suite = Suite {
        run: RunOptions {
            coverage: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let rep = run_tests(&[dir], &suite).unwrap();

    assert!(rep.ok(), "{:?}", rep.failures());
    let cov = rep.coverage.expect("asked for");
    assert!(cov.total > 0, "statements were counted");
    assert!(
        cov.modules.iter().any(|m| m.path.ends_with("purse.tl")),
        "the module the tests required is in the report: {:?}",
        cov.modules.iter().map(|m| &m.path).collect::<Vec<_>>()
    );
    // And it is the same value the lcov writer works from.
    assert!(cov.lcov(Path::new("/")).contains("purse.tl"));
}
