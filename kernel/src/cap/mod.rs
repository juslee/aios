//! Capability system — per-process capability tables and enforcement.
//!
//! Each process holds a `CapabilityTable` of up to 256 tokens. IPC operations
//! check capabilities before proceeding. A checked create mints the creator's
//! access token as a child of the authorising create token
//! (`create_with_access`). Revocation cascades to child tokens and destroys
//! the regions and channels the process created under a revoked token.
//!
//! Per model/layers.md §2.2, model/capabilities.md §3.1–3.5, ipc.md §8.1–8.3.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::observability::metrics::METRICS;
use crate::syscall::IpcError;
use crate::task::process::{process_mut, process_ref, ProcessId, ProcessTable, PROCESS_TABLE};
use crate::task::{ThreadId, CURRENT_THREAD, THREAD_TABLE};

// Re-export shared types for ergonomic kernel-side imports.
pub use shared::{
    Capability, CapabilityHandle, CapabilityTable, CapabilityToken, CapabilityTokenId,
};

// ---------------------------------------------------------------------------
// Token ID generator
// ---------------------------------------------------------------------------

/// Monotonically increasing token ID counter. Starts at 1 (0 = invalid).
static NEXT_TOKEN_ID: AtomicU64 = AtomicU64::new(1);

/// Allocate a new unique token ID.
pub fn new_token_id() -> CapabilityTokenId {
    CapabilityTokenId(NEXT_TOKEN_ID.fetch_add(1, Ordering::Relaxed))
}

// ---------------------------------------------------------------------------
// Enforcement API — convenience functions for IPC checks
// ---------------------------------------------------------------------------

/// Look up the process ID that owns a thread.
pub fn process_of_thread(tid: ThreadId) -> Option<ProcessId> {
    let table = THREAD_TABLE.lock();
    let idx = tid.0 as usize;
    if idx >= table.len() {
        return None;
    }
    table[idx].as_ref().and_then(|t| t.owner_pid)
}

/// Get the current thread's process ID.
pub fn current_process_id() -> Option<ProcessId> {
    let cpu = crate::arch::aarch64::exceptions::core_id() as usize;
    // Two statements, so that the guard is dropped before the `?` test. In
    // `{ *lock() }?` (edition 2021) the guard lives to the end of the
    // statement, and the test's branch can land inside the critical section.
    let current = *CURRENT_THREAD[cpu].lock();
    process_of_thread(current?)
}

/// Error for a `check_*` function or `create_with_access` whose process
/// lookup failed.
///
/// The pid they check is never a syscall argument. It is the calling
/// thread's `owner_pid` (via `current_process_id` or `process_of_thread`),
/// or the pid a kernel caller passes to `shared_memory_create` /
/// `shared_memory_map`. An out-of-range pid is therefore handled like a pid
/// with no process: a denial, EPERM, counted in `ipc_cap_denied`. Neither
/// case logs a warning.
fn deny_missing_process(_lookup_err: i64) -> i64 {
    #[cfg(feature = "kernel-metrics")]
    METRICS.ipc_cap_denied.inc();
    IpcError::Eperm as i64
}

