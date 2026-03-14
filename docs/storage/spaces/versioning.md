# AIOS Space Storage — Version Store

Part of: [spaces.md](../spaces.md) — Space Storage System
**Related:** [data-structures.md](./data-structures.md) — Core Data Structures, [block-engine.md](./block-engine.md) — Block Engine, [sync.md](./sync.md) — Space Sync

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

**Provenance signatures:** The `signature` field in `ProvenanceEntry` is an Ed25519 signature over the canonical Bincode encoding of `(action, agent, task, timestamp)` — the four fields of the entry itself, excluding the signature. The agent's signing keypair is held in the kernel identity store — agents never access raw keys directly, they request the kernel to sign on their behalf. On read, the signature is verified against the agent's public key (stored in `system/identity/`). If verification fails, the version node is flagged as corrupted or tampered and a security event is logged. Signatures are immutable — stored in the Merkle DAG and tied to the content hash chain. This provides non-repudiation: an agent cannot later deny creating or modifying an object.

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
/// Used for provenance chain signatures. See model.md §4.
fn kernel_sign(data: &[u8]) -> Signature;

/// Version DAG storage. Manages version history, branching, merging,
/// and retention policy enforcement for space objects.
pub struct VersionStore { /* internal state: LSM-tree handle, retention config */ }

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
    fn log(&self, space: SpaceId, object: ObjectId) -> impl Iterator<Item = Result<Version, StorageError>> {
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
    /// Note: uses ? on StorageError returns from get_version()/head(),
    /// requiring From<StorageError> for Error (via Error::IoError).
    /// Returns VersionError::ObjectMismatch on mismatch, requiring
    /// From<VersionError> for Error (via Error::VersionError).
    fn rollback(&self, space: SpaceId, object: ObjectId, target: Hash) -> Result<Version, Error> {
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
                object.as_bytes(),
                &Timestamp::now().as_millis().to_le_bytes(),
            ]),
            parent: Some(current_head.hash),
            merge_parent: None,  // rollback is linear, not a merge
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
    /// Objects deleted since the snapshot are restored. Objects created after
    /// the snapshot are intentionally retained (snapshot rollback is
    /// non-destructive for new content).
    /// Note: head() returns Result<_, StorageError>, but this method returns
    /// Result<_, Error>. The ? operator relies on From<StorageError> for Error
    /// (via Error::IoError(StorageError)). Similarly, rollback() may return
    /// VersionError, converted via Error::VersionError(VersionError).
    fn rollback_to_snapshot(&self, snapshot: &Snapshot) -> Result<u64, Error> {
        let mut rolled_back = 0u64;
        for (object_id, version_hash) in &snapshot.object_versions {
            match self.head(snapshot.space, *object_id) {
                Ok(current_head) if current_head.hash != *version_hash => {
                    self.rollback(snapshot.space, *object_id, *version_hash)?;
                    rolled_back += 1;
                }
                Ok(_) => {} // already at snapshot version
                Err(StorageError::BlockNotFound) => {
                    // Object's version data was deleted after snapshot — restore it
                    self.rollback(snapshot.space, *object_id, *version_hash)?;
                    rolled_back += 1;
                }
                Err(e) => return Err(Error::IoError(e)),
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

```text
        A (common ancestor)
       / \
      B   C       ← two devices diverged
      |   |
      D   E       ← continued independent edits
       \ /
        F         ← merge version (conflict resolved)
```

Branch creation is implicit — it happens whenever Space Sync (§8) discovers that both the local and remote DAGs have advanced past a common ancestor. The `SyncConflict` struct (§8) represents a detected fork, and resolution produces a merge version node with two parents. The `Version` struct (§5.1) already includes a `merge_parent: Option<Hash>` field (added early to avoid schema migration in Phase 9c) — a second parent, only set for merge commits. Single-device operation always produces a linear chain (every version has at most one parent; `merge_parent` is `None`).

Conflict resolution strategies are defined in §8. Branching semantics are Phase 9c work — single-device operation (Phase 4a-4l) always produces a linear chain.

-----
