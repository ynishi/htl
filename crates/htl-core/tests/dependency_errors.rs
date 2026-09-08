//! `CheckInfo::dependency_errors`: a type error in a module reached through `require` is
//! reported against the file that required it, transitively, each module once. The checker
//! hands a requirer only the dependency's *type*, so without this the error is found and
//! dropped — the project checks clean and fails at its first `require`.

use htl_core::Htl;
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-deperr", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// `src/geometry.tl` requires `mathx` (broken at line 5), which requires `inner` (broken
/// at line 4). Neither error is geometry's own.
fn project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(
        &root.join("src/geometry.tl"),
        "local mathx = require(\"mathx\")\nlocal record geometry\nend\n\
         function geometry.area(n: number): number\n   return mathx.twice(n)\nend\nreturn geometry\n",
    );
    write(
        &root.join("mods/mathx.tl"),
        "local inner = require(\"inner\")\nlocal record mathx\nend\n\
         function mathx.twice(n: number): number\n   local s: number = \"no\"\n   return n * 2 + inner.one() + s\nend\nreturn mathx\n",
    );
    write(
        &root.join("mods/inner.tl"),
        "local record inner\nend\nfunction inner.one(): number\n   local x: string = 1\n   return 1 + #x\nend\nreturn inner\n",
    );
    root
}

fn checker(root: &Path) -> Htl {
    let h = Htl::new().unwrap();
    h.add_path(&root.join("mods")).unwrap();
    h.add_path(&root.join("src")).unwrap();
    h
}

#[test]
fn errors_in_required_modules_are_reported_against_the_requirer_transitively() {
    let root = project("transitive");
    let h = checker(&root);
    let c = h.check(&root.join("src/geometry.tl")).unwrap();
    assert!(c.ok(), "geometry itself is fine: {:?}", c.errors);
    assert_eq!(
        c.dependency_errors.len(),
        2,
        "one error each in mathx and inner: {:?}",
        c.dependency_errors
    );
    let mathx = &c.dependency_errors[0];
    assert!(mathx.file.ends_with("mathx.tl"), "{:?}", mathx.file);
    assert!(
        mathx.required_by.ends_with("geometry.tl"),
        "{:?}",
        mathx.required_by
    );
    assert!(
        mathx.text.contains("mathx.tl:5:"),
        "the dependency's own path and line: {}",
        mathx.text
    );
    let inner = &c.dependency_errors[1];
    assert!(inner.file.ends_with("inner.tl"), "{:?}", inner.file);
    assert!(
        inner.required_by.ends_with("mathx.tl"),
        "the module that required it, not the file being checked: {:?}",
        inner.required_by
    );
    assert!(inner.text.contains("inner.tl:4:"), "{}", inner.text);
}

/// The second check of the requirer is served from the checker's store rather than
/// re-checking its dependencies; the errors are part of what was stored.
#[test]
fn a_check_served_from_the_store_still_carries_them() {
    let root = project("store");
    let h = checker(&root);
    let first = h.check(&root.join("src/geometry.tl")).unwrap();
    let again = h.check(&root.join("src/geometry.tl")).unwrap();
    assert_eq!(
        first.dependency_errors, again.dependency_errors,
        "a stored result must report what the fresh one did"
    );
    assert_eq!(again.dependency_errors.len(), 2);
}

/// Checked in its own right, a module's error is its own; only what *it* required is a
/// dependency error.
#[test]
fn a_module_checked_directly_owns_its_error() {
    let root = project("own");
    let h = checker(&root);
    let c = h.check(&root.join("mods/mathx.tl")).unwrap();
    assert_eq!(c.errors.len(), 1, "{:?}", c.errors);
    assert!(c.errors[0].contains("mathx.tl:5:"), "{}", c.errors[0]);
    assert_eq!(c.dependency_errors.len(), 1, "{:?}", c.dependency_errors);
    assert!(c.dependency_errors[0].file.ends_with("inner.tl"));
}

/// A declaration goes through the same path as a source.
#[test]
fn a_declaration_with_an_error_is_reported_the_same_way() {
    let root = scratch("decl");
    write(
        &root.join("types/host.d.tl"),
        "local record host\n   run: function(): Nope\nend\nreturn host\n",
    );
    write(
        &root.join("src/main.tl"),
        "local host = require(\"host\")\nhost.run()\n",
    );
    let h = Htl::new().unwrap();
    h.add_path(&root.join("types")).unwrap();
    h.add_path(&root.join("src")).unwrap();
    let c = h.check(&root.join("src/main.tl")).unwrap();
    assert_eq!(c.dependency_errors.len(), 1, "{:?}", c.dependency_errors);
    let d = &c.dependency_errors[0];
    assert!(d.file.ends_with("host.d.tl"), "{:?}", d.file);
    assert!(d.required_by.ends_with("main.tl"), "{:?}", d.required_by);
    assert!(d.text.contains("host.d.tl:2:"), "{}", d.text);
}

/// Nothing required, nothing reported; and a clean dependency contributes nothing.
#[test]
fn a_clean_dependency_reports_nothing() {
    let root = scratch("clean");
    write(
        &root.join("mods/mathx.tl"),
        "local record mathx\nend\nfunction mathx.twice(n: number): number\n   return n * 2\nend\nreturn mathx\n",
    );
    write(
        &root.join("src/main.tl"),
        "local mathx = require(\"mathx\")\nprint(mathx.twice(2))\n",
    );
    let h = checker(&root);
    let c = h.check(&root.join("src/main.tl")).unwrap();
    assert!(c.ok());
    assert!(c.dependency_errors.is_empty(), "{:?}", c.dependency_errors);
}
