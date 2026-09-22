//! A module that declares a `global` has an effect on the environment beyond its type:
//! walking it is what registers the name into `env.globals`. The checked-module store
//! carries a module's type and its result, and seeding those into a fresh environment
//! makes the vendored compiler return the result without walking the file — so the first
//! file that required the declaration saw the global and every later one did not.
//!
//! These cases pin the property the store must not break: a file's diagnostics do not
//! depend on what was checked before it.

use htl_core::Htl;
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-store-globals", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// What a host's declarations look like in a declaration file: a record type and two
/// values, all global. A declaration file has no bodies, so the function is declared as a
/// value of function type — a body-less `global function` is a syntax error even here.
/// Both of these are `global_type` and `global_declaration`; `global_function`, the third
/// kind that reaches `add_global`, needs a body and so belongs to `DECL_TL` below.
const DECL_DTL: &str = "\
global record std
   record Json
      encode: function(any): string
      decode: function(string): any
   end
   json: Json
end
global VERSION: string
global host_log: function(string)
";

/// The same names in an ordinary module rather than a declaration file, where the
/// function can be written as `global function` — the third AST kind.
const DECL_TL: &str = "\
global record std
   record Json
      encode: function(any): string
      decode: function(string): any
   end
   json: Json
end
global VERSION: string
global function host_log(msg: string)
   print(msg)
end
return {}
";

/// A module that uses all three globals it got from `host_globals`.
fn member(n: u32) -> String {
    format!(
        "require(\"host_globals\")

local record m{n}
end

function m{n}.run(): string
   host_log(VERSION)
   return std.json.encode({{ a = {n} }})
end

return m{n}
"
    )
}

/// A tree of `n` modules under `lib/`, each requiring the one declaration module. `decl`
/// is written where a project of that shape would put it.
fn tree(name: &str, decl_at: &str, decl: &str, n: u32) -> PathBuf {
    let root = scratch(name);
    write(&root.join(decl_at), decl);
    for i in 1..=n {
        write(&root.join(format!("lib/m{i}/init.tl")), &member(i));
    }
    root
}

/// One checker over `root`, with `lib/` and the declaration's directory on the path, the
/// way `[check] paths = ["lib", "types"]` would put them there.
fn checker(root: &Path, decl_dir: &str) -> Htl {
    let h = Htl::new().unwrap();
    h.reset_search_path().unwrap();
    h.add_search_paths(&[root.join("lib"), root.join(decl_dir)])
        .unwrap();
    h
}

/// Check the named members in the order given, in one checker, and return every error
/// as `file:message` so a failure says which file was blamed.
fn errors_in_order(root: &Path, decl_dir: &str, order: &[u32]) -> Vec<String> {
    let h = checker(root, decl_dir);
    let mut out = Vec::new();
    for n in order {
        let f = root.join(format!("lib/m{n}/init.tl"));
        let c = h.check(&f).unwrap();
        for e in c.errors {
            out.push(format!("m{n}: {e}"));
        }
    }
    out
}

fn assert_order_free(root: &Path, decl_dir: &str) {
    // Alone first: it is the baseline the other two have to reproduce, and it is the one
    // case that was right all along, so a failure here is about the fixture.
    for n in 1..=3 {
        let alone = errors_in_order(root, decl_dir, &[n]);
        assert!(alone.is_empty(), "m{n} alone: {alone:?}");
    }
    let forward = errors_in_order(root, decl_dir, &[1, 2, 3]);
    assert!(forward.is_empty(), "m1, m2, m3: {forward:?}");
    let backward = errors_in_order(root, decl_dir, &[3, 2, 1]);
    assert!(backward.is_empty(), "m3, m2, m1: {backward:?}");
}

#[test]
fn a_global_declared_in_a_d_tl_reaches_every_file_that_requires_it() {
    let root = tree("dtl", "types/host_globals.d.tl", DECL_DTL, 3);
    assert_order_free(&root, "types");
}

#[test]
fn a_global_declared_in_an_ordinary_module_reaches_every_file_that_requires_it() {
    let root = tree("tl", "lib/host_globals.tl", DECL_TL, 3);
    assert_order_free(&root, "lib");
}

/// The control: a module that declares no global is still replayed from the store, which
/// is what keeps the check of an unchanged project incremental.
///
/// What makes the replay observable is that the store answers about the file as it was
/// when it was checked: rewrite the module after the first requirer has been checked and
/// the second requirer still sees the old type. That staleness is the documented
/// behaviour of a seeded check (`Htl::check_written` exists for the caller that cannot
/// accept it), and here it is the only thing that separates "served from the store" from
/// "walked again".
#[test]
fn a_module_that_declares_no_global_is_still_served_from_the_store() {
    let root = scratch("no-global");
    let util = root.join("lib/util.tl");
    write(
        &util,
        "local record util\nend\nfunction util.f(): integer\n   return 1\nend\nreturn util\n",
    );
    for n in 1..=2 {
        write(
            &root.join(format!("lib/m{n}/init.tl")),
            &format!(
                "local util = require(\"util\")\nlocal record m{n}\nend\nfunction m{n}.run(): integer\n   return util.f()\nend\nreturn m{n}\n"
            ),
        );
    }

    let h = checker(&root, "lib");
    let c1 = h.check(&root.join("lib/m1/init.tl")).unwrap();
    assert!(c1.ok(), "{:?}", c1.errors);

    // util now says `string`. A walk in m2's environment would report the mismatch; the
    // store, which is what must happen here, answers `integer` as it was checked.
    write(
        &util,
        "local record util\nend\nfunction util.f(): string\n   return \"s\"\nend\nreturn util\n",
    );
    let c2 = h.check(&root.join("lib/m2/init.tl")).unwrap();
    assert!(
        c2.ok(),
        "util declares no global, so m2 must be served the stored result: {:?}",
        c2.errors
    );
}
