//! Exception vector table and boot diagnostics for aarch64.
//!
//! Phase 0: All 16 vector table entries branch-to-self (halt on exception).
//! Phase 1 replaces stubs with real exception handlers.
//!
//! The boot.S stub vectors serve as a safety net for the window between `_start`
//! and `kernel_main`. This Rust-owned table is installed from `kernel_main` and
//! is where Phase 1+ exception handlers will be added.

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
    "// Current EL with SP_ELx",
    ".balign 128",
    "    b .",
    ".balign 128",
    "    b .",
    ".balign 128",
    "    b .",
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

/// Read the current `VBAR_EL1` value.
pub fn read_vbar_el1() -> u64 {
    let vbar: u64;
    // SAFETY: Reading VBAR_EL1 is a pure register read, safe at EL1.
    unsafe {
        core::arch::asm!("mrs {}, VBAR_EL1", out(reg) vbar, options(nomem, nostack, preserves_flags))
    };
    vbar
}
