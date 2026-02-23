# AIOS Space Storage System

## Deep Technical Architecture

**Parent document:** [architecture.md](../project/architecture.md)
**Related:** [airs.md](../intelligence/airs.md) — AI Runtime Service (Space Indexer), [ipc.md](../kernel/ipc.md) — Syscall interface

-----

## 1. Core Insight

Every operating system has a storage abstraction. Unix has files in directories. Windows has files in folders. Both are hierarchical path-based systems designed in the 1970s for humans who navigate by remembering where they put things.

AIOS replaces this with **spaces** — collections of typed objects with semantic relationships, content-addressed storage, full version history, and AI-maintained indexes. Users find things by meaning, not by path. The AI maintains the organization. The storage system provides integrity, versioning, and encryption as primitives, not afterthoughts.

-----

## 2. Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Space API                                  │
│  query()  create()  relate()  version()  search()            │
│  similar_to()  traverse()  subscribe()  import()  export()   │
│  (what agents and the POSIX bridge see)                      │
├─────────────────────────────────────────────────────────────┤
│                    Query Engine                               │
│  SpaceQuery dispatch (Filter, TextSearch, Semantic, Traverse)│
│  Full-text index (inverted, BM25) — available from Phase 9a  │
│  Embedding index (HNSW) — requires AIRS                      │
│  Relationship graph (adjacency lists, bidirectional)         │
│  Temporal index (B-tree on timestamps)                       │
├─────────────────────────────────────────────────────────────┤
│                    Object Store                               │
│  Object metadata (ObjectId → content_hash, type, semantic)   │
│  Relation store (ObjectId → Vec<Relation>)                   │
│  Content-addressed blocks (SHA-256 hash → data)              │
│  Reference counting (hash → ref_count)                       │
│  Deduplication (automatic, transparent)                      │
├─────────────────────────────────────────────────────────────┤
│                    Version Store                              │
│  Merkle DAG (git-like)                                       │
│  Per-object version chains (ObjectId → Vec<Version>)         │
│  Per-space snapshots (SpaceId → Vec<Snapshot>)               │
│  Provenance chain per version (who, when, why)               │
├─────────────────────────────────────────────────────────────┤
│                    Encryption Layer                           │
│  Per-space encryption keys (AES-256-GCM)                     │
│  Key derivation from identity (Argon2id)                     │
│  No key escrow — prevention-based recovery design (§6.3)     │
│  Transparent encrypt/decrypt on read/write                   │
├─────────────────────────────────────────────────────────────┤
│                    Block Engine                               │
│  LSM-tree indexed blocks on raw storage device               │
│  Write-ahead log (WAL) for crash consistency                 │
│  Flash-aware zone allocation (hot/warm/cold separation)      │
│  Device-level transparent encryption (AES-256-GCM)           │
│  Sub-block dedup (Rabin rolling hash, content-defined chunks)│
│  Block-level checksums (CRC-32C), WAF tracking               │
│  No intermediate filesystem — AIOS owns the device           │
├─────────────────────────────────────────────────────────────┤
│                    Storage Drivers                            │
│  Core: VirtIO-Blk (QEMU) │ SD/eMMC │ Apple ANS              │
│  Extension: NVMe (via PCIe) │ USB Mass Storage (via USB)    │
└─────────────────────────────────────────────────────────────┘
```

-----

## 3. Core Data Structures

### 3.0 Primitive Types

All data structures in this document use the following base types. Sizes are chosen for content-addressed storage efficiency and cross-device compatibility:

```rust
/// Cryptographic hash (SHA-256). 32 bytes (256 bits). Used for content
/// addressing, Merkle chain linking, and deduplication.
pub struct Hash([u8; 32]);

/// Unique identifier for an object within a space. 128-bit UUID (v4).
/// Stable across versions — the same ObjectId refers to all versions of
/// an object. Never reused after deletion.
///
/// NOTE: Shown as a type alias for clarity. In implementation, these MUST be
/// newtypes (e.g., `pub struct ObjectId([u8; 16]);`) to prevent accidentally
/// passing an ObjectId where a SpaceId is expected. The compiler cannot
/// distinguish `[u8; 16]` aliases — newtypes provide compile-time safety.
pub type ObjectId = [u8; 16];         // UUID v4

/// Unique identifier for a space. 128-bit UUID (v4).
/// Implementation: newtype `pub struct SpaceId([u8; 16]);`
pub type SpaceId = [u8; 16];

/// Agent or service identity. Derived from the agent's Ed25519 public key.
/// Implementation: newtype `pub struct AgentId([u8; 32]);`
pub type AgentId = [u8; 32];          // Ed25519 public key

/// User identity. Each identity has an Ed25519 keypair for provenance signing.
/// Implementation: newtype `pub struct IdentityId([u8; 32]);`
pub type IdentityId = [u8; 32];       // Ed25519 public key

/// Monotonic timestamp wrapper. Milliseconds since Unix epoch.
/// Newtype struct (not a type alias) so it can carry associated functions.
pub struct Timestamp(pub u64);

impl Timestamp {
    pub fn now() -> Self { Timestamp(/* kernel monotonic clock ms */) }
    pub fn as_millis(&self) -> u64 { self.0 }
}

/// Task identifier for provenance tracking. Ties an action to an
/// agent's in-progress task (if applicable).
pub type TaskId = [u8; 16];           // UUID v4

/// AI model identifier. Human-readable string (e.g., "llama-3.1-8b-q4").
pub type ModelId = String;

/// Encryption key version for rotation tracking (§6.1).
pub type KeyId = u32;

/// POSIX file descriptor for the POSIX compatibility layer (§9).
pub type Fd = u32;

/// Snapshot identifier. UUID v4.
pub type SnapshotId = [u8; 16];

/// LSM-tree SSTable identifier. Monotonically increasing per level.
pub type SsTableId = u64;

/// Content-addressed block identifier. Same as the block's content hash
/// (SHA-256). Two blocks with identical content have the same BlockId.
pub type BlockId = Hash;

/// Signature type (Ed25519, 64 bytes).
pub type Signature = [u8; 64];

/// Physical location of a block on the storage device.
pub struct BlockLocation {
    /// Byte offset on the raw device partition.
    offset: u64,
    /// Total block size in bytes (header + compressed data).
    size: u32,
    /// Which temperature tier this block resides in (defined in §4.7).
    tier: StorageTier,
}

/// Serialization format: all structures in this document are serialized
/// with Bincode (compact Rust binary encoding) for LSM-tree storage and
/// WAL entries. Cross-device sync (§8) uses MessagePack for interoperability.
```

### 3.1 Spaces

```rust
pub struct Space {
    id: SpaceId,
    name: String,
    parent: Option<SpaceId>,            // space hierarchy (not path hierarchy)
    security_zone: SecurityZone,
    encryption: EncryptionState,
    quota: SpaceQuota,
    created_at: Timestamp,
    modified_at: Timestamp,
    object_count: u64,
    total_size: u64,
}

pub enum SecurityZone {
    /// Kernel and system services only
    Core,
    /// User's personal data, encrypted
    Personal,
    /// Shared with specific identities
    Collaborative { members: Vec<IdentityId> },
    /// Data from untrusted sources (web, unknown agents)
    Untrusted,
    /// Ephemeral data (/tmp), auto-cleaned on shutdown
    Ephemeral,
}

pub enum EncryptionState {
    /// Device-encrypted only (Core, Ephemeral zones).
    /// All blocks are encrypted with the device key (§4.10) regardless of this field.
    /// DeviceOnly means no additional per-space encryption layer.
    DeviceOnly,
    /// Device-encrypted AND per-space encrypted (Personal, Collaborative, Untrusted).
    /// Content is encrypted with the space key before reaching the Block Engine,
    /// then the Block Engine encrypts the block envelope with the device key.
    SpaceEncrypted {
        algorithm: EncryptionAlgorithm,
        key_id: KeyId,
    },
}

pub struct SpaceQuota {
    max_objects: Option<u64>,
    max_bytes: Option<u64>,
    max_versions_per_object: Option<u32>,
    version_retention: RetentionPolicy,
}

pub enum RetentionPolicy {
    KeepAll,                            // never prune versions
    KeepLast(u32),                      // keep last N versions
    KeepDuration(Duration),             // keep versions for N days
    KeepSize(u64),                      // keep versions within byte budget
}
```

### 3.2 System Spaces

AIOS creates these spaces at first boot:

```
system/                      ← Core zone, kernel-managed
  devices/                   ← Device registry (subsystem framework)
  audit/                     ← Audit logs (per-subsystem)
    network/
    audio/
    camera/
    input/
    ...
  models/                    ← AI model storage (AIRS)
  index/                     ← Search indexes (AIRS)
    embeddings/              ← HNSW embedding index
    fulltext/                ← Inverted index
  crash/                     ← Kernel panic logs
  config/                    ← System configuration
  credentials/               ← Credential store (NTM)
  agents/                    ← Installed agent manifests
  context/                   ← Context Engine learned patterns
  services/                  ← Service binaries (Phase 3-5, loaded from storage)
  session/                   ← Semantic snapshots, boot traces, proactive wake data
  identity/                  ← Identity keypairs and authentication state

user/                        ← Personal zone, encrypted
  home/                      ← Default personal space
  documents/
  media/
  conversations/             ← Conversation bar history
  preferences/               ← User preferences

web-storage/                 ← Untrusted zone
  [origin]/                  ← Per-origin web data
    cookies/
    local/
    indexed-db/
    cache-api/
    session/

shared/                      ← Collaborative zone
  [space-name]/              ← User-created shared spaces
```

### 3.3 Objects

```rust
pub struct Object {
    id: ObjectId,                       // stable UUID, never changes
    name: String,                       // human-readable name (last path component)
    content_hash: Hash,                 // SHA-256, changes with content
    content_type: ContentType,
    content_size: u64,                  // bytes
    semantic: SemanticMetadata,
    relations: Vec<Relation>,
    created_at: Timestamp,
    modified_at: Timestamp,
    created_by: AgentId,
    modified_by: AgentId,
    provenance: ProvenanceChain,
}

pub struct SemanticMetadata {
    /// Always available (set by creator or AIRS)
    summary: Option<String>,
    tags: Vec<String>,
    entities: Vec<Entity>,
    description: Option<String>,

    /// Requires AIRS (generated by Space Indexer)
    embedding: Option<Vec<f32>>,        // ~384 dimensions
    auto_tags: Vec<String>,             // AI-generated tags
    auto_summary: Option<String>,       // AI-generated summary

    /// Full-text index metadata (always maintained)
    text_content: Option<String>,       // extracted text for FTI
    indexed_at: Option<Timestamp>,
}

pub struct Entity {
    name: String,
    entity_type: EntityType,
    confidence: f32,
}

pub enum EntityType {
    Person, Organization, Location, Date, Concept, Technology, Event,
}

/// Content type of an object. Determines compression strategy, promotion
/// eligibility, and POSIX mode bit synthesis.
pub enum ContentType {
    // Structural
    Directory,              // container (POSIX mkdir creates these)

    // Documents
    Document,               // rich document (PDF, DOCX, etc.)
    Text,                   // plain text
    Code,                   // source code
    Markdown,
    Json,
    Xml,

    // Media
    Image,                  // JPEG, PNG, WebP, etc.
    Video,                  // MP4, WebM, etc.
    Audio,                  // MP3, FLAC, etc.

    // System
    Config,                 // configuration file (exempt from promotion)
    Credential,             // secret material (never promoted, never synced)
    Executable,             // binary or script

    // Agent-specific
    GameSave,               // game state (exempt from promotion)
    CacheEntry,             // cache artifact (exempt from promotion)
    SessionToken,           // session state (exempt from promotion)
    Cookie,                 // browser cookie (exempt from promotion)

    // Catch-all
    Binary,                 // unknown binary format
}
```

### 3.3.1 Compact vs Full Objects

By default, every new object is created as a **CompactObject** — a lightweight representation with minimal metadata overhead. Compact objects are promoted to full `Object` representation when the system determines they would benefit from rich metadata.

This is the storage-conservative default. On constrained devices, the overhead of embeddings (~1.5 KB per object at 384 dimensions × f32), provenance chains, AI-generated summaries, and entity extraction adds up quickly across thousands of objects. Most objects (config files, small notes, web storage entries, game saves, temp files) never benefit from this metadata.

```rust
/// Lightweight object representation — the default for all new objects.
/// Supports text search and basic queries. No embedding, no AI metadata.
pub struct CompactObject {
    id: ObjectId,
    /// Human-readable name (last path component). Required for POSIX directory
    /// listings and name-based lookup even before promotion to full Object.
    name: String,
    content_hash: Hash,
    content_type: ContentType,
    content_size: u64,
    created_at: Timestamp,
    modified_at: Timestamp,
    created_by: AgentId,
    modified_by: AgentId,
    /// Minimal text for full-text index (always maintained)
    text_content: Option<String>,
}

/// Promotion criteria — when a CompactObject becomes a full Object
pub struct PromotionPolicy {
    /// Promote when a user explicitly searches for and opens the object
    on_user_interaction: bool,          // default: true
    /// Promote when the object is edited more than N times
    edit_threshold: u32,                // default: 3
    /// Promote when the object exceeds N bytes (suggests meaningful content)
    size_threshold: u64,                // default: 4 KB
    /// Promote when another object creates a Relation to this one
    on_relation_created: bool,          // default: true
    /// Never promote these content types (even if other criteria are met)
    exempt_types: Vec<ContentType>,     // default: [Config, Credential, GameSave, CacheEntry, SessionToken, Cookie]
}

impl PromotionPolicy {
    pub fn default() -> Self {
        Self {
            on_user_interaction: true,
            edit_threshold: 3,
            size_threshold: 4 * KB,
            on_relation_created: true,
            exempt_types: vec![
                ContentType::Config,
                ContentType::Credential,
                ContentType::GameSave,
                ContentType::CacheEntry,
                ContentType::SessionToken,
                ContentType::Cookie,
            ],
        }
    }
}
```

**Promotion flow:**
```
1. Object created → stored as CompactObject
   (text extracted for full-text index, no embedding, no AI metadata)
     ↓
2. Promotion trigger fires (user opens object, edit threshold, size threshold)
     ↓
3. Space Indexer queues the object for full indexing:
   - Generate embedding vector (384 dimensions)
   - Extract entities (people, places, concepts)
   - Generate AI summary and tags
   - Build provenance chain
     ↓
4. Object promoted to full Object in-place (same ObjectId, same content_hash)
```

**Promotion logic:** An object is promoted when its type is NOT in `exempt_types` AND **any one** of the following (OR logic): the user opens the object (`on_user_interaction`), edits exceed `edit_threshold`, size exceeds `size_threshold`, or another object creates a Relation to it (`on_relation_created`). Promotion is triggered lazily during access — rarely-used objects stay compact.

**Promotion atomicity:** Promotion is a single LSM-tree metadata update. The Space Indexer generates embedding, entities, and AI summary offline. Once ready, it writes a single Version node that adds the rich metadata to the object (same ObjectId, same content_hash). If the object is modified during embedding generation, the promotion applies to the version that was current when generation started; newer versions are promoted separately. If the object is deleted mid-promotion, the promotion is abandoned — no partial metadata is written.

**Storage savings:** A CompactObject uses ~200 bytes of metadata (10 fields × variable encoding) vs ~2-6 KB for a full Object (with embedding at 384 × f32 = 1536 bytes + provenance + AI metadata). For a space with 10,000 objects where 80% remain compact, this saves 14-46 MB of metadata overhead — significant on a device with a 32 GB SD card.

**CompactObjects are still searchable.** Full-text search works on compact objects (the text index is always maintained). Semantic search (embedding-based) only works on promoted full objects. This means the system is fully functional with compact defaults — semantic search coverage grows organically as users interact with their data.

**Web storage is always compact.** Objects in `web-storage/` spaces (cookies, localStorage, sessionStorage, IndexedDB entries, Cache API responses) are never promoted. They are high-volume, low-value for semantic search, and typically accessed by origin rather than by meaning. Policy-enforced: `PromotionPolicy.exempt_types` includes all web storage content types (`Cookie`, `SessionToken`, `CacheEntry`), blocking promotion regardless of other triggers.

### 3.4 Relations

```rust
pub struct Relation {
    source: ObjectId,
    target: ObjectId,
    kind: RelationKind,
    confidence: f32,                    // 1.0 for explicit, <1.0 for AI-inferred
    explanation: Option<String>,
    created_by: RelationSource,
}

pub enum RelationKind {
    DerivedFrom,                        // this was created from that
    References,                         // this mentions/links to that
    DependsOn,                          // this needs that to work
    RelatedTo,                          // general semantic similarity
    CreatedBy,                          // agent or user that created this
    InputTo,                            // this was input to a task
    OutputOf,                           // this was output of a task
    ConversationContext,                // used as context in a conversation
    VersionOf,                          // different version of same content
    SiblingOf,                          // share a common source
    ChildOf,                            // hierarchical containment
    Attachment,                         // embedded/attached to parent
}

pub enum RelationSource {
    Explicit(AgentId),                  // agent or user created this relation
    AiInferred,                         // AIRS Space Indexer inferred it
    SystemGenerated,                    // OS created it (versioning, provenance)
}
```

**RelationSource examples:** `Explicit(agent_id)` — user or agent manually links document A → document B. `AiInferred` — Space Indexer detects semantic similarity and creates a RelatedTo edge. `SystemGenerated` — versioning system creates a VersionOf relation when content is derived from another object.

Relations are bidirectional in storage — creating `A → References → B` also indexes `B ← ReferencedBy → A`. The relationship graph is stored as adjacency lists with both forward and reverse edges.

-----

## 4. Block Engine

### 4.1 On-Disk Layout

The Block Engine manages raw storage directly — no ext4, no ZFS, no intermediate filesystem. AIOS owns the partition.

```
┌──────────────────────────────────────────────────────────┐
│  Superblock (4 KB)                                        │
│  Magic, version, block size, total blocks, free blocks,   │
│  LSM-tree L0 offset, WAL offset, checksum                 │
├──────────────────────────────────────────────────────────┤
│  Write-Ahead Log (configurable, default 64 MB)            │
│  Circular buffer of pending writes                        │
│  Each entry: block_id, old_data, new_data, checksum       │
├──────────────────────────────────────────────────────────┤
│  Block Index (LSM-tree)                                   │
│  Maps: content_hash → (block_offset, block_size, refcount)│
│  Also maps: ObjectId → (metadata_block, content_hash)     │
│  L0: in-memory MemTable (sorted, ~4 MB)                   │
│  L1-L3: on-disk SSTables with bloom filters               │
├──────────────────────────────────────────────────────────┤
│  Data Blocks (remainder of partition)                      │
│  Content-addressed blocks, variable size                  │
│  Each block: header (hash, size, checksum) + data          │
│  Hot/cold zone separation for flash-friendly write patterns│
└──────────────────────────────────────────────────────────┘
```

**Why LSM-tree instead of B-tree?** Flash storage refers to NAND-based devices (SD cards, eMMC, SSDs, NVMe) with erase-before-write constraints. Traditional write patterns (random, in-place updates) cause high write amplification on flash. The Block Engine's index was originally designed as a B-tree. B-trees are excellent for read-heavy workloads with random access, but on flash storage, B-tree updates cause **random writes** — each index update modifies an arbitrary node in the tree, requiring a read-modify-write cycle on the flash translation layer. This causes write amplification (WAF 10-30x on SD cards) and accelerates flash wear.

An LSM-tree (Log-Structured Merge-tree) converts random writes into sequential writes:

```rust
/// LSM-tree block index: all writes go to an in-memory MemTable,
/// which is periodically flushed as an immutable SSTable to disk.
/// Sequential writes only — no in-place updates on flash.
pub struct LsmBlockIndex {
    /// Active MemTable (sorted in-memory tree, receives all writes)
    memtable: MemTable,
    /// Immutable MemTable being flushed to disk (if any)
    immutable_memtable: Option<MemTable>,
    /// On-disk levels of sorted SSTables
    levels: [Vec<SSTable>; LSM_MAX_LEVELS],
    /// Bloom filters per SSTable (avoid unnecessary disk reads).
    /// Standard probabilistic structure: 10 bits per key, ~1% false positive rate.
    /// Implementation: `bloom` crate (or equivalent).
    bloom_filters: HashMap<SsTableId, BloomFilter>,
    /// Write amplification tracker (§4.8)
    waf_tracker: WriteAmplificationTracker,
}

