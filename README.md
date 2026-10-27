# F800-linux-tools

**F800-linux-tools** is a Rust toolkit to parse, extract, modify, and repack SPI NOR flash dumps and `IMAGEWTY` SD-card firmware update images for **Allwinner ARM Cortex-A7** devices running **Tina Linux** (specifically **V851S / V851SE / V853 (`sun8iw21`)** devices such as the **F800** CarPlay / Android Auto head unit (also known as Bauhn/ALDI smart display for the car) with 16 MiB **XMC XM25QH128D** SPI NOR flash).

Modeled after `d1s-melis-tools`, adapted for ARM Cortex-A7 (`sun8iw21`) and Tina Linux partitions.

---

## Tools Overview

| Crate / Binary | Role |
|---|---|
| [`dump_tool`](dump_tool/src/main.rs) | Extract & repack full 16 MB SPI NOR flash dumps (`extract`, `pack`) |
| [`img_tool`](img_tool/src/main.rs) | Extract, pack, convert from dump, and create bootlogo-only Allwinner `IMAGEWTY` `.img` update files (`extract`, `pack`, `from-dump`, `bootlogo`) |
| [`xfel_spi_nor`](xfel_spi_nor/src/bin/xfel_spi_nor.rs) | Read, write, erase, detect, and reset SPI NOR flash over USB FEL (`1f3a:efe8`) for V851S/V853 (`0x00188600`) |
| [`fel_switcher`](fel_switcher/src/main.rs) | Bare-metal ARMv7-A (`armv7a-none-eabi`) SD-card SPL (`eGON.BT0`) that boots at sector 16 and jumps into BROM USB FEL mode |
| [`aw-core`](aw-core/src/lib.rs) | Core library: `mtdparts` layout, `eGON.BT0`, `sunxi-package`, U-Boot `env`, Android `boot.img`, SquashFS (`backhand`), `bootlogo` JPEG, and `IMAGEWTY` |

---

## Download Command-Line Tools

