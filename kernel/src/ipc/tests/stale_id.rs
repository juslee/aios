//! Stale-id self-tests (#185), run from the ipc-timeout thread: an id whose
//! slot now holds a newer channel or region fails with EPIPE and never
//! reaches the new object. The Shm-cascade case: revoking a
//! SharedMemoryCreate token destroys the regions created under it, and a
//! kernel borrow keeps a destroyed region's frames, and its slot, until its
//! release.

use crate::cap::revoke_in_process;
use crate::ipc::channel::{ipc_recv, ipc_reply, ipc_send};
use crate::ipc::select::ipc_select;
use crate::ipc::{channel_create, channel_destroy, channel_destroy_unchecked, shmem};
use crate::mm::pgtable::VmFlags;
use crate::syscall::IpcError;
use crate::task::process::{process_ref, ProcessId, PROCESS_TABLE};
use crate::task::ThreadId;
use shared::{
    Capability, CapabilityHandle, CapabilityTokenId, ChannelId, SelectEntry, SelectKind,
    SharedMemoryId,
};

use super::bad_pid::{creation_cap, holds_live, token_id};
use super::TEST_PID;

/// Creates of the second object (B, R2) a case tries before it gives up on
/// getting the freed slot back.
const ATTEMPTS: usize = 8;

/// Run the channel case, the shared memory case and the Shm-cascade case in
/// the ipc-timeout thread `my_tid`, which must belong to process 1: each
/// grants and revokes capabilities there.
///
/// Allocation is lowest-free-slot first, so a free followed by a create in
/// one thread usually gets the same slot back, at the next generation. On SMP
/// another thread can take the slot in between (the bench creates channels),
/// so each case checks the reuse and retries up to `ATTEMPTS` creates in all;
/// if none lands in the freed slot it logs `slot moved` instead of its result
/// line.
///
/// The cases leave no capability behind: every token they grant, and every
/// access token a create mints, is revoked through `revoke_in_process`. None
/// logs a capability denial: each stale call is made while the stale token is
/// still held, so it passes the capability check and reaches the identity
/// check.
pub(super) fn stale_id_test(my_tid: ThreadId) {
    let pid = match crate::cap::process_of_thread(my_tid) {
        Some(p) if p == TEST_PID => p,
        other => {
            crate::kwarn!(Ipc, "Stale-id test: wrong owner {:?}", other.map(|p| p.0));
            return;
        }
    };
    stale_channel_case(pid, my_tid);
    stale_shm_case(pid);
    shm_cascade_case(pid);
}

