//! The dependency `htl new` writes by default, pulled through a scaffolded project by this
//! htl: `htl new --htl path:<this checkout>` writes `htlx = { git, tag = HTLX_TAG }`, `htl
//! pkg install` fetches it, and a test that requires `htlx.list` and `htlx.tablex` is checked
//! and run. htl-x's own CI installs htl from `main` and runs its suite; this is the other
//! direction of that contract — the tag the scaffold pins works with the htl in this tree,
//! so raising `scaffold::HTLX_TAG` is a change this gate sees.
//!
//! It is about a *generated* project, like `scaffold_targets.rs`, and is a separate file
//! for the one thing it needs that those do not: the network. `htl pkg install` clones the
//! dependency, so this is the one test under `just e2e` that does not run offline; a
//! machine without it gets a red `htl pkg install` naming the URL rather than a skip,
//! because a gate that skips is a gate nobody notices has stopped running.
//!
//! No cargo runs inside the project — it is a plain Teal tree — so none of the nested-cargo
//! care `scaffold_targets.rs` takes is needed here, and the binary is located the same way
//! that file does it: asked of cargo, never assembled from a path.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// The root of this workspace: this crate sits directly under it.
fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

/// The `htl` that writes and checks the project: `HTL_TEST_BIN` when it is set (the
/// packaged gate points it at an installed CLI), otherwise the one cargo just built,
/// at the path cargo reports for it.
fn htl_bin() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        if let Some(given) = std::env::var_os("HTL_TEST_BIN") {
            return PathBuf::from(given);
        }
        let mut cargo =
            Command::new(std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")));
        // The jobserver and this crate's identity would otherwise reach the child build;
        // `scaffold_targets.rs` says why each is dropped and these two kept.
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy();
            let keep = name == "CARGO_HOME" || name == "CARGO_TARGET_DIR";
            if name.starts_with("CARGO_") && !keep {
                cargo.env_remove(&key);
            }
        }
        let out = cargo
            .args([
                "build",
                "-p",
                "htl-cli",
                "--bin",
                "htl",
                "--message-format=json",
            ])
            .current_dir(workspace_root())
            .stderr(Stdio::inherit())
            .output()
            .expect("cargo build -p htl-cli could not be started");
        assert!(
            out.status.success(),
            "cargo build -p htl-cli: {}",
            out.status
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        stdout
            .lines()
            .filter_map(|line| {
                let msg: serde_json::Value = serde_json::from_str(line).ok()?;
                if msg["reason"] != "compiler-artifact" || msg["target"]["name"] != "htl" {
                    return None;
                }
                Some(PathBuf::from(msg["executable"].as_str()?))
            })
            .next_back()
            .expect("cargo built htl-cli but reported no executable for the htl bin target")
    })
}

/// A fresh directory under the system temp dir, outside the checkout so the CLI runs
/// against the project's configuration and not this repository's. Removed when the test
/// passes, left for reading when it does not.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("htl-e2e-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    println!("scaffolding into {}", dir.display());
    dir
}

/// `htl <args>` in `cwd`, both streams captured and printed — the reports these commands
/// write go to stderr, and the assertion below reads one — and the test fails naming the
/// command if it did not succeed.
fn htl(args: &[&str], cwd: &Path) -> String {
    let out = Command::new(htl_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|e| panic!("htl {args:?}: could not be started: {e}"));
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    println!("--- htl {} ---\n{text}", args.join(" "));
    assert!(out.status.success(), "htl {args:?}: {}\n{text}", out.status);
    text
}

/// One function from each of the two modules the scaffold's README names first, through
/// `htl.test`, so a tag whose `entry` moved or whose signatures changed is a failing
/// `htl check` here and not a user's.
const CONSUMER_TEST: &str = r#"local t = require("htl.test")
local list = require("htlx.list")
local tablex = require("htlx.tablex")

t.describe("htlx through a scaffolded project", function()
   t.it("list.map returns a new array", function()
      local doubled = list.map({ 1, 2, 3 }, function(n: integer): integer return n * 2 end)
      t.expect(doubled):to_equal({ 2, 4, 6 })
   end)

   t.it("tablex.sorted_pairs walks a map in key order", function()
      local keys: {string} = {}
      for k, _ in tablex.sorted_pairs({ b = 2, a = 1, c = 3 }) do
         table.insert(keys, k)
      end
      t.expect(keys):to_equal({ "a", "b", "c" })
   end)
end)
"#;

#[test]
fn the_scaffolds_htlx_dependency_installs_checks_and_tests_against_this_htl() {
    let root = scratch("htlx-consumer");
    let checkout = format!("path:{}", workspace_root().display());
    htl(&["new", "x-consumer", "--htl", &checkout], &root);
    let project = root.join("x-consumer");

    let manifest = fs::read_to_string(project.join("mlua-pkg.toml")).unwrap();
    assert!(
        manifest.contains("htlx = { git = \"https://github.com/ynishi/htl-x\", tag = \"v"),
        "the scaffold under a checkout pin names no htlx:\n{manifest}"
    );

    htl(&["pkg", "install"], &project);
    fs::write(project.join("tests").join("x_test.tl"), CONSUMER_TEST).unwrap();
    htl(&["check", "."], &project);
    let report = htl(&["test"], &project);
    assert!(
        report.contains("ok   ./tests/x_test.tl  (2 passed, 0 failed"),
        "htl test did not run the consumer file green:\n{report}"
    );

    let _ = fs::remove_dir_all(&root);
}
