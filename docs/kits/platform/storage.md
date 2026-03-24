# Storage Kit

**Layer:** Platform | **Crate:** `aios_storage` | **Architecture:** [`docs/storage/spaces.md`](../../storage/spaces.md)

## 1. Overview

Storage Kit is the primary data persistence layer in AIOS. All application data lives in
named Spaces composed of typed Objects connected by Relations. Unlike traditional
filesystems that expose blocks and inodes, Storage Kit provides a high-level object store
with built-in versioning, full-text and semantic search, encryption, and multi-device sync.
The underlying Block Engine handles durable on-disk layout through an LSM-tree with
write-ahead logging and AES-256-GCM encryption at rest.

Every agent gets its own Space by default, and the system creates three built-in Spaces at
boot: `system/` (core OS data), `user/home/` (personal files), and `ephemeral/` (temporary
data that does not survive reboot). Agents can create additional Spaces for structured data,
share Spaces with other agents through capability delegation, and subscribe to reactive
queries that push updates when underlying data changes. This reactive query model -- inspired
by BeOS's live queries -- means agents never need to poll for changes; they declare interest
in a query predicate and receive notifications as matching objects are created, modified, or
deleted.

Use Storage Kit when your agent needs to persist structured data, store files, maintain
version history, or query across objects. Do not use it for transient in-memory state (use
IPC Kit shared memory instead) or for streaming data exchange (use [Flow Kit](../platform/flow.md)
instead).

## 2. Core Traits

