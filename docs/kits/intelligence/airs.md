# AIRS Kit

**Layer:** Intelligence | **Architecture:** `docs/intelligence/airs.md` + 7 sub-docs

## Purpose

The AI Runtime System (AIRS) is the inference engine at the core of AIOS. It manages model
loading and lifecycle, schedules inference requests across available compute, and exposes
intelligence services (context, attention, intent, behavioral monitoring) to the rest of the
system. AIRS is the brain of AIOS — every subsystem that exhibits adaptive behavior depends
on it.

## Key APIs

| Trait / API | Description |
|---|---|
| `InferenceEngine` | Submit inference requests; returns streaming or batch outputs |
| `ModelRegistry` | Load, evict, and query available models by capability profile |
| `InferenceSession` | Scoped session with KV cache, token budget, and lifecycle hooks |
| `StreamingOutput` | Async token-by-token delivery with backpressure and cancellation |
| `InferenceMeter` | Per-session resource metering (tokens, latency, compute budget) |

## Dependencies

- **Memory Kit** — model region allocation, PagedAttention, KV cache management
- **Capability Kit** — capability-gated inference access and session isolation
- **Compute Kit** (Tier 3) — NPU/GPU dispatch for hardware-accelerated inference
- **Storage Kit** — model blob storage, registry persistence, quantized weight loading

## Consumers

- Context Kit (activity inference, context classification)
- Search Kit (embedding generation, query reranking)
- Conversation Kit (LLM inference for user-facing conversations)
- Attention Kit (notification priority scoring)
- Intent Kit (behavioral verification, anomaly detection)
- Preference Kit (NLU-driven settings parsing)
- All intelligence services across the system

## Implementation Phase

Phase 9+
