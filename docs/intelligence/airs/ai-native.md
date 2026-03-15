# AIOS AIRS AI-Native Intelligence

Part of: [airs.md](../airs.md) — AI Runtime Service
**Related:** [inference.md](./inference.md) — Inference engine, [model-registry.md](./model-registry.md) — Model registry, [scaling.md](./scaling.md) — Hardware scaling

-----

AIRS intelligence operates at two tiers. **Kernel-internal ML** (§13) uses lightweight statistical models — decision trees, EWMA counters, z-score detectors — that run with fixed-size state and O(1) per observation. These work without any LLM, even on 2 GB devices with no model loaded. **AIRS-dependent intelligence** (§14) requires the inference engine and a loaded model, enabling advanced techniques like speculative decoding, constrained generation, and retrieval-augmented generation.

This two-tier design ensures the OS gets progressively smarter as hardware capability increases, without creating hard dependencies on LLM availability.

-----

## 13. Kernel-Internal ML

These techniques require no LLM inference and no AIRS runtime. They are compiled into the AIRS binary (or kernel, where noted) as fixed-size statistical models. Total state overhead: under 55 KB across all six subsystems.

### 13.1 Inference Latency Prediction

Transformer decode is memory-bandwidth-bound on ARM hardware — the bottleneck is loading model weights from DRAM, not computing matrix multiplications. The roofline model (Williams et al., 2009) gives a first-order predictor:

```text
tokens/second ≈ memory_bandwidth_GB/s / model_size_GB
```

On a Cortex-A76 (Pi 5) with ~18 GB/s sustained LPDDR4X bandwidth, a 7B Q4_K_M model (~3.5 GB effective) yields ~5 tok/s. This simple formula is within 20% of measured llama.cpp performance.

AIRS maintains a runtime-calibrated predictor that corrects the roofline estimate using an EWMA (exponentially weighted moving average) of observed inference latency:

```rust
pub struct InferencePredictor {
    /// Roofline estimate (computed once per model load)
    roofline_tps: f32,
    /// EWMA of actual tokens/second (updated every inference)
    observed_tps_ewma: f32,
    /// EWMA of time-to-first-token (prefill latency)
    ttft_ewma_us: f32,
    /// Current memory pressure factor (0.0 = no pressure, 1.0 = severe)
    memory_pressure: f32,
    /// Correction factor: observed / roofline (converges to stable ratio)
    correction: f32,
}

impl InferencePredictor {
    /// Predict tokens/second for a given model and context length.
    /// Used by compute scheduler to set timeouts and by compositor
    /// to show progress indicators.
    pub fn predict_tps(&self, model_size_gb: f32, context_tokens: u32) -> f32 {
        let base = self.roofline_tps * self.correction;
        // Context length degrades throughput due to attention computation
        let context_factor = 1.0 - (context_tokens as f32 / 32768.0).min(0.3);
        // Memory pressure reduces effective bandwidth
        let pressure_factor = 1.0 - self.memory_pressure * 0.5;
        base * context_factor * pressure_factor
    }
}
```

**State overhead:** ~200 bytes. Updated once per inference completion.

### 13.2 KV Cache Eviction Scoring

The default KV cache eviction policy (§3.3 in [inference.md](./inference.md)) is LRU with priority weighting. This subsection describes an improved scoring function informed by PagedAttention (Kwon et al., SOSP 2023) and H2O — Heavy-Hitter Oracle (Zhang et al., 2023).

H2O observes that a small fraction of tokens (5-20%) accumulate the majority of attention weight across layers. Retaining these "heavy-hitter" tokens while evicting low-attention tokens allows aggressive KV cache compression with minimal quality loss — 20% of cache size retains near-full accuracy.

