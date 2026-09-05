//! `explicit-number` (opt-in): unannotated locals initialized with a numeric literal.

use htl_core::Htl;
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "htl-core-explnum-{name}-{}-{}",
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

fn lints_of(dir: &Path, file: &str, spec: Option<&str>) -> Vec<String> {
    let h = Htl::new().unwrap();
    if let Some(s) = spec {
        h.configure_lints(s).unwrap();
    }
    h.add_path(dir).unwrap();
    let ci = h.check(&dir.join(file)).unwrap();
    assert!(ci.ok(), "unexpected type errors: {:?}", ci.errors);
    ci.lints
}

const SRC: &str = "local total = 0\nlocal ratio = 0.5\nlocal neg = -1\nlocal typed: number = 0\nlocal s = \"x\"\nlocal a, b = 1, 2.0\nprint(total, ratio, neg, typed, s, a, b)\n";

#[test]
fn off_by_default() {
    let dir = scratch("default");
    write(&dir.join("n.tl"), SRC);
    let lints = lints_of(&dir, "n.tl", None);
    assert!(!lints.iter().any(|l| l.contains("explicit-number")), "{lints:?}");
}

#[test]
fn flags_unannotated_numeric_literals_only() {
    let dir = scratch("on");
    write(&dir.join("n.tl"), SRC);
    let lints: Vec<String> = lints_of(&dir, "n.tl", Some("+explicit-number"))
        .into_iter()
        .filter(|l| l.contains("explicit-number"))
        .collect();
    let hit = |name: &str, inferred: &str| {
        lints.iter().any(|l| l.contains(&format!("'{name}' is inferred as {inferred}")))
    };
    assert!(hit("total", "integer"), "{lints:?}");
    assert!(hit("ratio", "number"), "{lints:?}");
    assert!(hit("neg", "integer"), "{lints:?}");
    assert!(hit("a", "integer") && hit("b", "number"), "multi-assign: {lints:?}");
    assert!(!lints.iter().any(|l| l.contains("'typed'")), "annotated local flagged: {lints:?}");
    assert!(!lints.iter().any(|l| l.contains("'s'")), "string local flagged: {lints:?}");
    assert_eq!(lints.len(), 5, "{lints:?}");
}

#[test]
fn allow_comment_silences_it() {
    let dir = scratch("allow");
    write(&dir.join("n.tl"), "local total = 0 -- htl: allow(explicit-number)\nprint(total)\n");
    let lints = lints_of(&dir, "n.tl", Some("+explicit-number"));
    assert!(!lints.iter().any(|l| l.contains("explicit-number")), "{lints:?}");
}
