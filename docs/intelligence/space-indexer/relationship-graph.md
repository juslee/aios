# AIOS Space Indexer — Relationship Graph

Part of: [space-indexer.md](../space-indexer.md) — Space Indexer
**Related:** [pipeline.md](./pipeline.md) — Entity & relationship extraction, [embedding-index.md](./embedding-index.md) — Similarity-based edges, [search-integration.md](./search-integration.md) — Graph traversal queries, [../../storage/spaces/data-structures.md](../../storage/spaces/data-structures.md) — Relation types

-----

## 7. Relationship Graph

The relationship graph tracks connections between objects — who cites whom, what depends on what, which documents discuss the same topic. It enables provenance queries ("where did this data come from?"), dependency analysis ("what breaks if I delete this?"), and knowledge exploration ("show me related objects").

### 7.1 Relationship Types

Relationships in AIOS are typed, directional, and carry confidence scores that distinguish user-created connections from AI-inferred ones.

```rust
pub enum RelationKind {
    // --- Explicit relationships (confidence = 1.0) ---

    /// Object A was derived from Object B (e.g., summary from research paper).
    DerivedFrom,
    /// Object A references Object B (e.g., link, citation, @mention).
    References,
    /// Object A depends on Object B (e.g., code imports, config dependency).
    DependsOn,
    /// Object A is a version/variant of Object B (e.g., draft → final).
    VersionOf,
    /// Object A is a child/part of Object B (e.g., chapter in a book).
    PartOf,
    /// Object A replies to or comments on Object B.
    RepliesTo,

    // --- AI-inferred relationships (confidence < 1.0) ---

    /// Objects are semantically similar (embedding cosine > threshold).
    RelatedTo,
    /// Objects share one or more extracted entities.
    SharesEntity,
    /// Objects appear to discuss the same topic (tag overlap + semantic similarity).
    SameTopic,
    /// Object A appears to be a continuation of Object B (temporal + semantic).
    ContinuationOf,
    /// Objects are in the same project/workflow (inferred from naming, timing, entities).
    SameProject,
    /// Objects have contrasting or opposing viewpoints on the same topic.
    Contrasts,
}

pub struct Relationship {
    /// Source object (the object "doing" the relating).
    source: ObjectId,
    /// Target object (the object being related to).
    target: ObjectId,
    /// The kind of relationship.
    kind: RelationKind,
    /// Confidence score: 1.0 for explicit, <1.0 for AI-inferred.
    confidence: f32,
    /// How this relationship was created.
    origin: RelationOrigin,
    /// When this relationship was created or last confirmed.
    created_at: Timestamp,
    /// Optional human-readable explanation (for AI-inferred relationships).
    explanation: Option<String>,
}

pub enum RelationOrigin {
    /// User explicitly created this relationship (drag-drop, link insertion).
    User,
    /// Agent created this relationship via API.
    Agent(AgentId),
    /// Space Indexer inferred this relationship during indexing.
    Indexer,
    /// Imported from another device during sync.
    Sync,
}
```

**Confidence thresholds for AI-inferred relationships:**

| Relationship Kind | Method | Threshold | Typical Confidence |
|---|---|---|---|
| `RelatedTo` | Cosine similarity of embeddings | > 0.85 | 0.85–1.0 |
| `SharesEntity` | Same canonical entity in both objects | > 0.7 (entity extraction confidence) | 0.7–0.95 |
| `SameTopic` | Tag overlap ≥ 3 AND cosine similarity > 0.7 | Combined score > 0.75 | 0.75–0.9 |
| `ContinuationOf` | Created within 2 hours AND cosine > 0.8 AND same author | Combined > 0.8 | 0.8–0.95 |
| `SameProject` | ≥ 2 shared entities AND naming pattern match | Combined > 0.7 | 0.7–0.85 |
| `Contrasts` | High cosine (similar topic) + sentiment divergence | Combined > 0.8 | 0.8–0.9 |

### 7.2 Graph Storage

The relationship graph uses a bidirectional adjacency list representation, enabling efficient traversal in both directions:

```rust
pub struct RelationshipGraph {
    /// Forward edges: source → Vec<Relationship>.
    /// "What does this object relate to?"
    forward: HashMap<ObjectId, Vec<Relationship>>,
    /// Reverse edges: target → Vec<Relationship>.
    /// "What relates to this object?"
    reverse: HashMap<ObjectId, Vec<Relationship>>,
    /// Total edge count (for statistics and capacity management).
    edge_count: usize,
    /// Index of relationships by kind (for kind-specific traversal).
    kind_index: HashMap<RelationKind, Vec<(ObjectId, ObjectId)>>,
}
```

