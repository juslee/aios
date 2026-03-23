//! Storage budget — global storage usage tracking and pressure monitoring.
//!
//! Aggregates usage from Block Engine superblock, MemTable, ObjectIndex,
//! and WAL to provide a system-wide storage budget summary. Pressure
//! levels guide admission control and back-pressure decisions.
//!
//! Per spaces.md §10.

use shared::storage::{PressureLevel, StorageBudget, SECTOR_SIZE};

use super::block_engine;

/// Compute the current storage budget from Block Engine state.
pub fn storage_stats() -> Result<StorageBudget, shared::storage::StorageError> {
    block_engine::with_engine(|engine| {
        let sb = engine.superblock();
        let total_data_sectors = sb.total_sectors.saturating_sub(sb.data_start_sector);
        let used_data_sectors = engine
            .data_next_sector()
            .saturating_sub(sb.data_start_sector);
        let free_data_sectors = total_data_sectors.saturating_sub(used_data_sectors);

        let total_bytes = total_data_sectors * SECTOR_SIZE as u64;
        let used_bytes = used_data_sectors * SECTOR_SIZE as u64;
        let free_bytes = free_data_sectors * SECTOR_SIZE as u64;

        let data_blocks = engine.memtable().count() as u64;

        // head and tail are monotonically increasing entry indices (never wrap).
        let wal_used = sb.wal_head.saturating_sub(sb.wal_tail);

        let index_entries = engine.object_index().count() as u64;

        StorageBudget {
            total_bytes,
            used_bytes,
            free_bytes,
            data_blocks,
            wal_used,
            index_entries,
        }
    })
}

/// Compute the current storage pressure level.
pub fn check_pressure() -> Result<PressureLevel, shared::storage::StorageError> {
    let budget = storage_stats()?;
    let pct = (budget.free_bytes * 100)
        .checked_div(budget.total_bytes)
        .unwrap_or(0);
    Ok(PressureLevel::from_free_percentage(pct))
}
