//! Bundle format (`.hb`): one program's modules in a single file.
//!
//! ```text
//! magic "HTLB\x02"
//! u32 len, fingerprint      (Lua bytecode header this bundle was compiled by, or empty)
//! u32 len, htl version
//! u32 len, entry module name
//! u32 count, count x ( u32 len, host module name )   modules the host must provide
//! u32 count, count x ( u8 kind, u32 len, module name, u32 len, payload )
//!   kind 0 = Lua 5.4 bytecode (from this build's mlua), kind 1 = Lua source
//! ```
//! All integers little-endian.
//!
//! # Portability
//!
//! Nothing about the CPU or the operating system is in a Lua chunk. What decides
//! whether bytecode loads is Lua's own chunk header, and that is what the fingerprint
//! is: the first 31 bytes of a dumped chunk — signature, version byte, format, the
//! `LUAC_DATA` probe, the sizes of `Instruction` / `lua_Integer` / `lua_Number`, and the
//! `LUAC_INT` / `LUAC_NUM` probes that detect integer endianness and float format.
//! [`Htl::install_bundle`](crate::Htl::install_bundle) compares it to the host's and
//! refuses on mismatch, naming both sides ([`LuaHeader`] is the readable form), instead
//! of Lua's bare "bad binary format".
//!
//! The Lua htl vendors has a 4-byte instruction, an 8-byte integer and an 8-byte double
//! on every 64-bit little-endian platform, so a bytecode bundle built on one of them
//! runs on all of them: an arm64 Mac's bundle loads on x86_64 Linux. What the check
//! refuses is a big-endian host, and a Lua built with a non-default `LUA_INT_TYPE` /
//! `LUA_FLOAT_TYPE`. Source modules (`--source`) load anywhere and are the answer for
//! those cases, and for a bundle that has to outlive a Lua upgrade.
//!
//! The header cannot tell one 5.4.x from another, and htl pins the vendored Lua through
//! mlua, so `htl version` is the only record of which Lua produced the bytes. It is
//! advisory: a bundle from an older htl whose header agrees still loads, and when the
//! header disagrees the mismatch message says which htl built the bundle and which is
//! running, since the header alone cannot say why two 5.4 builds differ.
//!
//! Version 1 bundles (`HTLB\x01`: entry + bytecode modules, no metadata) still decode;
//! [`format_version`] tells the two apart from the bytes.

use anyhow::{Result, bail};
use serde::Serialize;
use std::fmt;

/// What a bundle of the current format starts with. Public because a reader that has
/// bytes from somewhere — a file, an embedded slice — tells a bundle from a Lua chunk or
/// a script by this before it decides what to do with them; [`Bundle::is_bundle`] is the
/// same question asked of both versions at once.
pub const MAGIC: &[u8] = b"HTLB\x02";
const MAGIC_V1: &[u8] = b"HTLB\x01";

/// The format version a byte string carries (`1` for `HTLB\x01`, `2` for `HTLB\x02`),
/// or `None` when it is not a bundle at all. [`Bundle::decode`] folds the two into one
/// struct, so this is how a reader says which one it was given.
pub fn format_version(bytes: &[u8]) -> Option<u8> {
    if bytes.starts_with(MAGIC) {
        Some(2)
    } else if bytes.starts_with(MAGIC_V1) {
        Some(1)
    } else {
        None
    }
}

/// How a module's payload is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Dumped Lua chunk, from the mlua this build vendors. Smaller and skips parsing, and
    /// the reason a bundle carries a fingerprint at all: a host whose Lua chunk header
    /// disagrees cannot load it.
    Bytecode,
    /// Lua source. Loads under any 5.4, which is what `--source` is for — a big-endian
    /// host, a Lua built with other integer or float types, a bundle meant to outlive a
    /// Lua upgrade.
    Source,
}

/// One module in a bundle: the name a `require` asks for, and the bytes that answer it.
#[derive(Debug, Clone)]
pub struct Module {
    /// The `require` name, not a path. A bundle is loaded by name — what file it came
    /// from is the linker's business and is gone by the time it is written.
    pub name: String,
    /// Which of the two forms `payload` is in. Per module rather than per bundle: a
    /// single build can hold bytecode for what compiled and source for what did not.
    pub kind: Kind,
    /// The chunk itself, bytecode or source by `kind`. Bytes rather than a `String`
    /// because bytecode is not text.
    pub payload: Vec<u8>,
}

