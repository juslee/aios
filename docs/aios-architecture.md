# AIOS: AI-First Operating System

## System Architecture Document

**Related documents:**

- [aios-development-plan.md](./aios-development-plan.md) — Phase plan, timeline, risks
- [aios-networking.md](./aios-networking.md) — Network Translation Module deep dive
- [aios-subsystem-framework.md](./aios-subsystem-framework.md) — Universal hardware abstraction architecture
- [aios-browser-architecture.md](./aios-browser-architecture.md) — Decomposed web content runtime

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
│  Hardware Abstraction                                               │
│  ├── GICv3 Interrupt Controller                                     │
│  ├── ARM Generic Timer                                              │
│  ├── PL011 UART                                                     │
│  ├── UEFI Runtime Services                                          │
│  └── Device Tree Parsing                                            │
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
│  B-tree indexed blocks on raw device        │
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
    access: CapabilitySet,
    parent: Option<SpaceId>,
    semantic_index: SemanticIndex,
    history: VersionLog,
    security_zone: SecurityZone,
    encryption_key: Option<SpaceKey>,
}

pub struct Object {
    id: ObjectId,
    content_type: ContentType,
    content: Content,
    semantic: SemanticMetadata,
    relations: Vec<Relation>,
    versions: Vec<Version>,
    provenance: ProvenanceChain,
}

pub struct SemanticMetadata {
    summary: String,
    tags: Vec<String>,
    embedding: Vec<f32>,
    entities: Vec<Entity>,
    description: String,
}

pub enum ContentType {
    Document, Code, Image, Audio, Video, Data,
    Conversation, Config, Agent, GameSave,
    WebPage, MediaReference, Task, Note,
}

pub struct Relation {
    target: ObjectId,
    kind: RelationKind,
    confidence: f32,
    explanation: Option<String>,
}

