# AIOS: AI-First Operating System — Architecture Overview

## Master Reference Document

**Version:** 2.0
**Target:** aarch64 (ARM64)
**Language:** Rust (kernel + userspace)
**License:** BSD-2-Clause (kernel, tools, SDK)
**Timeline:** 30 phases across ~138 weeks (~2.7 years)

---

## 1. Vision

AIOS is a clean-sheet microkernel operating system written in Rust for aarch64 where every subsystem is designed assuming AI exists. AI is not an application running on the OS — it is the infrastructure that makes every abstraction work better than on any other operating system.

The user never has to interact with AI to use the computer. AI enhances silently — performance, organization, security, search. When the user wants AI help, a conversation bar is always one gesture away. The system works perfectly without AI engagement and becomes extraordinary with it.

### 1.1 Design Principles

1. **AI is infrastructure, not interface.** The user never has to interact with AI to use the computer. AI enhances silently — performance, organization, security, search. When the user wants AI help, the conversation bar is always one gesture away.
2. **No legacy tax.** Every abstraction is designed for 2026, not inherited from 1970. Spaces instead of files. Tasks instead of processes. Flow instead of clipboard. Capabilities instead of permissions.
3. **The computer is one continuous experience.** Work, leisure, communication, creation — these aren't separate app silos. They're activities that share context through spaces, connected by relationships the AI maintains.
4. **Security through depth, not walls.** Eight layers of security, each designed for a world where autonomous agents act on your behalf. No single layer failing compromises the system.
5. **Developers build capabilities, not apps.** The SDK provides context, persistence, inference, security, and tool interop as system services. Developers write the interesting part.
6. **Portable where it matters.** The UI toolkit and developer tools run on Linux, macOS, and AIOS. Developers build on familiar platforms, deploy to AIOS. The OS earns adoption, it doesn't demand it.
7. **BSD-licensed ecosystem.** FreeBSD userland, musl libc, permissive licensing throughout. No GPL copyleft constraints on the OS or its users.
8. **One framework, every subsystem.** All hardware — networking, audio, USB, display, cameras, Bluetooth, printers — implements the same traits: capability gate, sessions, data channels, audit, power management, POSIX bridge. Adding new hardware is formulaic, not architectural.

### 1.2 What Makes AIOS Different From Every Other OS

| Traditional OS Concept | AIOS Concept | Why It's Better |
|---|---|---|
| Files & directories | Spaces & objects with relationships | AI maintains semantic index — find by meaning, not path |
| Processes | Tasks & agents | Users think about goals, not programs |
| Clipboard | Flow (context-aware data transfer) | Data transforms based on destination context |
| Notifications | Attention management (AI-triaged) | AI filters noise, surfaces what matters |
| Config files | Conversational preferences | Say what you want, AI translates to system parameters |
| Package manager | Capability-gated agents | No traditional installation — agents are signed, capability-scoped, and hot-swappable (see architecture.md §6.4) |
| Terminal shell | Conversation bar + POSIX terminal | Natural language primary interface, full terminal still available (architecture.md §2.10) |
| User accounts | Identity & relationships | Cryptographic identity, graduated trust |
| Window manager | Semantic compositor (GPU-native) | AI understands window content, mediates interactions |
| Filesystem permissions | 8-layer security model | Intent verification, behavioral boundaries, adversarial AI defense |
| Sockets & HTTP libraries | Network Translation Module | Apps see spaces, OS handles networking transparently |
| Application-level device APIs | Subsystem Framework | Universal hardware abstraction with capability gates |

---

## 2. System Architecture

### 2.1 Full Stack Overview

```mermaid
flowchart TD
    subgraph EXP["EXPERIENCE LAYER"]
        direction LR
        Workspace["`Workspace
*contextual home view*`"]
        ConvBar["`Conversation Bar
*always available, user-invoked*`"]
        MediaPlayer["`Media Player
*music, video, podcasts, streaming*`"]
        WebBrowser["`Web Browser
*Servo-based, semantic indexing*`"]
        GameLauncher["`Game Launcher
*library, saves as space objects*`"]
        Inspector["`Inspector
*provenance, security visibility*`"]
        AgentStore["`Agent Store
*discover, approve capabilities*`"]
        Settings["`Settings
*conversational, AI-mediated*`"]
    end

    subgraph SVC["SERVICES LAYER"]
        direction LR
        subgraph AIRS["AI Runtime Service — hot-swappable privileged service"]
            direction LR
            InfEngine["`Inference Engine
*GGML, NEON SIMD*`"]
            ModelReg["`Model Registry
*GGUF, LRU*`"]
            AgentLife["`Agent Lifecycle
*create, sandbox*`"]
            CtxMgr["`Context Manager
*state, compress*`"]
            ToolMgr["`Tool Manager
*register, exec*`"]
            SpaceIdx["`Space Indexer
*embed, relate*`"]
            CtxEngine["`Context Engine
*infer work/play*`"]
            AttnMgr["`Attention Mgr
*triage, digest*`"]
            IntentVer["`Intent Verifier
*action alignment*`"]
            BehMon["`Behavioral Monitor
*anomaly detect*`"]
            AdvDef["`Adversarial Def
*injection detect*`"]
            InfSched["`Inference Scheduler
*priority, deadline*`"]
        end

        SpaceStorage["`Space Storage
*object store, block engine, content-addr*`"]
        TaskMgr["`Task Manager
*intent to subtasks, orchestrate*`"]
        FlowSvc["`Flow Service
*context-aware data transfer, transform*`"]
        IdentitySvc["`Identity Svc
*crypto keys, relationships, trust model*`"]
        NTM["`Network Translation Module
*spaces to net*`"]
        PrefSvc["`Preference Svc
*conversational config, learn*`"]
        Compositor["`Compositor