/// A whole program as one file: [`decode`](Self::decode)d from bytes,
/// [`encode`](Self::encode)d back to them, and installed into a state by
/// [`Htl::install_bundle`](crate::Htl::install_bundle).
#[derive(Debug, Clone, Default)]
pub struct Bundle {
    /// The module to run once the rest are registered. A name in `modules`, not a path.
    pub entry: String,
    /// Lua bytecode header of the compiling state (see [`crate::Htl::fingerprint`]);
    /// empty when no module is bytecode.
    pub fingerprint: Vec<u8>,
    /// Which htl built this, for the mismatch message. Advisory — a bundle from an older
    /// htl whose fingerprint agrees still loads — and recorded because the Lua chunk
    /// header cannot tell one 5.4.x from another, so nothing else says which Lua produced
    /// the bytes.
    pub htl_version: String,
    /// `require` names the bundle expects the host to provide (Rust `#[host_module]`s,
    /// `preload`s): declared only by a `.d.tl` at link time, or listed in `[build] host`.
    pub host_modules: Vec<String>,
    /// Every module the entry's require closure reached, the entry included. Order is the
    /// linker's; `require` finds them by name, so nothing depends on it.
    pub modules: Vec<Module>,
}

impl Bundle {
    /// Whether these bytes are a bundle of either format — the question a caller holding
    /// an unknown file asks before [`decode`](Self::decode), which fails on anything else.
    pub fn is_bundle(bytes: &[u8]) -> bool {
        bytes.starts_with(MAGIC) || bytes.starts_with(MAGIC_V1)
    }

    /// The module registered under `name`, or `None` when the bundle does not carry it —
    /// which for a name in [`host_modules`](Self::host_modules) is the expected answer.
    ///
    /// A scan rather than a map: a bundle is decoded once and read a handful of times, and
    /// building an index would cost more than the walks it saves.
    pub fn module(&self, name: &str) -> Option<&Module> {
        self.modules.iter().find(|m| m.name == name)
    }

    /// The bytes, in the format at the top of this module. Always the current version —
    /// `HTLB\x01` is decoded for bundles that already exist and never written.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(MAGIC);
        put_bytes(&mut buf, &self.fingerprint);
        put_bytes(&mut buf, self.htl_version.as_bytes());
        put_bytes(&mut buf, self.entry.as_bytes());
        buf.extend_from_slice(&(self.host_modules.len() as u32).to_le_bytes());
        for h in &self.host_modules {
            put_bytes(&mut buf, h.as_bytes());
        }
        buf.extend_from_slice(&(self.modules.len() as u32).to_le_bytes());
        for m in &self.modules {
            buf.push(match m.kind {
                Kind::Bytecode => 0,
                Kind::Source => 1,
            });
            put_bytes(&mut buf, m.name.as_bytes());
            put_bytes(&mut buf, &m.payload);
        }
        buf
    }

    /// A bundle of either format, read from bytes.
    ///
    /// Both versions land in this one struct, so a caller does not branch on which it was
    /// given; [`format_version`] is there for the one that wants to say. A version 1
    /// bundle carries no fingerprint, htl version or host modules, and its every module is
    /// bytecode, so those fields come back empty rather than guessed at.
    ///
    /// Every failure is about the bytes — bad magic, truncated, a module kind this build
    /// does not know, a name that is not UTF-8 — and none of them is about the Lua inside.
    /// Whether the bytecode loads is [`Htl::install_bundle`](crate::Htl::install_bundle)'s
    /// question, and it is asked against the fingerprint this returns.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.starts_with(MAGIC_V1) {
            return Self::decode_v1(&bytes[MAGIC_V1.len()..]);
        }
        if !bytes.starts_with(MAGIC) {
            bail!("not an htl bundle (bad magic)");
        }
        let mut cur = &bytes[MAGIC.len()..];
        let fingerprint = take_bytes(&mut cur)?.to_vec();
        let htl_version = String::from_utf8(take_bytes(&mut cur)?.to_vec())?;
        let entry = String::from_utf8(take_bytes(&mut cur)?.to_vec())?;
        let n = take_u32(&mut cur)? as usize;
        let mut host_modules = Vec::with_capacity(n);
        for _ in 0..n {
            host_modules.push(String::from_utf8(take_bytes(&mut cur)?.to_vec())?);
        }
        let count = take_u32(&mut cur)? as usize;
        let mut modules = Vec::with_capacity(count);
        for _ in 0..count {
            let kind = match take_u8(&mut cur)? {
                0 => Kind::Bytecode,
                1 => Kind::Source,
                k => bail!("unknown module kind {k} in bundle"),
            };
            let name = String::from_utf8(take_bytes(&mut cur)?.to_vec())?;
            let payload = take_bytes(&mut cur)?.to_vec();
            modules.push(Module {
                name,
                kind,
                payload,
            });
        }
        Ok(Self {
            entry,
            fingerprint,
            htl_version,
            host_modules,
            modules,
        })
    }

    fn decode_v1(mut cur: &[u8]) -> Result<Self> {
        let entry = String::from_utf8(take_bytes(&mut cur)?.to_vec())?;
        let count = take_u32(&mut cur)? as usize;
        let mut modules = Vec::with_capacity(count);
        for _ in 0..count {
            let name = String::from_utf8(take_bytes(&mut cur)?.to_vec())?;
            let payload = take_bytes(&mut cur)?.to_vec();
            modules.push(Module {
                name,
                kind: Kind::Bytecode,
                payload,
            });
        }
        Ok(Self {
            entry,
            modules,
            ..Default::default()
        })
    }
}

