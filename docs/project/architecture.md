# AIOS: AI-First Operating System

## System Architecture Document

**Related documents:**

- [development-plan.md](./development-plan.md) — Phase plan, timeline, risks
- [ipc.md](../kernel/ipc.md) — IPC and syscall interface deep dive
- [scheduler.md](../kernel/scheduler.md) — Scheduler deep dive
- [memory.md](../kernel/memory.md) — Memory management architecture
- [boot.md](../kernel/boot.md) — Boot sequence and init system deep dive
- [boot-lifecycle.md](../kernel/boot-lifecycle.md) — Boot lifecycle, advanced topics, and design principles
- [hal.md](../kernel/hal.md) — Hardware Abstraction Layer (Platform trait, device drivers, porting guide)
- [spaces.md](../storage/spaces.md) — Space storage system deep dive
- [compositor.md](../platform/compositor.md) — GPU compositor and window management
- [networking.md](../platform/networking.md) — Network Translation Module deep dive
- [subsystem-framework.md](../platform/subsystem-framework.md) — Universal hardware abstraction architecture
- [airs.md](../intelligence/airs.md) — AI Runtime Service deep dive
- [agents.md](../applications/agents.md) — Agent framework and SDK specification
- [security.md](../security/security.md) — Eight-layer security model deep dive
- [flow.md](../storage/flow.md) — Flow system deep dive
- [context-engine.md](../intelligence/context-engine.md) — Context Engine deep dive
- [posix.md](../platform/posix.md) — POSIX compatibility layer deep dive
- [experience.md](../experience/experience.md) — Experience layer and UI design
- [browser.md](../applications/browser.md) — Decomposed web content runtime
- [ui-toolkit.md](../applications/ui-toolkit.md) — Portable UI toolkit specification
- [attention.md](../intelligence/attention.md) — Attention Manager and notification triage
- [preferences.md](../intelligence/preferences.md) — Preference Service
- [identity.md](../experience/identity.md) — Identity and trust model
- [task-manager.md](../intelligence/task-manager.md) — Task decomposition and orchestration
- [audio.md](../platform/audio.md) — Audio subsystem architecture
- [accessibility.md](../experience/accessibility.md) — Accessibility engine
- [power-management.md](../platform/power-management.md) — Power management policy engine

-----

## Terminology Glossary

AIOS reuses common OS terms but gives them specific meanings. This glossary defines canonical types for each meaning. **In code and API documentation, always use the specific type name, never the bare term.**

| Term | Context | Canonical Type | Definition |
|---|---|---|---|
| **Agent** | Installation | `AgentManifest` | Signed package declaring name, author, requested capabilities, code hash, dependencies, and AI security analysis |
| **Agent** | Runtime | `AgentProcess` | A running process created from a manifest, with a PID, capability table, resource limits, and behavioral baseline |
| **Task** | User-facing | `Task` | A user's goal decomposed into subtasks, with agents assigned, capabilities scoped, and activity logged |
| **Task** | Kernel | `Thread` | A schedulable unit of work assigned to a scheduling class (RT, Interactive, Normal, Idle) |
| **Session** | Hardware | `SubsystemSession` | A bounded interaction with a hardware subsystem (audio output session, camera capture session) |
| **Session** | AIRS | `InferenceSession` | A single inference request with its own KV cache, priority, token callback, and stop sequences |
| **Service** | System | Trust Level 1 process | A userspace daemon (AIRS, Space Storage, Compositor, NTM, Service Manager) with elevated capabilities |
| **Service** | IPC | `ChannelId` + protocol | A capability-gated IPC channel with a registered protocol that clients call via `IpcCall` |
| **Space** | Storage | `Space` | A named collection of typed objects with a security zone, encryption state, quota, and parent hierarchy |
| **Space** | Security | `SecurityZone` | The zone classification of a space: Core, Personal, Collaborative, Untrusted, or Ephemeral |

**Usage rules:**
- Write `AgentManifest`, not "agent," when referring to the installable package
- Write `InferenceSession`, not "session," when referring to an AIRS inference context
- Write `SubsystemSession`, not "session," when referring to a hardware interaction
- The bare term is acceptable in user-facing UI and conversation, never in code or API docs

-----

## 1. Vision

A clean-sheet microkernel operating system written in Rust for aarch64 where every subsystem is designed assuming AI exists. AI is the infrastructure — invisible when not needed, available when invoked, and the reason everything works better than on any other OS.

### Design Principles

1. **AI is infrastructure, not interface.** The user never has to interact with AI to use the computer. AI enhances silently — performance, organization, security, search. When the user wants AI help, the conversation bar is always one gesture away.
1. **No legacy tax.** Every abstraction is designed for 2026, not inherited from 1970. Spaces instead of files. Tasks instead of processes. Flow instead of clipboard. Capabilities instead of permissions.
1. **The computer is one continuous experience.** Work, leisure, communication, creation — these aren't separate app silos. They're activities that share context through spaces, connected by relationships the AI maintains.
1. **Security through depth, not walls.** Eight layers of security, each designed for a world where autonomous agents act on your behalf. No single layer failing compromises the system.
1. **Developers build capabilities, not apps.** The SDK provides context, persistence, inference, security, and tool interop as system services. Developers write the interesting part.
1. **Portable where it matters.** The UI toolkit and developer tools run on Linux, macOS, and AIOS. Developers build on familiar platforms, deploy to AIOS. The OS earns adoption, it doesn't demand it.
1. **BSD-licensed ecosystem.** FreeBSD userland, musl libc, permissive licensing throughout. No GPL copyleft constraints on the OS or its users.
1. **One framework, every subsystem.** All hardware — networking, audio, USB, display, cameras, Bluetooth, printers — implements the same traits: capability gate, sessions, data channels, audit, power management, POSIX bridge. Adding new hardware is formulaic, not architectural.

-----

## 2. System Architecture

### 2.1 Full Stack Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                        EXPERIENCE LAYER                             │
│                                                                     │
│  Workspace        Conversation    Media Player    Web Browser       │
│  (contextual      Bar (always     (music, video,  (Servo-based,     │
│   home view)      available,      podcasts,       semantic           │
│                   user-invoked)   streaming)      indexing)          │
│                                                                     │
│  Game Launcher    Inspector       Agent Store     Settings           │
│  (library,        (provenance,    (discover,      (conversational,   │
│   saves as        security        approve         AI-mediated)       │
│   space objects)  visibility)     capabilities)                      │
├─────────────────────────────────────────────────────────────────────┤
│                        SERVICES LAYER                               │
│                                                                     │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │ AI Runtime Service (AIRS) — hot-swappable privileged service │   │
│  │                                                             │   │
│  │  Inference Engine    Model Registry    Agent Lifecycle      │   │
│  │  (GGML, NEON SIMD)  (GGUF, LRU)      (create, sandbox)    │   │
│  │                                                             │   │
│  │  Context Manager     Tool Manager     Space Indexer         │   │
│  │  (state, compress)   (register, exec) (embed, relate)      │   │
│  │                                                             │   │
│  │  Context Engine      Attention Mgr    Intent Verifier       │   │
│  │  (infer work/play)   (triage, digest) (action alignment)    │   │
│  │                                                             │   │
│  │  Behavioral Monitor  Adversarial Def  Inference Scheduler   │   │
│  │  (anomaly detect)    (injection det)  (priority, deadline)  │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                                                                     │
│  Space Storage     Task Manager    Flow Service    Identity Svc     │
│  (object store,    (intent →       (context-aware  (crypto keys,    │
│   block engine,    subtasks,       data transfer,  relationships,   │
│   content-addr)    orchestrate)    transform)      trust model)     │
│                                                                     │
│  Network           Preference Svc  Compositor      Audio Service    │
│  Translation       (conversational (GPU-native,    (mixing, route,  │
│  Module            config, learn)  semantic-ready) decode, output)  │
│  (spaces → net)                                                     │
│                                                                     │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │ Subsystem Framework — universal hardware abstraction         │   │
│  │                                                             │   │
│  │  Capability Gate    Sessions       Data Channels            │   │
│  │  (kernel-enforced)  (bounded use)  (Flow-connected)         │   │
│  │                                                             │   │
│  │  Device Registry    Audit Spaces   Power Manager            │   │
│  │  (system/devices/)  (system/audit/) (idle policies)         │   │
│  │                                                             │   │
│  │  POSIX Bridge       Conflict Res   Hotplug Handler          │   │
│  │  (/dev nodes)       (share/queue)  (USB, BT, etc.)         │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                                                                     │
│  POSIX Compat      Agent Runtime   Connector Svc   Device Drivers   │
│  (BSD userland,    (sandbox, SDK   (Slack, GitHub,  (VirtIO, USB,   │
│   musl libc,       runtime, tool   external APIs)   WiFi, BT)      │
│   translation)     execution)                                       │
├─────────────────────────────────────────────────────────────────────┤
│                         KERNEL SPACE                                │
│                                                                     │
│  AI Kernel Primitives                                               │
│  ├── Model memory regions (shared, pinned, ref-counted)             │
│  ├── Compute device abstraction (CPU/GPU/NPU)                       │
│  ├── Agent capability tokens (fine-grained, revocable, expiring)    │
│  ├── Provenance chain (append-only, Merkle-linked, signed)          │
│  └── Inference scheduling primitives (priority, deadline, preempt)  │
│                                                                     │
│  Core Microkernel                                                   │
│  ├── Virtual Memory Manager (4-level, TTBR0/TTBR1, W^X, KASLR)    │
│  ├── IPC (sync message passing, capability transfer, zero-copy)     │
│  ├── Scheduler (priority + deadline, context-aware hints)           │
│  ├── Capability Manager (create, transfer, revoke, attenuate)       │
│  ├── Cryptographic Core (Ed25519, AES-256, key storage)             │
│  ├── Audit Log (kernel-enforced, tamper-evident)                    │
│  └── Process Manager (create, isolate, terminate)                   │
│                                                                     │
│  Hardware Abstraction Layer (hal.md)                                │
│  ├── Platform trait (7 init methods, one per hardware class)        │
│  ├── InterruptController (GICv2 on Pi 4, GICv3 on Pi 5/QEMU)      │
│  ├── Timer (ARM Generic Timer, platform-specific frequency)         │
│  ├── Uart (PL011 UART, platform-specific base address)             │
│  ├── GpuDevice (VirtIO-GPU / VideoCore VI / VideoCore VII)         │
│  ├── NetworkDevice (VirtIO-Net / Broadcom Genet)                   │
│  ├── StorageDevice (VirtIO-Blk / Arasan SDHCI)                    │
│  ├── RngDevice (VirtIO-RNG / bcm2835-rng)                         │
│  ├── UEFI Runtime Services                                          │
│  └── Device Tree Parsing + Platform Detection                       │
├─────────────────────────────────────────────────────────────────────┤
│                          HARDWARE                                   │
│  CPU (aarch64)  │  RAM  │  GPU  │  NPU  │  Storage  │  Network     │
└─────────────────────────────────────────────────────────────────────┘
```

### 2.2 Space Storage System

Replaces the traditional filesystem. Objects instead of files. Semantic relationships instead of directory trees. Content-addressed storage with AI-maintained indexes.

```
┌─────────────────────────────────────────────┐
│         Space API (what apps/agents see)     │
│  query()  create()  relate()  version()     │
│  similar_to()  traverse()  search()         │
├─────────────────────────────────────────────┤
│         Semantic Index (maintained by AIRS)  │
│  Embedding store │ Entity index │ Tag index  │
│  Relationship graph │ Temporal index        │
├─────────────────────────────────────────────┤
│         Object Store                         │
│  Content-addressed (SHA-256 hash keys)      │
│  Typed objects with structured metadata     │
│  Automatic deduplication                    │
│  Integrity verification                     │
├─────────────────────────────────────────────┤
│         Version Store                        │
│  Merkle DAG (like git) for full history     │
│  Per-object and per-space versioning        │
│  Provenance chain per version               │
├─────────────────────────────────────────────┤
│         Block Engine                         │
│  LSM-tree indexed blocks on raw device      │
│  No intermediate filesystem layer           │
│  Write-ahead log for crash recovery         │
│  Encryption at rest (per-space keys)        │
└─────────────────────────────────────────────┘
```

**Core data model:**

```rust
pub struct Space {
    id: SpaceId,
    name: String,
    parent: Option<SpaceId>,
    security_zone: SecurityZone,
    encryption: EncryptionState,
    quota: SpaceQuota,
    created_at: Timestamp,
    modified_at: Timestamp,
    object_count: u64,
    total_size: u64,
}

