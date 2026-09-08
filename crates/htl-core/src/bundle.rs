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
    Bytecode,
    Source,
}

#[derive(Debug, Clone)]
pub struct Module {
    pub name: String,
    pub kind: Kind,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct Bundle {
    pub entry: String,
    /// Lua bytecode header of the compiling state (see [`crate::Htl::fingerprint`]);
    /// empty when no module is bytecode.
    pub fingerprint: Vec<u8>,
    pub htl_version: String,
    /// `require` names the bundle expects the host to provide (Rust `#[host_module]`s,
    /// `preload`s): declared only by a `.d.tl` at link time, or listed in `[build] host`.
    pub host_modules: Vec<String>,
    pub modules: Vec<Module>,
}

impl Bundle {
    pub fn is_bundle(bytes: &[u8]) -> bool {
        bytes.starts_with(MAGIC) || bytes.starts_with(MAGIC_V1)
    }

    pub fn module(&self, name: &str) -> Option<&Module> {
        self.modules.iter().find(|m| m.name == name)
    }

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
    pub instruction_bytes: u8,
    pub integer_bytes: u8,
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
