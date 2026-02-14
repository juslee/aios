# AIOS Flow System

## Deep Technical Architecture

**Parent document:** [aios-architecture.md](../project/aios-architecture.md)
**Related:** [aios-compositor.md](../platform/aios-compositor.md) — Drag/drop integration, [aios-subsystem-framework.md](../platform/aios-subsystem-framework.md) — DataChannel/Flow pipes, [aios-agents.md](../applications/aios-agents.md) — SDK FlowClient, [aios-spaces.md](./aios-spaces.md) — History storage, [aios-experience.md](../experience/aios-experience.md) — Flow Tray UI

-----

## 1. Overview

The clipboard is the worst abstraction in modern computing. It is a single global buffer with no type information, no history, no provenance, no transformations, no multi-device awareness, and no context. You copy something, you paste something. If you copy again, the first thing is gone. You cannot see what is on the clipboard. You cannot search it. You cannot trace where data came from or where it went. Every application implements its own internal clipboard for rich content because the OS clipboard is useless for anything beyond plain text.

Flow replaces the clipboard entirely. Every copy, paste, drag, drop, and share action in AIOS goes through Flow. Flow is a system service that provides:

- **Typed content.** Flow knows it is carrying a PDF, a code snippet, an image, a URL — not just bytes. Content carries its MIME type, its AIOS semantic type, and alternative representations.
- **History.** Every transfer is recorded. You can search your Flow history by content, by agent, by time, by type. "Find that thing I copied last week about transformer architectures" is a real query.
- **Provenance.** Every transfer records where the data came from, who sent it, what transformations were applied, and where it went. The full chain is inspectable.
- **Transformations.** When the receiver cannot handle the source format, Flow transforms the content automatically. Rich text becomes plain text for a terminal. An image becomes a thumbnail for a preview. Audio becomes a transcript via AIRS.
- **Intent.** A transfer is not just "copy." It can be a copy, a move, a reference, a quote (with attribution), or a derivation (new object linked to source). Each intent has different semantics.
- **Multi-device.** Copy on your laptop, paste on your tablet. Flow syncs between AIOS devices sharing an identity.
- **Context awareness.** Flow knows what agent is sending, what agent is receiving, and what the user is doing. It adapts behavior accordingly.

No other operating system has this. macOS has Universal Clipboard (multi-device copy/paste) but no types, no history, no transforms, no provenance. Windows has clipboard history but no types, no transforms, no provenance. Linux has three separate clipboard buffers (PRIMARY, SECONDARY, CLIPBOARD) and none of them do anything useful beyond raw byte transfer.

Flow is the connective tissue of AIOS. It is how data moves between agents, between subsystems, between devices. It is how the user's work stays connected.

-----

## 2. Architecture

```
┌──────────────────────────────────────────────────────────┐
│                      Flow Service                          │
│              (system service, always running)               │
│                                                            │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────┐  │
│  │   Transfer    │  │   History    │  │   Transform    │  │
│  │   Manager     │  │   Store      │  │   Engine       │  │
│  │              │  │              │  │                │  │
│  │  initiate    │  │  index       │  │  negotiate     │  │
│  │  stage       │  │  search      │  │  select        │  │
│  │  deliver     │  │  retain      │  │  execute       │  │
│  │  cancel      │  │  prune       │  │  register      │  │
│  └──────────────┘  └──────────────┘  └────────────────┘  │
│                                                            │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────┐  │
│  │  Provenance   │  │   Type       │  │  Multi-Device  │  │
│  │  Tracker      │  │   System     │  │  Sync          │  │
│  │              │  │              │  │                │  │
│  │  chain       │  │  MIME        │  │  replicate     │  │
│  │  verify      │  │  semantic    │  │  merge         │  │
│  │  inspect     │  │  negotiate   │  │  resolve       │  │
│  └──────────────┘  └──────────────┘  └────────────────┘  │
│                                                            │
└───────────────────────┬──────────────────────────────────┘
                        │ IPC (sys.flow channel)
          ┌─────────────┼─────────────────┐
          ▼             ▼                 ▼
       Agents       Compositor       Subsystems
       (SDK          (drag/drop,      (DataChannels,
       FlowClient)   visual cues)     FlowPipes)
```

The Flow Service runs as a system service registered at `sys.flow`. It starts during Phase 3 of boot (service initialization) alongside Space Storage, AIRS, and the Task Manager. Agents connect via IPC channels with `FlowRead` and/or `FlowWrite` capabilities.

The six internal components:

| Component | Responsibility |
|---|---|
| Transfer Manager | Active transfer lifecycle: initiation, staging, delivery, cancellation |
| History Store | Persistent record of all completed transfers, stored in `system/flow/` space |
| Transform Engine | Content type negotiation and conversion between source and target formats |
| Provenance Tracker | Append-only chain linking each transfer to its source, transformations, and destination |
| Type System | MIME type registry, semantic type registry, compatibility matrix |
| Multi-Device Sync | Replication of active transfers and history across devices sharing an identity |

-----

## 3. Core Data Model

### 3.1 FlowEntry

Every completed transfer becomes a `FlowEntry` stored in the history:

```rust
pub struct FlowEntry {
    /// Unique identifier for this entry
    id: FlowEntryId,

    /// The agent that initiated the transfer
    source_agent: AgentId,

    /// The agent that received the content (None if still in clipboard/unclaimed)
    destination_agent: Option<AgentId>,

    /// The content that was transferred
    content: TypedContent,

    /// What the sender intended (Copy, Move, Reference, Quote, Derive)
    intent: TransferIntent,

    /// Transformations applied during delivery
    transformations: Vec<TransformRecord>,

    /// When the transfer was initiated
    initiated_at: Timestamp,

    /// When the transfer was delivered (None if cancelled or expired)
    delivered_at: Option<Timestamp>,

    /// Link to the provenance chain
    provenance: ProvenanceLink,

    /// Object references — source object, destination object (if created)
    source_object: Option<ObjectRef>,
    destination_object: Option<ObjectRef>,

    /// Device that originated this entry (for multi-device history merge)
    origin_device: DeviceId,

    /// Whether this entry's content has been pruned (large content retention policy)
    content_pruned: bool,
}

pub struct FlowEntryId(u128);

pub struct ProvenanceLink {
    /// Hash of the provenance record in the provenance chain
    hash: Hash,
    /// Previous link in the chain (if this content was derived from another transfer)
    parent: Option<Hash>,
}
```

FlowEntry is stored as a space object in `system/flow/history/`. The object's semantic metadata includes the entry's content type, source agent name, and a text summary for full-text search. The entry's content is stored as a content-addressed block — if the same content is transferred ten times, it is stored once.

### 3.2 Transfer Lifecycle

A transfer moves through a defined set of states:

```
┌──────────┐     ┌──────────┐     ┌──────────────┐     ┌───────────┐
│ Initiated │────→│  Staged  │────→│  Negotiating  │────→│ Delivered │
└──────────┘     └──────────┘     └──────────────┘     └───────────┘
      │                │                  │                    │
      │                │                  │                    ▼
      │                │                  │            ┌──────────────┐
      │                │                  │            │  Recorded    │
      │                │                  │            │  (history)   │
      │                │                  │            └──────────────┘
      ▼                ▼                  ▼
┌──────────────────────────────────────────────┐
│              Cancelled / Expired              │
└──────────────────────────────────────────────┘
```

