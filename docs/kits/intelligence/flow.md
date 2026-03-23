# Flow Kit

**Layer:** Intelligence | **Crate:** `aios_flow` | **Architecture:** [`docs/storage/flow.md`](../../storage/flow.md) + 7 sub-docs

## 1. Overview

The Flow Kit is the unified data exchange layer in AIOS. It replaces the traditional clipboard,
drag-and-drop, and inter-agent file transfer with a single typed, history-preserving channel.
Every transfer passes through a content transform pipeline that converts between formats on
demand -- a terminal agent can receive rich text, a browser can receive raw code, and an image
editor can receive a PDF page as a raster image, all without the sending agent knowing
anything about the receiver's capabilities.

Flow entries are persisted with full provenance so users can revisit and replay past transfers
across sessions and devices. The history is searchable and syncs via Space Mesh. Content type
negotiation follows the BeOS Media Kit pattern: senders declare what they have, receivers
declare what they accept, and the transform pipeline bridges the gap automatically. Agents
register custom transforms to extend the conversion graph with domain-specific formats.

Use the Flow Kit when your agent needs to exchange data with other agents (copy/paste,
drag-and-drop, share sheets), publish content to the system clipboard, or replay a previous
transfer from history. Do not use it for persistent storage (use the
[Storage Kit](../platform/storage.md)) or for point-to-point messaging between specific
agents (use IPC channels directly).

## 2. Core Traits

```rust
use aios_flow::{
    FlowChannel, FlowEntry, FlowHistory,
    TransformPipeline, TypedContent, TransferIntent,
};
use aios_capability::CapabilityHandle;

/// Publish and receive data through typed flow channels.
///
/// FlowChannel is the primary interface for agents participating in data
/// exchange. Agents publish content they want to share and subscribe to
/// channels to receive content from others. The system clipboard is a
/// built-in channel named "clipboard".
pub trait FlowChannel {
    /// Publish content to this channel.
    ///
    /// The content is typed (MIME-like content type plus payload). The
    /// transform pipeline may convert it to a different format if the
    /// receiver requires it. The transfer is recorded in FlowHistory
    /// with full provenance.
    fn publish(
        &self,
        content: TypedContent,
        intent: TransferIntent,
        cap: &CapabilityHandle,
    ) -> Result<FlowEntryId, FlowError>;

    /// Receive the most recent content from this channel, optionally
    /// requesting conversion to a preferred type.
    ///
    /// If `preferred_type` is set and differs from the published type,
    /// the transform pipeline attempts conversion. Returns `None` if
    /// the channel is empty.
    fn receive(
        &self,
        preferred_type: Option<&str>,
        cap: &CapabilityHandle,
    ) -> Result<Option<FlowEntry>, FlowError>;

    /// Subscribe to content notifications on this channel. The returned
    /// handle receives a callback whenever new content is published.
    fn subscribe(
        &self,
        cap: &CapabilityHandle,
    ) -> Result<FlowSubscription, FlowError>;

    /// List content types currently available on this channel.
    fn available_types(&self) -> Vec<String>;
}

/// A single transfer unit with typed payload and provenance.
pub struct FlowEntry {
    /// Unique identifier for this entry.
    pub id: FlowEntryId,
    /// The agent that published this content.
    pub source_agent: AgentId,
    /// The agent that received it (None if unclaimed).
    pub destination_agent: Option<AgentId>,
    /// The typed content payload.
    pub content: TypedContent,
    /// What the sender intended (Copy, Move, Reference, Quote, Derive).
    pub intent: TransferIntent,
    /// Transforms applied during delivery.
    pub transformations: Vec<TransformRecord>,
    /// When the transfer was initiated.
    pub initiated_at: Timestamp,
    /// Provenance chain linking to source objects.
    pub provenance: ProvenanceLink,
}

/// Typed content with MIME-like content type and payload.
///
/// Content can be inline (small payloads up to 64 KB) or backed by shared
/// memory (large payloads). The transform pipeline operates on either form.
pub struct TypedContent {
    /// The primary content payload.
    pub primary: ContentPayload,
    /// Standard MIME type (e.g., "text/plain", "image/png", "application/pdf").
    pub mime_type: String,
    /// AIOS semantic type -- richer than MIME, used for type negotiation.
    pub semantic_type: SemanticType,
    /// Same content in alternative formats, pre-computed by source.
    pub alternatives: Vec<ContentPayload>,
}

/// Transfer intent declares what the sender meant by this transfer.
pub enum TransferIntent {
    /// Standard copy -- source object is unchanged.
    Copy,
    /// Move -- source object should be deleted after delivery.
    Move,
    /// Reference -- only a link to the source, not the data itself.
    Reference,
    /// Quote -- embed source content with attribution.
    Quote,
    /// Derive -- create a new object based on the source.
    Derive,
}

/// Persistent, searchable history of past flow entries.
///
/// Flow history is stored in the `system/flow-history/` Space and synced
/// across devices via Space Mesh. Large content (>10 MB) may be pruned
/// under storage pressure, but metadata is always retained.
pub trait FlowHistory {
    /// Search past flow entries by text content or metadata.
    fn search(
        &self,
        query: &str,
        limit: usize,
        cap: &CapabilityHandle,
    ) -> Result<Vec<FlowEntry>, FlowError>;

    /// Retrieve a specific flow entry by ID.
    fn get(&self, id: FlowEntryId, cap: &CapabilityHandle) -> Result<FlowEntry, FlowError>;

    /// Replay a past entry -- re-publish it to a channel.
    fn replay(
        &self,
        id: FlowEntryId,
        channel: &dyn FlowChannel,
        cap: &CapabilityHandle,
    ) -> Result<FlowEntryId, FlowError>;

    /// List recent entries (newest first).
    fn recent(
        &self,
        limit: usize,
        cap: &CapabilityHandle,
    ) -> Result<Vec<FlowEntry>, FlowError>;
}

/// Content conversion pipeline.
///
/// The transform pipeline finds the cheapest path from source type to
/// target type through a directed conversion graph. System transforms
/// (e.g., HTML-to-plain-text) are always available. AIRS transforms
/// (e.g., speech-to-text, summarization) require the AIRS Kit.
/// Agents can register custom transforms to extend the graph.
pub trait TransformPipeline {
    /// Check if a transform path exists from source to target type.
    fn can_transform(&self, from: &str, to: &str) -> bool;

    /// Execute a transform. Returns the converted content.
    fn transform(
        &self,
        content: &TypedContent,
        target_type: &str,
    ) -> Result<TypedContent, FlowError>;

    /// Register a custom transform provided by an agent.
    fn register_transform(
        &self,
        from: &str,
        to: &str,
        handler: Box<dyn TransformHandler>,
        cap: &CapabilityHandle,
    ) -> Result<TransformId, FlowError>;

    /// List all available transform paths.
    fn available_transforms(&self) -> Vec<TransformPath>;
}
```

