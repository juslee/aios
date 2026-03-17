# AIOS Space Indexer — Security & Performance

Part of: [space-indexer.md](../space-indexer.md) — Space Indexer
**Related:** [airs/security.md](../airs/security.md) — AIRS security path isolation, [security/model.md](../../security/model.md) — Security model

-----

## 10. Security & Isolation

### 10.1 Resource Path Separation

The Space Indexer operates on the **resource path** of AIRS, not the security path. This distinction is critical for understanding its failure modes and damage ceiling:

| Property | Security Path | Resource Path (Space Indexer) |
|---|---|---|
| Services | Intent Verifier, Behavioral Monitor, Adversarial Defense | Space Indexer, Context Engine, Attention Manager |
| Failure mode | Disabled → kernel takes over (static heuristics) | Disabled → search degrades (full-text only) |
| Damage ceiling | If compromised: false-positive allow → security breach | If compromised: stale/missing embeddings → poor search quality |
| Priority | Never preempted by resource path | Yields to security path and foreground work |
| Scheduling | Dedicated budget, not borrowable | Normal + Idle class, spare compute |

The Space Indexer cannot compromise system security even if it produces incorrect results. The worst case is denial of semantic search — the system falls back to full-text search, which works independently.

### 10.2 Crash Containment

The Space Indexer runs within the AIRS subsystem runner, which provides crash isolation:

```rust
/// From airs/security.md §10.1 — SubsystemRunner wraps each AIRS service
pub struct SubsystemRunner {
    /// catch_unwind boundary around every entry point.
    /// A panic in the Space Indexer does not crash AIRS.
    panic_boundary: bool,
    /// Consecutive panic count. Circuit breaker trips at 3.
    consecutive_panics: AtomicU32,
    /// Maximum consecutive panics before disabling the subsystem.
    circuit_breaker_threshold: u32,     // 3
    /// Recovery state.
    state: SubsystemState,
}

pub enum SubsystemState {
    /// Normal operation.
    Running,
    /// Recovering from a panic. Re-initializing internal state.
    Recovering,
    /// Circuit breaker tripped. Subsystem disabled until manual reset.
    Disabled,
}
```

**Recovery protocol:**

1. Space Indexer panics (e.g., corrupt HNSW node, out-of-bounds vector access)
2. `catch_unwind` catches the panic; AIRS logs the failure
3. Space Indexer state is reset: queue cleared, indexes reloaded from last checkpoint
4. Recovery takes <500ms (index reload from Space Storage)
5. If 3 consecutive panics occur, the circuit breaker trips and the Space Indexer is disabled
6. Disabled state: no semantic indexing, no embedding generation. Full-text search continues unaffected.
7. Re-enable requires system administrator action (or automatic reset after 1 hour)

**What the Space Indexer CANNOT do even if compromised:**

- Access objects in spaces it doesn't have capability for (capability-gated per space)
- Modify object content (it only writes SemanticMetadata, not content)
- Read credential or security-sensitive objects (exempt types are enforced before content extraction)
- Affect the AIRS security path (separate subsystem, separate crash domain)
- Cause data loss (indexes are regenerable; source objects are unaffected)

### 10.3 Capability-Gated Access

The Space Indexer holds `ReadSpace` + `WriteSpace` capabilities for each space it indexes. These capabilities are granted by the Service Manager at boot and scoped per space:

```rust
pub struct IndexerCapabilities {
    /// Per-space capability tokens.
    /// ReadSpace: read object content for indexing.
    /// WriteSpace: write SemanticMetadata back to objects.
    space_caps: HashMap<SpaceId, (CapabilityHandle, CapabilityHandle)>,
}
```

**Access control rules:**

- The Space Indexer can only index objects in spaces it has capabilities for
- It cannot cross space boundaries — an object in space A cannot reference objects in space B during indexing unless the indexer holds capabilities for both
- Capability revocation (e.g., user removes a space) immediately stops indexing for that space
- Index data for a revoked space is purged within one compaction cycle

### 10.4 Embedding Privacy

Embeddings are dense vector representations that encode semantic meaning. A natural concern is whether embeddings can be "inverted" — reconstructed back to the original text.

**Embedding inversion risk:**

Embedding inversion attacks (recovering text from vectors) have been demonstrated in research settings for large, well-known models. AIOS mitigates this risk through multiple layers:

1. **Local-only processing:** Embeddings never leave the device. There is no cloud API, no telemetry, no sync of raw embeddings. Attack requires physical device access.
2. **Capability-gated access:** Reading embeddings requires the same `ReadSpace` capability as reading the original object. Embeddings do not provide a side channel.
3. **Exempt types:** Security-sensitive objects (Credential, SessionToken, Cookie) are never embedded. The exempt type list prevents the most dangerous content from entering the embedding space.
4. **Model obscurity:** The companion embedding model is small (~100 MB) and potentially unique per device (users can swap models). Model-specific inversion attacks require knowing the exact model.

**What AIOS does NOT do:**

- No differential privacy noise on embeddings (degrades search quality; the threat model is local-only, not federated)
- No embedding encryption at rest (the encryption zone of the space already encrypts all objects, including SemanticMetadata)
- No access-count throttling on embedding reads (capability enforcement is sufficient)

-----

## 11. Performance & Resource Management

### 11.1 Compute Budget

The Space Indexer runs as a Normal-class thread on the AIRS scheduling budget, yielding to foreground work:

| Activity | Scheduling Class | CPU Budget | When |
|---|---|---|---|
| Full-text index update | Synchronous (caller's context) | Part of write path | Every object mutation |
| Queue processing (batch embed) | Normal | Spare cycles on cores 2-3 | When queue non-empty |
| On-demand embedding | Interactive (elevated) | Up to 500ms budget | During user search |
| Compaction (all indexes) | Idle | Only when system idle | Hourly or on threshold |
| Batch re-indexing | Idle | Up to `max_batch_per_scan` objects | Hourly scan |

**Foreground yield:** When the user is actively interacting (typing, scrolling, generating text), the Space Indexer suspends batch processing and resumes when spare cycles are available. This ensures indexing never degrades interactive performance.

**On-demand elevation:** When a user performs a semantic search and the Space Indexer needs to generate on-demand embeddings (§4.3 in [indexing-policy.md](./indexing-policy.md)), the job temporarily runs at Interactive scheduling class to meet the 500ms latency budget.

### 11.2 Memory Budget

```rust
pub struct IndexMemoryBudget {
    /// Maximum memory for the HNSW graph structure (edges, layers).
    hnsw_graph: usize,                  // default: 2 MB
    /// Maximum memory for vector data (raw or quantized).
    vector_data: usize,                 // default: 2 MB (quantized) or 32 MB (raw)
    /// Maximum memory for the full-text inverted index.
    fulltext_index: usize,              // default: 32 MB
    /// Maximum memory for the relationship graph.
    relationship_graph: usize,          // default: 10 MB
    /// Maximum memory for the index queue.
    queue: usize,                       // default: 2 MB (65536 × ~32 bytes)
    /// Embedding model memory (reserved in AIRS model pool).
    /// NOT counted against the Space Indexer budget.
    model_memory: usize,               // ~100 MB (managed by Model Registry)
}
```

**Hardware tier scaling:**

| Hardware Tier | RAM | Embedding Index | Full-Text Index | Relationship Graph | Total |
|---|---|---|---|---|---|
| Minimal (4 GB) | 4 GB | 1 MB (quantized, 5K objects) | 10 MB | 5 MB | ~16 MB |
| Standard (8 GB) | 8 GB | 2 MB (quantized, 20K objects) | 32 MB | 10 MB | ~44 MB |
| Performance (16+ GB) | 16+ GB | 32 MB (raw, 20K objects) | 64 MB | 20 MB | ~116 MB |

On minimal hardware, the Space Indexer uses aggressive quantization and smaller index capacities. On performance hardware, it uses raw f32 vectors and larger indexes for better search quality.

### 11.3 Storage Budget

**Per-object storage overhead:**

| Component | Per Object | For 20K Promoted Objects | For 100K Total Objects |
|---|---|---|---|
| Embedding (raw f32) | 1,536 B | 30 MB | N/A (promoted only) |
| Embedding (quantized) | 48 B | 960 KB | N/A (promoted only) |
| Full-text posting entries | ~100 B avg | N/A | 10 MB |
| Relationship edges | ~500 B avg (5 edges) | 10 MB | N/A (promoted only) |
| SemanticMetadata | ~2 KB | 40 MB | N/A (promoted only) |
| Summary + tags | ~500 B | 10 MB | N/A (promoted only) |
| **Total (quantized)** | — | **~61 MB** | **~10 MB** |
| **Total (raw)** | — | **~90 MB** | **~10 MB** |

**Storage pressure response:** Under storage pressure (Space Storage §10), the Space Indexer sheds load in order:

1. Stop generating summaries and tags for newly promoted objects
2. Switch from raw to quantized embeddings (if not already)
3. Evict full f32 vectors for cold objects (keep quantized)
4. Evict embedding nodes for cold objects
5. Reduce full-text index compaction frequency
6. As a last resort: stop accepting new objects into the embedding index (full-text continues)

At each stage, search quality degrades slightly, but the system remains functional. Full-text search is never affected by storage pressure — it operates independently with a much smaller footprint.

### 11.4 Latency Budget

**Target latencies (end-to-end, from query to results):**

| Query Type | Target | Breakdown |
|---|---|---|
| Full-text only | < 50 ms | Tokenize (1ms) + BM25 lookup (10ms) + rank (5ms) + fetch metadata (30ms) |
| Semantic only (unfiltered) | < 100 ms | Embed query (5ms) + HNSW search (10ms, default ef_search=50) + fetch metadata (30ms) |
| Semantic with filter (>50% selectivity) | < 150 ms | Embed query (5ms) + filtered HNSW (30ms, moderate ef_search expansion) + fetch metadata (30ms) |
| Semantic with filter (<10% selectivity) | < 300 ms | Embed query (5ms) + filtered HNSW (100-200ms, ef_search up to 1000 per §5.6) + fetch metadata (30ms) |
| Hybrid (BM25 + semantic) | < 150 ms | Both in parallel + score fusion (5ms) + fetch metadata (30ms) |
| With on-demand embedding | < 500 ms | Full-text (50ms) + embed candidates (200ms) + re-rank (10ms) |
| Graph traversal (depth 3) | < 30 ms | BFS traversal (10ms/hop × 3) |
| PersonalRank | < 100 ms | 20 iterations of random walk over graph |

These latencies assume warm caches (indexes resident in memory). Cold-start latency after boot or index reload is higher (1-2 seconds for index deserialization) but occurs only once per session. The "Semantic only" row assumes unfiltered search at the default ef_search=50; filtered queries with low selectivity adaptively increase ef_search (see [embedding-index.md §5.6](./embedding-index.md)), which proportionally increases traversal time.