/// Create an object under `pid`'s `create` capability and mint the creator's
/// access token for it, in one `PROCESS_TABLE` hold (capability-lifetime ADR
/// §2).
///
/// In order, under the hold:
/// 1. `pid`'s process must exist (EPERM otherwise, as `deny_missing_process`).
/// 2. A live token must authorise `create`; on a miss, log the usual
///    `denied <create>` warning and return EPERM.
/// 3. The process's table needs a free slot for the minted token, else
///    ENOSPC. Nothing has been published yet.
/// 4. `publish(auth)` puts the object in its table with `auth` as its
///    `creation_cap` and returns the object and the access capability for its
///    full id. It nests `CHANNEL_TABLE` or `SHARED_REGION_TABLE` under
///    `PROCESS_TABLE` (both allowed edges), never takes `PROCESS_TABLE`,
///    never wakes a thread and never frees memory: its error (a full object
///    table) is returned as it is, and the caller cleans up after the hold.
/// 5. The access capability is minted as a child of `auth`
///    (`CapabilityTable::mint_child`), so revoking `auth` revokes it.
///
/// A concurrent revoke of `auth` therefore runs wholly before step 2 or
/// wholly after step 5, and an object never outlives a revoke of the token it
/// records. Returns the object and the minted token's handle.
///
/// # Panics
///
/// If the mint fails after step 3 checked the slot and step 2 the parent:
/// both hold under the same `PROCESS_TABLE` hold, so a failure is a kernel
/// bug.
pub fn create_with_access<T>(
    pid: ProcessId,
    create: Capability,
    publish: impl FnOnce(CapabilityTokenId) -> Result<(T, Capability), i64>,
) -> Result<(T, CapabilityHandle), i64> {
    let now = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);
    let mut table = PROCESS_TABLE.lock();
    let proc = process_mut(&mut table, pid).map_err(deny_missing_process)?;

    let Some(auth) = proc.cap_table.find_authorizing_token(&create, now) else {
        #[cfg(feature = "kernel-metrics")]
        METRICS.ipc_cap_denied.inc();
        crate::kwarn!(Cap, "pid={}: denied {:?}", pid.0, create);
        return Err(IpcError::Eperm as i64);
    };
    if proc.cap_table.free_slots() == 0 {
        return Err(IpcError::Enospc as i64);
    }

    let (object, access) = publish(auth)?;
    let handle = proc
        .cap_table
        .mint_child(auth, access, pid, new_token_id(), now)
        .expect("create_with_access: mint failed under the hold");
    Ok((object, handle))
}

/// Check that a process holds ChannelAccess(channel_id) capability.
///
/// Returns `Err(EINVAL)` for a channel id whose slot is `>= MAX_CHANNELS`
/// (`index()` is `None`) before touching any table, at any generation. No
/// channel can exist at such an id, and callers go on to look
/// the id up in CHANNEL_TABLE.
pub fn check_channel_access(pid: ProcessId, channel: shared::ChannelId) -> Result<(), i64> {
    if channel.index().is_none() {
        crate::observability::tripwire::bump(
            shared::tripwire::Key::Badchan,
            shared::tripwire::BadchanSite::Cap.index(),
        );
        return Err(IpcError::Einval as i64);
    }

    let now = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);
    let table = PROCESS_TABLE.lock();
    let proc = process_ref(&table, pid).map_err(deny_missing_process)?;

    if proc
        .cap_table
        .has_capability(&Capability::ChannelAccess(channel), now)
    {
        Ok(())
    } else {
        #[cfg(feature = "kernel-metrics")]
        METRICS.ipc_cap_denied.inc();
        crate::kwarn!(Cap, "pid={}: denied ChannelAccess({})", pid.0, channel);
        Err(IpcError::Eperm as i64)
    }
}

/// Check that a process holds SharedMemoryCreate capability.
pub fn check_shared_memory_create(pid: ProcessId) -> Result<CapabilityTokenId, i64> {
    let now = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);
    let table = PROCESS_TABLE.lock();
    let proc = process_ref(&table, pid).map_err(deny_missing_process)?;

    match proc
        .cap_table
        .find_authorizing_token(&Capability::SharedMemoryCreate, now)
    {
        Some(token_id) => Ok(token_id),
        None => {
            #[cfg(feature = "kernel-metrics")]
            METRICS.ipc_cap_denied.inc();
            crate::kwarn!(Cap, "pid={}: denied SharedMemoryCreate", pid.0);
            Err(IpcError::Eperm as i64)
        }
    }
}