pub enum RelationKind {
    DerivedFrom, References, DependsOn,
    RelatedTo, CreatedBy, InputTo,
    OutputOf, ConversationContext,
    VersionOf, SiblingOf,
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

pub enum TaskState {
    Active,
    WaitingForUser(Question),
    WaitingForResource,
    Background,
    Suspended,
    Completed(Outcome),
    Failed(Error),
}

pub enum Persistence {
    Ephemeral,   // gone when done
    Session,     // lives until closed
    Persistent,  // survives reboot
}

pub struct AgentManifest {
    name: String,
    author: Identity,
    requested_capabilities: Vec<CapabilityRequest>,
    code: ContentHash,
    dependencies: Vec<ContentHash>,
    ai_analysis: SecurityAnalysis,
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
    source: ObjectRef,
    content: TypedContent,
    intent: TransferIntent,
    transformations: Vec<Transform>,
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
    model: AttentionModel,
    context: ContextState,
}

pub struct AttentionItem {
    source: AgentId,
    content: TypedContent,
    urgency: Urgency,       // AI-assessed, not app-declared
    relevance: f32,
    auto_actionable: Option<ProposedAction>,
    group: Option<GroupId>,
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
pub struct Identity {
    id: IdentityId,
    keys: KeyPair,           // Ed25519
    relationships: Vec<Relationship>,
    space_access: Vec<(SpaceId, AccessLevel)>,
    trust: TrustModel,
}

pub struct Relationship {
    with: IdentityId,
    kind: RelationshipKind,  // Colleague, Family, Service, Unknown
    trust_level: TrustLevel,
    shared_spaces: Vec<SpaceId>,
}
```

### 2.8 Preference System

Replaces config files. Conversational configuration, AI-mediated, evolves through use.

```rust
pub struct Preference {
    id: PreferenceId,
    description: String,
    value: PreferenceValue,
    source: PreferenceSource,
    affects: Vec<SystemComponent>,
    history: Vec<PreferenceChange>,
}

pub enum PreferenceSource {
    UserExplicit,
    UserBehaviorInferred,
    SystemDefault,
    AgentSuggested(AgentId),
}
```

### 2.9 Network Translation Module

Replaces application-level networking. Applications see spaces; the OS handles all networking transparently. **Full design in [aios-networking.md](./aios-networking.md).**

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
- Implements the subsystem framework (see [aios-subsystem-framework.md](./aios-subsystem-framework.md))

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

**Full design in [aios-subsystem-framework.md](./aios-subsystem-framework.md).**

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

### 2.13 Browser Architecture

**Full design in [aios-browser-architecture.md](./aios-browser-architecture.md).**

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
│  Core / Personal / Collaborative / Untrusted zones.   │
│  Promotion between zones requires review.             │
├──────────────────────────────────────────────────────┤
│  Layer 5: Adversarial Defense                         │
│  Is this action the result of prompt injection?       │
│  Control/data plane separation, injection detection.  │
│  Agent instructions from kernel, never from data.     │
├──────────────────────────────────────────────────────┤
│  Layer 6: Cryptographic Enforcement                   │
│  Does the agent have the decryption key?              │
│  Spaces encrypted at rest with per-space keys.        │
│  Keys released only after intent verification.        │
├──────────────────────────────────────────────────────┤
│  Layer 7: Provenance Recording                        │
│  Action logged to tamper-evident Merkle chain.        │
│  Cryptographically signed, append-only.               │
│  Cannot be bypassed — even if action is allowed.      │
├──────────────────────────────────────────────────────┤
│  Layer 8: Blast Radius Containment                    │
│  Even if all above fail, damage is bounded.           │
│  Max objects writable, auto-snapshot before bulk ops. │
│  Rollback window — changes reversible for N hours.    │
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
    id: TokenId,
    capability: Capability,
    holder: AgentId,
    granted_by: Identity,
    expires: Option<Timestamp>,
    delegatable: bool,
    attenuations: Vec<Attenuation>,
}
```

All subsystem capabilities pass through the same kernel-enforced gate (see [aios-subsystem-framework.md](./aios-subsystem-framework.md) §5). The gate checks: does this agent hold the required capability? Does the capability permit this specific intent? Is it still valid? Is the resource budget exceeded? Every check is audited regardless of outcome.

### 3.3 Adversarial AI Defense

```rust
pub struct AdversarialDefense {
    input_screening: InputFilter,
    output_validation: OutputValidator,
    constraint_immutability: KernelEnforced,
    injection_detection: InjectionDetector,
}

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
|TrustZone (EL3)                   |Isolated secure world for key storage|Phase 12 (identity)                 |
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
        Inference(Priority::Normal),
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

**Full architecture in [aios-browser-architecture.md](./aios-browser-architecture.md).**

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

## 6. Production OS Requirements

Beyond the MVP, a production OS requires these additional subsystems. Each implements the subsystem framework (see [aios-subsystem-framework.md](./aios-subsystem-framework.md)) — the same capability gate, session model, audit logging, power management, and POSIX bridge as every other subsystem. See [aios-development-plan.md](./aios-development-plan.md) for implementation phases.

### 6.1 Power Management

CPU frequency scaling (DVFS), display power management, device suspend, sleep/hibernate, thermal management. Without this, AIOS is tethered to a power outlet.

### 6.2 USB Stack

xHCI host controller, USB hub support, mass storage, HID (keyboard, mouse, controllers), audio, video (webcams), serial, device hotplug. Real hardware uses USB for nearly everything.

### 6.3 WiFi & Bluetooth

WiFi: firmware loading, WPA2/WPA3 authentication, regulatory compliance. Bluetooth: HID peripherals, audio (A2DP), nearby device communication. Both require proprietary firmware blobs on most hardware.

### 6.4 Secure Boot & Updates

Verified boot chain (firmware → bootloader → kernel → AIRS → services). A/B partition scheme for atomic updates. Delta updates. Automatic rollback on failure. Separate model and agent update channels.

### 6.5 Display Protocol Compatibility

Wayland compatibility layer for existing Linux GUI applications. XWayland for X11 apps. This gives access to thousands of existing applications.

### 6.6 Accessibility

Screen reader support with semantic accessibility tree. Full keyboard navigation. High contrast / large text modes. Voice control. Switch access. Must be designed into compositor and toolkit from Phase 7, not retrofitted.

### 6.7 Internationalization

Full Unicode everywhere. Input methods for CJK and complex scripts. Locale support (date, number, currency formats). UI string externalization for translation. Right-to-left layout support.

### 6.8 Printing & Peripherals

CUPS port for printer support. Scanner support. Camera support. All require working network stack and USB stack.

### 6.9 Linux Binary Compatibility

Compatibility layer for running unmodified Linux ELF binaries. Translates Linux syscalls to AIOS syscalls. Eliminates the app gap entirely. Long-term goal.

### 6.10 Enterprise Features

MDM (Mobile Device Management), fleet management, remote wipe, compliance reporting, centralized policy enforcement. Required for organizational adoption.

-----

## 7. App Ecosystem Strategy

### Tier 1: BSD Command-Line Tools

Developers can work immediately. Compilers, editors, shell scripts — all functional through the POSIX layer.

### Tier 2: Web Applications

Through Servo/browser, users can access Gmail, Google Docs, Slack, YouTube, Netflix, Spotify, and thousands of other web apps. The web IS the app ecosystem.

### Tier 3: Native AIOS Agents

Purpose-built for AIOS, using the SDK. Start small (example agents) and grow as developers join.

### Tier 4 (Future): Linux Application Compatibility

A compatibility layer that runs Linux ELF binaries on AIOS. Eliminates the app gap entirely.

For launch, Tiers 1-3 must be solid. Tier 2 (web apps) is the critical one — it determines whether AIOS can be someone's only computer.

-----

## 8. Hardware Strategy

**Phase 1: QEMU aarch64 (development target).** All development and testing. HVF acceleration on macOS for near-native speed.

**Phase 2: Raspberry Pi 4/5 (first real hardware).** Proves the OS works on real silicon. Known, documented hardware. Large community.

**Phase 3: VM images (adoption path).** AIOS runs in UTM/QEMU on Mac/Linux/Windows. Low barrier to entry.

**Phase 4: Partner hardware (growth).** Pine64 (PineBook, PinePhone), Framework Laptop, or similar open-hardware vendors.

**Phase 5: Own hardware (maturity).** Only if AIOS becomes a real platform with users and funding.
