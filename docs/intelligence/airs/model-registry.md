# AIOS AIRS Model Registry

Part of: [airs.md](../airs.md) — AI Runtime Service
**Related:** [inference.md](./inference.md) — Inference engine, [scaling.md](./scaling.md) — Hardware scaling, [../../storage/spaces.md](../../storage/spaces.md) — Space Storage

-----

## 4. Model Registry

### 4.1 Model Storage

Models are stored in the `system/models/` space as content-addressed objects:

```rust
pub struct ModelEntry {
    id: ModelId,
    name: String,                   // "llama-3.1-8b-q4_k_m"
    family: String,                 // "llama", "phi", "mistral"
    parameters: u64,                // 8_000_000_000
    quantization: QuantFormat,      // Q4_K_M, Q5_K_M, Q6_K, F16
    file_size: u64,                 // bytes on disk
    ram_required: u64,              // bytes in RAM when loaded
    context_length: u32,            // max tokens
    capabilities: Vec<ModelCapability>,
    content_hash: Hash,             // integrity verification
    source: ModelSource,
}

pub enum ModelCapability {
    TextGeneration,
    Embedding,
    Classification,
    CodeGeneration,
    Summarization,
    Translation,
    VisionLanguage,
}

pub enum ModelSource {
    Bundled,                        // shipped with AIOS
    Downloaded { url: String },     // from model registry
    UserProvided { path: SpacePath },
}
```

**Disk storage reality:** Model GGUF files are the largest single item on disk. A single 8B Q4 model is 4.5 GB; a 70B Q4 is ~40 GB. AIRS coordinates with the Space Storage system's storage budget and device profiles (see [spaces.md §10](../../storage/spaces.md)) to respect model disk quotas:

- **Laptop/PC (initial target, 256 GB - 2 TB):** Multiple models stored on disk comfortably. A 256 GB laptop with a 20% model quota (~48 GB) can hold 10+ 8B models or 3-4 models including a 70B. LRU eviction when the quota is exceeded. Storage pressure from models is rare.
- **Phone (future, 256 GB with 50-70% apps):** 1-2 models on disk. Aggressive eviction — delete on model switch. Prefer smaller quantizations (8B Q4).
- **TV (future, 16-128 GB):** Streaming from network or hub device. Local cache for offline fallback only.
- **SBC (future, 32-256 GB):** Single model at a time on small storage. Delete old before downloading new.

Downloaded models are **always evictable** — they can be re-fetched from the model registry. User-provided models (local fine-tunes, custom GGUF files) are **never automatically deleted** because they may not be reproducible.

### 4.2 Model Profiles

Different tasks need different models. AIRS maps tasks to models:

```rust
pub struct ModelProfiles {
    profiles: HashMap<TaskType, ModelProfile>,
}

pub struct ModelProfile {
    task: TaskType,
    preferred: ModelId,
    fallback: Vec<ModelId>,        // if preferred unavailable
    min_quality: QuantFormat,      // minimum acceptable quantization
}

pub enum TaskType {
    /// Conversation bar interaction
    Conversation,
    /// Generating embeddings for space indexing
    Embedding,
    /// Intent verification (action vs declared intent)
    IntentVerification,
    /// Behavioral anomaly detection
    BehavioralAnalysis,
    /// Object summarization and tagging
    MetadataGeneration,
    /// Prompt injection detection
    AdversarialDetection,
    /// Context inference (work/leisure)
    ContextInference,
    /// Attention urgency assessment
    AttentionTriage,
    /// Agent-requested inference
    AgentInference,
}
```

**Default model strategy:**

- Ship one general-purpose model (7-8B, Q4_K_M, ~4.5 GB) for all tasks
- Ship one small embedding model (~100 MB) for Space Indexer
- Users can download larger/specialized models from the model registry
- System intelligently routes tasks to the best available model

### 4.3 Quantization Strategy by Hardware Tier

Different hardware tiers require different model quantization levels. AIRS selects the best model variant at first boot and when the user changes their model preferences.

```text
RAM Tier            Model Pool   Quantization   Model Size    Quality       Notes
─────────────────   ──────────   ────────────   ──────────    ────────      ─────
< 2 GB Degraded        0 MB      N/A            N/A          Cloud-only    No local inference
2-4 GB Minimal         1 GB      Q4_K_M         1B params    Minimal       Simple completions
4-8 GB Constrained     2 GB      Q4_K_M         3B params    Basic         Limited reasoning
8-16 GB Recommended    4 GB      Q4_K_M         8B params    Good          Target experience
≥ 16 GB Comfortable    8 GB      Q5_K_M         8B params    High          Best local quality
```

