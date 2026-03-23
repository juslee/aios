# AIRS Kit

**Layer:** Intelligence | **Crate:** `aios_airs` | **Architecture:** [`docs/intelligence/airs.md`](../../intelligence/airs.md)

## 1. Overview

The AI Runtime Service (AIRS) Kit is the inference engine at the core of AIOS. It manages
model loading and lifecycle, schedules inference requests across available compute (CPU NEON
SIMD, GPU, NPU), and delivers streaming token output with backpressure and cancellation.
AIRS is to intelligence what the kernel is to resource management: invisible infrastructure
that makes everything else smarter. Every subsystem that exhibits adaptive behavior -- context
inference, semantic search, attention scoring, behavioral monitoring, intent verification,
preference NLU -- depends on AIRS Kit for its ML capabilities.

Unlike cloud AI APIs, AIRS runs entirely on-device. Models are stored in GGUF format within
Space Storage, loaded into dedicated model memory regions with PagedAttention for KV cache
management, and scheduled across heterogeneous compute resources. The Model Registry manages
a catalog of system and user-installed models, routing inference requests to the most capable
model available for each task profile (e.g., embedding generation vs. conversational LLM vs.
lightweight classifier). When hardware accelerators are present, the Compute Scheduler
dispatches work to GPU or NPU; when they are not, AIRS falls back to CPU with NEON SIMD
without any API change to consumers.

You would use AIRS Kit when building agents that need on-device inference -- generating
embeddings, running classifiers, performing NLU, or hosting conversational AI sessions. You
would *not* use AIRS Kit for simple rule-based logic, static configuration, or tasks that do
not require ML. Every AIRS call consumes inference budget (tokens, compute time, memory), so
agents should prefer deterministic logic where ML is unnecessary.

## 2. Core Traits