fn put_bytes(buf: &mut Vec<u8>, b: &[u8]) {
    buf.extend_from_slice(&(b.len() as u32).to_le_bytes());
    buf.extend_from_slice(b);
}

fn take_u8(cur: &mut &[u8]) -> Result<u8> {
    if cur.is_empty() {
        bail!("truncated bundle");
    }
    let b = cur[0];
    *cur = &cur[1..];
    Ok(b)
}

fn take_u32(cur: &mut &[u8]) -> Result<u32> {
    if cur.len() < 4 {
        bail!("truncated bundle");
    }
    let n = u32::from_le_bytes([cur[0], cur[1], cur[2], cur[3]]);
    *cur = &cur[4..];
    Ok(n)
}

fn take_bytes<'a>(cur: &mut &'a [u8]) -> Result<&'a [u8]> {
    let n = take_u32(cur)? as usize;
    if cur.len() < n {
        bail!("truncated bundle");
    }
    let (head, rest) = cur.split_at(n);
    *cur = rest;
    Ok(head)
}

/// What a fingerprint says, field by field: the Lua a bundle's bytecode was compiled
/// for. `Display` is the one-line form the mismatch message and `htl bundle info` use,
/// `Lua 5.4, format 0, 4/8/8, little-endian` (the three numbers are the sizes of
/// `Instruction`, `lua_Integer` and `lua_Number` in bytes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LuaHeader {
    /// `"5.4"`: the version byte, split.
    pub version: String,
    /// The bytecode format number (`0` for stock Lua).
    pub format: u8,
    /// `sizeof(Instruction)`. These three are what make a bundle portable or not: two
    /// hosts agreeing on all of them, the version and the endianness can load each
    /// other's bytecode whatever CPU or operating system they run on.
    pub instruction_bytes: u8,
    /// `sizeof(lua_Integer)`. `8` unless the host's Lua was built with another
    /// `LUA_INT_TYPE`, which is one of the two cases `--source` exists for.
    pub integer_bytes: u8,
    /// `sizeof(lua_Number)`. `8` — a double — unless the host's Lua was built with another
    /// `LUA_FLOAT_TYPE`.
    pub number_bytes: u8,
    /// `"little"` or `"big"`: how `LUAC_INT` came out.
    pub endian: &'static str,
}

impl LuaHeader {
    /// Read a fingerprint as [`crate::Htl::fingerprint`] produces it. `None` when it is
    /// too short to be one (a truncated or foreign byte string), which is reported as
    /// such rather than guessed at.
    pub fn parse(fp: &[u8]) -> Option<Self> {
        // \x1bLua | version | format | LUAC_DATA(6) | sizeof(Instruction) | sizeof(lua_Integer) | sizeof(lua_Number) | LUAC_INT(8) | LUAC_NUM(8)
        if fp.len() < 23 {
            return None;
        }
        let ver = fp[4];
        Some(Self {
            version: format!("{}.{}", ver >> 4, ver & 0xf),
            format: fp[5],
            instruction_bytes: fp[12],
            integer_bytes: fp[13],
            number_bytes: fp[14],
            // LUAC_INT is 0x5678: its low byte comes first on a little-endian host.
            endian: if fp[15] == 0x78 { "little" } else { "big" },
        })
    }
}

impl fmt::Display for LuaHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Lua {}, format {}, {}/{}/{}, {}-endian",
            self.version,
            self.format,
            self.instruction_bytes,
            self.integer_bytes,
            self.number_bytes,
            self.endian
        )
    }
}

/// Human-readable form of a bytecode header (for mismatch messages): the
/// [`LuaHeader`] line, or the byte count when the bytes are not a header.
pub fn describe_fingerprint(fp: &[u8]) -> String {
    match LuaHeader::parse(fp) {
        Some(h) => h.to_string(),
        None => format!("{} byte(s)", fp.len()),
    }
}
