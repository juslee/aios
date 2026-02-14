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
│  Full-text index (inverted, BM25) — always available         │
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
│  Key escrow for recovery (optional, user-controlled)         │
│  Transparent encrypt/decrypt on read/write                   │
├─────────────────────────────────────────────────────────────┤
│                    Block Engine                               │
│  B-tree indexed blocks on raw storage device                 │
│  Write-ahead log (WAL) for crash consistency                 │
│  Block-level checksums (CRC-32C)                             │
│  No intermediate filesystem — AIOS owns the device           │
├─────────────────────────────────────────────────────────────┤
│                    Storage Drivers                            │
│  VirtIO-Blk (QEMU)  │  NVMe  │  SD/eMMC  │  USB Storage    │
└─────────────────────────────────────────────────────────────┘
```

-----

## 3. Core Data Structures

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
}

pub enum EncryptionState {
    /// Not encrypted (system spaces, temporary data)
    Plaintext,
    /// Encrypted with space-specific key
    Encrypted {
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
```

### 3.3.1 Compact vs Full Objects

By default, every new object is created as a **CompactObject** — a lightweight representation with minimal metadata overhead. Compact objects are promoted to full `Object` representation when the system determines they would benefit from rich metadata.

This is the storage-conservative default. On constrained devices, the overhead of embeddings (~1.5 KB per object at 384 dimensions × f32), provenance chains, AI-generated summaries, and entity extraction adds up quickly across thousands of objects. Most objects (config files, small notes, web storage entries, game saves, temp files) never benefit from this metadata.

```rust
/// Lightweight object representation — the default for all new objects.
/// Supports text search and basic queries. No embedding, no AI metadata.
pub struct CompactObject {
    id: ObjectId,
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
    exempt_types: Vec<ContentType>,     // default: [Config, GameSave, CacheEntry]
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
4. Object upgraded to full Object in-place (same ObjectId, same content_hash)
```

**Storage savings:** A CompactObject uses ~200 bytes of metadata vs ~2-6 KB for a full Object (with embedding + provenance + AI metadata). For a space with 10,000 objects where 80% remain compact, this saves 14-46 MB of metadata overhead — significant on a device with a 32 GB SD card.

**CompactObjects are still searchable.** Full-text search works on compact objects (the text index is always maintained). Semantic search (embedding-based) only works on promoted full objects. This means the system is fully functional with compact defaults — semantic search coverage grows organically as users interact with their data.

**Web storage is always compact.** Objects in `web-storage/` spaces (cookies, localStorage, sessionStorage, IndexedDB entries, Cache API responses) are never promoted. They are high-volume, low-value for semantic search, and typically accessed by origin rather than by meaning. The `PromotionPolicy.exempt_types` list ensures these stay lightweight regardless of other triggers.

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

Relations are bidirectional in storage — creating `A → References → B` also indexes `B ← ReferencedBy → A`. The relationship graph is stored as adjacency lists with both forward and reverse edges.

-----

## 4. Block Engine

### 4.1 On-Disk Layout

The Block Engine manages raw storage directly — no ext4, no ZFS, no intermediate filesystem. AIOS owns the partition.

```
┌──────────────────────────────────────────────────────────┐
│  Superblock (4 KB)                                        │
│  Magic, version, block size, total blocks, free blocks,   │
│  root B-tree offset, WAL offset, checksum                 │
├──────────────────────────────────────────────────────────┤
│  Write-Ahead Log (configurable, default 64 MB)            │
│  Circular buffer of pending writes                        │
│  Each entry: block_id, old_data, new_data, checksum       │
├──────────────────────────────────────────────────────────┤
│  Block Index (B-tree)                                     │
│  Maps: content_hash → (block_offset, block_size, refcount)│
│  Also maps: ObjectId → (metadata_block, content_hash)     │
├──────────────────────────────────────────────────────────┤
│  Data Blocks (remainder of partition)                      │
│  Content-addressed blocks, variable size                  │
│  Each block: header (hash, size, checksum) + data          │
└──────────────────────────────────────────────────────────┘
```

### 4.2 Write Path

