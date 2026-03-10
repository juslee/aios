//! Syscall dispatch and handlers.
//!
//! Dispatches SVC traps from EL0 based on syscall number in x8.
//! Per ipc.md §3.1–3.2.

use crate::arch::aarch64::trap::TrapFrame;

// ---------------------------------------------------------------------------
// Syscall numbers (ipc.md §3.1, 31 total)
// ---------------------------------------------------------------------------

/// Syscall numbers matching the IPC architecture spec.
#[repr(u64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Syscall {
    IpcCall = 0,
    IpcSend = 1,
    IpcRecv = 2,
    IpcReply = 3,
    IpcCancel = 4,
    IpcSelect = 5,
    ChannelCreate = 6,
    ChannelDestroy = 7,
    RingChannelCreate = 8,
    RingChannelDestroy = 9,
    NotificationCreate = 10,
    NotificationSignal = 11,
    NotificationWait = 12,
    ChannelStats = 13,
    CapabilityTransfer = 14,
    CapabilityAttenuate = 15,
    CapabilityRevoke = 16,
    CapabilityList = 17,
    MemoryMap = 18,
    MemoryUnmap = 19,
    SharedMemoryCreate = 20,
    SharedMemoryMap = 21,
    SharedMemoryShare = 22,
    ProcessCreate = 23,
    ProcessExit = 24,
    ProcessWait = 25,
    TimeGet = 26,
    TimeSleep = 27,
    TimerSet = 28,
    AuditLog = 29,
    DebugPrint = 30,
}

/// Total number of defined syscalls.
#[allow(dead_code)]
pub const SYSCALL_COUNT: usize = 31;

// ---------------------------------------------------------------------------
// IPC error codes (ipc.md §3.2)
// ---------------------------------------------------------------------------

/// Error codes returned in x0 (negative values).
#[repr(i64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum IpcError {
    Etimedout = -1,
    Epipe = -2,
    Eagain = -3,
    Ecanceled = -4,
    Eacces = -5,
    Eperm = -6,
    Enospc = -7,
    Eproto = -8,
    Enotsup = -9,
    EcapDormant = -10,
}

// ---------------------------------------------------------------------------
// Syscall dispatch
// ---------------------------------------------------------------------------

/// Main syscall dispatch. Called from `lower_el_sync_handler` on SVC trap.
///
/// Convention: x8 = syscall number, x0-x5 = args, return in x0.
pub fn syscall_dispatch(tf: &mut TrapFrame) {
    let nr = tf.x[8];

    let result: i64 = match nr {
        26 => sys_time_get(tf),
        27 => sys_time_sleep(),
        28 => IpcError::Enotsup as i64,
        30 => sys_debug_print(tf),
        _ => IpcError::Enotsup as i64,
    };

    // Return value in x0.
    tf.x[0] = result as u64;
}

// ---------------------------------------------------------------------------
// DebugPrint (nr=30) — development-only UART output from EL0
// ---------------------------------------------------------------------------

/// DebugPrint syscall: x0 = ptr, x1 = len.
///
/// Validates pointer is in user VA range (< 0x0000_8000_0000_0000)
/// and len ≤ 256. Copies message to kernel stack buffer before printing.
fn sys_debug_print(tf: &TrapFrame) -> i64 {
    let ptr = tf.x[0] as usize;
    let len = tf.x[1] as usize;

    // Validate length.
    if len > 256 {
        return IpcError::Enospc as i64;
    }

    // Validate pointer is in user VA range.
    if ptr >= 0x0000_8000_0000_0000 || ptr.wrapping_add(len) > 0x0000_8000_0000_0000 {
        return IpcError::Eperm as i64;
    }

    // Copy message to kernel stack buffer.
    let mut buf = [0u8; 256];
    // SAFETY: ptr has been validated to be in the user VA range.
    // The user address space is mapped via TTBR0. If the page is
    // unmapped, a data abort will occur (handled by the exception
    // framework, not here). The copy is bounded by `len ≤ 256`.
    unsafe {
        core::ptr::copy_nonoverlapping(ptr as *const u8, buf.as_mut_ptr(), len);
    }

    let msg = core::str::from_utf8(&buf[..len]).unwrap_or("<invalid utf8>");
    crate::kinfo!(Ipc, "{}", msg);
    0
}

// ---------------------------------------------------------------------------
// TimeGet (nr=26) — monotonic nanosecond clock
// ---------------------------------------------------------------------------

/// TimeGet syscall: returns current time in nanoseconds.
///
/// Uses CNTVCT_EL0 × 10^9 / CNTFRQ_EL0 with u128 intermediate
/// to avoid overflow.
fn sys_time_get(_tf: &TrapFrame) -> i64 {
    let ticks: u64;
    let freq: u64;
    // SAFETY: CNTVCT_EL0 and CNTFRQ_EL0 are always readable at EL1.
    unsafe {
        core::arch::asm!("mrs {}, CNTVCT_EL0", out(reg) ticks, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) freq, options(nomem, nostack, preserves_flags));
    }

    if freq == 0 {
        return 0;
    }

    // Overflow-safe conversion: (ticks * 1_000_000_000) / freq via u128.
    let ns = ((ticks as u128) * 1_000_000_000 / (freq as u128)) as u64;
    ns as i64
}

// ---------------------------------------------------------------------------
// TimeSleep (nr=27) — stub
// ---------------------------------------------------------------------------

/// TimeSleep syscall: stub that returns 0 immediately.
/// Full implementation in Phase 3 M11 (scheduler sleep queue).
fn sys_time_sleep() -> i64 {
    0
}
