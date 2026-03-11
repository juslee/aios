//! Syscall dispatch and handlers.
//!
//! Dispatches SVC traps from EL0 based on syscall number in x8.
//! Per ipc.md §3.1–3.2.

use crate::arch::aarch64::trap::TrapFrame;

// Re-export ABI types from shared crate.
pub use shared::IpcError;
#[allow(unused_imports)]
pub use shared::{Syscall, SYSCALL_COUNT};

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
        10..=12 => IpcError::Enotsup as i64, // Notification — Step 10
        13 => IpcError::Enotsup as i64,    // ChannelStats — future
        14 => sys_capability_transfer(tf),
        15 => sys_capability_attenuate(tf),
        16 => sys_capability_revoke(tf),
        17 => sys_capability_list(tf),
        18 => sys_memory_map(tf),
        19 => sys_memory_unmap(tf),
        20 => sys_shared_memory_create(tf),
        21 => sys_shared_memory_map(tf),
        22 => sys_shared_memory_share(tf),
        23..=25 => IpcError::Enotsup as i64, // Process — Step 11
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
    shared::validate_user_va(ptr, len)
}

// ---------------------------------------------------------------------------
// Capability syscalls (nr=14-17)
// ---------------------------------------------------------------------------

/// CapabilityTransfer (nr=14): x0=channel_id, x1=cap_handle.
///
/// Transfer a capability to the peer process via the channel.
/// Stub for Phase 3 — full implementation requires peer process tracking.
fn sys_capability_transfer(_tf: &mut TrapFrame) -> i64 {
    IpcError::Enotsup as i64
}

/// CapabilityAttenuate (nr=15): x0=cap_handle, x1=new_cap_type, x2=new_expiry, x3=resource_id.
///
/// Create a narrower child capability from an existing one.
/// x3 is required when new_cap_type is ChannelAccess(1) or SharedMemoryAccess(3).
fn sys_capability_attenuate(tf: &mut TrapFrame) -> i64 {
    let handle = shared::CapabilityHandle(tf.x[0] as u32);
    let new_cap_type = tf.x[1];
    let new_expiry = if tf.x[2] == 0 { None } else { Some(tf.x[2]) };

    let pid = match crate::cap::current_process_id() {
        Some(p) => p,
        None => return IpcError::Eperm as i64,
    };

    // Decode capability type from x1.
    // Encoding: 0=ChannelCreate, 1=ChannelAccess(x3), 2=ShmCreate,
    //           3=ShmAccess(x3), 4=SpawnAgent, 5=DebugPrint
    let new_cap = match new_cap_type {
        0 => shared::Capability::ChannelCreate,
        1 => shared::Capability::ChannelAccess(shared::ChannelId(tf.x[3] as u32)),
        2 => shared::Capability::SharedMemoryCreate,
        3 => shared::Capability::SharedMemoryAccess(tf.x[3] as u32),
        4 => shared::Capability::SpawnAgent,
        5 => shared::Capability::DebugPrint,
        _ => return IpcError::Eperm as i64,
    };

    let mut table = crate::task::process::PROCESS_TABLE.lock();
    let proc = match &mut table[pid.0 as usize] {
        Some(p) => p,
        None => return IpcError::Eperm as i64,
    };

    match proc.cap_table.attenuate(handle, new_cap, new_expiry, pid) {
        Ok(h) => h.0 as i64,
        Err(e) => e,
    }
}

/// CapabilityRevoke (nr=16): x0=cap_handle.
///
/// Revoke a capability and cascade to all children. Destroys channels
/// created under the revoked capability.
fn sys_capability_revoke(tf: &mut TrapFrame) -> i64 {
    let handle = shared::CapabilityHandle(tf.x[0] as u32);

    let pid = match crate::cap::current_process_id() {
        Some(p) => p,
        None => return IpcError::Eperm as i64,
    };

    // Get the token ID before revoking.
    let token_id = {
        let table = crate::task::process::PROCESS_TABLE.lock();
        let proc = match &table[pid.0 as usize] {
            Some(p) => p,
            None => return IpcError::Eperm as i64,
        };
        match proc.cap_table.get(handle) {
            Some(token) => token.id,
            None => return IpcError::Eperm as i64,
        }
    };

    crate::cap::revoke_in_process(pid, token_id);
    0
}

