# Plan: Phase 5 M18 — Storage Kit & Gate

## Context

Phase 5 (Kit Foundation) extracts SDK-facing trait hierarchies from Phases 0–4 implementations. M16 delivered Memory Kit (3 traits) + Capability Kit (1 trait). M17 delivered IPC Kit (4 traits). **M18 is the final milestone** — it defines 4 Storage Kit traits, implements them via zero-sized kernel wrappers, updates all Kit docs, and runs the triple audit before PR.

After M18 merges, Phase 5 is complete and Phase 6 (GPU & Display) can begin.

## Approach

**Pattern:** Follow the exact same zero-sized wrapper pattern from M16/M17:
- Traits defined in `shared/src/kits/storage.rs` (host-testable, no hardware deps)
- Error type: reuse existing `StorageError` from `shared/src/storage.rs` (already has 19 variants — no new error enum needed, unlike Memory/IPC Kits which needed domain-specific errors)
- 4 kernel wrapper structs delegate to existing module-level functions via `with_engine()`
- 4 new functions that don't exist yet (`block_exists`, `list_objects`, `get_head`, `pressure_level`) are implemented directly in the wrapper `impl` blocks

**Key design difference from M16/M17:** Storage Kit reuses `StorageError` directly rather than defining a new `StorageKitError`. The existing `StorageError` already has 19 well-defined, Copy-compatible variants covering all storage failure modes. Creating a parallel error type would add no value — unlike IPC where `IpcError` was a flat syscall-level enum needing richer context fields.

**Shared crate:** No refactoring needed — all shared storage types already live in `shared/src/storage.rs`. The Kit module just re-exports them.

## Progress

### Step 11: Storage Kit trait definitions
- [ ] 11a: Create `shared/src/kits/storage.rs` with module doc comment and imports
- [ ] 11b: Define `BlockStore` trait (3 methods: `write_block`, `read_block`, `block_exists`)
  - `write_block(&mut self, data: &[u8]) -> Result<BlockId, StorageError>`
  - `read_block(&self, id: &BlockId) -> Result<Vec<u8>, StorageError>`
  - `block_exists(&self, id: &BlockId) -> bool`
- [ ] 11c: Define `SpaceManager` trait (6 methods)
  - `create_space(&mut self, name: &str, zone: SecurityZone) -> Result<SpaceId, StorageError>`
  - `get_space(&self, id: &SpaceId) -> Result<Space, StorageError>`
  - `list_spaces(&self) -> Vec<Space>`
  - `delete_space(&mut self, id: &SpaceId) -> Result<(), StorageError>`
  - `storage_budget(&self) -> StorageBudget`
  - `pressure_level(&self) -> PressureLevel`
- [ ] 11d: Define `ObjectStore` trait (4 methods)
  - `create_object(&mut self, space_id: &SpaceId, name: &str, content_type: ContentType, data: &[u8]) -> Result<ObjectId, StorageError>`
  - `read_object(&self, id: &ObjectId) -> Result<Vec<u8>, StorageError>`
  - `delete_object(&mut self, id: &ObjectId) -> Result<(), StorageError>`
  - `list_objects(&self, space_id: &SpaceId) -> Vec<CompactObject>`
- [ ] 11e: Define `VersionStoreOps` trait (4 methods)
  - `create_version(&mut self, object_id: &ObjectId, data: &[u8], message: &str) -> Result<ContentHash, StorageError>`
  - `list_versions(&self, object_id: &ObjectId) -> Vec<Version>`
  - `get_head(&self, object_id: &ObjectId) -> Result<ContentHash, StorageError>`
  - `rollback(&mut self, object_id: &ObjectId, version_hash: &ContentHash) -> Result<ContentHash, StorageError>`
- [ ] 11f: Re-export key storage types from Kit module (StorageError, Space, CompactObject, Version, etc.)
- [ ] 11g: Add `pub mod storage;` to `shared/src/kits/mod.rs`
- [ ] 11h: Add `pub use kits::storage as storage_kit;` to `shared/src/lib.rs`
- [ ] 11i: Write host-side tests: all 4 traits are dyn-compatible (compile-time assertion functions)
- [ ] 11j: Verify: `just check` + `just test`

### Step 12: Implement Storage Kit traits on kernel types
- [ ] 12a: Create `KernelBlockStore` unit struct in `kernel/src/storage/block_engine.rs`
  - `write_block()` → delegates to `write_block()` (returns `BlockId` = first element of tuple)
  - `read_block()` → delegates to `read_block_by_hash()` (allocates Vec, copies data)
  - `block_exists()` → new: use `with_engine()` to check ObjectIndex/MemTable for hash
