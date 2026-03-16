# AIOS Flow Extensions

Part of: [flow.md](../flow.md) — Flow System
**Related:** [data-model.md](./data-model.md) — Core data model, [transforms.md](./transforms.md) — Transform engine, [history.md](./history.md) — History & sync, [security.md](./security.md) — Security model, [integration.md](./integration.md) — Compositor & subsystem integration, [sdk.md](./sdk.md) — SDK APIs

-----

## 15. Near-Term Extensions

These are concrete extensions that fit within the current architecture and should be considered for Phase 15 or early follow-on work.

### 15.1 Flow Batching

Allow pushing multiple items in a single transfer. Drag-selecting 5 files, copying a table with multiple cells, or multi-selecting images should produce a single `BatchTransfer` with one `TransferId`, not 5 separate transfers.

```rust
pub struct BatchContent {
    /// Ordered list of items in this batch
    items: Vec<TypedContent>,

    /// How the batch should be presented to the receiver
    presentation: BatchPresentation,
}

pub enum BatchPresentation {
    /// Items are independent (multi-select, e.g., 5 files)
    Collection,
    /// Items form a sequence (e.g., ordered steps, slides)
    Sequence,
    /// Items are alternatives (e.g., same image in multiple resolutions)
    Alternatives,
}
```

The receiver can accept the entire batch or pull individual items. The Flow Tray shows batches as a single expandable entry. History records one FlowEntry with `content: Some(TypedContent)` where the primary payload wraps the batch.

### 15.2 Paste-as Shortcuts

Users should be able to control the transform applied on paste, not just accept the automatic negotiation result.

| Shortcut | Action |
|---|---|
| Ctrl+V | Default paste (automatic type negotiation) |
| Ctrl+Shift+V | Open Flow History |
| Ctrl+Alt+V | Paste as plain text (force strip-formatting transform) |
| Ctrl+Alt+Shift+V | Paste-as menu (choose: plain text, markdown, reference, new note) |

"Paste as reference" creates an ObjectRef to the source object instead of copying content. "Paste into new note" creates a new space object from the clipboard content and inserts a reference. These are user-initiated transform hints that override the default negotiation.

### 15.3 Encryption at Rest

Flow history content should be encrypted at rest using the user's identity key. Currently §9.1 covers encryption in transit for multi-device sync and §11 covers capability enforcement. Content sitting in `system/flow/history/` benefits from device-level transparent encryption (spaces.md §4.10), but has no per-space encryption beyond that.

```rust
pub struct FlowEncryptionPolicy {
    /// Encrypt all Flow history content at rest
    encrypt_history: bool,               // default: true

    /// Key used for history encryption (derived from user's IdentityId keypair)
    history_key: Option<KeyId>,

    /// Encrypt ephemeral transfer content in shared memory
    /// (defense-in-depth — shared memory is already capability-gated)
    encrypt_staged: bool,                // default: false
}
```

Content-addressed deduplication still works: the content hash is computed before encryption, and the encrypted block is stored under the same hash. Decryption happens on read. This adds negligible overhead for the common case (text snippets, small images) and protects against offline storage attacks.

### 15.4 Undo Last Paste

Flow knows exactly what was just delivered, where it came from, and what the intent was. This enables true undo-paste:

- **Copy undo:** Remove the pasted content from the receiver. The receiver agent gets a `FlowUndoRequest` and can choose to honor it (remove the pasted text) or reject it (user has already modified the pasted content).
- **Move undo:** Restore the source object from Archived state and remove the content from the receiver. This is a two-phase operation coordinated by the Flow Service.
- **Quote undo:** Remove the pasted content and the DerivedFrom relation.

The undo window is configurable (default: 30 seconds after paste). After the window, the transfer is final. Undo is triggered via Ctrl+Z when the Flow Tray has focus, or via the Flow Tray UI (each recent entry shows an [Undo] button within the undo window).

```rust
pub enum FlowEvent {
    // ... existing variants ...

    /// Undo requested for a recently delivered transfer.
    /// The receiver agent should reverse the paste if possible.
    UndoRequested { entry_id: FlowEntryId },
}
```

