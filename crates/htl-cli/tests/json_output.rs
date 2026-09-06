//! `htl check --format json` / `htl test --format json` through the real binary.

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "htl-cli-json-{name}-{}-{}",
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

fn project() -> PathBuf {
    let root = scratch("proj");
    write(
        &root.join("src/util.tl"),
        "local record util\nend\nfunction util.twice(n: integer): integer\n   return n * 2\nend\n\
         function util.half(n: integer): integer\n   if n < 0 then\n      return 0\n   end\n   return n // 2\nend\nreturn util\n",
    );
    // A type error and a lint (nil-index) in one file.
    write(&root.join("src/bad.tl"), "local t: {string: {integer}} = {}\nlocal n: string = t[\"a\"][1]\nprint(n)\n");
    write(
        &root.join("tests/util_test.tl"),
        "local t = require(\"htl.test\")\nlocal util = require(\"util\")\n\
         t.it(\"twice\", function() t.expect(util.twice(2)):to_equal(4) end)\n\
         t.it(\"half fails\", function() t.expect(util.half(4)):to_equal(3) end)\n",
    );
    root
}

#[test]
fn check_json_lists_diagnostics_with_parts_and_summary() {
    let root = project();
    let (ok, stdout, stderr) = htl(&["check", "src", "--format", "json"], &root);
    assert!(!ok, "type error must fail the check");
    assert!(stderr.trim().is_empty(), "json mode keeps stderr silent: {stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is one JSON document");
    assert_eq!(v["files"], 2);
    assert_eq!(v["summary"]["errors"], 1);
    assert_eq!(v["summary"]["lints"], 1);
    assert_eq!(v["summary"]["ok"], false);
    let diags = v["diagnostics"].as_array().unwrap();
    let err = diags.iter().find(|d| d["severity"] == "error").unwrap();
    assert!(err["file"].as_str().unwrap().ends_with("bad.tl"), "{err}");
    assert_eq!(err["line"], 2);
    assert!(err["col"].as_u64().unwrap() > 0);
    assert!(err["message"].as_str().unwrap().contains("expected string"), "{err}");
    let lint = diags.iter().find(|d| d["severity"] == "lint").unwrap();
    assert_eq!(lint["rule"], "nil-index");
    assert!(!lint["message"].as_str().unwrap().contains("[htl"), "rule is split out of the message: {lint}");
}

#[test]
fn test_json_carries_files_tests_failures_and_coverage() {
    let root = project();
    let (ok, stdout, stderr) = htl(&["test", "tests", "--format", "json", "--coverage"], &root);
    assert!(!ok, "one failing test");
    assert!(stderr.trim().is_empty(), "{stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is one JSON document");
    assert_eq!(v["summary"]["files"], 1);
    assert_eq!(v["summary"]["passed"], 1);
    assert_eq!(v["summary"]["failed"], 1);
    assert_eq!(v["summary"]["ok"], false);
    let f = &v["files"][0];
    assert!(f["path"].as_str().unwrap().ends_with("util_test.tl"));
    assert_eq!(f["ok"], false);
    assert_eq!(f["tests"].as_array().unwrap().len(), 2);
    assert_eq!(f["tests"][1]["ok"], false);
    assert!(f["tests"][0]["ms"].is_number());
    assert!(f["failures"][0].as_str().unwrap().contains("expected 3, got 2"), "{}", f["failures"][0]);
    let cov = &v["coverage"];
    let util = cov["modules"].as_array().unwrap().iter().find(|m| m["path"].as_str().unwrap().ends_with("util.tl")).unwrap();
    assert!(util["executed"].as_u64().unwrap() < util["total"].as_u64().unwrap(), "the n < 0 branch did not run: {util}");
    assert!(util["unexecuted"].as_array().unwrap().iter().any(|r| r[0] == 8), "{util}");
    assert!(cov["total"].as_u64().unwrap() > 0);
}

#[test]
fn text_format_is_unchanged_by_default() {
    let root = project();
    let (ok, stdout, stderr) = htl(&["check", "src"], &root);
    assert!(!ok);
    assert!(stdout.trim().is_empty(), "text mode prints nothing on stdout: {stdout}");
    assert!(stderr.contains("error: ") && stderr.contains("htl check: 2 file(s)"), "{stderr}");
}