## 3. Usage Patterns

### Copy text to the system clipboard

```rust
use aios_flow::{FlowChannel, TypedContent, ContentPayload, TransferIntent};

fn copy_to_clipboard(
    clipboard: &dyn FlowChannel,
    cap: &CapabilityHandle,
    text: &str,
) -> Result<FlowEntryId, FlowError> {
    let content = TypedContent {
        content_type: "text/plain".into(),
        payload: ContentPayload::Inline(text.as_bytes().to_vec()),
        alternatives: vec![],
    };

    clipboard.publish(content, TransferIntent::Copy, cap)
}
```

### Paste with format negotiation

```rust
use aios_flow::FlowChannel;

fn paste_as_markdown(
    clipboard: &dyn FlowChannel,
    cap: &CapabilityHandle,
) -> Result<Option<String>, FlowError> {
    // Request Markdown. If the clipboard has HTML, the transform pipeline
    // automatically converts it. If it has plain text, it wraps in a
    // code block. The agent does not need to know the source format.
    let entry = clipboard.receive(Some("text/markdown"), cap)?;

    Ok(entry.map(|e| {
        String::from_utf8_lossy(e.content.payload.as_bytes()).to_string()
    }))
}
```

### Browse and replay clipboard history

```rust
use aios_flow::{FlowHistory, FlowChannel};

fn replay_recent_copy(
    history: &dyn FlowHistory,
    clipboard: &dyn FlowChannel,
    cap: &CapabilityHandle,
    index: usize,
) -> Result<(), FlowError> {
    let recent = history.recent(10, cap)?;

    if let Some(entry) = recent.get(index) {
        history.replay(entry.id, clipboard, cap)?;
    }

    Ok(())
}
```

## 4. Integration Examples

### Flow Kit + Interface Kit: drag-and-drop between agents

```rust
use aios_flow::{FlowChannel, TypedContent, ContentPayload, TransferIntent};

fn handle_drop(
    flow: &dyn FlowChannel,
    cap: &CapabilityHandle,
    drag_data: &DragPayload,
) -> Result<(), FlowError> {
    // The compositor routes drop events through Flow.
    // The source agent published content during drag-start;
    // the receiving agent receives it here with type negotiation.
    let entry = flow.receive(Some("application/json"), cap)?;

    if let Some(entry) = entry {
        // Process the dropped content in the receiver's preferred format.
        let json = String::from_utf8_lossy(entry.content.payload.as_bytes());
        // ... process json ...
    }

    Ok(())
}
```

### Flow Kit + Storage Kit: save clipboard to a Space

```rust
use aios_flow::FlowHistory;
use aios_storage::Space;

fn save_clipboard_entry_to_space(
    history: &dyn FlowHistory,
    space: &dyn aios_storage::Space,
    cap: &CapabilityHandle,
    entry_id: FlowEntryId,
) -> Result<ObjectId, Box<dyn core::error::Error>> {
    let entry = history.get(entry_id, cap)?;

    let object_id = space.create_object(
        &format!("clipboard-{}", entry.initiated_at),
        &entry.content.payload.as_bytes(),
        &entry.content.content_type,
        cap,
    )?;

    Ok(object_id)
}
```

