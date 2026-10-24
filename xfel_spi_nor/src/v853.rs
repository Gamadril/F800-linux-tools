use crate::fel::Fel;
use crate::{Error, Result};

/// Allwinner V851S / V851SE / V853 (sun8iw21) FEL ID
pub const V853_FEL_ID: u32 = 0x0018_8600;

pub const PAYLOAD_ADDR: u32 = 0x0002_8000;
pub const CMD_BUF: u32 = 0x0002_9000;
pub const SWAP_BUF: u32 = 0x0002_a000;
pub const SWAP_LEN: u32 = 65536;
pub const CMD_LEN: u32 = 4096;

/// Archived payload provenance and reproduction: docs/PROVENANCE.md.
pub static SPI_PAYLOAD: &[u8] = include_bytes!("../payloads/v853_spi_init.bin");

pub fn is_v853(fel: &Fel) -> bool {
    fel.version.id == V853_FEL_ID
}

pub fn upload_spi_payload(fel: &mut Fel) -> Result<()> {
    fel.write(PAYLOAD_ADDR, SPI_PAYLOAD)
}

pub fn spi_run(fel: &mut Fel, cbuf: &[u8]) -> Result<()> {
    if cbuf.len() > CMD_LEN as usize {
        return Err(Error::Msg(format!(
            "SPI cmd buffer too large ({} > {})",
            cbuf.len(),
            CMD_LEN
        )));
    }
    fel.write(CMD_BUF, cbuf)?;
    fel.exec(PAYLOAD_ADDR)
}

/// Write a 32-bit register via the ARM scratchpad payload (from `v851_v853.c` `payload_write32`).
pub fn payload_write32(fel: &mut Fel, addr: u32, val: u32) -> Result<()> {
    const PAYLOAD: &[u8] = &[
        0x00, 0x00, 0xa0, 0xe3, 0x17, 0x0f, 0x08, 0xee, 0x15, 0x0f, 0x07, 0xee, 0xd5, 0x0f, 0x07,
        0xee, 0x9a, 0x0f, 0x07, 0xee, 0x95, 0x0f, 0x07, 0xee, 0xff, 0xff, 0xff, 0xea, 0x08, 0x00,
        0x9f, 0xe5, 0x08, 0x10, 0x9f, 0xe5, 0x00, 0x10, 0x80, 0xe5, 0x1e, 0xff, 0x2f, 0xe1,
    ];
    let sp = fel.version.scratchpad;
    let mut params = [0u8; 8];
    params[..4].copy_from_slice(&addr.to_le_bytes());
    params[4..].copy_from_slice(&val.to_le_bytes());
    fel.write(sp, PAYLOAD)?;
    fel.write(sp + PAYLOAD.len() as u32, &params)?;
    fel.exec(sp)
}

/// Reset the V851S / V853 SoC via watchdog soft reset (`0x020500a8 = 0x16aa0001`).
pub fn reset(fel: &mut Fel) -> Result<()> {
    payload_write32(fel, 0x0205_00a8, (0x16aa << 16) | 1)
}
