# AIOS Agent Framework

## Deep Technical Architecture

**Parent document:** [architecture.md](../project/architecture.md)
**Related:** [ipc.md](../kernel/ipc.md) — IPC and syscall interface, [spaces.md](../storage/spaces.md) — Space storage, [subsystem-framework.md](../platform/subsystem-framework.md) — Hardware abstraction, [networking.md](../platform/networking.md) — Network Translation Module

-----

## 1. Overview

Agents are to AIOS what processes are to Unix — but with identity, declared capabilities, semantic context, and managed lifecycle. Every user-facing program on AIOS is an agent. The browser tabs are agents. The media player is an agent. Third-party developers write agents. The compositor is an agent. AIRS is an agent.

An agent is an isolated OS process paired with a manifest that declares what the agent is, what it needs, who wrote it, and what it has been verified to do. The kernel enforces the manifest. The Agent Runtime manages the lifecycle. The user approves the capabilities. Spaces belong to the user, never to the agent.

This document is the definitive specification for the agent execution model: anatomy, lifecycle, sandbox, SDK, communication, distribution, resource management, testing, and security integration.

-----

## 2. Agent Anatomy

### 2.1 What Is an Agent

An agent is an isolated OS process with:

- **A manifest** declaring identity, capabilities, code, and resource requirements.
- **A sandbox** enforcing memory isolation, capability confinement, and resource limits.
- **Space access** through capability-gated mounts — never direct filesystem access.
- **IPC channels** to system services and other agents — the only way to communicate.
- **A lifecycle** managed by the Agent Runtime — from installation through termination.

Agents do not share memory. Agents do not call each other's functions. Agents do not access hardware directly. Every interaction with the outside world goes through IPC, and every IPC message passes through the kernel's capability check. This is not a convention. It is enforced by hardware (TTBR0 address space isolation) and the capability system (kernel-managed, unforgeable tokens).

### 2.2 Agent Categories

```
┌────────────────────────────────────────────────────────────────────┐
│  System Agents                                                      │
│  Compositor, AIRS, Space Storage, Agent Runtime, Service Manager    │
│  Ship with the OS. Elevated capabilities. Trusted by the kernel.   │
├────────────────────────────────────────────────────────────────────┤
│  Native Experience Agents                                           │
│  Browser Shell, Media Player, Game Launcher, Inspector, Settings   │
│  Ship with the OS. User-level capabilities. No special privileges. │
├────────────────────────────────────────────────────────────────────┤
│  Third-Party Agents                                                 │
│  Installed from Agent Store or sideloaded in dev mode.             │
│  User-approved capabilities. AIRS-analyzed before install.         │
├────────────────────────────────────────────────────────────────────┤
│  Tab Agents                                                         │
│  One per browser tab, per web origin. Spawned by Browser Shell.    │
│  Ephemeral. Capabilities derived from URL origin. Sandboxed.       │
├────────────────────────────────────────────────────────────────────┤
│  Service Worker Agents                                              │
│  Persistent tab agents for background web tasks (push, sync).      │
│  Constrained capabilities. Survive tab close. Origin-scoped.       │
├────────────────────────────────────────────────────────────────────┤
│  Task Agents                                                        │
│  Ephemeral. Spawned for a specific task (e.g., "summarize this"). │
│  Terminated when the task completes or fails.                      │
└────────────────────────────────────────────────────────────────────┘
```

**System agents** have capabilities that user-level agents cannot obtain: raw device access, kernel IPC endpoints, capability minting. They are part of the trusted computing base. A bug in a system agent can compromise the system.

**Native experience agents** are first-party but unprivileged. The browser shell has the same capability model as a third-party agent — it requests capabilities in its manifest and the OS enforces them. The difference is that it ships with the OS and is pre-approved.

**Third-party agents** go through the full install flow: signature verification, AIRS analysis, capability review, user approval. They are the primary extension point for the platform.

**Tab agents** are an AIOS innovation. Because each browser tab runs as a separate OS agent, same-origin isolation is enforced by hardware (TTBR0), not browser logic. A compromised tab cannot read another tab's memory. The Browser Shell spawns tab agents with capabilities derived from the URL origin.

**Task agents** are created by AIRS or the Task Manager to handle user intents. "Summarize this document" might spawn a task agent that reads the document space, calls AIRS for inference, writes a summary, and exits. The agent's lifetime is the task's lifetime.

### 2.3 The AgentProcess

Every running agent is represented by an `AgentProcess` in the Agent Runtime:

```rust
pub struct AgentProcess {
    /// OS process identifier
    pid: ProcessId,

    /// Stable agent identifier (persists across restarts)
    agent_id: AgentId,

    /// Kernel-enforced capability set — what this agent can do
    capabilities: CapabilitySet,

    /// Maximum resident set size in bytes
    memory_limit: usize,

    /// CPU time allocation
    cpu_quota: CpuQuota,

    /// Registered IPC channel endpoints
    ipc_channels: Vec<ChannelId>,

    /// Mounted spaces with access mode
    space_access: Vec<SpaceMount>,

    /// Declared identity, code, and requirements
    manifest: AgentManifest,

    /// Current execution state
    state: AgentState,

    /// Cumulative resource usage statistics
    resource_stats: ResourceStats,

    /// Behavioral baseline for anomaly detection (Layer 3)
    behavioral_baseline: BehavioralBaseline,

    /// Parent agent (if spawned by another agent)
    parent: Option<AgentId>,

    /// Child agents spawned by this agent
    children: Vec<AgentId>,

    /// When this agent was started
    started_at: Timestamp,

    /// Associated task (if this is a task agent)
    task: Option<TaskId>,
}

pub struct SpaceMount {
    space: SpaceId,
    access: SpaceAccessMode,
    mount_point: String,      // agent-local name for this space
}

pub enum SpaceAccessMode {
    ReadOnly,
    ReadWrite,
    Append,                   // can create objects, cannot modify/delete existing
}

pub struct CpuQuota {
    /// Maximum CPU time per scheduling window (e.g., 50ms per 100ms)
    limit: Duration,
    /// Scheduling window duration
    window: Duration,
    /// Priority class
    priority: SchedulingPriority,
}

pub enum SchedulingPriority {
    Realtime,       // audio/video processing, compositor
    Interactive,    // user-facing agents with active windows
    Normal,         // background work agents
    Idle,           // maintenance, indexing, sync
}
```

### 2.4 The AgentManifest

The manifest is the agent's declaration of identity and intent. It is cryptographically signed by the author, analyzed by AIRS, and approved by the user. The kernel enforces it.

```rust
pub struct AgentManifest {
    // === Identity ===

    /// Human-readable name
    name: String,
    /// Semantic version
    version: Version,
    /// Cryptographic author identity
    author: Identity,
    /// Short description of what this agent does
    description: String,
    /// Icon (content hash of image object)
    icon: Option<ContentHash>,
    /// Unique identifier (reverse-domain: com.example.my-agent)
    bundle_id: String,

    // === Code ===

    /// Content hash of the code bundle
    code: ContentHash,
    /// What runtime executes this agent
    runtime: RuntimeType,
    /// Dependencies (other agent bundles this agent requires)
    dependencies: Vec<Dependency>,

    // === Capabilities ===

    /// What this agent requests permission to do
    requested_capabilities: Vec<CapabilityRequest>,

    // === Resources ===

    /// Memory requirements
    memory: MemoryRequirements,
    /// CPU requirements
    cpu: CpuRequirements,
    /// GPU requirements (if any)
    gpu: Option<GpuRequirements>,

    // === Lifecycle ===

    /// Should this agent start on boot?
    autostart: bool,
    /// Should this agent persist across reboots?
    persistent: bool,
    /// Can this agent run in the background?
    background: bool,

    // === Security ===

    /// Ed25519 signature of the manifest by the author
    signature: Signature,
    /// AIRS security analysis (populated at install time)
    ai_analysis: Option<SecurityAnalysis>,
    /// Minimum AIOS version required
    min_os_version: Option<Version>,
}

pub enum RuntimeType {
    /// Native aarch64 binary — compiled Rust or C
    Native,
    /// Python script — executed by embedded interpreter
    Python { version: PythonVersion },
    /// TypeScript — executed by embedded JS runtime
    TypeScript,
    /// WebAssembly module — executed by wasmtime
    Wasm,
}

pub struct CapabilityRequest {
    /// What capability is being requested
    capability: Capability,
    /// Why this capability is needed (shown to user)
    justification: String,
    /// Is this capability required or optional?
    required: bool,
}

pub struct MemoryRequirements {
    /// Minimum memory to function
    minimum: usize,
    /// Recommended memory for good performance
    recommended: usize,
    /// Hard maximum (agent killed if exceeded)
    maximum: usize,
}

pub struct CpuRequirements {
    /// Priority hint
    priority: SchedulingPriority,
    /// Expected CPU usage pattern
    pattern: CpuPattern,
}

pub enum CpuPattern {
    /// Mostly idle, occasional bursts (typical)
    Bursty,
    /// Sustained computation (ML inference, rendering)
    Sustained,
    /// Real-time requirements (audio, video)
    Realtime,
}

pub struct SecurityAnalysis {
    /// Overall risk assessment
    risk_level: RiskLevel,
    /// Capabilities actually used in the code (vs declared)
    capabilities_used: Vec<Capability>,
    /// Capabilities declared but not used (suspicious)
    capabilities_unused: Vec<Capability>,
    /// Detected patterns of concern
    concerns: Vec<SecurityConcern>,
    /// Timestamp of analysis
    analyzed_at: Timestamp,
    /// Model that performed the analysis
    model: ModelId,
}

pub enum RiskLevel {
    Low,        // standard capabilities, no concerns
    Medium,     // some elevated capabilities, justified
    High,       // sensitive capabilities, needs manual review
    Critical,   // system-level capabilities, reserved for system agents
}
```

