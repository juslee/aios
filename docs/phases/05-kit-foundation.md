# Phase 5: Kit Foundation

**Tier:** 1.5 — Kit Foundation
**Duration:** 3 weeks
**Deliverable:** Memory Kit, IPC Kit, Capability Kit, and Storage Kit trait hierarchies extracted from Phases 0–4 implementation; Kit module structure in `shared/src/kits/`; kernel-side `impl` blocks; comprehensive host-side tests
**Status:** Planned
**Prerequisites:** Phase 4 (Block Storage & Object Store)
**Unlocks:** Phase 6 (GPU & Display)

-----

## Objective

Phases 0–4 built a working microkernel with physical/virtual memory management, IPC channels, a capability system, a scheduler, and a block storage subsystem. The code works, but the API surface is implicit — scattered across module-level functions, struct methods, and global state. Phase 5 makes this surface explicit by extracting **Kit trait hierarchies** that formalize the contract each subsystem exposes.

AIOS adopts a BeOS-inspired Kit architecture (see `docs/kits/README.md`): 30 Kits organized in 4 strict layers (Kernel, Platform, Intelligence, Application). Phase 5 defines the first 4 Kits — all at the Kernel or Platform layer — establishing the crate structure and patterns that all subsequent phases follow. Each Kit is a collection of Rust traits in `shared/src/kits/` with corresponding `impl` blocks in `kernel/`. This separation ensures Kit traits are testable on the host (`just test`) while kernel implementations remain `aarch64-unknown-none` only.

By the end of this phase: (1) `shared/src/kits/` contains trait definitions, error types, and supporting types for Memory Kit, IPC Kit, Capability Kit, and Storage Kit; (2) `kernel/` implements all Kit traits on existing concrete types; (3) host-side tests verify trait object safety, error type round-trips, and shared type invariants; (4) Kit docs in `docs/kits/` are updated to reflect the actual trait signatures.

-----

## Architecture References

These existing documents define the technical design. This phase doc focuses on implementation order and acceptance criteria — not duplicating the architecture.

| Topic | Document | Relevant Sections |
|---|---|---|
| Kit architecture overview | [kits/README.md](../kits/README.md) | Core Insight; Design Principles; Kit Discovery and Registration; Document Map |
| Kit architecture ADR | [decisions/2026-03-22-jl-kit-architecture.md](../knowledge/decisions/2026-03-22-jl-kit-architecture.md) | Full ADR: 4-layer hierarchy, 30 Kits, organic extraction |
| Custom Core principle | [decisions/2026-03-16-jl-custom-core-principle.md](../knowledge/decisions/2026-03-16-jl-custom-core-principle.md) | Full ADR: AIOS-native implementations, open-source bridges on top |
| Memory Kit API surface | [kits/kernel/memory.md](../kits/kernel/memory.md) | Core Traits (FrameAllocator, AddressSpace); Error Handling |
| IPC Kit API surface | [kits/kernel/ipc.md](../kits/kernel/ipc.md) | Core Traits (Channel, Notification, Select); Error Handling |
| Capability Kit API surface | [kits/kernel/capability.md](../kits/kernel/capability.md) | Core Traits (CapabilityEnforcer); Error Handling |
| Storage Kit API surface | [kits/platform/storage.md](../kits/platform/storage.md) | Core Traits (Space, Object, VersionStore); Error Handling |
| Memory management architecture | [kernel/memory.md](../kernel/memory.md) | §1 Overview; §14 Implementation Order |
| Physical memory (buddy, pools, frames) | [kernel/memory/physical.md](../kernel/memory/physical.md) | §2.2 BuddyAllocator; §2.3 FrameAllocator; §2.4 PagePools |
| Virtual memory (page tables, ASID) | [kernel/memory/virtual.md](../kernel/memory/virtual.md) | §3.2 PageTableEntry; §3.4 TLB/ASID; §5 Per-Agent Memory |
| IPC channels and syscalls | [kernel/ipc.md](../kernel/ipc.md) | §2 Channel Model; §3 Notifications; §4 Select; §5 Shared Memory |
| Capability system internals | [security/model/capabilities.md](../security/model/capabilities.md) | §3.1–§3.6 Token lifecycle, kernel table, attenuation, delegation |
| Security model overview | [security/model.md](../security/model.md) | §2.2 Capability Check; §3 Capability System Internals |
| Space Storage overview | [storage/spaces.md](../storage/spaces.md) | §1 Core Insight; §2 Architecture; §3 Data Structures; §12 Implementation Order |
| Block Engine and WAL | [storage/spaces/block-engine.md](../storage/spaces/block-engine.md) | §4.1–§4.10 On-disk layout, LSM-tree, WAL, compression, encryption |
| Version Store (Merkle DAG) | [storage/spaces/versioning.md](../storage/spaces/versioning.md) | §5.1–§5.5 Merkle DAG, snapshots, retention |
| Subsystem framework | [platform/subsystem-framework.md](../platform/subsystem-framework.md) | §2 What Every Subsystem Shares; §3 Five-Layer Architecture |

