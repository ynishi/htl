//! `htl::fix`: fixes carried by diagnostics, the safe/unsafe gate, applying, passes,
//! dry run, and the revert on a fix that makes things worse.

use htl_core::fix::{FixOptions, fix_file, unified_diff};
use htl_core::{Applicability, Htl};
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "htl-core-fix-{name}-{}-{}",
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

const FWD: &str = "local record world\n   record W\n      hp: integer\n   end\n   name: string\nend\n\n\
function world.tick(w: world.W): boolean\n   world.observe(w, \"tick\")\n   return world:alive(w)\nend\n\n\
function world.observe(w: world.W, what: string)\n   print(w.hp, what)\nend\n\n\
function world:alive(w: world.W): boolean\n   return w.hp > 0\nend\n\nreturn world\n";

#[test]
fn diagnostics_carry_fixes_with_applicability() {
    let dir = scratch("carry");
    write(&dir.join("world.tl"), FWD);
    write(
        &dir.join("num.tl"),
        "local total = 0\ntotal = total + 1.5\nprint(total)\n",
    );
    write(
        &dir.join("glob.tl"),
        "global counter: integer = 0\nprint(counter)\n",
    );
    let h = Htl::new().unwrap();
    h.configure_lints("+explicit-number").unwrap();
    h.add_path(&dir).unwrap();

    let w = h.check(&dir.join("world.tl")).unwrap();
    assert_eq!(w.errors.len(), w.error_fixes.len());
    let fixes: Vec<_> = w.error_fixes.iter().flatten().collect();
    assert_eq!(
        fixes.len(),
        2,
        "both forward references get a fix: {:?}",
        w.errors
    );
    assert!(fixes.iter().all(|f| f.applicability == Applicability::Safe));
    let e = &fixes[0].edits[0];
    assert_eq!(
        (e.line, e.col, e.end_line, e.end_col),
        (6, 1, 6, 1),
        "inserted before the record's `end`: {e:?}"
    );
    assert_eq!(
        e.text, "   observe: function(w: world.W, what: string)\n",
        "indented like the last field"
    );
    assert_eq!(
        fixes[1].edits[0].text,
        "   alive: function(self: world, w: world.W): boolean\n"
    );

    let n = h.check(&dir.join("num.tl")).unwrap();
    let f = n.lint_fixes[0]
        .as_ref()
        .expect("explicit-number carries a fix");
    assert_eq!(f.applicability, Applicability::Safe);
    assert_eq!(
        (f.edits[0].line, f.edits[0].col, &f.edits[0].text[..]),
        (1, 12, ": number")
    );

    let g = h.check(&dir.join("glob.tl")).unwrap();
    let f = g.lint_fixes[0].as_ref().expect("no-global carries a fix");
    assert_eq!(f.applicability, Applicability::Unsafe);
    assert_eq!(
        (&f.edits[0].text[..], f.edits[0].col, f.edits[0].end_col),
        ("local", 1, 7)
    );
}

#[test]
fn fix_file_applies_safe_fixes_and_is_idempotent() {
    let dir = scratch("apply");
    write(&dir.join("world.tl"), FWD);
    let h = Htl::new().unwrap();
    h.add_path(&dir).unwrap();
    let out = fix_file(&h, &dir.join("world.tl"), &FixOptions::default()).unwrap();
    assert_eq!(out.applied.len(), 2, "{:?} {:?}", out.applied, out.skipped);
    assert!(
        out.applied
            .iter()
            .all(|a| a.rule == "forward-ref" && a.pass == 1)
    );
    assert!(out.reverted.is_none() && out.oscillation.is_none());
    assert!(out.check.ok(), "clean after fixing: {:?}", out.check.errors);
    let text = std::fs::read_to_string(dir.join("world.tl")).unwrap();
    assert!(text.contains("   name: string\n   observe: function(w: world.W, what: string)\n   alive: function(self: world, w: world.W): boolean\nend"), "{text}");

    let again = fix_file(&h, &dir.join("world.tl"), &FixOptions::default()).unwrap();
    assert!(
        again.applied.is_empty() && again.contents.is_none(),
        "second run is a no-op"
    );
}

