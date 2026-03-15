# AIOS AIRS Hardware Scaling

Part of: [airs.md](../airs.md) — AI Runtime Service
**Related:** [inference.md](./inference.md) — Inference engine, [model-registry.md](./model-registry.md) — Model storage and selection, [ai-native.md](./ai-native.md) — AI-native intelligence

-----

## 11. Future: Scaling with Hardware

### 11.1 Model Capability Trajectory

As SBC (single-board computer) RAM grows and model efficiency improves, AIRS capabilities scale with hardware:

```text
Hardware         Model Pool   What Fits                           User Experience
──────────────   ──────────   ──────────────────────────────      ────────────────────────
2 GB (degraded)    0 MB       Cloud inference only                Basic, connectivity-dependent
4 GB (current)     2 GB       3B Q4 general-purpose               Functional AI, limited reasoning
8 GB (target)      4 GB       8B Q4 + embedding model             Full AI-native experience
16 GB (near)       8 GB       8B Q5 + code specialist + vision    Multi-model, no switching
32 GB (future)    16 GB       13B Q4 + 3 specialists loaded       Desktop-class AI
64 GB (future)    32 GB       70B Q4 or 13B F16 + specialists     Near-cloud-quality local AI
```

### 11.2 Multi-Model Architecture

As RAM grows, AIRS evolves from single-model switching to multi-model concurrency:

**Phase 1 (4-8 GB) — Single model, serial switching:**
The current design. One primary model loaded at a time. Specialist tasks require eviction and reload. Acceptable on 8 GB, limiting on 4 GB.

**Phase 2 (16 GB) — Primary + specialists:**
Primary model stays resident. 1-2 small specialists (code, vision, embedding) loaded alongside. Most tasks are handled without any model switching. AIRS routes based on task type.

**Phase 3 (32+ GB) — Model ensemble:**
Multiple full-size models loaded simultaneously. AIRS routes each request to the best specialist. Intent verification uses a dedicated security model. Code generation uses a code-tuned model. Vision tasks use a multimodal model. Conversation uses a general-purpose model. Zero switching latency for any task type.

```rust
pub struct ModelEnsemble {
    /// All currently loaded models with their specializations
    loaded: Vec<(ModelId, Vec<TaskType>)>,
    /// Routing table: task type → preferred model → fallback chain
    routing: HashMap<TaskType, Vec<ModelId>>,
    /// Total model memory used
    total_memory: usize,
    /// Budget remaining for additional models
    budget_remaining: usize,
}

impl ModelEnsemble {
    pub fn route(&self, task: TaskType) -> ModelId {
        // Return the best loaded model for this task type
        // Falls back through the chain if preferred model is busy
        self.routing.get(&task)
            .and_then(|chain| chain.iter()
                .find(|id| self.is_available(id))
                .copied())
            .unwrap_or(self.primary())
    }
}
```

### 11.3 Longer Context Windows

More RAM directly enables longer conversations and richer context:

| Device RAM | Practical Context | What It Enables |
|---|---|---|
| 4 GB | 4K-8K tokens | Short conversations, single-page documents |
| 8 GB | 8K-32K tokens | Multi-turn conversations, short documents |
| 16 GB | 32K-128K tokens | Extended conversations, full documents, rich system context |
| 32 GB+ | 128K-256K+ tokens | Entire codebases in context, book-length documents, persistent agent memory |

Longer context windows reduce the need for context compression (§5.8 in [intelligence-services.md](./intelligence-services.md)) and allow system services (intent verifier, behavioral monitor, context engine) to maintain richer working memory, improving their accuracy.

### 11.4 NPU and Accelerator Integration

Future SBCs increasingly include Neural Processing Units (NPUs) and dedicated ML accelerators. AIRS's compute scheduler is already designed for heterogeneous compute:

```text
Current (Pi 5):       CPU (NEON SIMD) — 4-8 tok/s for 8B model
Near future:          CPU + NPU (Rockchip RK3588: 6 TOPS) — 15-30 tok/s
Future:               CPU + NPU + GPU compute — 40-100+ tok/s
```

The `ComputeDevice` enum (§3.2 in [inference.md](./inference.md)) already includes NPU as a variant. When NPU drivers are available through the subsystem framework, the compute scheduler routes small models and embedding generation to the NPU (where fixed-point arithmetic excels) and keeps large model inference on CPU/GPU.
