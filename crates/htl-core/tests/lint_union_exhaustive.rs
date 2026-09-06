//! `union-exhaustive`: an `is` chain over a union that leaves a variant untested.
//!
//! The union's members come from the checker, not from the tests, so a chain that names
//! two variants is still measured against however many there are.

use htl_core::Htl;
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "htl-core-union-{name}-{}-{}",
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

/// Three `where`-discriminated records, then `body` in a file that requires them.
fn lints_of(name: &str, body: &str) -> Vec<String> {
    let dir = scratch(name);
    write(
        &dir.join("kinds.tl"),
        "local record kinds\n\
         \n   record A\n      where self.ty == \"A\"\n      ty: string\n      a: integer\n   end\n\
         \n   record B\n      where self.ty == \"B\"\n      ty: string\n      b: integer\n   end\n\
         \n   record C\n      where self.ty == \"C\"\n      ty: string\n      c: integer\n   end\n\
         end\nreturn kinds\n",
    );
    let src = format!("local kinds = require(\"kinds\")\n{body}");
    write(&dir.join("use.tl"), &src);
    let h = Htl::new().unwrap();
    h.add_path(&dir).unwrap();
    let ci = h.check(&dir.join("use.tl")).unwrap();
    assert!(ci.ok(), "unexpected type errors: {:?}", ci.errors);
    ci.lints
}

#[test]
fn a_variant_left_out_of_the_chain_is_named() {
    let lints = lints_of(
        "missing",
        "local function show(x: kinds.A | kinds.B | kinds.C)\n\
         \n   if x is kinds.A then\n      print(x.a)\n   elseif x is kinds.B then\n      print(x.b)\n   end\nend\nreturn show\n",
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert!(
        lints[0].contains("if-chain on 'x' does not cover C"),
        "{}",
        lints[0]
    );
    assert!(lints[0].contains("[htl union-exhaustive]"), "{}", lints[0]);
}

#[test]
fn every_variant_tested_says_nothing() {
    let lints = lints_of(
        "all",
        "local function show(x: kinds.A | kinds.B | kinds.C)\n\
         \n   if x is kinds.A then\n      print(x.a)\n   elseif x is kinds.B then\n      print(x.b)\n\
         \n   elseif x is kinds.C then\n      print(x.c)\n   end\nend\nreturn show\n",
    );
    assert!(lints.is_empty(), "{lints:?}");
}

#[test]
fn an_else_is_exhaustive_by_construction() {
    let lints = lints_of(
        "else",
        "local function show(x: kinds.A | kinds.B | kinds.C)\n\
         \n   if x is kinds.A then\n      print(x.a)\n   else\n      print(x.ty)\n   end\nend\nreturn show\n",
    );
    assert!(lints.is_empty(), "{lints:?}");
}

/// One test is a guard (an early case), not a dispatch over the union.
#[test]
fn a_single_test_is_not_a_dispatch() {
    let lints = lints_of(
        "guard",
        "local function show(x: kinds.A | kinds.B | kinds.C)\n\
         \n   if x is kinds.A then\n      print(x.a)\n   end\nend\nreturn show\n",
    );
    assert!(lints.is_empty(), "{lints:?}");
}

/// Every branch returns and code follows: the code after the chain is the `else`.
#[test]
fn a_fallthrough_after_returning_branches_counts_as_the_else() {
    let lints = lints_of(
        "fallthrough",
        "local function pick(x: kinds.A | kinds.B | kinds.C): integer\n\
         \n   if x is kinds.A then\n      return x.a\n   elseif x is kinds.B then\n      return x.b\n   end\n\
         \n   return 0\nend\nreturn pick\n",
    );
    assert!(lints.is_empty(), "{lints:?}");
}

/// Not a union: an `is` chain over something else has nothing to be exhaustive about.
#[test]
fn a_chain_over_a_non_union_is_left_alone() {
    let lints = lints_of(
        "nonunion",
        "local function show(x: kinds.A)\n\
         \n   if x is kinds.A then\n      print(x.a)\n   elseif x is kinds.A then\n      print(x.a)\n   end\nend\nreturn show\n",
    );
    assert!(lints.is_empty(), "{lints:?}");
}
