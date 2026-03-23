# Search Kit

**Layer:** Intelligence | **Crate:** `aios_search` | **Architecture:** [`docs/intelligence/space-indexer.md`](../../intelligence/space-indexer.md)

## 1. Overview

The Search Kit provides full-text and semantic search across all Spaces in AIOS. It runs a
continuous indexing pipeline that extracts content, generates embeddings, builds entity
relationship graphs, and maintains inverted indexes -- so that every object in every Space is
searchable by keyword, by meaning, or by relationship. Unlike Spotlight or Windows Search,
which index file metadata and extracted text, Search Kit builds three overlapping index tiers:
a full-text inverted index (BM25), an embedding vector index (HNSW), and a relationship graph
(PersonalRank traversal). Results from all three tiers are fused using Reciprocal Rank Fusion
(RRF) to deliver contextually relevant results.

Search Kit follows AIOS's tiered availability principle. Full-text indexing is synchronous and
universal -- every object, including CompactObjects, is indexed immediately on creation. This
tier has zero AI dependency. Embedding generation is asynchronous and selective -- only
promoted objects receive embeddings, or objects are embedded on-demand when full-text search
yields poor results. Relationship extraction is opportunistic, occurring during object
promotion or when AIRS has spare compute capacity. This means search always works, and
semantic capabilities grow organically as the user interacts with data.

A key design influence from BeOS is **reactive query patterns** (BeOS lesson 2). Search Kit
supports live queries that push updates to subscribers when index state changes. Instead of
polling for new results, agents register a query and receive a callback whenever matching
objects are created, modified, or deleted. This enables real-time search-as-you-type,
live folder views, and dashboard widgets that update automatically.

## 2. Core Traits

