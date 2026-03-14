# Phase 4: Block Storage & Object Store

**Tier:** 2 — Core System Services
**Duration:** 5 weeks
**Deliverable:** VirtIO-blk driver, LSM-tree block engine with WAL, content-addressed object store with deduplication, version store with Merkle DAG, device-level encryption, POSIX bridge
**Status:** In Progress (M13 complete)
**Prerequisites:** Phase 3 (IPC & Capability System)
**Unlocks:** Phase 5 (GPU & Display), Phase 13 (Security Hardening)

-----

## Objective

Build the storage subsystem that replaces a traditional filesystem. Phase 3 delivered IPC channels, a capability system, a scheduler, and a service manager. Phase 4 uses these to implement Space Storage as a user-space service: a VirtIO-blk driver for QEMU, a Block Engine with LSM-tree indexing and WAL for crash consistency, a content-addressed Object Store with deduplication, a Version Store with Merkle DAG history, device-level transparent encryption, and a POSIX compatibility bridge for BSD tools.

Space Storage owns the block device directly — no ext4, no intermediate filesystem. Every block is encrypted with a device key before reaching storage drivers. Objects are content-addressed by SHA-256 hash, automatically deduplicated, and versioned through a Merkle DAG (similar to git). The Object Store supports CompactObjects (lightweight, storage-efficient defaults) and full Objects (rich metadata, promoted on demand). Spaces organize objects into security zones (Core, Personal, Ephemeral) with per-zone quotas and encryption policies.

By the end of this phase, the kernel has a working block device driver (VirtIO-blk on QEMU), the Block Engine persists data with crash recovery, the Object Store creates and retrieves content-addressed objects, the Version Store tracks history, device encryption is active on every block, and BSD tools can read/write files through the POSIX bridge. System spaces (`system/`, `user/home/`, `ephemeral/`) are created at boot (per spaces.md §3.2; the `ephemeral/` space uses SecurityZone::Ephemeral for auto-cleaned temporary data).

-----

## Architecture References

| Topic | Document | Relevant Sections |
|---|---|---|
| Space Storage overview and architecture | [spaces.md](../storage/spaces.md) | §1 Core Insight; §2 Architecture |
| Core data structures (Space, Object, Hash, IDs) | [spaces.md](../storage/spaces.md) | §3 Core Data Structures; §3.0 Primitive Types; §3.1 Spaces; §3.2 System Spaces; §3.3 Objects |
| CompactObject and promotion policy | [spaces.md](../storage/spaces.md) | §3.3.1 Compact vs Full Objects |
| Relations and relation kinds | [spaces.md](../storage/spaces.md) | §3.4 Relations |
| Block Engine: on-disk layout, LSM-tree | [spaces.md](../storage/spaces.md) | §4.1 On-Disk Layout; §4.2 Write Path (Flash-Aware); §4.3 Read Path |
| Crash recovery (WAL replay) | [spaces.md](../storage/spaces.md) | §4.4 Crash Recovery |
| Garbage collection | [spaces.md](../storage/spaces.md) | §4.5 Garbage Collection |
| Block-level compression | [spaces.md](../storage/spaces.md) | §4.6 Block-Level Compression |
| Write amplification tracking | [spaces.md](../storage/spaces.md) | §4.8 Write Amplification Tracking |
| Device-level transparent encryption | [spaces.md](../storage/spaces.md) | §4.10 Device-Level Transparent Encryption; §4.10.1 Device Key Hierarchy; §4.10.2 Encryption in the Write Path |
| Version Store (Merkle DAG, snapshots) | [spaces.md](../storage/spaces.md) | §5 Version Store; §5.1 Merkle DAG; §5.2 Space Snapshots; §5.3 DAG Operations |
| POSIX compatibility bridge | [spaces.md](../storage/spaces.md) | §9 POSIX Compatibility; §9.1 Path Mapping; §9.2 Translation Layer |
| Storage budget and pressure management | [spaces.md](../storage/spaces.md) | §10 Storage Budget and Pressure Management; §10.5 Storage Pressure Response |
| Implementation order (sub-phases) | [spaces.md](../storage/spaces.md) | §12 Implementation Order |
| Subsystem framework (5-layer architecture) | [subsystem-framework.md](../platform/subsystem-framework.md) | §2 What Every Subsystem Shares; §3 The Five-Layer Subsystem Architecture |
| IPC channels and service model | [ipc.md](../kernel/ipc.md) | §5.4 Multi-Client Service Model |
| Capability system for access control | [security.md](../security/security.md) | §2.2 Capability Check; §3 Capability System Internals |
| Memory management (page pools, user pool) | [physical.md](../kernel/memory/physical.md) | §2.2 Buddy Allocator; §2.4 Page Pools |
| Security model and security zones | [security.md](../security/security.md) | §2 The Eight Security Layers (Deep Dive) |
| System overview and architecture diagram | [architecture.md](../project/architecture.md) | §2.2 Space Storage System |

