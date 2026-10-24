//! U-Boot environment (`crc32 LE` + optional 1-byte flags + `key=value\0`... + `\0`).

use anyhow::{bail, Result};

#[derive(Debug, Clone)]
pub struct Env {
    /// `Some(flag)` if the image uses a 5-byte redundant-env header (`crc32 + flags`),
    /// `None` if it uses a 4-byte header (`crc32` only).
    pub flags: Option<u8>,
    pub vars: Vec<(String, String)>,
    /// First byte covered by the stored CRC; sunxi and redundant envs differ.
    pub crc_start: usize,
}

impl Env {
    /// Parse and verify CRC. `blob` is the whole env partition.
    pub fn parse(blob: &[u8]) -> Result<Self> {
        if blob.len() < 8 {
            bail!("env too small");
        }
        let crc = u32::from_le_bytes(blob[..4].try_into().unwrap());
        let (flags, body_start, crc_start) = if crc == crc32fast::hash(&blob[5..]) {
            (Some(blob[4]), 5, 5)
        } else if crc == crc32fast::hash(&blob[4..]) {
            // Even when CRC is over blob[4..], sunxi mkenvimage sometimes has a leading 0x00 flag byte at blob[4]
            if blob[4] == 0 && blob[5] != 0 {
                (Some(0), 5, 4)
            } else {
                (None, 4, 4)
            }
        } else {
            bail!("env CRC mismatch");
        };
        let mut vars = Vec::new();
        for ent in blob[body_start..].split(|&c| c == 0) {
            if ent.is_empty() {
                break;
            }
            let s = std::str::from_utf8(ent)?.to_string();
            match s.split_once('=') {
                Some((k, v)) => vars.push((k.to_string(), v.to_string())),
                None => vars.push((s, String::new())),
            }
        }
        Ok(Env {
            flags,
            vars,
            crc_start,
        })
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// One `key=value` per line (editable text form).
    pub fn to_text(&self) -> String {
        self.vars
            .iter()
            .map(|(k, v)| format!("{k}={v}\n"))
            .collect()
    }

    pub fn from_text(t: &str) -> Self {
        let vars = t
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .map(|l| match l.split_once('=') {
                Some((k, v)) => (k.to_string(), v.to_string()),
                None => (l.to_string(), String::new()),
            })
            .collect();
        Env {
            flags: Some(0),
            vars,
            crc_start: 4,
        }
    }

    /// Serialize into a partition image of `size` bytes (padded with 0x00, CRC patched).
    pub fn build(&self, size: usize) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        for (k, v) in &self.vars {
            if k.is_empty() || k.contains(['=', '\0', '\n', '\r']) || v.contains(['\0', '\n', '\r'])
            {
                bail!("invalid environment key/value");
            }
            body.extend_from_slice(k.as_bytes());
            body.push(b'=');
            body.extend_from_slice(v.as_bytes());
            body.push(0);
        }
        body.push(0);
        let hdr_len = if self.flags.is_some() { 5 } else { 4 };
        if !matches!(self.crc_start, 4 | 5) || self.crc_start > hdr_len {
            bail!("invalid env CRC coverage");
        }
        if body.len() + hdr_len > size {
            bail!("env too large: {} > {}", body.len() + hdr_len, size);
        }
        let mut out = vec![0u8; size];
        if let Some(f) = self.flags {
            out[4] = f;
        }
        out[hdr_len..hdr_len + body.len()].copy_from_slice(&body);
        let crc = crc32fast::hash(&out[self.crc_start..]);
        out[..4].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }
}
