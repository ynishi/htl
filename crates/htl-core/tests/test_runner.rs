//! `htl.test` matchers, `expect_all`, per-test timing and fail-fast through the runner.

use htl_core::testing::{RunOptions, run_test_file};
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "htl-core-runner-{name}-{}-{}",
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

#[test]
fn matchers_and_expect_all_are_typed_and_run() {
    let dir = scratch("matchers");
    write(
        &dir.join("m_test.tl"),
        "local t = require(\"htl.test\")\n\
         local function valid(n: integer): boolean, string\n   if n > 0 then return true, \"\" end\n   return false, \"no door\"\nend\n\
         t.describe(\"matchers\", function()\n\
            t.it(\"numbers\", function()\n\
               t.expect(3):to_be_greater_than(2)\n\
               t.expect(3):to_be_less_than(4)\n\
               t.expect(3):to_be_at_least(3)\n\
               t.expect(3):to_be_at_most(3)\n\
            end)\n\
            t.it(\"strings and arrays\", function()\n\
               t.expect(\"not enough mana\"):to_contain(\"mana\")\n\
               t.expect({ \"a\", \"b\" }):to_contain(\"b\")\n\
               t.expect(\"costs 12 gold\"):to_match(\"%d+ gold\")\n\
               t.expect({ 1, 2, 3 }):to_have_length(3)\n\
            end)\n\
            t.it(\"two values\", function()\n\
               t.expect_all(valid(0)):to_equal(false, \"no door\")\n\
               t.expect_all(valid(1)):to_equal(true, \"\")\n\
            end)\n\
            t.it(\"failing matcher names both sides\", function()\n\
               t.expect(1):to_be_greater_than(2)\n\
            end)\n\
            t.it(\"failing expect_all shows both values\", function()\n\
               t.expect_all(valid(0)):to_equal(false, \"locked\")\n\
            end)\n\
         end)\n",
    );
    let rep = run_test_file(&dir.join("m_test.tl"), None, "htl.test", None, &RunOptions::default()).unwrap();
    assert!(rep.check.ok(), "{:?}", rep.check.errors);
    assert_eq!((rep.passed, rep.failed), (3, 2), "{:?}", rep.failures);
    assert!(rep.failures[0].contains("greater than 2, got 1"), "{}", rep.failures[0]);
    assert!(rep.failures[1].contains("expected (false, \"locked\"), got (false, \"no door\")"), "{}", rep.failures[1]);
    assert_eq!(rep.tests.len(), 5);
    assert!(rep.tests.iter().all(|t| t.ms >= 0.0));
    assert!(rep.tests[0].ok && !rep.tests[3].ok);
    assert!(rep.duration_ms > 0.0);
}

/// The three negations that had no opposite matcher; each fails with its own message.
#[test]
fn negated_matchers_pass_and_fail_with_specific_messages() {
    let dir = scratch("negated");
    write(
        &dir.join("n_test.tl"),
        "local t = require(\"htl.test\")\n\
         t.it(\"holds\", function()\n\
            t.expect(\"closing on the bat\"):to_not_contain(\"fetching\")\n\
            t.expect({ \"a\", \"b\" }):to_not_contain(\"c\")\n\
            t.expect(\"costs 12 gold\"):to_not_match(\"%d+ mana\")\n\
            t.expect(1):to_not_be_nil()\n\
         end)\n\
         t.it(\"string contains\", function() t.expect(\"closing on the bat\"):to_not_contain(\"bat\") end)\n\
         t.it(\"array contains\", function() t.expect({ \"a\", \"b\" }):to_not_contain(\"b\") end)\n\
         t.it(\"matches\", function() t.expect(\"costs 12 gold\"):to_not_match(\"%d+ gold\") end)\n\
         t.it(\"is nil\", function() t.expect(nil):to_not_be_nil() end)\n",
    );
    let rep = run_test_file(&dir.join("n_test.tl"), None, "htl.test", None, &RunOptions::default()).unwrap();
    assert!(rep.check.ok(), "{:?}", rep.check.errors);
    assert_eq!((rep.passed, rep.failed), (1, 4), "{:?}", rep.failures);
    assert!(rep.failures[0].contains("expected \"closing on the bat\" not to contain \"bat\""), "{}", rep.failures[0]);
    assert!(rep.failures[1].contains("not to contain \"b\" (found at index 2)"), "{}", rep.failures[1]);
    assert!(rep.failures[2].contains("not to match /%d+ gold/ (matched \"12 gold\" at 7)"), "{}", rep.failures[2]);
    assert!(rep.failures[3].contains("expected a value, got nil"), "{}", rep.failures[3]);
}