```rust
use aios_capability::CapabilityHandle;
use aios_compute::ComputeDevice;
use aios_storage::SpaceId;

/// The primary interface for submitting inference requests.
///
/// InferenceEngine routes requests to the appropriate model based on
/// the task profile, manages compute scheduling, and returns results
/// either as a complete batch or as a streaming token sequence.
pub trait InferenceEngine {
    /// Submit a batch inference request and block until completion.
    fn infer(&self, request: InferenceRequest) -> Result<InferenceResult, AirsError>;

    /// Submit a streaming inference request, returning a token stream.
    fn infer_stream(&self, request: InferenceRequest) -> Result<StreamingOutput, AirsError>;

    /// Generate embeddings for a batch of text inputs.
    fn embed(&self, texts: &[&str], model: Option<&ModelId>) -> Result<Vec<Embedding>, AirsError>;

    /// Cancel a running inference session.
    fn cancel(&self, session: &SessionId) -> Result<(), AirsError>;

    /// Query the current inference load and available capacity.
    fn capacity(&self) -> InferenceCapacity;
}

/// A scoped inference session with its own KV cache, token budget, and lifecycle.
///
/// Sessions persist context across multiple inference calls, enabling
/// multi-turn conversations and incremental document processing without
/// re-encoding the full context each time.
pub trait InferenceSession {
    /// The session's unique identifier.
    fn id(&self) -> &SessionId;

    /// The model bound to this session.
    fn model(&self) -> &ModelId;

    /// Current token usage within this session.
    fn token_usage(&self) -> TokenUsage;

    /// The remaining token budget before the session must be extended or closed.
    fn remaining_budget(&self) -> u64;

    /// Submit a turn within this session (preserves KV cache from prior turns).
    fn submit(&mut self, input: &str) -> Result<StreamingOutput, AirsError>;

    /// Fork this session, creating a new session with a copy of the current KV cache.
    fn fork(&self) -> Result<Box<dyn InferenceSession>, AirsError>;

    /// Close the session, releasing its KV cache and compute reservation.
    fn close(self) -> Result<SessionSummary, AirsError>;
}

/// Manages the catalog of available models.
///
/// The Model Registry stores models in Space Storage, tracks their
/// capability profiles, and handles LRU eviction when model memory
/// pressure rises.
pub trait ModelRegistry {
    /// List all available models, optionally filtered by task profile.
    fn list(&self, filter: Option<TaskProfile>) -> Result<Vec<ModelInfo>, AirsError>;

    /// Load a model into memory, returning when it is ready for inference.
    fn load(&self, model: &ModelId) -> Result<(), AirsError>;

    /// Evict a model from memory (does not delete from storage).
    fn evict(&self, model: &ModelId) -> Result<(), AirsError>;

    /// Query a model's current state (loaded, evicted, downloading).
    fn status(&self, model: &ModelId) -> Result<ModelStatus, AirsError>;

    /// Resolve the best model for a given task profile on current hardware.
    fn resolve(&self, profile: TaskProfile) -> Result<ModelId, AirsError>;

    /// Register a user-installed model from a Space path.
    fn register(&self, path: &SpacePath, manifest: ModelManifest) -> Result<ModelId, AirsError>;
}

/// Async token-by-token delivery with backpressure and cancellation.
pub trait StreamingOutput {
    /// Read the next token. Returns `None` when generation is complete.
    fn next_token(&mut self) -> Result<Option<Token>, AirsError>;

    /// Collect all remaining tokens into a single string.
    fn collect_all(&mut self) -> Result<String, AirsError>;

    /// Cancel the stream, stopping generation immediately.
    fn cancel(&mut self) -> Result<(), AirsError>;

    /// Check whether the stream has been cancelled or completed.
    fn is_done(&self) -> bool;
}

/// Per-session resource metering.
pub trait InferenceMeter {
    /// Total tokens consumed (prompt + generated) in this session.
    fn total_tokens(&self) -> u64;

    /// Wall-clock latency for the last inference call.
    fn last_latency(&self) -> Duration;

    /// Tokens generated per second (throughput).
    fn tokens_per_second(&self) -> f32;

    /// Compute budget consumed by this session (normalized 0.0-1.0).
    fn budget_consumed(&self) -> f32;
}

/// Task profiles used to select the appropriate model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskProfile {
    /// Conversational LLM (multi-turn dialogue).
    Conversation,
    /// Text embedding generation (384-dim vectors).
    Embedding,
    /// Lightweight classification (context, urgency, intent).
    Classifier,
    /// Natural language understanding (settings parsing, query interpretation).
    Nlu,
    /// Code generation and analysis.
    Code,
    /// Summarization and content extraction.
    Summary,
    /// Custom task profile with model hints.
    Custom(String),
}

/// Information about a registered model.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub id: ModelId,
    pub name: String,
    pub profiles: Vec<TaskProfile>,
    pub parameters: u64,
    pub quantization: Quantization,
    pub memory_required: u64,
    pub status: ModelStatus,
}

/// Model quantization level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Quantization {
    F16,
    Q8_0,
    Q5_K_M,
    Q4_K_M,
    Q3_K_S,
    Q2_K,
}

/// Current state of a model in the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelStatus {
    Available,
    Loading { progress: u8 },
    Loaded,
    Evicted,
    Downloading { progress: u8 },
    Error(String),
}
```

## 3. Usage Patterns

**Minimal -- single-shot inference:**

```rust
use aios_airs::{AirsKit, InferenceRequest, TaskProfile};

let engine = AirsKit::engine()?;
let result = engine.infer(InferenceRequest {
    prompt: "Summarize: The quarterly earnings report shows...".into(),
    profile: TaskProfile::Summary,
    max_tokens: 256,
    temperature: 0.3,
    ..Default::default()
})?;

println!("{}", result.text);
```

**Realistic -- streaming conversation session:**

```rust
use aios_airs::{AirsKit, TaskProfile};

// Open a session -- AIRS picks the best conversational model
let mut session = AirsKit::open_session(TaskProfile::Conversation, SessionConfig {
    max_context_tokens: 4096,
    system_prompt: Some("You are a helpful coding assistant.".into()),
    ..Default::default()
})?;

// First turn
let mut stream = session.submit("Explain Rust lifetimes in simple terms")?;
while let Some(token) = stream.next_token()? {
    print!("{}", token.text);
}

// Second turn (KV cache preserved -- no re-encoding of prior context)
let mut stream = session.submit("Now show me an example with structs")?;
let response = stream.collect_all()?;
println!("\n{}", response);

// Check resource usage
let meter = session.meter();
println!("Tokens used: {}, throughput: {:.1} tok/s",
    meter.total_tokens(), meter.tokens_per_second());

session.close()?;
```