/// CapabilityList (nr=17): x0=buf_ptr, x1=max_count.
///
/// List non-revoked capability token IDs into a user buffer.
/// Returns number of token IDs written.
fn sys_capability_list(tf: &mut TrapFrame) -> i64 {
    let buf_ptr = tf.x[0] as usize;
    let max_count = tf.x[1] as usize;

    // Each token ID is u64 = 8 bytes.
    let byte_len = max_count.saturating_mul(8);
    if !validate_user_ptr(buf_ptr, byte_len) {
        return IpcError::Eperm as i64;
    }

    let pid = match crate::cap::current_process_id() {
        Some(p) => p,
        None => return IpcError::Eperm as i64,
    };

    let table = crate::task::process::PROCESS_TABLE.lock();
    let proc = match &table[pid.0 as usize] {
        Some(p) => p,
        None => return IpcError::Eperm as i64,
    };

    // Collect to kernel stack buffer (max 256 entries × 8 bytes = 2 KiB).
    let capped = max_count.min(shared::MAX_CAPS_PER_PROCESS);
    let mut ids = [shared::CapabilityTokenId(0); 256];
    let count = proc.cap_table.list(&mut ids, capped);

    // Copy to user buffer.
    // SAFETY: buf_ptr validated in user VA range, bounded by count × 8 bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(ids.as_ptr() as *const u8, buf_ptr as *mut u8, count * 8);
    }

    count as i64
}

// ---------------------------------------------------------------------------
// Memory syscalls (nr=18-22)
// ---------------------------------------------------------------------------

/// MemoryMap (nr=18): x0=size, x1=flags.
///
/// Allocate private pages from Pool::User.
fn sys_memory_map(tf: &TrapFrame) -> i64 {
    let size = tf.x[0] as usize;
    let flags_raw = tf.x[1] as u32;
    let flags = crate::mm::pgtable::VmFlags::from_bits(flags_raw);

    let pid = match crate::cap::current_process_id() {
        Some(p) => p,
        None => return IpcError::Eperm as i64,
    };

    match crate::ipc::shmem::memory_map(pid, size, flags) {
        Ok(va) => va as i64,
        Err(e) => e,
    }
}

/// MemoryUnmap (nr=19): x0=va, x1=size.
///
/// Handles both private and shared memory unmap.
fn sys_memory_unmap(tf: &TrapFrame) -> i64 {
    let va = tf.x[0] as usize;
    let size = tf.x[1] as usize;

    let pid = match crate::cap::current_process_id() {
        Some(p) => p,
        None => return IpcError::Eperm as i64,
    };

    match crate::ipc::shmem::memory_unmap(pid, va, size) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// SharedMemoryCreate (nr=20): x0=size, x1=flags.
///
/// Create a new shared memory region.
fn sys_shared_memory_create(tf: &TrapFrame) -> i64 {
    let size = tf.x[0] as usize;
    let flags_raw = tf.x[1] as u32;
    let flags = crate::mm::pgtable::VmFlags::from_bits(flags_raw);

    let pid = match crate::cap::current_process_id() {
        Some(p) => p,
        None => return IpcError::Eperm as i64,
    };

    match crate::ipc::shmem::shared_memory_create(pid, size, flags) {
        Ok(id) => id.0 as i64,
        Err(e) => e,
    }
}

/// SharedMemoryMap (nr=21): x0=region_id, x1=flags.
///
/// Map a shared memory region into the caller's address space.
fn sys_shared_memory_map(tf: &TrapFrame) -> i64 {
    let region_id = shared::SharedMemoryId(tf.x[0] as u32);
    let flags_raw = tf.x[1] as u32;
    let flags = crate::mm::pgtable::VmFlags::from_bits(flags_raw);

    let pid = match crate::cap::current_process_id() {
        Some(p) => p,
        None => return IpcError::Eperm as i64,
    };

    match crate::ipc::shmem::shared_memory_map(pid, region_id, flags) {
        Ok(va) => va as i64,
        Err(e) => e,
    }
}

/// SharedMemoryShare (nr=22): x0=region_id, x1=target_pid.
///
/// Share a region with another process by granting capability.
fn sys_shared_memory_share(tf: &TrapFrame) -> i64 {
    let region_id = shared::SharedMemoryId(tf.x[0] as u32);
    let target_pid = crate::task::process::ProcessId(tf.x[1] as u32);

    let pid = match crate::cap::current_process_id() {
        Some(p) => p,
        None => return IpcError::Eperm as i64,
    };

    match crate::ipc::shmem::shared_memory_share(pid, region_id, target_pid) {
        Ok(()) => 0,
        Err(e) => e,
    }
}