```rust
use aios_capability::CapabilityHandle;
use aios_storage::query::{Query, QueryResult, ReactiveHandle};

/// A named container for objects with quota enforcement and security zones.
///
/// Spaces are the top-level organizational unit. Each Space has a unique name,
/// a security zone governing encryption policy, and a quota limiting storage
/// consumption. Agents interact with objects through their containing Space.
pub trait Space {
    /// The Space's unique identifier.
    fn id(&self) -> SpaceId;

    /// Human-readable name (e.g., "user/home/", "com.example.notes").
    fn name(&self) -> &str;

    /// The security zone governing encryption and access policy.
    fn security_zone(&self) -> SecurityZone;

    /// Current storage usage in bytes.
    fn usage(&self) -> u64;

    /// Configured quota for this Space.
    fn quota(&self) -> &SpaceQuota;

    /// Create a new object in this Space.
    fn create_object(&mut self, request: CreateObjectRequest) -> Result<Object, StorageError>;

    /// Look up an object by its unique identifier.
    fn get_object(&self, id: &ObjectId) -> Result<Object, StorageError>;

    /// Delete an object and all its versions.
    fn delete_object(&mut self, id: &ObjectId) -> Result<(), StorageError>;

    /// List all objects matching optional filters.
    fn list_objects(&self, filter: Option<&ObjectFilter>) -> Result<Vec<Object>, StorageError>;

    /// Execute a query against objects in this Space.
    fn query(&self, query: &Query) -> Result<QueryResult, StorageError>;

    /// Subscribe to a reactive query that pushes updates on changes.
    ///
    /// The returned handle receives notifications whenever objects matching
    /// the query predicate are created, modified, or deleted. This is AIOS's
    /// equivalent of BeOS live queries -- declare interest once, receive
    /// updates continuously without polling.
    fn watch(&self, query: &Query) -> Result<ReactiveHandle, StorageError>;
}

/// A typed content unit with content-addressed storage and provenance.
///
/// Objects are the fundamental data unit. Each Object belongs to exactly one
/// Space, has a content type, and is stored with a SHA-256 content hash for
/// integrity. Objects are versioned automatically -- every mutation creates a
/// new version in the Merkle DAG.
pub trait Object {
    /// The Object's unique identifier.
    fn id(&self) -> &ObjectId;

    /// The Space containing this Object.
    fn space_id(&self) -> &SpaceId;

    /// The Object's display name.
    fn name(&self) -> &str;

    /// The content type (e.g., text/plain, application/json, image/png).
    fn content_type(&self) -> ContentType;

    /// Read the Object's content as bytes.
    fn read(&self) -> Result<Vec<u8>, StorageError>;

    /// Write new content, creating a new version.
    fn write(&mut self, data: &[u8], message: &str) -> Result<VersionId, StorageError>;

    /// The SHA-256 content hash of the current version.
    fn content_hash(&self) -> &ContentHash;

    /// Timestamp of last modification.
    fn modified_at(&self) -> Timestamp;

    /// Access the version history for this Object.
    fn versions(&self) -> Result<VersionStore, StorageError>;

    /// Searchable text content extracted from the Object.
    fn text_content(&self) -> Option<&str>;
}

/// A directed edge between two Objects enabling graph traversal.
///
/// Relations connect Objects within or across Spaces. They have a typed label
/// (e.g., "references", "parent_of", "tagged_with") and optional metadata.
pub trait Relation {
    /// The source Object.
    fn source(&self) -> &ObjectId;

    /// The target Object.
    fn target(&self) -> &ObjectId;

    /// The relation type label.
    fn label(&self) -> &str;

    /// Optional metadata attached to the relation.
    fn metadata(&self) -> Option<&[u8]>;

    /// When the relation was created.
    fn created_at(&self) -> Timestamp;
}

/// Merkle DAG version history for an Object.
///
/// Every Object mutation creates a new version. Versions form a DAG where
/// each version points to its parent(s). The VersionStore supports snapshot,
/// rollback, and branch operations.
pub trait VersionStore {
    /// List all versions of an Object, newest first.
    fn list(&self, object_id: &ObjectId) -> Result<Vec<Version>, StorageError>;

    /// Get the current (head) version.
    fn head(&self, object_id: &ObjectId) -> Result<Version, StorageError>;

    /// Roll back to a specific version, creating a new version that
    /// restores the content of the target version.
    fn rollback(&mut self, object_id: &ObjectId, version_id: &VersionId)
        -> Result<Version, StorageError>;

    /// Create a named snapshot of the current state.
    fn snapshot(&mut self, name: &str) -> Result<SnapshotId, StorageError>;

    /// Create a branch from a specific version for parallel editing.
    fn branch(&mut self, version_id: &VersionId, name: &str) -> Result<BranchId, StorageError>;
}

/// Full-text, semantic, and hybrid query dispatch.
///
/// QueryEngine dispatches queries to the appropriate index: BM25 for
/// full-text, HNSW for semantic (embedding-based), or a fusion of both
/// using reciprocal rank fusion (RRF).
pub trait QueryEngine {
    /// Full-text search using BM25 scoring.
    fn search_text(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, StorageError>;

    /// Semantic search using embedding similarity (requires AIRS).
    fn search_semantic(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, StorageError>;

    /// Hybrid search fusing full-text and semantic results via RRF.
    fn search_hybrid(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, StorageError>;

    /// Graph traversal query following Relations.
    fn traverse(&self, start: &ObjectId, label: &str, depth: u32)
        -> Result<Vec<Object>, StorageError>;
}

/// Merkle-exchange based multi-device sync engine.
pub trait SpaceSync {
    /// Initiate sync with a remote peer for a given Space.
    fn sync(&mut self, space_id: &SpaceId, peer: &PeerId) -> Result<SyncResult, StorageError>;

    /// Check the sync status for a Space.
    fn status(&self, space_id: &SpaceId) -> SyncStatus;

    /// Resolve a conflict using the specified strategy.
    fn resolve_conflict(&mut self, conflict: &Conflict, strategy: ConflictStrategy)
        -> Result<(), StorageError>;

    /// List pending conflicts that require manual resolution.
    fn pending_conflicts(&self, space_id: &SpaceId) -> Result<Vec<Conflict>, StorageError>;
}

/// Reactive query handle for live-query subscriptions.
///
/// Inspired by BeOS live queries. Once a watch is registered, the handle
/// delivers change notifications without polling.
pub struct ReactiveHandle {
    query: Query,
    space_id: SpaceId,
}

impl ReactiveHandle {
    /// Receive the next batch of changes (blocks until changes arrive or timeout).
    pub fn recv(&self, timeout: Duration) -> Result<Vec<ChangeEvent>, StorageError> {
        // Provided by runtime
        unimplemented!()
    }

    /// Check for changes without blocking.
    pub fn try_recv(&self) -> Result<Vec<ChangeEvent>, StorageError> {
        unimplemented!()
    }

    /// Stop receiving updates and release resources.
    pub fn unsubscribe(self) -> Result<(), StorageError> {
        unimplemented!()
    }
}

/// A change event delivered to reactive query subscribers.
pub enum ChangeEvent {
    /// A new object matches the query predicate.
    Created(ObjectId),
    /// An existing matching object was modified.
    Modified(ObjectId),
    /// A previously matching object was deleted or no longer matches.
    Removed(ObjectId),
}
```

