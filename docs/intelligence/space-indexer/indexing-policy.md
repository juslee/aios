# AIOS Space Indexer — Selective Indexing Policy

Part of: [space-indexer.md](../space-indexer.md) — Space Indexer
**Related:** [pipeline.md](./pipeline.md) — Indexing pipeline, [embedding-index.md](./embedding-index.md) — HNSW index, [../../storage/spaces/data-structures.md](../../storage/spaces/data-structures.md) — CompactObject & promotion

-----

## 4. Selective Indexing Policy

Not every object deserves the same indexing effort. A credential file and a research paper have radically different search value. The Space Indexer uses a two-tier indexing policy that balances search coverage against compute and storage cost.

### 4.1 Full-Text vs Embedding Split

The fundamental split:

| Index Tier | Scope | Update Mode | AIRS Required | Storage Cost |
|---|---|---|---|---|
| Full-text | All objects (CompactObject + Object) | Synchronous on write | No | ~10-30% of text size |
| Embedding | Promoted objects only (full Object) | Asynchronous via queue | Yes | ~1.5 KB raw / ~48 B quantized per object |
| Relationship graph | Explicit: all objects. Inferred: promoted only | Mixed | Inferred edges: Yes | ~100 B per edge |
| Summary + tags | Promoted objects only | Asynchronous via queue | Yes | ~500 B per object |

**Why this split matters:**

Consider a device with 100,000 objects where 20% are promoted:

- **Full-text index** covers 100,000 objects — keyword search always finds everything
- **Embedding index** covers 20,000 objects — semantic search covers the objects users care about
- **Storage overhead** for embeddings: 20,000 × 1.5 KB = 30 MB (raw) or 20,000 × 48 B ≈ 1 MB (quantized)
- **Without selective indexing:** 100,000 × 1.5 KB = 150 MB (raw) — a 5x penalty for marginal search improvement on objects users never look at

### 4.2 Promotion Criteria

An object is promoted from CompactObject to full Object when its type is **not exempt** and any one of the following triggers fires:

```rust
pub struct PromotionPolicy {
    /// Promote when a user explicitly searches for and opens the object.
    on_user_interaction: bool,          // default: true
    /// Promote when the object is edited more than N times.
    edit_threshold: u32,                // default: 3
    /// Promote when the object exceeds N bytes (suggests meaningful content).
    size_threshold: u64,                // default: 4 KB
    /// Promote when another object creates a Relation to this one.
    on_relation_created: bool,          // default: true
    /// Never promote these content types (even if other criteria are met).
    exempt_types: Vec<ContentType>,
    // default: [Config, Credential, GameSave, CacheEntry, SessionToken, Cookie]
}
```

**Promotion is OR logic:** Any single trigger promotes the object. This ensures that important objects are promoted early:

- A document opened once → promoted (user showed interest)
- A note edited 3 times → promoted (user is actively working on it)
- A file > 4 KB → promoted (substantial content worth embedding)
- An object referenced by another → promoted (part of a knowledge web)

**Exempt types** are never promoted regardless of triggers. These are high-volume, low-semantic-value object types:

| Exempt Type | Rationale |
|---|---|
| `Config` | Machine-readable, not useful for semantic search |
| `Credential` | Security-sensitive, must not be embedded (embedding inversion risk) |
| `GameSave` | Binary blobs, not textually meaningful |
| `CacheEntry` | Ephemeral, high-volume, low value |
| `SessionToken` | Security-sensitive, ephemeral |
| `Cookie` | Security-sensitive, ephemeral |

**Web storage is always compact.** Objects in `web-storage/` spaces (cookies, localStorage, sessionStorage, IndexedDB entries, Cache API responses) are policy-enforced compact — the exempt types list includes all web storage content types, blocking promotion regardless of other triggers.

**Promotion atomicity:** Promotion is a single metadata update in the LSM-tree. The Space Indexer generates embedding, entities, and summary offline. Once ready, it writes a single Version node that adds the SemanticMetadata to the object (same ObjectId, same content_hash). If the object is modified during generation, the promotion applies to the version that was current when generation started. If the object is deleted mid-promotion, the promotion is abandoned.

### 4.3 On-Demand Embedding

When a user performs a semantic search and the full-text index returns poor results (BM25 score below a configurable threshold), the Space Indexer can generate embeddings **on the fly**:

```text
1. User queries: "quarterly revenue projections"
2. Full-text search (BM25): returns 5 results, all below score threshold 2.0
3. Space Indexer detects poor match quality
4. Takes top 20 full-text candidates (relaxed threshold)
5. Generates embeddings for unembedded candidates in real-time (~200ms batch)
6. Computes cosine similarity against the query embedding
7. Re-ranks results by semantic similarity
8. Returns re-ranked results to the user
```

**On-demand embedding policy:**

```rust
pub struct OnDemandPolicy {
    /// Enable on-demand embedding when full-text results are poor.
    enabled: bool,                      // default: true
    /// BM25 score threshold below which on-demand embedding triggers.
    bm25_threshold: f32,                // default: 2.0
    /// Maximum number of candidates to embed on-demand.
    max_candidates: usize,              // default: 20
    /// Maximum latency budget for on-demand embedding (ms).
    /// If exceeded, return full-text results as-is.
    latency_budget_ms: u64,             // default: 500
}
```

**On-demand embeddings are cached:** After generating an on-demand embedding, it is stored in the HNSW index and on the object's SemanticMetadata. Future semantic searches find it directly. This means on-demand embedding is a one-time cost per object — subsequent searches are fast.

**On-demand does NOT promote the object.** The embedding is generated and cached, but the object remains a CompactObject with a partial SemanticMetadata (embedding only, no entities or summary). Full promotion requires a trigger from PromotionPolicy. This avoids a feedback loop where searching for something promotes everything in the results.

### 4.4 Batch Re-Indexing

The Space Indexer periodically scans for objects that need re-indexing:

```rust
pub struct ReindexPolicy {
    /// How often to scan for stale or missing embeddings.
    scan_interval: Duration,            // default: 1 hour
    /// Re-embed objects whose embedding was generated by a different model.
    reembed_on_model_change: bool,      // default: true
    /// Re-embed objects older than this duration (catch gradual drift).
    max_embedding_age: Duration,        // default: 30 days
    /// Maximum batch size per scan (avoid monopolizing AIRS).
    max_batch_per_scan: usize,          // default: 256
}
```

**Scan triggers:**

1. **Model update:** When the embedding model is updated (user downloads a better model), all existing embeddings become stale. The indexer queues all promoted objects for re-embedding at `Scheduled` priority.
2. **Periodic sweep:** Every `scan_interval`, scan for promoted objects without embeddings (missed during a previous outage) or with embeddings older than `max_embedding_age`.
3. **Space import:** When a new space is synced from another device, its objects may lack embeddings generated by the local model. The indexer queues them for local embedding.
