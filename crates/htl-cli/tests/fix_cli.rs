//! `htl fix` through the binary: the git guard, --dry-run / --diff, and JSON output.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-fix", name)
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

fn git(args: &[&str], cwd: &Path) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

const NUM: &str = "local total = 0\ntotal = total + 1.5\nprint(total)\n";

fn repo() -> Option<PathBuf> {
    let root = scratch("repo");
    if !git(&["init", "-q"], &root) {
        return None; // no git on this machine: the guard tests cannot run
    }
    git(&["config", "user.email", "t@example.com"], &root);
    git(&["config", "user.name", "t"], &root);
    write(
        &root.join("htl.toml"),
        "[lint.rules]\nexplicit-number = \"warn\"\n",
    );
    write(&root.join("src/num.tl"), NUM);
    git(&["add", "."], &root);
    git(&["commit", "-q", "-m", "init"], &root);
    Some(root)
}

#[test]
fn refuses_outside_a_repo_and_dirty_files() {
    let plain = scratch("plain");
    write(
        &plain.join("htl.toml"),
        "[lint.rules]\nexplicit-number = \"warn\"\n",
    );
    write(&plain.join("src/num.tl"), NUM);
    let (ok, _, err) = htl(&["fix", "src"], &plain);
    assert!(
        !ok && err.contains("not in a git repository") && err.contains("--allow-no-vcs"),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(plain.join("src/num.tl")).unwrap(),
        NUM,
        "untouched"
    );
    let (ok, _, err) = htl(&["fix", "src", "--allow-no-vcs"], &plain);
    assert!(ok, "{err}");
    assert!(
        std::fs::read_to_string(plain.join("src/num.tl"))
            .unwrap()
            .starts_with("local total: number")
    );

    let Some(root) = repo() else { return };
    write(
        &root.join("src/num.tl"),
        "-- edited\nlocal total = 0\ntotal = total + 1.5\nprint(total)\n",
    );
    let (ok, _, err) = htl(&["fix", "src"], &root);
    assert!(
        !ok && err.contains("uncommitted changes") && err.contains("--allow-dirty"),
        "{err}"
    );
    let (ok, _, err) = htl(&["fix", "src", "--allow-dirty"], &root);
    assert!(ok, "{err}");
}

#[test]
fn clean_repo_fixes_and_reports() {
    let Some(root) = repo() else { return };
    let (ok, _, err) = htl(&["fix", "src"], &root);
    assert!(ok, "{err}");
    assert!(
        err.contains("fixed: src/num.tl:1: explicit-number (safe)"),
        "{err}"
    );
    assert!(err.contains("1 changed"), "{err}");
    assert!(
        std::fs::read_to_string(root.join("src/num.tl"))
            .unwrap()
            .starts_with("local total: number = 0")
    );
}

#[test]
fn diff_prints_and_writes_nothing_even_in_a_dirty_tree() {
    let Some(root) = repo() else { return };
    write(
        &root.join("src/num.tl"),
        "-- edited\nlocal total = 0\ntotal = total + 1.5\nprint(total)\n",
    );
    let (ok, out, err) = htl(&["fix", "src", "--diff"], &root);
    assert!(ok, "{err}");
    assert!(
        out.contains("-local total = 0") && out.contains("+local total: number = 0"),
        "{out}"
    );
    assert!(
        std::fs::read_to_string(root.join("src/num.tl"))
            .unwrap()
            .contains("local total = 0"),
        "untouched"
    );
    assert!(err.contains("nothing written"), "{err}");
}

#[test]
fn json_carries_fixes_on_check_and_the_fix_report() {
    let Some(root) = repo() else { return };
    let (_, out, _) = htl(&["check", "src", "--format", "json"], &root);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let d = &v["diagnostics"][0];
    assert_eq!(d["rule"], "explicit-number");
    assert_eq!(d["fix"]["applicability"], "safe");
    assert_eq!(d["fix"]["edits"][0]["text"], ": number");

    let (ok, out, err) = htl(&["fix", "src", "--format", "json"], &root);
    assert!(ok, "{err}");
    assert!(
        err.trim().is_empty(),
        "json mode is silent on stderr: {err}"
    );
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["applied"][0]["rule"], "explicit-number");
    assert_eq!(v["summary"]["files_changed"], 1);
    assert_eq!(v["summary"]["ok"], true);
    assert_eq!(v["files"][0]["changed"], true);
}

const STRUCT_DEFS: &str = "local record defs\n   ---@struct\n   record Node\n      zeta: string\n\
   \x20     alpha: integer\n   end\nend\nreturn defs\n";
const STRUCT_SITE: &str =
    "local defs = require(\"defs\")\nlocal n: defs.Node = { zeta = \"z\" }\nreturn n\n";