-----

## Milestones

Milestones are numbered continuously across all phases. Phase 3 used M10–M12; Phase 4 continues with M13–M15.

| Milestone | Steps | Target | Observable result |
|---|---|---|---|
| **M13 — VirtIO-blk Driver & Block Engine** | 1–4 | End of week 2 | VirtIO-blk reads/writes raw sectors; Block Engine with WAL, LSM-tree MemTable, and crash-safe writes; superblock written and verified on QEMU |
| **M14 — Object Store, Version Store & Encryption** | 5–8 | End of week 4 | Content-addressed objects with deduplication; Merkle DAG version history; device-level AES-256-GCM encryption on every block; system spaces created at boot |
| **M15 — POSIX Bridge, Compression & Gate** | 9–12 | End of week 5 | POSIX path mapping; BSD `ls`/`cat`/`mkdir` semantics via Space API; LZ4 block compression; storage budget enforcement; end-to-end test: write file via POSIX → read back verified |

-----

## Milestone 13 — VirtIO-blk Driver & Block Engine (End of Week 2)

*Goal: Implement a VirtIO-blk block device driver for QEMU, build the Block Engine with superblock, WAL, and LSM-tree MemTable index, and verify crash-safe writes persist across QEMU restarts.*

### Step 1: Shared storage types and VirtIO-blk shared definitions

**What:** Define the core shared types used throughout the storage subsystem, and the VirtIO data structures needed for the block driver.

**Tasks:**
- [x] Create `shared/src/storage.rs` with core types:
  - `Hash([u8; 32])` — SHA-256 content hash (newtype, not alias)
  - `ObjectId([u8; 16])` — 128-bit unique object identifier (newtype; timer+pid entropy, not RFC 4122 UUID v4)
  - `SpaceId([u8; 16])` — 128-bit unique space identifier (newtype; same generation scheme as ObjectId)
  - `Timestamp(pub u64)` — milliseconds since epoch
  - `BlockId` = `Hash` (content-addressed block identifier)
  - `ContentType` enum (Directory, Text, Code, Binary, etc.)
  - `SecurityZone` enum (Core, Personal, Ephemeral, Untrusted, Collaborative placeholder)
  - `StorageError` enum (BlockNotFound, ChecksumFailed, IoError, QuotaExceeded, etc.)
  - `StorageTier` enum (Hot, Warm, Cold)
- [x] Add `pub mod storage;` to `shared/src/lib.rs`, re-export key types
- [x] Add unit tests for Hash ordering, ObjectId/SpaceId Display, Timestamp arithmetic
- [x] Create `kernel/src/drivers/` directory with `mod.rs` and `virtio_blk.rs` stub
- [x] Define VirtIO MMIO constants: `MAGIC_VALUE = 0x74726976`, version, device ID (2 = block), feature bits
- [x] Define virtqueue descriptor, available ring, used ring structures (VirtIO spec §2.7)

**Note:** All shared storage types must be `no_std` compatible. Use fixed-size arrays, not `Vec` or `String`. For fields like `name` in the architecture doc that use `String`, use `[u8; N]` with length tracking (similar to `ServiceName` in Phase 3).

**Key reference:** spaces.md §3.0 Primitive Types; VirtIO specification §2 (virtqueue), §5.2 (block device)

**Acceptance:** `just check` passes. `just test` passes with new shared type tests.

### Step 2: VirtIO-blk driver — device probe and initialization

**What:** Implement VirtIO-blk device discovery via MMIO transport (QEMU virt machine), negotiate features, set up virtqueue, and perform a raw sector read/write.

