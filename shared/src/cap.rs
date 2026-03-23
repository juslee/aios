//! Capability types shared between kernel and user-space.
//!
//! Defines the Capability enum (Phase 3 subset), token/handle identifiers,
//! the `permits()` matching logic, and the CapabilityTable data structure.
//! Per security.md §2.2, §3.1–3.5.

use crate::ipc::ChannelId;
use crate::sched::ProcessId;
use crate::syscall::IpcError;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum capabilities per process (security.md §3.1).
pub const MAX_CAPS_PER_PROCESS: usize = 256;

// ---------------------------------------------------------------------------
// Identity types
// ---------------------------------------------------------------------------

/// Unique capability token identifier (monotonic, system-wide).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityTokenId(pub u64);

/// Index into a process's capability table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityHandle(pub u32);

// ---------------------------------------------------------------------------
// Capability enum — Phase 3 subset
// ---------------------------------------------------------------------------

/// What a capability grants access to.
///
/// Phase 3 subset: channel operations, shared memory (placeholder),
/// agent spawning, and debug output. Full set comes in later phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// Permission to create IPC channels.
    ChannelCreate,
    /// Permission to call/send/recv on a specific channel.
    ChannelAccess(ChannelId),
    /// Permission to create shared memory regions (placeholder for Phase 4+).
    SharedMemoryCreate,
    /// Permission to access a specific shared memory region (placeholder).
    SharedMemoryAccess(u32),
    /// Permission to spawn new agents/processes.
    SpawnAgent,
    /// Permission to use the DebugPrint syscall.
    DebugPrint,
}

impl Capability {
    /// Check if this capability grants access for the requested `action`.
    ///
    /// Exact-match for simple capabilities. For parameterized capabilities
    /// (ChannelAccess, SharedMemoryAccess), the resource ID must match.
    pub fn permits(&self, action: &Capability) -> bool {
        match (self, action) {
            (Capability::ChannelCreate, Capability::ChannelCreate) => true,
            (Capability::ChannelAccess(held), Capability::ChannelAccess(needed)) => {
                held.0 == needed.0
            }
            (Capability::SharedMemoryCreate, Capability::SharedMemoryCreate) => true,
            (Capability::SharedMemoryAccess(held), Capability::SharedMemoryAccess(needed)) => {
                held == needed
            }
            (Capability::SpawnAgent, Capability::SpawnAgent) => true,
            (Capability::DebugPrint, Capability::DebugPrint) => true,
            _ => false,
        }
    }

    /// Check if `self` is equal to or broader than `other` (for attenuation).
    ///
    /// Attenuation must be monotonically restrictive: a ChannelCreate cap
    /// can be attenuated to ChannelAccess(specific_id), but not vice versa.
    /// For Phase 3, only same-variant attenuation is supported (identity).
    pub fn can_attenuate_to(&self, other: &Capability) -> bool {
        match (self, other) {
            // ChannelCreate can be narrowed to ChannelAccess (broader → narrower).
            (Capability::ChannelCreate, Capability::ChannelAccess(_)) => true,
            // SharedMemoryCreate can be narrowed to SharedMemoryAccess.
            (Capability::SharedMemoryCreate, Capability::SharedMemoryAccess(_)) => true,
            // Same-variant is always valid (identity attenuation).
            _ => self.permits(other),
        }
    }
}

// ---------------------------------------------------------------------------
// Capability token
// ---------------------------------------------------------------------------

/// A capability token granting a specific permission to a process.
#[derive(Clone)]
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

