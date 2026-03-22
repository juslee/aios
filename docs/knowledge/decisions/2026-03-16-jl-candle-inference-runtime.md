---
author: Justin Lee
date: 2026-03-16
tags: [intelligence, airs, inference]
status: final
---

# ADR: candle Replaces GGML for AIRS Inference

## Context

AIRS needs a local ML inference runtime to run LLMs (Llama, Mistral, Phi) on-device. The original architecture specified GGML (C library). Should we stick with GGML or switch to candle (pure Rust, by Hugging Face)?

## Options Considered

### Option A: GGML (C library)

- Pros: Mature, widely used (llama.cpp), extensive model support, proven ARM NEON performance
- Cons: C code requires FFI, unsafe C code in kernel context, build system complexity, doesn't align with "Custom Core" principle (wrapping a C library)

### Option B: candle (pure Rust)

- Pros: Pure Rust (no C FFI, no unsafe C code), GGUF model support (same quantized formats), ARM NEON SIMD, Metal/CUDA backends for Apple Silicon and NVIDIA, natural integration with Rust build system, aligns with Custom Core principle
- Cons: Less mature than GGML, smaller community, performance may differ (benchmark needed)

## Decision

candle (Option B). Pure Rust inference runtime aligns with AIOS's Custom Core principle.

AIRS architecture unchanged — still runs existing LLMs locally. candle is the inference runtime; Intelligence Services (Space Indexer, Context Engine, etc.) remain custom Rust orchestration code. Kernel-internal ML uses tiny decision trees (not LLMs).

With the Compute Kit architecture (decided 2026-03-22), candle becomes one implementation behind the Inference Pipeline trait (Compute Kit Tier 3). The runtime is swappable — if candle's performance is insufficient at implementation time, alternatives (burn, tract, or even GGML via FFI) can be substituted without architectural changes.

Performance comparison (candle vs GGML on ARM NEON for 7B Q4) deferred to AIRS Kit implementation time. Compute Kit Tier 3 abstracts the runtime.

## Consequences

- No C FFI in the inference path
- GGUF model files still supported (candle reads them natively)
- Performance must be validated when AIRS Kit is implemented (Phase 9+)
- Compute Kit Tier 3 (Inference Pipeline) abstracts the runtime — swapping is an implementation detail
- `docs/intelligence/airs/inference.md` needs updating (GGML -> candle)
