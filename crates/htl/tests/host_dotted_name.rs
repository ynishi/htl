//! A dotted `#[host_module(name = "..")]`: `ns.inner` is the whole string `require` and
//! `package.preload` use, but a Teal declaration has room for one identifier where the
//! module sits (`local record ..`, `self: ..`, `return ..`), so that identifier is the
//! name's last segment, `inner`. See `htl_macros::host_module`'s doc for why.
//!
//! This file exercises `Htl` directly, with no project model involved: the declaration
//! text, a run through `require` and `package.preload`, and `Htl::add_path`'s own
//! search-root convention (`dir/<a>/<b>.d.tl` answers to `a.b`). The same split resolved
//! through a project's model (`#[host_module]` in a crate around a project, `htl check`,
//! `htl resolve`) is `crates/htl-cli/tests/host_dotted_module.rs`.

use htl::config::LangConfig;
use htl::teal::HostModule as _;
use htl::{Htl, host_module};
use std::path::Path;

mod common;

pub struct Inner;

#[host_module(
    name = "ns.inner",
    dts = "tests/fixtures/host_dotted_name/ns/inner.d.tl"
)]
impl Inner {
    pub fn greet(&self, who: String) -> String {
        format!("hello {who}")
    }
}

fn host() -> Htl {
    let h = Htl::new().unwrap();
    Inner.htl_preload(&h).unwrap();
    h
}

/// `MODULE` (and so `require`'s key, and `htl_preload`'s `package.preload` key) is the
/// whole dotted string; the declaration's record, every `self:`, and the `return` are all
/// `inner`, the name's last segment.
#[test]
fn the_declaration_splits_the_dotted_name() {
    assert_eq!(Inner::MODULE, "ns.inner");
    assert_eq!(
        Inner::DECL,
        "local record inner\n\
         \x20  greet: function(self: inner, who: string): string\n\
         end\n\
         \n\
         return inner\n"
    );
}

/// `require("ns.inner")` reaches the method at run time: `htl_preload` registered the
/// instance under the whole dotted key, not under `inner` alone.
#[test]
fn require_the_dotted_name_from_teal_calls_the_method() {
    let h = host();
    let got: String = h
        .lua()
        .load("local inner = require('ns.inner') return inner:greet('world')")
        .eval()
        .unwrap();
    assert_eq!(got, "hello world");
}

/// `Htl::add_path` reads the `.d.tl` the macro actually wrote — not a copy rewritten
/// into a tempdir — by pointing at the fixture directory itself
/// (`tests/fixtures/host_dotted_name/`, tracked, the way `task_channels.rs`'s
/// `daemon.d.tl` is) so `ns/inner.d.tl` under it answers to `ns.inner` under
/// `add_path`'s own `dir/<a>/<b>.d.tl` -> `a.b` rule; only `main.tl` goes in a tempdir.
/// Checked against (not only run with) the fixture's bytes, asserted equal to `DECL` the
/// way `task_channels.rs`'s
/// `the_generated_declaration_imports_htl_task_and_types_the_channels` checks its own
/// fixture.
#[test]
fn add_path_reads_the_fixture_the_macro_wrote_and_checks_against_it() {
    let fixture_dir = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/host_dotted_name"
    ));
    let written = std::fs::read_to_string(fixture_dir.join("ns/inner.d.tl")).unwrap();
    assert_eq!(written, Inner::DECL);

    let dir = common::tempdir("htl-host-dotted-name", "check");
    let main = dir.join("main.tl");
    std::fs::write(
        &main,
        "local inner = require(\"ns.inner\")\n\
         local s: string = inner:greet(\"world\")\n\
         print(s)\n",
    )
    .unwrap();

    let h = Htl::new().unwrap();
    h.set_lang(&LangConfig {
        async_: Some(false),
    })
    .unwrap();
    h.add_path(fixture_dir).unwrap();
    let ci = h.check(&main).unwrap();
    assert!(ci.errors.is_empty(), "{:#?}", ci.errors);
}