pub struct Object {
    id: ObjectId,
    /// Human-readable name (last path component)
    name: String,
    content_hash: Hash,
    content_type: ContentType,
    content_size: u64,
    semantic: SemanticMetadata,
    relations: Vec<Relation>,
    created_at: Timestamp,
    modified_at: Timestamp,
    created_by: AgentId,
    modified_by: AgentId,
    provenance: ProvenanceChain,
}

pub struct SemanticMetadata {
    summary: Option<String>,
    tags: Vec<String>,
    auto_tags: Vec<String>,
    embedding: Option<Vec<f32>>,
    entities: Vec<Entity>,
    description: Option<String>,
    auto_summary: Option<String>,
    text_content: Option<String>,
    indexed_at: Option<Timestamp>,
}

/// Simplified content type enum; see spaces.md §3.3 for the full canonical definition
/// with additional variants (Directory, Text, Markdown, Json, Xml, Credential, etc.).
pub enum ContentType {
    Document, Code, Image, Audio, Video, Data,
    Conversation, Config, Agent, GameSave,
    WebPage, MediaReference, Task, Note,
    CacheEntry, SessionToken, Cookie,
}

pub struct Relation {
    source: ObjectId,
    target: ObjectId,
    kind: RelationKind,
    confidence: f32,
    explanation: Option<String>,
    created_by: RelationSource,
}

pub enum RelationKind {
    DerivedFrom, References, DependsOn,
    RelatedTo, CreatedBy, InputTo,
    OutputOf, ConversationContext,
    VersionOf, SiblingOf,
    ChildOf, Attachment,
}
```

### 2.3 Task & Agent Model

Replaces the process model for user-facing work. Users think about goals, not programs. Processes still exist underneath for isolation.

```rust
pub struct Task {
    id: TaskId,
    intent: Intent,
    state: TaskState,
    agents: Vec<AgentId>,
    capabilities: CapabilitySet,
    activity_log: Vec<ActivityEntry>,
    children: Vec<TaskId>,
    persistence: Persistence,
    context: ContextLink,
}

/// Links a task to its surrounding context — the active space, identity,
/// and Context Engine snapshot at the time the task was created. Used by
/// agents to access contextual information without querying the Context
/// Engine repeatedly. The snapshot is a frozen ContextState (see §2.5 below).
pub struct ContextLink {
    /// The space the task was initiated from
    space_id: SpaceId,
    /// The identity that owns this task
    identity_id: IdentityId,
    /// Snapshot of context at task creation (focus state, recent activity)
    snapshot_id: ObjectId,
}

pub enum TaskState {
    Active,
    WaitingForUser(Question),
    WaitingForResource,
    Background,
    Suspended,
    Completed(Outcome),
    Failed(Error),
}

/// A user-facing question presented when a task needs input.
pub struct Question {
    text: String,
    options: Option<Vec<String>>,
    default: Option<String>,
}

/// Result of a successfully completed task.
pub struct Outcome {
    summary: String,
    artifacts: Vec<ObjectId>,
}

/// Structured intent describing what a task or agent is trying to accomplish.
/// Used by Intent Verification (Layer 1) to compare observed actions against
/// declared goals. Distinct from TransferIntent (§2.4) which governs Flow.
pub struct Intent {
    /// Human-readable goal description (e.g., "organize research papers")
    goal: String,
    /// Structured action categories this intent permits
    permitted_actions: Vec<ActionCategory>,
    /// Maximum scope (spaces, object count) the intent covers
    scope: IntentScope,
}

pub enum ActionCategory {
    Read, Write, Delete, Create, Search, Infer, Network, Spawn,
}

pub struct IntentScope {
    spaces: Vec<SpaceId>,
    max_objects: Option<u64>,
    max_network_requests: Option<u64>,
}

/// AI engagement level driven by Context Engine signals.
pub enum AiEngagement {
    /// Pure infrastructure — scheduling, security, indexing.
    /// User sees no AI activity.
    Invisible,
    /// Results visible, process hidden — search works, defaults adapt.
    Ambient,
    /// Conversation bar responsive, suggestions ready.
    Available,
}

/// Resource allocation priority driven by Context Engine.
pub enum ResourcePriority {
    /// Foreground task gets maximum resources
    Foreground,
    /// Background tasks get fair share
    Background,
    /// System is in low-power mode
    LowPower,
}

/// A named entity extracted from content by AIRS (person, place,
/// organization, date, concept, etc.).
pub struct Entity {
    name: String,
    kind: EntityKind,
    confidence: f32,
    /// Byte offset range in the source content
    span: Option<(usize, usize)>,
}

pub enum EntityKind {
    Person, Organization, Location, Date, Concept,
    Technology, Event, Product, Other(String),
}

/// Who created a relation between objects.
pub enum RelationSource {
    /// Created by AIRS during indexing
    Ai,
    /// Created explicitly by a user action
    User,
    /// Created by an agent during its operation
    Agent(AgentId),
}

pub enum Persistence {
    Ephemeral,   // gone when done
    Session,     // lives until closed
    Persistent,  // survives reboot
}

/// Simplified; see agents.md §2.4 for full definition.
pub struct AgentManifest {
    name: String,
    author: Identity,
    requested_capabilities: Vec<CapabilityRequest>,
    code: ContentHash,
    dependencies: Vec<Dependency>,
    ai_analysis: Option<SecurityAnalysis>,
}

/// Set of capabilities held by a task or agent process. Capabilities are
/// kernel-managed tokens — agents hold references, not the capabilities
/// themselves. The kernel validates every token on every syscall.
/// See ipc.md §4 for capability transfer and boot.md §3.3 Step 12 for
/// the root capability from which all others derive.
pub struct CapabilitySet {
    /// Active capability tokens, keyed by capability type for O(1) lookup
    tokens: HashMap<CapabilityType, Vec<CapabilityToken>>,
}

/// Simplified capability discriminant for HashMap keying in CapabilitySet.
/// Maps to the broader `Capability` enum (§3.2) which carries full parameters.
/// CapabilityType identifies the *kind* of access; Capability carries the
/// full token with specific parameters (e.g., AudioCapability details).
pub enum CapabilityType {
    ReadSpace(SpaceId),
    WriteSpace(SpaceId),
    FlowRead,
    FlowWrite,
    Network(NetworkScope),
    Spawn,
    DeviceAccess(DeviceClass),
    IpcConnect(ServiceName),
}

/// A single entry in a task's activity log. Records what an agent did,
/// when, and in what context. Used by Intent Verification (Layer 1) to
/// compare observed actions against the task's declared Intent.
pub struct ActivityEntry {
    timestamp: Timestamp,
    agent: AgentId,
    action: ActivityAction,
    /// Capability that authorized this action
    capability: CapabilityType,
    /// Time spent on this action (for CPU accounting)
    duration: Option<Duration>,
}

pub enum ActivityAction {
    SpaceRead { space: SpaceId, object: ObjectId },
    SpaceWrite { space: SpaceId, object: ObjectId },
    SpaceCreate { space: SpaceId, object: ObjectId },
    SpaceDelete { space: SpaceId, object: ObjectId },
    FlowTransfer { intent: TransferIntent },
    NetworkRequest { endpoint: String },
    InferenceRequest { model: ModelId },
    AgentSpawn { child: AgentId },
    IpcMessage { channel: ChannelId },
}

/// Append-only provenance chain for an object. Each entry links to the
/// previous via hash, forming a Merkle chain. Stored in the Version Store
/// (see spaces.md §5.1 for per-version ProvenanceEntry). The chain here
/// is the object-level summary — it aggregates provenance across all
/// versions for quick inspection without walking the full version DAG.
pub struct ProvenanceChain {
    /// Hash of the most recent provenance entry
    head: Hash,
    /// Total number of entries in the chain
    length: u64,
    /// Who originally created this object
    origin: ProvenanceOrigin,
}

