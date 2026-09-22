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
//! One kind of line and one file depend on where the binary under test was built. The
//! line is a dependency on a crate of this repository in `Cargo.toml` — `htl`, and
//! `htl-mq` in a window project — which is what `--htl` picks and, when it is not given,
//! the htl this CLI was built with: a version on crates.io for a published CLI, this
//! checkout's path for the one `cargo test` builds. Each is normalised to
//! `<name> = "{{htl}}"` here so the same snapshot holds for both, and both are normalised
//! the same way, because the thing a window project has to get right is that the two name
//! one tree. The file is `mise.toml`,
//! the same pin as the command: written by a CLI that pins a release and not by one that
//! pins a checkout, so it is left out of the tree here rather than normalised. What the
//! line actually says under each pin, and that the file is there exactly under a release,
//! is `scaffold_cli.rs`'s; the packaged gate runs both suites against a tarball CLI
//! (`HTL_TEST_BIN`), which is how the release side of each is exercised. Every other byte
//! of these trees is pinned exactly.

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
/// platform uses — less `mise.toml`, the one file whose presence is the pin's (see the
/// module doc).
fn paths(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    collect(root, root, &mut out);
    out.retain(|p| p != "mise.toml");
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

/// The crates of this repository a scaffolded project may depend on, as the left-hand side
/// of the manifest line each gets. Every one of them is pinned by the same `HtlPin`, so
/// every one of them is normalised the same way.
const PINNED_DEPS: &[&str] = &["htl", "htl-mq"];

/// Pin the htl the scaffold was written by, without pinning where it is: a version, the
/// repository (`--htl main`) or a checkout's path (the default under `cargo test`, and
/// `--htl path:`), with or without the features a target adds, all read as one line — and
/// the same for a sibling crate under that pin. What each pin actually puts on those lines
/// is `scaffold_cli.rs`'s (`new_pins_the_htl_this_binary_was_built_with`,
/// `new_htl_main_writes_a_git_pin`, `the_window_targets_htl_mq_line_follows_the_pin`).
fn normalise(body: &str) -> String {
    body.lines()
        .map(|l| {
            match PINNED_DEPS
                .iter()
                .find(|d| l.starts_with(&format!("{d} = ")))
            {
                Some(d) => format!("{d} = \"{{{{htl}}}}\""),
                None => l.to_string(),
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

/// The manifest names `htlx` and the README's first step is the fetch, under `main` as
/// under the default: the same snapshot is the assertion that the pin has no say in what
/// a project starts with, and `plain` is where a change to the dependency line — the tag
/// above all — is reviewed.
#[test]
fn new_htl_main_writes_the_plain_tree() {
    let root = common::scratch("htl-cli-snapshot", "plain-main");
    htl(&["new", "sample", "--htl", "main"], &root);
    assert_tree("plain", &root.join("sample"));
}

/// `--no-x` is the plain tree without the dependency. Its own snapshot, so that the diff
/// between the two files is exactly what opting out changes: the dependency line and the
/// README paragraph that mentions it.
#[test]
fn new_no_x_writes_the_plain_tree_without_the_dependency() {
    let root = common::scratch("htl-cli-snapshot", "plain-no-x");
    htl(&["new", "sample", "--no-x"], &root);
    assert_tree("plain-no-x", &root.join("sample"));
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

/// `--htl main` writes the `embed` tree: the pin decides the `htl` line of `Cargo.toml`
/// and nothing else, and sharing the snapshot is that assertion. The pin line is the one
/// thing the two differ in, normalised away above.
#[test]
fn new_embed_htl_main_writes_what_the_default_writes() {
    let root = common::scratch("htl-cli-snapshot", "embed-main");
    htl(&["new", "sample", "--embed", "--htl", "main"], &root);
    assert_tree("embed", &root.join("sample"));
}

/// `--embed` is the shorthand for `--target bin`, and sharing the snapshot is what says
/// the two write the same tree rather than two trees that happen to look alike.
#[test]
fn new_target_bin_writes_what_embed_writes() {
    let root = common::scratch("htl-cli-snapshot", "target-bin");
    htl(&["new", "sample", "--target", "bin"], &root);
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
fn new_lib_target_cdylib_writes_the_c_abi_library_and_its_callers() {
    let root = common::scratch("htl-cli-snapshot", "cdylib");
    htl(&["new", "sample", "--lib", "--target", "cdylib"], &root);
    assert_tree("cdylib", &root.join("sample"));
}

/// The window target: the library with the project's own `fx` host module beside htl-mq's
/// `mq`, the binary that opens the window, the Teal engine and the game table it drives.
/// Every byte pinned, because what this target writes is mostly prose and Teal — the two
/// halves a compiler does not check until somebody runs the project.
#[test]
fn new_target_window_writes_the_window_host() {
    let root = common::scratch("htl-cli-snapshot", "window");
    htl(&["new", "sample", "--target", "window"], &root);
    assert_tree("window", &root.join("sample"));
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
    htl(&["init", "--target", "bin"], &dir);
    assert_tree("init-target", &dir);
}