-----

## 3. Agent Lifecycle

### 3.1 Installation

Agents are distributed as `.aios-agent` packages (see Section 8.2). The installation flow:

```
┌───────────────────────────────────────────────────────────────┐
│  1. Package received (from Store, sideload, or enterprise)    │
│     Verify .aios-agent archive integrity (checksums)          │
│                          │                                     │
│                          ▼                                     │
│  2. Verify author signature (Ed25519)                         │
│     Check author identity against trust store                 │
│     Reject if signature invalid or author unknown             │
│                          │                                     │
│                          ▼                                     │
│  3. AIRS security analysis                                    │
│     Static analysis of code bundle                            │
│     Verify capabilities used match capabilities declared      │
│     Flag unused capabilities, suspicious patterns             │
│     Produce SecurityAnalysis, attach to manifest              │
│                          │                                     │
│                          ▼                                     │
│  4. Present to user                                           │
│     Show: name, author, description, risk level               │
│     Show: each requested capability with justification        │
│     Show: AIRS analysis summary and concerns                  │
│     User approves, denies, or approves with restrictions      │
│                          │                                     │
│                          ▼                                     │
│  5. Store in system/agents/                                   │
│     Manifest stored as space object                           │
│     Code bundle stored content-addressed                      │
│     Approved capabilities recorded                            │
│                          │                                     │
│                          ▼                                     │
│  6. Register with Agent Runtime                               │
│     Agent ready to launch                                     │
│     If autostart: agent started immediately                   │
└───────────────────────────────────────────────────────────────┘
```

**Sideloading (dev mode):** During development, `aios agent dev` installs an agent without Store review. The agent runs in a sandboxed test space with synthetic data. AIRS analysis still runs but the user is not prompted — dev mode implies consent. Sideloaded agents are marked `dev-mode` in the Inspector and cannot access production spaces.

### 3.2 Startup

When an agent is launched, the Agent Runtime creates a fully isolated execution environment:

```
Agent Runtime: start(manifest) →

  1. Allocate address space (TTBR0)
     New page tables, empty user-space mapping
     Kernel mapped at TTBR1 (shared, read-only)

  2. Load code
     Native: map ELF sections into address space
     Python: load interpreter + script
     TypeScript: load JS runtime + script
     WASM: load wasmtime + module

  3. Initialize runtime
     Set up stack, heap, TLS
     Initialize language runtime (if non-native)
     SDK runtime init: parse manifest, set up event loop

  4. Mount spaces
     For each approved SpaceMount:
       Create IPC channel to Space Storage service
       Register capability for that space
       Map mount point in agent's space table

  5. Create IPC channels
     Channel to Agent Runtime (lifecycle management)
     Channel to Space Storage (if space capabilities granted)
     Channel to AIRS (if inference capability granted)
     Channel to Compositor (if display capability granted)
     Channel to Network Translation Module (if network caps granted)
     Channels to other subsystems as capabilities require

  6. Grant capability tokens
     Mint kernel capability tokens for each approved capability
     Tokens are unforgeable, revocable, optionally expiring
     Tokens stored in kernel's per-process capability table

  7. Call entry point
     Native: jump to _start → main()
     Python: exec("agent.py")
     TypeScript: evaluate("agent.ts")
     WASM: call _start export

  8. Agent event loop begins
     SDK handles event dispatch
     Agent code responds to events
```

**Startup latency target:** < 50ms from launch to event loop running. This requires lazy initialization of heavyweight runtimes (Python interpreter starts core only, imports lazily). Native agents meet this easily. WASM agents need module pre-compilation (done at install time, cached).

### 3.3 Running States

```
                              ┌──────────┐
                              │Installed │
                              └────┬─────┘
                                   │ start()
                                   ▼
                              ┌──────────┐
                         ┌────│ Starting │
                         │    └────┬─────┘
                         │         │ init complete
                         │         ▼
                         │    ┌──────────┐
               error     │    │  Active  │◄──────────────────┐
                         │    └──┬───┬───┘                   │
                         │       │   │                       │
                         │       │   ├── user switches away  │
                         │       │   │         │             │
                         │       │   │         ▼             │
                         │       │   │   ┌───────────┐       │
                         │       │   │   │  Paused   │───────┘
                         │       │   │   └───────────┘  user returns
                         │       │   │
                         │       │   ├── memory pressure
                         │       │   │         │
                         │       │   │         ▼
                         │       │   │   ┌───────────┐
                         │       │   │   │ Suspended │───────┘
                         │       │   │   └───────────┘  resources free
                         │       │   │
                         │       │   └── background=true, no window
                         │       │             │
                         │       │             ▼
                         │       │       ┌────────────┐
                         │       │       │ Background │──────┘
                         │       │       └────────────┘  foreground
                         │       │
                         │       ├── task complete
                         │       │         │
                         │       │         ▼
                         │       │   ┌───────────┐
                         │       │   │ Completed │
                         │       │   └───────────┘
                         │       │
                         │       └── unrecoverable error
                         │                 │
                         ▼                 ▼
                    ┌──────────┐    ┌──────────┐
                    │  Failed  │    │  Failed  │
                    └──────────┘    └──────────┘
                                        │
                    ┌───────────────────┘
                    ▼
              ┌─────────────┐
              │ Terminated  │  ← forced kill (OOM, security, user)
              └─────────────┘
```

```rust
pub enum AgentState {
    /// Installed but not running
    Installed,
    /// Process created, runtime initializing
    Starting,
    /// Fully running, processing events
    Active,
    /// User switched away, reduced priority, state preserved in memory
    Paused,
    /// Memory pressure: state serialized to space, process may be killed
    Suspended { state_space: SpaceId },
    /// Running without a visible window (if background=true)
    Background,
    /// Task completed successfully
    Completed { result: AgentResult },
    /// Unrecoverable error
    Failed { error: AgentError },
    /// Forcibly killed by the system
    Terminated { reason: TerminationReason },
}

pub enum TerminationReason {
    UserRequested,
    OutOfMemory,
    CpuQuotaExhausted,
    SecurityViolation { details: String },
    ParentTerminated,
    SystemShutdown,
    UpdateInProgress,
}
```

**State definitions:**

| State | Memory | CPU | IPC | Display |
|---|---|---|---|---|
| Active | full allocation | scheduled normally | all channels open | rendering |
| Paused | preserved | deprioritized | channels open, not serviced | not rendering |
| Suspended | serialized to space | none | channels closed | none |
| Background | full allocation | idle priority | channels open | no surface |
| Completed | freed | none | closed | destroyed |
| Terminated | freed | none | closed | destroyed |

### 3.4 Shutdown

**Graceful shutdown** (user closes agent, system update, task complete):

```
1. Agent Runtime sends ShutdownRequested event to agent
2. Agent has 5 seconds to:
   - Persist state to its spaces
   - Close active subsystem sessions
   - Send final IPC messages
   - Return from event loop
3. After 5 seconds (or agent returns), Runtime:
   - Revokes all capability tokens
   - Closes all IPC channels
   - Destroys all shared memory regions
   - Frees address space (TTBR0 page tables)
   - Updates audit log: agent shutdown
   - Removes from Agent Runtime registry
```

