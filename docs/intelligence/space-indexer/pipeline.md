# AIOS Space Indexer — Indexing Pipeline

Part of: [space-indexer.md](../space-indexer.md) — Space Indexer
**Related:** [indexing-policy.md](./indexing-policy.md) — Selective indexing, [embedding-index.md](./embedding-index.md) — HNSW index, [fulltext-index.md](./fulltext-index.md) — Full-text index

-----

## 3. Indexing Pipeline

The Space Indexer executes a multi-stage pipeline for every object mutation. The pipeline is designed for two modes: a fast synchronous path (full-text only) that runs on every write, and a slower asynchronous path (AI-enriched) that runs for promoted objects via the index queue.

### 3.1 Index Queue & Job Scheduling

```rust
pub struct SpaceIndexer {
    /// Priority queue of pending indexing jobs.
    /// Higher-priority jobs (user-requested, recently promoted) execute first.
    queue: IndexQueue,
    /// Handle to the loaded embedding model (~100 MB, 384-dim).
    /// Always resident in AIRS model pool alongside the primary LLM.
    embedding_model: ModelHandle,
    /// Number of objects to batch for embedding generation.
    /// Batching amortizes AIRS inference overhead.
    batch_size: usize,                  // default: 16
}

pub struct IndexJob {
    object: ObjectId,
    space: SpaceId,
    trigger: IndexTrigger,
    /// Priority: higher = processed sooner.
    /// Requested > Created/Modified > Scheduled.
    priority: u8,
    /// Timestamp when the job was queued.
    queued_at: Timestamp,
}

pub enum IndexTrigger {
    /// Object just created — full-text index updated synchronously,
    /// AI indexing queued if the object is already promoted.
    Created,
    /// Object content changed — re-index text and (if promoted) re-embed.
    Modified,
    /// Object just promoted from CompactObject to full Object.
    /// Highest-priority asynchronous trigger.
    Promoted,
    /// Periodic re-index pass — catch objects whose embedding model
    /// has been updated since last indexing.
    Scheduled,
    /// Agent or user explicitly requested re-indexing.
    Requested,
}
```

**Queue implementation:** The IndexQueue is a bounded priority queue backed by a `BinaryHeap<IndexJob>`. Maximum capacity is 65,536 jobs. When the queue is full, the lowest-priority jobs (Scheduled) are dropped — they will be re-queued on the next periodic scan. Priority ordering:

| Priority | Trigger | Rationale |
|---|---|---|
| 255 | `Requested` | User or agent explicitly asked — respond immediately |
| 200 | `Promoted` | Object just became eligible for AI features — user is likely interacting |
| 128 | `Created` | New object — index while content is fresh in cache |
| 128 | `Modified` | Content changed — stale embeddings are worse than none |
| 64 | `Scheduled` | Background maintenance — droppable under load |

**Scheduling:** The Space Indexer runs as a Normal-class thread on the AIRS scheduling budget. It yields to foreground work (interactive tasks, conversation generation) and consumes spare compute cycles. On a 4-core system, it typically runs on cores 2-3 when cores 0-1 are busy with interactive work. The indexer sleeps when the queue is empty, waking on:

- Object mutation notifications from Space Storage (IPC channel)
- Periodic timer (every `batch_reindex_interval`, default 1 hour)
- Explicit wake from AIRS when the embedding model is updated

**Batch processing:** The indexer dequeues up to `batch_size` jobs at once and processes embeddings in a single AIRS inference batch. This amortizes the fixed overhead of model invocation (tokenizer warm-up, KV cache allocation). For a batch of 16 documents at ~512 tokens each, a single batch embedding takes ~200ms on a Cortex-A72 with NEON SIMD, vs ~50ms per document individually (16 × 50 = 800ms sequential).

### 3.2 Content Extraction

Before any indexing, the Space Indexer extracts searchable content from the object:

```rust
pub struct ExtractedContent {
    /// Plain text suitable for tokenization and embedding.
    /// Stripped of formatting, markup, and binary data.
    text: String,
    /// Structured metadata fields for filter indexing.
    metadata: ObjectMetadata,
    /// Content type determines extraction strategy.
    content_type: ContentType,
    /// Byte length of original content (for size-based decisions).
    original_size: u64,
}

pub enum ExtractionStrategy {
    /// Text-based content: extract directly (notes, code, config).
    /// UTF-8 decode, strip markup (Markdown, HTML), normalize whitespace.
    DirectText,
    /// Structured data: extract field values (JSON, TOML, YAML).
    /// Flatten nested keys into searchable text.
    StructuredData,
    /// Rich documents: extract via format-specific parser.
    /// PDF → text extraction, DOCX → paragraph text, etc.
    RichDocument,
    /// Binary content: extract metadata only (images, audio, video).
    /// File name, EXIF data, duration, dimensions — no text content.
    MetadataOnly,
    /// Exempt: skip extraction entirely (credentials, session tokens).
    Skip,
}
```

**Content type to strategy mapping:**

| Content Type | Strategy | Searchable Text |
|---|---|---|
| `Document`, `Note`, `Code` | `DirectText` | Full content |
| `Config` | `StructuredData` | Flattened key-value pairs |
| `RichMedia` | `RichDocument` | Extracted text (if parser available) |
| `Image`, `Audio`, `Video` | `MetadataOnly` | File name, EXIF, dimensions, duration |
| `Credential`, `SessionToken`, `Cookie` | `Skip` | None (security-sensitive) |

**Text normalization:** Extracted text is normalized before tokenization:

1. Unicode NFC normalization (canonical composition)
2. Lowercase folding (for case-insensitive search)
3. Whitespace collapsing (multiple spaces/newlines → single space)
4. Markup stripping (Markdown headers/links/formatting → plain text)
5. Maximum length truncation (64 KB for full-text, 4 KB for embedding input)