-----

## Milestones

Milestones are numbered continuously across all phases. Phase 4 used M13–M15; Phase 5 continues with M16–M18.

| Milestone | Steps | Target | Observable result |
|---|---|---|---|
| **M16 — Memory Kit & Capability Kit** | 1–6 | End of week 1 | `shared/src/kits/` module exists with Memory Kit traits (`FrameAllocator`, `AddressSpace`, `MemoryPressureMonitor`) + `MemoryError`, Capability Kit trait (`CapabilityEnforcer`) + `CapabilityError`; kernel `impl` blocks compile; host-side tests pass |
| **M17 — IPC Kit** | 7–10 | End of week 2 | IPC Kit traits (`ChannelOps`, `NotificationOps`, `SelectOps`, `SharedMemoryOps`) + `IpcKitError`; kernel `impl` blocks compile; all existing IPC self-tests still pass on QEMU |
| **M18 — Storage Kit & Gate** | 11–14 | End of week 3 | Storage Kit traits (`SpaceManager`, `ObjectStore`, `VersionStoreOps`, `BlockStore`); kernel `impl` blocks compile; Kit docs updated; full audit clean; all quality gates pass |

-----

## Milestone 16 — Memory Kit & Capability Kit (End of Week 1)

*Goal: Create the `shared/src/kits/` module structure. Define Memory Kit and Capability Kit trait hierarchies with error types, supporting types, and kernel-side implementations. Verify with host-side tests.*

### Step 1: Kit module structure in shared crate

**What:** Create `shared/src/kits/mod.rs` with sub-modules. This establishes the foundational directory structure that all 30 Kits will eventually use.

**Tasks:**
- [ ] Create `shared/src/kits/mod.rs` with `pub mod memory;` and `pub mod capability;`
- [ ] Create `shared/src/kits/memory.rs` — stub with module-level doc comment referencing `docs/kits/kernel/memory.md`
- [ ] Create `shared/src/kits/capability.rs` — stub with module-level doc comment referencing `docs/kits/kernel/capability.md`
- [ ] Add `pub mod kits;` to `shared/src/lib.rs`
- [ ] Add `pub use kits::memory as memory_kit;` and `pub use kits::capability as capability_kit;` re-exports in `shared/src/lib.rs`

**Key reference:** `docs/kits/README.md` (Kit Discovery and Registration — Kernel Kits use static linking)

**Acceptance:** `cargo build --target aarch64-unknown-none` zero warnings. `just test` passes (new module compiles on host). `shared/src/kits/mod.rs` exists.

-----

### Step 2: Memory Kit error type and supporting types

**What:** Define `MemoryError` and the supporting types that Memory Kit traits reference: `PhysFrame`, `PagePermissions` (with W^X enforcement), `Mapping`.

**Tasks:**
- [ ] Define `MemoryError` enum in `shared/src/kits/memory.rs` with variants: `OutOfMemory { pool: Pool, requested: usize, available: usize }`, `WxViolation`, `InvalidRegion`, `CapabilityDenied`, `AlreadyMapped { vaddr: VirtAddr }`, `NotMapped { vaddr: VirtAddr }`, `BudgetExceeded`, `TooManyRegions`
- [ ] Derive `Debug`, `Clone`, `PartialEq`, `Eq` on `MemoryError`
- [ ] Define `PhysFrame` struct: `pub addr: PhysAddr`, `pub pool: Pool` — wraps a physical page address with its originating pool
- [ ] Define `PagePermissions` struct with fields: `read: bool`, `write: bool`, `execute: bool`, `user_accessible: bool` — constructor `new()` that enforces W^X invariant (returns `Err(MemoryError::WxViolation)` if both `write` and `execute` are true)
- [ ] Define `Mapping` struct: `pub vaddr: VirtAddr`, `pub size: usize`, `pub perms: PagePermissions`, `pub pool: Pool`
- [ ] Write host-side tests: `MemoryError` Debug formatting, `PagePermissions::new()` W^X enforcement (valid cases pass, W+X rejected), `PhysFrame` construction