## 3. Usage Patterns

**Minimal -- create a Space and store an object:**

```rust
use aios_storage::{StorageKit, CreateObjectRequest, ContentType};

// Open or create a Space for your agent's data
let mut space = StorageKit::open_space("com.example.notes")?;

// Create a new object
let note = space.create_object(CreateObjectRequest {
    name: "meeting-notes-2026-03-23".into(),
    content_type: ContentType::Text,
    data: b"Discussed Q2 roadmap and priorities.".to_vec(),
    tags: vec!["meeting".into(), "q2".into()],
})?;

println!("Created note: {} (hash: {:?})", note.name(), note.content_hash());
```

**Realistic -- reactive queries for live updates (BeOS-style):**

```rust
use aios_storage::{StorageKit, Query};
use std::time::Duration;

let space = StorageKit::open_space("com.example.notes")?;

// Register a live query that watches for all notes tagged "urgent"
let query = Query::full_text("tag:urgent");
let watcher = space.watch(&query)?;

// In your event loop, receive updates as they happen -- no polling
loop {
    match watcher.recv(Duration::from_secs(30)) {
        Ok(changes) => {
            for change in changes {
                match change {
                    ChangeEvent::Created(id) => {
                        let obj = space.get_object(&id)?;
                        show_notification(&obj);
                    }
                    ChangeEvent::Modified(id) => refresh_view(&id),
                    ChangeEvent::Removed(id) => remove_from_view(&id),
                }
            }
        }
        Err(StorageError::Timeout) => continue,
        Err(e) => return Err(e),
    }
}
```

**Advanced -- versioning, rollback, and graph traversal:**

```rust
use aios_storage::StorageKit;

let mut space = StorageKit::open_space("com.example.docs")?;
let mut doc = space.get_object(&doc_id)?;

// Every write creates a new version automatically
doc.write(b"Updated content with corrections.", "Fix typos in section 3")?;
doc.write(b"Final reviewed content.", "Incorporate reviewer feedback")?;

// List version history
let versions = doc.versions()?.list(&doc.id())?;
for v in &versions {
    println!("{}: {} by {}", v.id, v.message, v.author);
}

// Roll back to a previous version (creates a new version with old content)
doc.versions()?.rollback(&doc.id(), &versions[1].id)?;

// Create a relation between objects
space.create_relation(&doc.id(), &attachment.id(), "references")?;

// Traverse the relation graph
let query_engine = space.query_engine();
let referenced = query_engine.traverse(&doc.id(), "references", 2)?;
```

> **Common Mistakes**
>
> - **Forgetting to open a Space before accessing objects.** All objects live in Spaces.
>   Call `open_space()` first; accessing objects without a Space context returns
>   `StorageError::SpaceNotFound`.
> - **Polling instead of watching.** Use `Space::watch()` for reactive queries instead of
>   repeatedly calling `Space::query()` in a loop. Watches are kernel-optimized and consume
>   far fewer resources.
> - **Ignoring quota limits.** Each Space has a quota. Writes that exceed the quota return
>   `StorageError::QuotaExceeded`. Check `Space::usage()` proactively for large operations.
> - **Using Storage Kit for ephemeral IPC data.** For transient data exchange between agents,
>   use IPC Kit shared memory or Flow Kit. Storage Kit is optimized for persistence, not
>   message passing.

