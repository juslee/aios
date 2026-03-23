# Conversation Kit

**Layer:** Application | **Crate:** `aios_conversation` | **Architecture:** [`docs/intelligence/conversation-manager.md`](../../intelligence/conversation-manager.md)

## 1. Overview

Conversation Kit is the primary interface between users and AIRS. It manages the full
lifecycle of a conversation -- session creation, context window assembly, streaming token
delivery, tool orchestration, and cross-device continuity. Every interaction a user has
with the system's AI capabilities flows through Conversation Kit, whether initiated from
the Conversation Bar, a voice command, an agent's tool call, or a programmatic API.

The Kit's central abstraction is the `ConversationSession`, a persistent, forkable object
that accumulates turns of dialogue along with the context needed to produce coherent
responses. Each session maintains a token budget that Conversation Kit fills from multiple
sources: conversation history, RAG results from Search Kit, ambient signals from Context
Kit, and structured output from previous tool invocations. The context window assembly
pipeline compresses, prioritizes, and truncates these sources to fit the active model's
token limit while preserving semantic coherence.

Tool orchestration is the second major responsibility. When AIRS decides to invoke a tool
during generation, Conversation Kit intercepts the tool-call tokens, validates the tool
against the session's capability set, dispatches execution through Tool Manager (via AIRS
Kit), and injects the result back into the generation stream. Tool chains -- where one
tool's output feeds into the next -- are managed as sub-turns within the same session,
with full provenance tracking. The Conversation Bar, the system-wide UI surface for AI
interaction, is built on top of Conversation Kit's streaming delivery pipeline and renders
through Interface Kit's compositor integration.

## 2. Core Traits