**Forced termination** (OOM, security violation, unresponsive):

```
1. Agent Runtime immediately:
   - Revokes all capability tokens (atomic)
   - Closes all IPC channels (peers get ChannelClosed error)
   - Kills the process (SIGKILL equivalent)
   - Frees address space
   - Logs termination reason to audit
2. No cleanup opportunity for the agent
3. Subsystem sessions are cleaned up by the subsystems
   (audio stops, network connections close, display surface destroyed)
```

**Invariant:** After termination, the agent holds zero capabilities, zero IPC channels, zero memory. Nothing leaks. Spaces the agent wrote to are unaffected — spaces belong to the user.

### 3.5 Update

When a new version of an agent is available:

```
1. New manifest received (from Store push or user action)
2. Compare new manifest against installed manifest

3. Capability diff:
   a. Capabilities unchanged →
      Hot-swap: stop old, start new with same tokens
      Active sessions preserved if possible

   b. Capabilities reduced →
      Auto-approve: fewer permissions is strictly safer
      Revoke tokens for removed capabilities
      Start new version

   c. Capabilities expanded →
      Re-approval required: user sees diff
      "Research Assistant v2.1 now requests: WriteSpace('notes')"
      User approves or denies new capabilities
      If denied: update blocked, old version continues

4. Data is preserved
   Spaces belong to the user, not the agent
   Updating an agent never deletes space data
   New version accesses the same spaces (if capabilities match)

5. Active sessions drained
   Old version gets ShutdownRequested
   5-second cleanup window
   New version spawned with fresh capability tokens
```

-----

## 4. Sandbox

### 4.1 Isolation Mechanisms

Five mechanisms enforce agent isolation. All five are always active. Disabling any one of them requires kernel modification.

```
┌─────────────────────────────────────────────────────────┐
│  1. Address Space Isolation (TTBR0)                      │
│     Each agent has its own page tables.                  │
│     Agent A cannot address Agent B's memory.             │
│     Hardware-enforced by ARM MMU.                        │
├─────────────────────────────────────────────────────────┤
│  2. Capability Confinement                               │
│     Every operation requires a kernel capability token.  │
│     Tokens are unforgeable (kernel objects, not data).   │
│     No token = no access. No exceptions.                 │
├─────────────────────────────────────────────────────────┤
│  3. IPC-Only Communication                               │
│     No shared memory without explicit capability grant.  │
│     No signals between agents.                           │
│     All communication through kernel IPC.                │
│     All IPC metadata logged to audit.                    │
├─────────────────────────────────────────────────────────┤
│  4. Resource Limits                                      │
│     Memory: hard RSS limit, exceeded = paused + notify.  │
│     CPU: fair-share quota, exceeded = deprioritized.     │
│     Network: per-agent bandwidth accounting.             │
│     Inference: per-agent token quota.                    │
├─────────────────────────────────────────────────────────┤
│  5. Syscall Whitelist                                    │
│     Agents can only invoke AIOS syscalls (31 total).     │
│     No raw Linux syscalls. No ioctl. No ptrace.          │
│     W^X enforced: memory is writable XOR executable.     │
└─────────────────────────────────────────────────────────┘
```

### 4.2 What an Agent Cannot Do

An agent CANNOT:

- **Read another agent's memory.** Hardware-enforced. TTBR0 isolation means Agent A's page tables do not map Agent B's pages. Period.
- **Forge a capability.** Capabilities are kernel objects referenced by opaque handles. An agent cannot construct a capability from raw data.
- **Communicate without IPC.** No shared files, no signals, no shared memory (unless explicitly granted via capability). All communication goes through the kernel.
- **Access hardware directly.** All hardware access goes through subsystem services. An agent that wants audio goes through the audio subsystem. An agent that wants network goes through the Network Translation Module. The subsystem checks capabilities.
- **Exceed resource limits.** The kernel enforces memory and CPU limits. An agent that allocates beyond its limit is paused. An agent that spins CPU is deprioritized.
- **Persist data outside approved spaces.** The agent has no filesystem access. It can only write to spaces it has been granted write capability for.
- **Spawn processes.** An agent can spawn child agents only if it holds the `SpawnAgent` capability. Child agents inherit a subset of the parent's capabilities, never more.
- **Escalate privileges.** Capabilities can only be attenuated (restricted), never amplified. A child agent cannot have more capabilities than its parent.
- **Bypass AIRS analysis.** The install flow always includes AIRS code analysis. Even sideloaded dev-mode agents are analyzed (though the user is not prompted).

### 4.3 Syscall Interface

Agents interact with the kernel through a minimal syscall set. These are the only operations an agent can perform at the hardware level. Everything else goes through IPC to userspace services.

```rust
pub enum AgentSyscall {
    // === IPC ===

    /// Send a message and wait for reply (synchronous RPC)
    IpcCall {
        channel: ChannelId,
        send_buf: *const u8,
        send_len: usize,
        recv_buf: *mut u8,
        recv_len: usize,
    },

    /// Send a message without waiting for reply
    IpcSend {
        channel: ChannelId,
        send_buf: *const u8,
        send_len: usize,
    },

    /// Wait for a message on a channel
    IpcRecv {
        channel: ChannelId,
        recv_buf: *mut u8,
        recv_len: usize,
    },

    /// Wait for a message on any of multiple channels (multiplexed wait)
    IpcSelect {
        channels: *const ChannelId,
        channel_count: usize,
        recv_buf: *mut u8,
        recv_len: usize,
        timeout: Option<Duration>,
    },

    // === Memory ===

    /// Allocate virtual memory pages
    MemAlloc {
        size: usize,
        flags: MemoryFlags,        // Read, Write — never Execute with Write (W^X)
    },

    /// Free virtual memory pages
    MemFree {
        addr: usize,
        size: usize,
    },

    /// Map a shared memory region received via IPC
    MemMapShared {
        region: SharedMemoryId,
        flags: MemoryFlags,
    },

    // === Capabilities ===

    /// Create a restricted derivative of a held capability
    CapDerive {
        source: CapabilityTokenId,
        restrictions: AttenuationSpec,
    },

    /// Revoke a capability (own or delegated)
    CapRevoke {
        capability: CapabilityTokenId,
    },

    // === Flow ===

    /// Push data to the Flow system
    FlowPush {
        content: *const u8,
        content_len: usize,
        content_type: ContentType,
    },

    /// Pull current Flow content
    FlowPull {
        buf: *mut u8,
        buf_len: usize,
    },

    // === System ===

    /// Yield remaining time slice to scheduler
    Yield,

    /// Exit the process
    Exit {
        code: i32,
    },

    /// Sleep for a duration
    Sleep {
        duration: Duration,
    },

    /// Get current time
    GetTime {
        clock: ClockId,
    },

    /// Set a timer that wakes IpcSelect
    TimerSet {
        duration: Duration,
        repeat: bool,
    },

    // === Debug ===

    /// Log a message to the agent's audit space
    Log {
        level: LogLevel,
        msg: *const u8,
        msg_len: usize,
    },
}

pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}
```

Total: 17 agent-facing syscalls (out of 31 kernel syscalls — see ipc.md §3.4). The remaining 14 are kernel-internal or service-only (ChannelCreate, ProcessCreate, IpcReply, AuditLog, etc.). Compare with Linux (~450). Every syscall is exhaustively fuzz-tested. The small surface area is a security property, not a limitation — everything agents need is accessible through IPC to userspace services.

-----

## 5. Agent SDK

### 5.1 Architecture

The SDK provides a high-level API that agents use to interact with the system. It hides the syscall layer, IPC serialization, and event loop mechanics:

```
┌─────────────────────────────────────────────────────┐
│                    Agent Code                        │
│  Business logic written by the developer             │
│  Uses AgentContext methods, responds to events        │
├─────────────────────────────────────────────────────┤
│                  SDK High-Level API                   │
│  AgentContext, spaces(), infer(), respond(), etc.    │
│  Typed, async, idiomatic Rust/Python/TypeScript      │
├─────────────────────────────────────────────────────┤
│                   SDK Runtime                         │
│  Event loop, IPC dispatch, message serialization     │
│  Capability token management, channel multiplexing   │
├─────────────────────────────────────────────────────┤
│                  Syscall Wrapper                      │
│  Thin unsafe layer: Rust → SVC #0 → kernel           │
│  One function per syscall, validated parameters      │
├─────────────────────────────────────────────────────┤
│                     Kernel                            │
│  Validates, enforces, dispatches                     │
└─────────────────────────────────────────────────────┘
```