### 15.5 Per-Agent Rate Visibility

Extend the Inspector and Flow Tray to show per-agent Flow usage:

- Which agents have FlowRead/FlowWrite capabilities
- Transfer count per agent (last hour, last day)
- Total bytes transferred per agent
- Rate limit hits per agent

This gives users visibility into which agents are active data consumers/producers and whether any agent is behaving unexpectedly. Accessible via the Inspector's Flow tab and summarized in the Flow Tray footer.

### 15.6 Content Sanitization Pipeline

Content pushed into the Flow Service is validated and sanitized before being staged for delivery. System-recognized semantic types (`RichText`, `Image`, `Code`, `Link`, `StructuredData`) receive type-specific sanitization on `push()`. Custom and agent-defined types pass through unmodified but are marked with `sanitized: false` in their `ContentMetadata`, leaving the receiver responsible for safe handling.

This sanitization layer operates **before** the AIRS-based sensitive-data screening described in §11.2. The two layers are complementary: sanitization removes structurally dangerous content (script injection, polyglot files, oversized JSON trees); AIRS screening identifies semantically sensitive content (credentials, PII). Neither replaces the other.

```rust
/// Content sanitization policy, applied by the Flow Service on every push().
/// Operates before AIRS content screening (§11.2) as a defense-in-depth layer.
pub struct SanitizationPolicy {
    /// Per-type sanitizers (system types get automatic sanitization)
    sanitizers: HashMap<SemanticType, Box<dyn ContentSanitizer>>,

    /// Whether to reject unsanitizable content or pass it through marked untrusted
    reject_unsanitizable: bool,  // default: false (pass through, mark untrusted)
}

/// Trait implemented by content sanitizers for each system semantic type.
pub trait ContentSanitizer: Send + Sync {
    /// Sanitize the content in-place. Returns true if content was modified.
    fn sanitize(&self, content: &mut ContentPayload) -> Result<bool, FlowError>;

    /// Human-readable description of what this sanitizer does.
    fn description(&self) -> &str;
}
```

Built-in sanitizers for system semantic types:

| SemanticType | Sanitizer | What it does |
|---|---|---|
| RichText | HtmlSanitizer | Strip `<script>`, `<iframe>`, `on*` event handlers, `javascript:` URIs |
| Image | ImageSanitizer | Validate image headers, strip EXIF GPS data, detect polyglot files |
| Code | CodeSanitizer | Strip ANSI escape sequences, validate UTF-8 encoding |
| Link | LinkSanitizer | Validate URL scheme (allow http/https/aios, block javascript/data) |
| StructuredData | JsonSanitizer | Validate JSON structure, enforce depth limit (default: 64 levels) |
| Custom / File | (none) | Passed through with `sanitized: false` — receiver is responsible |

When `reject_unsanitizable` is `false` (the default), content that cannot be fully sanitized is passed through with a `sanitized: false` flag in its `ContentMetadata`. Receivers that require clean content can inspect this flag and refuse delivery. When `reject_unsanitizable` is `true`, the `push()` call returns `FlowError::SanitizationFailed` and the content is never staged.

Note: Inspired by Fuchsia RFC-0179 (clipboard content is untrusted by default) and Chrome's Async Clipboard API (sanitized vs unsanitized custom formats).

### 15.7 Focus-Gated Clipboard Access

When an agent calls `pull()` or `subscribe()` with `target: Any` (the global clipboard), the Flow Service queries the compositor to verify the agent's surface currently holds focus. Background agents without focus receive `FlowError::PermissionDenied`. This prevents background agents from silently reading the global clipboard even if they hold the `FlowRead` capability.

The focus check applies only to **global clipboard reads** — it does not affect targeted transfers. The full exception table:

- **System agents** (compositor, POSIX bridge, audit service): exempt from the focus check. These agents operate at a privilege level above normal agents and require clipboard access regardless of surface focus.
- **`FlowBackgroundRead` capability**: agents holding this capability bypass the focus check. This capability is granted sparingly, requires explicit user approval, and is visible in the Inspector's capability panel.
- **Targeted transfers** (`FlowTarget::Agent(id)`): unaffected. Agents can always receive content explicitly addressed to them by a sender — focus is irrelevant for point-to-point delivery.
- **`push()` calls**: unaffected. Writing to the clipboard does not require focus; only reading the global clipboard does.

Note: Inspired by Fuchsia RFC-0179, which gates clipboard access on view focus to prevent background snooping.

### 15.8 Action-as-Content Semantic Type

Flow supports a `SemanticType::Action` variant where pasting triggers behavior in the receiver rather than delivering data. This is analogous to Android Intents placed on the clipboard: the copied item is not content to be displayed, but an instruction for the receiver to execute.

```rust
/// New variant in the SemanticType enum (see data-model.md §3.4).
/// Action { action_id: String, params: HashMap<String, String> }

/// Registration for an action handler.
pub struct ActionRegistration {
    /// The action identifier (e.g., "aios.action.create_event", "aios.action.add_contact")
    action_id: String,

    /// Human-readable description shown in the paste-as menu
    description: String,

    /// The agent that handles this action
    handler_agent: AgentId,

    /// Fallback content if no handler is registered (degrades gracefully)
    fallback_content: Option<TypedContent>,
}
```

Actions are capability-controlled and user-confirmed:

- Actions require the `FlowActionHandle` capability on the handler agent. Without this capability, the Flow Service refuses to route the action.
- The first invocation of an action by an untrusted agent requires user confirmation via a system dialog. Subsequent invocations from the same agent to the same action handler are permitted without re-confirmation, subject to the rate limits in §11.3.
- If no handler is registered for an `action_id`, the `fallback_content` is delivered instead. This ensures graceful degradation: pasting an `aios.action.create_event` action on a system without a calendar agent delivers a plain-text representation of the event parameters rather than silently failing.
- Agents register actions via `register_action()` on the `FlowClient` trait (see sdk.md §12.1). Registration is persisted across agent restarts and survives reboots.

Example use cases: copying a contact card that, when pasted into the address book agent, creates a new contact entry; copying a calendar event that, when pasted into the calendar agent, opens the event creation dialog pre-filled with the event parameters.

Note: Inspired by Android's `ClipboardManager` Intent support.

-----

## 16. Future Directions

These are more ambitious extensions that would build on top of the Phase 15-37 implementation. They are not committed to any phase but are worth designing toward.

### 16.1 Standing Flow Rules (Pipelines)

Let users define persistent rules that automatically process content as it flows through the system:

```text
Rule: "Browser → Research"
  Trigger: Any transfer from browser-tab agents with intent Copy
  Filter: content_type matches "text/*"
  Transform: Summarize (AIRS) + add source URL attribution
  Deliver to: research-agent
  Auto-accept: true
```

This is analogous to email filters but for data flow. Rules are stored in `system/flow/config/rules/` and evaluated by the Flow Service on every push. Rules can chain: a transfer can match multiple rules and be delivered to multiple destinations (fan-out).

The Conversation Bar provides a natural interface for creating rules: "Whenever I copy text from the browser, summarize it and save it to my research space." AIRS parses this into a FlowRule.

```rust
pub struct FlowRule {
    /// Unique identifier
    id: FlowRuleId,
    /// Human-readable name
    name: String,
    /// When this rule triggers
    trigger: FlowRuleTrigger,
    /// Transforms to apply (in order)
    transforms: Vec<TransformId>,
    /// Where to deliver the result
    destination: FlowTarget,
    /// Whether the destination should auto-accept (skip pull())
    auto_accept: bool,
    /// Whether this rule is currently active
    enabled: bool,
}

pub struct FlowRuleTrigger {
    /// Source agent filter (specific agent, agent category, or any)
    source: Option<AgentId>,
    /// Content type filter
    content_type: Option<String>,
    /// Intent filter
    intent: Option<TransferIntent>,
    /// Semantic type filter
    semantic_type: Option<SemanticType>,
}

pub struct FlowRuleId(u128);
```