```rust
pub struct Transfer {
    /// Unique transfer identifier
    id: TransferId,

    /// Current state
    state: TransferState,

    /// The source agent and object reference
    source: TransferEndpoint,

    /// The content being transferred
    content: TypedContent,

    /// What the sender intends
    intent: TransferIntent,

    /// Target: specific agent, any agent, or global clipboard
    target: FlowTarget,

    /// Transformations that have been applied or are pending
    transformations: Vec<Transform>,

    /// Whether the content should be purged after delivery
    ephemeral: bool,

    /// Expiration time (for unclaimed transfers)
    expires_at: Option<Timestamp>,

    /// Shared memory region holding the content (COW)
    content_region: SharedMemoryId,
}

pub enum TransferState {
    /// Source has initiated, content not yet staged
    Initiated,
    /// Content staged in Flow's shared memory, ready for receiver
    Staged,
    /// Receiver has accepted, type negotiation in progress
    Negotiating,
    /// Content delivered to receiver
    Delivered,
    /// Transfer cancelled by source or system
    Cancelled,
    /// Transfer expired (no receiver claimed it)
    Expired,
}

pub struct TransferEndpoint {
    agent: AgentId,
    object: Option<ObjectRef>,
    surface: Option<SurfaceId>,  // for drag/drop: which surface
}

pub enum FlowTarget {
    /// Any agent can pull this (global clipboard behavior)
    Any,
    /// Specific agent only
    Agent(AgentId),
    /// Specific surface (for drag/drop targeting)
    Surface(SurfaceId),
}
```

**Detailed lifecycle walkthrough:**

```
1. INITIATE
   Source agent calls flow.push(content, options)
   Flow Service: check FlowWrite capability → create Transfer(state: Initiated)

2. STAGE
   Content copied into Flow's shared memory region (copy-on-write)
   If content is an ObjectRef and intent is Reference: no copy, just store the ref
   Transfer state → Staged
   Transfer visible in Flow Tray

3. ACCEPT
   Receiver calls flow.pull() or user drops onto target
   Flow Service: check FlowRead capability on receiver
   Transfer state → Negotiating

4. NEGOTIATE
   Flow compares source content type with receiver's accepted types
   If compatible: proceed directly
   If incompatible: Transform Engine finds conversion path
   If no path exists: transfer fails with IncompatibleType error

5. TRANSFORM (if needed)
   Transform Engine executes the cheapest conversion path
   Transformation recorded in TransformRecord
   Transformed content staged in new shared memory region

6. DELIVER
   Content (original or transformed) mapped into receiver's address space
   For Move intent: source object archived/deleted
   For Reference intent: receiver gets an ObjectRef, not content
   For Quote intent: DerivedFrom relation created in space
   Transfer state → Delivered

7. RECORD
   FlowEntry created in history store
   Provenance chain appended
   Content deduplicated in history (content-addressed)
   Transfer state → Recorded (final)
```

### 3.3 TransferIntent

Each intent carries different semantics for how the content relates to its source:

```rust
pub enum TransferIntent {
    /// Duplicate content. New independent object, no link to original.
    /// This is the default clipboard behavior. Receiver gets a full copy.
    /// Source is unaffected.
    Copy,

    /// Transfer ownership. Content moves from source to destination.
    /// After delivery, the source object is archived (versioned, then removed
    /// from active space). The destination object is the canonical copy.
    Move,

    /// Share a reference, not the content. Receiver gets an ObjectRef.
    /// The receiver sees the live object — changes to the original are visible.
    /// No content duplication. Efficient for large objects.
    Reference,

    /// Copy with attribution. Creates a new object with a DerivedFrom
    /// relation linking it to the source. Used for quoting, citing, or
    /// pasting with context. The receiver can trace back to the original.
    Quote,

    /// Transform and create a new object with a provenance link.
    /// The content is modified (summarized, translated, reformatted)
    /// and the resulting object records its derivation.
    Derive,
}
```

**Intent behavior matrix:**

| Intent | Content copied? | Source affected? | Relation created? | Provenance link? |
|---|---|---|---|---|
| Copy | Yes (full copy) | No | No | Yes (for history) |
| Move | Yes (transferred) | Archived | No | Yes |
| Reference | No (ObjectRef only) | No | References | Yes |
| Quote | Yes (full copy) | No | DerivedFrom | Yes |
| Derive | Yes (transformed) | No | DerivedFrom | Yes |

### 3.4 TypedContent

Content in Flow is never raw bytes. It always carries type information and alternative representations:

```rust
pub struct TypedContent {
    /// The primary content payload
    primary: ContentPayload,

    /// Standard MIME type (e.g., "text/html", "image/png", "application/pdf")
    mime_type: String,

    /// AIOS semantic type — richer than MIME, used for type negotiation
    semantic_type: SemanticType,

    /// Same content in alternative formats, pre-computed by source
    /// e.g., rich text agent provides both text/html and text/plain
    alternatives: Vec<ContentPayload>,

    /// Metadata about the content
    metadata: ContentMetadata,
}

pub struct ContentPayload {
    /// Shared memory region containing the data
    data: SharedMemoryId,
    /// Size in bytes
    size: u64,
    /// MIME type of this specific payload
    mime_type: String,
}

pub enum SemanticType {
    /// Plain text (terminal output, notes, logs)
    PlainText,
    /// Rich text (formatted document content)
    RichText,
    /// Source code with language info
    Code { language: String },
    /// URL or URI
    Link { title: Option<String> },
    /// Image (photograph, screenshot, diagram)
    Image { width: u32, height: u32 },
    /// Audio (recording, music, podcast clip)
    Audio { duration: Duration },
    /// Video (clip, recording, screen capture)
    Video { duration: Duration, width: u32, height: u32 },
    /// Document (PDF, office document, ebook)
    Document { page_count: Option<u32> },
    /// Structured data (JSON, CSV, table)
    StructuredData { schema: Option<String> },
    /// Space object reference
    ObjectReference,
    /// File (generic, for POSIX compat)
    File { extension: String },
    /// Agent-defined custom type
    Custom { type_id: String },
}

pub struct ContentMetadata {
    /// Human-readable title or summary of the content
    title: Option<String>,
    /// Source description (e.g., "Copied from arxiv.org/abs/2026.12345")
    source_description: Option<String>,
    /// Content hash (SHA-256) for deduplication
    content_hash: Hash,
    /// When the content was originally created (not when it was transferred)
    created_at: Option<Timestamp>,
    /// Size of the primary payload
    size: u64,
    /// Thumbnail (small preview image, ≤ 64KB)
    thumbnail: Option<Vec<u8>>,
}
```

When a source agent pushes content, it provides the primary payload and optionally pre-computed alternatives. If the receiver cannot handle the primary type and no pre-computed alternative matches, the Transform Engine generates one on the fly.