**Advanced -- embedding generation for search:**

```rust
use aios_airs::AirsKit;

let engine = AirsKit::engine()?;
let texts = &["quantum computing basics", "machine learning tutorial", "rust ownership"];
let embeddings = engine.embed(texts, None)?; // None = use default embedding model

// Each embedding is a 384-dimensional f32 vector
for (text, emb) in texts.iter().zip(embeddings.iter()) {
    println!("{}: {} dimensions", text, emb.dimensions());
}
```

> **Common Mistakes**
>
> - **Not closing sessions.** Open sessions hold KV cache memory. Always call `close()` or
>   use a scoped guard. Leaked sessions are reclaimed after a timeout, but this wastes memory.
> - **Using Conversation profile for single-shot tasks.** Conversational models are larger and
>   slower. Use `TaskProfile::Summary` or `TaskProfile::Classifier` for one-off tasks.
> - **Ignoring backpressure on streams.** If you do not consume tokens from `StreamingOutput`,
>   the generation buffer fills and blocks the inference thread. Always drain or cancel.
> - **Hardcoding model IDs.** Use `ModelRegistry::resolve()` with a `TaskProfile` instead.
>   The best model changes based on available hardware and installed models.

## 4. Integration Examples

**AIRS Kit + Search Kit -- embedding-powered semantic search:**

```rust
use aios_airs::AirsKit;
use aios_search::{SearchKit, SearchQuery, SearchScope};

// Generate a query embedding
let engine = AirsKit::engine()?;
let query_embedding = engine.embed(&["documents about project budgets"], None)?;

// Pass to Search Kit for nearest-neighbor lookup
let results = SearchKit::search(SearchQuery {
    text: "documents about project budgets".into(),
    embedding: Some(query_embedding[0].clone()),
    scope: SearchScope::AllSpaces,
    limit: 10,
})?;

for result in results {
    println!("{}: {:.2} relevance", result.title, result.score);
}
```

**AIRS Kit + Context Kit -- activity classification:**

```rust
use aios_airs::{AirsKit, InferenceRequest, TaskProfile};
use aios_context::{ContextKit, ContextSignal};

// Context Kit collects signals and uses AIRS for classification
let signals = ContextKit::current_signals()?;
let feature_vector = signals.to_feature_vector();

let result = AirsKit::engine()?.infer(InferenceRequest {
    prompt: format!("Classify activity: {:?}", feature_vector),
    profile: TaskProfile::Classifier,
    max_tokens: 16,
    ..Default::default()
})?;

// Result: "deep_work" / "browsing" / "communication" / "media" / "idle"
```

**AIRS Kit + Capability Kit -- scoped inference access:**

```rust
use aios_airs::AirsKit;
use aios_capability::CapabilityKit;

// Check inference budget before starting
let cap = CapabilityKit::check("InferenceAccess")?;
let budget = AirsKit::engine()?.capacity();

if budget.available_tokens < 1000 {
    // Fall back to deterministic logic when inference budget is low
    return handle_without_ai();
}

let result = AirsKit::engine()?.infer(request)?;
```

## 5. Capability Requirements

| Method | Required Capability | Notes |
| --- | --- | --- |
| `InferenceEngine::infer` | `InferenceAccess` | Per-request token metering |
| `InferenceEngine::infer_stream` | `InferenceAccess` | Same budget as batch |
| `InferenceEngine::embed` | `InferenceAccess` | Lower cost than generative inference |
| `InferenceSession::submit` | `InferenceAccess` | Budget checked per turn |
| `InferenceSession::fork` | `InferenceAccess` | Forked session shares parent budget |
| `ModelRegistry::list` | `ModelRead` | Read-only catalog access |
| `ModelRegistry::load` | `ModelManage` | Restricted; system models only |
| `ModelRegistry::register` | `ModelManage` | User-installed models |
| `InferenceMeter::*` | `InferenceAccess` | Own session metrics always visible |

