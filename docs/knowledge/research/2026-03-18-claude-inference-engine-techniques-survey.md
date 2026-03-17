---
author: claude
date: 2026-03-18
tags: [intelligence, inference, memory, scheduling, kernel]
status: final
---

# Research: LLM Inference Engine Techniques for AIOS AIRS

## Question

What are the state-of-the-art techniques in LLM inference scheduling, KV cache management, memory optimization, and request batching that should inform AIOS's AIRS inference engine design? How should each technique be categorized within the AIOS two-tier intelligence model (Kernel-Internal ML vs. AIRS-Dependent)?

This survey covers six domains:
1. Request scheduling and batching
2. KV cache management and eviction
3. Memory optimization and quantization
4. Speculative and parallel decoding
5. Disaggregated and distributed inference
6. Hardware-aware and thermal-aware scheduling

Each technique is classified into one of three AIOS categories:
- **Algorithmic**: Fixed algorithms with no learned component. Compiled into AIRS binary. Deterministic, auditable, zero runtime overhead.
- **Kernel-Internal ML**: Lightweight statistical models (EWMA, z-score, decision trees) with O(1) per-observation cost and fixed-size state. No LLM required.
- **AIRS-Dependent**: Requires the inference engine running with a loaded model, or requires substantial compute/memory for learned policies.

-----

## 1. Request Scheduling and Batching

### 1.1 Continuous Batching (Orca)

**Paper:** Yu et al., "Orca: A Distributed Serving System for Transformer-Based Generative Models," OSDI 2022.

**Technique:** Traditional batching waits for a batch to fill before processing. Orca introduces *iteration-level scheduling*: requests are added to and removed from the batch at every decode step, rather than waiting for an entire batch to complete. When one request finishes generating, a new request immediately takes its slot.

**Key insight:** In autoregressive generation, each token depends only on the previous tokens for that sequence. Different sequences in a batch can be at different stages (some in prefill, others mid-decode, others about to finish). By scheduling at the iteration level, GPU utilization stays high and tail latency drops.

**Relevance to AIOS:** Limited for single-user edge inference. Continuous batching shines when serving dozens to thousands of concurrent requests on GPU servers. On a single-user ARM device with 1-4 concurrent AIRS sessions, the complexity of iteration-level scheduling adds overhead without meaningful throughput gain. The existing priority-based serial scheduling in AIRS inference.md §3.2 is more appropriate.

**AIOS category:** Algorithmic (if adopted). Not recommended for edge deployment.

**What AIOS should take from Orca:** The *concept* of preempting a lower-priority decode mid-generation to serve a higher-priority prefill is valuable. AIRS already does this coarsely (Interactive preempts Background). Orca shows this can be done at token granularity — useful if AIOS ever serves multiple interactive users or agent conversations simultaneously.

### 1.2 Sarathi-Serve / Chunked Prefill

**Paper:** Agrawal et al., "Sarathi-Serve: Efficient LLM Serving with Chunked Prefills and Stall-Free Scheduling," 2024 (arXiv: 2308.16369, revised 2024).

**Technique:** Long prefills (processing a 4K+ token prompt) create "bubbles" — they monopolize the compute pipeline for hundreds of milliseconds, stalling all decode requests. Sarathi-Serve splits long prefills into fixed-size chunks (e.g., 512 tokens) and interleaves them with decode steps from other requests. This eliminates prefill-induced stalls and keeps time-to-first-token predictable.

**Key insight:** On GPU, a single prefill of 4096 tokens takes the same wall-clock time as 4096 decode steps but blocks all other requests. Chunking the prefill into 8 chunks of 512 tokens lets 8 decode steps from other requests execute between chunks, reducing decode latency variance by 3-5x.

**Relevance to AIOS:** Directly applicable to AIRS prefix caching (ai-native.md §14.2). When a system prompt (2048 tokens for the intent verifier) needs processing, chunked prefill prevents it from blocking an active user conversation. On ARM CPU, a 2048-token prefill of an 8B Q4 model takes 1-5 seconds — chunking this into 256-token segments with interleaved user decode steps would keep the conversation bar responsive.

**AIOS category:** Algorithmic. The chunk size is a fixed parameter (tuned per hardware tier). No learning required.

**Recommendation:** Implement chunked prefill in the compute scheduler (inference.md §3.2). Chunk size: 256 tokens on 4-core ARM, 512 tokens on 8-core. Interleave with interactive decode when prefill priority < Interactive.

### 1.3 FastServe — Preemptive Scheduling with Skip-Join MLFQ

**Paper:** Wu et al., "Fast Distributed Inference Serving for Large Language Models," 2023 (arXiv: 2305.05920).

**Technique:** FastServe applies a Multi-Level Feedback Queue (MLFQ) to LLM request scheduling. New requests start in the highest-priority queue. As they consume more compute (more tokens generated), they demote to lower-priority queues. This automatically prioritizes short requests over long ones without knowing generation length in advance.

**Key insight:** LLM output length is unpredictable. MLFQ naturally adapts — a request that generates 10 tokens stays at high priority, while a request generating 2000 tokens gradually demotes, reducing its impact on shorter requests' latency. Skip-join optimization: when a preempted request resumes, it can skip intermediate queues if its remaining time is short.

**Relevance to AIOS:** The MLFQ concept maps well to AIRS's existing 4-tier priority system (Interactive > System > Background > Batch). The difference is that FastServe's demotion is automatic based on tokens generated, while AIRS assigns priority based on requester type. A hybrid approach — start at the requester's base priority, demote within that tier after N tokens — could improve fairness between concurrent AIRS services.

**AIOS category:** Algorithmic. The queue structure and demotion thresholds are fixed parameters.

**Recommendation:** Consider for Phase 21+ when AIRS handles 4+ concurrent inference sessions. Current 1-2 session workload doesn't benefit from MLFQ complexity.

### 1.4 DistServe — Prefill-Decode Disaggregation

**Paper:** Zhong et al., "DistServe: Disaggregating Prefill and Decoding for Goodput-optimized Large Language Model Serving," OSDI 2024.

**Technique:** Prefill (processing the prompt) is compute-bound — it benefits from high-FLOPS hardware. Decode (generating tokens one at a time) is memory-bandwidth-bound — it benefits from high-bandwidth memory. DistServe runs prefill and decode on *separate* hardware pools, each optimized for its workload. Prefill workers have dense compute (many cores, high clock); decode workers have wide memory buses.

**Key insight:** On a single device, prefill and decode contend for the same resources — cache, bandwidth, compute units. Separating them lets each phase run at its hardware optimum. DistServe reports 1.5-2x throughput improvement over colocated serving.

**Relevance to AIOS:** Not directly applicable to single-device edge inference. However, the *conceptual* separation is valuable for AIOS multi-device scenarios (Phase 37+ multi-device.md). A Raspberry Pi cluster could designate one node as a "prefill worker" (processing system prompts, building KV caches) and another as a "decode worker" (generating tokens). The multi-device Space Mesh (multi-device/experience.md §4.3) already envisions intelligence continuity across devices.

**AIOS category:** Algorithmic (architecture choice, not a learned policy).

**Recommendation:** Revisit when implementing multi-device inference coordination (Phase 37+). For single-device, the compute scheduler should be aware that prefill and decode have different hardware affinities (prefill → big cores, decode → memory bandwidth).

### 1.5 S-LoRA — Serving Many LoRA Adapters Efficiently

**Paper:** Sheng et al., "S-LoRA: Serving Thousands of Concurrent LoRA Adapters," MLSys 2024.

