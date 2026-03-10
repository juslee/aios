//! Trap handling for lower-EL synchronous exceptions (SVC, aborts).
//!
//! The vector table branches to assembly stubs that save a full TrapFrame,
//! then call `lower_el_sync_handler` which dispatches based on ESR_EL1.
//! Per ipc.md §3.2 and scheduler.md §4.1.

use crate::arch::aarch64::uart::putc;

// ---------------------------------------------------------------------------
// TrapFrame (repr(C), matches assembly save/restore offsets)
// ---------------------------------------------------------------------------

/// Full register context saved on exception from a lower EL (EL0).
///
/// Layout (repr(C), 272 bytes):
///   x[0]     = offset 0x000
///   x[1]     = offset 0x008
///   ...
///   x[30]    = offset 0x0F0
///   sp_el0   = offset 0x0F8
///   elr_el1  = offset 0x100
///   spsr_el1 = offset 0x108
#[repr(C)]
pub struct TrapFrame {
    /// General-purpose registers x0–x30.
    pub x: [u64; 31],
    /// User stack pointer (SP_EL0).
    pub sp_el0: u64,
    /// Exception link register (return address).
    pub elr_el1: u64,
    /// Saved processor state.
    pub spsr_el1: u64,
}

const _: () = assert!(core::mem::size_of::<TrapFrame>() == 272);

// ---------------------------------------------------------------------------
// Exception class constants (ESR_EL1 EC field, bits [31:26])
// ---------------------------------------------------------------------------

/// SVC instruction from AArch64.
const EC_SVC_AARCH64: u64 = 0x15;
/// Data abort from a lower EL.
const EC_DATA_ABORT_LOWER: u64 = 0x24;
/// Instruction abort from a lower EL.
const EC_INST_ABORT_LOWER: u64 = 0x20;

// ---------------------------------------------------------------------------
// Lower EL synchronous exception handler
// ---------------------------------------------------------------------------

/// Called from the assembly stub after saving a full TrapFrame.
///
/// Reads ESR_EL1 to determine exception class:
/// - EC 0x15 (SVC): dispatch to syscall handler
/// - EC 0x24 (Data Abort): log and halt
/// - EC 0x20 (Instruction Abort): log and halt
/// - Other: log and halt
#[no_mangle]
extern "C" fn lower_el_sync_handler(tf: &mut TrapFrame) {
    let esr: u64;
    // SAFETY: ESR_EL1 is always readable at EL1.
    unsafe {
        core::arch::asm!("mrs {}, ESR_EL1", out(reg) esr, options(nomem, nostack, preserves_flags));
    }

    let ec = (esr >> 26) & 0x3F;

    match ec {
        EC_SVC_AARCH64 => {
            crate::syscall::syscall_dispatch(tf);
        }
        EC_DATA_ABORT_LOWER => {
            let far: u64;
            // SAFETY: FAR_EL1 is always readable at EL1.
            unsafe {
                core::arch::asm!("mrs {}, FAR_EL1", out(reg) far, options(nomem, nostack, preserves_flags));
            }
            put_str("DATA ABORT (EL0): FAR=0x");
            put_hex(far);
            put_str(" ELR=0x");
            put_hex(tf.elr_el1);
            putc(b'\r');
            putc(b'\n');
        }
        EC_INST_ABORT_LOWER => {
            let far: u64;
            // SAFETY: FAR_EL1 is always readable at EL1.
            unsafe {
                core::arch::asm!("mrs {}, FAR_EL1", out(reg) far, options(nomem, nostack, preserves_flags));
            }
            put_str("INST ABORT (EL0): FAR=0x");
            put_hex(far);
            put_str(" ELR=0x");
            put_hex(tf.elr_el1);
            putc(b'\r');
            putc(b'\n');
        }
        _ => {
            put_str("UNKNOWN EXCEPTION (EL0): EC=0x");
            put_hex(ec);
            put_str(" ESR=0x");
            put_hex(esr);
            putc(b'\r');
            putc(b'\n');
        }
    }
}

/// Print a string using direct putc (safe in exception context).
fn put_str(s: &str) {
    for b in s.bytes() {
        putc(b);
    }
}

/// Print a 64-bit value as hex using direct putc.
fn put_hex(val: u64) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for i in (0..16).rev() {
        let nibble = ((val >> (i * 4)) & 0xF) as usize;
        putc(HEX[nibble]);
    }
}