For AIRS, the eviction decision operates at the **session** level (which session's cache to evict when memory is exhausted) using a composite score:

```rust
pub struct KvCacheScore {
    /// Recency: EWMA of time since last inference in this session
    recency_ewma: f32,
    /// Frequency: number of inference calls in the last hour
    access_frequency: u32,
    /// Size: memory bytes consumed by this cache
    cache_bytes: usize,
    /// Priority class: Interactive > System > Background
    priority: InferencePriority,
}

impl KvCacheScore {
    /// Higher score = more valuable = evict last.
    /// Weights tuned for single-user edge device with 2-4 concurrent sessions.
    pub fn eviction_score(&self) -> f32 {
        let w_recency = 0.4;
        let w_frequency = 0.3;
        let w_size = 0.3;
        let priority_boost = match self.priority {
            InferencePriority::Interactive => 10.0,
            InferencePriority::System => 5.0,
            InferencePriority::Background => 1.0,
            InferencePriority::Batch => 0.5,
        };
        (w_recency * self.recency_ewma
            + w_frequency * self.access_frequency as f32
            + w_size * (1.0 / self.cache_bytes as f32) * 1e9)
            * priority_boost
    }
}
```

**Within-session** token eviction (H2O-style) is a future optimization: when a session's context exceeds the model's window, evict low-attention tokens rather than truncating the oldest messages. This requires per-layer attention score tracking during inference, adding ~64 KB overhead per active session.

**State overhead:** ~64 bytes per session (score struct) + optional 64 KB per session for H2O attention tracking.

### 13.3 Model Loading Progress Estimation

Loading a 4 GB model from an SD card takes ~45 seconds (sequential read at ~90 MB/s). From NVMe, ~3 seconds. Users need accurate progress indicators. AIRS tracks sequential read throughput and page fault service time to predict remaining load time:

```rust
pub struct ModelLoadTracker {
    /// Total bytes to load (model file size)
    total_bytes: u64,
    /// Bytes loaded so far (pages faulted in)
    loaded_bytes: u64,
    /// EWMA of page fault service time (microseconds)
    fault_time_ewma_us: f32,
    /// EWMA of sequential read throughput (bytes/second)
    throughput_ewma: f64,
    /// Layer bitmap: which transformer layers are resident in RAM
    layers_resident: u64,
    /// Total layer count
    total_layers: u32,
}

impl ModelLoadTracker {
    /// Estimated seconds remaining until model is fully loaded.
    pub fn eta_seconds(&self) -> f32 {
        let remaining = self.total_bytes - self.loaded_bytes;
        (remaining as f64 / self.throughput_ewma) as f32
    }

    /// Whether enough layers are loaded to begin inference
    /// (progressive loading: start generating after first N layers are resident).
    pub fn can_start_inference(&self) -> bool {
        // Need at least embedding layer + first attention layer
        self.layers_resident & 0x3 == 0x3
    }
}
```

**Progressive loading** (inspired by LLM in a Flash, Alizadeh et al., Apple 2023): instead of waiting for the entire model to load, AIRS can begin inference as soon as the embedding layer and first transformer layer are resident. Subsequent layers are prefaulted by a background thread using sequential `madvise(MADV_WILLNEED)` calls. On an SD card, this reduces perceived first-token latency from ~45 seconds to ~5 seconds for a 4 GB model.

**State overhead:** ~450 bytes + 8-byte layer bitmap.

### 13.4 Thermal Throttle Prediction

ARM Cortex-A76 cores (Pi 5) throttle at ~85°C, reducing clock speed by 20-50%. Sustained LLM inference can reach throttle temperatures within 30-60 seconds on passively cooled SBCs. AIRS predicts throttling before it happens using temperature derivative (dT/dt) and recent compute load:

```rust
pub struct ThermalPredictor {
    /// Current temperature (millidegrees Celsius, from thermal zone sysfs)
    temp_mc: i32,
    /// Temperature derivative: EWMA of dT/dt (millidegrees per second)
    dt_ewma: f32,
    /// Seconds until predicted throttle temperature (85°C default)
    time_to_throttle: f32,
    /// Current throttle state
    state: ThermalState,
}

pub enum ThermalState {
    /// < 65°C: full speed, no intervention
    Normal,
    /// 65-75°C: reduce background inference duty cycle to 50%
    Fair,
    /// 75-85°C: suspend background inference, migrate to efficiency cores if available
    Serious,
    /// ≥ 85°C: throttled by hardware, reduce interactive inference thread count
    Critical,
}

impl ThermalPredictor {
    /// Update with new temperature reading (called every 1-5 seconds).
    pub fn update(&mut self, new_temp_mc: i32, elapsed_ms: u32) {
        let dt = (new_temp_mc - self.temp_mc) as f32 / elapsed_ms as f32 * 1000.0;
        self.dt_ewma = 0.3 * dt + 0.7 * self.dt_ewma;
        self.temp_mc = new_temp_mc;

        let remaining_mc = 85_000 - new_temp_mc;
        self.time_to_throttle = if self.dt_ewma > 0.0 {
            remaining_mc as f32 / self.dt_ewma / 1000.0
        } else {
            f32::INFINITY
        };

        self.state = match new_temp_mc {
            t if t < 65_000 => ThermalState::Normal,
            t if t < 75_000 => ThermalState::Fair,
            t if t < 85_000 => ThermalState::Serious,
            _ => ThermalState::Critical,
        };
    }
}
```

The compute scheduler (§3.2 in [inference.md](./inference.md)) consults `ThermalPredictor` before starting background inference. In `Fair` state, background work runs at 50% duty cycle. In `Serious` state, only interactive and system-priority inference proceeds. This prevents the 40-50% throughput collapse that occurs when hardware thermal throttling engages suddenly.

**State overhead:** ~100 bytes. Polled every 1-5 seconds from ARM thermal zone sensors.

### 13.5 Token Budget Anomaly Detection

Each agent has a token budget (§3.1 in [inference.md](./inference.md)). The anomaly detector identifies agents whose inference consumption deviates significantly from their learned baseline, without requiring AIRS behavioral analysis (§5.5 in [intelligence-services.md](./intelligence-services.md)).

The detector uses Welford's online algorithm (1962) for running mean and variance computation with O(1) per observation and fixed state:

```rust
pub struct TokenAnomalyDetector {
    /// Per-agent running statistics (Welford's algorithm)
    agents: [Option<AgentTokenStats>; 64],
}

pub struct AgentTokenStats {
    agent_id: AgentId,
    /// Running mean of tokens consumed per hour
    mean: f64,
    /// Running M2 (for variance computation)
    m2: f64,
    /// Number of observation windows
    count: u64,
    /// EWMA of recent consumption (faster response to change)
    recent_ewma: f64,
    /// Anomaly flag: set when z-score exceeds threshold
    anomaly: bool,
}

impl AgentTokenStats {
    /// Update with a new hourly observation.
    pub fn observe(&mut self, tokens_this_hour: u64) {
        let x = tokens_this_hour as f64;
        self.count += 1;
        let delta = x - self.mean;
        self.mean += delta / self.count as f64;
        let delta2 = x - self.mean;
        self.m2 += delta * delta2;

        self.recent_ewma = 0.3 * x + 0.7 * self.recent_ewma;

        // Z-score anomaly detection (after warmup period)
        if self.count > 24 {
            let variance = self.m2 / (self.count - 1) as f64;
            let stddev = variance.sqrt();
            let z_score = (x - self.mean) / stddev.max(1.0);
            self.anomaly = z_score > 3.0;
        }
    }
}
```

When `anomaly` is true, the token budget enforcer can throttle the agent (queue further requests) or alert the behavioral monitor. This catches agents that suddenly consume 10-100x more tokens than their baseline — a potential sign of compromised agent behavior, prompt injection loops, or runaway generation.

**State overhead:** ~2.3 KB total (64 agents × ~36 bytes each — `AgentTokenStats` contains two `f64`, one `u64`, one `bool`, and one `AgentId`).

### 13.6 Embedding Index Compaction Scheduling

The HNSW embedding index (used by Space Indexer, §5.1 in [intelligence-services.md](./intelligence-services.md)) accumulates tombstones as objects are deleted or re-embedded. The compaction scheduler monitors index health and triggers rebuilds at optimal times:

```rust
pub struct HnswHealthMonitor {
    /// Total entries in the index (including tombstones)
    total_entries: u32,
    /// Tombstone count (deleted entries not yet compacted)
    tombstone_count: u32,
    /// Pre-computed reference vectors for recall probing (32-64 vectors)
    probe_set: Vec<(EmbeddingVector, Vec<ObjectId>)>,
    /// Last measured recall at ef_search=64
    last_recall: f32,
    /// Compaction in progress
    compacting: bool,
}

impl HnswHealthMonitor {
    /// Check whether compaction is needed. Called every 5 minutes.
    /// Cost: ~0.6ms (32 probe queries against the index).
    pub fn needs_compaction(&self) -> bool {
        let tombstone_ratio = self.tombstone_count as f32 / self.total_entries.max(1) as f32;
        // Trigger if >20% tombstones OR recall has degraded below 0.95
        tombstone_ratio > 0.20 || self.last_recall < 0.95
    }

    /// Measure recall by querying the probe set and comparing against known ground truth.
    pub fn measure_recall(&mut self, index: &HnswIndex) -> f32 {
        let mut hits = 0u32;
        let mut total = 0u32;
        for (query, expected) in &self.probe_set {
            let results = index.search(query, 10);
            for expected_id in expected.iter().take(10) {
                total += 1;
                if results.contains(expected_id) {
                    hits += 1;
                }
            }
        }
        self.last_recall = hits as f32 / total.max(1) as f32;
        self.last_recall
    }
}
```

Compaction is scheduled as a `Batch`-priority background task, yielding to interactive and system inference. On a Pi 5, compacting a 100K-entry HNSW index takes ~30 seconds. The probe set (32-64 reference vectors with known nearest neighbors) is rebuilt after each compaction.

**State overhead:** ~50 KB (probe set vectors). Recall check: ~0.6 ms every 5 minutes.

### 13.7 Kernel-Internal ML Summary

| Subsystem | Technique | State | Per-Observation Cost | Update Frequency |
|---|---|---|---|---|
| Inference predictor | Roofline + EWMA | 200 B | O(1) | Per inference |
| KV cache scoring | Composite EWMA | 64 B/session | O(1) | Per eviction check |
| Model loading | Throughput EWMA + bitmap | 450 B | O(1) | Per page fault batch |
| Thermal prediction | dT/dt EWMA | 100 B | O(1) | Every 1-5 s |
| Token anomaly | Welford's + z-score | 2.3 KB | O(1) | Hourly |
| HNSW health | Tombstone ratio + probe recall | 50 KB | O(k) probes | Every 5 min |

Total overhead: **< 55 KB**. No neural networks. No heap allocation beyond initialization. All techniques are deterministic and auditable.

-----

## 14. AIRS-Dependent Intelligence

These techniques require the AIRS inference engine to be running with a loaded model. They enhance inference quality, throughput, and capability. When AIRS is unavailable, none of these features operate — the system falls back to the baseline inference pipeline described in [inference.md](./inference.md).

### 14.1 Speculative Decoding

Standard autoregressive decoding generates one token per forward pass. Speculative decoding (Leviathan et al., ICML 2023; Chen et al., 2023) exploits the memory-bandwidth-bound nature of transformer inference to generate multiple tokens per pass.

A small "draft" mechanism generates K candidate tokens cheaply. The target model verifies all K candidates in a single forward pass (verification is as expensive as generating one token, since the bottleneck is loading weights). A modified rejection sampling scheme guarantees the output distribution is identical to the target model — zero quality loss.

Three approaches, in order of memory efficiency:

```rust
pub enum SpeculativeStrategy {
    /// N-gram lookup from prompt/prior context. Zero extra memory.
    /// Matches 2-4 token sequences from earlier in the conversation.
    /// Speedup: 1.1-1.3x (low acceptance rate, but free).
    NgramLookup { window: usize },

    /// Medusa-style self-drafting (Cai et al., ICML 2024).
    /// Adds lightweight prediction heads on top of the target model's
    /// hidden states. Each head predicts a future token position.
    /// Extra memory: 10-50 MB. Speedup: 1.5-2.5x.
    /// Requires fine-tuned heads per model.
    MedusaHeads { num_heads: u32 },

    /// Separate draft model (classic speculative decoding).
    /// A small model (68M-500M params) generates candidates.
    /// Extra memory: 50-200 MB. Speedup: 1.3-1.8x on ARM CPU.
    /// Draft model must be architecturally compatible with target.
    SeparateDraft { draft_model: ModelId, num_candidates: u32 },
}
```

**Recommendation for AIOS:** On 4 GB devices, use n-gram lookup only (zero memory cost). On 8 GB devices, Medusa heads are viable if available for the loaded model. Separate draft models are practical only on 16 GB+ devices where 200 MB is negligible.

**Interaction with constrained decoding (§14.3):** Speculative decoding and grammar-constrained generation interact poorly — the draft model doesn't know the grammar constraints, so acceptance rates drop significantly. When grammar constraints are active, AIRS should disable speculative decoding and use standard autoregressive generation.

### 14.2 Continuous Batching and Prefix Caching

Full continuous batching (Orca, Yu et al., OSDI 2022) interleaves multiple inference requests at the iteration level. This is primarily valuable for multi-tenant GPU servers and provides minimal benefit for a single-user edge device.

The relevant technique for AIOS is **prefix caching**: reusing the computed KV cache for shared prompt prefixes across inference requests. Many AIRS services share a long system prompt (e.g., the intent verifier's security instructions, the context engine's classification schema). Without prefix caching, each invocation re-processes this prompt from scratch.

```rust
pub struct PrefixCache {
    /// Cached KV states keyed by prompt hash
    entries: HashMap<u64, CachedPrefix>,
    /// Maximum cache size in bytes
    max_bytes: usize,
    /// Current usage
    used_bytes: usize,
}

pub struct CachedPrefix {
    /// Hash of the prompt tokens that produced this KV state
    prompt_hash: u64,
    /// Number of tokens in the cached prefix
    token_count: u32,
    /// Serialized KV cache state (loadable by GGML)
    kv_state: Vec<u8>,
    /// Last used timestamp for LRU eviction
    last_used: Timestamp,
    /// Size in bytes
    size: usize,
}
```

**Impact:** A 2048-token system prompt for an 8B model consumes ~128 MB of KV cache and takes 1-5 seconds to process on ARM CPU. Prefix caching eliminates this cost for repeated invocations. For AIRS services that invoke inference 10-50 times per hour with the same system prompt, this saves 10-250 seconds of cumulative prefill time per hour.

**Memory tradeoff:** Each cached prefix consumes its KV cache size in RAM. With 3-5 cached prefixes (~400-640 MB), the benefit is significant but the memory cost is substantial on 4-8 GB devices. AIRS should cache only the 2-3 most frequently used prefixes (intent verifier, conversation bar, context engine).

### 14.3 Structured Output Generation

AIRS intelligence services produce structured output — the intent verifier returns `VerificationResult` enums, the context engine returns `ContextState` structs, the attention manager returns urgency scores. Without constrained decoding, these services free-generate text and parse it, which is fragile and can produce malformed output requiring retries.

Constrained decoding (Outlines, Willard & Louf 2023) compiles a grammar into a finite-state automaton and masks invalid tokens at each decoding step, guaranteeing 100% valid output with zero retries:

```rust
pub struct ConstrainedDecoder {
    /// Pre-compiled grammars for AIRS service outputs
    grammars: HashMap<TaskType, CompiledGrammar>,
}

pub struct CompiledGrammar {
    /// DFA states with allowed token masks per state
    states: Vec<DfaState>,
    /// Token vocabulary (shared across grammars)
    vocab_size: u32,
    /// Memory consumed by this grammar's DFA
    size_bytes: usize,
}

pub struct DfaState {
    /// Bitmask of allowed token IDs at this state (vocab_size bits)
    allowed_tokens: BitVec,
    /// Transition table: token → next state
    transitions: HashMap<u32, u32>,
}
```

**Pre-compiled grammars for AIRS services:**

| Service | Grammar | Output Type | DFA States |
|---|---|---|---|
| Intent Verifier | `{ "result": "aligned" \| "suspicious" \| "violation", "confidence": float, "explanation": string }` | JSON | ~50 |
| Context Engine | `{ "work_engagement": float, "ai_engagement": enum, "notification_threshold": enum }` | JSON | ~30 |
| Attention Manager | `{ "urgency": enum, "reason": string }` | JSON | ~20 |
| Tool Manager | `{ "function": string, "arguments": { ... } }` | JSON (tool call) | ~100 |
| Metadata Generation | `{ "summary": string, "tags": [string], "entities": [...] }` | JSON | ~80 |

**Performance:** llama.cpp's GBNF grammar implementation adds < 1 ms overhead per token for JSON schemas. The DFA pre-computation is a one-time cost of 100-500 ms per grammar. For AIOS, grammars are compiled at AIRS startup and cached.

**Memory:** Each compiled grammar consumes 1-5 MB depending on DFA state count and vocabulary size. Total for 5 pre-compiled grammars: ~10-25 MB.

### 14.4 Retrieval-Augmented Generation Pipeline

The current Space Indexer (§5.1 in [intelligence-services.md](./intelligence-services.md)) performs one-shot retrieval: compute query embedding → search HNSW index → inject top-K results into context. RAG (Lewis et al., 2020) extends this with iterative retrieval and re-ranking:

```rust
pub struct RagPipeline {
    /// Embedding model for query and document encoding
    embedding_model: ModelHandle,
    /// HNSW index for approximate nearest-neighbor search
    index: HnswIndex,
    /// Re-ranker: uses the primary LLM to score relevance
    reranker: Option<ModelHandle>,
    /// Maximum retrieval rounds
    max_rounds: u32,
}

pub struct RagConfig {
    /// Number of candidates to retrieve per round
    top_k: u32,
    /// Whether to decompose complex queries into sub-queries
    query_decomposition: bool,
    /// Whether to re-rank candidates with the primary model
    rerank: bool,
    /// Maximum total context tokens from retrieved documents
    max_context_tokens: u32,
}
```

**Iterative retrieval:** For complex queries ("find all documents about the project Alpha budget changes in Q3"), the pipeline decomposes the query into sub-queries ("project Alpha", "budget changes", "Q3"), retrieves candidates for each, merges and deduplicates results, then re-ranks by relevance using the primary model. This improves recall for multi-faceted queries at the cost of 2-3x more embedding computations.

**Embedding model on ARM:** Small embedding models like all-MiniLM-L6-v2 (22M parameters, 384 dimensions) require ~23 MB quantized and produce embeddings in 5-15 ms on ARM CPU. This is fast enough for real-time query embedding and on-demand document embedding.

**Latency budget:** One-shot retrieval adds ~20-50 ms (embedding + HNSW search). Iterative retrieval with re-ranking adds 100-300 ms total. Both are acceptable for interactive use where the user is waiting for a conversation response (which takes 1-10 seconds for generation).

### 14.5 Adaptive Quantization

Standard quantization (Q4_K_M) applies the same precision to all model layers. Research shows that different layers have different sensitivity to quantization error — attention layers (especially Q/K projections) lose more quality from aggressive quantization than FFN layers (SqueezeLLM, Kim et al. 2023; AWQ, Lin et al. 2024; AQLM, Egiazarian et al. 2024).

```rust
pub struct AdaptiveQuantConfig {
    /// Per-layer quantization assignments
    layer_quants: Vec<LayerQuant>,
    /// Total model memory (computed from layer assignments)
    total_memory: usize,
    /// Quality estimate relative to full precision (0.0-1.0)
    estimated_quality: f32,
}

pub struct LayerQuant {
    layer_index: u32,
    layer_type: LayerType,
    /// Quantization format for this layer
    quant: QuantFormat,
    /// Memory consumed by this layer at this quantization
    memory_bytes: usize,
}

pub enum LayerType {
    Embedding,
    AttentionQK,     // sensitive to quantization
    AttentionVO,     // moderately sensitive
    FeedForward,     // tolerant of aggressive quantization
    LayerNorm,       // always FP32 (tiny, critical for stability)
    OutputHead,      // sensitive (affects token probabilities)
}
```

**Mixed-precision strategy:** Use Q5_K_M for attention Q/K projections and output head, Q4_K_M for attention V/O and FFN layers, FP32 for layer norms. This increases model memory by ~10% compared to uniform Q4_K_M but recovers ~50% of the quality gap between Q4 and Q5 quantization.

**AIRS integration:** Adaptive quantization is a model-level decision, made at model download/conversion time. AIRS's QuantizationSelector (§4.3 in [model-registry.md](./model-registry.md)) would select mixed-precision GGUF files when available, falling back to uniform quantization otherwise. GGUF format already supports per-tensor quantization — the infrastructure exists.

### 14.6 On-Device Fine-Tuning (LoRA)

LoRA (Hu et al., 2022) and QLoRA (Dettmers et al., 2023) enable lightweight fine-tuning by training small rank-decomposition matrices alongside frozen base model weights. The adapter is typically 50-200 MB for a 7B model.

```rust
pub struct LoraConfig {
    /// Base model to fine-tune
    base_model: ModelId,
    /// LoRA rank (4-64; lower = less memory, less capacity)
    rank: u32,
    /// Alpha scaling factor
    alpha: f32,
    /// Target modules (which layers get LoRA adapters)
    target_modules: Vec<String>,
    /// Training data source
    data_source: LoraDataSource,
    /// Maximum training examples per session
    max_examples: u32,
    /// Output path for saved adapter
    adapter_path: SpacePath,
}

pub enum LoraDataSource {
    /// Learn from user's writing style in spaces
    UserContent { spaces: Vec<SpaceId> },
    /// Learn from conversation history
    ConversationHistory { min_rating: f32 },
    /// Learn from preference signals (Context Engine + Preferences)
    PreferenceSignals,
}
```

**Memory requirements for on-device training:**

| Device RAM | Base Model | Adapter Size | Gradient Memory | Feasible? |
|---|---|---|---|---|
| 4 GB | 3B Q4 (1.8 GB) | ~30 MB (rank 8) | ~800 MB | Marginal — requires aggressive memory management |
| 8 GB | 3B Q4 (1.8 GB) | ~30 MB (rank 8) | ~800 MB | Yes — ~2.6 GB total, leaves 5.4 GB for OS |
| 8 GB | 8B Q4 (4.5 GB) | ~50 MB (rank 8) | ~2 GB | No — exceeds available memory |
| 16 GB | 8B Q4 (4.5 GB) | ~80 MB (rank 16) | ~2 GB | Yes — ~6.6 GB total |

**Use cases for AIOS:** Fine-tuning the primary model on user writing style (from space content), domain-specific terminology (from work spaces), and preference patterns (from Context Engine signals). Training happens during idle periods as a `Batch`-priority task. The resulting LoRA adapter is saved to `system/models/adapters/` and loaded at next AIRS startup.

**Privacy:** All training data comes from the user's own spaces. No data leaves the device. The adapter is a local artifact.

### 14.7 Federated Model Improvement

When multiple AIOS devices are in a fleet (§4 in [../../platform/multi-device.md](../../platform/multi-device.md)), federated learning enables collaborative model improvement without sharing raw user data. Each device trains a LoRA adapter locally, then shares only the gradient updates (not the training data) with a coordination server.

```rust
pub struct FederatedConfig {
    /// Whether this device participates in federated learning
    enabled: bool,
    /// Privacy budget (ε in differential privacy)
    epsilon: f32,
    /// Maximum gradient upload size per round
    max_upload_bytes: usize,
    /// Minimum local training examples before contributing
    min_local_examples: u32,
    /// Fleet coordination endpoint
    coordinator: Option<EndpointId>,
}
```

**Protocol (FedAvg, McMahan et al. 2017):**

1. Coordinator distributes current global LoRA adapter to all devices
2. Each device trains locally for 1-5 epochs on its data
3. Each device clips gradients and adds calibrated Gaussian noise (differential privacy)
4. Devices upload noised gradient deltas (~50-200 MB per round for LoRA adapters)
5. Coordinator averages gradients and updates global adapter
6. Repeat every 24-72 hours

**Privacy guarantee:** With ε = 8 (moderate privacy) and 10+ devices, individual training examples are not recoverable from the shared gradients. The noise calibration ensures plausible deniability for any single data point.

**Communication overhead:** A LoRA adapter for an 8B model at rank 16 is ~80 MB. With gradient compression, upload per round is ~20-50 MB. Feasible over WiFi but not practical over metered cellular connections.

**Limitations:** Requires multi-device infrastructure (Phase 25+). Convergence is slow with heterogeneous user data (non-IID). Most valuable for domain-specific improvements (e.g., fleet of devices used in the same industry).

### 14.8 Model Distillation Pipeline

On-device distillation transfers knowledge from a larger cloud model to the local model through (prompt, response) pair collection. When the user optionally routes queries through a cloud API (for higher quality), AIRS collects these pairs to fine-tune the local model:

```rust
pub struct DistillationConfig {
    /// Whether cloud distillation is enabled (user opt-in)
    enabled: bool,
    /// Minimum pairs collected before triggering fine-tuning
    min_pairs: u32,
    /// Maximum pairs stored (FIFO eviction)
    max_pairs: u32,
    /// Quality filter: only store pairs where the cloud response
    /// is rated higher than the local model's response
    quality_filter: bool,
    /// Storage location for collected pairs
    pair_storage: SpacePath,
}

pub struct DistillationPair {
    prompt: Prompt,
    cloud_response: String,
    local_response: Option<String>,
    quality_delta: Option<f32>,
    timestamp: Timestamp,
}
```

**How it works:**

1. User enables cloud fallback for certain tasks (e.g., complex code generation)
2. AIRS sends the query to both cloud and local model (local for speed, cloud for quality)
3. If the user prefers the cloud response (explicit signal or inferred from behavior), the pair is saved
4. After collecting 1000-5000 quality-filtered pairs, AIRS runs a LoRA fine-tuning session (§14.6) using the cloud responses as training targets
5. Over time, the local model improves for the user's specific use patterns, reducing cloud dependency

**Privacy:** The user controls which queries go to cloud. Collected pairs stay on-device. The fine-tuned adapter is local. Cloud providers see only the queries the user explicitly chose to send.

### 14.9 Semantic Cache

Before running inference, AIRS checks whether a semantically similar query was recently answered, potentially skipping inference entirely:

```rust
pub enum CacheStrategy {
    /// Hash-based exact match. Zero overhead, zero risk of wrong cached response.
    /// Hit rate: 2-5% for diverse queries, higher for repeated system prompts.
    ExactMatch,

    /// Template-based match: match by command template + content hash.
    /// E.g., "summarize [file_hash]" → cached summary for that file.
    /// Hit rate: 10-20% for agent tool calls.
    TemplateMatch {
        templates: Vec<CacheTemplate>,
    },

    /// Embedding-based semantic similarity. Requires embedding model.
    /// Hit rate: 5-15% for diverse queries, but risk of returning wrong cached response.
    /// Threshold must be tuned carefully (cosine > 0.95 recommended).
    SemanticMatch {
        embedding_model: ModelHandle,
        similarity_threshold: f32,
        max_entries: usize,
    },
}

pub struct CacheTemplate {
    /// Pattern: "summarize {content_hash}" or "translate {content_hash} {target_lang}"
    pattern: String,
    /// TTL: how long cached responses are valid
    ttl: Duration,
}
```

**Recommendation for AIOS:** Use `ExactMatch` (always — zero cost) + `TemplateMatch` (for agent tool calls with deterministic inputs). Full `SemanticMatch` is not recommended on 4-8 GB devices because:

- The embedding model adds 23-90 MB of persistent memory
- Query embedding adds 5-50 ms latency per request (even on cache miss)
- Hit rates of 5-15% don't justify the memory cost on constrained hardware
- Risk of returning stale/incorrect cached responses for contextually different queries

Prefix caching (§14.2) provides a larger performance win with less memory overhead.

### 14.10 Multi-Modal Inference Pipeline

Extending AIRS beyond text to process images (camera, screenshots) and audio (voice commands):

```rust
pub struct MultiModalPipeline {
    /// Text-only model (always available)
    text_model: ModelHandle,
    /// Vision-language model (loaded on demand)
    vision_model: Option<ModelHandle>,
    /// Speech-to-text model (loaded on demand)
    speech_model: Option<ModelHandle>,
    /// Routing: input modality → appropriate model
    router: ModalityRouter,
}

pub enum InputModality {
    Text,
    Image { format: ImageFormat, resolution: (u32, u32) },
    Audio { format: AudioFormat, duration_ms: u32 },
    /// Combined text + image (e.g., "describe this screenshot")
    TextImage { text: String, image: ImageData },
}

pub struct ModalityRouter {
    /// Whether vision model is loaded and available
    vision_available: bool,
    /// Whether speech model is loaded and available
    speech_available: bool,
    /// Fallback behavior when multimodal model is unavailable
    fallback: ModalityFallback,
}

pub enum ModalityFallback {
    /// Return error: "Vision model not loaded"
    Error,
    /// Use text model with image description from metadata
    TextDescription,
    /// Load the multimodal model on demand (may take 10-30 seconds)
    LoadOnDemand,
}
```

**Model sizes and performance on ARM64:**

| Model | Parameters | RAM (Q4) | ARM CPU Speed | Use Case |
|---|---|---|---|---|
| LLaVA 1.5 7B | 7B | ~4.5 GB | 2-4 tok/s | Image understanding, screenshot analysis |
| Whisper tiny | 39M | ~39 MB | ~10x realtime | Basic speech-to-text |
| Whisper base | 74M | ~74 MB | ~5x realtime | Better speech-to-text |
| Whisper small | 244M | ~244 MB | ~2x realtime | Good speech-to-text |

**Practical deployment:** On 8 GB devices, a vision-language model cannot coexist with the text model in RAM — one must be evicted. Whisper tiny/base can coexist as companions (~40-75 MB). Voice commands would use Whisper for transcription, then route the text to the primary model for understanding.

**GGUF multimodal support:** The GGUF format supports vision-language models (LLaVA adapter + CLIP vision encoder packaged alongside the language model). llama.cpp's `llava` example demonstrates the inference pipeline. Integration requires the image preprocessor (CLIP-style patch encoding) to run before the language model forward pass.

### 14.11 AIRS-Dependent Intelligence Summary

| Technique | Memory Cost | Speedup / Benefit | Minimum RAM | Maturity |
|---|---|---|---|---|
| Speculative decoding (n-gram) | 0 | 1.1-1.3x tok/s | 4 GB | Production (llama.cpp) |
| Speculative decoding (Medusa) | 10-50 MB | 1.5-2.5x tok/s | 8 GB | Research → production |
| Prefix caching | 128-640 MB | Save 1-5s per invocation | 8 GB | Production (llama.cpp) |
| Constrained decoding (GBNF) | 10-25 MB | Eliminate retries, 100% valid output | 4 GB | Production (llama.cpp) |
| RAG pipeline | 23 MB (embedding model) | Better search quality | 4 GB | Production |
| Adaptive quantization | +10% model size | ~50% quality gap recovery | 4 GB | Research → production |
| On-device LoRA | 800 MB-2 GB (training) | Personalized model | 8 GB (3B), 16 GB (8B) | Research |
| Federated learning | 20-50 MB (upload) | Fleet-wide improvement | 8 GB + fleet | Research |
| Model distillation | ~50 MB (pair storage) | Reduce cloud dependency | 8 GB + cloud | Research |
| Semantic cache | 0-90 MB | Skip 2-20% of inference | 4 GB (exact only) | Production |
| Multi-modal | 40 MB-4.5 GB | Vision + voice understanding | 8 GB (voice), 16 GB (vision) | Research → production |

-----

## 15. Future Directions

### 15.1 Neuromorphic Compute Integration

As neuromorphic processors (Intel Loihi 2, BrainChip Akida) become available on SBC form factors, AIRS's ComputeDevice enum can be extended with a `Neuromorphic` variant. Spiking neural networks (SNNs) excel at always-on low-power classification tasks — ideal for the Context Engine's signal processing (keyboard cadence detection, activity classification) at sub-milliwatt power consumption. The thermal predictor (§13.4) and anomaly detector (§13.5) could run on neuromorphic hardware with near-zero energy cost.

### 15.2 Mixture-of-Experts (MoE) On-Device

MoE models (Mixtral, Switch Transformer) activate only a fraction of parameters per token, offering higher quality at lower compute cost than dense models of equivalent total size. A Mixtral 8x7B model has 47B total parameters but activates only ~13B per token. With expert offloading (keep inactive experts on disk, swap in on demand), MoE models could provide 13B-quality inference within the memory budget of a 7B dense model. The challenge is expert swap latency on SD card storage.

### 15.3 Continuous Learning from Interaction

Beyond LoRA fine-tuning (§14.6), future AIRS versions could implement online learning — updating model behavior in real-time based on user feedback without explicit training sessions. Techniques like online distillation, experience replay, and contextual bandits for response selection could make the model continuously improve during normal use. The privacy implications (model memorizes user data) require careful analysis.

### 15.4 Cross-Model Knowledge Transfer

When a user upgrades their hardware and AIRS switches from a 3B to an 8B model, LoRA adapters trained on the old model are incompatible. Cross-model knowledge transfer would extract learned behaviors from the old adapter (via activation matching or distillation) and transfer them to a new adapter for the larger model, preserving personalization across hardware upgrades.

### 15.5 Formal Verification of Constrained Decoding

The DFA-based constrained decoding (§14.3) guarantees grammatical validity but not semantic correctness. Future work could extend grammar constraints with semantic type systems — e.g., ensuring that the intent verifier's confidence score is actually between 0.0 and 1.0, or that object IDs reference existing objects. This bridges grammar-level guarantees with application-level correctness.

-----

## References

- Alizadeh et al. "LLM in a Flash: Efficient Large Language Model Inference with Limited Memory." Apple, 2023.
- Cai et al. "Medusa: Simple LLM Inference Acceleration Framework with Multiple Decoding Heads." ICML 2024.
- Chen et al. "Accelerating Large Language Model Decoding with Speculative Sampling." DeepMind, 2023.
- Dettmers et al. "QLoRA: Efficient Finetuning of Quantized Large Language Models." NeurIPS 2023.
- Egiazarian et al. "AQLM: Extreme Compression of Large Language Models via Additive Quantization." 2024.
- Hu et al. "LoRA: Low-Rank Adaptation of Large Language Models." ICLR 2022.
- Kim et al. "SqueezeLLM: Dense-and-Sparse Quantization." 2023.
- Kwon et al. "Efficient Memory Management for Large Language Model Serving with PagedAttention." SOSP 2023.
- Leviathan et al. "Fast Inference from Transformers via Speculative Decoding." ICML 2023.
- Lewis et al. "Retrieval-Augmented Generation for Knowledge-Intensive NLP Tasks." NeurIPS 2020.
- Lin et al. "AWQ: Activation-aware Weight Quantization for LLM Compression and Acceleration." MLSys 2024.
- McMahan et al. "Communication-Efficient Learning of Deep Networks from Decentralized Data." AISTATS 2017.
- Williams et al. "Roofline: An Insightful Visual Performance Model for Multicore Architectures." CACM 2009.
- Willard & Louf. "Efficient Guided Generation for LLMs." 2023.
- Yu et al. "Orca: A Distributed Serving System for Transformer-Based Generative Models." OSDI 2022.
- Zhang et al. "H2O: Heavy-Hitter Oracle: Efficient Generative Inference of Large Language Models with Heavy Hitters." NeurIPS 2023.
