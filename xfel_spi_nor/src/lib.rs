//! SPI NOR access on Allwinner V851S / V853 (sun8iw21) via USB FEL
//! (ported from [xfel](https://github.com/xboot/xfel) `spinor.c` + `chips/v851_v853.c`).

mod fel;
mod progress;
mod spinor;
pub mod v853;

pub use fel::Fel;
pub use spinor::SpinorFlash;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("USB: {0}")]
    Usb(#[from] rusb::Error),
    #[error("no Allwinner FEL device (1f3a:efe8)")]
    NoDevice,
    #[error("FEL handshake failed")]
    FelInit,
    #[error("not a V851S/V853 family SoC (expected FEL ID 0x00188600, got 0x{0:08x})")]
    NotV853(u32),
    #[error("SPI NOR not detected")]
    NoSpinor,
    #[error("{0}")]
    Msg(String),
}