**Note:** `PhysAddr`, `VirtAddr`, and `Pool` are already defined in `shared/src/lib.rs` and `shared/src/memory.rs`. The Kit types build on these existing definitions.

**Key reference:** `docs/kits/kernel/memory.md` (Core Traits section); `docs/kernel/memory/virtual.md` §3.2 (PageTableEntry W^X)

**Acceptance:** `just check` zero warnings. `just test` — new tests pass, existing 364+ tests still pass.

-----

### Step 3: Memory Kit trait definitions

**What:** Define the three Memory Kit traits: `FrameAllocator`, `AddressSpace`, `MemoryPressureMonitor`.

**Tasks:**
- [ ] Define `FrameAllocator` trait:
  - `fn alloc_frame(&self, pool: Pool) -> Result<PhysFrame, MemoryError>` — allocate one 4 KiB frame from the specified pool
  - `fn free_frame(&self, frame: PhysFrame) -> Result<(), MemoryError>` — return a frame to its pool
  - `fn pool_pressure(&self, pool: Pool) -> MemoryPressure` — current pressure level for a pool
  - `fn pool_stats(&self, pool: Pool) -> PoolStats` — free/total counts for a pool
- [ ] Define `PoolStats` struct: `pub free_frames: usize`, `pub total_frames: usize`
- [ ] Define `AddressSpace` trait:
  - `fn map(&mut self, vaddr: VirtAddr, frames: &[PhysFrame], perms: PagePermissions) -> Result<(), MemoryError>` — map contiguous virtual pages to physical frames
  - `fn unmap(&mut self, vaddr: VirtAddr, pages: usize) -> Result<(), MemoryError>` — unmap pages, return frames to pool
  - `fn protect(&mut self, vaddr: VirtAddr, pages: usize, perms: PagePermissions) -> Result<(), MemoryError>` — change permissions on existing mapping
  - `fn query(&self, vaddr: VirtAddr) -> Option<Mapping>` — look up mapping at a virtual address
- [ ] Define `MemoryPressureMonitor` trait:
  - `fn current_level(&self) -> MemoryPressure` — aggregate pressure across all pools
- [ ] Write host-side test verifying `FrameAllocator`, `AddressSpace`, and `MemoryPressureMonitor` are dyn-compatible (object-safe): `fn _assert_object_safe(_: &dyn FrameAllocator) {}` etc.

**Key reference:** `docs/kits/kernel/memory.md` (Core Traits)

**Acceptance:** `just check` zero warnings. `just test` — trait object safety tests pass.

-----

### Step 4: Capability Kit error type and trait definition

**What:** Define `CapabilityError` (replacing raw `i64` error codes in the kernel) and the `CapabilityEnforcer` trait.

**Tasks:**
- [ ] Define `CapabilityError` enum in `shared/src/kits/capability.rs` with variants: `NotGranted { requested: Capability }`, `Revoked { token_id: CapabilityTokenId }`, `Expired { token_id: CapabilityTokenId }`, `TableFull`, `InvalidAttenuation { reason: &'static str }`, `InvalidHandle { handle: CapabilityHandle }`, `NotDelegatable { token_id: CapabilityTokenId }`
- [ ] Derive `Debug`, `Clone`, `PartialEq`, `Eq` on `CapabilityError`
- [ ] Define `CapabilityEnforcer` trait:
  - `fn check(&self, holder: ProcessId, action: &Capability) -> Result<CapabilityHandle, CapabilityError>` — verify the holder has a valid token granting the action; return the handle if found
  - `fn grant(&mut self, holder: ProcessId, cap: Capability, granted_by: ProcessId) -> Result<CapabilityHandle, CapabilityError>` — create a new token
  - `fn revoke(&mut self, holder: ProcessId, handle: CapabilityHandle) -> Result<(), CapabilityError>` — revoke token and cascade to children
  - `fn attenuate(&mut self, holder: ProcessId, handle: CapabilityHandle, narrowed: Capability) -> Result<CapabilityHandle, CapabilityError>` — create attenuated child token
  - `fn list_active(&self, holder: ProcessId) -> &[Option<CapabilityToken>]` — return the holder's capability table