```
Agent writes object:
  1. Content hashed (SHA-256) → content_hash
  2. Check block index: does content_hash already exist?
     YES → increment refcount, skip write (deduplication)
     NO  → continue to step 3
  3. WAL entry written: (new_block_id, content, metadata)
  4. WAL entry fsynced to disk (crash-safe point)
  5. Data block written to free space
  6. Block index updated: content_hash → block location
  7. Object metadata updated: ObjectId → content_hash
  8. Version store appended: (ObjectId, content_hash, timestamp, agent_id)
  9. WAL entry marked committed
```

### 4.3 Read Path

```
Agent reads object:
  1. Object metadata lookup: ObjectId → content_hash
  2. Block index lookup: content_hash → block location
  3. Read block from disk
  4. Verify checksum (CRC-32C)
  5. If encrypted space: decrypt with space key
  6. Return content to agent
```

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
    /// Blocks with refcount 0
    pending: Vec<BlockId>,
    /// Grace period before reclaiming (allows version history to reference old blocks)
    grace_period: Duration,
    /// Run GC when free space drops below threshold
    trigger_threshold: f64,         // fraction of total space
}
```

GC runs in the background and never blocks reads or writes.

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
    checksum: u32,                      // CRC-32C of compressed data
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

    fn select_compression(&self, data: &[u8], tier: StorageTier) -> CompressionStrategy {
        match tier {
            StorageTier::Hot => CompressionStrategy::Lz4,
            StorageTier::Warm => CompressionStrategy::Zstd { level: 3 },
            StorageTier::Cold => CompressionStrategy::Zstd { level: 9 },
        }
    }
}
```

**Why block-level:** Content-addressed blocks are immutable after write — ideal for compression. The decompression cost is paid once on read and amortized across multiple accesses by the page cache. On a Pi 5, LZ4 decompresses at ~4 GB/s (faster than SD card read speed), so compression is effectively free on the read path.

**Incompressible content:** Encrypted blocks and already-compressed media (JPEG, MP4, FLAC) don't benefit from compression. The `select_compression` heuristic samples the first 4 KB — if the sample compresses poorly (<5% savings), the block is stored uncompressed to avoid wasting CPU.

**Security: compress before encrypt.** The Block Engine compresses data before the Encryption Layer encrypts it (see architecture diagram in section 2). This ordering is critical — compressing ciphertext is useless (encrypted data is indistinguishable from random), and encrypting compressed data avoids CRIME/BREACH-style attacks where compression ratio changes leak information about plaintext. Since AIOS uses content-addressed blocks (each block has a unique content_hash), an attacker cannot perform the chosen-plaintext injection required for CRIME-style attacks. The compress-then-encrypt ordering is safe.

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

**Tier transitions:** A background thread scans block access timestamps. When a Hot block hasn't been accessed within `warm_window`, it is recompressed with zstd and demoted to Warm. When a Warm block hasn't been accessed within `warm_window`, it is recompressed at a higher zstd level and demoted to Cold. Promotion (Cold → Hot) happens automatically on access — the block is decompressed and rewritten with LZ4.

**Recompression is lazy.** The recompressor runs at lowest I/O priority and yields to any foreground read or write. On a Pi with an SD card, recompression is throttled to avoid wearing the card. Tier transitions are batched — the recompressor processes blocks in groups during idle periods.

**Multi-device tiering (future):** On systems with both NVMe and SD storage, Hot data lives on NVMe and Cold data on SD. The tier manager handles migration transparently. This is a Phase 14 optimization — single-device tiering via compression is the Phase 4 implementation.

-----

## 5. Version Store

### 5.1 Merkle DAG

Every object modification creates a new version node in a Merkle DAG (like git):