```rust
use aios_capability::CapabilityHandle;
use aios_airs::InferenceSession;
use aios_context::ContextSnapshot;
use aios_search::SearchResult;
use aios_flow::FlowEntry;

/// A persistent conversation with history, forking, and cross-device sync.
///
/// Sessions are stored in a Space and survive reboots. A session can be
/// forked to explore alternative conversation branches without losing
/// the original thread.
pub trait ConversationSession {
    /// The unique identifier for this session.
    fn id(&self) -> SessionId;

    /// Add a user message to the conversation.
    fn add_user_turn(&mut self, message: UserMessage) -> Result<TurnId, ConversationError>;

    /// Begin generating an assistant response, returning a streaming handle.
    /// The response is produced token-by-token via the `StreamingOutput` trait.
    fn generate(&mut self) -> Result<Box<dyn StreamingOutput>, ConversationError>;

    /// Fork this session at the given turn, creating an independent branch.
    fn fork(&self, at_turn: TurnId) -> Result<Box<dyn ConversationSession>, ConversationError>;

    /// List all turns in chronological order.
    fn history(&self) -> &[Turn];

    /// Search through this session's history for matching turns.
    fn search_history(&self, query: &str) -> Result<Vec<Turn>, ConversationError>;

    /// Persist the session to storage. Called automatically on turn boundaries
    /// and periodically during long generations.
    fn save(&self) -> Result<(), ConversationError>;

    /// Close the session, releasing inference resources.
    fn close(self) -> Result<(), ConversationError>;
}

/// Assembles the token budget from history, RAG, context, and tool outputs.
///
/// The context window is the single most important input to the model.
/// ContextWindow manages the assembly pipeline that determines what the
/// model sees on each generation request.
pub trait ContextWindow {
    /// Return the total token budget for the active model.
    fn budget(&self) -> TokenBudget;

    /// Return the current token usage breakdown by source.
    fn usage(&self) -> ContextUsage;

    /// Add a RAG result to the context window.
    fn inject_search_result(&mut self, result: SearchResult) -> Result<(), ConversationError>;

    /// Add ambient context signals (time, location, active app, etc.).
    fn inject_context(&mut self, snapshot: ContextSnapshot) -> Result<(), ConversationError>;

    /// Add a tool invocation result to the context window.
    fn inject_tool_result(&mut self, result: ToolResult) -> Result<(), ConversationError>;

    /// Compress older turns to free tokens for new content.
    /// Uses tiered compression: summarization > pruning > truncation.
    fn compress(&mut self) -> Result<TokensFreed, ConversationError>;

    /// Assemble the final token sequence for the model.
    fn assemble(&self) -> Result<TokenSequence, ConversationError>;
}

/// Discovers, validates, and invokes tools on behalf of AIRS during generation.
///
/// When the model emits a tool-call token sequence, ToolOrchestrator parses
/// the call, checks capabilities, dispatches execution, and returns the
/// result for injection back into the generation stream.
pub trait ToolOrchestrator {
    /// List tools available to the current session based on its capabilities.
    fn available_tools(&self) -> Vec<ToolDescriptor>;

    /// Execute a tool call, blocking until completion or timeout.
    fn execute(&mut self, call: ToolCall) -> Result<ToolResult, ConversationError>;

    /// Execute a chain of tool calls where each output feeds the next.
    fn execute_chain(&mut self, calls: &[ToolCall]) -> Result<Vec<ToolResult>, ConversationError>;

    /// Cancel an in-progress tool execution.
    fn cancel(&mut self, execution_id: ExecutionId) -> Result<(), ConversationError>;

    /// Return the provenance record for a completed tool execution.
    fn provenance(&self, execution_id: ExecutionId) -> Option<&ToolProvenance>;
}

/// Token-by-token delivery with backpressure and cancellation.
///
/// The consumer (Conversation Bar, API client, agent) reads tokens as
/// they arrive. If the consumer falls behind, backpressure signals the
/// inference engine to pause generation rather than buffering unboundedly.
pub trait StreamingOutput {
    /// Read the next token. Returns `None` when generation is complete.
    fn next_token(&mut self) -> Result<Option<Token>, ConversationError>;

    /// Read tokens in bulk (up to `max` at a time) for batch consumers.
    fn next_tokens(&mut self, max: usize) -> Result<Vec<Token>, ConversationError>;

    /// Signal that the consumer is not ready for more tokens.
    /// The inference engine will pause until `resume()` is called.
    fn pause(&mut self) -> Result<(), ConversationError>;

    /// Resume token delivery after a pause.
    fn resume(&mut self) -> Result<(), ConversationError>;

    /// Cancel generation entirely. Partially generated content is retained
    /// in the session history with a `cancelled` marker.
    fn cancel(&mut self) -> Result<PartialResponse, ConversationError>;

    /// Check whether a tool call is pending in the stream.
    /// When true, the orchestrator should call `ToolOrchestrator::execute`.
    fn pending_tool_call(&self) -> Option<&ToolCall>;

    /// Return streaming metadata (tokens generated, latency, model info).
    fn metadata(&self) -> StreamMetadata;
}

/// The system-wide Conversation Bar surface.
///
/// Rendered by Interface Kit as a compositor-integrated overlay. Supports
/// text input, voice input, structured output, and inline tool results.
pub trait ConversationBar {
    /// Show the Conversation Bar, optionally with a pre-filled prompt.
    fn show(&mut self, prefill: Option<&str>) -> Result<(), ConversationError>;

    /// Hide the Conversation Bar.
    fn hide(&mut self) -> Result<(), ConversationError>;

    /// Check whether the Conversation Bar is currently visible.
    fn is_visible(&self) -> bool;

    /// Set the active session displayed in the Bar.
    fn set_session(&mut self, session: &dyn ConversationSession) -> Result<(), ConversationError>;

    /// Register a handler for structured output rendering.
    fn on_structured_output(&mut self, handler: Box<dyn StructuredOutputHandler>);
}
```

## 3. Usage Patterns

**Minimal -- ask a question and print the response:**

```rust
use aios_conversation::{ConversationKit, SessionConfig};

let session = ConversationKit::create_session(SessionConfig::default())?;
session.add_user_turn(UserMessage::text("What is the capital of France?"))?;

let mut stream = session.generate()?;
while let Some(token) = stream.next_token()? {
    print!("{}", token.text);
}
println!();
```

**Realistic -- multi-turn conversation with context injection:**

```rust
use aios_conversation::{ConversationKit, SessionConfig, UserMessage};
use aios_context::ContextKit;
use aios_search::SearchKit;

let mut session = ConversationKit::create_session(SessionConfig {
    model: ModelPreference::Default,
    max_turns: 100,
    persist: true,
    ..Default::default()
})?;

// Inject ambient context (time of day, active application, etc.)
let context = ContextKit::current_snapshot()?;
session.context_window().inject_context(context)?;

// First turn
session.add_user_turn(UserMessage::text("Summarize my recent meeting notes"))?;

// The context window automatically pulls RAG results from Search Kit
// based on the user's query and the ambient context signals.
let mut stream = session.generate()?;
let response = drain_to_string(&mut stream)?;

// Second turn -- follow-up question
session.add_user_turn(UserMessage::text("What action items did we agree on?"))?;
let mut stream = session.generate()?;

// Handle streaming with tool calls
while let Some(token) = stream.next_token()? {
    if let Some(tool_call) = stream.pending_tool_call() {
        let result = session.tool_orchestrator().execute(tool_call.clone())?;
        session.context_window().inject_tool_result(result)?;
    }
    print!("{}", token.text);
}
```

