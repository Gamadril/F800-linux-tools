#![no_std]
#![no_main]

// Allwinner eGON.BT0 header + ARMv7-A (Cortex-A7) BROM entry for V851S / V853 (sun8iw21).
// Loaded at 0x00020000 in SRAM A1 from SD card sector 16 (8 KiB offset).
core::arch::global_asm!(
    r#"
    .arm
    .section .text._start, "ax", %progbits
    .global _start
_start:
    b       _reset
    .byte   'e','G','O','N'
    .byte   '.','B','T','0'
    .word   0x5F0A6C39          // checksum placeholder (patched by egon_pad)
    .word   0x00004000          // length (16 KiB)
    .word   0x00000030          // pub_head_size
    .byte   '3','0','0','0'
    .word   0x00020000          // ret_addr
    .word   0x00020000          // run_addr
    .word   0x00000000
    .byte   0x00, 0x00, 0x02, 0x00
    .byte   '4', '.', '0', 0x00

    .section .text, "ax", %progbits
    .global _reset
_reset:
    // Power hold: the board's push button only *starts* the supply; software must drive PH0 high
    // to keep it on once the button is released. boot0 does this as its very first action
    // (F800 boot0.bin main @ 0x20000+0x68b0; the 4.9 kernel `sys-power` driver does the same for
    // device-tree node `sys_power`, gpios = <&pio PH 0 ...>). Without it the board dies as soon as
    // the button is let go and the FEL USB device vanishes.
    // PIO base 0x02000000, port stride 0x30, PH = port 7: CFG0 = 0x02000150, DAT = 0x02000160.
    // Needs no stack. DAT bit is set before the pin becomes an output, so there is no low glitch.
    ldr     r1, =0x02000160         // PH_DAT
    ldr     r0, [r1]
    orr     r0, r0, #1              // PH0 = 1
    str     r0, [r1]
    ldr     r1, =0x02000150         // PH_CFG0
    ldr     r0, [r1]
    bic     r0, r0, #0xf
    orr     r0, r0, #1              // PH0 = output
    str     r0, [r1]

    // Switch to SVC mode with IRQ/FIQ disabled
    mrs     r0, cpsr
    bic     r0, r0, #0x1f
    orr     r0, r0, #0xd3
    msr     cpsr_c, r0

    // Set stack at top of SRAM A1 and enter Rust main
    ldr     sp, =0x00028000
    b       main
"#
);

use core::arch::asm;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{compiler_fence, Ordering};

// V851S / V853 (sun8iw21) CCU registers (verified from F800 boot0.bin @ 0x21278)
const CCU_CPU_AXI_CFG: usize = 0x0200_1500;
const CCU_PSI_AHB1_AHB2_CFG: usize = 0x0200_1510;
const CCU_APB1_CFG: usize = 0x0200_1520;
const CCU_USB_BGR: usize = 0x0200_1A8C;

// PRCM / RTC FEL flag registers (verified from F800 boot0.bin @ 0x22ecc..0x22f74)
const R_PRCM_RTC_BGR: usize = 0x0701_020C;
const RTC_FEL_INDEX2: usize = 0x0709_0108; // 0x07090100 + 2 * 4
const EFEX_FLAG: u32 = 0x5AA5_A55A;

// Watchdog soft reset register (verified from xfel chips/v851_v853.c)
const WDOG_SRST: usize = 0x0205_00A8;
const WDOG_SRST_VAL: u32 = 0x16AA_0001;

// BROM FEL entry point in low / high vectors (see sunxi-tools fel-sdboot.S)
const BROM_FEL_LOW: u32 = 0x0000_0020;
const BROM_FEL_HIGH: u32 = 0xFFFF_0020;

#[inline(always)]
unsafe fn read32(addr: usize) -> u32 {
    read_volatile(addr as *const u32)
}

#[inline(always)]
unsafe fn write32(addr: usize, val: u32) {
    write_volatile(addr as *mut u32, val);
    compiler_fence(Ordering::SeqCst);
}

fn delay_ms(ms: u32) {
    for _ in 0..ms {
        for _ in 0..24_000 {
            core::hint::spin_loop();
        }
    }
}

fn board_clock_reset() {
    unsafe {
        let mask = !((0x3 << 24) | (0x3 << 8) | 0x3);
        let v = read32(CCU_PSI_AHB1_AHB2_CFG) & mask;
        write32(CCU_PSI_AHB1_AHB2_CFG, v);

        let v = read32(CCU_APB1_CFG) & mask;
        write32(CCU_APB1_CFG, v);

        write32(CCU_CPU_AXI_CFG, 0x0301);
    }
}

fn usb0_clock_on() {
    unsafe {
        let mut v = read32(CCU_USB_BGR);
        v |= 1 << 16;
        write32(CCU_USB_BGR, v);
    }
    delay_ms(1);
    unsafe {
        let mut v = read32(CCU_USB_BGR);
        v |= 1 << 0;
        write32(CCU_USB_BGR, v);
    }
}

fn rtc_clear_fel_flag() {
    loop {
        unsafe {
            write32(RTC_FEL_INDEX2, 0);
            if read32(RTC_FEL_INDEX2) == 0 {
                break;
            }
        }
    }
}

/// Disable I-cache/D-cache/MMU/branch predictor, invalidate I-cache & BTB, and jump to BROM FEL.
/// Matches F800 `boot0.bin` `FUN_00020f7a` + `FUN_00020fb8` and `sunxi-tools/fel-sdboot.S`.
#[inline(never)]
fn jmp_to_fel() -> ! {
    unsafe {
        asm!(
            "mrc p15, 0, r0, c1, c0, 0",
            "movw r1, #0x1007",
            "bic r0, r0, r1",
            "mcr p15, 0, r0, c1, c0, 0",
            "isb",
            "mov r1, #0",
            "mcr p15, 0, r1, c7, c5, 0",
            "mcr p15, 0, r1, c7, c5, 6",
            "dsb",
            "isb",
            "tst r0, #(1 << 13)",
            "moveq lr, {low}",
            "ldrne lr, ={high}",
            "bx lr",
            low = const BROM_FEL_LOW,
            high = const BROM_FEL_HIGH,
            options(noreturn),
        );
    }
}

fn enter_fel() -> ! {
    board_clock_reset();
    usb0_clock_on();
    delay_ms(10);
    jmp_to_fel();
}

#[allow(dead_code)]
fn system_reset() -> ! {
    unsafe {
        write32(WDOG_SRST, WDOG_SRST_VAL);
    }
    loop {
        unsafe {
            asm!("wfi", options(nomem, nostack));
        }
    }
}

#[no_mangle]
pub extern "C" fn main() -> ! {
    unsafe {
        let v = read32(R_PRCM_RTC_BGR) | (1 << 16) | (1 << 0);
        write32(R_PRCM_RTC_BGR, v);
    }

    if unsafe { read32(RTC_FEL_INDEX2) } == EFEX_FLAG {
        rtc_clear_fel_flag();
        enter_fel();
    }

    // Set RTC FEL flag as well in case the board resets before BROM FEL entry, then jump directly
    unsafe {
        write32(RTC_FEL_INDEX2, EFEX_FLAG);
    }
    rtc_clear_fel_flag();
    enter_fel();
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