The 4 KB embedding input limit reflects the context window of the small embedding model (~100 MB). For objects longer than 4 KB, the first 4 KB is embedded (capturing the document's introduction/summary, which typically carries the most semantic weight). Full-text indexing uses the complete text without truncation.

### 3.3 Embedding Generation

Embedding generation is the most compute-intensive step. It runs only for promoted objects (or on-demand; see [indexing-policy.md §4.3](./indexing-policy.md)).

```rust
pub struct EmbeddingResult {
    /// Dense vector representation of the object's content.
    /// Dimension is model-dependent; typically 384 for the companion model.
    vector: Vec<f32>,
    /// Model identifier used for this embedding.
    /// Stored with the embedding for invalidation when the model is updated.
    model_id: ModelId,
    /// Timestamp of embedding generation.
    generated_at: Timestamp,
}
```

**Model specification:**

| Property | Value | Rationale |
|---|---|---|
| Model size | ~100 MB | Fits in AIRS model pool alongside primary LLM |
| Dimensions | 384 | Good accuracy-to-size ratio for on-device search |
| Format | GGUF (quantized) | Native GGML runtime support |
| Input limit | ~512 tokens (~4 KB text) | Matches companion model context window |
| Residency | Always loaded (companion model) | Never evicted; reserved in model pool budget |
| Inference | Batch of 16, ~200ms on Cortex-A72 | NEON SIMD acceleration |

**Determinism:** Embeddings are deterministic — the same content with the same model always produces the same vector. This property enables:

- **Eviction and regeneration:** Embeddings can be deleted under storage pressure and regenerated on demand without loss of information (see [embedding-index.md §5.3](./embedding-index.md)).
- **Model update detection:** When the embedding model is updated, the indexer compares `model_id` on existing embeddings and re-queues objects with stale embeddings.
- **Deduplication:** Objects with identical content produce identical embeddings, enabling storage optimization.

### 3.4 Entity & Relationship Extraction

Entity extraction identifies structured information within object content. Relationship extraction discovers connections between objects.

```rust
pub struct ExtractedEntities {
    /// Named entities found in the content.
    entities: Vec<Entity>,
    /// Relationships discovered between this object and others.
    relationships: Vec<InferredRelationship>,
}

pub struct Entity {
    /// The entity text as it appears in the content.
    mention: String,
    /// Normalized form (e.g., "John Smith" for "J. Smith", "John", "Smith, J.")
    canonical: String,
    /// Entity type classification.
    kind: EntityKind,
    /// Byte offset in the original content where this entity appears.
    offset: u32,
    /// Confidence score from the extraction model (0.0-1.0).
    confidence: f32,
}

pub enum EntityKind {
    Person,
    Organization,
    Location,
    Date,
    Event,
    Concept,
    /// Technical terms, API names, library names.
    Technical,
    /// Financial amounts, percentages, metrics.
    Numeric,
}

pub struct InferredRelationship {
    /// The target object that this object is related to.
    target: ObjectId,
    /// The kind of relationship inferred.
    kind: RelationKind,
    /// Confidence score (< 1.0 for AI-inferred relationships).
    confidence: f32,
    /// Human-readable explanation of why this relationship was inferred.
    explanation: String,
}
```

**Extraction approaches (tiered by resource availability):**

| Tier | Method | When Used | Entities | Relationships |
|---|---|---|---|---|
| Full AIRS | LLM-based extraction via primary model | AIRS fully available, spare compute | All entity types, high accuracy | Cross-object similarity, reference detection |
| Companion model | Small embedding model with NER head | AIRS available but primary model busy | Person, Org, Location, Date | Embedding cosine similarity only |
| Rule-based | Regex + heuristics (no ML) | AIRS unavailable | Date patterns, email addresses, URLs | Explicit references only (links, mentions) |

**Cross-object relationship discovery:** When a new object is indexed, the Space Indexer checks for relationships with existing objects:

1. **Embedding similarity:** Compute cosine similarity between the new embedding and all embeddings in the HNSW index. Objects above 0.85 similarity threshold get a `RelatedTo` edge.
2. **Entity co-occurrence:** If two objects mention the same canonical entity (e.g., same person name), create a `References` edge.
3. **Explicit references:** If the content contains links, file paths, or `@mentions` pointing to other objects, create `References` or `DependsOn` edges with confidence 1.0.

### 3.5 Summary & Tag Generation

Summaries and tags provide human-readable semantic metadata for browsing and filtering.

```rust
pub struct SemanticSummary {
    /// 1-2 sentence summary of the object's content.
    /// Generated by the primary LLM or companion model.
    text: String,
    /// 5-10 tags describing the object's topics.
    /// Tags are lowercase, hyphenated (e.g., "quarterly-revenue", "project-alpha").
    tags: Vec<String>,
    /// Model that generated this summary.
    model_id: ModelId,
    /// Timestamp of generation.
    generated_at: Timestamp,
}
```

**Generation strategy:** Summary and tag generation uses the same tiered approach as entity extraction. When the primary LLM is available, summaries are higher quality (contextual, nuanced). When only the companion model is available, summaries are simpler but still useful. When AIRS is unavailable, no summaries are generated — objects retain any previously generated summary.

**Tag normalization:** Tags are normalized to a canonical form: lowercase, spaces replaced with hyphens, deduplicated against existing tags in the space. A tag vocabulary is maintained per space to ensure consistency (e.g., "q3-review" and "q3-reviews" are merged).

### 3.6 SemanticMetadata Storage

All AI-generated metadata is stored in a single `SemanticMetadata` struct attached to promoted objects:

```rust
pub struct SemanticMetadata {
    /// Embedding vector (384-dim f32, ~1.5 KB raw; less with quantization).
    embedding: Option<Vec<f32>>,
    /// Extracted entities.
    entities: Vec<Entity>,
    /// AI-generated summary (1-2 sentences).
    summary: Option<String>,
    /// AI-generated tags (5-10).
    tags: Vec<String>,
    /// Model ID used for embedding (for staleness detection).
    embedding_model: Option<ModelId>,
    /// Model ID used for entity/summary extraction.
    extraction_model: Option<ModelId>,
    /// Timestamp of last AI indexing pass.
    last_indexed: Timestamp,
}
```

**Storage location:** SemanticMetadata is stored as part of the full Object in Space Storage. It is a metadata-only update — the object's `content_hash` does not change when SemanticMetadata is updated. This means:

- Embedding updates do not trigger new versions in the Version Store
- SemanticMetadata can be regenerated from scratch without data loss
- Under storage pressure, SemanticMetadata can be partially evicted (embedding first, then entities, then summary/tags last)

**Write path:** The Space Indexer writes SemanticMetadata via the normal Space Storage IPC channel. It holds `WriteSpace` capability for the target space. The write is atomic — either all metadata is updated or none is.
