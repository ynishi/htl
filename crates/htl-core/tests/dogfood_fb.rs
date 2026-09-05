//! Feedback from the sgen / tsk dogfooding: self-require on case-insensitive
//! filesystems, run-time-only requires, and user-facing error text.

use htl_core::{Htl, user_message};
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "htl-core-fb-{name}-{}-{}",
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

/// `require("site")` from `Site.tl` on a case-insensitive filesystem finds the requiring
/// file itself; the error must say so instead of a bare "no type information".
#[test]
fn self_require_on_case_insensitive_fs_is_explained() {
    let dir = scratch("selfreq");
    write(
        &dir.join("Site.tl"),
        "local site = require(\"site\")\nlocal c: site.Config = {}\nprint(c)\n",
    );
    // Only meaningful where `site.tl` resolves to `Site.tl`.
    if !dir.join("site.tl").is_file() {
        eprintln!("skipped: case-sensitive filesystem");
        return;
    }
    let h = Htl::new().unwrap();
    h.add_path(&dir).unwrap();
    let ci = h.check(&dir.join("Site.tl")).unwrap();
    assert!(!ci.ok(), "self-require must not type-check");
    assert!(
        ci.errors.iter().any(|e| e.contains("the requiring file itself") && e.contains("case-insensitive")),
        "{:?}",
        ci.errors
    );
}

/// A module that exists only at run time (the user's `Tasks.tl`, loaded by a host built
/// long before) is declared by a `.d.tl` in the host's tree: the build checks against the
/// declaration, the run resolves the real file. No dynamic require, no `any`.
#[test]
fn runtime_provided_module_is_declared_by_a_dts() {
    let sdk = scratch("sdk");
    write(
        &sdk.join("tsk.tl"),
        "local record tsk\n   record Tasks\n      name: string\n   end\nend\nreturn tsk\n",
    );
    // Contract for the user's file. Only types; nothing to run.
    write(&sdk.join("Tasks.d.tl"), "local tsk = require(\"tsk\")\nlocal Tasks: tsk.Tasks\nreturn Tasks\n");
    write(
        &sdk.join("main.tl"),
        "local tasks = require(\"Tasks\")\nprint(\"task: \" .. tasks.name)\n",
    );

    // Build time (host's tree only): the literal require type-checks against Tasks.d.tl.
    let h = Htl::new().unwrap();
    h.add_path(&sdk).unwrap();
    let (code, ci) = h.gen_lua(&sdk.join("main.tl")).unwrap();
    assert!(ci.ok(), "{:?}", ci.errors);
    let code = code.unwrap();

    // Run time (user's project): the real Tasks.tl is served by a resolver rooted there.
    let project = scratch("project");
    write(&project.join("Tasks.tl"), "return { name = \"build\" }\n");
    let rt = Htl::new().unwrap();
    let mut reg = mlua_pkg::Registry::new();
    reg.add(htl_core::pkg::TealResolver::new(&project).unwrap().expect_type("tsk.Tasks"));
    reg.install(rt.lua()).unwrap();
    rt.add_path(&sdk).unwrap(); // tsk.tl for expect_type's checker
    rt.exec(&code, "=main.tl", &[]).unwrap();
}

/// A host function's `Err` reaches the user as its own text, without Lua's traceback.
#[test]
fn user_message_strips_traceback_and_unwraps_host_errors() {
    let h = Htl::new().unwrap();
    let host = h.lua().create_table().unwrap();
    host.set(
        "pages",
        h.lua()
            .create_function(|_, ()| -> mlua::Result<()> {
                Err(mlua::Error::external("content/no-date.md: front matter: 'date' is required"))
            })
            .unwrap(),
    )
    .unwrap();
    h.preload_value("host", host).unwrap();

    let err = h
        .exec("local host = require('host')\nhost.pages()\n", "=main.lua", &[])
        .unwrap_err();
    let raw = err.to_string();
    assert!(raw.contains("stack traceback"), "precondition: mlua includes a traceback: {raw}");
    let msg = user_message(&err);
    assert_eq!(msg, "content/no-date.md: front matter: 'date' is required");

    // A plain Lua error keeps its `file:line:` prefix but drops the traceback.
    let err = h.exec("error('boom')", "=s.lua", &[]).unwrap_err();
    let msg = user_message(&err);
    assert!(msg.ends_with("s.lua:1: boom"), "{msg}");
    assert!(!msg.contains("traceback"), "{msg}");
}
