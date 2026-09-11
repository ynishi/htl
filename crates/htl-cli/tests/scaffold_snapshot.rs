//! Every tree `htl new` and `htl init` write, pinned byte for byte.
//!
//! The scaffold is data now — a host profile turned into paths and bodies — and the point
//! of that change is that it did not move a single byte of the output. String assertions
//! (`scaffold_cli.rs`) say a few things about a few files; these say everything about all
//! of them, which is what makes rearranging the generator safe to review: the diff of a
//! refactor is empty here, and the diff of a deliberate change is exactly the change.
//!
//! A snapshot is a plain text file under `tests/snapshots/scaffold/`: a manifest of the
//! sorted relative paths, then every file under a `===== <path> =====` header. No
//! snapshot library — the format is a page of code and reads as a diff without one.
//! `HTL_UPDATE_SNAPSHOTS=1 cargo test -p htl-cli --test scaffold_snapshot` rewrites them;
//! the rewritten files are the thing under review, so read the diff before committing it.
//!
//! Two lines are derived from this crate's version — the `htl` dependency in `Cargo.toml`
//! and `[toolchain] htl` in `htl.toml`, which are the same derivation — so both are
//! normalised to `htl = "{{htl}}"` here and the derivation keeps its own tests in
//! `scaffold_cli.rs` and `toolchain_pin.rs`. Otherwise every release would rewrite these
//! files.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn htl(args: &[&str], cwd: &Path) {
    let out = Command::new(common::htl_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "htl {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn snapshot_path(case: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots/scaffold")
        .join(format!("{case}.txt"))
}

/// Relative paths of every file under `root`, sorted, with `/` separators whatever the
/// platform uses.
fn paths(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    collect(root, root, &mut out);
    out.sort();
    out
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(root, &path, out);
        } else {
            let rel = path.strip_prefix(root).unwrap();
            out.push(
                rel.components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/"),
            );
        }
    }
}

/// The whole tree as one text: the manifest, then each file's body.
fn render(root: &Path) -> String {
    let mut s = String::from("===== files =====\n");
    let paths = paths(root);
    for p in &paths {
        s.push_str(p);
        s.push('\n');
    }
    for p in &paths {
        let body = std::fs::read_to_string(root.join(p)).unwrap();
        assert!(body.ends_with('\n'), "{p} does not end with a newline");
        s.push_str(&format!("===== {p} =====\n"));
        s.push_str(&normalise(&body));
    }
    s
}

/// Pin the release the scaffold was written by, without pinning its number.
fn normalise(body: &str) -> String {
    body.lines()
        .map(|l| {
            if l.starts_with("htl = \"") {
                "htl = \"{{htl}}\"".to_string()
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn assert_tree(case: &str, root: &Path) {
    let got = render(root);
    let path = snapshot_path(case);
    if std::env::var_os("HTL_UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}\nHTL_UPDATE_SNAPSHOTS=1 cargo test -p htl-cli --test scaffold_snapshot writes it",
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
            (
                want.lines().count().min(got.lines().count()) + 1,
                "<end>".into(),
                "<end>".into(),
            )
        });
    panic!(
        "{} differs at line {n}\n  snapshot: {w}\n  scaffold: {g}\nHTL_UPDATE_SNAPSHOTS=1 rewrites it once the change is intended",
        path.display()
    );
}

#[test]
fn new_writes_the_plain_tree() {
    let root = common::scratch("htl-cli-snapshot", "plain");
    htl(&["new", "sample"], &root);
    assert_tree("plain", &root.join("sample"));
}

#[test]
fn new_lib_writes_no_entry_script() {
    let root = common::scratch("htl-cli-snapshot", "lib");
    htl(&["new", "sample", "--lib"], &root);
    assert_tree("lib", &root.join("sample"));
}

#[test]
fn new_embed_writes_the_rust_host() {
    let root = common::scratch("htl-cli-snapshot", "embed");
    htl(&["new", "sample", "--embed"], &root);
    assert_tree("embed", &root.join("sample"));
}

/// `--embed` is the shorthand for `--target rust`, and sharing the snapshot is what says
/// the two write the same tree rather than two trees that happen to look alike.
#[test]
fn new_target_rust_writes_what_embed_writes() {
    let root = common::scratch("htl-cli-snapshot", "target-rust");
    htl(&["new", "sample", "--target", "rust"], &root);
    assert_tree("embed", &root.join("sample"));
}

#[test]
fn new_lib_embed_writes_the_rust_host_without_a_binary() {
    let root = common::scratch("htl-cli-snapshot", "lib-embed");
    htl(&["new", "sample", "--lib", "--embed"], &root);
    assert_tree("lib-embed", &root.join("sample"));
}

/// The C ABI target: the library, the `#[c_export]` block, and the two reference callers
/// under `examples/`. Every byte of them is pinned here — a caller in another language
/// is the one part of a scaffold nobody compiles by accident, so a change to the Python
/// caller's `restype` or the C caller's `free` shows up in this diff or nowhere.
#[test]
fn new_lib_target_ffi_writes_the_c_abi_library_and_its_callers() {
    let root = common::scratch("htl-cli-snapshot", "ffi");
    htl(&["new", "sample", "--lib", "--target", "ffi"], &root);
    assert_tree("ffi", &root.join("sample"));
}

/// `htl init` in an empty directory is `htl new` — the same plan, only the name comes
/// from the directory. Sharing the snapshot is the assertion.
#[test]
fn init_in_an_empty_directory_writes_what_new_would() {
    let root = common::scratch("htl-cli-snapshot", "init");
    let dir = root.join("sample");
    std::fs::create_dir_all(&dir).unwrap();
    htl(&["init"], &dir);
    assert_tree("plain", &dir);
}

#[test]
fn init_embed_in_an_empty_directory_writes_what_new_would() {
    let root = common::scratch("htl-cli-snapshot", "init-embed");
    let dir = root.join("sample");
    std::fs::create_dir_all(&dir).unwrap();
    htl(&["init", "--embed"], &dir);
    assert_tree("embed", &dir);
}

/// Adding a target to a project that already exists. It is deliberately *not* the `embed`
/// tree: `README.md` and `src/main.tl` were written without a target and are kept, so the
/// project ends up with the Rust side filled in and its own prose untouched. That
/// difference is the reason this has a snapshot of its own — it is what a reader of
/// `htl init --target` on a real project actually gets.
#[test]
fn init_target_fills_in_the_rust_side_of_a_plain_project() {
    let root = common::scratch("htl-cli-snapshot", "init-target");
    let dir = root.join("sample");
    htl(&["new", "sample"], &root);
    htl(&["init", "--target", "rust"], &dir);
    assert_tree("init-host", &dir);
}