```rust
pub struct Version {
    hash: Hash,                         // SHA-256 of (parent_hash + content_hash + metadata)
    parent: Option<Hash>,               // previous version
    content_hash: Hash,                 // content at this version
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

**Blast radius containment (Security Layer 8):** Before any bulk operation (agent writing >10 objects, bulk delete, space import), the system automatically creates a snapshot. If the operation goes wrong, the user can roll back to the pre-operation state.

-----

## 6. Encryption

### 6.1 Key Management

```rust
pub struct SpaceKeyManager {
    /// Master key derived from user's identity
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

**Key rotation:** Space keys can be rotated without re-encrypting all data. New writes use the new key. Old data is re-encrypted in the background. The rotation is atomic — at no point is data unencrypted on disk.

### 6.2 Encryption Zones

| Zone | Encrypted | Key Source |
|---|---|---|
| Core (system/) | No | System data, not user-sensitive |
| Personal (user/) | Yes | User identity master key |
| Collaborative (shared/) | Yes | Shared key (distributed via capability exchange) |
| Untrusted (web-storage/) | Yes | Per-origin key derived from master key |
| Ephemeral (/tmp) | No | Temporary, auto-deleted |

-----

## 7. Query Engine

### 7.1 Query Dispatch

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
    /// Inverted index: term → Vec<(ObjectId, positions)>
    index: BTreeMap<String, PostingList>,
    /// Document frequency for BM25 scoring
    doc_count: u64,
    term_frequencies: HashMap<String, u64>,
}
```

Updated synchronously on every write. When an object is created or modified, its text content is extracted and tokenized, and the inverted index is updated. This ensures search always returns current results.

### 7.3 Embedding Index

Maintained by AIRS Space Indexer. Available when AIRS is running:

```rust
pub struct EmbeddingIndex {
    /// HNSW graph for approximate nearest-neighbor search
    hnsw: HnswGraph,
    /// Dimension of embedding vectors
    dimensions: usize,                  // typically 384
    /// Map from embedding position to ObjectId
    id_map: Vec<ObjectId>,
}
```

Updated asynchronously by the Space Indexer. New objects are queued for embedding generation. The index may lag slightly behind the latest writes, but full-text search is always current.

### 7.4 Relationship Graph

```rust
pub struct RelationshipGraph {
    /// Forward edges: source → Vec<(target, kind, confidence)>
    forward: HashMap<ObjectId, Vec<Edge>>,
    /// Reverse edges: target → Vec<(source, kind, confidence)>
    reverse: HashMap<ObjectId, Vec<Edge>>,
}
```

Traverse queries walk this graph with configurable depth and direction. Used for provenance chains ("where did this data come from?"), dependency graphs ("what depends on this?"), and similarity exploration ("show me related objects").

-----

## 8. Space Sync Protocol

Spaces can synchronize across devices. This is how collaborative spaces work and how user data replicates across AIOS devices.

```rust
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
    resolution: SyncConflictPolicy,     // from networking.md
}
```

**Sync uses the Network Translation Module.** Remote spaces are accessed via space operations (`space::remote("device-b/shared/project")`). The NTM handles the transport. The Space Sync protocol exchanges Merkle roots to efficiently determine what's changed, then syncs only the deltas.

-----

## 9. POSIX Compatibility

### 9.1 Path Mapping

The POSIX emulation layer maps filesystem paths to space operations:

```
/spaces/[space-name]/[object-path]  →  space query + object access
/home/user/                          →  user/home/ space
/tmp/                                →  ephemeral space (auto-cleaned)
/dev/null, /dev/urandom             →  device capabilities
/proc/self/                          →  process introspection
/bin/, /usr/bin/                     →  system utilities space
```

### 9.2 Translation Layer

```rust
pub struct PosixSpaceBridge {
    mount_table: Vec<MountEntry>,
}