**Storage overhead:** Each edge consumes approximately 100 bytes (two ObjectIds + kind + confidence + timestamp + optional explanation reference). For a typical personal device with 20,000 promoted objects and an average of 5 edges per object, the graph consumes ~10 MB.

**Persistence:** The relationship graph is stored in the `system/index/relationships/` space:

```text
system/index/relationships/
├── forward           (serialized forward adjacency lists)
├── reverse           (serialized reverse adjacency lists)
├── kind_index        (per-kind edge lists for fast kind-filtered traversal)
├── metadata          (edge_count, statistics, last compaction timestamp)
└── wal/              (incremental edge additions/deletions)
```

Like the other indexes, the graph uses incremental WAL writes with periodic full serialization and crash recovery via WAL replay.

### 7.3 Edge Lifecycle

**Creation:**

Edges are created through four mechanisms:

1. **Explicit user action** (confidence = 1.0): User creates a link between objects (drag-drop, reference insertion, @mention). These are always `References`, `DerivedFrom`, or `PartOf`.

2. **Agent action** (confidence = 1.0): An agent creates a relationship via the Space Storage API. The agent's `AgentId` is recorded in `RelationOrigin::Agent`.

3. **Indexer inference** (confidence < 1.0): During the AI indexing pipeline ([pipeline.md §3.4](./pipeline.md)), the Space Indexer discovers relationships:
   - Embedding similarity (cosine > 0.85) → `RelatedTo`
   - Entity co-occurrence → `SharesEntity`
   - Explicit references in content (links, paths) → `References` (confidence 1.0)
   - Temporal + semantic patterns → `ContinuationOf`, `SameProject`

4. **Sync import** (original confidence preserved): When objects are synced from another device, their relationships are imported with the original confidence scores.

**Deletion:**

Edges are deleted when:

- Either source or target object is deleted (cascade)
- User explicitly removes a relationship
- AI-inferred edge drops below confidence threshold on re-evaluation (edge aging, §7.5)
- Edge compaction removes duplicates

**Update:**

Edges are not updated in place. When an object is re-indexed and its relationships change, old AI-inferred edges are removed and new ones are created. Explicit edges (user/agent created) are never affected by re-indexing.

### 7.4 Graph Traversal

The relationship graph supports three traversal patterns:

**Direct traversal:** Follow edges from a starting object with configurable depth and direction.

```rust
pub struct TraversalQuery {
    /// Starting object.
    start: ObjectId,
    /// Which relationship kinds to follow (None = all kinds).
    kinds: Option<Vec<RelationKind>>,
    /// Maximum traversal depth (hops from start).
    max_depth: u32,                     // default: 3
    /// Traversal direction.
    direction: TraverseDirection,
    /// Minimum confidence threshold for edges to follow.
    min_confidence: f32,                // default: 0.5
    /// Maximum number of results to return.
    limit: usize,                       // default: 100
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

**Traversal algorithm:** Breadth-first search with visited-set deduplication. At each hop, only edges matching the kind filter and exceeding the confidence threshold are followed. Results are returned ordered by distance from the start node (closest first), with ties broken by edge confidence.

**PersonalRank traversal:** For exploratory queries ("show me everything related to Project X"), the graph supports a random-walk-based relevance ranking inspired by Personalized PageRank:

```rust
pub struct PersonalRankQuery {
    /// Seed objects (starting points for the random walk).
    seeds: Vec<ObjectId>,
    /// Damping factor: probability of following an edge vs. restarting.
    /// 0.85 is the standard default (85% follow, 15% restart at seed).
    damping: f32,                       // default: 0.85
    /// Number of random walk iterations.
    iterations: u32,                    // default: 20
    /// Maximum results to return (ranked by visit probability).
    limit: usize,                       // default: 20
    /// Minimum visit probability to include in results.
    min_score: f32,                     // default: 0.001
}
```

PersonalRank naturally balances direct connections (high confidence, close neighbors) against indirect connections (objects reachable through multiple hops). Objects with many paths from the seed set rank higher, even if no single path is strong. This produces exploratory results that feel more "complete" than simple traversal.

**Provenance chains:** A specialized traversal pattern for tracking object origin:

```text
Object A (summary)
  └── DerivedFrom → Object B (research notes)
       └── References → Object C (paper.pdf)
            └── DependsOn → Object D (dataset.csv)
