//! Core library for Allwinner ARM / Tina Linux (V851S, V853 family) SPI NOR firmware tooling.
//!
//! * [`dump`]     – full SPI NOR dump extract / pack
//! * [`imagewty`] – Allwinner `IMAGEWTY` update image read / write (+ cdr style md5 trailer)
//! * [`bootlogo`] – boot logo (JPEG) helpers
//! * [`bootimg`], [`env`], [`package`], [`squash`], [`layout`], [`egon`] – partition level formats

pub mod bootimg;
pub mod bootlogo;
#[cfg(not(target_arch = "wasm32"))]
pub mod dump;
pub mod egon;
pub mod env;
#[cfg(not(target_arch = "wasm32"))]
pub mod host;
pub mod imagewty;
pub mod layout;
pub mod package;
#[cfg(not(target_arch = "wasm32"))]
pub mod partition;
#[cfg(not(target_arch = "wasm32"))]
pub mod squash;
pub mod util;

pub use anyhow::{anyhow, bail, Context, Result};