- [ ] Add conversion: `impl From<CapabilityError> for i64` (for syscall return compatibility) and `impl TryFrom<i64> for CapabilityError`
- [ ] Write host-side tests: `CapabilityError` round-trip through `i64`, Debug formatting, `CapabilityEnforcer` is dyn-compatible

**Key reference:** `docs/kits/kernel/capability.md` (Core Traits); `shared/src/cap.rs` (existing types)

**Acceptance:** `just check` zero warnings. `just test` passes.

-----

### Step 5: Implement Memory Kit traits on kernel types

**What:** Add `impl FrameAllocator` and `impl MemoryPressureMonitor` on existing kernel memory management types. The `AddressSpace` trait implementation is deferred to a wrapper struct since the kernel's address space operations use global state.

**Tasks:**
- [ ] In `kernel/src/mm/frame.rs`: import `shared::kits::memory::{FrameAllocator as FrameAllocatorKit, PhysFrame, PoolStats, MemoryError}`
- [ ] Create `KernelFrameAllocator` unit struct in `kernel/src/mm/frame.rs` that wraps the existing global `FRAME_ALLOC` state
- [ ] Implement `FrameAllocatorKit` for `KernelFrameAllocator`:
  - `alloc_frame()` delegates to existing `alloc_page()` / `alloc_user_page()` / `alloc_dma_page()` (pool-dispatched), wraps result in `PhysFrame`
  - `free_frame()` delegates to existing `buddy::free_page()` (unsafe, kernel wraps safely)
  - `pool_pressure(pool)` computes pressure for the given pool from per-pool free/total data (using `pool_free_pages(pool)` and pool size), rather than delegating to the global `FrameAllocator::pressure()` which only covers the user pool
  - `pool_stats()` computes free/total from existing pool data
- [ ] Implement `MemoryPressureMonitor` for `KernelFrameAllocator`:
  - `current_level()` returns worst pressure across all pools
- [ ] Verify existing kernel boot sequence unaffected — all existing code continues using the module-level functions; Kit trait is an additional API layer

**Note:** The `AddressSpace` trait implementation is more complex because kernel address space operations (`pgtable.rs`, `kmap.rs`, `uspace.rs`) use `unsafe` operations with hardware page tables. A `KernelAddressSpace` wrapper will be added in a later phase when user-space address space management is formalized. For Phase 5, defining the trait in shared is sufficient — the kernel can implement it incrementally.

**Key reference:** `kernel/src/mm/frame.rs`; `kernel/src/mm/pools.rs`

**Acceptance:** `just check` zero warnings. `just run` — kernel boots with identical UART output (no regressions).

-----

### Step 6: Implement Capability Kit trait on kernel types and shared crate refactoring

**What:** Add `impl CapabilityEnforcer` on the kernel capability system. Run final M16 shared crate cleanup and host-side tests.

**Tasks:**
- [ ] In `kernel/src/cap/mod.rs`: import `shared::kits::capability::{CapabilityEnforcer, CapabilityError}`
- [ ] Create `KernelCapabilitySystem` unit struct in `kernel/src/cap/mod.rs`
- [ ] Implement `CapabilityEnforcer` for `KernelCapabilitySystem`:
  - `check()` delegates to existing `check_channel_create()` / `check_channel_access()` / `check_shared_memory_create()` / `check_shared_memory_access()` (action-dispatched), converts `i64` result to `CapabilityError`
  - `grant()` delegates to existing `grant_to_process()` function
  - `revoke()` delegates to existing `revoke_in_process()` function, includes cascade
  - `attenuate()` — new implementation: create child token with narrowed capability from parent (not yet in kernel; implement directly in the `impl` block)
  - `list_active()` — new implementation: return process's capability table slice (not yet in kernel; implement directly by reading `PROCESS_TABLE`)
- [ ] Ensure `Debug`, `Clone`, `PartialEq`, `Eq` on all new Kit types in `shared/src/kits/`
- [ ] Add comprehensive host-side tests for all M16 Kit types:
  - `MemoryError` all variants construct correctly
  - `PagePermissions` W^X invariant cannot be violated
  - `CapabilityError <-> i64` round-trip for all variants
  - All 4 Kit traits are dyn-compatible
- [ ] Update `shared/src/lib.rs` re-exports for ergonomic imports

**Acceptance:** `just check` zero warnings. `just test` — all pass (new test count > 380). `just run` — kernel boots normally, IPC and capability self-tests still pass.

-----

## Milestone 17 — IPC Kit (End of Week 2)

*Goal: Define IPC Kit trait hierarchy with proper error types. Implement traits on kernel IPC subsystem. Verify no regressions in IPC self-tests.*

