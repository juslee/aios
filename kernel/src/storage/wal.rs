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

use super::block_engine::crc32c;

/// On-disk WAL entry (64 bytes, fixed layout).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WalEntry {
    /// Monotonically increasing sequence number.
    pub sequence_number: u64,
    /// Content hash (SHA-256) of the block data.
    pub block_id: [u8; 32],
    /// Byte offset of the data in the data region (BlockLocation.offset).
    pub data_offset: u64,
    /// Data size in bytes (BlockLocation.size).
    pub data_size: u32,
    /// 0 = pending, 1 = committed.
    pub committed: u8,
    /// Padding.
    _pad: [u8; 3],
    /// CRC-32C of all fields above (bytes 0..56).
    pub checksum: u32,
    /// Padding to 64 bytes.
    _pad2: [u8; 4],
}

const _: () = assert!(core::mem::size_of::<WalEntry>() == WAL_ENTRY_SIZE);

impl WalEntry {
    /// Compute CRC-32C over the first 56 bytes (everything except checksum + pad2).
    fn compute_checksum(&self) -> u32 {
        // SAFETY: WalEntry is repr(C), 64 bytes. First 56 bytes are the checksummed payload.
        let bytes = unsafe { core::slice::from_raw_parts(self as *const Self as *const u8, 56) };
        crc32c(bytes)
    }

    /// Check if this entry has a valid checksum.
    pub fn is_valid(&self) -> bool {
        self.checksum == self.compute_checksum()
    }

    /// Get the content hash as a ContentHash.
    pub fn content_hash(&self) -> ContentHash {
        ContentHash(self.block_id)
    }
}

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
        let mut entry = WalEntry {
            sequence_number: seq,
            block_id: block_id.0,
            data_offset,
            data_size,
            committed: 0,
            _pad: [0; 3],
            checksum: 0,
            _pad2: [0; 4],
        };
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
        virtio_blk::read_sector(disk_sector, array_ref_mut(&mut sector_buf))?;

        let offset = entry_in_sector * WAL_ENTRY_SIZE;
        let entry_bytes = &sector_buf[offset..offset + WAL_ENTRY_SIZE];

        // SAFETY: WalEntry is repr(C), 64 bytes, all fields are plain data (no pointers).
        // Use read_unaligned because entry_bytes is a &[u8] subslice with alignment 1.
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
        virtio_blk::read_sector(disk_sector, array_ref_mut(&mut sector_buf))?;

        let offset = entry_in_sector * WAL_ENTRY_SIZE;
        // SAFETY: WalEntry is repr(C), 64 bytes. Writing into a [u8; 512] at valid offset.
        unsafe {
            core::ptr::copy_nonoverlapping(
                entry as *const WalEntry as *const u8,
                sector_buf[offset..].as_mut_ptr(),
                WAL_ENTRY_SIZE,
            );
        }

        virtio_blk::write_sector(disk_sector, array_ref(&sector_buf))?;
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

/// Helper to convert `&[u8; 512]` to `&[u8; 512]` for virtio_blk API.
#[inline(always)]
fn array_ref(buf: &[u8; SECTOR_SIZE]) -> &[u8; 512] {
    buf
}

/// Helper to convert `&mut [u8; 512]` to `&mut [u8; 512]` for virtio_blk API.
#[inline(always)]
fn array_ref_mut(buf: &mut [u8; SECTOR_SIZE]) -> &mut [u8; 512] {
    buf
}