**Copy-on-write semantics:** Content in Flow uses shared memory regions with COW (copy-on-write) page mappings. When Agent A pushes content into Flow, the kernel maps the same physical pages into Flow's address space. No copy occurs until someone modifies the data. For read-only transfers (the common case), the content is never copied at all — just the page table entries.

-----

## 4. Transform Engine

### 4.1 What Transforms Do

Transforms convert content between types so that data can flow between agents that speak different formats. Without transforms, a terminal agent could not receive rich text, and a browser agent could not receive raw code. Transforms make Flow universal.

Core transform categories:

| Source Type | Target Type | Transform | Requires AIRS? |
|---|---|---|---|
| Rich text (HTML) | Plain text | Strip tags, preserve structure | No |
| Plain text | Rich text (HTML) | Wrap in `<pre>`, escape entities | No |
| Code | Formatted HTML | Syntax highlighting | No |
| Image (any) | Image (PNG/JPEG) | Format conversion, resize | No |
| Image | Thumbnail (64KB max) | Downscale, compress | No |
| PDF | Plain text | Text extraction | No |
| Audio | Text transcript | Speech-to-text | Yes |
| Document | Summary | Summarization | Yes |
| Any text | Translation | Language translation | Yes |
| Any | Embedding (Vec<f32>) | Embedding generation | Yes |
| Rich text | Markdown | HTML-to-Markdown conversion | No |
| Structured data (JSON) | Formatted table (HTML) | Table rendering | No |
| URL | Page content (HTML) | Fetch and extract | No (network) |

### 4.2 Transform Pipeline

When a receiver accepts a transfer, the Transform Engine determines if conversion is needed and executes it:

```
Source content (TypedContent)
  │
  ▼
Type Negotiation
  What types can the receiver accept?
  Does the source provide any of those types (primary or alternatives)?
  ├── YES → deliver directly, no transform needed
  └── NO  → continue to transform selection
  │
  ▼
Transform Selection
  Find conversion path from source type to accepted target type
  Multiple paths may exist — select the cheapest:
    cost = execution_time_estimate + resource_estimate
  ├── System transform available → use it (fast, always available)
  ├── AIRS transform needed → check AIRS availability
  │     ├── AIRS running → use it
  │     └── AIRS not running → fall back to system transform or fail
  └── Agent transform registered → route through agent
  │
  ▼
Transform Execution
  Execute the selected transform
  Input: source ContentPayload (shared memory)
  Output: new ContentPayload (new shared memory region)
  Record: TransformRecord (what, when, cost)
  │
  ▼
Delivery to receiver
  Map transformed content into receiver's address space
```

### 4.3 Transform Registry

Transforms are registered with the Flow Service. System transforms ship with the OS. AIRS transforms become available when AIRS is running. Third-party agents can contribute transforms.

```rust
pub struct Transform {
    /// Unique identifier
    id: TransformId,

    /// Human-readable name
    name: String,

    /// Source types this transform accepts
    input_types: Vec<TypeMatcher>,

    /// Target type this transform produces
    output_type: TypeSpec,

    /// Estimated cost (lower is preferred)
    cost: TransformCost,

    /// Who provides this transform
    provider: TransformProvider,
}

pub struct TypeMatcher {
    /// MIME type pattern (supports wildcards: "text/*", "image/*")
    mime_pattern: String,
    /// Optional semantic type constraint
    semantic_type: Option<SemanticType>,
}

pub struct TypeSpec {
    mime_type: String,
    semantic_type: SemanticType,
}

pub struct TransformCost {
    /// Estimated time in milliseconds
    time_ms: u32,
    /// Estimated memory in bytes
    memory: u64,
    /// Whether this transform requires AIRS
    requires_airs: bool,
    /// Whether this is lossy (information may be lost)
    lossy: bool,
}

pub enum TransformProvider {
    /// Built-in system transform (always available)
    System,
    /// AIRS-powered transform (available when AIRS is running)
    Airs,
    /// Agent-provided transform (available when agent is running)
    Agent(AgentId),
}

pub struct TransformRegistry {
    /// All registered transforms, indexed by input type
    transforms: HashMap<String, Vec<Transform>>,  // mime pattern → transforms

    /// Precomputed shortest-path conversion graph
    /// Updated when transforms are registered/unregistered
    conversion_graph: ConversionGraph,
}

pub struct TransformRecord {
    /// Which transform was applied
    transform: TransformId,
    /// Input type
    input_type: String,
    /// Output type
    output_type: String,
    /// Execution time
    duration: Duration,
    /// Whether information was lost
    lossy: bool,
    /// Timestamp
    executed_at: Timestamp,
}
```

**Conversion graph:** The registry maintains a directed graph where nodes are content types and edges are transforms. When a conversion is needed, the engine finds the shortest (cheapest) path. The graph is recomputed when transforms are added or removed.

```
PlainText ←──── RichText ←──── HTML
    │              │               ↑
    ▼              ▼               │
 Embedding     Markdown        Code (highlight)
                   │
                   ▼
              FormattedHTML
```

If a receiver needs Markdown but the source provides HTML, the engine walks: HTML → RichText → Markdown (two system transforms, no AIRS needed, fast).

If a receiver needs an embedding but the source provides Audio, the engine walks: Audio → Transcript (AIRS) → Embedding (AIRS). The engine checks AIRS availability before committing to this path.

-----

## 5. Flow History

### 5.1 Storage

Flow history is stored in the `system/flow/` space:

```
system/flow/
  history/            ← FlowEntry objects, content-addressed
  index/              ← Full-text index of entry metadata
  transforms/         ← Transform registry (persistent transforms)
  config/             ← Retention policy, user preferences
```

Each FlowEntry is a space object. The content payload is stored as a content-addressed block, so identical content shared ten times occupies storage once. The object's semantic metadata includes:

- Content type and semantic type (for filtering)
- Source agent name and destination agent name (for searching)
- Content title or first 200 characters (for full-text search)
- Timestamp (for temporal queries)

AIRS indexes Flow history like any other space. Users can search semantically: "that code snippet I copied from the browser yesterday" routes through AIRS to the Flow history space and returns matching entries.

### 5.2 History UI

The Flow Tray (see [aios-experience.md](../experience/aios-experience.md)) provides the user-facing history interface:

```
┌─ FLOW HISTORY (Ctrl+Shift+V) ──────────────────────────────────┐
│                                                                   │
│  Search: [                                               ] [🔍]  │
│                                                                   │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │  "pub fn transform_engine..."                    3 min ago  │ │
│  │  Code (Rust) · from: editor-agent · to: browser-agent      │ │
│  │  Intent: Copy                                    [Re-send]  │ │
│  ├─────────────────────────────────────────────────────────────┤ │
│  │  [thumbnail]  Screenshot of architecture diagram  15 min ago│ │
│  │  Image (PNG, 1920x1080) · from: screenshot-agent            │ │
│  │  Intent: Copy                                    [Re-send]  │ │
│  ├─────────────────────────────────────────────────────────────┤ │
│  │  "Attention mechanisms allow models to..."       1 hour ago │ │
│  │  RichText · from: browser-tab (arxiv.org) · to: research   │ │
│  │  Intent: Quote · Transform: HTML → PlainText     [Re-send]  │ │
│  ├─────────────────────────────────────────────────────────────┤ │
│  │  research/papers/transformer-survey.pdf        yesterday    │ │
│  │  Reference · from: research-agent                           │ │
│  │  Intent: Reference                               [Re-send]  │ │
│  └─────────────────────────────────────────────────────────────┘ │
│                                                                   │
│  Showing 4 of 247 entries · [Load more]                          │
└──────────────────────────────────────────────────────────────────┘
```