### Step 7: IPC Kit error type

**What:** Create `IpcKitError` — a richer error type than the existing syscall-level `IpcError`. The syscall `IpcError` remains for binary compatibility; `IpcKitError` is the Kit-level error with more context.

**Tasks:**
- [ ] Create `shared/src/kits/ipc.rs`
- [ ] Add `pub mod ipc;` to `shared/src/kits/mod.rs`
- [ ] Add `pub use kits::ipc as ipc_kit;` to `shared/src/lib.rs`
- [ ] Define `IpcKitError` enum with variants:
  - `InvalidChannel { id: ChannelId }` — channel does not exist
  - `ChannelFull { id: ChannelId, capacity: usize }` — ring buffer at capacity
  - `Timeout { elapsed_ticks: u64 }` — operation timed out
  - `Cancelled` — operation cancelled by peer
  - `CapabilityDenied { required: Capability }` — missing required capability
  - `SharedMemoryError { reason: &'static str }` — shared memory operation failed
  - `MessageTooLarge { size: usize, max: usize }` — payload exceeds MAX_MESSAGE_SIZE
  - `NoReply` — call completed but no reply received
- [ ] Derive `Debug`, `Clone`, `PartialEq`, `Eq`
- [ ] Add conversions: `From<IpcKitError> for IpcError` and `From<IpcError> for IpcKitError`
- [ ] Write host-side tests: conversion round-trips, Debug formatting

**Key reference:** `docs/kits/kernel/ipc.md` (Error Handling); `shared/src/syscall.rs` (existing `IpcError`)

**Acceptance:** `just check` zero warnings. `just test` passes with new IPC Kit error tests.

-----

### Step 8: IPC Kit trait definitions

**What:** Define the core IPC Kit traits: `ChannelOps`, `NotificationOps`, `SelectOps`, `SharedMemoryOps`.

**Tasks:**
- [ ] Define `ChannelOps` trait:
  - `fn channel_create(&mut self) -> Result<ChannelId, IpcKitError>` — create a new channel pair
  - `fn channel_destroy(&mut self, id: ChannelId) -> Result<(), IpcKitError>` — destroy a channel
  - `fn send(&self, id: ChannelId, msg: &RawMessage) -> Result<(), IpcKitError>` — fire-and-forget send
  - `fn recv(&self, id: ChannelId, timeout_ticks: u64) -> Result<RawMessage, IpcKitError>` — blocking receive
  - `fn call(&self, id: ChannelId, request: &RawMessage, timeout_ticks: u64) -> Result<RawMessage, IpcKitError>` — synchronous call (send + wait for reply)
  - `fn reply(&self, msg: &RawMessage) -> Result<(), IpcKitError>` — reply to a pending call
- [ ] Define `NotificationOps` trait:
  - `fn notification_create(&mut self) -> Result<NotificationId, IpcKitError>` — create a notification object
  - `fn signal(&self, id: NotificationId, bits: u64) -> Result<(), IpcKitError>` — atomic OR into notification word
  - `fn wait(&self, id: NotificationId, mask: u64, timeout_ticks: u64) -> Result<u64, IpcKitError>` — wait for masked bits, return matched
- [ ] Define `SelectOps` trait:
  - `fn select(&self, entries: &[SelectEntry], timeout_ticks: u64) -> Result<(usize, u64), IpcKitError>` — multi-wait on channels + notifications
- [ ] Define `SharedMemoryOps` trait:
  - `fn shmem_create(&mut self, size: usize, flags: u64) -> Result<SharedMemoryId, IpcKitError>` — create shared region
  - `fn shmem_map(&mut self, id: SharedMemoryId, vaddr: VirtAddr, flags: u64) -> Result<(), IpcKitError>` — map into caller's address space
  - `fn shmem_unmap(&mut self, id: SharedMemoryId) -> Result<(), IpcKitError>` — unmap from caller
  - `fn shmem_destroy(&mut self, id: SharedMemoryId) -> Result<(), IpcKitError>` — destroy region
- [ ] Write host-side tests: all 4 traits are dyn-compatible

**Key reference:** `docs/kits/kernel/ipc.md` (Core Traits)

**Acceptance:** `just check` zero warnings. `just test` passes.

-----

### Step 9: Implement IPC Kit traits on kernel types

**What:** Add trait implementations on the existing kernel IPC subsystem. Use wrapper structs that delegate to existing module-level functions.