/// Check that a process holds a live SharedMemoryAccess(region_id) and that
/// the region still exists, in one `PROCESS_TABLE → SHARED_REGION_TABLE`
/// hold (capability-lifetime ADR §4, the compositor's attach check).
///
/// In the ADR's failure order: EINVAL for a malformed region slot, before
/// any lock; EPERM when the process does not exist (as
/// `deny_missing_process`) or holds no live token for the full id, logging
/// the usual `denied SharedMemoryAccess` warning after the release; EPIPE
/// when the region is gone, draining or of another generation.
pub fn check_shared_memory_access(
    pid: ProcessId,
    region_id: shared::SharedMemoryId,
) -> Result<(), i64> {
    if region_id.index().is_none() {
        return Err(IpcError::Einval as i64);
    }
    let now = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);
    let held = {
        let table = PROCESS_TABLE.lock();
        let held = has_live_token(&table, pid, &Capability::SharedMemoryAccess(region_id), now)?;
        if held {
            let mut regions = crate::ipc::shmem::SHARED_REGION_TABLE.lock();
            crate::ipc::shmem::region_mut(&mut regions, region_id)?;
        }
        held
    };
    if !held {
        note_shm_access_denied(pid, region_id);
        return Err(IpcError::Eperm as i64);
    }
    Ok(())
}

/// Whether `pid` holds a live, unexpired token for `cap`, read from a locked
/// `PROCESS_TABLE`. Takes no lock and logs nothing, so it serves the
/// one-hold shared memory paths (map, share, destroy).
///
/// `Err(EPERM)` when `pid` names no process (out of range or an empty slot,
/// counted as `deny_missing_process` counts it); `Ok(false)` when it holds no
/// such token, which the caller reports.
pub(crate) fn has_live_token(
    table: &ProcessTable,
    pid: ProcessId,
    cap: &Capability,
    now: u64,
) -> Result<bool, i64> {
    let proc = process_ref(table, pid).map_err(deny_missing_process)?;
    Ok(proc.cap_table.has_capability(cap, now))
}

/// Count and log a `SharedMemoryAccess(region_id)` denial for `pid`: the
/// `pid=N: denied SharedMemoryAccess(S.G)` warning. Called with no lock held,
/// after a one-hold path found no token.
pub(crate) fn note_shm_access_denied(pid: ProcessId, region_id: shared::SharedMemoryId) {
    #[cfg(feature = "kernel-metrics")]
    METRICS.ipc_cap_denied.inc();
    crate::kwarn!(
        Cap,
        "pid={}: denied SharedMemoryAccess({})",
        pid.0,
        region_id
    );
}

/// The capability half of `SharedMemoryShare`, run on the caller's locked
/// `PROCESS_TABLE` with `SHARED_REGION_TABLE` nested (capability-lifetime ADR
/// §4): it takes no lock, frees, logs and wakes nothing.
///
/// Reads the caller's delegation parent, its live token for
/// `SharedMemoryAccess(region)` (`CapabilityTable::delegation_parent`), and
/// the target's liveness, holding and free slots from `table`, and asks the
/// pure `share_decision`, with `region_check` (the caller's region lookup
/// and creator check, made under the nested hold) as its step 3. On `Grant`
/// it puts the parent's `delegated_child` in the target's table: not
/// delegatable, with the caller's token as its `parent_token`. Every read and
/// the grant use the one guard, so a concurrent revoke or exit runs wholly
/// before or after the share.
///
/// Returns whether a token was granted (`false` for an idempotent share), or
/// the verdict's errno.
pub(crate) fn mint_share_token(
    table: &mut ProcessTable,
    caller: ProcessId,
    target: ProcessId,
    region: shared::SharedMemoryId,
    region_check: Result<(), i64>,
    now: u64,
) -> Result<bool, i64> {
    let access = Capability::SharedMemoryAccess(region);
    let caller_proc = process_ref(table, caller).ok();
    // A copy: the grant below needs the table mutably.
    let caller_token =
        caller_proc.and_then(|p| p.cap_table.delegation_parent(&access, now).cloned());
    let target_proc = process_ref(table, target).ok();
    let facts = shared::cap::ShareFacts {
        caller,
        target,
        caller_alive: caller_proc.is_some(),
        caller_token: caller_token.as_ref(),
        region: region_check,
        target_alive: target_proc.is_some(),
        target_holds: target_proc.is_some_and(|p| p.cap_table.has_capability(&access, now)),
        target_free_slots: target_proc.map_or(0, |p| p.cap_table.free_slots()),
    };
    match shared::cap::share_decision(&facts)? {
        shared::cap::ShareVerdict::AlreadyHeld => Ok(false),
        shared::cap::ShareVerdict::Grant { parent } => {
            let child = parent.delegated_child(target, new_token_id(), now);
            // The verdict found the target alive with a free slot in this
            // same hold, so neither the lookup nor the grant fails.
            process_mut(table, target)?.cap_table.grant(child)?;
            Ok(true)
        }
    }
}