pub enum ProvenanceOrigin {
    /// Created by a user action via an agent
    UserCreated { agent: AgentId },
    /// AI-generated content
    AiGenerated { model: ModelId },
    /// Imported from external source
    Imported { source: String },
    /// Derived from another object
    DerivedFrom { source: ObjectId },
}
```

### 2.4 Flow System

Replaces the clipboard. Context-aware data transfer with transformation and history.

```rust
pub struct Flow {
    history: Vec<FlowEntry>,
    active_transfer: Option<Transfer>,
}

pub struct Transfer {
    source: ObjectRef,      // see spaces.md §3.0 for ObjectRef definition
    content: TypedContent,  // see flow.md §3.4 for TypedContent definition
    intent: TransferIntent,
    transformations: Vec<Transform>,
}

/// A content transformation applied during Flow transfer.
/// Converts content from one type to another (e.g., rich text → plain text,
/// image → thumbnail, audio → transcript).
pub struct Transform {
    id: TransformId,
    name: String,
    input_types: Vec<String>,    // MIME patterns
    output_type: String,         // MIME type
    provider: TransformProvider,
}

pub enum TransformProvider {
    /// Built-in system transforms (e.g., text encoding conversion)
    System,
    /// AI-powered transforms via AIRS (e.g., audio → transcript)
    Airs,
    /// Agent-provided transforms
    Agent(AgentId),
}

pub enum TransferIntent {
    Copy, Move, Reference, Quote, Derive,
}
```

### 2.5 Context Engine

Replaces explicit Work/Play modes. Continuously infers user context from signals. No toggles required.

```rust
pub struct ContextEngine {
    signals: Vec<ContextSignal>,
    current: ContextState,
    overrides: Vec<Override>,
}

pub enum ContextSignal {
    ActiveSpace(SpaceId),
    RunningAgents(Vec<AgentId>),
    InputPattern(InputActivity),
    TimeOfDay(Time),
    CalendarState(CalendarContext),
    MediaPlayback(MediaState),
    UserHistory(Pattern),
    ExplicitIntent(Option<Intent>),
}

pub struct ContextState {
    work_engagement: f32,     // 0.0 = deep leisure, 1.0 = deep work
    ai_engagement: AiEngagement,
    notification_threshold: Urgency,
    resource_priority: ResourcePriority,
}

pub struct Override {
    intent: String,        // "heads down for 2 hours"
    effect: ContextState,
    expires: Timestamp,    // always time-bounded
}
```

**AI engagement tiers driven by context:**

|Tier     |AI Behavior                                                   |Triggered When                            |
|---------|--------------------------------------------------------------|------------------------------------------|
|Invisible|Pure infrastructure — scheduling, security, indexing          |Gaming, media playback, casual browsing   |
|Ambient  |Results visible, process hidden — search works, defaults adapt|Light work, mixed activity                |
|Available|Conversation bar responsive, suggestions ready                |Active work in spaces, explicit invocation|

### 2.6 Attention Management

Replaces notifications. AI-triaged, context-aware, never interruptive during leisure unless genuinely urgent.

```rust
pub struct AttentionManager {
    incoming: PriorityQueue<AttentionItem>,
    model: AttentionModel,   // see attention.md §4 for AttentionModel (urgency classification parameters)
    context: ContextState,
}

pub struct AttentionItem {
    source: AgentId,
    content: AttentionContent,  // see attention.md §3 for AttentionContent enum (distinct from flow.md §3.4 TypedContent struct)
    urgency: Urgency,       // AI-assessed, not app-declared
    relevance: f32,
    auto_actionable: Option<ProposedAction>,
    group: Option<GroupId>,  // opaque identifier for grouping related notifications
}

pub enum Urgency {
    Interrupt,   // show immediately (system error, critical person)
    NextBreak,   // show when user pauses
    Digest,      // batch into periodic summary
    Silent,      // log but never show
}
```

### 2.7 Identity & Relationships

Replaces user accounts. Cryptographic identity, graduated trust, relationship-aware sharing.

```rust
/// Ed25519 keypair for identity signing and verification.
/// See identity.md §4 for key management and derivation.
pub struct KeyPair {
    public: [u8; 32],   // Ed25519 public key
    private: [u8; 64],  // Ed25519 expanded private key (encrypted at rest)
}

pub struct Identity {
    id: IdentityId,
    keys: KeyPair,
    relationships: Vec<Relationship>,
    space_access: Vec<(SpaceId, AccessLevel)>,  // see identity.md §7 for AccessLevel
    trust: TrustModel,                          // see identity.md §6 for TrustModel
}

pub struct Relationship {
    with: IdentityId,
    kind: RelationshipKind,  // see identity.md §5 for RelationshipKind
    trust_level: TrustLevel, // see identity.md §5 for TrustLevel
    shared_spaces: Vec<SpaceId>,
}
```

### 2.8 Preference System

Replaces config files. Conversational configuration, AI-mediated, evolves through use.

```rust
pub struct Preference {
    id: PreferenceId,
    description: String,
    value: PreferenceValue,       // see preferences.md §3 for PreferenceValue variants
    source: PreferenceSource,
    affects: Vec<SystemComponent>,// see preferences.md §3 for SystemComponent enum
    history: Vec<PreferenceChange>,// see preferences.md §3 for PreferenceChange
}

pub enum PreferenceSource {
    UserExplicit,
    UserBehaviorInferred,
    SystemDefault,
    AgentSuggested(AgentId),
}
```

### 2.9 Network Translation Module

Replaces application-level networking. Applications see spaces; the OS handles all networking transparently. **Full design in [networking.md](../platform/networking.md).**

**Core principle:** Applications never see the network. There are only space operations — some of which happen to involve remote spaces — and the OS handles everything else.

```
Application:  space::read("openai/v1/models")
                    ↓
Network Translation Module:
  ├── Space Resolver      (semantic name → endpoint + protocol + auth)
  ├── Connection Manager  (pool, TLS, multiplex, keepalive)
  ├── Shadow Engine       (offline transparency, local cache, sync)
  ├── Resilience Engine   (retry, backoff, circuit breaker)
  ├── Bandwidth Scheduler (priority, multi-path, QoS, metered awareness)
  └── Capability Gate     (verify net capability before ANY operation)
                    ↓
Protocol Engines:  HTTP/2 │ HTTP/3/QUIC │ AIOS Peer │ MQTT │ Raw Socket
                    ↓
Transport:         TLS 1.3 (rustls) │ QUIC (quinn) │ Plain TCP/UDP
                    ↓
Network Stack:     smoltcp (TCP/UDP/ICMP/IPv4/IPv6/ARP/DHCP)
                    ↓
Interface Drivers: VirtIO-Net │ Ethernet │ WiFi │ Bluetooth │ Cellular
```

**Key innovations:**

- Mandatory kernel capability gate — agents cannot bypass network access control
- Layered optional services — TLS, HTTP, connection pooling are userspace services agents can use or bypass with appropriate capability labeling
- Six error types instead of hundreds (Unreachable, Unavailable, PermissionDenied, NotFound, Conflict, TooLarge)
- Offline transparency — applications never know whether they're online or offline
- Credential isolation — agents use credentials without possessing them
- Per-space capability enforcement — no default network access
- Trust labeling — agents using OS TLS get higher trust rating than self-managed TLS
- AIOS Peer Protocol for native device-to-device communication with capability exchange
- Implements the subsystem framework (see [subsystem-framework.md](../platform/subsystem-framework.md))

### 2.10 BSD Compatibility Layer

AIOS uses FreeBSD userland (BSD-licensed) instead of GNU tools (GPL), providing immediate productivity for every Unix developer.

**Compatibility architecture:**

```
BSD Tools (unmodified FreeBSD userland)
  ↓
libc (musl-based, MIT-licensed, ~100K lines vs glibc 1.5M)
  ↓
POSIX Emulation Layer (translates POSIX → AIOS syscalls)
  ↓
