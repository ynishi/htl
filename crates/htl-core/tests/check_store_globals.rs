//! A module that declares a `global` has an effect on the environment beyond its type:
//! walking it is what registers the name into `env.globals`. The checked-module store
//! carries a module's type and its result, and seeding those into a fresh environment
//! makes the vendored compiler return the result without walking the file — so the first
//! file that required the declaration saw the global and every later one did not.
//!
//! These cases pin the property the store must not break: a file's diagnostics do not
//! depend on what was checked before it. The second half of that property is the one
//! #302 traded away: walking the declaring module once per requiring environment gives
//! every environment its own instance of each record the module declares, and Teal
//! compares records by instance — so a value made through one requirer could not be
//! passed to a function typed by another. One walk per run, and the globals delivered to
//! each environment that requires the module, is what these cases pin now.

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

/// Check the named files under `lib/` in the order given, in one checker, and return every
/// error as `<module>: message`.
fn errors_for(root: &Path, decl_dir: &str, order: &[&str]) -> Vec<String> {
    let h = checker(root, decl_dir);
    let mut out = Vec::new();
    for m in order {
        let f = root.join(format!("lib/{m}/init.tl"));
        let c = h.check(&f).unwrap();
        for e in c.errors {
            out.push(format!("{m}: {e}"));
        }
    }
    out
}

