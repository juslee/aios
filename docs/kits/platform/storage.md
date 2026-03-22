# Storage Kit

**Layer:** Platform | **Architecture:** `docs/storage/spaces.md` + 8 sub-docs

## Purpose

Space-based object storage with versioning, full-text and semantic query, and multi-device sync. All data lives in named Spaces composed of typed Objects and Relations. The Block Engine handles durable on-disk layout via an LSM-tree with WAL and AES-256-GCM encryption.

## Key APIs

| Trait / API | Description |
|---|---|
| `Space` | Named container for objects; enforces quotas and security zones |
| `Object` | Typed content unit with content hash, timestamps, and provenance |
| `Relation` | Directed edge between objects enabling graph traversal |
| `BlockEngine` | LSM-tree + WAL on-disk storage with CRC-32C integrity and encryption |
| `VersionStore` | Merkle DAG of object revisions; snapshot, branch, and rollback |
| `QueryEngine` | Full-text (BM25), embedding (HNSW), and learned-index dispatch |
| `SpaceSync` | Merkle-exchange based conflict-resolving multi-device sync |

## Dependencies

Memory Kit, Capability Kit, Compute Kit (query acceleration)

## Consumers

All applications, Flow Kit, Search Kit, POSIX compatibility layer

## Implementation Phase

Phase 4+ (Block Engine, objects, versions). Query Engine Phase 10+