AIOS Kernel (capabilities, IPC, spaces)
```

**Why BSD, not GNU:** FreeBSD tools are BSD-licensed (permissively usable), self-contained, portable, proven in macOS/PlayStation/Switch, and assume less about the host system. GNU tools are GPL (copyleft creates legal complexity for OS distribution) and deeply tied to Linux/glibc.

**Why musl, not glibc:** MIT-licensed (vs GPL), designed for static linking and portability, much smaller codebase (~100K lines vs ~1.5M), already proven on Alpine Linux and non-Linux systems, not tied to Linux kernel specifics.

**Included tools:**

- Core utilities: ls, cp, mv, rm, mkdir, cat, grep, sed, awk, find, sort, diff, patch
- Development: make (BSD make), clang/lld (LLVM, BSD-licensed), ar, nm, strip
- Shell: FreeBSD /bin/sh (ash-based, POSIX-compliant, BSD-licensed) — NOT bash (GPL)
- Compression: tar, gzip, bzip2, xz
- Network: curl, OpenSSH
- Editor: nvi (BSD vi)

**POSIX-to-Spaces path mapping:**

```
/spaces/research/          → Objects in "research" space
/spaces/research/paper.md  → Object "paper" (type: document)
/home/user/                → Personal space (default)
/tmp/                      → Ephemeral space (auto-cleaned)
/dev/null, /dev/urandom    → Device capabilities
/proc/self/                → Process introspection (minimal, read-only)
/bin/, /usr/bin/           → System utilities (initramfs/system space)
```

**Translation mechanics:**

```
ls /spaces/research/   → space query, list objects, present as directory entries
grep "term" /spaces/*  → glob expansion to space objects, grep sees regular file content
open()                 → space object read capability check + content retrieval
stat()                 → object metadata query
fork()                 → process create with inherited capabilities
pipe()                 → IPC channel pair
mmap()                 → shared memory object
```

Tools never know they're not on a traditional filesystem. Self-hosting capability via clang/lld means AIOS can compile software for itself.

### 2.11 Portable UI Toolkit

The UI toolkit runs on Linux, macOS, and AIOS. Developers build on familiar platforms, deploy to AIOS.

**Why portability matters:**

1. **Developer adoption.** Build/test on familiar platform, no AIOS boot required for development
1. **Ecosystem bootstrapping.** Developers invest knowing work isn't trapped on a zero-user platform
1. **Proving abstractions.** Multi-platform proves design isn't accidentally kernel-coupled
1. **Fast iteration.** Development loop stays fast (edit on Mac, test in QEMU)

**Architecture:**

```
Application UI Code (identical across platforms)
  ↓
UI Toolkit - Portable Core
  ├── Widget library (button, label, input, list, scroll...)
  ├── Layout engine (flexbox-like)
  ├── Theme system (colors, fonts, spacing)
  ├── Event model (click, hover, focus, keyboard)
  ├── Render tree → display list
  └── Text layout (shaping, line breaking, bidi)
  ↓
Platform Backend (one per target)
  ├── AIOS: Compositor protocol + GPU direct
  ├── Linux: wgpu + winit (Wayland/X11)
  ├── macOS: wgpu + winit (Metal)
  └── Web: Canvas + DOM (WASM)
```

**Toolkit choice: iced (Elm-inspired, pure Rust)** — Already works on Linux/macOS/Windows/Web. MIT-licensed. GPU-rendered via wgpu. Architecture naturally separates platform from toolkit. Adding AIOS backend is a defined task, not research.

**AIOS backend unique capabilities (gracefully degrade on other platforms):**

- Semantic window hints → compositor understands content (ignored on Linux/macOS)
- Flow integration → context-aware drag/drop (falls back to standard D&D)
- Space-backed data → versioned, searchable (falls back to file I/O)
- Capability-aware UI → disable elements based on permissions (no restrictions elsewhere)

**Cross-platform example:**

```rust
use aios_toolkit::prelude::*;

fn view(state: &AppState) -> Element {
    column![
        text(&state.title).size(24),
        text_input("Search...", &state.query).on_input(Message::QueryChanged),
        button("Search").on_press(Message::Search),
        results_list(&state.results),
    ]
}

// Backend handles platform differences:
// AIOS: search via Space API → space objects
// Linux: search via SQLite → rows
// Web: search via REST → JSON
// UI code unchanged
```

### 2.12 Subsystem Framework

**Full design in [subsystem-framework.md](../platform/subsystem-framework.md).**

Every hardware subsystem in AIOS (networking, audio, USB, display, cameras, Bluetooth, printers, GPS, etc.) implements the same framework of traits and types. The framework handles everything generic — capability enforcement, session lifecycle, audit logging, power management, POSIX bridge, device registry, hotplug. Subsystem-specific code is the minimal amount needed for the domain.

**Core principle:** Define the framework once, instantiate it for each hardware class. Adding a new hardware class becomes formulaic, not architectural.

**Five-layer architecture (every subsystem):**

```
Agent API Layer        → What agents see: typed, semantic, capability-gated
POSIX Translation      → What BSD tools see: /dev nodes, ioctl, read/write
Subsystem Service      → Policy, multiplexing, routing, format negotiation
Device Abstraction     → Uniform trait per device class, regardless of hardware
Hardware Driver        → VirtIO, USB, PCI, platform-specific
    ↕ Capability Gate (kernel-enforced) + Audit Space (all access logged)
```

**Key abstractions:**

|Concept            |What It Is                                               |Why It Matters                                           |
|-------------------|---------------------------------------------------------|---------------------------------------------------------|
|`Subsystem` trait  |Registration, device lifecycle, session creation         |Every hardware class implements the same interface       |
|`DeviceClass` trait|What a device offers, expressed as capabilities          |Agents don't need to know hardware details               |
|`DeviceSession`    |Bounded, audited interaction with hardware               |OS knows who is using what, for how long, and why        |
|`SessionIntent`    |Why the agent wants access (purpose, priority, direction)|Enables QoS, conflict resolution, meaningful audit       |
|`DataChannel`      |Universal data pipe with Flow integration                |Hardware pipelines: mic → agent → speaker through Flow   |
|`ConflictPolicy`   |How to handle multiple agents wanting same device        |Share (audio mixer), Queue (printer), Prompt (camera)    |
|`PosixBridge`      |Maps /dev nodes and ioctl to subsystem sessions          |BSD tools work unmodified on any subsystem               |
|`PowerManaged`     |Idle policy, suspend, wake events                        |Uniform power management across all hardware             |
|`AuditRecord`      |Timestamped, agent-tagged event for any subsystem        |Cross-subsystem queries: "what hardware did agent X use?"|

**Mandatory kernel gate + optional userspace services pattern:** The capability gate is the only part in the kernel — a few hundred lines that enforce who can access what. Everything above (TLS service, audio mixer, connection pooling) is a userspace service that agents can use for convenience or bypass with raw capabilities and appropriate trust labeling.

**USB as meta-subsystem:** USB is a bus, not a device class. The USB subsystem identifies what's connected and routes it to the right subsystem (audio, input, storage, camera, network, print). The audio subsystem doesn't care whether a microphone is USB, Bluetooth, or built-in.

**All subsystems at a glance:**

|Subsystem|Channel Format   |Conflict Policy                     |POSIX Interface  |
|---------|-----------------|------------------------------------|-----------------|
|Network  |ByteStream       |Share (multiplex)                   |socket API       |
|Audio    |Audio samples    |Output: Share (mixer), Input: Prompt|/dev/audio*      |
|Display  |RenderSurface    |Share (compositor)                  |/dev/fb*, DRM    |
|Input    |Events           |Share (broadcast to focus)          |/dev/input/event*|
|Camera   |Video frames     |Prompt user                         |/dev/video*      |
|Storage  |ByteStream       |Share (filesystem layer)            |/dev/sd*, block  |
|Bluetooth|ByteStream/Events|Per-profile                         |/dev/bluetooth*  |
|Print    |Frames (pages)   |Queue (FIFO)                        |/dev/lp*, CUPS   |
|GPS      |Events (location)|Share (read-only)                   |—                |
|USB      |Varies by class  |Varies by class                     |/dev/usb*        |
|Power    |Control commands |Exclusive (kernel)                  |/sys/power/*     |

### 2.13 Browser Architecture

**Full design in [browser.md](../applications/browser.md).**

Traditional browsers are mini-operating systems because the actual OS provides nothing for web security. AIOS already has capabilities, isolation, audited networking, spaces, and Flow. The browser doesn't rebuild all of that — it uses what the OS provides and focuses on the one thing only a browser can do: **execute web content.**

**Decomposition:** The browser becomes a constellation of agents:

- **Browser Shell Agent** — tab management, URL bar, bookmarks (stored in spaces), history (stored in spaces), built with portable UI toolkit
- **Tab Agents** — one per site, each a literal AIOS agent with capabilities derived from the URL origin. Contains Servo rendering engine and SpiderMonkey JS runtime
- **Service Worker Agents** — persistent Tab Agents with constrained capabilities for background operation

**Same-origin policy becomes kernel-enforced capability isolation.** A Tab Agent for `weather.com` physically cannot read memory belonging to a Tab Agent for `bank.com`. Not a browser policy — a hardware-enforced capability boundary.

**Web APIs bridge to OS services through the subsystem framework:**

|Web API                     |Subsystem|Mechanism                                    |
|----------------------------|---------|---------------------------------------------|
|`fetch()`                   |Network  |Service channel with origin-scoped NetworkCap|
|`getUserMedia()` (camera)   |Camera   |CameraCapability, user prompted              |
|`getUserMedia()` (mic)      |Audio    |AudioCapability, user prompted               |
|`navigator.geolocation`     |GPS      |GpsCapability, user prompted                 |
|`WebGL` / `WebGPU`          |Display  |GpuCapability (limited)                      |
|`localStorage` / `IndexedDB`|Storage  |Web-storage space (origin sub-space)         |

**Web storage is a space.** All web storage (cookies, localStorage, IndexedDB, Cache API) maps to sub-spaces within `web-storage/`, scoped by origin. Unified quota, searchable by AIRS, syncable across devices, fully inspectable by the user.

**Unique capabilities:** Ad/tracker blocking at capability level (undetectable by anti-adblock), cross-agent web integration through Flow, spaces as PWA backends (sync without a server), transparent phishing protection using cross-subsystem context.

-----

## 3. Security Architecture

### 3.1 Eight-Layer Security Model

Every action by every agent passes through all eight layers. No single layer failing compromises the system.

```
┌──────────────────────────────────────────────────────┐
│  Layer 1: Intent Verification                         │
│  Does this action align with the declared task/intent?│
│  AI compares observed actions against user's goal.    │
│  Catches legitimate capabilities used inappropriately.│
├──────────────────────────────────────────────────────┤
│  Layer 2: Capability Check                            │
│  Does the agent hold the required capability token?   │
│  Kernel-enforced, unforgeable, revocable, expiring.   │
│  Fine-grained: per-space, per-object, per-device.     │
├──────────────────────────────────────────────────────┤
│  Layer 3: Behavioral Boundary                         │
│  Is the access pattern normal for this agent?         │
│  Rate limits, anomaly detection, baseline comparison. │
│  Catches compromised agents with valid capabilities.  │
├──────────────────────────────────────────────────────┤
│  Layer 4: Security Zone                               │
│  Is this data in a zone this agent can reach?         │
│  Core / Personal / Collaborative / Untrusted /         │
│  Ephemeral zones.                                      │
│  Promotion between zones requires review.             │
├──────────────────────────────────────────────────────┤
│  Layer 5: Adversarial Defense                         │
│  Is this action the result of prompt injection?       │
│  Control/data plane separation, injection detection.  │
│  Agent instructions from kernel, never from data.     │
├──────────────────────────────────────────────────────┤
│  Layer 6: Cryptographic Enforcement                   │
│  Does the agent have the decryption key?              │
│  Per-space encryption (spaces.md §6) + device-level   │
│  encryption (spaces.md §4.10). Keys released only     │
│  after intent verification.                           │
├──────────────────────────────────────────────────────┤
│  Layer 7: Provenance Recording                        │
│  Action logged to tamper-evident Merkle chain.        │
│  Cryptographically signed, append-only.               │
│  Cannot be bypassed — even if action is allowed.      │
├──────────────────────────────────────────────────────┤
│  Layer 8: Blast Radius Containment                    │
│  Even if all above fail, damage is bounded.           │
│  Max objects writable, auto-snapshot before bulk ops. │
│  Rollback window — changes reversible for 72 hours.   │
└──────────────────────────────────────────────────────┘
```

### 3.2 Capability System

```rust
pub enum Capability {
    // Space capabilities
    ReadSpace(SpaceId),
    WriteSpace(SpaceId),
    ReadObject(ObjectId),
    WriteObject(ObjectId),

    // Inference capabilities
    InferenceCpu(Priority),
    InferenceGpu(Priority),
    InferenceNpu(Priority),

    // Network capabilities (subsystem-specific)
    Network(NetworkCapability),     // per-service, per-method, per-path

    // Hardware subsystem capabilities (via subsystem framework)
    Audio(AudioCapability),         // direction, device, format constraints
    Camera(CameraCapability),       // resolution, frame rate limits
    Gps(GpsCapability),             // precision, update frequency
    Input(InputCapability),         // device types (keyboard, mouse, gamepad)
    Display(GpuCapability),         // memory limits, shader constraints
    Bluetooth(BluetoothCapability), // profile, device constraints
    Usb(UsbCapability),             // device class, raw access level
    Print(PrintCapability),         // printer, page limits

    // Agent capabilities
    SpawnAgent(AgentManifest),

    // Flow capabilities
    FlowRead,
    FlowWrite,

    // System capabilities
    AttentionPost(Urgency),
    IdentityRead,
    PreferenceRead,
    PreferenceWrite,
    AuditRead(Scope),
    UseCredential(CredentialId),
}

pub struct CapabilityToken {
    id: CapabilityTokenId,
    capability: Capability,
    holder: AgentId,
    granted_by: Identity,
    created_at: Timestamp,
    expires: Timestamp,
    delegatable: bool,
    attenuations: Vec<AttenuationSpec>,  // see security.md §3 for AttenuationSpec
    revoked: bool,
    parent_token: Option<TokenId>,  // for delegation chains
    usage_count: u64,
    last_used: Timestamp,
}
```

All subsystem capabilities pass through the same kernel-enforced gate (see [subsystem-framework.md](../platform/subsystem-framework.md) §5). The gate checks: does this agent hold the required capability? Does the capability permit this specific intent? Is it still valid? Is the resource budget exceeded? Every check is audited regardless of outcome.

### 3.3 Adversarial AI Defense

```rust
pub struct AdversarialDefense {
    input_screening: InputFilter,
    output_validation: OutputValidator,
    constraint_immutability: KernelEnforced,
    injection_detection: InjectionDetector,
}

/// Screens agent inputs for known injection patterns before they reach
/// the inference engine. Runs at the boundary between data and control planes.
pub struct InputFilter {
    /// Pattern-based detectors (regex, keyword, structural)
    pattern_detectors: Vec<PatternDetector>,
    /// ML-based detector trained on known injection corpora
    ml_detector: Option<ModelId>,
    /// Action on detection: block, sanitize, or flag for review
    on_detection: FilterAction,
}

/// Validates agent outputs before they are committed to spaces or
/// delivered via Flow. Catches data exfiltration and policy violations.
pub struct OutputValidator {
    /// Maximum output size per action
    max_output_bytes: u64,
    /// Forbidden content patterns (e.g., credential-shaped strings)
    forbidden_patterns: Vec<PatternDetector>,
    /// Space write rate limit (objects per minute)
    write_rate_limit: u32,
}

/// Detects prompt injection attempts by analyzing the boundary between
/// system instructions (from kernel/manifest) and user/data content.
pub struct InjectionDetector {
    /// Confidence threshold for flagging (0.0-1.0)
    threshold: f32,
    /// Whether to block or log-and-continue on detection
    enforcement: EnforcementMode,
}

pub enum FilterAction { Block, Sanitize, FlagForReview }
pub enum EnforcementMode { Block, LogOnly }

// Critical principle: agent instructions come from kernel,
// never from data objects. This is the control/data plane
// separation that prevents prompt injection from escalating
// to system-level compromise.
```

### 3.4 ARM Hardware Security Integration

|Feature                           |Use                                  |Phase                               |
|----------------------------------|-------------------------------------|------------------------------------|
|PAC (Pointer Authentication)      |Sign return addresses, mitigate ROP  |Phase 2 (kernel), Phase 13 (enforce)|
|BTI (Branch Target Identification)|Mitigate JOP attacks                 |Phase 2 (kernel), Phase 13 (enforce)|
|MTE (Memory Tagging Extension)    |Hardware use-after-free detection    |Phase 13                            |
|TrustZone (EL3)                   |Isolated secure world for key storage|Phase 24 (Secure Boot)              |
|TTBR0/TTBR1 separation            |User/kernel address space isolation  |Phase 2                             |
|W^X enforcement                   |Prevent code injection               |Phase 2                             |
|KASLR                             |Randomize kernel base address        |Phase 2                             |

-----

## 4. Developer Experience

### 4.1 Agent SDK

```rust
use aios_sdk::prelude::*;

#[agent(
    name = "Research Assistant",
    capabilities = [
        ReadSpace("research"),
        WriteSpace("research"),
        InferenceCpu(Priority::Normal),
        Network(services = ["api.anthropic.com", "arxiv.org"]),
    ]
)]
async fn research_agent(ctx: AgentContext) -> Result<()> {
    let history = ctx.conversation().await?;

    let related = ctx.spaces()
        .query("papers about transformer architectures")
        .since(Duration::weeks(4))
        .await?;

    let response = ctx.infer()
        .with_context(&history)
        .with_references(&related)
        .prompt(&user_message)
        .await?;

    ctx.spaces().save(
        "research",
        Object::document(response.summary())
            .derived_from(&related)
    ).await?;

    ctx.respond(response).await?;
    Ok(())
}
```

### 4.2 What Developers Get For Free

|System Service                |What It Replaces                             |Developer Effort Saved                   |
|------------------------------|---------------------------------------------|-----------------------------------------|
|Space Storage + Semantic Index|Vector DB, embedding pipeline, search        |Weeks of setup → zero                    |
|Context Manager               |Conversation persistence, state management   |Custom DB schema → zero                  |
|Inference Scheduler           |Model loading, GPU management, queueing      |CUDA setup, OOM handling → zero          |
|Capability System             |OAuth flows, permission models, consent UIs  |Per-service auth → one manifest          |
|Network Translation           |Socket management, TLS, retry, offline       |Networking code → zero                   |
|Subsystem Framework           |Device access, permissions, power mgmt       |Per-device driver code → one session open|
|Tool Manager                  |Tool calling frameworks, execution sandboxing|LangChain/CrewAI plumbing → zero         |
|Behavioral Monitor            |Security auditing, rate limiting             |Custom middleware → zero                 |
|Provenance Chain              |Audit logging, compliance                    |Custom logging → zero                    |

### 4.3 What Developers Build

|Type              |Description                              |Example                                              |
|------------------|-----------------------------------------|-----------------------------------------------------|
|Domain agents     |Bring expertise to a domain              |Legal contract reviewer, medical symptom analyzer    |
|Tools             |Single-purpose capabilities for any agent|PDF parser, web scraper, data visualizer             |
|Workflows         |Orchestrate agents for a use case        |Sales pipeline, academic research, course design     |
|Connectors        |Bridge external services into spaces     |Slack, GitHub, Notion, Google Workspace              |
|Space templates   |Pre-structured spaces for common needs   |Project management, client onboarding                |
|Experience plugins|Custom compositor UI components          |Specialized data visualization, domain-specific views|

### 4.4 Developer Workflow

```
1. Write agent using SDK (on Mac/Linux using portable toolkit)
2. Declare capabilities in manifest (including remote spaces)
3. `aios agent dev` → live-reload in sandboxed test space (QEMU or host)
4. `aios agent test` → run against mock spaces with synthetic data
5. `aios agent audit` → security analysis of capability usage
6. `aios agent publish` → sign, package, submit to agent store
7. Users discover via conversation bar or agent store
8. Users approve capabilities → agent runs
```

-----

## 5. Native Experiences

### 5.1 The Workspace (Home View)

The first thing the user sees. Not a desktop with icons. A contextual view that adapts to the inferred context.

**During work context:** Active tasks and their state. Recent spaces with activity indicators. Attention digest (triaged notifications). Conversation bar (prominent).

**During leisure context:** Recently played games/media. Casual browsing shortcuts. Minimal notifications (only urgent). Conversation bar (present but subtle).

**Always available:** Quick launch for any space or experience. System status (subtle, non-intrusive). Inspector access (for power users).

### 5.2 Conversation Bar

Always one gesture away. Never forced. User-invoked only (except genuine emergencies).

Handles: Natural language queries → space search. Task creation → "help me with X". System control → "I'm heads down for 2 hours". Quick actions → "send this to Alex". Help → "how do I do X".

Does NOT: Pop up uninvited. Suggest things unprompted. Interrupt activities. Require interaction for basic computer use.

### 5.3 Web Browser

**Full architecture in [browser.md](../applications/browser.md).**

Decomposed web content runtime based on Servo (Rust-based, embeddable). Each tab is a literal AIOS agent with capabilities derived from the URL origin. The Browser Shell manages tabs, bookmarks, history — all stored in spaces. Looks and feels like a normal browser to users.

Standard features: address bar, tabs, back/forward, bookmarks, downloads. All Web APIs work through thin shims that bridge to OS subsystems via the subsystem framework.

AI-enhanced (invisible): Closed tabs remain indexed in browsing space (findable later). Same-origin policy enforced by kernel capabilities, not browser logic. Content extraction available when user asks. Privacy-first by architecture (not extensions). Ad/tracker blocking at capability level (undetectable). Cross-agent integration through Flow. Web storage as spaces (searchable, syncable, inspectable).

### 5.4 Media Player

Standard player: play, pause, skip, volume, queue, library, playlists, album art.

AI-enhanced (invisible): Library is a space — semantic search works. Content indexed for later retrieval. Context Engine adjusts system behavior during playback.

### 5.5 Game Launcher

Standard launcher: library with game art, play time tracking, save management.

AI-enhanced (invisible): Game saves are space objects (versioned, never lost). Resources auto-optimized when game launches. Notifications suppressed during gameplay.

### 5.6 Inspector

Power user tool. Shows the 8 security layers in action. Full provenance chains. Agent activity logs. Behavioral analysis. Capability audit. Network activity per agent (what remote spaces each agent accessed). Cross-subsystem hardware audit — which agents accessed microphone, camera, GPS, and when. Per-tab browser resource accounting.

Available always, used by those who want transparency into the system.

-----

## 6. System Lifecycle

### 6.1 Boot Sequence

```
┌─ UEFI Firmware ─────────────────────────────────────────────┐
│  Hardware init, memory map, framebuffer, device tree        │
│  Load kernel ELF from EFI System Partition                  │
└──────────────────────┬──────────────────────────────────────┘
                       ▼
┌─ Kernel Early Boot (summary — see boot.md §3.3 for full 17 steps) ──┐
│  1. Exception vectors + UART init                             │
│  2. Device tree parse + platform detection                    │
│  3. GIC (v2 on Pi 4, v3 on Pi 5/QEMU) + timer init          │
│  4. MMU + page tables (TTBR0/TTBR1, W^X)                     │
│  5. Page allocator + heap init                                │
│  6. RNG + KASLR                                               │
│  7. Capability manager init (root capability created)         │
│  8. IPC subsystem init                                        │
│  9. Audit log init (kernel ring buffer until storage ready)   │
│  10. Process manager + provenance + scheduler init            │
└──────────────────────┬──────────────────────────────────────┘
                       ▼
┌─ Service Manager ───────────────────────────────────────────┐
│  Spawned as first userspace process with root capabilities  │
│                                                              │
│  Phase 1 — Storage (no dependencies):                        │
│    Block Engine → Object Store → Space Storage Service       │
│    System spaces created: system/devices, system/audit       │
│                                                              │
│  Phase 2 — Core services (depends on storage):               │
│    Device Registry → Subsystem Framework init                │
│    Input subsystem → Display subsystem → Compositor          │
│    Network subsystem (basic TCP/IP)                          │
│                                                              │
│  Phase 3 — AI services (depends on storage + compute):       │
│    AIRS loads → model registry scanned → default model loaded│
│    Space Indexer starts background indexing                   │
│    Context Engine begins signal collection                   │
│                                                              │
│  Phase 4 — User services (depends on all above):             │
│    Identity service → user authenticated                     │
│    Preference service → user settings applied                │
│    Attention manager → notification pipeline ready           │
│    Agent runtime → ready to spawn agents                     │
│                                                              │
│  Phase 5 — Experience (depends on compositor + services):    │
│    Workspace (home view) displayed                           │
│    Conversation bar available                                │
│    Boot complete                                             │
└─────────────────────────────────────────────────────────────┘
```

**Key invariant:** The system is usable at each phase boundary. If AIRS fails to load, the system still boots to a functional desktop — semantic search degrades to keyword search, intent verification is skipped (capability checks still enforced), and the conversation bar shows an "AI unavailable" status. Users can still launch agents, browse the web, and use BSD tools.

### 6.2 Graceful Degradation Without AIRS

AIRS is infrastructure, not a hard dependency. Every AIRS-dependent feature has a non-AI fallback:

| Feature | With AIRS | Without AIRS |
|---|---|---|
| Space search | Semantic (embedding similarity) | Keyword (full-text index, always maintained) |
| Intent verification (Layer 1) | AI compares actions against declared intent | Skipped — capability check (Layer 2) still enforced |
| Behavioral monitoring (Layer 3) | Anomaly detection via baseline comparison | Rate limits still enforced, anomaly detection disabled |
| Context Engine | Infers work/leisure from signals | Falls back to time-of-day heuristic + explicit overrides |
| Attention management | AI-triaged urgency assessment | Rule-based: source priority + keyword matching |
| Object metadata | AI-generated summaries, tags, embeddings | User-provided tags only, no embeddings |
| Conversation bar | Natural language interaction | Disabled — shows "AI service unavailable" |
| Adversarial defense (Layer 5) | Prompt injection detection | Disabled — other 7 layers still active |

**When AIRS goes down at runtime:** Active inference requests return `AirsUnavailable`. Agents handle this like any service error. The Context Engine freezes its last known state. The Attention Manager falls back to rules. No user-visible crash — the system becomes slightly less intelligent but fully functional.

**During early boot:** AIRS loads after storage but before the desktop. If model loading takes too long (>5 seconds), the Service Manager proceeds without it and AIRS loads in the background. The user sees the desktop immediately.

### 6.3 Agent Sandbox and Execution Model

Agents are the primary execution model for user-facing work. Each agent runs as an isolated OS process with a restricted capability set:

```rust
/// Agent process — isolation-relevant fields shown here.
/// Full struct also includes address_space, memory_stats, priority,
/// and suspended flag (see memory.md §5.1 for memory-related fields).
pub struct AgentProcess {
    pid: ProcessId,
    agent_id: AgentId,
    capabilities: CapabilitySet,       // kernel-enforced
    address_space: AddressSpace,       // per-agent page tables (TTBR0)
    memory_limit: usize,               // max RSS
    cpu_quota: CpuQuota,               // fair-share scheduling
    ipc_channels: Vec<ChannelId>,      // registered IPC endpoints
    space_access: Vec<SpaceMount>,     // mounted spaces
    manifest: AgentManifest,           // declared capabilities + metadata
    priority: AgentPriority,           // from manifest, for OOM scoring
    suspended: bool,                   // e.g., by thrash detector
}

/// A space mounted into an agent's namespace. Determines which spaces
/// an agent can access and at what POSIX path they appear.
pub struct SpaceMount {
    space_id: SpaceId,
    /// POSIX path where this space appears (e.g., "/spaces/research")
    mount_point: String,
    /// Access level: read-only or read-write
    access: MountAccess,
}

pub enum MountAccess { ReadOnly, ReadWrite }
```

**Isolation mechanisms:**
- **Memory isolation:** Each agent has its own address space (TTBR0). No shared memory except through explicit IPC shared regions, which require capability grants.
- **Capability confinement:** An agent cannot forge capabilities. Capabilities are kernel objects — agents hold references (tokens), not the capabilities themselves. The kernel validates every token on every syscall.
- **IPC mediation:** All inter-agent communication goes through kernel IPC. No direct memory sharing, no signals, no pipes between agents. The kernel logs every message exchange.
- **Resource limits:** Each agent has memory and CPU quotas. An agent that exceeds its memory limit is paused and the user is notified. An agent that spins CPU gets deprioritized by the scheduler.

**Language support:** The agent SDK has first-class support for:
- **Rust** — native, highest performance, direct syscall access
- **Python** — embedded interpreter (CPython or RustPython), SDK bindings, popular for AI/ML agents
- **TypeScript** — embedded V8 or QuickJS runtime, SDK bindings, popular for web-adjacent agents
- **WASM** — sandboxed execution, any language that compiles to WASM

All language runtimes run within the agent process. The SDK abstracts the syscall layer so agents written in Python or TypeScript use the same capability system as Rust agents.

### 6.4 Agent Update and Migration

When an agent is updated to a new version:

```
1. New manifest compared against old manifest
2. If capabilities unchanged → hot-swap path: active sessions are drained
   gracefully (agent gets shutdown signal, 5s to clean up), then new code
   loads and the agent is respawned. Session *state* (conversations, tasks)
   is preserved in spaces — the new instance reads it back on startup.
3. If capabilities expanded → user re-approval required before step 2
4. If capabilities reduced → auto-approved, old tokens revoked, then step 2
5. Agent data in spaces is preserved (spaces belong to the user, not the agent)
6. New version spawned with fresh capability tokens
```

**Key principle:** Spaces belong to users, not agents. An agent's data lives in spaces the user granted access to. Updating or removing an agent never deletes user data. The user can revoke an agent's space access at any time, and the data remains in the space.

### 6.5 Multi-Identity and Shared Devices

AIOS supports multiple identities on a single device:

```rust
pub struct DeviceIdentities {
    owner: IdentityId,               // device owner, full admin
    active: IdentityId,              // currently active identity
    registered: Vec<IdentityProfile>,
}

pub struct IdentityProfile {
    identity: IdentityId,
    spaces: Vec<SpaceId>,            // this identity's spaces
    agents: Vec<AgentManifest>,      // this identity's approved agents
    preferences: PreferenceSet,
    security_zone: SecurityZone,
}
```

**Identity switching:** When the active identity changes, the OS:
1. Suspends all agents belonging to the previous identity
2. Unmounts previous identity's spaces (encrypted at rest — inaccessible without identity keys)
3. Loads new identity's spaces and preferences
4. Resumes new identity's agents
5. Compositor switches to new identity's workspace

**Shared device mode (family computer):**
- Each family member has their own identity, spaces, and agents
- A shared space can be created with multiple identity access
- Children can have restricted capability sets (no agent installation, content filtering)
- The device owner can manage all identities

**Guest mode:** Ephemeral identity with minimal capabilities. All data in ephemeral space — deleted on logout. No agent installation. Network access through shared credential space only (WiFi).

### 6.6 Space Query Language

Spaces support three query modes:

```rust
/// Programmatic queries (always available, even without AIRS)
pub enum SpaceQuery {
    /// Exact match on metadata fields
    Filter {
        content_type: Option<ContentType>,
        tags: Vec<String>,
        created_after: Option<Timestamp>,
        created_before: Option<Timestamp>,
        modified_after: Option<Timestamp>,
        relations: Vec<(RelationKind, ObjectId)>,
    },

    /// Full-text search on content and metadata
    TextSearch {
        text: String,
        boost_recent: bool,
        limit: Option<usize>,
    },

    /// Semantic similarity (requires AIRS)
    Semantic {
        text: String,               // natural language
        threshold: f32,             // minimum similarity score
        limit: usize,
    },

    /// Graph traversal
    Traverse {
        start: ObjectId,
        relation: RelationKind,
        depth: u32,
        direction: TraverseDirection,  // Forward, Reverse, Bidirectional
    },
}
```

**Filter and TextSearch** work without AIRS — they use LSM-tree indexes and a full-text index maintained by the Space Storage service. **Semantic** queries require AIRS to generate query embeddings and compute similarity against the embedding index. **Traverse** queries walk the relationship graph.

The Conversation Bar translates natural language to `SpaceQuery` via AIRS:
```
User: "Find my notes about transformer architectures from last month"
  → SpaceQuery::Semantic {
      text: "transformer architectures",
      threshold: 0.7,
    } AND SpaceQuery::Filter {
      content_type: Some(Note),
      created_after: Some(one_month_ago),
    }
```

### 6.7 Content-Addressed Storage and Mutability

Objects have two identifiers:

```rust
pub struct Object {
    id: ObjectId,          // stable, mutable reference (UUID)
    content_hash: Hash,    // content-addressed, changes with content (SHA-256)
    // ...
}
```

- **`ObjectId`** is a stable UUID assigned at creation. It never changes. References between objects use `ObjectId`. Space queries return `ObjectId`.
- **`content_hash`** is the SHA-256 hash of the object's content. It changes every time the content is modified. The Version Store records each `(ObjectId, content_hash, timestamp)` tuple.

**How mutations work:**
```
1. Agent calls space.write(object_id, new_content)
2. Space Storage hashes new_content → new_hash
3. If new_hash == old_hash → no-op (content unchanged, deduplicated)
4. Store new content block at new_hash
5. Update object's content_hash pointer to new_hash
6. Append to Version Store: (object_id, new_hash, timestamp, agent_id)
7. Old content block is NOT deleted — it's still referenced by the version history
8. Garbage collection reclaims unreferenced blocks when version history is pruned
```

**Deduplication:** If two objects have identical content, they share the same content block. The block is reference-counted. Writing the same document twice doesn't double storage usage.

### 6.8 Error Recovery and System Resilience

**Service crash recovery:** The Service Manager monitors all services. If a service crashes:
```
1. Service Manager detects process exit
2. Active sessions on that service are terminated (clients get ServiceUnavailable)
3. Service is restarted with exponential backoff (immediate, 1s, 2s, 4s, max 30s)
4. After restart, service reloads state from its space (spaces survive crashes)
5. If service fails 5 times in 60 seconds → mark as degraded, notify user via Attention Manager (Urgency::Interrupt). Thresholds (5 failures, 60s window) are constants in the Service Manager config; they are chosen to distinguish transient failures (1-2 crashes from bad input) from persistent bugs (rapid crash loops).
6. Dependent services fall back to degraded mode (see §6.2 for AIRS example)
```

**Space corruption recovery:**
- Write-ahead log (WAL) ensures crash consistency — incomplete writes are rolled back on recovery
- Content-addressed storage provides integrity verification — hash mismatch = corruption detected
- Version history enables rollback — corrupt objects can be reverted to any previous version
- Block-level checksums detect storage media errors

**Kernel panic handling:**
- Kernel panics dump register state and backtrace to UART and a reserved memory region
- On reboot, the kernel checks the reserved region and saves the panic log to `system/crash/`
- The Space Storage WAL ensures no data loss from in-flight writes

### 6.9 Performance Targets

| Metric | Target | Rationale |
|---|---|---|
| Boot to desktop | < 3 seconds | Competitive with mobile, faster than most Linux distros |
| Compositor frame rate | 60 fps sustained | Smooth visual experience, no dropped frames |
| IPC round-trip latency | < 5 microseconds | Microkernel viability — services communicate via IPC constantly |
| Agent spawn time | < 50 milliseconds | Agents should feel instant to the user |
| Space object read | < 1 millisecond | Storage should not be a bottleneck for UI |
| Semantic search (AIRS) | < 500 milliseconds | Natural language queries must feel responsive |
| LLM inference (first token) | < 500 milliseconds | Conversation bar must respond quickly |
| Context switch | < 10 microseconds | Scheduler must be efficient with many agents |
| Memory per agent (minimum) | < 4 MB | Lightweight agents should be cheap |
| Minimum system RAM (kernel) | 2 GB | Pi 4 baseline — kernel-only, no local AI |
| Minimum for AI features | 4 GB | Local inference requires model pool (8 GB ideal) |
| Kernel image size | < 2 MB | Microkernel should be small |
| Base system disk usage | < 500 MB | Reasonable for embedded/Pi targets |

-----

## 7. Production OS Requirements

Beyond the MVP, a production OS requires these additional subsystems. Each implements the subsystem framework (see [subsystem-framework.md](../platform/subsystem-framework.md)) — the same capability gate, session model, audit logging, power management, and POSIX bridge as every other subsystem. See [development-plan.md](./development-plan.md) for implementation phases.

### 7.1 Power Management (Phase 19)

CPU frequency scaling (DVFS), display power management, device suspend, sleep/hibernate, thermal management. Without this, AIOS is tethered to a power outlet.

### 7.2 USB Stack (Phase 17)

xHCI host controller, USB hub support, mass storage, HID (keyboard, mouse, controllers), audio, video (webcams), serial, device hotplug. Real hardware uses USB for nearly everything.

### 7.3 WiFi & Bluetooth (Phase 18)

WiFi: firmware loading, WPA2/WPA3 authentication, regulatory compliance. Bluetooth: HID peripherals, audio (A2DP), nearby device communication. Both require proprietary firmware blobs on most hardware.

### 7.4 Secure Boot & Updates (Phase 24)

Verified boot chain (firmware → bootloader → kernel → AIRS → services). A/B partition scheme for atomic updates. Delta updates. Automatic rollback on failure. Separate model and agent update channels.

### 7.5 Display Protocol Compatibility (Phase 25)

Wayland compatibility layer for existing Linux GUI applications. XWayland for X11 apps. This gives access to thousands of existing applications.

### 7.6 Accessibility (Phase 23)

Screen reader support with semantic accessibility tree. Full keyboard navigation. High contrast / large text modes. Voice control. Switch access. Must be designed into compositor and toolkit from Phase 6, not retrofitted.

### 7.7 Internationalization (Phase 23)

Full Unicode everywhere. Input methods for CJK and complex scripts. Locale support (date, number, currency formats). UI string externalization for translation. Right-to-left layout support.

### 7.8 Printing & Peripherals (Phase 22)

CUPS port for printer support. Scanner support. Camera support. All require working network stack and USB stack.

### 7.9 Linux Binary Compatibility (Phase 25)

Compatibility layer for running unmodified Linux ELF binaries. Translates Linux syscalls to AIOS syscalls. Eliminates the app gap entirely. Long-term goal.

### 7.10 Enterprise Features (Phase 26)

MDM (Mobile Device Management), fleet management, remote wipe, compliance reporting, centralized policy enforcement. Required for organizational adoption.

-----

## 8. App Ecosystem Strategy

### Tier 1: BSD Command-Line Tools

Developers can work immediately. Compilers, editors, shell scripts — all functional through the POSIX layer.

### Tier 2: Web Applications

Through Servo/browser, users can access Gmail, Google Docs, Slack, YouTube, Netflix, Spotify, and thousands of other web apps. The web IS the app ecosystem.

### Tier 3: Native AIOS Agents

Purpose-built for AIOS, using the SDK. Start small (example agents) and grow as developers join.

### Tier 4: Linux Binary Compatibility

A compatibility layer that runs Linux ELF binaries on AIOS. Eliminates the app gap entirely.

### Tier 5: Wayland Applications

Native Wayland protocol support enables running existing Linux GUI applications that target the Wayland display protocol. This builds on Tier 4's Linux binary compatibility and the compositor's Wayland-compatible surface management (see compositor.md §10).

For launch, Tiers 1-3 must be solid. Tier 2 (web apps) is the critical one — it determines whether AIOS can be someone's only computer.

-----

## 9. Hardware Strategy

### 9.1 Development Roadmap

**Stage 1: QEMU aarch64 (development target, dev Phases 0-15).** All development and testing. HVF acceleration on macOS for near-native speed.

**Stage 2: Raspberry Pi 4/5 (first real hardware, dev Phase 16+).** Proves the OS works on real silicon. Known, documented hardware. Large community. See overview.md §9 for the phase-aligned hardware timeline.

**Stage 3: VM images (adoption path, dev Phase 24+).** AIOS runs in UTM/QEMU on Mac/Linux/Windows. Low barrier to entry.

**Stage 4: Partner hardware (growth).** Pine64 (PineBook, PinePhone), Framework Laptop, or similar open-hardware vendors.

**Stage 5: Own hardware (maturity).** Only if AIOS becomes a real platform with users and funding.

### 9.2 Initial Target: Laptops and PCs

AIOS initially targets **laptops and PCs**. This is where the hardware is generous enough to deliver the full AI-native experience without compromise:

| Resource | Typical Laptop (2024-2026) | What AIOS Gets |
|---|---|---|
| Storage | 256 GB - 2 TB NVMe SSD | Enough for multiple AI models, generous version history, full embedding indexes |
| RAM | 8 - 64 GB | Load 8B-13B models fully in RAM. 70B models viable at 32 GB+ with quantization |
| CPU | 4-16 cores, 2-5 GHz | Real parallelism for inference, indexing, and compositor simultaneously |
| GPU/NPU | Integrated or discrete | Future: GPU-accelerated inference, NPU offload for always-on tasks |
| Network | WiFi 6/6E, Gigabit Ethernet | Fast model downloads, low-latency sync |

The laptop/PC target means storage pressure is low. A 256 GB SSD gives AIOS ~180 GB after the host OS and user apps. A 512 GB SSD gives ~350 GB. Storage budgeting still matters (see [spaces.md §10](../storage/spaces.md)) but the constraints are comfortable — multiple models, generous version retention, full indexes.

### 9.3 Future Device Classes

AIOS is architectured for multi-device support, even though only laptops/PCs are supported at launch. The device profile system (see [spaces.md §10.1](../storage/spaces.md)) and the subsystem framework (see [subsystem-framework.md](../platform/subsystem-framework.md)) are designed so that adding a new device class requires writing hardware drivers and tuning profiles, not rearchitecting the system.

**Planned future targets (in rough priority order):**

| Device | When | Why | Key Constraints |
|---|---|---|---|
| **Tablets** (iPad-class) | After laptop stabilizes | Same form factor, touch input, split-screen UX | RAM-limited (6-8 GB), apps consume 40-50% storage |
| **Phones** | After tablet | Pocket AI assistant, always-on context | RAM-limited, 50-70% storage to apps/media, small screen |
| **TVs / Set-top boxes** | After core is stable | Living room AI — voice-first, media-centric | Very limited storage (16-128 GB), streaming-first for models |
| **Single-board computers** | Niche/enthusiast | Makers, kiosks, embedded AI | Tight on everything — 2-8 GB RAM, SD card storage |

Each future target brings a unique constraint that the architecture must handle:

- **Phones:** Apps and media consume 50-70% of storage. Today's iPhones have a minimum of 128 GB with 256 GB being the practical buy. AIOS competes for the remaining 30-50%. The storage budget and pressure system handles this, but the model strategy shifts to 1-2 small models with aggressive eviction.
- **TVs:** Minimal local storage. Models are streamed from a hub device on the local network or downloaded on demand. The NTM's mmap-over-network capability enables this — model weights are fetched as page faults, block by block.
- **SBCs:** The tightest constraints but also the simplest use case. Single model, minimal version history, aggressive compression.

### 9.4 Hardware Trends and AIOS Adaptation

Consumer hardware capabilities have grown exponentially and this trend shows no sign of slowing. AIOS's architecture is designed to ride this curve — what feels constrained on today's entry-level hardware will feel generous on tomorrow's baseline.

#### Storage Trajectory

```
Storage trends (mainstream consumer devices):

Year    Phone (base)    Phone (practical)   Laptop (base)    Laptop (practical)
────    ────────────    ─────────────────   ─────────────    ──────────────────
2020    64 GB           128 GB              256 GB           512 GB
2022    128 GB          128-256 GB          256 GB           512 GB
2024    128 GB          256 GB              256-512 GB       512 GB - 1 TB
2026    128-256 GB      256-512 GB          512 GB           1-2 TB
2028†   256 GB          512 GB - 1 TB       1 TB             2-4 TB
2030†   512 GB          1-2 TB              2 TB             4-8 TB

† Projections based on NAND flash pricing trends (~30-40% cost reduction per year
  for equivalent capacity) and historical upgrade patterns.
```

**What this means for AIOS:**

- **Today (2026):** A 256 GB laptop stores 3-6 AI models comfortably. A phone with 256 GB and 60% apps leaves ~100 GB for AIOS — workable but requires budgeting.
- **2028:** A 512 GB phone with 60% apps leaves ~200 GB — enough for the full AI experience with multiple models. The phone profile starts to resemble today's laptop profile.
- **2030:** Storage stops being a meaningful constraint on any mainstream device. Version history can default to `KeepAll`. Multiple large models fit on every device class. The storage pressure system still exists (users will always find ways to fill storage) but triggers rarely.

#### RAM Trajectory

RAM is the more critical constraint for AI workloads. Unlike storage (which is about caching and history), RAM directly determines what models can run and how fast:

```
RAM trends (mainstream consumer devices):

Year    Phone       Tablet      Laptop (base)    Laptop (power)
────    ─────       ──────      ─────────────    ──────────────
2020    4-6 GB      4-6 GB      8 GB             16-32 GB
2022    6-8 GB      6-8 GB      8-16 GB          32-64 GB
2024    8 GB        8 GB        8-16 GB          32-64 GB
2026    8-12 GB     8-12 GB     16-32 GB         64-128 GB
2028†   12-16 GB    12-16 GB    32 GB            128-256 GB
2030†   16-24 GB    16-24 GB    32-64 GB         128-512 GB

† Projections based on LPDDR pricing trends and the AI hardware arms race
  (Apple, Qualcomm, Samsung all investing in on-device AI RAM).
```

**What this means for AIOS:**

| RAM Available | Models That Fit | Experience |
|---|---|---|
| < 2 GB | No local model | Cloud-only or degraded — no local inference |
| 2-4 GB | 1B Q4_K_M (~0.9 GB) | Minimal — simple completions, limited context |
| 4-8 GB | 3B Q4_K_M (~2.0 GB) | Basic — simple queries, summarization |
| 8-16 GB | 8B Q4_K_M (~4.5 GB) | Good — conversational AI, search |
| ≥ 16 GB | 8B Q5_K_M (~4.5 GB, default) or Q6_K | Great — higher quality, room for vision model |
| 32 GB+ | 13B Q6_K or 70B Q4_K_M | Excellent — near-cloud quality locally |
| 64 GB+ | 70B Q6_K or multiple models loaded | Outstanding — full model library in RAM |

- **Today (2026):** 16 GB laptops are becoming baseline (Apple's M-series ships 16 GB minimum). This is the sweet spot for AIOS — one good 8B model fully loaded with room for the OS, compositor, browser, and agents.
- **2028:** 32 GB laptops become common. 13B models or quantized 70B models become viable on mainstream hardware. Phones reach 12-16 GB — enough for 8B inference, making phone AIOS a real product.
- **2030:** 64 GB laptops are mainstream. Full 70B inference without aggressive quantization. On-device AI quality approaches cloud. The distinction between "local AI" and "cloud AI" blurs for most tasks.

#### Compute Trajectory (CPU, GPU, NPU)

```
Compute trends relevant to on-device AI:

Capability            2024                   2028†                  2030†
─────────────         ────                   ────                   ────
CPU inference         ~10-15 tok/s (8B Q4)   ~25-40 tok/s (8B Q4)  ~40-60 tok/s
  (laptop)            on M3/Snapdragon X     ISA improvements       Wider SIMD, more cores

NPU (dedicated AI)    Apple Neural Engine    Pervasive in all SoCs  Standard co-processor
                      Qualcomm Hexagon       40-100 TOPS            100-200+ TOPS
                      Intel NPU              Standardized APIs      Unified memory w/ GPU
                      10-40 TOPS

GPU for inference     Offload possible       Better quantized       Real-time 13B on
                      (Metal, Vulkan)        inference support      integrated GPU

Memory bandwidth      50-100 GB/s (LPDDR5)   100-200 GB/s           200-400 GB/s
  (determines         Bottleneck for         Bottleneck eases       70B models become
   tok/s ceiling)     large models           for 13B+               truly interactive
```

**What this means for AIOS:**

- **NPUs are the game changer.** Current NPUs (10-40 TOPS) are used for image processing and simple ML. By 2028, dedicated AI accelerators at 40-100+ TOPS will be standard in every laptop, tablet, and phone SoC. AIOS should detect and use NPUs via the subsystem framework — the inference engine talks to an abstract `AcceleratorDevice`, and the subsystem driver handles the hardware specifics.
- **Memory bandwidth, not raw compute, is the bottleneck for LLM inference.** Token generation speed is primarily limited by how fast model weights can be read from RAM. LPDDR5x (2024) provides ~50-100 GB/s. LPDDR6 (2027-2028) will push 100-200 GB/s. This directly translates to faster inference without any software changes — AIOS just gets faster on newer hardware.
- **Inference speed improves ~2-3x per generation.** An 8B model that runs at 15 tok/s today will run at 30-45 tok/s on 2028 hardware. This makes the AI experience feel increasingly native and instantaneous.

#### Architectural Implications

AIOS's architecture is designed to **scale with hardware** rather than target a fixed hardware generation:

1. **Device profiles adapt automatically.** `DeviceProfile::detect()` examines actual hardware (RAM size, storage capacity, accelerator presence) rather than matching device labels. A phone from 2028 with 16 GB RAM and 512 GB storage will automatically get more generous quotas than a 2024 phone with 8 GB and 256 GB. No software update needed — the thresholds are capability-based.

2. **The memory pool system scales.** The kernel's physical memory manager (see [memory.md](../kernel/memory.md) §2.4) doesn't hardcode pool sizes. The Kernel, User, Model, and DMA pools are sized as percentages of available RAM. 8 GB machine → 4 GB model pool. 16 GB machine → 8 GB model pool. Bigger models load automatically.

3. **Storage budgets are percentage-based.** Quotas like "20% for models" mean 48 GB on a 256 GB laptop and 400 GB on a 2 TB laptop. The architecture doesn't need to know the absolute size — it adapts.

4. **The subsystem framework abstracts accelerators.** When NPUs become standard, AIOS adds an NPU subsystem driver. The inference engine doesn't change — it requests "accelerated matrix multiply" from the subsystem framework, which routes to CPU SIMD, GPU compute shader, or NPU depending on what's available. The best hardware wins automatically.

5. **Model quality improves with hardware.** On a 2024 laptop with 16 GB RAM, AIOS loads an 8B Q5_K_M model. On a 2028 laptop with 32 GB, it loads a 13B Q6_K — better quantization, more parameters, higher quality. The model profile system (see [airs.md §4.2](../intelligence/airs.md)) selects the best model that fits the current hardware. Users don't configure this — the system figures it out.

6. **Version history retention grows with storage.** On a 256 GB laptop, the default is `KeepLast(50)` (laptop profile override; base default is `KeepLast(20)` — see spaces.md §10.7). On a 2 TB laptop or a 2030 phone with 1 TB, the default can be `KeepAll`. The user never loses history if the hardware can afford it.

#### The Convergence Thesis

By 2030, the hardware gap between device classes narrows dramatically:

```
2024:   Phone (8 GB / 256 GB)  ←——— huge gap ———→  Laptop (16 GB / 512 GB)
2026:   Phone (12 GB / 256 GB) ←—— large gap ——→   Laptop (32 GB / 1 TB)
2028:   Phone (16 GB / 512 GB) ←— moderate gap —→   Laptop (64 GB / 2 TB)
2030:   Phone (24 GB / 1 TB)   ←— small gap ——→    Laptop (64 GB / 4 TB)
```

The phone of 2030 has the capabilities of a 2024 power-user laptop. This means:

- **AIOS's device profiles converge.** What requires separate Phone/Tablet/Laptop profiles today may need only two profiles in 2030: "standard" and "constrained" (TVs, SBCs, legacy devices).
- **On-device AI quality converges with cloud.** When every phone can run a 13B model at 30+ tok/s, the case for cloud inference weakens significantly for most tasks. AIOS's local-first architecture becomes the natural default, not a compromise.
- **The multi-device experience becomes seamless.** When every device can run the same model at acceptable quality, the user experience is consistent everywhere. Space Sync handles data; AIRS handles intelligence; the device profile handles resource tuning. Same OS, same AI, same experience — just different screens.

This is why AIOS invests in device profiles and adaptive systems now: the architecture built for 2024's hardware diversity gracefully handles 2030's hardware convergence. The code doesn't change — the profiles just get more generous.