```rust
use aios_capability::CapabilityHandle;
use aios_storage::{SpaceId, ObjectId};
use aios_airs::Embedding;

/// Structured or natural-language query with filters, scope, and ranking hints.
///
/// SearchQuery supports both keyword and semantic search. When an embedding
/// is provided, the HNSW index is included in result fusion. When omitted,
/// only full-text BM25 results are returned.
pub trait SearchQuery {
    /// The query text (natural language or keyword).
    fn text(&self) -> &str;

    /// An optional pre-computed embedding for semantic search.
    fn embedding(&self) -> Option<&Embedding>;

    /// Scope the search to specific Spaces.
    fn scope(&self) -> &SearchScope;

    /// Filter results by content type.
    fn content_type_filter(&self) -> Option<&[ContentType]>;

    /// Filter results by time range.
    fn time_range(&self) -> Option<&TimeRange>;

    /// Maximum number of results to return.
    fn limit(&self) -> usize;

    /// Minimum relevance score threshold (0.0-1.0).
    fn min_score(&self) -> f32;
}

/// The indexing pipeline: content extraction, embedding, entity extraction.
///
/// Agents typically do not interact with the pipeline directly -- Space
/// Storage triggers indexing automatically on object creation and update.
/// The pipeline is exposed for agents that need to force re-indexing or
/// index external content.
pub trait IndexPipeline {
    /// Force re-indexing of a specific object.
    fn reindex(&self, object: &ObjectId) -> Result<(), SearchError>;

    /// Submit external content for indexing (not stored in a Space).
    fn index_external(&self, content: ExternalContent) -> Result<IndexHandle, SearchError>;

    /// Query the indexing backlog size and estimated completion time.
    fn backlog(&self) -> IndexBacklog;

    /// Pause indexing (e.g., during heavy compute load).
    fn pause(&self) -> Result<(), SearchError>;

    /// Resume indexing after a pause.
    fn resume(&self) -> Result<(), SearchError>;
}

/// HNSW graph with quantized vectors for approximate nearest-neighbor search.
pub trait EmbeddingIndex {
    /// Find the k nearest neighbors to a query embedding.
    fn search(&self, query: &Embedding, k: usize) -> Result<Vec<EmbeddingHit>, SearchError>;

    /// Find nearest neighbors with a capability-aware space filter.
    fn search_filtered(
        &self,
        query: &Embedding,
        k: usize,
        filter: &SearchFilter,
    ) -> Result<Vec<EmbeddingHit>, SearchError>;

    /// Return the total number of indexed embeddings.
    fn count(&self) -> u64;

    /// Return the current quantization mode (SQ8, PQ, RaBitQ).
    fn quantization(&self) -> QuantizationMode;
}

/// Inverted index with BM25 scoring, CJK bigram tokenization, and phrase queries.
pub trait FullTextIndex {
    /// Execute a text query, returning BM25-scored results.
    fn search(&self, query: &str, limit: usize) -> Result<Vec<TextHit>, SearchError>;

    /// Execute a phrase query (exact sequence match).
    fn search_phrase(&self, phrase: &str, limit: usize) -> Result<Vec<TextHit>, SearchError>;

    /// Suggest completions for a partial query (typeahead).
    fn suggest(&self, prefix: &str, limit: usize) -> Result<Vec<String>, SearchError>;
}

/// Entity relationship store with PersonalRank traversal.
pub trait RelationshipGraph {
    /// Find objects related to a given object within N hops.
    fn related(&self, object: &ObjectId, max_hops: u32) -> Result<Vec<Relationship>, SearchError>;

    /// Run PersonalRank from a seed set, returning ranked related objects.
    fn personal_rank(
        &self,
        seeds: &[ObjectId],
        limit: usize,
    ) -> Result<Vec<RankedObject>, SearchError>;

    /// Query the relationship type between two objects.
    fn relationship_between(
        &self,
        a: &ObjectId,
        b: &ObjectId,
    ) -> Result<Option<RelationshipType>, SearchError>;
}

/// A live query that pushes updates when matching results change.
///
/// This follows BeOS's reactive query pattern -- register once, receive
/// updates as the index changes. No polling required.
pub trait LiveQuery {
    /// The query this live query is watching.
    fn query(&self) -> &dyn SearchQuery;

    /// Register a callback for result changes.
    fn on_change(&mut self, callback: Box<dyn Fn(LiveQueryEvent) + Send>);

    /// Get the current result set (snapshot).
    fn current_results(&self) -> Result<Vec<SearchResult>, SearchError>;

    /// Stop watching and release resources.
    fn close(self) -> Result<(), SearchError>;
}

/// Events emitted by a live query.
#[derive(Debug)]
pub enum LiveQueryEvent {
    /// A new object matches the query.
    Added(SearchResult),
    /// A matching object was updated (score or content changed).
    Updated(SearchResult),
    /// A previously matching object no longer matches.
    Removed(ObjectId),
}

/// A single search result with fused scoring.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub object_id: ObjectId,
    pub space_id: SpaceId,
    pub title: String,
    pub snippet: String,
    pub score: f32,
    pub sources: ResultSources,
}

/// Which index tiers contributed to a result's score.
#[derive(Debug, Clone)]
pub struct ResultSources {
    pub bm25_score: Option<f32>,
    pub embedding_score: Option<f32>,
    pub graph_score: Option<f32>,
}
```

## 3. Usage Patterns

**Minimal -- keyword search across all Spaces:**

```rust
use aios_search::{SearchKit, SearchScope};

let results = SearchKit::search("quarterly revenue projections", SearchScope::AllSpaces, 10)?;
for result in &results {
    println!("{}: {:.2} -- {}", result.title, result.score, result.snippet);
}
```

**Realistic -- semantic search with filters:**

