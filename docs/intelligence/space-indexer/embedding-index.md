# AIOS Space Indexer — Embedding Index

Part of: [space-indexer.md](../space-indexer.md) — Space Indexer
**Related:** [pipeline.md](./pipeline.md) — Embedding generation, [indexing-policy.md](./indexing-policy.md) — Selective indexing, [fulltext-index.md](./fulltext-index.md) — Full-text index, [../../storage/spaces/query-engine.md](../../storage/spaces/query-engine.md) — EmbeddingIndex struct

-----

## 5. Embedding Index (HNSW)

The embedding index enables semantic search — finding objects by meaning rather than keywords. It stores dense vector representations of object content and supports approximate nearest-neighbor (ANN) search via an HNSW graph.

### 5.1 HNSW Graph Structure

HNSW (Hierarchical Navigable Small World) is a graph-based ANN algorithm that builds a multi-layer navigable graph. Higher layers contain fewer nodes with long-range connections for fast coarse navigation; lower layers contain all nodes with short-range connections for precise local search.

```rust
pub struct EmbeddingIndex {
    /// Multi-layer HNSW graph for approximate nearest-neighbor search.
    /// Each layer is a navigable small-world graph with different connectivity.
    hnsw: HnswGraph,
    /// Dimension of embedding vectors (model-dependent).
    dimensions: usize,                  // typically 384
    /// Map from internal HNSW node ID to ObjectId.
    id_map: Vec<ObjectId>,
    /// Reverse map: ObjectId → HNSW node ID (for deletion/update).
    reverse_map: HashMap<ObjectId, usize>,
    /// Total number of vectors in the index.
    count: usize,
    /// Optional quantized representation for memory-efficient search.
    quantized: Option<QuantizedIndex>,
}
```

**HNSW parameters:**

| Parameter | Value | Rationale |
|---|---|---|
| `m` (max connections per node) | 16 | Good recall-to-memory ratio for 384-dim vectors. Higher m improves recall but increases memory per node. |
| `ef_construction` | 200 | Build-time search width. Higher values produce a better graph at the cost of slower insertion. 200 gives >99% recall@10 for typical workloads. |
| `ef_search` | 50 | Query-time search width. Adjustable per-query. 50 gives ~97% recall@10 in <5ms for 20K vectors on Cortex-A72. |
| `max_layer` | auto (ln(N)) | Layer count grows logarithmically with dataset size. For 20K objects: ~4 layers. |

**Why HNSW over alternatives:**

| Algorithm | Search Latency | Build Time | Memory | Dynamic Insert/Delete | Filtered Search |
|---|---|---|---|---|---|
| **HNSW** | O(log N) | O(N log N) | High (graph edges) | Yes (incremental) | Yes (in-algorithm) |
| IVF-PQ | O(√N) | O(N) | Low (quantized) | Partial (rebuild clusters) | Post-filter only |
| DiskANN (Vamana) | O(log N) | O(N log N) | Medium (on-disk graph) | Limited | Yes |
| SPANN | O(√N) | O(N) | Low (disk-resident postings) | Limited | Post-filter only |
| Linear scan | O(N) | O(1) | Lowest | Trivial | Trivial |

HNSW is the right choice for AIOS because:

1. **Dynamic insertions and deletions** — objects are promoted and modified continuously. IVF requires periodic re-clustering; DiskANN requires graph rebuild.
2. **In-memory efficiency** — with 20K promoted objects and quantized vectors, the entire index fits in <2 MB. Disk-resident algorithms (DiskANN, SPANN) add unnecessary I/O latency.
3. **Filtered search support** — AIOS queries often combine semantic search with metadata filters (content type, space, date range). HNSW supports in-algorithm filtering naturally.

### 5.2 Vector Quantization

Raw 384-dimensional f32 vectors consume 1,536 bytes each. For a device with 20,000 promoted objects, that's 30 MB of raw embeddings — manageable but significant on memory-constrained devices. Quantization reduces storage and accelerates distance computation at the cost of some recall loss.

**Quantization options (documented for implementation selection):**

