//! Syscall argument hardening self-test (#188) and the shared memory
//! EINVAL/EPERM split (#190), run from the ipc-timeout thread.

use crate::arch::aarch64::mmu::DIRECT_MAP_BASE;
use crate::arch::aarch64::trap::TrapFrame;
use crate::ipc::notify::{self, NOTIFICATION_TABLE};
use crate::ipc::shmem;
use crate::mm::pgtable::VmFlags;
use crate::syscall::IpcError;
use crate::task::process::ProcessId;
use crate::task::ThreadId;
use shared::{
    Capability, ChannelId, NotificationId, RawSelectEntry, SelectEntry, SelectKind, Syscall,
    MAX_NOTIFICATIONS, RING_CAPACITY, USER_VA_MIN,
};

use super::bad_pid::{revoke_region_access, revoke_token, token_id};
use super::TEST_PID;

const PAGE: u64 = 4096;
const READ: u64 = 0b001;
const READ_WRITE: u64 = 0b011;
const WRITE_EXECUTE: u64 = 0b110;

/// Issue syscall `nr` with `args` in x0.. through `syscall_dispatch`, the
/// path an EL0 SVC takes, and return x0 as the signed result.
fn svc(nr: Syscall, args: &[u64]) -> i64 {
    let mut tf = TrapFrame {
        x: [0; 31],
        sp_el0: 0,
        elr_el1: 0,
        spsr_el1: 0,
    };
    tf.x[8] = nr as u64;
    tf.x[..args.len()].copy_from_slice(args);
    crate::syscall::syscall_dispatch(&mut tf);
    tf.x[0] as i64
}