/// Grant a capability to a process as a parentless token. Returns the handle.
///
/// Returns `Err(EINVAL)` for a pid `>= MAX_PROCESSES` and `Err(EPERM)` when
/// no such process exists.
///
/// **Rule** (capability-lifetime ADR §11): at runtime, an object-bound token
/// (`ChannelAccess`, `SharedMemoryAccess`, `GpuBufferAccess`) comes only from
/// `create_with_access`, which mints it for the creator, or from
/// `shared_memory_share`, which delegates it. Ambient grants of one through
/// this function are allowed only in init code before `enter_scheduler` and
/// in self-tests: an interim exception to owner decision 1 (#163, never by
/// ambient grant), with the kernel acting as the Service Manager until
/// `CapabilityTransfer` exists.
pub fn grant_to_process(
    pid: ProcessId,
    cap: Capability,
    delegatable: bool,
) -> Result<CapabilityHandle, i64> {
    let now = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);
    let mut table = PROCESS_TABLE.lock();
    let proc = process_mut(&mut table, pid)?;

    let token = CapabilityToken {
        id: new_token_id(),
        capability: cap,
        holder: pid,
        delegatable,
        revoked: false,
        parent_token: None,
        usage_count: 0,
        created_at_tick: now,
        expires_at_tick: None,
    };

    proc.cap_table.grant(token)
}

/// Revoke a capability token in a process, with its same-table descendants,
/// and destroy every region and channel the process created under a token
/// that is now revoked.
///
/// Returns `Err(EINVAL)` for a pid `>= MAX_PROCESSES` and `Err(EPERM)` for a
/// pid whose slot holds no process, before revoking or destroying anything.
///
/// Locks, in order: `PROCESS_TABLE`; under it, `SHARED_REGION_TABLE` nested
/// for the region cascade (`shmem::revoke_regions_for_cap`), released; then
/// `CHANNEL_TABLE` nested for the channel collection, released; then
/// `PROCESS_TABLE` released. With no lock held it then frees the detached
/// regions' frames (`shmem::finish_region_cascade`), destroys the collected
/// channels, because a destroy wakes the threads blocked on them, and logs.
pub fn revoke_in_process(pid: ProcessId, token_id: CapabilityTokenId) -> Result<(), i64> {
    let mut regions = crate::ipc::shmem::FreeList::new();
    let mut doomed = [None; crate::ipc::MAX_CHANNELS];
    let count = {
        let mut table = PROCESS_TABLE.lock();
        let proc = process_mut(&mut table, pid)?;
        proc.cap_table.revoke(token_id);
        crate::ipc::shmem::revoke_regions_for_cap(pid, &proc.cap_table, &mut regions);
        collect_revoked_channels(pid, &proc.cap_table, &mut doomed)
    };
    crate::ipc::shmem::finish_region_cascade(pid, regions);
    destroy_channels(&doomed[..count]);

    crate::kinfo!(Cap, "pid={}: revoked token {}", pid.0, token_id.0);
    Ok(())
}