**Technique:** When serving multiple users with personalized LoRA adapters, the naive approach loads/unloads adapters per request. S-LoRA stores all adapters in a unified memory pool with a custom CUDA kernel that applies the correct adapter per-request within a batch. It uses a memory manager inspired by PagedAttention to page adapter weights in and out.

**Key insight:** LoRA adapters are small (10-200 MB) but the switching overhead is significant. By keeping a pool of hot adapters in GPU memory and batching requests across different adapters, S-LoRA eliminates per-request adapter loading latency.

**Relevance to AIOS:** Directly applicable to on-device personalization (ai-native.md §14.6). When multiple agents have personal LoRA adapters (learned from user interaction), AIRS needs to switch between them. S-LoRA's memory management approach — pool adapters, keep hot ones resident, page cold ones — maps to the existing KvCachePool concept.

**AIOS category:** Algorithmic (memory management) with Kernel-Internal ML (LRU/frequency scoring for adapter eviction).

**Recommendation:** Integrate adapter memory management into KvCachePool (inference.md §3.3). Each loaded adapter gets an eviction score alongside KV caches. On 8 GB devices, budget ~200 MB for 2-4 resident adapters.

-----

## 2. KV Cache Management and Eviction

### 2.1 PagedAttention (vLLM)

**Paper:** Kwon et al., "Efficient Memory Management for Large Language Model Serving with PagedAttention," SOSP 2023.

**Technique:** Traditional KV cache allocation is contiguous — each sequence gets a single large buffer allocated for its maximum possible length. This wastes 60-80% of GPU memory on internal fragmentation (sequences rarely reach max length). PagedAttention borrows the virtual memory concept: KV cache is stored in fixed-size blocks (pages), allocated on demand, mapped via a page table. Non-contiguous physical blocks appear contiguous to the attention kernel.

**Key insight:** KV cache memory waste is the primary throughput bottleneck in LLM serving. By moving to paged allocation, vLLM achieves near-zero waste, increasing effective batch size and throughput by 2-4x. Blocks can be shared across sequences (copy-on-write for beam search and prefix sharing).

**Relevance to AIOS:** Highly relevant. The existing KvCachePool (inference.md §3.3) uses per-session contiguous allocation. Switching to paged allocation would:
- Reduce memory waste from ~40% to ~4% (last-block fragmentation only)
- Enable prefix sharing (§14.2) at block granularity rather than requiring separate cache copies
- Allow partial eviction (evict some blocks of a session, not all-or-nothing)

**AIOS category:** Algorithmic. The paging mechanism is a data structure choice, not a learned policy.

**Recommendation:** Adopt PagedAttention for KvCachePool in Phase 9b. This is the single highest-impact change for memory efficiency. GGML/llama.cpp added KV cache quantization and partial eviction support; paged allocation is the next step.

**Implementation sketch for AIOS:**

```rust
pub struct PagedKvCache {
    /// Fixed-size blocks (e.g., 16 tokens per block)
    block_size: u32,
    /// Free block pool
    free_blocks: Vec<BlockId>,
    /// Per-session block tables (logical block → physical block)
    session_tables: HashMap<SessionId, Vec<Option<BlockId>>>,
    /// Reference counts for shared blocks (prefix caching)
    ref_counts: HashMap<BlockId, u32>,
}
```

### 2.2 H2O — Heavy-Hitter Oracle

**Paper:** Zhang et al., "H2O: Heavy-Hitter Oracle: Efficient Generative Inference of Large Language Models with Heavy Hitters," NeurIPS 2023.

**Technique:** In transformer attention, a small fraction of tokens (5-20%) accumulate the majority of attention weight across all layers — these are "heavy hitters." H2O maintains only the recent window plus heavy-hitter tokens in KV cache, evicting low-attention tokens. This achieves 20% cache size with near-lossless quality for many tasks.

**Key insight:** Attention patterns are highly skewed. Initial tokens (BOS, system prompt prefix) and semantically important tokens (names, numbers, key concepts) receive disproportionate attention. Keeping only these plus a recent window captures 95%+ of the information.

**Relevance to AIOS:** Already referenced in ai-native.md §13.2. The current implementation tracks eviction at the *session* level. H2O-style within-session token eviction is the next step — when a session's context exceeds the model window, evict low-attention tokens rather than truncating oldest messages.

**AIOS category:** Kernel-Internal ML. Requires per-layer attention score accumulation (EWMA over attention weights), but this is a lightweight statistical tracker, not a neural model.

**State overhead:** ~64 KB per active session (attention score accumulators for all tokens × layers).

**Recommendation:** Implement as a within-session eviction policy option in Phase 21+. Essential for devices with limited RAM where context windows must be extended beyond the model's native limit.

### 2.3 StreamingLLM — Attention Sink Tokens

**Paper:** Xiao et al., "Efficient Streaming Language Models with Attention Sinks," ICLR 2024.

**Technique:** StreamingLLM observes that the first few tokens in a sequence (typically positions 0-3) act as "attention sinks" — they receive high attention weight regardless of their semantic content. This is an artifact of softmax normalization: the model learns to "dump" excess attention probability mass on early tokens. Removing these sink tokens causes catastrophic quality degradation, even if they seem semantically irrelevant.

**Key insight:** For infinite-length streaming contexts, keep the first K tokens (attention sinks, K=4 typically), discard intermediate tokens, keep the last W tokens (recent window). This yields stable perplexity over arbitrarily long contexts with fixed-size KV cache.

**Relevance to AIOS:** Critical for long-running AIRS services. The Context Engine, Behavioral Monitor, and Conversation Manager all process streaming input over extended periods. StreamingLLM's attention sink insight means these services can maintain infinite context with a fixed-size KV cache:
- Attention sinks (4 tokens): ~0.1% of cache, prevents quality collapse
- Recent window (512-2048 tokens): working memory for current processing
- Total: fixed 516-2052 tokens regardless of how long the service has been running

**AIOS category:** Algorithmic. The sink detection is a fixed rule (keep first K tokens), not a learned policy. K=4 works across all tested model architectures.

**Recommendation:** Implement attention sink preservation in the KV cache eviction path. When cache is full: never evict positions 0..K, always evict oldest non-sink tokens, keep recent window. This is simpler than H2O (no per-token attention tracking) and sufficient for streaming services.

### 2.4 Scissorhands — Token Importance via Persistence

**Paper:** Liu et al., "Scissorhands: Exploiting the Persistence of Importance Hypothesis for LLM KV Cache Compression," NeurIPS 2023.

**Technique:** Scissorhands observes that tokens important in one layer tend to be important in subsequent layers ("persistence of importance"). Rather than tracking attention scores across all layers (as in H2O), Scissorhands uses a single importance score per token, updated incrementally. Tokens below the importance threshold are evicted from KV cache.

**Key insight:** The persistence hypothesis means you don't need per-layer tracking — a single running importance score suffices. This reduces the overhead of within-session eviction from O(layers × tokens) to O(tokens).

**Relevance to AIOS:** Offers a simpler alternative to full H2O for within-session eviction. On ARM devices where per-layer attention tracking (64 KB/session) is expensive, Scissorhands' single-score approach requires only ~4 bytes per token (one float) — ~16 KB for a 4K context window.

**AIOS category:** Kernel-Internal ML. Single EWMA-style importance score per token, O(1) per observation.

**Recommendation:** Evaluate as an alternative to H2O for within-session eviction. Lower overhead, potentially less accurate for pathological attention patterns but sufficient for AIRS use cases.

### 2.5 KIVI — KV Cache Quantization

**Paper:** Liu et al., "KIVI: A Tuning-Free Asymmetric 2bit Quantization for KV Cache," 2024 (arXiv: 2402.02750).