fn assert_files_order_free(root: &Path, decl_dir: &str, files: &[&str]) {
    for m in files {
        let alone = errors_for(root, decl_dir, &[m]);
        assert!(alone.is_empty(), "{m} alone: {alone:?}");
    }
    let forward = errors_for(root, decl_dir, files);
    assert!(forward.is_empty(), "{files:?}: {forward:?}");
    let mut rev: Vec<&str> = files.to_vec();
    rev.reverse();
    let backward = errors_for(root, decl_dir, &rev);
    assert!(backward.is_empty(), "{rev:?}: {backward:?}");
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

/// A value of a record the declaration file declares crosses a module boundary: `modx`
/// makes an `Ev`, `midx` takes one and hands it back to `modx`, and two consumers drive
/// that. With `midx` served from the store and `host_ev` walked again in each consumer's
/// environment, the `Ev` in `midx`'s signature and the `Ev` the consumer holds were two
/// instances of one declaration, and the call was an error naming the same line twice.
#[test]
fn a_record_a_global_declaring_file_declares_is_one_type_in_every_requirer() {
    let root = scratch("record-dtl");
    write(
        &root.join("types/host_ev.d.tl"),
        "global record Ev\n   kind: string\nend\n",
    );
    write(
        &root.join("lib/modx/init.tl"),
        "require(\"host_ev\")\nlocal record M\n   mk: function(): Ev\n   use: function(Ev): string\nend\n\
         function M.mk(): Ev return nil end\nfunction M.use(e: Ev): string return e.kind end\nreturn M\n",
    );
    write(
        &root.join("lib/midx/init.tl"),
        "require(\"host_ev\")\nlocal k = require(\"modx\")\nlocal record S\n   whole: function(Ev, string): string\nend\n\
         function S.whole(e: Ev, _who: string): string return k.use(e) end\nreturn S\n",
    );
    for (m, who) in [("topx", "x"), ("topy", "y")] {
        write(
            &root.join(format!("lib/{m}/init.tl")),
            &format!(
                "require(\"host_ev\")\nlocal k = require(\"modx\")\nlocal S = require(\"midx\")\n\
                 local function go(e: Ev): string return S.whole(e, \"{who}\") end\nprint(go(k.mk()))\n"
            ),
        );
    }
    assert_files_order_free(&root, "types", &["modx", "midx", "topx", "topy"]);
}

/// The same seam in an ordinary module: a `global` beside a `local record` that the
/// module exposes as a nested type. Nothing about the record is global; it split because
/// the file declaring it was walked once per requirer.
#[test]
fn a_record_declared_beside_a_global_is_one_type_in_every_requirer() {
    let root = scratch("record-tl");
    write(
        &root.join("lib/modx/init.tl"),
        "global knl: {string:any}\nlocal record SessionX\n   id: function(SessionX): string\nend\n\
         local record M\n   type Session = SessionX\n   open: function(): SessionX\n   use: function(SessionX): string\nend\n\
         function M.open(): SessionX return nil end\nfunction M.use(s: SessionX): string return s:id() end\nreturn M\n",
    );
    write(
        &root.join("lib/midx/init.tl"),
        "local k = require(\"modx\")\nlocal record S\n   whole: function(k.Session, string): string\nend\n\
         function S.whole(s: k.Session, _who: string): string return s:id() end\nreturn S\n",
    );
    for (m, who) in [("topx", "x"), ("topy", "y")] {
        write(
            &root.join(format!("lib/{m}/init.tl")),
            &format!(
                "local k = require(\"modx\")\nlocal S = require(\"midx\")\n\
                 local function go(s: k.Session): string return S.whole(s, \"{who}\") end\nprint(go(k.open()))\n"
            ),
        );
    }
    assert_files_order_free(&root, "lib", &["modx", "midx", "topx", "topy"]);
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

/// Check the named files under `lib/` in the order given and return the errors of the
/// last one only, as `<module>: message` — for a file whose error is the point.
fn errors_of_last(root: &Path, decl_dir: &str, order: &[&str]) -> Vec<String> {
    let h = checker(root, decl_dir);
    let mut out = Vec::new();
    for (i, m) in order.iter().enumerate() {
        let f = root.join(format!("lib/{m}/init.tl"));
        let c = h.check(&f).unwrap();
        if i + 1 == order.len() {
            out.extend(c.errors.into_iter().map(|e| format!("{m}: {e}")));
        }
    }
    out
}

/// A global reached through a chain: `host_v` declares it, `modx` requires `host_v`,
/// `midx` requires only `modx`, `topx` requires only `midx`. In an environment where `modx`
/// is taken from the store its own `require("host_v")` never runs, so the global has to
/// travel with `modx` — everything a module's requires declare, transitively, is what a
/// `require` of it delivers. Without that, `midx` and `topx` were told `unknown variable`
/// exactly when `modx` had been checked before them.
fn chain_tree(name: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("types/host_v.d.tl"), "global VERSION: string\n");
    write(
        &root.join("lib/modx/init.tl"),
        "require(\"host_v\")\nlocal record M\n   v: function(): string\nend\n\
         function M.v(): string return VERSION end\nreturn M\n",
    );
    write(
        &root.join("lib/midx/init.tl"),
        "local k = require(\"modx\")\nlocal record S\n   both: function(): string\nend\n\
         function S.both(): string return k.v() .. VERSION end\nreturn S\n",
    );
    write(
        &root.join("lib/topx/init.tl"),
        "local s = require(\"midx\")\nprint(s.both() .. VERSION)\n",
    );
    root
}

#[test]
fn a_global_travels_the_require_chain_to_a_file_that_never_requires_its_declaration_directly() {
    let root = chain_tree("chain-value");
    assert_files_order_free(&root, "types", &["modx", "midx", "topx"]);
}

/// The same chain for a record type: `Ev` is declared global by `host_ev`, which only
/// `modx` requires; `midx` and the consumers reach `Ev` through `modx`, and a value made
/// by `modx` crosses `midx`'s signature — which is only well-typed if every environment
/// on the chain holds the one instance the single walk produced.
#[test]
fn a_global_record_travels_the_require_chain_as_one_instance() {
    let root = scratch("chain-record");
    write(
        &root.join("types/host_ev.d.tl"),
        "global record Ev\n   kind: string\nend\n",
    );
    write(
        &root.join("lib/modx/init.tl"),
        "require(\"host_ev\")\nlocal record M\n   mk: function(): Ev\n   use: function(Ev): string\nend\n\
         function M.mk(): Ev return nil end\nfunction M.use(e: Ev): string return e.kind end\nreturn M\n",
    );
    write(
        &root.join("lib/midx/init.tl"),
        "local k = require(\"modx\")\nlocal record S\n   whole: function(Ev, string): string\nend\n\
         function S.whole(e: Ev, _who: string): string return k.use(e) end\nreturn S\n",
    );
    for (m, who) in [("topx", "x"), ("topy", "y")] {
        write(
            &root.join(format!("lib/{m}/init.tl")),
            &format!(
                "local k = require(\"modx\")\nlocal S = require(\"midx\")\n\
                 local function go(e: Ev): string return S.whole(e, \"{who}\") end\nprint(go(k.mk()))\n"
            ),
        );
    }
    assert_files_order_free(&root, "types", &["modx", "midx", "topx", "topy"]);
}

/// The other half of delivering at the `require`: a file that reads the global and
/// requires nothing gets `unknown variable`, whether or not the declaring module was
/// checked into the store before it. The global is visible where its module was required
/// and nowhere else.
#[test]
fn a_file_that_requires_nothing_does_not_see_a_global_the_store_holds() {
    let root = chain_tree("chain-loose");
    write(&root.join("lib/loose/init.tl"), "print(VERSION)\n");
    for order in [
        &["loose"][..],
        &["modx", "midx", "loose"][..],
        &["midx", "modx", "loose"][..],
        &["topx", "loose"][..],
    ] {
        let errs = errors_of_last(&root, "types", order);
        assert_eq!(errs.len(), 1, "{order:?}: {errs:?}");
        assert!(
            errs[0].contains("unknown variable: VERSION"),
            "{order:?}: {errs:?}"
        );
    }
}