## 4. Integration Examples

**Storage Kit + Flow Kit -- clipboard with version history:**

```rust
use aios_storage::StorageKit;
use aios_flow::{FlowKit, FlowEntry, TypedContent};

// When the user copies content, store it in both Flow (for clipboard)
// and Storage (for persistent history)
fn on_copy(content: &[u8], content_type: ContentType) -> Result<(), Box<dyn Error>> {
    // Push to Flow for immediate clipboard availability
    FlowKit::push(FlowEntry {
        content: TypedContent::new(content, content_type.clone()),
        source: AgentId::current(),
        ..Default::default()
    })?;

    // Also persist to a clipboard-history Space for long-term access
    let mut space = StorageKit::open_space("system/clipboard-history")?;
    space.create_object(CreateObjectRequest {
        name: format!("clip-{}", Timestamp::now()),
        content_type,
        data: content.to_vec(),
        tags: vec!["clipboard".into()],
    })?;

    Ok(())
}
```

**Storage Kit + Capability Kit -- sharing a Space with another agent:**

```rust
use aios_storage::StorageKit;
use aios_capability::{CapabilityKit, Capability};

// Share a Space with another agent using attenuated capabilities
let space = StorageKit::open_space("com.example.shared-project")?;

// Grant read-only access to the collaborator agent
let read_cap = CapabilityKit::attenuate(
    &space_capability,
    Capability::SpaceAccess {
        space_id: space.id(),
        permissions: SpacePermissions::READ_ONLY,
    },
)?;

CapabilityKit::delegate(&read_cap, &collaborator_agent_id)?;

// The collaborator can now open the Space in read-only mode
// Any write attempt returns StorageError::CapabilityDenied
```

**Storage Kit + Search Kit -- indexing objects for semantic search:**

```rust
use aios_storage::StorageKit;
use aios_search::{SearchKit, IndexRequest};

let space = StorageKit::open_space("com.example.notes")?;

// Storage Kit automatically indexes objects via Space Indexer.
// For custom search experiences, use Search Kit directly:
let results = SearchKit::search_hybrid(
    "meeting notes about Q2 roadmap",
    SearchOptions {
        spaces: vec![space.id()],
        content_types: vec![ContentType::Text],
        limit: 10,
    },
)?;

for hit in results {
    let obj = space.get_object(&hit.object_id)?;
    println!("[{:.2}] {}: {}", hit.score, obj.name(), hit.snippet);
}
```

## 5. Capability Requirements

| Method | Required Capability | Notes |
| --- | --- | --- |
| `StorageKit::open_space` | `SpaceAccess` | Scoped to specific Space name or pattern |
| `Space::create_object` | `SpaceAccess(Write)` | Write permission on the target Space |
| `Space::get_object` | `SpaceAccess(Read)` | Read permission on the target Space |
| `Space::delete_object` | `SpaceAccess(Write)` | Write permission; deletion is permanent |
| `Space::query` | `SpaceAccess(Read)` | Read permission; queries scoped to accessible Spaces |
| `Space::watch` | `SpaceAccess(Read)` | Reactive queries require persistent read access |
| `StorageKit::create_space` | `SpaceCreate` | Restricted; most agents use their default Space |
| `SpaceSync::sync` | `SpaceSync` | Required for multi-device sync operations |
| `QueryEngine::search_semantic` | `SpaceAccess(Read)` + `InferenceAccess` | Semantic search requires AIRS inference |
| `VersionStore::rollback` | `SpaceAccess(Write)` | Creates a new version with restored content |

```toml
# Agent manifest example
[capabilities.required]
SpaceAccess = { spaces = ["com.example.notes"], permissions = "read_write" }

[capabilities.optional]
SpaceCreate = { reason = "Create project-specific Spaces for user data" }
SpaceSync = { reason = "Sync notes across user devices" }
InferenceAccess = { reason = "Semantic search over notes content" }
```

