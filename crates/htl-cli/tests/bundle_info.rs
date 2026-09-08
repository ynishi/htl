//! `htl bundle info` through the real binary: what a bundle records, printed without
//! running it, as text and as JSON; format 1 bundles; source payloads; not a bundle.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-bundle-info", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn htl(args: &[&str], cwd: &Path) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_htl"))
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// main -> util (.tl); main also requires `host`, declared only by a `.d.tl`.
fn project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(
        &root.join("src/main.tl"),
        "local util = require(\"util\")\nlocal host = require(\"host\")\n\
         print(util.twice(host.base()))\n",
    );
    write(
        &root.join("src/util.tl"),
        "local record util\nend\nfunction util.twice(n: integer): integer\n   return n * 2\nend\nreturn util\n",
    );
    write(
        &root.join("src/host.d.tl"),
        "local record host\n   base: function(): integer\nend\nreturn host\n",
    );
    root
}

fn build(root: &Path, extra: &[&str]) {
    let mut args = vec!["build", "src/main.tl", "-o", "app.hb"];
    args.extend_from_slice(extra);
    let (ok, _, stderr) = htl(&args, root);
    assert!(ok, "build: {stderr}");
}

#[test]
fn info_prints_what_a_bytecode_bundle_records() {
    let root = project("bytecode");
    build(&root, &[]);
    let (ok, stdout, stderr) = htl(&["bundle", "info", "app.hb"], &root);
    assert!(ok, "{stderr}");
    let expect_lua = if cfg!(target_endian = "little") {
        "Lua 5.4, format 0, 4/8/8, little-endian"
    } else {
        "Lua 5.4, format 0, 4/8/8, big-endian"
    };
    let want = format!(
        "app.hb: htl bundle, format 2, built by htl {}\n  payload:  bytecode\n  lua:      {expect_lua}\n  entry:    main\n  modules:  main, util\n  host:     host\n",
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(stdout, want);
}

#[test]
fn info_json_carries_the_same_fields() {
    let root = project("json");
    build(&root, &[]);
    let (ok, stdout, stderr) = htl(&["bundle", "info", "app.hb", "--format", "json"], &root);
    assert!(ok, "{stderr}");
    assert!(
        stderr.trim().is_empty(),
        "json mode keeps stderr silent: {stderr}"
    );
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["file"], "app.hb");
    assert_eq!(v["format"], 2);
    assert_eq!(v["htl_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(v["payload"], "bytecode");
    assert_eq!(v["lua"]["version"], "5.4");
    assert_eq!(v["lua"]["format"], 0);
    assert_eq!(v["lua"]["instruction_bytes"], 4);
    assert_eq!(v["lua"]["integer_bytes"], 8);
    assert_eq!(v["lua"]["number_bytes"], 8);
    assert_eq!(
        v["lua"]["endian"],
        if cfg!(target_endian = "little") {
            "little"
        } else {
            "big"
        }
    );
    assert_eq!(v["entry"], "main");
    let names: Vec<&str> = v["modules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["main", "util"]);
    assert!(
        v["modules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["kind"] == "bytecode" && m["bytes"].as_u64().unwrap() > 0)
    );
    assert_eq!(v["host_modules"], serde_json::json!(["host"]));
}

#[test]
fn a_source_bundle_binds_to_no_lua() {
    let root = project("source");
    build(&root, &["--source"]);
    let (ok, stdout, _) = htl(&["bundle", "info", "app.hb"], &root);
    assert!(ok);
    assert!(stdout.contains("  payload:  source\n"), "{stdout}");
    assert!(
        stdout.contains("  lua:      any (source payload, no bytecode to bind)\n"),
        "{stdout}"
    );
    let (_, json, _) = htl(&["bundle", "info", "app.hb", "--format", "json"], &root);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["payload"], "source");
    assert!(v["lua"].is_null(), "{v}");
}

#[test]
fn a_format_1_bundle_says_the_fingerprint_is_absent() {
    let root = scratch("v1");
    let mut v1 = b"HTLB\x01".to_vec();
    let put = |buf: &mut Vec<u8>, b: &[u8]| {
        buf.extend_from_slice(&(b.len() as u32).to_le_bytes());
        buf.extend_from_slice(b);
    };
    put(&mut v1, b"main");
    v1.extend_from_slice(&1u32.to_le_bytes());
    put(&mut v1, b"main");
    put(&mut v1, b"\x1bLua");
    std::fs::write(root.join("old.hb"), &v1).unwrap();

    let (ok, stdout, stderr) = htl(&["bundle", "info", "old.hb"], &root);
    assert!(ok, "{stderr}");
    assert_eq!(
        stdout,
        "old.hb: htl bundle, format 1, built by htl (version not recorded)\n  payload:  bytecode\n  lua:      not recorded (a format 1 bundle carries no fingerprint)\n  entry:    main\n  modules:  main\n  host:     (none)\n"
    );
    let (_, json, _) = htl(&["bundle", "info", "old.hb", "--format", "json"], &root);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["format"], 1);
    assert!(v["htl_version"].is_null(), "{v}");
    assert!(v["lua"].is_null(), "{v}");
    assert_eq!(v["host_modules"], serde_json::json!([]));
}

#[test]
fn not_a_bundle_is_refused() {
    let root = scratch("notabundle");
    std::fs::write(root.join("main.lua"), "return 1\n").unwrap();
    let (ok, _, stderr) = htl(&["bundle", "info", "main.lua"], &root);
    assert!(!ok);
    assert!(stderr.contains("not an htl bundle"), "{stderr}");
}
