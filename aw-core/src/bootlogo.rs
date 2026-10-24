//! Boot logo helpers. The `bootlogo` partition (128K) holds one baseline JPEG (1024x600 on the
//! F800 head unit) at offset 0, followed by zero padding to a 512 byte boundary and 0xFF.

use anyhow::{bail, Context, Result};

pub const DEFAULT_WIDTH: u32 = crate::layout::F800.logo_width;
pub const DEFAULT_HEIGHT: u32 = crate::layout::F800.logo_height;

#[derive(Debug, Clone, Copy)]
pub struct JpegInfo {
    pub width: u32,
    pub height: u32,
    pub progressive: bool,
    pub baseline: bool,
    /// Offset one past the final EOI marker of the top level image.
    pub end: usize,
}

/// Walk the JPEG marker structure (skips APPn segments, so EXIF thumbnails are not mistaken
/// for the image).
pub fn jpeg_info(d: &[u8]) -> Result<JpegInfo> {
    if d.len() < 4 || d[0] != 0xFF || d[1] != 0xD8 {
        bail!("not a JPEG (no SOI)");
    }
    let (mut w, mut h, mut prog, mut baseline) = (0, 0, false, false);
    let mut scanned = false;
    let mut framed = false;
    let mut i = 2;
    loop {
        while i < d.len() && d[i] != 0xFF {
            i += 1;
        }
        while i < d.len() && d[i] == 0xFF {
            i += 1;
        }
        if i >= d.len() {
            bail!("JPEG truncated (no EOI)");
        }
        let m = d[i];
        i += 1;
        match m {
            0xD9 => {
                if !framed || !scanned {
                    bail!("JPEG has no frame or image scan");
                }
                return Ok(JpegInfo {
                    width: w,
                    height: h,
                    progressive: prog,
                    baseline,
                    end: i,
                });
            }
            0x01 | 0xD0..=0xD7 => continue,
            _ => {}
        }
        if i + 2 > d.len() {
            bail!("JPEG truncated");
        }
        let len = u16::from_be_bytes([d[i], d[i + 1]]) as usize;
        if len < 2 || len > d.len().saturating_sub(i) {
            bail!("invalid JPEG segment length");
        }
        if matches!(m, 0xC0..=0xCF) && !matches!(m, 0xC4 | 0xC8 | 0xCC) {
            if framed || len < 8 || d[i + 2] != 8 {
                bail!("unsupported JPEG frame");
            }
            framed = true;
            prog = m == 0xC2;
            baseline = m == 0xC0;
            h = u16::from_be_bytes([d[i + 3], d[i + 4]]) as u32;
            w = u16::from_be_bytes([d[i + 5], d[i + 6]]) as u32;
        }
        i += len;
        if m == 0xDA {
            if !framed {
                bail!("JPEG scan precedes frame");
            }
            scanned = true;
            // entropy coded data: skip to next real marker
            while i + 1 < d.len() {
                if d[i] == 0xFF && d[i + 1] != 0x00 && !(0xD0..=0xD7).contains(&d[i + 1]) {
                    break;
                }
                i += 1;
            }
        }
    }
}

/// Extract the JPEG from a bootlogo partition image.
pub fn extract_jpeg(part: &[u8]) -> Result<&[u8]> {
    let info = jpeg_info(part).context("bootlogo partition does not start with a JPEG")?;
    Ok(&part[..info.end])
}

/// Check that `jpeg` is acceptable for the boot loader (baseline, expected size, fits).
pub fn validate(jpeg: &[u8], width: u32, height: u32, max_len: usize) -> Result<JpegInfo> {
    let info = jpeg_info(jpeg)?;
    if !info.baseline {
        bail!("boot logo requires an 8-bit baseline JPEG (SOF0)");
    }
    if info.width != width || info.height != height {
        bail!(
            "JPEG is {}x{}, device expects {width}x{height}",
            info.width,
            info.height
        );
    }
    if jpeg.len() > max_len {
        bail!(
            "JPEG is {} bytes, partition holds only {max_len}",
            jpeg.len()
        );
    }
    Ok(info)
}

/// JPEG + zero padding to 512 bytes + 0xFF up to `part_size`.
pub fn build_partition(jpeg: &[u8], part_size: usize) -> Result<Vec<u8>> {
    if jpeg.len() > part_size {
        bail!("JPEG ({}) larger than partition ({part_size})", jpeg.len());
    }
    let mut v = jpeg.to_vec();
    let al = ((v.len() + 511) & !511).min(part_size);
    v.resize(al, 0);
    v.resize(part_size, 0xFF);
    Ok(v)
}

/// Load a JPEG image and validate it as a baseline JPEG with the expected dimensions and size.
pub fn prepare(
    input: &[u8],
    width: u32,
    height: u32,
    _quality: u8,
    max_len: usize,
) -> Result<Vec<u8>> {
    validate(input, width, height, max_len)?;
    Ok(extract_jpeg(input)?.to_vec())
}
