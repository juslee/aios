# AIOS Space Indexer — Search & Query Integration

Part of: [space-indexer.md](../space-indexer.md) — Space Indexer
**Related:** [embedding-index.md](./embedding-index.md) — Semantic search, [fulltext-index.md](./fulltext-index.md) — BM25 search, [relationship-graph.md](./relationship-graph.md) — Graph traversal, [../../storage/spaces/query-engine.md](../../storage/spaces/query-engine.md) — SpaceQuery enum

-----

## 8. Search & Query Integration

### 8.1 SpaceQuery::Semantic Interface

The semantic search interface is defined in the Space Storage query engine ([query-engine.md §7.1](../../storage/spaces/query-engine.md)). The Space Indexer handles the embedding generation and HNSW search steps:

```rust
/// Semantic query lifecycle:
/// 1. Query engine receives SpaceQuery::Semantic { text, threshold, limit }
/// 2. Query engine sends text to AIRS for embedding generation
/// 3. AIRS generates query embedding via the companion model (~5ms)
/// 4. Space Indexer searches HNSW index for nearest neighbors
/// 5. Results (ObjectId, similarity_score) returned to query engine
/// 6. Query engine fetches object metadata and returns to caller

pub struct SemanticSearchResult {
    /// Object matching the query.
    object_id: ObjectId,
    /// Cosine similarity score (0.0–1.0).
    similarity: f32,
}
```

**Query embedding:** The query text is embedded using the same model and parameters as document embeddings. This ensures query-document similarity is meaningful — the embedding space is shared. Query embedding takes ~5ms for a single query string (vs ~200ms for a batch of 16 documents, because the batch amortizes model invocation overhead).

**Score interpretation:** Cosine similarity scores for 384-dimensional embeddings typically distribute as:

| Score Range | Interpretation | Example |
|---|---|---|
| 0.95–1.0 | Near-identical content | Same document, minor edits |
| 0.85–0.95 | Highly related | Same topic, different perspective |
| 0.70–0.85 | Moderately related | Same domain, different subtopic |
| 0.50–0.70 | Loosely related | Shared keywords or concepts |
| < 0.50 | Unrelated | Different domains entirely |

### 8.2 Composed Queries & Score Fusion

The most powerful queries combine full-text and semantic search. The Space Indexer implements score fusion to merge results from different index types into a single ranked list.

**Reciprocal Rank Fusion (RRF) — default:**

RRF merges ranked lists by reciprocal rank position, independent of score scale:

```text
RRF_score(d) = Σ 1 / (k + rank_i(d))

Where:
  d       = document (object)
  rank_i  = rank of d in result list i (1-indexed; ∞ if absent)
  k       = smoothing constant (default: 60)
```

```rust
pub struct RRFConfig {
    /// Smoothing constant. Higher values reduce the impact of high ranks.
    /// 60 is the standard default (Cormack et al., 2009).
    k: u32,                             // default: 60
}
```

**RRF advantages for AIOS:**

- No score normalization needed — BM25 scores (unbounded) and cosine similarity (0–1) have incompatible scales
- Robust to outliers — a single very high BM25 score doesn't dominate
- No training data required — works out of the box

**Linear combination with learned weights (adaptive):**

For users who perform many searches, the Space Indexer can learn optimal weights for combining BM25 and semantic scores:

```text
combined_score(d) = α × norm_bm25(d) + (1 - α) × cosine(d)

Where:
  α       = learned weight (0.0 = pure semantic, 1.0 = pure full-text)
  norm_bm25 = BM25 score normalized to [0, 1] via min-max across result set
```

```rust
pub struct LinearCombinationConfig {
    /// Weight for full-text (BM25) score. Semantic weight = 1.0 - alpha.
    alpha: f32,                         // default: 0.5
    /// Whether to auto-learn alpha from user click-through feedback.
    adaptive: bool,                     // default: true
    /// Minimum number of queries before switching from RRF to learned linear combination.
    min_queries_for_adaptation: usize,  // default: 100
}
```

**Automatic fusion method selection:**

The Space Indexer starts with RRF (no training required) and switches to learned linear combination after sufficient user interaction data. The switch is transparent — users see the same query interface.