**Tasks:**
- [x] Implement VirtIO MMIO device probe: enumerate `virtio,mmio` nodes from DTB (per hal.md §6) to discover VirtIO block devices (device ID 2). Fallback: scan QEMU virt MMIO region (0x0A000000–0x0A003E00, 512-byte stride) if DTB enumeration is unavailable
- [x] Implement device initialization sequence per VirtIO spec §3.1: reset → acknowledge → driver → features → driver_ok
- [x] Negotiate feature bits: `VIRTIO_BLK_F_SIZE_MAX`, `VIRTIO_BLK_F_SEG_MAX`, `VIRTIO_BLK_F_BLK_SIZE`
- [x] Set up a single virtqueue (queue 0): allocate descriptor table, available ring, used ring from kernel pool (page-aligned, DMA-safe)
- [x] Read device configuration: capacity (sectors), block size (usually 512)
- [x] Implement `read_sector(sector: u64, buf: &mut [u8; 512]) -> Result<(), StorageError>`
- [x] Implement `write_sector(sector: u64, buf: &[u8; 512]) -> Result<(), StorageError>`
- [x] Add `pub mod drivers;` to `kernel/src/main.rs`
- [x] Test: write a known pattern to sector 0, read it back, verify match

**Note:** VirtIO MMIO on QEMU virt uses memory-mapped registers. The driver must use `read_volatile`/`write_volatile` for all MMIO access. Virtqueue memory must be physically contiguous and accessible via the direct map. Use `DSB SY` barriers between descriptor writes and doorbell notification.

**Key reference:** VirtIO specification §2.7 (split virtqueues), §4.2 (MMIO transport), §5.2 (block device)

**Acceptance:** `just run` shows VirtIO-blk device detected, capacity logged, read/write test passes. `just check` passes.

### Step 3: Block Engine — superblock, WAL, and write path

**What:** Build the Block Engine's on-disk layout: superblock at sector 0, WAL circular buffer, and the crash-safe write path. Content is written through the WAL first, then committed.

**Tasks:**
- [x] Create `kernel/src/storage/` directory with `mod.rs`, `block_engine.rs`, `wal.rs`, `lsm.rs`
- [x] Define `Superblock` struct: magic (`0x41494F53_50414345`, "AIOSPACE"), version, block_size, total_blocks, free_blocks, wal_offset, wal_size, index_offset, data_offset, checksum (CRC-32C)
- [x] Implement superblock read/write with integrity verification
- [x] Define `WalEntry` struct: sequence_number, block_id (Hash), data_offset, data_len, index_entry (Hash → location), committed flag, checksum
- [x] Implement WAL as circular buffer: `append(entry) -> Result<(), StorageError>`, `replay() -> Vec<WalEntry>`, `trim_committed()`
- [x] WAL size: 64 MB default (configurable in superblock)
- [x] Implement Block Engine write path (per spaces.md §4.2):
  1. Compute SHA-256 content hash
  2. Check LSM-tree MemTable: if hash exists, increment refcount (deduplication)
  3. Write WAL entry (crash-safe point after fsync)
  4. Write data block to data region (append-only)
  5. Insert index entry into MemTable
  6. Mark WAL entry committed
- [x] Implement format/init: write superblock, initialize WAL region, create empty data region

