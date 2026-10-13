//! Share self-test (capability-lifetime ADR §4), run from the ipc-timeout
//! thread: SharedMemoryShare delegates the creator's SharedMemoryAccess token,
//! needs a delegatable token for another process, and is idempotent; the Kit
//! `shmem_destroy` destroys a region only for its creator.

use crate::cap::revoke_in_process;
use crate::ipc::{shmem, KernelIpc};
use crate::mm::pgtable::VmFlags;
use crate::syscall::IpcError;
use crate::task::process::{process_ref, ProcessId, PROCESS_TABLE};
use crate::task::ThreadId;
use shared::kits::ipc::SharedMemoryOps;
use shared::{Capability, CapabilityToken};

use super::bad_pid::{creation_cap, grant_shm_create, holds_live, token_id};
use super::stale_id::revoke_all;
use super::TEST_PID;

/// The share target: process 2 ("pi-test"), a live process other than the
/// test's own.
const PEER: ProcessId = ProcessId(2);

/// Flags of the test's regions.
const SHM_FLAGS: VmFlags = VmFlags::READ.union(VmFlags::WRITE);

/// Run the three cases in the ipc-timeout thread `my_tid`, which must belong
/// to process 1.
///
/// On entry neither process 1 nor process 2 may hold a live
/// SharedMemoryCreate (otherwise it logs `holds SharedMemoryCreate` and
/// stops), so each temporary SharedMemoryCreate the cases grant is the one
/// its region records as its `creation_cap`, which they check.
///
/// - **Plain:** under a non-delegatable grant Tn, region Rn's minted token is
///   not delegatable: a share of Rn to process 2 is EPERM and grants nothing,
///   and a self-share is `Ok`. Revoking Tn destroys Rn (`shm_cascade: pid=1
///   destroyed 1 regions`).
/// - **Delegated:** under a delegatable grant Td, sharing region Rd to
///   process 2 twice is `Ok` both times and leaves process 2 exactly one live
///   SharedMemoryAccess(Rd), not delegatable, whose `parent_token` is process
///   1's minted token. `KernelIpc::shmem_destroy(Rd)` is `Ok` and destroys
///   Rd (`shm_destroy: region S.G`). Process 2's token and Td are then revoked
///   through `revoke_in_process`; Rd is gone, so neither cascade destroys
///   anything.
/// - **Foreign:** under a temporary grant T2 to process 2, region R2 is
///   created for process 2. `KernelIpc::shmem_destroy(R2)` from process 1,
///   which neither created R2 nor holds a token for it, fails and leaves R2
///   alive; revoking T2 through `revoke_in_process` destroys R2
///   (`shm_cascade: pid=2 destroyed 1 regions`).
///
/// Every token the cases grant, and every token a create or a share mints,
/// is revoked before the test returns. No call logs a capability denial:
/// share and destroy log none. No lock is held across any call. Logs
/// `Share test: delegation as expected`, or the bitmask of failed checks
/// (bit n is check n: plain 0-3, delegated 4-9, foreign 10-12).
pub(super) fn share_test(my_tid: ThreadId) {
    let pid = match crate::cap::process_of_thread(my_tid) {
        Some(p) if p == TEST_PID => p,
        other => {
            crate::kwarn!(Ipc, "Share test: wrong owner {:?}", other.map(|p| p.0));
            return;
        }
    };
    if holds_live(pid, &Capability::SharedMemoryCreate)
        || holds_live(PEER, &Capability::SharedMemoryCreate)
    {
        crate::kwarn!(Ipc, "Share test: holds SharedMemoryCreate");
        return;
    }

    let plain = plain_case(pid);
    let delegated = delegated_case(pid);
    let foreign = foreign_case();

    let failed = plain
        .iter()
        .chain(delegated.iter())
        .chain(foreign.iter())
        .enumerate()
        .filter(|(_, ok)| !**ok)
        .fold(0u32, |mask, (i, _)| mask | 1 << i);
    // A log message keeps at most 96 bytes (two ring entries); longer text
    // is cut and marked with `~`, so keep these lines short.
    if failed == 0 {
        crate::kinfo!(Ipc, "Share test: delegation as expected");
    } else {
        crate::kwarn!(Ipc, "Share test: failed {:#x}", failed);
    }
}

