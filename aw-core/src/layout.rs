//! SPI NOR flash layout of Tina Linux devices (V851S/V853).
//!
//! ```text
//! 0x000000  boot0     64K   eGON.BT0 (SPL)
//! 0x010000  package  448K   "sunxi-package": OP-TEE, U-Boot, DTB
//! 0x080000  <mtdparts> ...  env, boot, rootfs, userdata, bootlogo, UDISK
//! ```
//! The `mtdparts` part is read from the U-Boot environment (`mtdparts=spi0.0:128K(env),...`).

use crate::env::Env;
use crate::util::parse_size;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const BOOT0_OFFSET: u64 = 0;
pub const BOOT0_SIZE: u64 = 0x10000;
pub const PACKAGE_OFFSET: u64 = 0x10000;
/// Start of the first `mtdparts` partition (env).
pub const MTD_BASE: u64 = 0x80000;

/// Board assumptions are separate from mtdparts parsing. Add another profile
/// only when its boot regions and environment geometry have been verified.
#[derive(Debug, Clone, Copy)]
pub struct DeviceProfile {
    pub name: &'static str,
    pub boot0_size: u64,
    pub package_offset: u64,
    pub mtd_base: u64,
    pub env_size: usize,
    pub flash_size: u64,
    pub logo_width: u32,
    pub logo_height: u32,
}
pub const F800: DeviceProfile = DeviceProfile {
    name: "f800",
    boot0_size: BOOT0_SIZE,
    package_offset: PACKAGE_OFFSET,
    mtd_base: MTD_BASE,
    env_size: 0x20000,
    flash_size: 0x1000000,
    logo_width: 1024,
    logo_height: 600,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Part {
    pub name: String,
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layout {
    pub flash_size: u64,
    pub parts: Vec<Part>,
}

impl Layout {
    pub fn validate(&self) -> Result<()> {
        if self.flash_size == 0 || self.flash_size > 64 * 1024 * 1024 {
            bail!("unsupported flash image size {}", self.flash_size);
        }
        let mut names = std::collections::BTreeSet::new();
        let mut ranges = Vec::new();
        for p in &self.parts {
            if p.name.is_empty()
                || p.name == "."
                || p.name == ".."
                || p.name.contains(['/', '\\', '\0', '\t', '\n', '\r'])
                || !names.insert(&p.name)
            {
                bail!("unsafe or duplicate partition name {:?}", p.name);
            }
            let end = p
                .offset
                .checked_add(p.size)
                .context("partition range overflows")?;
            if end > self.flash_size {
                bail!("partition '{}' exceeds flash size", p.name);
            }
            if p.size > 0 {
                ranges.push((p.offset, end));
            }
        }
        ranges.sort_unstable();
        if ranges.windows(2).any(|p| p[0].1 > p[1].0) {
            bail!("overlapping partitions");
        }
        Ok(())
    }
    pub fn get(&self, name: &str) -> Result<&Part> {
        self.parts
            .iter()
            .find(|p| p.name == name)
            .with_context(|| format!("partition '{name}' not in layout"))
    }

    /// Build from an `mtdparts` string, e.g. `spi0.0:128K(env),2368K(boot),...,0K(UDISK)`.
    /// A size of 0 (or `-`) means "up to next partition / end of flash".
    pub fn from_mtdparts(mtdparts: &str, flash_size: u64, mtd_base: u64) -> Result<Self> {
        if mtd_base < PACKAGE_OFFSET || mtd_base > flash_size || mtdparts.contains(';') {
            bail!("unsupported mtd base or multiple MTD devices");
        }
        let list = mtdparts.split_once(':').map(|x| x.1).unwrap_or(mtdparts);
        let mut parts = vec![
            Part {
                name: "boot0".into(),
                offset: BOOT0_OFFSET,
                size: BOOT0_SIZE,
            },
            Part {
                name: "package".into(),
                offset: PACKAGE_OFFSET,
                size: mtd_base - PACKAGE_OFFSET,
            },
        ];
        let mut off = mtd_base;
        for ent in list.split(',') {
            let ent = ent.trim();
            let (size_s, rest) = ent
                .split_once('(')
                .with_context(|| format!("bad mtdparts entry '{ent}'"))?;
            let (name, flags) = rest.split_once(')').context("missing ')' in mtdparts")?;
            if !flags.is_empty() && flags != "ro" {
                bail!("unsupported mtdparts flags: {flags}");
            }
            let (size_s, explicit) = size_s
                .split_once('@')
                .map_or((size_s, None), |(s, o)| (s, Some(o)));
            if let Some(offset) = explicit {
                off = mtd_base
                    .checked_add(parse_size(offset)?)
                    .context("MTD offset overflows")?;
            }
            let size = if size_s == "-" {
                flash_size
                    .checked_sub(off)
                    .context("MTD offset outside flash")?
            } else {
                parse_size(size_s)?
            };
            let name = name.to_string();
            parts.push(Part {
                name,
                offset: off,
                size,
            });
            off = off.checked_add(size).context("mtdparts sizes overflow")?;
        }
        if off > flash_size {
            bail!("mtdparts exceed flash size ({off:#x} > {flash_size:#x})");
        }
        let n = parts.len();
        for i in 0..n {
            if parts[i].size == 0 {
                let next = if i + 1 < n {
                    parts[i + 1].offset
                } else {
                    flash_size
                };
                parts[i].size = next.saturating_sub(parts[i].offset);
            }
        }
        let layout = Layout { flash_size, parts };
        layout.validate()?;
        Ok(layout)
    }

    /// Read layout from the env partition of a dump.
    pub fn from_dump(dump: &[u8], mtd_base: u64) -> Result<Self> {
        Self::from_dump_profile(dump, &DeviceProfile { mtd_base, ..F800 })
    }

    pub fn from_dump_profile(dump: &[u8], profile: &DeviceProfile) -> Result<Self> {
        if dump.len() as u64 != profile.flash_size {
            bail!(
                "{} profile expects {} bytes, got {}",
                profile.name,
                profile.flash_size,
                dump.len()
            );
        }
        let env_off = usize::try_from(profile.mtd_base)?;
        let env_end = env_off
            .checked_add(profile.env_size)
            .context("environment range overflows")?;
        let env = Env::parse(
            dump.get(env_off..env_end)
                .context("dump truncated before environment")?,
        )
        .context("U-Boot env not valid at mtd base")?;
        let mtd = env
            .get("mtdparts")
            .context("no 'mtdparts' variable in env")?;
        let mut layout = Self::from_mtdparts(mtd, dump.len() as u64, profile.mtd_base)?;
        layout.parts[0].size = profile.boot0_size;
        layout.parts[1].offset = profile.package_offset;
        layout.parts[1].size = profile
            .mtd_base
            .checked_sub(profile.package_offset)
            .context("invalid package region")?;
        layout.validate()?;
        Ok(layout)
    }
}
