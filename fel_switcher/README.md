# fel_switcher (ARMv7-A / V851S / V853)

Bare-metal Rust (`no_std`, `armv7a-none-eabi`) `eGON.BT0` SPL flashed at **SD sector 16** (8 KiB offset).
Built specifically for the **F800-MAIN-V2.0** board, including its PH0 power-hold circuit.
When inserted into an Allwinner V851S / V851SE / V853 (`sun8iw21`) device, BROM loads `fel_switcher.bin` into SRAM A1 at `0x00020000`, resets CCU clocks, clears the RTC FEL flag (`0x07090108`), disables caches/MMU, and jumps to BROM FEL (`0x00000020` / `0xFFFF0020`) so the board enumerates on USB as `1f3a:efe8`.

## Build

```bash
rustup target add armv7a-none-eabi   # once
./build.sh                           # -> target/fel_switcher.bin (16 KiB)
```

No external C cross-compiler or `objcopy` is required: `rust-lld` links the ELF and the host `egon_pad` binary extracts `PT_LOAD` segments and patches the `eGON.BT0` length and checksum.

## Flash to SD Card

```bash
sudo dd if=target/fel_switcher.bin of=/dev/sdX bs=1024 seek=8 conv=notrunc
```

`/dev/sdX` is a placeholder for the SD card's whole-device path on Linux, such as `/dev/sdb` or `/dev/mmcblk0`, rather than a partition such as `/dev/sdb1`.
Use `lsblk -p -o NAME,SIZE,MODEL,MOUNTPOINTS` to identify the card and verify its size and model before replacing `/dev/sdX` in the command.

## Power hold (PH0)

The F800 board's power button is momentary and only starts the supply.
Software must drive **PH0 high** to keep the board powered after releasing the button, as the original boot0 and kernel `sys-power` driver do.
The switcher sets PH0 high at the start of `_reset`, before stack setup, clock changes, and entry into FEL.
It sets `PH_DAT` bit 0 before configuring PH0 as an output, preserving the other pins' register bits.
**Hold the power button until the USB FEL device (`1f3a:efe8`) appears, then release it.**

Registers: `PH_DAT = 0x02000160` and `PH_CFG0 = 0x02000150` (PIO base `0x02000000`, port stride `0x30`, PH = port 7).