**Tasks:**
- [ ] Create `KernelIpc` unit struct in `kernel/src/ipc/mod.rs`
- [ ] Implement `ChannelOps` for `KernelIpc`:
  - `channel_create()` delegates to existing `channel_create()` function
  - `channel_destroy()` delegates to existing `channel_destroy()` function
  - `send()` delegates to `ipc_send()`
  - `recv()` delegates to `ipc_recv()`
  - `call()` delegates to `ipc_call()`
  - `reply()` delegates to `ipc_reply()`
- [ ] Implement `NotificationOps` for `KernelIpc`:
  - `notification_create()` delegates to `notification_create()`
  - `signal()` delegates to `notification_signal()`
  - `wait()` delegates to `notification_wait()`
- [ ] Implement `SelectOps` for `KernelIpc`:
  - `select()` delegates to `ipc_select()`
- [ ] Implement `SharedMemoryOps` for `KernelIpc`:
  - Delegates to `shared_memory_create()`, `shared_memory_map()`, `shared_memory_unmap()`
  - `shmem_destroy()` — new implementation: unmap all mappings then release region (no dedicated destroy function exists yet; implement in the `impl` block)
- [ ] Verify no regressions: all existing IPC self-tests (echo service, select, notifications, shared memory, priority inheritance) still pass

**Key reference:** `kernel/src/ipc/mod.rs`; `kernel/src/ipc/channel.rs`; `kernel/src/ipc/notify.rs`; `kernel/src/ipc/select.rs`; `kernel/src/ipc/shmem.rs`

**Acceptance:** `just check` zero warnings. `just run` — all IPC self-tests produce identical UART output.

-----

### Step 10: IPC Kit shared crate refactoring and host-side tests

**What:** Ensure IPC Kit types are properly organized and comprehensively tested. Verify backward compatibility of existing `shared::ipc::*` imports.

**Tasks:**
- [ ] Verify `shared/src/ipc.rs` types (ChannelId, RawMessage, SelectEntry, etc.) are re-exported from `shared::kits::ipc` for convenience
- [ ] Add tests for `IpcKitError <-> IpcError` round-trip for every variant
- [ ] Add tests verifying constants (MAX_CHANNELS, RING_CAPACITY, MAX_MESSAGE_SIZE, DEFAULT_TIMEOUT_TICKS) are accessible from Kit module
- [ ] Ensure backward compatibility: all existing `shared::ipc::*` imports still work (no breaking changes)
- [ ] Run full `just test` — all existing 364+ tests pass alongside new Kit tests

**Acceptance:** `just check` zero warnings. `just test` — all pass, new test count > 390.

-----

## Milestone 18 — Storage Kit & Gate (End of Week 3)

*Goal: Define Storage Kit trait hierarchy. Implement on kernel storage types. Update Kit docs. Run full audit. Create PR.*

### Step 11: Storage Kit trait definitions

**What:** Define Storage Kit traits: `BlockStore`, `SpaceManager`, `ObjectStore`, `VersionStoreOps`. Storage Kit is a Platform Kit (Layer 2) per the Kit hierarchy, but shares its types with the Kernel layer.

**Tasks:**
- [ ] Create `shared/src/kits/storage.rs`
- [ ] Add `pub mod storage;` to `shared/src/kits/mod.rs`
- [ ] Add `pub use kits::storage as storage_kit;` to `shared/src/lib.rs`
- [ ] Define `BlockStore` trait:
  - `fn write_block(&mut self, data: &[u8]) -> Result<BlockId, StorageError>` — write data, return content hash as block ID
  - `fn read_block(&self, id: &BlockId) -> Result<alloc::vec::Vec<u8>, StorageError>` — read block by content hash
  - `fn block_exists(&self, id: &BlockId) -> bool` — check if block exists
- [ ] Define `SpaceManager` trait:
  - `fn create_space(&mut self, name: &str, zone: SecurityZone) -> Result<SpaceId, StorageError>` — create a new space
  - `fn get_space(&self, id: &SpaceId) -> Result<crate::storage::Space, StorageError>` — retrieve space metadata
  - `fn list_spaces(&self) -> alloc::vec::Vec<crate::storage::Space>` — list all spaces
  - `fn delete_space(&mut self, id: &SpaceId) -> Result<(), StorageError>` — delete a space
  - `fn storage_budget(&self) -> crate::storage::StorageBudget` — current storage usage
  - `fn pressure_level(&self) -> crate::storage::PressureLevel` — current pressure
