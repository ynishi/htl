//! htl's own asynchronous I/O for Teal: `require("std.fs")`, `require("std.proc")`.
//!
//! `std.*` is htl's re-export of Rust libraries to Teal. `crates/htl-core/src/batteries.rs`'s
//! slice of it is mlua-batteries, and deliberately narrow: its default feature set is json,
//! env, path, time, string, validate, pretty, argparse — modules that do not read or write
//! files (`path`'s own `absolute` still touches the filesystem, to canonicalise one; what
//! none of the default set does is move bytes into or out of one), reach the network, run
//! another process, or need a runtime. This crate is the asynchronous I/O slice of the same
//! namespace: `std.fs`, installed by the CLI through `htl_core::registry` and by a Rust
//! host through [`install`], and it offers no synchronous form beside it: a script that
//! wants a file without blocking the program while it waits reads it with `await`, and a
//! script that is content to block still has Lua's own `io`, global and untouched, in its
//! own namespace — this crate adds the asynchronous way beside it, not instead of it.
//! mlua-batteries has its own synchronous `fs` feature (outside its default set, so
//! htl-core's `std` feature does not carry it in); a host that turned that feature on
//! as well would have two things registering `require("std.fs")`, and whichever of the
//! two called `htl_preload` / `preload_all` last would shadow the other's under the
//! same name — not a conflict this crate resolves, since it has no way to see what
//! else is installed. [`proc`]'s
//! `std.proc` is the same decision applied to a child process rather than a file:
//! mlua-batteries has no synchronous `proc` feature at all, so there is nothing beside
//! it to shadow or be shadowed by.
//!
//! # Why a crate, and why not inside `htl-core`
//!
//! `#[host_module]` is `htl-macros`'s, built on top of `htl-core` rather than inside it,
//! and a crate that uses the macro to publish a `require`-able module is a consumer of
//! `htl`, the umbrella crate, the way `htl-mq`'s `Mq` is. [`Fs`] and [`Proc`] are exactly
//! that shape: a `#[host_module]` crate with nothing in it but the two modules, minus the
//! window `htl-mq` carries alongside its own.
//!
//! # One call, one blocking operation
//!
//! Every function here is exactly one `tokio::fs` call, with two exceptions. tokio's own
//! `fs` module doc says why that matters: file I/O there "will always use
//! `spawn_blocking`... behind the scenes", and "it is recommended to batch your
//! operations into as few `spawn_blocking` calls as possible" — each one hands work to a
//! thread pool and waits on it, and a function built out of several small ones pays that
//! cost several times over for no benefit a script watching it can see. [`Fs::read`],
//! [`Fs::write`], [`Fs::read_binary`], [`Fs::write_binary`], [`Fs::exists`],
//! [`Fs::is_file`], [`Fs::is_dir`] and [`Fs::mkdir`] are each a single call for exactly
//! this reason. [`Fs::remove`] is one call in the common case — `remove_file` succeeds
//! directly — and only when that fails does a second call (`symlink_metadata`) ask
//! whether the reason is that `path` is a directory: there is no single `tokio::fs`
//! primitive for "remove whichever kind this is", and the three platforms this crate
//! ships on (`dist-workspace.toml`'s release targets: Linux, macOS, Windows) do not
//! agree on what error `remove_file` gives for a directory — `IsADirectory` on Linux,
//! `PermissionDenied` on macOS, access-denied on Windows — so a check on that error's
//! kind could only ever have covered the one platform it was tested on; asking the
//! filesystem what `path` actually is covers all three, and a third call
//! (`remove_dir_all`) runs only once that answer says so. [`Fs::walk`] cannot be a
//! `tokio::fs` call at all, since there is no async directory-tree walk in it, so it is
//! one `spawn_blocking` of its own around a synchronous `walkdir` walk (which also
//! checks, inside that one call, that `path` is a directory before descending into it) —
//! the same rule, applied to the one function that has no `tokio::fs` primitive to lean
//! on in the first place. [`Proc::run`] is not a `tokio::fs` call at all, but the same
//! economy holds at the one level up: it is one child process run to completion, not
//! several — stdin written and closed, stdout and stderr drained, the exit (or the
//! timeout's kill) all inside that one call, rather than a `spawn` a caller would then
//! have to poll, write to and read from itself over several round trips to get the same
//! answer.
//!
//! # Cancellation
//!
//! A call a script writes with no `await` at all is not a cancellation story: the
//! checker's `await-missing` denies it before the program runs (`crates/htl-core/src/lint.rs`'s
//! own words for why — without `await` the call "returns a task the caller never reads,
//! and the work either never runs or runs after the caller has moved on, with the
//! failure surfacing at a line that did not cause it"), so `std.fs` never sees that case
//! at run time to begin with. What it does see is a future dropped out from under an
//! `await` that was there: a cancel of the whole program (Ctrl-C under `htl run`, or the
//! host's own cancel token) returns at the next await through mlua-isle's `cancellable`
//! (`#[host_module]`'s own doc, "With the `async` feature a method may be `async`",
//! `htl-macros/src/lib.rs`), or an `async local` task that is never `await`ed before its
//! scope ends is cancelled and waited for there — the shape `examples/embed`'s own
//! `Http::get` test exercises: the future for `/slow` is dropped (it prints `dropped`)
//! before the caller ever sees `/fast`'s result. Either way, dropping the Rust future a
//! `tokio::fs` or `spawn_blocking` call returned does not reach into the thread pool and
//! stop the operation already running there — there is nothing to cancel it with, short
//! of the process exiting — it only drops the channel the result would have come back
//! on. So a write or a walk already underway when its caller is cancelled runs to
//! completion on the pool, and its result, success or failure, is discarded unread.
//!
//! [`Proc::run`] answers the same dropped future differently, because a child process is
//! not a thread-pool call: dropping the future — the same program cancel or unawaited
//! `async local` as above — reaches the process rather than letting it run to
//! completion. File: the result is dropped and the operation completes. Process:
//! killed — see [`proc`]'s own doc for exactly what gets killed, on which platform, and
//! for the rest of the detail (the two tasks that drain its pipes, and how a
//! `timeout_ms` kill is the same signal under a different trigger).
//!
//! # Errors
//!
//! Every function that can fail does so by raising a Lua error (the macro's default;
//! nothing here asks for `errors = "return"`), with the OS message and the path both
//! baked into that one text — `mlua::Error::external`'s `Display` prints only the
//! outermost message it is given, not a chain underneath it, so the cause is written
//! into the message itself rather than attached beside it with `anyhow::Context`. A
//! script that wants to handle a failure rather than let it propagate wraps the call in
//! `pcall`:
//!
//! ```lua
//! local fs = require("std.fs")
//! local ok, err = pcall(async function(): string return await fs.read("/missing") end)
//! ```
//!
//! # Installing it
//!
//! ```rust,ignore
//! let h = htl::Htl::new()?;
//! h.install_std()?;      // std.json, std.string, ... — mlua-batteries, synchronous
//! htl_std::install(&h)?; // std.fs, std.proc — this crate, asynchronous
//! ```
//!
//! [`install`] also puts both declarations on `h`'s own checker search path
//! ([`Htl::install_declarations`]), so a state with no project around it — this crate's
//! own tests, a host's `Htl::new` with nothing else on the path — types `require("std.fs")`
//! and `require("std.proc")` the moment `install` returns, with no `[package.metadata.htl]
//! dts` of its own to ask for it. A project that depends on this crate still gets its own
//! copy through that `dts` / `dts_root` metadata, materialised under `types/htl-std/` by
//! `htl dts`, and that copy is the one the checker reads: the project's model answers
//! `require("std.fs")` from its own `types/` before `h`'s search path is ever consulted —
//! see [`htl::registry`]'s "Which copy wins when a project has one too" for why that is
//! the model's doing and not an ordering between this function and the project's own
//! directories.

