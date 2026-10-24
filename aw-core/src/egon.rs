//! eGON.BT0 (boot0 / SPL) header helpers. Checksum algorithm is the common sunxi one.

use anyhow::{bail, Result};

pub const MAGIC: &[u8; 8] = b"eGON.BT0";
pub const SEED: u32 = 0x5F0A_6C39;
pub const CHECKSUM_OFFSET: usize = 0x0C;
pub const LENGTH_OFFSET: usize = 0x10;

/// Check magic, stored length and checksum. Returns the stored boot0 length.
pub fn verify(boot0: &[u8]) -> Result<u32> {
    if boot0.len() < 0x30 || &boot0[4..12] != MAGIC {
        bail!("not an eGON.BT0 image");
    }
    let len = u32::from_le_bytes(boot0[LENGTH_OFFSET..LENGTH_OFFSET + 4].try_into().unwrap());
    if len < 0x30 || len as usize > boot0.len() || len % 4 != 0 {
        bail!("eGON length {len:#x} invalid");
    }
    let stored = u32::from_le_bytes(
        boot0[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4]
            .try_into()
            .unwrap(),
    );
    let mut tmp = boot0[..len as usize].to_vec();
    tmp[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].copy_from_slice(&SEED.to_le_bytes());
    let sum = tmp
        .as_chunks::<4>()
        .0
        .iter()
        .fold(0u32, |a, c| a.wrapping_add(u32::from_le_bytes(*c)));
    if sum != stored {
        bail!("eGON checksum mismatch: stored {stored:#010x}, computed {sum:#010x}");
    }
    Ok(len)
}
