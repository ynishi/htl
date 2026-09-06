//! `explicit-number` (opt-in): an unannotated local inferred `integer` from its literal
//! that is *later* assigned a number expression. Plain integer counters are not reported.

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
    ci.lints
        .into_iter()
        .filter(|l| l.contains("explicit-number"))
        .collect()
}

const SRC: &str = "local total = 0\n\
                   local ratio = 0.5\n\
                   local count = 1\n\
                   local MASK = 0xff\n\
                   local typed: number = 0\n\
                   total = total + 1.5\n\
                   count = count + 1\n\
                   ratio = ratio * 2\n\
                   local avg = 0\n\
                   avg = total / 2\n\
                   print(total, ratio, count, MASK, typed, avg)\n";

#[test]
fn off_by_default() {
    let dir = scratch("default");
    write(&dir.join("n.tl"), SRC);
    assert!(lints_of(&dir, "n.tl", None).is_empty());
}

#[test]
fn flags_integer_locals_that_later_meet_a_number() {
    let dir = scratch("on");
    write(&dir.join("n.tl"), SRC);
    let lints = lints_of(&dir, "n.tl", Some("+explicit-number"));
    let hit = |name: &str| {
        lints
            .iter()
            .any(|l| l.contains(&format!("'{name}' is inferred as integer")))
    };
    assert!(hit("total"), "assigned `+ 1.5`: {lints:?}");
    assert!(
        lints
            .iter()
            .any(|l| l.contains("'total'") && l.contains("line 6")),
        "names the assignment: {lints:?}"
    );
    assert!(hit("avg"), "assigned a `/` result: {lints:?}");
    assert!(
        !hit("count"),
        "integer counter must not be reported: {lints:?}"
    );
    assert!(!hit("MASK"), "hex constant must not be reported: {lints:?}");
    assert!(
        !lints
            .iter()
            .any(|l| l.contains("'ratio'") || l.contains("'typed'")),
        "{lints:?}"
    );
    assert_eq!(lints.len(), 2, "{lints:?}");
}

#[test]
fn allow_comment_silences_it() {
    let dir = scratch("allow");
    write(
        &dir.join("n.tl"),
        "local total = 0 -- htl: allow(explicit-number)\ntotal = total + 1.5\nprint(total)\n",
    );
    assert!(lints_of(&dir, "n.tl", Some("+explicit-number")).is_empty());
}
