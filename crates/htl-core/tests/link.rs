//! `htl::link`: the require closure of an entry as a bundle; host-provided modules;
//! unresolved requires; source vs bytecode; running the result.

use htl_core::Htl;
use htl_core::bundle::{Bundle, Kind};
use htl_core::link::{LinkOptions, link};
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "htl-core-link-{name}-{}-{}",
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

/// entry -> util (.tl) -> mathx (vendored .lua); entry also requires `host` (.d.tl only)
fn project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(
        &root.join("src/main.tl"),
        "local util = require(\"util\")\nlocal host = require(\"host\")\nlocal unused = require(\"unused\")\n\
         print(util.twice(host.base()), unused.x)\nreturn util.twice(host.base())\n",
    );
    write(
        &root.join("src/util.tl"),
        "local mathx = require(\"mathx\")\nlocal record util\nend\nfunction util.twice(n: integer): integer\n   return mathx.double(n)\nend\nreturn util\n",
    );
    write(&root.join("src/unused.tl"), "return { x = 1 }\n");
    write(&root.join("src/host.d.tl"), "local record host\n   base: function(): integer\nend\nreturn host\n");
    write(&root.join("src/mathx.d.tl"), "local record mathx\n   double: function(integer): integer\nend\nreturn mathx\n");
    write(&root.join("vendor/mathx.lua"), "local M = {}\nfunction M.double(n) return n * 2 end\nreturn M\n");
    write(&root.join("src/orphan.tl"), "return { orphan = true }\n");
    root
}

fn checker(root: &Path) -> Htl {
    let h = Htl::new().unwrap();
    h.add_path(&root.join("src")).unwrap();
    h.add_path(&root.join("vendor")).unwrap();
    h
}

#[test]
fn links_the_closure_and_records_host_modules() {
    let root = project("closure");
    let h = checker(&root);
    let linked = link(&h, &root.join("src/main.tl"), &LinkOptions::default()).unwrap();
    assert!(linked.errors.is_empty(), "{:?}", linked.errors);
    let names: Vec<&str> = linked.modules.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, vec!["main", "util", "unused", "mathx"], "BFS from the entry; orphan.tl untouched");
    assert_eq!(linked.host_modules, vec!["host".to_string()], "declared only by host.d.tl");
    assert!(linked.modules.iter().find(|m| m.name == "mathx").unwrap().path.ends_with("vendor/mathx.lua"));
    assert!(!linked.modules.iter().find(|m| m.name == "mathx").unwrap().typed);
    assert_eq!(linked.bundle.entry, "main");
    assert!(!linked.bundle.fingerprint.is_empty());
    assert!(linked.bundle.modules.iter().all(|m| m.kind == Kind::Bytecode));
    // `.d.tl` for mathx exists next to the .lua: the typed check used the declaration,
    // the bundle carries the implementation.
}

#[test]
fn unresolved_require_is_a_link_error_unless_declared_host() {
    let root = project("unresolved");
    write(&root.join("src/main.tl"), "local ghost = require(\"ghost\")\nprint(ghost)\n");
    let h = checker(&root);
    let linked = link(&h, &root.join("src/main.tl"), &LinkOptions::default()).unwrap();
    // The checker already reports the missing module; the linker adds where to declare it.
    assert!(linked.errors.iter().any(|e| e.contains("require(\"ghost\") is not on the search path")), "{:?}", linked.errors);
    assert!(linked.errors.iter().any(|e| e.contains("[build] host") && e.contains("[build] extra")), "{:?}", linked.errors);
}

#[test]
fn extra_modules_are_bundled_for_dynamic_requires() {
    let root = project("extra");
    write(&root.join("src/main.tl"), "local name = \"plug\" .. \"in\"\nlocal p = require(name)\nprint(p)\n");
    write(&root.join("src/plugin.tl"), "return { plugged = true }\n");
    let h = checker(&root);
    let opts = LinkOptions { extra: vec!["plugin".into()], ..Default::default() };
    let linked = link(&h, &root.join("src/main.tl"), &opts).unwrap();
    assert!(linked.errors.is_empty(), "{:?}", linked.errors);
    assert!(linked.bundle.module("plugin").is_some());
}

