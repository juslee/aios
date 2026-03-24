//! Storage subsystem — content-addressed block storage with WAL.
//!
//! Provides crash-safe, content-addressed storage backed by VirtIO-blk.
//! The Block Engine manages: superblock (disk metadata), WAL (crash recovery),
//! and data region (content-addressed blocks with CRC-32C integrity).
//!
//! Per spaces.md §4.

pub mod block_engine;
pub mod budget;
pub mod crypto;
pub mod lsm;
pub mod object_store;
pub mod posix_bridge;
pub mod space;
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

    // Initialize system spaces and register as a service.
    space::init_system_spaces();
    space::register_service();

    // Log space count.
    let space_count = block_engine::with_engine(|e| e.space_table().count()).unwrap_or(0);
    crate::kinfo!(Storage, "Spaces: {} active", space_count);

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

    // --- Test 8: Space management ---
    test_spaces();

    // --- Test 9: POSIX bridge ---
    test_posix_bridge();

    // --- Test 10: LZ4 compression ---
    test_compression();

    // --- Test 11: Storage budget ---
    test_budget();
}

/// Write 100 unique blocks and verify all are readable.
#[cfg(feature = "storage-tests")]
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
    use shared::storage::ContentType;

    // Use a real system space ID.
    let space = match space::space_list() {
        Ok(spaces) if !spaces.is_empty() => spaces[0].id,
        _ => {
            crate::kerror!(Storage, "VersionStore: no spaces available for test");
            return;
        }
    };

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

    // List versions (expect 4: initial creation + 3 updates = 4 version blocks in chain).
    match version_store::version_list(&obj_id) {
        Ok(versions) => {
            crate::kinfo!(Storage, "VersionStore: listed {} versions", versions.len());
            if versions.len() == 4 {
                crate::kinfo!(
                    Storage,
                    "VersionStore: version count OK (1 initial + 3 updates)"
                );
            } else {
                crate::kwarn!(
                    Storage,
                    "VersionStore: expected 4 versions, got {}",
                    versions.len()
                );
            }

            // Rollback to version 2 (second in list = index 1, since newest-first).
            if versions.len() >= 2 {
                let target = &versions[1]; // Version 3 content (second-newest)
                match version_store::version_rollback(&obj_id, &target.hash) {
                    Ok(_content_hash) => {
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

                        // After rollback, version list should have 5 (4 original + 1 rollback).
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

/// Space management self-tests: verify system spaces, create user space, list.
#[cfg(feature = "storage-tests")]
fn test_spaces() {
    use shared::storage::{SecurityZone, SpaceQuota};

    // System spaces should already exist (created in init).
    match space::space_list() {
        Ok(spaces) => {
            crate::kinfo!(Storage, "SpaceTest: {} spaces exist", spaces.len());
            if spaces.len() >= 3 {
                crate::kinfo!(Storage, "SpaceTest: system spaces OK");
            } else {
                crate::kerror!(
                    Storage,
                    "SpaceTest: expected >= 3 system spaces, got {}",
                    spaces.len()
                );
            }
        }
        Err(e) => {
            crate::kerror!(Storage, "SpaceTest: list failed: {:?}", e);
            return;
        }
    }

    // Create a user space.
    let quota = SpaceQuota {
        max_bytes: 1024 * 1024,
        max_objects: 100,
        _padding: [0; 4],
    };
    match space::space_create(b"test-space", SecurityZone::Personal, quota) {
        Ok(id) => {
            crate::kinfo!(Storage, "SpaceTest: created user space {:?}", id);

            // Get it back.
            match space::space_get(&id) {
                Ok(s) => {
                    if s.name_bytes() == b"test-space" {
                        crate::kinfo!(Storage, "SpaceTest: get verified OK");
                    } else {
                        crate::kerror!(Storage, "SpaceTest: name mismatch!");
                    }
                }
                Err(e) => crate::kerror!(Storage, "SpaceTest: get failed: {:?}", e),
            }

            // Delete it (it's empty).
            match space::space_delete(&id) {
                Ok(()) => crate::kinfo!(Storage, "SpaceTest: delete OK"),
                Err(e) => crate::kerror!(Storage, "SpaceTest: delete failed: {:?}", e),
            }
        }
        Err(e) => {
            crate::kerror!(Storage, "SpaceTest: create failed: {:?}", e);
        }
    }

    // Final count.
    let count = block_engine::with_engine(|e| e.space_table().count()).unwrap_or(0);
    crate::kinfo!(Storage, "SpaceTest: {} spaces after test", count);
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
                let data_crc = shared::storage::crc32c(plaintext);
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
    use shared::storage::ContentType;

    // Use a real system space ID (created by init_system_spaces).
    let space = match space::space_list() {
        Ok(spaces) if !spaces.is_empty() => spaces[0].id,
        _ => {
            crate::kerror!(Storage, "ObjStore: no spaces available for test");
            return;
        }
    };

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
        // Check refcount — should be 2 (one per object via write_block dedup).
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

/// POSIX bridge self-tests: open, write, read, stat, readdir, unlink.
#[cfg(feature = "storage-tests")]
fn test_posix_bridge() {
    use shared::storage::posix_flags;

    let mut bridge = posix_bridge::PosixSpaceBridge::new();

    // Create a file via POSIX open (O_CREAT | O_WRONLY).
    let fd = match bridge.open(
        b"/home/user/test.txt",
        posix_flags::O_CREAT | posix_flags::O_WRONLY,
    ) {
        Ok(fd) => {
            crate::kinfo!(Storage, "POSIX: open(create) OK — fd={}", fd);
            fd
        }
        Err(e) => {
            crate::kerror!(Storage, "POSIX: open(create) failed: {:?}", e);
            return;
        }
    };

    // Write data.
    match bridge.write(fd, b"Hello, AIOS!") {
        Ok(n) => crate::kinfo!(Storage, "POSIX: write OK — {} bytes", n),
        Err(e) => {
            crate::kerror!(Storage, "POSIX: write failed: {:?}", e);
            return;
        }
    }

    // Close.
    if let Err(e) = bridge.close(fd) {
        crate::kerror!(Storage, "POSIX: close failed: {:?}", e);
        return;
    }

    // Re-open for reading.
    let fd2 = match bridge.open(b"/home/user/test.txt", posix_flags::O_RDONLY) {
        Ok(fd) => fd,
        Err(e) => {
            crate::kerror!(Storage, "POSIX: open(read) failed: {:?}", e);
            return;
        }
    };

    // Read back.
    let mut buf = [0u8; 128];
    match bridge.read(fd2, &mut buf) {
        Ok(n) => {
            if &buf[..n] == b"Hello, AIOS!" {
                crate::kinfo!(Storage, "POSIX: read verified OK — {} bytes", n);
            } else {
                crate::kerror!(Storage, "POSIX: read data mismatch!");
            }
        }
        Err(e) => {
            crate::kerror!(Storage, "POSIX: read failed: {:?}", e);
            return;
        }
    }

    let _ = bridge.close(fd2);

    // Stat.
    match bridge.stat(b"/home/user/test.txt") {
        Ok(stat) => {
            crate::kinfo!(
                Storage,
                "POSIX: stat OK — size={}, mode=0o{:o}",
                stat.size,
                stat.mode
            );
        }
        Err(e) => crate::kerror!(Storage, "POSIX: stat failed: {:?}", e),
    }

    // Readdir.
    match bridge.readdir(b"/home/user") {
        Ok(entries) => {
            crate::kinfo!(Storage, "POSIX: readdir OK — {} entries", entries.len());
        }
        Err(e) => crate::kerror!(Storage, "POSIX: readdir failed: {:?}", e),
    }

    // Stat directory root.
    match bridge.stat(b"/home/user") {
        Ok(stat) => {
            crate::kinfo!(Storage, "POSIX: stat dir OK — mode=0o{:o}", stat.mode);
        }
        Err(e) => crate::kerror!(Storage, "POSIX: stat dir failed: {:?}", e),
    }

    // Unlink.
    match bridge.unlink(b"/home/user/test.txt") {
        Ok(()) => crate::kinfo!(Storage, "POSIX: unlink OK"),
        Err(e) => crate::kerror!(Storage, "POSIX: unlink failed: {:?}", e),
    }
}

/// LZ4 compression self-test: write compressible data, verify round-trip.
#[cfg(feature = "storage-tests")]
fn test_compression() {
    // Create highly compressible data (repeated pattern).
    let mut compressible = [0u8; 256];
    for (i, byte) in compressible.iter_mut().enumerate() {
        *byte = b"AIOS-compress-test-data!"[i % 24];
    }

    // Write it — should trigger LZ4 compression (high redundancy).
    match block_engine::write_block(&compressible) {
        Ok((hash, _loc)) => {
            // Read back — should decompress transparently.
            let mut buf = [0u8; 512];
            match block_engine::read_block_by_hash(&hash, &mut buf) {
                Ok(n) => {
                    if n == 256 && buf[..256] == compressible {
                        crate::kinfo!(
                            Storage,
                            "Compression: 256B compressible data — round-trip OK"
                        );
                    } else {
                        crate::kerror!(Storage, "Compression: data mismatch (got {} bytes)", n);
                    }
                }
                Err(e) => crate::kerror!(Storage, "Compression: read failed: {:?}", e),
            }

            // Check on-disk size (wrapped_len in BlockLocation).
            let on_disk = block_engine::with_engine(|engine| {
                engine.memtable().get(&hash).map(|e| e.location.size)
            })
            .unwrap_or(None);

            if let Some(disk_size) = on_disk {
                // disk_size includes 5-byte compression header + payload.
                // If compressed, payload < 256; if not, payload = 256.
                let payload_size = disk_size as usize - shared::storage::COMPRESSION_HEADER_SIZE;
                let ratio = payload_size * 100 / 256;
                crate::kinfo!(
                    Storage,
                    "Compression: on-disk {}B ({}% of original 256B)",
                    disk_size,
                    ratio
                );
            }
        }
        Err(e) => crate::kerror!(Storage, "Compression: write failed: {:?}", e),
    }

    // Write incompressible data (should skip compression).
    // Use a simple pseudo-random sequence.
    let mut random_data = [0u8; 64];
    for (i, byte) in random_data.iter_mut().enumerate() {
        *byte = ((i * 131 + 17) % 256) as u8;
    }

    match block_engine::write_block(&random_data) {
        Ok((hash, _loc)) => {
            let mut buf = [0u8; 512];
            match block_engine::read_block_by_hash(&hash, &mut buf) {
                Ok(n) => {
                    if n == 64 && buf[..64] == random_data {
                        crate::kinfo!(Storage, "Compression: 64B random data — round-trip OK");
                    } else {
                        crate::kerror!(
                            Storage,
                            "Compression: random data mismatch (got {} bytes)",
                            n
                        );
                    }
                }
                Err(e) => crate::kerror!(Storage, "Compression: random read failed: {:?}", e),
            }
        }
        Err(e) => crate::kerror!(Storage, "Compression: random write failed: {:?}", e),
    }
}

/// Test storage budget stats and quota enforcement.
#[cfg(feature = "storage-tests")]
fn test_budget() {
    use shared::storage::{ContentType, PressureLevel, SecurityZone, SpaceQuota};

    // 1. Get budget stats.
    match budget::storage_stats() {
        Ok(b) => {
            let used_pct = (b.used_bytes * 100).checked_div(b.total_bytes).unwrap_or(0);
            crate::kinfo!(
                Storage,
                "Budget: {}KB used / {}KB total ({}%), {} blocks, {} objects",
                b.used_bytes / 1024,
                b.total_bytes / 1024,
                used_pct,
                b.data_blocks,
                b.index_entries
            );
        }
        Err(e) => {
            crate::kerror!(Storage, "Budget: stats failed: {:?}", e);
            return;
        }
    }

    // 2. Check pressure level.
    match budget::check_pressure() {
        Ok(level) => {
            crate::kinfo!(Storage, "Budget: pressure={:?}", level);
            // After boot tests, most of 256MB is free → should be Normal.
            if level != PressureLevel::Normal {
                crate::kwarn!(Storage, "Budget: unexpected pressure level {:?}", level);
            }
        }
        Err(e) => crate::kerror!(Storage, "Budget: pressure check failed: {:?}", e),
    }

    // 3. Quota enforcement test: create a space with a tight quota, try to exceed it.
    let quota = SpaceQuota {
        max_bytes: 100,
        max_objects: 2,
        _padding: [0u8; 4],
    };
    match space::space_create(b"quota-test", SecurityZone::Personal, quota) {
        Ok(sid) => {
            // First object (small) should succeed.
            match object_store::object_create(sid, b"small.txt", b"hello", ContentType::Text) {
                Ok(_) => crate::kinfo!(Storage, "Budget: quota obj1 OK"),
                Err(e) => {
                    crate::kerror!(Storage, "Budget: quota obj1 failed: {:?}", e);
                    return;
                }
            }
            // Second object should succeed (under limit).
            match object_store::object_create(sid, b"med.txt", b"world!", ContentType::Text) {
                Ok(_) => crate::kinfo!(Storage, "Budget: quota obj2 OK"),
                Err(e) => {
                    crate::kerror!(Storage, "Budget: quota obj2 failed: {:?}", e);
                    return;
                }
            }
            // Third object should fail (max_objects=2, already have 2).
            match object_store::object_create(sid, b"over.txt", b"too many", ContentType::Text) {
                Ok(_) => crate::kwarn!(Storage, "Budget: quota should have rejected obj3!"),
                Err(shared::storage::StorageError::QuotaExceeded) => {
                    crate::kinfo!(Storage, "Budget: quota enforcement OK (obj3 rejected)")
                }
                Err(e) => crate::kerror!(Storage, "Budget: quota obj3 unexpected error: {:?}", e),
            }

            // Clean up: delete objects and space.
            let objs =
                block_engine::with_engine(|engine| engine.object_index().list_by_space(&sid))
                    .unwrap_or_default();
            for oid in &objs {
                let _ = object_store::object_delete(oid);
            }
            let _ = space::space_delete(&sid);
        }
        Err(e) => crate::kerror!(Storage, "Budget: quota-test space creation failed: {:?}", e),
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