/// The plain case, checks 0-3: 0, Rn records Tn; 1, a share to process 2 is
/// EPERM and grants it nothing; 2, a self-share is `Ok`; 3, revoking Tn
/// destroys Rn. All false if the grant or the create fails.
fn plain_case(pid: ProcessId) -> [bool; 4] {
    let mut checks = [false; 4];
    let Some(tn) = grant_shm_create(pid, false) else {
        return checks;
    };
    let Ok((rn, _access)) = shmem::shared_memory_create(pid, 4096, SHM_FLAGS) else {
        let _ = revoke_in_process(pid, tn);
        return checks;
    };
    let access = Capability::SharedMemoryAccess(rn);
    checks[0] = creation_cap(rn) == Some(tn);
    checks[1] = shmem::shared_memory_share(pid, rn, PEER) == Err(IpcError::Eperm as i64)
        && !holds_live(PEER, &access);
    checks[2] = shmem::shared_memory_share(pid, rn, pid).is_ok();
    checks[3] = revoke_in_process(pid, tn).is_ok() && creation_cap(rn).is_none();
    checks
}

/// The delegated case, checks 4-9: 4, Rd records Td; 5 and 6, the two shares
/// to process 2 are `Ok`; 7, process 2 holds exactly one live
/// SharedMemoryAccess(Rd), a non-delegatable child of process 1's minted
/// token; 8, `KernelIpc::shmem_destroy(Rd)` is `Ok` and Rd is gone; 9, the
/// revokes of process 2's token and of Td succeed. All false if the grant or
/// the create fails.
fn delegated_case(pid: ProcessId) -> [bool; 6] {
    let mut checks = [false; 6];
    let Some(td) = grant_shm_create(pid, true) else {
        return checks;
    };
    let Ok((rd, minted)) = shmem::shared_memory_create(pid, 4096, SHM_FLAGS) else {
        let _ = revoke_in_process(pid, td);
        return checks;
    };
    let access = Capability::SharedMemoryAccess(rd);
    checks[0] = creation_cap(rd) == Some(td);
    checks[1] = shmem::shared_memory_share(pid, rd, PEER).is_ok();
    checks[2] = shmem::shared_memory_share(pid, rd, PEER).is_ok();
    checks[3] = match (live_tokens(PEER, &access), token_id(pid, minted)) {
        ((1, Some(child)), Some(parent)) => {
            child.parent_token == Some(parent) && !child.delegatable
        }
        _ => false,
    };
    checks[4] = KernelIpc.shmem_destroy(rd).is_ok() && creation_cap(rd).is_none();
    // Process 2's token is revoked through revoke_in_process in its own
    // table; Rd is already destroyed, so neither revoke cascades.
    revoke_all(PEER, access);
    checks[5] = live_tokens(PEER, &access).0 == 0 && revoke_in_process(pid, td).is_ok();
    checks
}

/// The foreign case, checks 10-12: 10, R2 records T2; 11, the destroy from
/// process 1 fails and R2 stays; 12, revoking T2 destroys R2. All false if
/// the grant or the create fails.
fn foreign_case() -> [bool; 3] {
    let mut checks = [false; 3];
    let Some(t2) = grant_shm_create(PEER, false) else {
        return checks;
    };
    let Ok((r2, _access)) = shmem::shared_memory_create(PEER, 4096, SHM_FLAGS) else {
        let _ = revoke_in_process(PEER, t2);
        return checks;
    };
    checks[0] = creation_cap(r2) == Some(t2);
    checks[1] = KernelIpc.shmem_destroy(r2).is_err() && creation_cap(r2) == Some(t2);
    checks[2] = revoke_in_process(PEER, t2).is_ok() && creation_cap(r2).is_none();
    checks
}

/// How many live, unexpired tokens for exactly `cap` `pid` holds, and a copy
/// of the first.
fn live_tokens(pid: ProcessId, cap: &Capability) -> (usize, Option<CapabilityToken>) {
    let now = crate::arch::aarch64::timer::TICK_COUNT.load(core::sync::atomic::Ordering::Relaxed);
    let table = PROCESS_TABLE.lock();
    let Ok(proc) = process_ref(&table, pid) else {
        return (0, None);
    };
    let mut live = proc
        .cap_table
        .tokens()
        .iter()
        .flatten()
        .filter(|t| !t.revoked && !t.is_expired(now) && t.capability == *cap);
    let first = live.next().cloned();
    (usize::from(first.is_some()) + live.count(), first)
}
