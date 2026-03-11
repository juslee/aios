//! Capability types shared between kernel and user-space.
//!
//! Defines the Capability enum (Phase 3 subset), token/handle identifiers,
//! and the `permits()` matching logic. Per security.md §2.2, §3.1–3.5.

use crate::ipc::ChannelId;

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
}