/// Hostile syscall arguments are rejected with EINVAL before they touch
/// memory, MemoryUnmap frees only the caller's exact allocation, and the
/// shared memory calls return EINVAL for a malformed request but EPERM for a
/// missing capability. NotificationSignal on a live notification succeeds and
/// sets the bits a NotificationWait then returns, so a fix that rejected
/// every id cannot pass checks 13-14 alone.
///
/// Runs in the ipc-timeout thread `my_tid` (process 1). Rejected calls change
/// no state and grant nothing. Checks 0-20 and 26-40 log no warning; checks
/// 21-25 (#190) log the kernel's usual W^X, max_flags and denied-capability
/// warnings. The one MemoryMap allocation is freed by the
/// test's own exact unmap, the one shared region by its unmap once the
/// test has revoked every capability it granted, and the one notification by
/// `notification_destroy`.
///
/// DebugPrint, IpcSelect and CapabilityList get this thread's stack buffers
/// as kernel addresses. Before #188 IpcSelect accepted them and read them
/// (check 3); DebugPrint and CapabilityList already rejected them, but with
/// EPERM, so checks 1 and 5 pin the EINVAL. Check 26 is a SharedMemoryCreate
/// of 2^50 bytes while SharedMemoryCreate is held: before the size bound its
/// out-of-memory warning overflowed a shift and panicked the kernel in a
/// build with overflow checks.
///
/// IpcRecv and IpcCall validate their buffers before the call (checks
/// 32-36), on `open`, a channel process 1 can access and that is empty
/// (`select_cap_test`'s `owned_a`): a later copy-out would also return
/// EINVAL, but only after the receive had consumed a message or the call had
/// sent one. IpcRecv gets a bad buffer while one message is queued, which a
/// later in-kernel receive must still find; IpcCall gets an empty send
/// buffer at `USER_VA_MIN` and a page-0 or null reply buffer, and the ring
/// must stay empty. A late check would make IpcCall time out after one tick
/// instead. Checks 39-40 show the handlers accept empty buffers at
/// `USER_VA_MIN`, so a fix that rejected every buffer cannot pass checks
/// 32-36 alone, and that check 35's EINVAL comes from the reply buffer. With
/// the ring filled, so that nothing blocks, an IpcCall with both buffers
/// there gets past validation to the full ring (ENOSPC), and an IpcRecv into
/// an empty buffer there takes a queued message (0 bytes). `open` is left
/// empty for `kit_errors_test`. The checks fail if `open` is `None`.
///
/// MemoryUnmap, which freed direct-map addresses before #188, gets the
/// direct-map address of physical page 0 instead of a stack buffer: that
/// page is in no allocator pool, so a regression that freed it would reach
/// the frame allocator's pool check, not hand this thread's stack to the
/// buddy allocator.
///
/// Logs one line: the success text, or the bitmask of failed checks (bit n
/// is check n below).
pub(super) fn syscall_args_test(my_tid: ThreadId, open: Option<ChannelId>) {
    match crate::cap::process_of_thread(my_tid) {
        Some(p) if p == TEST_PID => {}
        other => {
            crate::kwarn!(
                Ipc,
                "Syscall-arg test: wrong owner {:?}",
                other.map(|p| p.0)
            );
            return;
        }
    }

    let einval = IpcError::Einval as i64;
    let msg = *b"TEST";
    let kernel_msg = msg.as_ptr() as u64;
    let entry = RawSelectEntry::from(SelectEntry {
        kind: SelectKind::Channel(ChannelId(0)),
    });
    let kernel_entries = &entry as *const RawSelectEntry as u64;
    let mut kernel_ids = [0u64; 4];
    let kernel_ids_ptr = kernel_ids.as_mut_ptr() as u64;
    // Page 0, where no user buffer can start.
    let page_zero = 0x800u64;
    let empty_notification = NOTIFICATION_TABLE
        .lock()
        .iter()
        .position(Option::is_none)
        .unwrap_or(MAX_NOTIFICATIONS) as u64;

    let mut checks = [false; 41];

    // User pointers: null, page 0 and kernel addresses are EINVAL.
    checks[0] = svc(Syscall::DebugPrint, &[0, 4]) == einval;
    checks[1] = svc(Syscall::DebugPrint, &[kernel_msg, 4]) == einval;
    checks[2] = svc(Syscall::IpcSelect, &[0, 1, 0]) == einval;
    checks[3] = svc(Syscall::IpcSelect, &[kernel_entries, 1, 0]) == einval;
    checks[4] = svc(Syscall::IpcSend, &[0, page_zero, 4]) == einval;
    checks[5] = svc(Syscall::CapabilityList, &[kernel_ids_ptr, 4]) == einval;

    // Memory flags: bits 32-63, undefined bits, USER and W^X are EINVAL.
    checks[6] = svc(Syscall::MemoryMap, &[PAGE, 0x1_0000_0002]) == einval;
    checks[7] = svc(Syscall::MemoryMap, &[PAGE, 0x10]) == einval;
    checks[8] = svc(Syscall::MemoryMap, &[PAGE, u64::MAX]) == einval;
    checks[9] = svc(Syscall::MemoryMap, &[PAGE, 0b1000]) == einval;
    checks[10] = svc(Syscall::MemoryMap, &[PAGE, 0b110]) == einval;
    checks[11] = svc(Syscall::SharedMemoryCreate, &[PAGE, 0x1_0000_0001]) == einval;

    // An unknown CapabilityAttenuate type, and a missing notification.
    checks[12] = svc(Syscall::CapabilityAttenuate, &[0, 99, 0, 0]) == einval;
    checks[13] = svc(Syscall::NotificationSignal, &[MAX_NOTIFICATIONS as u64, 1]) == einval;
    checks[14] = svc(Syscall::NotificationSignal, &[empty_notification, 1]) == einval;

    // MemoryUnmap frees only the caller's exact allocation.
    let va = svc(Syscall::MemoryMap, &[3 * PAGE, READ_WRITE]);
    let mapped = IpcError::try_from(va).is_err() && va > 0;
    checks[15] = mapped;
    if mapped {
        let va = va as u64;
        // An address inside the allocation, with its full size: only the
        // exact base names it.
        checks[16] = svc(Syscall::MemoryUnmap, &[va + PAGE, 3 * PAGE]) == einval;
        checks[17] = svc(Syscall::MemoryUnmap, &[va, PAGE]) == einval;
        checks[18] = shmem::memory_unmap(ProcessId(TEST_PID.0 + 1), va as usize, 3 * PAGE as usize)
            == Err(einval);
        checks[19] = svc(Syscall::MemoryUnmap, &[DIRECT_MAP_BASE as u64, PAGE]) == einval;
        checks[20] = svc(Syscall::MemoryUnmap, &[va, 3 * PAGE]) == 0
            && svc(Syscall::MemoryUnmap, &[va, 3 * PAGE]) == einval;
    }

    // Shared memory (#190): W^X is EINVAL before the capability check, for
    // SharedMemoryCreate and for SharedMemoryMap of any in-range id.
    checks[21] = svc(Syscall::SharedMemoryCreate, &[PAGE, WRITE_EXECUTE]) == einval;
    checks[22] = svc(Syscall::SharedMemoryMap, &[0, WRITE_EXECUTE]) == einval;
    let [beyond_max, mapped, denied, oversize, high_bits, undefined_bit] =
        shm_rights_checks(TEST_PID);
    checks[23] = beyond_max;
    checks[24] = mapped;
    checks[25] = denied;
    checks[26] = oversize;
    checks[37] = high_bits;
    checks[38] = undefined_bit;

    // A capability handle past the table is malformed, not a missing
    // capability: EINVAL, before any table lookup. A MemoryMap above 64
    // pages is EINVAL too: no caller can allocate it.
    let past_table = shared::MAX_CAPS_PER_PROCESS as u64;
    checks[27] = svc(Syscall::CapabilityRevoke, &[past_table]) == einval;
    checks[28] = svc(Syscall::CapabilityAttenuate, &[past_table, 0, 0, 0]) == einval;
    checks[29] = svc(Syscall::MemoryMap, &[65 * PAGE, READ]) == einval;

    // A live notification (#188 item 8): NotificationSignal returns 0, and
    // the bits it set are the ones a NotificationWait with timeout 0 returns
    // from its fast path. Timeout 0 is not a poll for notification_wait: if
    // the bits were not set, the wait would block until the next tick and
    // time out, so this check fails late rather than at once.
    let id = svc(Syscall::NotificationCreate, &[]);
    if IpcError::try_from(id).is_err() && id >= 0 {
        checks[30] = svc(Syscall::NotificationSignal, &[id as u64, 0b101]) == 0;
        checks[31] = svc(Syscall::NotificationWait, &[id as u64, u64::MAX, 0]) == 0b101;
        notify::notification_destroy(NotificationId(id as u32));
    }

    if let Some(open) = open {
        let [recv_null, recv_page_zero, kept, call_rejected, nothing_sent, call_ok, recv_ok] =
            bad_buffer_checks(open, page_zero);
        checks[32] = recv_null;
        checks[33] = recv_page_zero;
        checks[34] = kept;
        checks[35] = call_rejected;
        checks[36] = nothing_sent;
        checks[39] = call_ok;
        checks[40] = recv_ok;
    }

    let failed = checks
        .iter()
        .enumerate()
        .filter(|(_, ok)| !**ok)
        .fold(0u64, |mask, (i, _)| mask | 1 << i);
    // Log messages are cut at 48 bytes, so keep them short.
    if failed == 0 {
        crate::kinfo!(Ipc, "Syscall-arg test: errnos as expected");
    } else {
        crate::kwarn!(Ipc, "Syscall-arg test: failed {:#x}", failed);
    }
}