const LSM_MAX_LEVELS: usize = 4; // L0 (flushed MemTables, unsorted) + L1-L3 (sorted, compacted)
const MEMTABLE_SIZE: usize = 4 * MB; // Flush to disk when full

pub struct MemTable {
    /// Sorted key-value pairs (content_hash → BlockLocation)
    entries: BTreeMap<Hash, BlockLocation>,
    /// Current size in bytes
    size: usize,
}

pub struct SSTable {
    /// On-disk sorted table of key-value pairs
    id: SsTableId,
    /// Level in the LSM-tree (0-3)
    level: u8,
    /// Key range [min_key, max_key] for binary search across SSTables
    key_range: (Hash, Hash),
    /// Disk offset and size
    offset: u64,
    size: u64,
    /// Number of entries
    entry_count: u64,
}
```

**LSM-tree write path:**

```
Index update (e.g., new block stored):
  1. Insert (content_hash, block_location) into MemTable (in-memory, O(log n))
  2. If MemTable size >= 4 MB:
     a. Freeze current MemTable → immutable_memtable
     b. Create new empty MemTable for incoming writes
     c. Background: flush immutable_memtable to disk as L0 SSTable
        → single sequential write (flash-friendly)
  3. If L0 has too many SSTables (> 4):
     a. Background: merge L0 SSTables into L1 (compaction)
     b. Compaction produces sorted, deduplicated SSTables
     c. Old L0 SSTables deleted after compaction completes
  4. Same compaction process for L1 → L2, L2 → L3 when threshold exceeded