**Technique:** KV cache entries are typically stored in FP16 (2 bytes per element). KIVI quantizes KV cache to 2-bit precision per element with per-channel asymmetric quantization, achieving 4x memory reduction with minimal quality loss. Key cache uses per-channel quantization (along the head dimension), value cache uses per-token quantization.

**Key insight:** KV cache entries are more compressible than model weights because they represent activations (narrower dynamic range). 2-bit quantization preserves enough information for attention computation. The asymmetric quantization with per-group scaling factors keeps quantization error bounded.

**Relevance to AIOS:** Transformative for edge deployment. On an 8 GB device:
- FP16 KV cache for 8B model, 8K context: ~1 GB
- 2-bit KV cache for 8B model, 8K context: ~250 MB
- Savings: ~750 MB, enough to fit a companion specialist model or extend context to 32K

**AIOS category:** Algorithmic. Quantization parameters are computed from cache statistics (min/max per channel), no learning required.

**Recommendation:** High priority for Phase 9b. Implement alongside PagedAttention. Each page block stores quantized KV entries with per-block scale/zero-point metadata.

### 2.6 KVQuant — Finer-Grained KV Quantization

**Paper:** Hooper et al., "KVQuant: Towards 10 Million Context Tokens by Quantizing the KV Cache with Non-Uniform Quantization," 2024.

**Technique:** KVQuant extends KV cache quantization with non-uniform (k-means-based) codebooks rather than uniform grids. It also introduces per-channel sensitivity-aware quantization — channels with high variance get more bits, channels with low variance get fewer. Additionally, it uses dense-and-sparse decomposition: outlier values are stored separately in full precision while the bulk is quantized to 2-3 bits.

**Key insight:** KV cache values have non-uniform distributions with heavy tails. Uniform quantization wastes bits on the dense center and loses information in the tails (outliers). Non-uniform quantization with outlier isolation achieves near-lossless quality at 2-3 bits average.

**Relevance to AIOS:** Extends KIVI's approach with better quality at the same bit-width. The outlier isolation technique is particularly valuable for AIRS's security-critical services (intent verifier, behavioral monitor) where even small quality degradations could affect safety judgments.

**AIOS category:** Algorithmic (codebook computation) with Kernel-Internal ML (online codebook adaptation from cache statistics).

**Recommendation:** Evaluate as a quality improvement over KIVI for security-critical inference paths. The codebook computation adds ~10 ms overhead per context reset, which is negligible.

### 2.7 CacheGen — KV Cache Compression for Transfer

**Paper:** Liu et al., "CacheGen: Fast Context Loading for Language Model Applications via KV Cache Streaming," 2024.

**Technique:** CacheGen compresses KV cache for storage and transfer. Rather than recomputing KV cache from the prompt (which requires a full prefill pass), CacheGen stores the computed KV cache in a compressed format and loads it directly. Uses learned codebooks per layer for compression, achieving 3-5x compression over raw KV cache.