**Features:**
- Keyboard shortcut (Ctrl+Shift+V) opens Flow History as a compositor overlay
- Each entry shows: content preview, content type, source agent, destination agent (if known), timestamp, intent
- Text search filters entries by content, agent name, or type
- Re-send button pushes a historical entry back into active Flow as a new transfer
- AIRS semantic search available: type a natural language query, AIRS finds matching entries
- Entries are grouped by time (now, minutes ago, hours ago, yesterday, older)

### 5.3 Retention Policy

```rust
pub struct FlowRetentionPolicy {
    /// Maximum number of entries to keep
    max_entries: u64,                    // default: 1000

    /// Maximum age of entries
    max_age: Duration,                   // default: 30 days

    /// Maximum total storage for Flow history
    max_storage: u64,                    // default: 500 MB

    /// Threshold above which content is stored as reference only
    /// (the FlowEntry metadata is kept, but the content block may be pruned)
    large_content_threshold: u64,        // default: 10 MB

    /// How long to keep ephemeral entries after delivery
    ephemeral_retention: Duration,       // default: 0 (purge immediately)
}
```

**Retention rules:**

1. **Default:** Keep the last 1000 entries or 30 days, whichever is more restrictive.
2. **Large content (>10 MB):** The FlowEntry metadata and a thumbnail are kept. The full content block may be pruned if storage pressure exists. The entry shows "[Content pruned — original in source space]" with a link to the source object if it still exists.
3. **Ephemeral transfers:** Content marked `ephemeral: true` is purged from history immediately after delivery. The FlowEntry metadata record remains (for audit trail) but the content block is zeroed and freed. Use case: password manager copying credentials into a form.
4. **User override:** Users can pin specific entries (never pruned), delete entries manually, or adjust the retention policy through preferences.
5. **Pruning order:** When the limit is reached, oldest unpinned entries are pruned first. Entries with `content_pruned: true` are pruned before entries with live content.

-----

## 6. Compositor Integration (Drag and Drop)

### 6.1 Semantic Drag and Drop

Traditional drag and drop is a brittle protocol. The source application serializes data into one or more clipboard formats. The target application advertises which formats it accepts. The window manager mediates the format negotiation but has no understanding of the content. The result: drag and drop between applications is unreliable, type-lossy, and context-free.

Flow drag and drop is different. The compositor initiates a Flow transfer on drag start. The source agent provides TypedContent with its full type information and alternatives. As the cursor moves over potential drop targets, Flow negotiates type compatibility in real time. The drop target receives semantic content, not raw bytes.

**Detailed drag/drop protocol:**

```
1. USER STARTS DRAG
   Compositor detects drag gesture (pointer down + movement threshold)
   Compositor sends DragStarted event to source surface's agent
   Source agent responds with:
     TypedContent { primary, mime_type, semantic_type, alternatives, metadata }

2. FLOW TRANSFER INITIATED
   Compositor calls Flow Service: initiate_drag(content, source_agent, source_surface)
   Flow Service creates Transfer(state: Staged, target: Surface(cursor_position))
   Content staged in Flow's shared memory (COW from source agent's buffer)

3. CURSOR MOVES OVER TARGETS
   For each surface the cursor enters:
     Compositor queries Flow: can_accept(target_agent, content_type)?
     Flow checks:
       a. Does target agent have FlowRead capability?
       b. Does target agent accept this content type?
       c. If not directly: can Transform Engine convert it?
     Flow returns: Compatible | NeedsTransform(transform_name) | Incompatible

4. VISUAL FEEDBACK (see §6.2)
   Compositor updates cursor and target surface appearance based on compatibility

5. USER DROPS
   Compositor sends DropReceived event to target surface's agent
   Flow Service: negotiate and transform if needed
   Content delivered to target agent's address space
   Transfer state → Delivered → Recorded

6. DRAG CANCELLED
   User releases outside any valid target, or presses Escape
   Transfer state → Cancelled
   Content region freed
```

```rust
/// Compositor → Flow Service messages during drag/drop
pub enum DragFlowRequest {
    /// Drag started: source provides content
    DragStarted {
        source_agent: AgentId,
        source_surface: SurfaceId,
        content: TypedContent,
    },

    /// Cursor entered a potential drop target
    DragEntered {
        target_agent: AgentId,
        target_surface: SurfaceId,
    },

    /// Cursor left a potential drop target
    DragLeft {
        target_surface: SurfaceId,
    },

    /// User dropped on a target
    Drop {
        target_agent: AgentId,
        target_surface: SurfaceId,
        position: (f32, f32),
    },

    /// Drag cancelled (escape, dropped outside targets)
    DragCancelled,
}

/// Flow Service → Compositor responses
pub enum DragFlowResponse {
    /// Transfer initiated, drag may proceed
    DragAccepted { transfer_id: TransferId },

    /// Target compatibility result
    TargetCompatibility {
        surface: SurfaceId,
        result: DropCompatibility,
    },

    /// Drop completed
    DropCompleted { transform_applied: Option<String> },

    /// Drop failed
    DropFailed { reason: String },
}

pub enum DropCompatibility {
    /// Target accepts this content directly
    Compatible,
    /// Target can accept after transformation
    NeedsTransform { transform_name: String, lossy: bool },
    /// Target cannot accept this content
    Incompatible,
}
```

### 6.2 Visual Feedback

The compositor provides visual cues during drag operations, informed by Flow's type negotiation:

```
┌─────────────────────────────────────┐
│  Source Surface                      │
│                                     │
│    [dragging: code snippet]         │
│        ╲                            │
└─────────╲───────────────────────────┘
           ╲
            ╲   ┌── Drag Preview ─────────┐
             ╲  │  pub fn transform()...   │
              ╲ │  Rust · 12 lines         │
               ╲└──────────────────────────┘
                ╲
  ┌──────────────╲──────────┐  ┌────────────────────────┐
  │  Terminal     ╲         │  │  Browser Tab            │
  │  (compatible)  •        │  │  (needs transform)      │
  │                         │  │                          │
  │  ┌─green glow──────┐   │  │  ┌─amber glow───────┐   │
  │  │ drop here       │   │  │  │ will convert to   │   │
  │  │                 │   │  │  │ formatted HTML     │   │
  │  └─────────────────┘   │  │  └────────────────────┘  │
  └─────────────────────────┘  └──────────────────────────┘
```

**Visual states:**

