//! Who sees the frames of a run-time failure, and what the frames are named.
//!
//! `user_message` and `developer_message` are the same cause with and without Lua's
//! `stack traceback:` block; the chunk name is what every frame in that block is
//! reported under, so a module registered without one still has to name a file.

use htl_core::{Htl, developer_message, user_message};

/// The two ends of the same error: a host function's `Err` reaches an embedding host's
/// users as its own text, and reaches whoever wrote the Teal with the frames below it.
#[test]
fn developer_message_keeps_the_frames_user_message_drops() {
    let h = Htl::new().unwrap();
    let host = h.lua().create_table().unwrap();
    host.set(
        "pages",
        h.lua()
            .create_function(|_, ()| -> mlua::Result<()> {
                Err(mlua::Error::external(
                    "content/no-date.md: front matter: 'date' is required",
                ))
            })
            .unwrap(),
    )
    .unwrap();
    h.preload_value("host", host).unwrap();

    let err = h
        .exec(
            "local host = require('host')\nhost.pages()\n",
            "@site.tl",
            &[],
        )
        .unwrap_err();

    let user = user_message(&err);
    assert_eq!(user, "content/no-date.md: front matter: 'date' is required");

    let dev = developer_message(&err);
    assert!(
        dev.starts_with(&user),
        "the cause comes first, unchanged: {dev}"
    );
    assert!(dev.contains("stack traceback:"), "{dev}");
    assert!(
        dev.contains("site.tl:2:"),
        "the frame that called the host function names the chunk: {dev}"
    );
}

/// Nothing to add when there is no traceback to add: `developer_message` is then exactly
/// what `user_message` returns, rather than an empty section.
#[test]
fn developer_message_is_user_message_when_there_are_no_frames() {
    let err = anyhow::anyhow!("reading app.hb: No such file or directory");
    assert_eq!(developer_message(&err), user_message(&err));
}

/// A module registered without a chunk name is named for the `.tl` a `require` of that
/// name would have found, so a frame in it points at something openable — not at `=name`,
/// which reads as a label and resolves nowhere.
#[test]
fn preload_names_the_tl_a_require_would_have_found() {
    let h = Htl::new().unwrap();
    h.preload(
        "app.helpers",
        "local M = {}\nfunction M.boom()\n   error('nope')\nend\nreturn M\n",
    )
    .unwrap();
    let err = h
        .exec("require('app.helpers').boom()\n", "@main.tl", &[])
        .unwrap_err();
    let msg = developer_message(&err);
    assert!(
        msg.contains("app/helpers.tl:3: nope"),
        "the dotted module name becomes a path: {msg}"
    );
    assert!(
        msg.contains("main.tl:1:"),
        "and the caller's frame is there too: {msg}"
    );
}

/// The exception the README names. Stripping drops the name a chunk was compiled under
/// along with its line numbers, and `lua_load`'s name does not stand in for it — so every
/// frame from a stripped payload is `?`. Pinned because it is what a reader of an embedded
/// host's failure meets, and because it is the reason `htl build --debug` exists.
#[test]
fn stripped_bytecode_has_no_name_and_no_lines_to_show() {
    let h = Htl::new().unwrap();
    let src = "local M = {}\nfunction M.boom()\n   error('nope')\nend\nreturn M\n";
    let bytecode = h.compile("shipped", src).unwrap();
    h.preload_bytes("shipped", &bytecode).unwrap();
    let err = h
        .exec("require('shipped').boom()\n", "@main.tl", &[])
        .unwrap_err();
    let msg = developer_message(&err);
    assert!(msg.contains("?:"), "no file, no line: {msg}");
    assert!(!msg.contains("shipped.tl"), "{msg}");
    assert!(
        msg.contains("main.tl:1:"),
        "the source chunk that called it still names itself: {msg}"
    );
}

/// A module no file backs says so: the host passes `=label`, and htl does not invent a
/// path for it. This is how the test library is registered.
#[test]
fn preload_at_takes_the_name_the_host_gives() {
    let h = Htl::new().unwrap();
    h.preload_at(
        "builtin",
        "=builtin",
        "local M = {}\nfunction M.boom()\n   error('nope')\nend\nreturn M\n",
    )
    .unwrap();
    let err = h
        .exec("require('builtin').boom()\n", "@main.tl", &[])
        .unwrap_err();
    let msg = developer_message(&err);
    assert!(msg.contains("builtin:3: nope"), "{msg}");
    assert!(!msg.contains("builtin.tl"), "no invented file: {msg}");
}
