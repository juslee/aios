# AIOS Flow System

## Deep Technical Architecture

**Parent document:** [architecture.md](../project/architecture.md)
**Related:** [compositor.md](../platform/compositor.md) — Drag/drop integration, [subsystem-framework.md](../platform/subsystem-framework.md) — DataChannel/Flow pipes, [agents.md](../applications/agents.md) — SDK FlowClient, [spaces.md](./spaces.md) — History storage, [experience.md](../experience/experience.md) — Flow Tray UI

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
┌────────────────────────────────────────────────────────────┐
│                       Flow Service                         │
│               (system service, always running)             │
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
└────────────────────────────┬───────────────────────────────┘
                             │ IPC (sys.flow channel)
               ┌─────────────┼─────────────────┐
               ▼             ▼                 ▼
            Agents       Compositor       Subsystems
            (SDK          (drag/drop,      (DataChannels,
            FlowClient)   visual cues)     FlowPipes)
```

The Flow Service runs as a system service registered at `sys.flow`. The core service lands in dev Phase 11 (Tasks, Flow & Attention), with compositor drag/drop protocol scaffolded in Phase 6 and AIRS transform scaffolding in Phase 8 (see §13 for full implementation order). At runtime, it starts during boot Phase 4 (user services), after Space Storage (boot Phase 1) and IPC (boot Phase 2) are available. AIRS-powered transforms become available when AIRS completes boot Phase 3 initialization. Agents connect via IPC channels with `FlowRead` and/or `FlowWrite` capabilities.

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

### 3.0 External Types

Flow uses types defined in other documents. Canonical definitions:

| Type | Defined In | Description |
|---|---|---|
| `AgentId` | [spaces.md §3.0](./spaces.md) | Agent identity (Ed25519 public key, 32 bytes) |
| `ObjectId` | [spaces.md §3.0](./spaces.md) | Object identifier (UUID v4, 16 bytes) |
| `SpaceId` | [spaces.md §3.0](./spaces.md) | Space identifier (UUID v4, 16 bytes) |
| `Hash` | [spaces.md §3.0](./spaces.md) | SHA-256 hash (32 bytes) |
| `Timestamp` | [spaces.md §3.0](./spaces.md) | Milliseconds since Unix epoch |
| `Signature` | [spaces.md §3.0](./spaces.md) | Ed25519 signature (64 bytes) |
| `ObjectRef` | [spaces.md §3.0](./spaces.md) | Reference to a space object (SpaceId + ObjectId + optional version Hash) |
| `SharedMemoryId` | [memory.md §7](../kernel/memory.md) | Kernel-issued handle for a shared memory region |
| `ChannelId` | [ipc.md §3.1](../kernel/ipc.md) | IPC channel identifier |
| `SurfaceId` | [compositor.md §3](../platform/compositor.md) | Compositor surface identifier |
| `DeviceId` | [subsystem-framework.md §4](../platform/subsystem-framework.md) | Device identifier within the subsystem framework |
| `IdentityId` | [spaces.md §3.0](./spaces.md) | Identity identifier (Ed25519 public key); shared across a user's devices |
| `TrustLevel` | [identity.md §5](../experience/identity.md) | Trust classification for identities (Trusted/Verified/Known/Unknown) |
| `Duration` | Rust `core::time::Duration` | Time span; used for expiration, retention, and streaming durations |

Types defined locally in this document: `FlowEntryId` (§3.1), `TransferId` (§3.1), `TransformId` (§3.1).

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

    /// The content that was transferred. None when content has been pruned
    /// by the retention policy (large content >10 MB under storage pressure,
    /// or ephemeral transfers after delivery). The FlowEntry metadata
    /// (type, agents, timestamps, provenance) is always retained even when
    /// content is pruned. See §5.3 for retention rules.
    content: Option<TypedContent>,

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

    /// Device that originated this entry (for multi-device sync).
    /// DeviceId is a per-device hardware identifier (see subsystem-framework.md §4).
    /// Each physical device has a unique DeviceId even when multiple devices share
    /// the same user IdentityId. This allows the sync protocol to distinguish
    /// entries per device and track per-device watermarks.
    origin_device: DeviceId,
}

pub struct FlowEntryId(u128);

/// Unique identifier for an active transfer (in-flight, not yet recorded).
/// Distinct from FlowEntryId which identifies completed/recorded transfers.
pub struct TransferId(u128);

/// Unique identifier for a registered content transform.
pub struct TransformId(u64);

/// Error type for Flow operations. Used by push(), pull(), transfer
/// lifecycle management, and POSIX clipboard bridge.
pub enum FlowError {
    /// Agent does not hold the required FlowRead or FlowWrite capability.
    PermissionDenied,
    /// Source content type cannot be converted to any type the receiver accepts.
    IncompatibleType,
    /// The referenced TransferId does not exist or has already completed.
    TransferNotFound,
    /// A content transform failed during execution.
    TransformFailed(String),
    /// The transfer expired before a receiver claimed it.
    Expired,
    /// The agent's capability was revoked mid-transfer.
    CapabilityRevoked,
    /// The agent has exceeded its rate limit (see §11.3).
    RateLimited,
    /// Underlying I/O or IPC error.
    IoError(String),
}

/// Links a Flow transfer to the provenance chain in the Version Store
/// (see spaces.md §5.1). When a transfer creates or modifies a space object,
/// a ProvenanceEntry is recorded in the object's Version node. This
/// ProvenanceLink stores the hash of that entry, connecting the Flow history
/// to the space's Merkle DAG. The `parent` field links to the previous
/// FlowEntry's provenance (if the content was derived from a prior transfer),
/// forming a separate Flow-level chain alongside the per-object Version chain.
pub struct ProvenanceLink {
    /// Hash of the ProvenanceEntry recorded in the space Version Store
    hash: Hash,
    /// Previous link in the Flow provenance chain (if this content was
    /// derived from another transfer — e.g., Quote or Derive intent)
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
    /// Transfer recorded in history (final state). Separate from Delivered
    /// because history persistence is async and may fail or be skipped for
    /// ephemeral transfers. See lifecycle step 7 for details.
    Recorded,
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

   Note: Delivered and Recorded are separate states because recording
   may fail (storage full, I/O error) or be skipped (ephemeral transfers
   with ephemeral_retention=0 go directly to content pruning after
   delivery). The separation also allows the Transfer Manager to release
   the receiver immediately at Delivered without blocking on the
   potentially slower history write. If recording fails, the transfer
   remains in Delivered state and is retried on the next history flush.
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

| Intent | Content copied? | Source affected? | Relation created? | Provenance link? | Ephemeral interaction |
|---|---|---|---|---|---|
| Copy | Yes (full copy) | No | No | Yes (FlowEntry recorded, source/dest tracked for history search — no Relation object in the space) | If ephemeral=true, content is set to None after delivery; FlowEntry metadata remains for audit |
| Move | Yes (transferred) | Archived (source object's state set to Archived) | No | Yes (full chain: source → transfer → destination) | ephemeral=true is disallowed for Move (Move implies persistence) |
| Reference | No (ObjectRef only) | No | References (Relation created in destination space) | Yes (lightweight — only ObjectRef is tracked) | If source object is deleted, ObjectRef becomes dangling; receiver gets `ObjectNotFound` on access |
| Quote | Yes (full copy) | No | DerivedFrom (Relation with attribution metadata) | Yes (includes source quote context) | Ephemeral=true allowed; DerivedFrom relation persists even after content is purged |
| Derive | Yes (transformed copy) | No | DerivedFrom (Relation linking to original) | Yes (includes transform chain) | Ephemeral=true allowed; behaves like ephemeral Copy but with the DerivedFrom relation |

**Key distinction — Copy vs Quote:** Copy creates no Relation between objects. Quote creates a `DerivedFrom` relation with attribution, linking the new object to its source. Use Copy for "I want this data." Use Quote for "I'm citing this data."

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
    /// Shared memory region containing the data.
    /// SharedMemoryId is a kernel-issued opaque handle (see memory.md §7)
    /// identifying a reference-counted shared memory region. The region is
    /// COW (copy-on-write): the source agent's content is not duplicated
    /// until the receiver modifies it. The region's lifetime is managed by
    /// the kernel — it is freed when all mapping agents unmap it AND the
    /// Flow Service releases its reference (after the transfer completes
    /// or the history entry is pruned). Maximum lifetime for unclaimed
    /// transfers is controlled by Transfer.expires_at (default: 5 minutes).
    data: SharedMemoryId,
    /// Size in bytes
    size: u64,
    /// MIME type of this specific payload
    mime_type: String,
}

impl ContentPayload {
    /// Create a ContentPayload from raw bytes (allocates a shared memory region).
    /// Used by the POSIX clipboard bridge (§10) and SDK convenience methods.
    pub fn from_bytes(data: &[u8], mime_type: &str) -> Self;

    /// Read the content as a byte slice (maps the shared memory region).
    pub fn as_bytes(&self) -> &[u8];
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

/// Directed graph where nodes are content types and edges are transforms.
/// Used by the Transform Engine to find cheapest conversion paths.
/// Recomputed when transforms are added or removed.
pub struct ConversionGraph {
    /// Content type nodes (MIME types)
    nodes: Vec<String>,
    /// Edges: (source_index, target_index, transform_id, cost)
    edges: Vec<(usize, usize, TransformId, u32)>,
}

pub struct TransformRegistry {
    /// All registered transforms, indexed by input type
    transforms: HashMap<String, Vec<Transform>>,  // mime pattern → transforms

    /// Precomputed shortest-path conversion graph
    /// Updated when transforms are registered/unregistered
    conversion_graph: ConversionGraph,
}

/// Transform selection tiebreaker rules (applied in order):
///
/// When multiple transforms match the same input → output type conversion:
/// 1. **Lowest cost wins.** Compare TransformCost.time_ms first, then memory.
/// 2. **Provider priority.** If costs are equal: System > Airs > Agent.
///    System transforms are always preferred because they are deterministic,
///    always available, and have no inference overhead.
/// 3. **Agent tiebreaker.** If two Agent providers have equal cost:
///    the agent with the longer runtime (more established) wins.
///    This is a stable sort — the first registered agent wins if both
///    started at the same time. No randomness.
/// 4. **Lossless preferred.** If costs and providers are equal, prefer
///    the non-lossy transform.

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
 Embedding     Markdown        Code ──→ FormattedHTML
    ↑              │              (syntax highlight)
    │              ▼
    │         FormattedTable
    │
 Audio ──→ Transcript (AIRS)
```