/// The channel half of a revoke's cascade, run inside the revoking
/// `PROCESS_TABLE` hold: write into `out` the full id of every channel that
/// `pid` created (`creator == Some(pid)`) and whose `creation_cap` is revoked
/// in `caps`, `pid`'s capability table. Returns how many it wrote.
///
/// It reads the table, not one token id, so a revoke of an ancestor reaches
/// channels created under any of its descendants, and a `creation_cap` that
/// no slot holds counts as revoked (`is_revoked` fails closed). Kernel
/// channels (`creator: None`) never match. Takes `CHANNEL_TABLE` nested under
/// the caller's `PROCESS_TABLE` and releases it before returning.
fn collect_revoked_channels(
    pid: ProcessId,
    caps: &CapabilityTable,
    out: &mut [Option<shared::ChannelId>; crate::ipc::MAX_CHANNELS],
) -> usize {
    let table = crate::ipc::CHANNEL_TABLE.lock();
    let mut count = 0;
    let doomed = table
        .iter()
        .filter_map(|slot| slot.channel.as_ref())
        .filter(|ch| {
            ch.creator == Some(pid) && ch.creation_cap.is_some_and(|t| caps.is_revoked(t))
        });
    // At most one id per slot, so `out` (MAX_CHANNELS long) never fills.
    for (entry, ch) in out.iter_mut().zip(doomed) {
        *entry = Some(ch.id);
        count += 1;
    }
    count
}

/// Destroy the channels a cascade collected, with no lock held. Each destroy
/// wakes the channel's blocked threads with EPIPE. The destroy is unchecked:
/// this is a kernel-initiated teardown, and the full id makes a channel
/// destroyed since the collection an EPIPE no-op that never reaches the
/// channel now in its slot.
fn destroy_channels(ids: &[Option<shared::ChannelId>]) {
    for &id in ids.iter().flatten() {
        let _ = crate::ipc::channel_destroy_unchecked(id);
    }
}

// ---------------------------------------------------------------------------
// Capability Kit trait implementation
// ---------------------------------------------------------------------------

use alloc::vec::Vec;

use shared::kits::capability::{self as capability_kit, CapabilityError};

/// Kernel-side implementation of the Capability Kit's `CapabilityEnforcer` trait.
///
/// This is a zero-sized unit struct that delegates to the global `PROCESS_TABLE`.
/// A holder pid `>= MAX_PROCESSES` gets the same result as a pid with no
/// process (`CapabilityError` has no invalid-argument variant).
#[allow(dead_code)]
pub struct KernelCapabilitySystem;

impl capability_kit::CapabilityEnforcer for KernelCapabilitySystem {
    fn check(
        &self,
        holder: ProcessId,
        action: &Capability,
    ) -> Result<CapabilityHandle, CapabilityError> {
        let now = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);
        let table = PROCESS_TABLE.lock();
        let proc = process_ref(&table, holder)
            .map_err(|_| CapabilityError::NotGranted { requested: *action })?;

        // Find a token authorizing this action; convert token_id → handle.
        let token_id = proc
            .cap_table
            .find_authorizing_token(action, now)
            .ok_or(CapabilityError::NotGranted { requested: *action })?;

        // Find the handle (slot index) for this token ID.
        for (i, slot) in proc.cap_table.tokens().iter().enumerate() {
            if let Some(t) = slot {
                if t.id == token_id && !t.revoked {
                    return Ok(CapabilityHandle(i as u32));
                }
            }
        }