#[test]
fn fail_fast_stops_after_the_first_failure_in_a_file() {
    let dir = scratch("failfast");
    write(
        &dir.join("f_test.tl"),
        "local t = require(\"htl.test\")\n\
         t.it(\"first\", function() t.expect(1):to_equal(1) end)\n\
         t.it(\"second\", function() t.expect(1):to_equal(2) end)\n\
         t.it(\"third\", function() t.expect(1):to_equal(3) end)\n",
    );
    let all = run_test_file(&dir.join("f_test.tl"), None, "htl.test", None, &RunOptions::default()).unwrap();
    assert_eq!((all.passed, all.failed), (1, 2));
    let fast = run_test_file(&dir.join("f_test.tl"), None, "htl.test", None, &RunOptions { fail_fast: true, ..Default::default() }).unwrap();
    assert_eq!((fast.passed, fast.failed), (1, 1), "{:?}", fast.failures);
    assert_eq!(fast.tests.len(), 2, "third must not have run");
}

#[test]
fn session_shares_the_checker_but_not_program_state() {
    use htl_core::testing::TestSession;
    let dir = scratch("session");
    // Module with state: a counter that survives only within one program state.
    write(
        &dir.join("src/counter.tl"),
        "local record counter\n   n: integer\nend\ncounter.n = 0\nfunction counter.bump(): integer\n   counter.n = counter.n + 1\n   return counter.n\nend\nreturn counter\n",
    );
    write(
        &dir.join("tests/a_test.tl"),
        "local t = require(\"htl.test\")\nlocal counter = require(\"counter\")\n\
         t.it(\"bumps from a fresh module\", function()\n   t.expect(counter.bump()):to_equal(1)\n   t.expect(counter.bump()):to_equal(2)\nend)\n\
         global LEAK: integer = 42\n",
    );
    write(
        &dir.join("tests/b_test.tl"),
        "local t = require(\"htl.test\")\nlocal counter = require(\"counter\")\n\
         local g = _G as {string: any}\n\
         t.it(\"sees a fresh module and no globals from a\", function()\n   t.expect(counter.bump()):to_equal(1)\n   t.expect(g[\"LEAK\"]):to_be_nil()\nend)\n",
    );
    let session = TestSession::new(None, "htl.test", None, RunOptions::default()).unwrap();
    let a = session.run_file(&dir.join("tests/a_test.tl")).unwrap();
    assert!(a.check.ok() && a.error.is_none() && a.failed == 0, "{:?} {:?} {:?}", a.check.errors, a.error, a.failures);
    let b = session.run_file(&dir.join("tests/b_test.tl")).unwrap();
    assert!(b.check.ok() && b.error.is_none(), "{:?} {:?}", b.check.errors, b.error);
    assert_eq!((b.passed, b.failed), (1, 0), "{:?}", b.failures);

    // A type error in a file is still that file's, and a module with a type error
    // still fails at require in a later file.
    write(&dir.join("tests/c_test.tl"), "local t = require(\"htl.test\")\nlocal n: integer = \"x\"\nprint(n)\n");
    let c = session.run_file(&dir.join("tests/c_test.tl")).unwrap();
    assert!(!c.check.ok());
    write(&dir.join("src/bad.tl"), "local record bad\nend\nfunction bad.f(): integer\n   return \"s\"\nend\nreturn bad\n");
    write(&dir.join("tests/d_test.tl"), "local t = require(\"htl.test\")\nlocal bad = require(\"bad\")\nt.it(\"x\", function() t.expect(bad.f()):to_equal(1) end)\n");
    let d = session.run_file(&dir.join("tests/d_test.tl")).unwrap();
    let msg = format!("{:?} {:?}", d.check.errors, d.error);
    assert!(msg.contains("expected integer"), "bad's own error must surface: {msg}");
}

