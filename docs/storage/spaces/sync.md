# AIOS Space Storage — Space Sync Protocol

Part of: [spaces.md](../spaces.md) — Space Storage System
**Related:** [versioning.md](./versioning.md) — Version Store (Merkle DAG), [encryption.md](./encryption.md) — Encryption (sync key exchange)

-----

## 8. Space Sync Protocol

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

Sync proceeds in three rounds over the ANM mesh transport (Noise IK authenticated, see [networking/mesh.md](../../platform/networking/mesh.md)):

```text
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
3. **Encrypted transport.** All sync traffic is encrypted end-to-end by the ANM mesh layer ([networking/mesh.md](../../platform/networking/mesh.md)). The Space Sync protocol never sees plaintext on the wire — it hands structured messages to the NTM, which handles Noise IK encryption over whichever transport mode the mesh selects (Direct Link, Relay, or Tunnel).
4. **Content verification.** Every received version node and content block is verified against its content hash before being written to the local DAG. A malicious or corrupted remote cannot inject invalid data — the Merkle chain rejects it.

### 8.4 Transport Failure Handling

Network connections are unreliable. The sync protocol handles failures gracefully:

- **Resumable transfers.** Sync state (§8 `SyncState`) tracks `pending_push` and `pending_pull` queues. If the connection drops mid-sync, the next sync attempt resumes from where it left off — already-transferred objects are not re-sent (content-hash dedup catches this).
- **Exponential backoff.** Failed sync attempts retry with exponential backoff (30s, 1m, 2m, 5m, 15m, capped at 1h). Background sync is opportunistic — it does not burn battery or bandwidth on repeated failures.
- **Bandwidth throttling.** Sync runs at `Idle` scheduling class (scheduler.md §3.1) and respects a configurable bandwidth ceiling. Interactive network traffic (agent API calls, web requests) always takes priority.

**Encryption for synced spaces:** Personal spaces use per-device space keys — each device derives its own key from the user's passphrase (same passphrase, same derivation, same key). Collaborative spaces use a shared key distributed during the pairing ceremony (encrypted with the receiving device's public key). Untrusted spaces (web storage) are not synced.

**Sync uses the ANM mesh layer.** The NTM ([networking.md](../../platform/networking.md)) provides encrypted point-to-point channels between devices via the ANM Mesh Protocol (Noise IK). Space Sync sends structured messages to the NTM, which handles Noise IK encryption, mesh routing (Direct Link, Relay, or Tunnel), and retry logic. Space Sync code never deals with plaintext on the wire. Remote spaces are accessed via space operations (`space::remote("device-b/shared/project")`). Sync IPC messages will be defined in ipc.md (Phase 13c, not yet specified).

-----