```toml
# Agent manifest example
[capabilities.required]
InferenceAccess = { max_tokens_per_day = 50000 }

[capabilities.optional]
ModelRead = {}
```

## 6. Error Handling

```rust
/// Errors returned by AIRS Kit operations.
#[derive(Debug)]
pub enum AirsError {
    /// No model available for the requested task profile.
    NoModelAvailable(TaskProfile),

    /// The inference token budget for this agent has been exhausted.
    BudgetExhausted { used: u64, limit: u64 },

    /// The model failed to load (corrupt, incompatible, or out of memory).
    ModelLoadFailed { model: ModelId, reason: String },

    /// The inference session has expired or was closed.
    SessionExpired(SessionId),

    /// The KV cache is full and cannot accept more context.
    ContextOverflow { capacity: u64, requested: u64 },

    /// The streaming output was cancelled by the caller or system.
    Cancelled,

    /// The required capability was not granted.
    CapabilityDenied(String),

    /// Hardware compute resource is unavailable (GPU/NPU offline).
    ComputeUnavailable(String),

    /// The AIRS service is not running (early boot or disabled).
    ServiceUnavailable,

    /// Internal inference error (GGML runtime failure).
    InternalError(String),
}
```

**Recovery guidance:**

| Error | Recovery |
| --- | --- |
| `NoModelAvailable` | Check `ModelRegistry::list()` for installed models; prompt user to install one |
| `BudgetExhausted` | Fall back to deterministic logic; budget resets at the next day boundary |
| `ContextOverflow` | Fork the session with summarized context, or close and start fresh |
| `ServiceUnavailable` | Use static/rule-based fallback; AIRS is not yet loaded or is disabled |
| `ComputeUnavailable` | AIRS retries on CPU automatically; this error means even CPU is overloaded |

## 7. Platform & AI Availability

AIRS Kit is the foundation of AI availability on AIOS. When AIRS itself is unavailable, all
dependent Kits degrade to their non-AI fallbacks.

**Hardware scaling:**

| Platform | Compute Path | Typical Throughput | Notes |
| --- | --- | --- | --- |
| QEMU virt | CPU (emulated NEON) | ~2 tok/s | Testing only; Q2_K models |
| Raspberry Pi 4 | CPU (Cortex-A72 NEON) | ~5-8 tok/s | Q4_K_M 1-3B models |
| Raspberry Pi 5 | CPU (Cortex-A76 NEON) | ~10-15 tok/s | Q4_K_M 3-7B models |
| Apple Silicon | CPU + GPU + ANE | ~30-80 tok/s | Q5_K_M 7-13B models |

**Feature availability:**

| Feature | AIRS Available | AIRS Unavailable |
| --- | --- | --- |
| Conversational inference | Full streaming LLM | Error: `ServiceUnavailable` |
| Embedding generation | On-device 384-dim vectors | Error: `ServiceUnavailable` |
| Classification | ML classifier (~1ms) | Consumers fall back to rules |
| Model management | Full registry + LRU | Static model list from disk |
| Session management | KV cache + multi-turn | Not available |

**Feature detection pattern:**

```rust
use aios_airs::AirsKit;

match AirsKit::engine() {
    Ok(engine) => {
        // AIRS available -- use ML inference
        let result = engine.infer(request)?;
        process_ml_result(result)
    }
    Err(AirsError::ServiceUnavailable) => {
        // AIRS not loaded -- use deterministic fallback
        process_with_rules()
    }
    Err(e) => return Err(e.into()),
}
```

**Implementation phase:** Phase 9+. AIRS Kit depends on [Memory Kit](../kernel/memory.md) for
model region allocation, [Compute Kit](../kernel/compute.md) for hardware dispatch,
[Storage Kit](../platform/storage.md) for model blob storage, and
[Capability Kit](../kernel/capability.md) for access control.

---

*See also: [Search Kit](search.md) | [Context Kit](context.md) | [Attention Kit](attention.md) | [Compute Kit](../kernel/compute.md) | [Memory Kit](../kernel/memory.md)*