### Flow Kit + AIRS Kit: AI-powered transform

```rust
use aios_flow::{TransformPipeline, TypedContent, ContentPayload};

fn summarize_clipboard_content(
    transform: &dyn TransformPipeline,
    clipboard: &dyn aios_flow::FlowChannel,
    cap: &CapabilityHandle,
) -> Result<Option<String>, FlowError> {
    let entry = clipboard.receive(None, cap)?;

    if let Some(entry) = entry {
        // The "text/summary" target type triggers an AIRS-powered transform
        // that summarizes the input content. This only works when AIRS is loaded.
        if transform.can_transform(&entry.content.content_type, "text/summary") {
            let summary = transform.transform(&entry.content, "text/summary")?;
            return Ok(Some(
                String::from_utf8_lossy(summary.payload.as_bytes()).to_string(),
            ));
        }
    }

    Ok(None)
}
```

## 5. Capability Requirements

| Capability | What It Gates | Default Grant |
| --- | --- | --- |
| `FlowPublish` | Publishing content to any flow channel | Granted to all agents |
| `FlowReceive` | Receiving content from flow channels | Granted to all agents |
| `FlowSubscribe` | Creating channel subscriptions | Granted to all agents |
| `FlowHistoryRead` | Browsing and searching flow history | Granted to all agents |
| `FlowHistoryReplay` | Replaying past entries to channels | Granted to all agents |
| `FlowTransformRegister` | Registering custom transform handlers | Agents with `TransformProvider` manifest |
| `FlowAdmin` | Managing channels, purging history | System agents only |

## 6. Error Handling

```rust
/// Errors returned by the Flow Kit.
pub enum FlowError {
    /// The agent lacks the required flow capability.
    /// Recovery: request the capability or declare it in the agent manifest.
    CapabilityDenied(String),

    /// The requested flow channel does not exist.
    /// Recovery: verify the channel name; the system clipboard is always "clipboard".
    ChannelNotFound(String),

    /// Content exceeds the maximum inline size (64 KB) and shared memory
    /// allocation failed. Recovery: reduce content size or free shared memory.
    ContentTooLarge {
        size: usize,
        max_inline: usize,
    },

    /// No transform path exists from the source type to the requested target.
    /// Recovery: check `TransformPipeline::can_transform()` before transforming,
    /// or accept a different target type.
    NoTransformPath {
        from: String,
        to: String,
    },

    /// An AIRS-powered transform was requested but AIRS is not available.
    /// Recovery: fall back to a non-AIRS transform or accept the source type.
    AirsUnavailable,

    /// The flow entry has been pruned (content removed by retention policy).
    /// Metadata is still available. Recovery: accept that the content is gone.
    ContentPruned(FlowEntryId),

    /// The agent has exceeded the per-agent rate limit for flow operations.
    /// Recovery: wait and retry. Default limit: 100 publishes per minute.
    RateLimitExceeded {
        limit: u32,
        retry_after: core::time::Duration,
    },

    /// A custom transform handler returned an error.
    /// Recovery: check the transform handler's logs; retry with a different path.
    TransformFailed(String),

    /// Storage error during history persistence.
    StorageError(String),
}
```

## 7. Platform & AI Availability

The Flow Kit separates its core data exchange functionality from AI-powered features:

**Always available (no AIRS dependency):**

- Publishing and receiving content on all channels.
- System clipboard (copy/paste/cut).
- Type negotiation between agents.
- System transforms: HTML-to-text, text-to-HTML, image format conversion, code syntax
  highlighting, PDF text extraction, Markdown-to-HTML, JSON-to-table.
- Flow history persistence, search, and replay.
- Multi-device sync via Space Mesh.
- Custom transform registration and execution.

**Available when AIRS is loaded:**

- AI-powered transforms: speech-to-text, text summarization, language translation,
  embedding generation.
- Content screening for sensitive data (PII detection before transfer).
- Smart paste: AIRS infers the best target format based on the receiver's context.
- Provenance explanation: AIRS generates human-readable descriptions of transform chains.

**Feature detection:**

```rust
use aios_flow::TransformPipeline;

fn has_ai_transforms(pipeline: &dyn TransformPipeline) -> bool {
    pipeline.can_transform("audio/wav", "text/plain")  // speech-to-text
}

fn has_summarization(pipeline: &dyn TransformPipeline) -> bool {
    pipeline.can_transform("text/plain", "text/summary")
}
```

**Content payload sizing:**

| Payload Size | Storage | Transfer Method |
| --- | --- | --- |
| < 64 KB | Inline in FlowEntry | Direct IPC message |
| 64 KB - 16 MB | Shared memory region | Zero-copy via shared memory handle |
| > 16 MB | Space object reference | Object reference with lazy loading |