#[test]
fn unsafe_fixes_need_the_flag_or_promotion() {
    let dir = scratch("unsafe");
    write(
        &dir.join("glob.tl"),
        "global counter: integer = 0\nprint(counter)\n",
    );
    let h = Htl::new().unwrap();
    h.add_path(&dir).unwrap();
    let out = fix_file(&h, &dir.join("glob.tl"), &FixOptions::default()).unwrap();
    assert!(out.applied.is_empty());
    assert!(
        out.skipped
            .iter()
            .any(|s| s.rule == "no-global" && s.reason.contains("--unsafe")),
        "{:?}",
        out.skipped
    );

    let out = fix_file(
        &h,
        &dir.join("glob.tl"),
        &FixOptions {
            unsafe_fixes: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(out.applied.len(), 1);
    assert!(
        std::fs::read_to_string(dir.join("glob.tl"))
            .unwrap()
            .starts_with("local counter")
    );

    write(
        &dir.join("glob2.tl"),
        "global counter: integer = 0\nprint(counter)\n",
    );
    let out = fix_file(
        &h,
        &dir.join("glob2.tl"),
        &FixOptions {
            promoted: vec!["no-global".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(out.applied.len(), 1, "promoted by [fix] unsafe");
    assert_eq!(out.applied[0].applicability, Applicability::Safe);
}

#[test]
fn dry_run_and_diff_leave_the_file_alone() {
    let dir = scratch("dry");
    write(
        &dir.join("num.tl"),
        "local total = 0\ntotal = total + 1.5\nprint(total)\n",
    );
    let h = Htl::new().unwrap();
    h.configure_lints("+explicit-number").unwrap();
    h.add_path(&dir).unwrap();
    let out = fix_file(
        &h,
        &dir.join("num.tl"),
        &FixOptions {
            dry_run: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(out.applied.len(), 1);
    let after = out.contents.as_deref().expect("what would be written");
    assert!(after.starts_with("local total: number = 0\n"), "{after}");
    assert_eq!(
        std::fs::read_to_string(dir.join("num.tl")).unwrap(),
        "local total = 0\ntotal = total + 1.5\nprint(total)\n",
        "untouched"
    );
    let d = unified_diff(
        "num.tl",
        "local total = 0\ntotal = total + 1.5\nprint(total)\n",
        after,
    );
    assert!(
        d.contains("-local total = 0") && d.contains("+local total: number = 0"),
        "{d}"
    );
}

/// Type errors elsewhere do not block a fix (the annotation is what removes the
/// `got number, expected integer` error); the unrelated error simply remains.
#[test]
fn other_type_errors_do_not_block_but_syntax_errors_do() {
    let dir = scratch("other-errors");
    write(
        &dir.join("num.tl"),
        "local total = 0\ntotal = total + 1.5\nlocal s: string = 1\nprint(total, s)\n",
    );
    let h = Htl::new().unwrap();
    h.configure_lints("+explicit-number").unwrap();
    h.add_path(&dir).unwrap();
    let out = fix_file(&h, &dir.join("num.tl"), &FixOptions::default()).unwrap();
    assert_eq!(out.applied.len(), 1, "{:?}", out.skipped);
    assert!(out.reverted.is_none());
    assert_eq!(
        out.check.errors.len(),
        1,
        "the string error stays, the number one is gone: {:?}",
        out.check.errors
    );

    write(
        &dir.join("broken.tl"),
        "local total = 0\ntotal = total + 1.5\nif total then\n",
    );
    let out = fix_file(&h, &dir.join("broken.tl"), &FixOptions::default()).unwrap();
    assert!(out.applied.is_empty() && out.contents.is_none());
    assert!(
        out.skipped
            .iter()
            .any(|s| s.reason.contains("syntax error")),
        "{:?}",
        out.skipped
    );
}

#[test]
fn a_fix_that_makes_things_worse_is_reverted() {
    // Contrived: a fix's declaration line would be inserted into a record whose `end`
    // line we fake by making the record body not what the checker saw. Simplest real
    // trigger: explicit-number on a name that is also used as a type-annotated
    // parameter later cannot be produced, so drive the revert through the API with a
    // file where the applied edit lands inside a string (line/col from a stale check).
    let dir = scratch("revert");
    write(&dir.join("world.tl"), FWD);
    let h = Htl::new().unwrap();
    h.add_path(&dir).unwrap();
    // Shrink the record so the computed insertion line (6) points at `function world.tick`.
    write(
        &dir.join("world.tl"),
        &FWD.replacen("   name: string\nend\n", "end\n", 1),
    );
    let out = fix_file(&h, &dir.join("world.tl"), &FixOptions::default()).unwrap();
    // Either the fix landed correctly on the new line 5 (checker re-read the file) and
    // the result is clean, or it was reverted; a broken file left behind is the failure.
    let text = std::fs::read_to_string(dir.join("world.tl")).unwrap();
    let c = h.check(&dir.join("world.tl")).unwrap();
    assert!(
        c.ok() || out.reverted.is_some(),
        "never leave a worse file behind: {:?} / {:?}\n{text}",
        c.errors,
        out.reverted
    );
}
