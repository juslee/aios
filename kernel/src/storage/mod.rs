//! Storage subsystem — content-addressed block storage with WAL.
//!
//! Provides crash-safe, content-addressed storage backed by VirtIO-blk.
//! The Block Engine manages: superblock (disk metadata), WAL (crash recovery),
//! and data region (content-addressed blocks with CRC-32C integrity).
//!
//! Per spaces.md §4.

pub mod block_engine;
pub mod lsm;
pub mod wal;

use shared::storage::ContentHash;

/// Initialize the storage subsystem.
///
/// Called from kernel_main after service::init() and VirtIO-blk probe.
/// Initializes Block Engine (superblock + WAL + MemTable), runs comprehensive tests.
pub fn init() {
    crate::kinfo!(Storage, "Storage subsystem initializing...");
    crate::observability::drain_logs();

    // Initialize Block Engine (reads or formats superblock, inits WAL, replays recovery).
    if let Err(e) = block_engine::init() {
        crate::kerror!(Storage, "Block Engine init failed: {:?}", e);
        return;
    }

    // Log MemTable stats after init (recovery may have populated it).
    let (mt_count, mt_cap) =
        block_engine::with_engine(|e| (e.memtable().count(), MEMTABLE_MAX_ENTRIES))
            .unwrap_or((0, 0));
    crate::kinfo!(
        Storage,
        "Block Engine initialized (MemTable: {} / {})",
        mt_count,
        mt_cap
    );

    // Self-tests: only run during development, gated behind feature flag.
    #[cfg(feature = "storage-tests")]
    run_self_tests();

    // Log final MemTable stats.
    let final_count = block_engine::with_engine(|e| e.memtable().count()).unwrap_or(0);
    crate::kinfo!(
        Storage,
        "MemTable: {} entries / {} capacity",
        final_count,
        MEMTABLE_MAX_ENTRIES
    );

    // Flush superblock to persist state.
    if let Err(e) = block_engine::flush_superblock() {
        crate::kerror!(Storage, "Superblock flush failed: {:?}", e);
    }
}

/// Boot-time self-tests for the storage subsystem (write/read/dedup/100-block).
/// Gated behind `storage-tests` feature to avoid mutating disk on every production boot.
#[cfg(feature = "storage-tests")]
fn run_self_tests() {
    // --- Test 1: Write + read round-trip ---
    let test_data = b"Hello, AIOS!";
    let (hash1, _loc1) = match block_engine::write_block(test_data) {
        Ok(result) => {
            crate::kinfo!(
                Storage,
                "Test: wrote {} bytes — hash={:?}",
                test_data.len(),
                HashPrefix(&result.0)
            );
            result
        }
        Err(e) => {
            crate::kerror!(Storage, "Test: write failed: {:?}", e);
            return;
        }
    };

    // Read back by hash (MemTable lookup path).
    let mut read_buf = [0u8; 512];
    match block_engine::read_block_by_hash(&hash1, &mut read_buf) {
        Ok(n) => {
            if &read_buf[..n] == test_data {
                crate::kinfo!(Storage, "Test: read-by-hash verified OK ({} bytes)", n);
            } else {
                crate::kerror!(Storage, "Test: read-by-hash data mismatch!");
            }
        }
        Err(e) => crate::kerror!(Storage, "Test: read-by-hash failed: {:?}", e),
    }

    // --- Test 2: Dedup (write same data again, should bump refcount) ---
    match block_engine::write_block(test_data) {
        Ok((hash2, _loc2)) => {
            if hash2.0 == hash1.0 {
                // Check refcount via with_engine.
                let rc = block_engine::with_engine(|e| {
                    e.memtable().get(&hash1).map(|entry| entry.refcount)
                })
                .unwrap_or(None);
                crate::kinfo!(
                    Storage,
                    "Test: dedup hit — hash={:?} refcount={}",
                    HashPrefix(&hash1),
                    rc.unwrap_or(0)
                );
            } else {
                crate::kerror!(Storage, "Test: dedup failed — different hash!");
            }
        }
        Err(e) => crate::kerror!(Storage, "Test: dedup write failed: {:?}", e),
    }

    // --- Test 3: Write different content ---
    let test_data2 = b"Different content";
    match block_engine::write_block(test_data2) {
        Ok((hash2, _)) => {
            crate::kinfo!(
                Storage,
                "Test: wrote {} bytes — hash={:?}",
                test_data2.len(),
                HashPrefix(&hash2)
            );

            let mut buf2 = [0u8; 512];
            match block_engine::read_block_by_hash(&hash2, &mut buf2) {
                Ok(n) => {
                    if &buf2[..n] == test_data2 {
                        crate::kinfo!(Storage, "Test: read hash2 verified OK ({} bytes)", n);
                    } else {
                        crate::kerror!(Storage, "Test: read hash2 data mismatch!");
                    }
                }
                Err(e) => crate::kerror!(Storage, "Test: read hash2 failed: {:?}", e),
            }
        }
        Err(e) => crate::kerror!(Storage, "Test: write2 failed: {:?}", e),
    }

    // --- Test 4: 100-block write/read ---
    test_100_blocks();
}

/// Write 100 unique blocks and verify all are readable.
fn test_100_blocks() {
    let mut ok_count = 0u32;
    let mut fail_count = 0u32;

    for i in 0u32..100 {
        // Generate unique content: "block-NNN" with zero-padded index.
        let mut content = [0u8; 16];
        let prefix = b"block-";
        content[..6].copy_from_slice(prefix);
        // Write index as 3 decimal digits.
        content[6] = b'0' + ((i / 100) % 10) as u8;
        content[7] = b'0' + ((i / 10) % 10) as u8;
        content[8] = b'0' + (i % 10) as u8;
        let data = &content[..9]; // "block-NNN"

        match block_engine::write_block(data) {
            Ok((hash, _loc)) => {
                let mut buf = [0u8; 512];
                match block_engine::read_block_by_hash(&hash, &mut buf) {
                    Ok(n) => {
                        if &buf[..n] == data {
                            ok_count += 1;
                        } else {
                            fail_count += 1;
                        }
                    }
                    Err(_) => fail_count += 1,
                }
            }
            Err(_) => fail_count += 1,
        }
    }

    if fail_count == 0 {
        crate::kinfo!(Storage, "Test: 100-block write/read — all {} OK", ok_count);
    } else {
        crate::kerror!(
            Storage,
            "Test: 100-block write/read — {} OK, {} FAILED",
            ok_count,
            fail_count
        );
    }
}

use shared::storage::MEMTABLE_MAX_ENTRIES;

/// Helper to display first 8 bytes of a content hash.
struct HashPrefix<'a>(&'a ContentHash);

impl<'a> core::fmt::Debug for HashPrefix<'a> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for &b in &self.0 .0[..8] {
            write!(f, "{:02x}", b)?;
        }
        write!(f, "...")
    }
}