**Key insight:** For repeated system prompts (AIRS's intent verifier, context engine), storing the precomputed KV cache is cheaper than recomputing it. But raw KV cache is large (1 GB+ for 8K tokens). CacheGen compresses it to 200-300 MB, making storage and loading practical even on SD cards.

**Relevance to AIOS:** Directly enhances prefix caching (ai-native.md §14.2). Current design stores raw KV state in `CachedPrefix.kv_state`. CacheGen-style compression would reduce storage from ~128 MB to ~30-40 MB per cached prefix, making 3-5 cached prefixes practical on 4 GB devices.

**AIOS category:** Algorithmic (compression) with AIRS-Dependent (codebook training requires inference engine for calibration).

**Recommendation:** Implement compressed prefix cache storage in Phase 21a. Train codebooks once per model at first load; apply compression for all subsequent prefix saves/loads.

### 2.8 PyramidKV / SnapKV / DMC — Layer-Aware Cache Budgets

**Papers:**
- Cai et al., "PyramidKV: Dynamic KV Cache Compression based on Pyramidal Information Funneling," 2024.
- Li et al., "SnapKV: LLM Knows What You are Looking for Before Generation," 2024.
- Nawrot et al., "Dynamic Memory Compression: Retrofitting LLMs for Accelerated Inference," 2024.

**Technique:** These papers share a key insight: different transformer layers need different KV cache sizes. Early layers have diffuse attention (need more tokens), later layers have focused attention (need fewer tokens). PyramidKV allocates cache budgets in a pyramid shape — more KV entries for early layers, fewer for later layers. SnapKV identifies which tokens are important *before* generation begins (during the prefill phase), using attention patterns from a "observation window" at the end of the prompt. DMC compresses KV cache dynamically during inference using learned compression policies.

**Key insight:** Uniform cache allocation across layers wastes memory. A 4:2:1 ratio (early:middle:late layers) can achieve the same quality as uniform allocation at ~50% total cache size.

**Relevance to AIOS:** Complementary to KIVI/KVQuant. After quantizing to 2 bits, further reduce cache by allocating non-uniformly across layers. Combined: 2-bit quantization (4x) × pyramid allocation (2x) = ~8x total cache reduction. An 8K context that normally costs 1 GB becomes ~125 MB.

**AIOS category:** Kernel-Internal ML. Per-layer attention variance tracking (EWMA, one float per layer) determines budget allocation. SnapKV's observation window analysis is Algorithmic (fixed rule applied during prefill).

**Recommendation:** Implement SnapKV-style observation window analysis in Phase 21+. During prefill, track per-token attention scores in the last 64 tokens of the prompt; use these to pre-select important tokens for cache retention. Lower complexity than full H2O, applied once during prefill rather than continuously.

-----

## 3. Memory Optimization and Quantization

### 3.1 LLM in a Flash — Flash-Aware Model Loading

**Paper:** Alizadeh et al., "LLM in a Flash: Efficient Large Language Model Inference with Limited Memory," Apple, 2023.

**Technique:** When the model exceeds available RAM, keep only hot parameters in memory and page cold parameters from flash storage (SSD/NVMe). Two key innovations: (1) *row-column bundling* — coalesce sparse reads into sequential reads by bundling neuron weights that are activated together, and (2) *predictive loading* — use the current layer's activations to predict which neurons the next layer will need, pre-fetching them before they're required.

**Key insight:** Flash storage has asymmetric characteristics — sequential reads are 10-100x faster than random reads. By organizing model weights to maximize sequential access patterns and predicting future access, effective inference throughput can match in-memory performance for models 2x larger than RAM.

**Relevance to AIOS:** Already referenced in ai-native.md §13.3 for progressive loading. The full technique extends further: AIRS could run a 14B model on an 8 GB device by keeping only the currently active layers in RAM and prefetching the next layer from flash. On NVMe (~3 GB/s sequential), a 4 GB layer prefetch takes ~1.3 seconds — too slow per-token but viable with 2-3 layers of pipeline prefetching.

**AIOS category:** Algorithmic (row-column bundling is a storage layout) with Kernel-Internal ML (predictive loading uses activation statistics to predict next-layer access patterns).

**Recommendation:** Critical for SD card devices. The existing ModelLoadTracker (ai-native.md §13.3) should be extended with sequential prefetch ordering and activation-based prediction. On SD cards (~90 MB/s), this determines whether a 4 GB model takes 45 seconds or 5 seconds to become interactive.

### 3.2 AWQ — Activation-Aware Weight Quantization

**Paper:** Lin et al., "AWQ: Activation-aware Weight Quantization for LLM Compression and Acceleration," MLSys 2024.

**Technique:** Not all weight channels are equally important. AWQ identifies the top 1% of salient channels (those that process large-magnitude activations) and protects them from quantization error by scaling them up before quantization and scaling down during inference. This simple per-channel scaling achieves quality equivalent to much higher-bit quantization with no additional compute.

**Key insight:** Quantization error in salient channels causes disproportionate output degradation. Protecting just 1% of channels with a scale factor recovers most quality loss from aggressive quantization.

**Relevance to AIOS:** AIRS already uses GGML quantization formats (Q4_K_M, Q5_K_M). AWQ's per-channel scaling is compatible with GGML's group-quantization scheme. The adaptive quantization strategy (ai-native.md §14.5) could incorporate AWQ-style salient channel identification for further quality improvement within the same memory budget.

**AIOS category:** Algorithmic. Salient channel identification is a one-time calibration (requires a small calibration dataset), not a runtime learned policy.

### 3.3 SqueezeLLM — Dense-and-Sparse Quantization

**Paper:** Kim et al., "SqueezeLLM: Dense-and-Sparse Quantization," 2023.

**Technique:** Decompose model weights into dense (low-rank, uniformly quantized) and sparse (outlier values stored in full precision) components. The sparse component is stored in CSR format and adds minimal memory. This allows the dense component to be aggressively quantized (3-4 bits) while outliers maintain full precision.

**Key insight:** Weight distributions have heavy tails. A few outlier values per tensor cause disproportionate quantization error if forced into a uniform grid. Extracting outliers to a sparse matrix and quantizing the remainder uniformly achieves 0.1-0.3 perplexity improvement over uniform quantization at the same average bit-width.

**Relevance to AIOS:** Complementary to AWQ. SqueezeLLM addresses weight outliers; AWQ addresses activation-driven saliency. Both techniques can be applied during model conversion (GGUF creation time) and require no runtime overhead.

**AIOS category:** Algorithmic. Applied at model conversion time, not runtime.

### 3.4 AQLM — Additive Quantization for LLMs

**Paper:** Egiazarian et al., "AQLM: Extreme Compression of Large Language Models via Additive Quantization," 2024.

**Technique:** AQLM applies multi-codebook quantization to weight matrices. Each weight group is represented as a sum of entries from multiple small codebooks (additive quantization). At 2 bits per parameter, AQLM significantly outperforms other methods, achieving quality competitive with 3-4 bit uniform quantization.

**Key insight:** Additive codebooks create a richer representation space than uniform grids at the same bit-width. With 2 codebooks of 256 entries each, the effective representation has 65536 distinct values per group — far more than a 2-bit uniform grid's 4 values.

**Relevance to AIOS:** Enables running 8B models in ~2 GB RAM (down from 4.5 GB at Q4_K_M). This makes 8B-class models viable on 4 GB devices. The compute overhead of codebook lookup adds ~5-10% to inference time on ARM CPU (sequential codebook reads are cache-friendly).

**AIOS category:** Algorithmic. Codebooks are computed at model conversion time.

**Recommendation:** Track GGUF support for AQLM-style codebook quantization. When available, this becomes the preferred format for 4 GB devices.

### 3.5 QLoRA — Quantized Fine-Tuning

**Paper:** Dettmers et al., "QLoRA: Efficient Finetuning of Quantized Large Language Models," NeurIPS 2023.

**Technique:** Fine-tune a quantized (4-bit) base model using LoRA adapters with paged optimizers that spill optimizer state to CPU RAM / disk when GPU memory is exhausted. Introduces the NF4 (NormalFloat4) data type — a quantization format optimized for normally-distributed weights.

**Key insight:** QLoRA demonstrates that fine-tuning a 4-bit quantized model preserves nearly all the quality of full-precision fine-tuning while reducing memory requirements by 4x. The NF4 data type is information-theoretically optimal for normally-distributed data (which neural network weights approximately follow).

**Relevance to AIOS:** Already referenced in ai-native.md §14.6. QLoRA makes on-device fine-tuning viable on 8 GB devices with 3B models. The paged optimizer concept maps to AIOS's existing PagePool-based memory management — optimizer state pages can be backed by the DMA pool and spilled to flash storage.

**AIOS category:** AIRS-Dependent. Requires inference engine for forward/backward passes during fine-tuning.

### 3.6 GPTQ — Post-Training Quantization via Optimal Brain Compression

**Paper:** Frantar et al., "GPTQ: Accurate Post-Training Quantization for Generative Pre-Trained Transformers," ICLR 2023.

**Technique:** GPTQ applies layer-wise quantization using an approximate second-order method (based on Optimal Brain Surgeon). It quantizes one weight at a time, adjusting remaining weights to compensate for the quantization error of the current weight. Processes an entire transformer layer in minutes on a single GPU.

**Key insight:** By accounting for inter-weight correlations during quantization, GPTQ achieves significantly better quality than round-to-nearest quantization. The layer-wise decomposition makes it tractable for billion-parameter models.

**Relevance to AIOS:** GPTQ is a model preparation technique, not a runtime technique. GGUF files created with GPTQ-based calibration are already supported by llama.cpp. AIRS benefits indirectly — better-quantized models in the model registry.

**AIOS category:** Algorithmic. Applied at model conversion time.

-----

## 4. Speculative and Parallel Decoding

### 4.1 Speculative Decoding (Leviathan / Chen)

**Papers:**
- Leviathan et al., "Fast Inference from Transformers via Speculative Decoding," ICML 2023.
- Chen et al., "Accelerating Large Language Model Decoding with Speculative Sampling," DeepMind, 2023.

**Technique:** A small "draft" model generates K candidate tokens. The large "target" model verifies all K candidates in a single forward pass. A modified rejection sampling scheme guarantees the output distribution is *identical* to the target model alone — zero quality loss. Accepted tokens skip separate forward passes; rejected tokens trigger a correction sample.

**Key insight:** Verification of K tokens costs the same as generating 1 token (both are memory-bandwidth-bound, loading the same model weights). If the draft model's acceptance rate is α, speedup is approximately K × α. With K=5 and α=70%, speedup is ~3.5x.

**Relevance to AIOS:** Already detailed in ai-native.md §14.1. The three strategies (n-gram, Medusa, separate draft) are correctly categorized by memory tier. On ARM CPU, the bottleneck shifts: verification of K tokens is not free because the CPU must also compute the draft tokens' attention, making effective speedup 1.3-1.8x rather than the theoretical 3.5x on GPU.

**AIOS category:** Algorithmic (n-gram draft), AIRS-Dependent (Medusa heads, separate draft model).

### 4.2 Medusa — Self-Speculative Decoding

**Paper:** Cai et al., "Medusa: Simple LLM Inference Acceleration Framework with Multiple Decoding Heads," ICML 2024.

**Technique:** Instead of a separate draft model, Medusa adds lightweight prediction heads on top of the target model's hidden states. Each head predicts a token at a different future position. A tree-based attention structure allows verifying multiple speculation paths simultaneously. Heads are trained on the target model's own outputs (self-distillation).

**Key insight:** Medusa eliminates the need for a separate draft model and its associated memory. The prediction heads add only 10-50 MB (vs. 200+ MB for a separate draft). Tree attention enables exploring multiple speculation branches without proportional compute increase.

**Relevance to AIOS:** Already referenced in ai-native.md §14.1. The key decision for AIOS is whether Medusa heads are available for the user's chosen model. Medusa requires fine-tuned heads per model architecture — they're not transferable between model families.

**AIOS category:** AIRS-Dependent. Requires model-specific trained heads and modified inference loop.

### 4.3 EAGLE / EAGLE-2 — Feature-Level Speculation

**Papers:**
- Li et al., "EAGLE: Speculative Sampling Requires Rethinking Feature Uncertainty," ICML 2024.
- Li et al., "EAGLE-2: Faster Inference of Language Models with Dynamic Draft Trees," 2024.

**Technique:** EAGLE uses the target model's hidden features (not token logits) to drive speculation. A lightweight "auto-regression head" predicts the next token's feature vector from the current feature, then converts to token logits. EAGLE-2 adds dynamic draft trees — adjusting the speculation tree shape based on the draft model's confidence per token.

**Key insight:** Feature-level prediction is more accurate than token-level prediction because features contain richer information (the model's internal representation) than the compressed logit vector. Acceptance rates reach 80-90%, significantly higher than token-level drafting (~70%). EAGLE-2's dynamic trees avoid wasting compute on low-confidence branches.

**Relevance to AIOS:** EAGLE achieves the highest acceptance rates of any speculative method. On ARM CPU, the feature-level auto-regression head is small (~20 MB) and fast. EAGLE-2's dynamic trees are particularly valuable for edge devices where every wasted forward pass costs 100+ ms.

**AIOS category:** AIRS-Dependent. Requires trained auto-regression head and modified inference pipeline.

**Recommendation:** Prefer EAGLE over Medusa for AIOS when heads are available. Higher acceptance rate = more tokens per verify pass = greater benefit on slow ARM hardware.

### 4.4 Lookahead Decoding

**Paper:** Fu et al., "Break the Sequential Dependency of LLM Inference Using Lookahead Decoding," 2024.

**Technique:** Lookahead decoding breaks the sequential dependency of autoregressive generation without a draft model. It maintains a window of Jacobi iterations — treating next-token prediction as a fixed-point equation and iterating toward convergence. Multiple future tokens are "guessed" and iteratively refined.

**Key insight:** With enough parallel compute, Jacobi iterations converge to the same tokens as sequential decoding. On GPU, this translates to generating multiple tokens per wall-clock step by parallelizing the fixed-point iteration across future positions.

**Relevance to AIOS:** Limited on ARM CPU. Lookahead decoding requires parallel compute to be effective — each iteration processes multiple positions simultaneously. On a 4-core ARM CPU, the parallelism is insufficient to outperform sequential decoding. Potentially relevant for GPU-equipped AIOS devices.

**AIOS category:** Algorithmic. No learning required.

**Recommendation:** Not recommended for ARM CPU edge deployment. Revisit for GPU-equipped devices.

### 4.5 Parallel Decoding with Jacobi Iteration (CLLM)

**Paper:** Kou et al., "CLLMs: Consistency Large Language Models," 2024.

**Technique:** Trains the model to be a "consistency model" — one that converges to the correct output in fewer Jacobi iterations. A standard LLM might need 10-20 Jacobi iterations to converge; a consistency-trained LLM converges in 2-3 iterations, producing 3-4 tokens per iteration.

**Key insight:** Training the model for Jacobi convergence is more efficient than training separate draft models. The consistency training objective directly optimizes for parallel decodability.

**Relevance to AIOS:** Interesting but requires model-level training changes. AIOS uses third-party models (llama.cpp ecosystem). Unless consistency-trained models become available in GGUF format, this technique is not applicable.

**AIOS category:** AIRS-Dependent (requires specially trained models).

-----

## 5. Disaggregated and Distributed Inference

### 5.1 Splitwise — Prefill/Decode Separation on Heterogeneous Hardware

**Paper:** Patel et al., "Splitwise: Efficient Generative LLM Inference Using Phase Splitting," ISCA 2024.

**Technique:** Similar to DistServe (§1.4), Splitwise separates prefill and decode phases but focuses on heterogeneous hardware: prefill on compute-optimized machines, decode on memory-bandwidth-optimized machines. It adds a KV cache transfer mechanism between machines and a scheduler that dynamically assigns workloads based on current load.

**Relevance to AIOS:** Same as DistServe — relevant for multi-device scenarios (Phase 37+). The KV cache transfer mechanism is directly applicable to AIOS's multi-device intelligence continuity (multi-device/experience.md §4.4).

**AIOS category:** Algorithmic (architecture) with Kernel-Internal ML (load-based dynamic assignment uses utilization EWMA).

### 5.2 PowerInfer — Neuron-Aware Sparse Inference

**Paper:** Song et al., "PowerInfer: Fast Large Language Model Serving with a Consumer-grade GPU," 2023.

**Technique:** In large FFN layers, only ~10% of neurons are activated per token (the rest produce near-zero outputs). PowerInfer identifies "hot neurons" (frequently activated across many inputs) and keeps them resident in GPU memory. Cold neurons are loaded from CPU memory on demand. A neuron-aware scheduling engine routes computation between GPU (hot neurons) and CPU (cold neurons).

**Key insight:** The activation sparsity of LLM FFN layers creates a natural partition between frequently and rarely used weights. By profiling activation patterns offline, PowerInfer pre-computes which neurons are hot and organizes memory accordingly. This enables running a 70B model on a consumer GPU with 24 GB VRAM (keeping ~7B hot neurons in GPU, loading cold neurons from 64 GB CPU RAM).

**Relevance to AIOS:** Partially applicable. AIOS runs on ARM CPU without discrete GPU, but the *concept* of hot/cold neuron partitioning applies to flash-memory offloading. Hot neurons stay in RAM; cold neurons are paged from NVMe/SD. This extends LLM in a Flash (§3.1) with profiled access patterns rather than just sequential prefetching.

**AIOS category:** Kernel-Internal ML. Neuron activation profiling uses frequency counters (one counter per neuron, updated per token), similar to the existing EWMA-based predictors.

**Recommendation:** Investigate for enabling 14B+ models on 8 GB devices via hot/cold neuron partitioning to flash. State overhead: ~1 byte per neuron × ~1 billion neurons for a 7B model = ~1 GB profiling data — too large for runtime profiling but feasible as a one-time offline step stored in the GGUF metadata.

### 5.3 FlexGen — Throughput-Optimized Offloading

**Paper:** Sheng et al., "FlexGen: High-Throughput Generative Inference of Large Language Models with a Single GPU," ICML 2023.

**Technique:** FlexGen optimizes for *throughput* (tokens per second across all requests) rather than *latency* (time to first token for one request). It computes an optimal offloading strategy: how to partition model weights, KV cache, and activations across GPU, CPU, and disk to maximize throughput for a given batch size. Uses linear programming to find the optimal strategy.

**Key insight:** For batch workloads (AIRS background indexing, summarization), throughput matters more than latency. FlexGen's LP-based strategy can find non-obvious configurations — e.g., keeping attention weights on GPU, FFN weights on CPU, and KV cache on disk with aggressive compression.

**Relevance to AIOS:** Applicable to AIRS Batch-priority workloads. When the Space Indexer needs to process 1000 documents, throughput optimization is more valuable than latency optimization. The LP solver could run once at the start of a batch job to determine optimal memory partitioning.

**AIOS category:** Algorithmic (LP solver) with Kernel-Internal ML (throughput measurements feed back into cost model).

**Recommendation:** Consider for Phase 10+ Space Indexer batch processing. The LP formulation is lightweight and produces a static strategy per batch job.

### 5.4 Mooncake — KV Cache-Centric Disaggregation

**Paper:** Qin et al., "Mooncake: A KVCache-Centric Disaggregated Architecture for LLM Serving," 2024.

**Technique:** Mooncake treats KV cache as a first-class distributed resource. It separates KV cache storage from compute, placing KV caches in a distributed cache layer that multiple compute nodes can access. Prefill nodes write KV cache to the distributed store; decode nodes read from it. This eliminates KV cache transfer between machines and enables cache reuse across requests.

**Key insight:** In multi-tenant serving, the same prompt prefix (system prompt, few-shot examples) appears across many requests. Storing the computed KV cache centrally and sharing it is more efficient than recomputing or transferring per-request.

**Relevance to AIOS:** Maps to multi-device prefix sharing. When multiple AIOS devices in a fleet share system prompts (enterprise fleet with common security policy), a hub device could maintain a KV cache store that satellite devices query. Reduces per-device prefill compute.

**AIOS category:** Algorithmic (architecture design for multi-device).

**Recommendation:** Revisit for Phase 37+ enterprise fleet scenarios.

-----

## 6. Hardware-Aware and Thermal-Aware Scheduling

### 6.1 Roofline Model Applied to LLM Inference

**Paper:** Williams et al., "Roofline: An Insightful Visual Performance Model for Multicore Architectures," CACM 2009. Applied to LLM inference in numerous subsequent works.

**Technique:** The roofline model characterizes a workload by its arithmetic intensity (FLOPS per byte of memory accessed). LLM decode is memory-bandwidth-bound (arithmetic intensity < 1 on most hardware), meaning throughput is limited by memory bandwidth, not compute capability. Prefill is compute-bound for short sequences and bandwidth-bound for long sequences.

**Key insight:** tokens/second ≈ memory_bandwidth / model_size for decode. This simple formula predicts throughput within 20% on most hardware and determines the hardware bottleneck without benchmarking.

**Relevance to AIOS:** Already implemented in ai-native.md §13.1. The roofline model with EWMA correction is the foundation of AIRS's latency prediction.

**AIOS category:** Kernel-Internal ML (roofline base + EWMA correction). Already implemented.

### 6.2 Thermal-Aware Inference Scheduling

No single canonical paper; synthesized from multiple sources including the ARM DynamIQ technical reference manual, Pi 5 thermal design documentation, and the DVFS literature.

**Technique:** ARM big.LITTLE / DynamIQ architectures have heterogeneous cores with different power-performance characteristics. Under thermal pressure, migrating inference from performance cores to efficiency cores (or reducing DVFS state) prevents the 40-50% throughput collapse that hardware thermal throttling causes. Proactive thermal management (reducing load *before* reaching throttle temperature) maintains higher sustained throughput.

**Key insight:** On a passively cooled Pi 5, sustained 8B Q4 inference hits thermal throttle (~85C) within 30-60 seconds, reducing throughput from 5 tok/s to 2-3 tok/s. Proactive throttle management (reduce to 80% compute at 75C) maintains 4 tok/s sustained — higher average throughput than unmanaged burst-and-throttle.

**Relevance to AIOS:** Already implemented in ai-native.md §13.4. The ThermalPredictor with dT/dt EWMA and 4-state classification (Normal/Fair/Serious/Critical) handles this correctly.

**AIOS category:** Kernel-Internal ML. Already implemented.

### 6.3 Learned Batch Sizing

**Technique:** Rather than fixed batch sizes, adjust the batch size per iteration based on current memory availability, thermal state, and queue depth. A simple controller: increase batch size when GPU utilization is below target, decrease when memory pressure or thermal state exceeds thresholds.

**Key insight:** On edge devices with dynamic memory pressure (other system services competing for RAM), a fixed batch size leads to either underutilization or OOM. Adaptive sizing maintains high utilization without exceeding memory bounds.

**Relevance to AIOS:** Applicable to AIRS batch workloads (Space Indexer, metadata generation). The batch size for embedding computation should adapt to current memory pressure from the FrameAllocator's MemoryPressure signal.

**AIOS category:** Kernel-Internal ML. Simple control loop using memory pressure signal from existing infrastructure (memory.md §8).

**Recommendation:** Implement a reactive batch sizer for Space Indexer:
- If MemoryPressure::None: batch size = max (32 documents)
- If MemoryPressure::Low: batch size = 16
- If MemoryPressure::Medium: batch size = 4
- If MemoryPressure::Critical: suspend batch work

-----

## 7. Additional Techniques

### 7.1 Outlines / GBNF — Constrained Decoding

**Paper:** Willard & Louf, "Efficient Guided Generation for LLMs," 2023.

Already detailed in ai-native.md §14.3. GBNF grammar compilation into DFA with per-state token masks. Production-ready in llama.cpp.

**AIOS category:** Algorithmic.

### 7.2 GQA/MQA — Grouped/Multi-Query Attention

**Papers:**
- Shazeer, "Fast Transformer Decoding: One Write-Head is All You Need," 2019 (MQA).
- Ainslie et al., "GQA: Training Generalized Multi-Query Transformer Models from Multi-Head Checkpoints," EMNLP 2023.

**Technique:** Reduce KV cache size by sharing key/value heads across multiple query heads. MQA uses a single KV head for all query heads (maximum compression, some quality loss). GQA groups query heads into G groups, each sharing one KV head (tunable compression/quality tradeoff). Modern models (Llama 2 70B, Llama 3, Mistral) use GQA natively.

**Key insight:** KV cache size scales linearly with the number of KV heads. GQA with 8 groups vs. 32 full heads reduces KV cache by 4x with minimal quality impact.

**Relevance to AIOS:** Not a runtime technique — GQA/MQA is a model architecture choice. AIOS benefits automatically when using models that employ GQA (Llama 3, Mistral). The model registry (model-registry.md §4.1) should track `kv_heads` as a capability field to accurately predict KV cache memory.

**AIOS category:** Algorithmic (model architecture). No runtime implementation needed.

### 7.3 Flash Attention / Flash Decoding

**Papers:**
- Dao et al., "FlashAttention: Fast and Memory-Efficient Exact Attention with IO-Awareness," NeurIPS 2022.
- Dao et al., "FlashAttention-2: Faster Attention with Better Parallelism and Work Partitioning," ICLR 2024.
- Dao et al., "Flash-Decoding for Long-Context LLMs," 2023 (blog post).

**Technique:** FlashAttention computes exact attention without materializing the full N×N attention matrix in HBM. By tiling the computation and using the online softmax trick, it reduces memory from O(N^2) to O(N) and improves wall-clock speed by reducing HBM reads. Flash-Decoding extends this to the decode phase, parallelizing across the KV cache length.

**Key insight:** Attention computation is IO-bound on modern hardware. By restructuring the algorithm to minimize reads from slow HBM (exploiting fast SRAM/registers), FlashAttention achieves 2-4x speedup with numerically identical results.

**Relevance to AIOS:** FlashAttention is implemented in llama.cpp's CPU attention kernel (using tiled computation with NEON SIMD). AIOS benefits automatically via the GGML runtime. Flash-Decoding's KV-length parallelism maps to multi-threaded decode on ARM's 4 cores.

**AIOS category:** Algorithmic. Already available via GGML.

### 7.4 Ring Attention — Distributed Long Context

**Paper:** Liu et al., "Ring Attention with Blockwise Transformers for Near-Infinite Context," ICLR 2024.

**Technique:** Distribute attention computation across multiple devices in a ring topology. Each device holds a segment of the KV cache and passes key/value blocks to the next device in a ring. Overlaps computation and communication to hide transfer latency.

**Relevance to AIOS:** Applicable to multi-device long-context scenarios (Phase 37+). A 3-device AIOS cluster could each hold 1/3 of a 128K-token KV cache and compute attention in a ring, enabling context lengths 3x beyond any single device's capacity.

**AIOS category:** Algorithmic (architecture for multi-device).

### 7.5 Mixture-of-Experts Inference

**Papers:**
- Fedus et al., "Switch Transformers: Scaling to Trillion Parameter Models with Simple and Efficient Sparsity," JMLR 2022.
- Jiang et al., "Mixtral of Experts," 2024.

**Technique:** MoE models activate only K out of N experts per token. A gating network selects experts based on input features. Inference cost scales with K (active experts) not N (total experts), enabling larger models at the same compute budget.

**Key insight:** Expert offloading — keep inactive experts on disk, load on demand — enables running MoE models larger than RAM. The gating network's output reveals which experts are needed 1-2 layers ahead, enabling prefetching.

**Relevance to AIOS:** Already discussed in ai-native.md §15.2. MoE with expert offloading is a natural fit for AIOS: Mixtral 8x7B activates ~13B parameters per token, fitting in an 8B model's compute budget, but the full 47B must be on disk. With NVMe prefetching, expert swap latency can be hidden.

**AIOS category:** Algorithmic (gating) with Kernel-Internal ML (expert access frequency tracking for LRU cache sizing).

### 7.6 Prompt Compression — LLMLingua / LongLLMLingua

**Papers:**
- Jiang et al., "LLMLingua: Compressing Prompts for Accelerated Inference of Large Language Models," EMNLP 2023.
- Jiang et al., "LongLLMLingua: Accelerating and Enhancing LLMs in Long Context Scenarios via Prompt Compression," 2024.

**Technique:** Reduce prompt length (and thus prefill cost and KV cache size) by removing low-information tokens. Uses a small model to score each token's importance (perplexity contribution), then removes tokens with minimal impact. Achieves 2-10x compression with minimal quality loss.

**Key insight:** Natural language prompts contain significant redundancy. Removing "the", "is", "a" and low-information phrases from a 4K-token prompt can compress it to 400-2000 tokens with equivalent model understanding.

**Relevance to AIOS:** Valuable for AIRS services with long, repetitive system prompts. The intent verifier's security instructions, context engine's classification schema, and attention manager's triage rules are verbose natural language. Compressing them reduces prefill cost and KV cache by 2-5x.

**AIOS category:** AIRS-Dependent. Requires a small model to score token importance (the embedding companion model could serve double duty).

**Recommendation:** Evaluate for Phase 21+. If system prompt compression saves 1-3 seconds of prefill and 50-200 MB of KV cache per AIRS service, the cost of running the compression model (5-50 ms on the embedding model) is negligible.

-----

## 8. Summary: AIOS Categorization Matrix

### Algorithmic (Fixed, Deterministic)

| Technique | Impact | Priority | Target Phase |
|---|---|---|---|
| PagedAttention (§2.1) | 2-4x memory efficiency | **Critical** | 9b |
| Chunked Prefill (§1.2) | Eliminates prefill stalls | **High** | 9b |
| KIVI KV quantization (§2.5) | 4x KV cache compression | **High** | 9b |
| StreamingLLM sinks (§2.3) | Infinite streaming context | **High** | 9c |
| Constrained decoding (§7.1) | 100% valid structured output | **High** | 9d |
| Flash Attention (§7.3) | 2-4x attention speedup | **Available** | via GGML |
| GQA/MQA (§7.2) | Reduced KV per model | **Available** | via model arch |
| N-gram speculative (§4.1) | 1.1-1.3x decode speed | **Medium** | 9c |
| Continuous batching (§1.1) | Multi-session throughput | **Low** (edge) | 21+ |
| MLFQ scheduling (§1.3) | Fairness under contention | **Low** (edge) | 21+ |
| Row-column bundling (§3.1) | Sequential flash reads | **Medium** | 9b |

### Kernel-Internal ML (Lightweight, No LLM)

| Technique | State Overhead | Update Cost | Target Phase |
|---|---|---|---|
| Roofline + EWMA predictor (§6.1) | 200 B | O(1)/inference | **Implemented** |
| Thermal dT/dt predictor (§6.2) | 100 B | O(1)/5s | **Implemented** |
| KV cache eviction scoring (§2.2) | 64 B/session | O(1)/check | **Implemented** |
| Scissorhands importance (§2.4) | 4 B/token | O(1)/token | 21+ |
| SnapKV layer budgets (§2.8) | 4 B/layer | O(1)/prefill | 21+ |
| H2O attention tracking (§2.2) | 64 KB/session | O(1)/token | 21+ |
| Adaptive batch sizing (§6.3) | 16 B | O(1)/batch | 10+ |
| Neuron frequency profiling (§5.2) | N/A (offline) | N/A | 21+ |
| Utilization prediction (§6.1) | 12 B/device | O(1)/100ms | **Implemented** |

### AIRS-Dependent (Requires Inference Engine)

| Technique | Memory Cost | Benefit | Target Phase |
|---|---|---|---|
| Prefix caching (§1.2, §2.7) | 30-640 MB | Skip repeated prefills | 9d |
| Speculative decoding — Medusa (§4.2) | 10-50 MB | 1.5-2.5x decode | 21+ |
| Speculative decoding — EAGLE (§4.3) | 20 MB | 1.5-3x decode | 21+ |
| Prompt compression (§7.6) | ~23 MB (embed model) | 2-5x prompt reduction | 21+ |
| On-device LoRA (§3.5) | 800 MB-2 GB | Personalization | 21+ |
| Adapter management — S-LoRA (§1.5) | ~200 MB pool | Multi-adapter switching | 21+ |
| CacheGen compressed prefixes (§2.7) | 30-40 MB/prefix | 3-5x prefix storage reduction | 21a |
| KVQuant non-uniform (§2.6) | ~1 MB codebook | Better quality at 2-bit | 21+ |
| Model distillation (§14.8 existing) | 50 MB pairs | Reduce cloud dependency | 21+ |

-----

## 9. Gaps in Current AIOS Design

Based on this survey, the following techniques represent significant gaps in the current AIRS design (ai-native.md):

### 9.1 Missing: PagedAttention for KV Cache

The current `KvCachePool` uses per-session contiguous allocation. PagedAttention would be the single highest-impact improvement — it reduces memory waste from ~40% to ~4%, enables block-level prefix sharing, and allows partial eviction. This should be a Phase 9b priority.

### 9.2 Missing: KV Cache Quantization

The current design stores KV cache in full precision. KIVI-style 2-bit quantization would provide 4x cache compression, making the difference between 4K and 16K context on an 8 GB device. This is complementary to PagedAttention — quantize each page block.

### 9.3 Missing: Chunked Prefill

The current compute scheduler has no mechanism for chunking long prefills. A 2048-token system prompt blocks all decode for 1-5 seconds. Chunked prefill with 256-token segments and interleaved decode would keep the conversation bar responsive during system service initialization.

### 9.4 Missing: StreamingLLM Attention Sinks

The current KV eviction is LRU-based. StreamingLLM's attention sink preservation (keep first 4 tokens always) is a zero-cost addition that prevents quality collapse in long-running streaming services (Context Engine, Behavioral Monitor).

### 9.5 Missing: Layer-Aware KV Budget

The current design allocates KV uniformly across layers. SnapKV/PyramidKV-style pyramidal allocation could reduce cache by an additional 50% on top of quantization.

### 9.6 Weak: Within-Session Token Eviction

The current design mentions H2O-style within-session eviction as a "future optimization" (ai-native.md §13.2). Based on this survey, Scissorhands' persistence-of-importance approach offers a lower-overhead alternative (4 B/token vs. 64 KB/session) that should be the default, with full H2O as an optional upgrade for sessions where quality is critical.

### 9.7 Missing: CacheGen-Style Prefix Compression

The current `CachedPrefix` stores raw KV state. CacheGen compression would reduce storage by 3-5x, making prefix caching viable on 4 GB devices.

-----

## 10. Recommended Implementation Roadmap

### Phase 9b: Foundation (Memory Efficiency)
1. PagedAttention for KvCachePool
2. KIVI 2-bit KV quantization per page block
3. Chunked prefill in compute scheduler
4. StreamingLLM attention sink preservation

### Phase 9c-9d: Decode Optimization
5. N-gram speculative decoding (zero memory cost)
6. GBNF constrained decoding (already planned)
7. Prefix caching with raw KV state storage

### Phase 10+: Batch Intelligence
8. Adaptive batch sizing for Space Indexer
9. FlexGen-style offloading strategy for batch workloads

### Phase 21+: Advanced Optimization
10. CacheGen compressed prefix storage
11. SnapKV pyramidal layer budgets
12. Scissorhands within-session token eviction
13. EAGLE speculative decoding (if heads available)
14. S-LoRA adapter memory management
15. Prompt compression for system prompts

### Phase 37+: Multi-Device
16. DistServe prefill/decode disaggregation
17. Ring Attention for distributed long context
18. Mooncake-style distributed KV cache

-----

## References

Cited in order of first appearance in this document:

1. Yu et al. "Orca: A Distributed Serving System for Transformer-Based Generative Models." OSDI 2022.
2. Agrawal et al. "Sarathi-Serve: Efficient LLM Serving with Chunked Prefills and Stall-Free Scheduling." 2024.
3. Wu et al. "Fast Distributed Inference Serving for Large Language Models." (FastServe) 2023.
4. Zhong et al. "DistServe: Disaggregating Prefill and Decoding for Goodput-optimized Large Language Model Serving." OSDI 2024.
5. Sheng et al. "S-LoRA: Serving Thousands of Concurrent LoRA Adapters." MLSys 2024.
6. Kwon et al. "Efficient Memory Management for Large Language Model Serving with PagedAttention." (vLLM) SOSP 2023.
7. Zhang et al. "H2O: Heavy-Hitter Oracle: Efficient Generative Inference of Large Language Models with Heavy Hitters." NeurIPS 2023.
8. Xiao et al. "Efficient Streaming Language Models with Attention Sinks." (StreamingLLM) ICLR 2024.
9. Liu et al. "Scissorhands: Exploiting the Persistence of Importance Hypothesis for LLM KV Cache Compression." NeurIPS 2023.
10. Liu et al. "KIVI: A Tuning-Free Asymmetric 2bit Quantization for KV Cache." 2024.
11. Hooper et al. "KVQuant: Towards 10 Million Context Tokens by Quantizing the KV Cache with Non-Uniform Quantization." 2024.
12. Liu et al. "CacheGen: Fast Context Loading for Language Model Applications via KV Cache Streaming." 2024.
13. Cai et al. "PyramidKV: Dynamic KV Cache Compression based on Pyramidal Information Funneling." 2024.
14. Li et al. "SnapKV: LLM Knows What You are Looking for Before Generation." 2024.
15. Nawrot et al. "Dynamic Memory Compression: Retrofitting LLMs for Accelerated Inference." (DMC) 2024.
16. Alizadeh et al. "LLM in a Flash: Efficient Large Language Model Inference with Limited Memory." Apple, 2023.
17. Lin et al. "AWQ: Activation-aware Weight Quantization for LLM Compression and Acceleration." MLSys 2024.
18. Kim et al. "SqueezeLLM: Dense-and-Sparse Quantization." 2023.
19. Egiazarian et al. "AQLM: Extreme Compression of Large Language Models via Additive Quantization." 2024.
20. Dettmers et al. "QLoRA: Efficient Finetuning of Quantized Large Language Models." NeurIPS 2023.
21. Frantar et al. "GPTQ: Accurate Post-Training Quantization for Generative Pre-Trained Transformers." ICLR 2023.
22. Leviathan et al. "Fast Inference from Transformers via Speculative Decoding." ICML 2023.
23. Chen et al. "Accelerating Large Language Model Decoding with Speculative Sampling." DeepMind, 2023.
24. Cai et al. "Medusa: Simple LLM Inference Acceleration Framework with Multiple Decoding Heads." ICML 2024.
25. Li et al. "EAGLE: Speculative Sampling Requires Rethinking Feature Uncertainty." ICML 2024.
26. Li et al. "EAGLE-2: Faster Inference of Language Models with Dynamic Draft Trees." 2024.
27. Fu et al. "Break the Sequential Dependency of LLM Inference Using Lookahead Decoding." 2024.
28. Kou et al. "CLLMs: Consistency Large Language Models." 2024.
29. Patel et al. "Splitwise: Efficient Generative LLM Inference Using Phase Splitting." ISCA 2024.
30. Song et al. "PowerInfer: Fast Large Language Model Serving with a Consumer-grade GPU." 2023.
31. Sheng et al. "FlexGen: High-Throughput Generative Inference of Large Language Models with a Single GPU." ICML 2023.
32. Qin et al. "Mooncake: A KVCache-Centric Disaggregated Architecture for LLM Serving." 2024.
33. Williams et al. "Roofline: An Insightful Visual Performance Model for Multicore Architectures." CACM 2009.
34. Willard & Louf. "Efficient Guided Generation for LLMs." (Outlines) 2023.
35. Shazeer. "Fast Transformer Decoding: One Write-Head is All You Need." (MQA) 2019.
36. Ainslie et al. "GQA: Training Generalized Multi-Query Transformer Models from Multi-Head Checkpoints." EMNLP 2023.
37. Dao et al. "FlashAttention: Fast and Memory-Efficient Exact Attention with IO-Awareness." NeurIPS 2022.
38. Dao et al. "FlashAttention-2: Faster Attention with Better Parallelism and Work Partitioning." ICLR 2024.
39. Liu et al. "Ring Attention with Blockwise Transformers for Near-Infinite Context." ICLR 2024.
40. Fedus et al. "Switch Transformers: Scaling to Trillion Parameter Models with Simple and Efficient Sparsity." JMLR 2022.
41. Jiang et al. "Mixtral of Experts." 2024.
42. Jiang et al. "LLMLingua: Compressing Prompts for Accelerated Inference of Large Language Models." EMNLP 2023.
43. Jiang et al. "LongLLMLingua: Accelerating and Enhancing LLMs in Long Context Scenarios via Prompt Compression." 2024.
44. Hu et al. "LoRA: Low-Rank Adaptation of Large Language Models." ICLR 2022.
45. Lewis et al. "Retrieval-Augmented Generation for Knowledge-Intensive NLP Tasks." NeurIPS 2020.

-----

## Implications for AIOS

### Critical Path Items (Phase 9)

The four highest-impact techniques for AIOS's inference engine are:

1. **PagedAttention** — eliminates 40% memory waste, enables prefix sharing and partial eviction
2. **KIVI KV quantization** — 4x cache compression, extends context from 4K to 16K on 8 GB
3. **Chunked prefill** — prevents 1-5 second stalls during system prompt processing
4. **StreamingLLM sinks** — zero-cost quality preservation for long-running services

Combined impact on an 8 GB device:
- Current: 8B Q4 model (4.5 GB) + 1 GB KV cache (FP16, ~4K context) + 2.5 GB OS
- Optimized: 8B Q4 model (4.5 GB) + 250 MB KV cache (2-bit, ~16K context) + 3.25 GB OS headroom

### Architecture Alignment

All recommended techniques fit within the existing AIOS two-tier intelligence model:
- Algorithmic techniques (PagedAttention, chunked prefill, KIVI, sinks) require no model changes
- Kernel-Internal ML additions (Scissorhands, SnapKV, batch sizing) are EWMA/counter-based with < 1 KB state
- AIRS-Dependent techniques (EAGLE, CacheGen, prompt compression) activate only with loaded model

The compute scheduler (inference.md §3.2), KvCachePool (inference.md §3.3), and ThermalPredictor (ai-native.md §13.4) provide the integration points. No architectural changes to AIRS are needed — these are refinements within existing subsystems.