## 6. Error Handling

```rust
/// Errors returned by Storage Kit operations.
#[derive(Debug)]
pub enum StorageError {
    /// The requested Space does not exist.
    SpaceNotFound(SpaceId),

    /// The requested Object does not exist in the Space.
    ObjectNotFound(ObjectId),

    /// The Space's storage quota has been exceeded.
    QuotaExceeded { space_id: SpaceId, usage: u64, quota: u64 },

    /// The required capability was not granted or has been revoked.
    CapabilityDenied(Capability),

    /// A version conflict occurred during concurrent writes.
    VersionConflict { object_id: ObjectId, expected: VersionId, actual: VersionId },

    /// The block engine encountered a CRC integrity failure on read.
    IntegrityError { block_id: BlockId, expected_crc: u32, actual_crc: u32 },

    /// Encryption or decryption failed (wrong key, corrupted ciphertext).
    CryptoError(String),

    /// The WAL is full and cannot accept new entries until trimmed.
    WalFull,

    /// Multi-device sync failed for the specified peer.
    SyncFailed { peer: PeerId, reason: String },

    /// A merge conflict requires manual resolution.
    ConflictPending(Conflict),

    /// The reactive query subscription timed out waiting for changes.
    Timeout,

    /// An I/O error occurred at the block device layer.
    IoError(String),

    /// The content type is not supported for the requested operation.
    UnsupportedContentType(ContentType),
}
```

**Recovery guidance:**

| Error | Recovery |
| --- | --- |
| `QuotaExceeded` | Delete unused objects or request quota increase via Settings |
| `VersionConflict` | Re-read the object, merge changes, retry the write |
| `IntegrityError` | Object may be corrupted; attempt restore from version history or sync peer |
| `CryptoError` | Verify encryption key; may indicate tampered data |
| `WalFull` | Transient; retry after a short delay (WAL is trimmed automatically) |
| `SyncFailed` | Check network connectivity; sync will retry automatically |
| `ConflictPending` | Present conflict to user or resolve programmatically via `resolve_conflict()` |

## 7. Platform & AI Availability

**AIRS-enhanced features (require AIRS Kit):**

| Feature | What AIRS provides | Without AIRS |
| --- | --- | --- |
| Semantic search | Embedding-based similarity using HNSW index | Full-text BM25 search only |
| Content extraction | ML-powered text extraction from images, PDFs | Metadata-only indexing |
| Smart summarization | Object summaries for search result snippets | First N characters as snippet |
| Conflict resolution | AI-suggested merge strategies for sync conflicts | Manual resolution or last-writer-wins |
| Quota management | Predictive storage pressure and proactive cleanup hints | Static quota enforcement |
| Learned indexes | Adaptive index structures based on access patterns | Static BM25 + HNSW indexes |

**Platform availability:**

| Platform | Block Engine | Versioning | Full-Text Search | Semantic Search | Sync |
| --- | --- | --- | --- | --- | --- |
| QEMU virt | VirtIO-blk | Full | Full | Requires AIRS | Via Network Kit |
| Raspberry Pi 4 | SD/USB storage | Full | Full | Limited (CPU inference) | Via Network Kit |
| Raspberry Pi 5 | NVMe/SD/USB | Full | Full | Limited (CPU inference) | Via Network Kit |
| Apple Silicon | NVMe | Full | Full | Full (ANE acceleration) | Full mesh sync |

**Implementation phase:** Phase 4+ (Block Engine, Object Store, Version Store, encryption).
Query Engine arrives in Phase 11+. Space Sync in Phase 13+. Semantic search requires
AIRS Kit (Phase 14+).

---

*See also: [Flow Kit](./flow.md) | [Capability Kit](../kernel/capability.md) | [Memory Kit](../kernel/memory.md) | [Search Kit](../intelligence/search.md) | [IPC Kit](../kernel/ipc.md)*
