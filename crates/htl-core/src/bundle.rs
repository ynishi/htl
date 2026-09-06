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
//! All integers little-endian. Bytecode is only portable between hosts whose Lua
//! agrees with the fingerprint (version, instruction / integer / number sizes,
//! endianness); [`Htl::install_bundle`](crate::Htl::install_bundle) checks it and
//! says so, instead of Lua's bare "bad binary format". Source modules load anywhere.
//!
//! Version 1 bundles (`HTLB\x01`: entry + bytecode modules, no metadata) still decode.

use anyhow::{Result, bail};

pub const MAGIC: &[u8] = b"HTLB\x02";
const MAGIC_V1: &[u8] = b"HTLB\x01";

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

/// Human-readable form of a bytecode header (for mismatch messages).
pub fn describe_fingerprint(fp: &[u8]) -> String {
    // \x1bLua | version | format | LUAC_DATA(6) | sizeof(Instruction) | sizeof(lua_Integer) | sizeof(lua_Number) | LUAC_INT(8) | LUAC_NUM(8)
    if fp.len() < 15 {
        return format!("{} byte(s)", fp.len());
    }
    let ver = fp[4];
    let endian = if fp.len() >= 23 && fp[15] == 0x78 {
        "little-endian"
    } else {
        "big-endian"
    };
    format!(
        "Lua {}.{}, format {}, sizeof(Instruction)={} sizeof(Integer)={} sizeof(Number)={}, {endian}",
        ver >> 4,
        ver & 0xf,
        fp[5],
        fp[12],
        fp[13],
        fp[14]
    )
}