use htl::mlua::{Lua, LuaString};
use htl::teal::HostModule;
use htl::{Htl, host_module};

pub mod proc;
pub use proc::Proc;

/// `Fs::MODULE` and `Proc::MODULE` — never a hand-written `"std.fs"` / `"std.proc"`, the
/// way [`htl::registry::register_installer`]'s own doc asks a registered crate's
/// `provides` to come from the registering crate's `#[host_module]` constants rather
/// than a second list a rename could leave behind. What a binary that links this crate
/// hands `register_installer` as `provides`.
pub const PROVIDES: &[&str] = &[Fs::MODULE, Proc::MODULE];

/// Stateless: every function is a plain `tokio::fs` (or `walkdir`) call with nothing of
/// its own to keep between them, so `require("std.fs")` is called with `.`, the way
/// `examples/embed`'s own `Http::get` is — `fs.read(p)`, not `fs:read(p)`.
pub struct Fs;

#[host_module(name = "std.fs", dts = "dts/std/fs.d.tl")]
impl Fs {
    /// Reads `path` as UTF-8 text. Raises if the file does not exist, is not valid UTF-8,
    /// or cannot otherwise be read.
    pub async fn read(path: String) -> anyhow::Result<String> {
        tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| anyhow::anyhow!("std.fs.read: {path}: {e}"))
    }

    /// Writes `text` to `path`, creating it if it does not exist and truncating it if it
    /// does. Does not create the path's parent directories; [`Fs::mkdir`] does that.
    pub async fn write(path: String, text: String) -> anyhow::Result<()> {
        tokio::fs::write(&path, text)
            .await
            .map_err(|e| anyhow::anyhow!("std.fs.write: {path}: {e}"))
    }

    /// [`Fs::read`] for bytes that need not be valid UTF-8: the Teal side still sees
    /// `string` (a Lua string holds any byte sequence), but the host hands back the raw
    /// bytes read rather than requiring them to decode. `lua` is filled from the call's
    /// own Lua state — see the `#[host_module]` doc, "The Lua state as a parameter" — so
    /// a script passes only `path`.
    pub async fn read_binary(path: String, lua: &Lua) -> anyhow::Result<LuaString> {
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|e| anyhow::anyhow!("std.fs.read_binary: {path}: {e}"))?;
        Ok(lua.create_string(bytes)?)
    }

    /// [`Fs::write`] for a `LuaString` that may hold bytes outside UTF-8, taken as
    /// written rather than through Rust's `String` and its validity requirement.
    pub async fn write_binary(path: String, bytes: LuaString) -> anyhow::Result<()> {
        let data = bytes.as_bytes().to_vec();
        tokio::fs::write(&path, data)
            .await
            .map_err(|e| anyhow::anyhow!("std.fs.write_binary: {path}: {e}"))
    }

    /// `true` if `path` can be reached at all — a file, a directory, anything `stat`
    /// resolves. Any failure to tell, permission denied included, reads as `false`
    /// rather than raising: the question this answers is binary.
    pub async fn exists(path: String) -> bool {
        tokio::fs::try_exists(&path).await.unwrap_or(false)
    }

    /// `true` if `path` resolves (following a symlink) to a regular file. `false` for
    /// anything else, missing path and unreadable metadata both included.
    pub async fn is_file(path: String) -> bool {
        tokio::fs::metadata(&path)
            .await
            .map(|m| m.is_file())
            .unwrap_or(false)
    }

    /// `true` if `path` resolves (following a symlink) to a directory. `false` for
    /// anything else, missing path and unreadable metadata both included.
    pub async fn is_dir(path: String) -> bool {
        tokio::fs::metadata(&path)
            .await
            .map(|m| m.is_dir())
            .unwrap_or(false)
    }

    /// Creates `path` as a directory, creating every missing parent along the way
    /// (`create_dir_all`). Does not raise if `path` already exists as a directory.
    pub async fn mkdir(path: String) -> anyhow::Result<()> {
        tokio::fs::create_dir_all(&path)
            .await
            .map_err(|e| anyhow::anyhow!("std.fs.mkdir: {path}: {e}"))
    }

    /// Removes `path`: a file with one call (`remove_file`); a directory, and everything
    /// below it, with that same call's failure followed by a check of what `path`
    /// actually is and then `remove_dir_all` — see the crate doc's "One call, one
    /// blocking operation" for why a kind check on the first error cannot tell the
    /// platforms this crate ships on apart, where asking the filesystem can. Raises
    /// `remove_file`'s own error (the OS message and `path`) when `path` turns out not to
    /// be a directory either — missing, no permission, or anything else `remove_file`
    /// already named correctly.
    pub async fn remove(path: String) -> anyhow::Result<()> {
        let first = match tokio::fs::remove_file(&path).await {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };
        match tokio::fs::symlink_metadata(&path).await {
            Ok(meta) if meta.is_dir() => tokio::fs::remove_dir_all(&path)
                .await
                .map_err(|e| anyhow::anyhow!("std.fs.remove: {path}: {e}")),
            _ => Err(anyhow::anyhow!("std.fs.remove: {path}: {first}")),
        }
    }

    /// Every regular file below `path`, recursively, as a path formed the same way
    /// `path` itself was given — relative in, relative out; absolute in, absolute out —
    /// sorted so the order is the same run to run regardless of the directory's own order
    /// on disk. `path` itself is not included, and neither is a symlink found during the
    /// walk: `walkdir`'s own default does not follow one into a directory, unlike
    /// [`Fs::is_file`] / [`Fs::is_dir`], which do. `path` itself is checked the way
    /// `is_dir` checks it (`metadata`, following a symlink), so a symlink to a directory
    /// walks that directory, as `walkdir` follows a root symlink too. Raises if `path`
    /// is not a directory (`std.fs.walk: <path>: not a directory` when it is a file) or
    /// if a name below it is not valid UTF-8, rather than silently changing it
    /// the way `Path::to_string_lossy` would. One `spawn_blocking` around the whole
    /// synchronous `walkdir` walk — see the crate doc's "One call, one blocking
    /// operation".
    pub async fn walk(path: String) -> anyhow::Result<Vec<String>> {
        let p = path.clone();
        tokio::task::spawn_blocking(move || walk_sync(&p))
            .await
            .map_err(|e| {
                anyhow::anyhow!("std.fs.walk: {path}: the walk's own task panicked: {e}")
            })?
    }
}

