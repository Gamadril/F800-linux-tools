//! Allwinner `IMAGEWTY` update image as consumed by `/usr/bin/cdr` (upgrade/imgDecode.c).
//!
//! Layout verified against `cdr` (Ghidra, `imgOpen` / `imgOpenItem` / `imgReadItem`):
//!
//! * 0x400 byte header, `IMAGEWTY` magic, `item_count` at +0x3C (version is not checked)
//! * the item table follows **immediately** (`item_count` x 0x400 bytes, read sequentially
//!   right after the header, `item_offset` is not used)
//! * item: `sub_type[16]` at +0x10 is the lookup name, `file_length` (u64) at +0x12C,
//!   `offset` (u64) at +0x134
//! * item data is read as `(file_length + 0x3FF) & !0x3FF` bytes, so every item must be
//!   padded to 1 KiB inside the file; exactly `file_length` bytes are written to flash
//! * integrity: 40 byte trailer `"md5sum " + 32 hex + "\n"` (md5 of everything before it)

use crate::util::{cstr, md5_hex, put_fixed, rd_u32, rd_u64, wr_u32, wr_u64};
use anyhow::{bail, Context, Result};

pub const MAGIC: &[u8; 8] = b"IMAGEWTY";
pub const HEADER_LEN: usize = 0x400;
pub const ITEM_LEN: usize = 0x400;
pub const ALIGN: usize = 0x400;
pub const TRAILER_LEN: usize = 0x28;

/// Item names `cdr` knows (index = bit in its write mask, 0xEF is used on update).
pub mod names {
    pub const BOOT: &str = "BOOT_FEX00000000"; // /dev/by-name/boot
    pub const ROOTFS: &str = "ROOTFS_FEX000000"; // /dev/by-name/rootfs
    pub const BOOTLOGO: &str = "BOOTLOGO_FEX0000"; // /dev/by-name/bootlogo
    pub const ENV: &str = "ENV_NOR_FEX00000"; // /dev/by-name/env
    pub const OVERLAY: &str = "overlay"; // /dev/by-name/overlay (mask bit 4, not set by cdr)
    pub const USERDATA: &str = "userdata"; // /dev/by-name/userdata
    pub const BOOT0: &str = "1234567890BNOR_0"; // /dev/mtdblock0 @ 0
    pub const BOOTPKG: &str = "BOOTPKG-NOR00000"; // /dev/mtdblock0 @ 0x10000
    pub const MBR: &str = "1234567890___MBR"; // only used to "clear" partitions - never add
}

#[derive(Debug, Clone)]
pub struct Item {
    pub main_type: String,
    pub sub_type: String,
    pub name: String,
    pub data: Vec<u8>,
    /// Original item record; opaque attributes are retained on repack.
    pub record: Vec<u8>,
}

