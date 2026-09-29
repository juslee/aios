//! Syscall dispatch and handlers.
//!
//! Dispatches SVC traps from EL0 based on syscall number in x8.
//! Per ipc.md §3.1–3.2.

mod user;

use crate::arch::aarch64::trap::TrapFrame;
use shared::syscall::id_arg;
use user::{copy_from_user, copy_to_user, validate_user_ptr};

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
///
/// Id arguments (channel, shared memory region, notification, process,
/// capability handle) are decoded with `id_arg`: a register value that does
/// not fit in `u32` returns `EINVAL` rather than being truncated.
///
/// User buffers are range-checked with `validate_user_ptr` and read or written
/// only through `copy_from_user` / `copy_to_user`: a null, page-0, kernel or
/// overflowing range returns `EINVAL`.
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
        5 => sys_ipc_select(tf),
        6 => sys_channel_create(tf),
        7 => sys_channel_destroy(tf),
        8 | 9 => IpcError::Enotsup as i64, // RingChannel — future
        10 => sys_notification_create(tf),
        11 => sys_notification_signal(tf),
        12 => sys_notification_wait(tf),
        13 => IpcError::Enotsup as i64, // ChannelStats — future
        14 => sys_capability_transfer(tf),
        15 => sys_capability_attenuate(tf),
        16 => sys_capability_revoke(tf),
        17 => sys_capability_list(tf),
        18 => sys_memory_map(tf),
        19 => sys_memory_unmap(tf),
        20 => sys_shared_memory_create(tf),
        21 => sys_shared_memory_map(tf),
        22 => sys_shared_memory_share(tf),
        23 => IpcError::Enotsup as i64, // ProcessCreate — kernel-only in Phase 3
        24 => sys_process_exit(tf),
        25 => sys_process_wait(tf),
        26 => sys_time_get(tf),
        27 => sys_time_sleep(tf),
        28 => IpcError::Enotsup as i64,
        29 => sys_audit_log(tf),
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
/// ENOSPC if len > 256; EINVAL unless the range passes `validate_user_ptr`.
/// Copies the message to a kernel stack buffer before printing.
fn sys_debug_print(tf: &TrapFrame) -> i64 {
    let ptr = tf.x[0] as usize;
    let len = tf.x[1] as usize;

    // Validate length.
    if len > 256 {
        return IpcError::Enospc as i64;
    }

    let mut buf = [0u8; 256];
    if let Err(e) = copy_from_user(&mut buf[..len], ptr) {
        return e;
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
    // SAFETY: MRS of CNTVCT_EL0 and CNTFRQ_EL0 reads no memory, has no side
    // effects, and is always permitted at EL1 (CNTKCTL_EL1 gates only EL0).
    // The architecture guarantees both registers (the generic timer is
    // mandatory in ARMv8-A), and syscall_dispatch runs only at EL1.
    // Executed at EL0 with CNTKCTL_EL1 denying access, the MRS would trap as
    // an undefined instruction and halt the CPU; no memory is touched either way.
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
///
/// Both buffers are validated before the call starts, so a bad reply buffer
/// fails with EINVAL before anything is sent. The reply is received into a
/// kernel stack buffer and copied out after the call returns: the replier
/// writes the reply from its own thread, possibly on another CPU, through the
/// pointer in `REPLY_SLOTS`, so that pointer must be a kernel address.
fn sys_ipc_call(tf: &mut TrapFrame) -> i64 {
    let channel = match id_arg(tf.x[0]) {
        Ok(id) => crate::ipc::ChannelId(id),
        Err(e) => return e,
    };
    let send_ptr = tf.x[1] as usize;
    let send_len = tf.x[2] as usize;
    let recv_ptr = tf.x[3] as usize;
    let recv_len = tf.x[4] as usize;
    let timeout = tf.x[5];

    if send_len > crate::ipc::MAX_MESSAGE_SIZE || recv_len > crate::ipc::MAX_MESSAGE_SIZE {
        return IpcError::Enospc as i64;
    }
    if !validate_user_ptr(send_ptr, send_len) || !validate_user_ptr(recv_ptr, recv_len) {
        return IpcError::Einval as i64;
    }

    let mut send_buf = [0u8; crate::ipc::MAX_MESSAGE_SIZE];
    if let Err(e) = copy_from_user(&mut send_buf[..send_len], send_ptr) {
        return e;
    }

    let mut recv_buf = [0u8; crate::ipc::MAX_MESSAGE_SIZE];
    let result = crate::ipc::ipc_call(
        channel,
        &send_buf[..send_len],
        &mut recv_buf[..recv_len],
        timeout,
    );
    if result < 0 {
        return result;
    }
    // ipc_call returns the reply length, which never exceeds recv_len.
    let reply_len = (result as usize).min(recv_len);
    match copy_to_user(recv_ptr, &recv_buf[..reply_len]) {
        Ok(()) => reply_len as i64,
        Err(e) => e,
    }
}

/// IpcSend (nr=1): x0=channel, x1=send_ptr, x2=send_len.
fn sys_ipc_send(tf: &TrapFrame) -> i64 {
    let channel = match id_arg(tf.x[0]) {
        Ok(id) => crate::ipc::ChannelId(id),
        Err(e) => return e,
    };
    let send_ptr = tf.x[1] as usize;
    let send_len = tf.x[2] as usize;

    if send_len > crate::ipc::MAX_MESSAGE_SIZE {
        return IpcError::Enospc as i64;
    }

    let mut send_buf = [0u8; crate::ipc::MAX_MESSAGE_SIZE];
    if let Err(e) = copy_from_user(&mut send_buf[..send_len], send_ptr) {
        return e;
    }

    crate::ipc::ipc_send(channel, &send_buf[..send_len])
}

/// IpcRecv (nr=2): x0=channel, x1=recv_ptr, x2=recv_len, x3=timeout.
/// Returns bytes_received in x0, sender_tid in x1.
///
/// The buffer is validated before the receive, so a bad buffer fails with
/// EINVAL without consuming a message. The message is received into a
/// kernel stack buffer and copied out after `ipc_recv` has released
/// `CHANNEL_TABLE`.
fn sys_ipc_recv(tf: &mut TrapFrame) -> i64 {
    let channel = match id_arg(tf.x[0]) {
        Ok(id) => crate::ipc::ChannelId(id),
        Err(e) => return e,
    };
    let recv_ptr = tf.x[1] as usize;
    let recv_len = tf.x[2] as usize;
    let timeout = tf.x[3];

    if recv_len > crate::ipc::MAX_MESSAGE_SIZE {
        return IpcError::Enospc as i64;
    }
    if !validate_user_ptr(recv_ptr, recv_len) {
        return IpcError::Einval as i64;
    }

    let mut recv_buf = [0u8; crate::ipc::MAX_MESSAGE_SIZE];
    match crate::ipc::ipc_recv(channel, &mut recv_buf[..recv_len], timeout) {
        Ok((bytes, sender)) => {
            if let Err(e) = copy_to_user(recv_ptr, &recv_buf[..bytes]) {
                return e;
            }
            tf.x[1] = sender.0 as u64; // Return sender_tid in x1.
            bytes as i64
        }
        Err(e) => e,
    }
}

/// IpcReply (nr=3): x0=channel, x1=reply_ptr, x2=reply_len.
fn sys_ipc_reply(tf: &TrapFrame) -> i64 {
    let channel = match id_arg(tf.x[0]) {
        Ok(id) => crate::ipc::ChannelId(id),
        Err(e) => return e,
    };
    let reply_ptr = tf.x[1] as usize;
    let reply_len = tf.x[2] as usize;

    if reply_len > crate::ipc::MAX_MESSAGE_SIZE {
        return IpcError::Enospc as i64;
    }

    let mut reply_buf = [0u8; crate::ipc::MAX_MESSAGE_SIZE];
    if let Err(e) = copy_from_user(&mut reply_buf[..reply_len], reply_ptr) {
        return e;
    }

    crate::ipc::ipc_reply(channel, &reply_buf[..reply_len])
}

/// IpcCancel (nr=4): x0=channel.
fn sys_ipc_cancel(tf: &TrapFrame) -> i64 {
    let channel = match id_arg(tf.x[0]) {
        Ok(id) => crate::ipc::ChannelId(id),
        Err(e) => return e,
    };
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
    let channel = match id_arg(tf.x[0]) {
        Ok(id) => crate::ipc::ChannelId(id),
        Err(e) => return e,
    };
    match crate::ipc::channel_destroy(channel) {
        Ok(()) => 0,
        Err(e) => e,
    }
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
/// EINVAL for an unknown new_cap_type or an x3 that does not fit in u32.
fn sys_capability_attenuate(tf: &mut TrapFrame) -> i64 {
    let handle = match id_arg(tf.x[0]) {
        Ok(h) => shared::CapabilityHandle(h),
        Err(e) => return e,
    };
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
        1 => match id_arg(tf.x[3]) {
            Ok(id) => shared::Capability::ChannelAccess(shared::ChannelId(id)),
            Err(e) => return e,
        },
        2 => shared::Capability::SharedMemoryCreate,
        3 => match id_arg(tf.x[3]) {
            Ok(id) => shared::Capability::SharedMemoryAccess(id),
            Err(e) => return e,
        },
        4 => shared::Capability::SpawnAgent,
        5 => shared::Capability::DebugPrint,
        // A malformed argument, like an out-of-range x3 above: EINVAL, so a
        // caller can tell it from a permission denial (EPERM).
        _ => return IpcError::Einval as i64,
    };

    let mut table = crate::task::process::PROCESS_TABLE.lock();
    let proc = match crate::task::process::process_mut(&mut table, pid) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let new_id = crate::cap::new_token_id();
    match proc
        .cap_table
        .attenuate(handle, new_cap, new_expiry, pid, new_id)
    {
        Ok(h) => h.0 as i64,
        Err(e) => e,
    }
}

/// CapabilityRevoke (nr=16): x0=cap_handle.
///
/// Revoke a capability and cascade to all children. Destroys channels
/// created under the revoked capability.
fn sys_capability_revoke(tf: &mut TrapFrame) -> i64 {
    let handle = match id_arg(tf.x[0]) {
        Ok(h) => shared::CapabilityHandle(h),
        Err(e) => return e,
    };

    let pid = match crate::cap::current_process_id() {
        Some(p) => p,
        None => return IpcError::Eperm as i64,
    };

    // Get the token ID before revoking.
    let token_id = {
        let table = crate::task::process::PROCESS_TABLE.lock();
        let proc = match crate::task::process::process_ref(&table, pid) {
            Ok(p) => p,
            Err(e) => return e,
        };
        match proc.cap_table.get(handle) {
            Some(token) => token.id,
            None => return IpcError::Eperm as i64,
        }
    };

    match crate::cap::revoke_in_process(pid, token_id) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// CapabilityList (nr=17): x0=buf_ptr, x1=max_count.
///
/// List non-revoked capability token IDs into a user buffer.
/// Returns number of token IDs written.
///
/// Each id is written as a native-endian u64. EINVAL unless the whole buffer,
/// `max_count` ids, passes `validate_user_ptr`.
fn sys_capability_list(tf: &mut TrapFrame) -> i64 {
    const ID_BYTES: usize = core::mem::size_of::<u64>();

    let buf_ptr = tf.x[0] as usize;
    let max_count = tf.x[1] as usize;

    let byte_len = max_count.saturating_mul(ID_BYTES);
    if !validate_user_ptr(buf_ptr, byte_len) {
        return IpcError::Einval as i64;
    }

    let pid = match crate::cap::current_process_id() {
        Some(p) => p,
        None => return IpcError::Eperm as i64,
    };

    // Collect to kernel stack buffer (max 256 entries × 8 bytes = 2 KiB).
    let capped = max_count.min(shared::MAX_CAPS_PER_PROCESS);
    let mut ids = [shared::CapabilityTokenId(0); shared::MAX_CAPS_PER_PROCESS];
    let count = {
        let table = crate::task::process::PROCESS_TABLE.lock();
        let proc = match crate::task::process::process_ref(&table, pid) {
            Ok(p) => p,
            Err(e) => return e,
        };
        proc.cap_table.list(&mut ids, capped)
    };

    // PROCESS_TABLE is released: a fault on the user page must not halt this
    // CPU with the top lock held, which would stall every capability check.
    for (i, id) in ids[..count].iter().enumerate() {
        if let Err(e) = copy_to_user(buf_ptr + i * ID_BYTES, &id.0.to_ne_bytes()) {
            return e;
        }
    }

    count as i64
}

// ---------------------------------------------------------------------------
// Memory syscalls (nr=18-22)
// ---------------------------------------------------------------------------

/// MemoryMap (nr=18): x0=size, x1=flags.
///
/// Allocate private pages from Pool::User and return their address in the
/// private VA window (`shmem::memory_map`, ipc.md §4.7). `flags` is decoded
/// with `VmFlags::from_caller_bits`: READ, WRITE and EXECUTE only, EINVAL for
/// any other bit (USER included).
fn sys_memory_map(tf: &TrapFrame) -> i64 {
    let size = tf.x[0] as usize;
    let flags = match crate::mm::pgtable::VmFlags::from_caller_bits(tf.x[1]) {
        Ok(f) => f,
        Err(e) => return e,
    };

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
/// Handles both private and shared memory unmap. A private unmap frees only
/// the caller's own MemoryMap allocation at exactly `va` with the same page
/// count; anything else is EINVAL (`shmem::memory_unmap`, ipc.md §4.7).
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
/// Create a new shared memory region. `flags` (the region's maximum
/// permissions) is decoded with `VmFlags::from_caller_bits`: READ, WRITE and
/// EXECUTE only, EINVAL for any other bit (USER included).
fn sys_shared_memory_create(tf: &TrapFrame) -> i64 {
    let size = tf.x[0] as usize;
    let flags = match crate::mm::pgtable::VmFlags::from_caller_bits(tf.x[1]) {
        Ok(f) => f,
        Err(e) => return e,
    };

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
/// Map a shared memory region into the caller's address space. `flags` is
/// decoded with `VmFlags::from_caller_bits`: READ, WRITE and EXECUTE only,
/// EINVAL for any other bit (USER included).
fn sys_shared_memory_map(tf: &TrapFrame) -> i64 {
    let region_id = match id_arg(tf.x[0]) {
        Ok(id) => shared::SharedMemoryId(id),
        Err(e) => return e,
    };
    let flags = match crate::mm::pgtable::VmFlags::from_caller_bits(tf.x[1]) {
        Ok(f) => f,
        Err(e) => return e,
    };

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
    let region_id = match id_arg(tf.x[0]) {
        Ok(id) => shared::SharedMemoryId(id),
        Err(e) => return e,
    };
    let target_pid = match id_arg(tf.x[1]) {
        Ok(pid) => crate::task::process::ProcessId(pid),
        Err(e) => return e,
    };

    let pid = match crate::cap::current_process_id() {
        Some(p) => p,
        None => return IpcError::Eperm as i64,
    };

    match crate::ipc::shmem::shared_memory_share(pid, region_id, target_pid) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

// ---------------------------------------------------------------------------
// Notification syscalls (nr=10-12)
// ---------------------------------------------------------------------------

/// NotificationCreate (nr=10): no args.
///
/// Create a new notification object owned by the caller's process.
fn sys_notification_create(_tf: &TrapFrame) -> i64 {
    let pid = match crate::cap::current_process_id() {
        Some(p) => p,
        None => return IpcError::Eperm as i64,
    };

    match crate::ipc::notify::notification_create(pid) {
        Ok(id) => id.0 as i64,
        Err(e) => e,
    }
}

/// NotificationSignal (nr=11): x0=notification_id, x1=bits.
///
/// Atomically OR bits into the notification word and wake matching waiters.
/// EINVAL for an out-of-range id or a notification that does not exist.
fn sys_notification_signal(tf: &TrapFrame) -> i64 {
    let id = match id_arg(tf.x[0]) {
        Ok(id) => shared::NotificationId(id),
        Err(e) => return e,
    };
    let bits = tf.x[1];

    match crate::ipc::notify::notification_signal(id, bits) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// NotificationWait (nr=12): x0=notification_id, x1=mask, x2=timeout_ticks.
///
/// Wait for matching bits. Returns matched bits or error.
fn sys_notification_wait(tf: &TrapFrame) -> i64 {
    let id = match id_arg(tf.x[0]) {
        Ok(id) => shared::NotificationId(id),
        Err(e) => return e,
    };
    let mask = tf.x[1];
    let timeout = tf.x[2];

    match crate::ipc::notify::notification_wait(id, mask, timeout) {
        Ok(bits) => bits as i64,
        Err(e) => e,
    }
}

// ---------------------------------------------------------------------------
// IpcSelect (nr=5)
// ---------------------------------------------------------------------------

/// IpcSelect (nr=5): x0=entries_ptr, x1=entry_count, x2=timeout_ticks.
///
/// Wait on multiple channels/notifications. Returns ready index in x0,
/// matched bits (for notifications) in x1.
///
/// `entries_ptr` is a user address: an array of `entry_count`
/// `shared::RawSelectEntry` (16 bytes each, any alignment). EINVAL if
/// `entry_count` is 0 or above `MAX_SELECT_ENTRIES`, if the array fails
/// `validate_user_ptr`, or if an entry has an unknown kind. The array is
/// copied into a kernel buffer once and decoded from there.
fn sys_ipc_select(tf: &mut TrapFrame) -> i64 {
    use shared::{RawSelectEntry, SelectEntry, SelectKind, MAX_SELECT_ENTRIES};

    let entries_ptr = tf.x[0] as usize;
    let entry_count = tf.x[1] as usize;
    let timeout = tf.x[2];

    if entry_count == 0 || entry_count > MAX_SELECT_ENTRIES {
        return IpcError::Einval as i64;
    }

    let mut wire = [0u8; MAX_SELECT_ENTRIES * RawSelectEntry::SIZE];
    let wire_len = entry_count * RawSelectEntry::SIZE;
    if let Err(e) = copy_from_user(&mut wire[..wire_len], entries_ptr) {
        return e;
    }

    let mut entries = [SelectEntry {
        kind: SelectKind::Channel(shared::ChannelId(0)),
    }; MAX_SELECT_ENTRIES];
    // wire_len is a whole number of entries, so there is no remainder.
    let (chunks, _) = wire[..wire_len].as_chunks::<{ RawSelectEntry::SIZE }>();
    for (entry, bytes) in entries.iter_mut().zip(chunks) {
        *entry = match SelectEntry::try_from(RawSelectEntry::from_bytes(bytes)) {
            Ok(e) => e,
            Err(e) => return e,
        };
    }

    match crate::ipc::select::ipc_select(&entries[..entry_count], timeout) {
        Ok((idx, bits)) => {
            // Store matched bits in x1 for the caller.
            tf.x[1] = bits;
            idx as i64
        }
        Err(e) => e,
    }
}

// ---------------------------------------------------------------------------
// Process syscalls (nr=23-25)
// ---------------------------------------------------------------------------

/// ProcessExit (nr=24): x0=exit_code.
///
/// Exit the current process. Cleans up all threads, channels, and
/// notifies the service manager.
fn sys_process_exit(tf: &TrapFrame) -> i64 {
    let exit_code = tf.x[0] as i32;

    let pid = match crate::cap::current_process_id() {
        Some(p) => p,
        None => return IpcError::Eperm as i64,
    };

    crate::task::process::process_exit(pid, exit_code);
    0
}

/// ProcessWait (nr=25): x0=child_pid.
///
/// Block until a child process exits, return its exit code.
fn sys_process_wait(tf: &TrapFrame) -> i64 {
    let child_pid = match id_arg(tf.x[0]) {
        Ok(pid) => shared::ProcessId(pid),
        Err(e) => return e,
    };
    let tid = match crate::ipc::current_thread_id() {
        Some(t) => t,
        None => return IpcError::Eperm as i64,
    };
    match crate::task::process::process_wait(tid, child_pid) {
        Ok(code) => code as i64,
        Err(e) => e,
    }
}

// ---------------------------------------------------------------------------
// AuditLog syscall (nr=29)
// ---------------------------------------------------------------------------

/// AuditLog (nr=29): x0=event_ptr, x1=event_len.
///
/// Append an audit event from user space.
fn sys_audit_log(tf: &TrapFrame) -> i64 {
    let ptr = tf.x[0] as usize;
    let len = tf.x[1] as usize;

    if len > 48 {
        return IpcError::Enospc as i64;
    }

    let mut buf = [0u8; 48];
    if let Err(e) = copy_from_user(&mut buf[..len], ptr) {
        return e;
    }

    let pid = match crate::cap::current_process_id() {
        Some(p) => p,
        None => return IpcError::Eperm as i64,
    };

    crate::service::audit_log(pid, &buf[..len]);
    0
}