The SDK is language-specific at the top (idiomatic Rust, Python, or TypeScript) and shared at the bottom (same syscall wrappers, same IPC protocol, same capability model). An agent written in Python has identical security properties to one written in Rust.

### 5.2 AgentContext API

`AgentContext` is the primary interface agents use to interact with the system. It is created by the SDK runtime at startup and passed to the agent's entry point.

```rust
#[async_trait]
pub trait AgentContext: Send + Sync {
    // === Spaces ===

    /// Access the space subsystem. Query, read, write, and manage
    /// objects in spaces this agent has capabilities for.
    fn spaces(&self) -> &dyn SpaceClient;

    // === AI Inference ===

    /// Access the AIRS inference engine. Generate text, embeddings,
    /// classifications, and summaries. Requires InferenceCpu/Gpu/Npu cap.
    fn infer(&self) -> &dyn InferenceClient;

    // === Conversation ===

    /// Access the conversation history for this agent's task.
    /// Returns the user's messages and the agent's previous responses.
    async fn conversation(&self) -> Result<Vec<ConversationMessage>>;

    // === Response ===

    /// Send a response to the user (text, structured data, or UI).
    /// Routed through the Conversation Bar or agent's own surface.
    async fn respond(&self, response: Response) -> Result<()>;

    // === Flow ===

    /// Access the Flow system for context-aware data transfer.
    fn flow(&self) -> &dyn FlowClient;

    // === Attention ===

    /// Post an attention item (notification). Urgency is AI-assessed,
    /// not self-declared — the agent provides content, AIRS decides urgency.
    async fn attention(&self, item: AttentionRequest) -> Result<()>;

    // === Tools ===

    /// Register a tool that other agents (or AIRS) can call.
    async fn register_tool(&self, tool: ToolDefinition) -> Result<()>;

    /// Call a tool registered by another agent or system service.
    async fn call_tool(&self, name: &str, params: Value) -> Result<Value>;

    /// List available tools (from all agents this agent can communicate with).
    async fn list_tools(&self) -> Result<Vec<ToolInfo>>;

    // === Child Agents ===

    /// Spawn a child agent. Requires SpawnAgent capability.
    /// Child receives a subset of parent's capabilities.
    async fn spawn_agent(
        &self,
        manifest: &AgentManifest,
        capabilities: &[Capability],
    ) -> Result<AgentId>;

    // === Preferences ===

    /// Read user preferences relevant to this agent.
    /// Requires PreferenceRead capability.
    async fn preferences(&self) -> Result<PreferenceSet>;

    // === Identity ===

    /// Get the current user's identity (public info only).
    /// Requires IdentityRead capability.
    async fn identity(&self) -> Result<PublicIdentity>;

    // === Lifecycle ===

    /// Get this agent's manifest.
    fn manifest(&self) -> &AgentManifest;

    /// Get this agent's ID.
    fn agent_id(&self) -> AgentId;

    /// Request graceful shutdown.
    async fn shutdown(&self) -> Result<()>;
}
```

### 5.3 Tool System

Agents can register tools — named operations with typed parameters that other agents or AIRS can call. Tools are the mechanism for agent-to-agent cooperation.

```rust
pub struct ToolDefinition {
    /// Tool name (unique within this agent)
    name: String,
    /// Human-readable description (used by AIRS for tool selection)
    description: String,
    /// JSON Schema describing the parameters
    parameters: JsonSchema,
    /// What capability the caller needs to invoke this tool
    capability_required: Option<Capability>,
    /// The handler function
    handler: Box<dyn ToolHandler>,
}

pub struct ToolInfo {
    /// Agent that provides this tool
    provider: AgentId,
    /// Tool name
    name: String,
    /// Description
    description: String,
    /// Parameter schema
    parameters: JsonSchema,
    /// Required capability to call
    capability_required: Option<Capability>,
}

#[async_trait]
pub trait ToolHandler: Send + Sync {
    async fn invoke(&self, params: Value, ctx: &dyn AgentContext) -> Result<Value>;
}
```

**Tool call flow:**

```
Agent A wants to call "pdf-extract" tool on Agent B:

1. Agent A: ctx.call_tool("pdf-extract", params)
2. SDK serializes call, sends IPC to Agent Runtime
3. Agent Runtime:
   a. Looks up "pdf-extract" in tool registry
   b. Finds provider: Agent B
   c. Checks: does Agent A hold capability to communicate with Agent B?
   d. Checks: does Agent A hold the capability Agent B requires for this tool?
   e. If both pass: forwards call via IPC to Agent B
4. Agent B's SDK receives IPC, deserializes, calls handler
5. Handler returns result
6. Result flows back through IPC to Agent A
7. Agent A receives typed result
```

**Example: PDF extraction tool**

```rust
#[agent(
    name = "PDF Tools",
    capabilities = [ReadSpace("documents")],
)]
async fn pdf_tools(ctx: AgentContext) -> Result<()> {
    ctx.register_tool(ToolDefinition {
        name: "pdf-extract".into(),
        description: "Extract text and metadata from a PDF document".into(),
        parameters: json_schema!({
            "object_id": { "type": "string", "description": "ObjectId of the PDF" },
            "pages": { "type": "string", "description": "Page range, e.g. '1-5'" },
        }),
        capability_required: Some(Capability::ReadSpace("documents".into())),
        handler: Box::new(PdfExtractHandler),
    }).await?;

    // Agent stays alive to handle tool calls
    ctx.run_event_loop().await
}
```

### 5.4 The #[agent] Macro

The `#[agent]` macro generates boilerplate: the entry point, manifest extraction, capability list, event loop setup, and signal handling. The developer writes the agent's logic; the macro writes everything else.

**What the developer writes:**

```rust
use aios_sdk::prelude::*;

#[agent(
    name = "Research Assistant",
    version = "1.0.0",
    description = "Finds and summarizes research papers",
    capabilities = [
        ReadSpace("research"),
        WriteSpace("research"),
        InferenceCpu(Priority::Normal),
        Network(services = ["arxiv.org"]),
    ],
    background = false,
)]
async fn research(ctx: AgentContext) -> Result<()> {
    let query = ctx.conversation().await?.last_user_message()?;
    let papers = ctx.spaces().query("research", &query).await?;
    let summary = ctx.infer()
        .with_context(&papers)
        .prompt("Summarize the key findings")
        .await?;
    ctx.respond(Response::text(summary)).await?;
    Ok(())
}
```

**What the macro generates (simplified):**

```rust
// Generated manifest (compiled into the binary)
#[link_section = ".aios_manifest"]
static MANIFEST: AgentManifest = AgentManifest {
    name: "Research Assistant",
    version: Version::new(1, 0, 0),
    description: "Finds and summarizes research papers",
    bundle_id: "com.aios.research-assistant",
    runtime: RuntimeType::Native,
    requested_capabilities: vec![
        CapabilityRequest {
            capability: Capability::ReadSpace(SpaceId::named("research")),
            justification: "Read research papers",
            required: true,
        },
        // ... remaining capabilities ...
    ],
    // ... remaining fields with defaults ...
};

// Generated entry point
#[no_mangle]
pub extern "C" fn _start() {
    let runtime = SdkRuntime::init(&MANIFEST);
    let ctx = runtime.create_context();

    // Install shutdown handler
    runtime.on_shutdown(|| {
        // Agent gets 5 seconds to clean up
    });

    // Run the agent's async function on the SDK event loop
    runtime.block_on(async {
        match research(ctx).await {
            Ok(()) => runtime.exit(0),
            Err(e) => {
                runtime.log_error(&e);
                runtime.exit(1);
            }
        }
    });
}

// Generated event loop (handles IPC dispatch, timers, signals)
impl SdkRuntime {
    fn event_loop(&self) {
        loop {
            match syscall::ipc_select(&self.channels, self.next_timer()) {
                IpcMessage(channel, msg) => self.dispatch(channel, msg),
                Timer(id) => self.fire_timer(id),
                Shutdown => break,
            }
        }
    }
}
```

### 5.5 Event Model

Agents are event-driven. The SDK event loop waits on IPC channels and delivers typed events to the agent:

