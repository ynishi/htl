//! What `htl run` and `htl test` print when a program fails two frames down.
//!
//! The frames are the information a reader who has only the output cannot reconstruct,
//! so a development command shows them by default. These pin that they are there, that
//! they name `.tl` files and Teal lines, and that the JSON document carries the same.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-traceback", name)
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

/// `depth.field` is line 8 and `depth.describe` is line 12: a failure inside the first,
/// reached through the second, has two frames to show above the entry script's own.
const DEPTH: &str = "local record depth\n\
                     \x20  record Cfg\n\
                     \x20     name: string\n\
                     \x20  end\n\
                     end\n\
                     \n\
                     function depth.field(c: depth.Cfg): string\n\
                     \x20  return c.name\n\
                     end\n\
                     \n\
                     function depth.describe(c: depth.Cfg): string\n\
                     \x20  return \"cfg \" .. depth.field(c)\n\
                     end\n\
                     \n\
                     return depth\n";

fn project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("depth.tl"), DEPTH);
    write(
        &root.join("boom.tl"),
        "local depth = require(\"depth\")\n\nprint(depth.describe(nil as depth.Cfg))\n",
    );
    root
}

#[test]
fn run_shows_every_frame_that_reached_the_failure() {
    let root = project("run");
    let (ok, _out, err) = htl(&["run", "boom.tl"], &root);
    assert!(!ok, "the script raises: {err}");
    assert!(
        err.contains("depth.tl:8: attempt to index a nil value (local 'c')"),
        "the innermost cause names the Teal line: {err}"
    );
    assert!(err.contains("stack traceback:"), "{err}");
    assert!(
        err.contains("depth.tl:12: in function 'depth.describe'"),
        "the caller two frames down is a Teal file and a Teal line: {err}"
    );
    assert!(
        err.contains("boom.tl:3: in main chunk"),
        "and so is the entry script: {err}"
    );
}

#[test]
fn test_shows_the_frames_of_a_failing_test() {
    let root = project("test");
    write(
        &root.join("depth_test.tl"),
        "local t = require(\"htl.test\")\nlocal depth = require(\"depth\")\n\
         t.it(\"describes\", function()\n\
         \x20  t.expect(depth.describe(nil as depth.Cfg)):to_equal(\"cfg x\")\n\
         end)\n",
    );
    let (ok, _out, err) = htl(&["test", "depth_test.tl"], &root);
    assert!(!ok, "the test raises: {err}");
    assert!(
        err.contains("depth.tl:8: attempt to index a nil value (local 'c')"),
        "{err}"
    );
    assert!(
        err.contains("depth.tl:12: in function 'depth.describe'"),
        "the frame between the test and the failure: {err}"
    );
    assert!(
        err.contains("depth_test.tl:4:"),
        "and the line of the test itself: {err}"
    );
}

/// A file that raises while loading fails outside any test, so its error is the file's
/// rather than a test's. The cause stays on the file's own line and the frames go under
/// it, because a traceback inside `(error: …, 3 ms)` would bury the timing.
#[test]
fn test_shows_the_frames_of_a_file_that_raises_while_loading() {
    let root = project("load");
    write(
        &root.join("load_test.tl"),
        "local t = require(\"htl.test\")\nlocal depth = require(\"depth\")\n\
         local greeting = depth.describe(nil as depth.Cfg)\n\
         t.it(\"never runs\", function() t.expect(greeting):to_equal(\"cfg x\") end)\n",
    );
    let (ok, _out, err) = htl(&["test", "load_test.tl"], &root);
    assert!(!ok, "{err}");
    let first = err.lines().next().unwrap_or_default();
    assert!(
        first.contains("error: ") && first.ends_with("ms)"),
        "the file's line keeps its shape: {first}"
    );
    assert!(!first.contains("stack traceback"), "{first}");
    assert!(err.contains("stack traceback:"), "{err}");
    assert!(
        err.contains("depth.tl:12: in function 'depth.describe'"),
        "{err}"
    );
    assert!(err.contains("load_test.tl:3: in main chunk"), "{err}");

    // The document says the same thing as the terminal.
    let (_ok, out, _err) = htl(&["test", "load_test.tl", "--format", "json"], &root);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let error = doc["files"][0]["error"].as_str().unwrap_or_default();
    assert!(error.contains("stack traceback:\n"), "{error}");
    assert!(
        error.contains("depth.tl:12: in function 'depth.describe'"),
        "{error}"
    );
}