*GPU-native, semantic-ready*`"]
        AudioSvc["`Audio Service
*mixing, route, decode, output*`"]

        subgraph SUBSYS["Subsystem Framework — universal hardware abstraction"]
            direction LR
            CapGate["`Capability Gate
*kernel-enforced*`"]
            Sessions["`Sessions
*bounded use*`"]
            DataChan["`Data Channels
*Flow-connected*`"]
            DevReg["`Device Registry
*system/devices/*`"]
            AuditSp["`Audit Spaces
*system/audit/*`"]
            PwrMgr["`Power Manager
*idle policies*`"]
            PosixBr["`POSIX Bridge
*/dev nodes*`"]
            ConflRes["`Conflict Res
*share/queue*`"]
            Hotplug["`Hotplug Handler
*USB, BT, etc.*`"]
        end

        POSIXCompat["`POSIX Compat
*BSD userland, musl libc, translation*`"]
        AgentRT["`Agent Runtime
*sandbox, SDK runtime, tool execution*`"]
        ConnSvc["`Connector Svc
*Slack, GitHub, external APIs*`"]
        DevDrivers["`Device Drivers
*VirtIO, USB, WiFi, BT*`"]
    end

    subgraph KERN["KERNEL SPACE"]
        subgraph AIKP["AI Kernel Primitives"]
            direction LR
            ModelMem["`Model memory regions
*shared, pinned, ref-counted*`"]
            ComputeAbs["`Compute device abstraction
*CPU/GPU/NPU*`"]
            AgentCap["`Agent capability tokens
*fine-grained, revocable, expiring*`"]
            ProvChain["`Provenance chain
*append-only, Merkle-linked, signed*`"]
            InfPrim["`Inference scheduling primitives
*priority, deadline, preempt*`"]
        end

        subgraph MICRO["Core Microkernel"]
            direction LR
            VMM["`Virtual Memory Manager
*4-level, TTBR0/TTBR1, W^X, KASLR*`"]
            IPC["`IPC
*sync message passing, capability transfer, zero-copy*`"]
            Sched["`Scheduler
*priority + deadline, context-aware hints*`"]
            CapMgr["`Capability Manager
*create, transfer, revoke, attenuate*`"]
            CryptoCore["`Cryptographic Core
*Ed25519, AES-256, key storage*`"]
            AuditLog["`Audit Log
*kernel-enforced, tamper-evident*`"]
            ProcMgr["`Process Manager
*create, isolate, terminate*`"]
        end

        subgraph HAL["Hardware Abstraction Layer"]
            direction LR
            PlatTrait["`Platform trait
*7 init methods, one per hardware class*`"]
            IntCtrl["`InterruptController
*GICv2 on Pi 4, GICv3 on Pi 5/QEMU*`"]
            Timer["`Timer
*ARM Generic Timer*`"]
            Uart["`Uart
*PL011 UART*`"]
            GpuDev["`GpuDevice
*VirtIO-GPU / VideoCore VI / VII*`"]
            NetDev["`NetworkDevice
*VirtIO-Net / Broadcom Genet*`"]
            StorDev["`StorageDevice
*VirtIO-Blk / Arasan SDHCI*`"]
            RngDev["`RngDevice
*VirtIO-RNG / bcm2835-rng*`"]
            UEFIRS["UEFI Runtime Services"]
            DTBParse["Device Tree Parsing + Platform Detection"]
        end
    end

    subgraph HW["HARDWARE"]
        direction LR
        CPU["`CPU
*aarch64*`"]
        RAM["RAM"]
        GPU["GPU"]
        NPU["NPU"]
        Storage["Storage"]
        Network["Network"]
    end

    EXP --> SVC
    SVC --> KERN
    KERN --> HW
```

### 2.2 Core Abstractions

**Spaces** replace the traditional filesystem. Objects instead of files. Semantic relationships instead of directory trees. Content-addressed storage with AI-maintained indexes.

**Tasks** replace the process model for user-facing work. Users think about goals, not programs. Processes still exist underneath for isolation, but users see tasks.

**Flow** replaces the clipboard. Context-aware data transfer with transformation, history, and provenance tracking.

**Context Engine** replaces explicit modes. Continuously infers user context from signals — active spaces, running agents, input patterns, time of day, media state. Adjusts AI engagement and notification thresholds automatically.

**Attention Management** replaces notifications. AI-triaged, context-aware, never interruptive during leisure unless genuinely urgent.

**Identity & Relationships** replaces user accounts. Cryptographic identity (Ed25519), graduated trust, relationship-aware sharing.

**Preferences** replaces config files. Conversational configuration, AI-mediated, evolves through observed behavior.

**Network Translation Module** replaces application-level networking. Applications see space operations; the OS handles DNS, TLS, connection pooling, retry logic, offline caching, bandwidth scheduling.

**Subsystem Framework** replaces ad-hoc hardware abstraction. Every hardware subsystem (network, audio, USB, display, camera, Bluetooth, GPS, print) implements the same five-layer architecture with capability gates, sessions, data channels, audit, and POSIX bridges.

### 2.3 Data Model

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
    content_hash: Hash,              // content stored separately (content-addressed)
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

/// Simplified overview; see spaces.md §3.3 for the canonical definition
/// and shared/src/storage.rs for the kernel-layer ContentType enum.
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