pub struct MountEntry {
    posix_path: String,                 // "/spaces/research"
    space: SpaceId,
    capabilities: CapabilitySet,        // from calling process's agent
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
        let object = space.resolve_object(object_path)?;
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

-----

## 10. Storage Budget and Pressure Management

### 10.1 Storage Budget by Device

AIOS's storage overhead is fundamentally different from traditional operating systems. Phones and desktops spend 50-70% of storage on apps and games. AIOS replaces that with AI models and gains new overhead from version history, semantic indexes, and audit logs. Agents are lightweight (manifests + code, typically <10 MB each), but models are massive.

```
Storage budget by device (estimated, after OS and partition overhead):

                        16 GB SD     32 GB SD     64 GB SD     128 GB+ SSD
                        ────────     ────────     ────────     ───────────
Usable after format:    ~14.5 GB     ~29.5 GB     ~59.5 GB     ~119 GB

AI models:               2 GB         4.5 GB       8 GB         15 GB
  (1 small model)       (1 × 8B Q4)  (1 × 8B +    (2-3 models)
                                      1 vision)

OS + system spaces:      1.5 GB       2 GB         2.5 GB       3 GB
  (kernel, agents,
   credentials, config)

Indexes + audit:         0.5 GB       1 GB         2 GB         4 GB
  (FTI, HNSW, audit
   Merkle chain)

Version history:         1 GB         2-5 GB       5-10 GB      10-20 GB
  (depends on editing
   frequency)

User data:               3-5 GB       8-12 GB      20-30 GB     50-70 GB
  (documents, media,
   conversations)

Web storage:             0.5-1 GB     1-3 GB       2-5 GB       5-10 GB
  (per-origin storage,
   browser cache)

Free headroom:           3-6 GB       5-12 GB      10-22 GB     17-57 GB
  (target: ≥15% free)
```

**16 GB is not recommended.** A single 8B Q4 model (4.5 GB) consumes 31% of usable space. With OS, indexes, and minimal user data, free headroom drops below the 15% target. AIOS should warn at first boot: "16 GB storage detected. Storage will be constrained. 32 GB or larger is recommended."

**32 GB is the practical minimum** for the full AI-native experience. One model, moderate user data, version history, and reasonable headroom.

### 10.2 Storage Quotas by Category

Each storage category has a quota to prevent any single concern from consuming the device:

```rust
pub struct StorageBudget {
    total_usable: u64,
    quotas: StorageQuotas,
}

pub struct StorageQuotas {
    /// AI model storage — GGUF files on disk
    /// Default: 30% of usable space
    models: StorageQuota,

    /// System spaces (OS, agents, credentials, config)
    /// Default: 10% of usable space, minimum 1.5 GB
    system: StorageQuota,

    /// Indexes and audit (FTI, HNSW, Merkle chain)
    /// Default: 8% of usable space, minimum 500 MB
    indexes_audit: StorageQuota,

    /// Version history (Merkle DAG, old content blocks)
    /// Default: 15% of usable space
    versions: StorageQuota,

    /// User data (personal spaces — documents, media, conversations)
    /// Default: no hard limit — gets whatever is left
    user_data: StorageQuota,

    /// Web storage (per-origin: cookies, localStorage, IndexedDB, cache)
    /// Default: 10% of usable space, max 5 GB per origin
    web_storage: StorageQuota,

    /// Minimum free headroom — triggers pressure response when breached
    /// Default: 15% of usable space
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

### 10.3 Storage Pressure Response

Like memory pressure (see [memory.md §8](../kernel/memory.md)), storage has pressure levels with escalating responses:

```rust
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
```

```
Pressure response table:

Level       Free %    Actions
──────────  ──────    ──────────────────────────────────────────────────────
Normal      > 20%     Normal operation. GC runs on schedule.
                      Version retention per space quota.

Low         10-20%    - Tighten version retention: KeepLast(10) → KeepLast(5)
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

### 10.4 Model Storage Strategy

AI model files are the single largest storage consumer and unlike user data, they are **reproducible** — a deleted model can be re-downloaded. This makes them the best target for reclamation under storage pressure.

```rust
pub struct ModelStoragePolicy {
    /// Maximum disk space for all model files combined
    max_disk: u64,                      // from StorageQuotas.models
    /// Models currently on disk
    on_disk: Vec<ModelDiskEntry>,
    /// Keep only the active model + companion on constrained devices
    aggressive_eviction: bool,          // true when storage < 32 GB
}

pub struct ModelDiskEntry {
    model_id: ModelId,
    file_size: u64,
    last_loaded: Timestamp,
    source: ModelSource,                // Bundled, Downloaded, UserProvided
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
            .filter(|m| m.reproducible && !self.is_primary(m.model_id))
            .sorted_by(|a, b| b.file_size.cmp(&a.file_size))
            .map(|m| m.model_id)
            .collect()
    }
}
```

**On 32 GB devices:** Only one model is kept on disk at a time. When the user switches models, the old model file is deleted after the new one finishes downloading. Two 4.5 GB model files simultaneously would consume 30% of a 32 GB card.

**On 64 GB+ devices:** Multiple models can be cached on disk. LRU eviction removes the least recently used model file when the model storage quota is exceeded.

**Streaming model download:** Instead of downloading the entire GGUF file before starting inference, AIOS can stream model weights via mmap over a network-backed file. The NTM fetches blocks on demand as page faults occur. This eliminates the need to store the full model file on disk at the cost of inference speed (network latency per page fault). Useful as a fallback when storage is critically low but the network is available.

### 10.5 Version History Budget

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

### 10.6 Storage Monitoring

The Inspector exposes real-time storage analytics:

```
Storage Dashboard:
┌──────────────────────────────────────────────┐
│  Total: 29.5 GB   Used: 18.2 GB   Free: 11.3 GB (38%)  │
│                                                           │
│  ██████████████████░░░░░░░░░░  62% used                  │
│                                                           │
│  AI Models          4.5 GB  ████████░░░  15%             │
│  User Data          6.2 GB  ████████████░  21%           │
│  Version History    3.1 GB  ██████░░░░░░  11%            │
│  Web Storage        1.8 GB  ████░░░░░░░  6%              │
│  Indexes + Audit    1.1 GB  ██░░░░░░░░░  4%              │
│  System             1.5 GB  ███░░░░░░░░  5%              │
│                                                           │
│  Biggest spaces:                                          │
│    user/media/       3.1 GB  (photos, 2,400 objects)     │
│    user/documents/   1.8 GB  (docs, 340 objects)         │
│    web-storage/      1.8 GB  (12 origins)                │
│                                                           │
│  Version history savings:                                 │
│    Deduplication saved: 8.4 GB (73% of version data)     │
│    Compression saved:   2.1 GB (across all tiers)        │
└──────────────────────────────────────────────┘
```

-----

## 11. Design Principles

1. **Find by meaning, not by path.** Semantic search, relationship traversal, and entity queries replace directory navigation.
2. **Never lose data silently.** Version history, content-addressing, and WAL ensure no data loss from crashes, bugs, or user mistakes. Under storage pressure, version retention is reduced transparently — the user is always informed.
3. **Encryption is structural.** Per-space encryption is a property of the space, not an afterthought. Identity change = spaces lock automatically.
4. **Deduplication is free.** Content-addressing means identical content is stored once, regardless of how many objects reference it.
5. **Indexes are always current.** Full-text index updates synchronously. Embedding index updates asynchronously but as fast as compute allows.
6. **POSIX is a view.** The filesystem is a compatibility layer over spaces, not the other way around. Spaces are the truth; paths are a translation.
7. **Spaces belong to users.** Agents access spaces via capabilities. Removing an agent never removes user data.
8. **Storage-aware by default.** CompactObjects minimize metadata overhead. Block compression extends capacity. Adaptive retention responds to storage pressure. AI models are reproducible and evictable — user data is not. The system works on a 32 GB SD card and scales to 128 GB+ SSDs.
9. **Reproducible data yields first.** Under storage pressure, reproducible data (model files, embeddings, web caches) is reclaimed before user data. Downloaded models can be re-fetched. Embeddings can be regenerated. Version history is compressed. User files are never touched without explicit user action.

-----

## 11. Implementation Order

```
Phase 4a:  Block engine + WAL                      → raw persistent storage
Phase 4b:  Object store + content addressing        → objects with deduplication
Phase 4c:  Space API + basic queries (Filter)       → spaces usable by services
Phase 4d:  Version store + Merkle DAG               → full version history
Phase 4e:  POSIX bridge + path mapping              → BSD tools work
Phase 4f:  CompactObject + promotion policy           → storage-efficient default objects
Phase 4g:  Block-level compression (LZ4/zstd)         → 2-4x storage savings
Phase 4h:  Storage budget + quotas + pressure levels  → bounded storage per category
Phase 4i:  Adaptive version retention                 → pressure-responsive history pruning
Phase 9a:  Full-text index + text search              → keyword search
Phase 9b:  Embedding index + selective embedding      → semantic search (promoted objects only)
Phase 9c:  Space Sync protocol                        → cross-device sync
Phase 13a: Encryption layer + key management          → encrypted spaces
Phase 14a: Tiered storage (hot/warm/cold)             → automatic tier migration + recompression
Phase 14b: Audit retention + chain compaction         → bounded audit storage growth
Phase 14c: Model disk eviction + streaming download   → reclaim model storage under pressure
Phase 14d: Storage monitoring dashboard (Inspector)   → user-visible storage analytics
Phase 24a: Secure Boot integration + key escrow       → TrustZone key storage
```
