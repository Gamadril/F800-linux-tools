//! Android boot image (v0 header) as used by Tina `boot` partition: header page, kernel,
//! ramdisk, second stage, each padded to `page_size`.

use crate::util::{cstr, rd_u32, wr_u32};
use anyhow::{bail, Context, Result};
use sha1::{Digest, Sha1};

pub const MAGIC: &[u8; 8] = b"ANDROID!";

#[derive(Debug, Clone)]
pub struct BootImg {
    /// Raw header page (page_size bytes); sizes and id are patched on [`BootImg::build`].
    pub header: Vec<u8>,
    pub kernel: Vec<u8>,
    pub ramdisk: Vec<u8>,
    pub second: Vec<u8>,
}

fn pages(n: usize, page: usize) -> usize {
    n.div_ceil(page)
}

impl BootImg {
    pub fn page_size(&self) -> usize {
        rd_u32(&self.header, 0x24) as usize
    }
    pub fn name(&self) -> String {
        cstr(&self.header[0x30..0x40])
    }
    pub fn cmdline(&self) -> String {
        let mut c = cstr(&self.header[0x40..0x240]);
        c.push_str(&cstr(&self.header[0x260..0x660]));
        c
    }
    pub fn kernel_addr(&self) -> u32 {
        rd_u32(&self.header, 0x0C)
    }

    pub fn parse(img: &[u8]) -> Result<Self> {
        if img.len() < 0x660 || &img[..8] != MAGIC {
            bail!("not an Android boot image");
        }
        let ksz = rd_u32(img, 8) as usize;
        let rsz = rd_u32(img, 0x10) as usize;
        let ssz = rd_u32(img, 0x18) as usize;
        let page = rd_u32(img, 0x24) as usize;
        if !page.is_power_of_two() || page < 0x800 || page > img.len() {
            bail!("bad boot image page size {page}");
        }
        let koff = page;
        let padded = |n: usize| {
            pages(n, page)
                .checked_mul(page)
                .context("boot component overflows")
        };
        let roff = koff
            .checked_add(padded(ksz)?)
            .context("kernel range overflows")?;
        let soff = roff
            .checked_add(padded(rsz)?)
            .context("ramdisk range overflows")?;
        let end = soff
            .checked_add(ssz)
            .context("second stage range overflows")?;
        if end > img.len() {
            bail!(
                "boot image truncated (needs {:#x}, have {:#x})",
                end,
                img.len()
            );
        }
        Ok(BootImg {
            header: img[..page].to_vec(),
            kernel: img[koff..koff + ksz].to_vec(),
            ramdisk: img[roff..roff + rsz].to_vec(),
            second: img[soff..soff + ssz].to_vec(),
        })
    }

    /// Length of the image produced by [`build`](Self::build).
    pub fn built_len(&self) -> usize {
        let p = self.page_size();
        p + (pages(self.kernel.len(), p)
            + pages(self.ramdisk.len(), p)
            + pages(self.second.len(), p))
            * p
    }

    pub fn build(&self) -> Result<Vec<u8>> {
        if self.header.len() < 0x660 {
            bail!("boot header too small");
        }
        let page = self.page_size();
        if !page.is_power_of_two() || page < 0x800 || self.header.len() != page {
            bail!("invalid boot header page size");
        }
        for bytes in [&self.kernel, &self.ramdisk, &self.second] {
            u32::try_from(bytes.len()).context("boot component exceeds 32 bits")?;
        }
        let mut h = self.header.clone();
        wr_u32(&mut h, 8, self.kernel.len() as u32);
        wr_u32(&mut h, 0x10, self.ramdisk.len() as u32);
        wr_u32(&mut h, 0x18, self.second.len() as u32);
        // id = sha1(kernel, size, ramdisk, size, second, size)
        let mut s = Sha1::new();
        for d in [&self.kernel, &self.ramdisk, &self.second] {
            s.update(d);
            s.update((d.len() as u32).to_le_bytes());
        }
        let digest = s.finalize();
        h[0x240..0x260].fill(0);
        h[0x240..0x254].copy_from_slice(&digest);
        let mut out = h;
        for d in [&self.kernel, &self.ramdisk, &self.second] {
            out.extend_from_slice(d);
            out.resize(out.len() + pages(d.len(), page) * page - d.len(), 0);
        }
        Ok(out)
    }
}