- [ ] Define `ObjectStore` trait:
  - `fn create_object(&mut self, space_id: &SpaceId, name: &str, content_type: ContentType, data: &[u8]) -> Result<crate::storage::ObjectId, StorageError>` — create a new object
  - `fn read_object(&self, id: &crate::storage::ObjectId) -> Result<alloc::vec::Vec<u8>, StorageError>` — read object data
  - `fn delete_object(&mut self, id: &crate::storage::ObjectId) -> Result<(), StorageError>` — delete an object
  - `fn list_objects(&self, space_id: &SpaceId) -> alloc::vec::Vec<crate::storage::CompactObject>` — list objects in a space
- [ ] Define `VersionStoreOps` trait:
  - `fn create_version(&mut self, object_id: &crate::storage::ObjectId, data: &[u8], message: &str) -> Result<ContentHash, StorageError>` — create a new version
  - `fn list_versions(&self, object_id: &crate::storage::ObjectId) -> alloc::vec::Vec<crate::storage::Version>` — list version history
  - `fn get_head(&self, object_id: &crate::storage::ObjectId) -> Result<ContentHash, StorageError>` — current head version hash
  - `fn rollback(&mut self, object_id: &crate::storage::ObjectId, version_hash: &ContentHash) -> Result<ContentHash, StorageError>` — rollback to a previous version
- [ ] Re-export `StorageError` from `shared::storage` (already well-defined with 19 variants)
- [ ] Write host-side tests: all 4 traits are dyn-compatible

**Key reference:** `docs/kits/platform/storage.md` (Core Traits); `shared/src/storage.rs` (existing types)

**Acceptance:** `just check` zero warnings. `just test` passes.

-----

### Step 12: Implement Storage Kit traits on kernel types

**What:** Implement Kit traits on existing kernel storage subsystem. Use wrapper structs that delegate to existing `with_engine` operations.

**Tasks:**
- [ ] Create `KernelBlockStore` unit struct in `kernel/src/storage/block_engine.rs`
- [ ] Implement `BlockStore` for `KernelBlockStore`:
  - `write_block()` delegates to `BlockEngine::write_block()`
  - `read_block()` delegates to `BlockEngine::read_block()`
  - `block_exists()` delegates to `BlockEngine::block_exists()` (or index lookup)
- [ ] Create `KernelSpaceManager` unit struct in `kernel/src/storage/space.rs`
- [ ] Implement `SpaceManager` for `KernelSpaceManager`:
  - Delegates to existing `space_create()`, `space_get()`, `space_list()`, `space_delete()`, `storage_stats()`
  - `pressure_level()` — new implementation: derive `PressureLevel` from `StorageBudget` free/total ratio (thresholds in `shared/src/storage.rs`)
- [ ] Create `KernelObjectStore` unit struct in `kernel/src/storage/object_store.rs`
- [ ] Implement `ObjectStore` for `KernelObjectStore`:
  - Delegates to existing `object_create()`, `object_read()`, `object_delete()`
  - `list_objects()` — new implementation: iterate `ObjectIndex` entries filtered by `SpaceId` (no dedicated list function exists yet)
