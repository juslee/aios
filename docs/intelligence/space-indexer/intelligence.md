# AIOS Space Indexer — AI-Native Intelligence

Part of: [space-indexer.md](../space-indexer.md) — Space Indexer
**Related:** [airs/ai-native.md](../airs/ai-native.md) — AIRS AI-native features, [embedding-index.md](./embedding-index.md) — HNSW index, [relationship-graph.md](./relationship-graph.md) — Relationship graph

-----

## 12. AI-Native Intelligence

The Space Indexer has unique access to the semantic structure of all user data. This section describes how AIRS and kernel-internal ML can leverage that structure for features beyond basic search.

### 12.1 AIRS-Dependent Features

These features require AIRS inference and are unavailable when AIRS is offline.

**12.1.1 Adaptive Indexing Priority**

The Space Indexer observes which objects users search for and interact with. Over time, it learns to prioritize indexing for objects that are likely to be searched:

- **Search hit tracking:** Objects that appear in search results and are subsequently opened receive a priority boost for re-indexing (fresher embeddings).
- **Temporal patterns:** If a user frequently searches for objects modified in the last week, the indexer prioritizes recently modified objects.
- **Topic clusters:** If the user's recent searches cluster around a topic (detected via embedding similarity of query strings), the indexer proactively re-embeds objects in that topic cluster.

Implementation: A lightweight logistic regression model (frozen, deployed as a decision tree with ~50 nodes) predicts the probability that an object will be searched in the next hour, based on features: last_accessed, edit_count, search_hit_count, content_type, object_age, embedding_age. Objects with high predicted probability are re-embedded at `Scheduled` priority.

**12.1.2 Cross-Space Semantic Clustering**

When a user has objects across multiple spaces (work, personal, research), the Space Indexer can discover semantic clusters that span space boundaries:

- Objects about "machine learning" in the work space and "neural network tutorials" in the personal space form a cross-space cluster
- The Context Engine uses these clusters to provide unified context when the user switches between related work across spaces

This requires cross-space embedding comparison, which is only allowed when the user holds `ReadSpace` capabilities for all involved spaces. The clustering algorithm (k-means on embedding centroids) runs as an Idle-class background task.

**12.1.3 Query-Aware Index Optimization**

After observing enough queries (>1000), the Space Indexer can optimize its indexes based on actual query patterns:

- **HNSW ef_search tuning:** If queries consistently achieve high recall at ef_search=30, reduce the default from 50 to 30 (saving ~40% query latency). If recall is poor, increase ef_search.
- **BM25 parameter tuning:** Adjust k1 and b parameters based on observed relevance judgments (implicit: user clicks on search results).
- **Quantization selection:** If the user's query patterns show that quantized search produces acceptable recall (>95%), the indexer can aggressive quantize to save memory. If recall is poor, it can selectively upgrade hot clusters to raw vectors.

**12.1.4 Relationship Prediction**

Beyond discovering relationships during indexing, AIRS can predict future relationships:

- When a user is editing a document, predict which existing objects they might reference (based on embedding similarity + entity overlap with partially written content)
- Surface these as "suggested links" in the editing UI
- If the user accepts a suggestion, create an explicit `References` edge

This is a lightweight recommendation system powered by the embedding index and entity co-occurrence data.

### 12.2 Kernel-Internal ML

These features use purely statistical methods (no AIRS inference) and can run as frozen models in the kernel or as simple heuristics in the Space Indexer. They work regardless of AIRS availability.

**12.2.1 Index Access Pattern Prediction**

A frozen decision tree (~20 nodes) predicts which HNSW regions will be accessed next, based on the query embedding's approximate location in the vector space. This enables prefetching of HNSW graph nodes from disk (when the graph is partially on disk) or from slower memory tiers.

Features:
- Query embedding quantized to 8 cluster IDs (pre-computed via k-means on historical queries)
- Time of day (4 buckets: morning/afternoon/evening/night)
- Content type filter (if present)

Prediction: The 3 most likely HNSW entry-point regions to prefetch. Hit rate: ~70% for users with consistent search patterns.

**12.2.2 Eviction Policy Learning**

A simple frequency-recency score (combination of access count and last access time, weighted by a learned parameter) determines which embeddings to evict under memory pressure. This replaces pure LRU with a policy that accounts for both recency and popularity:

```text
eviction_score = (1.0 - w) × recency_score + w × frequency_score
w = learned weight (default: 0.3, adjusted based on cache hit rate)
```

Objects with the lowest eviction_score are evicted first. The weight `w` is adjusted every 1000 queries based on observed cache hit rates — if eviction decisions are poor (many re-fetches), w shifts toward recency; if decisions are good, w shifts toward frequency.

**12.2.3 Full-Text Index Bloom Filters**

For each posting list, a Bloom filter (~10 bits per entry) provides fast negative lookups — "this term definitely does not appear in the index." This avoids loading posting list data from disk for terms that don't exist, reducing I/O for queries containing rare or misspelled terms.

The Space Indexer maintains a global Bloom filter (all indexed terms) and per-space Bloom filters (terms in each space). Query evaluation checks the Bloom filter first:

```text
Query: "quarterly revenue"
  → Bloom("quarterly"): HIT → load posting list
  → Bloom("revenuee"): MISS → skip (term not in index, likely a typo)
```

**12.2.4 Auto-Tuning Compaction Thresholds**

A simple moving-average model tracks the ratio of tombstones to live entries in each index. When the tombstone ratio exceeds a learned threshold (adjusted based on observed compaction benefit), compaction is triggered. This avoids both premature compaction (wasting CPU) and delayed compaction (wasting memory).

Initial threshold: 15% tombstone ratio. Adjusted ±2% per compaction cycle based on whether the compaction freed significant space (>5% of index size) or was wasteful (<1% freed).