### 16.2 Context-Aware Smart Paste

When pasting into a specific context, Flow could apply context-aware transforms beyond just type matching. The receiver agent advertises not just what types it accepts, but *how* it wants content formatted for the current cursor position:

| Source | Destination context | Smart transform |
|---|---|---|
| URL | Markdown editor | Format as `[page title](url)` |
| Color hex string | Design tool | Convert to tool's native color format |
| Date string | Calendar agent | Parse and create event draft |
| Code snippet | Chat/email composer | Wrap in code fence with language tag |
| Image | Terminal | Convert to ASCII art (AIRS) or show "image at path" |
| Table (HTML) | Spreadsheet agent | Parse into cells |

Smart paste requires the receiver to provide a `PasteContext` alongside its accepted types:

```rust
pub struct PasteContext {
    /// What type of editing surface the cursor is in
    surface_type: PasteSurfaceType,
    /// Language/format of the surrounding content (if applicable)
    content_language: Option<String>,
    /// Additional context hints from the receiver
    hints: HashMap<String, String>,
}

pub enum PasteSurfaceType {
    PlainTextEditor,
    RichTextEditor,
    CodeEditor { language: String },
    Terminal,
    Spreadsheet,
    Canvas,
    FormField,
}
```

### 16.3 Flow Analytics and Insights

AIRS could analyze Flow patterns over time and surface actionable insights:

- **Workflow detection:** "You copy between the browser and editor 40 times a day. Consider enabling the direct browser-to-editor Flow Rule."
- **Redundancy detection:** "You keep re-copying the same 5 API keys. Consider using the credential manager agent."
- **Privacy warnings:** "Agent X has read 200 clipboard entries in the last hour. This is unusual — review its permissions?"
- **Content suggestions:** "You copied a paper abstract yesterday and a related diagram today. Link them?"

Analytics are opt-in, computed locally by AIRS (never sent off-device), and presented in the Flow Tray as a collapsible "Insights" section. Users can dismiss individual insights or disable the feature entirely.

### 16.4 Flow Reactions and Annotations

Let users annotate Flow history entries beyond the automatic metadata:

```rust
pub struct FlowAnnotation {
    /// The entry being annotated
    entry_id: FlowEntryId,
    /// Who created this annotation
    author: AgentId,
    /// Annotation type
    kind: AnnotationKind,
    /// When the annotation was created
    created_at: Timestamp,
}

pub enum AnnotationKind {
    /// Pin this entry (never prune)
    Pin,
    /// Star / favorite
    Star,
    /// Tag with a user-defined label
    Tag(String),
    /// Free-text note
    Note(String),
}
```

Annotations are searchable via AIRS: "find the Flow entries I starred last week" or "show me everything tagged 'project-alpha'." Annotations persist even when content is pruned — the metadata and annotation survive.

### 16.5 Agent-to-Agent Flow Contracts

Agents could declare persistent flow relationships — standing agreements to send and receive specific content types:

```rust
pub struct FlowContract {
    /// The agent offering to send content
    provider: AgentId,
    /// The agent agreeing to receive content
    consumer: AgentId,
    /// Content types covered by this contract
    content_types: Vec<TypeMatcher>,
    /// Whether transfers under this contract skip the Flow Tray
    /// (silent delivery for high-frequency automated pipelines)
    silent: bool,
    /// Maximum transfer rate (transfers per minute)
    max_rate: u32,
    /// Whether the user has approved this contract
    user_approved: bool,
}
```

Contracts enable autonomous data pipelines between agents without user interaction on each transfer. Example: a download agent has a contract with a document indexer — every downloaded PDF is automatically sent for indexing. Contracts require explicit user approval and are visible in the Inspector.

### 16.6 Selective Multi-Device Content Sync

Let users configure which types of content sync across devices, replacing the current all-or-nothing model (with the 10 MB size cutoff):

