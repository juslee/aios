//! Write-Ahead Log — circular buffer for crash-safe block writes.
//!
//! Each WAL entry is 64 bytes (`#[repr(C)]`), stored 8 per sector.
//! The WAL occupies a contiguous region on disk starting at WAL_START_SECTOR.
//!
//! Per spaces.md §4.4: committed entries are replayed on recovery;
//! uncommitted entries with valid data blocks are salvaged.

use shared::storage::{
    ContentHash, StorageError, SECTOR_SIZE, WAL_ENTRIES_PER_SECTOR, WAL_ENTRY_SIZE,
};

use crate::drivers::virtio_blk;

pub use shared::storage::WalEntry;

/// Write-Ahead Log state.
pub struct Wal {
    /// First sector of the WAL region on disk.
    start_sector: u64,
    /// Total sectors in the WAL region.
    size_sectors: u64,
    /// Next write position (entry index from start of WAL).
    head: u64,
    /// Oldest valid entry index.
    tail: u64,
    /// Next sequence number to assign.
    next_sequence: u64,
}

impl Wal {
    /// Create a new WAL at the given disk region.
    pub fn new(start_sector: u64, size_sectors: u64) -> Self {
        Self {
            start_sector,
            size_sectors,
            head: 0,
            tail: 0,
            next_sequence: 1,
        }
    }

    /// Total capacity in entries.
    pub fn capacity_entries(&self) -> u64 {
        self.size_sectors * WAL_ENTRIES_PER_SECTOR as u64
    }

    /// Number of active entries (head - tail).
    #[allow(dead_code)]
    pub fn active_entries(&self) -> u64 {
        self.head - self.tail
    }

    /// Current head position.
    pub fn head(&self) -> u64 {
        self.head
    }

    /// Current tail position.
    pub fn tail(&self) -> u64 {
        self.tail
    }

    /// Set head/tail from superblock recovery.
    pub fn set_positions(&mut self, head: u64, tail: u64, next_seq: u64) {
        self.head = head;
        self.tail = tail;
        self.next_sequence = next_seq;
    }

    /// Append a new WAL entry (uncommitted).
    ///
    /// Parameters are byte-level, matching WalEntry fields directly.
    /// Returns `(sequence_number, logical_index)` so commit() can update directly.
    pub fn append(
        &mut self,
        block_id: ContentHash,
        data_offset: u64,
        data_size: u32,
    ) -> Result<(u64, u64), StorageError> {
        // Check WAL capacity.
        if self.head - self.tail >= self.capacity_entries() {
            return Err(StorageError::WalFull);
        }

        let seq = self.next_sequence;
        let mut entry = WalEntry::new(seq, block_id.0, data_offset, data_size, 0);
        entry.checksum = entry.compute_checksum();

        let index = self.head;
        self.write_entry(index, &entry)?;
        self.head += 1;
        self.next_sequence += 1;

        Ok((seq, index))
    }

    /// Mark a WAL entry as committed by logical index (O(1), no scan).
    pub fn commit_at(&mut self, index: u64) -> Result<(), StorageError> {
        let mut entry = self.read_entry(index)?;
        entry.committed = 1;
        entry.checksum = entry.compute_checksum();
        self.write_entry(index, &entry)
    }

    /// Mark a WAL entry as committed by sequence number (O(n) scan fallback).
    /// Used during recovery when only the sequence number is known.
    pub fn commit(&mut self, sequence_number: u64) -> Result<(), StorageError> {
        for idx in self.tail..self.head {
            let entry = self.read_entry(idx)?;
            if entry.sequence_number == sequence_number {
                let mut updated = entry;
                updated.committed = 1;
                updated.checksum = updated.compute_checksum();
                self.write_entry(idx, &updated)?;
                return Ok(());
            }
        }
        Err(StorageError::IoError)
    }

    /// Read a WAL entry at the given logical index.
    pub fn read_entry(&self, index: u64) -> Result<WalEntry, StorageError> {
        let wrapped = index % self.capacity_entries();
        let sector_in_wal = wrapped / WAL_ENTRIES_PER_SECTOR as u64;
        let entry_in_sector = (wrapped % WAL_ENTRIES_PER_SECTOR as u64) as usize;

        let disk_sector = self.start_sector + sector_in_wal;
        let mut sector_buf = [0u8; SECTOR_SIZE];
        virtio_blk::read_sector(disk_sector, &mut sector_buf)?;

        let offset = entry_in_sector * WAL_ENTRY_SIZE;
        let entry_bytes = &sector_buf[offset..offset + WAL_ENTRY_SIZE];

        // SAFETY: WalEntry is repr(C), 64 bytes, plain data (no pointers).
        // Maintained by repr(C) attribute, compile-time size assertion, and bounds check above.
        // If violated, read_unaligned returns garbage; is_valid() rejects corrupt entries.
        let entry = unsafe { core::ptr::read_unaligned(entry_bytes.as_ptr() as *const WalEntry) };
        Ok(entry)
    }

    /// Write a WAL entry at the given logical index.
    fn write_entry(&self, index: u64, entry: &WalEntry) -> Result<(), StorageError> {
        let wrapped = index % self.capacity_entries();
        let sector_in_wal = wrapped / WAL_ENTRIES_PER_SECTOR as u64;
        let entry_in_sector = (wrapped % WAL_ENTRIES_PER_SECTOR as u64) as usize;

        let disk_sector = self.start_sector + sector_in_wal;

        // Read-modify-write the sector (8 entries per sector).
        let mut sector_buf = [0u8; SECTOR_SIZE];
        virtio_blk::read_sector(disk_sector, &mut sector_buf)?;

        let offset = entry_in_sector * WAL_ENTRY_SIZE;
        // SAFETY: WalEntry is repr(C), 64 bytes, plain data (no pointers).
        // Maintained by modular arithmetic: offset + WAL_ENTRY_SIZE <= SECTOR_SIZE (512).
        // If violated, copy_nonoverlapping writes past sector_buf, corrupting stack memory.
        unsafe {
            core::ptr::copy_nonoverlapping(
                entry as *const WalEntry as *const u8,
                sector_buf[offset..].as_mut_ptr(),
                WAL_ENTRY_SIZE,
            );
        }

        virtio_blk::write_sector(disk_sector, &sector_buf)?;
        Ok(())
    }

    /// Advance tail past committed+replayed entries, freeing WAL space.
    /// Not called in M13 (MemTable is in-memory only); deferred to M14+ when SSTables exist.
    #[allow(dead_code)]
    pub fn trim_committed(&mut self) {
        // Advance tail while entries are committed (best-effort, ignore read errors).
        while self.tail < self.head {
            if let Ok(entry) = self.read_entry(self.tail) {
                if entry.is_valid() && entry.committed == 1 {
                    self.tail += 1;
                } else {
                    break;
                }
            } else {
                break;
            }
        }
    }
}