/// Channel case, run by thread `tid` of process `pid`. Checked creates under
/// process 1's ChannelCreate, each minting the creator's ChannelAccess: create
/// A, destroy it, create B in A's slot. With A's minted token still held,
/// `ipc_recv`, `ipc_send`, `ipc_reply`, `ipc_select` and `channel_destroy` on
/// A return EPIPE, and B, untouched, still answers `ipc_reply` with EPROTO
/// (no pending call).
///
/// A wrong-slot B is destroyed with its own minted token. If the checked
/// destroy of A fails, A is destroyed unchecked and the case reports check 0
/// failed without trying B. A create that fails, A's or a retry's, logs
/// `channel_create failed`.
fn stale_channel_case(pid: ProcessId, tid: ThreadId) {
    // Every channel created, A first: their minted tokens are revoked at the
    // end.
    let mut created = [None; ATTEMPTS + 1];
    let Ok((a, a_handle)) = channel_create(tid) else {
        crate::kwarn!(Ipc, "Stale-id test: channel_create failed");
        return;
    };
    created[0] = Some(a);
    if channel_destroy(a).is_err() {
        // A is still live, so no create can take its slot and the retry would
        // only log `slot moved`. Free A unchecked and report check 0 (the
        // destroy) failed; the bit after it is this cleanup.
        let cleaned_up = channel_destroy_unchecked(a).is_ok();
        revoke_minted(pid, &created);
        report("Stale-id", &[false], cleaned_up);
        return;
    }

    let mut wrong_slots_destroyed = true;
    let reused = create_in_slot(tid, a, &mut created[1..], &mut wrong_slots_destroyed);
    let (b, b_handle) = match reused {
        None => {
            crate::kwarn!(Ipc, "Stale-id test: channel_create failed");
            revoke_minted(pid, &created);
            return;
        }
        Some(None) => {
            crate::kwarn!(Ipc, "Stale-id test: slot moved");
            revoke_minted(pid, &created);
            return;
        }
        Some(Some(b)) => b,
    };

    let epipe = IpcError::Epipe as i64;
    let mut buf = [0u8; 16];
    let stale_select = [SelectEntry {
        kind: SelectKind::Channel(a),
    }];
    let checks = [
        // Check 0, the destroy of A, passed above.
        true,
        // Each create returned the handle of its own minted ChannelAccess.
        names_access(pid, a_handle, a) && names_access(pid, b_handle, b),
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

    let b_destroyed = channel_destroy(b).is_ok();
    revoke_minted(pid, &created);

    report("Stale-id", &checks, b_destroyed && wrong_slots_destroyed);
}

/// Create channels from thread `tid` until one lands in `a`'s slot, one per
/// entry of `created`, recording each there. A wrong-slot channel is
/// destroyed with its minted token, and `destroyed` is cleared if a destroy
/// fails. Returns the reused channel and its handle; `Some(None)` if no create
/// reused the slot, and `None` if a create failed.
fn create_in_slot(
    tid: ThreadId,
    a: ChannelId,
    created: &mut [Option<ChannelId>],
    destroyed: &mut bool,
) -> Option<Option<(ChannelId, CapabilityHandle)>> {
    for slot in created.iter_mut() {
        let (b, b_handle) = channel_create(tid).ok()?;
        *slot = Some(b);
        if b.index() == a.index() {
            return Some(Some((b, b_handle)));
        }
        *destroyed &= channel_destroy(b).is_ok();
    }
    Some(None)
}

/// Whether `handle` names a live `ChannelAccess(channel)` token in `pid`'s
/// capability table.
fn names_access(pid: ProcessId, handle: CapabilityHandle, channel: ChannelId) -> bool {
    let table = PROCESS_TABLE.lock();
    process_ref(&table, pid).is_ok_and(|proc| {
        proc.cap_table
            .get(handle)
            .is_some_and(|t| t.capability == Capability::ChannelAccess(channel))
    })
}

/// Revoke the minted ChannelAccess of every channel in `created`.
fn revoke_minted(pid: ProcessId, created: &[Option<ChannelId>]) {
    for &channel in created.iter().flatten() {
        revoke_all(pid, Capability::ChannelAccess(channel));
    }
}

/// Shared memory case. On entry process 1 must hold no live
/// SharedMemoryCreate, so the temporary grant T is the only one the creates
/// can use. Create R1, map it and unmap it, which frees it (its minted
/// token stays); create R2 in R1's slot (a wrong-slot R2 is freed the same
/// way, and a failure to free one fails the cleanup bit). With R1's token
/// still held, `shared_memory_map` and a self-share of R1 return EPIPE, and
/// `shared_memory_map` of R2 succeeds. R2 is then unmapped, which frees it.
/// A create that fails, R1's or a retry's, logs `shm_create failed`. Each
/// create mints its region's SharedMemoryAccess as a child of T, so revoking
/// T at the end revokes them too.
fn stale_shm_case(pid: ProcessId) {
    if holds_live(pid, &Capability::SharedMemoryCreate) {
        crate::kwarn!(Ipc, "Stale-shm test: holds SharedMemoryCreate");
        return;
    }
    if crate::cap::grant_to_process(pid, Capability::SharedMemoryCreate, false).is_err() {
        crate::kwarn!(Ipc, "Stale-shm test: grant failed");
        return;
    }

    let r1 = shmem::shared_memory_create(pid, 4096, SHM_FLAGS).map(|(r, _access)| r);
    // None if any create failed, R1's or a retry's; Some(None) if no retry
    // reused R1's slot.
    let checks = r1.ok().and_then(|r1| {
        let r1_freed = map_and_unmap(pid, r1);

        let mut r2 = None;
        // Whether every wrong-slot R2 was freed: part of the cleanup bit.
        let mut wrong_slots_freed = true;
        for _ in 0..ATTEMPTS {
            let (region, _access) = shmem::shared_memory_create(pid, 4096, SHM_FLAGS).ok()?;
            if region.index() == r1.index() {
                r2 = Some(region);
                break;
            }
            wrong_slots_freed &= map_and_unmap(pid, region);
        }

        Some(r2.map(|r2| {
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
            (checks, r2_freed && wrong_slots_freed)
        }))
    });

    // Revoking T also revokes every region's minted SharedMemoryAccess, its
    // child.
    revoke_all(pid, Capability::SharedMemoryCreate);

    match checks {
        None => crate::kwarn!(Ipc, "Stale-shm test: shm_create failed"),
        Some(None) => crate::kwarn!(Ipc, "Stale-shm test: slot moved"),
        Some(Some((checks, cleaned_up))) => report("Stale-shm", &checks, cleaned_up),
    }
}

/// Shm-cascade case (capability-lifetime ADR §5). On entry process 1 must
/// hold no live SharedMemoryCreate (otherwise it logs `holds
/// SharedMemoryCreate` and stops). Grant a temporary SharedMemoryCreate T,
/// create R, map it and borrow it as `b`. `revoke_in_process(T)` destroys R
/// while it is mapped and borrowed: R's frames drain (`shm: region S.G
/// draining (1 borrows)`), the unmap of R returns EPIPE and `b.is_live()` is
/// false. Grant T2 and create R2, which cannot land in R's draining slot, on
/// SMP too, because no allocator takes a draining slot. Releasing `b` frees
/// R's frames (`shm: region S.G drained`), and revoking T2 destroys the
/// never-mapped R2. Each of the two revokes logs `shm_cascade: pid=1
/// destroyed 1 regions`.
///
/// Checks (bit n of the failure mask is check n): 0, R records T as its
/// `creation_cap`; 1, R mapped; 2, T revoked; 3, unmap of R is EPIPE; 4, `b`
/// no longer live; 5, R2 records T2; 6, R2 is in another slot. The cleanup
/// bit is T2's revoke. No lock is held across any call.
fn shm_cascade_case(pid: ProcessId) {
    if holds_live(pid, &Capability::SharedMemoryCreate) {
        crate::kwarn!(Ipc, "Shm-cascade test: holds SharedMemoryCreate");
        return;
    }
    let Some(t) = grant_shm_create(pid) else {
        crate::kwarn!(Ipc, "Shm-cascade test: grant failed");
        return;
    };
    let r = match shmem::shared_memory_create(pid, 4096, SHM_FLAGS) {
        Ok((r, _access)) => r,
        Err(_) => {
            let _ = revoke_in_process(pid, t);
            crate::kwarn!(Ipc, "Shm-cascade test: shm_create failed");
            return;
        }
    };
    let r_recorded = creation_cap(r) == Some(t);
    let mapped = shmem::shared_memory_map(pid, r, SHM_FLAGS).is_ok();
    let b = match shmem::region_borrow(pid, r) {
        Ok(b) => b,
        Err(e) => {
            // Revoking T destroys R, mapped or not.
            let _ = revoke_in_process(pid, t);
            crate::kwarn!(Ipc, "Shm-cascade test: borrow failed {}", e);
            return;
        }
    };

    let revoked = revoke_in_process(pid, t).is_ok();
    let unmap_epipe = shmem::shared_memory_unmap(pid, r) == Err(IpcError::Epipe as i64);
    let gone = !b.is_live();

    let t2 = grant_shm_create(pid);
    let r2 = t2
        .and_then(|_| shmem::shared_memory_create(pid, 4096, SHM_FLAGS).ok())
        .map(|(r2, _access)| r2);
    let r2_recorded = r2.is_some_and(|r2| creation_cap(r2) == t2);
    let other_slot = r2.is_some_and(|r2| r2.index() != r.index());

    // The last borrow of R's draining frames: frees them.
    shmem::region_release(b);
    // Destroys R2, never mapped, with its minted SharedMemoryAccess.
    let cleaned_up = t2.is_some_and(|t2| revoke_in_process(pid, t2).is_ok());

    report(
        "Shm-cascade",
        &[
            r_recorded,
            mapped,
            revoked,
            unmap_epipe,
            gone,
            r2_recorded,
            other_slot,
        ],
        cleaned_up,
    );
}

/// Grant `pid` a parentless SharedMemoryCreate and return its token id.
fn grant_shm_create(pid: ProcessId) -> Option<CapabilityTokenId> {
    crate::cap::grant_to_process(pid, Capability::SharedMemoryCreate, false)
        .ok()
        .and_then(|handle| token_id(pid, handle))
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
