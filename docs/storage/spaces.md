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

## 10. Design Principles

1. **Find by meaning, not by path.** Semantic search, relationship traversal, and entity queries replace directory navigation.
2. **Never lose data.** Version history, content-addressing, and WAL ensure no data loss from crashes, bugs, or user mistakes.
3. **Encryption is structural.** Per-space encryption is a property of the space, not an afterthought. Identity change = spaces lock automatically.
4. **Deduplication is free.** Content-addressing means identical content is stored once, regardless of how many objects reference it.
5. **Indexes are always current.** Full-text index updates synchronously. Embedding index updates asynchronously but as fast as compute allows.
6. **POSIX is a view.** The filesystem is a compatibility layer over spaces, not the other way around. Spaces are the truth; paths are a translation.
7. **Spaces belong to users.** Agents access spaces via capabilities. Removing an agent never removes user data.

-----

## 11. Implementation Order

```
Phase 4a:  Block engine + WAL                      → raw persistent storage
Phase 4b:  Object store + content addressing        → objects with deduplication
Phase 4c:  Space API + basic queries (Filter)       → spaces usable by services
Phase 4d:  Version store + Merkle DAG               → full version history
Phase 4e:  POSIX bridge + path mapping              → BSD tools work
Phase 9a:  Full-text index + text search            → keyword search
Phase 9b:  Embedding index + semantic search        → semantic search (requires AIRS)
Phase 9c:  Space Sync protocol                      → cross-device sync
Phase 13a: Encryption layer + key management        → encrypted spaces
Phase 24a: Secure Boot integration + key escrow     → TrustZone key storage
```