impl Default for CapabilityTable {
    fn default() -> Self {
        Self::new()
    }
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
    ///
    /// `new_id` is the unique token ID for the child (caller provides it to
    /// decouple from the kernel's atomic ID generator).
    pub fn attenuate(
        &mut self,
        handle: CapabilityHandle,
        new_cap: Capability,
        new_expiry: Option<u64>,
        holder: ProcessId,
        new_id: CapabilityTokenId,
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
            id: new_id,
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

    /// Return the number of active (non-revoked) tokens.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// Read-only access to the token slots (for Kit trait implementations).
    pub fn tokens(&self) -> &[Option<CapabilityToken>; MAX_CAPS_PER_PROCESS] {
        &self.tokens
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Identity type tests ---

    #[test]
    fn token_id_copy_clone() {
        let id = CapabilityTokenId(42);
        let id2 = id;
        assert_eq!(id, id2);
    }

    #[test]
    fn token_id_equality() {
        assert_eq!(CapabilityTokenId(1), CapabilityTokenId(1));
        assert_ne!(CapabilityTokenId(1), CapabilityTokenId(2));
    }

    #[test]
    fn handle_copy_clone() {
        let h = CapabilityHandle(7);
        let h2 = h;
        assert_eq!(h, h2);
    }

    #[test]
    fn handle_equality() {
        assert_eq!(CapabilityHandle(0), CapabilityHandle(0));
        assert_ne!(CapabilityHandle(0), CapabilityHandle(1));
    }

    #[test]
    fn max_caps_is_power_of_two() {
        assert!(MAX_CAPS_PER_PROCESS.is_power_of_two());
    }

    // --- Capability permits tests ---

    #[test]
    fn permits_channel_create() {
        assert!(Capability::ChannelCreate.permits(&Capability::ChannelCreate));
    }

    #[test]
    fn permits_channel_access_same() {
        let cap = Capability::ChannelAccess(ChannelId(5));
        assert!(cap.permits(&Capability::ChannelAccess(ChannelId(5))));
    }

    #[test]
    fn denies_channel_access_different() {
        let cap = Capability::ChannelAccess(ChannelId(5));
        assert!(!cap.permits(&Capability::ChannelAccess(ChannelId(6))));
    }

    #[test]
    fn denies_cross_variant() {
        assert!(!Capability::ChannelCreate.permits(&Capability::DebugPrint));
        assert!(!Capability::DebugPrint.permits(&Capability::ChannelCreate));
        assert!(!Capability::ChannelCreate.permits(&Capability::ChannelAccess(ChannelId(0))));
        assert!(!Capability::SpawnAgent.permits(&Capability::SharedMemoryCreate));
    }

    #[test]
    fn permits_shared_memory_create() {
        assert!(Capability::SharedMemoryCreate.permits(&Capability::SharedMemoryCreate));
    }

    #[test]
    fn permits_shared_memory_access_same() {
        let cap = Capability::SharedMemoryAccess(42);
        assert!(cap.permits(&Capability::SharedMemoryAccess(42)));
    }

    #[test]
    fn denies_shared_memory_access_different() {
        let cap = Capability::SharedMemoryAccess(42);
        assert!(!cap.permits(&Capability::SharedMemoryAccess(43)));
    }

    #[test]
    fn permits_spawn_agent() {
        assert!(Capability::SpawnAgent.permits(&Capability::SpawnAgent));
    }

    #[test]
    fn permits_debug_print() {
        assert!(Capability::DebugPrint.permits(&Capability::DebugPrint));
    }

    // --- Attenuation tests ---

    #[test]
    fn attenuate_channel_create_to_access() {
        assert!(
            Capability::ChannelCreate.can_attenuate_to(&Capability::ChannelAccess(ChannelId(5)))
        );
    }

    #[test]
    fn attenuate_shm_create_to_access() {
        assert!(Capability::SharedMemoryCreate.can_attenuate_to(&Capability::SharedMemoryAccess(1)));
    }

    #[test]
    fn attenuate_identity() {
        assert!(Capability::DebugPrint.can_attenuate_to(&Capability::DebugPrint));
        assert!(Capability::ChannelAccess(ChannelId(3))
            .can_attenuate_to(&Capability::ChannelAccess(ChannelId(3))));
    }

    #[test]
    fn attenuate_denies_broadening() {
        // Cannot broaden from access to create.
        assert!(
            !Capability::ChannelAccess(ChannelId(5)).can_attenuate_to(&Capability::ChannelCreate)
        );
        assert!(
            !Capability::SharedMemoryAccess(1).can_attenuate_to(&Capability::SharedMemoryCreate)
        );
    }

    #[test]
    fn attenuate_denies_cross_variant() {
        assert!(!Capability::ChannelCreate.can_attenuate_to(&Capability::DebugPrint));
        assert!(!Capability::SpawnAgent.can_attenuate_to(&Capability::ChannelCreate));
    }

    // --- Helper: create a test token ---

    fn make_token(id: u64, cap: Capability, holder: u32) -> CapabilityToken {
        CapabilityToken {
            id: CapabilityTokenId(id),
            capability: cap,
            holder: ProcessId(holder),
            delegatable: false,
            revoked: false,
            parent_token: None,
            usage_count: 0,
            created_at_tick: 0,
            expires_at_tick: None,
        }
    }

    fn make_child_token(id: u64, cap: Capability, holder: u32, parent: u64) -> CapabilityToken {
        CapabilityToken {
            id: CapabilityTokenId(id),
            capability: cap,
            holder: ProcessId(holder),
            delegatable: false,
            revoked: false,
            parent_token: Some(CapabilityTokenId(parent)),
            usage_count: 0,
            created_at_tick: 0,
            expires_at_tick: None,
        }
    }

    fn make_expiring_token(id: u64, cap: Capability, expires: u64) -> CapabilityToken {
        CapabilityToken {
            id: CapabilityTokenId(id),
            capability: cap,
            holder: ProcessId(1),
            delegatable: false,
            revoked: false,
            parent_token: None,
            usage_count: 0,
            created_at_tick: 0,
            expires_at_tick: Some(expires),
        }
    }

    // --- CapabilityTable: new ---

    #[test]
    fn table_new_is_empty() {
        let table = CapabilityTable::new();
        assert_eq!(table.count(), 0);
    }

    // --- CapabilityTable: grant ---

    #[test]
    fn grant_returns_handle_zero() {
        let mut table = CapabilityTable::new();
        let handle = table
            .grant(make_token(1, Capability::DebugPrint, 1))
            .unwrap();
        assert_eq!(handle.0, 0);
        assert_eq!(table.count(), 1);
    }

    #[test]
    fn grant_successive_handles() {
        let mut table = CapabilityTable::new();
        let h0 = table
            .grant(make_token(1, Capability::DebugPrint, 1))
            .unwrap();
        let h1 = table
            .grant(make_token(2, Capability::SpawnAgent, 1))
            .unwrap();
        assert_eq!(h0.0, 0);
        assert_eq!(h1.0, 1);
        assert_eq!(table.count(), 2);
    }

    // --- CapabilityTable: get ---

    #[test]
    fn get_valid_handle() {
        let mut table = CapabilityTable::new();
        let h = table
            .grant(make_token(42, Capability::DebugPrint, 1))
            .unwrap();
        let token = table.get(h).unwrap();
        assert_eq!(token.id, CapabilityTokenId(42));
        assert_eq!(token.capability, Capability::DebugPrint);
    }

    #[test]
    fn get_out_of_bounds() {
        let table = CapabilityTable::new();
        assert!(table
            .get(CapabilityHandle(MAX_CAPS_PER_PROCESS as u32))
            .is_none());
        assert!(table.get(CapabilityHandle(u32::MAX)).is_none());
    }

    #[test]
    fn get_empty_slot() {
        let table = CapabilityTable::new();
        assert!(table.get(CapabilityHandle(0)).is_none());
    }

    #[test]
    fn get_revoked_returns_none() {
        let mut table = CapabilityTable::new();
        let h = table
            .grant(make_token(1, Capability::DebugPrint, 1))
            .unwrap();
        table.revoke(CapabilityTokenId(1));
        assert!(table.get(h).is_none());
    }

    // --- CapabilityTable: has_capability ---

    #[test]
    fn has_capability_found() {
        let mut table = CapabilityTable::new();
        table
            .grant(make_token(1, Capability::ChannelCreate, 1))
            .unwrap();
        assert!(table.has_capability(&Capability::ChannelCreate, 0));
    }

    #[test]
    fn has_capability_not_found() {
        let mut table = CapabilityTable::new();
        table
            .grant(make_token(1, Capability::ChannelCreate, 1))
            .unwrap();
        assert!(!table.has_capability(&Capability::DebugPrint, 0));
    }

    #[test]
    fn has_capability_skips_revoked() {
        let mut table = CapabilityTable::new();
        table
            .grant(make_token(1, Capability::DebugPrint, 1))
            .unwrap();
        table.revoke(CapabilityTokenId(1));
        assert!(!table.has_capability(&Capability::DebugPrint, 0));
    }

    // --- CapabilityTable: find_authorizing_token with expiry ---

    #[test]
    fn find_authorizing_unexpired() {
        let mut table = CapabilityTable::new();
        table
            .grant(make_expiring_token(1, Capability::DebugPrint, 100))
            .unwrap();
        // At tick 50, not yet expired.
        assert_eq!(
            table.find_authorizing_token(&Capability::DebugPrint, 50),
            Some(CapabilityTokenId(1))
        );
    }

    #[test]
    fn find_authorizing_expired_exact() {
        let mut table = CapabilityTable::new();
        table
            .grant(make_expiring_token(1, Capability::DebugPrint, 100))
            .unwrap();
        // At tick 100, expired (>= expires_at).
        assert!(table
            .find_authorizing_token(&Capability::DebugPrint, 100)
            .is_none());
    }

    #[test]
    fn find_authorizing_expired_past() {
        let mut table = CapabilityTable::new();
        table
            .grant(make_expiring_token(1, Capability::DebugPrint, 100))
            .unwrap();
        assert!(table
            .find_authorizing_token(&Capability::DebugPrint, 200)
            .is_none());
    }

    #[test]
    fn find_authorizing_no_expiry_never_expires() {
        let mut table = CapabilityTable::new();
        table
            .grant(make_token(1, Capability::DebugPrint, 1))
            .unwrap();
        // Even at u64::MAX, token with no expiry is valid.
        assert_eq!(
            table.find_authorizing_token(&Capability::DebugPrint, u64::MAX),
            Some(CapabilityTokenId(1))
        );
    }

    #[test]
    fn find_authorizing_skips_expired_finds_valid() {
        let mut table = CapabilityTable::new();
        // First token: expired.
        table
            .grant(make_expiring_token(1, Capability::DebugPrint, 50))
            .unwrap();
        // Second token: not expired.
        table
            .grant(make_expiring_token(2, Capability::DebugPrint, 200))
            .unwrap();
        // At tick 100, first is expired, second is valid.
        assert_eq!(
            table.find_authorizing_token(&Capability::DebugPrint, 100),
            Some(CapabilityTokenId(2))
        );
    }

    // --- CapabilityTable: revoke ---

    #[test]
    fn revoke_single_token() {
        let mut table = CapabilityTable::new();
        table
            .grant(make_token(1, Capability::DebugPrint, 1))
            .unwrap();
        assert_eq!(table.count(), 1);
        table.revoke(CapabilityTokenId(1));
        assert_eq!(table.count(), 0);
        assert!(!table.has_capability(&Capability::DebugPrint, 0));
    }

    #[test]
    fn revoke_nonexistent_is_noop() {
        let mut table = CapabilityTable::new();
        table
            .grant(make_token(1, Capability::DebugPrint, 1))
            .unwrap();
        table.revoke(CapabilityTokenId(99)); // doesn't exist
        assert_eq!(table.count(), 1); // unchanged
    }

    #[test]
    fn revoke_cascade_parent_child() {
        let mut table = CapabilityTable::new();
        table
            .grant(make_token(1, Capability::ChannelCreate, 1))
            .unwrap();
        table
            .grant(make_child_token(
                2,
                Capability::ChannelAccess(ChannelId(5)),
                1,
                1,
            ))
            .unwrap();
        assert_eq!(table.count(), 2);

        // Revoking parent cascades to child.
        table.revoke(CapabilityTokenId(1));
        assert_eq!(table.count(), 0);
        assert!(!table.has_capability(&Capability::ChannelCreate, 0));
        assert!(!table.has_capability(&Capability::ChannelAccess(ChannelId(5)), 0));
    }

    #[test]
    fn revoke_cascade_transitive() {
        let mut table = CapabilityTable::new();
        // Grandparent → Parent → Child
        table
            .grant(make_token(1, Capability::ChannelCreate, 1))
            .unwrap();
        table
            .grant(make_child_token(
                2,
                Capability::ChannelAccess(ChannelId(5)),
                1,
                1,
            ))
            .unwrap();
        table
            .grant(make_child_token(
                3,
                Capability::ChannelAccess(ChannelId(5)),
                1,
                2,
            ))
            .unwrap();
        assert_eq!(table.count(), 3);

        // Revoking grandparent cascades through entire chain.
        table.revoke(CapabilityTokenId(1));
        assert_eq!(table.count(), 0);
    }

    #[test]
    fn revoke_child_leaves_parent() {
        let mut table = CapabilityTable::new();
        table
            .grant(make_token(1, Capability::ChannelCreate, 1))
            .unwrap();
        table
            .grant(make_child_token(
                2,
                Capability::ChannelAccess(ChannelId(5)),
                1,
                1,
            ))
            .unwrap();

        // Revoking only the child leaves the parent intact.
        table.revoke(CapabilityTokenId(2));
        assert_eq!(table.count(), 1);
        assert!(table.has_capability(&Capability::ChannelCreate, 0));
    }

    #[test]
    fn revoke_already_revoked_is_noop() {
        let mut table = CapabilityTable::new();
        table
            .grant(make_token(1, Capability::DebugPrint, 1))
            .unwrap();
        table.revoke(CapabilityTokenId(1));
        assert_eq!(table.count(), 0);
        // Revoking again doesn't underflow count.
        table.revoke(CapabilityTokenId(1));
        assert_eq!(table.count(), 0);
    }

    // --- CapabilityTable: attenuate ---

    #[test]
    fn attenuate_channel_create_to_access_ok() {
        let mut table = CapabilityTable::new();
        let h = table
            .grant(make_token(1, Capability::ChannelCreate, 1))
            .unwrap();
        let child_h = table
            .attenuate(
                h,
                Capability::ChannelAccess(ChannelId(7)),
                None,
                ProcessId(1),
                CapabilityTokenId(2),
            )
            .unwrap();

        let child = table.get(child_h).unwrap();
        assert_eq!(child.id, CapabilityTokenId(2));
        assert_eq!(child.capability, Capability::ChannelAccess(ChannelId(7)));
        assert_eq!(child.parent_token, Some(CapabilityTokenId(1)));
        assert_eq!(table.count(), 2);
    }

    #[test]
    fn table_attenuate_denies_broadening() {
        let mut table = CapabilityTable::new();
        let h = table
            .grant(make_token(1, Capability::ChannelAccess(ChannelId(5)), 1))
            .unwrap();
        let result = table.attenuate(
            h,
            Capability::ChannelCreate,
            None,
            ProcessId(1),
            CapabilityTokenId(2),
        );
        assert_eq!(result, Err(IpcError::Eperm as i64));
    }

    #[test]
    fn attenuate_invalid_handle() {
        let mut table = CapabilityTable::new();
        let result = table.attenuate(
            CapabilityHandle(0),
            Capability::DebugPrint,
            None,
            ProcessId(1),
            CapabilityTokenId(2),
        );
        assert_eq!(result, Err(IpcError::Eperm as i64));
    }

    #[test]
    fn attenuate_with_expiry() {
        let mut table = CapabilityTable::new();
        let h = table
            .grant(make_token(1, Capability::ChannelCreate, 1))
            .unwrap();
        let child_h = table
            .attenuate(
                h,
                Capability::ChannelAccess(ChannelId(3)),
                Some(500),
                ProcessId(1),
                CapabilityTokenId(2),
            )
            .unwrap();

        let child = table.get(child_h).unwrap();
        assert_eq!(child.expires_at_tick, Some(500));
    }

    #[test]
    fn attenuate_revoked_parent_denied() {
        let mut table = CapabilityTable::new();
        let h = table
            .grant(make_token(1, Capability::ChannelCreate, 1))
            .unwrap();
        table.revoke(CapabilityTokenId(1));
        // get() returns None for revoked → attenuate returns Eperm.
        let result = table.attenuate(
            h,
            Capability::ChannelAccess(ChannelId(5)),
            None,
            ProcessId(1),
            CapabilityTokenId(2),
        );
        assert_eq!(result, Err(IpcError::Eperm as i64));
    }

    // --- CapabilityTable: list ---

    #[test]
    fn list_empty_table() {
        let table = CapabilityTable::new();
        let mut out = [CapabilityTokenId(0); 4];
        assert_eq!(table.list(&mut out, 4), 0);
    }

    #[test]
    fn list_returns_non_revoked() {
        let mut table = CapabilityTable::new();
        table
            .grant(make_token(1, Capability::DebugPrint, 1))
            .unwrap();
        table
            .grant(make_token(2, Capability::SpawnAgent, 1))
            .unwrap();
        table
            .grant(make_token(3, Capability::ChannelCreate, 1))
            .unwrap();
        table.revoke(CapabilityTokenId(2)); // revoke the middle one

        let mut out = [CapabilityTokenId(0); 4];
        let count = table.list(&mut out, 4);
        assert_eq!(count, 2);
        assert_eq!(out[0], CapabilityTokenId(1));
        assert_eq!(out[1], CapabilityTokenId(3));
    }

    #[test]
    fn list_respects_max() {
        let mut table = CapabilityTable::new();
        table
            .grant(make_token(1, Capability::DebugPrint, 1))
            .unwrap();
        table
            .grant(make_token(2, Capability::SpawnAgent, 1))
            .unwrap();
        table
            .grant(make_token(3, Capability::ChannelCreate, 1))
            .unwrap();

        let mut out = [CapabilityTokenId(0); 4];
        let count = table.list(&mut out, 1); // max=1
        assert_eq!(count, 1);
        assert_eq!(out[0], CapabilityTokenId(1));
    }

    #[test]
    fn list_respects_buffer_size() {
        let mut table = CapabilityTable::new();
        table
            .grant(make_token(1, Capability::DebugPrint, 1))
            .unwrap();
        table
            .grant(make_token(2, Capability::SpawnAgent, 1))
            .unwrap();
        table
            .grant(make_token(3, Capability::ChannelCreate, 1))
            .unwrap();

        let mut out = [CapabilityTokenId(0); 2]; // buffer smaller than max
        let count = table.list(&mut out, 10);
        assert_eq!(count, 2);
    }

    // --- CapabilityTable: count tracking ---

    #[test]
    fn count_tracks_grants_and_revokes() {
        let mut table = CapabilityTable::new();
        assert_eq!(table.count(), 0);

        table
            .grant(make_token(1, Capability::DebugPrint, 1))
            .unwrap();
        assert_eq!(table.count(), 1);

        table
            .grant(make_token(2, Capability::SpawnAgent, 1))
            .unwrap();
        assert_eq!(table.count(), 2);

        table.revoke(CapabilityTokenId(1));
        assert_eq!(table.count(), 1);

        table.revoke(CapabilityTokenId(2));
        assert_eq!(table.count(), 0);
    }

    // --- CapabilityTable: grant reuses revoked slots ---

    #[test]
    fn grant_reuses_revoked_slot() {
        let mut table = CapabilityTable::new();
        let h0 = table
            .grant(make_token(1, Capability::DebugPrint, 1))
            .unwrap();
        assert_eq!(h0.0, 0);

        // Revoke doesn't remove the slot (it marks revoked), so next grant
        // goes to slot 1. But if the slot is set to None, it would be reused.
        // Current impl: revoked tokens stay in their slot (not cleared).
        let h1 = table
            .grant(make_token(2, Capability::SpawnAgent, 1))
            .unwrap();
        assert_eq!(h1.0, 1);
    }
}