```rust
pub enum AgentEvent {
    // === IPC ===

    /// A message was received on an IPC channel
    MessageReceived {
        channel: ChannelId,
        message: TypedMessage,
    },

    /// A tool call was received from another agent
    ToolCallReceived {
        caller: AgentId,
        tool: String,
        params: Value,
        reply_to: ReplyHandle,
    },

    // === Capabilities ===

    /// A new capability was granted to this agent
    CapabilityGranted {
        capability: Capability,
        token: CapabilityTokenId,
    },

    /// A capability was revoked from this agent
    CapabilityRevoked {
        capability: Capability,
        reason: RevocationReason,
    },

    // === Spaces ===

    /// An object in a mounted space was created, modified, or deleted
    SpaceObjectChanged {
        space: SpaceId,
        object: ObjectId,
        change: ChangeType,
    },

    // === Timers ===

    /// A timer fired
    TimerFired {
        timer_id: TimerId,
    },

    // === Lifecycle ===

    /// The system is requesting this agent to shut down
    ShutdownRequested {
        reason: ShutdownReason,
        deadline: Timestamp,    // must exit by this time
    },

    /// The agent's window gained or lost focus
    FocusChanged {
        focused: bool,
    },

    /// The agent was resumed from paused/suspended state
    Resumed,

    // === User Input ===

    /// A new message from the user in the conversation bar
    UserMessage {
        content: String,
        attachments: Vec<ObjectId>,
    },
}

pub enum ChangeType {
    Created,
    Modified,
    Deleted,
}

pub enum ShutdownReason {
    UserRequested,
    SystemUpdate,
    TaskCompleted,
    ResourcePressure,
}
```

Agents handle events by implementing an event handler or by using the high-level `AgentContext` API, which wraps events in async methods. The SDK never blocks the event loop — long-running work is dispatched to async tasks.

-----

## 6. Language Runtimes

### 6.1 Rust (Native)

Rust agents compile to aarch64 ELF binaries and run directly in the agent process. They use the SDK crate (`aios-sdk`) which provides the `AgentContext` trait, the `#[agent]` macro, and syscall wrappers.

**Characteristics:**
- Zero overhead. SDK methods compile to direct syscall invocations and IPC messages.
- Full control over memory layout and allocation.
- Can use `unsafe` for performance-critical paths (subject to AIRS audit).
- Best for: system agents, performance-critical agents, agents processing large data.

```rust
use aios_sdk::prelude::*;

#[agent(
    name = "Image Optimizer",
    capabilities = [ReadSpace("photos"), WriteSpace("photos")],
)]
async fn optimize(ctx: AgentContext) -> Result<()> {
    let images = ctx.spaces().query("photos",
        SpaceQuery::filter().content_type(ContentType::Image)
    ).await?;

    for img in images {
        let data = ctx.spaces().read("photos", img.id).await?;
        let optimized = compress_image(&data)?;    // pure Rust, no overhead
        ctx.spaces().write("photos", img.id, optimized).await?;
    }

    ctx.respond(Response::text(format!("Optimized {} images", images.len()))).await
}
```

### 6.2 Python

Python agents run in an embedded Python interpreter within the agent process. On AIOS, the interpreter is RustPython (pure Rust, no C dependencies). On the portable development toolkit, CPython is used for compatibility.

**Characteristics:**
- PyO3 bindings expose `AgentContext` as a native Python module.
- Restricted stdlib: `os.system()`, `subprocess`, `socket`, `ctypes` are removed. These bypass the capability system.
- Package management: dependencies declared in manifest, installed into agent-local `site-packages/` at install time. No pip at runtime.
- Best for: AI/ML agents, data processing, rapid prototyping, agents leveraging Python ML libraries.

```python
from aios_sdk import agent, AgentContext, SpaceQuery

@agent(
    name="Paper Analyzer",
    capabilities=["ReadSpace('research')", "WriteSpace('research')", "InferenceCpu(Normal)"],
)
async def analyze(ctx: AgentContext):
    papers = await ctx.spaces().query("research",
        SpaceQuery.text_search("machine learning")
    )

    for paper in papers:
        content = await ctx.spaces().read("research", paper.id)
        analysis = await ctx.infer().prompt(
            f"Analyze the methodology of this paper:\n{content}"
        )
        await ctx.spaces().write("research", paper.id,
            metadata={"analysis": analysis}
        )

    await ctx.respond(f"Analyzed {len(papers)} papers")
```

**Restricted stdlib modules:**

| Module | Status | Reason |
|---|---|---|
| `os.system`, `os.exec*` | Removed | Bypasses sandbox |
| `subprocess` | Removed | Bypasses sandbox |
| `socket` | Removed | Bypasses network capability gate |
| `ctypes`, `cffi` | Removed | Allows arbitrary native code |
| `multiprocessing` | Removed | Bypasses process isolation |
| `os.path`, `os.getcwd` | Redirected | Routes through space API |
| `open()` (builtin) | Redirected | Routes through space API |
| `importlib` | Restricted | Only agent-local packages |

### 6.3 TypeScript

TypeScript agents run in an embedded JavaScript runtime. On AIOS, QuickJS (small, embeddable, pure C) is used. On the portable toolkit, V8 is used for full compatibility.

**Characteristics:**
- napi-like bridge exposes `AgentContext` as TypeScript-native APIs.
- Node.js standard library is not available. Agents use the AIOS SDK.
- `fetch()` is redirected through the Network Translation Module (respects capability gates).
- Best for: web-adjacent agents, agents written by web developers, UI-heavy agents.

```typescript
import { agent, AgentContext } from "aios-sdk";

agent({
    name: "Bookmark Manager",
    capabilities: [
        "ReadSpace('bookmarks')",
        "WriteSpace('bookmarks')",
        "Network(services=['*'])",  // read any URL for bookmark metadata
    ],
}, async (ctx: AgentContext) => {
    const url = (await ctx.conversation()).lastUserMessage();

    // fetch() goes through NTM — capability-gated, audited
    const page = await fetch(url);
    const title = extractTitle(await page.text());

    await ctx.spaces().create("bookmarks", {
        contentType: "WebPage",
        content: { url, title, savedAt: Date.now() },
    });

    await ctx.respond(`Saved: ${title}`);
});
```

### 6.4 WASM

WASM agents are compiled to WebAssembly modules and run in wasmtime. They interact with the system through a WASI-like interface that maps to AIOS syscalls.

**Characteristics:**
- Language-agnostic: any language that compiles to WASM works (Rust, C, Go, AssemblyScript).
- Most constrained runtime: only WASI imports, no direct syscall access, no shared memory.
- Double-sandboxed: WASM sandbox inside the agent sandbox.
- Best for: untrusted third-party plugins, agents from unknown authors, sandboxed computation.

**Pre-compilation:** WASM modules are compiled to native aarch64 code at install time (wasmtime AOT). This eliminates JIT compilation at startup and meets the 50ms startup target.

### 6.5 Runtime Abstraction

All language runtimes present the same `AgentContext` API through a common abstraction layer:

```rust
pub trait RuntimeAdapter: Send + Sync {
    /// Initialize the runtime (load interpreter, JIT, etc.)
    fn init(&mut self, manifest: &AgentManifest) -> Result<()>;

    /// Load the agent's code
    fn load(&mut self, code: &[u8]) -> Result<()>;

    /// Create an AgentContext bridge for this runtime
    fn create_context(&self, channels: &ChannelSet) -> Box<dyn AgentContext>;

    /// Start the agent's event loop
    fn run(&mut self, ctx: Box<dyn AgentContext>) -> Result<AgentResult>;

    /// Signal shutdown
    fn shutdown(&mut self, deadline: Timestamp);

    /// Runtime type identifier
    fn runtime_type(&self) -> RuntimeType;
}

/// Concrete implementations
pub struct NativeRuntime;     // direct execution, no interpreter
pub struct PythonRuntime;     // RustPython or CPython
pub struct TypeScriptRuntime; // QuickJS or V8
pub struct WasmRuntime;       // wasmtime
```

**Security equivalence:** All runtimes enforce the same capability model. A Python agent cannot do anything a Rust agent cannot do (and vice versa, given the same capabilities). The runtime is an implementation detail. The capability set is the security boundary.

-----

## 7. Agent Communication

### 7.1 IPC Patterns

All inter-agent communication uses kernel IPC. Four patterns cover all use cases:

**Request/Response (synchronous):**
```rust
// Agent A calls Space Storage to read an object
let reply = ipc_call(space_channel, SpaceRequest::Read {
    space: my_space,
    object: doc_id,
})?;
// Agent A blocks until Space Storage replies
```

**Fire-and-Forget (asynchronous):**
```rust
// Agent sends a log event — doesn't wait for confirmation
ipc_send(audit_channel, AuditEvent::AgentAction {
    action: "processed document",
    object: doc_id,
});
// Agent continues immediately
```

