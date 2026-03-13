# AIOS Space Storage — Core Data Structures

Part of: [spaces.md](./spaces.md) — Space Storage System
**Related:** [spaces-block-engine.md](./spaces-block-engine.md) — Block Engine, [spaces-versioning.md](./spaces-versioning.md) — Version Store, [spaces-encryption.md](./spaces-encryption.md) — Encryption

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
pub struct ObjectId(pub [u8; 16]);        // UUID v4

/// Unique identifier for a space. 128-bit UUID (v4).
pub struct SpaceId(pub [u8; 16]);

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
pub type BlockId = ContentHash;

/// Content-addressed identifier for blocks, executable code, manifests, and
/// dependencies. Primary hash newtype for content-addressable lookups.
pub struct ContentHash(pub [u8; 32]);

/// Unique identifier for a capability token in the kernel's capability table.
pub type CapabilityTokenId = u64;

/// Signature type (Ed25519, 64 bytes).
pub type Signature = [u8; 64];

/// Reference to an object within a space. Used by Flow (flow.md §3.1),
/// architecture.md §2.3, and boot-lifecycle.md §15 for cross-space
/// object references without copying the object itself.
pub struct ObjectRef {
    pub space_id: SpaceId,
    pub object_id: ObjectId,
    /// Optional version hash — when set, pins this reference to a
    /// specific version. When None, the reference tracks the latest.
    pub version: Option<Hash>,
}

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

**Implementation note:** In the codebase (`shared/src/storage.rs`), `ContentHash` is the primary hash newtype. `Hash` in this document corresponds to `ContentHash` in code. `BlockId` is a type alias for `ContentHash`.

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

```text
system/                      ← Core zone, kernel-managed
  devices/                   ← Device registry (subsystem framework)
  audit/                     ← Audit logs (per-subsystem)
    network/
    audio/
    camera/
    input/
    flow/                    ← Flow transfer audit trail (flow.md §11.2)
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
  flow/                      ← Flow transfer history and provenance (flow.md §3.1)
    history/                 ← Completed FlowEntry objects (content-addressed)
    index/                   ← Full-text index of entry metadata
    transforms/              ← Transform registry (persistent transforms)
    config/                  ← Retention policy, user preferences

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

/// Append-only Merkle-linked provenance chain for an object. Summarizes
/// the full per-version ProvenanceEntry records (§5.1) for quick inspection
/// without walking the entire version DAG.
/// Canonical definition: architecture.md §2.3.
pub struct ProvenanceChain {
    /// Hash of the most recent ProvenanceEntry in the chain
    head: Hash,
    /// Total number of entries (versions) in the chain
    length: u64,
    /// Who originally created this object
    origin: ProvenanceOrigin,
}

pub enum ProvenanceOrigin {
    UserCreated { agent: AgentId },
    AiGenerated { model: ModelId },
    Imported { source: String },
    DerivedFrom { source: ObjectId },
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
```text
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
