//! `sunxi-package` (TOC1 style boot package: OP-TEE, U-Boot, DTB ...).
//!
//! Layout: 0x40 byte header (`name[16]`, magic 0x89119800, checksum @0x14, items_nr @0x20,
//! valid_len @0x24, end tag "MIE;" @0x3C) followed by `items_nr` records of 0x170 bytes
//! (`name[64]`, data_offset @0x40, data_len @0x44, ... end tag "IIE;" @0x16C).

use crate::util::{cstr, rd_u32, wr_u32};
use anyhow::{bail, Result};

const HDR_LEN: usize = 0x40;
const ITEM_LEN: usize = 0x170;
const STAMP: u32 = 0x5F0A_6C39;
pub const MAGIC_NAME: &[u8] = b"sunxi-package";

#[derive(Debug, Clone)]
pub struct Item {
    pub name: String,
    pub offset: usize,
    pub len: usize,
    pub index: usize,
}

pub struct Package<'a> {
    pub data: &'a [u8],
    pub items: Vec<Item>,
    pub valid_len: usize,
    pub checksum_ok: bool,
}

fn checksum(data: &[u8], valid_len: usize) -> u32 {
    let mut tmp = data[..valid_len].to_vec();
    wr_u32(&mut tmp, 0x14, STAMP);
    tmp.as_chunks::<4>()
        .0
        .iter()
        .fold(0u32, |a, c| a.wrapping_add(u32::from_le_bytes(*c)))
}

impl<'a> Package<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        if data.len() < HDR_LEN || !data.starts_with(MAGIC_NAME) {
            bail!("not a sunxi-package");
        }
        let n = rd_u32(data, 0x20) as usize;
        let valid_len = rd_u32(data, 0x24) as usize;
        if rd_u32(data, 0x10) != 0x89119800
            || n > 32
            || !valid_len.is_multiple_of(4)
            || valid_len > data.len()
            || valid_len < HDR_LEN + n * ITEM_LEN
        {
            bail!("sunxi-package header looks invalid (items {n}, len {valid_len:#x})");
        }
        let mut items = Vec::new();
        for i in 0..n {
            let o = HDR_LEN + i * ITEM_LEN;
            items.push(Item {
                name: cstr(&data[o..o + 64]),
                offset: rd_u32(data, o + 0x40) as usize,
                len: rd_u32(data, o + 0x44) as usize,
                index: i,
            });
        }
        let checksum_ok = checksum(data, valid_len) == rd_u32(data, 0x14);
        if !checksum_ok {
            bail!("sunxi-package checksum mismatch");
        }
        let mut names = std::collections::BTreeSet::new();
        let mut ranges = Vec::new();
        for item in &items {
            if item.name.is_empty()
                || item.name.contains(['/', '\\', '\0'])
                || !names.insert(&item.name)
            {
                bail!("invalid or duplicate package name {:?}", item.name);
            }
            let end = item
                .offset
                .checked_add(item.len)
                .ok_or_else(|| anyhow::anyhow!("package item range overflows"))?;
            if item.offset < HDR_LEN + n * ITEM_LEN || end > valid_len {
                bail!("package item '{}' out of range", item.name);
            }
            if item.len != 0 {
                ranges.push((item.offset, end));
            }
        }
        ranges.sort_unstable();
        if ranges.windows(2).any(|p| p[0].1 > p[1].0) {
            bail!("overlapping package items");
        }
        Ok(Package {
            data,
            items,
            valid_len,
            checksum_ok,
        })
    }

    pub fn item_data(&self, it: &Item) -> &[u8] {
        &self.data[it.offset..it.offset + it.len]
    }

    /// Room available for item `i` (up to next item or valid_len).
    pub fn slot_len(&self, i: usize) -> usize {
        let start = self.items[i].offset;
        let next = self
            .items
            .iter()
            .filter(|x| x.offset > start)
            .map(|x| x.offset)
            .min()
            .unwrap_or(self.valid_len);
        next - start
    }
}

/// Replace an item in place (must fit its slot); updates data_len and checksum.
pub fn replace_item(pkg: &mut [u8], name: &str, new: &[u8]) -> Result<()> {
    let (offset, slot, rec) = {
        let p = Package::parse(pkg)?;
        let Some(it) = p.items.iter().find(|i| i.name == name) else {
            bail!("package has no item '{name}'");
        };
        (
            it.offset,
            p.slot_len(it.index),
            HDR_LEN + it.index * ITEM_LEN,
        )
    };
    if new.len() > slot {
        bail!("item '{name}' is {} bytes, slot only {slot}", new.len());
    }
    pkg[offset..offset + slot].fill(0);
    pkg[offset..offset + new.len()].copy_from_slice(new);
    wr_u32(pkg, rec + 0x44, new.len() as u32);
    let vl = rd_u32(pkg, 0x24) as usize;
    let c = checksum(pkg, vl);
    wr_u32(pkg, 0x14, c);
    Ok(())
}
