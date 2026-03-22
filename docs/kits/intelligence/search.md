# Search Kit

**Layer:** Intelligence | **Architecture:** `docs/intelligence/space-indexer.md` + 8 sub-docs

## Purpose

The Search Kit provides full-text and semantic search across all Spaces. It runs an indexing
pipeline that extracts content, generates embeddings, and builds a relationship graph of
entities and objects. Search results are fused from BM25 full-text scoring, HNSW vector
similarity, and PersonalRank graph traversal to deliver contextually relevant results.

## Key APIs

| Trait / API | Description |
|---|---|
| `SearchQuery` | Structured or natural-language query with filters, scope, and ranking hints |
| `IndexPipeline` | Async pipeline: content extraction → entity extraction → embedding → index write |
| `EmbeddingIndex` | HNSW graph with SQ8/PQ quantization; supports filtered approximate nearest-neighbor |
| `FullTextIndex` | Inverted index with BM25 scoring, CJK bigram tokenization, phrase queries |
| `RelationshipGraph` | Entity relationship store with PersonalRank traversal and cross-object discovery |

## Dependencies

- **AIRS Kit** — embedding model inference, query reranking, learned index scoring
- **Storage Kit** — Space object access, index persistence, block-level I/O
- **Compute Kit** (Tier 3) — hardware-accelerated embedding generation

## Consumers

- Conversation Kit (retrieval-augmented generation, context injection)
- Applications (in-app search UI, object lookup)
- Compositor (system-wide search surface)

## Implementation Phase

Phase 10+
