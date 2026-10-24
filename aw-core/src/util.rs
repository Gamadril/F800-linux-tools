use anyhow::{bail, Result};

pub fn rd_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
pub fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
pub fn rd_u64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
pub fn wr_u32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
pub fn wr_u64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

/// Text of a NUL padded fixed size field.
pub fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

/// Copy `s` into a zero padded fixed size field.
pub fn put_fixed(dst: &mut [u8], s: &str) {
    dst.fill(0);
    let n = s.len().min(dst.len());
    dst[..n].copy_from_slice(&s.as_bytes()[..n]);
}

/// Parse "0x1000", "4096", "128K", "2M".
pub fn parse_size(s: &str) -> Result<u64> {
    let s = s.trim();
    let (num, mul) = match s.chars().last() {
        Some('K') | Some('k') => (&s[..s.len() - 1], 1024),
        Some('M') | Some('m') => (&s[..s.len() - 1], 1024 * 1024),
        _ => (s, 1),
    };
    let v = if let Some(h) = num.strip_prefix("0x").or_else(|| num.strip_prefix("0X")) {
        u64::from_str_radix(h, 16)
    } else {
        num.parse::<u64>()
    };
    match v {
        Ok(v) => v
            .checked_mul(mul)
            .ok_or_else(|| anyhow::anyhow!("size overflows: {s}")),
        Err(_) => bail!("invalid number/size: {s}"),
    }
}

pub fn md5_hex(data: &[u8]) -> String {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}
