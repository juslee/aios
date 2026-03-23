//! Capability system — per-process capability tables and enforcement.
//!
//! Each process holds a `CapabilityTable` of up to 256 tokens. IPC operations
//! check capabilities before proceeding. Revocation cascades to child tokens
//! and destroys channels created under the revoked capability.
//!
//! Per security.md §2.2, §3.1–3.5, ipc.md §8.1–8.3.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::observability::metrics::METRICS;
use crate::syscall::IpcError;
use crate::task::process::{ProcessId, PROCESS_TABLE};
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
    let tid = { *CURRENT_THREAD[cpu].lock() }?;
    process_of_thread(tid)
}

/// Check that a process holds ChannelCreate capability.
/// Returns the authorizing token ID on success (for recording in Channel.creation_cap).
pub fn check_channel_create(pid: ProcessId) -> Result<CapabilityTokenId, i64> {
    let now = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);
    let table = PROCESS_TABLE.lock();
    let proc = match &table[pid.0 as usize] {
        Some(p) => p,
        None => {
            #[cfg(feature = "kernel-metrics")]
            METRICS.ipc_cap_denied.inc();
            return Err(IpcError::Eperm as i64);
        }
    };

    match proc
        .cap_table
        .find_authorizing_token(&Capability::ChannelCreate, now)
    {
        Some(token_id) => Ok(token_id),
        None => {
            #[cfg(feature = "kernel-metrics")]
            METRICS.ipc_cap_denied.inc();
            crate::kwarn!(Cap, "pid={}: denied ChannelCreate", pid.0);
            Err(IpcError::Eperm as i64)
        }
    }
}

/// Check that a process holds ChannelAccess(channel_id) capability.
pub fn check_channel_access(pid: ProcessId, channel: shared::ChannelId) -> Result<(), i64> {
    let now = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);
    let table = PROCESS_TABLE.lock();
    let proc = match &table[pid.0 as usize] {
        Some(p) => p,
        None => {
            #[cfg(feature = "kernel-metrics")]
            METRICS.ipc_cap_denied.inc();
            return Err(IpcError::Eperm as i64);
        }
    };

    if proc
        .cap_table
        .has_capability(&Capability::ChannelAccess(channel), now)
    {
        Ok(())
    } else {
        #[cfg(feature = "kernel-metrics")]
        METRICS.ipc_cap_denied.inc();
        crate::kwarn!(Cap, "pid={}: denied ChannelAccess({})", pid.0, channel.0);
        Err(IpcError::Eperm as i64)
    }
}

/// Check that a process holds SharedMemoryCreate capability.
pub fn check_shared_memory_create(pid: ProcessId) -> Result<CapabilityTokenId, i64> {
    let now = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);
    let table = PROCESS_TABLE.lock();
    let proc = match &table[pid.0 as usize] {
        Some(p) => p,
        None => {
            #[cfg(feature = "kernel-metrics")]
            METRICS.ipc_cap_denied.inc();
            return Err(IpcError::Eperm as i64);
        }
    };

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

/// Check that a process holds SharedMemoryAccess(region_id) capability.
pub fn check_shared_memory_access(pid: ProcessId, region_id: u32) -> Result<(), i64> {
    let now = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);
    let table = PROCESS_TABLE.lock();
    let proc = match &table[pid.0 as usize] {
        Some(p) => p,
        None => {
            #[cfg(feature = "kernel-metrics")]
            METRICS.ipc_cap_denied.inc();
            return Err(IpcError::Eperm as i64);
        }
    };

    if proc
        .cap_table
        .has_capability(&Capability::SharedMemoryAccess(region_id), now)
    {
        Ok(())
    } else {
        #[cfg(feature = "kernel-metrics")]
        METRICS.ipc_cap_denied.inc();
        crate::kwarn!(
            Cap,
            "pid={}: denied SharedMemoryAccess({})",
            pid.0,
            region_id
        );
        Err(IpcError::Eperm as i64)
    }
}

/// Grant a capability to a process. Returns the handle.
pub fn grant_to_process(
    pid: ProcessId,
    cap: Capability,
    delegatable: bool,
) -> Result<CapabilityHandle, i64> {
    let now = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);
    let mut table = PROCESS_TABLE.lock();
    let proc = match &mut table[pid.0 as usize] {
        Some(p) => p,
        None => return Err(IpcError::Eperm as i64),
    };

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

