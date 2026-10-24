//! Host tool: extract `.text` from an ARM ELF32 binary (or take a raw binary) and patch the
//! eGON.BT0 length + checksum. No external `arm-none-eabi-objcopy` required.

use std::env;
use std::fs;
use std::path::Path;
use std::process::exit;

#[path = "../egon.rs"]
mod egon;

use egon::{patch_file, CHECKSUM_OFFSET};

/// If `data` is a 32-bit little-endian ELF, extract `PT_LOAD` segments flattened from base vaddr.
fn extract_elf_load_segments(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 52 || &data[0..4] != b"\x7fELF" || data[4] != 1 || data[5] != 1 {
        return None;
    }
    let e_phoff = u32::from_le_bytes(data[28..32].try_into().ok()?) as usize;
    let e_phentsize = u16::from_le_bytes(data[42..44].try_into().ok()?) as usize;
    let e_phnum = u16::from_le_bytes(data[44..46].try_into().ok()?) as usize;
    if e_phentsize < 32 || u16::from_le_bytes(data[18..20].try_into().ok()?) != 40 {
        return None;
    }

    let mut min_vaddr = u32::MAX;
    let mut max_vaddr = 0u32;
    let mut segs = Vec::new();
    for i in 0..e_phnum {
        let off = e_phoff.checked_add(i.checked_mul(e_phentsize)?)?;
        if off + 32 > data.len() {
            return None;
        }
        let p_type = u32::from_le_bytes(data[off..off + 4].try_into().ok()?);
        let p_offset = u32::from_le_bytes(data[off + 4..off + 8].try_into().ok()?) as usize;
        let p_vaddr = u32::from_le_bytes(data[off + 8..off + 12].try_into().ok()?);
        let p_filesz = u32::from_le_bytes(data[off + 16..off + 20].try_into().ok()?) as usize;
        if p_type == 1 && p_filesz > 0 {
            let end = p_offset.checked_add(p_filesz)?;
            if end > data.len() {
                return None;
            }
            min_vaddr = min_vaddr.min(p_vaddr);
            max_vaddr = max_vaddr.max(p_vaddr.checked_add(u32::try_from(p_filesz).ok()?)?);
            segs.push((p_vaddr, &data[p_offset..end]));
        }
    }
    if segs.is_empty() {
        return None;
    }
    if min_vaddr != 0x20000 || max_vaddr.checked_sub(min_vaddr)? > 0x4000 {
        return None;
    }
    let mut out = vec![0u8; (max_vaddr - min_vaddr) as usize];
    for (vaddr, slice) in segs {
        let start = (vaddr - min_vaddr) as usize;
        out[start..start + slice.len()].copy_from_slice(slice);
    }
    Some(out)
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 || args.len() > 3 {
        eprintln!("Usage: egon_pad <input-elf-or-bin> [output-bin]");
        exit(1);
    }
    let in_path = Path::new(&args[1]);
    let out_path = Path::new(args.get(2).unwrap_or(&args[1]));

    let raw = match fs::read(in_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("failed to read {}: {e}", in_path.display());
            exit(1);
        }
    };
    let bin = if raw.starts_with(b"\x7fELF") {
        extract_elf_load_segments(&raw).unwrap_or_else(|| {
            eprintln!("invalid ARM ELF or load segments outside FEL SRAM image");
            exit(1)
        })
    } else {
        raw
    };
    if let Err(e) = fs::write(out_path, &bin) {
        eprintln!("failed to write {}: {e}", out_path.display());
        exit(1);
    }

    match patch_file(out_path) {
        Ok((total, len)) => {
            println!("[OK] checksum = 0x{total:08x}  (offset 0x{CHECKSUM_OFFSET:02x})");
            println!("[OK] length   = {len} bytes = {} KiB", len / 1024);
            println!();
            println!("Flash to SD card sector 16 (8 KiB offset) with:");
            println!(
                "    sudo dd if={} of=/dev/sdX bs=1024 seek=8 conv=notrunc",
                out_path.display()
            );
        }
        Err(e) => {
            eprintln!("{e}");
            exit(1);
        }
    }
}