| State | Appearance |
|---|---|
| Drag preview | Content-aware thumbnail attached to cursor (text: first few lines; image: scaled preview; file: icon + name) |
| Compatible target | Subtle green glow on drop zone, cursor changes to "drop" icon |
| Needs transform | Amber glow on drop zone, label shows what conversion will happen ("will convert to plain text") |
| Incompatible target | No glow, cursor remains "drag" icon, target surface is visually neutral |
| Drag over non-target area | No feedback, standard cursor |

The drag preview is generated from `ContentMetadata.thumbnail` if available, or synthesized by the compositor (first 5 lines of text, scaled-down image, icon for documents).

-----

## 7. Subsystem Data Channels

### 7.1 How Subsystems Connect to Flow

Every subsystem's DataChannel (see [aios-subsystem-framework.md](../platform/aios-subsystem-framework.md)) has a `connect_flow()` method. This is the bridge between hardware data streams and the Flow system:

```rust
/// From the subsystem framework DataChannel trait
fn connect_flow(&self, flow: FlowPipe) -> Result<()>;
```

A `FlowPipe` is a unidirectional connection between a DataChannel and the Flow Service:

```rust
pub struct FlowPipe {
    /// Direction: hardware → Flow, or Flow → hardware
    direction: FlowPipeDirection,

    /// The content type flowing through this pipe
    content_type: TypedContentSpec,

    /// Back-pressure control
    buffer_size: usize,

    /// The IPC channel to the Flow Service
    channel: ChannelId,
}

pub enum FlowPipeDirection {
    /// Data flows from hardware/subsystem into Flow
    Source,
    /// Data flows from Flow into hardware/subsystem
    Sink,
}

pub struct TypedContentSpec {
    mime_type: String,
    semantic_type: SemanticType,
    streaming: bool,
}
```

**Example pipelines:**

```rust
// Microphone → Flow → Speech-to-text agent
let mic_session = audio.open_session(agent, mic_cap, &intent)?;
let speech_pipe = flow.create_pipe(FlowPipeDirection::Sink, speech_agent)?;
mic_session.channel().connect_flow(speech_pipe)?;
// Audio samples flow from hardware → Flow Service → speech agent
// Zero-copy if both are on the same device (shared memory)

// Camera → Flow → Image analysis agent
let cam_session = camera.open_session(agent, cam_cap, &intent)?;
let analysis_pipe = flow.create_pipe(FlowPipeDirection::Sink, analysis_agent)?;
cam_session.channel().connect_flow(analysis_pipe)?;

// Clipboard (POSIX tools) → Flow → native agent
// (handled by the POSIX clipboard bridge — see §10)
```

### 7.2 Streaming Flow

One-shot transfers (copy/paste) are the common case. But Flow also supports streaming transfers for continuous data — audio, video, sensor feeds:

```rust
pub struct StreamingTransfer {
    /// Base transfer metadata
    transfer: Transfer,

    /// Stream state
    stream_state: StreamState,

    /// Chunks delivered so far
    chunks_delivered: u64,

    /// Total bytes delivered
    bytes_delivered: u64,

    /// Back-pressure: receiver's buffer fullness (0.0 = empty, 1.0 = full)
    receiver_pressure: f32,

    /// Transform applied per-chunk (if any)
    chunk_transform: Option<TransformId>,
}

pub enum StreamState {
    /// Stream is active, data flowing
    Active,
    /// Stream paused (back-pressure or user action)
    Paused,
    /// Stream completed (source has no more data)
    Completed,
    /// Stream aborted (error or cancellation)
    Aborted,
}
```

**Back-pressure:** The receiver controls the data rate. When the receiver's buffer fills (pressure approaches 1.0), Flow signals the source to slow down. For subsystem DataChannels, this maps to the existing `pressure()` method on the DataChannel trait. Hardware that cannot be slowed (e.g., a live microphone) buffers in the kernel's ring buffer; if that fills, samples are dropped and the drop is logged.

**Per-chunk transforms:** For streaming data, transforms can be applied incrementally. Real-time transcription: each audio chunk is sent to AIRS for speech-to-text, and the text result is delivered to the receiver. The receiver sees a stream of text chunks, not audio. The transform overhead is amortized across the stream.

-----

## 8. Cross-Agent Flow

### 8.1 Agent-to-Agent Transfer

The most common Flow pattern: one agent pushes content, another pulls it.

```
Agent A (editor)                Flow Service                Agent B (terminal)
      │                              │                            │
      │  push(TypedContent,          │                            │
      │    intent: Copy,             │                            │
      │    target: Any)              │                            │
      │─────────────────────────────→│                            │
      │                              │                            │
      │  [FlowWrite cap checked]     │                            │
      │  [Content staged in          │                            │
      │   shared memory, COW]        │                            │
      │                              │                            │
      │  Ok(TransferId)              │                            │
      │←─────────────────────────────│                            │
      │                              │                            │
      │                              │  pull(FlowFilter {         │
      │                              │    content_type: Text })   │
      │                              │←───────────────────────────│
      │                              │                            │
      │                              │  [FlowRead cap checked]   │
      │                              │  [Type negotiation:        │
      │                              │   source=Code(Rust),       │
      │                              │   target accepts PlainText │
      │                              │   → apply strip-formatting │
      │                              │   transform]               │
      │                              │                            │
      │                              │  Ok(FlowEntry {            │
      │                              │    content: plain text,    │
      │                              │    transform: applied })   │
      │                              │───────────────────────────→│
      │                              │                            │
```

**Targeted transfers:** When Agent A knows the destination, it can target specifically:

```rust
// Agent A pushes content to Agent B specifically
ctx.flow().push(content, FlowOptions {
    intent: TransferIntent::Copy,
    target: FlowTarget::Agent(agent_b_id),
    ephemeral: false,
}).await?;
```

Only Agent B (or an agent with elevated FlowRead that covers this transfer) can pull this content. Other agents calling `flow.pull()` will not see it.

### 8.2 Capability Requirements

Flow access is governed by two capabilities defined in the system capability enum:

```rust
// From aios-architecture.md §2.11
pub enum Capability {
    // ...
    FlowRead,
    FlowWrite,
    // ...
}
```

**FlowWrite** allows an agent to push content into Flow. Without this capability, an agent cannot initiate transfers. Most agents request this — it is the equivalent of "can put things on the clipboard."

**FlowRead** allows an agent to pull content from Flow. Without this capability, an agent cannot receive transfers. This prevents malicious agents from snooping on clipboard content.

**Capability enforcement rules:**

| Operation | Required Capability | Additional Check |
|---|---|---|
| Push content (any target) | FlowWrite | None |
| Push content (specific agent) | FlowWrite | Target agent must have FlowRead |
| Pull content (any) | FlowRead | Only sees transfers targeted to this agent or to Any |
| Pull content (targeted) | FlowRead | Only if transfer is targeted to this agent |
| Browse history | FlowRead | Only sees entries where this agent was source or destination |
| Search history | FlowRead | AIRS search returns only entries visible to this agent |
| Register transform | FlowWrite | Transform becomes available system-wide |