/// The blocking half of [`Fs::walk`], run on tokio's blocking pool rather than the
/// executor: a synchronous `walkdir::WalkDir` walk, filtered to regular files, collected
/// and sorted by path.
fn walk_sync(path: &str) -> anyhow::Result<Vec<String>> {
    let meta = std::fs::metadata(path).map_err(|e| anyhow::anyhow!("std.fs.walk: {path}: {e}"))?;
    if !meta.is_dir() {
        anyhow::bail!("std.fs.walk: {path}: not a directory");
    }
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(path) {
        let entry = entry.map_err(|e| anyhow::anyhow!("std.fs.walk: {path}: {e}"))?;
        if entry.file_type().is_file() {
            let p = entry.path();
            let s = p
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("std.fs.walk: {}: not valid UTF-8", p.display()))?;
            out.push(s.to_string());
        }
    }
    out.sort();
    Ok(out)
}

/// `Fs.htl_preload(h)` and `Proc.htl_preload(h)`: the two `require` entries at run time,
/// plus [`Htl::install_declarations`] for both their `.d.tl`, under `"htl-std"`, so a
/// state with no project around it is typed the moment this returns — the way
/// `Htl::install_std` leaves nothing for the checker to catch up
/// on either. A state built for a project that depends on this crate still ends up reading
/// the project's own copy: `dep_dts` (`crates/htl-core/src/dep_dts.rs`) materialises it
/// under `types/htl-std/` from `[package.metadata.htl] dts` / `dts_root`, and that copy is
/// what answers `require("std.fs")` — not because of any order between this call and the
/// project's own directories going on the path, but because the project's model answers
/// every name it has on its own, before the path this function wrote to is ever searched;
/// see [`htl::registry`]'s "Which copy wins when a project has one too". A binary that
/// registers this crate with `htl_core::registry::register_installer` instead of calling
/// this function directly gets the same two lines from
/// [`Htl::install_registered`](htl::Htl::install_registered), which runs whatever was
/// registered — this included.
pub fn install(h: &Htl) -> anyhow::Result<()> {
    Fs.htl_preload(h)?;
    Proc.htl_preload(h)?;
    h.install_declarations(
        "htl-std",
        &[("std/fs.d.tl", Fs::DECL), ("std/proc.d.tl", Proc::DECL)],
    )?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::walk_sync;

    /// A name below `path` that is not valid UTF-8 (built with raw bytes, since a Teal
    /// string literal — and the `.tl` source file it would sit in — is itself UTF-8) is
    /// reported rather than silently reinterpreted: `walk_sync` is exercised directly
    /// here because the only way to construct such a name from a script would be to
    /// write non-UTF-8 bytes into a Teal source file, which is not what this is testing.
    #[test]
    fn a_non_utf8_name_below_path_raises_instead_of_being_changed() {
        use std::os::unix::ffi::OsStrExt;

        let dir = std::env::temp_dir().join(format!(
            "htl-std-walk-sync-non-utf8-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let name = std::ffi::OsStr::from_bytes(b"not-utf8-\xFF\xFE");
        std::fs::write(dir.join(name), b"x").unwrap();

        let err = walk_sync(dir.to_str().unwrap()).unwrap_err().to_string();
        assert!(err.contains("std.fs.walk"), "{err}");
        assert!(err.contains("not valid UTF-8"), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