/// Simplified; see spaces.md §3.4 for the canonical RelationSource definition
/// (Explicit/AiInferred/SystemGenerated).
pub enum RelationSource {
    Ai,               // created by AIRS during indexing
    User,             // created explicitly by user action
    Agent(AgentId),   // created by an agent during operation
}
pub struct SemanticMetadata {
    summary: Option<String>,          // set by creator or AIRS (see spaces.md §3.3)
    tags: Vec<String>,
    auto_tags: Vec<String>,           // AIRS-generated tags
    embedding: Option<Vec<f32>>,      // AIRS-generated embedding
    entities: Vec<Entity>,
    description: Option<String>,
    auto_summary: Option<String>,
    text_content: Option<String>,     // extracted text for indexing
    indexed_at: Option<Timestamp>,
}
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
/// Links a task to its surrounding context (space, identity, context snapshot).
/// Full definition: architecture.md §2.3 (Task & Agent Model).
pub struct ContextLink {
    space_id: SpaceId,
    identity_id: IdentityId,
    snapshot_id: ObjectId,
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

/// Set of kernel-managed capability tokens held by a task or agent.
/// Full definition: architecture.md §2.3 (Task & Agent Model).
pub struct CapabilitySet {
    tokens: HashMap<CapabilityType, Vec<CapabilityToken>>,
}

/// Entry in a task's activity log. Records what an agent did and when.
/// Used by Intent Verification (Layer 1). Full definition: architecture.md §2.3.
pub struct ActivityEntry {
    timestamp: Timestamp,
    agent: AgentId,
    action: ActivityAction,
    capability: CapabilityType,
    duration: Option<Duration>,
}

/// Append-only Merkle-linked provenance chain for an object. Aggregates
/// per-version ProvenanceEntry records (spaces.md §5.1) for quick inspection.
/// Full definition: architecture.md §2.3 (Task & Agent Model).
pub struct ProvenanceChain {
    head: Hash,
    length: u64,
    origin: ProvenanceOrigin,
}
```

---

## 3. Security Architecture

### 3.1 Eight-Layer Security Model

Every action by every agent passes through all eight layers. No single layer failing compromises the system.

```mermaid
flowchart TD
    L1["`Layer 1: Intent Verification
*Does this action align with the declared task/intent?
AI compares observed actions against user's goal.*`"]
    L2["`Layer 2: Capability Check
*Does the agent hold the required capability token?
Kernel-enforced, unforgeable, revocable, expiring.*`"]
    L3["`Layer 3: Behavioral Boundary
*Is the access pattern normal for this agent?
Rate limits, anomaly detection, baseline comparison.*`"]
    L4["`Layer 4: Security Zone
*Is this data in a zone this agent can reach?
Core / Personal / Collaborative / Untrusted / Ephemeral.*`"]
    L5["`Layer 5: Adversarial Defense
*Is this action the result of prompt injection?
Control/data plane separation, injection detection.*`"]
    L6["`Layer 6: Cryptographic Enforcement
*Does the agent have the decryption key?
Spaces encrypted at rest with per-space keys.*`"]
    L7["`Layer 7: Provenance Recording
*Action logged to tamper-evident Merkle chain.
Cryptographically signed, append-only.*`"]
    L8["`Layer 8: Blast Radius Containment
*Even if all above fail, damage is bounded.
Max objects writable, auto-snapshot before bulk ops.*`"]

    L1 --> L2 --> L3 --> L4 --> L5 --> L6 --> L7 --> L8
```

### 3.2 ARM Hardware Security Integration

| Feature | Purpose | Phase |
|---|---|---|
| PAC (Pointer Authentication) | Sign return addresses, mitigate ROP | Phase 2 (kernel), Phase 13 (enforce) |
| BTI (Branch Target Identification) | Mitigate JOP attacks | Phase 2 (kernel), Phase 13 (enforce) |
| MTE (Memory Tagging Extension) | Hardware use-after-free detection | Phase 13 |
| TrustZone (EL3) | Isolated secure world for key storage | Phase 24 (Secure Boot) |
| TTBR0/TTBR1 separation | User/kernel address space isolation | Phase 2 |
| W^X enforcement | Prevent code injection | Phase 2 |
| KASLR | Randomize kernel base address | Phase 2 |

---

## 4. Subsystem Framework

Every hardware subsystem implements the same five-layer architecture:

```mermaid
flowchart TD
    A["`Agent API Layer
*What agents see: typed, semantic, capability-gated*`"]
    B["`POSIX Translation
*What BSD tools see: /dev nodes, ioctl, read/write*`"]
    C["`Subsystem Service
*Policy, multiplexing, routing, format negotiation*`"]
    D["`Device Abstraction
*Uniform trait per device class, regardless of hardware*`"]
    E["`Hardware Driver
*VirtIO, USB, PCI, platform-specific*`"]
    F(["Capability Gate (kernel-enforced) + Audit Space (all access logged)"])

    A --> C
    B --> C
    C --> D --> E
    F -.- C
    F -.- D
    F -.- E
```

All subsystems at a glance:

| Subsystem | Channel Format | Conflict Policy | POSIX Interface | Phase |
|---|---|---|---|---|
| Network | ByteStream | Share (multiplex) | socket API | 7, 16 |
| Audio | Audio samples | Output: Share (mixer), Input: Prompt | /dev/audio* | 22 |
| Display | RenderSurface | Share (compositor) | /dev/fb*, DRM | 5 |
| Input | Events | Share (broadcast to focus) | /dev/input/event* | 7 |
| Camera | Video frames | Prompt user | /dev/video* | 22 |
| Storage | ByteStream | Share (filesystem layer) | /dev/sd*, block | 4 |
| USB | Varies by class | Varies by class | /dev/usb* | 17 |
| Bluetooth | ByteStream/Events | Per-profile | /dev/bluetooth* | 18 |
| Print | Frames (pages) | Queue (FIFO) | /dev/lp*, CUPS | 22 |
| GPS | Events (location) | Share (read-only) | — | 22 |
| Power | Control commands | Exclusive (kernel) | /sys/power/* | 19 |

---

## 5. Network Translation Module

Applications never see the network. There are only space operations — some of which happen to involve remote spaces — and the OS handles everything else.

```mermaid
flowchart TD
    App["`Application
space::read#40;openai/v1/models#41;`"]

    subgraph NTM["Network Translation Module"]
        direction LR
        SpaceRes["`Space Resolver
*semantic name to endpoint + protocol + auth*`"]
        ConnMgr["`Connection Manager
*pool, TLS, multiplex, keepalive*`"]
        Shadow["`Shadow Engine
*offline transparency, local cache, sync*`"]
        Resilience["`Resilience Engine
*retry, backoff, circuit breaker*`"]
        BWSched["`Bandwidth Scheduler
*priority, multi-path, QoS, metered awareness*`"]
        CapGate["`Capability Gate
*verify net capability before ANY operation*`"]
    end

    Proto["`Protocol Engines
*HTTP/2 | HTTP/3/QUIC | AIOS Peer | MQTT | Raw Socket*`"]
    Transport["`Transport
*TLS 1.3 rustls | QUIC quinn | Plain TCP/UDP*`"]
    NetStack["`Network Stack
*smoltcp: TCP/UDP/ICMP/IPv4/IPv6/ARP/DHCP*`"]
    Drivers["`Interface Drivers
*VirtIO-Net | Ethernet | WiFi | Bluetooth | Cellular*`"]

    App --> NTM
    NTM --> Proto --> Transport --> NetStack --> Drivers
```

---

## 6. Browser Architecture

The browser is a constellation of agents, not a monolithic application:

- **Browser Shell Agent** — tab management, URL bar, bookmarks, history (all stored in spaces)
- **Tab Agents** — one per site, each a literal AIOS agent with capabilities derived from URL origin
- **Service Worker Agents** — persistent Tab Agents with constrained capabilities

Same-origin policy becomes kernel-enforced capability isolation. Web APIs bridge to OS subsystem services. Web storage maps to spaces (searchable, syncable, inspectable).

---

## 7. Developer Experience

Developers build six things on AIOS (see architecture.md §4.3 for full details):

| Type | Description | Example |
|---|---|---|
| **Agents** | Autonomous domain-specific programs | Research assistant, file organizer |
| **Tools** | Single-purpose functions agents can call | PDF parser, image classifier |
| **Workflows** | Orchestrate agents for a use case | Sales pipeline, academic research |
| **Connectors** | Bridge external services into spaces | Slack, GitHub, Google Workspace |
| **Space templates** | Pre-structured spaces for common needs | Project management, client onboarding |
| **Experience plugins** | Custom compositor UI components (views, widgets) | Chart widget, domain-specific data visualization |

The SDK provides inference, storage, security, networking, and context as system services. Developers write the domain-specific part.

---

## 8. App Ecosystem Strategy

**Tier 1: BSD Command-Line Tools** — Developers can work immediately via POSIX layer.

**Tier 2: Web Applications** — Through Servo browser, users access Gmail, Slack, YouTube, etc.

**Tier 3: Native AIOS Agents** — Purpose-built, using the SDK.

**Tier 4: Linux Binary Compatibility** — Compatibility layer runs unmodified Linux ELF binaries.

**Tier 5: Wayland Applications** — Existing Linux GUI apps via Wayland/XWayland compatibility.

---

## 9. Hardware Strategy

| Stage | Target | Purpose |
|---|---|---|
| Phase 0–15 | QEMU aarch64 (HVF on macOS) | All development and testing |
| Phase 16–19 | Raspberry Pi 4/5 | First real hardware validation (Tier 5 milestone) |
| Phase 20–23 | QEMU + Raspberry Pi | Rich experience development on both targets |
| Phase 24–27 | VM images (UTM/QEMU) | Low-barrier adoption path |
| Post-MVP | Pine64, Framework Laptop | Open-hardware partners |
| Maturity | Own hardware | Only if platform achieves critical mass |

---

## 10. Phase Plan Overview

### Tier 1: Hardware Foundation — Phases 0–3 (Weeks 1–16)

Boot, memory, IPC. Pure OS fundamentals that don't change regardless of what's built on top.

| Phase | Name | Duration | Deliverable |
|---|---|---|---|
| 0 | Foundation & Tooling | 2 weeks | Project scaffold, CI, `just build && just run` |
| 1 | Boot & First Pixels | 4 weeks | UEFI boot, framebuffer console, timer, exceptions |
| 2 | Memory Management | 4 weeks | Virtual memory, heap, W^X, KASLR |
| 3 | IPC & Capability System | 6 weeks | Process isolation, capabilities, service manager |

### Tier 2: Core System Services — Phases 4–7 (Weeks 17–34)

Storage, GPU, compositor, input, networking. The plumbing everything above depends on.

| Phase | Name | Duration | Deliverable |
|---|---|---|---|
| 4 | Block Storage & Object Store | 5 weeks | Persistent spaces, content-addressing, versioning |
| 5 | GPU & Display | 4 weeks | GPU-accelerated rendering, font rendering |
| 6 | Window Compositor & Shell | 5 weeks | Boot to GUI desktop with window management |
| 7 | Input, Terminal & Basic Networking | 4 weeks | Keyboard/mouse, terminal emulator, TCP/IP |

### Tier 3: AI & Intelligence — Phases 8–11 (Weeks 35–54)

This is where AIOS becomes what no other OS is.

| Phase | Name | Duration | Deliverable |
|---|---|---|---|
| 8 | AIRS Core (Inference Engine) | 5 weeks | Local LLM inference with streaming responses |
| 9 | Space Intelligence & Conversation | 5 weeks | Semantic search, Conversation Bar, conversational config |
| 10 | Agent Framework | 5 weeks | Capability-gated agents with intent verification |
| 11 | Tasks, Flow & Attention | 5 weeks | Task decomposition, smart clipboard, triaged notifications |

### Tier 4: Platform Maturity — Phases 12–15 (Weeks 55–74)

Developer ecosystem, security hardening, performance, POSIX compatibility. Includes 3 weeks buffer for integration testing across phases.

| Phase | Name | Duration | Deliverable |
|---|---|---|---|
| 12 | Developer Experience & SDK | 5 weeks | Multi-language SDK, CLI toolchain, documentation |
| 13 | Security Hardening | 4 weeks | Fuzzing, ARM PAC/BTI/MTE, encrypted spaces |
| 14 | Performance & Optimization | 3 weeks | Boot <3s, 60fps compositor, <500ms inference |
| 15 | POSIX Compatibility & BSD Userland | 5 weeks | FreeBSD tools, musl libc, self-hosting capability |

### Tier 5: Hardware & Connectivity — Phases 16–19 (Weeks 75–92)

Full networking, USB, wireless, power management. Required for real hardware.

| Phase | Name | Duration | Deliverable |
|---|---|---|---|
| 16 | Network Translation Module | 5 weeks | Full NTM: space resolver, shadow engine, protocols |
| 17 | USB Stack & Hotplug | 4 weeks | xHCI, HID, mass storage, device hotplug |
| 18 | WiFi, Bluetooth & Wireless | 5 weeks | WPA2/WPA3, BT audio/HID, firmware loading |
| 19 | Power Management & Thermal | 4 weeks | DVFS, sleep/hibernate, thermal throttling |

### Tier 6: Rich Experience — Phases 20–23 (Weeks 93–112)

Portable UI toolkit, web browser, media, accessibility.

| Phase | Name | Duration | Deliverable |
|---|---|---|---|
| 20 | Portable UI Toolkit | 5 weeks | Cross-platform toolkit (AIOS/Linux/macOS/Web) |
| 21 | Web Browser (Servo) | 5 weeks | Decomposed browser with tab-per-agent isolation |
| 22 | Media, Audio & Camera Subsystems | 5 weeks | Audio mixing, media player, camera subsystem |
| 23 | Accessibility & Internationalization | 5 weeks | Screen reader, keyboard nav, Unicode, i18n |

### Tier 7: Production OS — Phases 24–27 (Weeks 113–130)

Secure boot, Linux compatibility, enterprise features, hardware certification, launch.

| Phase | Name | Duration | Deliverable |
|---|---|---|---|
| 24 | Secure Boot & Update System | 5 weeks | Verified boot chain, A/B updates, rollback |
| 25 | Linux Binary & Wayland Compatibility | 5 weeks | Run unmodified Linux apps and GUI programs |
| 26 | Enterprise & Multi-Device | 4 weeks | MDM, fleet management, cross-device sync |
| 27 | Real Hardware, Certification & Launch | 4 weeks | Pi 4/5, Pine64, VM images, documentation site |

### Tier 8: Security Intelligence — Phases 28–29 (Weeks 131–138)

Composable capability profiles, AIRS-powered agent capability analysis.

| Phase | Name | Duration | Deliverable |
|---|---|---|---|
| 28 | Composable Capability Profiles | 4 weeks | 5-layer profile system, resolution algorithm, profile storage |
| 29 | AIRS Capability Intelligence | 4 weeks | 5-stage analysis pipeline, profile suggestions, security audit |

### Timeline Summary

```text
Year 1 (Weeks 1–54):     Tiers 1–3 — Functioning AI-first OS on QEMU
Year 2 (Weeks 55–92):    Tiers 4–5 — Developer platform with real hardware support
Year 2.5 (Weeks 93–130): Tiers 6–7 — Production OS ready for daily use
Year 2.7 (Weeks 131–138): Tier 8 — Security intelligence and capability profiles
```

---

## 11. Document Index

### Architecture Documents

```text
docs/
├── project/
│   ├── overview.md                          ← This document
│   ├── architecture.md                      System architecture deep dive
│   ├── development-plan.md                  Timeline, risks, dependencies, decision gates
│   ├── developer-guide.md                   Kernel development guide for Rust OS development
│   ├── ai-agent-context.md                  Context-loading checklist for AI coding agents
│   ├── language-ecosystem.md                Language runtime hub (Rust, Python, TS, WASM)
│   │   ├── runtimes.md   Runtime deep dives per language
│   │   ├── integration.md  Integration and build plan
│   │   ├── operations.md   Operations, security, resource isolation
│   │   └── ai.md         AI-driven runtime optimization strategies
│   │
├── kernel/
│   ├── boot.md                              Boot sequence (QEMU direct and UEFI)
│   ├── boot/                    Boot lifecycle stages and secondary core init
│   ├── hal.md                               Hardware Abstraction Layer (UART, GIC, timer, MMU)
│   ├── memory.md                            Memory management hub
│   │   ├── physical.md               Buddy allocator, frame allocator, slab, heap
│   │   ├── virtual.md                Page tables, KASLR, per-agent address spaces, TLB
│   │   ├── memory-ai.md                     AI model memory, PagedAttention, KV caches
│   │   ├── reclamation.md            Memory pressure, OOM, swap/zram, scaling
│   │   └── hardening.md              W^X, PAC, BTI, MTE, performance hardening
│   ├── ipc.md                               IPC channels, shared memory, notifications, select
│   ├── scheduler.md                         Per-CPU run queues, 4-class FIFO, load balancing
│   ├── deadlock-prevention.md               Lock ordering, priority inheritance, timeouts
│   └── observability.md                     Structured logging, metrics, tracing, ring buffers
│
├── intelligence/
│   ├── airs.md                              AI Runtime Service (inference, context, optimization)
│   ├── attention.md                         Attention management and notification triage
│   ├── context-engine.md                    Context inference and state integration
│   ├── preferences.md                       Conversational configuration and preference learning
│   └── task-manager.md                      Task decomposition and agent orchestration
│
├── storage/
│   ├── spaces.md                            Space Storage hub (object store, content-addressing)
│   │   ├── data-structures.md        Primitive types, Objects, Relations
│   │   ├── block-engine.md           On-disk layout, LSM-tree, WAL, compression
│   │   ├── versioning.md             Merkle DAG, snapshots, branching, temporal queries
│   │   ├── encryption.md             Per-space encryption, key management, nonces
│   │   ├── query-engine.md           Full-text search, embeddings, learned indexes
│   │   ├── sync.md                   Merkle exchange, conflict resolution, sync security
│   │   ├── posix.md                  Path mapping, fd lifecycle, POSIX translation
│   │   └── budget.md                 Device profiles, quotas, pressure, AI-driven policies
│   ├── flow.md                              Flow System hub (context-aware data transfer)
│   │   ├── data-model.md               External types, transfers, typed content
│   │   ├── transforms.md               Transform engine, format negotiation, AIRS transforms
│   │   ├── history.md                  History storage, retention, multi-device sync
│   │   ├── integration.md              Compositor, subsystem channels, cross-agent, POSIX
│   │   ├── security.md                 Capability enforcement, content screening, rate limiting
│   │   ├── sdk.md                      Rust/Python/TypeScript/PWA APIs
│   │   └── extensions.md               Near-term features and future directions
│
├── platform/
│   ├── compositor.md                        GPU-native compositor, window management, rendering
│   ├── subsystem-framework.md               Universal hardware abstraction framework
│   ├── networking.md                        Network Translation Module (Space Resolver, Shadow Engine)
│   ├── audio.md                             Audio mixing, routing, decode, output
│   ├── posix.md                             POSIX compatibility (BSD userland, musl libc)
│   └── power-management.md                  DVFS, sleep/hibernate, thermal throttling
│
├── experience/
│   ├── experience.md                        GUI experience layer (5 surfaces, design language)
│   ├── identity.md                          Cryptographic identity, graduated trust, relationships
│   └── accessibility.md                     Screen reader, keyboard nav, assistive technology
│
├── applications/
│   ├── agents.md                            Agent framework, SDK, lifecycle management
│   ├── browser.md                           Decomposed browser (Servo, tab-per-agent isolation)
│   ├── inspector.md                         Security and capability management dashboard
│   └── ui-toolkit.md                        Portable UI toolkit (AIOS/Linux/macOS/Web)
│
├── security/
│   ├── security.md                          Security model hub (threat model, 8-layer overview)
│   │   ├── layers.md               Eight defense layers deep dive
│   │   ├── capabilities.md         Capability tokens, attenuation, delegation, profiles
│   │   ├── hardening.md            Cryptography, ARM hardware security, testing
│   │   └── operations.md           Events, audit, zero trust, AIRS integration
│   ├── fuzzing.md             Fuzzing and input hardening hub
│   │   ├── strategies.md  Language, syscall, memory, IPC, driver hardening
│   │   ├── adoption-roadmap.md      Phased adoption (host-side through formal verification)
│   │   ├── tooling.md               Tiered tooling and fuzz target catalog
│   │   └── ai-native.md             AI-driven fuzzing (dev-time, kernel, AIRS)
│   └── static-analysis.md                   Static analysis and formal verification
│
├── research/
│   └── ccc-integration-analysis.md          Claude C Compiler integration analysis
│
└── phases/                                  Implementation milestones per phase
    ├── 00-foundation-and-tooling.md         Phase 0: project scaffold, CI, build
    ├── 01-boot-and-first-pixels.md          Phase 1: boot flow and first pixels
    └── ...                                  (30 phases total, created as work begins)
```

### Phase Implementation Documents

Each phase has a single implementation doc in `docs/phases/` with milestone steps, acceptance criteria, and references to architecture docs. Architecture lives in the domain directories above; phase docs define the build sequence. See [development-plan.md](./development-plan.md) §8 for the full list.

---

## 12. Related Architecture Documents

Deep-dive technical specifications organized by domain. Hub documents (bold) contain sub-document maps for their subsystem.

### Project

| Document | Scope |
|---|---|
| [architecture.md](./architecture.md) | Full system architecture — data models, boot sequence, agent sandbox, graceful degradation, performance targets |
| [development-plan.md](./development-plan.md) | Timeline, tier milestones, dependency graph, risk register, decision gates, staffing model |
| [developer-guide.md](./developer-guide.md) | Kernel development guide — build workflow, code patterns, testing, debugging |
| [ai-agent-context.md](./ai-agent-context.md) | Context-loading checklist for AI coding agents before writing kernel code |
| **[language-ecosystem.md](./language-ecosystem.md)** | Language runtimes (Rust, Python, TypeScript, WASM) — hub with 4 sub-docs |

### Kernel

| Document | Scope |
|---|---|
| [boot.md](../kernel/boot.md) | Boot sequence — QEMU direct boot, UEFI flow, BootInfo struct, kernel entry |
| [lifecycle.md](../kernel/boot/lifecycle.md) | Boot lifecycle stages, early boot phases, secondary core initialization |
| [hal.md](../kernel/hal.md) | Hardware Abstraction Layer — Platform trait, PL011 UART, GICv3, ARM Generic Timer |
| **[memory.md](../kernel/memory.md)** | Memory management — hub with 5 sub-docs (physical, virtual, AI, reclamation, hardening) |
| [ipc.md](../kernel/ipc.md) | IPC — channel-based messaging, shared memory, notifications, select, syscall table |
| [scheduler.md](../kernel/scheduler.md) | Scheduler — per-CPU run queues, 4-class FIFO, load balancing, context switching |
| [deadlock-prevention.md](../kernel/deadlock-prevention.md) | Lock ordering, priority inheritance, timeout mechanisms |
| [observability.md](../kernel/observability.md) | Structured logging, metrics, tracing, diagnostic ring buffers |

### Intelligence

| Document | Scope |
|---|---|
| [airs.md](../intelligence/airs.md) | AI Runtime Service — inference engine, model registry, Space Indexer, Context Engine, intent verification |
| [context-engine.md](../intelligence/context-engine.md) | Context inference — work/play detection, signal fusion, state integration |
| [attention.md](../intelligence/attention.md) | Attention management — notification triage, context-aware filtering |
| [task-manager.md](../intelligence/task-manager.md) | Task decomposition, intent-to-subtask orchestration, agent coordination |
| [preferences.md](../intelligence/preferences.md) | Conversational configuration, preference learning, AI-mediated settings |

### Storage

| Document | Scope |
|---|---|
| **[spaces.md](../storage/spaces.md)** | Space Storage — hub with 8 sub-docs (data structures, block engine, versioning, encryption, queries, sync, POSIX, budget) |
| **[flow.md](../storage/flow.md)** | Flow System — hub with 7 sub-docs (data model, transforms, history, integration, security, SDK, extensions) |

### Platform

| Document | Scope |
|---|---|
| [compositor.md](../platform/compositor.md) | Compositor — GPU-native rendering, semantic hints, layout engine, input routing, multi-monitor |
| [subsystem-framework.md](../platform/subsystem-framework.md) | Universal hardware abstraction — traits, types, patterns for every subsystem |
| [networking.md](../platform/networking.md) | Network Translation Module — Space Resolver, Shadow Engine, Bandwidth Scheduler, AIOS Peer Protocol |
| [audio.md](../platform/audio.md) | Audio subsystem — mixing, routing, decode, spatial audio, output |
| [posix.md](../platform/posix.md) | POSIX compatibility — BSD userland, musl libc, syscall translation |
| [power-management.md](../platform/power-management.md) | Power management — DVFS, sleep/hibernate, thermal throttling, battery policy |

### Experience

| Document | Scope |
|---|---|
| [experience.md](../experience/experience.md) | Experience Layer — 5 surfaces (Workspace, Activity Windows, Conversation Bar, Flow Tray, Status Strip), design language |
| [identity.md](../experience/identity.md) | Identity — cryptographic keys, graduated trust, relationship-aware sharing |
| [accessibility.md](../experience/accessibility.md) | Accessibility — screen reader, keyboard navigation, WCAG compliance |

### Applications

| Document | Scope |
|---|---|
| [agents.md](../applications/agents.md) | Agent framework — SDK, runtime, lifecycle management, capability-gated installation |
| [browser.md](../applications/browser.md) | Decomposed browser — Servo integration, tab-per-agent, Web API bridging, web storage as spaces |
| [inspector.md](../applications/inspector.md) | Inspector — security dashboard, capability visibility, provenance inspection |
| [ui-toolkit.md](../applications/ui-toolkit.md) | Portable UI toolkit — cross-platform (AIOS/Linux/macOS/Web) |

### Security

| Document | Scope |
|---|---|
| **[security.md](../security/security.md)** | Security model — hub with 4 sub-docs (layers, capabilities, hardening, operations) |
| **[fuzzing.md](../security/fuzzing.md)** | Fuzzing — hub with 4 sub-docs (strategies, adoption roadmap, tooling, AI-native) |
| [static-analysis.md](../security/static-analysis.md) | Static analysis and formal verification across all phases |

---

## 13. Success Criteria (Full Production OS)

**Core OS:**

- Boots on QEMU and real hardware (Pi 4/5, Pine64) in under 3 seconds
- GUI desktop with window management, taskbar, app switching
- Keyboard, mouse, touchpad, USB peripherals all functional
- WiFi and Bluetooth connectivity
- Power management (sleep, hibernate, thermal throttling)

**Spaces & Storage:**

- Objects persist with full version history and provenance
- Semantic search returns relevant results from natural language
- Content deduplication and integrity verification
- Encrypted spaces for personal security zone

**AI:**

- Local LLM inference with streaming responses
- Conversation Bar for natural language interaction
- AI-generated metadata on all space objects
- Intent verification and behavioral monitoring

**Agents & Ecosystem:**

- Agent Store with capability-gated installation
- Multi-language SDK (Rust, Python, TypeScript)
- Testing harness works without QEMU
- Four demo applications, comprehensive documentation

**Compatibility:**

- BSD userland (FreeBSD tools, musl libc)
- Linux binary compatibility for unmodified ELF binaries
- Wayland compatibility for Linux GUI apps
- Servo-based web browser for web applications

**Security:**

- 8-layer model implemented and tested
- All syscalls fuzzed, ARM PAC/BTI/MTE enabled
- Secure boot chain with A/B updates
- All kernel unsafe blocks documented

**Enterprise:**

- MDM support, fleet management, remote wipe
- Cross-device sync and collaborative spaces
- Compliance reporting and centralized policy

## 14. Future Directions

This section surveys recent advances in OS research, AI systems, and security that
inform AIOS's long-term roadmap. Items are categorized as **AIRS-dependent** (requiring
semantic understanding from the AI Runtime) or **kernel-internal** (purely statistical
or structural, operable without AIRS).

### 14.1 AI-First Operating System Research

The convergence of AI and operating systems is an active research frontier:

- **AIOS (Rutgers, COLM 2025)** — An LLM agent OS providing syscall-like primitives
  for agent lifecycle, context management, memory, storage, and tool access. Validates
  AIOS's core thesis that AI agents need first-class OS support, not bolted-on
  middleware. Key differences: Rutgers AIOS runs atop Linux as a user-space runtime;
  our AIOS provides these primitives at the kernel level with hardware-enforced
  capability isolation. *(AIRS-dependent)*

- **Semantic File System (ICLR 2025)** — Replaces path-based hierarchies with
  content-aware, LLM-indexed storage where files are retrieved by meaning rather than
  location. Directly validates AIOS's Space Storage design (§2.2) where objects have
  semantic metadata and are queryable via natural language. *(AIRS-dependent)*

- **OS-R1 (2025)** — Applies reinforcement learning to kernel parameter tuning
  (scheduler time slices, memory pressure thresholds, I/O queue depths), treating the
  kernel as an environment and performance metrics as reward signals. AIOS can adopt
  this approach in the scheduler (see `scheduler.md §16`) and memory subsystem
  without AIRS dependency — frozen decision trees derived from offline RL training
  can run entirely in-kernel. *(Kernel-internal)*

### 14.2 Modern Microkernel Advances

Production microkernel systems continue to push the boundary on isolation, verification,
and extensibility:

- **LionsOS (seL4-based, SOSP 2024)** — A multi-server OS framework on seL4 that
  achieves near-Linux performance through zero-copy I/O virtualization and
  asynchronous IPC. Demonstrates that microkernel overhead is solvable with careful
  data-plane design. Relevant to AIOS's IPC fast path (`ipc.md §4`) and VirtIO
  driver architecture. *(Kernel-internal)*

- **sched_ext / ghOSt (Linux 6.12+, Google 2024)** — eBPF-extensible and user-space
  pluggable scheduling frameworks that allow per-workload scheduling policies without
  kernel recompilation. AIOS's AIRS-driven scheduler (`scheduler.md §16`) can adopt
  a similar philosophy: frozen scheduling policies derived from ML training, hot-loaded
  as verified kernel modules. *(Kernel-internal for frozen policies; AIRS-dependent
  for online adaptation)*

- **Verus + Asterinas (2024-2025)** — Verus enables automated formal verification of
  Rust code with SMT-backed proofs. Asterinas is a Rust-based OS kernel designed for
  verification from the ground up. AIOS can adopt Verus for critical kernel
  invariants: capability table integrity, IPC message ring bounds, page table
  correctness. See `static-analysis.md` for AIOS's formal verification roadmap.
  *(Kernel-internal)*

- **seL4 Pancake (2024)** — A verified C-like language for writing verified device
  drivers that compile to CakeML and then to machine code with end-to-end correctness
  proofs. Relevant to AIOS's driver subsystem as a future path for verified VirtIO
  and platform drivers. *(Kernel-internal)*

### 14.3 AI-in-Kernel Techniques

Machine learning techniques applicable directly within the kernel, without requiring
the full AIRS runtime:

- **PagedAttention (vLLM, SOSP 2023)** — Borrows virtual memory paging concepts for
  KV cache management in LLM inference: non-contiguous physical blocks mapped via a
  block table, enabling memory sharing across requests and near-zero fragmentation.
  Already designed into AIOS's AI model memory subsystem (`memory-ai.md §6`).
  *(Kernel-internal for page management; AIRS-dependent for inference scheduling)*

- **Learning-Based Page Replacement (2024)** — Replaces LRU/Clock with lightweight
  neural predictors trained on access traces. A frozen decision tree (≤1KB) can
  outperform LRU by 10-15% on mixed AI/interactive workloads. Applicable to AIOS's
  memory reclamation subsystem (`memory/reclamation.md §8`). *(Kernel-internal)*

- **Deep Learning Prefetch (Google, 2024-2025)** — Uses LSTM/transformer models
  trained offline on I/O traces to predict future block accesses. The trained model
  is distilled to a compact predictor running in the block I/O path. Applicable to
  AIOS's Block Engine (`spaces/block-engine.md §4`) for proactive data staging.
  *(Kernel-internal)*

- **Learned Indexes (2024)** — Replaces B-trees with regression models that predict
  key positions, offering O(1) expected lookup with smaller memory footprint.
  Applicable to AIOS's Query Engine (`spaces/query-engine.md §7`) for content-hash
  lookups and metadata indexing. *(Kernel-internal for frozen models; AIRS-dependent
  for online retraining)*

### 14.4 Agent Security

As AI agents become first-class OS citizens, new threat models emerge:

- **OWASP Agentic AI Threats (2025)** — Identifies 15 threat categories specific to
  autonomous agents, including tool misuse, privilege escalation via prompt injection,
  inter-agent manipulation, and memory poisoning. AIOS's 8-layer security model
  (`security.md §1`) addresses many of these at the kernel level, but the taxonomy
  highlights gaps in runtime behavioral monitoring. *(AIRS-dependent for behavioral
  analysis; kernel-internal for capability enforcement)*

- **Inter-Agent Tool Call Attacks (2025)** — Demonstrates that agents can manipulate
  other agents through crafted tool call results, bypassing traditional access
  controls. AIOS's capability system (`capabilities.md §3`) provides
  structural defense: agents cannot invoke tools beyond their granted capabilities,
  and all cross-agent communication passes through audited IPC channels. The attack
  surface is further reduced by content screening in Flow (`flow/security.md §11`).
  *(Kernel-internal for capability gates; AIRS-dependent for content analysis)*

- **Prompt Injection Defense Pipelines (2024-2025)** — Multi-stage defense combining
  input sanitization, output validation, and behavioral anomaly detection. AIOS can
  implement the structural stages (input/output gates) at the IPC layer and delegate
  semantic analysis to AIRS. The Inspector (`inspector.md`) provides the monitoring
  dashboard. *(AIRS-dependent for semantic detection; kernel-internal for structural
  gates)*

- **Microkernel Sandboxing for AI Agents (Firecracker/microVM model)** — Each agent
  runs in a hardware-isolated address space with a minimal capability set, analogous
  to how Firecracker provides per-function isolation for serverless workloads. AIOS's
  per-agent address spaces (`memory/virtual.md §5`) and capability tables
  (`capabilities.md §3`) already implement this model natively.
  *(Kernel-internal)*
