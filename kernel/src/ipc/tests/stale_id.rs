//! Stale-id self-tests (#185), run from the ipc-timeout thread: an id whose
//! slot now holds a newer channel or region fails with EPIPE and never
//! reaches the new object.

use crate::cap::revoke_in_process;
use crate::ipc::channel::{ipc_recv, ipc_reply, ipc_send};
use crate::ipc::select::ipc_select;
use crate::ipc::{channel_create_unchecked, channel_destroy, channel_destroy_unchecked, shmem};
use crate::mm::pgtable::VmFlags;
use crate::syscall::IpcError;
use crate::task::process::{process_ref, ProcessId, PROCESS_TABLE};
use crate::task::ThreadId;
use shared::{Capability, SelectEntry, SelectKind, SharedMemoryId};

use super::TEST_PID;

/// Creates of the second object (B, R2) a case tries before it gives up on
/// getting the freed slot back.
const ATTEMPTS: usize = 8;

/// Run the channel case and then the shared memory case in the ipc-timeout
/// thread `my_tid`, which must belong to process 1: both grant and revoke
/// capabilities there.
///
/// Allocation is lowest-free-slot first, so a free followed by a create in
/// one thread usually gets the same slot back, at the next generation. On SMP
/// another thread can take the slot in between (the bench creates channels),
/// so each case checks the reuse and retries up to `ATTEMPTS` creates in all;
/// if none lands in the freed slot it logs `slot moved` instead of its result
/// line.
///
/// Both cases leave no capability behind: every token they grant, and every
/// SharedMemoryAccess token a create grants, is revoked through
/// `revoke_in_process`, after the objects are gone. Neither logs a capability
/// denial: each stale call is made while the stale token is still held, so it
/// passes the capability check and reaches the identity check.
pub(super) fn stale_id_test(my_tid: ThreadId) {
    let pid = match crate::cap::process_of_thread(my_tid) {
        Some(p) if p == TEST_PID => p,
        other => {
            crate::kwarn!(Ipc, "Stale-id test: wrong owner {:?}", other.map(|p| p.0));
            return;
        }
    };
    stale_channel_case(pid);
    stale_shm_case(pid);
}

/// Channel case. Kernel channels from `channel_create_unchecked`, each made
/// accessible with a self-test `ChannelAccess` grant (ADR §11's interim
/// exception, as in `select_cap_test`): create A, destroy it, create B in
/// A's slot. With A's token still held, `ipc_recv`, `ipc_send`, `ipc_reply`,
/// `ipc_select` and `channel_destroy` on A return EPIPE, and B, untouched,
/// still answers `ipc_reply` with EPROTO (no pending call).
///
/// A wrong-slot B gets no grant: it is destroyed unchecked, like the other
/// kernel channels a self-test frees.
fn stale_channel_case(pid: ProcessId) {
    let a = channel_create_unchecked(pid);
    if crate::cap::grant_to_process(pid, Capability::ChannelAccess(a), false).is_err() {
        crate::kwarn!(Ipc, "Stale-id test: grant failed");
        let _ = channel_destroy_unchecked(a);
        return;
    }
    let destroyed = channel_destroy(a).is_ok();

    let reused = (0..ATTEMPTS).find_map(|_| {
        let b = channel_create_unchecked(pid);
        if b.index() == a.index() {
            Some(b)
        } else {
            let _ = channel_destroy_unchecked(b);
            None
        }
    });
    let Some(b) = reused else {
        crate::kwarn!(Ipc, "Stale-id test: slot moved");
        revoke_all(pid, Capability::ChannelAccess(a));
        return;
    };
    let granted = crate::cap::grant_to_process(pid, Capability::ChannelAccess(b), false).is_ok();

    let epipe = IpcError::Epipe as i64;
    let mut buf = [0u8; 16];
    let stale_select = [SelectEntry {
        kind: SelectKind::Channel(a),
    }];
    let checks = [
        destroyed,
        granted,
        b != a,
        ipc_recv(a, &mut buf, 0) == Err(epipe),
        ipc_send(a, b"STALE") == epipe,
        ipc_reply(a, b"STALE") == epipe,
        ipc_select(&stale_select, 0) == Err(epipe),
        channel_destroy(a) == Err(epipe),
        // After every stale call, B is still there and still empty.
        ipc_reply(b, b"FRESH") == IpcError::Eproto as i64,
        ipc_recv(b, &mut buf, 0) == Err(IpcError::Eagain as i64),
    ];

    let b_destroyed = if granted {
        channel_destroy(b).is_ok()
    } else {
        channel_destroy_unchecked(b).is_ok()
    };
    revoke_all(pid, Capability::ChannelAccess(a));
    revoke_all(pid, Capability::ChannelAccess(b));

    report("Stale-id", &checks, b_destroyed);
}

