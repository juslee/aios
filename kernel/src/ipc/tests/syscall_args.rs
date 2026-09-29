//! Syscall argument hardening self-test (#188), run from the ipc-timeout
//! thread.

use crate::arch::aarch64::trap::TrapFrame;
use crate::ipc::notify::NOTIFICATION_TABLE;
use crate::ipc::shmem;
use crate::syscall::IpcError;
use crate::task::process::ProcessId;
use crate::task::ThreadId;
use shared::{ChannelId, RawSelectEntry, SelectEntry, SelectKind, Syscall, MAX_NOTIFICATIONS};

use super::TEST_PID;

const PAGE: u64 = 4096;
const READ_WRITE: u64 = 0b011;

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
/// memory, and MemoryUnmap frees only the caller's exact allocation.
///
/// Runs in the ipc-timeout thread `my_tid` (process 1). Every rejected call
/// fails before it takes a lock, grants anything or logs a warning; the one
/// MemoryMap allocation is freed by the test's own exact unmap. The kernel
/// addresses passed below are this thread's stack buffers: before #188 they
/// were accepted (IpcSelect read them, MemoryUnmap freed their pages).
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

    let mut checks = [false; 21];

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
        checks[19] = svc(Syscall::MemoryUnmap, &[kernel_ids_ptr, PAGE]) == einval;
        checks[20] = svc(Syscall::MemoryUnmap, &[va, 3 * PAGE]) == 0
            && svc(Syscall::MemoryUnmap, &[va, 3 * PAGE]) == einval;
    }

    let failed = checks
        .iter()
        .enumerate()
        .filter(|(_, ok)| !**ok)
        .fold(0u32, |mask, (i, _)| mask | 1 << i);
    // Log messages are cut at 48 bytes, so keep them short.
    if failed == 0 {
        crate::kinfo!(Ipc, "Syscall-arg test: EINVAL as expected");
    } else {
        crate::kwarn!(Ipc, "Syscall-arg test: failed {:#x}", failed);
    }
}