If a receiver needs Markdown but the source provides HTML, the engine walks: HTML → RichText → Markdown (two system transforms, no AIRS needed, fast).

If a receiver needs an embedding but the source provides Audio, the engine walks: Audio → Transcript (AIRS) → PlainText → Embedding (AIRS). The engine checks AIRS availability before committing to this path.

Code → FormattedHTML uses syntax highlighting (a system transform, no AIRS needed). This is distinct from the HTML node, which represents generic HTML content — FormattedHTML is specifically syntax-highlighted markup.

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

The Flow Tray (see [experience.md](../experience/experience.md)) provides the user-facing history interface:

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
2. **Large content (>10 MB):** The FlowEntry metadata and a thumbnail are kept. The full content block may be pruned if storage pressure exists (`content` set to `None`). The entry shows "[Content pruned — original in source space]" with a link to the source object if it still exists.
3. **Ephemeral transfers:** Content marked `ephemeral: true` is purged from history immediately after delivery. The FlowEntry metadata record remains (for audit trail) but the `content` field is set to `None` and the shared memory region is zeroed and freed. Use case: password manager copying credentials into a form.
4. **User override:** Users can pin specific entries (never pruned), delete entries manually, or adjust the retention policy through preferences.
5. **Pruning order:** When the limit is reached, oldest unpinned entries are pruned first. Entries with `content: None` (already pruned) are fully removed before entries with live content.

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

