//! LSM-tree MemTable — in-memory sorted index for content-addressed blocks.
//!
//! Stub for Step 3. Full implementation in Step 4.

#![allow(dead_code)]

use shared::storage::{BlockLocation, ContentHash, StorageError};

/// In-memory sorted index entry.
pub struct MemTableEntry {
    pub key: ContentHash,
    pub location: BlockLocation,
    pub refcount: u32,
}

/// Sorted array MemTable for block lookups.
pub struct MemTable {
    _capacity: usize,
}

impl MemTable {
    /// Create a new empty MemTable (stub — no allocation yet).
    pub fn new(capacity: usize) -> Self {
        Self {
            _capacity: capacity,
        }
    }

    /// Number of entries (stub: always 0).
    pub fn count(&self) -> usize {
        0
    }

    /// Look up a block by content hash (stub: always None).
    #[allow(dead_code)]
    pub fn get(&self, _key: &ContentHash) -> Option<&MemTableEntry> {
        None
    }

    /// Insert a new entry (stub: always succeeds).
    #[allow(dead_code)]
    pub fn insert(
        &mut self,
        _key: ContentHash,
        _location: BlockLocation,
    ) -> Result<(), StorageError> {
        Ok(())
    }
}
