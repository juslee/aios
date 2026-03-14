//! Space management — security-zoned containers for objects.
//!
//! Spaces organize objects into security zones with metadata and quotas.
//! Three system spaces are created at boot: system/, user/home/, ephemeral/.
//!
//! Per spaces.md §3.1 Spaces, §3.2 System Spaces.

extern crate alloc;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use shared::storage::*;

use super::block_engine;

// ---------------------------------------------------------------------------
// Space ID generation
// ---------------------------------------------------------------------------

/// Monotonic counter for unique space ID generation.
static SPACE_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a unique SpaceId from TICK_COUNT + monotonic counter.
fn generate_space_id() -> SpaceId {
    let tick = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);
    let counter = SPACE_ID_COUNTER.fetch_add(1, Ordering::Relaxed);

    let mut id = [0u8; 16];
    id[..8].copy_from_slice(&tick.to_le_bytes());
    id[8..].copy_from_slice(&counter.to_le_bytes());
    SpaceId(id)
}

// ---------------------------------------------------------------------------
// Space table (stored in BlockEngine)
// ---------------------------------------------------------------------------

/// In-memory space registry. Fixed-size array of optional spaces.
pub struct SpaceTable {
    spaces: [Option<Space>; MAX_SPACES],
}

impl SpaceTable {
    /// Create an empty space table.
    pub const fn new() -> Self {
        Self {
            spaces: [None; MAX_SPACES],
        }
    }

    /// Number of active spaces.
    pub fn count(&self) -> usize {
        self.spaces.iter().filter(|s| s.is_some()).count()
    }

    /// Find a space by ID.
    pub fn get(&self, id: &SpaceId) -> Option<&Space> {
        self.spaces
            .iter()
            .filter_map(|s| s.as_ref())
            .find(|s| s.id == *id)
    }

    /// Find a space by ID (mutable, used for quota updates).
    pub fn get_mut(&mut self, id: &SpaceId) -> Option<&mut Space> {
        self.spaces
            .iter_mut()
            .filter_map(|s| s.as_mut())
            .find(|s| s.id == *id)
    }

    /// Insert a new space. Returns error if table is full.
    pub fn insert(&mut self, space: Space) -> Result<(), StorageError> {
        // Find an empty slot.
        for slot in self.spaces.iter_mut() {
            if slot.is_none() {
                *slot = Some(space);
                return Ok(());
            }
        }
        Err(StorageError::QuotaExceeded) // Table full (MAX_SPACES reached)
    }

    /// Remove a space by ID. Returns the removed space.
    pub fn remove(&mut self, id: &SpaceId) -> Option<Space> {
        for slot in self.spaces.iter_mut() {
            if let Some(space) = slot {
                if space.id == *id {
                    return slot.take();
                }
            }
        }
        None
    }

    /// List all active spaces.
    pub fn list(&self) -> Vec<Space> {
        self.spaces.iter().filter_map(|s| *s).collect()
    }
}

// ---------------------------------------------------------------------------
// Space operations (module-level, use with_engine)
// ---------------------------------------------------------------------------

/// Create a new space with the given name, security zone, and quota.
pub fn space_create(
    name: &[u8],
    zone: SecurityZone,
    quota: SpaceQuota,
) -> Result<SpaceId, StorageError> {
    block_engine::with_engine(|engine| {
        let id = generate_space_id();

        let tick = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);
        let now = Timestamp(tick);

        let mut space = Space::ZERO;
        space.id = id;
        space.set_name(name);
        space.security_zone = zone;
        space.set_quota(quota);
        space.created_at = now;
        space.modified_at = now;

        engine.space_table_mut().insert(space)?;

        Ok(id)
    })?
}

/// List all spaces.
pub fn space_list() -> Result<Vec<Space>, StorageError> {
    block_engine::with_engine(|engine| engine.space_table().list())
}

/// Get a space by ID.
pub fn space_get(id: &SpaceId) -> Result<Space, StorageError> {
    block_engine::with_engine(|engine| {
        engine
            .space_table()
            .get(id)
            .copied()
            .ok_or(StorageError::SpaceNotFound)
    })?
}

/// Delete a space (only if it has no objects).
pub fn space_delete(id: &SpaceId) -> Result<(), StorageError> {
    block_engine::with_engine(|engine| {
        // Check if space has objects.
        let space = engine
            .space_table()
            .get(id)
            .ok_or(StorageError::SpaceNotFound)?;
        if space.object_count > 0 {
            return Err(StorageError::SpaceNotEmpty);
        }

        engine
            .space_table_mut()
            .remove(id)
            .ok_or(StorageError::SpaceNotFound)?;
        Ok(())
    })?
}

// ---------------------------------------------------------------------------
// System space initialization
// ---------------------------------------------------------------------------

/// Create the three system spaces at boot (per spaces.md §3.2).
///
/// - `system/`      — Core zone (kernel config, audit, credentials)
/// - `user/home/`   — Personal zone (default personal space)
/// - `ephemeral/`   — Ephemeral zone (auto-cleaned, no version history)
pub fn init_system_spaces() {
    let results = [
        space_create(b"system", SecurityZone::Core, SpaceQuota::UNLIMITED),
        space_create(b"user/home", SecurityZone::Personal, SpaceQuota::UNLIMITED),
        space_create(b"ephemeral", SecurityZone::Ephemeral, SpaceQuota::UNLIMITED),
    ];

    for (i, result) in results.iter().enumerate() {
        let names = ["system", "user/home", "ephemeral"];
        match result {
            Ok(_id) => {
                crate::kinfo!(Storage, "System space '{}' created", names[i]);
            }
            Err(e) => {
                crate::kerror!(
                    Storage,
                    "Failed to create system space '{}': {:?}",
                    names[i],
                    e
                );
            }
        }
    }
}

/// Register the space-storage service with the service manager.
///
/// Creates an IPC channel and registers it so other subsystems can
/// discover and communicate with the Space Storage service.
pub fn register_service() {
    use crate::ipc;
    use crate::task::process::ProcessId;
    use crate::task::ThreadId;

    // Create a channel for the space-storage service (kernel-internal, no cap check).
    let space_tid = ThreadId(0x800);
    let ch = ipc::channel_create_unchecked(space_tid);

    // Register with kernel process (PID 0).
    if let Err(e) = crate::service::service_register(b"space-storage", ProcessId(0), ch) {
        crate::kerror!(Storage, "space-storage register failed: {}", e);
    } else {
        crate::kinfo!(Storage, "Registered 'space-storage' service (ch={})", ch.0);
    }
}