**Streaming (chunked transfer):**
```rust
// AIRS sends inference tokens one at a time
loop {
    let token = ipc_recv(airs_channel)?;
    match token {
        AirsReply::Token { text, finished: false } => output.push_str(&text),
        AirsReply::Token { text, finished: true } => {
            output.push_str(&text);
            break;
        }
        AirsReply::Error(e) => return Err(e),
    }
}
```

**Pub/Sub (notifications):**
```rust
// Agent subscribes to space changes
ipc_send(notification_channel, NotificationRequest::Subscribe {
    service: ServiceId::SpaceStorage,
    filter: NotificationFilter::SpaceChanged {
        space: my_space,
        object: None,  // all objects
    },
});

// Later, in event loop:
match ipc_select(&[work_channel, notification_channel], timeout)? {
    (notification_channel, msg) => {
        let change: SpaceChanged = deserialize(msg)?;
        handle_change(change);
    }
    // ...
}
```

### 7.2 Service Discovery

Agents find services through well-known endpoints and the Agent Runtime registry:

**System services** have well-known channel names assigned at boot:

| Service | Channel | Capability Required |
|---|---|---|
| Space Storage | `sys.spaces` | `ReadSpace(*)` or `WriteSpace(*)` |
| AIRS | `sys.airs` | `InferenceCpu(*)` or `InferenceGpu(*)` |
| Compositor | `sys.compositor` | `Display(*)` |
| Network Translation Module | `sys.network` | `Network(*)` |
| Agent Runtime | `sys.agents` | (always available to agents) |
| Attention Manager | `sys.attention` | `AttentionPost(*)` |
| Flow Service | `sys.flow` | `FlowRead` or `FlowWrite` |
| Preference Service | `sys.preferences` | `PreferenceRead` or `PreferenceWrite` |
| Identity Service | `sys.identity` | `IdentityRead` |

**Third-party agents** register in the Agent Runtime's tool and service registry. Agents discover each other through `ctx.list_tools()` or by querying the Agent Runtime.

**Conversation Bar** routes user intent to agents. When the user says "summarize this PDF," the Conversation Bar (via AIRS) identifies the intent, finds an agent with PDF capabilities, and routes the request.

### 7.3 Agent-to-Agent Communication

Direct agent-to-agent communication requires:
1. Both agents must be running.
2. The calling agent must hold a capability that the target agent accepts.
3. The Agent Runtime mediates the connection (creates an IPC channel pair).
4. All messages are logged to the audit space.

Agents cannot discover or communicate with arbitrary other agents. The Agent Runtime acts as a broker — it creates channels only between agents that have compatible capabilities and declared communication intent.

For high-level data transfer between agents, the Flow system provides context-aware data passing without explicit IPC channel management.

### 7.4 Message Format

At the kernel level, IPC messages are untyped byte buffers. The SDK provides typed wrappers:

```rust
/// Kernel-level IPC message (untyped)
pub struct IpcMessage {
    /// Sending agent
    sender: AgentId,
    /// Receiving agent (or service)
    receiver: AgentId,
    /// Message payload (serialized)
    payload: *const u8,
    payload_len: usize,
    /// Capability tokens transferred with this message
    capability_transfers: Vec<CapabilityTokenId>,
    /// Shared memory regions transferred with this message
    shared_memory: Vec<SharedMemoryId>,
}

/// SDK-level typed message (serialized to/from IpcMessage)
pub struct TypedMessage<T: Serialize + Deserialize> {
    pub header: MessageHeader,
    pub payload: T,
}

pub struct MessageHeader {
    /// Operation identifier (enum discriminant)
    message_type: u32,
    /// Sequence number for matching requests to replies
    sequence: u32,
    /// Message flags
    flags: MessageFlags,
}

pub struct MessageFlags {
    /// This message expects a reply
    expects_reply: bool,
    /// This is a reply to a previous message
    is_reply: bool,
    /// This message contains capability transfers
    has_capabilities: bool,
    /// This message references shared memory
    has_shared_memory: bool,
}
```

Messages over 256 bytes use shared memory for zero-copy transfer. The SDK handles this transparently — the developer works with typed messages and the SDK decides whether to inline or use shared memory.

-----

## 8. Agent Store

### 8.1 Distribution Model

Three distribution channels:

**Centralized Agent Store.** The primary distribution channel. Agents are submitted by developers, analyzed by AIRS, reviewed (for sensitive capabilities), signed by the store, and published. Users discover agents through the Conversation Bar or Store UI.

**Sideloading (dev mode).** Developers load agents directly via `aios agent dev`. The agent runs in a sandboxed test environment. Not available on production devices without developer mode enabled.

**Enterprise private stores.** Organizations host internal agents on private stores. Managed by IT policy. Agents are pre-approved by the organization's administrator.

### 8.2 Package Format

An `.aios-agent` package is a signed archive containing everything needed to install and run an agent:

```
research-assistant-1.0.0.aios-agent
├── manifest.toml           ← Agent manifest (human-readable)
├── signature.ed25519        ← Ed25519 signature of manifest + code
├── code/                    ← Code bundle
│   ├── agent.elf           ← (Native) compiled binary
│   ├── agent.py            ← (Python) entry point
│   ├── agent.ts            ← (TypeScript) entry point
│   └── agent.wasm          ← (WASM) compiled module
├── assets/                  ← Icons, images, static data
│   └── icon.png
└── deps/                    ← Bundled dependencies
    ├── lib1.whl             ← (Python) wheel packages
    └── lib2.js              ← (TypeScript) libraries
```

**Example manifest.toml:**

```toml
[agent]
name = "Research Assistant"
version = "1.0.0"
bundle_id = "com.example.research-assistant"
description = "Finds and summarizes research papers from arxiv"
runtime = "python"
min_os_version = "0.1.0"

[agent.author]
name = "Jane Developer"
identity = "did:aios:abc123"

[agent.lifecycle]
autostart = false
persistent = false
background = false

[agent.resources.memory]
minimum = "16MB"
recommended = "64MB"
maximum = "256MB"

[agent.resources.cpu]
priority = "normal"
pattern = "bursty"

[[capabilities]]
type = "ReadSpace"
space = "research"
justification = "Read research papers and notes"
required = true

[[capabilities]]
type = "WriteSpace"
space = "research"
justification = "Save summaries and analysis"
required = true

[[capabilities]]
type = "InferenceCpu"
priority = "normal"
justification = "Summarize and analyze paper content"
required = true

[[capabilities]]
type = "Network"
services = ["arxiv.org"]
justification = "Search and download papers from arxiv"
required = false

[dependencies]
aios-sdk = "0.1"
```

### 8.3 Review Process

```
Developer submits .aios-agent to Store
          │
          ▼
┌─────────────────────────────────┐
│  Automated Analysis              │
│                                   │
│  1. Signature verification       │
│  2. AIRS code analysis           │
│     - Capability usage audit     │
│     - Unused capability flags    │
│     - Suspicious patterns        │
│     - Dependency vulnerability   │
│  3. Resource profiling           │
│     - Run in sandbox, measure    │
│     - Memory, CPU, network       │
│  4. Capability audit             │
│     - Are requested caps minimal?│
│     - Do caps match description? │
└────────────┬────────────────────┘
             │
             ▼
    ┌──── Risk Level? ────┐
    │                      │
  Low/Med              High/Critical
    │                      │
    ▼                      ▼
 Auto-approve       Manual review
    │               (human reviewer)
    │                      │
    ▼                      ▼
  Published            Approve/Reject
    │                      │
    ▼                      ▼
  Store signs with      Published
  Store key              (or feedback
  Available to users     to developer)
```

**Signing chain:**

```
Author signs manifest → Store verifies → Store signs package → User verifies

User sees:
  "Research Assistant v1.0.0"
  "By: Jane Developer (verified identity)"
  "Reviewed: AIOS Store (automated + manual)"
  "Risk: Low"
```

### 8.4 Discovery

Users find agents through multiple channels:

**Conversation Bar.** "I need help organizing my research." → AIRS identifies intent → suggests agents with research capabilities → user approves → agent installed and launched.

**Store UI.** Browse by category, search by name or description, view ratings and reviews, inspect capability requests before installing.

**Recommendations.** Based on installed agents, active spaces, and usage patterns. "You use the code editor frequently — the Code Review Agent might help."

-----

## 9. Resource Management

### 9.1 Memory

