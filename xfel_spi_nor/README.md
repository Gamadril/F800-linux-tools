# xfel_spi_nor (V851S / V853)

Rust port of [xfel](https://github.com/xboot/xfel) **SPI NOR read/write/erase** for **Allwinner V851S / V851SE / V853 (`sun8iw21`)** over USB FEL (`1f3a:efe8`, FEL ID `0x00188600`, ARMv7-A SPI payload from `chips/v851_v853.c`).

Workspace member of [F800-linux-tools](../); release bundles ship `bin/xfel_spi_nor` plus `toolbox/xfel_spinor_{dump,recovery}.sh`.

## Device Setup

The device must be in **FEL mode**, for example using the [SD card FEL switcher](../fel_switcher/).

On Linux, USB access may require root or suitable udev rules; the Bash dump/recovery helpers invoke `sudo` when needed.
On Windows, use a suitable WinUSB/libusb driver for USB device `1f3a:efe8`.

## Usage

```bash
./bin/xfel_spi_nor version                 # FEL ID, NOR name/capacity, JEDEC ID and geometry source
./bin/xfel_spi_nor spinor                  # name + capacity in bytes
./bin/xfel_spi_nor read 0 0x1000000 dump.bin # full 16 MiB F800 dump
./bin/xfel_spi_nor read 0 0x1000 -          # first 4 KiB to stdout
./bin/xfel_spi_nor sfdp sfdp.bin           # save the first 256 SFDP bytes
./bin/xfel_spi_nor reset                   # watchdog soft reset
```

Addresses and lengths accept decimal values or hexadecimal values prefixed with `0x`.
Read/write/erase display an xfel-style progress bar on **stderr when it is a terminal**, including transfer rate and estimated time remaining.
The final line shows total size and rate, and binary reads to stdout stay separate from progress output.

### Write / Recovery

Inspect the erase/program range before writing:

```bash
./bin/xfel_spi_nor write --dry-run 0 image.bin
```

To erase the covered sectors, program the image, and read it back for verification:

```bash
./bin/xfel_spi_nor write --yes 0 image.bin
```

`write` performs the required erase itself, so no separate erase is needed before or after it.
Both the start address and image length must be aligned to the detected minimum erase size.
Bytes outside the requested range are preserved.

### Erase Only

The standalone `erase` command removes data without programming a replacement.
For example, inspect an erase of the F800's 128 KiB bootlogo partition:

```bash
./bin/xfel_spi_nor erase --dry-run 0xfe0000 0x20000
```

Replacing `--dry-run` with `--yes` executes that erase and removes the boot logo.

Read/detection never unlock the flash or alter protection registers.
Ranges are checked against detected capacity.
Write/erase require complete erase units; protected chips are rejected.
Read and SFDP output files must not already exist.

## Helpers

Dump the full detected NOR capacity, or restore a complete dump, from the extracted release directory:

```bash
# Linux / macOS
./xfel_spinor_dump.sh dump.bin
./xfel_spinor_recovery.sh flash_dump.bin
```

```powershell
# Windows PowerShell
.\xfel_spinor_dump.ps1 dump.bin
.\xfel_spinor_recovery.ps1 flash_dump.bin
```

The dump helper detects the flash size automatically.
The recovery helper requires a file matching the detected capacity, performs a dry run, and asks for confirmation before erasing/programming.
Helper sources are in [`toolbox/`](../toolbox/).

## Library

```rust
fn main() -> xfel_spi_nor::Result<()> {
    let mut flash = xfel_spi_nor::SpinorFlash::open()?;
    let info = flash.detect()?;
    println!("{}: {} bytes", info.name, info.capacity);

    let mut buf = vec![0u8; 0x1000];
    flash.read(0, &mut buf)?;
    Ok(())
}
```

`flash.write(address, &data)` erases, programs, and verifies the supplied data.
`flash.erase(address, length)` performs an erase only.
These library calls perform the operation directly; `--yes` and `--dry-run` are CLI controls.

## Scope

- **Included:** FEL USB protocol, V851S/V853-family detection (`0x00188600`), ARM SRAM SPI payload, SFDP geometry detection with JEDEC fallback, NOR read/write/erase with range/protection checks and write verification, raw SFDP reads, and watchdog reset.
- **Excluded:** other SoC families (including D1/F133), DDR initialization, JTAG, SPI NAND, and generic `xfel` memory commands.

Payload: [`payloads/v853_spi_init.bin`](payloads/v853_spi_init.bin) (1524 bytes, extracted from upstream `chips/v851_v853.c`).

## Build from Source (Optional)

For development, build from the repository root:

```bash
cargo build --release -p xfel_spi_nor
```

The default build links system libusb and needs its development package and pkg-config.
To build with vendored libusb:

```bash
cargo build --release -p xfel_spi_nor --features vendored
```