#[test]
fn bundle_runs_with_host_module_and_refuses_without() {
    let root = project("run");
    let h = checker(&root);
    let linked = link(&h, &root.join("src/main.tl"), &LinkOptions::default()).unwrap();
    assert!(linked.errors.is_empty(), "{:?}", linked.errors);
    let bytes = linked.bundle.encode();
    let b = Bundle::decode(&bytes).unwrap();
    assert_eq!(b.host_modules, vec!["host".to_string()]);
    assert_eq!(b.htl_version, env!("CARGO_PKG_VERSION"));

    // A fresh state without the host module: refused up front, naming it.
    let bare = Htl::new().unwrap();
    let err = bare.run_bundle(&b, &[]).unwrap_err().to_string();
    assert!(err.contains("host-provided module(s) 'host'"), "{err}");

    // With it: runs, and the entry's return value / loader contract hold.
    let r = Htl::new().unwrap();
    let t = r.lua().create_table().unwrap();
    t.set("base", r.lua().create_function(|_, ()| Ok(21)).unwrap()).unwrap();
    r.preload_value("host", t).unwrap();
    r.install_bundle(&b).unwrap();
    // `...` in a bundled module: (modname, "bundle:<name>") like a file searcher.
    let (name, origin): (String, String) = r
        .lua()
        .load("local a, b = ... return a, b")
        .into_function()
        .unwrap()
        .call(("x", "y"))
        .unwrap();
    assert_eq!((name.as_str(), origin.as_str()), ("x", "y"));
    let v: i64 = r.lua().load("return require('util').twice(21)").eval().unwrap();
    assert_eq!(v, 42, "util -> vendored mathx.lua, both from the bundle");
    let origin: String = r.lua().load("return select(2, package.searchers[2]('util'))").eval().unwrap();
    assert_eq!(origin, "bundle:util");
}

#[test]
fn source_bundles_carry_no_fingerprint_and_run() {
    let root = project("source");
    let h = checker(&root);
    let opts = LinkOptions { source: true, ..Default::default() };
    let linked = link(&h, &root.join("src/main.tl"), &opts).unwrap();
    assert!(linked.bundle.fingerprint.is_empty());
    assert!(linked.bundle.modules.iter().all(|m| m.kind == Kind::Source));
    let src = std::str::from_utf8(&linked.bundle.module("util").unwrap().payload).unwrap();
    assert!(src.contains("mathx.double"), "generated Lua, readable: {src}");
    let r = Htl::new().unwrap();
    let t = r.lua().create_table().unwrap();
    t.set("base", r.lua().create_function(|_, ()| Ok(1)).unwrap()).unwrap();
    r.preload_value("host", t).unwrap();
    r.install_bundle(&linked.bundle).unwrap();
    let v: i64 = r.lua().load("return require('util').twice(4)").eval().unwrap();
    assert_eq!(v, 8);
}

#[test]
fn foreign_bytecode_is_refused_with_a_readable_message() {
    let root = project("fp");
    let h = checker(&root);
    let mut linked = link(&h, &root.join("src/main.tl"), &LinkOptions::default()).unwrap();
    // Pretend the bundle came from a 32-bit Lua: sizeof(lua_Integer) byte differs.
    linked.bundle.fingerprint[13] = 4;
    let r = Htl::new().unwrap();
    let err = r.install_bundle(&linked.bundle).unwrap_err().to_string();
    assert!(err.contains("compiled for Lua 5.4") && err.contains("sizeof(Integer)=4"), "{err}");
    assert!(err.contains("this host runs Lua 5.4") && err.contains("sizeof(Integer)=8"), "{err}");
    assert!(err.contains("--source"), "{err}");
}

#[test]
fn debug_bytecode_keeps_line_numbers() {
    let root = project("debug");
    write(&root.join("src/main.tl"), "local function boom()\n   error(\"kaboom\")\nend\nboom()\n");
    let h = checker(&root);
    let stripped = link(&h, &root.join("src/main.tl"), &LinkOptions::default()).unwrap();
    let debug = link(&h, &root.join("src/main.tl"), &LinkOptions { debug: true, ..Default::default() }).unwrap();
    let run = |b: &Bundle| Htl::new().unwrap().run_bundle(b, &[]).unwrap_err().to_string();
    let e1 = run(&stripped.bundle);
    let e2 = run(&debug.bundle);
    assert!(e2.contains("main:2:"), "debug info keeps the line: {e2}");
    assert!(!e1.contains("main:2:"), "stripped has no line: {e1}");
}

#[test]
fn version_1_bundles_still_decode() {
    let mut v1 = b"HTLB\x01".to_vec();
    let put = |buf: &mut Vec<u8>, b: &[u8]| {
        buf.extend_from_slice(&(b.len() as u32).to_le_bytes());
        buf.extend_from_slice(b);
    };
    put(&mut v1, b"main");
    v1.extend_from_slice(&1u32.to_le_bytes());
    put(&mut v1, b"main");
    put(&mut v1, b"\x1bLua");
    let b = Bundle::decode(&v1).unwrap();
    assert_eq!(b.entry, "main");
    assert_eq!(b.modules.len(), 1);
    assert!(b.host_modules.is_empty() && b.fingerprint.is_empty());
}