/// IpcRecv and IpcCall on `open` with bad buffers, then with empty ones
/// (checks 32-36 and 39-40 of `syscall_args_test`). Returns `[recv_null,
/// recv_page_zero, kept, call_rejected, nothing_sent, call_ok, recv_ok]`,
/// with `kept` and `nothing_sent` false if the message could not be queued.
/// Drains `open` before it returns, whatever the results.
fn bad_buffer_checks(open: ChannelId, page_zero: u64) -> [bool; 7] {
    let einval = IpcError::Einval as i64;
    let channel = open.0 as u64;
    let mut buf = [0u8; 4];
    let mut result = [false; 7];

    let queued = crate::ipc::ipc_send(open, b"TEST") == 0;
    result[0] = svc(Syscall::IpcRecv, &[channel, 0, 4, 0]) == einval;
    result[1] = svc(Syscall::IpcRecv, &[channel, page_zero, 4, 0]) == einval;
    // The message is still queued: neither rejected receive consumed it.
    result[2] = queued && crate::ipc::ipc_recv(open, &mut buf, 0).map(|(n, _)| n) == Ok(4);

    // A valid empty send buffer and a bad reply buffer: EINVAL before the
    // call sends anything, so the ring stays empty.
    let user_va_min = USER_VA_MIN as u64;
    result[3] = svc(
        Syscall::IpcCall,
        &[channel, user_va_min, 0, page_zero, 4, 1],
    ) == einval
        && svc(Syscall::IpcCall, &[channel, user_va_min, 0, 0, 4, 1]) == einval;
    result[4] = queued && crate::ipc::ipc_recv(open, &mut buf, 0) == Err(IpcError::Eagain as i64);

    // Positive controls: empty buffers at USER_VA_MIN pass validation and
    // the copies never touch them. The ring is filled first, so neither call
    // blocks: the call fails at the push with ENOSPC, which only a call that
    // passed validation reaches, and the receive takes a queued message,
    // 0 bytes.
    let mut send = 0;
    for _ in 0..=RING_CAPACITY {
        send = crate::ipc::ipc_send(open, b"TEST");
        if send != 0 {
            break;
        }
    }
    let full = send == IpcError::Eagain as i64;
    result[5] = full
        && svc(
            Syscall::IpcCall,
            &[channel, user_va_min, 0, user_va_min, 0, 1],
        ) == IpcError::Enospc as i64;
    result[6] = full && svc(Syscall::IpcRecv, &[channel, user_va_min, 0, 0]) == 0;

    // Leave `open` empty for kit_errors_test even after a failed check.
    while crate::ipc::ipc_recv(open, &mut buf, 0).is_ok() {}
    result
}

