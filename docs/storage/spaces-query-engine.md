# AIOS Space Storage — Query Engine

Part of: [spaces.md](./spaces.md) — Space Storage System
**Related:** [spaces-data-structures.md](./spaces-data-structures.md) — Core Data Structures, [spaces-block-engine.md](./spaces-block-engine.md) — Block Engine (LSM-tree index)

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
/// Query execution engine. Handles filter, full-text, semantic, and graph
/// traversal queries over the space object index.
pub struct QueryEngine { /* internal state: LSM-tree, full-text index, HNSW index, graph store */ }

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

```text
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

### 7.6 Future Direction: Learned Indexes

Traditional index structures (B-trees, bloom filters, hash maps) treat data distribution as unknown. Learned indexes replace these structures with ML models trained on the actual data distribution, achieving significant space and latency improvements:

**Bourbon (OSDI '20)** demonstrated that replacing bloom filters in LSM-trees with learned models reduces false positive rates by 30-80% at the same memory budget, or achieves the same false positive rate with 40-60% less memory. For AIOS's Block Engine, this means fewer unnecessary disk reads during LSM-tree lookups.

**LearnedKV** extends learned indexes to the full key-value lookup path, using lightweight neural networks to predict which SSTable and offset contains a given key. On workloads with predictable access patterns (common in content-addressed storage where hashes are uniformly distributed), learned indexes can reduce read amplification by 2-3x compared to traditional bloom filters.

**Applicability to AIOS:** The Space Storage query engine maintains multiple index types (LSM-tree, full-text, HNSW). Each is a candidate for learned optimization:

- **LSM-tree bloom filters** → learned filters (Bourbon approach) — immediate win, low complexity
- **Full-text posting lists** → learned term-to-posting models — moderate complexity, moderate win
- **HNSW parameters** → auto-tuned via workload profiling — already adaptive by design

Integration path: AIRS (Phase 8+) provides the ML runtime. A background "Index Tuner" agent could profile query patterns and train lightweight models to replace or augment traditional index structures. This aligns with AIOS's AI-first philosophy — the storage system improves itself based on usage patterns.