        Err(CapabilityError::NotGranted { requested: *action })
    }

    /// Grant `cap` to `holder` as a parentless, delegatable token.
    ///
    /// Refuses the object-bound capabilities (`ChannelAccess`,
    /// `SharedMemoryAccess`, `GpuBufferAccess`) with `NotGranted`, before any
    /// lock: a token for one object comes only from minting at creation or
    /// from delegation (capability-lifetime ADR §3), never from a Kit grant.
    fn grant(
        &mut self,
        holder: ProcessId,
        cap: Capability,
        granted_by: ProcessId,
    ) -> Result<CapabilityHandle, CapabilityError> {
        if matches!(
            cap,
            Capability::ChannelAccess(_)
                | Capability::SharedMemoryAccess(_)
                | Capability::GpuBufferAccess(_)
        ) {
            return Err(CapabilityError::NotGranted { requested: cap });
        }

        let now = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);
        let mut table = PROCESS_TABLE.lock();
        let proc = process_mut(&mut table, holder)
            .map_err(|_| CapabilityError::NotGranted { requested: cap })?;

        let token = CapabilityToken {
            id: new_token_id(),
            capability: cap,
            holder,
            delegatable: true,
            revoked: false,
            parent_token: None,
            usage_count: 0,
            created_at_tick: now,
            expires_at_tick: None,
        };

        // Suppress unused variable warning — granted_by is recorded in the
        // token's provenance in future phases but not yet used.
        let _ = granted_by;

        proc.cap_table
            .grant(token)
            .map_err(|_| CapabilityError::TableFull)
    }

    fn revoke(
        &mut self,
        holder: ProcessId,
        handle: CapabilityHandle,
    ) -> Result<(), CapabilityError> {
        let mut table = PROCESS_TABLE.lock();
        let proc = process_mut(&mut table, holder)
            .map_err(|_| CapabilityError::InvalidHandle { handle })?;

        // Look up token ID from handle by inspecting the underlying slot.
        // We use tokens() instead of get() to distinguish:
        //   - out-of-bounds/empty → InvalidHandle
        //   - present + revoked   → Revoked { token_id }
        //   - present + active    → proceed with revocation
        let token_id = {
            let tokens = proc.cap_table.tokens();
            let idx = handle.0 as usize;
            let slot = tokens
                .get(idx)
                .ok_or(CapabilityError::InvalidHandle { handle })?;
            let token = slot
                .as_ref()
                .ok_or(CapabilityError::InvalidHandle { handle })?;
            if token.revoked {
                return Err(CapabilityError::Revoked { token_id: token.id });
            }
            token.id
        };

        proc.cap_table.revoke(token_id);
        // Both cascades, in `revoke_in_process`'s lock sequence: detach the
        // regions and collect the channels under this hold, each table nested
        // and released in turn; free and destroy after it.
        let mut regions = crate::ipc::shmem::FreeList::new();
        crate::ipc::shmem::revoke_regions_for_cap(holder, &proc.cap_table, &mut regions);
        let mut doomed = [None; crate::ipc::MAX_CHANNELS];
        let count = collect_revoked_channels(holder, &proc.cap_table, &mut doomed);
        drop(table);
        crate::ipc::shmem::finish_region_cascade(holder, regions);
        destroy_channels(&doomed[..count]);
        Ok(())
    }

    /// Attenuate with no requested expiry: the child takes the parent's.
    ///
    /// Errors: `TableFull` for a full table (ENOSPC); `Expired`, naming the
    /// parent, when the live token at `handle` has expired; every other
    /// refusal (no live token at `handle`, a narrowing it does not permit) is
    /// `InvalidAttenuation`.
    fn attenuate(
        &mut self,
        holder: ProcessId,
        handle: CapabilityHandle,
        narrowed: Capability,
    ) -> Result<CapabilityHandle, CapabilityError> {
        let now = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);
        let mut table = PROCESS_TABLE.lock();
        let proc = process_mut(&mut table, holder)
            .map_err(|_| CapabilityError::InvalidHandle { handle })?;

        match proc
            .cap_table
            .attenuate(handle, narrowed, None, holder, new_token_id(), now)
        {
            Ok(child) => Ok(child),
            Err(code) if code == IpcError::Enospc as i64 => Err(CapabilityError::TableFull),
            Err(_) => Err(
                match proc.cap_table.get(handle).filter(|t| t.is_expired(now)) {
                    Some(parent) => CapabilityError::Expired {
                        token_id: parent.id,
                    },
                    None => CapabilityError::InvalidAttenuation {
                        reason: "parent does not permit narrowing to requested capability",
                    },
                },
            ),
        }
    }

    fn list_active(&self, holder: ProcessId) -> Vec<CapabilityToken> {
        let table = PROCESS_TABLE.lock();
        let proc = match process_ref(&table, holder) {
            Ok(p) => p,
            Err(_) => return Vec::new(),
        };

        let now = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);
        let mut result = Vec::new();
        for token in proc.cap_table.tokens().iter().flatten() {
            if !token.revoked {
                // Also filter expired tokens.
                if let Some(exp) = token.expires_at_tick {
                    if now >= exp {
                        continue;
                    }
                }
                result.push(token.clone());
            }
        }
        result
    }
}