Each agent has a declared memory limit (`MemoryRequirements.maximum`). The kernel tracks resident set size (RSS) per agent process.

```
Agent RSS tracking:

  RSS < recommended     → normal operation
  RSS > recommended     → Agent Runtime warns agent via event
  RSS > maximum         → Agent paused, user notified
                           "Research Assistant is using too much memory (256MB).
                            Pause it or increase its limit?"
  RSS > maximum + 25%   → Agent terminated (OOM kill)
```

**Shared memory accounting.** When two agents share a memory region (e.g., compositor shared buffers), the memory is split-charged: each agent is charged `region_size / sharing_agent_count`. This prevents agents from inflating their apparent usage by sharing.

### 9.2 CPU

The scheduler assigns CPU time based on agent priority and quota:

```rust
pub struct CpuAccounting {
    /// CPU time consumed in current window
    used: Duration,
    /// CPU time limit per window
    limit: Duration,
    /// Window duration (e.g., 100ms)
    window: Duration,
    /// Priority class (affects scheduling weight)
    priority: SchedulingPriority,
}
```

| Priority | Scheduling Weight | Use Case |
|---|---|---|
| Realtime | Highest, preemptive | Compositor, audio processing |
| Interactive | High, fair-share | User-facing agents with active windows |
| Normal | Medium, fair-share | Background work agents |
| Idle | Lowest, only when idle | Indexing, sync, maintenance |

**Context-aware scheduling.** The Context Engine provides hints to the scheduler. During gaming, the game agent gets `Interactive` priority while background agents drop to `Idle`. During a video call, the call agent gets `Realtime` while other agents get `Normal`.

### 9.3 Network

Per-agent network usage is tracked by the Network Translation Module. Each agent's space operations are accounted:

```rust
pub struct NetworkAccounting {
    agent: AgentId,
    bytes_sent: u64,
    bytes_received: u64,
    active_connections: u32,
    bandwidth_limit: Option<u64>,   // bytes per second, if set
}
```

**Metered connection awareness.** On cellular or metered WiFi, the NTM restricts background agents to zero network usage. Only interactive agents and user-initiated requests are allowed. The agent receives `SpaceError::Unavailable { retry_after }` and handles it like any other error.

### 9.4 Inference

AIRS inference is a scarce resource. Per-agent inference quotas prevent any single agent from monopolizing the LLM:

```
Priority queue:
  1. Interactive — user waiting for response (Conversation Bar)
  2. Active task — agent working on a task the user initiated
  3. Background — indexing, pre-computation, speculative inference
  4. Batch — large-scale processing, no urgency

Per-agent limits:
  - Max concurrent inference requests: 2
  - Max tokens per request: configurable (default 4096)
  - Rate limit: configurable per priority tier
```

### 9.5 Resource Accounting

The Agent Runtime maintains comprehensive resource statistics per agent:

```rust
pub struct ResourceStats {
    /// Memory usage
    memory_rss: usize,                  // current RSS in bytes
    memory_peak: usize,                 // peak RSS since start
    memory_shared: usize,              // shared memory regions

    /// CPU usage
    cpu_user: Duration,                 // user-mode CPU time
    cpu_system: Duration,              // kernel-mode CPU time (IPC, syscalls)

    /// IPC usage
    ipc_messages_sent: u64,
    ipc_messages_received: u64,
    ipc_bytes_transferred: u64,

    /// Network usage (from NTM)
    network_bytes_sent: u64,
    network_bytes_received: u64,
    network_connections: u32,

    /// Inference usage (from AIRS)
    inference_requests: u64,
    inference_tokens_generated: u64,
    inference_time: Duration,

    /// Space usage
    space_reads: u64,
    space_writes: u64,
    space_bytes_written: u64,

    /// Uptime
    started_at: Timestamp,
    total_active_time: Duration,
    total_paused_time: Duration,
}
```

These statistics are exposed in the Inspector. Users can see exactly what each agent is doing: how much memory it uses, how many network requests it makes, how much inference time it consumes. Full transparency.

-----

## 10. Testing and Development

### 10.1 Development Mode

```
$ aios agent dev ./my-agent/

  1. Reads manifest.toml from project directory
  2. Creates sandboxed test space (isolated from production)
  3. Pre-populates test space with synthetic data
  4. Builds and loads agent into test environment
  5. Watches for file changes → hot-reloads agent
  6. Shows live logs, resource usage, IPC trace
  7. Agent cannot access production spaces
  8. Ctrl+C → agent terminated, test space cleaned up
```

**Hot-reload:** When a source file changes, the dev server:
1. Rebuilds the agent (< 2s for Rust incremental, instant for Python/TS).
2. Sends `ShutdownRequested` to the running agent.
3. Launches the new build with the same test space.
4. State in the test space persists across reloads.

**Mock services:** The dev environment provides mock implementations of system services:
- Mock AIRS: returns canned responses, configurable via test fixtures.
- Mock NTM: records network requests, returns fixture data.
- Mock Compositor: headless rendering, screenshot capture for UI tests.

### 10.2 Testing Framework

```
$ aios agent test ./my-agent/

  1. Discovers test files (test_*.py, *_test.rs, *.test.ts)
  2. Creates isolated test space per test
  3. Runs tests with full capability simulation
  4. Reports: pass/fail, coverage, performance profile
```

```rust
#[cfg(test)]
mod tests {
    use aios_sdk::testing::*;

    #[aios_test]
    async fn test_paper_analysis(ctx: TestContext) {
        // Arrange: populate test space with a sample paper
        ctx.spaces().create_test_object("research", TestData::pdf("sample.pdf")).await;

        // Mock AIRS inference
        ctx.mock_infer().returning(|prompt| {
            assert!(prompt.contains("methodology"));
            "The paper uses a transformer-based architecture...".into()
        });

        // Act: run the agent
        let result = ctx.run_agent(analyze).await;

        // Assert
        assert!(result.is_ok());
        let analysis = ctx.spaces().read("research", "sample").await;
        assert!(analysis.metadata["analysis"].contains("transformer"));
    }
}
```

**Integration testing:** Test agents can be run against real system services in a sandboxed environment. The test framework creates ephemeral spaces, grants test capabilities, and cleans up after each test.

**Performance profiling:** The test framework measures startup time, memory usage, IPC latency, and CPU consumption. Regression alerts fire if performance degrades beyond configured thresholds.

### 10.3 Audit Tool

```
$ aios agent audit ./my-agent/

  Agent: Research Assistant v1.0.0
  Author: Jane Developer

  Capability Analysis:
    ✓ ReadSpace("research")  — used in agent.py:12, agent.py:45
    ✓ WriteSpace("research") — used in agent.py:38
    ✓ InferenceCpu(Normal)   — used in agent.py:28
    ⚠ Network(arxiv.org)     — declared but not used in code
      Suggestion: remove if not needed, or add network calls

  Security Scan:
    ✓ No use of restricted stdlib modules
    ✓ No dynamic code execution (eval, exec)
    ✓ No file system access outside space API
    ⚠ Unbounded loop at agent.py:22 — could consume CPU quota
      Suggestion: add iteration limit or progress check

  AIRS Code Review:
    Risk Level: Low
    Summary: Agent reads papers, calls inference for summarization,
    writes results. Capability usage is appropriate and minimal.
    No security concerns detected.

  Resource Profile (from test run):
    Memory: 42 MB peak (limit: 256 MB) ✓
    CPU: 1.2s per paper (bursty pattern) ✓
    Startup: 38ms ✓
```

### 10.4 Portable Development

Developers build agents on macOS or Linux using the portable toolkit. The same agent code runs on the host OS (for development) and on AIOS (for production).

```
Development machine (macOS/Linux):
  ├── aios-sdk (cargo crate / pip package / npm package)
  ├── aios-cli (agent dev, test, audit, publish commands)
  ├── QEMU + AIOS image (for full-system testing)
  └── Portable UI toolkit (iced on wgpu, same API as AIOS)

Development loop:
  1. Edit agent code on host OS
  2. `aios agent dev` → runs on host with mock services
  3. `aios agent test` → unit and integration tests on host
  4. `aios agent dev --qemu` → runs in QEMU with real AIOS
  5. `aios agent audit` → static analysis and AIRS review
  6. `aios agent publish` → package, sign, submit to Store
```

The SDK abstracts platform differences. `ctx.spaces().query()` hits a SQLite database on the host and the real Space Storage service on AIOS. `ctx.infer()` calls an HTTP API on the host and the in-process AIRS on AIOS. The agent code is identical.

