# AIOS Space Indexer — Full-Text Index

Part of: [space-indexer.md](../space-indexer.md) — Space Indexer
**Related:** [pipeline.md](./pipeline.md) — Content extraction, [embedding-index.md](./embedding-index.md) — Semantic search, [search-integration.md](./search-integration.md) — Query composition, [../../storage/spaces/query-engine.md](../../storage/spaces/query-engine.md) — FullTextIndex struct

-----

## 6. Full-Text Index

The full-text index is the Space Indexer's reliability backbone. It covers every object (CompactObject and full Object), updates synchronously on every write, and requires no AI inference. When AIRS is unavailable, semantic search degrades, but full-text search always works.

### 6.1 Inverted Index Structure

The full-text index is a classic inverted index: for each term, it stores a posting list of objects containing that term, along with positional information for phrase queries and proximity scoring.

```rust
pub struct FullTextIndex {
    /// Inverted index: term → posting list.
    /// BTreeMap for sorted term iteration (prefix queries, range scans).
    index: BTreeMap<String, PostingList>,
    /// Total number of indexed objects (for BM25 IDF calculation).
    doc_count: u64,
    /// Per-term document frequency (number of objects containing each term).
    /// Cached separately for fast IDF computation.
    term_doc_freq: HashMap<String, u64>,
    /// Average document length in tokens (for BM25 normalization).
    avg_doc_length: f64,
    /// Per-object token count (for BM25 length normalization).
    doc_lengths: HashMap<ObjectId, u32>,
}

pub struct PostingList {
    /// Objects containing this term, sorted by ObjectId for merge joins.
    entries: Vec<PostingEntry>,
}

pub struct PostingEntry {
    /// The object containing this term.
    object_id: ObjectId,
    /// Byte offsets where this term appears within the extracted text.
    /// Used for phrase queries and proximity scoring.
    positions: Vec<u32>,
    /// Term frequency in this object (TF component of BM25).
    frequency: u32,
}
```

**Why BTreeMap over HashMap for the term index:** Sorted iteration enables prefix queries ("proj*" matches "project", "projection", "projector") and range scans for auto-complete suggestions. The overhead vs HashMap is minimal for the term vocabulary sizes in personal computing (typically <100K unique terms).

### 6.2 BM25 Scoring

BM25 (Best Match 25) is the scoring function for full-text search. It balances term frequency (how often the query term appears in a document) against inverse document frequency (how rare the term is across all documents) and document length normalization (longer documents get a slight penalty to avoid bias toward verbose content).

```text
score(q, d) = Σ IDF(t) × ( TF(t,d) × (k1 + 1) ) / ( TF(t,d) + k1 × (1 - b + b × |d| / avgdl) )

Where:
  q     = query (set of terms)
  d     = document (object)
  t     = individual query term
  TF    = term frequency in document
  IDF   = log((N - n(t) + 0.5) / (n(t) + 0.5) + 1)
  N     = total document count
  n(t)  = number of documents containing term t
  |d|   = document length (token count)
  avgdl = average document length across all documents
  k1    = 1.2 (term frequency saturation)
  b     = 0.75 (length normalization)
```

```rust
pub struct BM25Config {
    /// Term frequency saturation parameter.
    /// Higher values increase the impact of additional term occurrences.
    /// 1.2 is the standard default; range [1.0, 2.0] is typical.
    k1: f32,                            // default: 1.2
    /// Document length normalization parameter.
    /// 0.0 = no length normalization; 1.0 = full normalization.
    /// 0.75 is the standard default.
    b: f32,                             // default: 0.75
}
```

**Multi-term queries:** For queries with multiple terms, BM25 scores are summed across terms. This naturally handles both AND and OR semantics — documents matching more query terms score higher, but documents matching any term still appear in results (ranked lower).

**Boost for recent objects:** The query engine optionally applies a recency boost that multiplies the BM25 score by a decay factor based on the object's last modification time. This helps surface recently edited documents when multiple objects match equally well:

```text
boosted_score = bm25_score × recency_factor
recency_factor = 1.0 + 0.5 × exp(-days_since_modification / 30.0)
```

### 6.3 Tokenization & Analysis

Text must be tokenized (split into searchable terms) before indexing. The tokenization pipeline transforms raw text into normalized terms suitable for matching:

```rust
pub struct TokenizerPipeline {
    /// Step 1: Unicode segmentation (UAX #29 word boundaries).
    segmenter: UnicodeSegmenter,
    /// Step 2: Lowercase folding (Unicode-aware).
    case_folder: CaseFold,
    /// Step 3: Stop word removal (language-specific).
    stop_words: StopWordFilter,
    /// Step 4: Stemming (optional, language-specific).
    stemmer: Option<Stemmer>,
}
```

**Pipeline stages:**

| Stage | Input | Output | Purpose |
|---|---|---|---|
| Unicode segmentation | Raw text | Word tokens | Split on UAX #29 word boundaries (handles CJK, emoji, punctuation) |
| Case folding | "Quarterly Report" | "quarterly report" | Case-insensitive search |
| Stop word removal | "the quarterly report for the year" | "quarterly report year" | Remove high-frequency, low-information words |
| Stemming | "quarterly reports projected" | "quarter report project" | Match morphological variants |

**CJK handling:** Chinese, Japanese, and Korean text does not use whitespace word boundaries. The tokenizer uses character-level bigram tokenization for CJK ranges (Unicode blocks CJK Unified Ideographs, Hiragana, Katakana, Hangul): each overlapping pair of CJK characters becomes a token. This is a simple but effective approach for on-device search without requiring a language-specific dictionary.

**Stop words:** A compact stop word list (~300 words for English) is built into the tokenizer. Stop words are not indexed, saving ~30% of posting list space. The stop word list is configurable per space (users working in non-English languages can substitute their own list).

**Stemming:** Stemming is optional and off by default. When enabled, it uses a simple suffix-stripping algorithm (Porter2 for English). Stemming improves recall (finding "projected" when searching "projection") but can reduce precision (conflating "university" and "universal"). Users can enable stemming per space.

### 6.4 Index Maintenance

**Synchronous updates:** The full-text index is updated on every object mutation. When an object is created or modified:

1. Extract text content (via Content Extraction pipeline, [pipeline.md §3.2](./pipeline.md))
2. Tokenize extracted text
3. For modified objects: remove old posting entries (diff against stored token set)
4. Insert new posting entries for each token
5. Update doc_count, term_doc_freq, avg_doc_length, doc_lengths

This synchronous update ensures search results are always current. The update is atomic — either all posting entries for an object are updated or none are (partial failures are rolled back).

**Deletion:** When an object is deleted, its posting entries are lazily marked (tombstoned). Tombstones are cleaned up during compaction, which runs periodically (hourly or after 10,000 tombstones accumulate).

**Compaction:** Index compaction performs three operations:

1. **Tombstone cleanup:** Remove deleted posting entries from posting lists
2. **Posting list merge:** Merge small posting lists that accumulated from incremental updates
3. **Statistics refresh:** Recompute global statistics (doc_count, avg_doc_length) from scratch to correct for accumulated floating-point drift

Compaction runs as a background operation at `Idle` scheduling class. On a system with 100,000 objects, compaction takes ~500ms and frees ~5% of index space (from tombstones and fragmentation).

### 6.5 Index Storage

The full-text index is stored in the `system/index/fulltext/` space:

```text
system/index/fulltext/
├── terms             (serialized BTreeMap<String, PostingList>)
├── metadata          (doc_count, avg_doc_length, BM25 config)
├── doc_lengths       (per-object token counts for BM25 normalization)
└── wal/              (incremental updates since last full write)
```

**Persistence strategy:** Same as the embedding index (§5.3) — incremental WAL writes with periodic full serialization. Crash recovery replays the WAL against the last checkpoint.

**Storage overhead:** The full-text index typically consumes 10-30% of the indexed text size. For 100,000 objects averaging 1 KB of extracted text (100 MB total), the index occupies ~10-30 MB. This is always resident in memory for fast query response (<50ms).

### 6.6 Phrase & Proximity Queries

Positional information in posting entries enables two advanced query types:

**Phrase queries** (`"quarterly revenue"`): Match objects where the query terms appear consecutively in order. The query engine intersects posting lists and checks that positions are adjacent:

```text
"quarterly revenue" matches if:
  positions_of("quarterly") contains P
  AND positions_of("revenue") contains P+1
  for some position P
```

**Proximity queries** (`quarterly NEAR/5 revenue`): Match objects where the query terms appear within N tokens of each other, in any order:

```text
quarterly NEAR/5 revenue matches if:
  |position_of("quarterly") - position_of("revenue")| <= 5
  for some pair of positions
```

These query types are implemented entirely within the full-text index — they require no AI inference and work regardless of AIRS availability.