```rust
pub struct DeviceSyncPolicy {
    /// Content types to sync (empty = sync everything)
    sync_types: Vec<TypeMatcher>,

    /// Content types to never sync (takes precedence over sync_types)
    exclude_types: Vec<TypeMatcher>,

    /// Maximum single-entry size for automatic sync
    max_auto_sync_size: u64,           // default: 10 MB

    /// Whether to sync history or only active transfers
    sync_history: bool,                // default: true

    /// Sync only when on WiFi (not cellular, if applicable)
    wifi_only: bool,                   // default: false
}
```

Example configuration: "Sync text and links everywhere. Sync images only on WiFi. Never sync video. Sync history for the last 24 hours only." This gives users fine-grained control over bandwidth and privacy trade-offs across their devices.

### 16.7 Lazy Alternative Materialization

Replace the pre-materialized `alternatives: Vec<ContentPayload>` model with deferred `AlternativeOffer` futures. The source agent declares what types it *can* produce, but generates the content only when a receiver requests a specific type. This avoids wasting memory materializing alternatives that nobody uses.

```rust
/// Replaces ContentPayload in the alternatives vector (see data-model.md §3.4).
pub struct AlternativeOffer {
    /// MIME type this alternative would produce
    mime_type: String,
    /// Semantic type
    semantic_type: SemanticType,
    /// Estimated size (for UI display and back-pressure decisions)
    estimated_size: u64,
    /// Materialization state
    state: AlternativeState,
}

pub enum AlternativeState {
    /// Not yet materialized — source agent will generate on demand
    Deferred,
    /// Materialization in progress
    Materializing,
    /// Materialized and available
    Ready(ContentPayload),
    /// Materialization failed
    Failed(String),
}
```

When a receiver calls `pull()` and requests a specific MIME type, the Flow Service checks whether the corresponding alternative is `Deferred`. If so, it sends a `MaterializeRequest` to the source agent, waits for the `AlternativeState` to transition to `Ready`, then delivers the content. The source agent must remain alive (or have cached the result) until the transfer completes — the Flow Service tracks outstanding materialization requests and notifies the source if the receiver disconnects before delivery.

Lazy alternatives are preferred over Transform Engine conversions when both could produce the same output type. The source agent knows its own content best: a vector graphics editor can produce a high-fidelity SVG alternative on demand more accurately than the Transform Engine converting from a PNG lossy intermediate.

Inspired by Wayland's deferred MIME type offer model, Android's `ContentProvider` URI resolution, and ZeroIPC's shared-memory futures.

### 16.8 Segment-Based Progressive Delivery

Large transfers above a configurable threshold (default: 10 MB) are split into consumable segments. The receiver can start processing early segments while later segments are still being staged, reducing time-to-first-byte for large content transfers.

```rust
pub struct ProgressiveTransfer {
    /// Base transfer metadata
    transfer: Transfer,
    /// Segment size (default: 1 MB)
    segment_size: u64,
    /// Total expected segments (may be unknown for streaming sources)
    total_segments: Option<u64>,
    /// Segments delivered so far
    delivered_segments: u64,
    /// Whether the receiver is actively consuming (triggers priority promotion)
    receiver_active: bool,
    /// Delivery mode: eager (push as available) or on-demand (pull next)
    delivery: SegmentDelivery,
}

pub enum SegmentDelivery {
    /// Deliver segments as they become available (default)
    Eager,
    /// Deliver only when receiver requests next segment
    OnDemand,
}
```

The `receiver_active` flag is set when the receiver calls `pull_segment()`. When `true`, the kernel's async copy offload (DMA engine or background copy thread) is promoted to a higher scheduling priority for this transfer — the user is actively waiting and latency matters. When `false` (background prefetch or pipeline staging), the copy runs at normal priority.

`Eager` delivery is the default for interactive transfers where the receiver wants to display content as it arrives (e.g., a large image being progressively decoded). `OnDemand` is appropriate for pipeline stages where each segment must be fully processed before the next is needed (e.g., a multi-pass document transform).

