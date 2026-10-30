//! Capability types shared between kernel and user-space.
//!
//! Defines the Capability enum (Phase 3 subset), token/handle identifiers,
//! the `permits()` matching logic, and the CapabilityTable data structure.
//! Per model/layers.md §2.2, model/capabilities.md §3.1–3.5.

use crate::ipc::{ChannelId, SharedMemoryId};
use crate::sched::ProcessId;
use crate::syscall::IpcError;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum capabilities per process (model/capabilities.md §3.2).
pub const MAX_CAPS_PER_PROCESS: usize = 256;

/// Free slots of a process's capability table that a token another process
/// causes to be granted (a `SharedMemoryShare` to a foreign target) may not
/// use (capability-lifetime ADR §4). A share is refused with ENOSPC when the
/// target has this many free slots or fewer, so another process can never
/// fill a table and block its owner's own mints, which need a slot.
pub const CAP_FOREIGN_RESERVE: usize = 64;

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
/// Phases 3–6: channel operations, shared memory, agent spawning,
/// debug output, and GPU access. Full set comes in later phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// Permission to create IPC channels.
    ChannelCreate,
    /// Permission to call/send/recv/select on, cancel, or destroy a specific channel.
    ChannelAccess(ChannelId),
    /// Permission to create shared memory regions (placeholder for Phase 4+).
    SharedMemoryCreate,
    /// Permission to access a specific shared memory region (placeholder).
    SharedMemoryAccess(SharedMemoryId),
    /// Permission to spawn new agents/processes.
    SpawnAgent,
    /// Permission to use the DebugPrint syscall.
    DebugPrint,
    /// Permission to access GPU MMIO registers (GPU Service only).
    GpuMmioAccess,
    /// Permission to allocate GPU framebuffers.
    GpuBufferCreate,
    /// Permission to access a specific GPU buffer by resource ID.
    GpuBufferAccess(u32),
    /// Permission to configure display scanout (mode setting, page flip).
    DisplayControl,
    /// Permission to ask the compositor to create a surface (Phase 7 M24).
    CompositorCreateSurface,
    /// Permission to take fullscreen — bypasses windowed layout (Phase 7 M24).
    CompositorFullscreen,
    /// Permission to place a surface on the Overlay layer (Phase 7 M24).
    CompositorOverlay,
    /// Permission to receive raw input events from the compositor without focus
    /// (used by the system shell and accessibility tools, Phase 7 M24).
    CompositorInputAccess,
}

impl Capability {
    /// Check if this capability grants access for the requested `action`.
    ///
    /// Exact-match for simple capabilities. For parameterized capabilities
    /// (ChannelAccess, SharedMemoryAccess), the full resource id (slot and
    /// generation) must match.
    pub fn permits(&self, action: &Capability) -> bool {
        match (self, action) {
            (Capability::ChannelCreate, Capability::ChannelCreate) => true,
            (Capability::ChannelAccess(held), Capability::ChannelAccess(needed)) => held == needed,
            (Capability::SharedMemoryCreate, Capability::SharedMemoryCreate) => true,
            (Capability::SharedMemoryAccess(held), Capability::SharedMemoryAccess(needed)) => {
                held == needed
            }
            (Capability::SpawnAgent, Capability::SpawnAgent) => true,
            (Capability::DebugPrint, Capability::DebugPrint) => true,
            (Capability::GpuMmioAccess, Capability::GpuMmioAccess) => true,
            (Capability::GpuBufferCreate, Capability::GpuBufferCreate) => true,
            (Capability::GpuBufferAccess(held), Capability::GpuBufferAccess(needed)) => {
                held == needed
            }
            (Capability::DisplayControl, Capability::DisplayControl) => true,
            (Capability::CompositorCreateSurface, Capability::CompositorCreateSurface) => true,
            (Capability::CompositorFullscreen, Capability::CompositorFullscreen) => true,
            (Capability::CompositorOverlay, Capability::CompositorOverlay) => true,
            (Capability::CompositorInputAccess, Capability::CompositorInputAccess) => true,
            _ => false,
        }
    }

    /// Check if a token for `self` may be attenuated to a child for `other`.
    ///
    /// Only identity attenuation exists: the child names the same capability,
    /// and for an object-bound one (ChannelAccess, SharedMemoryAccess,
    /// GpuBufferAccess) the same full id. A Create capability never attenuates
    /// to an Access one (capability-lifetime ADR, owner decision 1): access to
    /// a new object is minted for its creator at creation
    /// (`CapabilityTable::mint_child`) and reaches others only by delegation.
    pub fn can_attenuate_to(&self, other: &Capability) -> bool {
        self.permits(other)
    }
}

// ---------------------------------------------------------------------------
// Capability token
// ---------------------------------------------------------------------------

/// A capability token granting a specific permission to a process.
#[derive(Debug, Clone, PartialEq, Eq)]
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

impl CapabilityToken {
    /// Whether the token has expired at `now_tick`: it expires at its
    /// `expires_at_tick`, not after it.
    pub fn is_expired(&self, now_tick: u64) -> bool {
        self.expires_at_tick.is_some_and(|exp| now_tick >= exp)
    }

    /// The token a share delegates from this one to `holder`, another
    /// process (capability-lifetime ADR §4): the same capability, not
    /// delegatable, this token as its `parent_token`, this token's expiry
    /// (never widened), created at `now_tick`, with id `id`.
    ///
    /// The child lives in `holder`'s table and its parent in this token's
    /// holder's, so revoking this token does not reach it
    /// (`CapabilityTable::revoke` cascades within one table); the region
    /// cascade, which destroys the object, makes it stale instead.
    pub fn delegated_child(
        &self,
        holder: ProcessId,
        id: CapabilityTokenId,
        now_tick: u64,
    ) -> CapabilityToken {
        CapabilityToken {
            id,
            capability: self.capability,
            holder,
            delegatable: false,
            revoked: false,
            parent_token: Some(self.id),
            usage_count: 0,
            created_at_tick: now_tick,
            expires_at_tick: self.expires_at_tick,
        }
    }
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

