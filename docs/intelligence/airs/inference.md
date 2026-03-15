# AIOS AIRS Inference Engine

Part of: [airs.md](../airs.md) — AI Runtime Service
**Related:** [model-registry.md](./model-registry.md) — Model storage and selection, [scaling.md](./scaling.md) — Hardware scaling and NPU integration

-----

## 3. Inference Engine

The inference engine runs local LLM inference. No cloud dependency. All inference happens on-device.

### 3.1 Runtime: GGML with NEON SIMD

GGML is the inference runtime — a C library purpose-built for running quantized language models on consumer hardware. AIOS wraps it in a Rust safety layer:

```rust
pub struct InferenceEngine {
    runtime: GgmlRuntime,
    active_sessions: HashMap<SessionId, InferenceSession>,
    compute_scheduler: ComputeScheduler,
    kv_cache_pool: KvCachePool,
}

pub struct InferenceSession {
    id: SessionId,
    model: ModelHandle,
    kv_cache: KvCache,
    agent: AgentId,
    priority: InferencePriority,
    token_callback: TokenCallback,    // streaming output
    max_tokens: u32,
    temperature: f32,
    stop_sequences: Vec<String>,
}

pub enum InferencePriority {
    /// User is waiting for a response (conversation bar)
    Interactive,
    /// System service needs inference (intent verification, context engine)
    System,
    /// Background task (space indexing, metadata generation)
    Background,
    /// Scheduled batch work (re-indexing, summarization)
    Batch,
}

/// Cost-aware inference metering — tracks per-agent token usage
/// and enforces budgets. Conceptually inspired by OpenFang's cost metering
/// and GCRA rate limiting, but this implementation aggregates usage per
/// agent (not per model). See: https://github.com/RightNow-AI/openfang for
/// a per-model tracking example.
pub struct InferenceMeter {
    /// Per-agent cumulative token usage
    agent_usage: HashMap<AgentId, TokenUsage>,
    /// Per-agent budget limits (None = unlimited for system agents)
    agent_budgets: HashMap<AgentId, Option<TokenBudget>>,
    /// GCRA rate limiter: prevents burst inference that starves other agents
    rate_limiter: GcraRateLimiter,
}

pub struct TokenUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    /// Estimated cost based on model quantization and compute time
    compute_cost: Duration,
}

pub struct TokenBudget {
    /// Maximum tokens per scheduling window
    max_tokens_per_window: u64,
    /// Window duration (e.g., 1 hour)
    window: Duration,
    /// Action when budget exceeded
    exceeded_policy: BudgetPolicy,
}

pub enum BudgetPolicy {
    /// Queue requests until next window
    Queue,
    /// Downgrade to smaller/faster model
    Downgrade,
    /// Reject with error
    Reject,
}
```

**Why GGML, not a full ML framework:**

- Designed specifically for LLM inference on consumer hardware
- Optimized quantization (Q4_K_M, Q5_K_M, Q6_K) — 7B models fit in 4-6 GB RAM
- NEON SIMD optimizations for aarch64 (the only architecture AIOS targets)
- No Python dependency, no CUDA dependency, no GPU required (GPU optional)
- C library with stable ABI — straightforward Rust FFI binding

### 3.2 Compute Scheduler

Multiple services need inference simultaneously. The Compute Scheduler allocates compute resources. Rather than maintaining its own device list, the scheduler queries the kernel's ComputeRegistry ([compute/registry.md](../../kernel/compute/registry.md) §5) for available devices and their capabilities:

```rust
pub struct ComputeScheduler {
    /// Reference to the kernel's centralized compute device registry.
    /// AIRS queries this for device capabilities, utilization, and
    /// thermal state — it does not maintain a separate device list.
    /// See compute/classification.md §3 for ComputeDevice trait.
    registry: ComputeRegistryHandle,
    queue: PriorityQueue<InferenceRequest>,
    active: Vec<ActiveInference>,
    policy: SchedulingPolicy,
}

/// Compute device classes that the scheduler can target.
/// Maps to kernel ComputeClass (compute/classification.md §3.2).
pub enum ComputeDeviceClass {
    Cpu {
        cores: u32,
        neon: bool,           // NEON SIMD available (always true on aarch64)
        threads_available: u32,
    },
    Gpu {
        memory: usize,
        compute_units: u32,
        api: GpuApi,          // Vulkan, Metal (future)
    },
    Npu {
        tops: f32,            // tera-operations per second
        supported_formats: Vec<QuantFormat>,
    },
    Dsp {
        macs_per_cycle: u32,  // multiply-accumulate ops per cycle
    },
}

pub struct SchedulingPolicy {
    /// Interactive requests preempt background work
    preemption: bool,
    /// Maximum concurrent inference sessions
    max_concurrent: u32,
    /// Memory budget for KV caches
    kv_cache_budget: usize,
    /// Background inference throttle (% of compute)
    background_throttle: f32,
}
```

**Scheduling rules:**

1. Interactive requests (user waiting) always preempt background work
2. System requests (security services) get second priority
3. Background requests (indexing) use remaining compute
4. If memory is exhausted, oldest background KV cache is evicted
5. NPU is preferred for small models; GPU for large models; CPU as fallback

### 3.3 KV Cache Management

KV (key-value) caches are the memory cost of keeping a conversation context. Each active inference session has a KV cache proportional to context length:

```rust
pub struct KvCachePool {
    allocated: HashMap<SessionId, KvCache>,
    total_budget: usize,           // total bytes available for KV caches
    eviction_order: LruList<SessionId>,
}

pub struct KvCache {
    session: SessionId,
    model: ModelId,
    context_length: u32,           // current token count
    max_context: u32,              // model's max (e.g., 8192)
    memory_bytes: usize,           // actual memory used
    last_used: Timestamp,
}
```

**Eviction policy:** LRU with priority weighting. Background session caches are evicted before system session caches, which are evicted before interactive session caches. When a conversation bar session is idle for >5 minutes, its KV cache is compressed to disk (the conversation history is still in a space — the cache can be reconstructed).

### 3.4 Streaming Output

All inference produces streaming output — tokens are delivered one at a time as they're generated:

```rust
pub trait TokenCallback: Send {
    /// Called for each generated token
    fn on_token(&mut self, token: &str) -> TokenAction;

    /// Called when generation is complete
    fn on_complete(&mut self, stats: InferenceStats);

    /// Called on error
    fn on_error(&mut self, error: InferenceError);
}

pub enum TokenAction {
    Continue,           // keep generating
    Stop,               // stop generation (user cancelled, stop sequence hit)
    Pause(Duration),    // pause briefly (backpressure from consumer)
}

pub struct InferenceStats {
    tokens_generated: u32,
    tokens_per_second: f32,
    time_to_first_token: Duration,
    total_time: Duration,
    model_used: ModelId,
    compute_device: ComputeDevice,
}
```

The conversation bar displays tokens as they arrive. Intent verification streams tokens internally for speed. Space indexing discards tokens and only keeps the final result.
