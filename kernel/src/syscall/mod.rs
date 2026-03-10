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
        // IPC syscalls (ipc.md §3.1): extract args from TrapFrame, delegate
        // to the same functions used by kernel threads' direct calls.
        0 => sys_ipc_call(tf),
        1 => sys_ipc_send(tf),
        2 => sys_ipc_recv(tf),
        3 => sys_ipc_reply(tf),
        4 => sys_ipc_cancel(tf),
        5 => IpcError::Enotsup as i64, // IpcSelect — Phase 3+ (requires poll set)
        6 => sys_channel_create(tf),
        7 => sys_channel_destroy(tf),
        8 | 9 => IpcError::Enotsup as i64, // RingChannel — future
        13 => IpcError::Enotsup as i64,    // ChannelStats — future
        26 => sys_time_get(tf),
        27 => sys_time_sleep(tf),
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
    // Use checked_add to reject overflow (defense-in-depth; the first check
    // already catches all kernel-range pointers).
    let end = match ptr.checked_add(len) {
        Some(e) => e,
        None => return IpcError::Eperm as i64,
    };
    if ptr >= 0x0000_8000_0000_0000 || end > 0x0000_8000_0000_0000 {
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

/// TimeSleep syscall: x0 = nanoseconds to sleep.
///
/// Converts nanoseconds to ticks and blocks via IPC timeout infrastructure.
fn sys_time_sleep(tf: &TrapFrame) -> i64 {
    let ns = tf.x[0];
    if ns == 0 {
        return 0;
    }
    // Convert nanoseconds to ticks (1 tick = 1ms = 1_000_000 ns).
    let ticks = ns.div_ceil(1_000_000);
    crate::ipc::sleep_ticks(ticks);
    0
}

// ---------------------------------------------------------------------------
// IPC syscall wrappers (EL0 → kernel IPC functions)
// ---------------------------------------------------------------------------

/// IpcCall (nr=0): x0=channel, x1=send_ptr, x2=send_len, x3=recv_ptr, x4=recv_len, x5=timeout.
fn sys_ipc_call(tf: &mut TrapFrame) -> i64 {
    let channel = crate::ipc::ChannelId(tf.x[0] as u32);
    let send_ptr = tf.x[1] as *const u8;
    let send_len = tf.x[2] as usize;
    let recv_ptr = tf.x[3] as *mut u8;
    let recv_len = tf.x[4] as usize;
    let timeout = tf.x[5];

    if send_len > crate::ipc::MAX_MESSAGE_SIZE || recv_len > crate::ipc::MAX_MESSAGE_SIZE {
        return IpcError::Enospc as i64;
    }

    // Validate user pointers.
    if !validate_user_ptr(send_ptr as usize, send_len)
        || !validate_user_ptr(recv_ptr as usize, recv_len)
    {
        return IpcError::Eperm as i64;
    }

    // Copy send data to kernel stack.
    let mut send_buf = [0u8; crate::ipc::MAX_MESSAGE_SIZE];
    // SAFETY: send_ptr validated to be in user VA range, bounded by send_len.
    unsafe { core::ptr::copy_nonoverlapping(send_ptr, send_buf.as_mut_ptr(), send_len) };

    // SAFETY: recv_ptr validated to be in user VA range, bounded by recv_len.
    let recv_slice = unsafe { core::slice::from_raw_parts_mut(recv_ptr, recv_len) };

    crate::ipc::ipc_call(channel, &send_buf[..send_len], recv_slice, timeout)
}

/// IpcSend (nr=1): x0=channel, x1=send_ptr, x2=send_len.
fn sys_ipc_send(tf: &TrapFrame) -> i64 {
    let channel = crate::ipc::ChannelId(tf.x[0] as u32);
    let send_ptr = tf.x[1] as *const u8;
    let send_len = tf.x[2] as usize;

    if send_len > crate::ipc::MAX_MESSAGE_SIZE {
        return IpcError::Enospc as i64;
    }
    if !validate_user_ptr(send_ptr as usize, send_len) {
        return IpcError::Eperm as i64;
    }

    let mut send_buf = [0u8; crate::ipc::MAX_MESSAGE_SIZE];
    // SAFETY: send_ptr validated in user VA range.
    unsafe { core::ptr::copy_nonoverlapping(send_ptr, send_buf.as_mut_ptr(), send_len) };

    crate::ipc::ipc_send(channel, &send_buf[..send_len])
}

/// IpcRecv (nr=2): x0=channel, x1=recv_ptr, x2=recv_len, x3=timeout.
/// Returns bytes_received in x0, sender_tid in x1.
fn sys_ipc_recv(tf: &mut TrapFrame) -> i64 {
    let channel = crate::ipc::ChannelId(tf.x[0] as u32);
    let recv_ptr = tf.x[1] as *mut u8;
    let recv_len = tf.x[2] as usize;
    let timeout = tf.x[3];

    if recv_len > crate::ipc::MAX_MESSAGE_SIZE {
        return IpcError::Enospc as i64;
    }
    if !validate_user_ptr(recv_ptr as usize, recv_len) {
        return IpcError::Eperm as i64;
    }

    // SAFETY: recv_ptr validated in user VA range.
    let recv_slice = unsafe { core::slice::from_raw_parts_mut(recv_ptr, recv_len) };

    match crate::ipc::ipc_recv(channel, recv_slice, timeout) {
        Ok((bytes, sender)) => {
            tf.x[1] = sender.0 as u64; // Return sender_tid in x1.
            bytes as i64
        }
        Err(e) => e,
    }
}

/// IpcReply (nr=3): x0=channel, x1=reply_ptr, x2=reply_len.
fn sys_ipc_reply(tf: &TrapFrame) -> i64 {
    let channel = crate::ipc::ChannelId(tf.x[0] as u32);
    let reply_ptr = tf.x[1] as *const u8;
    let reply_len = tf.x[2] as usize;

    if reply_len > crate::ipc::MAX_MESSAGE_SIZE {
        return IpcError::Enospc as i64;
    }
    if !validate_user_ptr(reply_ptr as usize, reply_len) {
        return IpcError::Eperm as i64;
    }

    let mut reply_buf = [0u8; crate::ipc::MAX_MESSAGE_SIZE];
    // SAFETY: reply_ptr validated in user VA range.
    unsafe { core::ptr::copy_nonoverlapping(reply_ptr, reply_buf.as_mut_ptr(), reply_len) };

    crate::ipc::ipc_reply(channel, &reply_buf[..reply_len])
}

/// IpcCancel (nr=4): x0=channel.
fn sys_ipc_cancel(tf: &TrapFrame) -> i64 {
    let channel = crate::ipc::ChannelId(tf.x[0] as u32);
    crate::ipc::ipc_cancel(channel)
}

/// ChannelCreate (nr=6): returns channel_id in x0.
fn sys_channel_create(_tf: &TrapFrame) -> i64 {
    let tid = match crate::ipc::current_thread_id() {
        Some(t) => t,
        None => return IpcError::Eperm as i64,
    };
    match crate::ipc::channel_create(tid) {
        Ok(ch) => ch.0 as i64,
        Err(e) => e,
    }
}

/// ChannelDestroy (nr=7): x0=channel_id.
fn sys_channel_destroy(tf: &TrapFrame) -> i64 {
    let channel = crate::ipc::ChannelId(tf.x[0] as u32);
    match crate::ipc::channel_destroy(channel) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// Validate a user-space pointer is within the valid user VA range.
fn validate_user_ptr(ptr: usize, len: usize) -> bool {
    let end = match ptr.checked_add(len) {
        Some(e) => e,
        None => return false,
    };
    ptr < 0x0000_8000_0000_0000 && end <= 0x0000_8000_0000_0000
}