/// Shared memory case. On entry process 1 must hold no live
/// SharedMemoryCreate, so the temporary grant T is the only one the creates
/// can use. Create R1, map it and unmap it, which frees it (its creator
/// token stays); create R2 in R1's slot (a wrong-slot R2 is freed the same
/// way). With R1's token still held, `shared_memory_map` and a self-share of
/// R1 return EPIPE, and `shared_memory_map` of R2 succeeds. R2 is then
/// unmapped, which frees it.
fn stale_shm_case(pid: ProcessId) {
    if holds_live(pid, &Capability::SharedMemoryCreate) {
        crate::kwarn!(Ipc, "Stale-shm test: holds SharedMemoryCreate");
        return;
    }
    if crate::cap::grant_to_process(pid, Capability::SharedMemoryCreate, false).is_err() {
        crate::kwarn!(Ipc, "Stale-shm test: grant failed");
        return;
    }

    // Every region created, R1 first: their creator tokens are revoked at
    // the end.
    let mut created = [None; ATTEMPTS + 1];
    let r1 = shmem::shared_memory_create(pid, 4096, SHM_FLAGS);
    let checks = r1.ok().map(|r1| {
        created[0] = Some(r1);
        let r1_freed = map_and_unmap(pid, r1);

        let mut r2 = None;
        for slot in created[1..].iter_mut() {
            let Ok(region) = shmem::shared_memory_create(pid, 4096, SHM_FLAGS) else {
                break;
            };
            *slot = Some(region);
            if region.index() == r1.index() {
                r2 = Some(region);
                break;
            }
            map_and_unmap(pid, region);
        }

        r2.map(|r2| {
            let epipe = Err(IpcError::Epipe as i64);
            let mapped = shmem::shared_memory_map(pid, r2, SHM_FLAGS);
            let checks = [
                r1_freed,
                r2 != r1,
                shmem::shared_memory_map(pid, r1, SHM_FLAGS) == epipe,
                shmem::shared_memory_share(pid, r1, pid) == Err(IpcError::Epipe as i64),
                mapped.is_ok(),
            ];
            let r2_freed = mapped.is_ok() && shmem::shared_memory_unmap(pid, r2).is_ok();
            (checks, r2_freed)
        })
    });

    revoke_all(pid, Capability::SharedMemoryCreate);
    for region in created.into_iter().flatten() {
        revoke_all(pid, Capability::SharedMemoryAccess(region));
    }

    match checks {
        None => crate::kwarn!(Ipc, "Stale-shm test: shm_create failed"),
        Some(None) => crate::kwarn!(Ipc, "Stale-shm test: slot moved"),
        Some(Some((checks, r2_freed))) => report("Stale-shm", &checks, r2_freed),
    }
}

/// Flags of the shared memory case's regions and mappings.
const SHM_FLAGS: VmFlags = VmFlags::READ.union(VmFlags::WRITE);

/// Map `region` for `pid` and unmap it again: the unmap of its only mapping
/// frees it. Whether both succeeded.
fn map_and_unmap(pid: ProcessId, region: SharedMemoryId) -> bool {
    shmem::shared_memory_map(pid, region, SHM_FLAGS).is_ok()
        && shmem::shared_memory_unmap(pid, region).is_ok()
}

/// Log `<name> test: EPIPE as expected` when every check and the cleanup
/// passed, otherwise the bitmask of failed checks (bit n is check n, and the
/// bit past the last check is the cleanup). A log message keeps at most 96
/// bytes (two ring entries); longer text is cut and marked with `~`, so the
/// lines are short.
fn report(name: &str, checks: &[bool], cleaned_up: bool) {
    let failed = checks
        .iter()
        .chain(core::iter::once(&cleaned_up))
        .enumerate()
        .filter(|(_, ok)| !**ok)
        .fold(0u32, |mask, (i, _)| mask | 1 << i);
    if failed == 0 {
        crate::kinfo!(Ipc, "{} test: EPIPE as expected", name);
    } else {
        crate::kwarn!(Ipc, "{} test: failed {:#x}", name, failed);
    }
}

/// Whether `pid` holds a live token for `cap`.
fn holds_live(pid: ProcessId, cap: &Capability) -> bool {
    let now = crate::arch::aarch64::timer::TICK_COUNT.load(core::sync::atomic::Ordering::Relaxed);
    let table = PROCESS_TABLE.lock();
    process_ref(&table, pid).is_ok_and(|proc| proc.cap_table.has_capability(cap, now))
}

/// Revoke, through `revoke_in_process`, every unrevoked token for exactly
/// `cap` in `pid`'s capability table. Each revoke runs with no lock held:
/// `revoke_in_process` takes PROCESS_TABLE and then CHANNEL_TABLE itself.
fn revoke_all(pid: ProcessId, cap: Capability) {
    loop {
        let next = {
            let table = PROCESS_TABLE.lock();
            process_ref(&table, pid).ok().and_then(|proc| {
                proc.cap_table
                    .tokens()
                    .iter()
                    .flatten()
                    .find(|t| t.capability == cap && !t.revoked)
                    .map(|t| t.id)
            })
        };
        let Some(token_id) = next else {
            return;
        };
        if revoke_in_process(pid, token_id).is_err() {
            return;
        }
    }
}
