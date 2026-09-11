//! What `htl fmt` writes, pinned byte for byte.
//!
//! Until this file, nothing in the repository checked the formatter's *output*. Two tests
//! read its exit status — `pkg_patch_cli.rs` asks that a patched dependency is left alone,
//! `toolchain_pin.rs` that `fmt` reads the config like every other command — and both are
//! satisfied by a formatter that writes nothing at all. The rules in `htl-core/src/fmt.lua`
//! (indentation recomputed from the syntax tree, one extra level for continuation lines,
//! blank-line runs capped at two, leading and trailing blank lines and trailing whitespace
//! dropped, a final newline, and every line that starts inside a long string or long comment
//! left exactly where it is) had no test between them.
//!
//! The mechanism is `scaffold_snapshot.rs`'s, deliberately: a snapshot is a plain text file
//! under `tests/snapshots/fmt/`, `HTL_UPDATE_SNAPSHOTS=1 cargo test -p htl-cli --test
//! fmt_snapshot` rewrites it, and the rewritten file is the thing under review. Here the
//! snapshot is simply the formatted source, because one file needs no manifest around it.
//! The compare-and-bless block is a second copy of that file's rather than a shared helper:
//! `tests/common/` is compiled into all thirty-odd test binaries and anything unused in one
//! of them is a warning that CI treats as an error, so a helper moves there when most of
//! them want it, not when the second one does.
//!
//! The fixture is copied into a scratch directory before it is formatted. `htl fmt` writes
//! in place, so running it on the checkout would leave the fixture formatted and every
//! subsequent run would compare the formatter against its own previous output.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/fmt")
        .join(name)
}

fn snapshot_path(case: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots/fmt")
        .join(format!("{case}.txt"))
}

/// Run the binary in `cwd` and return whether it succeeded together with everything it
/// wrote. `htl fmt` reports to stderr, as everything the CLI prints for a person does.
fn htl(args: &[&str], cwd: &Path) -> (bool, String) {
    let out = Command::new(common::htl_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

fn assert_snapshot(case: &str, got: &str) {
    let path = snapshot_path(case);
    if std::env::var_os("HTL_UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}\nHTL_UPDATE_SNAPSHOTS=1 cargo test -p htl-cli --test fmt_snapshot writes it",
            path.display()
        )
    });
    if got == want {
        return;
    }
    let (n, w, g) = got
        .lines()
        .zip(want.lines())
        .enumerate()
        .find(|(_, (g, w))| g != w)
        .map(|(n, (g, w))| (n + 1, w.to_string(), g.to_string()))
        .unwrap_or_else(|| {
            // Every line the two share matches, so one of them ran out first. Printing the
            // first line the longer one has past that point says which, where a pair of
            // `<end>`s would leave the reader to count.
            let at = want.lines().count().min(got.lines().count());
            (
                at + 1,
                want.lines().nth(at).unwrap_or("<end of file>").to_string(),
                got.lines().nth(at).unwrap_or("<end of file>").to_string(),
            )
        });
    panic!(
        "{} differs at line {n}\n  snapshot: {w}\n  htl fmt:  {g}\nHTL_UPDATE_SNAPSHOTS=1 rewrites it once the change is intended",
        path.display()
    );
}

/// Copy `<case>.tl` out of the checkout, format it, and hold the result to the snapshot.
///
/// Three claims, and the snapshot is only the first. The report is asserted because a
/// formatter that silently skipped the file would write the same bytes back and the snapshot
/// alone could not tell the difference. And the output is fed to `fmt --check`, which says
/// that formatting is a fixed point: without it a rule that alternated between two layouts
/// would still match a snapshot blessed from one of them.
fn assert_formats(case: &str) {
    let dir = common::scratch("htl-cli-fmt", case);
    let file = dir.join(format!("{case}.tl"));
    std::fs::copy(fixture(&format!("{case}.tl")), &file).unwrap();

    let (ok, out) = htl(&["fmt", &format!("{case}.tl")], &dir);
    assert!(ok, "htl fmt must succeed on {case}.tl:\n{out}");
    assert!(
        out.contains("htl fmt: 1 file(s), 1 reformatted, 0 failed"),
        "the fixture is unformatted, so fmt reports rewriting exactly one file:\n{out}"
    );

    assert_snapshot(case, &std::fs::read_to_string(&file).unwrap());

    let (ok, out) = htl(&["fmt", "--check", &format!("{case}.tl")], &dir);
    assert!(ok, "formatting {case}.tl again must change nothing:\n{out}");
}

/// Indentation, recomputed. A record with a record nested inside it, an `if` / `elseif` /
/// `else` whose bodies are all indented differently in the source, a `for` inside one of
/// them, a continuation line after a trailing `..`, a nested table constructor — and a long
/// bracket string in the middle of it whose two interior lines keep the indentation they
/// were written with, because they are the formatter's one documented exception.
#[test]
fn fmt_recomputes_the_indentation_of_a_messy_module() {
    assert_formats("messy");
}

/// The whitespace rules the file above does not reach: blank lines at the top of the file
/// and at the bottom, a run of four in the middle, three lines ending in spaces, a last line
/// with no newline after it, and a long *comment* rather than a long string.
///
/// `tests/fixtures/fmt/edges.tl` therefore ends without a trailing newline, on purpose —
/// that is the input, not an oversight, and adding one deletes half of what this case asks.
#[test]
fn fmt_normalises_the_whitespace_around_the_code() {
    assert_formats("edges");
}