Every subsystem's DataChannel (see [subsystem-framework.md](../platform/subsystem-framework.md)) has a `connect_flow()` method. This is the bridge between hardware data streams and the Flow system:

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
let speech_pipe = flow.create_pipe(FlowPipeDirection::Source, FlowTarget::Agent(speech_agent))?;
mic_session.channel().connect_flow(speech_pipe)?;
// Audio samples flow from hardware → Flow Service → speech agent
// Zero-copy if both are on the same device (shared memory)

// Camera → Flow → Image analysis agent
let cam_session = camera.open_session(agent, cam_cap, &intent)?;
let analysis_pipe = flow.create_pipe(FlowPipeDirection::Source, FlowTarget::Agent(analysis_agent))?;
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
// From architecture.md §3.2
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

**Transport:** Cross-device Flow uses the AIOS Peer Protocol (defined in [networking.md](../platform/networking.md)). Content is encrypted in transit using the shared identity's keys. The Peer Protocol handles device discovery, connection establishment, and reliable delivery.

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

**History merge:** History entries from all devices are merged into a unified timeline. Each entry carries its `origin_device` field (a DeviceId). Entries are uniquely identified by `(FlowEntryId, origin_device)`, so there are no conflicts — just union. This is equivalent to a grow-only CRDT (each device's history is append-only, merge is union).

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
            primary: ContentPayload::from_bytes(data, mime_type),
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

        let content = entry.content.ok_or(FlowError::TransferNotFound)?;
        Ok(content.primary.as_bytes().to_vec())
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

**X11 selection protocol:** The X11 compatibility layer (Phase 15) translates X11's three selections (PRIMARY, SECONDARY, CLIPBOARD) into Flow operations. PRIMARY (select-to-copy) maps to `flow.push(intent: Copy)`. CLIPBOARD (explicit copy) maps to the same. SECONDARY is rarely used and maps to a targeted transfer.

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

### 11.3 Rate Limiting and Abuse Prevention

A misbehaving or compromised agent could flood the Flow Service with transfers, consuming shared memory and filling history storage. Flow enforces per-agent rate limits:

```rust
pub struct FlowRatePolicy {
    /// Maximum transfers an agent can initiate per minute
    max_transfers_per_minute: u32,          // default: 120

    /// Maximum total bytes an agent can stage concurrently
    /// (across all in-flight transfers, before delivery)
    max_staged_bytes: u64,                  // default: 256 MB

    /// Maximum number of concurrent in-flight transfers per agent
    max_concurrent_transfers: u32,          // default: 32

    /// Maximum history entries an agent can create per hour
    /// (prevents history spam from rapid automated copy/paste)
    max_history_entries_per_hour: u32,      // default: 500

    /// Cooldown period after hitting a rate limit
    /// (agent must wait before retrying)
    cooldown: Duration,                     // default: 5 seconds
}

pub enum RateLimitAction {
    /// Transfer rejected, agent receives FlowError::RateLimited
    Reject,
    /// Transfer queued and delivered when budget allows
    Queue,
}
```

**Enforcement rules:**

| Limit | Action when exceeded | System agents exempt? |
|---|---|---|
| Transfers per minute | Reject (FlowError::RateLimited) | Yes |
| Staged bytes | Reject until existing transfers complete | Yes |
| Concurrent transfers | Queue (deliver in order when slots free) | Yes |
| History entries per hour | Entries still created but marked low-priority for pruning | Yes |

**Escalation:** If an agent repeatedly hits rate limits (>10 rejections in 5 minutes), the Flow Service emits a `FlowAbuse` event to the Inspector and the Attention Panel. The user sees: "[Agent X] is making unusually frequent clipboard operations." The user can revoke the agent's FlowWrite capability from the Attention Panel.

System agents (compositor, POSIX bridge) are exempt from rate limits because they are trusted and mediate on behalf of the user's direct actions.

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
    /// The handler implements the transform logic (see TransformHandler trait below).
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

/// Trait implemented by agents that provide content transforms.
/// Registered via FlowClient::register_transform().
/// Async because transforms may call AIRS (speech-to-text, summarization)
/// or perform I/O (network fetch, file conversion).
#[async_trait]
pub trait TransformHandler: Send + Sync {
    /// Transform input content into the declared output type.
    async fn transform(&self, input: ContentPayload) -> Result<ContentPayload, FlowError>;
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

impl FlowSubscription {
    /// Wait for the next Flow event matching the subscription filter.
    /// Blocks until an event is available.
    pub async fn recv(&self) -> Result<FlowEvent, FlowError>;
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

Phases reference the canonical project-wide phase numbers from [development-plan.md](../project/development-plan.md). The dependency chain for Flow is: Phase 6 (compositor scaffolds drag/drop protocol) → Phase 8 (AIRS scaffolds transform engine) → Phase 11 (Flow service lands, connects to compositor and AIRS) → later phases extend.

```
Phase 6:   Compositor drag/drop protocol scaffold
             DragFlowRequest/DragFlowResponse message types defined
             Drag preview generation and visual feedback framework
             Drop target type query API (stubbed — real Flow negotiation
             connects in Phase 11 when the Flow Service exists)

Phase 8:   AIRS transform scaffold
             Transform Engine data structures and ConversionGraph
             AIRS-powered transforms registered: summarize, transcribe,
             translate, embed
             TransformRegistry with system + AIRS providers
             Conversion graph shortest-path selection algorithm

Phase 11:  Flow Service (core phase — most Flow work lands here)
             Flow Service process, sys.flow IPC channel, boot Phase 4
             FlowWrite/FlowRead capability enforcement
             Push/pull with full TypedContent and type negotiation
             Transfer lifecycle (Initiated → Staged → Delivered → Recorded)
             In-memory transfer staging with COW shared memory
             Connect to compositor: live drag/drop type negotiation,
               visual feedback (compatible/incompatible/needs-transform)
             Connect to AIRS: transform execution during type negotiation
             System transforms (text conversion, image resize, format conversion)
             Agent-contributed transforms via TransformHandler trait
             Flow History Store (system/flow/ space, content-addressed)
             Flow History UI (Ctrl+Shift+V, search, re-send)
             Provenance chain (append-only, linked to space provenance)
             Retention policy and content pruning
             Semantic search over history (via AIRS)
             FlowEntry as space object with full metadata
             Content screening for sensitive data (§11.2)
             Rate limiting and per-agent transfer quotas (§11.3)

Phase 15:  POSIX clipboard bridge
             pbcopy/pbpaste equivalents
             X11 selection protocol translation
             Wayland clipboard protocol translation
             POSIX tools see a standard clipboard, Flow sees typed transfers

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

-----

## 15. Near-Term Extensions

These are concrete extensions that fit within the current architecture and should be considered for Phase 11 or early follow-on work.

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

Flow history content should be encrypted at rest using the user's identity key. Currently §9.1 covers encryption in transit for multi-device sync and §11 covers capability enforcement, but content sitting in `system/flow/history/` is stored as content-addressed blocks without encryption.

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

-----

## 16. Future Directions

These are more ambitious extensions that would build on top of the Phase 11-26 implementation. They are not committed to any phase but are worth designing toward.

### 16.1 Standing Flow Rules (Pipelines)

Let users define persistent rules that automatically process content as it flows through the system:

```
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