```rust
use aios_search::{SearchKit, SearchQueryBuilder, SearchScope, ContentType};
use aios_airs::AirsKit;

// Generate query embedding for semantic search
let embedding = AirsKit::engine()?.embed(&["budget planning documents"], None)?;

let results = SearchKit::query(
    SearchQueryBuilder::new("budget planning")
        .embedding(embedding[0].clone())
        .scope(SearchScope::Space(my_space_id))
        .content_type_filter(&[ContentType::Document, ContentType::Spreadsheet])
        .time_range(TimeRange::last_days(90))
        .min_score(0.3)
        .limit(20)
        .build()
)?;

for result in &results {
    println!("{} [{:?}] -- score: {:.2} (bm25: {:?}, semantic: {:?})",
        result.title,
        result.space_id,
        result.score,
        result.sources.bm25_score,
        result.sources.embedding_score,
    );
}
```

**Advanced -- live query (reactive pattern):**

```rust
use aios_search::{SearchKit, SearchQueryBuilder, SearchScope, LiveQueryEvent};

// Register a live query -- results update automatically as objects change
let mut live = SearchKit::live_query(
    SearchQueryBuilder::new("meeting notes")
        .scope(SearchScope::Space(work_space))
        .limit(50)
        .build()
)?;

live.on_change(Box::new(|event| {
    match event {
        LiveQueryEvent::Added(result) => {
            println!("New match: {}", result.title);
            refresh_ui_list();
        }
        LiveQueryEvent::Updated(result) => {
            println!("Updated: {} (score: {:.2})", result.title, result.score);
        }
        LiveQueryEvent::Removed(id) => {
            println!("No longer matches: {:?}", id);
            remove_from_ui_list(id);
        }
    }
}));

// The query stays active until closed. New documents matching "meeting notes"
// trigger the callback automatically, without polling.
```

> **Common Mistakes**
>
> - **Always providing embeddings for simple keyword searches.** Embedding generation has
>   latency and consumes inference budget. For exact keyword lookups, omit the embedding and
>   rely on the BM25 full-text index.
> - **Not closing live queries.** Each live query consumes index watcher resources. Close them
>   when the UI that displays results is dismissed.
> - **Searching with `min_score(0.0)`.** This returns everything. Use a reasonable threshold
>   (0.2-0.4) to avoid flooding results with low-relevance matches.

## 4. Integration Examples

**Search Kit + AIRS Kit -- retrieval-augmented generation (RAG):**

```rust
use aios_search::{SearchKit, SearchScope};
use aios_airs::{AirsKit, TaskProfile, SessionConfig};

// Step 1: Search for relevant context
let results = SearchKit::search("AIOS memory management", SearchScope::AllSpaces, 5)?;

// Step 2: Build context from search results
let context: String = results.iter()
    .map(|r| format!("## {}\n{}\n", r.title, r.snippet))
    .collect();

// Step 3: Use AIRS with retrieved context
let mut session = AirsKit::open_session(TaskProfile::Conversation, SessionConfig {
    system_prompt: Some(format!(
        "Answer using only the following context:\n\n{}", context
    )),
    ..Default::default()
})?;

let response = session.submit("How does the buddy allocator work?")?.collect_all()?;
```

**Search Kit + Flow Kit -- searchable transfer history:**

```rust
use aios_search::SearchKit;
use aios_flow::{FlowKit, FlowChannel};

// Flow entries are indexed automatically. Search past clipboard contents.
let results = SearchKit::search(
    "code snippet about error handling",
    SearchScope::FlowHistory,
    5,
)?;

for result in &results {
    println!("{} (score: {:.2}): {}", result.title, result.score, result.snippet);
}
```

**Search Kit + Storage Kit -- Space-scoped search with capability enforcement:**

```rust
use aios_search::{SearchKit, SearchScope, SearchFilter};
use aios_storage::StorageKit;

// Search respects Space capabilities -- agents can only see results
// from Spaces they have ReadSpace capability for.
let accessible_spaces = StorageKit::list_accessible_spaces()?;
let results = SearchKit::query(
    SearchQueryBuilder::new("design document")
        .scope(SearchScope::Spaces(accessible_spaces))
        .build()
)?;
```

## 5. Capability Requirements