**Advanced -- forking a conversation and comparing branches:**

```rust
use aios_conversation::ConversationKit;

let session = ConversationKit::open_session(session_id)?;
let turn_3 = session.history()[2].id;

// Fork at turn 3 to explore an alternative direction
let mut branch = session.fork(turn_3)?;
branch.add_user_turn(UserMessage::text("Actually, let's try a different approach..."))?;

let mut stream = branch.generate()?;
// The branch has its own independent history from turn 3 onward,
// but shares the compressed context of turns 1-3 with the original.
```

> **Common Mistakes**
>
> - **Ignoring backpressure.** If you consume tokens slower than the model generates them,
>   call `stream.pause()` to avoid unbounded buffering. Failing to do so wastes inference
>   compute on tokens the consumer may never display.
> - **Not handling `pending_tool_call()`.** Tool calls arrive inline in the token stream.
>   If you skip them, the model's response will be incoherent because the tool result
>   was never injected.
> - **Creating sessions without `persist: true`.** Ephemeral sessions are lost on reboot.
>   For user-facing conversations, always enable persistence.
> - **Manually assembling context windows.** Let the `ContextWindow` pipeline handle
>   compression and prioritization. Manual injection should be limited to app-specific
>   context that the automatic pipeline cannot discover.

## 4. Integration Examples

**Conversation Kit + AIRS Kit + Search Kit -- RAG-augmented conversation:**

```rust
use aios_conversation::{ConversationKit, SessionConfig};
use aios_search::SearchKit;

let mut session = ConversationKit::create_session(SessionConfig::default())?;
session.add_user_turn(UserMessage::text("What did I write about rust lifetimes?"))?;

// Before generation, the context window pipeline automatically:
// 1. Sends the query to Search Kit for semantic search across user's Spaces
// 2. Ranks results by relevance and recency
// 3. Injects top-K results into the context window
// 4. Compresses older turns if the token budget is tight

let mut stream = session.generate()?;
// The model's response cites specific documents from the user's Spaces,
// with provenance links that the Conversation Bar renders as clickable references.
```

**Conversation Kit + Flow Kit -- sharing conversation output:**

```rust
use aios_conversation::ConversationKit;
use aios_flow::{FlowKit, TypedContent};

let session = ConversationKit::open_session(session_id)?;
let last_response = session.history().last().unwrap();

// Share the assistant's response through Flow (system clipboard / cross-app)
FlowKit::publish(FlowEntry {
    content: TypedContent::RichText(last_response.text.clone()),
    source: FlowSource::Conversation(session.id()),
    ..Default::default()
})?;
// The response is now available in the clipboard and in Flow history,
// with provenance linking back to the original conversation.
```

**Conversation Kit + Context Kit + Attention Kit -- context-aware interruption:**

```rust
use aios_conversation::ConversationBar;
use aios_context::ContextKit;
use aios_attention::AttentionKit;

// Context Kit detects the user is in a focused work session
let context = ContextKit::current_snapshot()?;

if context.activity == Activity::DeepWork {
    // Attention Kit gates whether the Conversation Bar should interrupt
    let priority = AttentionKit::evaluate_priority(&notification)?;
    if priority < AttentionThreshold::Urgent {
        // Queue the notification for later rather than showing the Bar
        AttentionKit::defer(notification)?;
        return Ok(());
    }
}

// For urgent items, show the Conversation Bar with context pre-loaded
let mut bar = ConversationBar::instance()?;
bar.show(Some("Urgent: your flight has been delayed..."))?;
```

## 5. Capability Requirements