**Inspector visibility:** The Inspector agent (system diagnostic tool) can see the full Flow audit trail regardless of targeting. This is a system agent privilege, not available to third-party agents. The Inspector shows all transfers with source, destination, type, intent, and timestamp — but not content, unless the user explicitly requests it.

-----

## 9. Multi-Device Flow

### 9.1 Cross-Device Transfer

AIOS devices sharing an identity can sync Flow. Copy on your laptop, paste on your tablet.

```
Device A (laptop)                               Device B (tablet)
      │                                               │
      │  User copies text                             │
      │  → Flow push(content,                         │
      │      intent: Copy,                            │
      │      target: Any)                             │
      │                                               │
      │  Flow Service stores locally                  │
      │  Flow Service replicates via                  │
      │  AIOS Peer Protocol                           │
      │  ─────────────────────────────────────────→   │
      │  [encrypted with identity keys]               │
      │  [includes: TypedContent, intent,             │
      │   source_device, timestamp]                   │
      │                                               │
      │                              Flow Service receives
      │                              Stores in local history
      │                              Available for pull()
      │                                               │
      │                              User pastes      │
      │                              → Flow pull()    │
      │                              → content delivered
      │                                               │
```

**Transport:** Cross-device Flow uses the AIOS Peer Protocol (defined in [aios-networking.md](../platform/aios-networking.md)). Content is encrypted in transit using the shared identity's keys. The Peer Protocol handles device discovery, connection establishment, and reliable delivery.

**What syncs:**
- Active transfers (target: Any) are replicated to all devices
- Active transfers (target: specific agent) are replicated only if that agent exists on the remote device
- History entries are replicated for unified history across devices
- Large content (>10 MB) syncs metadata first; full content syncs on demand (when user pulls)

**What does not sync:**
- Ephemeral transfers (ephemeral: true) are never replicated
- Streaming transfers are not replicated (they are device-local)
- Transfers between agents on the same device with target: Agent(id) are not replicated

### 9.2 Conflict Resolution

Flow uses a simple conflict resolution model:

**Active transfers:** Latest-write-wins. If both devices push content to the global clipboard (target: Any) simultaneously, the most recent push wins. The older push remains in history but is no longer the active transfer. Timestamps are synchronized via NTP; in case of exact tie, the device with the lower DeviceId wins (deterministic).

