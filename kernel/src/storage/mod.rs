//! Storage subsystem — content-addressed block storage with WAL.
//!
//! Provides crash-safe, content-addressed storage backed by VirtIO-blk.
//! The Block Engine manages: superblock (disk metadata), WAL (crash recovery),
//! and data region (content-addressed blocks with CRC-32C integrity).
//!
//! Per spaces.md §4.

pub mod block_engine;
pub mod crypto;
pub mod lsm;
pub mod object_store;
pub mod version_store;
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

    // --- Test 5: Object Store CRUD + dedup ---
    test_object_store();

    // --- Test 6: Version Store — Merkle DAG ---
    test_version_store();

    // --- Test 7: Encryption verification ---
    test_encryption();
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

/// Version Store self-tests: create → update 3x → list 4 → rollback → verify.
#[cfg(feature = "storage-tests")]
fn test_version_store() {
    use shared::storage::{ContentType, SpaceId};

    let space = SpaceId([2u8; 16]);

    // Create an object for versioning.
    let content_v1 = b"Version 1 content";
    let (obj_id, _hash_v1) =
        match object_store::object_create(space, b"versioned.txt", content_v1, ContentType::Text) {
            Ok(r) => r,
            Err(e) => {
                crate::kerror!(Storage, "VersionStore: create failed: {:?}", e);
                return;
            }
        };
    crate::kinfo!(Storage, "VersionStore: created object {:?}", obj_id);

    // Update 3 times.
    let updates = [
        b"Version 2 content" as &[u8],
        b"Version 3 content",
        b"Version 4 content",
    ];
    for (i, content) in updates.iter().enumerate() {
        match version_store::object_update(&obj_id, content, b"test-agent", b"update") {
            Ok(_hash) => {
                crate::kinfo!(Storage, "VersionStore: update {} OK", i + 2);
            }
            Err(e) => {
                crate::kerror!(Storage, "VersionStore: update {} failed: {:?}", i + 2, e);
                return;
            }
        }
    }

    // List versions (expect 4: initial creation + 3 updates = 4 version hashes in chain).
    // Note: initial object_create computes a version_head hash but doesn't store a Version block.
    // The first version_create happens on first object_update. So we have 3 version blocks
    // in the chain. The initial version_head from object_create is not a stored Version block.
    match version_store::version_list(&obj_id) {
        Ok(versions) => {
            crate::kinfo!(Storage, "VersionStore: listed {} versions", versions.len());
            if versions.len() == 3 {
                crate::kinfo!(
                    Storage,
                    "VersionStore: version count OK (3 update versions)"
                );
            } else {
                crate::kwarn!(
                    Storage,
                    "VersionStore: expected 3 versions, got {}",
                    versions.len()
                );
            }

            // Rollback to version 2 (second in list = index 1, since newest-first).
            if versions.len() >= 2 {
                let target = &versions[1]; // Version 3 content (second-newest)
                match version_store::version_rollback(&obj_id, &target.hash) {
                    Ok(()) => {
                        crate::kinfo!(Storage, "VersionStore: rollback OK");

                        // Verify content matches the target version.
                        let mut buf = [0u8; 512];
                        match object_store::object_read(&obj_id, &mut buf) {
                            Ok((_, n)) => {
                                if &buf[..n] == b"Version 3 content" {
                                    crate::kinfo!(
                                        Storage,
                                        "VersionStore: rollback content verified"
                                    );
                                } else {
                                    crate::kerror!(
                                        Storage,
                                        "VersionStore: rollback content mismatch!"
                                    );
                                }
                            }
                            Err(e) => {
                                crate::kerror!(
                                    Storage,
                                    "VersionStore: read after rollback failed: {:?}",
                                    e
                                );
                            }
                        }

                        // After rollback, version list should have 4 (3 original + 1 rollback).
                        match version_store::version_list(&obj_id) {
                            Ok(post_versions) => {
                                crate::kinfo!(
                                    Storage,
                                    "VersionStore: post-rollback {} versions",
                                    post_versions.len()
                                );
                            }
                            Err(e) => {
                                crate::kerror!(
                                    Storage,
                                    "VersionStore: post-rollback list failed: {:?}",
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        crate::kerror!(Storage, "VersionStore: rollback failed: {:?}", e);
                    }
                }
            }
        }
        Err(e) => {
            crate::kerror!(Storage, "VersionStore: list failed: {:?}", e);
        }
    }
}

/// Encryption self-test: verify blocks are encrypted on disk and readable back.
#[cfg(feature = "storage-tests")]
fn test_encryption() {
    // Write a known block.
    let plaintext = b"encryption test payload";
    let (hash, _loc) = match block_engine::write_block(plaintext) {
        Ok(r) => r,
        Err(e) => {
            crate::kerror!(Storage, "Encryption test: write failed: {:?}", e);
            return;
        }
    };

    // Read it back — should get correct plaintext.
    let mut buf = [0u8; 512];
    match block_engine::read_block_by_hash(&hash, &mut buf) {
        Ok(n) => {
            if &buf[..n] == plaintext {
                crate::kinfo!(Storage, "Encryption test: read-back verified OK");
            } else {
                crate::kerror!(Storage, "Encryption test: read-back data mismatch!");
                return;
            }
        }
        Err(e) => {
            crate::kerror!(Storage, "Encryption test: read-back failed: {:?}", e);
            return;
        }
    }

    // Verify raw sector is NOT plaintext (encryption is transparent).
    let is_encrypted = block_engine::with_engine(|engine| engine.crypto_active()).unwrap_or(false);

    if is_encrypted {
        // Read raw sector at the block's location and check it doesn't contain plaintext.
        let loc =
            block_engine::with_engine(|engine| engine.memtable().get(&hash).map(|e| e.location))
                .unwrap_or(None);

        if let Some(loc) = loc {
            let start_sector = loc.offset / shared::storage::SECTOR_SIZE as u64;
            let mut raw = [0u8; 512];
            if crate::drivers::virtio_blk::read_sector(start_sector, &mut raw).is_ok() {
                // First 12 bytes should be AES-GCM nonce, not CRC header.
                // If encrypted, bytes 0..4 should NOT equal the CRC of plaintext.
                let raw_prefix = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
                let data_crc = block_engine::crc32c(plaintext);
                if raw_prefix != data_crc {
                    crate::kinfo!(
                        Storage,
                        "Encryption test: raw sector is encrypted (not plaintext)"
                    );
                } else {
                    crate::kwarn!(Storage, "Encryption test: raw sector may not be encrypted");
                }
            }
        }
    } else {
        crate::kinfo!(
            Storage,
            "Encryption test: crypto not active (plaintext mode)"
        );
    }
}

/// Object Store self-tests: create, read-back, dedup, delete.
#[cfg(feature = "storage-tests")]
fn test_object_store() {
    use shared::storage::{ContentType, SpaceId};

    let space = SpaceId([1u8; 16]);

    // Create an object.
    let content = b"Hello, Object Store!";
    let (obj_id, hash1) =
        match object_store::object_create(space, b"hello.txt", content, ContentType::Text) {
            Ok(result) => {
                crate::kinfo!(
                    Storage,
                    "ObjStore: created id={:?} hash={:?}",
                    result.0,
                    HashPrefix(&result.1)
                );
                result
            }
            Err(e) => {
                crate::kerror!(Storage, "ObjStore: create failed: {:?}", e);
                return;
            }
        };

    // Read it back.
    let mut buf = [0u8; 512];
    match object_store::object_read(&obj_id, &mut buf) {
        Ok((obj, n)) => {
            if &buf[..n] == content {
                crate::kinfo!(
                    Storage,
                    "ObjStore: read OK — name_len={} content_size={}",
                    obj.name_len,
                    obj.content_size
                );
            } else {
                crate::kerror!(Storage, "ObjStore: read data mismatch!");
            }
        }
        Err(e) => {
            crate::kerror!(Storage, "ObjStore: read failed: {:?}", e);
            return;
        }
    }

    // Dedup: create another object with same content.
    let (obj_id2, hash2) =
        match object_store::object_create(space, b"hello_copy.txt", content, ContentType::Text) {
            Ok(result) => result,
            Err(e) => {
                crate::kerror!(Storage, "ObjStore: dedup create failed: {:?}", e);
                return;
            }
        };

    if hash1 == hash2 {
        // Check refcount — should be 3 (original write + dedup in write_block + second object).
        let rc =
            block_engine::with_engine(|e| e.memtable().get(&hash1).map(|entry| entry.refcount))
                .unwrap_or(None);
        crate::kinfo!(
            Storage,
            "ObjStore: dedup verified — same hash, refcount={}",
            rc.unwrap_or(0)
        );
    } else {
        crate::kerror!(Storage, "ObjStore: dedup failed — different hashes!");
    }

    // Delete first object — content block should still exist (refcount > 0).
    match object_store::object_delete(&obj_id) {
        Ok(()) => {
            crate::kinfo!(Storage, "ObjStore: deleted first object");
        }
        Err(e) => {
            crate::kerror!(Storage, "ObjStore: delete failed: {:?}", e);
            return;
        }
    }

    // Second object should still be readable.
    match object_store::object_read(&obj_id2, &mut buf) {
        Ok((_, n)) => {
            if &buf[..n] == content {
                crate::kinfo!(
                    Storage,
                    "ObjStore: second object still readable after delete"
                );
            } else {
                crate::kerror!(
                    Storage,
                    "ObjStore: second object data mismatch after delete!"
                );
            }
        }
        Err(e) => {
            crate::kerror!(Storage, "ObjStore: read after delete failed: {:?}", e);
        }
    }

    // Object count check.
    let count = block_engine::with_engine(|e| e.object_index().count()).unwrap_or(0);
    crate::kinfo!(Storage, "ObjStore: {} objects in index", count);
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
