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
pub use shared::{Capability, CapabilityHandle, CapabilityTokenId, MAX_CAPS_PER_PROCESS};

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
// Capability token
// ---------------------------------------------------------------------------

/// A capability token granting a specific permission to a process.
#[allow(dead_code)]
pub struct CapabilityToken {
    /// Unique token identifier.
    pub id: CapabilityTokenId,
    /// What this token grants.
    pub capability: Capability,
    /// Process that holds this token.
    pub holder: ProcessId,
    /// Whether this token can be delegated to other processes.
    pub delegatable: bool,
    /// Whether this token has been revoked.
    pub revoked: bool,
    /// Parent token (for cascade revocation).
    pub parent_token: Option<CapabilityTokenId>,
    /// Usage count (incremented on each access check hit).
    pub usage_count: u64,
    /// Tick when this token was created.
    pub created_at_tick: u64,
    /// Tick when this token expires (None = no expiry).
    pub expires_at_tick: Option<u64>,
}

// ---------------------------------------------------------------------------
// Capability table (per-process)
// ---------------------------------------------------------------------------

/// Per-process capability table. Fixed array of token slots.
pub struct CapabilityTable {
    tokens: [Option<CapabilityToken>; MAX_CAPS_PER_PROCESS],
    count: u32,
}

impl CapabilityTable {
    /// Create an empty capability table.
    pub const fn new() -> Self {
        Self {
            tokens: [const { None }; MAX_CAPS_PER_PROCESS],
            count: 0,
        }
    }

    /// Grant a token, returning its handle. Returns Enospc if table is full.
    pub fn grant(&mut self, token: CapabilityToken) -> Result<CapabilityHandle, i64> {
        for (i, slot) in self.tokens.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(token);
                self.count += 1;
                return Ok(CapabilityHandle(i as u32));
            }
        }
        Err(IpcError::Enospc as i64)
    }

    /// Look up a token by handle. Returns None if out of bounds, empty, or revoked.
    pub fn get(&self, handle: CapabilityHandle) -> Option<&CapabilityToken> {
        let idx = handle.0 as usize;
        if idx >= MAX_CAPS_PER_PROCESS {
            return None;
        }
        self.tokens[idx].as_ref().filter(|t| !t.revoked)
    }

    /// Check if this table holds a valid (non-revoked, non-expired) token
    /// that permits the given action.
    pub fn has_capability(&self, cap: &Capability, now_tick: u64) -> bool {
        self.find_authorizing_token(cap, now_tick).is_some()
    }

    /// Find the first valid token that authorizes the given capability.
    /// Returns the token's ID, or None if no matching token exists.
    pub fn find_authorizing_token(
        &self,
        cap: &Capability,
        now_tick: u64,
    ) -> Option<CapabilityTokenId> {
        for token in self.tokens.iter().flatten() {
            if token.revoked {
                continue;
            }
            if let Some(exp) = token.expires_at_tick {
                if now_tick >= exp {
                    continue;
                }
            }
            if token.capability.permits(cap) {
                return Some(token.id);
            }
        }
        None
    }

    /// Revoke a token by ID, cascading to all children.
    pub fn revoke(&mut self, token_id: CapabilityTokenId) {
        // Collect IDs to revoke (cascade).
        let mut to_revoke = [None; MAX_CAPS_PER_PROCESS];
        let mut count = 0;

        // Mark the primary token.
        for token in self.tokens.iter_mut().flatten() {
            if token.id == token_id && !token.revoked {
                token.revoked = true;
                to_revoke[count] = Some(token_id);
                count += 1;
                self.count = self.count.saturating_sub(1);
            }
        }

        // Cascade: revoke children (tokens whose parent_token matches any revoked ID).
        // Iterate until no new revocations (handles transitive chains).
        let mut changed = true;
        while changed {
            changed = false;
            for token in self.tokens.iter_mut().flatten() {
                if !token.revoked {
                    if let Some(parent) = token.parent_token {
                        if to_revoke[..count].contains(&Some(parent)) {
                            token.revoked = true;
                            if count < MAX_CAPS_PER_PROCESS {
                                to_revoke[count] = Some(token.id);
                                count += 1;
                            }
                            self.count = self.count.saturating_sub(1);
                            changed = true;
                        }
                    }
                }
            }
        }
    }

    /// Attenuate a capability: create a narrower child token from an existing one.
    pub fn attenuate(
        &mut self,
        handle: CapabilityHandle,
        new_cap: Capability,
        new_expiry: Option<u64>,
        holder: ProcessId,
    ) -> Result<CapabilityHandle, i64> {
        let parent = match self.get(handle) {
            Some(t) => t,
            None => return Err(IpcError::Eperm as i64),
        };

        if !parent.capability.can_attenuate_to(&new_cap) {
            return Err(IpcError::Eperm as i64);
        }

        let parent_id = parent.id;
        let created_at = parent.created_at_tick;

        let child = CapabilityToken {
            id: new_token_id(),
            capability: new_cap,
            holder,
            delegatable: false, // Attenuated tokens are not further delegatable by default.
            revoked: false,
            parent_token: Some(parent_id),
            usage_count: 0,
            created_at_tick: created_at,
            expires_at_tick: new_expiry,
        };

        self.grant(child)
    }

    /// List non-revoked token IDs into an output buffer. Returns count written.
    pub fn list(&self, out: &mut [CapabilityTokenId], max: usize) -> usize {
        let mut written = 0;
        for slot in self.tokens.iter() {
            if written >= max || written >= out.len() {
                break;
            }
            if let Some(token) = slot {
                if !token.revoked {
                    out[written] = token.id;
                    written += 1;
                }
            }
        }
        written
    }
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