/// On a region with `max_flags` READ | USER that process `pid` (the calling
/// thread's process) creates: SharedMemoryMap with WRITE, beyond the
/// region's `max_flags`, is EINVAL; SharedMemoryMap with READ plus bit 32,
/// or with READ plus USER (bit 3), is EINVAL while SharedMemoryAccess is
/// held (a decode that masked or passed the bit would map the region there,
/// and the test then still unmaps it); a READ map succeeds; and once
/// SharedMemoryAccess is revoked the same map is EPERM. While
/// SharedMemoryCreate is held, a SharedMemoryCreate of 2^50 bytes, above the
/// largest region (4 MiB), is EINVAL.
///
/// Returns `[beyond_max, mapped, denied, oversize, high_bits,
/// undefined_bit]`, one flag per check, all false if the grant fails and
/// all but `oversize` false if the region could not be created. Like
/// `shm_bad_pid_test`, it revokes the SharedMemoryCreate token it granted
/// right after the create, and every SharedMemoryAccess token of the region
/// before the unmap that frees it.
fn shm_rights_checks(pid: ProcessId) -> [bool; 6] {
    let create_token = crate::cap::grant_to_process(pid, Capability::SharedMemoryCreate, false)
        .ok()
        .and_then(|handle| token_id(pid, handle));
    let Some(create_token) = create_token else {
        return [false; 6];
    };
    // The size is checked before the capability, but with the capability
    // held a missing bound would reach the allocation and its warning.
    let oversize = svc(Syscall::SharedMemoryCreate, &[1 << 50, READ]) == IpcError::Einval as i64;
    // Created in-kernel with USER in `max_flags`, which no syscall can ask
    // for, so the `max_flags` check passes a READ | USER map and only the
    // syscall's flag decode can reject it.
    let created = shmem::shared_memory_create(pid, PAGE as usize, VmFlags::READ | VmFlags::USER);
    revoke_token(pid, create_token);
    let Ok(region) = created else {
        return [false, false, false, oversize, false, false];
    };

    let map = |flags: u64| svc(Syscall::SharedMemoryMap, &[region.0 as u64, flags]);
    // The region's limit, not the caller's rights: EINVAL.
    let beyond_max = map(READ_WRITE) == IpcError::Einval as i64;
    // Undefined bits are EINVAL even with the right held: bit 32 above READ,
    // and bit 3 (USER) with READ, which no caller may set.
    let high_bits_result = map(0x1_0000_0001);
    let undefined_bit_result = map(0b1001);
    let high_bits = high_bits_result == IpcError::Einval as i64;
    let undefined_bit = undefined_bit_result == IpcError::Einval as i64;
    // A probe that regressed and mapped the region makes this map EEXIST;
    // the region must still be unmapped below.
    let probe_mapped = high_bits_result > 0 || undefined_bit_result > 0;
    // The creator holds SharedMemoryAccess(region), so READ maps.
    let mapped = map(READ) > 0;
    revoke_region_access(pid, region);
    // The capability is checked before the duplicate-mapping check, so the
    // repeated map fails for the missing right: EPERM, not EEXIST.
    let denied = map(READ) == IpcError::Eperm as i64;

    if mapped || probe_mapped {
        let _ = shmem::shared_memory_unmap(pid, region);
    }
    [
        beyond_max,
        mapped,
        denied,
        oversize,
        high_bits,
        undefined_bit,
    ]
}