Inspired by Copier (SOSP 2025, Best Paper) — treats memory copy as a first-class async OS service with segment-based progress tracking and task promotion.

### 16.9 OR-Set CRDT for History Sync

Upgrade the multi-device history sync from a grow-only set (§9.2 in history.md) to an OR-Set CRDT. This enables propagated deletions: when a user deletes a history entry on Device A, the deletion propagates to Device B rather than reappearing on next sync.

Key design:

- Each `FlowEntry` addition carries a unique tag (`FlowEntryId` + `DeviceId`) that identifies the specific insertion event.
- Deletion records a tombstone: `(FlowEntryId, DeviceId, deleted_at)`.
- Merge rule: the union of all entries, minus entries matching any tombstone. An entry is visible only if at least one of its addition tags has not been tombstoned.
- Tombstone compaction: tombstones are pruned after ALL devices have acknowledged them (watermark-based). The Flow Service tracks per-device sync watermarks and removes tombstones once every device's watermark exceeds the `deleted_at` timestamp.
- Flow configuration (retention policy, encryption policy, sync policy) syncs via LWW-Register CRDT — last-writer-wins with device timestamps. Configuration is low-frequency and does not benefit from OR-Set semantics.

Trade-off: tombstones consume storage until compacted. With typical retention policies (30 days, 1000 entries), tombstone overhead is negligible even on a three-device setup.

### 16.10 Scatter-Gather Delivery

Hybrid delivery model for `TypedContent`: small alternatives below 4 KB are inlined directly in the IPC message payload, while large alternatives use zero-copy shared memory mapping. This avoids the overhead of shared memory region creation, page table manipulation, and TLB invalidation for tiny payloads.

```rust
pub enum AlternativeDelivery {
    /// Small payload inlined in the IPC message (avoids shared memory setup overhead)
    Inline(Vec<u8>),
    /// Large payload delivered via zero-copy shared memory mapping
    ZeroCopy(SharedMemoryId),
}
```

The 4 KB threshold aligns with the system page size: anything smaller than a page gains nothing from shared memory mapping — the page table entry, TLB slot, and shared memory region bookkeeping cost more than just copying the bytes inline. A plain-text alternative of a rich content transfer is typically under 1 KB and should always be inlined. A full-resolution image alternative should always use zero-copy.

The Flow Service selects the delivery mode automatically based on the materialized size of each alternative. Receivers do not need to handle the distinction explicitly — the `AlternativeDelivery` enum is transparent to the `FlowClient` trait (see sdk.md §12.1).

Inspired by Cornflakes (SOSP 2023) — hybrid zero-copy serialization where fields above 512 bytes are zero-copied and smaller fields are inlined to avoid per-field shared memory overhead.

### 16.11 Per-Subscriber Queues

Replace the single-stream `FlowSubscription` model with per-subscriber independent queues. Each `subscribe()` call gets its own bounded queue with configurable depth and overflow policy. Slow subscribers do not block fast ones, and a misbehaving subscriber cannot apply back-pressure to the global Flow Service.

```rust
pub struct SubscriptionConfig {
    /// Maximum queue depth (default: 64 entries)
    queue_depth: u32,
    /// What happens when the queue is full
    overflow_policy: OverflowPolicy,
}

pub enum OverflowPolicy {
    /// Drop the oldest entry to make room (default)
    DropOldest,
    /// Drop the incoming entry (newest loses)
    DropNewest,
    /// Block the producer until this subscriber consumes (use with caution)
    BlockProducer,
}
```

The default `DropOldest` policy ensures the subscriber always sees the most recent content, which matches clipboard semantics — the last copied item is what matters, and a stale queue entry from five minutes ago has little value. `DropNewest` is appropriate for subscribers that process entries in strict order and must not skip entries (e.g., an audit log subscriber). `BlockProducer` should only be used for lossless pipelines where every entry matters and the producer is known to be well-behaved; it risks deadlock if the subscriber stalls.

Inspired by Eclipse iceoryx2 — Rust-native zero-copy IPC with per-subscriber shared-memory queues and configurable overflow policies.