```rust
pub struct QuantizationSelector {
    model_pool_size: usize,
    available_models: Vec<ModelEntry>,
}

impl QuantizationSelector {
    /// Select the best model variant that fits in the model pool
    /// alongside the embedding model and KV cache budget
    pub fn select_best(&self) -> ModelSelection {
        let embedding_overhead = 100 * MB;  // embedding model
        let kv_budget = self.model_pool_size / 4;  // 25% for KV caches
        let model_budget = self.model_pool_size - embedding_overhead - kv_budget;

        match model_budget {
            0 => ModelSelection::CloudOnly,
            b if b < 1500 * MB => ModelSelection::Local {
                // Small model, aggressive quantization
                preferred_params: "1-3B",
                min_quant: QuantFormat::Q3_K_S,
                preferred_quant: QuantFormat::Q4_K_S,
            },
            b if b < 4000 * MB => ModelSelection::Local {
                // Full-size model, standard quantization
                preferred_params: "7-8B",
                min_quant: QuantFormat::Q3_K_S,
                preferred_quant: QuantFormat::Q4_K_M,
            },
            _ => ModelSelection::Local {
                // Full-size model, high-quality quantization
                preferred_params: "7-8B",
                min_quant: QuantFormat::Q4_K_M,
                preferred_quant: QuantFormat::Q5_K_M,
            },
        }
    }
}
```

**Quality vs fit tradeoffs:**

- **Q3_K_S:** Aggressive quantization. Noticeable quality loss, but fits in tight memory. Acceptable for intent verification, metadata generation, and simple tasks. Not ideal for extended conversation.
- **Q4_K_M:** Sweet spot for 8 GB devices. Minor quality loss from full precision. Good enough for all AIRS tasks including conversation.
- **Q5_K_M / Q6_K:** Near-full-precision quality. Only fits on 16 GB+ devices alongside other system needs. Worth it if the hardware supports it.
- **Cloud:** No local quality tradeoff. Latency and connectivity are the costs instead.

### 4.4 LRU Model Eviction

Multiple models can't fit in RAM simultaneously on low-memory devices. The registry manages loading/unloading:

```text
RAM Budget: 4 GB available for models

Loaded models:
  llama-3.1-8b-q4_k_m   (4.5 GB)  ← active (conversation bar)

User opens a vision task → needs vision model (3 GB)
  1. llama model is idle → evict from RAM (weights still on disk)
  2. Load vision model → 3 GB
  3. When conversation bar is used again → evict vision, reload llama
  4. Model weights are memory-mapped — loading is fast (no parsing, just mmap)
```

### 4.5 Model Switching Optimization

Model switching (evict one model, load another) is the most expensive operation in AIRS. On an SD card, loading a 4 GB model takes 10-30 seconds. On NVMe, it takes 1-3 seconds. AIOS minimizes switching through several strategies:

**1. Primary model residence:** The primary model (conversation bar, intent verification) is loaded at boot and stays resident. It is never evicted for a background task. If a specialist model is needed, AIRS checks whether the primary model can handle the task at acceptable quality first.

```rust
pub struct ModelResidencyPolicy {
    /// Primary model — loaded at boot, never evicted for background work
    primary: ModelId,
    /// Companion model — small specialist that stays alongside primary
    /// (e.g., embedding model at ~100 MB)
    companion: Option<ModelId>,
    /// Maximum time to keep a specialist model loaded after its task completes
    specialist_ttl: Duration,           // default: 5 minutes
}

impl ModelResidencyPolicy {
    pub fn can_evict(&self, model: &LoadedModel, reason: EvictionReason) -> bool {
        match reason {
            // Never evict primary for background work
            EvictionReason::BackgroundTaskNeeds(_) => {
                model.model_id != self.primary
                    && Some(model.model_id) != self.companion
            },
            // Only evict primary for interactive user request
            EvictionReason::InteractiveTaskNeeds(_) => {
                model.active_sessions == 0
            },
        }
    }
}
```

**2. Small specialist alongside primary:** On 8 GB devices, a small specialist model (1-2B parameters, ~500 MB-1 GB) can stay loaded alongside the primary 8B model. This specialist handles focused tasks (classification, entity extraction, embedding) without requiring model switching. The Space Indexer's embedding model (~100 MB) is always the companion.

**3. Task routing to avoid switching:** When a task requests a model that isn't loaded, AIRS first evaluates whether the primary model can handle the task:

```text
Task: "Generate embedding for this document"
Ideal model: embedding-model (loaded as companion)
  → Route to companion. No switch needed.

Task: "Classify this image"
Ideal model: vision-model (not loaded)
Primary model: llama-8b (loaded, no vision capability)
  → Cannot route to primary. Must switch.
  → Check: any queued vision tasks? Batch them.
  → Evict least-recently-used non-primary model.
  → Load vision model, process all queued vision tasks.
  → Keep vision model loaded for specialist_ttl (5 min).
  → If no more vision tasks: evict, reclaim memory.
```

**4. Predictive pre-loading (future):** Based on user behavior patterns (Context Engine signals), AIRS can predict which model will be needed next and begin loading it in the background before the user requests it. Example: user opens a photo space → AIRS begins loading the vision model in a background thread while the user browses thumbnails.