```rust
pub enum QuantizationMethod {
    /// No quantization — full f32 vectors.
    /// Storage: 384 × 4 = 1,536 bytes per vector.
    /// Recall: baseline (100%).
    None,

    /// Scalar Quantization (SQ8): quantize each dimension to uint8.
    /// Storage: 384 × 1 = 384 bytes per vector (4x compression).
    /// Recall: ~99% of full precision for cosine similarity.
    /// Compute: SIMD-friendly uint8 dot product.
    ScalarUint8,

    /// Product Quantization (PQ): split vector into subvectors,
    /// quantize each subvector to a codebook index.
    /// Storage: ~48 bytes per vector (32x compression with 48 subquantizers × 8 bits).
    /// Recall: ~95-97% depending on training data and codebook size.
    /// Compute: lookup table distance computation.
    ProductQuantization {
        /// Number of subquantizers (vector split into this many parts).
        num_subquantizers: usize,   // typically 48 for 384-dim
        /// Bits per subquantizer code (codebook size = 2^bits).
        bits_per_code: u8,          // typically 8 (256 centroids)
    },

    /// RaBitQ (SIGMOD 2024): randomized binary quantization.
    /// Storage: 384 / 8 = 48 bytes per vector (32x compression).
    /// Recall: ~97-99% — competitive with PQ at same compression ratio.
    /// Compute: Hamming distance + correction factor (fast bitwise ops).
    /// Advantage over PQ: no training phase (codebook-free), lower CPU cost.
    RandomizedBinary,
}

pub struct QuantizedIndex {
    /// Quantization method in use.
    method: QuantizationMethod,
    /// Quantized vector data (layout depends on method).
    data: Vec<u8>,
    /// For PQ: trained codebooks (num_subquantizers × 2^bits × subdim floats).
    /// For RaBitQ: random rotation matrix (384 × 384 f32, shared across all vectors).
    /// For SQ8: min/max per dimension (384 × 2 f32).
    auxiliary: Vec<f32>,
}
```

**Quantization comparison for AIOS (20K objects, 384-dim):**

| Method | Per-Vector Size | Total (20K) | Recall@10 | Training Required | CPU Cost |
|---|---|---|---|---|---|
| None (f32) | 1,536 B | 30 MB | 100% | No | Baseline |
| SQ8 | 384 B | 7.5 MB | ~99% | Per-dimension stats | SIMD uint8 ops |
| PQ (48×8) | 48 B | 960 KB | ~95-97% | Codebook training | Lookup table |
| RaBitQ | 48 B | 960 KB | ~97-99% | No (random rotation) | Hamming + correction |

**Re-ranking with full vectors:** When quantization is enabled, the search pipeline uses a two-stage approach:

1. **Coarse search** over quantized vectors — fast, approximate distances
2. **Re-rank top-K** using full f32 vectors — exact distances for final ordering

Full f32 vectors are stored alongside quantized data for the re-ranking stage. Under storage pressure, full vectors for cold objects can be evicted (§5.3) — re-ranking falls back to quantized distances, slightly reducing result quality.

### 5.3 Persistence & Storage

The embedding index is stored as a Space Storage object within the `system/index/embeddings/` space:

```rust
pub struct EmbeddingIndexStorage {
    /// Serialized HNSW graph structure (edges, layers).
    /// Stored as a single block in Space Storage.
    graph_data: BlockId,
    /// Serialized vector data (raw f32 or quantized).
    /// Stored as a separate block for independent eviction.
    vector_data: BlockId,
    /// Index metadata: count, dimensions, model_id, parameters.
    metadata: EmbeddingIndexMetadata,
}

pub struct EmbeddingIndexMetadata {
    /// Number of vectors in the index.
    count: usize,
    /// Vector dimensions.
    dimensions: usize,
    /// HNSW parameters used to build this index.
    m: usize,
    ef_construction: usize,
    /// Model that generated the embeddings.
    model_id: ModelId,
    /// Quantization method in use.
    quantization: QuantizationMethod,
    /// Timestamp of last persistence.
    last_persisted: Timestamp,
}
```

**Persistence strategy:**

1. **Incremental writes**: After each batch of insertions (batch_size = 16), the Space Indexer writes a delta (new nodes + edges) to the WAL. Full serialization happens periodically (every 256 insertions or 5 minutes, whichever comes first).
2. **Crash recovery**: On startup, the Space Indexer loads the last full serialization, then replays WAL entries to reconstruct the latest state. If the WAL is corrupted, the index rebuilds from SemanticMetadata stored on individual objects.
3. **Versioning**: The index is stored via Space Storage's normal versioning mechanism. Snapshots can be taken for rollback, though this is rarely needed since embeddings are deterministically regenerable.

**Storage layout in `system/index/` space:**

```text
system/index/
├── embeddings/
│   ├── graph             (HNSW edge structure, ~200 KB for 20K nodes)
│   ├── vectors           (raw or quantized vector data)
│   ├── metadata          (index parameters, model ID, statistics)
│   └── wal/              (incremental updates since last full write)
├── fulltext/             (inverted index — see fulltext-index.md §6)
└── relationships/        (graph edges — see relationship-graph.md §7)
```

### 5.4 Eviction & Regeneration

Under storage pressure, the embedding index supports partial eviction to free memory:

