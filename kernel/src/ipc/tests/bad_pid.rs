//! Out-of-range pid self-test on the SharedMemoryShare path (#177), run from
//! the ipc-timeout thread.

use crate::arch::aarch64::trap::TrapFrame;
use crate::ipc::shmem;
use crate::mm::pgtable::VmFlags;
use crate::syscall::IpcError;
use crate::task::process::{process_mut, ProcessId, MAX_PROCESSES, PROCESS_TABLE};
use shared::Capability;

/// Out-of-range pid on the SharedMemoryShare path → EINVAL, not an
/// index-out-of-bounds panic on PROCESS_TABLE (#177).
///
/// Runs in the ipc-timeout thread (process 1), which creates and owns the
/// region it shares. The share calls go through `syscall_dispatch`, the path
/// an EL0 caller takes, so they also cover the register decoding (#178).
///
/// The test leaves no capability behind. Process 1 holds SharedMemoryCreate
/// only around the create call. The region's SharedMemoryAccess tokens (the
/// creator grant and the self-share) are revoked before the unmap that frees
/// the region, because region ids are reused.
pub(super) fn shm_bad_pid_test() {
    let pid = match crate::cap::current_process_id() {
        Some(p) => p,
        None => {
            crate::kwarn!(Ipc, "Bad-pid test: no current process");
            return;
        }
    };
    let flags = VmFlags::READ | VmFlags::WRITE;

    let created = match crate::cap::grant_to_process(pid, Capability::SharedMemoryCreate, false) {
        Ok(_) => shmem::shared_memory_create(pid, 4096, flags),
        Err(e) => Err(e),
    };
    revoke_all(pid, Capability::SharedMemoryCreate);
    let region = match created {
        Ok(r) => r,
        Err(e) => {
            crate::kwarn!(Ipc, "Bad-pid test: shm_create failed {}", e);
            return;
        }
    };

    // SharedMemoryShare: x8 = syscall number, x0 = region, x1 = target pid.
    let share = |target_pid: u64| -> i64 {
        let mut tf = TrapFrame {
            x: [0; 31],
            sp_el0: 0,
            elr_el1: 0,
            spsr_el1: 0,
        };
        tf.x[8] = shared::Syscall::SharedMemoryShare as u64;
        tf.x[0] = region.0 as u64;
        tf.x[1] = target_pid;
        crate::syscall::syscall_dispatch(&mut tf);
        tf.x[0] as i64
    };

    let einval = IpcError::Einval as i64;
    let at_max = share(MAX_PROCESSES as u64);
    let at_u32_max = share(u32::MAX as u64);
    // Truncated to pid 0 before #178.
    let past_u32 = share(1 << 32);
    // The last valid pid reaches the table; no process uses that slot → EPERM.
    let last_slot = share((MAX_PROCESSES - 1) as u64);
    // A live in-range pid (the caller itself) → success.
    let live = share(pid.0 as u64);
    // The original panic site, called directly.
    let grant = match crate::cap::grant_to_process(
        ProcessId(MAX_PROCESSES as u32),
        Capability::SharedMemoryAccess(region.0),
        false,
    ) {
        Ok(_) => 0,
        Err(e) => e,
    };

    // Free the region: it is released when its only mapping goes away.
    // shared_memory_map needs SharedMemoryAccess and shared_memory_unmap does
    // not, so revoke in between: no token outlives the region.
    let mapped = shmem::shared_memory_map(pid, region, flags).is_ok();
    revoke_all(pid, Capability::SharedMemoryAccess(region.0));
    if mapped {
        let _ = shmem::shared_memory_unmap(pid, region);
    }

    // Log messages are cut at 48 bytes, so keep them short.
    if at_max == einval
        && at_u32_max == einval
        && past_u32 == einval
        && last_slot == IpcError::Eperm as i64
        && live == 0
        && grant == einval
    {
        crate::kinfo!(Ipc, "Bad-pid test: EINVAL as expected");
    } else {
        crate::kwarn!(
            Ipc,
            "Bad-pid: {} {} {} {} {} {}",
            at_max,
            at_u32_max,
            past_u32,
            last_slot,
            live,
            grant
        );
    }
}

/// Revoke every live token in `pid`'s capability table that grants exactly
/// `cap`. Process 1 holds no SharedMemoryCreate or SharedMemoryAccess tokens
/// besides the ones this test creates.
fn revoke_all(pid: ProcessId, cap: Capability) {
    let mut table = PROCESS_TABLE.lock();
    if let Ok(proc) = process_mut(&mut table, pid) {
        while let Some(token_id) = proc
            .cap_table
            .tokens()
            .iter()
            .flatten()
            .find(|t| t.capability == cap && !t.revoked)
            .map(|t| t.id)
        {
            proc.cap_table.revoke(token_id);
        }
    }
}