/// A `suggest` fix through the binary: reported as fixable, shown by `--diff`, carried by
/// JSON, and written by nothing — not even `--unsafe`.
#[test]
fn a_suggestion_is_shown_and_never_written() {
    let dir = scratch("suggest");
    write(&dir.join("src/defs.tl"), STRUCT_DEFS);
    write(&dir.join("src/mod.tl"), STRUCT_SITE);

    let (ok, _, err) = htl(&["check", "src"], &dir);
    assert!(ok, "{err}");
    assert!(
        err.contains("[htl struct-fields] (fixable: htl fix)"),
        "{err}"
    );

    let (ok, out, err) = htl(&["fix", "src", "--diff"], &dir);
    assert!(ok, "{err}");
    assert!(
        err.contains("skipped: src/mod.tl:2: struct-fields — suggestion only"),
        "{err}"
    );
    assert!(
        out.contains("(suggested)")
            && out
                .contains("+local n: defs.Node = { zeta = \"z\", alpha = htl_fixme(\"integer\") }"),
        "{out}"
    );

    for args in [
        &["fix", "src", "--allow-no-vcs"][..],
        &["fix", "src", "--allow-no-vcs", "--unsafe"][..],
    ] {
        let (ok, _, err) = htl(args, &dir);
        assert!(ok, "{err}");
        assert!(err.contains("0 changed"), "{args:?}: {err}");
        assert_eq!(
            std::fs::read_to_string(dir.join("src/mod.tl")).unwrap(),
            STRUCT_SITE,
            "{args:?} left the file alone"
        );
    }

    let (_, out, _) = htl(&["check", "src", "--format", "json"], &dir);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let d = &v["diagnostics"][0];
    assert_eq!(d["rule"], "struct-fields");
    assert_eq!(d["fix"]["applicability"], "suggest");
    assert_eq!(
        d["fix"]["edits"][0]["text"],
        ", alpha = htl_fixme(\"integer\")"
    );
}

#[test]
fn exit_non_zero_on_fix_for_ci() {
    let Some(root) = repo() else { return };
    let (ok, _, err) = htl(&["fix", "src", "--exit-non-zero-on-fix"], &root);
    assert!(!ok, "a change happened: {err}");
    // The fixed file is now modified in git: the guard would refuse, so allow it.
    let (ok, _, err) = htl(
        &["fix", "src", "--exit-non-zero-on-fix", "--allow-dirty"],
        &root,
    );
    assert!(ok, "second run changes nothing: {err}");
}

/// `--rule` and `[fix] disable` take the rules `--list-lints` names *and* the two classes
/// an error's fix is filed under, which the listing does not name. A name from neither set
/// is refused rather than matching nothing.
#[test]
fn the_fix_filters_take_the_rules_and_the_error_classes() {
    let Some(root) = repo() else { return };
    // The run happens (it reports on the file); its exit code is about what the file still
    // has wrong, which is not what this is asking.
    for rule in ["tl:error", "forward-ref", "explicit-number"] {
        let (_, _, err) = htl(&["fix", "src", "--dry-run", "--rule", rule], &root);
        assert!(err.contains("htl fix: 1 file(s)"), "--rule {rule}: {err}");
        assert!(!err.contains("unknown rule"), "--rule {rule}: {err}");
    }
    let (ok, _, err) = htl(&["fix", "src", "--dry-run", "--rule", "forwardref"], &root);
    assert!(!ok, "a name that is not a rule: {err}");
    assert!(err.contains("unknown rule `forwardref`"), "{err}");
    assert!(err.contains("--list-lints"), "{err}");
}

/// The old spelling of the second class. Renaming it is breaking on purpose; what a
/// project gets is the new name, not a run that silently fixed nothing.
#[test]
fn the_old_error_spelling_says_what_it_is_called_now() {
    let Some(root) = repo() else { return };
    let want = "`error` is now `tl:error`";

    let (ok, _, err) = htl(&["fix", "src", "--dry-run", "--rule", "error"], &root);
    assert!(!ok && err.contains(want), "{err}");
    assert!(err.contains("htl fix --rule:"), "{err}");

    write(
        &root.join("htl.toml"),
        "[lint.rules]\nexplicit-number = \"warn\"\n\n[fix]\ndisable = [\"error\"]\n",
    );
    let (ok, _, err) = htl(&["fix", "src", "--dry-run"], &root);
    assert!(!ok && err.contains(want), "{err}");
    assert!(err.contains("[fix] disable:"), "{err}");

    // And the name it points at is one the same table takes.
    write(
        &root.join("htl.toml"),
        "[lint.rules]\nexplicit-number = \"warn\"\n\n[fix]\ndisable = [\"tl:error\"]\n",
    );
    let (ok, _, err) = htl(&["fix", "src", "--dry-run"], &root);
    assert!(ok, "{err}");
}