**History merge:** History entries from all devices are merged into a unified timeline. Each entry carries its `origin_device` field. Entries are uniquely identified by `(FlowEntryId, origin_device)`, so there are no conflicts — just union. This is equivalent to a grow-only CRDT (each device's history is append-only, merge is union).

```rust
pub struct FlowHistorySync {
    /// Local history watermark per remote device
    /// "I have seen all entries from device X up to this timestamp"
    watermarks: HashMap<DeviceId, Timestamp>,

    /// Pending entries to send to each device
    outbound: HashMap<DeviceId, Vec<FlowEntryId>>,

    /// Entries received but not yet integrated
    inbound: Vec<FlowEntry>,
}
```

**Sync protocol:**

```
1. Device A connects to Device B via Peer Protocol
2. Exchange watermarks: "I have your entries up to timestamp T"
3. Each device sends entries the other hasn't seen (delta sync)
4. Entries are merged into local history (append, deduplicate by content_hash)
5. Watermarks updated
```

-----

## 10. POSIX Compatibility

### 10.1 Clipboard Bridge

BSD tools and POSIX applications expect a clipboard. They use `pbcopy`/`pbpaste` (macOS convention), X11 selections, or Wayland clipboard protocols. The Flow POSIX bridge translates these into Flow operations.

```rust
pub struct PosixClipboardBridge {
    /// The Flow Service IPC channel
    flow_channel: ChannelId,

    /// X11 selection protocol handler (for X11 compat layer)
    x11_selections: Option<X11SelectionHandler>,

    /// Wayland clipboard protocol handler (for Wayland compat layer)
    wayland_clipboard: Option<WaylandClipboardHandler>,
}

impl PosixClipboardBridge {
    /// Called when a POSIX tool writes to the clipboard
    /// (e.g., `echo "hello" | pbcopy`)
    fn clipboard_write(&self, data: &[u8], mime_type: &str) -> Result<()> {
        let content = TypedContent {
            primary: ContentPayload::from_bytes(data),
            mime_type: mime_type.to_string(),
            semantic_type: SemanticType::infer_from_mime(mime_type),
            alternatives: vec![],
            metadata: ContentMetadata::default(),
        };

        self.flow_push(content, TransferIntent::Copy, FlowTarget::Any)
    }

    /// Called when a POSIX tool reads from the clipboard
    /// (e.g., `pbpaste > file.txt`)
    fn clipboard_read(&self, requested_mime: &str) -> Result<Vec<u8>> {
        let entry = self.flow_pull(FlowFilter {
            content_type: Some(requested_mime.to_string()),
            ..Default::default()
        })?;

        Ok(entry.content.primary.as_bytes().to_vec())
    }
}
```

**POSIX clipboard commands:**

| Command | Equivalent Flow Operation |
|---|---|
| `pbcopy` (stdin → clipboard) | `flow.push(stdin_content, Copy, Any)` |
| `pbpaste` (clipboard → stdout) | `flow.pull(text/plain)` → stdout |
| `xclip -selection clipboard` | Same as pbcopy/pbpaste via X11 selection protocol |
| `wl-copy` / `wl-paste` | Same via Wayland clipboard protocol |
| `xdotool` (X11 drag/drop simulation) | Translated to compositor drag/drop events |

**X11 selection protocol:** The X11 compatibility layer (Phase 25) translates X11's three selections (PRIMARY, SECONDARY, CLIPBOARD) into Flow operations. PRIMARY (select-to-copy) maps to `flow.push(intent: Copy)`. CLIPBOARD (explicit copy) maps to the same. SECONDARY is rarely used and maps to a targeted transfer.

**Wayland clipboard protocol:** The Wayland compatibility layer translates `wl_data_offer` / `wl_data_source` into Flow push/pull. MIME type negotiation in Wayland maps to Flow's type negotiation.

POSIX tools never know they are talking to Flow. They see a standard clipboard interface. But the data they put on the "clipboard" gets full Flow treatment — typed, recorded in history, available across devices, searchable.

-----

## 11. Security

### 11.1 Capability Enforcement

Flow security is built on the same capability model as every other AIOS subsystem:

```
Agent wants to push content to Flow:
  1. Agent calls ctx.flow().push(content, options)
  2. SDK sends IPC to Flow Service (sys.flow channel)
  3. Kernel IPC handler checks: does this agent hold FlowWrite capability?
     NO  → IPC rejected, agent gets PermissionDenied
     YES → message delivered to Flow Service
  4. Flow Service checks: is the transfer target valid?
     If target is Agent(id): does target agent exist and have FlowRead?
  5. Transfer proceeds

Agent wants to pull content from Flow:
  1. Agent calls ctx.flow().pull(filter)
  2. SDK sends IPC to Flow Service
  3. Kernel checks FlowRead capability
  4. Flow Service checks: is there a transfer visible to this agent?
     - Transfers with target: Any → visible to all agents with FlowRead
     - Transfers with target: Agent(this_agent) → visible
     - Transfers with target: Agent(other_agent) → NOT visible
  5. Content delivered (with transform if needed)
```

**Isolation guarantee:** An agent with FlowRead cannot read transfers targeted at other agents. The Flow Service enforces this. The kernel enforces that only agents with FlowRead can even talk to the Flow Service's read endpoint.

### 11.2 Content Screening

AIRS screens Flow content for sensitive data before delivery to untrusted agents:

```rust
pub struct FlowContentScreen {
    /// Patterns that indicate sensitive content
    sensitive_patterns: Vec<SensitivePattern>,

    /// Agent trust levels (from security framework)
    trust_levels: HashMap<AgentId, TrustLevel>,
}

pub struct SensitivePattern {
    /// Human-readable name
    name: String,
    /// Detection method
    detector: SensitiveDetector,
    /// What to do when detected
    action: ScreenAction,
}

pub enum SensitiveDetector {
    /// Regex pattern (credit card numbers, SSNs, API keys)
    Regex(String),
    /// AIRS classification (passwords, credentials, PII)
    AirsClassifier(String),
}

pub enum ScreenAction {
    /// Allow but warn user
    Warn,
    /// Block transfer, require user confirmation
    Block,
    /// Redact the sensitive portion
    Redact,
}
```

**Screening rules:**

| Pattern | Detection | Action on untrusted agent |
|---|---|---|
| Credit card number | Regex (Luhn check) | Block, require confirmation |
| Social Security Number | Regex | Block, require confirmation |
| API key / token | Regex (`sk-`, `ghp_`, `AKIA`) | Warn |
| Password | AIRS classifier | Block |
| PII (address, phone) | AIRS classifier | Warn |
| Private key material | Regex (BEGIN PRIVATE KEY) | Block |

**Trust-based screening:** Screening is only applied when content flows to agents with lower trust than the source. System agents are fully trusted. Native experience agents are trusted. Third-party agents and tab agents are screened. Transfer between two system agents is never screened (performance optimization).

**Inspector audit trail:** Every Flow transfer is logged to the audit space (`system/audit/flow/`). The Inspector shows the full trail: timestamp, source agent, destination agent, content type, intent, any transforms applied, any screening actions taken. This is the transparency mechanism — the user can always see what data moved where.

-----

## 12. SDK API

### 12.1 Rust API

The agent SDK exposes Flow through the `FlowClient` trait on the `AgentContext`:

```rust
/// The Flow client interface, accessed via ctx.flow()
#[async_trait]
pub trait FlowClient: Send + Sync {
    /// Push content into Flow.
    /// Requires FlowWrite capability.
    async fn push(
        &self,
        content: TypedContent,
        options: FlowOptions,
    ) -> Result<TransferId>;

    /// Pull the most recent matching content from Flow.
    /// Requires FlowRead capability.
    /// Blocks until content is available or timeout expires.
    async fn pull(
        &self,
        filter: FlowFilter,
    ) -> Result<FlowEntry>;

    /// Pull without blocking. Returns None if no matching content is available.
    async fn try_pull(
        &self,
        filter: FlowFilter,
    ) -> Result<Option<FlowEntry>>;

    /// Browse Flow history.
    async fn history(
        &self,
        query: FlowQuery,
    ) -> Result<Vec<FlowEntry>>;

    /// Search Flow history semantically (requires AIRS).
    async fn search(
        &self,
        natural_language_query: &str,
        limit: u32,
    ) -> Result<Vec<FlowEntry>>;

    /// Create a Flow pipe for streaming data.
    async fn create_pipe(
        &self,
        direction: FlowPipeDirection,
        target: FlowTarget,
    ) -> Result<FlowPipe>;

    /// Register a transform with the Flow Service.
    async fn register_transform(
        &self,
        transform: Transform,
        handler: Box<dyn TransformHandler>,
    ) -> Result<TransformId>;

    /// Subscribe to Flow events (new transfers, deliveries).
    async fn subscribe(
        &self,
        filter: FlowFilter,
    ) -> Result<FlowSubscription>;
}

pub struct FlowOptions {
    /// Transfer intent
    pub intent: TransferIntent,
    /// Target (Any, specific agent, specific surface)
    pub target: FlowTarget,
    /// Whether to purge content after delivery
    pub ephemeral: bool,
    /// Optional expiration time
    pub expires_in: Option<Duration>,
}

pub struct FlowFilter {
    /// Filter by content type (MIME pattern, e.g., "text/*")
    pub content_type: Option<String>,
    /// Filter by semantic type
    pub semantic_type: Option<SemanticType>,
    /// Filter by source agent
    pub source_agent: Option<AgentId>,
    /// Filter by intent
    pub intent: Option<TransferIntent>,
    /// Timeout for blocking pull
    pub timeout: Option<Duration>,
}

pub struct FlowQuery {
    /// Time range
    pub since: Option<Timestamp>,
    pub until: Option<Timestamp>,
    /// Content type filter
    pub content_type: Option<String>,
    /// Semantic type filter
    pub semantic_type: Option<SemanticType>,
    /// Source or destination agent
    pub agent: Option<AgentId>,
    /// Maximum results
    pub limit: u32,
    /// Offset for pagination
    pub offset: u32,
}

pub struct FlowSubscription {
    /// Channel that receives FlowEvent notifications
    channel: ChannelId,
}

pub enum FlowEvent {
    /// New content available matching the subscription filter
    ContentAvailable { transfer_id: TransferId },
    /// A transfer was completed (delivered)
    TransferCompleted { entry: FlowEntry },
    /// A transfer was cancelled
    TransferCancelled { transfer_id: TransferId },
}
```

**Usage examples:**

```rust
// Simple copy/paste between agents
ctx.flow().push(
    TypedContent::plain_text("Hello from Agent A"),
    FlowOptions {
        intent: TransferIntent::Copy,
        target: FlowTarget::Any,
        ephemeral: false,
        expires_in: None,
    },
).await?;

// Pull from Flow (blocking, waits up to 5 seconds)
let entry = ctx.flow().pull(FlowFilter {
    content_type: Some("text/*".into()),
    timeout: Some(Duration::from_secs(5)),
    ..Default::default()
}).await?;

// Quote with attribution
ctx.flow().push(
    TypedContent::rich_text(selected_html, source_url),
    FlowOptions {
        intent: TransferIntent::Quote,
        target: FlowTarget::Agent(research_agent_id),
        ephemeral: false,
        expires_in: None,
    },
).await?;

// Browse history: last hour, code only
let history = ctx.flow().history(FlowQuery {
    since: Some(Timestamp::now() - Duration::from_secs(3600)),
    content_type: Some("text/x-*".into()),
    semantic_type: Some(SemanticType::Code { language: "rust".into() }),
    limit: 50,
    offset: 0,
    ..Default::default()
}).await?;

// Semantic search
let results = ctx.flow().search(
    "that architecture diagram from the meeting",
    10,
).await?;

// Ephemeral transfer (password)
ctx.flow().push(
    TypedContent::plain_text(password),
    FlowOptions {
        intent: TransferIntent::Copy,
        target: FlowTarget::Agent(browser_tab_id),
        ephemeral: true,
        expires_in: Some(Duration::from_secs(30)),
    },
).await?;

// Subscribe to incoming transfers
let subscription = ctx.flow().subscribe(FlowFilter {
    content_type: Some("application/pdf".into()),
    ..Default::default()
}).await?;

// In the agent's event loop:
loop {
    let event = subscription.recv().await?;
    match event {
        FlowEvent::ContentAvailable { transfer_id } => {
            let entry = ctx.flow().pull(FlowFilter::default()).await?;
            // Process the PDF
        }
        _ => {}
    }
}
```

### 12.2 Python API

The Python SDK wraps the same IPC protocol:

```python
import aios

@aios.agent(
    name="Research Assistant",
    capabilities=["FlowRead", "FlowWrite", "ReadSpace('research')"],
)
async def research_assistant(ctx: aios.AgentContext):
    # Push content to Flow
    await ctx.flow.push(
        content=aios.TypedContent.plain_text("Key finding: ..."),
        intent=aios.TransferIntent.QUOTE,
        target=aios.FlowTarget.ANY,
    )

    # Pull from Flow (blocking)
    entry = await ctx.flow.pull(
        content_type="text/*",
        timeout=5.0,
    )
    print(f"Received: {entry.content.as_text()}")

    # Browse history
    history = await ctx.flow.history(
        since=aios.Timestamp.hours_ago(1),
        content_type="application/pdf",
        limit=20,
    )
    for entry in history:
        print(f"{entry.source_agent}: {entry.content.metadata.title}")

    # Semantic search
    results = await ctx.flow.search("transformer paper from arxiv")
    for entry in results:
        print(f"{entry.content.metadata.title} ({entry.initiated_at})")

    # Streaming: subscribe to incoming content
    async for event in ctx.flow.subscribe(content_type="image/*"):
        if isinstance(event, aios.FlowEvent.ContentAvailable):
            entry = await ctx.flow.pull()
            await process_image(entry.content)
```

### 12.3 TypeScript API

The TypeScript SDK provides the same interface for agents written in TypeScript and for PWAs using the `aios.flow()` web API:

```typescript
import { agent, AgentContext, TransferIntent, FlowTarget } from '@aios/sdk';

export default agent({
  name: 'Research Assistant',
  capabilities: ['FlowRead', 'FlowWrite'],
}, async (ctx: AgentContext) => {

  // Push content to Flow
  await ctx.flow.push(
    TypedContent.plainText('Key finding: ...'),
    {
      intent: TransferIntent.Quote,
      target: FlowTarget.Any,
    }
  );

  // Pull from Flow
  const entry = await ctx.flow.pull({
    contentType: 'text/*',
    timeout: 5000,
  });
  console.log(`Received: ${entry.content.asText()}`);

  // Browse history
  const history = await ctx.flow.history({
    since: Timestamp.hoursAgo(1),
    contentType: 'application/pdf',
    limit: 20,
  });

  // Semantic search
  const results = await ctx.flow.search('transformer paper from arxiv');

  // Subscribe to incoming transfers
  for await (const event of ctx.flow.subscribe({ contentType: 'image/*' })) {
    if (event.type === 'content_available') {
      const entry = await ctx.flow.pull();
      await processImage(entry.content);
    }
  }
});
```

**PWA Web API (browser-specific):**

```javascript
// In a PWA running in a Tab Agent — AIOS-specific Web API extension
// Only available on AIOS, feature-detect with: if (window.aios?.flow)

// Push selected text to Flow
const selection = window.getSelection().toString();
await aios.flow.push({
  content: selection,
  mimeType: 'text/plain',
  intent: 'quote',
  metadata: {
    title: document.title,
    sourceDescription: `Selected from ${window.location.href}`,
  },
});

// Pull from Flow
const entry = await aios.flow.pull({ contentType: 'text/*' });
document.getElementById('input').value = entry.content;
```

-----

## 13. Implementation Order

```
Phase 3:   Basic Flow service
             Flow Service process, sys.flow IPC channel
             FlowWrite/FlowRead capability enforcement
             Simple push/pull (TypedContent with MIME type)
             In-memory transfer staging (no persistence)
             Agents can copy/paste text between each other

Phase 6:   Compositor integration
             Drag/drop protocol (DragFlowRequest/DragFlowResponse)
             Visual feedback (compatible/incompatible/needs-transform indicators)
             Drag preview generation
             Drop target type negotiation (real-time, as cursor moves)

Phase 8:   AIRS transforms
             Transform Engine scaffold
             AIRS-powered transforms: summarize, transcribe, translate, embed
             TransformRegistry with system + AIRS providers
             Conversion graph and shortest-path selection

Phase 10:  Full transform engine, history, provenance
             System transforms (text conversion, image resize, format conversion)
             TransformRegistry with agent-contributed transforms
             Flow History Store (system/flow/ space, content-addressed)
             Flow History UI (keyboard shortcut, search, re-send)
             Provenance chain (append-only, linked to space provenance)
             Retention policy and content pruning
             Semantic search over history (via AIRS)
             FlowEntry as space object with full metadata

Phase 15:  POSIX clipboard bridge
             pbcopy/pbpaste equivalents
             X11 selection protocol translation
             Wayland clipboard protocol translation
             BSD tools see a standard clipboard, Flow sees typed transfers

Phase 21:  Browser Flow integration
             aios.flow() Web API for PWAs
             Browser tab ↔ native agent transfers
             Cross-tab Flow (tab agents share clipboard through Flow)

Phase 26:  Multi-device sync
             Cross-device transfer via AIOS Peer Protocol
             History merge (CRDT-style, grow-only set)
             Conflict resolution (latest-write-wins for active transfers)
             Encrypted transit (identity keys)
             Large content on-demand sync
             Content screening for cross-device transfers
```

-----

## 14. Design Principles

1. **Typed, not raw.** Content always carries its type. No more "text/plain and pray." Agents declare what they produce and what they accept. Flow bridges the gap.
2. **History is free.** Every transfer is recorded. Storage is content-addressed, so duplicates cost nothing. Users should never lose something they copied.
3. **Provenance is mandatory.** Every transfer records where the data came from. This is not optional. Provenance is how the user (and the Inspector) understands data lineage.
4. **Transform, don't reject.** When the receiver cannot handle the source format, Flow converts. The user should never see "incompatible format" — they should see their data, perhaps in a different representation.
5. **Intent matters.** Copy, move, reference, quote, derive — these are fundamentally different operations. The clipboard treats them all as "copy." Flow distinguishes them because the distinction affects provenance, storage, and user expectations.
6. **Ephemeral when needed.** Passwords, tokens, and sensitive credentials can flow between agents without persisting in history. The transfer happens; the content vanishes.
7. **POSIX is a view.** The clipboard bridge makes BSD tools work. But the clipboard is a translation layer over Flow, not the other way around. Flow is the truth; the clipboard is a compatibility shim.
