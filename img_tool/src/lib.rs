//! Library and WebAssembly (`wasm32-unknown-unknown`) exports for `img_tool`.
//!
//! Provides `build_bootlogo_image` and `extract_bootlogo_jpeg` in Rust, plus low-level C-ABI
//! WebAssembly exports so the browser web UI can build a ready-to-flash SD-card `.img`
//! (`IMAGEWTY` + `BOOTLOGO_FEX0000` + `md5sum <32hex>\n` trailer) directly in memory without
//! requiring `wasm-bindgen`.

use anyhow::{bail, Context, Result};
use aw_core::bootlogo::{self, DEFAULT_HEIGHT, DEFAULT_WIDTH};
use aw_core::imagewty::{names, Image, Item};
use std::cell::RefCell;

pub const DEFAULT_BOOTLOGO_PART_SIZE: usize = 128 * 1024;

/// Build a bootlogo-only `IMAGEWTY` SD-card update `.img` (with 40-byte `md5sum` trailer)
/// from a baseline JPEG slice.
pub fn build_bootlogo_image(
    jpeg_bytes: &[u8],
    expected_width: u32,
    expected_height: u32,
    part_size: usize,
    check_dims: bool,
) -> Result<Vec<u8>> {
    let info = bootlogo::jpeg_info(jpeg_bytes).context("input is not a valid JPEG image")?;
    if !info.baseline {
        bail!("bootlogo requires an 8-bit baseline JPEG (SOF0)");
    }
    if check_dims && (info.width != expected_width || info.height != expected_height) {
        bail!(
            "JPEG dimensions are {}x{}, expected {}x{}",
            info.width,
            info.height,
            expected_width,
            expected_height
        );
    }
    let jpg_slice = &jpeg_bytes[..info.end];
    if jpg_slice.len() > part_size {
        bail!(
            "JPEG size ({} bytes) exceeds bootlogo partition size ({} bytes)",
            jpg_slice.len(),
            part_size
        );
    }
    let part_data = bootlogo::build_partition(jpg_slice, part_size)?;
    // Omit 1234567890___MBR so `/usr/bin/cdr` only writes `/dev/by-name/bootlogo`
    let img = Image::new(vec![Item::new(
        names::BOOTLOGO,
        "RFSFAT16",
        "bootlogo.fex",
        part_data,
    )]);
    img.build(true)
}

/// Extract the JPEG bytes from an existing `IMAGEWTY` `.img` or raw `bootlogo.bin` partition.
pub fn extract_bootlogo_jpeg(input: &[u8]) -> Result<Vec<u8>> {
    let part_bytes = if input.starts_with(aw_core::imagewty::MAGIC) {
        let (img, md5_ok) = Image::parse(input)?;
        if md5_ok == Some(false) {
            bail!("IMAGEWTY md5sum trailer mismatch");
        }
        let it = img
            .get(names::BOOTLOGO)
            .context("IMAGEWTY image does not contain BOOTLOGO_FEX0000 item")?;
        it.data.clone()
    } else {
        input.to_vec()
    };
    let jpg = bootlogo::extract_jpeg(&part_bytes)?;
    Ok(jpg.to_vec())
}

// ============================================================================
// WebAssembly C-ABI exports (used by web/index.html)
// ============================================================================

thread_local! {
    static OUTPUT_BUF: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static ERROR_MSG: RefCell<String> = const { RefCell::new(String::new()) };
}

fn set_output(data: Vec<u8>) {
    OUTPUT_BUF.with(|b| *b.borrow_mut() = data);
}

fn set_error(err: String) {
    ERROR_MSG.with(|e| *e.borrow_mut() = err);
}

/// Allocate `size` bytes in WASM linear memory for passing input bytes from JS.
#[no_mangle]
pub extern "C" fn wasm_alloc(size: usize) -> *mut u8 {
    Box::into_raw(vec![0u8; size].into_boxed_slice()) as *mut u8
}

/// Free a buffer previously allocated with `wasm_alloc`.
///
/// # Safety
/// `ptr` must come from `wasm_alloc(size)` and must not have been freed already.
#[no_mangle]
pub unsafe extern "C" fn wasm_dealloc(ptr: *mut u8, size: usize) {
    if !ptr.is_null() && size > 0 {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, size)));
    }
}

#[no_mangle]
pub extern "C" fn wasm_output_ptr() -> *const u8 {
    OUTPUT_BUF.with(|b| b.borrow().as_ptr())
}

#[no_mangle]
pub extern "C" fn wasm_output_len() -> usize {
    OUTPUT_BUF.with(|b| b.borrow().len())
}

#[no_mangle]
pub extern "C" fn wasm_error_ptr() -> *const u8 {
    ERROR_MSG.with(|e| e.borrow().as_ptr())
}

#[no_mangle]
pub extern "C" fn wasm_error_len() -> usize {
    ERROR_MSG.with(|e| e.borrow().len())
}

/// Default width (1024) for F800-MAIN V851S screen.
#[no_mangle]
pub extern "C" fn wasm_default_width() -> u32 {
    DEFAULT_WIDTH
}

/// Default height (600) for F800-MAIN V851S screen.
#[no_mangle]
pub extern "C" fn wasm_default_height() -> u32 {
    DEFAULT_HEIGHT
}

/// Default bootlogo partition size (131072 = 128 KiB).
#[no_mangle]
pub extern "C" fn wasm_default_part_size() -> usize {
    DEFAULT_BOOTLOGO_PART_SIZE
}

/// Build an SD-card update `.img` from a baseline JPEG buffer at `(ptr, len)`.
/// Returns `0` on success (result in `wasm_output_ptr()` / `wasm_output_len()`),
/// or `1` on error (message in `wasm_error_ptr()` / `wasm_error_len()`).
///
/// # Safety
/// `ptr` must reference `len` initialized readable bytes for the entire call.
#[no_mangle]
pub unsafe extern "C" fn wasm_build_bootlogo_img(
    ptr: *const u8,
    len: usize,
    width: u32,
    height: u32,
    part_size: usize,
    check_dims: u32,
) -> u32 {
    let slice = std::slice::from_raw_parts(ptr, len);
    let w = if width == 0 { DEFAULT_WIDTH } else { width };
    let h = if height == 0 { DEFAULT_HEIGHT } else { height };
    let psize = if part_size == 0 {
        DEFAULT_BOOTLOGO_PART_SIZE
    } else {
        part_size
    };
    match build_bootlogo_image(slice, w, h, psize, check_dims != 0) {
        Ok(img) => {
            set_output(img);
            set_error(String::new());
            0
        }
        Err(e) => {
            set_output(Vec::new());
            set_error(format!("{e:#}"));
            1
        }
    }
}

/// Extract the JPEG from an existing `.img` or `bootlogo.bin` buffer at `(ptr, len)`.
/// Returns `0` on success (JPEG in `wasm_output_ptr()` / `wasm_output_len()`),
/// or `1` on error (message in `wasm_error_ptr()` / `wasm_error_len()`).
///
/// # Safety
/// `ptr` must reference `len` initialized readable bytes for the entire call.
#[no_mangle]
pub unsafe extern "C" fn wasm_extract_bootlogo_jpg(ptr: *const u8, len: usize) -> u32 {
    let slice = std::slice::from_raw_parts(ptr, len);
    match extract_bootlogo_jpeg(slice) {
        Ok(jpg) => {
            set_output(jpg);
            set_error(String::new());
            0
        }
        Err(e) => {
            set_output(Vec::new());
            set_error(format!("{e:#}"));
            1
        }
    }
}