#[test]
fn snapshots_write_compare_diff_and_update() {
    use htl_core::testing::{TestSession, snapshot_dir};
    let dir = scratch("snap");
    let file = dir.join("tests").join("screen_test.tl");
    let src = |title: &str| {
        format!(
            "local t = require(\"htl.test\")\n\
             t.it(\"frame\", function()\n   t.expect({{ \"{title}\", \"hp 24/24\", \"@..#\" }}):to_match_snapshot(\"first floor\")\nend)\n\
             t.it(\"record\", function()\n   t.expect({{ name = \"x\", hp = 3, tags = {{ \"a\", \"b\" }} }}):to_match_snapshot(\"a record\")\nend)\n"
        )
    };
    write(&file, &src("Depth 1"));
    let run = |update: bool| {
        TestSession::new(None, "htl.test", None, RunOptions { update_snapshots: update, ..Default::default() })
            .unwrap()
            .run_file(&file)
            .unwrap()
    };

    // First run: written, and reported as such.
    let r1 = run(false);
    assert!(r1.check.ok() && r1.failed == 0, "{:?} {:?}", r1.check.errors, r1.failures);
    assert_eq!(r1.snapshots_written.len(), 2, "{:?}", r1.snapshots_written);
    let sdir = snapshot_dir(&file);
    assert!(sdir.ends_with("tests/__snapshots__/screen_test"), "{}", sdir.display());
    let frame = std::fs::read_to_string(sdir.join("first_floor.snap")).unwrap();
    assert_eq!(frame, "Depth 1\nhp 24/24\n@..#\n", "array of strings = lines");
    let rec = std::fs::read_to_string(sdir.join("a_record.snap")).unwrap();
    assert_eq!(rec, "{\n  hp = 3,\n  name = \"x\",\n  tags = {\n    [1] = \"a\",\n    [2] = \"b\",\n  },\n}\n", "sorted, one entry per line");

    // Second run: same value, nothing written.
    let r2 = run(false);
    assert_eq!((r2.failed, r2.snapshots_written.len()), (0, 0), "{:?}", r2.failures);

    // Changed value: fails with a line diff naming the file.
    write(&file, &src("Depth 2"));
    let r3 = run(false);
    assert_eq!(r3.failed, 1, "{:?}", r3.failures);
    let msg = &r3.failures[0];
    assert!(msg.contains("snapshot 'first floor' differs from"), "{msg}");
    assert!(msg.contains("-Depth 1") && msg.contains("+Depth 2"), "{msg}");
    assert!(msg.contains("--update"), "{msg}");

    // --update accepts it and reports the rewrite; the next plain run is green.
    let r4 = run(true);
    assert_eq!((r4.failed, r4.snapshots_updated.len()), (0, 1), "{:?}", r4.failures);
    assert!(std::fs::read_to_string(sdir.join("first_floor.snap")).unwrap().starts_with("Depth 2\n"));
    let r5 = run(false);
    assert_eq!(r5.failed, 0, "{:?}", r5.failures);

    // The same name twice in one file is an error, not a silent overwrite.
    write(
        &file,
        "local t = require(\"htl.test\")\nt.it(\"a\", function() t.expect(\"x\"):to_match_snapshot(\"dup\") end)\n\
         t.it(\"b\", function() t.expect(\"y\"):to_match_snapshot(\"dup\") end)\n",
    );
    let r6 = run(false);
    assert_eq!(r6.failed, 1, "{:?}", r6.failures);
    assert!(r6.failures[0].contains("used twice"), "{}", r6.failures[0]);
}

#[test]
fn a_library_without_tests_field_still_reports() {
    // The runner contract only requires passed / failed / failures.
    let dir = scratch("minimal-lib");
    write(
        &dir.join("mini.lua"),
        "local M = { n = 0 }\nfunction M.check(b) M.n = M.n + 1 M.ok = (M.ok == nil or M.ok) and b end\n\
         function M.run() return { passed = M.ok and M.n or 0, failed = M.ok and 0 or 1, failures = M.ok and {} or { \"x\" } } end\nreturn M\n",
    );
    write(&dir.join("mini.d.tl"), "local record mini\n   check: function(boolean)\nend\nreturn mini\n");
    write(&dir.join("k_test.tl"), "local m = require(\"mini\")\nm.check(1 == 1)\nm.check(2 == 2)\n");
    let rep = run_test_file(&dir.join("k_test.tl"), None, "mini", None, &RunOptions::default()).unwrap();
    assert!(rep.check.ok(), "{:?}", rep.check.errors);
    assert_eq!((rep.passed, rep.failed), (2, 0), "{:?}", rep.failures);
    assert!(rep.tests.is_empty());
}
