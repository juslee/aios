---
author: claude
date: 2026-03-23
tags: [storage, posix, compression, budget, refactoring]
status: in-progress
phase: 4
milestone: M15
---

# Plan: Phase 4 M15 — POSIX Bridge, Compression, Budget & Shared Refactoring

## Approach

Phase 4 M13-M14 built the full storage stack: VirtIO-blk driver, Block Engine (WAL, LSM-tree MemTable, CRC-32C), Object Store (content-addressed, dedup), Version Store (Merkle DAG), device-level AES-256-GCM encryption, and Space management (3 system spaces at boot). M15 adds the final three layers: POSIX compatibility bridge, LZ4 block compression, and storage budget enforcement, then runs end-to-end validation, and finally refactors pure data structures to shared/ with host-side unit tests.

**Key gap found during exploration:** The ObjectIndex is keyed by `ObjectId` only — there's no `find_by_name(space_id, name)` method. The SpaceTable similarly has no `find_by_name(name)` lookup. The POSIX bridge needs both. These will be added as part of Step 9.

**Shared crate refactoring (end of milestone):** Several pure data structures from Phase 4 (M13-M15) are currently trapped in `kernel/` where they can only be tested via QEMU. After all features are working, we move them to `shared/` with comprehensive host-side unit tests. This covers ALL of Phase 4, not just M15.

## Progress

- [ ] Step 9: POSIX Bridge — path mapping and file operations
  - [ ] 9a: Add shared POSIX types to `shared/src/storage.rs`
  - [ ] 9b: Add `find_by_name`, `list_by_space` to ObjectIndex; `find_by_name` to SpaceTable
  - [ ] 9c: Create `posix_bridge.rs` with PosixSpaceBridge + all operations
  - [ ] 9d: Add POSIX self-tests to `mod.rs`
  - [ ] 9e: Verify: `just check` + `just test` + `just run`
- [ ] Step 10: LZ4 block-level compression
  - [ ] 10a: Add `lz4_flex` to Cargo.toml, `CompressionType` to shared types
  - [ ] 10b: Integrate compression into write/read paths
  - [ ] 10c: Add compression self-test
  - [ ] 10d: Verify: `just check` + `just test` + `just run`
- [ ] Step 11: Storage budget and quota enforcement
  - [ ] 11a: Add `StorageBudget`/`PressureLevel` to shared types; create `budget.rs`
  - [ ] 11b: Wire quota enforcement into object_create
  - [ ] 11c: Add budget self-test
  - [ ] 11d: Verify: `just check` + `just test` + `just run`
- [ ] Step 12: End-to-end validation and quality gates
  - [ ] 12a: Add end-to-end POSIX test to self-tests
  - [ ] 12b: Update CLAUDE.md, phase doc, developer-guide.md
  - [ ] 12c: Run full audit loop
  - [ ] 12d: Verify all gates
- [ ] Step 13: Shared crate refactoring (ALL Phase 4 data structures)
  - [ ] 13a: Move CRC-32C to shared + tests
  - [ ] 13b: Move MemTable to shared + tests
  - [ ] 13c: Move ObjectIndex to shared + tests
  - [ ] 13d: Move SpaceTable to shared + tests
  - [ ] 13e: Move WalEntry struct to shared + tests
  - [ ] 13f: Verify: `just check` + `just test` + `just run`

## Issues Encountered

(none yet)

## Decisions Made

- Shared crate refactoring placed at END of milestone (Step 13), not beginning. Reason: implement features first in kernel where QEMU validates behavior, then refactor to shared. Safer approach.
- Pure data structures identified for sharing: CRC-32C, MemTable, ObjectIndex, SpaceTable, WalEntry struct. Non-sharable: Wal I/O (virtio_blk), ID generators (TICK_COUNT), with_engine operations.

## Lessons Learned

(to be filled during implementation)
