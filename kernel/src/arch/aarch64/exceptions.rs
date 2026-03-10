//! Exception vector table and boot diagnostics for aarch64.
//!
//! The boot.S stub vectors serve as a safety net for the window between `_start`
//! and `kernel_main`. This Rust-owned table is installed from `kernel_main`.
//!
//! Current EL with SP_ELx synchronous handler: reads ESR_EL1/FAR_EL1 and
//! prints diagnostics for data/instruction aborts (guard page faults).
//! All other vector entries halt on exception.

use core::arch::global_asm;

// ---------------------------------------------------------------------------
// Rust-owned exception vector table
// ---------------------------------------------------------------------------
// Section .text.rvectors is placed by the linker script with ALIGN(2048).
// Each of the 16 entries occupies 128 bytes (.balign 128) per ARMv8-A spec.
global_asm!(
    ".section .text.rvectors, \"ax\"",
    ".balign 2048",
    ".global __vector_table_el1",
    "__vector_table_el1:",
    "",
    "// Current EL with SP_EL0",
    ".balign 128",
    "    b .",
    ".balign 128",
    "    b .",
    ".balign 128",
    "    b .",
    ".balign 128",
    "    b .",
    "",
    "// Current EL with SP_ELx — Synchronous",
    ".balign 128",
    "    stp x29, x30, [sp, #-16]!",
    "    stp x0, x1, [sp, #-16]!",
    "    stp x2, x3, [sp, #-16]!",
    "    mrs x0, ESR_EL1",
    "    mrs x1, FAR_EL1",
    "    mrs x2, ELR_EL1",
    "    bl sync_exception_handler",
    "    ldp x2, x3, [sp], #16",
    "    ldp x0, x1, [sp], #16",
    "    ldp x29, x30, [sp], #16",
    "    b .", // halt after handler (no eret for now)
    "// Current EL with SP_ELx — IRQ",
    ".balign 128",
    "    b .",
    "// Current EL with SP_ELx — FIQ",
    ".balign 128",
    "    b .",
    "// Current EL with SP_ELx — SError",
    ".balign 128",
    "    b .",
    "",
    "// Lower EL using AArch64",
    ".balign 128",
    "    b .",
    ".balign 128",
    "    b .",
    ".balign 128",
    "    b .",
    ".balign 128",
    "    b .",
    "",
    "// Lower EL using AArch32",
    ".balign 128",
    "    b .",
    ".balign 128",
    "    b .",
    ".balign 128",
    "    b .",
    ".balign 128",
    "    b .",
);

/// Read the current exception level from `CurrentEL` (bits [3:2]).
pub fn current_el() -> u8 {
    let el: u64;
    // SAFETY: Reading CurrentEL is a pure register read with no side effects,
    // safe at any exception level.
    unsafe {
        core::arch::asm!("mrs {}, CurrentEL", out(reg) el, options(nomem, nostack, preserves_flags))
    };
    ((el >> 2) & 0x3) as u8
}

/// Read the core ID from `MPIDR_EL1` (Aff0 field, bits [7:0]).
pub fn core_id() -> u8 {
    let mpidr: u64;
    // SAFETY: Reading MPIDR_EL1 is a pure register read with no side effects,
    // safe at EL1.
    unsafe {
        core::arch::asm!("mrs {}, MPIDR_EL1", out(reg) mpidr, options(nomem, nostack, preserves_flags))
    };
    (mpidr & 0xFF) as u8
}

/// Install the Rust-defined exception vector table to `VBAR_EL1`.
///
/// Replaces the boot.S stub vectors. Returns the installed address for
/// diagnostic verification.
pub fn install_vector_table() -> u64 {
    extern "C" {
        static __vector_table_el1: u8;
    }
    let addr = core::ptr::addr_of!(__vector_table_el1) as u64;
    // SAFETY: __vector_table_el1 is defined in the global_asm above, placed in
    // a 2048-byte aligned section as required by VBAR_EL1. Writing VBAR_EL1
    // at EL1 is safe — it only changes where exceptions vector to.
    unsafe {
        core::arch::asm!(
            "msr VBAR_EL1, {addr}",
            "isb",
            addr = in(reg) addr,
            options(nomem, nostack, preserves_flags),
        );
    }
    addr
}

/// Synchronous exception handler called from the vector table.
///
/// Decodes ESR_EL1 to identify the exception class and prints diagnostics.
/// Uses direct putc() instead of println!() to avoid recursive faults when
/// TTBR0 has been switched away from the identity map.
#[no_mangle]
extern "C" fn sync_exception_handler(esr: u64, far: u64, elr: u64) {
    use crate::arch::aarch64::uart::putc;

    let ec = (esr >> 26) & 0x3F;

    // Print using only putc — safe even when TTBR0 is switched
    put_str("EXCEPTION: ESR=0x");
    put_hex(esr);
    put_str(" EC=0x");
    put_hex(ec);
    put_str(" FAR=0x");
    put_hex(far);
    put_str(" ELR=0x");
    put_hex(elr);
    putc(b'\r');
    putc(b'\n');

    match ec {
        0x24 | 0x25 => {
            put_str("  Data Abort at 0x");
            put_hex(far);
            putc(b'\r');
            putc(b'\n');
        }
        0x20 | 0x21 => {
            put_str("  Instruction Abort at 0x");
            put_hex(far);
            putc(b'\r');
            putc(b'\n');
        }
        _ => {}
    }
}

/// Print a string using direct putc (no formatting machinery).
fn put_str(s: &str) {
    for b in s.bytes() {
        crate::arch::aarch64::uart::putc(b);
    }
}

/// Print a 64-bit value as hex using direct putc.
fn put_hex(val: u64) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for i in (0..16).rev() {
        let nibble = ((val >> (i * 4)) & 0xF) as usize;
        crate::arch::aarch64::uart::putc(HEX[nibble]);
    }
}

/// Read the current `VBAR_EL1` value.
pub fn read_vbar_el1() -> u64 {
    let vbar: u64;
    // SAFETY: Reading VBAR_EL1 is a pure register read, safe at EL1.
    unsafe {
        core::arch::asm!("mrs {}, VBAR_EL1", out(reg) vbar, options(nomem, nostack, preserves_flags))
    };
    vbar
}