- [ ] Create `KernelVersionStore` unit struct in `kernel/src/storage/version_store.rs`
- [ ] Implement `VersionStoreOps` for `KernelVersionStore`:
  - Delegates to existing `version_create()`, `version_list()`, `version_rollback()`
  - `get_head()` — new implementation: look up `version_head` in `ObjectIndex` entry (no dedicated function exists yet; read from `BlockEngine`'s object index)
- [ ] Verify no regressions: all existing storage self-tests still pass on QEMU

**Acceptance:** `just check` zero warnings. `just run` — storage self-tests produce identical UART output.

-----

### Step 13: Kit doc updates, CLAUDE.md, and cross-references

**What:** Update Kit docs to reflect actual trait signatures. Update status from "Overview" to "Traits Defined". Update CLAUDE.md and README.md.

**Tasks:**
- [ ] Update `docs/kits/kernel/memory.md` — verify trait signatures match `shared/src/kits/memory.rs` exactly
- [ ] Update `docs/kits/kernel/ipc.md` — verify trait signatures match `shared/src/kits/ipc.rs` exactly
- [ ] Update `docs/kits/kernel/capability.md` — verify trait signatures match `shared/src/kits/capability.rs` exactly
- [ ] Update `docs/kits/platform/storage.md` — verify trait signatures match `shared/src/kits/storage.rs` exactly
- [ ] Update `docs/kits/README.md` — change status for Memory Kit, IPC Kit, Capability Kit from "Overview" to "Traits Defined"; change Storage Kit from "Overview" to "Traits Defined"
- [ ] Update `CLAUDE.md`:
  - Workspace Layout: add `shared/src/kits/` directory with sub-modules
  - Key Technical Facts: add Kit trait counts, wrapper struct names
  - Architecture Doc Map: add Kit doc status
- [ ] Update `README.md` project structure if applicable
- [ ] Update `docs/project/development-plan.md` Phase 5 row: status from "Planned" to "In Progress" (or "Complete" if this is the final step)
- [ ] Update this phase doc: check off completed task boxes, update Status field

**Key reference:** All Kit docs in `docs/kits/kernel/` and `docs/kits/platform/`

**Acceptance:** All Kit doc trait signatures match code exactly. Cross-references resolve. `docs/kits/README.md` shows 4 Kits at "Traits Defined" status.

-----

### Step 14: Full audit and quality gate

**What:** Run the mandatory triple audit (doc, code, security) recursively until clean. Verify all quality gates. Create PR.

**Tasks:**
- [ ] Run doc audit: cross-reference accuracy between Kit docs and code, naming consistency, no stale references
- [ ] Run code review: convention compliance (naming, unsafe docs, dead code), trait object safety, error type completeness
- [ ] Run security/bug review: no unsound trait implementations, error handling covers all paths, no capability bypasses
- [ ] Fix all issues found, commit, re-run all three audits
- [ ] Repeat until a full round returns 0 issues across all three categories
- [ ] Verify all quality gates:
  - `cargo build --target aarch64-unknown-none` — zero warnings
  - `just check` (fmt-check + clippy + build) — zero warnings, zero errors
  - `just test` — all pass (should be >400 tests)
  - `just run` — QEMU boots, all existing self-tests pass identically to Phase 4 output
  - CI — push to GitHub, all jobs pass
- [ ] Create PR to `main` with description summarizing Kit trait extraction

**Acceptance:** All 3 audits clean in a single pass. All quality gates pass. PR created and pushed.

-----

## Decision Points

| Decision | Options | Recommendation | Rationale |
|---|---|---|---|
| Kit traits in shared vs. separate crates | (A) `shared/src/kits/` sub-module (B) New `aios_memory`, `aios_ipc`, etc. workspace crates | A — sub-module | Adding 4 crates is premature; `shared` is already `no_std` + testable. Revisit when APIs stabilize (Phase 6+). |
| Trait granularity | (A) One big trait per Kit (B) Multiple focused traits | B — focused traits | Interface Segregation Principle. Consumers don't need `AddressSpace` if they only allocate frames. |
| Error type strategy | (A) One error per Kit (B) Shared error across Kits | A — per-Kit errors | Each Kit has domain-specific failure modes. Conversions (`From<IpcKitError> for IpcError`) bridge layers. |
| AddressSpace impl timing | (A) Phase 5 (B) Later phase | B — later | Kernel address space ops use `unsafe` hardware page tables. Defining the trait now but implementing incrementally is safer. |
| Kit trait `unsafe` boundary | (A) Traits have `unsafe` methods (B) Traits are safe, impl is `unsafe` | B — safe traits | Kit users shouldn't need `unsafe` blocks. The kernel impl handles `unsafe` internally, returning `Result`. |

-----

## Phase Completion Criteria

- [ ] **M16 complete:** `shared/src/kits/` module exists. Memory Kit traits (`FrameAllocator`, `AddressSpace`, `MemoryPressureMonitor`) + `MemoryError` defined. Capability Kit trait (`CapabilityEnforcer`) + `CapabilityError` defined. Kernel `impl` blocks for `KernelFrameAllocator` and `KernelCapabilitySystem` compile. Host-side tests pass (>380 tests).
- [ ] **M17 complete:** IPC Kit traits (`ChannelOps`, `NotificationOps`, `SelectOps`, `SharedMemoryOps`) + `IpcKitError` defined. `KernelIpc` implements all 4 traits. All existing IPC self-tests pass on QEMU unchanged. Host-side tests pass (>390 tests).
- [ ] **M18 complete:** Storage Kit traits (`BlockStore`, `SpaceManager`, `ObjectStore`, `VersionStoreOps`) defined. Kernel wrappers implement all 4 traits. Kit docs updated to "Traits Defined". All quality gates pass. Triple audit clean. PR created.
