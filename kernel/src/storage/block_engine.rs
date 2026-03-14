//! Block Engine — content-addressed storage with superblock and WAL.
//!
//! Provides crash-safe write path: WAL append → data write → WAL commit.
//! Data blocks are content-addressed by SHA-256 hash with CRC-32C integrity.
//!
//! Per spaces.md §4.1 (superblock), §4.4 (WAL), §3.0 (content addressing).

use sha2::{Digest, Sha256};
use shared::storage::*;
use spin::Mutex;

use crate::drivers::virtio_blk;

use super::crypto::DeviceKeyManager;
use super::lsm::MemTable;
use super::object_store::ObjectIndex;
use super::space::SpaceTable;
use super::wal::Wal;

// ---------------------------------------------------------------------------
// CRC-32C (Castagnoli) — 256-entry lookup table
// ---------------------------------------------------------------------------

/// CRC-32C lookup table using Castagnoli polynomial 0x1EDC6F41.
const CRC32C_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let poly: u32 = 0x82F6_3B78; // bit-reversed 0x1EDC6F41
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ poly;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

/// Compute CRC-32C checksum of `data`.
pub fn crc32c(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc = CRC32C_TABLE[((crc ^ byte as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

// ---------------------------------------------------------------------------
// Superblock (4096 bytes, on-disk at sectors 0-7)
// ---------------------------------------------------------------------------

/// On-disk superblock (padded to 4096 bytes = 8 sectors).
#[repr(C)]
pub struct Superblock {
    pub magic: u64,
    pub version: u32,
    pub block_size: u32,
    pub total_sectors: u64,
    pub wal_start_sector: u64,
    pub wal_size_sectors: u64,
    pub data_start_sector: u64,
    /// Next free sector in the data region (append pointer).
    pub data_next_sector: u64,
    pub wal_head: u64,
    pub wal_tail: u64,
    pub free_data_sectors: u64,
    /// Reserved for LSM-tree L0 offset (0 for M13).
    pub lsm_l0_offset: u64,
    /// Encryption key epoch (0 = unencrypted, 1+ = encrypted).
    pub encryption_epoch: u64,
    /// Global monotonic nonce counter for AES-GCM.
    pub nonce_counter: u64,
    /// Random prefix for nonce generation (first 4 bytes of 12-byte nonce).
    pub nonce_random_prefix: u32,
    /// CRC-32C of all fields above.
    pub checksum: u32,
    _padding: [u8; SUPERBLOCK_PADDING],
}

/// Padding size to fill superblock to exactly BLOCK_SIZE (4096) bytes.
/// Fields: magic(8) + version(4) + block_size(4) + total_sectors(8) + wal_start(8)
///       + wal_size(8) + data_start(8) + data_next(8) + wal_head(8) + wal_tail(8)
///       + free_data(8) + lsm_l0(8) + enc_epoch(8) + nonce_counter(8) + nonce_prefix(4)
///       + checksum(4) = 112
const SUPERBLOCK_PADDING: usize = BLOCK_SIZE - 112;

const _: () = assert!(core::mem::size_of::<Superblock>() == BLOCK_SIZE);

impl Superblock {
    /// Compute CRC-32C over the superblock fields (everything before checksum).
    fn compute_checksum(&self) -> u32 {
        // Checksum covers bytes 0..108 (magic through nonce_random_prefix).
        // 8+4+4+8+8+8+8+8+8+8+8+8+8+8+4 = 108 bytes
        let offset_of_checksum = 108;

        // SAFETY: Superblock is repr(C), plain data (no pointers or padding).
        // Maintained by the fixed field layout and compile-time size_of check.
        // If violated, CRC-32C is computed over incorrect bytes, causing checksum mismatch.
        let bytes = unsafe {
            core::slice::from_raw_parts(self as *const Self as *const u8, offset_of_checksum)
        };
        crc32c(bytes)
    }

    /// Validate the superblock: magic, version, and checksum.
    fn is_valid(&self) -> bool {
        self.magic == SUPERBLOCK_MAGIC
            && self.version == SUPERBLOCK_VERSION
            && self.checksum == self.compute_checksum()
    }

    /// Create a fresh superblock for a new disk.
    fn format(total_sectors: u64) -> Self {
        let data_sectors = total_sectors - DATA_START_SECTOR;

        // Generate a random prefix from timer entropy.
        let tick =
            crate::arch::aarch64::timer::TICK_COUNT.load(core::sync::atomic::Ordering::Relaxed);
        let random_prefix = (tick & 0xFFFF_FFFF) as u32;

        let mut sb = Superblock {
            magic: SUPERBLOCK_MAGIC,
            version: SUPERBLOCK_VERSION,
            block_size: BLOCK_SIZE as u32,
            total_sectors,
            wal_start_sector: WAL_START_SECTOR,
            wal_size_sectors: WAL_SIZE_SECTORS,
            data_start_sector: DATA_START_SECTOR,
            data_next_sector: DATA_START_SECTOR,
            wal_head: 0,
            wal_tail: 0,
            free_data_sectors: data_sectors,
            lsm_l0_offset: 0,
            encryption_epoch: 1,
            nonce_counter: 0,
            nonce_random_prefix: random_prefix,
            checksum: 0,
            _padding: [0; SUPERBLOCK_PADDING],
        };
        sb.checksum = sb.compute_checksum();
        sb
    }
}

// ---------------------------------------------------------------------------
// Block Engine
// ---------------------------------------------------------------------------

/// Block Engine state: superblock + WAL + MemTable + object index + spaces + crypto + data region pointer.
pub struct BlockEngine {
    superblock: Superblock,
    wal: Wal,
    memtable: MemTable,
    object_index: ObjectIndex,
    space_table: SpaceTable,
    /// Device-level encryption manager (None = plaintext mode).
    crypto: Option<DeviceKeyManager>,
    /// Next free sector in the data region.
    data_next_sector: u64,
}

/// Global Block Engine instance.
/// Lock ordering: BLOCK_ENGINE > VIRTIO_BLK (Block Engine calls read/write_sector internally).
static BLOCK_ENGINE: Mutex<Option<BlockEngine>> = Mutex::new(None);

impl BlockEngine {
    /// Initialize the Block Engine: read or format the superblock, init WAL.
    fn init() -> Result<Self, StorageError> {
        let total_sectors = virtio_blk::capacity_sectors();
        if total_sectors == 0 {
            return Err(StorageError::DeviceNotFound);
        }

        // Try to read existing superblock.
        let sb = Self::read_superblock()?;

        if let Some(sb) = sb {
            crate::kinfo!(
                Storage,
                "Superblock: valid (v{}, {} sectors)",
                sb.version,
                sb.total_sectors
            );
            let mut wal = Wal::new(sb.wal_start_sector, sb.wal_size_sectors);
            wal.set_positions(sb.wal_head, sb.wal_tail, sb.wal_head + 1);
            let data_next = sb.data_next_sector;
            // Init encryption from superblock state.
            let crypto = if sb.encryption_epoch > 0 {
                Some(DeviceKeyManager::from_passphrase(
                    b"aios-dev-key",
                    sb.nonce_counter,
                    sb.nonce_random_prefix,
                ))
            } else {
                None
            };

            let mut engine = Self {
                superblock: sb,
                wal,
                memtable: MemTable::with_default_capacity(),
                object_index: ObjectIndex::new(),
                space_table: SpaceTable::new(),
                crypto,
                data_next_sector: data_next,
            };

            // Replay WAL to rebuild MemTable index.
            let recovered = engine.recover();
            crate::kinfo!(Storage, "WAL recovery: {} entries replayed", recovered);

            Ok(engine)
        } else {
            // Format new disk.
            crate::kinfo!(
                Storage,
                "Superblock: formatting new disk ({} sectors)",
                total_sectors
            );
            let sb = Superblock::format(total_sectors);
            Self::write_superblock(&sb)?;
            let wal = Wal::new(sb.wal_start_sector, sb.wal_size_sectors);
            let data_next = sb.data_start_sector;
            let crypto = if sb.encryption_epoch > 0 {
                Some(DeviceKeyManager::from_passphrase(
                    b"aios-dev-key",
                    sb.nonce_counter,
                    sb.nonce_random_prefix,
                ))
            } else {
                None
            };
            Ok(Self {
                superblock: sb,
                wal,
                memtable: MemTable::with_default_capacity(),
                object_index: ObjectIndex::new(),
                space_table: SpaceTable::new(),
                crypto,
                data_next_sector: data_next,
            })
        }
    }

    /// Read the superblock from disk (sectors 0-7).
    fn read_superblock() -> Result<Option<Superblock>, StorageError> {
        let mut buf = [0u8; BLOCK_SIZE];
        for i in 0..8 {
            let mut sector_buf = [0u8; SECTOR_SIZE];
            virtio_blk::read_sector(i, &mut sector_buf)?;
            let offset = i as usize * SECTOR_SIZE;
            buf[offset..offset + SECTOR_SIZE].copy_from_slice(&sector_buf);
        }

        // SAFETY: Superblock is repr(C), BLOCK_SIZE bytes, plain data (no pointers).
        // Maintained by repr(C) layout and is_valid() check after deserialization.
        // If violated, read_unaligned returns garbage; is_valid() rejects it.
        let sb = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const Superblock) };
        if sb.is_valid() {
            Ok(Some(sb))
        } else {
            Ok(None)
        }
    }

    /// Write the superblock to disk (sectors 0-7).
    fn write_superblock(sb: &Superblock) -> Result<(), StorageError> {
        // SAFETY: Superblock is repr(C), BLOCK_SIZE bytes, plain data (no pointers).
        // Maintained by repr(C) layout and checksum field set before serialization.
        // If violated, from_raw_parts produces incorrect bytes, corrupting the on-disk superblock.
        let bytes = unsafe {
            core::slice::from_raw_parts(sb as *const Superblock as *const u8, BLOCK_SIZE)
        };
        for i in 0..8 {
            let offset = i * SECTOR_SIZE;
            let mut sector_buf = [0u8; SECTOR_SIZE];
            sector_buf.copy_from_slice(&bytes[offset..offset + SECTOR_SIZE]);
            virtio_blk::write_sector(i as u64, &sector_buf)?;
        }
        Ok(())
    }

    /// Write a data block to disk with crash safety via WAL.
    ///
    /// Returns (content_hash, BlockLocation) on success.
    /// When encryption is enabled, the on-disk format is:
    ///   `[nonce(12B) | encrypted{crc32c|data_len|data|pad} | tag(16B)]`
    pub fn write_block(
        &mut self,
        data: &[u8],
    ) -> Result<(ContentHash, BlockLocation), StorageError> {
        if data.is_empty() {
            return Err(StorageError::IoError);
        }

        // 1. SHA-256 content hash.
        let mut hasher = Sha256::new();
        hasher.update(data);
        let hash_bytes: [u8; 32] = hasher.finalize().into();
        let content_hash = ContentHash(hash_bytes);

        // 2. Dedup check: if hash exists in MemTable, bump refcount and return.
        if let Some(entry) = self.memtable.get_mut(&content_hash) {
            entry.refcount += 1;
            let loc = entry.location;
            return Ok((content_hash, loc));
        }

        // 3. Compute on-disk layout: [crc32c: u32 | data_len: u32 | data | padding]
        let header_size = 8; // crc32c (4) + data_len (4)
        let plaintext_envelope = header_size + data.len();
        let on_disk_size = if self.crypto.is_some() {
            plaintext_envelope + ENCRYPTION_OVERHEAD
        } else {
            plaintext_envelope
        };
        let sectors_needed = on_disk_size.div_ceil(SECTOR_SIZE) as u64;

        // Check space.
        let data_end = self.superblock.total_sectors;
        if self.data_next_sector + sectors_needed > data_end {
            return Err(StorageError::DeviceFull);
        }

        // 4. CRC-32C of raw data.
        let data_crc = crc32c(data);

        // 5. Build the byte offset and size for BlockLocation.
        if data.len() > u32::MAX as usize {
            return Err(StorageError::IoError);
        }
        let byte_offset = self.data_next_sector * SECTOR_SIZE as u64;
        let byte_size = data.len() as u32;

        // 6. WAL append (uncommitted).
        let (_seq, wal_index) = self.wal.append(content_hash, byte_offset, byte_size)?;

        // 7. Build plaintext envelope: [crc32c | data_len | data]
        let mut envelope = alloc::vec![0u8; plaintext_envelope];
        envelope[0..4].copy_from_slice(&data_crc.to_le_bytes());
        envelope[4..8].copy_from_slice(&(data.len() as u32).to_le_bytes());
        envelope[8..].copy_from_slice(data);

        // 8. Encrypt if enabled.
        let write_data = if let Some(ref crypto) = self.crypto {
            let mut encrypted = alloc::vec![0u8; on_disk_size];
            crypto.encrypt(&envelope, &mut encrypted)?;
            // Persist nonce counter BEFORE writing data sectors.
            // If we crash after data write but before nonce flush, we'd reuse nonces.
            // By flushing first, the persisted counter is always >= any nonce used on disk.
            self.flush_superblock()?;
            encrypted
        } else {
            envelope
        };

        // 9. Write sectors to disk.
        let mut write_offset = 0usize;
        let mut sector_idx = 0u64;
        while write_offset < write_data.len() {
            let mut sector_buf = [0u8; SECTOR_SIZE];
            let chunk = (write_data.len() - write_offset).min(SECTOR_SIZE);
            sector_buf[..chunk].copy_from_slice(&write_data[write_offset..write_offset + chunk]);
            virtio_blk::write_sector(self.data_next_sector + sector_idx, &sector_buf)?;
            write_offset += chunk;
            sector_idx += 1;
        }

        // 10. WAL commit (O(1) using index from append).
        self.wal.commit_at(wal_index)?;

        // 11. Advance append pointer.
        self.data_next_sector += sectors_needed;

        let location = BlockLocation {
            offset: byte_offset,
            size: byte_size,
            tier: StorageTier::Hot,
        };

        // 12. Insert into MemTable index.
        self.memtable
            .insert(content_hash, location)
            .map_err(|_| StorageError::MemTableFull)?;

        Ok((content_hash, location))
    }

    /// Read a data block from disk by BlockLocation.
    ///
    /// Handles transparent decryption when device encryption is enabled.
    /// Returns the number of bytes read into `buf`.
    pub fn read_block(&self, loc: &BlockLocation, buf: &mut [u8]) -> Result<usize, StorageError> {
        let start_sector = loc.offset / SECTOR_SIZE as u64;
        let data_len = loc.size as usize;

        if buf.len() < data_len {
            return Err(StorageError::IoError);
        }

        let header_size = 8usize; // crc32c(4) + data_len(4)
        let plaintext_envelope = header_size + data_len;

        if let Some(ref crypto) = self.crypto {
            // Encrypted path: read all sectors, decrypt, then parse envelope.
            let on_disk_size = plaintext_envelope + ENCRYPTION_OVERHEAD;
            let sectors = on_disk_size.div_ceil(SECTOR_SIZE) as u64;
            let mut raw = alloc::vec![0u8; sectors as usize * SECTOR_SIZE];

            let mut sector_buf = [0u8; SECTOR_SIZE];
            for i in 0..sectors {
                virtio_blk::read_sector(start_sector + i, &mut sector_buf)?;
                let off = i as usize * SECTOR_SIZE;
                raw[off..off + SECTOR_SIZE].copy_from_slice(&sector_buf);
            }

            // Decrypt: [nonce(12) | ciphertext | tag(16)] → plaintext at raw[12..12+pt_len].
            let pt_len = crypto.decrypt(&mut raw[..on_disk_size])?;
            let envelope = &raw[12..12 + pt_len];

            let stored_crc =
                u32::from_le_bytes([envelope[0], envelope[1], envelope[2], envelope[3]]);
            let stored_len =
                u32::from_le_bytes([envelope[4], envelope[5], envelope[6], envelope[7]]) as usize;

            if stored_len != data_len {
                return Err(StorageError::ChecksumFailed);
            }

            buf[..data_len].copy_from_slice(&envelope[8..8 + data_len]);

            if crc32c(&buf[..data_len]) != stored_crc {
                return Err(StorageError::ChecksumFailed);
            }

            Ok(data_len)
        } else {
            // Plaintext path: read sector-by-sector, parse header, verify CRC.
            let mut sector_buf = [0u8; SECTOR_SIZE];
            virtio_blk::read_sector(start_sector, &mut sector_buf)?;

            let stored_crc =
                u32::from_le_bytes([sector_buf[0], sector_buf[1], sector_buf[2], sector_buf[3]]);
            let stored_len =
                u32::from_le_bytes([sector_buf[4], sector_buf[5], sector_buf[6], sector_buf[7]])
                    as usize;

            if stored_len != data_len {
                return Err(StorageError::ChecksumFailed);
            }

            let first_chunk = data_len.min(SECTOR_SIZE - header_size);
            buf[..first_chunk].copy_from_slice(&sector_buf[8..8 + first_chunk]);

            let mut buf_offset = first_chunk;
            let mut sector_idx = 1u64;
            while buf_offset < data_len {
                virtio_blk::read_sector(start_sector + sector_idx, &mut sector_buf)?;
                let chunk = (data_len - buf_offset).min(SECTOR_SIZE);
                buf[buf_offset..buf_offset + chunk].copy_from_slice(&sector_buf[..chunk]);
                buf_offset += chunk;
                sector_idx += 1;
            }

            if crc32c(&buf[..data_len]) != stored_crc {
                return Err(StorageError::ChecksumFailed);
            }

            Ok(data_len)
        }
    }

    /// Verify a block's CRC (and decrypt if encrypted).
    ///
    /// Delegates to `read_block()` which handles both plaintext and encrypted paths.
    /// Used during WAL recovery to check if an uncommitted block was actually written.
    fn verify_block_crc(&self, loc: &BlockLocation) -> Result<(), StorageError> {
        let data_len = loc.size as usize;
        let mut buf = alloc::vec![0u8; data_len];
        self.read_block(loc, &mut buf)?;
        Ok(())
    }

    /// Read a data block by content hash (MemTable lookup → disk read).
    pub fn read_block_by_hash(
        &self,
        hash: &ContentHash,
        buf: &mut [u8],
    ) -> Result<usize, StorageError> {
        let entry = self.memtable.get(hash).ok_or(StorageError::BlockNotFound)?;
        self.read_block(&entry.location, buf)
    }

    /// Replay WAL entries to rebuild the MemTable after boot.
    ///
    /// - Committed entries: insert into MemTable (rebuild index).
    /// - Uncommitted entries with valid data on disk: salvage (recover + commit).
    /// - Uncommitted entries with no valid data: discard.
    ///
    /// Returns the total number of entries replayed (committed + salvaged).
    fn recover(&mut self) -> u64 {
        let mut replayed = 0u64;
        let mut max_data_sector = self.data_next_sector;

        let tail = self.wal.tail();
        let head = self.wal.head();

        for idx in tail..head {
            let entry = match self.wal.read_entry(idx) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.is_valid() {
                continue;
            }

            let content_hash = entry.content_hash();
            let location = BlockLocation {
                offset: entry.data_offset,
                size: entry.data_size,
                tier: StorageTier::Hot,
            };

            if entry.committed == 1 {
                // Committed entry: insert into MemTable.
                if self.memtable.insert(content_hash, location).is_ok() {
                    replayed += 1;
                } else {
                    crate::kerror!(
                        Storage,
                        "WAL recovery: MemTable full, cannot replay seq={}",
                        entry.sequence_number
                    );
                    break; // Stop recovery — no point continuing if MemTable is full.
                }
            } else {
                // Uncommitted: check if data block was actually written by
                // verifying CRC across all sectors (works for any block size).
                if entry.data_size > 0 && self.verify_block_crc(&location).is_ok() {
                    // Salvage: data is on disk, insert into MemTable.
                    if self.memtable.insert(content_hash, location).is_ok() {
                        // Mark committed in WAL (best-effort).
                        let _ = self.wal.commit(entry.sequence_number);
                        replayed += 1;
                        crate::kinfo!(
                            Storage,
                            "WAL recovery: salvaged uncommitted entry seq={}",
                            entry.sequence_number
                        );
                    } else {
                        crate::kerror!(
                            Storage,
                            "WAL recovery: MemTable full, cannot salvage seq={}",
                            entry.sequence_number
                        );
                        break;
                    }
                }
            }

            // Track highest data sector for append pointer recovery.
            // On-disk size: header(8) + data + optional ENCRYPTION_OVERHEAD(28).
            let on_disk_bytes = if self.crypto.is_some() {
                entry.data_size as u64 + 8 + ENCRYPTION_OVERHEAD as u64
            } else {
                entry.data_size as u64 + 8
            };
            let entry_sectors = on_disk_bytes.div_ceil(SECTOR_SIZE as u64);
            let entry_end = entry.data_offset / SECTOR_SIZE as u64 + entry_sectors;
            if entry_end > max_data_sector {
                max_data_sector = entry_end;
            }
        }

        // Restore append pointer to after the highest known data.
        if max_data_sector > self.data_next_sector {
            self.data_next_sector = max_data_sector;
        }

        // NOTE: Do NOT trim WAL entries here. The MemTable is in-memory only
        // (no persistent SSTable yet). Trimming would lose the ability to rebuild
        // the index on next reboot. WAL trimming is deferred until M14+ when
        // SSTables provide durable index persistence.

        replayed
    }

    /// Access the MemTable (read-only).
    pub fn memtable(&self) -> &MemTable {
        &self.memtable
    }

    /// Increment the reference count for a content hash.
    ///
    /// Used when a new object references the same content (dedup).
    pub fn inc_ref(&mut self, hash: &ContentHash) -> Result<(), StorageError> {
        let entry = self
            .memtable
            .get_mut(hash)
            .ok_or(StorageError::BlockNotFound)?;
        entry.refcount += 1;
        Ok(())
    }

    /// Decrement the reference count for a content hash.
    ///
    /// If refcount reaches 0, the block is logically freed (entry removed from MemTable).
    /// Returns `true` if the block was freed.
    pub fn dec_ref(&mut self, hash: &ContentHash) -> Result<bool, StorageError> {
        match self.memtable.dec_ref(hash) {
            Some((_loc, freed)) => Ok(freed),
            None => Err(StorageError::BlockNotFound),
        }
    }

    /// Access the object index (read-only).
    pub fn object_index(&self) -> &ObjectIndex {
        &self.object_index
    }

    /// Access the object index (mutable).
    pub fn object_index_mut(&mut self) -> &mut ObjectIndex {
        &mut self.object_index
    }

    /// Access the space table (read-only).
    pub fn space_table(&self) -> &SpaceTable {
        &self.space_table
    }

    /// Access the space table (mutable).
    pub fn space_table_mut(&mut self) -> &mut SpaceTable {
        &mut self.space_table
    }

    /// Returns true if device-level encryption is active.
    pub fn crypto_active(&self) -> bool {
        self.crypto.is_some()
    }

    /// Flush the superblock to disk with current state.
    pub fn flush_superblock(&mut self) -> Result<(), StorageError> {
        self.superblock.data_next_sector = self.data_next_sector;
        self.superblock.wal_head = self.wal.head();
        self.superblock.wal_tail = self.wal.tail();
        let data_end = self.superblock.total_sectors;
        self.superblock.free_data_sectors = data_end - self.data_next_sector;
        // Persist encryption nonce counter for crash recovery.
        if let Some(ref crypto) = self.crypto {
            self.superblock.nonce_counter = crypto.nonce_counter();
        }
        self.superblock.checksum = self.superblock.compute_checksum();
        Self::write_superblock(&self.superblock)
    }

    /// Access the WAL.
    #[allow(dead_code)]
    pub fn wal(&self) -> &Wal {
        &self.wal
    }

    /// Mutable WAL access.
    #[allow(dead_code)]
    pub fn wal_mut(&mut self) -> &mut Wal {
        &mut self.wal
    }

    /// Current data append sector.
    #[allow(dead_code)]
    pub fn data_next_sector(&self) -> u64 {
        self.data_next_sector
    }

    /// Set data append sector.
    #[allow(dead_code)]
    pub fn set_data_next_sector(&mut self, sector: u64) {
        self.data_next_sector = sector;
    }
}

// ---------------------------------------------------------------------------
// Module-level accessor functions (same pattern as FRAME_ALLOC, VIRTIO_BLK)
// ---------------------------------------------------------------------------

/// Initialize the global Block Engine.
pub fn init() -> Result<(), StorageError> {
    let engine = BlockEngine::init()?;
    *BLOCK_ENGINE.lock() = Some(engine);
    Ok(())
}

/// Write a block via the global Block Engine.
pub fn write_block(data: &[u8]) -> Result<(ContentHash, BlockLocation), StorageError> {
    let mut guard = BLOCK_ENGINE.lock();
    let engine = guard.as_mut().ok_or(StorageError::DeviceNotFound)?;
    engine.write_block(data)
}

/// Read a block by location via the global Block Engine.
#[allow(dead_code)]
pub fn read_block(loc: &BlockLocation, buf: &mut [u8]) -> Result<usize, StorageError> {
    let guard = BLOCK_ENGINE.lock();
    let engine = guard.as_ref().ok_or(StorageError::DeviceNotFound)?;
    engine.read_block(loc, buf)
}

/// Read a block by content hash via the global Block Engine.
pub fn read_block_by_hash(hash: &ContentHash, buf: &mut [u8]) -> Result<usize, StorageError> {
    let guard = BLOCK_ENGINE.lock();
    let engine = guard.as_ref().ok_or(StorageError::DeviceNotFound)?;
    engine.read_block_by_hash(hash, buf)
}

/// Flush the superblock via the global Block Engine.
pub fn flush_superblock() -> Result<(), StorageError> {
    let mut guard = BLOCK_ENGINE.lock();
    let engine = guard.as_mut().ok_or(StorageError::DeviceNotFound)?;
    engine.flush_superblock()
}

/// Access the global Block Engine under lock (for recovery / advanced operations).
pub fn with_engine<F, R>(f: F) -> Result<R, StorageError>
where
    F: FnOnce(&mut BlockEngine) -> R,
{
    let mut guard = BLOCK_ENGINE.lock();
    let engine = guard.as_mut().ok_or(StorageError::DeviceNotFound)?;
    Ok(f(engine))
}