    /// The token with id `id`, revoked or not. None if no slot holds it.
    fn find_by_id(&self, id: CapabilityTokenId) -> Option<&CapabilityToken> {
        self.tokens.iter().flatten().find(|t| t.id == id)
    }

    /// Number of empty slots. A revoked token keeps its slot, so it counts as
    /// occupied.
    pub fn free_slots(&self) -> usize {
        self.tokens.iter().filter(|slot| slot.is_none()).count()
    }

    /// Whether the token `id` is revoked. True for an id no slot holds, so a
    /// cascade that asks about a token that is gone fails closed.
    pub fn is_revoked(&self, id: CapabilityTokenId) -> bool {
        self.find_by_id(id).is_none_or(|t| t.revoked)
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
            if token.revoked || token.is_expired(now_tick) {
                continue;
            }
            if token.capability.permits(cap) {
                return Some(token.id);
            }
        }
        None
    }

    /// The live, unexpired token for `cap` that a share delegates from: a
    /// delegatable match if the table holds one, otherwise the first match,
    /// so a share is not refused because a non-delegatable token for the
    /// same object happens to come first. None if no live token permits
    /// `cap`.
    pub fn delegation_parent(&self, cap: &Capability, now_tick: u64) -> Option<&CapabilityToken> {
        let mut live = self
            .tokens
            .iter()
            .flatten()
            .filter(|t| !t.revoked && !t.is_expired(now_tick) && t.capability.permits(cap));
        let first = live.clone().next();
        live.find(|t| t.delegatable).or(first)
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

    /// Revoke every token in the table that is not revoked yet, in one pass.
    /// Returns the number of tokens it revoked. Every slot stays occupied.
    pub fn revoke_all(&mut self) -> u32 {
        let mut revoked = 0;
        for token in self.tokens.iter_mut().flatten() {
            if !token.revoked {
                token.revoked = true;
                revoked += 1;
            }
        }
        self.count = 0;
        revoked
    }

    /// Empty every slot in place (no table-sized temporary on the stack), for
    /// process install and reap. Afterwards the table is as `new()` makes it.
    pub fn clear(&mut self) {
        for slot in self.tokens.iter_mut() {
            *slot = None;
        }
        self.count = 0;
    }

    /// Attenuate a capability: create a child token from the live token at
    /// `handle`.
    ///
    /// EPERM when the handle names no live token (out of range, empty or
    /// revoked), when that token has expired at `now_tick`, when its
    /// capability cannot attenuate to `new_cap` (identity only,
    /// `Capability::can_attenuate_to`), or when `new_expiry` is later than the
    /// parent's expiry: expiry never grows (`docs/security/model/capabilities.md`
    /// §3.3, `ExpiryExpansion`). A `None` request gives the child the parent's
    /// expiry; under a parent that never expires the child gets `new_expiry`
    /// as requested, so the child never expires only when neither does.
    /// ENOSPC when the table is full.
    ///
    /// The child is not delegatable and records the parent as `parent_token`.
    /// `new_id` is the unique token ID for the child (caller provides it to
    /// decouple from the kernel's atomic ID generator).
    pub fn attenuate(
        &mut self,
        handle: CapabilityHandle,
        new_cap: Capability,
        new_expiry: Option<u64>,
        holder: ProcessId,
        new_id: CapabilityTokenId,
        now_tick: u64,
    ) -> Result<CapabilityHandle, i64> {
        let eperm = Err(IpcError::Eperm as i64);
        let parent = match self.get(handle) {
            Some(t) => t,
            None => return eperm,
        };
        if parent.is_expired(now_tick) || !parent.capability.can_attenuate_to(&new_cap) {
            return eperm;
        }
        let expires_at_tick = match (parent.expires_at_tick, new_expiry) {
            (Some(limit), Some(requested)) if requested > limit => return eperm,
            (Some(limit), None) => Some(limit),
            (_, requested) => requested,
        };

        let child = CapabilityToken {
            id: new_id,
            capability: new_cap,
            holder,
            delegatable: false, // Attenuated tokens are not further delegatable by default.
            revoked: false,
            parent_token: Some(parent.id),
            usage_count: 0,
            created_at_tick: parent.created_at_tick,
            expires_at_tick,
        };

        self.grant(child)
    }

    /// Mint `capability` as a child of the live token `parent`, for the
    /// kernel's create paths (capability-lifetime ADR §2). Kernel-only: it
    /// does not consult `can_attenuate_to`, so a `ChannelCreate` parent can
    /// mint the `ChannelAccess` of the channel it just authorised.
    ///
    /// The child records `parent` as `parent_token`, so revoking the parent
    /// revokes it, and takes the parent's `delegatable` and expiry, which never
    /// widens what the parent allows; `created_at_tick` is `now_tick`.
    ///
    /// EPERM when no slot holds `parent`, or it is revoked or expired at
    /// `now_tick`. ENOSPC when the table is full.
    pub fn mint_child(
        &mut self,
        parent: CapabilityTokenId,
        capability: Capability,
        holder: ProcessId,
        id: CapabilityTokenId,
        now_tick: u64,
    ) -> Result<CapabilityHandle, i64> {
        let parent_token = match self.find_by_id(parent) {
            Some(t) if !t.revoked && !t.is_expired(now_tick) => t,
            _ => return Err(IpcError::Eperm as i64),
        };

        let child = CapabilityToken {
            id,
            capability,
            holder,
            delegatable: parent_token.delegatable,
            revoked: false,
            parent_token: Some(parent),
            usage_count: 0,
            created_at_tick: now_tick,
            expires_at_tick: parent_token.expires_at_tick,
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
// Delegation (SharedMemoryShare)
// ---------------------------------------------------------------------------

/// What a `SharedMemoryShare` found, read by the kernel under one
/// `PROCESS_TABLE → SHARED_REGION_TABLE` hold, for `share_decision`.
pub struct ShareFacts<'a> {
    /// The process asking to share.
    pub caller: ProcessId,
    /// The process to receive access; may be `caller`.
    pub target: ProcessId,
    /// Whether `caller` is a live process.
    pub caller_alive: bool,
    /// The caller's live token for `SharedMemoryAccess(full id)`
    /// (`CapabilityTable::delegation_parent`), or None.
    pub caller_token: Option<&'a CapabilityToken>,
    /// The region check, the ADR's step 3: `Ok`, or EPIPE for a slot that is
    /// empty, draining or of another generation, or EPERM when `caller` is not
    /// the region's creator.
    pub region: Result<(), i64>,
    /// Whether `target` is a live process.
    pub target_alive: bool,
    /// Whether `target` already holds a live token for the full id.
    pub target_holds: bool,
    /// Free slots in `target`'s capability table.
    pub target_free_slots: usize,
}

/// What `share_decision` allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareVerdict<'a> {
    /// The target already holds a live token for the region: `Ok`, and
    /// nothing is granted (a repeat share, and every self-share).
    AlreadyHeld,
    /// Grant the target `parent.delegated_child(..)`, a non-delegatable
    /// child of the caller's token.
    Grant {
        /// The caller's token the child is delegated from.
        parent: &'a CapabilityToken,
    },
}

/// The verdict on a `SharedMemoryShare` (capability-lifetime ADR §4, steps
/// 2-4; the malformed-argument EINVALs of step 1 come before any lock). In
/// order:
///
/// 1. EPERM: the caller is not alive; it holds no live token for the full
///    id; or the target is another process and that token is not
///    delegatable.
/// 2. The region check (`facts.region`): EPIPE or EPERM as the kernel found.
/// 3. EPERM if the target is not alive. `AlreadyHeld` if the target already
///    holds a live token for the full id (idempotent). ENOSPC if the target
///    is another process with `CAP_FOREIGN_RESERVE` or fewer free slots, or
///    is the caller with none. Otherwise `Grant`.
///
/// Pure: the kernel reads every fact and applies the verdict in one hold.
pub fn share_decision<'a>(facts: &ShareFacts<'a>) -> Result<ShareVerdict<'a>, i64> {
    let eperm = Err(IpcError::Eperm as i64);
    let foreign = facts.target != facts.caller;
    let parent = match facts.caller_token {
        Some(t) if facts.caller_alive && (t.delegatable || !foreign) => t,
        _ => return eperm,
    };
    facts.region?;
    if !facts.target_alive {
        return eperm;
    }
    if facts.target_holds {
        return Ok(ShareVerdict::AlreadyHeld);
    }
    let reserve = if foreign { CAP_FOREIGN_RESERVE } else { 0 };
    if facts.target_free_slots <= reserve {
        return Err(IpcError::Enospc as i64);
    }
    Ok(ShareVerdict::Grant { parent })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::ID_GEN_FIRST;

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
        let cap = Capability::ChannelAccess(ChannelId::new(5, ID_GEN_FIRST));
        assert!(cap.permits(&Capability::ChannelAccess(ChannelId::new(5, ID_GEN_FIRST))));
    }

    #[test]
    fn denies_channel_access_different() {
        let cap = Capability::ChannelAccess(ChannelId::new(5, ID_GEN_FIRST));
        assert!(!cap.permits(&Capability::ChannelAccess(ChannelId::new(6, ID_GEN_FIRST))));
    }

    #[test]
    fn denies_cross_variant() {
        assert!(!Capability::ChannelCreate.permits(&Capability::DebugPrint));
        assert!(!Capability::DebugPrint.permits(&Capability::ChannelCreate));
        assert!(!Capability::ChannelCreate
            .permits(&Capability::ChannelAccess(ChannelId::new(0, ID_GEN_FIRST))));
        assert!(!Capability::SpawnAgent.permits(&Capability::SharedMemoryCreate));
    }

    #[test]
    fn permits_shared_memory_create() {
        assert!(Capability::SharedMemoryCreate.permits(&Capability::SharedMemoryCreate));
    }

    #[test]
    fn permits_shared_memory_access_same() {
        let cap = Capability::SharedMemoryAccess(SharedMemoryId::new(42, ID_GEN_FIRST));
        assert!(
            cap.permits(&Capability::SharedMemoryAccess(SharedMemoryId::new(
                42,
                ID_GEN_FIRST
            )))
        );
    }

    #[test]
    fn denies_shared_memory_access_different() {
        let cap = Capability::SharedMemoryAccess(SharedMemoryId::new(42, ID_GEN_FIRST));
        assert!(
            !cap.permits(&Capability::SharedMemoryAccess(SharedMemoryId::new(
                43,
                ID_GEN_FIRST
            )))
        );
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
    fn attenuate_denies_channel_create_to_access() {
        assert!(!Capability::ChannelCreate
            .can_attenuate_to(&Capability::ChannelAccess(ChannelId::new(5, ID_GEN_FIRST))));
    }

    #[test]
    fn attenuate_denies_shm_create_to_access() {
        assert!(
            !Capability::SharedMemoryCreate.can_attenuate_to(&Capability::SharedMemoryAccess(
                SharedMemoryId::new(1, ID_GEN_FIRST)
            ))
        );
    }

    #[test]
    fn attenuate_identity() {
        assert!(Capability::DebugPrint.can_attenuate_to(&Capability::DebugPrint));
        assert!(Capability::ChannelAccess(ChannelId::new(3, ID_GEN_FIRST))
            .can_attenuate_to(&Capability::ChannelAccess(ChannelId::new(3, ID_GEN_FIRST))));
    }

    #[test]
    fn attenuate_denies_broadening() {
        // Cannot broaden from access to create.
        assert!(!Capability::ChannelAccess(ChannelId::new(5, ID_GEN_FIRST))
            .can_attenuate_to(&Capability::ChannelCreate));
        assert!(
            !Capability::SharedMemoryAccess(SharedMemoryId::new(1, ID_GEN_FIRST))
                .can_attenuate_to(&Capability::SharedMemoryCreate)
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
                Capability::ChannelAccess(ChannelId::new(5, ID_GEN_FIRST)),
                1,
                1,
            ))
            .unwrap();
        assert_eq!(table.count(), 2);

        // Revoking parent cascades to child.
        table.revoke(CapabilityTokenId(1));
        assert_eq!(table.count(), 0);
        assert!(!table.has_capability(&Capability::ChannelCreate, 0));
        assert!(!table.has_capability(
            &Capability::ChannelAccess(ChannelId::new(5, ID_GEN_FIRST)),
            0
        ));
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
                Capability::ChannelAccess(ChannelId::new(5, ID_GEN_FIRST)),
                1,
                1,
            ))
            .unwrap();
        table
            .grant(make_child_token(
                3,
                Capability::ChannelAccess(ChannelId::new(5, ID_GEN_FIRST)),
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
                Capability::ChannelAccess(ChannelId::new(5, ID_GEN_FIRST)),
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

    /// Attenuate the token at `h` to `cap` with the requested expiry, at tick
    /// `now`, as child token 2 held by pid 1.
    fn attenuate_at(
        table: &mut CapabilityTable,
        h: CapabilityHandle,
        cap: Capability,
        expiry: Option<u64>,
        now: u64,
    ) -> Result<CapabilityHandle, i64> {
        table.attenuate(h, cap, expiry, ProcessId(1), CapabilityTokenId(2), now)
    }

    fn chan_access(slot: usize) -> Capability {
        Capability::ChannelAccess(ChannelId::new(slot, ID_GEN_FIRST))
    }

    #[test]
    fn attenuate_identity_access_ok() {
        let mut table = CapabilityTable::new();
        let h = table.grant(make_token(1, chan_access(7), 1)).unwrap();
        let child_h = attenuate_at(&mut table, h, chan_access(7), None, 0).unwrap();

        let child = table.get(child_h).unwrap();
        assert_eq!(child.id, CapabilityTokenId(2));
        assert_eq!(child.capability, chan_access(7));
        assert_eq!(child.parent_token, Some(CapabilityTokenId(1)));
        assert!(!child.delegatable);
        assert_eq!(child.expires_at_tick, None);
        assert_eq!(table.count(), 2);
    }

    #[test]
    fn attenuate_denies_other_channel() {
        let mut table = CapabilityTable::new();
        let h = table.grant(make_token(1, chan_access(7), 1)).unwrap();
        assert_eq!(
            attenuate_at(&mut table, h, chan_access(8), None, 0),
            Err(IpcError::Eperm as i64)
        );
        assert_eq!(table.count(), 1);
    }

    /// SVC 15 type 1 from a `ChannelCreate` handle: x1 = 1 decodes to
    /// `ChannelAccess(x3)`, x2 = 0 to a `None` expiry.
    #[test]
    fn attenuate_svc15_channel_create_to_access_eperm() {
        let mut table = CapabilityTable::new();
        let h = table
            .grant(make_token(1, Capability::ChannelCreate, 1))
            .unwrap();
        assert_eq!(
            attenuate_at(&mut table, h, chan_access(7), None, 0),
            Err(IpcError::Eperm as i64)
        );
        assert_eq!(table.count(), 1);
    }

    #[test]
    fn attenuate_denies_shm_create_to_access_in_table() {
        let mut table = CapabilityTable::new();
        let h = table
            .grant(make_token(1, Capability::SharedMemoryCreate, 1))
            .unwrap();
        let access = Capability::SharedMemoryAccess(SharedMemoryId::new(3, ID_GEN_FIRST));
        assert_eq!(
            attenuate_at(&mut table, h, access, None, 0),
            Err(IpcError::Eperm as i64)
        );
    }

    #[test]
    fn table_attenuate_denies_broadening() {
        let mut table = CapabilityTable::new();
        let h = table.grant(make_token(1, chan_access(5), 1)).unwrap();
        let result = attenuate_at(&mut table, h, Capability::ChannelCreate, None, 0);
        assert_eq!(result, Err(IpcError::Eperm as i64));
    }

    #[test]
    fn attenuate_invalid_handle() {
        let mut table = CapabilityTable::new();
        let result = attenuate_at(
            &mut table,
            CapabilityHandle(0),
            Capability::DebugPrint,
            None,
            0,
        );
        assert_eq!(result, Err(IpcError::Eperm as i64));
    }

    #[test]
    fn attenuate_with_expiry() {
        let mut table = CapabilityTable::new();
        let h = table.grant(make_token(1, chan_access(3), 1)).unwrap();
        let child_h = attenuate_at(&mut table, h, chan_access(3), Some(500), 0).unwrap();

        let child = table.get(child_h).unwrap();
        assert_eq!(child.expires_at_tick, Some(500));
    }

    #[test]
    fn attenuate_revoked_parent_denied() {
        let mut table = CapabilityTable::new();
        let h = table.grant(make_token(1, chan_access(5), 1)).unwrap();
        table.revoke(CapabilityTokenId(1));
        // get() returns None for revoked → attenuate returns Eperm.
        let result = attenuate_at(&mut table, h, chan_access(5), None, 0);
        assert_eq!(result, Err(IpcError::Eperm as i64));
    }

    #[test]
    fn attenuate_expiring_parent_later_request_eperm() {
        let mut table = CapabilityTable::new();
        let h = table
            .grant(make_expiring_token(1, Capability::DebugPrint, 100))
            .unwrap();
        assert_eq!(
            attenuate_at(&mut table, h, Capability::DebugPrint, Some(101), 10),
            Err(IpcError::Eperm as i64)
        );
        assert_eq!(table.count(), 1);
    }

    #[test]
    fn attenuate_expiring_parent_equal_request_ok() {
        let mut table = CapabilityTable::new();
        let h = table
            .grant(make_expiring_token(1, Capability::DebugPrint, 100))
            .unwrap();
        let child_h = attenuate_at(&mut table, h, Capability::DebugPrint, Some(100), 10).unwrap();
        assert_eq!(table.get(child_h).unwrap().expires_at_tick, Some(100));
    }

    #[test]
    fn attenuate_expiring_parent_earlier_request_ok() {
        let mut table = CapabilityTable::new();
        let h = table
            .grant(make_expiring_token(1, Capability::DebugPrint, 100))
            .unwrap();
        let child_h = attenuate_at(&mut table, h, Capability::DebugPrint, Some(50), 10).unwrap();
        assert_eq!(table.get(child_h).unwrap().expires_at_tick, Some(50));
    }

    #[test]
    fn attenuate_expiring_parent_none_request_inherits() {
        let mut table = CapabilityTable::new();
        let h = table
            .grant(make_expiring_token(1, Capability::DebugPrint, 100))
            .unwrap();
        let child_h = attenuate_at(&mut table, h, Capability::DebugPrint, None, 10).unwrap();
        assert_eq!(table.get(child_h).unwrap().expires_at_tick, Some(100));
    }

    #[test]
    fn attenuate_never_expiring_parent_allows_any_expiry() {
        let mut table = CapabilityTable::new();
        let h = table
            .grant(make_token(1, Capability::DebugPrint, 1))
            .unwrap();
        for (requested, id) in [(u64::MAX, 2), (11, 3)] {
            let child_h = table
                .attenuate(
                    h,
                    Capability::DebugPrint,
                    Some(requested),
                    ProcessId(1),
                    CapabilityTokenId(id),
                    10,
                )
                .unwrap();
            assert_eq!(table.get(child_h).unwrap().expires_at_tick, Some(requested));
        }
    }

    #[test]
    fn attenuate_never_expiring_parent_none_request_never_expires() {
        let mut table = CapabilityTable::new();
        let h = table
            .grant(make_token(1, Capability::DebugPrint, 1))
            .unwrap();
        let child_h = attenuate_at(&mut table, h, Capability::DebugPrint, None, 10).unwrap();
        assert_eq!(table.get(child_h).unwrap().expires_at_tick, None);
    }

    #[test]
    fn attenuate_expired_parent_eperm() {
        let mut table = CapabilityTable::new();
        let h = table
            .grant(make_expiring_token(1, Capability::DebugPrint, 100))
            .unwrap();
        // One tick before its expiry the parent is still live: both requests
        // succeed, and the child keeps the parent's expiry.
        for (new_id, requested) in [(2, None), (3, Some(100))] {
            let child_h = table
                .attenuate(
                    h,
                    Capability::DebugPrint,
                    requested,
                    ProcessId(1),
                    CapabilityTokenId(new_id),
                    99,
                )
                .unwrap();
            assert_eq!(table.get(child_h).unwrap().expires_at_tick, Some(100));
        }
        // At its expiry tick and after it, with or without a requested expiry.
        for now in [100, 200] {
            assert_eq!(
                attenuate_at(&mut table, h, Capability::DebugPrint, None, now),
                Err(IpcError::Eperm as i64)
            );
            assert_eq!(
                attenuate_at(&mut table, h, Capability::DebugPrint, Some(100), now),
                Err(IpcError::Eperm as i64)
            );
        }
        // The parent and the two children from tick 99; the denials added none.
        assert_eq!(table.count(), 3);
    }

    #[test]
    fn attenuate_full_table_enospc() {
        let mut table = CapabilityTable::new();
        for i in 0..MAX_CAPS_PER_PROCESS as u64 {
            table
                .grant(make_token(i + 1, Capability::DebugPrint, 1))
                .unwrap();
        }
        assert_eq!(table.free_slots(), 0);
        // The parent is live and the identity attenuation is allowed, so only
        // the missing slot refuses it.
        let h = CapabilityHandle(0);
        assert!(table.get(h).is_some());
        assert_eq!(
            table.attenuate(
                h,
                Capability::DebugPrint,
                None,
                ProcessId(1),
                CapabilityTokenId(1000),
                0,
            ),
            Err(IpcError::Enospc as i64)
        );
        assert_eq!(table.count(), MAX_CAPS_PER_PROCESS as u32);
    }

    // --- CapabilityTable: mint_child ---

    #[test]
    fn mint_child_sets_parent_delegatable_and_expiry() {
        let mut table = CapabilityTable::new();
        let mut parent = make_expiring_token(1, Capability::ChannelCreate, 900);
        parent.delegatable = true;
        parent.created_at_tick = 5;
        table.grant(parent).unwrap();

        // A Create parent mints the Access child that attenuation refuses.
        let h = table
            .mint_child(
                CapabilityTokenId(1),
                chan_access(4),
                ProcessId(1),
                CapabilityTokenId(2),
                40,
            )
            .unwrap();
        let child = table.get(h).unwrap();
        assert_eq!(child.id, CapabilityTokenId(2));
        assert_eq!(child.capability, chan_access(4));
        assert_eq!(child.holder, ProcessId(1));
        assert_eq!(child.parent_token, Some(CapabilityTokenId(1)));
        assert!(child.delegatable);
        assert!(!child.revoked);
        assert_eq!(child.expires_at_tick, Some(900));
        assert_eq!(child.created_at_tick, 40);
        assert_eq!(child.usage_count, 0);
        assert_eq!(table.count(), 2);

        // A non-delegatable parent that never expires passes both on.
        table
            .grant(make_token(3, Capability::SharedMemoryCreate, 1))
            .unwrap();
        let access = Capability::SharedMemoryAccess(SharedMemoryId::new(2, ID_GEN_FIRST));
        let h = table
            .mint_child(
                CapabilityTokenId(3),
                access,
                ProcessId(1),
                CapabilityTokenId(4),
                41,
            )
            .unwrap();
        let child = table.get(h).unwrap();
        assert!(!child.delegatable);
        assert_eq!(child.expires_at_tick, None);
        assert_eq!(child.parent_token, Some(CapabilityTokenId(3)));
    }

    #[test]
    fn mint_child_revoke_parent_revokes_child_and_grandchild() {
        let mut table = CapabilityTable::new();
        table
            .grant(make_token(1, Capability::ChannelCreate, 1))
            .unwrap();
        let child = table
            .mint_child(
                CapabilityTokenId(1),
                Capability::ChannelCreate,
                ProcessId(1),
                CapabilityTokenId(2),
                0,
            )
            .unwrap();
        let grandchild = table
            .mint_child(
                CapabilityTokenId(2),
                chan_access(9),
                ProcessId(1),
                CapabilityTokenId(3),
                0,
            )
            .unwrap();
        assert_eq!(table.count(), 3);

        table.revoke(CapabilityTokenId(1));
        assert!(table.get(child).is_none());
        assert!(table.get(grandchild).is_none());
        assert!(table.is_revoked(CapabilityTokenId(2)));
        assert!(table.is_revoked(CapabilityTokenId(3)));
        assert_eq!(table.count(), 0);
    }

    #[test]
    fn mint_child_revoked_expired_or_missing_parent_eperm() {
        let mut table = CapabilityTable::new();
        table
            .grant(make_token(1, Capability::ChannelCreate, 1))
            .unwrap();
        table
            .grant(make_expiring_token(2, Capability::ChannelCreate, 100))
            .unwrap();
        table.revoke(CapabilityTokenId(1));
        let eperm = Err(IpcError::Eperm as i64);

        let mut mint = |parent: u64, now: u64| {
            table.mint_child(
                CapabilityTokenId(parent),
                chan_access(1),
                ProcessId(1),
                CapabilityTokenId(10),
                now,
            )
        };
        assert_eq!(mint(1, 0), eperm, "revoked parent");
        assert_eq!(mint(2, 100), eperm, "parent at its expiry tick");
        assert_eq!(mint(2, 200), eperm, "parent past its expiry");
        assert_eq!(mint(99, 0), eperm, "missing parent");
        assert_eq!(table.free_slots(), MAX_CAPS_PER_PROCESS - 2);
    }

    #[test]
    fn mint_child_full_table_enospc() {
        let mut table = CapabilityTable::new();
        for i in 0..MAX_CAPS_PER_PROCESS as u64 {
            table
                .grant(make_token(i + 1, Capability::ChannelCreate, 1))
                .unwrap();
        }
        assert_eq!(table.free_slots(), 0);
        assert_eq!(
            table.mint_child(
                CapabilityTokenId(1),
                chan_access(0),
                ProcessId(1),
                CapabilityTokenId(1000),
                0,
            ),
            Err(IpcError::Enospc as i64)
        );
    }

    // --- CapabilityTable: free_slots, is_revoked, revoke_all, clear ---

    #[test]
    fn free_slots_counts_revoked_as_occupied() {
        let mut table = CapabilityTable::new();
        assert_eq!(table.free_slots(), MAX_CAPS_PER_PROCESS);
        table
            .grant(make_token(1, Capability::DebugPrint, 1))
            .unwrap();
        table
            .grant(make_token(2, Capability::SpawnAgent, 1))
            .unwrap();
        assert_eq!(table.free_slots(), MAX_CAPS_PER_PROCESS - 2);
        table.revoke(CapabilityTokenId(1));
        assert_eq!(table.free_slots(), MAX_CAPS_PER_PROCESS - 2);
        assert_eq!(table.count(), 1);
    }

    #[test]
    fn is_revoked_missing_live_and_cascaded() {
        let mut table = CapabilityTable::new();
        table
            .grant(make_token(1, Capability::ChannelCreate, 1))
            .unwrap();
        table
            .mint_child(
                CapabilityTokenId(1),
                chan_access(2),
                ProcessId(1),
                CapabilityTokenId(2),
                0,
            )
            .unwrap();
        assert!(table.is_revoked(CapabilityTokenId(99)), "missing id");
        assert!(!table.is_revoked(CapabilityTokenId(1)), "live parent");
        assert!(!table.is_revoked(CapabilityTokenId(2)), "live child");

        table.revoke(CapabilityTokenId(1));
        assert!(table.is_revoked(CapabilityTokenId(1)));
        assert!(
            table.is_revoked(CapabilityTokenId(2)),
            "child of a revoked parent"
        );
    }

    #[test]
    fn revoke_all_counts_newly_revoked() {
        let mut table = CapabilityTable::new();
        for i in 1..=4 {
            table
                .grant(make_token(i, Capability::DebugPrint, 1))
                .unwrap();
        }
        table.revoke(CapabilityTokenId(2));
        assert_eq!(table.revoke_all(), 3);
        assert_eq!(table.count(), 0);
        assert_eq!(table.free_slots(), MAX_CAPS_PER_PROCESS - 4);
        for i in 1..=4 {
            assert!(table.is_revoked(CapabilityTokenId(i)));
        }
        assert!(!table.has_capability(&Capability::DebugPrint, 0));
        assert_eq!(table.revoke_all(), 0);
    }

    #[test]
    fn revoke_all_empty_table() {
        let mut table = CapabilityTable::new();
        assert_eq!(table.revoke_all(), 0);
        assert_eq!(table.count(), 0);
    }

    #[test]
    fn clear_leaves_empty_table() {
        let mut table = CapabilityTable::new();
        table
            .grant(make_token(1, Capability::DebugPrint, 1))
            .unwrap();
        table
            .grant(make_token(2, Capability::SpawnAgent, 1))
            .unwrap();
        table.revoke(CapabilityTokenId(1));
        table.clear();

        assert_eq!(table.count(), 0);
        assert_eq!(table.free_slots(), MAX_CAPS_PER_PROCESS);
        assert!(table.tokens().iter().all(Option::is_none));
        assert!(table.is_revoked(CapabilityTokenId(2)));
        // The next grant takes slot 0 again.
        let h = table
            .grant(make_token(3, Capability::DebugPrint, 1))
            .unwrap();
        assert_eq!(h, CapabilityHandle(0));
        assert_eq!(table.count(), 1);
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

    // --- GPU capability permits tests (Phase 6 M20) ---

    #[test]
    fn permits_gpu_mmio_access() {
        assert!(Capability::GpuMmioAccess.permits(&Capability::GpuMmioAccess));
    }

    #[test]
    fn permits_gpu_buffer_create() {
        assert!(Capability::GpuBufferCreate.permits(&Capability::GpuBufferCreate));
    }

    #[test]
    fn permits_gpu_buffer_access_same() {
        let cap = Capability::GpuBufferAccess(1);
        assert!(cap.permits(&Capability::GpuBufferAccess(1)));
    }

    #[test]
    fn denies_gpu_buffer_access_different() {
        let cap = Capability::GpuBufferAccess(1);
        assert!(!cap.permits(&Capability::GpuBufferAccess(2)));
    }

    #[test]
    fn permits_display_control() {
        assert!(Capability::DisplayControl.permits(&Capability::DisplayControl));
    }

    #[test]
    fn denies_gpu_cross_variant() {
        assert!(!Capability::GpuMmioAccess.permits(&Capability::GpuBufferCreate));
        assert!(!Capability::GpuBufferCreate.permits(&Capability::DisplayControl));
        assert!(!Capability::DisplayControl.permits(&Capability::GpuMmioAccess));
        assert!(!Capability::GpuBufferAccess(1).permits(&Capability::ChannelCreate));
        assert!(!Capability::ChannelCreate.permits(&Capability::GpuMmioAccess));
    }

    // --- GPU capability attenuation tests ---

    #[test]
    fn attenuate_denies_gpu_buffer_create_to_access() {
        assert!(!Capability::GpuBufferCreate.can_attenuate_to(&Capability::GpuBufferAccess(42)));
    }

    #[test]
    fn attenuate_gpu_buffer_access_denies_broadening() {
        assert!(!Capability::GpuBufferAccess(1).can_attenuate_to(&Capability::GpuBufferCreate));
    }

    #[test]
    fn attenuate_gpu_identity() {
        assert!(Capability::GpuMmioAccess.can_attenuate_to(&Capability::GpuMmioAccess));
        assert!(Capability::DisplayControl.can_attenuate_to(&Capability::DisplayControl));
        assert!(Capability::GpuBufferAccess(5).can_attenuate_to(&Capability::GpuBufferAccess(5)));
    }

    #[test]
    fn attenuate_gpu_denies_cross_domain() {
        assert!(!Capability::GpuBufferCreate
            .can_attenuate_to(&Capability::ChannelAccess(ChannelId::new(0, ID_GEN_FIRST))));
        assert!(!Capability::ChannelCreate.can_attenuate_to(&Capability::GpuBufferAccess(1)));
    }

    // --- Compositor capability tests (Phase 7 M24) ---
    //
    // M24 uses flat capability variants (no parameters, no creation→access
    // narrowing). Phase 18 will introduce the rich `DisplayCapability` struct
    // with per-surface scoping; until then identity attenuation is all we need.

    #[test]
    fn permits_compositor_create_surface() {
        assert!(Capability::CompositorCreateSurface.permits(&Capability::CompositorCreateSurface));
    }

    #[test]
    fn permits_compositor_fullscreen() {
        assert!(Capability::CompositorFullscreen.permits(&Capability::CompositorFullscreen));
    }

    #[test]
    fn permits_compositor_overlay() {
        assert!(Capability::CompositorOverlay.permits(&Capability::CompositorOverlay));
    }

    #[test]
    fn permits_compositor_input_access() {
        assert!(Capability::CompositorInputAccess.permits(&Capability::CompositorInputAccess));
    }

    #[test]
    fn denies_compositor_cross_variant() {
        assert!(!Capability::CompositorCreateSurface.permits(&Capability::CompositorFullscreen));
        assert!(!Capability::CompositorOverlay.permits(&Capability::CompositorInputAccess));
        assert!(!Capability::CompositorInputAccess.permits(&Capability::CompositorCreateSurface));
    }

    #[test]
    fn denies_compositor_versus_gpu() {
        // Compositor and GPU caps are unrelated — neither implies the other.
        assert!(!Capability::CompositorCreateSurface.permits(&Capability::GpuBufferCreate));
        assert!(!Capability::GpuMmioAccess.permits(&Capability::CompositorCreateSurface));
    }

    #[test]
    fn attenuate_compositor_identity() {
        assert!(Capability::CompositorCreateSurface
            .can_attenuate_to(&Capability::CompositorCreateSurface));
        assert!(
            Capability::CompositorFullscreen.can_attenuate_to(&Capability::CompositorFullscreen)
        );
        assert!(Capability::CompositorOverlay.can_attenuate_to(&Capability::CompositorOverlay));
        assert!(
            Capability::CompositorInputAccess.can_attenuate_to(&Capability::CompositorInputAccess)
        );
    }

    #[test]
    fn attenuate_compositor_denies_cross_domain() {
        // No creation→access narrowing exists for compositor caps in M24.
        assert!(!Capability::CompositorCreateSurface
            .can_attenuate_to(&Capability::CompositorFullscreen));
        assert!(!Capability::CompositorCreateSurface
            .can_attenuate_to(&Capability::ChannelAccess(ChannelId::new(0, ID_GEN_FIRST))));
        assert!(!Capability::ChannelCreate.can_attenuate_to(&Capability::CompositorCreateSurface));
    }

    // --- Delegation: share_decision, delegation_parent, delegated_child ---

    fn region_access() -> Capability {
        Capability::SharedMemoryAccess(SharedMemoryId::new(3, ID_GEN_FIRST))
    }

    /// A caller token for `region_access()`, delegatable or not.
    fn caller_token(delegatable: bool) -> CapabilityToken {
        let mut t = make_token(10, region_access(), 1);
        t.delegatable = delegatable;
        t
    }

    /// Facts for a share from pid 1 to `target` that every check passes,
    /// with the target holding no token and `free` free slots.
    fn facts(token: &CapabilityToken, target: u32, free: usize) -> ShareFacts<'_> {
        ShareFacts {
            caller: ProcessId(1),
            target: ProcessId(target),
            caller_alive: true,
            caller_token: Some(token),
            region: Ok(()),
            target_alive: true,
            target_holds: false,
            target_free_slots: free,
        }
    }

    #[test]
    fn share_decision_foreign_reserve_boundary() {
        let token = caller_token(true);
        assert_eq!(CAP_FOREIGN_RESERVE, 64);
        assert_eq!(
            share_decision(&facts(&token, 2, 64)),
            Err(IpcError::Enospc as i64)
        );
        assert_eq!(
            share_decision(&facts(&token, 2, 65)),
            Ok(ShareVerdict::Grant { parent: &token })
        );
        assert_eq!(
            share_decision(&facts(&token, 2, 0)),
            Err(IpcError::Enospc as i64)
        );
    }

    #[test]
    fn share_decision_self_share_ignores_the_reserve() {
        let token = caller_token(false);
        // A self-target is not foreign: only a full table refuses it.
        assert_eq!(
            share_decision(&facts(&token, 1, 1)),
            Ok(ShareVerdict::Grant { parent: &token })
        );
        assert_eq!(
            share_decision(&facts(&token, 1, 0)),
            Err(IpcError::Enospc as i64)
        );
    }

    #[test]
    fn share_decision_delegatable_rule() {
        let eperm = Err(IpcError::Eperm as i64);
        let plain = caller_token(false);
        let delegatable = caller_token(true);
        // Another target needs a delegatable token.
        assert_eq!(share_decision(&facts(&plain, 2, 200)), eperm);
        assert_eq!(
            share_decision(&facts(&delegatable, 2, 200)),
            Ok(ShareVerdict::Grant {
                parent: &delegatable
            })
        );
        // The caller itself does not: it already holds the token, so the
        // share is a no-op either way.
        for token in [&plain, &delegatable] {
            let mut f = facts(token, 1, 200);
            f.target_holds = true;
            assert_eq!(share_decision(&f), Ok(ShareVerdict::AlreadyHeld));
        }
    }

    #[test]
    fn share_decision_idempotent() {
        let token = caller_token(true);
        let mut f = facts(&token, 2, 200);
        f.target_holds = true;
        assert_eq!(share_decision(&f), Ok(ShareVerdict::AlreadyHeld));
        // A target that already holds the token needs no slot, so the reserve
        // does not refuse a repeat share.
        f.target_free_slots = 0;
        assert_eq!(share_decision(&f), Ok(ShareVerdict::AlreadyHeld));
    }

    #[test]
    fn share_decision_failure_order() {
        let eperm = Err(IpcError::Eperm as i64);
        let epipe = Err(IpcError::Epipe as i64);
        let token = caller_token(true);

        // Caller not alive, or no token: EPERM before the region check.
        let mut f = facts(&token, 2, 200);
        f.caller_alive = false;
        f.region = epipe;
        assert_eq!(share_decision(&f), eperm);
        let mut f = facts(&token, 2, 200);
        f.caller_token = None;
        f.region = epipe;
        assert_eq!(share_decision(&f), eperm);
        // A non-delegatable token for a foreign target: EPERM before EPIPE.
        let plain = caller_token(false);
        let mut f = facts(&plain, 2, 200);
        f.region = epipe;
        assert_eq!(share_decision(&f), eperm);

        // The region check's error comes next, before the target's checks.
        let mut f = facts(&token, 2, 0);
        f.region = epipe;
        f.target_alive = false;
        assert_eq!(share_decision(&f), Err(IpcError::Epipe as i64));

        // Then a dead target, before idempotence.
        let mut f = facts(&token, 2, 200);
        f.target_alive = false;
        f.target_holds = true;
        assert_eq!(share_decision(&f), eperm);
    }

    #[test]
    fn delegation_parent_prefers_a_delegatable_token() {
        let mut table = CapabilityTable::new();
        assert!(table.delegation_parent(&region_access(), 0).is_none());

        table.grant(caller_token(false)).unwrap();
        let first = table.delegation_parent(&region_access(), 0).unwrap();
        assert_eq!(first.id, CapabilityTokenId(10));

        let mut second = make_token(11, region_access(), 1);
        second.delegatable = true;
        table.grant(second).unwrap();
        let parent = table.delegation_parent(&region_access(), 0).unwrap();
        assert_eq!(parent.id, CapabilityTokenId(11));

        // Revoked and expired tokens never qualify.
        table.revoke(CapabilityTokenId(11));
        let parent = table.delegation_parent(&region_access(), 0).unwrap();
        assert_eq!(parent.id, CapabilityTokenId(10));
        let mut table = CapabilityTable::new();
        let mut expiring = make_expiring_token(12, region_access(), 50);
        expiring.delegatable = true;
        table.grant(expiring).unwrap();
        assert!(table.delegation_parent(&region_access(), 49).is_some());
        assert!(table.delegation_parent(&region_access(), 50).is_none());
    }

    #[test]
    fn delegated_child_is_a_non_delegatable_child() {
        let mut parent = make_expiring_token(10, region_access(), 900);
        parent.delegatable = true;
        parent.created_at_tick = 5;
        let child = parent.delegated_child(ProcessId(2), CapabilityTokenId(20), 40);
        assert_eq!(child.id, CapabilityTokenId(20));
        assert_eq!(child.capability, region_access());
        assert_eq!(child.holder, ProcessId(2));
        assert!(!child.delegatable);
        assert!(!child.revoked);
        assert_eq!(child.parent_token, Some(CapabilityTokenId(10)));
        assert_eq!(child.usage_count, 0);
        assert_eq!(child.created_at_tick, 40);
        assert_eq!(child.expires_at_tick, Some(900));
    }
}
