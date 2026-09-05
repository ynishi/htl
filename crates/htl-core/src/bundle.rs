//! Stripped-bytecode bundle format (`.hb`).
//!
//! ```text
//! magic "HTLB\x01"
//! u32 len, entry module name
//! u32 count
//! count x ( u32 len, module name, u32 len, bytecode )
//! ```
//! All integers little-endian. Bytecode is Lua 5.4 as produced by this build's mlua;
//! bundles are only portable between htl binaries of the same Lua generation.

use anyhow::{Result, bail};

pub const MAGIC: &[u8] = b"HTLB\x01";

#[derive(Debug, Clone, Default)]
pub struct Bundle {
    pub entry: String,
    pub modules: Vec<(String, Vec<u8>)>,
}

impl Bundle {
    pub fn is_bundle(bytes: &[u8]) -> bool {
        bytes.starts_with(MAGIC)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(MAGIC);
        put_bytes(&mut buf, self.entry.as_bytes());
        buf.extend_from_slice(&(self.modules.len() as u32).to_le_bytes());
        for (name, bc) in &self.modules {
            put_bytes(&mut buf, name.as_bytes());
            put_bytes(&mut buf, bc);
        }
        buf
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if !Self::is_bundle(bytes) {
            bail!("not an htl bundle (bad magic)");
        }
        let mut cur = &bytes[MAGIC.len()..];
        let entry = String::from_utf8(take_bytes(&mut cur)?.to_vec())?;
        let count = take_u32(&mut cur)? as usize;
        let mut modules = Vec::with_capacity(count);
        for _ in 0..count {
            let name = String::from_utf8(take_bytes(&mut cur)?.to_vec())?;
            let bc = take_bytes(&mut cur)?.to_vec();
            modules.push((name, bc));
        }
        Ok(Self { entry, modules })
    }
}

fn put_bytes(buf: &mut Vec<u8>, b: &[u8]) {
    buf.extend_from_slice(&(b.len() as u32).to_le_bytes());
    buf.extend_from_slice(b);
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
