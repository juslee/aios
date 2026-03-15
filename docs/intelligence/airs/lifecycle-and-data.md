# AIOS AIRS Agent Lifecycle, Data Model & Technology Choices

Part of: [airs.md](../airs.md) — AI Runtime Service
**Related:** [intelligence-services.md](./intelligence-services.md) — Intelligence services, [security.md](./security.md) — Resource orchestration security, [../../applications/agents.md](../../applications/agents.md) — Agent framework

-----

## 6. Agent Lifecycle

AIRS manages the full lifecycle of agents — from manifest analysis to runtime monitoring:

```text
1. Agent submitted (from store or local development)
     ↓
2. AIRS static security analysis:
   - Capability requests reviewed (are they reasonable for declared purpose?)
   - Code scanned for known vulnerability patterns
   - Dependency chain verified (all content hashes match)
   - Risk score assigned (low/medium/high)
     ↓
3. User approval:
   - Capabilities shown in plain language
   - Risk score displayed
   - User approves or denies each capability individually
     ↓
4. Agent spawned:
   - Capability tokens minted by kernel
   - Process created with restricted address space
   - IPC channels established
   - Spaces mounted (read-only or read-write per capability)
     ↓
5. Runtime monitoring:
   - Intent verification (Layer 1) on every action
   - Behavioral monitoring (Layer 3) builds baseline
   - All actions logged to audit space
     ↓
6. Agent termination:
   - Sessions closed gracefully
   - Capability tokens revoked
   - Audit summary generated
   - Space data preserved (belongs to user, not agent)
```

-----

## 7. Data Model

```rust
/// AIRS configuration
pub struct AirsConfig {
    model_directory: SpacePath,         // system/models/
    index_directory: SpacePath,         // system/index/
    default_model: ModelId,
    embedding_model: ModelId,
    max_model_memory: usize,            // bytes
    max_concurrent_sessions: u32,
    background_indexing: bool,
    context_engine_mode: ContextModelMode,
}

/// Inference request from any service or agent
pub struct InferenceRequest {
    requester: AgentId,
    priority: InferencePriority,
    model: Option<ModelId>,             // None = use default
    prompt: Prompt,
    parameters: InferenceParameters,
    callback: Box<dyn TokenCallback>,
}

pub struct InferenceParameters {
    max_tokens: u32,
    temperature: f32,
    top_p: f32,
    stop_sequences: Vec<String>,
    system_prompt: Option<String>,
}

pub struct Prompt {
    messages: Vec<Message>,
    context_objects: Vec<ObjectId>,     // injected as context
}

pub struct Message {
    role: Role,
    content: String,
}

pub enum Role {
    System,
    User,
    Assistant,
    Tool { name: String },
}
```

-----

## 8. Key Technology Choices

| Component | Choice | License | Rationale |
|---|---|---|---|
| Inference runtime | GGML / llama.cpp | MIT | Purpose-built for local LLM inference on consumer hardware |
| Model format | GGUF | MIT | Standard format for quantized models, metadata-rich |
| SIMD | NEON (aarch64) | — | Only architecture we target, maximum optimization |
| Embedding index | HNSW (custom) | BSD-2-Clause | Fast approximate nearest-neighbor for semantic search |
| Full-text index | Custom inverted index | BSD-2-Clause | BM25 ranking, always available without AIRS |
| Tokenizer | Sentencepiece / tiktoken | Apache-2.0 | Per-model tokenization, no Python dependency |