**Eviction priority (evict first → evict last):**

1. **Full f32 vectors for cold objects** — objects not accessed within 30 days. Quantized representations remain for search; re-ranking uses quantized distances (slightly lower quality).
2. **HNSW edges for cold objects** — remove nodes from the graph entirely. These objects fall out of semantic search results until re-indexed.
3. **Quantized vectors for cold objects** — last resort. Object is fully removed from the embedding index.

```rust
pub struct EvictionPolicy {
    /// Objects not accessed within this duration are candidates for eviction.
    cold_threshold: Duration,           // default: 30 days
    /// Minimum number of vectors to keep in the index (never evict below this).
    min_vectors: usize,                 // default: 1000
    /// Target memory usage for the embedding index.
    target_memory: usize,              // default: 10 MB
    /// Whether to prefer evicting full vectors (keeping quantized) over removing nodes.
    prefer_partial_eviction: bool,      // default: true
}
```

**Regeneration:** Because embeddings are deterministic (same content + same model = same vector), any evicted embedding can be regenerated on demand:

1. Object is accessed or appears in a query result set
2. Space Indexer detects missing embedding
3. Re-queues the object at `Promoted` priority (priority 200)
4. Embedding is regenerated from the object's content within the next batch cycle (~200ms)

This property — evict freely, regenerate on demand — is fundamental to the Space Indexer's storage efficiency. The embedding model is always resident in the AIRS model pool, so regeneration cost is purely compute, not model loading.

### 5.5 Index Updates

**Insertion (object promoted or embedding regenerated):**

1. Generate embedding vector via AIRS inference engine (batch of 16, ~200ms)
2. If quantization is enabled, compute quantized representation
3. Insert node into HNSW graph: traverse from entry point, find neighbors at each layer, connect
4. Update id_map and reverse_map
5. Write delta to WAL

**Deletion (object deleted or demoted):**

1. Look up HNSW node ID via reverse_map
2. Mark node as deleted in the graph (lazy deletion — actual cleanup during compaction)
3. Remove from id_map and reverse_map
4. Write deletion to WAL

**Update (object content changed):**

1. Delete old embedding (lazy mark)
2. Generate new embedding from updated content
3. Insert as a new node
4. The old node is cleaned up during compaction

**Compaction:** Periodic compaction (every 1000 deletions or hourly) rebuilds sections of the HNSW graph to reclaim space from lazily deleted nodes. This is a background operation that does not block queries.

### 5.6 Filtered Search

Many AIOS queries combine semantic similarity with metadata constraints: "find documents about machine learning created this week." Naive post-filtering — running semantic search first, then filtering results — has poor recall when the filter is selective (few objects match the filter).

**In-algorithm filtering (FCVI/SIEVE approach):**

The Space Indexer implements in-algorithm filtering, where metadata predicates are evaluated during HNSW graph traversal rather than after:

```rust
pub struct FilteredSearchParams {
    /// Query embedding vector.
    query: Vec<f32>,
    /// Metadata filter predicate (evaluated during traversal).
    filter: SearchFilter,
    /// Minimum cosine similarity threshold.
    threshold: f32,
    /// Maximum results to return.
    limit: usize,
    /// ef_search parameter (may be increased for selective filters).
    ef_search: usize,
}

pub enum SearchFilter {
    /// No filter — pure semantic search.
    None,
    /// Filter by content type.
    ContentType(ContentType),
    /// Filter by space.
    Space(SpaceId),
    /// Filter by creation date range.
    DateRange { after: Option<Timestamp>, before: Option<Timestamp> },
    /// Filter by tags (any match).
    Tags(Vec<String>),
    /// Composite filter (AND of sub-filters).
    And(Vec<SearchFilter>),
}
```

**Adaptive ef_search:** When a filter is selective (matches <10% of objects), the search automatically increases `ef_search` to compensate for the reduced candidate pool. The formula:

```text
effective_ef = max(ef_search, ef_search / filter_selectivity)
```

Where `filter_selectivity` is the fraction of objects matching the filter (estimated from index metadata statistics). For example, if ef_search=50 and the filter matches 5% of objects, effective_ef = max(50, 50/0.05) = 1000.

**Performance impact:**

| Filter Selectivity | Post-Filter Recall@10 | In-Algorithm Recall@10 | Speedup |
|---|---|---|---|
| 100% (no filter) | 97% | 97% | 1x (equivalent) |
| 50% | 94% | 97% | ~1.5x |
| 10% | 78% | 96% | ~2.6x |
| 1% | 42% | 93% | ~8x |

In-algorithm filtering is critical for AIOS because users frequently search within a specific space (often <10% of total objects) or for specific content types, making post-filtering inadequate.