**SD card reality:** On a Pi with an SD card, even mmap-based loading is slow because every page fault requires an SD card read (~100 μs per 4 KB page, vs ~5 μs for NVMe). A 4 GB model requires ~1 million page faults to fully warm up. AIOS mitigates this with sequential pre-faulting — after the mmap, a background thread reads the model file sequentially (which aligns with SD card's best-case sequential read performance of ~90 MB/s) to populate all pages before inference begins. First-token latency is ~45 seconds on SD vs ~3 seconds on NVMe for a 4 GB model.

### 4.6 Boot-Time Model Selection

At boot (Phase 3), AIRS must select which model to load before any user interaction occurs. This selection is based on available RAM, detected compute hardware, and what model files exist in `system/models/`. The thresholds are hardcoded for deterministic boot behavior — no inference is needed to select the first model.

**RAM-based default selection thresholds:**

```text
Available RAM        Model Pool Alloc    Default Model Selection
──────────────────   ─────────────────   ─────────────────────────────────────
< 2 GB               0 MB                No local model. AIRS starts in
                                          cloud-only mode. Intelligence services
                                          that require inference are disabled.
                                          Rule-based fallbacks active.

2 GB – 3.9 GB        1 GB                1B parameter model, Q4_K_M quantization.
                                          ~600 MB on disk, ~900 MB in RAM.
                                          Sufficient for: context inference,
                                          intent verification, metadata generation.
                                          Insufficient for: extended conversation,
                                          complex summarization.

4 GB – 7.9 GB        2 GB                3B parameter model, Q4_K_M quantization.
                                          ~1.7 GB on disk, ~2 GB in RAM.
                                          Sufficient for: all AIRS tasks at
                                          reduced quality. Conversation works
                                          but with shorter context windows.

≥ 8 GB (target)      4 GB                8B parameter model, Q4_K_M quantization.
                                          ~4.5 GB on disk, ~4.5 GB in RAM.
                                          Full quality for all AIRS tasks.
                                          This is the target experience.

≥ 16 GB              8 GB                8B parameter model, Q5_K_M or Q6_K.
                                          Higher quantization = better quality.
                                          Room for specialist models alongside
                                          the primary model.
```

```rust
pub struct BootModelSelector {
    available_ram: usize,
    model_catalog: Vec<ModelEntry>,
}

impl BootModelSelector {
    /// Called during Phase 3 boot to select the initial model.
    /// This function is deterministic — same RAM + same catalog = same selection.
    pub fn select_boot_model(&self) -> BootModelDecision {
        let model_pool = self.compute_model_pool();

        if model_pool == 0 {
            return BootModelDecision::CloudOnly;
        }

        // Find the best model that fits in the pool
        let embedding_reserve = 100 * MB;
        let kv_reserve = model_pool / 4;
        let model_budget = model_pool - embedding_reserve - kv_reserve;

        let candidates: Vec<&ModelEntry> = self.model_catalog.iter()
            .filter(|m| m.ram_required <= model_budget)
            .filter(|m| m.capabilities.contains(&ModelCapability::TextGeneration))
            .collect();

        if candidates.is_empty() {
            return BootModelDecision::CloudOnly;
        }

        // Prefer larger parameter count, then better quantization
        let best = candidates.iter()
            .max_by_key(|m| (m.parameters, m.quantization.quality_rank()))
            .unwrap();

        BootModelDecision::Local {
            model_id: best.id,
            model_pool_size: model_pool,
        }
    }

    fn compute_model_pool(&self) -> usize {
        match self.available_ram {
            r if r < 2 * GB => 0,
            r if r < 4 * GB => 1 * GB,
            r if r < 8 * GB => 2 * GB,
            r if r < 16 * GB => 4 * GB,
            _ => 8 * GB,
        }
    }
}
```

**First boot with no model files.** On a completely fresh installation where `system/models/` is empty (no bundled models, no downloads):

1. AIRS starts in **degraded mode** — inference engine is idle, no model loaded.
2. All intelligence services fall back to rule-based operation:
   - Context Engine: time-of-day heuristics (see [context-engine.md §8](../context-engine.md))
   - Attention Manager: keyword + category triage (see [attention.md §15.2](../attention.md))
   - Space Indexer: metadata extraction only (file type, size, dates — no semantic embeddings)
   - Intent Verifier: disabled (capabilities enforced by kernel regardless)
   - Behavioral Monitor: disabled (no baseline to compare against)
3. The system is fully usable but not intelligent. The user sees a prompt in the Conversation Bar: "Download an AI model to enable intelligent features" with a one-tap action.
4. When connected to the network, AIRS can download the recommended model for the device's RAM tier. The download progress is shown in the Status Strip.
5. Once downloaded, AIRS loads the model and transitions from degraded to normal operation. No reboot required — services hot-switch from rule-based to AI-backed.

**Model file integrity.** At boot, AIRS verifies each model file's SHA-256 hash against the hash stored in the `ModelEntry` space object. If a model file is corrupted (hash mismatch), AIRS skips it and tries the next candidate. If all local models are corrupted, AIRS enters degraded mode and logs a warning to `system/audit/airs/`.