impl Item {
    pub fn new(sub_type: &str, main_type: &str, name: &str, data: Vec<u8>) -> Self {
        Item {
            main_type: main_type.into(),
            sub_type: sub_type.into(),
            name: name.into(),
            data,
            record: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Image {
    pub items: Vec<Item>,
    pub header: Vec<u8>,
}

fn align_up(n: usize) -> Result<usize> {
    Ok(n.checked_add(ALIGN - 1)
        .context("IMAGEWTY size overflows")?
        & !(ALIGN - 1))
}

impl Image {
    pub fn new(items: Vec<Item>) -> Self {
        Self {
            items,
            header: Vec::new(),
        }
    }
    pub fn get(&self, sub_type: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.sub_type == sub_type)
    }

    /// Serialize (header + item table + padded data), optionally with md5 trailer.
    pub fn build(&self, trailer: bool) -> Result<Vec<u8>> {
        let n = self.items.len();
        let table_end = n
            .checked_mul(ITEM_LEN)
            .and_then(|n| n.checked_add(HEADER_LEN))
            .context("item table overflows")?;
        if n > u32::MAX as usize {
            bail!("too many image items");
        }
        if !self.header.is_empty() && self.header.len() != HEADER_LEN {
            bail!("bad preserved IMAGEWTY header length");
        }
        if !self.header.is_empty()
            && (rd_u32(&self.header, 0x48) != 0 || rd_u64(&self.header, 0x4c) != 0)
        {
            bail!("IMAGEWTY append sections are unsupported");
        }
        let mut out = vec![0u8; table_end];
        let mut off = table_end;
        let mut offs = Vec::new();
        let mut names = std::collections::BTreeSet::new();
        for it in &self.items {
            if it.main_type.len() > 8
                || it.sub_type.is_empty()
                || it.sub_type.len() > 16
                || it.name.len() > 256
                || it.main_type.contains('\0')
                || it.sub_type.contains('\0')
                || it.name.contains('\0')
                || !names.insert(&it.sub_type)
            {
                bail!("invalid or duplicate IMAGEWTY item {:?}", it.sub_type);
            }
            if !it.record.is_empty() && it.record.len() != ITEM_LEN {
                bail!("bad preserved item record length");
            }
            offs.push(off);
            off = off
                .checked_add(align_up(it.data.len())?)
                .context("image size overflows")?;
        }
        out.resize(off, 0);
        for (it, &o) in self.items.iter().zip(&offs) {
            out[o..o + it.data.len()].copy_from_slice(&it.data);
        }
        // header
        if self.header.is_empty() {
            wr_u32(&mut out, 8, 0x0300);
            wr_u32(&mut out, 12, 0x60);
            wr_u32(&mut out, 0x20, ALIGN as u32);
            wr_u32(&mut out, 0x34, 0x100);
        } else {
            out[..HEADER_LEN].copy_from_slice(&self.header);
        }
        out[..8].copy_from_slice(MAGIC);
        wr_u64(&mut out, 0x18, off as u64); // image size
        wr_u32(&mut out, 0x38, ITEM_LEN as u32);
        wr_u32(&mut out, 0x3C, n as u32);
        wr_u32(&mut out, 0x40, HEADER_LEN as u32); // item table offset
        for (i, (it, &o)) in self.items.iter().zip(&offs).enumerate() {
            let b = HEADER_LEN + i * ITEM_LEN;
            if it.record.is_empty() {
                wr_u32(&mut out, b, 0x100);
                wr_u32(&mut out, b + 4, ITEM_LEN as u32);
            } else {
                out[b..b + ITEM_LEN].copy_from_slice(&it.record);
            }
            put_fixed(&mut out[b + 8..b + 16], &it.main_type);
            put_fixed(&mut out[b + 0x10..b + 0x20], &it.sub_type);
            put_fixed(&mut out[b + 0x24..b + 0x124], &it.name);
            wr_u64(&mut out, b + 0x124, align_up(it.data.len())? as u64); // data_length
            wr_u64(&mut out, b + 0x12C, it.data.len() as u64); // file_length
            wr_u64(&mut out, b + 0x134, o as u64);
        }
        if trailer {
            let md5 = md5_hex(&out);
            out.extend_from_slice(format!("md5sum {md5}\n").as_bytes());
        }
        Ok(out)
    }

    /// Parse the way `cdr` does. A trailing md5 trailer is verified and removed.
    pub fn parse(file: &[u8]) -> Result<(Self, Option<bool>)> {
        let mut md5_ok = None;
        let mut body = file;
        if file.len() > TRAILER_LEN && file[file.len() - TRAILER_LEN..].starts_with(b"md5sum") {
            let t = &file[file.len() - TRAILER_LEN..];
            if !t.starts_with(b"md5sum ")
                || t[39] != b'\n'
                || !t[7..39].iter().all(u8::is_ascii_hexdigit)
            {
                bail!("malformed md5sum trailer");
            }
            let hex = String::from_utf8_lossy(&t[7..39]).into_owned();
            body = &file[..file.len() - TRAILER_LEN];
            md5_ok = Some(md5_hex(body).eq_ignore_ascii_case(&hex));
            if md5_ok == Some(false) {
                bail!("IMAGEWTY md5sum trailer mismatch");
            }
        }
        if body.len() < HEADER_LEN || &body[..8] != MAGIC {
            bail!("iamge magic is bad (not IMAGEWTY)");
        }
        if rd_u32(body, 0x48) != 0 || rd_u64(body, 0x4c) != 0 {
            bail!("IMAGEWTY append sections are unsupported");
        }
        let n = rd_u32(body, 0x3C) as usize;
        let table_end = n
            .checked_mul(ITEM_LEN)
            .and_then(|n| n.checked_add(HEADER_LEN))
            .context("item count overflows")?;
        if table_end > body.len() {
            bail!("read iamge item table fail");
        }
        let mut items = Vec::new();
        let mut ranges = Vec::new();
        let mut names = std::collections::BTreeSet::new();
        for i in 0..n {
            let b = HEADER_LEN + i * ITEM_LEN;
            let len = usize::try_from(rd_u64(body, b + 0x12C)).context("item length too large")?;
            let off = usize::try_from(rd_u64(body, b + 0x134)).context("item offset too large")?;
            let end = off.checked_add(len).context("item range overflows")?;
            let padded_end = off
                .checked_add(align_up(len)?)
                .context("padded item range overflows")?;
            if off < table_end || off % ALIGN != 0 {
                bail!("item {i} overlaps header/table or is unaligned");
            }
            let data = body
                .get(off..end)
                .with_context(|| format!("item {i} out of range"))?;
            // cdr reads len rounded up to 1 KiB in one fread
            if padded_end > body.len() {
                bail!("item {i}: padded data beyond end of image (cdr fread would fail)");
            }
            if len > 0 {
                ranges.push((off, padded_end));
            }
            let sub_type = cstr(&body[b + 0x10..b + 0x20]);
            if sub_type.is_empty() || !names.insert(sub_type.clone()) {
                bail!("empty or duplicate item subtype");
            }
            items.push(Item {
                main_type: cstr(&body[b + 8..b + 16]),
                sub_type,
                name: cstr(&body[b + 0x24..b + 0x124]),
                data: data.to_vec(),
                record: body[b..b + ITEM_LEN].to_vec(),
            });
        }
        ranges.sort_unstable();
        if ranges.windows(2).any(|p| p[0].1 > p[1].0) {
            bail!("overlapping IMAGEWTY items");
        }
        Ok((
            Image {
                items,
                header: body[..HEADER_LEN].to_vec(),
            },
            md5_ok,
        ))
    }
}
