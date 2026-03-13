//! LSM-tree MemTable — in-memory sorted index for content-addressed blocks.
//!
//! Heap-allocated sorted array with binary search for O(log n) lookups.
//! Capacity: 65536 entries (48B × 65536 = 3 MiB). Slab allocator falls
//! through to buddy for >PAGE_SIZE allocations.
//!
//! Per spaces.md §4.2.

use alloc::vec;
use alloc::vec::Vec;

use shared::storage::{BlockLocation, ContentHash, StorageError, MEMTABLE_MAX_ENTRIES};

/// In-memory sorted index entry.
pub struct MemTableEntry {
    pub key: ContentHash,
    pub location: BlockLocation,
    /// Reference count (separate from BlockLocation per arch doc).
    pub refcount: u32,
}

/// Sorted array MemTable for content-addressed block lookups.
pub struct MemTable {
    entries: Vec<MemTableEntry>,
    capacity: usize,
}

impl MemTable {
    /// Create a new empty MemTable with the given capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: vec![],
            capacity,
        }
    }

    /// Create a MemTable with default capacity (MEMTABLE_MAX_ENTRIES).
    pub fn with_default_capacity() -> Self {
        Self::new(MEMTABLE_MAX_ENTRIES)
    }

    /// Number of entries.
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// Capacity.
    #[allow(dead_code)]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Is the table full?
    pub fn is_full(&self) -> bool {
        self.entries.len() >= self.capacity
    }

    /// Look up a block by content hash.
    pub fn get(&self, key: &ContentHash) -> Option<&MemTableEntry> {
        let idx = self.binary_search(key).ok()?;
        Some(&self.entries[idx])
    }

    /// Look up a block by content hash (mutable, for refcount updates).
    pub fn get_mut(&mut self, key: &ContentHash) -> Option<&mut MemTableEntry> {
        let idx = self.binary_search(key).ok()?;
        Some(&mut self.entries[idx])
    }

    /// Insert a new entry. If key already exists, increments refcount instead.
    ///
    /// Returns `true` if this was a new insertion, `false` if dedup (refcount bump).
    pub fn insert(
        &mut self,
        key: ContentHash,
        location: BlockLocation,
    ) -> Result<bool, StorageError> {
        match self.binary_search(&key) {
            Ok(idx) => {
                // Key exists — increment refcount (dedup).
                self.entries[idx].refcount += 1;
                Ok(false)
            }
            Err(insert_pos) => {
                if self.is_full() {
                    return Err(StorageError::MemTableFull);
                }
                self.entries.insert(
                    insert_pos,
                    MemTableEntry {
                        key,
                        location,
                        refcount: 1,
                    },
                );
                Ok(true)
            }
        }
    }

    /// Remove an entry by key.
    #[allow(dead_code)]
    pub fn remove(&mut self, key: &ContentHash) -> Option<BlockLocation> {
        let idx = self.binary_search(key).ok()?;
        let entry = self.entries.remove(idx);
        Some(entry.location)
    }

    /// Binary search for a key. Returns Ok(index) if found, Err(insert_pos) if not.
    fn binary_search(&self, key: &ContentHash) -> Result<usize, usize> {
        self.entries
            .binary_search_by(|entry| entry.key.0.cmp(&key.0))
    }
}
