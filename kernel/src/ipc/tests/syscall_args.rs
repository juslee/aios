//! Syscall argument hardening self-test (#188) and the shared memory
//! EINVAL/EPERM split (#190), run from the ipc-timeout thread.

use crate::arch::aarch64::mmu::DIRECT_MAP_BASE;
use crate::arch::aarch64::trap::TrapFrame;
use crate::ipc::notify::NOTIFICATION_TABLE;
use crate::ipc::shmem;
use crate::mm::pgtable::VmFlags;
use crate::syscall::IpcError;
use crate::task::process::ProcessId;
use crate::task::ThreadId;
use shared::{
    Capability, ChannelId, RawSelectEntry, SelectEntry, SelectKind, Syscall, MAX_NOTIFICATIONS,
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
/// missing capability.
///
/// Runs in the ipc-timeout thread `my_tid` (process 1). Rejected calls change
/// no state and grant nothing. Checks 0-20 (#188) log no warning; checks
/// 21-25 (#190) log the kernel's usual W^X, max_flags and denied-capability
/// warnings. The one MemoryMap allocation is freed by the test's own exact
/// unmap, and the one shared region by its unmap once the test has revoked
/// every capability it granted.
///
/// DebugPrint, IpcSelect and CapabilityList get this thread's stack buffers
/// as kernel addresses; before #188 they were accepted (IpcSelect read
/// them). MemoryUnmap, which freed direct-map addresses before #188, gets
/// the direct-map address of physical page 0 instead: that page is in no
/// allocator pool, so a regression that freed it would reach the frame
/// allocator's pool check, not hand this thread's stack to the buddy
/// allocator.
///
/// Logs one line: the success text, or the bitmask of failed checks (bit n
/// is check n below).
pub(super) fn syscall_args_test(my_tid: ThreadId) {
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

    let mut checks = [false; 26];

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
        checks[16] = svc(Syscall::MemoryUnmap, &[va + PAGE, 2 * PAGE]) == einval;
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
    let [beyond_max, mapped, denied] = shm_rights_checks(TEST_PID);
    checks[23] = beyond_max;
    checks[24] = mapped;
    checks[25] = denied;

    let failed = checks
        .iter()
        .enumerate()
        .filter(|(_, ok)| !**ok)
        .fold(0u32, |mask, (i, _)| mask | 1 << i);
    // Log messages are cut at 48 bytes, so keep them short.
    if failed == 0 {
        crate::kinfo!(Ipc, "Syscall-arg test: errnos as expected");
    } else {
        crate::kwarn!(Ipc, "Syscall-arg test: failed {:#x}", failed);
    }
}

/// On a READ-only region that process `pid` creates: SharedMemoryMap with
/// WRITE, beyond the region's `max_flags`, is EINVAL; a READ map succeeds;
/// and once SharedMemoryAccess is revoked the same map is EPERM.
///
/// Returns `[beyond_max, mapped, denied]`, one flag per check, all false if
/// the region could not be created. Like `shm_bad_pid_test`, it revokes the
/// SharedMemoryCreate token it granted right after the create, and every
/// SharedMemoryAccess token of the region before the unmap that frees it.
fn shm_rights_checks(pid: ProcessId) -> [bool; 3] {
    let create_token = crate::cap::grant_to_process(pid, Capability::SharedMemoryCreate, false)
        .ok()
        .and_then(|handle| token_id(pid, handle));
    let Some(create_token) = create_token else {
        return [false; 3];
    };
    let created = shmem::shared_memory_create(pid, PAGE as usize, VmFlags::READ);
    revoke_token(pid, create_token);
    let Ok(region) = created else {
        return [false; 3];
    };

    let map = |flags: u64| svc(Syscall::SharedMemoryMap, &[region.0 as u64, flags]);
    // The region's limit, not the caller's rights: EINVAL.
    let beyond_max = map(READ_WRITE) == IpcError::Einval as i64;
    // The creator holds SharedMemoryAccess(region), so READ maps.
    let mapped = map(READ) > 0;
    revoke_region_access(pid, region);
    // The capability is checked before the duplicate-mapping check, so the
    // repeated map fails for the missing right: EPERM, not EEXIST.
    let denied = map(READ) == IpcError::Eperm as i64;

    if mapped {
        let _ = shmem::shared_memory_unmap(pid, region);
    }
    [beyond_max, mapped, denied]
}
