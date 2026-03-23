//! LSM-tree MemTable — re-export from shared crate.
//!
//! The pure data structure lives in `shared::storage` for host-side unit testing.
//! Kernel code imports MemTable/MemTableEntry via `shared::storage::*` in block_engine.rs.