| Method | Required Capability | Notes |
| --- | --- | --- |
| `ConversationSession::generate` | `InferenceAccess` | Consumes inference compute budget |
| `ConversationSession::fork` | `InferenceAccess` + `StorageWrite` | Fork creates a new persisted session |
| `ConversationSession::save` | `StorageWrite` | Writes to the session's Space |
| `ContextWindow::inject_search_result` | `SearchRead` | RAG results require search capability |
| `ContextWindow::inject_context` | `ContextRead` | Ambient signals are privacy-sensitive |
| `ToolOrchestrator::execute` | `ToolExecution` + per-tool caps | Each tool has its own capability requirements |
| `StreamingOutput::next_token` | None | Reading from an active stream requires no extra cap |
| `StreamingOutput::cancel` | None | Consumer can always cancel their own stream |
| `ConversationBar::show` | `SurfaceCreate` | Bar is a compositor surface |
| `ConversationBar::set_session` | `InferenceAccess` | Switching sessions may trigger context reload |

## 6. Error Handling & Degradation

```rust
/// Errors returned by Conversation Kit operations.
#[derive(Debug)]
pub enum ConversationError {
    /// Inference engine is unavailable (model not loaded, compute exhausted).
    InferenceUnavailable,

    /// The session's token budget is exhausted and compression cannot free more.
    ContextOverflow { used: usize, budget: usize },

    /// A tool execution failed. The partial response is still available.
    ToolFailed { tool: ToolId, reason: String },

    /// The tool execution timed out.
    ToolTimeout { tool: ToolId, elapsed: Duration },

    /// The required capability was not granted.
    CapabilityDenied(Capability),

    /// The session was not found (deleted or not synced to this device).
    SessionNotFound(SessionId),

    /// The model rejected the input (content safety filter triggered).
    ContentFiltered { turn: TurnId, reason: String },

    /// Streaming was cancelled by the consumer.
    Cancelled(PartialResponse),

    /// Storage error while persisting the session.
    StorageFailed(StorageError),

    /// Cross-device sync conflict on the session.
    SyncConflict { local: TurnId, remote: TurnId },
}
```

**Fallback cascade:**

| Failure | Degradation |
| --- | --- |
| Primary model unavailable | Falls back to smaller on-device model with reduced capability |
| Context overflow | Aggressive compression (summarize all turns older than 5) |
| Tool execution fails | Model receives error message, can retry or explain the failure |
| Tool execution times out | Stream resumes with timeout notification injected |
| Search Kit unavailable | Generation proceeds without RAG augmentation |
| Context Kit unavailable | Generation proceeds without ambient context signals |
| Session storage fails | Session remains in memory; retry persistence on next turn |
| Cross-device sync conflict | Both branches preserved; user chooses which to keep |

## 7. Platform & AI Availability

**AIRS-enhanced features (require AIRS Kit):**

| Feature | What AIRS provides | Without AIRS |
| --- | --- | --- |
| Token generation | LLM inference for assistant responses | No generation; Kit is inert |
| Context compression | Semantic summarization of older turns | Truncation (drop oldest turns) |
| Tool selection | Model chooses which tools to invoke | No automatic tool invocation |
| Query rewriting | Reformulates search queries for better RAG | Verbatim user query sent to Search Kit |
| Conversation Bar | Streaming output with structured rendering | Text input/output only (no AI features) |
| Session summarization | Auto-generated session titles and summaries | Manual titles; no summaries |
| Multi-model routing | Selects optimal model per query complexity | Fixed model for all queries |

Conversation Kit is fundamentally dependent on AIRS Kit for its core functionality. Without
a loaded model, `generate()` returns `InferenceUnavailable`. The Kit gracefully handles
model unavailability at the edges (compression, summarization, RAG rewriting) but cannot
function as a conversation engine without inference.

**Platform availability:**

| Platform | Model Support | Context Window | Tool Orchestration | Notes |
| --- | --- | --- | --- | --- |
| QEMU virt | Small models only (CPU) | Full | Full | Testing; no GPU acceleration |
| Raspberry Pi 4 | 1-3B quantized | Limited (4K tokens) | Full | Memory-constrained |
| Raspberry Pi 5 | 3-7B quantized | Standard (8K tokens) | Full | 8GB RAM enables larger models |
| Apple Silicon | 7-70B models | Large (32K+ tokens) | Full | Neural Engine + unified memory |

**Implementation phase:** Phase 14+. Conversation Kit is one of the first intelligence-layer
features, built on top of AIRS Kit's inference engine (Phase 5+), Search Kit (Phase 12+),
and Context Kit (Phase 14+).

---

*See also: [AIRS Kit](../intelligence/airs.md) | [Context Kit](../intelligence/context.md) | [Search Kit](../intelligence/search.md) | [Flow Kit](../intelligence/flow.md) | [Attention Kit](../intelligence/attention.md) | [Interface Kit](interface.md)*