```

Provenance traversal follows only `DerivedFrom`, `References`, and `DependsOn` edges in the forward direction. It produces a DAG (directed acyclic graph) showing the full lineage of an object — where it came from, what it references, and what it depends on.

### 7.5 Cross-Object Discovery

When a new object is indexed (or an existing object is re-indexed), the Space Indexer discovers potential relationships with existing objects through three mechanisms:

**1. Embedding similarity (requires AIRS):**

After generating the new object's embedding, compute cosine similarity against all embeddings in the HNSW index. Objects exceeding the similarity threshold (0.85) receive a `RelatedTo` edge.

```text
New object "Q3 Revenue Analysis"
  → cosine(embed("Q3 Revenue Analysis"), embed("Q3 Financial Forecast")) = 0.92
  → create RelatedTo edge (confidence: 0.92)

  → cosine(embed("Q3 Revenue Analysis"), embed("Lunch Menu")) = 0.12
  → below threshold, no edge
```

**2. Entity co-occurrence (requires AIRS):**

After extracting entities from the new object, check if any canonical entities match entities in existing objects. Matching canonical entities (same person, organization, date, etc.) produce `SharesEntity` edges.

```text
New object mentions "Acme Corp" (canonical: "Acme Corporation")
  → Object B also mentions "Acme" (canonical: "Acme Corporation")
  → create SharesEntity edge (confidence: avg of extraction confidences)
```

Entity co-occurrence is particularly valuable for connecting objects that discuss the same people or organizations but use different terminology — something pure keyword search misses.

**3. Explicit reference detection (no AIRS required):**

The content extraction pipeline scans for explicit references:

- **URLs and links** pointing to other objects → `References` (confidence 1.0)
- **File paths** matching other object names → `References` (confidence 1.0)
- **@mentions** matching user or agent identifiers → `References` (confidence 1.0)
- **Citation patterns** (e.g., `[1]`, `(Smith 2024)`) → `References` (confidence 0.9, lower because citation resolution may be imprecise)

Explicit reference detection is the only relationship discovery mechanism that works without AIRS. This ensures the relationship graph is always partially populated, even on devices where AIRS inference is unavailable.

### 7.6 Edge Aging & Maintenance

AI-inferred relationships may become stale as objects are modified or the embedding model is updated. The Space Indexer periodically re-evaluates inferred edges:

**Re-evaluation triggers:**

1. **Object modification:** When either endpoint of an edge is modified, all AI-inferred edges involving that object are re-evaluated during the next indexing pass.
2. **Model update:** When the embedding model changes, `RelatedTo` edges (which depend on embedding similarity) are invalidated. They are re-created during the batch re-indexing pass.
3. **Periodic sweep:** Every 24 hours, the indexer samples 5% of AI-inferred edges and re-evaluates them. Edges whose confidence has dropped below the threshold are removed.

**Edge decay:** AI-inferred edges that are not confirmed during re-evaluation receive a confidence penalty:

```text
new_confidence = original_confidence × (1.0 - decay_rate × days_since_creation)
decay_rate = 0.001 per day (edges lose ~3% confidence per month)
```

Edges whose confidence falls below 0.3 are automatically removed. This prevents the graph from accumulating stale relationships over time. Explicit edges (user/agent created) never decay.

**Graph compaction:** Periodic compaction (daily) performs:

1. Remove edges with confidence below 0.3
2. Merge duplicate edges (same source, target, kind) by keeping the highest confidence
3. Remove orphaned edges (where source or target object no longer exists)
4. Rebuild kind_index from forward/reverse maps
5. Update statistics (edge_count, per-kind counts)

### 7.7 Graph Statistics & Diagnostics

The relationship graph maintains statistics for monitoring and tuning:

```rust
pub struct GraphStatistics {
    /// Total number of edges.
    edge_count: usize,
    /// Edges by kind.
    edges_by_kind: HashMap<RelationKind, usize>,
    /// Edges by origin.
    edges_by_origin: HashMap<RelationOrigin, usize>,
    /// Average edges per object.
    avg_degree: f32,
    /// Maximum edges on any single object.
    max_degree: usize,
    /// Distribution of confidence scores (histogram buckets).
    confidence_distribution: [usize; 10],   // [0.0-0.1), [0.1-0.2), ..., [0.9-1.0]
    /// Timestamp of last compaction.
    last_compaction: Timestamp,
    /// Number of edges removed in last compaction.
    last_compaction_removed: usize,
}
```

These statistics are exposed via the Space Storage IPC interface for diagnostic tools (Inspector, [../../applications/inspector.md](../../applications/inspector.md)) and for AIRS intelligence services that use the graph for context inference.