-----

## 11. Security Integration

Agents interact with all eight layers of the AIOS security model. Every agent action passes through every layer.

### 11.1 Layer 1: Intent Verification

AIRS compares the agent's observed actions against its declared purpose. An agent that declares "I read research papers and summarize them" but starts writing to the user's personal space triggers an alert.

**Agent-specific enforcement:**
- The manifest `description` field is the declared intent.
- AIRS builds a behavioral model from the manifest and code analysis.
- At runtime, AIRS periodically compares actual IPC patterns against the model.
- Anomalies trigger alerts (not immediate termination — the user decides).

### 11.2 Layer 2: Capability Check

Every IPC message, every space operation, every subsystem access requires a capability token. The kernel validates the token before dispatching the operation.

**Agent-specific enforcement:**
- Capabilities are granted at install time based on manifest and user approval.
- The set of capabilities is fixed for the agent's lifetime (until update).
- No runtime capability escalation. An agent cannot request new capabilities without a manifest update and user re-approval.
- Capabilities can be attenuated (restricted) but never amplified.

### 11.3 Layer 3: Behavioral Boundary

The Agent Runtime maintains a behavioral baseline for each agent: typical IPC rate, typical memory usage, typical space access patterns.

**Agent-specific enforcement:**
- Baseline built during first N runs (configurable, default 10).
- Deviations beyond 3 standard deviations trigger AIRS review.
- Rate limits enforced per agent: max IPC messages/second, max space writes/second.
- Example: an agent that normally sends 10 IPC messages/second suddenly sending 10,000/second is flagged.

### 11.4 Layer 4: Security Zone

Agents access spaces in specific security zones. Zone transitions require review.

**Agent-specific enforcement:**
- Third-party agents default to `Untrusted` zone access.
- Accessing `Personal` zone requires explicit user approval per space.
- System agents can access `Core` zone. No third-party agent can.
- An agent granted `ReadSpace("research")` where "research" is in `Personal` zone — this is a capability check AND a zone check.

### 11.5 Layer 5: Adversarial Defense

Agent instructions come from the kernel, never from data objects. This prevents prompt injection from escalating through the agent system.

**Agent-specific enforcement:**
- AIRS inference requests carry a `source` tag: `kernel` (trusted) or `data` (untrusted).
- System prompts and agent instructions are `kernel`-sourced.
- User documents, web content, and external data are `data`-sourced.
- AIRS applies input screening to `data`-sourced content before processing.
- An agent cannot modify its own system prompt at runtime.

### 11.6 Layer 6: Cryptographic Enforcement

Agents access encrypted spaces only if the OS releases the decryption key. The key is released after intent verification (Layer 1) and capability check (Layer 2).

**Agent-specific enforcement:**
- Agents never see encryption keys. The Space Storage service handles encryption/decryption.
- An agent with `ReadSpace("personal/documents")` receives plaintext content via IPC — but only after the OS has verified the agent's capability and the space key has been unlocked by the user's identity.
- If the user's identity is locked (screen lock), encrypted spaces are inaccessible to all agents.

### 11.7 Layer 7: Provenance Recording

Every agent action is logged to a tamper-evident Merkle chain. The agent cannot prevent or modify logging.

**Agent-specific enforcement:**
- Space writes record: `(object_id, agent_id, timestamp, action, signature)`.
- IPC messages record: `(sender, receiver, message_type, timestamp)`.
- Tool calls record: `(caller, provider, tool_name, timestamp, result_status)`.
- The provenance chain is append-only. The kernel enforces this. Even a compromised agent cannot alter its audit trail.

### 11.8 Layer 8: Blast Radius Containment

Even if all other layers fail, the damage an agent can cause is bounded.

**Agent-specific enforcement:**
- Maximum objects writable per hour (configurable, default 1000).
- Automatic space snapshot before bulk operations (>10 writes in <1 second).
- Rollback window: changes made by any agent are reversible for a configurable period (default 24 hours).
- Memory limit prevents a single agent from OOM-killing the system.
- CPU quota prevents a single agent from starving others.
- An agent cannot spawn unlimited child agents (spawn count is capped by capability).

-----

## 12. Implementation Order

Agent support is built incrementally across multiple development phases. Each phase delivers independently testable functionality.

```
Phase 1-3: Foundation
  ├── Phase 2:  Process manager — create isolated address spaces (TTBR0)
  ├── Phase 3a: IPC — synchronous message passing between processes
  ├── Phase 3b: Capability system — kernel-managed, unforgeable tokens
  └── Phase 3c: Shared memory — zero-copy data transfer

Phase 7: Basic Agent Model
  ├── AgentProcess struct, basic lifecycle (start/stop)
  ├── Agent Runtime service (manages running agents)
  ├── Manifest parsing (minimal: name, code, capabilities)
  └── Capability grant/revoke at agent level

Phase 8: AIRS Integration
  ├── AIRS security analysis of agent code
  ├── Intent verification (Layer 1) — basic behavioral comparison
  └── SecurityAnalysis attached to manifests

Phase 10: Full Agent Framework
  ├── Complete AgentManifest (all fields)
  ├── Agent states (Active, Paused, Suspended, Background, etc.)
  ├── Agent SDK — AgentContext trait, #[agent] macro
  ├── Tool system — register, discover, call
  ├── Rust runtime (native agents)
  ├── Event model — AgentEvent enum, event loop
  ├── Agent-to-agent IPC mediation
  └── Resource accounting (ResourceStats)

Phase 11: Tasks, Flow & Attention
  ├── Task agents — ephemeral agents for user intents
  ├── Flow integration — FlowPush/FlowPull syscalls
  └── Attention posting from agents

Phase 12: Developer Experience & SDK
  ├── `aios agent dev` — development mode with hot-reload
  ├── `aios agent test` — testing framework with mocks
  ├── `aios agent audit` — static analysis and AIRS review
  ├── `aios agent publish` — packaging and signing
  ├── Python runtime (RustPython + PyO3 bindings)
  ├── TypeScript runtime (QuickJS + bridge)
  └── WASM runtime (wasmtime)

Phase 13: Security Hardening
  ├── Behavioral baseline and anomaly detection (Layer 3)
  ├── Adversarial defense for agent inference (Layer 5)
  ├── Blast radius containment (Layer 8)
  └── Full 8-layer security integration

Phase 21: Browser (Tab Agents)
  ├── Tab agents — per-origin browser tab isolation
  ├── Service worker agents — persistent background web agents
  └── Web API capability mapping

Phase 26: Agent Store
  ├── .aios-agent package format
  ├── Store submission and review pipeline
  ├── Automated AIRS analysis at scale
  ├── Enterprise private stores
  └── Discovery and recommendation engine
```

**Critical dependencies:**
- Agents require IPC (Phase 3) — agents cannot do anything without IPC.
- Agent SDK requires Space Storage (Phase 4) — `ctx.spaces()` needs a backend.
- Agent analysis requires AIRS (Phase 8) — `SecurityAnalysis` needs inference.
- Tab agents require browser shell (Phase 21) — tab agent lifecycle is browser-managed.
- Agent Store requires network (Phase 16) — distribution needs connectivity.

-----

## 13. Design Principles

1. **Agents are processes, not plugins.** Full address space isolation, not in-process sandboxing. A compromised agent cannot corrupt the kernel or other agents.

2. **Capabilities are the security boundary.** The manifest declares what an agent needs. The user approves. The kernel enforces. No runtime escalation. No ambient authority.

3. **Spaces belong to users, not agents.** An agent writes data to the user's spaces. Removing the agent does not remove the data. The user can revoke space access at any time.

4. **All runtimes are security-equivalent.** A Python agent has the same isolation and capability model as a native Rust agent. The runtime is a performance choice, not a security choice.

5. **Transparency over trust.** Every agent action is auditable. The Inspector shows exactly what every agent is doing. Resource usage, IPC traffic, space access, network requests — all visible.

6. **The SDK is generous, the sandbox is strict.** The SDK gives developers easy access to spaces, inference, tools, flow, attention, and preferences. The sandbox ensures they cannot abuse that access.

7. **Developer experience is a feature.** `aios agent dev` with hot-reload, mock services, and live logging. `aios agent test` with space simulation. `aios agent audit` with AIRS code review. Developers should enjoy building for AIOS.

8. **Progressive trust.** System agents are fully trusted. First-party agents are pre-approved. Third-party agents are AIRS-analyzed and user-approved. Tab agents are untrusted and maximally constrained. The trust level matches the provenance.