**Note:** For Phase 4, use a single zone (no hot/warm/cold separation yet — that's Phase 4i). All data blocks are appended sequentially to the data region. CRC-32C checksums on every block and WAL entry.

**Key reference:** spaces.md §4.1 On-Disk Layout; §4.2 Write Path; §4.4 Crash Recovery

**Acceptance:** `just run` shows Block Engine initialized with superblock, WAL write/read cycle logged. `just check` passes.

### Step 4: LSM-tree MemTable and read path

**What:** Implement the LSM-tree's in-memory MemTable for the block index, the read path, and crash recovery via WAL replay. No on-disk SSTables yet — the MemTable is rebuilt from the WAL on boot.

**Tasks:**
- [x] Implement `MemTable`: sorted map of `Hash → BlockLocation` using a fixed-size sorted array (bounded memory, no dynamic allocation; use insertion sort on a bounded array, max 65536 entries). Note: `alloc::collections::BTreeMap` is available via the kernel's `extern crate alloc`, but a fixed-size array is preferred for predictable memory usage and no heap fragmentation
- [x] Implement `MemTable::insert(hash, location)`, `MemTable::get(hash) -> Option<BlockLocation>`, `MemTable::remove(hash)` (tombstone)
- [x] Implement Block Engine read path (per spaces.md §4.3):
  1. Look up content_hash in MemTable → get block location
  2. Read block from data region via VirtIO-blk
  3. Verify CRC-32C checksum
  4. Return content
- [x] Implement crash recovery (per spaces.md §4.4):
  1. Read superblock, verify integrity
  2. Scan WAL from oldest entry
  3. Replay uncommitted entries: re-insert index entries into MemTable, skip entries where data block wasn't written
  4. Rebuild MemTable from committed WAL entries
- [x] Implement Block Engine `init()`: probe VirtIO-blk → read superblock (or format if uninitialized) → replay WAL → MemTable ready
- [x] Test: write 100 blocks, verify all readable, simulate crash (skip commit), verify WAL replay recovers consistent state

**Note:** The MemTable-only approach is sufficient for Phase 4. On-disk SSTables and compaction are deferred to a later optimization step (Phase 14). The MemTable is bounded at 65536 entries, covering up to 256 MB of data when using 4 KB blocks (the index itself uses far less memory). For Phase 4 workloads on QEMU this is more than sufficient.

**Key reference:** spaces.md §4.1 (LSM-tree), §4.3 Read Path, §4.4 Crash Recovery

**Acceptance:** `just run` shows Block Engine write/read cycle with CRC verification, WAL replay on boot. `just check` passes. `just test` passes with MemTable unit tests.

-----

## Milestone 14 — Object Store, Version Store & Encryption (End of Week 4)

*Goal: Build the content-addressed Object Store with deduplication, the Version Store with Merkle DAG history, device-level encryption, and system space initialization.*

*Note on ordering: spaces.md §12 specifies device-level encryption (Phase 4b) before the Object Store (Phase 4c). This milestone implements encryption as Step 7 (after Object Store and Version Store) to allow those layers to be tested against a plaintext Block Engine first, then integrated with encryption. The final write path matches §12: all blocks are encrypted before reaching the driver.*

### Step 5: Object Store — content-addressed objects

**What:** Build the Object Store on top of the Block Engine. Objects are content-addressed by SHA-256 hash. Deduplication is automatic: storing the same content twice increments a reference count instead of writing a duplicate block.

**Tasks:**
- [ ] Create `kernel/src/storage/object_store.rs`
- [ ] Define `CompactObject` struct (per spaces.md §3.3.1): id, name ([u8; 64] + len), content_hash, content_type, content_size, created_at, modified_at, created_by ([u8; 32]), modified_by, text_content (Option<[u8; N]> — extracted text for full-text index, always maintained per §3.3.1)
- [ ] Define object metadata key format: `ObjectId → CompactObject` stored in a separate MemTable (object index)
- [ ] Implement `object_create(space: SpaceId, name: &[u8], content: &[u8], content_type: ContentType) -> Result<ObjectId, StorageError>`:
  1. Hash content (SHA-256) → content_hash
  2. Store content via Block Engine (dedup check happens in Block Engine)
  3. Create CompactObject metadata
  4. Store metadata in object index MemTable
  5. Return ObjectId (generated as a 128-bit unique ID from CNTPCT_EL0 + pid entropy — not RFC 4122 UUID v4; cryptographic uniqueness deferred to Phase 13)
- [ ] Implement `object_read(id: ObjectId) -> Result<(CompactObject, Vec<u8>), StorageError>`:
  1. Look up ObjectId in object index → CompactObject
  2. Read content via Block Engine using content_hash
  3. Return metadata + content
- [ ] Implement `object_delete(id: ObjectId) -> Result<(), StorageError>`:
  1. Look up object → get content_hash
  2. Decrement refcount in Block Engine (block freed when refcount = 0)
  3. Remove from object index
- [ ] Implement reference counting in Block Engine: `inc_ref(hash)`, `dec_ref(hash) -> bool` (returns true if block freed)
- [ ] Test: create object, read back, create duplicate content, verify only 1 block exists (dedup), delete one copy, verify other still readable

**Note:** For Phase 4, use `CompactObject` exclusively. Full `Object` with semantic metadata, embeddings, and provenance chains is Phase 9+ (requires AIRS). UUID v4 generation uses `CNTPCT_EL0` timer entropy mixed with process ID — not cryptographically strong but sufficient for uniqueness.

**Key reference:** spaces.md §3.3 Objects; §3.3.1 Compact vs Full Objects; §4.2 Write Path (dedup at step 2)

**Acceptance:** `just run` shows object create/read/delete cycle with deduplication logged. `just check` passes.

### Step 6: Version Store — Merkle DAG

**What:** Implement the Version Store with a Merkle DAG. Every object modification creates a new version node linked to its parent. Supports version listing, rollback, and space snapshots.

**Tasks:**
- [ ] Create `kernel/src/storage/version_store.rs`
- [ ] Define `Version` struct (per spaces.md §5.1): hash, parent, merge_parent (always None for Phase 4), content_hash, content_size, object_id, timestamp, author, provenance (stub ProvenanceEntry — Ed25519 signatures deferred to Phase 13), message
- [ ] Define version key format: `(SpaceId, ObjectId, reverse_timestamp) → Version` for newest-first iteration
- [ ] Store version nodes in the Block Engine (content-addressed, same as data blocks)
- [ ] Implement `version_create(object_id, content_hash, author, message) -> Hash`:
  1. Look up current head version for this object
  2. Create Version node with parent = current head
  3. Compute version hash: SHA-256(parent_hash + content_hash + timestamp + object_id)
  4. Store version node in Block Engine
  5. Update object's head pointer
- [ ] Implement `version_list(object_id) -> Vec<Version>`: walk the chain from head, collecting versions
- [ ] Implement `version_rollback(object_id, target_hash) -> Result<(), StorageError>`:
  1. Verify target version exists and belongs to this object
  2. Read content from target version's content_hash
  3. Create a new version node (parent = current head, content = old content)
  4. Update object's head pointer (rollback is a new version, not a rewrite)
- [ ] Implement `object_update(id, new_content) -> Result<Hash, StorageError>`:
  1. Store new content via Block Engine
  2. Create version node
  3. Update CompactObject metadata (content_hash, modified_at)
  4. Decrement refcount on old content block
- [ ] Test: create object → update 3 times → list versions (expect 4) → rollback to version 2 → verify content matches version 2

**Note:** Provenance signatures (Ed25519) are deferred to Phase 13 (Security Hardening). For Phase 4, the `author` field stores a stub `AgentId` and the provenance chain is simplified.

**Key reference:** spaces.md §5.1 Merkle DAG; §5.2 Space Snapshots; §5.3 DAG Operations

**Acceptance:** `just run` shows version create/list/rollback cycle. `just check` passes.

### Step 7: Device-level transparent encryption

**What:** Every block written to the storage device is encrypted with a device-bound AES-256-GCM key before reaching the VirtIO-blk driver. This is the lowest encryption layer — it protects against physical access to the storage medium.

**Tasks:**
- [ ] Create `kernel/src/storage/crypto.rs`
- [ ] Implement AES-256-GCM encrypt/decrypt using a software implementation (no hardware AES on QEMU cortex-a72 without ARMv8 Crypto Extensions — use a `no_std` AES crate or a minimal software AES-256 implementation)
- [ ] Define `DeviceKeyManager` struct: active_key, epoch, key_source
- [ ] For Phase 4 on QEMU: use `PassphraseDerived` key source with a hardcoded test passphrase ("aios-dev-key") — real key derivation (Argon2id, hardware binding) is Phase 24
- [ ] Key derivation: SHA-256(passphrase + salt) → 32-byte device key (placeholder for Argon2id)
- [ ] Integrate encryption into Block Engine write path:
  1. After computing content hash and CRC checksum (on plaintext)
  2. Encrypt block envelope (header + data) with device key + random 12-byte nonce
  3. Store: `[nonce (12B) | ciphertext | auth_tag (16B)]`
  4. Write encrypted block to VirtIO-blk
- [ ] Integrate decryption into Block Engine read path:
  1. Read encrypted block from VirtIO-blk
  2. Extract nonce from block header
  3. Decrypt with device key, verify auth tag
  4. Verify CRC checksum (on decrypted plaintext)
  5. Return plaintext content
- [ ] Store device key epoch in superblock; support single-key mode (no rotation in Phase 4)
- [ ] Test: write encrypted block, read raw sector (verify ciphertext ≠ plaintext), read via Block Engine (verify decrypted content matches original)

**Note:** The nonce for each block can use a counter (epoch + block_sequence_number) to avoid nonce reuse. AES-GCM nonce reuse is catastrophic — the counter approach guarantees uniqueness as long as the epoch is correctly tracked. For Phase 4, the sequence number is derived from the block's offset in the data region.

**Key reference:** spaces.md §4.10 Device-Level Transparent Encryption; §4.10.1 Device Key Hierarchy; §4.10.2 Encryption in the Write Path

**Acceptance:** `just run` shows encrypted block write/read cycle, raw ciphertext verification logged. `just check` passes.

### Step 8: Space management and system space initialization

**What:** Implement space management (create, list, delete) and create the system spaces at boot. Spaces organize objects into security zones with metadata and quotas.

**Tasks:**
- [ ] Create `kernel/src/storage/space.rs`
- [ ] Define `Space` struct (per spaces.md §3.1): id, name, parent, security_zone, encryption (EncryptionState — DeviceOnly for all Phase 4 spaces), quota (SpaceQuota: max_objects, max_bytes), created_at, modified_at, object_count, total_size
- [ ] Space metadata stored in Block Engine as special objects (SpaceId → Space metadata)
- [ ] Implement `space_create(name, zone, quota) -> Result<SpaceId, StorageError>`
- [ ] Implement `space_list() -> Vec<Space>` (scan space metadata index)
- [ ] Implement `space_delete(id) -> Result<(), StorageError>` (only if empty)
- [ ] Implement `space_get(id) -> Result<Space, StorageError>`
- [ ] Create system spaces at Block Engine init (per spaces.md §3.2):
  - `system/` — Core zone, kernel-managed (config, audit, crash, credentials, services, identity)
  - `user/home/` — Personal zone (default personal space)
  - `ephemeral/` — Ephemeral zone (auto-cleaned on shutdown, no version history)
- [ ] Wire Space Storage initialization into `kernel/src/main.rs` boot sequence (after pool init, after service manager init)
- [ ] Register Space Storage as a kernel service via `service_register(b"space-storage", pid, channel_id)` (matches the existing `&[u8]` name + pid + channel signature from Phase 3)
- [ ] Test: verify system spaces created at boot, create a user space, list all spaces

**Note:** Per-space encryption (Personal, Collaborative zones) is Phase 13a. For Phase 4, all spaces use device-level encryption only (EncryptionState::DeviceOnly). The Collaborative security zone is defined but not implemented (no multi-identity support yet).

**Key reference:** spaces.md §3.1 Spaces; §3.2 System Spaces; §12 Implementation Order

**Acceptance:** `just run` shows system spaces created at boot, space list logged. `just check` passes.

-----

## Milestone 15 — POSIX Bridge, Compression & Gate (End of Week 5)

*Goal: Implement the POSIX compatibility bridge for BSD tool support, add block-level LZ4 compression, implement storage budget enforcement, and run end-to-end validation.*

### Step 9: POSIX bridge — path mapping and file operations

**What:** Implement the POSIX compatibility bridge that maps filesystem paths to Space API operations. BSD tools (`ls`, `cat`, `mkdir`, `rm`) work through this bridge.

**Tasks:**
- [ ] Create `kernel/src/storage/posix_bridge.rs`
- [ ] Implement path mapping (per spaces.md §9.1):
  - `/spaces/[name]/[path]` → space lookup + object access
  - `/home/user/` → `user/home/` space
  - `/tmp/` → `ephemeral/` space
- [ ] Define `PosixSpaceBridge` struct with mount table entries
- [ ] Implement `open(path, flags) -> Result<Fd, StorageError>`:
  1. Parse path → (SpaceId, object_path)
  2. Look up or create object (O_CREAT)
  3. Allocate file descriptor (bounded table, max 256 per process)
  4. Return Fd
- [ ] Implement `read(fd, buf, count) -> Result<usize, StorageError>`:
  1. Look up Fd → (ObjectId, offset)
  2. Read object content via Object Store
  3. Copy to buffer from current offset
  4. Advance offset
- [ ] Implement `write(fd, buf, count) -> Result<usize, StorageError>`:
  1. Look up Fd → ObjectId
  2. Read current content, modify at offset, write back (copy-on-write via new version)
  3. Update object via Object Store (creates new version)
- [ ] Implement `close(fd)`, `stat(path) -> Stat`, `readdir(path) -> Vec<DirEntry>`, `mkdir(path)`, `unlink(path)`
- [ ] Synthesize POSIX mode bits from security zone (per spaces.md §9.2): directories 0o755, files 0o644
- [ ] Wire POSIX file operations as IPC calls to the Space Storage service (dispatched via `IpcCall` to the space-storage channel, not as new syscall numbers — the 31 syscall table from Phase 3 is unchanged)
- [ ] Test: create file via POSIX open+write, read back via POSIX open+read, mkdir, readdir, stat, unlink

**Note:** For Phase 4, POSIX operations are dispatched via IPC to the Space Storage service. The syscall handler translates POSIX calls into `IpcCall` messages and blocks on the reply (synchronous from the caller's perspective), but the actual storage operations run in the Space Storage service thread, not in-kernel. Phase 15 (POSIX Compatibility & BSD Userland) adds the full POSIX process model with fork/exec.

**Key reference:** spaces.md §9 POSIX Compatibility; §9.1 Path Mapping; §9.2 Translation Layer

**Acceptance:** `just run` shows POSIX file create/read/write/stat/readdir cycle. `just check` passes.

### Step 10: Block-level compression

**What:** Add LZ4 compression to the Block Engine. Blocks are compressed before encryption. Adaptive selection skips incompressible content (images, already-compressed data).

**Tasks:**
- [ ] Add LZ4 compression support (use `lz4_flex` crate — `no_std` compatible, pure Rust)
- [ ] Define `CompressionStrategy` enum: None, Lz4
- [ ] Add compression header to block format: `[compression_type (1B) | uncompressed_size (4B) | compressed_data...]`
- [ ] Integrate compression into Block Engine write path (before encryption):
  1. Attempt LZ4 compression
  2. If compressed size >= original size × 0.9 → store uncompressed (skip incompressible content)
  3. Store compression type in block header
- [ ] Integrate decompression into Block Engine read path (after decryption):
  1. Read compression type from header
  2. Decompress if needed
  3. Verify uncompressed size matches header
- [ ] Classify content types for compression eligibility (per spaces.md §4.6):
  - Always compress: Text, Code, Markdown, Json, Xml, Document
  - Never compress: Image, Video, Audio, Binary (already compressed)
  - Auto-detect: other types (try compress, skip if ratio < 0.9)
- [ ] Log compression stats: compressed/uncompressed sizes, ratio, skipped count
- [ ] Test: write text content (expect ~2:1 compression), write random bytes (expect skip), verify read-back correctness

**Key reference:** spaces.md §4.6 Block-Level Compression

**Acceptance:** `just run` shows compression ratios logged for different content types. `just check` passes.

### Step 11: Storage budget and quota enforcement

**What:** Implement storage budget tracking and per-space quota enforcement. Monitor free space and raise pressure events when thresholds are crossed.

**Tasks:**
- [ ] Create `kernel/src/storage/budget.rs`
- [ ] Define `StorageBudget` struct: total_bytes, used_bytes, free_bytes, category breakdowns (system, user, version_history, indexes)
- [ ] Define `PressureLevel` enum: Normal (>20% free), Warning (10-20%), Critical (5-10%), Emergency (<5%)
- [ ] Implement `check_pressure() -> PressureLevel`: compute from free_bytes / total_bytes
- [ ] Implement per-space quota enforcement in Object Store:
  - Before `object_create`: check space quota (max_objects, max_bytes)
  - Return `StorageError::QuotaExceeded` if over limit
- [ ] Track Block Engine utilization: data blocks used, WAL usage, index size
- [ ] Implement `storage_stats() -> StorageBudget`: aggregate all usage
- [ ] Expose storage stats via IPC to the space-storage service (clients query budget information via `IpcCall`, not a separate syscall — the 31 syscall table from Phase 3 is unchanged)
- [ ] Log pressure level changes via klog
- [ ] Test: create space with 1 KB quota, write objects until quota exceeded, verify error

**Note:** Adaptive version retention (pruning old versions under pressure) is Phase 4k per the implementation order. For Phase 4, quota enforcement prevents overcommit but does not actively reclaim. GC reclaims unreferenced blocks.

**Key reference:** spaces.md §10 Storage Budget and Pressure Management; §10.5 Storage Pressure Response

**Acceptance:** `just run` shows storage budget stats, quota enforcement on test space. `just check` passes.

### Step 12: End-to-end validation and quality gates

**What:** Run the complete storage stack end-to-end: POSIX create → Object Store → Version Store → Block Engine → VirtIO-blk → encrypted disk. Verify all quality gates.

**Tasks:**
- [ ] End-to-end test sequence:
  1. Boot → VirtIO-blk probed → Block Engine initialized (format or WAL replay)
  2. System spaces created (system/, user/home/, ephemeral/)
  3. POSIX open("/spaces/user/home/test.txt", O_CREAT | O_WRONLY) → write "Hello, AIOS!"
  4. POSIX close → verify version created in Version Store
  5. POSIX open("/spaces/user/home/test.txt", O_RDONLY) → read → verify "Hello, AIOS!"
  6. Update file → verify new version (2 versions total)
  7. Rollback to version 1 → verify original content
  8. Create duplicate content object → verify dedup (refcount = 2)
  9. Delete one copy → verify other still readable (refcount = 1)
  10. Verify raw disk sector is encrypted (read sector, compare to plaintext)
  11. Log storage budget: used, free, compression ratio
- [ ] Run `just check` — zero warnings, zero errors
- [ ] Run `just test` — all unit tests pass (existing + new storage type tests)
- [ ] Run `just run` — UART output shows complete storage lifecycle
- [ ] Update CLAUDE.md: Workspace Layout (add storage/, drivers/), Key Technical Facts (VirtIO address, block size, encryption, storage types)
- [ ] Update phase doc: check off M13–M15 boxes, update Status field
- [ ] Update developer-guide.md: add storage module file sizes, test counts

**Acceptance:** All quality gates pass. UART shows end-to-end storage lifecycle: VirtIO probe → Block Engine init → system spaces → POSIX create/read/update/rollback → dedup → encryption verification → storage budget.

-----

## Decision Points

| Decision | When | Options | Impact |
|---|---|---|---|
| MemTable-only vs full SSTable LSM-tree | M13 Step 4 | MemTable-only (Phase 4) vs SSTables (Phase 14) | MemTable-only limits index to ~65K entries (sufficient for Phase 4-8 workloads); SSTables needed for production |
| Software AES vs hardware AES | M14 Step 7 | Software (portable) vs ARMv8 Crypto Extensions | Software is ~10x slower but works everywhere; hardware AES deferred to Phase 14 optimization |
| Block size for VirtIO | M13 Step 2 | 512B (standard) vs 4 KB (optimal for flash) | Use 512B sectors, accumulate into 4 KB blocks internally; matches QEMU default |
| Compression library | M15 Step 10 | `lz4_flex` (pure Rust, fast) vs `miniz_oxide` (zlib, better ratio) | LZ4 for Phase 4 (speed over ratio); zstd deferred to optimization phase |
| UUID generation quality | M14 Step 5 | Timer-based (Phase 4) vs crypto-random (Phase 13+) | Timer entropy is sufficient for uniqueness but not unguessability; upgrade in Phase 13 |

-----

## Phase Completion Criteria

- [x] **M13 complete:** VirtIO-blk driver reads/writes sectors; Block Engine with WAL and MemTable index; superblock persists across boots
- [ ] **M14 complete:** Content-addressed objects with dedup; Merkle DAG version history; device-level AES-256-GCM encryption; system spaces at boot
- [ ] **M15 complete:** POSIX bridge maps paths to spaces; LZ4 compression active; storage budget enforced; end-to-end create/read/update/rollback/dedup/encrypt verified
- [ ] `just check` — zero warnings, zero errors
- [ ] `just test` — all shared crate tests pass (existing + new storage type tests)
- [ ] `just run` — UART shows complete storage lifecycle through all layers
- [ ] All code reviewed (convention compliance, unsafe documentation, W^X)
- [ ] All docs updated (CLAUDE.md, phase doc, developer-guide.md)