/// Revoke a capability token in a process and destroy channels created under it.
pub fn revoke_in_process(pid: ProcessId, token_id: CapabilityTokenId) {
    // First, revoke in the process's cap table.
    {
        let mut table = PROCESS_TABLE.lock();
        if let Some(proc) = &mut table[pid.0 as usize] {
            proc.cap_table.revoke(token_id);
        }
    }

    // Walk CHANNEL_TABLE and destroy channels created under this token.
    revoke_channels_for_cap(token_id);

    crate::kinfo!(Cap, "pid={}: revoked token {}", pid.0, token_id.0);
}

// ---------------------------------------------------------------------------
// Capability Kit trait implementation
// ---------------------------------------------------------------------------

extern crate alloc;
use alloc::vec::Vec;

use shared::kits::capability::{self as capability_kit, CapabilityError};

/// Kernel-side implementation of the Capability Kit's `CapabilityEnforcer` trait.
///
/// This is a zero-sized unit struct that delegates to the global `PROCESS_TABLE`.
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
        let proc = table[holder.0 as usize]
            .as_ref()
            .ok_or(CapabilityError::NotGranted { requested: *action })?;

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

    fn grant(
        &mut self,
        holder: ProcessId,
        cap: Capability,
        granted_by: ProcessId,
    ) -> Result<CapabilityHandle, CapabilityError> {
        let now = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);
        let mut table = PROCESS_TABLE.lock();
        let proc = table[holder.0 as usize]
            .as_mut()
            .ok_or(CapabilityError::TableFull)?;

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
        let proc = table[holder.0 as usize]
            .as_mut()
            .ok_or(CapabilityError::InvalidHandle { handle })?;

        // Look up token ID from handle, then revoke.
        let token_id = proc
            .cap_table
            .get(handle)
            .map(|t| t.id)
            .ok_or(CapabilityError::InvalidHandle { handle })?;

        proc.cap_table.revoke(token_id);
        // Must drop the PROCESS_TABLE lock before touching CHANNEL_TABLE
        // to respect lock ordering: PROCESS_TABLE > CHANNEL_TABLE.
        drop(table);

        // Cascade: destroy channels created under this capability token.
        revoke_channels_for_cap(token_id);
        Ok(())
    }

    fn attenuate(
        &mut self,
        holder: ProcessId,
        handle: CapabilityHandle,
        narrowed: Capability,
    ) -> Result<CapabilityHandle, CapabilityError> {
        let mut table = PROCESS_TABLE.lock();
        let proc = table[holder.0 as usize]
            .as_mut()
            .ok_or(CapabilityError::InvalidHandle { handle })?;

        proc.cap_table
            .attenuate(handle, narrowed, None, holder, new_token_id())
            .map_err(|_| CapabilityError::InvalidAttenuation {
                reason: "parent does not permit narrowing to requested capability",
            })
    }

    fn list_active(&self, holder: ProcessId) -> Vec<CapabilityToken> {
        let table = PROCESS_TABLE.lock();
        let proc = match &table[holder.0 as usize] {
            Some(p) => p,
            None => return Vec::new(),
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

/// Walk CHANNEL_TABLE and destroy channels whose creation_cap matches token_id.
/// Wakes blocked threads with EPIPE.
fn revoke_channels_for_cap(token_id: CapabilityTokenId) {
    let mut channels_to_destroy = [None; 128];
    let mut count = 0;

    {
        let table = crate::ipc::CHANNEL_TABLE.lock();
        for (i, slot) in table.iter().enumerate() {
            if let Some(ch) = slot {
                if ch.creation_cap == Some(token_id) && count < channels_to_destroy.len() {
                    channels_to_destroy[count] = Some(shared::ChannelId(i as u32));
                    count += 1;
                }
            }
        }
    }

    // Destroy outside the lock to avoid lock ordering issues.
    // Use unchecked destroy — this is a kernel-initiated teardown during
    // cascade revocation, so capability checks must be bypassed.
    for ch_id in channels_to_destroy[..count].iter().flatten() {
        let _ = crate::ipc::channel_destroy_unchecked(*ch_id);
    }
}