| Method | Required Capability | Notes |
| --- | --- | --- |
| `SearchKit::search` | `SearchRead` | Filtered to accessible Spaces |
| `SearchKit::query` | `SearchRead` | Same, with advanced filters |
| `SearchKit::live_query` | `SearchRead` + `SearchLive` | Persistent watcher resource |
| `IndexPipeline::reindex` | `SearchManage` | Restricted to own Space objects |
| `IndexPipeline::index_external` | `SearchManage` | External content indexing |
| `RelationshipGraph::related` | `SearchRead` | Filtered by Space access |
| `FullTextIndex::suggest` | `SearchRead` | Typeahead suggestions |

```toml
# Agent manifest example
[capabilities.required]
SearchRead = { spaces = ["user/home/", "user/projects/"] }

[capabilities.optional]
SearchLive = { max_queries = 5 }
SearchManage = {}
```

## 6. Error Handling

```rust
/// Errors returned by Search Kit operations.
#[derive(Debug)]
pub enum SearchError {
    /// The search query syntax is invalid.
    InvalidQuery(String),

    /// The requested Space is not accessible (no capability).
    SpaceAccessDenied(SpaceId),

    /// The embedding index is not available (AIRS not loaded).
    EmbeddingUnavailable,

    /// The live query limit has been reached for this agent.
    LiveQueryLimitExceeded { max: u32 },

    /// The indexing pipeline is paused.
    IndexingPaused,

    /// The object was not found in any index.
    NotFound(ObjectId),

    /// Required capability was not granted.
    CapabilityDenied(String),

    /// Internal index corruption or storage error.
    InternalError(String),
}
```

**Recovery guidance:**

| Error | Recovery |
| --- | --- |
| `EmbeddingUnavailable` | Omit embedding from query; BM25 full-text results still work |
| `LiveQueryLimitExceeded` | Close unused live queries before opening new ones |
| `SpaceAccessDenied` | Request capability via agent manifest or prompt user |
| `IndexingPaused` | Search still works on existing index; new content not yet searchable |

## 7. Platform & AI Availability

**AIRS-enhanced features:**

| Feature | AIRS Available | Without AIRS |
| --- | --- | --- |
| Full-text search (BM25) | Available | Available (no AI dependency) |
| Semantic search (embeddings) | 384-dim HNSW with on-device model | Not available; full-text fallback |
| Query reranking | ML-based score fusion | Static RRF weights |
| Typeahead suggestions | Learned completion model | Prefix-based trie lookup |
| Relationship extraction | NER + semantic similarity | Explicit references only |
| Live query | Available | Available (no AI dependency) |

**Platform availability:**

| Platform | Full-Text | Embeddings | Relationship Graph | Notes |
| --- | --- | --- | --- | --- |
| QEMU virt | Yes | Slow (~2s/embedding) | Explicit only | Testing only |
| Raspberry Pi 4 | Yes | ~100ms/embedding | Yes | Q4_K_M embedding model |
| Raspberry Pi 5 | Yes | ~50ms/embedding | Yes | Faster embedding model |
| Apple Silicon | Yes | ~10ms/embedding | Full | ANE-accelerated embeddings |

**Feature detection:**

```rust
use aios_search::SearchKit;

let capabilities = SearchKit::available_tiers()?;
if capabilities.has_embeddings {
    // Use semantic search
    let results = SearchKit::query(semantic_query)?;
} else {
    // Fall back to keyword search
    let results = SearchKit::search("exact keywords", scope, 10)?;
}
```

**Implementation phase:** Phase 10+. Search Kit depends on [AIRS Kit](airs.md) for embedding
generation, [Storage Kit](../platform/storage.md) for Space object access, and
[Compute Kit](../kernel/compute.md) for hardware-accelerated embedding.

---

*See also: [AIRS Kit](airs.md) | [Storage Kit](../platform/storage.md) | [Flow Kit](flow.md) | [Context Kit](context.md)*