- [ ] 12b: Create `KernelSpaceManager` unit struct in `kernel/src/storage/space.rs`
  - Delegates to `space_create()`, `space_get()`, `space_list()`, `space_delete()`
  - `storage_budget()` → delegates to `budget::storage_stats()`
  - `pressure_level()` → delegates to `budget::check_pressure()`
- [ ] 12c: Create `KernelObjectStore` unit struct in `kernel/src/storage/object_store.rs`
  - Delegates to `object_create()`, `object_read()`, `object_delete()`
  - `list_objects()` → new: use `with_engine()` to iterate ObjectIndex, filter by SpaceId, collect CompactObjects
- [ ] 12d: Create `KernelVersionStore` unit struct in `kernel/src/storage/version_store.rs`
  - Delegates to `version_create()`, `version_list()`, `version_rollback()`
  - `get_head()` → new: use `with_engine()` to look up ObjectIndex entry's `version_head` field
- [ ] 12e: Verify: `just check` + `just run` (storage self-tests produce identical UART output)

### Step 13: Kit doc updates, CLAUDE.md, and cross-references
- [ ] 13a: Update `docs/kits/platform/storage.md` — verify trait signatures match `shared/src/kits/storage.rs`
- [ ] 13b: Verify `docs/kits/kernel/memory.md`, `ipc.md`, `capability.md` — trait signatures still match code
- [ ] 13c: Update `docs/kits/README.md` — mark all 4 Kits as "Traits Defined"
- [ ] 13d: Update `CLAUDE.md` — Workspace Layout (add storage.rs to kits), Key Technical Facts (Kit counts, wrapper names)
- [ ] 13e: Update `README.md` project structure
- [ ] 13f: Update `docs/project/development-plan.md` Phase 5 row
- [ ] 13g: Update phase doc `05-kit-foundation.md` — check off tasks, Status → Complete
- [ ] 13h: Verify: cross-references resolve, no stale signatures

### Step 14: Full audit and quality gate
- [ ] 14a: Run doc audit
- [ ] 14b: Run code review
- [ ] 14c: Run security/bug review
- [ ] 14d: Fix issues, commit, re-run — loop until 0 issues
- [ ] 14e: Verify all quality gates: `just check`, `just test`, `just run`, CI
- [ ] 14f: Create PR to `main`

## Code Structure Decisions

- **Reuse `StorageError` directly (no `StorageKitError`):** Unlike Memory/IPC Kits which needed richer error types than their syscall-level counterparts, `StorageError` already has 19 domain-specific variants with no field data (all `Copy`). A parallel Kit error type would be redundant. If field-rich errors become needed later, we can add `StorageKitError` at that point.

- **`alloc::vec::Vec` in trait return types:** Storage Kit traits return `Vec<u8>`, `Vec<Space>`, `Vec<CompactObject>`, `Vec<Version>`. This requires `extern crate alloc` in `shared/`. The shared crate already has `alloc` enabled (used by IPC Kit's `SelectEntry` and other collections). Returning `Vec` is cleaner than requiring callers to pass pre-sized buffers for variable-length data.

- **`list_objects` scans ObjectIndex:** No existing kernel function lists objects by space. The wrapper will use `with_engine()` to access the `ObjectIndex` (sorted Vec of `ObjectIndexEntry`) and filter by `SpaceId`. This is O(n) but adequate for M18's max 16384 entries.

- **`block_exists` checks MemTable:** The kernel's `MemTable` (sorted Vec with binary search) maps `ContentHash → BlockLocation`. A lookup returning `Some(_)` confirms existence. Uses `with_engine()`.

- **`get_head` reads ObjectIndex entry:** Each `ObjectIndexEntry` stores `version_head: ContentHash`. The wrapper looks up the ObjectId in the index and returns this field.

- **`pressure_level` delegates to `budget::check_pressure()`:** This function already exists and returns `PressureLevel`.

## Dependencies & Risks

- **Depends on:** M16 + M17 complete (confirmed), all storage types in `shared/src/storage.rs`, all kernel storage functions in `kernel/src/storage/`
- **Risk: `with_engine()` lock contention** — All 4 kernel wrappers use `with_engine()` which acquires a global `Mutex<BlockEngine>`. This is fine for Phase 5 (single-threaded storage access in self-tests) but may need refactoring for concurrent access in later phases.
- **Risk: `Vec` allocation in `no_std` kernel** — Returning `Vec` from Kit trait methods requires the kernel's global allocator (slab-backed). This works because the allocator is initialized before storage init. No risk for Phase 5.

## Phase Doc Reconciliation

The phase doc Step 11 lists `[x]` for the dyn-compatibility test item — this appears pre-checked (likely a typo). Will verify and uncheck if needed during implementation.

No other changes needed — the phase doc is well-aligned with this plan.

## Issues Encountered

(to be filled during implementation)

## Decisions Made

(to be filled during implementation)

## Lessons Learned

(to be filled during implementation)