Download the package for your platform from [GitHub Releases](https://github.com/Gamadril/F800-linux-tools/releases/latest) and extract it.

| Platform | Package |
|---|---|
| Linux x86_64 | `f800-linux-tools-linux-x86_64.tar.gz` |
| macOS Apple Silicon | `f800-linux-tools-macos-aarch64.tar.gz` |
| Windows x86_64 | `f800-linux-tools-windows-x86_64.zip` |

Run the commands below from the extracted package directory, which contains the executables in `bin/`, helper scripts, and `fel_switcher.bin`.
On Windows, run the tools in PowerShell with the `.exe` suffix, for example `.\bin\img_tool.exe`.
Windows packages include PowerShell (`.ps1`) helpers; Linux and macOS packages include Bash (`.sh`) helpers.

---

## Quick Start: Change Boot Logo Only (SD Card Update)

Based on the reverse-engineered `/usr/bin/cdr` updater, any `.img` file in the root of a FAT32 SD card (`/mnt/extsd/`) is checked for a 40-byte `md5sum <32hex>\n` trailer and flashed per partition.

The easiest way to replace the boot logo is to use the [web bootlogo generator](https://gamadril.github.io/F800-linux-tools/), with no tools to download or install.
Open your image, crop/resize it to `1024×600`, and download `bootlogo_update.img`.

For command-line use, download and extract the release package above, then generate the same bootlogo-only SD update from your own `1024×600` baseline JPEG:

```bash
# Create the update containing your replacement boot logo
./bin/img_tool bootlogo my_logo_1024x600.jpg bootlogo_update.img
```

In Windows PowerShell:

```powershell
.\bin\img_tool.exe bootlogo my_logo_1024x600.jpg bootlogo_update.img
```

With either method, copy `bootlogo_update.img` to the root folder of a FAT32 SD card, insert it into the running unit, and confirm the upgrade prompt to replace the boot logo.

---

## Usage

### 1. Full SPI NOR Dump (`dump_tool`)

```bash
# Extract full 16 MB SPI NOR dump
./bin/dump_tool extract -v dumps/EN25QH128.BIN out_dump

# Repack extracted directory back into a 16 MB SPI NOR dump
# (Unmodified trees repack byte-for-byte identically; modified env.txt, bootlogo.jpg,
#  boot.bin.out/kernel.img, package.bin.out/*.bin, or rootfs.bin.out/ are automatically rebuilt)
./bin/dump_tool pack -v out_dump repacked.bin
```

**Extracted layout (`out_dump/`):**
```text
layout.json               # Partition layout parsed from U-Boot env mtdparts
.aw-original.bin          # Original dump template, preserving gaps/opaque bytes
boot0.bin                 # 0x000000 (64K)    eGON.BT0 SPL
package.bin               # 0x010000 (448K)   sunxi-package (TOC1)
package.bin.out/          #   optee.bin, u-boot-lzma.bin, dtb.bin
env.bin                   # 0x080000 (128K)   U-Boot environment partition
env.txt                   #   editable key=value U-Boot environment
boot.bin                  # 0x0A0000 (2368K)  Tina Linux kernel in Android boot.img format
boot.bin.out/             #   kernel.img
rootfs.bin                # 0x2F0000 (12736K) SquashFS (XZ) rootfs
rootfs.bin.out/           #   extracted Linux rootfs + manifest.tsv
userdata.bin              # 0xF60000 (512K)   JFFS2 userdata partition
bootlogo.bin              # 0xFE0000 (128K)   raw bootlogo partition
bootlogo.jpg              #   extracted 1024x600 baseline JPEG
```

> [!NOTE]
> **SquashFS on Windows:** Linux permissions, ownership, symbolic-link targets and special files (such as device nodes) are preserved in `rootfs.bin.out/manifest.tsv`.
> Symbolic links and special files are recorded there instead of being created on the Windows filesystem; repacking restores them inside SquashFS.
> Edit the manifest to change this metadata, and keep it together with the extracted `.aw-*` metadata files.
> New Windows files default to mode `0644`, and directories to `0755`.
> Extraction does not require administrator privileges or Developer Mode.

### 2. SD Update Images (`img_tool`)

```bash
# Build a full (or filtered) SD update .img from an extracted dump directory
./bin/img_tool from-dump -v out_dump firmware_update.img
./bin/img_tool from-dump -v --only bootlogo,rootfs out_dump custom_update.img

# Extract an existing IMAGEWTY .img (verifies md5sum trailer and unpacks nested items)
./bin/img_tool extract -v firmware_update.img out_img

# Repack an extracted IMAGEWTY directory back into a .img with fresh md5sum trailer
./bin/img_tool pack -v out_img repacked_update.img

# Build or extract a bootlogo-only image directly
./bin/img_tool bootlogo custom_1024x600.jpg bootlogo_update.img
./bin/img_tool bootlogo --extract bootlogo_update.img extracted_logo.jpg
```

#### Browser / WebAssembly Bootlogo Generator (`web/`)

Open the [web bootlogo generator](https://gamadril.github.io/F800-linux-tools/).

Open or drag-and-drop any image, crop/resize it to `1024×600`, and download a ready-to-flash `bootlogo_update.img`.
The browser tool loads the WebAssembly version of `img_tool` from `img_tool.wasm` alongside `app.js`.

### 3. USB FEL SPI NOR Tool (`xfel_spi_nor`)

For boards in USB FEL mode (`1f3a:efe8`, SoC ID `0x00188600` V851S/V853):

```bash
./bin/xfel_spi_nor version                  # Chip ID + detected SPI NOR name & size
./bin/xfel_spi_nor spinor                   # SPI NOR name + size in bytes
./bin/xfel_spi_nor read 0 0x1000000 dump.bin
./bin/xfel_spi_nor write --dry-run 0 repacked.bin
./bin/xfel_spi_nor write --yes 0 repacked.bin  # erase, program, read back and verify
./bin/xfel_spi_nor reset                    # Watchdog soft-reset
```

On Linux and macOS, use the helper scripts in the extracted package directory:

* `./xfel_spinor_dump.sh [output.bin]`
* `./xfel_spinor_recovery.sh <flash_dump.bin>`
* `./extract_dump.sh <dump.bin>`
* `./pack_dump.sh [output.bin]`

On Windows, use the corresponding PowerShell helpers:

```powershell
.\xfel_spinor_dump.ps1 dump.bin
.\xfel_spinor_recovery.ps1 flash_dump.bin
.\extract_dump.ps1 dump.bin
.\pack_dump.ps1 repacked.bin
```

### 4. SD Card FEL Switcher (`fel_switcher`)

Bare-metal ARMv7-A (`armv7a-none-eabi`) `eGON.BT0` SPL image that forces the V851S/V853 into USB FEL mode when booted from an SD card.

Built specifically for the **F800-MAIN-V2.0** board.
On this board, **PH0 is the power-hold GPIO** and must be driven high to keep the device powered after releasing the power button.
The switcher drives PH0 high at the start of the reset handler, before stack setup and entry into FEL.

```bash
# Write the release package's FEL image to SD card sector 16 (8 KiB offset):
sudo dd if=fel_switcher.bin of=/dev/sdX bs=1024 seek=8 conv=notrunc
```

`/dev/sdX` is a placeholder for the SD card's whole-device path on Linux, such as `/dev/sda` or `/dev/mmcblk0`, rather than a partition such as `/dev/sda1`.
Use `lsblk -p -o NAME,SIZE,MODEL,MOUNTPOINTS` to identify the card and verify its size and model before replacing `/dev/sdX` in the command.
