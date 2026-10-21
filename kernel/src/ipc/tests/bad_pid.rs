//! Out-of-range pid self-test on the SharedMemoryShare path (#177), run from
//! the ipc-timeout thread.

use crate::arch::aarch64::trap::TrapFrame;
use crate::cap::revoke_in_process;
use crate::ipc::shmem;
use crate::mm::pgtable::VmFlags;
use crate::syscall::IpcError;
use crate::task::process::{process_ref, ProcessId, MAX_PROCESSES, PROCESS_TABLE};
use crate::task::ThreadId;
use shared::{Capability, CapabilityHandle, CapabilityTokenId, SharedMemoryId};

use super::TEST_PID;

/// Out-of-range pid on the SharedMemoryShare path → EINVAL, not an
/// index-out-of-bounds panic on PROCESS_TABLE (#177).
///
/// Runs in the ipc-timeout thread `my_tid`, which belongs to process 1 and
/// creates and owns the region it shares. The share calls go through
/// `syscall_dispatch`, the path an EL0 caller takes, so they also cover the
/// register decoding (#178).
///
/// The test changes no capability outside process 1 and leaves none behind.
/// It checks that `my_tid` belongs to process 1 before it grants or revokes
/// anything, and that process 1 holds no live SharedMemoryCreate on entry
/// (otherwise it logs `holds SharedMemoryCreate` and stops), so the
/// SharedMemoryCreate token T it grants is the one the region records as its
/// `creation_cap`, which it checks. T is delegatable, so the region's minted
/// SharedMemoryAccess is too: a share to another pid passes the token check,
/// and the last valid pid's EPERM comes from its empty process slot. The
/// self-share is idempotent and mints nothing; the minted token authorises
/// the map. Then T is revoked through `revoke_in_process`: revoking the
/// create token revokes the minted token, its child, and destroys the region
/// (its cascade logs `shm_cascade: pid=1 destroyed 1 regions`), so no token
/// or region outlives the test.
pub(super) fn shm_bad_pid_test(my_tid: ThreadId) {
    let pid = match crate::cap::process_of_thread(my_tid) {
        Some(p) if p == TEST_PID => p,
        other => {
            crate::kwarn!(Ipc, "Bad-pid test: wrong owner {:?}", other.map(|p| p.0));
            return;
        }
    };
    if holds_live(pid, &Capability::SharedMemoryCreate) {
        crate::kwarn!(Ipc, "Bad-pid test: holds SharedMemoryCreate");
        return;
    }
    let flags = VmFlags::READ | VmFlags::WRITE;

    let Some(create_token) = grant_shm_create(pid, true) else {
        crate::kwarn!(Ipc, "Bad-pid test: grant failed");
        return;
    };
    let region = match shmem::shared_memory_create(pid, 4096, flags) {
        Ok((r, _access)) => r,
        Err(e) => {
            let _ = revoke_in_process(pid, create_token);
            crate::kwarn!(Ipc, "Bad-pid test: shm_create failed {}", e);
            return;
        }
    };
    let recorded = creation_cap(region) == Some(create_token);

    // SharedMemoryShare: x8 = syscall number, x0 = region, x1 = target pid.
    let share = |target_pid: u64| -> i64 {
        let mut tf = TrapFrame {
            x: [0; 31],
            sp_el0: 0,
            elr_el1: 0,
            spsr_el1: 0,
        };
        tf.x[8] = shared::Syscall::SharedMemoryShare as u64;
        tf.x[0] = u64::from(region.raw());
        tf.x[1] = target_pid;
        crate::syscall::syscall_dispatch(&mut tf);
        tf.x[0] as i64
    };

    let einval = IpcError::Einval as i64;
    let at_max = share(MAX_PROCESSES as u64);
    let at_u32_max = share(u32::MAX as u64);
    // Bit 32 set above the last slot. Before #178 this truncated to pid 31,
    // which no process uses, so a regression shows as EPERM (not EINVAL)
    // without granting anything to a live process.
    let past_u32 = share((1 << 32) | (MAX_PROCESSES - 1) as u64);
    // The last valid pid reaches the table and passes the token check (the
    // minted token is delegatable); no process uses that slot → EPERM.
    let last_slot = share((MAX_PROCESSES - 1) as u64);
    // A live in-range pid (the caller itself, which holds the token) →
    // success, minting nothing.
    let live = share(pid.0 as u64);
    // The original panic site, called directly.
    let grant = match crate::cap::grant_to_process(
        ProcessId(MAX_PROCESSES as u32),
        Capability::SharedMemoryAccess(region),
        false,
    ) {
        Ok(_) => 0,
        Err(e) => e,
    };

    // Map the region, then revoke T. Revoking T revokes the minted
    // SharedMemoryAccess, its child, and destroys the region while it is
    // mapped: the cascade force-unmaps it and frees its frames, so the unmap
    // that follows finds it gone (EPIPE).
    let mapped = shmem::shared_memory_map(pid, region, flags).is_ok();
    let destroyed = revoke_in_process(pid, create_token).is_ok()
        && shmem::shared_memory_unmap(pid, region) == Err(IpcError::Epipe as i64);

    // A log message keeps at most 96 bytes (two ring entries); longer text
    // is cut and marked with `~`, so keep these lines short.
    if at_max == einval
        && at_u32_max == einval
        && past_u32 == einval
        && last_slot == IpcError::Eperm as i64
        && live == 0
        && grant == einval
        && mapped
        && recorded
        && destroyed
    {
        crate::kinfo!(Ipc, "Bad-pid test: EINVAL as expected");
    } else {
        crate::kwarn!(
            Ipc,
            "Bad-pid: {} {} {} {} {} {} {} {} {}",
            at_max,
            at_u32_max,
            past_u32,
            last_slot,
            live,
            grant,
            mapped,
            recorded,
            destroyed
        );
    }
}

/// Grant `pid` a parentless SharedMemoryCreate, delegatable or not, and
/// return its token id. The token is temporary: the shared memory
/// self-tests revoke it through `revoke_in_process` before they return.
pub(super) fn grant_shm_create(pid: ProcessId, delegatable: bool) -> Option<CapabilityTokenId> {
    crate::cap::grant_to_process(pid, Capability::SharedMemoryCreate, delegatable)
        .ok()
        .and_then(|handle| token_id(pid, handle))
}

/// Id of the live token behind `handle` in `pid`'s capability table.
pub(super) fn token_id(pid: ProcessId, handle: CapabilityHandle) -> Option<CapabilityTokenId> {
    let table = PROCESS_TABLE.lock();
    let proc = process_ref(&table, pid).ok()?;
    proc.cap_table.get(handle).map(|t| t.id)
}

/// Whether `pid` holds a live, unexpired token that authorises `cap`. The
/// shared memory tests check SharedMemoryCreate on entry: a create records
/// the first live match as its region's `creation_cap`, so a token left live
/// by an earlier test would be recorded instead of the test's own.
pub(super) fn holds_live(pid: ProcessId, cap: &Capability) -> bool {
    let now = crate::arch::aarch64::timer::TICK_COUNT.load(core::sync::atomic::Ordering::Relaxed);
    let table = PROCESS_TABLE.lock();
    process_ref(&table, pid).is_ok_and(|proc| proc.cap_table.has_capability(cap, now))
}

/// The `creation_cap` of the live region `region`, or `None` if it is gone.
pub(super) fn creation_cap(region: SharedMemoryId) -> Option<CapabilityTokenId> {
    let mut table = shmem::SHARED_REGION_TABLE.lock();
    shmem::region_mut(&mut table, region)
        .ok()
        .map(|r| r.creation_cap)
}
