//! The exhaustiveness lints over declarations `#[derive(TealRecord)]` writes: an enum
//! derived from a Rust enum, and a union derived from a data-carrying one, are checked
//! exactly as their hand-written forms are.
#![cfg(feature = "dts")]

use htl_core::Htl;
use htl_core::dts;
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-derived", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn lints_of(dir: &Path, file: &str) -> Vec<String> {
    let h = Htl::new().unwrap();
    h.add_path(dir).unwrap();
    let ci = h.check(&dir.join(file)).unwrap();
    assert!(ci.ok(), "unexpected type errors: {:?}", ci.errors);
    ci.lints
}

/// A derived unit enum in a `.d.tl` module of its own, imported with `local type`.
#[test]
fn enum_exhaustive_sees_a_derived_enum() {
    let dir = scratch("enum");
    let item: syn::Item =
        syn::parse_str("#[derive(TealRecord)] pub enum Behavior { Chase, Wander, Flee }").unwrap();
    let rd = dts::record_decl(&item).unwrap();
    write(&dir.join("Behavior.d.tl"), &rd.decl);
    write(
        &dir.join("ai.tl"),
        "local type Behavior = require(\"Behavior\")\n\n\
         local function act(b: Behavior)\n   if b == \"Chase\" then\n      print(1)\n   elseif b == \"Wander\" then\n      print(2)\n   end\nend\n\nact(\"Chase\")\n",
    );
    let lints = lints_of(&dir, "ai.tl");
    assert!(
        lints
            .iter()
            .any(|l| l.contains("enum-exhaustive") && l.contains("Flee")),
        "expected enum-exhaustive over the derived enum, got {lints:?}"
    );
}

/// Renaming the variants does not hide them from the lints: the `enum` entries are the
/// renamed words, and a chain that leaves one out is still reported by its word.
#[test]
fn enum_exhaustive_sees_the_renamed_words() {
    let dir = scratch("enum-renamed");
    let item: syn::Item = syn::parse_str(
        "#[derive(TealRecord)] #[teal(rename_all = \"snake_case\")] pub enum State { Open, InReview, Closed }",
    )
    .unwrap();
    let rd = dts::record_decl(&item).unwrap();
    write(&dir.join("State.d.tl"), &rd.decl);
    write(
        &dir.join("flow.tl"),
        "local type State = require(\"State\")\n\n\
         local function act(s: State)\n   if s == \"open\" then\n      print(1)\n   elseif s == \"in_review\" then\n      print(2)\n   end\nend\n\nact(\"open\")\n",
    );
    let lints = lints_of(&dir, "flow.tl");
    assert!(
        lints
            .iter()
            .any(|l| l.contains("enum-exhaustive") && l.contains("closed")),
        "expected enum-exhaustive naming the renamed word, got {lints:?}"
    );
}

/// A renamed data enum: the `where` clauses carry the renamed words while the variant
/// records keep their Rust names, and `union-exhaustive` counts them as it always did.
#[test]
fn union_exhaustive_counts_the_variants_of_a_renamed_union() {
    let dir = scratch("union-renamed");
    let file: syn::File = syn::parse_str(
        "#[derive(TealRecord)] #[teal(rename_all = \"snake_case\")] pub enum Step { Idle, InReview(f64), NeedsWork { why: String } }\n\
         pub struct Host;\n\
         #[host_module(name = \"host\", records = [Step])]\n\
         impl Host {\n    pub fn any(&self) -> Step { todo!() }\n}\n",
    )
    .unwrap();
    let imp = file
        .items
        .iter()
        .find_map(|i| match i {
            syn::Item::Impl(imp) => Some(imp),
            _ => None,
        })
        .unwrap();
    let attrs = dts::parse_host_module_attr(&imp.attrs).unwrap().unwrap();
    let hd = dts::host_decl(imp, attrs, Some(&file.items)).unwrap();
    assert!(
        hd.decl.contains("where self.kind == \"needs_work\""),
        "{}",
        hd.decl
    );
    write(&dir.join("host.d.tl"), &hd.decl);
    write(
        &dir.join("use.tl"),
        "local type host = require(\"host\")\n\n\
         local function n(s: host.Step): number\n\
         \n   if s is host.Step_Idle then\n      return 0\n   elseif s is host.Step_InReview then\n      return s.value\n   end\nend\n\nreturn n\n",
    );
    let lints = lints_of(&dir, "use.tl");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert!(
        lints[0].contains("[htl union-exhaustive]") && lints[0].contains("Step_NeedsWork"),
        "{}",
        lints[0]
    );
}

/// A derived data enum nested in a host module: `is host.Shape_<Variant>` chains are
/// measured against every variant the Rust enum has.
#[test]
fn union_exhaustive_sees_a_derived_union() {
    let dir = scratch("union");
    let file: syn::File = syn::parse_str(
        "#[derive(TealRecord)] pub enum Shape { Dot, Circle(f64), Rect { w: f64, h: f64 } }\n\
         pub struct Host;\n\
         #[host_module(name = \"host\", records = [Shape])]\n\
         impl Host {\n    pub fn any(&self) -> Shape { todo!() }\n}\n",
    )
    .unwrap();
    let imp = file
        .items
        .iter()
        .find_map(|i| match i {
            syn::Item::Impl(imp) => Some(imp),
            _ => None,
        })
        .unwrap();
    let attrs = dts::parse_host_module_attr(&imp.attrs).unwrap().unwrap();
    let hd = dts::host_decl(imp, attrs, Some(&file.items)).unwrap();
    write(&dir.join("host.d.tl"), &hd.decl);
    write(
        &dir.join("use.tl"),
        "local type host = require(\"host\")\n\n\
         local function area(s: host.Shape): number\n\
         \n   if s is host.Shape_Dot then\n      return 0\n   elseif s is host.Shape_Circle then\n      return s.value * s.value\n   end\nend\n\nreturn area\n",
    );
    let lints = lints_of(&dir, "use.tl");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert!(
        lints[0].contains("[htl union-exhaustive]") && lints[0].contains("Shape_Rect"),
        "{}",
        lints[0]
    );
}