```

**LSM-tree read path:**

```
Index lookup (e.g., find block for content_hash):
  1. Check MemTable (in-memory, O(log n)) → found? return
  2. Check immutable_memtable (if exists) → found? return
  3. For each L0 SSTable (unsorted, may overlap):
     a. Check bloom filter → skip or binary search within SSTable
     b. Found? return
  4. For each level L1, L2, L3:
     a. Check bloom filter: is key possibly in this SSTable?
        → NO (~99% of non-matches): skip SSTable entirely
        → YES: binary search within SSTable
     b. Found? return
  5. Key not found (block doesn't exist)
```

**Read performance:** Bloom filters (10 bits per key, ~1% false positive rate) ensure that reads rarely touch disk unnecessarily. A typical lookup checks the MemTable (microseconds), then 1-2 bloom filters (microseconds), and at most one SSTable disk read. On average, LSM-tree reads are within 2x of B-tree reads — a small price for 10-30x write amplification reduction.

**Compaction scheduling:** Compaction runs at the Block Engine's lowest internal I/O priority (distinct from CPU scheduling classes — see scheduler.md §3.1) and is paused during active inference (when SD card bandwidth is needed for model loading). On battery-powered future devices, compaction can be deferred to charging periods.

**SSTable manifest for crash safety:** Production LSM-trees (LevelDB, RocksDB) use a manifest file to track which SSTables are live. AIOS maintains an `SsTableManifest` that records the current set of valid SSTables per level:

```rust
pub struct SsTableManifest {
    /// Current live SSTables per level
    levels: [Vec<SsTableId>; LSM_MAX_LEVELS],
    /// Manifest version (incremented on every compaction)
    version: u64,
    /// Written atomically to a dedicated manifest block on disk.
    /// On crash recovery, the manifest identifies which SSTables are
    /// live and which are orphaned (partially-written compaction output).
    /// Orphaned SSTables are deleted during recovery.
}
```

A crash during compaction (between writing new SSTables and updating the manifest) leaves orphaned SSTable files on disk. Recovery detects these by comparing on-disk SSTables against the manifest and deletes any not listed. The old SSTables (compaction inputs) remain valid until the manifest atomically switches to the new set.

**WAL captures index entries:** The WAL entry format includes both data block writes and their corresponding LSM-tree index entries. On crash recovery, the WAL replay re-inserts any index entries that were in the MemTable at crash time but not yet flushed to an SSTable:

```
WAL entry format (extended for LSM-tree):
  block_id | new_data | content_hash | block_location | checksum
                        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
                        These fields reconstruct the MemTable entry
                        during crash recovery.
```

**Tombstone handling for block deletion:** When a block is deleted (refcount reaches 0), the LSM-tree writes a tombstone marker instead of immediately removing the entry:

```rust
pub enum IndexEntry {
    /// Live block: content_hash maps to a block location
    Live(BlockLocation),
    /// Tombstone: block was deleted. Shadows any Live entry in lower levels.
    /// Removed during compaction when no lower level contains the key.
    Tombstone,
}
```

Without tombstones, a deleted key in L1 would still be found in L2 or L3 during reads, returning a block that should have been garbage collected. Tombstones are written to the MemTable and flushed to SSTables like normal entries. Compaction removes tombstones when no lower level contains the corresponding key.

**Write stall for compaction backlog:** If compaction falls behind (e.g., paused during inference, or sustained burst of writes), L0 accumulates unbounded SSTables. AIOS implements write stalling to prevent this:

```rust
impl LsmBlockIndex {
    const L0_SLOWDOWN_THRESHOLD: usize = 8;  // 8 SSTables in L0: slow writes
    const L0_STOP_THRESHOLD: usize = 12;     // 12 SSTables in L0: stall writes

    fn check_write_stall(&self) -> WriteStallAction {
        let l0_count = self.levels[0].len();
        if l0_count >= Self::L0_STOP_THRESHOLD {
            WriteStallAction::Stall  // block writes until compaction catches up
        } else if l0_count >= Self::L0_SLOWDOWN_THRESHOLD {
            WriteStallAction::Slowdown  // rate-limit writes to compaction throughput
        } else {
            WriteStallAction::None
        }
    }
}
```

```rust
pub enum WriteStallAction {
    /// Proceed normally — compaction is keeping up.
    None,
    /// Rate-limit writes to match compaction throughput.
    Slowdown,
    /// Block all writes until compaction reduces L0 below threshold.
    Stall,
}

pub enum AllocError {
    /// Target zone and all overflow zones are full.
    ZoneFull,
    /// Entire device is full (even after zone compaction).
    DeviceFull,
    /// Requested allocation exceeds maximum block size.
    InvalidSize,
}

pub enum StorageError {
    /// Block not found at expected location.
    BlockNotFound,
    /// CRC-32C checksum failed on read.
    ChecksumFailed,
    /// AES-256-GCM authentication tag verification failed.
    DecryptionFailed,
    /// Block Engine I/O error (driver-level).
    IoError(String),
    /// Space or category quota exceeded.
    QuotaExceeded,
}

/// Space-level error enum. Used by Object Store, Version Store, POSIX bridge,
/// and device key manager. Maps to POSIX errno via §9.5.
pub enum Error {
    ObjectNotFound,
    SpaceNotFound,
    CapabilityDenied,
    ReadOnlySpace,
    SpaceFull,
    DeviceFull,
    ObjectLocked,
    InvalidPath,
    NameExists,
    TooManyOpenFiles,
    VersionConflict,
    EncryptionKeyUnavailable,
    UnknownKeyEpoch(u64),
    IoError(StorageError),
}
```

When stalled, the Block Engine queues incoming writes in the WAL (which is sequential and low-WAF) and returns to the caller once the WAL entry is fsynced. The writes are applied to the MemTable when compaction reduces L0 below the threshold. Agents see slightly increased write latency during stalls but never lose data.

### 4.2 Write Path (Flash-Aware)

The write path is designed with **F2FS-style flash awareness** — writes are append-preferred and zone-separated to minimize flash wear and write amplification. Traditional filesystems scatter writes randomly across the device, causing the flash translation layer (FTL) to perform expensive read-modify-erase cycles. AIOS structures writes to work *with* the flash, not against it.

```
Agent writes object:
  1. Content hashed (SHA-256) → content_hash
  2. Check block index (LSM-tree): does content_hash already exist?
     YES → increment refcount, skip write (deduplication)
     NO  → continue to step 3
  3. Classify write temperature (hot/warm/cold) based on content type and access prediction
  4. WAL entry written: (new_block_id, content, metadata)
     → WAL is append-only circular buffer (sequential writes only)
  5. WAL entry fsynced to disk (crash-safe point)
  6. Data block written to temperature-appropriate zone:
     → Hot zone: recently created objects, frequently modified metadata
     → Warm zone: user data, recent version history, KV cache blocks
     → Cold zone: old version history, audit archives, model files
     → Append-preferred: new blocks written to the end of the zone's free region
  7. Block index updated via LSM-tree MemTable insertion (in-memory, no disk I/O)
  8. Object metadata updated: ObjectId → content_hash
  9. Version store appended: (ObjectId, content_hash, timestamp, agent_id)
 10. WAL entry marked committed
```

**Hot/cold zone separation:**

Flash storage wears unevenly when hot data (frequently modified) and cold data (rarely modified) share the same erase blocks. The FTL must copy cold data out of the way every time it erases a block to make room for hot writes. F2FS-style zone separation places hot and cold data on different regions of the device, reducing this unnecessary copying:

```rust
/// Zone allocation for flash-aware write placement
pub struct FlashZoneAllocator {
    /// Hot zone: metadata, recently written objects, active space indexes
    /// Expect high rewrite rate — placed on fresh erase blocks
    hot_zone: Zone,
    /// Warm zone: user data, version history < 30 days, KV cache blocks
    /// Moderate rewrite rate
    warm_zone: Zone,
    /// Cold zone: old versions, audit archives, model files, backups
    /// Rarely rewritten — placed on worn erase blocks (flash wear leveling)
    cold_zone: Zone,
    /// WAL zone: dedicated sequential write region
    wal_zone: Zone,
    /// Number of times a block was placed in an overflow zone
    /// (allocated to a different tier than requested because the target zone was full)
    overflow_count: u64,
}

pub struct Zone {
    /// Start and end offsets on the block device
    start: u64,
    end: u64,
    /// Next write position (append pointer)
    write_head: u64,
    /// Free space in this zone
    free_bytes: u64,
    /// Write count for WAF tracking
    bytes_written: u64,
}

impl Zone {
    /// Compact live blocks within this zone: walk from start to write_head,
    /// copy live blocks to the front, update write_head. Dead blocks (unreferenced
    /// by the LSM-tree index) are reclaimed. Called when zone free space is low.
    fn compact_live_blocks(&mut self) -> usize { /* implementation omitted */ 0 }
}

impl FlashZoneAllocator {
    /// Classify a write into a temperature zone
    fn zone_for_tier(&mut self, tier: StorageTier) -> &mut Zone {
        match tier {
            StorageTier::Hot => &mut self.hot_zone,
            StorageTier::Warm => &mut self.warm_zone,
            StorageTier::Cold => &mut self.cold_zone,
        }
    }

    /// Allocate space for a new block — always append-preferred.
    /// Returns the write offset within the appropriate zone.
    /// If the target zone is full, attempts zone overflow (steal from
    /// a colder zone with available space).
    /// Thread safety: the Block Engine serializes all writes through the WAL
    /// append lock, so allocate() is always called from a single writer thread.
    fn allocate(&mut self, size: usize, tier: StorageTier) -> Result<u64, AllocError> {
        // Try the target zone first
        let zone = self.zone_for_tier(tier);
        if zone.free_bytes >= size as u64 {
            let offset = zone.write_head;
            zone.write_head += size as u64;
            zone.free_bytes -= size as u64;
            zone.bytes_written += size as u64;
            return Ok(offset);
        }

        // Target zone full — attempt overflow allocation.
        // Hot can overflow into Warm; Warm can overflow into Cold.
        // Cold zone full is a true disk-full condition.
        let overflow_tier = match tier {
            StorageTier::Hot => Some(StorageTier::Warm),
            StorageTier::Warm => Some(StorageTier::Cold),
            StorageTier::Cold => None,
        };

        if let Some(fallback) = overflow_tier {
            let zone = self.zone_for_tier(fallback);
            if zone.free_bytes >= size as u64 {
                let offset = zone.write_head;
                zone.write_head += size as u64;
                zone.free_bytes -= size as u64;
                zone.bytes_written += size as u64;
                // Track overflow writes for zone rebalancing
                self.overflow_count += 1;
                return Ok(offset);
            }
        }

        // All zones exhausted — trigger zone-aware GC before failing
        Err(AllocError::ZoneFull)
    }

    /// Rebalance zones: compact live blocks within each zone to reclaim
    /// fragmented space from deleted blocks. Run by the GC (§4.5).
    fn compact_zone(&mut self, tier: StorageTier) -> usize {
        let zone = self.zone_for_tier(tier);
        // Walk the zone from start to write_head.
        // Live blocks (refcount > 0) are compacted toward the start.
        // Dead blocks (refcount == 0) are reclaimed.
        // Returns bytes reclaimed.
        // After compaction, write_head is reset to end of live data.
        zone.compact_live_blocks()
    }
}
```

**Why append-preferred writes matter for SD cards:**

```
Random write (B-tree index update, traditional filesystem):
  1. FTL reads entire erase block (128-512 KB) into buffer
  2. FTL modifies the target 4 KB page in buffer
  3. FTL erases the block (~2 ms, wears one P/E cycle)
  4. FTL writes back entire buffer (~1 ms)
  Total: ~3 ms, 128-512 KB written for a 4 KB change (WAF: 32-128x)

Append-preferred write (LSM-tree + zone allocation):
  1. New data appended to zone's write head (sequential)
  2. FTL writes directly to fresh page in current erase block
  3. No erase needed until block is full
  Total: ~0.1 ms, 4 KB written for a 4 KB change (WAF: ~1x)

On a consumer SD card (TLC, ~1000 P/E cycles):
  Random writes: card degradation in weeks of heavy use
  Append-preferred: card lasts years under same workload
```

### 4.3 Read Path

```
Agent reads object:
  1. Object metadata lookup: ObjectId → content_hash
  2. Block index lookup: content_hash → block location
  3. Read encrypted block from disk
  4. Decrypt block envelope with device key (§4.10) — always, every block
  5. Verify checksum (CRC-32C)
  6. If space-encrypted (Personal, Collaborative, Untrusted): decrypt content with space key
  7. Return content to agent
```

#### 4.3.1 AIRS Prefetch Path

AIRS resource orchestration can direct Space Storage to prefetch objects into the page cache before an agent requests them. Prefetch uses the **same read path** as normal agent reads — there is no shortcut that bypasses decryption, capability checks, or checksum verification.

```
AIRS prefetch directive:
  1. AIRS sends ResourcePrefetch { objects, reason, triggered_by } to kernel
  2. Kernel validates: AIRS holds ReadSpace capability for the target space
  3. Kernel forwards prefetch request to Space Storage
  4. Space Storage executes the NORMAL read path for each object:
     a. Object metadata lookup: ObjectId → content_hash
     b. Block index lookup: content_hash → block location
     c. Read encrypted block from disk
     d. Decrypt block envelope with device key (§4.10)
     e. Verify checksum (CRC-32C)
     f. If space-encrypted: decrypt content with space key (Space Storage holds key)
     g. Decrypted content sits in page cache (user pool)
  5. Error handling: if checksum verification fails at step (e):
     - The corrupted block is NOT placed in the page cache
     - An integrity error is logged to system/audit/ (CRC mismatch event)
     - The prefetch for this specific object is silently dropped
     - No error is propagated to AIRS (prefetch is advisory)
     - If the agent later reads this object, the normal read path will
       encounter the same corruption and return Err(IntegrityError) to the agent
  6. No content is returned to AIRS — prefetch is fire-and-forget
  7. When agent later reads the object, step 4 hits page cache → fast
  8. Provenance chain records: ResourcePrefetch event (logged by kernel)
```

**Why AIRS never touches keys:** AIRS does not hold space decryption keys. It does not need them. AIRS sends a directive to the kernel, which forwards it to Space Storage. Space Storage holds the space keys (released by the kernel after authentication + capability verification) and performs the decryption. The decrypted content enters the page cache, where it is accessible to any agent that holds the appropriate `ReadSpace` capability. AIRS's role is purely advisory — "this object will likely be needed soon" — not operational.

**Why no shortcut:** A prefetch shortcut that bypasses the normal read path would be a security regression:
- Skipping checksum verification would allow corrupted blocks into the page cache
- Skipping decryption would place encrypted blocks in the cache, useless to agents
- Skipping capability validation would allow AIRS to prefetch objects from spaces it shouldn't access
- Skipping provenance logging would hide AIRS's prefetch activity from the audit trail

The normal read path is the only read path. Prefetch is just "read it now instead of later."

### 4.4 Crash Recovery

On boot, the Block Engine replays the WAL:
```
1. Read superblock, verify integrity
2. Scan WAL from oldest uncommitted entry
3. For each uncommitted entry:
   - If data block was written but index not updated → update index
   - If data block was NOT written → discard entry
4. WAL is now clean
5. Verify block index consistency (background, non-blocking)
```

The WAL guarantees that the storage is always in a consistent state. A crash during any step of the write path is recoverable.

### 4.5 Garbage Collection

Content-addressed blocks are reference-counted. When an object is modified (content_hash changes) or deleted, the old block's refcount decreases. When refcount reaches zero, the block is eligible for GC:

```rust
pub struct GarbageCollector {
    /// Blocks with refcount 0, organized by zone for targeted reclamation
    pending_by_zone: [Vec<BlockId>; 3],  // [hot, warm, cold]
    /// Grace period before reclaiming (allows version history to reference old blocks)
    grace_period: Duration,
    /// Run GC when free space drops below threshold
    trigger_threshold: f64,         // fraction of total space
    /// Per-zone trigger: run zone-specific GC when a zone's free space is low
    zone_trigger_threshold: f64,    // default: 0.10 (10% free in any zone)
}
```

**Zone-aware GC:** When a specific zone runs low on space (e.g., the hot zone fills up from frequent metadata writes), GC targets that zone specifically — it reclaims dead blocks in the hot zone and optionally compacts live blocks to defragment the zone's append region. This is more efficient than global GC because:
- Only the affected zone is scanned (less I/O)
- Zone compaction restores the append-preferred write pattern for that zone
- Other zones are undisturbed (no unnecessary I/O on cold data)

GC runs in the background and never blocks reads or writes. When the zone allocator returns `AllocError::ZoneFull` (§4.2), the Block Engine triggers zone-specific GC before failing the write. If zone compaction cannot free enough space (all blocks in the zone are live), the write fails with `StorageError::QuotaExceeded` and the system raises a `StorageEvent::PressureChanged` (§10.5) to trigger emergency reclamation. Interactive writes (user-initiated actions) always take priority over background operations, so at least one write path remains available. The grace period (default: 24 hours) prevents GC from reclaiming blocks that old versions or open file descriptors (§9.4) may still reference. Under Critical/Emergency storage pressure (§10.5), the grace period is shortened to 1 hour/0 respectively — open file descriptors pinning old versions receive `EIO` on subsequent reads if their underlying blocks are reclaimed.

### 4.6 Block-Level Compression

AIOS compresses data blocks on disk to extend storage lifetime on capacity-constrained devices (SD cards, small SSDs). Compression operates at the block level — transparent to the Object Store and everything above it.

```rust
pub enum CompressionStrategy {
    /// No compression (already-compressed content: images, video, encrypted data)
    None,
    /// LZ4 — fast compression/decompression, moderate ratio (~2:1)
    /// Used for recently written and frequently accessed blocks
    Lz4,
    /// Zstd — slower compression, better ratio (~3-4:1)
    /// Used for cold data (old versions, inactive spaces, audit archives)
    Zstd { level: u8 },                // 1-19, default 3 for warm, 9 for cold
}

pub struct BlockHeader {
    content_hash: Hash,
    uncompressed_size: u32,
    compressed_size: u32,
    compression: CompressionStrategy,
    checksum: u32,                      // CRC-32C of compressed data (integrity for non-encrypted blocks)
    /// Per-space AES-256-GCM nonce (96 bits). Unique per block write under the same key.
    /// Stored alongside ciphertext — nonces are not secret.
    /// Only present for blocks in encrypted spaces (Personal, Collaborative, Untrusted).
    space_nonce: Option<[u8; 12]>,
    /// Per-space AES-256-GCM authentication tag (128 bits). Verifies both ciphertext
    /// integrity and authenticity. Replaces CRC-32C for encrypted blocks —
    /// CRC-32C is retained as a secondary check for storage-level corruption.
    space_auth_tag: Option<[u8; 16]>,
    /// Device key epoch under which this block was encrypted (§4.10).
    /// Used during reads to select the correct device key after key rotation.
    device_epoch: u64,
    /// Device-level encryption nonce (96 bits). Always present — every block
    /// on disk is encrypted with the device key (§4.10). This is the outer
    /// encryption layer; space encryption (above) is the inner layer.
    device_nonce: [u8; 12],
    /// Device-level authentication tag (128 bits). Authenticates the block
    /// envelope (header + data) under the device key.
    device_auth_tag: [u8; 16],
}

impl BlockEngine {
    fn write_block(&self, data: &[u8], tier: StorageTier) -> BlockId {
        let strategy = self.select_compression(data, tier);
        let compressed = match strategy {
            CompressionStrategy::None => data.to_vec(),
            CompressionStrategy::Lz4 => lz4::compress(data),
            CompressionStrategy::Zstd { level } => zstd::compress(data, level),
        };

        // Only use compression if it actually saves space
        let (stored, used_strategy) = if compressed.len() < data.len() {
            (compressed, strategy)
        } else {
            (data.to_vec(), CompressionStrategy::None)
        };

        self.write_raw_block(stored, used_strategy)
    }

    /// Adaptive compression selection: detects already-compressed,
    /// encrypted, or random data and skips compression to avoid
    /// wasting CPU. Uses byte entropy estimation on a 4 KB sample.
    fn select_compression(&self, data: &[u8], tier: StorageTier) -> CompressionStrategy {
        // Fast entropy check: sample first 4 KB and estimate Shannon entropy.
        // High entropy (> 7.5 bits/byte) indicates encrypted, compressed, or
        // random data — compression will not help.
        let sample = &data[..data.len().min(4096)];
        let entropy = self.estimate_entropy(sample);

        if entropy > 7.5 {
            // Already compressed, encrypted, or random — skip entirely
            return CompressionStrategy::None;
        }

        if entropy > 6.5 {
            // Moderately complex data — only use fast LZ4 (low CPU cost)
            // Zstd won't achieve meaningfully better ratio on high-entropy data
            return CompressionStrategy::Lz4;
        }

        // Low-entropy data: full compression benefit available
        match tier {
            StorageTier::Hot => CompressionStrategy::Lz4,
            StorageTier::Warm => CompressionStrategy::Zstd { level: 3 },
            StorageTier::Cold => CompressionStrategy::Zstd { level: 9 },
        }
    }

    /// Estimate Shannon entropy of a byte sample.
    /// Returns bits per byte (0.0 = all identical, 8.0 = perfectly random).
    fn estimate_entropy(&self, sample: &[u8]) -> f32 {
        let mut counts = [0u32; 256];
        for &byte in sample {
            counts[byte as usize] += 1;
        }
        let len = sample.len() as f32;
        let mut entropy: f32 = 0.0;
        for &count in &counts {
            if count > 0 {
                let p = count as f32 / len;
                entropy -= p * p.log2();
            }
        }
        entropy
    }
}
```

**Why block-level:** Content-addressed blocks are immutable after write — ideal for compression. The decompression cost is paid once on read and amortized across multiple accesses by the page cache. On a laptop SSD, LZ4 decompresses at ~4 GB/s (faster than most SATA SSD read speeds), so compression is effectively free on the read path.

**Adaptive compression — why entropy estimation matters:**

Encrypted blocks and already-compressed media (JPEG, MP4, FLAC) have high byte entropy (> 7.5 bits/byte). Attempting to compress them wastes CPU and may actually *increase* the stored size (compression overhead > savings). The entropy check takes ~2 microseconds on a 4 KB sample — negligible compared to the 50-500 microsecond cost of running LZ4/Zstd on incompressible data that produces no savings.

```
Content Type         Entropy (bits/byte)   Compression Action        Savings
────────────         ───────────────────   ──────────────────        ───────
Text / JSON          3.0 - 5.0             Zstd (tier-appropriate)   60-80%
Code / markup        4.0 - 5.5             Zstd (tier-appropriate)   50-70%
Structured data      4.5 - 6.0             LZ4 or Zstd              40-60%
Already-LZ4'd data   6.5 - 7.5             LZ4 only (fast check)    5-15%
Encrypted data       7.8 - 8.0             None (skip)              0%
JPEG / MP4           7.5 - 7.9             None (skip)              0%
Random bytes         ~8.0                  None (skip)              0%
```

**Entropy sampling for large files:** The entropy check samples the first 4 KB, which is representative for most objects (documents, config files, code). For objects > 1 MB where the first 4 KB may be a low-entropy header with high-entropy payload, the Block Engine additionally samples a 4 KB region at the midpoint. If the two samples disagree by > 2 bits/byte, the higher entropy wins (conservative — avoids wasting CPU on incompressible data). For objects < 100 bytes, compression is skipped entirely (overhead exceeds savings). Entropy estimation is O(sample_size) = O(4 KB) = negligible compared to the 50-500 μs cost of running LZ4/Zstd on data that produces no savings.

The `CompressionStrategy::None` fast path means that spaces storing encrypted data (Personal zone) or media-heavy content (photos, video) pay zero compression CPU. Only spaces with compressible content (documents, code, conversations, config) invest CPU in compression.

**Security: compress before encrypt.** The Block Engine compresses data before either encryption layer acts on it: per-space encryption (§6) encrypts object content above the Block Engine, and device encryption (§4.10) encrypts the block envelope below it. Compression operates on plaintext content (for non-space-encrypted zones) or per-space ciphertext (which is high-entropy and skipped by the adaptive entropy check). This ordering is critical — compressing ciphertext is useless (encrypted data is indistinguishable from random), and encrypting compressed data avoids CRIME/BREACH-style attacks where compression ratio changes leak information about plaintext. Since AIOS uses content-addressed blocks (each block has a unique content_hash), an attacker cannot perform the chosen-plaintext injection required for CRIME-style attacks. The compress-then-encrypt ordering is safe.

### 4.7 Tiered Storage

Blocks are classified into temperature tiers based on access patterns. The tier determines compression strategy, and on systems with multiple storage devices, placement:

```rust
pub enum StorageTier {
    /// Recently written or frequently accessed — LZ4 or uncompressed
    Hot,
    /// Older versions, inactive spaces — zstd level 3
    Warm,
    /// Audit archives >30 days, old version history, cold spaces — zstd level 9
    Cold,
}

pub struct TierPolicy {
    /// Objects accessed in the last N hours are Hot
    hot_window: Duration,               // default: 24 hours
    /// Objects accessed in the last N days are Warm
    warm_window: Duration,              // default: 30 days
    /// Everything else is Cold
    /// Minimum time before an object can be demoted from Hot → Warm
    demotion_grace: Duration,           // default: 6 hours
}

pub struct TierManager {
    policy: TierPolicy,
    /// Background thread that recompresses blocks when demoted
    recompressor: RecompressorThread,
    /// Statistics for monitoring
    stats: TierStats,
}

pub struct TierStats {
    hot_blocks: u64,
    hot_bytes: u64,
    warm_blocks: u64,
    warm_bytes: u64,
    cold_blocks: u64,
    cold_bytes: u64,
    bytes_saved_by_compression: u64,
}
```

**Tier transitions:** A background thread scans block access timestamps. When a Hot block hasn't been accessed within `hot_window` (24 hours), it is recompressed with zstd and demoted to Warm. When a Warm block hasn't been accessed within `warm_window` (30 days), it is recompressed at a higher zstd level and demoted to Cold. Promotion (Cold → Hot) happens automatically on access — the block is decompressed and rewritten with LZ4.

**Recompression is lazy.** The recompressor runs at lowest I/O priority and yields to any foreground read or write. On a Pi with an SD card, recompression is throttled to avoid wearing the card. Tier transitions are batched — the recompressor processes blocks in groups during idle periods.

#### 4.7.1 AIRS-Directed Compression Scheduling

Compression scheduling can be initiated by two sources:

1. **Automatic tier demotion** (TierManager, independent of AIRS) — the normal background recompressor described above. Runs on access-time heuristics, no AI involvement. Always operational.

2. **AIRS resource directives** (during storage pressure or semantic prioritization) — AIRS can request that specific blocks be recompressed at a different level, or that compression be prioritized for blocks that AIRS predicts won't be accessed soon.

```
AIRS compression directive:
  1. AIRS sends ResourceCompress { space, blocks, algorithm, reason }
     to kernel via resource directive channel
  2. Kernel validates:
     a. AIRS holds ReadSpace capability for the target space
     b. Compression CPU quota not exceeded (blast radius for AIRS)
     c. Directive rate within AirsDirectiveMonitor baseline ([security.md §2.3.1](../security/security.md))
  3. Kernel forwards directive to Space Storage
  4. Space Storage executes compression through the NORMAL Block Engine path:
     a. Read block from disk
     b. Verify checksum (CRC-32C) — reject if corrupted
     c. Decompress existing content
     d. Recompress with requested algorithm
     e. Verify round-trip: decompress(recompressed) == original
     f. Write new block (new checksum computed)
     g. Update block index atomically (WAL-protected)
  5. Provenance chain records: ResourceCompress event
```

**Why AIRS cannot corrupt data:** Compression operates through the Block Engine, which verifies checksums on read and computes new checksums on write. The round-trip verification (step 4e) catches any compression error before the block is committed. If verification fails, the original block is retained unchanged and a storage integrity event is logged. AIRS never touches raw block data — it only specifies *which* blocks to compress and *with what algorithm*. The Block Engine does the actual I/O.

**Why no shortcut:** As with prefetch (§4.3.1), there is no bypass path. AIRS compression directives are advisory — "compress this block at zstd level 9" — not operational. Space Storage does the work through its existing, checksum-verified, WAL-protected write path.

**Multi-device tiering (future):** On systems with both NVMe and SD storage, Hot data lives on NVMe and Cold data on SD. The tier manager handles migration transparently. This is a Phase 14 optimization — single-device tiering via compression is the Phase 4 implementation.

### 4.8 Write Amplification Tracking

Write amplification factor (WAF) is the ratio of data written to the flash device versus data written by the application. A WAF of 10x means the device writes 10 bytes of flash for every byte the application intended to write — the other 9 bytes are overhead from the FTL's garbage collection, index updates, and journaling. On consumer SD cards with ~1000 P/E cycles per cell, high WAF directly shortens device lifetime.

AIOS tracks WAF continuously to validate that the flash-aware write strategies (LSM-tree, zone separation, append-preferred allocation) are actually working:

```rust
pub struct WriteAmplificationTracker {
    /// Bytes logically written by the application (object data + metadata)
    app_bytes_written: AtomicU64,
    /// Bytes physically written to the device (from device SMART data or
    /// kernel block layer accounting). Includes FTL overhead.
    device_bytes_written: AtomicU64,
    /// WAF history (rolling window, last 24 hours, hourly samples)
    history: [WafSample; 24],
    /// Alert threshold: warn if WAF exceeds this value
    alert_threshold: f32,           // default: 5.0 (WAF > 5x triggers alert)
}

pub struct WafSample {
    /// Timestamp of this sample
    timestamp: Timestamp,
    /// Application bytes in this interval
    app_bytes: u64,
    /// Device bytes in this interval
    device_bytes: u64,
}

impl WriteAmplificationTracker {
    /// Current instantaneous WAF
    pub fn current_waf(&self) -> f32 {
        let app = self.app_bytes_written.load(Ordering::Relaxed) as f32;
        let device = self.device_bytes_written.load(Ordering::Relaxed) as f32;
        if app == 0.0 { return 1.0; }
        device / app
    }

    /// Record an application-level write
    pub fn record_app_write(&self, bytes: u64) {
        self.app_bytes_written.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Record a device-level write (from block layer or SMART)
    pub fn record_device_write(&self, bytes: u64) {
        self.device_bytes_written.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Check if WAF exceeds threshold and emit warning
    pub fn check_alert(&self) -> Option<WafAlert> {
        let waf = self.current_waf();
        if waf > self.alert_threshold {
            Some(WafAlert {
                current_waf: waf,
                threshold: self.alert_threshold,
                recommendation: if waf > 15.0 {
                    "WAF critically high. Check for non-AIOS writes or compaction storms."
                } else {
                    "WAF elevated. Consider reducing compaction frequency."
                },
                timestamp: Timestamp::now(),
            })
        } else {
            None
        }
    }
}
```

**Target WAF by device class:**

```
Device              Target WAF   Max Acceptable   Notes
──────              ──────────   ──────────────   ─────
Laptop SSD (NVMe)   1.5 - 3x     5x              NVMe has built-in wear leveling
Laptop SSD (SATA)   2 - 4x       8x              SATA FTL less efficient
SD card (consumer)   1.5 - 3x     5x              Critical — low P/E endurance
SD card (industrial) 2 - 5x      10x              Higher endurance tolerates more
eMMC                 2 - 4x       8x              Similar to SATA SSD
```

The LSM-tree block index (§4.1), append-preferred write allocation (§4.2), and hot/cold zone separation together target a WAF of 1.5-3x — a 5-20x improvement over the B-tree random-write approach. The WAF tracker validates this in production and alerts if unexpected write patterns (e.g., a compaction storm, or a misbehaving agent writing excessive small updates) push WAF above the threshold.

```rust
pub struct WafAlert {
    current_waf: f32,
    threshold: f32,
    recommendation: &'static str,
    timestamp: Timestamp,
}
```

**Device write accounting:** Device-level bytes written are measured via kernel block layer hooks — the Block Engine instruments all I/O requests to the storage driver, counting bytes submitted to the device. This captures both application writes and internal overhead (WAL, compaction, index flushes). On devices with SMART support (NVMe, enterprise SSDs), the Block Engine cross-references its count against the device's internal write counter for validation.

**Inspector integration:** WAF data is exposed in the Storage Dashboard ([Inspector](../project/architecture.md), §5.6) alongside per-zone write statistics, enabling users and developers to understand flash wear patterns.

### 4.9 Sub-Block Deduplication

> **Implementation status:** Phase 14e. This section describes the design for sub-block deduplication. Phase 4 uses whole-block SHA-256 deduplication only. Near-duplicate content (e.g., edited documents) is stored in full until Phase 14e adds Rabin rolling hash chunk-level savings.

Standard content-addressed deduplication (§4.2) identifies identical blocks via SHA-256 hash comparison. This works perfectly when two objects contain the same content — the block is stored once and referenced by both objects. But it fails for **near-duplicate content**: if a user edits one paragraph in a 100 KB document, the entire block is stored again because the SHA-256 hash changed, even though 99% of the content is identical.

Sub-block deduplication uses a **rolling hash (Rabin fingerprint)** to identify shared sub-block regions between near-duplicate objects, reducing storage for common edit patterns by 60-80%:

```rust
/// Sub-block deduplication using content-defined chunking.
/// Splits objects into variable-size chunks at content-defined boundaries,
/// then deduplicates individual chunks via SHA-256.
pub struct SubBlockDedup {
    /// Rolling hash window size (bytes)
    window_size: usize,             // default: 48 bytes
    /// Target chunk size (bytes) — average, actual varies 50-200% of target
    target_chunk_size: usize,       // default: 4 KB
    /// Minimum chunk size (never split below this)
    min_chunk_size: usize,          // default: 2 KB
    /// Maximum chunk size (force split above this)
    max_chunk_size: usize,          // default: 16 KB
    /// Bitmask for Rabin fingerprint boundary detection
    /// When (fingerprint & mask) == 0, this is a chunk boundary
    boundary_mask: u64,             // tuned for target_chunk_size
}

pub struct Chunk {
    /// SHA-256 of chunk content
    hash: Hash,
    /// Offset within the original object
    offset: u64,
    /// Size of this chunk
    size: u32,
}

impl SubBlockDedup {
    /// Split an object into content-defined chunks using Rabin rolling hash.
    /// Chunk boundaries are determined by content, not position — so if content
    /// is inserted in the middle, only the surrounding chunks change. Chunks
    /// before and after the edit remain identical and deduplicate.
    pub fn chunk(&self, data: &[u8]) -> Vec<Chunk> {
        let mut chunks = Vec::new();
        let mut chunk_start = 0;
        let mut hasher = RabinHasher::new(self.window_size);  // Rabin fingerprint rolling hash (custom AIOS impl)

        for i in self.min_chunk_size..data.len() {
            hasher.slide(data[i]);

            let chunk_len = i - chunk_start;
            let at_boundary = (hasher.fingerprint() & self.boundary_mask) == 0
                && chunk_len >= self.min_chunk_size;
            let at_max = chunk_len >= self.max_chunk_size;

            if at_boundary || at_max {
                let chunk_data = &data[chunk_start..=i];
                chunks.push(Chunk {
                    hash: sha256(chunk_data),
                    offset: chunk_start as u64,
                    size: chunk_data.len() as u32,
                });
                chunk_start = i + 1;
                hasher.reset();
            }
        }

        // Final chunk
        if chunk_start < data.len() {
            let chunk_data = &data[chunk_start..];
            chunks.push(Chunk {
                hash: sha256(chunk_data),
                offset: chunk_start as u64,
                size: chunk_data.len() as u32,
            });
        }

        chunks
    }
}
```

**How it works with content-addressed storage:**

```
Original document (100 KB, target_chunk_size = 4 KB):
  Chunked into: [A, B, C, ..., Y] — ~25 chunks, ~4 KB each
  Each chunk stored once, addressed by SHA-256 hash
  Object metadata: ObjectId → [hash_A, hash_B, ..., hash_Y]

User edits paragraph in chunk D (new version):
  Chunked into: [A, B, C, D', E, ..., Y] — chunks A-C, E-Y unchanged
  New chunks stored: only D' (~4 KB)
  Old version: ObjectId_v1 → [hash_A, ..., hash_D, ..., hash_Y]
  New version: ObjectId_v2 → [hash_A, ..., hash_D', ..., hash_Y]

Storage used: 100 KB (original) + 4 KB (changed chunk) = 104 KB
Without sub-block dedup: 100 KB + 100 KB = 200 KB
Savings: 96 KB (48%)
```

**When sub-block dedup is applied:**

| Object Size | Dedup Strategy | Rationale |
|---|---|---|
| < 4 KB | Whole-block SHA-256 only | Too small to benefit from chunking overhead |
| 4 KB - 1 MB | Sub-block chunking | Sweet spot for document edits, code changes |
| > 1 MB | Sub-block chunking | Large files benefit the most from partial dedup |
| Binary blobs (JPEG, MP4) | Whole-block only | Compressed/encrypted content has no shared chunks after edits |

**Content-defined boundaries vs fixed-size blocks:** The Rabin rolling hash creates chunk boundaries based on content, not position. This is critical: if a user inserts 10 bytes at the beginning of a file, fixed-size chunking would shift every chunk boundary, making all chunks "new" and defeating dedup. Content-defined boundaries remain stable — only chunks near the insertion point change, while distant chunks stay identical and deduplicate.

**Integration with version history (§5):** Sub-block dedup multiplies the effectiveness of version history storage. Where whole-block dedup saves storage only when entire blocks are identical across versions, sub-block dedup captures partial overlaps — the common case for document editing, code modification, and configuration changes. Combined with the Merkle DAG (§5.1), each version stores only its unique chunk hashes.

### 4.10 Device-Level Transparent Encryption

Every block written to the storage device is encrypted with a device-bound key before it reaches the storage drivers. This is not per-space encryption (§6) — it is a lower layer. Per-space encryption protects cross-zone isolation within a running system. Device-level encryption protects against physical access to the storage medium: someone pulling the SD card, imaging the SSD, or analyzing flash chips.

```
Encryption layering (data flows top to bottom on writes, bottom to top on reads):

  Object content (plaintext)
         │
         ▼
  ┌─────────────────────────────────────┐
  │  Encryption Layer (§6)              │
  │  Per-space key (AES-256-GCM)        │  ← Only for Personal, Collaborative,
  │  Encrypts object content            │     Untrusted zones. Core and Ephemeral
  │  at this layer.                     │     are plaintext at this layer.
  └────────────────┬────────────────────┘
         │ ciphertext (or plaintext for Core/Ephemeral)
         ▼
  ┌─────────────────────────────────────┐
  │  Block Engine (§4)                  │
  │  Compression, chunking, indexing    │
  └────────────────┬────────────────────┘
         │ compressed block
         ▼
  ┌─────────────────────────────────────┐
  │  Device Encryption (this section)   │
  │  Device key (AES-256-GCM)           │  ← Always. Every block. No exceptions.
  │  Encrypts the block envelope:       │
  │  header + compressed data           │
  └────────────────┬────────────────────┘
         │ device-encrypted block
         ▼
  ┌─────────────────────────────────────┐
  │  Storage Drivers                    │
  │  VirtIO-Blk │ NVMe │ SD/eMMC │ USB │
  └─────────────────────────────────────┘
```

**What this means for each security zone:**

| Zone | Per-space encryption (§6) | Device encryption (§4.10) | On disk |
|---|---|---|---|
| Core (system/) | No | Yes | Single-layer ciphertext. Readable by the running system after boot unlock, unreadable to physical access. |
| Personal (user/) | Yes (user key) | Yes (device key) | Double-layer ciphertext. Even with the device key, an attacker cannot read Personal data without the user's passphrase. |
| Collaborative (shared/) | Yes (shared key) | Yes (device key) | Double-layer ciphertext. |
| Untrusted (web-storage/) | Yes (per-origin key) | Yes (device key) | Double-layer ciphertext. |
| Ephemeral (/tmp) | No | Yes | Single-layer ciphertext. Temporary data is still encrypted on the physical medium. |

**Why this matters:** Without device-level encryption, `system/credentials/`, `system/identity/` (which contains the encrypted master key blob), `system/audit/`, and `system/session/` are plaintext on disk. An attacker with physical access to the storage device can read all Core zone data — including the encrypted-master-key blob that, combined with a brute-forced passphrase, unlocks everything. Device encryption eliminates this: the encrypted master key blob is encrypted under the device key, which is derived from hardware-bound secrets (TPM, TrustZone) or the user's boot passphrase. Physical access to the raw medium yields only ciphertext.

#### 4.10.1 Device Key Hierarchy

```rust
/// Device-level encryption key management.
/// The device key encrypts every block before it reaches storage drivers.
/// It is derived from hardware-bound secrets when available, or from the
/// user's boot passphrase on devices without a secure element.
pub struct DeviceKeyManager {
    /// The active device encryption key. Loaded at boot, zeroized at shutdown.
    active_key: DecryptedDeviceKey,
    /// Previous device key (retained during key rotation until all blocks
    /// are re-encrypted during compaction).
    previous_key: Option<DecryptedDeviceKey>,
    /// Key derivation source — determines how the device key is unlocked at boot.
    key_source: DeviceKeySource,
    /// Epoch counter — incremented on each key rotation. Stored in the
    /// superblock so the Block Engine knows which key version each block uses.
    epoch: u64,
}

pub enum DeviceKeySource {
    /// Hardware-bound key derivation via platform secure element.
    /// The device key is sealed to the hardware — only this specific device
    /// can unseal it. Unlocked automatically at boot (no user interaction).
    /// Available on: ARM TrustZone (RPi 4/5), TPM 2.0 (laptops), Apple SEP.
    HardwareBound {
        /// Platform-specific handle to the sealed key blob.
        sealed_blob: Vec<u8>,
    },
    /// Passphrase-derived device key. Used on devices without a secure element.
    /// User enters a boot passphrase at startup; the device key is derived via
    /// Argon2id. This is distinct from the identity passphrase (§6.1) —
    /// the boot passphrase unlocks the device, the identity passphrase unlocks
    /// per-space keys. They CAN be the same passphrase (single-passphrase mode)
    /// but are derived independently with different salts.
    PassphraseDerived {
        salt: [u8; 32],
        argon2_params: Argon2Params,
    },
    /// Combined: hardware-bound with passphrase fallback.
    /// The device key is sealed to hardware AND encrypted with a passphrase.
    /// Either can unlock it. Hardware binding provides convenience (auto-unlock
    /// on the enrolled device); passphrase provides recovery if the secure
    /// element fails or the storage is moved to a new device.
    HardwareWithPassphraseFallback {
        sealed_blob: Vec<u8>,
        passphrase_salt: [u8; 32],
        argon2_params: Argon2Params,
    },
}

/// The decrypted device key. Stored on a pinned kernel page, zeroized on drop.
pub struct DecryptedDeviceKey {
    key_bytes: ZeroizeBox<[u8; 32]>,
    epoch: u64,
}

/// CPU feature detection for cipher selection. Wraps aarch64 ID_AA64ISAR0_EL1
/// register fields. Populated by HAL during early boot (hal.md §3 init_rng path).
pub struct CpuFeatures {
    has_aes: bool,      // ID_AA64ISAR0_EL1.AES >= 1
    has_sha2: bool,     // ID_AA64ISAR0_EL1.SHA2 >= 1
}

impl CpuFeatures {
    pub fn has_aes_ni(&self) -> bool { self.has_aes }      // x86 naming convention
    pub fn has_arm_crypto(&self) -> bool { self.has_aes }   // ARM naming convention
}

/// Cipher selection for device-level encryption.
/// Chosen at boot time based on CPU feature detection.
pub enum DeviceCipher {
    Aes256Gcm,
    ChaCha20Poly1305,
}

impl DeviceKeyManager {
    /// Return the device key for a given epoch. During key rotation, both
    /// active_key and previous_key are available. Blocks encrypted under
    /// the previous epoch can still be read until compaction re-encrypts them.
    fn key_for_epoch(&self, epoch: u64) -> Result<&DecryptedDeviceKey> {
        if self.active_key.epoch == epoch {
            Ok(&self.active_key)
        } else if let Some(ref prev) = self.previous_key {
            if prev.epoch == epoch {
                return Ok(prev);
            }
            Err(Error::UnknownKeyEpoch(epoch))
        } else {
            Err(Error::UnknownKeyEpoch(epoch))
        }
    }
}
```

**Boot sequence with device encryption:**

```
Cold boot:
  1. Superblock read (first 4 KB — the ONLY plaintext on disk)
     Contains: magic, version, device key source type, epoch, WAL offset
  2. Device key unlock:
     a. HardwareBound → unseal from TPM/TrustZone (automatic, no user input)
     b. PassphraseDerived → prompt user for boot passphrase → Argon2id derive
     c. HardwareWithPassphraseFallback → try hardware first, fall back to prompt
  3. Device key loaded into kernel memory (pinned page, VmFlags::PINNED | VmFlags::NO_DUMP)
  4. WAL replay (§4.4): WAL entries are device-encrypted; decrypt each during replay
  5. Block Engine operational — all subsequent reads decrypt transparently
  6. User authenticates (identity passphrase) → per-space keys unlocked (§6.1)
  7. Encrypted spaces (Personal, Collaborative, Untrusted) become accessible
```

**Single-passphrase mode:** Most users don't want two passphrases. When the identity passphrase and device passphrase are the same, AIOS derives both keys from a single user input using different Argon2id salts:

```rust
// Single-passphrase derivation: one input, two independent keys
let device_key = argon2id(passphrase, device_salt, device_params);  // unlocks the device
let master_key = argon2id(passphrase, identity_salt, identity_params);  // unlocks spaces
// Different salts → different keys. Compromising one does not reveal the other.
```

The user enters one passphrase at boot. Steps 2 and 6 of the boot sequence happen together. This is the default for devices without a secure element.

#### 4.10.2 Encryption in the Write Path

Device encryption integrates into the existing write path (§4.2) at the final step before I/O:

```
Agent writes object (updated write path with device encryption):
  1-9. [Same logical operations as §4.2 — content hashing, dedup, WAL,
        compression, zone allocation, LSM-tree index update, version store
        append. Note: WAL entries (step 4-5) are device-encrypted before
        being written to disk; data blocks (step 6) are device-encrypted
        below. Encryption is transparent to the logical operation sequence.]
 10.   Device encryption:
       a. Generate device nonce (counter-based, same scheme as §6.1.1)
       b. Encrypt block envelope (header + compressed data) with device key
       c. Compute device auth tag over ciphertext
       d. Write device_nonce and device_auth_tag into BlockHeader
 11.   Encrypted block written to storage driver
 12.   WAL entry marked committed
```

**Why encrypt after compression:** Compression operates on plaintext (or per-space ciphertext, which is already high-entropy and skipped by the entropy check — §4.6). Device encryption is the last transform before disk. This ordering preserves compression effectiveness: compressing after device encryption would be useless (encrypted data is incompressible).

**WAL entries are also device-encrypted.** The WAL sits on the raw device and must not contain plaintext. Each WAL entry is encrypted with the device key before being appended. On crash recovery, the device key is unlocked first (boot step 2), then WAL replay proceeds normally (boot step 4).

#### 4.10.3 Key Rotation via Compaction

Traditional full-disk encryption (LUKS, dm-crypt) requires re-encrypting the entire device to rotate the master key — a multi-hour operation on large disks. AIOS avoids this by piggybacking on LSM compaction:

```
Device key rotation:
  1. Generate new device key (epoch N+1)
  2. Store new key alongside old key in DeviceKeyManager
  3. Update superblock: epoch = N+1
  4. All NEW writes use epoch N+1 key
  5. Compaction naturally rewrites existing SSTables:
     a. Read SSTable blocks → decrypt with epoch N key
     b. Merge/compact as normal
     c. Re-encrypt output blocks with epoch N+1 key
     d. Write new SSTable
  6. When all SSTables from epoch N have been compacted away:
     a. Zeroize epoch N key
     b. Rotation complete — only epoch N+1 key exists
```

**Cost:** Zero additional I/O. Compaction already reads and rewrites every block. Re-encrypting during compaction adds only the AES-256-GCM cost (~1 GB/s on ARMv8 crypto extensions, ~3+ GB/s on x86 AES-NI), which is negligible compared to disk I/O.

**Time to complete:** A full key rotation completes when every SSTable has been compacted at least once. Under normal write load, this happens within days. The system can accelerate rotation by scheduling compaction of remaining old-epoch SSTables during idle periods.

**Epoch tracking:** Each `BlockHeader` stores the epoch it was encrypted under. The Block Engine maintains a small map of `epoch → key` (at most 2 entries: current and previous). On read, the epoch in the block header selects the correct decryption key.

```rust
impl BlockEngine {
    fn read_block_raw(&self, location: BlockLocation) -> Result<Vec<u8>, StorageError> {
        let encrypted = self.storage_driver.read(location.offset, location.size)?;
        let header = BlockHeader::parse(&encrypted)?;

        // Select device key by epoch
        let device_key = self.device_keys.key_for_epoch(header.device_epoch)?;

        // Decrypt block envelope
        let decrypted = aes_256_gcm_decrypt(
            device_key,
            &header.device_nonce,
            &encrypted[BlockHeader::SIZE..],
            &header.device_auth_tag,
        )?;

        // Verify CRC-32C (defense in depth — catches storage-level bit rot
        // that GCM auth tag might not catch if corruption hits the nonce or tag itself)
        verify_crc32c(&decrypted, header.checksum)?;

        Ok(decrypted)
    }
}
```

#### 4.10.4 Crypto-Shredding

When data must be irrecoverably destroyed — a space is deleted, old versions are garbage collected, or the user factory-resets the device — AIOS uses **crypto-shredding**: delete the key, not the data.

```
Why crypto-shredding is necessary on flash storage:

  Traditional secure erase (overwrite with zeros):
    1. Write zeros to block at logical address X
    2. FTL maps logical X to NEW physical page (flash is write-once-per-erase)
    3. OLD physical page still contains the original data
    4. FTL may eventually erase the old page... or may not (wear leveling)
    5. Data recoverable with flash chip imaging (academic attacks, forensics)

  Crypto-shredding (AIOS approach):
    1. All data was encrypted with a key
    2. Zeroize the key (volatile write, immediate)
    3. Ciphertext remains on flash — but without the key, it is computationally
       indistinguishable from random data
    4. No need to physically erase — the data is already destroyed
    5. Flash wear: zero (no write operations required)
```

**Crash recovery during device key rotation:** If the device crashes mid-rotation (some SSTables re-encrypted with the new key, others still under the old key), the `epoch` field in each `BlockHeader` identifies which key to use for decryption. The `DeviceKeyManager` retains both `active_key` (new epoch) and `previous_key` (old epoch) until rotation completes. On recovery, compaction resumes from where it left off, re-encrypting remaining old-epoch blocks. Both keys remain loaded in kernel memory until all blocks are under the new epoch. In the emergency case where rotation must be aborted, the superblock epoch is rolled back and any new-epoch blocks are re-encrypted to the old epoch during the next compaction pass.

**Epoch-based forward secrecy:** Device key rotation creates epoch boundaries. Once all data from epoch N has been compacted to epoch N+1 and the epoch N key is zeroized, all deleted data from epoch N is permanently unrecoverable — even if the device is later compromised and the epoch N+1 key is extracted, it cannot decrypt blocks that were encrypted under epoch N and never re-encrypted (because they were garbage collected before compaction reached them).

```
Timeline:
  Epoch 1: blocks [A, B, C, D, E] encrypted with key_1
  User deletes object containing blocks [B, D] → GC marks them dead
  Key rotation → epoch 2, key_2 generated
  Compaction rewrites live blocks: [A, C, E] re-encrypted with key_2
  Dead blocks [B, D] were never re-encrypted — still under key_1
  key_1 zeroized → [B, D] permanently unrecoverable
  Even if key_2 is later compromised: [B, D] remain safe
```

This gives AIOS **forward secrecy for deleted data** — a property that traditional FDE and per-space encryption alone cannot provide.

#### 4.10.5 Performance

Device encryption adds one AES-256-GCM operation per block read and per block write. On modern hardware:

```
Platform             AES-256-GCM throughput   Impact on Block Engine
──────────           ─────────────────────    ─────────────────────
x86-64 (AES-NI)     3-6 GB/s                 Negligible (<1% overhead)
ARMv8 (crypto ext)   0.8-1.5 GB/s            Negligible — faster than SD/eMMC I/O
ARMv8 (no crypto)    150-300 MB/s             Measurable on NVMe; unnoticed on SD

Fallback: ChaCha20-Poly1305 (software-friendly)
ARMv8 (no crypto)    400-600 MB/s            Better than AES without hardware support
```

On devices without AES hardware extensions, the Block Engine automatically selects ChaCha20-Poly1305 (same security level, faster in software on ARM):

```rust
pub fn select_device_cipher(cpu_features: &CpuFeatures) -> DeviceCipher {
    if cpu_features.has_aes_ni() || cpu_features.has_arm_crypto() {
        DeviceCipher::Aes256Gcm
    } else {
        DeviceCipher::ChaCha20Poly1305
    }
}
```

**Storage overhead:** Each block gains 36 bytes (8-byte epoch + 12-byte nonce + 16-byte auth tag) from device encryption. For the average 4 KB block, this is 0.9% overhead. For the system overall (assuming 200,000 blocks on a 256 GB device), the total overhead is ~6.9 MB — negligible.

**The superblock is the only plaintext on disk.** It contains: magic number, format version, device key source type, current epoch, WAL offset, and a checksum. No user data, no keys, no sensitive metadata. It is 4 KB. Everything else — WAL entries, LSM-tree SSTables, data blocks, index blocks — is device-encrypted.

-----

## 5. Version Store

### 5.1 Merkle DAG

Every object modification creates a new version node in a Merkle DAG (like git):

```rust
pub struct Version {
    hash: Hash,                         // SHA-256 of (parent_hash + content_hash + metadata)
    parent: Option<Hash>,               // previous version
    /// Second parent for merge commits (Phase 9c — Space Sync conflict resolution).
    /// Always None in single-device operation (Phases 4-8). When Space Sync detects
    /// a fork (two devices edited the same object independently), the merge version
    /// sets this field to the remote branch's head hash.
    /// NOTE: Added to the struct now (as None) to avoid schema migration in Phase 9c.
    merge_parent: Option<Hash>,
    content_hash: Hash,                 // content at this version
    content_size: u64,                  // size of content in bytes (for diff, quota tracking)
    object_id: ObjectId,
    timestamp: Timestamp,
    author: AgentId,
    provenance: ProvenanceEntry,
    message: Option<String>,            // optional description of change
}

pub struct ProvenanceEntry {
    agent: AgentId,
    task: Option<TaskId>,
    action: ProvenanceAction,
    timestamp: Timestamp,
    signature: Signature,               // Ed25519 signature by agent's identity
}

pub enum ProvenanceAction {
    Created,
    Modified { diff_summary: String },
    Derived { source: ObjectId },
    Imported { source: String },
    AiGenerated { model: ModelId, prompt_hash: Hash },
}
```

**Provenance signatures:** The `signature` field in `ProvenanceEntry` is an Ed25519 signature over the canonical Bincode encoding of `(action, object_id, timestamp, agent_id)`. The agent's signing keypair is held in the kernel identity store — agents never access raw keys directly, they request the kernel to sign on their behalf. On read, the signature is verified against the agent's public key (stored in `system/identity/`). If verification fails, the version node is flagged as corrupted or tampered and a security event is logged. Signatures are immutable — stored in the Merkle DAG and tied to the content hash chain. This provides non-repudiation: an agent cannot later deny creating or modifying an object.

**diff_summary format:** The `Modified.diff_summary` field contains a human-readable one-line description of the change (e.g., "edited paragraph 3", "appended 2 KB"). This is set by the agent that performed the modification. It is not a machine-parseable diff — for structural diffs, use the `VersionStore.diff()` method (§5.3).

### 5.2 Space Snapshots

A space snapshot is a point-in-time reference to all objects in a space:

```rust
pub struct Snapshot {
    id: SnapshotId,
    space: SpaceId,
    timestamp: Timestamp,
    root_hash: Hash,                    // Merkle root of all object versions
    object_versions: HashMap<ObjectId, Hash>,
    trigger: SnapshotTrigger,
}

pub enum SnapshotTrigger {
    Scheduled,                          // periodic (daily, weekly)
    Manual,                             // user requested
    PreBulkOperation,                   // automatic before bulk writes/deletes
    PreAgentInstall,                    // before installing new agent
}
```

**Snapshot access:** Automatic snapshots have generated UUIDs (`SnapshotId`). Manual snapshots can optionally be tagged with a user-provided name (string label). Snapshots are accessed via `space::snapshot(id)` in the Space API or through the Inspector UI. Rollback is via `space::rollback_to_snapshot(snapshot_id)` (see §5.3 for the rollback implementation). `SnapshotTrigger` variants: `Scheduled` — periodic snapshots (daily or weekly, configurable per space). `Manual` — user-requested via API or Inspector. `PreBulkOperation` — automatic before bulk writes (>10 objects) or bulk deletes. `PreAgentInstall` — automatic before installing a new agent (captures system state before untrusted code runs).

**Blast radius containment (Security Layer 8):** "Blast radius containment" limits the impact of a failed or malicious operation by ensuring a rollback point always exists. Before any bulk operation (agent writing >10 objects, bulk delete, space import), the system automatically creates a snapshot. If the operation goes wrong — buggy agent corrupting data, failed import, accidental deletion — the user can roll back to the pre-operation state. This is Security Layer 8 in the eight-layer security model ([architecture.md §3.1](../project/architecture.md)): even if all other layers fail to prevent a bad action, the damage is bounded and reversible.

### 5.3 DAG Operations

The Version Store exposes traversal, diffing, and rollback operations over the Merkle DAG. These operations use the following LSM-tree key types:

```rust
/// LSM-tree key for version nodes. Composed of (space, object, reverse_timestamp)
/// so that a prefix scan on (space, object) returns versions newest-first.
pub struct VersionKey {
    space: SpaceId,
    object: ObjectId,
    /// u64::MAX - timestamp. Ensures newest versions sort first in LSM-tree
    /// byte ordering (which is ascending).
    reverse_timestamp: u64,
}

impl VersionKey {
    /// Create a prefix key for range-scanning all versions of an object.
    pub fn prefix(space: SpaceId, object: ObjectId) -> Self {
        VersionKey { space, object, reverse_timestamp: 0 }
    }
}

pub enum ScanDirection {
    /// Newest version first (natural LSM-tree order for VersionKey).
    NewestFirst,
    /// Oldest version first (reverse scan).
    OldestFirst,
}
```

```rust
/// Kernel-provided intrinsic: returns the AgentId of the currently executing process.
/// Used throughout for provenance and capability checks. Defined in ipc.md.
fn current_agent() -> AgentId;

/// Kernel Crypto Core operation: signs data with the identity's Ed25519 key.
/// Used for provenance chain signatures. See security.md §4.
fn kernel_sign(data: &[u8]) -> Signature;

impl VersionStore {
    /// Get a specific version by its hash. O(log n) LSM-tree point lookup.
    fn get_version(&self, hash: Hash) -> Result<Version, StorageError>;

    /// Get the current head (newest) version of an object.
    fn head(&self, space: SpaceId, object: ObjectId) -> Result<Version, StorageError>;

    /// Append a new version node to the DAG. Writes to the LSM-tree MemTable
    /// and WAL atomically.
    fn append(&self, space: SpaceId, version: Version) -> Result<(), StorageError>;

    /// Walk the version chain for an object, newest to oldest.
    /// Returns an iterator that lazily loads version nodes from the LSM-tree
    /// using the key `(space_id, object_id, reverse_timestamp)`.
    fn log(&self, space: SpaceId, object: ObjectId) -> impl Iterator<Item = Version> {
        self.lsm.range_scan(
            VersionKey::prefix(space, object),
            ScanDirection::NewestFirst,
        )
        .map(|entry| entry.decode::<Version>())
    }

    /// Compute the diff between two versions of the same object.
    /// Uses content-hash comparison first — if hashes match, the versions are
    /// identical (no diff). If hashes differ, performs block-level diff using
    /// the content-addressed blocks from the Block Engine (§4).
    fn diff(&self, v1: &Version, v2: &Version) -> VersionDiff {
        if v1.content_hash == v2.content_hash {
            return VersionDiff::Identical;
        }
        let blocks_v1 = self.block_engine.blocks_for(v1.content_hash);
        let blocks_v2 = self.block_engine.blocks_for(v2.content_hash);
        VersionDiff::Changed {
            added: blocks_v2.difference(&blocks_v1).collect(),
            removed: blocks_v1.difference(&blocks_v2).collect(),
            size_delta: v2.content_size as i64 - v1.content_size as i64,
        }
    }

    /// Rollback an object to a previous version. Creates a NEW version node
    /// whose content_hash points to the old version's content. The DAG grows
    /// forward — rollback never rewrites history.
    fn rollback(&self, space: SpaceId, object: ObjectId, target: Hash) -> Result<Version> {
        let target_version = self.get_version(target)?;
        if target_version.object_id != object {
            return Err(VersionError::ObjectMismatch {
                expected: object,
                found: target_version.object_id,
            });
        }
        let current_head = self.head(space, object)?;
        let new_version = Version {
            hash: Hash::compute(&[
                current_head.hash.as_bytes(),
                target_version.content_hash.as_bytes(),
            ]),
            parent: Some(current_head.hash),
            content_hash: target_version.content_hash,
            content_size: target_version.content_size,
            object_id: object,
            timestamp: Timestamp::now(),
            author: current_agent(),
            provenance: ProvenanceEntry::rollback(target),
            message: Some(format!("rollback to {}", target)),
        };
        self.append(space, new_version.clone())?;
        Ok(new_version)
    }

    /// Rollback an entire space to a snapshot. Iterates every object in the
    /// snapshot and creates rollback version nodes for any that have diverged.
    /// Rollback an entire space to a snapshot. Objects deleted since the snapshot
    /// are restored. Objects created after the snapshot are intentionally retained
    /// (snapshot rollback is non-destructive for new content).
    fn rollback_to_snapshot(&self, snapshot: &Snapshot) -> Result<u64> {
        let mut rolled_back = 0u64;
        for (object_id, version_hash) in &snapshot.object_versions {
            match self.head(snapshot.space, *object_id) {
                Ok(current_head) if current_head.hash != *version_hash => {
                    self.rollback(snapshot.space, *object_id, *version_hash)?;
                    rolled_back += 1;
                }
                Ok(_) => {} // already at snapshot version
                Err(Error::ObjectNotFound) => {
                    // Object was deleted after snapshot — restore it
                    self.rollback(snapshot.space, *object_id, *version_hash)?;
                    rolled_back += 1;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(rolled_back)
    }
}

pub enum VersionDiff {
    Identical,
    Changed {
        added: Vec<BlockId>,
        removed: Vec<BlockId>,
        size_delta: i64,
    },
}

impl ProvenanceEntry {
    /// Create a rollback provenance entry.
    pub fn rollback(target: Hash) -> Self {
        ProvenanceEntry {
            agent: current_agent(),
            task: None,
            action: ProvenanceAction::Modified {
                diff_summary: format!("rollback to version {}", target),
            },
            timestamp: Timestamp::now(),
            signature: kernel_sign(&[/* action || object_id || timestamp */]),
        }
    }
}
```

**LSM-tree key layout for version nodes:** Each version is stored with key `(space_id, object_id, reverse_timestamp)` where `reverse_timestamp = u64::MAX - timestamp`. This layout means a prefix scan on `(space_id, object_id)` returns versions newest-first, which is the common access pattern for `log` and `head`. The LSM-tree's sorted structure gives O(log n) point lookups and efficient range scans without a secondary index.

**Rollback and concurrent readers:** Rollback creates a new version node — it never rewrites or removes existing versions. Concurrent readers on old versions (including those with pinned file descriptors, §9.4) are unaffected. New readers after rollback see the rolled-back content as the current head. If `diff()` fails because content blocks have been garbage-collected (version retention pruned the old version), it returns `StorageError::BlockNotFound`.

**Version deduplication across edits:** Each version stores a `content_hash` pointing to content blocks. Blocks are reference-counted across all versions and all objects. When a version is pruned (§5.4), its blocks' refcounts are decremented. Shared blocks (same content in multiple versions or objects, via content-addressing) survive as long as any version references them. Only when a block's refcount reaches zero is it eligible for GC (§4.5).

### 5.4 Adaptive Retention

Under storage pressure (§10), the Version Store reduces history depth to reclaim space. Adaptive retention (Phase 4k, §12) governs which version nodes are pruned:

1. **Exempt versions are never pruned:** Snapshot roots (§5.2), user-tagged versions (`message` is `Some`), and the current head of each object are always retained.
2. **Pruning order:** Oldest intermediate versions (no tag, not a snapshot root) are pruned first. Within a single object's DAG, the chain is compacted: if versions A → B → C exist and B is prunable, A's parent pointer is rewritten to skip B, and B's content blocks are released if no other version references them (content-addressed dedup means shared blocks survive).
3. **User notification:** Design Principle 2 ("Never lose data silently") requires that the user is informed when version retention is reduced. The storage budget system (§10) emits a `StorageEvent::VersionRetentionReduced { space, old_depth, new_depth }` notification visible in the Inspector ([Inspector](../project/architecture.md)).
4. **Minimum guarantee:** Even under maximum pressure, the system retains at least the current head and the most recent snapshot for each object. User data is never deleted — only version history depth is reduced.

See §10 for storage pressure levels and §12 Phase 4k for the implementation timeline.

### 5.5 Branching

The Merkle DAG structure naturally supports branching: two version nodes can share the same parent. This occurs when the same object is modified on two devices while offline, producing a fork in the DAG:

```
        A (common ancestor)
       / \
      B   C       ← two devices diverged
      |   |
      D   E       ← continued independent edits
       \ /
        F         ← merge version (conflict resolved)
```

Branch creation is implicit — it happens whenever Space Sync (§8) discovers that both the local and remote DAGs have advanced past a common ancestor. The `SyncConflict` struct (§8) represents a detected fork, and resolution produces a merge version node with two parents. In Phase 9c, the `Version` struct (§5.1) will be extended with a `merge_parent: Option<Hash>` field — a second parent, only set for merge commits. Until then, single-device operation always produces a linear chain (every version has at most one parent).

Conflict resolution strategies are defined in §8. Branching semantics are Phase 9c work — single-device operation (Phase 4a-4l) always produces a linear chain.

-----

## 6. Encryption

> **Implementation status:** Phase 13a (not active in Phase 4a-4l). This section documents the design for per-space encryption. Phase 4 uses device-level encryption only (§4.10). Per-space encryption for Personal/Collaborative/Untrusted zones will be added in Phase 13a, providing cross-zone isolation within the running system.
>
> **Security note for Phases 4-12:** During Phases 4-12, all spaces rely solely on device-level encryption (§4.10). This means an attacker who obtains the device key (e.g., via physical access after boot, when the TPM/TrustZone-sealed key is loaded into memory) can read plaintext from ALL spaces — Personal, Collaborative, and Untrusted zones are not individually encrypted. The 8-layer security model's Layer 6 (Cryptographic Enforcement) operates at device granularity only until Phase 13a adds per-space keys. The other 7 layers (capability checks, intent verification, behavioral monitoring, etc.) still provide defense-in-depth during this period.

### 6.1 Key Management

```rust
/// The master storage key, derived from the user's identity passphrase.
/// Independent of the device key (§4.10) — different derivation salt.
pub struct MasterKey {
    /// 256-bit key material. Stored on a pinned kernel page
    /// (VmFlags::PINNED | VmFlags::NO_DUMP). Zeroized on drop.
    key_bytes: ZeroizeBox<[u8; 32]>,
    /// How this key was derived.
    derivation: KeyDerivationMethod,
}

pub enum KeyDerivationMethod {
    Argon2id {
        salt: [u8; 32],
        params: Argon2Params,
    },
}

pub struct Argon2Params {
    /// Memory cost in KiB (default: 65536 = 64 MB)
    m_cost: u32,
    /// Time cost / iterations (default: 3)
    t_cost: u32,
    /// Parallelism (default: 4)
    parallelism: u32,
}

pub struct SpaceKeyManager {
    /// Master key derived from user's identity passphrase
    master_key: MasterKey,
    /// Per-space keys (encrypted with master key)
    space_keys: HashMap<SpaceId, EncryptedSpaceKey>,
}

pub struct EncryptedSpaceKey {
    space: SpaceId,
    algorithm: EncryptionAlgorithm,
    encrypted_key: Vec<u8>,             // encrypted with master key
    key_version: u32,
    created_at: Timestamp,
}

pub enum EncryptionAlgorithm {
    Aes256Gcm,                          // default
    ChaCha20Poly1305,                   // alternative
}
```

**Key derivation flow:**
```
1. User authenticates (password, biometric, hardware key)
2. Identity keys unlocked (Ed25519 keypair)
3. Master storage key derived: Argon2id(password, device_salt)
4. Per-space keys decrypted with master key
5. Spaces become accessible
```

**Key rotation:** Space keys can be rotated without re-encrypting all data. New writes use the new key. Old data is re-encrypted in the background. The rotation is tracked by a `KeyRotationManifest` in the WAL — if the system crashes during rotation, recovery resumes re-encryption from the last checkpointed block. Both old and new keys are retained until re-encryption completes, ensuring all blocks are always decryptable. During re-encryption, each block gets a fresh nonce from the `NonceGenerator` (§6.1.1) — the counter increments for every encryption operation, including re-encryption, so nonce reuse never occurs.

### 6.1.1 Nonce Management

AES-256-GCM requires a unique nonce (initialization vector) for every encryption operation under the same key. Reusing a nonce under the same key is catastrophic — it breaks GCM authentication and enables plaintext recovery via ciphertext XOR.

```rust
/// Counter-based nonce generation. Each space key tracks a monotonically
/// increasing counter. The nonce is constructed from the counter + a random
/// component to prevent nonce reuse across crash/recovery boundaries.
pub struct NonceGenerator {
    /// Monotonic counter, persisted to disk with the space key metadata.
    /// Incremented on every block write. On crash recovery, the counter
    /// is advanced by a safety margin (1000) to ensure no reuse.
    counter: AtomicU64,
    /// Random prefix (32 bits), generated at key creation time.
    /// Combined with the 64-bit counter to fill the 96-bit nonce.
    random_prefix: u32,
}

/// Overflow safety threshold. When the counter reaches this value,
/// the space key MUST be rotated before any further encryption.
/// Set to u64::MAX - 2^20 (~1 million operations of safety margin)
/// to ensure no accidental wraparound. At 1 TB/month write rate with
/// 4 KB blocks, this counter lasts ~2.3 billion years — but key rotation
/// after device migration, crash recovery advances, or bulk re-encryption
/// could consume counter space faster. The guard is cheap insurance.
const NONCE_COUNTER_LIMIT: u64 = u64::MAX - (1 << 20);

impl NonceGenerator {
    /// Generate the next nonce. MUST be called exactly once per encryption.
    /// Returns Err if the counter has reached the overflow safety threshold,
    /// requiring a space key rotation before further encryption.
    pub fn next_nonce(&self) -> Result<[u8; 12], NonceExhausted> {
        let count = self.counter.fetch_add(1, Ordering::SeqCst);
        if count >= NONCE_COUNTER_LIMIT {
            return Err(NonceExhausted { space_key_id: self.key_id });
        }
        let mut nonce = [0u8; 12];
        nonce[..4].copy_from_slice(&self.random_prefix.to_le_bytes());
        nonce[4..].copy_from_slice(&count.to_le_bytes());
        Ok(nonce)
    }

    /// On crash recovery: advance counter by safety margin to guarantee
    /// no nonce reuse, even if some writes were lost.
    pub fn recover(&self, last_persisted: u64) {
        self.counter.store(last_persisted + 1000, Ordering::SeqCst);
    }
}

/// Error returned when the nonce counter approaches u64::MAX.
/// The space key must be rotated (§4.10.3) before further encryption.
pub struct NonceExhausted { pub space_key_id: KeyId }
```

**Why counter-based, not random?** Random 96-bit nonces have a birthday collision probability of ~2^-32 after 2^32 encryptions. For a space with millions of blocks across years of edits, this is uncomfortably close. Counter-based nonces guarantee uniqueness as long as the counter never repeats — which the monotonic counter + crash recovery margin ensures.

### 6.1.2 Key Zeroization and Memory Protection

Decrypted space keys are security-critical material. AIOS ensures they cannot leak to swap, remain in memory longer than needed, or be observable via side channels:

```rust
/// A decrypted space key in memory. Automatically zeroized on drop.
pub struct DecryptedSpaceKey {
    /// Key material — allocated on a dedicated kernel page that is:
    /// 1. mlock'd (pinned, never paged to swap or zram)
    /// 2. mprotect'd PROT_READ only (writes go through dedicated API)
    /// 3. Excluded from core dumps
    key_bytes: ZeroizeBox<[u8; 32]>,
    /// Space this key belongs to
    space_id: SpaceId,
    /// Key version (for rotation tracking)
    version: u32,
}

impl Drop for DecryptedSpaceKey {
    fn drop(&mut self) {
        // Zeroize key material before deallocation.
        // Uses volatile writes to prevent compiler optimization.
        self.key_bytes.zeroize();
    }
}
```

**Key lifetime policy:**
- Decrypted keys are loaded when the user authenticates and a space is accessed
- Keys are zeroized when the user locks the screen, logs out, or the space is unmounted
- Keys are stored on pinned kernel pages — never eligible for zram compression or swap
- The kernel page holding key material is mapped with `VmFlags::PINNED | VmFlags::NO_DUMP`

### 6.1.3 Cross-Zone Deduplication Boundaries

Content-addressed storage deduplicates identical blocks — but deduplication across security zones creates a side channel. An agent with access to the Untrusted zone could write known content and check whether the refcount is >1, leaking whether that content exists in an encrypted Personal zone.

**AIOS deduplication is scoped per security zone:**

```
Dedup scope          Blocks compared against     Side channel risk
──────────           ───────────────────────     ────────────────
Core ↔ Core          Yes (same zone)             None (system data, not sensitive)
Personal ↔ Personal  Yes (same zone)             Low (all user's own data)
Untrusted ↔ Untrusted Yes (same zone)            Low (all web-origin data)
Core ↔ Personal      NO (cross-zone)             Blocked
Untrusted ↔ Personal NO (cross-zone)             Blocked
Collaborative ↔ any  Per-space only              Blocked across spaces
```

Each security zone maintains its own content-hash → block mapping in the LSM-tree index. An `Untrusted` block write checks dedup only against other `Untrusted` blocks. This means the same content stored in both `Personal` and `Untrusted` zones is stored twice — intentional, because blocks encrypted with different keys have different ciphertexts and SHA-256 hashes, so cross-zone dedup is impossible for encrypted zones anyway. For unencrypted zones (Core, Ephemeral), cross-zone dedup is still disabled to avoid the refcount side channel. Typical overhead: 5% for users with mostly-distinct content per zone; up to 20-30% for users who intentionally duplicate large media across zones (e.g., a 50 MB photo in both `user/media/` and `web-storage/[origin]/cache-api/`).

### 6.2 Encryption Zones

This table extends the §4.10 encryption zone table with key source information. §4.10 documents what is encrypted; this section documents where keys come from.

| Zone | Space encryption (§6.1) | Device encryption (§4.10) | Key Source |
|---|---|---|---|
| Core (system/) | No | Yes | Device key (hardware-bound or boot passphrase) |
| Personal (user/) | Yes | Yes | Space: user identity master key. Device: device key. |
| Collaborative (shared/) | Yes | Yes | Space: shared key (capability exchange). Device: device key. |
| Untrusted (web-storage/) | Yes | Yes | Space: per-origin key. Device: device key. |
| Ephemeral (/tmp) | No | Yes | Device key only |

All zones are encrypted at the device level. The "Encrypted" column in prior versions of this table referred only to per-space encryption. With device-level transparent encryption (§4.10), nothing is stored as plaintext on the physical medium. Per-space encryption provides additional cross-zone isolation within the running system.

### 6.3 Key Recovery: Prevention-Based Design

AIOS does not implement key escrow or key recovery. There is no seed phrase, no recovery key file, no mnemonic backup. If the user forgets their passphrase and the device is powered off, encrypted data is permanently irrecoverable. This follows the same model as full-disk encryption (LUKS without escrow, VeraCrypt, FileVault without iCloud recovery).

**Why no recovery mechanism:** Every recovery mechanism is an attack surface. A 24-word mnemonic can be stolen, photographed, or socially engineered. A recovery file can be exfiltrated. Key escrow requires either a trusted server (contradicts local-first) or offline material that creates the same custodial burden recovery is supposed to eliminate. For a single-device, local-first, offline-capable system, the added complexity and failure modes outweigh the benefit.

**Prevention-based approach (see [identity.md §14](../experience/identity.md)):**

| Mechanism | Purpose |
|---|---|
| Aggressive session persistence | Master key sealed to TPM/Secure Enclave across sleep/wake. User re-enters passphrase only after cold reboot. Minimizes forgetting. |
| Passphrase change while authenticated | While the session is live, the user can change their passphrase at any time. The "recovery" happens before the user forgets, not after. |
| Clear warning at setup | "If you forget your passphrase and your device is powered off, your data cannot be recovered. This is by design." |
| Multi-device key backup (Phase 9c+) | When multi-device support lands, Device A can hold an encrypted shard of Device B's master key. No seed phrases, no paper — just a second AIOS device. |

**Security properties:**
- No recovery key → no recovery key attack surface (theft, social engineering, phishing)
- No recovery key material on-device → no offline extraction target beyond the passphrase-derived master key
- No external infrastructure dependency → works fully offline, single-device, from day one
- Multi-device key backup (Phase 9c+) adds recovery without custodial burden — leverages Space Sync infrastructure already being built

-----

## 7. Query Engine

### 7.1 Query Dispatch

```rust
/// The four query types supported by the Space Storage query engine.
pub enum SpaceQuery {
    /// Field-based filtering on object metadata. Always available.
    Filter {
        content_type: Option<ContentType>,
        parent: Option<String>,          // object path prefix
        created_after: Option<Timestamp>,
        created_before: Option<Timestamp>,
        modified_after: Option<Timestamp>,
        size_min: Option<u64>,
        size_max: Option<u64>,
        created_by: Option<AgentId>,
    },
    /// Full-text search using the inverted index (BM25 scoring). Always available.
    TextSearch {
        text: String,
        boost_recent: bool,              // weight recent objects higher
        limit: Option<usize>,            // max results (default: 100)
    },
    /// Semantic nearest-neighbor search using HNSW embedding index. Requires AIRS.
    Semantic {
        text: String,                    // query text (embedded by AIRS before search)
        threshold: f32,                  // minimum similarity score (0.0-1.0)
        limit: usize,                    // max results (default: 20)
    },
    /// Graph traversal over the relationship graph (§7.4).
    Traverse {
        start: ObjectId,
        relation_kind: RelationKind,
        depth: u32,                      // max hops (default: 3)
        direction: TraverseDirection,
    },
}

pub enum TraverseDirection {
    /// Follow outgoing edges (source → target).
    Forward,
    /// Follow incoming edges (target → source).
    Reverse,
    /// Follow edges in both directions.
    Bidirectional,
}
```

```rust
impl QueryEngine {
    pub fn query(&self, space: SpaceId, query: SpaceQuery) -> Result<Vec<ObjectId>> {
        match query {
            SpaceQuery::Filter { .. } => self.filter_query(space, query),
            SpaceQuery::TextSearch { .. } => self.text_query(space, query),
            SpaceQuery::Semantic { .. } => self.semantic_query(space, query),
            SpaceQuery::Traverse { .. } => self.traverse_query(space, query),
        }
    }
}
```

### 7.2 Full-Text Index

Maintained by the Space Storage service (not AIRS). Always available:

```rust
pub struct FullTextIndex {
    /// Inverted index: term → posting list (document IDs + positions)
    index: BTreeMap<String, PostingList>,
    /// Total document count for BM25 scoring
    doc_count: u64,
    term_frequencies: HashMap<String, u64>,
}

pub struct PostingList {
    /// Objects containing this term, sorted by ObjectId.
    entries: Vec<PostingEntry>,
}

pub struct PostingEntry {
    object_id: ObjectId,
    /// Byte offsets where this term appears within text_content.
    /// Used for phrase queries and proximity scoring.
    positions: Vec<u32>,
    /// Term frequency in this document (for BM25 scoring).
    frequency: u32,
}
```

Updated synchronously on every write. When an object is created or modified, its text content is extracted and tokenized, and the inverted index is updated. This ensures search always returns current results.

### 7.3 Embedding Index

Maintained by AIRS Space Indexer. Available when AIRS is running:

```rust
pub struct EmbeddingIndex {
    /// HNSW (Hierarchical Navigable Small World) graph for approximate
    /// nearest-neighbor search. Implementation: `hnsw_rs` crate with
    /// AIOS-specific persistence layer (serialized to LSM-tree).
    /// Parameters: m=16, ef_construction=200, ef_search=50.
    hnsw: HnswGraph,
    /// Dimension of embedding vectors
    dimensions: usize,                  // typically 384
    /// Map from embedding position to ObjectId
    id_map: Vec<ObjectId>,
}
```

Updated asynchronously by the Space Indexer. Only promoted full Objects (§3.3.1) are queued for embedding generation — CompactObjects are not embedded until promotion. The index may lag slightly behind the latest writes, but full-text search is always current. Under storage pressure (§10.5), HNSW embeddings for cold objects (not accessed within 30 days) are evicted from memory and regenerated on demand by the Space Indexer on the next semantic query.

### 7.4 Relationship Graph

```rust
pub struct RelationshipGraph {
    /// Forward edges: source → Vec<(target, kind, confidence)>
    forward: HashMap<ObjectId, Vec<Edge>>,
    /// Reverse edges: target → Vec<(source, kind, confidence)>
    reverse: HashMap<ObjectId, Vec<Edge>>,
}

pub struct Edge {
    target: ObjectId,
    kind: RelationKind,
    confidence: f32,                    // 1.0 for explicit, <1.0 for AI-inferred
    created_at: Timestamp,
}
```

Traverse queries walk this graph with configurable depth and direction. Used for provenance chains ("where did this data come from?"), dependency graphs ("what depends on this?"), and similarity exploration ("show me related objects").

### 7.5 Query Composition and Latency

Queries compose by intersecting result sets. Each sub-query runs against its own index, then results are combined:

| Query Type | Backing Index | Always Available? | Expected Latency | Notes |
|---|---|---|---|---|
| `Filter` | Object metadata (in-memory hash maps) | Yes | < 1 ms | Field equality, range checks |
| `TextSearch` | Inverted index (BM25) | Yes (Phase 9a+) | < 50 ms | Full-text with ranking |
| `Semantic` | HNSW embedding index | Requires AIRS | < 500 ms | Nearest-neighbor on embeddings |
| `Traverse` | Relationship graph (adjacency lists) | Yes | < 10 ms/hop | Bidirectional graph walk |

**Composition rules:**

```
AND (implicit):  query(space, Filter { type: "document" } + TextSearch { text: "budget" })
                 → runs Filter (< 1ms), runs TextSearch (< 50ms), intersects results
                 → total: < 51 ms

OR:              union of two separate queries' result sets
                 → run each query independently, merge results

NOT:             difference of result sets
                 → run positive query, run negative query, subtract

Composed:        Filter + Semantic
                 → runs Filter (< 1ms), runs Semantic (< 500ms), intersects
                 → total: < 501 ms (parallel execution: < 500ms)
```

The SDK provides typed query builders that construct composed queries. Internally, the query engine runs independent sub-queries in parallel where possible and intersects the result `ObjectId` sets.

**Graceful degradation:** If AIRS is unavailable, `Semantic` queries return an empty set. Composed queries containing a `Semantic` sub-query fall back to the non-semantic sub-queries only. A `Filter + Semantic` query degrades to `Filter` alone. This is consistent with the system-wide principle that AIRS enhances but is never required.

-----

## 8. Space Sync Protocol

> **Implementation status:** Phase 9c. This section documents the design intent for cross-device synchronization. Single-device operation (Phases 4-8) does not use sync. The data structures and protocol described here are the target design, not current implementation.

Spaces can synchronize across devices. This is how collaborative spaces work and how user data replicates across AIOS devices.

```rust
/// Identifies a space on a remote device. Used for cross-device sync.
pub struct RemoteSpaceId {
    /// The remote device's identity (Ed25519 public key).
    device_id: IdentityId,
    /// The space's ID on the remote device.
    space_id: SpaceId,
}

pub struct SpaceSync {
    local: SpaceId,
    remote: RemoteSpaceId,
    policy: SyncPolicy,
    state: SyncState,
}

pub enum SyncPolicy {
    /// Full bidirectional sync
    Full,
    /// Pull only (read-only mirror)
    PullOnly,
    /// Push only (backup)
    PushOnly,
    /// Selective (sync objects matching filter)
    Selective { filter: SpaceQuery },
}

pub struct SyncState {
    last_sync: Timestamp,
    local_version: Hash,               // Merkle root of local space
    remote_version: Hash,              // last known remote Merkle root
    pending_push: Vec<ObjectId>,       // locally modified, not yet pushed
    pending_pull: Vec<ObjectId>,       // remotely modified, not yet pulled
    conflicts: Vec<SyncConflict>,
}

pub struct SyncConflict {
    object: ObjectId,
    local_version: Version,
    remote_version: Version,
    resolution: SyncConflictPolicy,
}
```

### 8.1 Merkle Exchange Protocol

Sync proceeds in three rounds over the NTM transport:

```
Round 1 — Root exchange:
  Local  → Remote:  { space_id, local_merkle_root, epoch }
  Remote → Local:   { remote_merkle_root, epoch }
  If roots match → spaces are identical, sync complete.

Round 2 — Subtree diff:
  Both sides walk their Merkle trees level by level, exchanging subtree hashes.
  At each level, mismatched subtrees are expanded; matching subtrees are skipped.
  This narrows the diff to the specific objects that changed.
  Cost: O(changed_objects × tree_depth), not O(total_objects).

Round 3 — Delta transfer:
  For each changed object:
    a. Sender transmits the Version chain (§5) from common ancestor to head
    b. Receiver verifies each Version hash (Merkle chain integrity)
    c. Content blocks are transferred only if not already present (content-addressing
       means the receiver may already have the block from another object)
    d. Receiver appends version nodes to its local DAG
```

**Bandwidth efficiency:** Content-addressed blocks mean identical content is never transferred twice, even across different objects. A 10 MB file that exists on both devices with different metadata requires only the version node transfer (~200 bytes), not the content.

### 8.2 Conflict Resolution

A conflict occurs when both sides have modified the same object since the last common ancestor (a DAG fork, §5.5). The `SyncConflictPolicy` determines resolution:

```rust
pub enum SyncConflictPolicy {
    /// Last-writer-wins based on timestamp. Simple, non-interactive.
    /// Risk: silently discards one side's changes.
    LastWriterWins,
    /// Keep both versions as branches. The user resolves manually.
    /// The object has two heads until resolution.
    Fork,
    /// For structured content (e.g., JSON, key-value): attempt field-level
    /// three-way merge using the common ancestor. Fall back to Fork if
    /// the merge produces ambiguity.
    ThreeWayMerge,
    /// Defer to user. Sync pauses for this object; both versions are
    /// available for inspection. User chooses via Inspector or CLI.
    Manual,
}
```

**Default policy per zone:**

| Zone | Default policy | Rationale |
|---|---|---|
| Personal (`user/`) | `Manual` | User data is precious — never silently discard |
| Collaborative (`shared/`) | `ThreeWayMerge`, fall back to `Fork` | Collaborative editing benefits from auto-merge |
| Core (`system/`) | `LastWriterWins` | System config changes are idempotent |
| Ephemeral (`/tmp`) | Not synced | Ephemeral data is device-local |

### 8.3 Sync Security

Sync introduces a network trust boundary. Before any data exchange:

1. **Mutual identity verification.** Both devices perform an Ed25519 challenge-response using their device identity keys (§6.1). The remote device must present a key that the local device has previously authorized via a pairing ceremony — manual confirmation on both devices (e.g., scan QR code, enter matching PIN, or biometric). Each space maintains a sync ACL: a list of `(device_id, permissions)` tuples authorized to participate in sync.
2. **Capability check.** Initiating sync requires `SyncSpace(space_id)` capability. Accepting sync requires that the remote identity is in the space's sync ACL.
3. **Encrypted transport.** All sync traffic is encrypted end-to-end by the NTM ([networking.md](../platform/networking.md)). The Space Sync protocol never sees plaintext on the wire — it hands structured messages to the NTM, which handles TLS/Noise encryption.
4. **Content verification.** Every received version node and content block is verified against its content hash before being written to the local DAG. A malicious or corrupted remote cannot inject invalid data — the Merkle chain rejects it.

### 8.4 Transport Failure Handling

Network connections are unreliable. The sync protocol handles failures gracefully:

- **Resumable transfers.** Sync state (§8 `SyncState`) tracks `pending_push` and `pending_pull` queues. If the connection drops mid-sync, the next sync attempt resumes from where it left off — already-transferred objects are not re-sent (content-hash dedup catches this).
- **Exponential backoff.** Failed sync attempts retry with exponential backoff (30s, 1m, 2m, 5m, 15m, capped at 1h). Background sync is opportunistic — it does not burn battery or bandwidth on repeated failures.
- **Bandwidth throttling.** Sync runs at `Idle` scheduling class (scheduler.md §3.1) and respects a configurable bandwidth ceiling. Interactive network traffic (agent API calls, web requests) always takes priority.

**Encryption for synced spaces:** Personal spaces use per-device space keys — each device derives its own key from the user's passphrase (same passphrase, same derivation, same key). Collaborative spaces use a shared key distributed during the pairing ceremony (encrypted with the receiving device's public key). Untrusted spaces (web storage) are not synced.

**Sync uses the Network Translation Module (NTM).** The NTM ([networking.md](../platform/networking.md)) provides encrypted point-to-point channels between devices. Space Sync sends structured messages to the NTM, which handles TLS/Noise encryption, routing, and retry logic. Space Sync code never deals with plaintext on the wire. Remote spaces are accessed via space operations (`space::remote("device-b/shared/project")`). Sync IPC messages will be defined in ipc.md (Phase 9c, not yet specified).

-----

## 9. POSIX Compatibility

### 9.1 Path Mapping

The POSIX emulation layer maps filesystem paths to space operations:

```
/spaces/[space-name]/[object-path]  →  space query + object access
/home/user/                          →  user/home/ space
/tmp/                                →  ephemeral space (auto-cleaned; no version history,
                                        device-encrypted only, cleared on shutdown)
/dev/null, /dev/urandom             →  device capabilities
/proc/self/                          →  process introspection
/bin/, /usr/bin/                     →  system utilities space
```

**Path resolution:** `/spaces/research/papers/ml/bert.pdf` resolves to space-name `"research"` (first component after `/spaces/`) and object-path `"papers/ml/bert.pdf"` (remaining components). Objects with `/` in their name (uncommon) are URL-encoded as `%2F` in the POSIX path. The POSIX bridge decodes on translation.

### 9.2 Translation Layer

```rust
/// POSIX directory entry, returned by readdir().
pub struct DirEntry {
    name: String,
    object_id: ObjectId,
    content_type: ContentType,
    size: u64,
    modified_at: Timestamp,
}

/// POSIX stat result, returned by stat().
pub struct Stat {
    size: u64,
    modified: u64,                      // seconds since epoch
    mode: u32,                          // synthesized POSIX mode bits
    nlink: u32,                         // always 1 (spaces don't have hard links)
}
```

Object methods used by the POSIX bridge:

```rust
impl Object {
    /// Convert to POSIX directory entry.
    pub fn to_dir_entry(&self) -> DirEntry {
        DirEntry {
            name: self.name.clone(),
            object_id: self.id,
            content_type: self.content_type,
            size: self.content_size,
            modified_at: self.modified_at,
        }
    }

    /// Synthesize POSIX mode bits from capabilities.
    /// Read bits set if calling agent has ReadSpace; write bits if WriteSpace.
    /// Directories get 0o755; files get 0o644 by default.
    pub fn to_posix_mode(&self) -> u32 {
        if self.content_type == ContentType::Directory { 0o755 } else { 0o644 }
    }
}
```

```rust
/// Standard POSIX types used in this section:
///   OpenFlags — bitflags (O_RDONLY, O_WRONLY, O_RDWR, O_CREAT, O_EXCL, etc.)
///   AccessMode, Mode — POSIX permission types
///   CapabilitySet — kernel capability set (security.md §4)

pub struct PosixSpaceBridge {
    mount_table: Vec<MountEntry>,
}

pub struct MountEntry {
    posix_path: String,                 // "/spaces/research"
    space: SpaceId,
    capabilities: CapabilitySet,        // from calling process's agent (security.md §4)
}

impl PosixSpaceBridge {
    fn open(&self, path: &str, flags: OpenFlags) -> Result<Fd> {
        let (space, object_path) = self.resolve_path(path)?;
        let cap = if flags.contains(O_WRONLY | O_RDWR) {
            Capability::WriteSpace(space)
        } else {
            Capability::ReadSpace(space)
        };
        gate_check(current_agent(), cap)?;
        let object = match space.resolve_object(object_path) {
            Ok(obj) => {
                if flags.contains(O_CREAT | O_EXCL) {
                    return Err(Error::NameExists); // EEXIST
                }
                obj
            }
            Err(Error::ObjectNotFound) if flags.contains(O_CREAT) => {
                space.create_object(object_path, current_agent())?
            }
            Err(e) => return Err(e),
        };
        Ok(self.create_fd(object, flags))
    }

    fn readdir(&self, path: &str) -> Result<Vec<DirEntry>> {
        let (space, prefix) = self.resolve_path(path)?;
        let objects = space.query(SpaceQuery::Filter {
            parent: Some(prefix),
            ..default()
        })?;
        Ok(objects.iter().map(|o| o.to_dir_entry()).collect())
    }

    fn stat(&self, path: &str) -> Result<Stat> {
        let (space, object_path) = self.resolve_path(path)?;
        let object = space.resolve_object(object_path)?;
        Ok(Stat {
            size: object.content_size,
            modified: object.modified_at.to_timespec(),
            mode: object.to_posix_mode(),
            // ...
        })
    }
}
```

BSD tools never know they're not on a traditional filesystem. `ls /spaces/research/` returns a directory listing. `grep` searches file content. `cat` reads objects. The translation is transparent.

POSIX syscall translations are dispatched through IPC to the Space Service. See [ipc.md §12.2 Gap 6](../kernel/ipc.md) for the POSIX translation performance model (5 μs round-trip target) and the read-ahead, vnode cache, batched readdir, and write-coalescing optimizations that amortize IPC cost.

### 9.3 Write Path

The POSIX bridge translates mutation syscalls into space operations:

```rust
impl PosixSpaceBridge {
    fn write(&self, fd: Fd, buf: &[u8]) -> Result<usize> {
        let file = self.fd_table.get_mut(fd)?;
        gate_check(current_agent(), Capability::WriteSpace(file.space))?;
        // Buffer writes in the fd's write buffer (write coalescing —
        // see ipc.md §12.2 Gap 6). Flush to Space Service on fsync/close
        // or when buffer is full (default 64 KB).
        file.write_buf.extend_from_slice(buf);
        if file.write_buf.len() >= WRITE_COALESCE_THRESHOLD {
            self.flush(fd)?;
        }
        file.cursor += buf.len() as u64;
        Ok(buf.len())
    }

    fn close(&self, fd: Fd) -> Result<()> {
        let file = self.fd_table.get(fd)?;
        // Flush any buffered writes
        if !file.write_buf.is_empty() {
            self.flush(fd)?;
        }
        // Release the fd. If this is the last reference (no dup'd copies),
        // the object handle is released back to the Space Service.
        self.fd_table.release(fd)
    }

    fn rename(&self, old_path: &str, new_path: &str) -> Result<()> {
        let (src_space, src_obj_path) = self.resolve_path(old_path)?;
        let (dst_space, dst_obj_path) = self.resolve_path(new_path)?;
        gate_check(current_agent(), Capability::WriteSpace(src_space))?;
        if src_space != dst_space {
            gate_check(current_agent(), Capability::WriteSpace(dst_space))?;
        }
        // Rename is a metadata update — the content blocks are unchanged.
        // Cross-space rename is a copy + delete (atomic via WAL).
        src_space.rename_object(src_obj_path, dst_space, dst_obj_path)
    }

    fn unlink(&self, path: &str) -> Result<()> {
        let (space, object_path) = self.resolve_path(path)?;
        gate_check(current_agent(), Capability::WriteSpace(space))?;
        // Unlink removes the object from the space. The version DAG (§5)
        // is retained — the object can be recovered via rollback until
        // version retention prunes the history (§5.4).
        space.delete_object(object_path)
    }

    fn mkdir(&self, path: &str, _mode: Mode) -> Result<()> {
        let (space, dir_path) = self.resolve_path(path)?;
        gate_check(current_agent(), Capability::WriteSpace(space))?;
        // Directories in spaces are implicit — they exist when objects
        // have matching prefixes. mkdir creates a zero-length marker object
        // with content_type Directory so that readdir returns the directory
        // even when empty.
        space.create_object(dir_path, ContentType::Directory, &[])
    }
}

const WRITE_COALESCE_THRESHOLD: usize = 64 * 1024; // 64 KB
```

Write coalescing applies to syscall-based writes (`write`, `pwrite`). `mmap` is not supported in Phase 4 (spaces use content-addressed blocks, not page-granularity storage). `O_DIRECT` flag is ignored — all writes go through the coalesce buffer for consistency.

**Atomicity:** `rename` within a single space is atomic (single LSM-tree key update, WAL-protected). Cross-space rename is atomic via a compound WAL entry: `[type=COMPOUND_OP][op1=DELETE src_space/src_path][op2=CREATE dst_space/dst_path content_hash][checksum]`. On crash recovery, WAL replay either commits both operations or neither — intermediate states (source deleted, destination not created) never persist to the LSM-tree.

### 9.4 File Descriptor Lifecycle

File descriptors are the POSIX bridge's core state. Each open fd tracks its object binding, cursor position, and buffered I/O state:

```rust
pub struct OpenFile {
    fd: Fd,
    space: SpaceId,
    object: ObjectId,
    /// The version hash this fd was opened against. Reads always see this
    /// version's content, even if the object is modified by another agent
    /// after open. This is snapshot isolation — consistent with POSIX
    /// semantics where open() returns a stable file reference.
    pinned_version: Hash,
    cursor: u64,
    flags: OpenFlags,
    mode: AccessMode,
    /// Reference count. Incremented by dup/dup2/fork. The fd is released
    /// only when refcount drops to zero.
    refcount: u32,
    /// Write buffer for coalesced writes (§9.3).
    write_buf: Vec<u8>,
    /// Read-ahead buffer (ipc.md §12.2 Gap 6).
    read_buf: ReadAheadBuffer,
}

pub struct ReadAheadBuffer {
    data: [u8; READ_AHEAD_SIZE],
    /// Range of the object currently cached in this buffer.
    /// Coherent with pinned_version: the buffer always contains data from
    /// the pinned version. If another agent modifies the object, a new version
    /// is created but the pinned version's content blocks are immutable, so
    /// the read-ahead buffer remains valid.
    cached_range: Option<Range<u64>>,
}

const READ_AHEAD_SIZE: usize = 64 * 1024; // 64 KB
```

**Version pinning:** When a file is opened, the fd records the current head version of the object. All reads through this fd return data from the pinned version. If another agent modifies the object while the fd is open, the fd still sees the old content. This matches POSIX behavior where `open()` + `read()` is not affected by concurrent `write()` to the same file (assuming no shared mmap). New `open()` calls see the latest version.

**dup / fork semantics:** `dup(fd)` increments the refcount on the `OpenFile` — the duplicate shares the same cursor, buffers, and pinned version. `fork()` duplicates the fd table for the child process, incrementing all refcounts. The `OpenFile` is released when all references are closed.

**What happens on object deletion:** If another agent deletes the object while an fd is open, reads through the existing fd continue to work (the pinned version's content blocks are still in the Block Engine — version retention guarantees this). New `open()` calls to the same path return `ENOENT`. This matches POSIX unlink semantics where open file handles survive deletion. In the rare case where the pinned version is garbage-collected due to extreme storage pressure (§5.4), subsequent reads through the fd return `EIO`. In practice, this is unlikely because version retention always preserves at least the current head and most recent snapshot, and fds are typically short-lived.

### 9.5 Error Mapping

Space operations produce structured errors. The POSIX bridge maps them to errno values:

| Space error | POSIX errno | Notes |
|---|---|---|
| `ObjectNotFound` | `ENOENT` | Object does not exist at this path |
| `SpaceNotFound` | `ENOENT` | Space does not exist |
| `CapabilityDenied` | `EACCES` | Agent lacks the required capability |
| `ReadOnlySpace` | `EROFS` | Write to a pull-only synced or system space |
| `SpaceFull` | `ENOSPC` | Space quota exceeded (§10) |
| `DeviceFull` | `ENOSPC` | Device storage exhausted |
| `ObjectLocked` | `EBUSY` | Object is exclusively locked by another operation |
| `InvalidPath` | `EINVAL` | Path contains invalid characters or exceeds length |
| `NameExists` | `EEXIST` | Object already exists at this path (for O_CREAT \| O_EXCL) |
| `TooManyOpenFiles` | `EMFILE` | Process fd table full |
| `VersionConflict` | `EAGAIN` | Concurrent modification detected; retry |
| `EncryptionKeyUnavailable` | `EACCES` | Space is encrypted and the key is not loaded (screen locked) |
| `IoError` | `EIO` | Block Engine or storage driver error |

**Unmapped POSIX concepts:** Spaces do not have traditional POSIX mode bits (`rwxrwxrwx`). The `stat()` call (§9.2) synthesizes mode bits from capabilities: if the calling agent has `ReadSpace`, the read bits are set; if it has `WriteSpace`, the write bits are set. Group and other bits are not meaningful — the capability system replaces POSIX user/group/other permissions. `chmod` and `chown` are no-ops that return success (POSIX compliance without effect, since capabilities are the real access control).

### 9.6 Change Notification

POSIX tools expect filesystem event APIs (`inotify` on Linux, `kqueue` on BSD). The POSIX bridge maps these to space event subscriptions:

```rust
impl PosixSpaceBridge {
    /// inotify_add_watch equivalent. Subscribes to changes on objects
    /// matching a path prefix within a space.
    fn watch(&self, path: &str, events: WatchEvents) -> Result<WatchId> {
        let (space, prefix) = self.resolve_path(path)?;
        gate_check(current_agent(), Capability::ReadSpace(space))?;
        // The Version Store (§5) emits events whenever a new version node
        // is appended. The watch subscription filters these events by
        // object path prefix and event type.
        let sub = space.subscribe(SpaceEventFilter {
            prefix: Some(prefix),
            event_types: events.to_space_events(),
        })?;
        Ok(self.watch_table.register(sub))
    }
}

/// Watch subscription identifier. Returned by watch(), used by unwatch().
pub type WatchId = u64;

pub struct SpaceEventFilter {
    /// Object path prefix to watch (None = entire space).
    prefix: Option<String>,
    /// Event types to subscribe to.
    event_types: Vec<SpaceEventType>,
}

pub enum SpaceEventType {
    Created,                // new object in prefix
    Modified,               // new version appended
    Deleted,                // object deleted
    Renamed,                // object renamed
}

pub struct WatchEvents {
    pub create: bool,    // IN_CREATE → SpaceEventType::Created
    pub modify: bool,    // IN_MODIFY → SpaceEventType::Modified
    pub delete: bool,    // IN_DELETE → SpaceEventType::Deleted
    pub rename: bool,    // IN_MOVED_FROM/TO → SpaceEventType::Renamed
}
```

The Version Store already tracks every modification as a version node (§5). Change notification is a read-only view of version events filtered by path prefix — no new storage machinery is needed. Events are delivered asynchronously via the IPC notification mechanism (ipc.md §3.1 `NotificationSignal`). Tools like `tail -f`, `fswatch`, and build systems with file watchers work transparently.

-----

## 10. Storage Budget and Pressure Management

### 10.1 Device Profiles

AIOS initially targets **laptops and PCs** but is architectured for multi-device support. The storage system uses device profiles to adapt quotas, pressure thresholds, and model caching strategies to each class of hardware. Only the Laptop/PC profile is active at launch; others are defined here for architectural foresight and will be activated when hardware support is added.

```rust
pub enum DeviceProfile {
    /// Initial target. 256 GB - 2 TB SSD. 8-64 GB RAM.
    /// Comfortable storage — multiple models, generous version history.
    LaptopPC,

    /// Future. 256 GB - 1 TB. 6-8 GB RAM.
    /// Storage similar to laptops but RAM is much tighter.
    /// Models compete with apps for limited RAM.
    Tablet,

    /// Future. 128 GB - 1 TB. 6-8 GB RAM.
    /// Apps and media consume 50-70% of storage.
    /// AIOS competes for the remaining 30-50%.
    Phone,

    /// Future. 16 GB - 128 GB. Limited RAM.
    /// Streaming-first: models streamed from network or hub device.
    /// Minimal local storage for config + cache.
    TV,

    /// Future. 32 GB - 256 GB SD/eMMC. 2-8 GB RAM.
    /// Tight on everything. Single model, aggressive eviction.
    SingleBoardComputer,
}

impl DeviceProfile {
    pub fn detect() -> Self {
        // At launch: always returns LaptopPC
        // Future: detect from hardware inventory (storage size, RAM, device tree)
        DeviceProfile::LaptopPC
    }
}
```

**Why device profiles matter for storage:**

| Device | Typical Storage | Apps/Media Pressure | AIOS Available | Model Strategy |
|---|---|---|---|---|
| **Laptop/PC** | 256 GB - 2 TB | Low (20-30%) | 180-1400 GB | Multiple models on disk |
| Tablet (future) | 256 GB - 1 TB | Medium (40-50%) | 130-500 GB | 2-3 models on disk |
| Phone (future) | 256 GB - 1 TB | **High (50-70%)** | 75-300 GB | 1-2 models on disk |
| TV (future) | 16-128 GB | Medium (apps) | 8-60 GB | Stream from network |
| SBC (future) | 32-256 GB | Low | 28-230 GB | Single model, aggressive eviction |

Phones are the tightest — 256 GB minimum these days, but apps and games consume 50-70% of that. On a 256 GB phone with 60% used by apps/media, AIOS gets ~100 GB. That's still workable but requires careful budgeting. This constraint doesn't apply to the initial laptop/PC target where storage pressure from other apps is much lower.

### 10.2 Storage Budget — Laptop/PC (Initial Target)

On laptops and PCs, storage is relatively generous. A typical laptop has 256 GB - 1 TB, and user apps/data (outside AIOS) rarely consume more than 20-30%. The storage budget reflects this:

```
Storage budget for laptops/PCs (estimated, after OS partition overhead):

                        256 GB SSD    512 GB SSD    1 TB SSD      2 TB SSD
                        ──────────    ──────────    ────────      ────────
Usable after format:    ~238 GB       ~476 GB       ~931 GB       ~1863 GB

AI models:               15-30 GB      30-60 GB      50-100 GB     100-200 GB
  (3-6 models)          (mix of 8B,   (8B + 13B +   (full model    (full library
                         13B, vision)  70B Q4)        library)      + large models)

OS + system spaces:      3-4 GB        3-4 GB        4-5 GB        4-5 GB
  (kernel, agents,
   credentials, config)

Indexes + audit:         2-5 GB        4-10 GB       8-20 GB       15-40 GB
  (FTI, HNSW, audit
   Merkle chain)

Version history:         10-25 GB      20-50 GB      40-80 GB      50-100 GB
  (generous retention;
   KeepLast(50) laptop override; base default KeepLast(20))

User data:               80-150 GB     200-300 GB    400-600 GB    800-1200 GB
  (documents, media,
   conversations, code)

Web storage:             3-10 GB       5-15 GB       10-25 GB      15-40 GB
  (per-origin storage,
   browser cache)

Free headroom:           35-70 GB      70-140 GB     140-280 GB    280-560 GB
  (target: ≥15% free)
```

**Key differences from constrained devices:**
- **Multiple models fit comfortably.** A 256 GB laptop can hold 3-6 models (15-30 GB) without meaningful pressure. A 1 TB laptop can store every model a user might want.
- **Generous version history.** Default retention can be `KeepLast(50)` instead of `KeepLast(20)`. On 512 GB+, `KeepAll` is viable for spaces the user cares about.
- **Full embedding index.** Enough space and RAM to maintain embeddings for all promoted objects, not just a subset.
- **70B models become feasible.** A Q4-quantized 70B model is ~40 GB. On a 512 GB laptop with 64 GB RAM, this is the first device class where it's practical to store and run.

### 10.3 Storage Budget — Future Device Classes

> **Implementation status:** Phase 14+. These budgets are for architectural planning. Phase 4 targets LaptopPC only. Phone, TV, and SBC profiles will be activated when hardware support is added.

These budgets are not active yet. They exist for architectural planning so the storage system doesn't make assumptions that only work on laptops.

```
Phone (future, 256 GB with 60% apps/media):
  AIOS available:      ~100 GB
  AI models:            8-15 GB   (1-2 models, prefer smaller quantizations)
  OS + system:          2-3 GB
  Indexes + audit:      1-3 GB
  Version history:      5-15 GB   (KeepLast(20) default)
  User data:            40-60 GB
  Web storage:          2-5 GB
  Free headroom:        15-25 GB

TV (future, 32 GB):
  AIOS available:       ~20 GB
  AI models:            0-2 GB    (stream from network or hub device;
                                   cache small model for offline)
  OS + system:          2 GB
  Indexes + audit:      0.5 GB
  Version history:      1-3 GB    (KeepLast(5) default)
  User data:            5-10 GB   (preferences, watchlists, conversation history)
  Web storage:          1-2 GB
  Free headroom:        3-5 GB

SBC (future, 64 GB):
  AIOS available:       ~55 GB
  AI models:            4.5-8 GB  (1-2 small models)
  OS + system:          2 GB
  Indexes + audit:      1-2 GB
  Version history:      3-8 GB    (KeepLast(10) default)
  User data:            15-25 GB
  Web storage:          1-3 GB
  Free headroom:        8-15 GB
```

### 10.4 Storage Quotas by Category

Each storage category has a quota to prevent any single concern from consuming the device. Quotas are parameterized by device profile:

```rust
pub struct StorageBudget {
    total_usable: u64,
    profile: DeviceProfile,
    quotas: StorageQuotas,
}

pub struct StorageQuotas {
    /// AI model storage — GGUF files on disk
    /// LaptopPC default: 20% of usable space
    /// Phone default: 15%
    /// TV default: 10% (streaming preferred)
    models: StorageQuota,

    /// System spaces (OS, agents, credentials, config)
    /// Default: 5% of usable space, minimum 2 GB
    system: StorageQuota,

    /// Indexes and audit (FTI, HNSW, Merkle chain)
    /// Default: 5% of usable space, minimum 1 GB
    indexes_audit: StorageQuota,

    /// Version history (Merkle DAG, old content blocks)
    /// LaptopPC default: 15% of usable space
    /// Phone default: 10%
    /// TV default: 5%
    versions: StorageQuota,

    /// User data (personal spaces — documents, media, conversations)
    /// Default: no hard limit — gets whatever is left
    user_data: StorageQuota,

    /// Web storage (per-origin: cookies, localStorage, IndexedDB, cache)
    /// Default: 5% of usable space, max 5 GB per origin
    web_storage: StorageQuota,

    /// Minimum free headroom — triggers pressure response when breached
    /// Default: 20% of usable space (matches StoragePressure::Normal threshold)
    free_headroom_target: f64,
}

pub struct StorageQuota {
    /// Percentage of total usable space
    percentage: f64,
    /// Absolute minimum (never go below this)
    minimum: Option<u64>,
    /// Absolute maximum (never exceed this)
    maximum: Option<u64>,
    /// Current usage
    used: u64,
}
```

**User data has no hard cap.** The user's own files are the reason the device exists. Every other category has a ceiling; user data gets whatever isn't claimed by quotas and headroom. If a user fills their device with photos and documents, that's their choice — the system adapts by tightening version retention and deferring index work.

### 10.5 Storage Pressure Response

Like memory pressure (see [memory.md §8](../kernel/memory.md)), storage has pressure levels with escalating responses:

```rust
/// Analogous to MemoryPressure in memory.md — same thresholds, but worst
/// level is Emergency (not Oom), since storage exhaustion is recoverable.
pub enum StoragePressure {
    /// > 20% free — normal operation
    Normal,
    /// 10-20% free — start reclaiming
    Low,
    /// 5-10% free — aggressive reclamation
    Critical,
    /// < 5% free — emergency mode
    Emergency,
}

/// Events emitted by the storage budget system. Visible in the Inspector
/// and delivered to subscribed agents via IPC notification.
pub enum StorageEvent {
    PressureChanged {
        from: StoragePressure,
        to: StoragePressure,
    },
    VersionRetentionReduced {
        space: SpaceId,
        old_depth: u32,
        new_depth: u32,
    },
    QuotaExceeded {
        category: StorageCategory,
        used: u64,
        limit: u64,
    },
    ModelEvicted {
        model_id: ModelId,
        freed_bytes: u64,
    },
}

pub enum StorageCategory {
    Models,
    System,
    IndexesAudit,
    Versions,
    UserData,
    WebStorage,
}
```

```
Pressure response table:

Level       Free %    Actions
──────────  ──────    ──────────────────────────────────────────────────────
Normal      > 20%     Normal operation. GC runs on schedule.
                      Version retention per space quota.

Low         10-20%    - Tighten retention: KeepAll → KeepLast(10); KeepLast(n) → KeepLast(min(n, 5))
                      - Run GC immediately (don't wait for threshold)
                      - Evict embedding index entries for cold objects
                        (regenerated on demand)
                      - Compress warm blocks → cold (zstd level 9)
                      - Notify user: "Storage getting low. [X] GB free."

Critical    5-10%     - Tighten version retention: KeepLast(5) → KeepLast(2)
                      - Purge web-storage caches (Cache API, not localStorage)
                      - Compact audit logs (force summary tier for >3 days)
                      - Evict all non-primary model files from disk
                        (re-download on demand)
                      - Pause Space Indexer (no new embeddings)
                      - Notify user: "Storage critically low. Free up space
                        or data may be affected."

Emergency   < 5%      - Version retention: KeepLast(1) (current version only)
                      - Purge ALL web-storage except localStorage
                      - Delete all non-primary model files
                      - Halt all background writes (indexing, audit flush)
                      - Block new object creation from background agents
                      - Interactive writes still allowed (user comes first)
                      - Notify user: "Storage full. Only essential operations
                        are possible. Please free space immediately."
```

**Reclamation coordination:** When pressure triggers reclamation, actions execute in priority order: (1) tier demotion — recompress warm blocks to cold (zstd level 9), often freeing 20-40% of block storage with no data loss; (2) version retention pruning — reduce history depth per the table above; (3) embedding index eviction — remove HNSW entries for cold objects (regenerated on demand); (4) web storage purge — delete Cache API and browser caches; (5) model eviction — delete re-downloadable model files. Blocks marked for tier demotion are never deleted before recompression completes. Blocks are released only after all versions referencing them are pruned.

**Quota enforcement:** Quotas use soft limits by default — pressure response triggers at threshold, but writes succeed. Hard limits apply to web storage (5 GB per origin) and model storage (configurable). User data has no hard limit — it gets whatever is left after other categories. When a hard limit is breached, writes fail with `ENOSPC`. When a soft limit is breached, the pressure response escalates but writes continue.

### 10.6 Model Storage Strategy

AI model files are the single largest storage consumer and unlike user data, they are **reproducible** — a deleted model can be re-downloaded. This makes them the best target for reclamation under storage pressure.

```rust
pub struct ModelStoragePolicy {
    /// Maximum disk space for all model files combined
    max_disk: u64,                      // from StorageQuotas.models
    /// Models currently on disk
    on_disk: Vec<ModelDiskEntry>,
    /// Device profile determines eviction behavior
    profile: DeviceProfile,
    /// The primary model (never evicted)
    primary_model: ModelId,
}

impl ModelStoragePolicy {
    fn is_primary(&self, id: &ModelId) -> bool { &self.primary_model == id }
}

pub enum ModelSource {
    Bundled,
    Downloaded,
    UserProvided,
}

pub struct ModelDiskEntry {
    model_id: ModelId,
    file_size: u64,
    last_loaded: Timestamp,
    source: ModelSource,
    /// Can this model be re-downloaded if deleted?
    reproducible: bool,
}

impl ModelStoragePolicy {
    /// Select models to delete from disk when storage is under pressure
    pub fn select_eviction(&self) -> Vec<ModelId> {
        // Never delete the primary model
        // Never delete user-provided models (not re-downloadable)
        // Delete downloaded models that haven't been loaded recently
        // Prefer deleting larger models first (more space recovered)
        self.on_disk.iter()
            .filter(|m| m.reproducible && !self.is_primary(&m.model_id))
            .sorted_by(|a, b| b.file_size.cmp(&a.file_size))
            .map(|m| m.model_id.clone())
            .collect()
    }
}
```

**Per-device strategy:**

| Device Profile | Models on Disk | Eviction | Notes |
|---|---|---|---|
| **Laptop/PC (initial)** | 3-10+ depending on SSD size | LRU when quota exceeded | 70B models feasible on 512 GB+ with 64 GB RAM |
| Tablet (future) | 2-3 | LRU when quota exceeded | Similar to laptop but RAM limits model size |
| Phone (future) | 1-2 | Aggressive — delete on model switch | Apps compete for storage; keep models small (8B Q4) |
| TV (future) | 0-1 (small cache) | Stream from hub device or network | Local cache only for offline fallback |
| SBC (future) | 1 | Delete old before downloading new | Single model at a time on <64 GB |

**On laptops/PCs (the initial target):** Storage pressure from models is rare. A 256 GB SSD with a 20% model quota has ~48 GB for models — enough for 10+ 8B models, or 3-4 8B models plus a 70B Q4. Eviction only triggers when the user collects more models than the quota allows, and even then it's LRU: the least recently loaded model file is deleted first. The user is notified and can re-download at any time.

**On future constrained devices (phones, TVs, SBCs):** Model storage becomes the critical constraint. On a phone where AIOS gets ~100 GB and the model quota is 15% (~15 GB), only 1-2 models fit. Model streaming becomes important: download on demand, cache while in use, evict when not needed.

**Streaming model download:** Instead of downloading the entire GGUF file before starting inference, AIOS can stream model weights via mmap over a network-backed file. The NTM fetches blocks on demand as page faults occur. This eliminates the need to store the full model file on disk at the cost of inference speed (network latency per page fault). On laptops with fast WiFi/ethernet, the latency penalty is small. On TVs with network access to a hub device on the local network, this is the primary model delivery mechanism.

### 10.7 Version History Budget

Version history is the hidden storage multiplier. A user who edits a 1 MB document daily for a year generates 365 MB of version data for that one file (before deduplication). Across thousands of objects, this adds up fast.

```rust
pub struct AdaptiveRetention {
    /// Base policy (from space quota)
    base: RetentionPolicy,
    /// Adjusted policy under storage pressure
    pressure_adjusted: Option<RetentionPolicy>,
}

impl AdaptiveRetention {
    pub fn effective_policy(&self, pressure: StoragePressure) -> RetentionPolicy {
        match pressure {
            StoragePressure::Normal => self.base.clone(),
            StoragePressure::Low => match &self.base {
                RetentionPolicy::KeepAll => RetentionPolicy::KeepLast(10),
                RetentionPolicy::KeepLast(n) => RetentionPolicy::KeepLast((*n).min(5)),
                other => other.clone(),
            },
            StoragePressure::Critical => RetentionPolicy::KeepLast(2),
            StoragePressure::Emergency => RetentionPolicy::KeepLast(1),
        }
    }
}
```

**Deduplication helps significantly.** Content-addressed blocks mean that small edits to a large file only store the changed blocks, not the entire file again. A 1 MB document with a one-line edit stores ~4 KB of new data (one changed block), not 1 MB. For typical editing patterns, deduplication reduces version history from 365× to ~20-50× the original size over a year.

**Space-level retention is configurable.** User-facing spaces default to `KeepLast(20)` — the last 20 versions of each object. System spaces default to `KeepLast(5)`. Web storage defaults to `KeepLast(1)` (current version only — no version history for cookies). Users can override these per space.

### 10.8 Storage Monitoring

The Inspector exposes real-time storage analytics:

```
Storage Dashboard (example: 512 GB laptop):
┌───────────────────────────────────────────────────────────┐
│  Device: LaptopPC                                         │
│  Total: 476 GB   Used: 142 GB   Free: 334 GB (70%)       │
│                                                           │
│  ██████████░░░░░░░░░░░░░░░░░░  30% used                  │
│                                                           │
│  AI Models         22.3 GB  █████░░░░░░  5%              │
│    llama-3.1-8b-q4   4.5 GB                              │
│    llama-3.1-13b-q4  7.4 GB                              │
│    phi-3-vision       3.2 GB                              │
│    llama-3.1-70b-q4   7.2 GB (partial, streaming)        │
│  User Data         68.4 GB  ██████████████░  14%         │
│  Version History   24.7 GB  █████░░░░░░  5%              │
│  Web Storage        8.2 GB  ██░░░░░░░░░  2%              │
│  Indexes + Audit    5.1 GB  █░░░░░░░░░░  1%              │
│  System             3.4 GB  █░░░░░░░░░░  1%              │
│  Other (non-AIOS)   9.9 GB  ██░░░░░░░░░  2%              │
│                                                           │
│  Biggest spaces:                                          │
│    user/media/      31.2 GB  (photos + video, 8,400 obj) │
│    user/code/       18.6 GB  (repos, 12,300 objects)     │
│    user/documents/   9.4 GB  (docs, 1,200 objects)       │
│    web-storage/      8.2 GB  (24 origins)                │
│                                                           │
│  Version history savings:                                 │
│    Deduplication saved: 41.2 GB (63% of version data)    │
│    Compression saved:   11.8 GB (across all tiers)       │
│                                                           │
│  Storage pressure: Normal (70% free)                      │
└───────────────────────────────────────────────────────────┘
```

-----

## 11. Design Principles

1. **Find by meaning, not by path.** Semantic search, relationship traversal, and entity queries replace directory navigation.
2. **Never lose data silently.** Version history, content-addressing, and WAL ensure no data loss from crashes, bugs, or user mistakes. Under storage pressure, version retention is reduced transparently — the user is always informed.
3. **Encryption is structural.** Device-level encryption (§4.10) ensures nothing is stored as plaintext on the physical medium — starting with Phase 4b, the system is encrypted at rest. Per-space encryption (§6) adds cross-zone isolation within the running system. Screen lock or logout zeroizes per-space keys; shutdown or device removal zeroizes the device key. No data survives physical access.
4. **Deduplication is deep.** Content-addressing deduplicates identical blocks. Sub-block deduplication (Rabin rolling hash) deduplicates shared regions within near-duplicate content — capturing 60-80% savings from typical document edits.
5. **Indexes are always current.** Full-text index updates synchronously. Embedding index updates asynchronously but as fast as compute allows.
6. **POSIX is a view.** The filesystem is a compatibility layer over spaces, not the other way around. Spaces are the truth; paths are a translation.
7. **Spaces belong to users.** Agents access spaces via capabilities. Removing an agent never removes user data.
8. **Storage-aware by default.** CompactObjects minimize metadata overhead. Adaptive block compression (entropy-based selection) extends capacity. Flash-aware writes (LSM-tree, zone separation, append-preferred allocation) minimize device wear. Write amplification is tracked and bounded. Adaptive retention responds to storage pressure. AI models are reproducible and evictable — user data is not. Device profiles adapt the system from laptop SSDs (256 GB - 2 TB, initial target) to future constrained devices (phones, TVs, SBCs).
9. **Reproducible data yields first.** Under storage pressure, reproducible data (model files, embeddings, web caches) is reclaimed before user data. Downloaded models can be re-fetched. Embeddings can be regenerated. Version history is compressed. User files are never touched without explicit user action.

-----

## 12. Implementation Order

Phase numbering follows the AIOS-wide phase plan ([development-plan.md](../project/development-plan.md)). Phases 1-3 cover kernel initialization and basic services. Phase 4 is the storage system (this document). Phases 5-8 cover other system layers (Phase 5: GPU & Display, Phase 6: Window Compositor & Shell, Phase 7: Input, Terminal & Basic Networking, Phase 8: AIRS Core — see development-plan.md for details). "Single-device operation" refers to the Phase 4a-4l sub-phases below. Multi-device features begin in Phase 9c.

```
Phase 4a:  Block engine + WAL + LSM-tree index      → raw persistent storage with flash-friendly index
Phase 4b:  Device-level transparent encryption (§4.10) → every block encrypted before hitting disk
           + device key derivation (passphrase mode) → no plaintext on the physical medium from day one
Phase 4c:  Object store + content addressing        → objects with whole-block deduplication
Phase 4d:  Space API + basic queries (Filter)       → spaces usable by services
Phase 4e:  Version store + Merkle DAG               → full version history
Phase 4f:  POSIX bridge + path mapping              → BSD tools work
Phase 4g:  CompactObject + promotion policy           → storage-efficient default objects
Phase 4h:  Block-level compression (LZ4/zstd)         → 2-4x storage savings
           + adaptive entropy-based selection          → skip incompressible content
Phase 4i:  Flash-aware zone allocation (hot/warm/cold) → write-time zone placement, reduced WAF
Phase 4j:  Storage budget + quotas + pressure levels  → bounded storage per category
Phase 4k:  Adaptive version retention                 → pressure-responsive history pruning
Phase 4l:  Write amplification tracking (§4.8)        → continuous WAF monitoring + alerts
Phase 9a:  Full-text index + text search              → keyword search
Phase 9b:  Embedding index + selective embedding      → semantic search (promoted objects only)
Phase 9c:  Space Sync protocol                        → cross-device sync
Phase 13a: Per-space encryption layer + key management → encrypted Personal/Collaborative/Untrusted zones
Phase 14a: Tiered storage (hot/warm/cold)             → background TierManager daemon + automatic tier migration + recompression
Phase 14b: Audit retention + chain compaction         → bounded audit storage growth
Phase 14c: Model disk eviction + streaming download   → reclaim model storage under pressure
Phase 14d: Storage monitoring dashboard (Inspector)   → user-visible storage analytics
Phase 14e: Sub-block deduplication (§4.9)             → Rabin rolling hash for near-duplicate savings
Phase 24a: Secure Boot integration + hardware key binding → TPM/TrustZone-sealed device keys
           + session persistence hardening            → hardware-bound device key auto-unlock
```