```text
Fusion pipeline:
1. Run BM25 full-text search → ranked list A
2. Run HNSW semantic search → ranked list B
3. If adaptive && queries >= min_queries_for_adaptation:
     Use linear combination with learned alpha
   Else:
     Use RRF
4. Return fused ranked list
```

### 8.3 Graceful Degradation

A core principle of the Space Indexer: **AIRS enhances but is never required.** The search system degrades gracefully when components are unavailable:

| AIRS State | Semantic Search | Relationship Inference | Full-Text | Score Fusion |
|---|---|---|---|---|
| Fully available | Full HNSW search | All methods active | Always works | RRF or learned |
| Primary model busy | On-demand embedding only | Companion model only | Always works | RRF (full-text weighted) |
| AIRS unavailable | Returns empty set | Explicit edges only | Always works | Full-text only |
| Embedding index empty | Returns empty set | No similarity edges | Always works | Full-text only |

**Composed query degradation:** When a composed query includes both `TextSearch` and `Semantic` sub-queries and AIRS is unavailable:

1. `Semantic` sub-query returns an empty result set
2. The fusion step receives only the full-text results
3. Final results = full-text results (no score fusion needed)
4. The query engine logs a degradation event for diagnostics

This degradation is transparent to the caller — they receive results, just with lower recall on semantic matches. The search API includes a response field indicating which indexes contributed to the results:

```rust
pub struct SearchResponse {
    /// Ranked result set.
    results: Vec<SearchResult>,
    /// Which indexes contributed to these results.
    sources: SearchSources,
    /// Query execution time.
    latency_ms: u64,
}

pub struct SearchSources {
    /// Full-text index contributed results.
    fulltext: bool,
    /// Embedding index contributed results.
    semantic: bool,
    /// Relationship graph contributed results.
    graph: bool,
    /// Whether score fusion was applied (false if only one source).
    fused: bool,
}
```

-----

## 9. Cross-Service Integration

The Space Indexer's outputs (embeddings, entities, relationships, summaries) are consumed by multiple AIRS intelligence services beyond direct search. This section maps those integration points.

### 9.1 Context Engine (AIRS §5.2)

The Context Engine ([context-engine.md](../context-engine.md)) infers the user's current activity context — what they are working on, which project, what task. It uses Space Indexer outputs as signals:

- **Embedding similarity** between the currently focused object and the user's recent object history → detect context switches ("user moved from Project A to Project B")
- **Relationship graph traversal** from the current object → identify the project cluster and related resources
- **Entity extraction** from recent objects → detect topic shifts ("user is now researching machine learning")

The Context Engine subscribes to Space Indexer events (new embedding generated, new relationship created) via the AIRS internal event bus.

### 9.2 Attention Manager (AIRS §5.3)

The Attention Manager ([attention.md](../attention.md)) triages notifications and suggests next actions. It uses Space Indexer outputs to rank relevance:

- **Semantic similarity** between notification content and the user's current context → priority scoring
- **Relationship graph** to identify notifications from objects the user is actively working with → boost priority
- **Summary generation** to create compact notification digests when multiple related notifications arrive

### 9.3 Conversation Manager (AIRS §5.8)

The Conversation Manager manages the AI conversation context window. When the user asks a question, the Conversation Manager uses semantic search to find relevant objects to include in the LLM context:

- **SpaceQuery::Semantic** with the user's question → retrieve relevant documents
- **Relationship graph traversal** from retrieved documents → expand context with related objects
- **Summaries** for objects that don't fit in the context window → compressed representation

This is the primary mechanism by which AIRS "knows" about the user's data — the conversation LLM sees search results and summaries, not raw object content.

### 9.4 Agent Capability Intelligence (AIRS §5.9)

Agent Capability Intelligence uses the embedding index for outlier detection — comparing an agent's data access patterns against the corpus of similar agents:

- **Embedding similarity** between agent-accessed objects and the agent's declared scope → detect out-of-scope access
- **Entity extraction** from agent-generated content → verify agent is operating within its stated domain

### 9.5 Flow Service (Storage §6)

The Flow service ([../../storage/flow.md](../../storage/flow.md)) handles clipboard, paste, and cross-application data transfer. The Space Indexer enriches Flow entries:

- **Semantic matching** between pasted content and existing objects → suggest related context
- **Entity extraction** from clipboard content → auto-link to referenced objects
- **Content type detection** → route through appropriate transform pipelines
