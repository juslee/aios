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
/// Initializes Block Engine (superblock + WAL), runs write/read test.
pub fn init() {
    // Initialize Block Engine (reads or formats superblock, inits WAL).
    if let Err(e) = block_engine::init() {
        crate::kerror!(Storage, "Block Engine init failed: {:?}", e);
        return;
    }
    crate::kinfo!(Storage, "Block Engine initialized");

    // Write/read test: store a small block and verify round-trip.
    let test_data = b"Hello, AIOS!";
    match block_engine::write_block(test_data) {
        Ok((hash, loc)) => {
            crate::kinfo!(
                Storage,
                "Test: wrote {} bytes — hash={:?}",
                test_data.len(),
                HashPrefix(&hash)
            );

            let mut read_buf = [0u8; 512];
            match block_engine::read_block(&loc, &mut read_buf) {
                Ok(n) => {
                    if &read_buf[..n] == test_data {
                        crate::kinfo!(Storage, "Test: read verified OK ({} bytes)", n);
                    } else {
                        crate::kerror!(Storage, "Test: read data mismatch!");
                    }
                }
                Err(e) => crate::kerror!(Storage, "Test: read failed: {:?}", e),
            }
        }
        Err(e) => crate::kerror!(Storage, "Test: write failed: {:?}", e),
    }

    // Flush superblock to persist state.
    if let Err(e) = block_engine::flush_superblock() {
        crate::kerror!(Storage, "Superblock flush failed: {:?}", e);
    }
}

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
