# AIOS IPC and Syscall Interface

## Deep Technical Architecture

**Parent document:** [architecture.md](../project/architecture.md)
**Related:** [compositor.md](../platform/compositor.md) — Compositor protocol, [subsystem-framework.md](../platform/subsystem-framework.md) — Subsystem sessions, [memory.md](./memory.md) — Memory management, shared memory regions (§7)

> **Naming note:** This document uses `MemoryFlags` for memory region permissions. This is a type alias for `VmFlags` defined in [memory.md §3.2](./memory.md): `type MemoryFlags = VmFlags;`. Both names refer to the same bitflags type (READ, WRITE, EXECUTE, USER, SHARED, PINNED, HUGE, NO_DUMP).

-----

## 1. Core Insight

A microkernel lives or dies by its IPC performance. In a monolithic kernel (Linux), calling a filesystem operation is a function call — a few nanoseconds. In a microkernel, the same operation is an IPC message to the filesystem service and a reply back — if that takes microseconds instead of nanoseconds, everything is 1000x slower.

AIOS IPC is designed for **sub-5-microsecond round-trip latency**. Every design decision optimizes for this: synchronous message passing, zero-copy transfers via shared memory, capability transfer as a first-class operation, and a minimal syscall interface that the kernel can execute in tens of instructions.

-----

## 2. Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    Agent / Application                    │
│                                                          │
│  SDK provides typed wrappers:                            │
│    spaces::read()  →  syscall(IPC_CALL, space_svc, msg) │
│    audio::play()   →  syscall(IPC_CALL, audio_svc, msg) │
│    compositor::*   →  syscall(IPC_CALL, comp_svc, msg)  │
└──────────────────────┬───────────────────────────────────┘
                       │ syscall trap (SVC instruction)
                       ▼
┌─────────────────────────────────────────────────────────┐
│                    Kernel Syscall Handler                 │
│                                                          │
│  1. Validate syscall number                              │
│  2. Validate parameters (in user address range)          │
│  3. Check capability (for IPC: channel capability)       │
│  4. Dispatch to handler                                  │
│  5. Return result                                        │
└──────────────────────┬───────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────────┐
│                    IPC Subsystem                          │
│                                                          │
│  Channel Manager     Message Router     Capability Xfer  │
│  (create, destroy)   (send, recv)       (transfer tokens)│
│                                                          │
│  Shared Memory Mgr   Notification Mgr   Audit Logger     │
│  (map, unmap, share)  (async signals)   (all IPC logged) │
└─────────────────────────────────────────────────────────┘
```

-----

## 3. Syscall Interface

AIOS has a minimal syscall set. The microkernel provides only what cannot be done in userspace.

### 3.1 Syscall Table

```rust
pub enum Syscall {
    // === IPC (core microkernel operations) ===

    /// Send a message and wait for reply (synchronous).
    /// Timeout is mandatory — prevents indefinite blocking on hung services.
    /// A service that does not reply within the timeout returns ETIMEDOUT.
    /// The kernel cleans up the pending call state on timeout.
    IpcCall {
        channel: ChannelId,
        send_buf: *const u8,
        send_len: usize,
        recv_buf: *mut u8,
        recv_len: usize,
        timeout: Duration,      // SDK type; raw syscall uses timeout_ns: u64 in registers
    },

    /// Send a message without waiting for reply (asynchronous)
    IpcSend {
        channel: ChannelId,
        send_buf: *const u8,
        send_len: usize,
    },

    /// Wait for a message on a channel.
    /// timeout_ns: maximum time to block (nanoseconds).
    /// 0 = non-blocking poll, u64::MAX = block indefinitely.
    /// All blocking IPC operations require a timeout to prevent
    /// indefinite resource lockup.
    IpcRecv {
        channel: ChannelId,
        recv_buf: *mut u8,
        recv_len: usize,
        timeout_ns: u64,
    },

    /// Reply to the last IpcCall received on the current channel.
    /// No channel capability required — the kernel tracks the caller.
    /// Can only be used once per received IpcCall (enforced by kernel).
    /// Saves ~30 cycles per round-trip by skipping reply-path capability
    /// validation. Also prevents misrouted replies structurally.
    IpcReply {
        reply_buf: *const u8,
        reply_len: usize,
    },

    /// Cancel a pending IpcCall. If the caller is blocked in IpcCall,
    /// the call is aborted and returns ECANCELED. If the service already
    /// received the request, a cancellation notification is delivered to
    /// the service (best-effort — the service may have already processed
    /// it). Used by the kernel during process teardown and by agents
    /// that need to abort long-running requests.
    IpcCancel {
        channel: ChannelId,
    },

    /// Wait for a message on any of multiple channels.
    /// On success, ready_channel is set to the channel that has data.
    IpcSelect {
        channels: *const ChannelId,
        channel_count: usize,
        recv_buf: *mut u8,
        recv_len: usize,
        timeout: Option<Duration>,     // SDK type; raw syscall: timeout_ns: u64 (0 = non-blocking, u64::MAX = indefinite)
        ready_channel: *mut ChannelId,
    },

    // === Channel Management ===

    /// Create a new IPC channel pair
    ChannelCreate {
        flags: ChannelFlags,
    },

    /// Destroy a channel endpoint
    ChannelDestroy {
        channel: ChannelId,
    },

    /// Create a shared-memory ring buffer channel for high-frequency,
    /// low-overhead messaging. No SVC trap per entry — producer/consumer
    /// use atomic pointer advance. A lightweight notification (see below)
    /// wakes the consumer when new entries arrive. Suitable for: AIRS
    /// directives, streaming inference tokens, agent telemetry, metrics.
    /// NOT suitable for: request/reply (use IpcCall), capability transfer
    /// (requires kernel mediation), security-sensitive operations.
    RingChannelCreate {
        submission_queue_size: u32,  // entries (power of 2)
        completion_queue_size: u32,
        entry_size: u32,            // max bytes per entry
        flags: RingChannelFlags,    // see RingChannelFlags below
    },

    /// Destroy a ring buffer channel
    RingChannelDestroy {
        ring: RingChannelId,        // see RingChannelId below
    },

    // === Lightweight Notifications (seL4-style bitmap signals) ===

    /// Create a lightweight notification object (single-word bitmap).
    /// No message body. Each bit position is a signal.
    NotificationCreate {},


    /// Signal a notification: atomic OR of bits into the notification word.
    /// ~10 cycles. No message allocation, no queue, no serialization.
    NotificationSignal {
        id: NotificationId,
        bits: u64,
    },

    /// Wait until any bit in mask is set. Returns the set bits and
    /// atomically clears them. Blocks if no bits are set.
    NotificationWait {
        id: NotificationId,
        mask: u64,
    },

    // === Channel Introspection ===

    /// Query channel statistics (latency, throughput, error counts).
    /// Returns ChannelStats. Used by AIRS behavioral monitoring and
    /// by the Inspector agent for debugging.
    ChannelStats {
        channel: ChannelId,
        buf: *mut u8,
        buf_len: usize,
    },

    // === Capability Operations ===

    /// Transfer a capability to another agent via IPC
    CapabilityTransfer {
        channel: ChannelId,
        capability: CapabilityTokenId,
    },

    /// Create a new attenuated capability from an existing one
    CapabilityAttenuate {
        source: CapabilityTokenId,
        restrictions: AttenuationSpec,
    },

    /// Revoke a capability
    CapabilityRevoke {
        capability: CapabilityTokenId,
    },

    /// Query capabilities held by this agent
    CapabilityList {
        buf: *mut CapabilityTokenId,
        buf_len: usize,
    },

    // === Memory Management ===

    /// Allocate virtual memory
    MemoryMap {
        addr: Option<usize>,           // hint or NULL for kernel choice
        size: usize,
        flags: MemoryFlags,            // Read, Write, Execute (W^X enforced)
    },

    /// Free virtual memory
    MemoryUnmap {
        addr: usize,
        size: usize,
    },

    /// Create a shared memory region
    SharedMemoryCreate {
        size: usize,
    },

    /// Map a shared memory region into this address space
    SharedMemoryMap {
        region: SharedMemoryId,
        flags: MemoryFlags,
    },

    /// Transfer shared memory access to another agent via IPC
    SharedMemoryShare {
        region: SharedMemoryId,
        channel: ChannelId,
        flags: MemoryFlags,            // can restrict: read-only share
    },

    // === Process Management ===

    /// Spawn a new process.
    /// resource_limits are mandatory — derived from the agent's trust level
    /// and blast radius policy. The kernel rejects ProcessCreate if limits
    /// are missing or exceed the parent's own limits (zero trust: no
    /// implicit resource authority — see §12.2 Gap 5, security.md §10.3 Gap 4).
    ProcessCreate {
        image: ContentHash,            // content-addressed executable
        capabilities: *const CapabilityTokenId,
        cap_count: usize,
        args: *const u8,
        args_len: usize,
        resource_limits: KernelResourceLimits,
    },

    /// Terminate the calling process
    ProcessExit {
        code: i32,
    },

    /// Wait for a child process to exit
    ProcessWait {
        pid: ProcessId,
        timeout: Option<Duration>,
    },

    // === Time ===

    /// Get current time
    TimeGet {
        clock: ClockId,                // Monotonic, Realtime, ProcessCpu
    },

    /// Sleep for a duration
    TimeSleep {
        duration: Duration,
    },

    /// Set a timer (wakes IpcSelect)
    TimerSet {
        duration: Duration,
        repeat: bool,
    },

    // === Audit ===

    /// Log an audit event (kernel-enforced, tamper-evident)
    AuditLog {
        event: *const u8,
        event_len: usize,
    },

    // === Debug (development only) ===

    /// Print to kernel console (UART)
    DebugPrint {
        msg: *const u8,
        msg_len: usize,
    },
}

/// Error codes returned in x0 (negative values).
#[repr(i32)]
pub enum IpcError {
    ETIMEDOUT    = -1,  // IpcCall timeout elapsed
    EPIPE        = -2,  // peer endpoint is dead
    EAGAIN       = -3,  // queue full (IpcSend) or would block
    ECANCELED    = -4,  // IpcCancel aborted the call
    EACCES       = -5,  // behavioral gate SUSPENDED
    EPERM        = -6,  // missing capability
    ENOSPC       = -7,  // subscriber list full
    EPROTO       = -8,  // message_type not in channel protocol
    ENOTSUP      = -9,  // operation not available (e.g., AIRS offline)
    ECAP_DORMANT = -10, // capability exists but is dormant
}

/// Ring buffer channel identifier (returned by RingChannelCreate).
pub struct RingChannelId(u64);

/// Flags for RingChannelCreate.
pub struct RingChannelFlags(u32);
// bit 0: NONBLOCKING — submission returns immediately even if queue is full
// bit 1: SHARED_MEMORY — map the ring into both processes (for zero-copy)

/// Lightweight notification object identifier (returned by NotificationCreate).
pub struct NotificationId(u64);
```

### 3.2 Syscall ABI

```
Syscall convention (aarch64):
  x8:  syscall number
  x0:  argument 0
  x1:  argument 1
  x2:  argument 2
  x3:  argument 3
  x4:  argument 4
  x5:  argument 5

  SVC #0 instruction triggers trap to EL1

  Struct-pointer convention: syscalls with aggregate or >6 scalar
  parameters (e.g., ProcessCreate, IpcSelect) pass a pointer to a
  packed argument struct in x0. The kernel copies the struct from
  user memory before validation.

  Return:
  x0:  result (0 = success, negative = error code)
  x1:  secondary return value (e.g., bytes transferred)
```

### 3.3 Kernel Resource Limits

Every process has hard limits on kernel object creation. Limits are set at `ProcessCreate` and cannot be increased after creation. A child process cannot exceed its parent's limits (monotonic restriction, same principle as capability attenuation).

```rust
pub struct KernelResourceLimits {
    max_channels: u32,                       // default: 64
    max_shared_regions: u32,                 // default: 32
    max_pending_messages: u32,               // default: 256
    max_notification_subscriptions: u32,     // default: 16
    max_child_processes: u32,                // default: 8
}
```

Defaults by trust level:

| Resource | Level 1 (System) | Level 2 (Native) | Level 3 (Third-party) | Level 4 (Web) |
|---|---|---|---|---|
| `max_channels` | 256 | 128 | 64 | 16 |
| `max_shared_regions` | 128 | 64 | 32 | 8 |
| `max_pending_messages` | 1024 | 512 | 256 | 64 |
| `max_notification_subscriptions` | 64 | 32 | 16 | 4 |
| `max_child_processes` | 32 | 16 | 8 | 0 |

The kernel's total heap usage is bounded by: `sum(per-process limits) * per-object size`. This guarantees that no combination of userspace actions can exhaust the kernel heap.

### 3.4 Syscall Count

Total: 31 syscalls. IPC: 6 (`IpcCall`, `IpcSend`, `IpcRecv`, `IpcReply`, `IpcCancel`, `IpcSelect`). Channels: 4 (`ChannelCreate`, `ChannelDestroy`, `RingChannelCreate`, `RingChannelDestroy`). Notifications: 3 (`NotificationCreate`, `NotificationSignal`, `NotificationWait`). Introspection: 1 (`ChannelStats`). Capabilities: 4 (`CapabilityTransfer`, `CapabilityAttenuate`, `CapabilityRevoke`, `CapabilityList`). Memory: 5 (`MemoryMap`, `MemoryUnmap`, `SharedMemoryCreate`, `SharedMemoryMap`, `SharedMemoryShare`). Process: 3 (`ProcessCreate`, `ProcessExit`, `ProcessWait`). Time: 3 (`TimeGet`, `TimeSleep`, `TimerSet`). Audit: 1 (`AuditLog`). Debug: 1 (`DebugPrint`). DebugPrint is development-only and excluded from production builds.

Compare with Linux (~450) or even seL4 (~12). AIOS targets the sweet spot: enough for a full-featured OS, few enough that every syscall can be audited and fuzz-tested exhaustively.

-----

## 4. IPC Design

### 4.1 Channels

IPC channels are the communication primitive. A channel is a bidirectional pipe between two endpoints:

```rust
pub struct Channel {
    id: ChannelId,
    endpoint_a: ProcessId,
    endpoint_b: ProcessId,
    /// State of each endpoint. When a process dies, the kernel sets its
    /// endpoint to Dead. Any IpcCall/IpcSend/IpcRecv on the peer endpoint
    /// returns EPIPE. Any blocked IpcCall on the peer unblocks with EPIPE.
    /// This is the IPC equivalent of TCP RST — immediate, unambiguous.
    state_a: EndpointState,
    state_b: EndpointState,
    capability: ChannelCapability,
    /// The capability token that authorized this channel's creation.
    /// On revocation, the kernel walks all channels and destroys any whose
    /// creation_capability has been revoked (zero trust: §10.3 Gap 1 in
    /// security.md). This ensures cached channel access cannot outlive
    /// the credential that granted it.
    creation_capability: CapabilityTokenId,
    message_queue: RingBuffer<RawMessage>,
    /// Fixed-size array. Bounded per-channel to prevent kernel heap
    /// exhaustion from userspace (zero trust: §10.3 Gap 4 in security.md).
    shared_regions: [Option<SharedMemoryId>; MAX_SHARED_REGIONS_PER_CHANNEL],
    /// Registered protocol — valid message_type values per direction.
    /// Kernel rejects messages with unregistered types before delivery.
    /// None for untyped channels (e.g., POSIX pipes).
    protocol: Option<ChannelProtocol>,
    /// Per-channel statistics. Updated by kernel on every IPC operation.
    /// Queryable via ChannelStats syscall (§3.1). Feeds AIRS behavioral monitoring.
    stats: ChannelStatsData,
    audit: bool,                        // log all messages?
}

pub enum EndpointState {
    /// Normal operation
    Active,
    /// Process has exited or been killed. Peer gets EPIPE on all operations.
    Dead,
    /// Process suspended by behavioral gating. Peer gets EACCES.
    /// Same error code as §9.1 behavioral gate SUSPENDED state.
    Suspended,
}

pub struct ChannelStatsData {
    messages_sent_a_to_b: u64,
    messages_sent_b_to_a: u64,
    bytes_transferred: u64,
    /// Exponentially weighted moving average of round-trip latency (nanoseconds).
    avg_latency_ns: u32,
    errors: u32,
    /// Number of times the message queue was full (backpressure events).
    backpressure_events: u32,
}

const MAX_SHARED_REGIONS_PER_CHANNEL: usize = 16;

pub struct ChannelProtocol {
    /// Valid message_type values for direction A→B.
    valid_types_a_to_b: [u32; MAX_PROTOCOL_TYPES],
    valid_types_a_to_b_count: u8,
    /// Valid message_type values for direction B→A.
    valid_types_b_to_a: [u32; MAX_PROTOCOL_TYPES],
    valid_types_b_to_a_count: u8,
}

const MAX_PROTOCOL_TYPES: usize = 32;

pub struct ChannelFlags {
    /// Maximum message size
    max_message: usize,
    /// Queue depth for async IpcSend. When the queue is full:
    ///   - IpcSend returns EAGAIN (non-blocking: caller retries or drops).
    ///   - IpcCall is unaffected (synchronous: blocks until reply, with timeout).
    /// This is explicit backpressure — the sender always knows.
    queue_depth: u32,
    /// Should messages be audited?
    audit: bool,
}
```

**Channel creation flow:**
```
1. Service Manager creates service processes at boot
2. Service Manager creates channels: (agent ↔ space_service), (agent ↔ audio_service), etc.
3. Channel endpoints are distributed via capability transfer
4. Agents receive channel capabilities at spawn time
5. All IPC uses these pre-established channels
```

### 4.2 Synchronous IPC (IpcCall)

The primary IPC pattern. Agent sends a request and blocks until the service replies:

```
Agent                           Service
  │                                │
  │ IpcCall(channel, request) ──→  │
  │ (agent thread blocks)         │
  │                                │ (service processes request)
  │                                │
  │ ←── IpcReply(reply)            │
  │ (agent thread resumes)         │
  │                                │
```

**Why synchronous:** Synchronous IPC is simpler, debuggable, and avoids the complexity of asynchronous callback chains. For a microkernel where every system call is an IPC, synchronous is the proven approach (seL4, L4, QNX all use synchronous IPC).

**Latency target:** < 5 microseconds round-trip. This requires:
- No memory allocation in the IPC path
- No context switch overhead (direct thread switch from sender to receiver)
- Message copied once (sender buffer → receiver buffer) or zero-copy via shared memory
- Capability validation cached per-channel (checked at creation, not per-message;
  revocation propagates to channels — see security.md §10.3 Gap 1)

### 4.3 Message Format

Messages are untyped byte buffers at the kernel level. The SDK provides typed wrappers:

```rust
/// Kernel-level message (untyped). All arrays are fixed-size to avoid
/// kernel heap allocation on the IPC hot path.
pub struct RawMessage {
    channel: ChannelId,
    /// Discriminates message type at the kernel level. Used by the
    /// kernel for ChannelProtocol validation (§8.3 level 2, §9.1 step 4).
    message_type: u32,
    data: *const u8,
    len: usize,
    capabilities: [Option<CapabilityTokenId>; MAX_CAPS_PER_MESSAGE],
    cap_count: u8,
    shared_memory: [Option<SharedMemoryId>; MAX_SHARED_PER_MESSAGE],
    shared_count: u8,
}

const MAX_CAPS_PER_MESSAGE: usize = 4;
const MAX_SHARED_PER_MESSAGE: usize = 4;

/// SDK-level typed message (serialized to/from RawMessage)
pub struct TypedMessage<T: Serialize + Deserialize> {
    pub header: MessageHeader,
    pub payload: T,
}

pub struct MessageHeader {
    pub message_type: u32,              // operation identifier
    pub sequence: u32,                  // for matching requests to replies
    pub flags: MessageFlags,
}
```

### 4.4 Zero-Copy Transfers

For large data (file content, audio buffers, display frames), IPC uses shared memory instead of copying:

```
Agent wants to write 1 MB to a space:

Without shared memory (slow):
  Agent: IpcCall(space_svc, [1 MB data])  ← kernel copies 1 MB
  Space: processes, replies
  Round trip: ~500 microseconds (memory copy dominates)

With shared memory (fast):
  Agent: SharedMemoryCreate(1 MB) → region_id
  Agent: write data to shared region (direct memory access)
  Agent: IpcCall(space_svc, {region: region_id, offset: 0, len: 1MB})
  Space: reads from shared region (direct memory access)
  Space: replies
  Round trip: ~10 microseconds (no copy, just pointer exchange)
```

Shared memory regions are mapped into both address spaces. The kernel manages access permissions (read-only or read-write). Shared memory capabilities can be transferred between agents.

**Available to all userland processes.** Any agent (Trust Levels 1-4) can create shared memory regions via `SharedMemoryCreate` and share them via `SharedMemoryShare`. Third-party agents (Level 3) use shared memory for agent-to-agent communication, large data transfers to Space Storage, display buffers to the Compositor, and audio buffers to the Audio Service. The agent SDK provides typed wrappers that handle the create/map/share lifecycle automatically. Developers never need to manually manage shared memory unless they want fine-grained control.

**Agent-to-agent shared memory.** Two agents can share memory if they have an IPC channel between them:
```
Agent A: SharedMemoryCreate(size) → region_id
Agent A: SharedMemoryShare(region_id, channel_to_B, READ_WRITE)
Agent B: receives region_id, calls SharedMemoryMap(region_id, READ_WRITE)
Both agents now read/write the same physical pages.
```
This is the fastest inter-agent data path — no kernel involvement after setup. The kernel enforces that permissions never exceed what was granted: if A shares as read-only, B cannot map as read-write.

**Exception: AIRS.** AIRS does not accept shared memory from agents. All agent→AIRS data is kernel-copied to prevent TOCTOU attacks on adversarial prompts. See §5.2 for details. Other system services (Space Storage, Compositor, Audio) use shared memory normally because they process structured data with known formats, not adversarial natural-language input.

### 4.5 Shared Memory Lifecycle

Shared memory regions are reference-counted by the kernel. The reference count tracks how many processes have the region mapped:

```rust
/// See memory.md §7.1 for the canonical definition.
pub struct SharedMemoryRegion {
    id: SharedMemoryId,
    physical_pages: PageRange,
    /// Reference count: incremented on SharedMemoryMap, decremented on
    /// SharedMemoryUnmap or process death. When it reaches 0, the
    /// physical pages are freed.
    ref_count: AtomicU32,
    /// The process that created the region. Only the creator (or
    /// kernel) can destroy it explicitly.
    creator: ProcessId,
    /// Maximum permissions granted at creation time.
    max_flags: MemoryFlags,
    /// Capability required to access this shared region.
    capability: CapabilityTokenId,
    /// Per-mapping permissions (may be more restrictive than max_flags).
    mappings: [Option<SharedMapping>; MAX_SHARED_MAPPINGS],
}

pub struct SharedMapping {
    process: ProcessId,
    vaddr: VirtualAddress,          // where mapped in this process's address space
    flags: VmFlags,                 // must be subset of max_flags (VmFlags = MemoryFlags alias)
}

const MAX_SHARED_MAPPINGS: usize = 8;  // bounded: no heap growth
```

**Process death cleanup.** When a process dies (exit, kill, OOM):

1. Kernel iterates the process's mapped shared memory regions
2. For each region: unmap from the dying process's page table, decrement `ref_count`
3. If `ref_count` reaches 0: free the physical pages (no other process uses them)
4. If `ref_count` > 0: other mappings remain valid — no use-after-free

**No dangling references.** A process can never access a shared memory region after its mapping is removed. The page table entry is cleared atomically. Even if the physical pages are later freed and reused, the process cannot reach them — its virtual mapping is gone.

### 4.6 Capability Transfer

Capabilities can be transferred through IPC channels. This is how the Service Manager distributes capabilities at boot and how agents delegate capabilities:

```rust
/// Transfer capability to the other end of a channel
fn transfer_capability(channel: ChannelId, cap: CapabilityTokenId) -> Result<()> {
    // Kernel:
    // 1. Verify caller holds the capability
    // 2. Verify capability is marked delegatable
    // 3. Create new token for the receiver (may be attenuated)
    // 4. Remove token from sender (move semantics) or clone it
    // 5. Deliver to receiver's pending capability queue
    syscall(Syscall::CapabilityTransfer { channel, capability: cap })
}
```

**Move vs. clone:** By default, capability transfer is a **move** — the sender no longer holds the capability. For capabilities marked `delegatable: true`, the sender can choose to clone (both hold a copy). This prevents capability amplification.

-----

## 5. Service Protocol

All system services follow the same request/reply protocol. The SDK provides typed bindings:

### 5.1 Space Service Protocol (Space Storage Subsystem)

```rust
pub enum SpaceRequest {
    /// Read an object
    Read { space: SpaceId, object: ObjectId },
    /// Write an object
    Write { space: SpaceId, object: ObjectId, content: SharedMemoryId },
    /// Create an object
    Create { space: SpaceId, content_type: ContentType, content: SharedMemoryId },
    /// Delete an object
    Delete { space: SpaceId, object: ObjectId },
    /// Query objects
    Query { space: SpaceId, query: SpaceQuery },
    /// List objects
    List { space: SpaceId, filter: Option<Filter> },
    /// Get version history
    Versions { space: SpaceId, object: ObjectId },
    /// Rollback to version
    Rollback { space: SpaceId, object: ObjectId, version: Hash },
    /// Create/list/delete spaces
    SpaceCreate { name: String, parent: Option<SpaceId>, zone: SecurityZone },
    SpaceList { parent: Option<SpaceId> },
    SpaceDelete { space: SpaceId },
    /// Snapshot operations (spaces.md §5.2)
    SnapshotCreate { space: SpaceId, trigger: SnapshotTrigger },
    SnapshotRollback { space: SpaceId, snapshot: SnapshotId },
    /// Diff between two versions of an object (spaces.md §5.3)
    Diff { space: SpaceId, object: ObjectId, v1: Hash, v2: Hash },
    /// Space Sync operations (spaces.md §8) — Phase 9c
    SyncStart { space: SpaceId, remote: RemoteSpaceId, policy: SyncPolicy },
    SyncStatus { space: SpaceId },
    SyncCancel { space: SpaceId },
    SyncResolveConflict { space: SpaceId, object: ObjectId, resolution: SyncConflictPolicy },
}

pub enum SpaceReply {
    Object { content: SharedMemoryId, metadata: ObjectMetadata },
    ObjectId { id: ObjectId },
    ObjectList { objects: Vec<ObjectSummary> },
    VersionList { versions: Vec<VersionSummary> },
    SpaceList { spaces: Vec<SpaceSummary> },
    SnapshotId { id: SnapshotId },
    VersionDiff { diff: VersionDiff },              // spaces.md §5.3
    SyncStatus { state: SyncState },                // spaces.md §8
    Ok,
    Error(SpaceError),
}
```

### 5.2 AIRS Service Protocol

**No shared memory for AIRS.** AIRS is a Trust Level 1 system service (security.md §9.3). Agents submit prompts — which are untrusted, potentially adversarial input (security.md §1.1, prompt injection). Mapping an agent's shared memory region directly into AIRS's address space creates a TOCTOU (time-of-check/time-of-use) attack surface: the agent can mutate the shared region while AIRS is reading it. All data crossing the agent→AIRS boundary is **kernel-copied** into AIRS's own memory. This adds ~50 cycles per KB (copy cost) but eliminates the attack vector entirely. AIRS's own security.md §9.3 confirms this principle: AIRS prefetches "use the NORMAL Space Storage read path" — AIRS never maps external data directly.

```rust
pub enum AirsRequest {
    /// Request inference. Prompt is kernel-copied (NOT shared memory)
    /// from the agent's buffer into AIRS's address space. The agent
    /// cannot mutate the prompt after submission.
    Infer {
        prompt_buf: *const u8,
        prompt_len: usize,
        parameters: InferenceParameters,
        priority: InferencePriority,
        /// Ring buffer channel for streaming token delivery (see below).
        /// If None, reply is delivered as a single IpcReply when complete.
        stream_channel: Option<RingChannelId>,
    },
    /// Generate embedding. Content kernel-copied.
    Embed {
        content_buf: *const u8,
        content_len: usize,
    },
    /// Search semantically
    SemanticSearch {
        query_buf: *const u8,
        query_len: usize,
        space: SpaceId,
        limit: u32,
    },
    /// Register a tool
    ToolRegister {
        name_buf: *const u8,
        name_len: usize,
        schema: ToolSchema,
    },
}

pub enum AirsReply {
    /// Non-streaming: full response in one IPC reply. Kernel-copied.
    Complete { response_buf: *const u8, response_len: usize },
    /// Embedding vector. Kernel-copied into agent's buffer.
    Embedding { vector_buf: *const u8, vector_len: usize },
    /// Search results
    SearchResults { results: [SearchResult; MAX_SEARCH_RESULTS], count: u16 },
    Ok,
    Error(AirsError),
}

pub struct SearchResult {
    object: ObjectId,
    score: f32,
}

const MAX_SEARCH_RESULTS: usize = 64;
```

**Streaming inference via ring buffer channel.** Generating tokens via individual `IpcReply` messages means one SVC trap per token (~4 bytes each at ~30ms intervals). For a 500-token response, that's 500 SVC traps. Instead, when `stream_channel` is provided:

1. Agent creates a ring buffer channel (`RingChannelCreate`) before calling `Infer`
2. AIRS writes tokens into the ring buffer (no SVC trap per token — shared-memory write + atomic pointer advance)
3. AIRS signals the agent via lightweight notification when tokens are available
4. Agent reads tokens from the ring buffer in batches
5. AIRS writes a sentinel `STREAM_END` entry when generation is complete
6. Agent destroys the ring buffer channel

This reduces the per-token overhead from ~415 cycles (full IPC) to ~10 cycles (atomic write + pointer advance). For interactive agents that display tokens as they arrive, this is the difference between perceptible latency and invisible overhead.

### 5.3 Compositor Protocol

See [compositor.md](../platform/compositor.md) for the full protocol. The compositor uses the same IPC channel pattern as all other services.

### 5.4 Multi-Client Service Model

System services handle concurrent clients. The model is single-threaded event loop with `IpcSelect`:

```
Service startup:
  1. Service Manager creates per-client channels at boot
  2. Service receives all channel endpoints via capability transfer
  3. Service enters IpcSelect loop over all client channels

Event loop:
  loop {
      let (channel_id, message) = IpcSelect(all_client_channels, timeout)?;
      let reply = handle_request(channel_id, message);
      IpcReply(reply);
      // IpcSelect resumes — next ready channel is serviced
  }
```

**Why single-threaded.** A service handling one request at a time is simpler, has no internal concurrency bugs, and is cache-friendly. The direct thread switch (§9.3) means only one client is active at a time anyway — the kernel switches directly from the client to the service and back. Multi-threading the service only helps if requests are long-running (e.g., inference). For typical IPC latencies (< 5 μs), single-threaded is optimal.

**Exceptions.** AIRS uses an internal thread pool for inference requests (airs.md §2.1). The inference engine runs on dedicated CPU cores (scheduler.md §6). The AIRS main thread accepts requests via `IpcSelect` and dispatches to inference worker threads. The worker threads signal completion via internal synchronization (not IPC). Long-running inference does not block short requests like `SemanticSearch` or `ToolRegister`.

**Fairness.** `IpcSelect` returns the *oldest* ready channel (FIFO). A client that sends many requests does not starve other clients — each client's channel has its own queue (bounded by `queue_depth`), and `IpcSelect` round-robins across ready channels.

### 5.5 Service Restart and Reconnection

A microkernel's core advantage is that services are restartable. When a service crashes (or is killed by OOM, behavioral gating, or manual action), the Service Manager detects the death, restarts the service, and re-establishes channels:

```
Service crash → recovery sequence:

1. KERNEL: Detects service process death
   ├── All channels to the dead service: endpoint set to Dead
   ├── All blocked IpcCalls from clients: unblock with EPIPE
   ├── All shared memory regions: ref_count decremented, pages freed if 0
   └── Service Manager notified via AgentEvent notification

2. SERVICE MANAGER: Restarts the service
   ├── ProcessCreate with same image, same capabilities
   ├── New service process boots, re-initializes internal state
   └── Service registers with Service Manager: "I am Space Storage, ready"

3. SERVICE MANAGER: Rebuilds channels
   ├── For each client that was connected to the old service:
   │   ├── ChannelCreate (new channel pair)
   │   ├── CapabilityTransfer: send service endpoint to new service
   │   └── CapabilityTransfer: send client endpoint to client
   └── Clients receive CHANNEL_RECONNECT notification

4. CLIENTS: Resume operations
   ├── Agent SDK detects CHANNEL_RECONNECT
   ├── SDK replaces old (dead) channel handle with new one
   ├── In-flight requests that got EPIPE are retried automatically
   └── Application code is unaware of the restart (SDK handles it)
```

**What is NOT recovered:** In-flight state inside the service is lost. If Space Storage was mid-write when it crashed, that write is lost (the agent retries). AIRS inference that was mid-generation is lost (the agent re-submits). This is acceptable — services are stateless request processors; persistent state is in spaces.

**Recovery time target:** < 500 ms from crash to new channels established. The Service Manager keeps service images in the page cache for instant reload.

-----

## 6. Notification Mechanism

For asynchronous events (device hotplug, file system changes, attention items), services use **notification channels** — one-way channels where the service can push events to interested agents:

```rust
pub struct NotificationChannel {
    id: NotificationId,
    /// Bounded subscriber list. When full, Subscribe returns ENOSPC.
    /// System services (Level 1) have higher limits than agents.
    subscribers: [Option<ProcessId>; MAX_NOTIFICATION_SUBSCRIBERS],
    subscriber_count: u16,
    filter: NotificationFilter,
}

const MAX_NOTIFICATION_SUBSCRIBERS: usize = 64;

/// Agent subscribes to notifications
pub enum NotificationRequest {
    Subscribe {
        service: ServiceId,
        filter: NotificationFilter,
    },
    Unsubscribe {
        subscription: SubscriptionId,
    },
}

/// Notifications wake IpcSelect
pub enum Notification {
    SpaceChanged { space: SpaceId, object: ObjectId, change: ChangeType },
    DeviceEvent { subsystem: SubsystemId, event: HardwareEvent },
    AttentionItem { item: AttentionItem },
    AgentEvent { agent: AgentId, event: AgentLifecycleEvent },
}
```

Agents use `IpcSelect` to wait on both their service channels and notification channels simultaneously, handling whichever message arrives first.

-----

## 7. POSIX Syscall Translation

BSD tools use POSIX syscalls (open, read, write, fork, exec, pipe, socket, etc.). The POSIX emulation layer translates these to AIOS syscalls + IPC:

```
POSIX syscall          AIOS translation
──────────────         ─────────────────
open("/path", flags)   → IPC to Space Service: resolve path, open object
read(fd, buf, len)     → IPC to Space Service: read from object
write(fd, buf, len)    → IPC to Space Service: write to object
close(fd)              → IPC to Space Service: close object
stat(path, buf)        → IPC to Space Service: query object metadata
readdir(path)          → IPC to Space Service: list objects

fork()                 → ProcessCreate (copy-on-write address space)
exec(path, args)       → ProcessCreate (new image, inherit capabilities)
waitpid(pid)           → ProcessWait
exit(code)             → ProcessExit

pipe()                 → ChannelCreate (anonymous IPC channel)
dup2(old, new)         → fd table manipulation (userspace)

socket(AF_INET, ...)   → IPC to Network Service: create connection
connect(fd, addr)      → IPC to Network Service: connect to remote
send(fd, data, len)    → IPC to Network Service: send data
recv(fd, buf, len)     → IPC to Network Service: receive data

mmap(addr, len, ...)   → MemoryMap (direct syscall)
munmap(addr, len)      → MemoryUnmap (direct syscall)

ioctl(fd, req, arg)    → IPC to relevant subsystem's POSIX bridge

clock_gettime(...)     → TimeGet (direct syscall)
nanosleep(...)         → TimeSleep (direct syscall)
```

The POSIX layer is a userspace library (part of musl libc). It translates POSIX calls to the appropriate IPC messages. The kernel never sees POSIX syscall numbers — it only sees AIOS syscalls.

-----

## 8. Security

### 8.1 Syscall Validation

Every syscall parameter is validated:
- Pointers checked: is the address in user space (TTBR0 range)?
- Lengths checked: does buffer + length overflow?
- Capabilities checked: does the caller hold the required capability?
- All validation happens before any kernel state is modified

### 8.2 IPC Audit

All IPC messages can be audited. The kernel logs:
- Source and destination agent
- Message type (from header)
- Timestamp
- Capability used
- Whether the call succeeded or failed

Full message content is NOT logged by default (privacy). Content logging can be enabled per-channel for debugging.

### 8.3 Capability Enforcement

The IPC system enforces capabilities at five levels (zero trust enforcement stack — see security.md §10.4):

1. **Structural check (kernel):** Does this agent hold a valid, non-expired, non-revoked capability for this channel? Cached per-channel; revocation invalidates the channel.
2. **Protocol check (kernel):** Does the `message_type` match the channel's registered protocol for this direction? Rejects malformed or misrouted messages before delivery.
3. **Behavioral check (kernel, AIRS-informed):** Is this agent's IPC pattern consistent with its behavioral baseline? Rate-limits or suspends anomalous agents. Degrades to pass-through if AIRS is unavailable.
4. **Service check (service):** Does this agent's operation-level capability permit this specific action?
5. **Audit (kernel):** Log source, destination, message type, timestamp, capability used, success/failure.

Levels 1, 2, and 5 are always active (kernel-enforced). Level 3 is active when AIRS is available (graceful fallback). Level 4 is always active (service-enforced). All five must pass for an IPC call to succeed.

-----

## 9. Performance Design

### 9.1 Fast Path

The IPC fast path (synchronous call/reply, small message, no capability transfer):
```
1. SVC trap to kernel                    ~20 cycles
2. Validate syscall + parameters          ~30 cycles
3. Behavioral gate check                  ~15 cycles (see below)
4. Protocol type check (if registered)    ~10 cycles
5. Find destination thread                ~10 cycles (direct lookup)
6. Copy message (≤ 256 bytes in-line)     ~50 cycles
7. Switch to destination thread           ~100 cycles (TTBR swap if needed)
8. Service processes request              (variable)
9. IpcReply: kernel-tracked caller        ~180 cycles (no cap check on reply)
                                          ─────────
Total kernel overhead:                    ~415 cycles (~0.2 μs at 2 GHz)
```

**Why < 5 microseconds.** The sub-5-μs round-trip target is not arbitrary. It is derived from the microkernel penalty chain:

| | Monolithic (Linux) | Microkernel (AIOS) | Penalty |
|---|---|---|---|
| Cached `read()` | ~0.2–0.5 μs (function call) | ~5 μs (IPC round-trip) | 10–25x |
| Build tool (thousands of small reads) | ~0.5 s total | ~5 s total (at 5 μs) | 10x |
| Same build tool at 50 μs IPC | ~0.5 s total | ~50 s total | 100x |

At 5 μs, the POSIX compatibility layer is noticeably slower but usable — the shim caching in §12.2 (Gap 6) mitigates this for common workloads. At 50 μs, POSIX tools (grep, find, cc) become unusable. The development-plan.md decision gate uses < 10 μs as the go/no-go threshold; the 5 μs target is aspirational.

**Comparable systems:**

| Microkernel | Raw IPC | Full round-trip | Notes |
|---|---|---|---|
| seL4 (ARM) | ~0.5–1 μs | ~2–4 μs | Formally verified, benchmark reference |
| QNX (ARM) | ~1–2 μs | ~2–5 μs | Production microkernel, shipped in cars |
| Fuchsia/Zircon | ~0.5–1 μs | ~1–3 μs | Google's capability OS |
| AIOS (target) | ~0.2 μs | < 5 μs | Includes service processing time |

AIOS's ~0.2 μs kernel overhead is competitive with seL4 on ARM. The gap between kernel overhead and 5 μs round-trip is service processing time (Space Service lookup, encryption, etc.). Optimization focus should be on service fast paths, not kernel IPC.

**Step 3: Behavioral gate.** The kernel maintains a per-process `behavioral_state` byte, written by AIRS via lightweight notification (see security.md §10.3 Gap 3). Values:

| State | Meaning | Kernel action |
|---|---|---|
| `0x00` NORMAL | Baseline behavior | IPC proceeds |
| `0x01` ELEVATED | Minor anomaly detected | IPC proceeds; audit flag forced on |
| `0x02` RATE_LIMITED | Significant anomaly | IPC rate-limited (token bucket) |
| `0x03` SUSPENDED | Critical anomaly | IPC rejected with `EACCES` |

The check is a single byte comparison (~15 cycles). In the common case (NORMAL), it's a branch-not-taken. When AIRS is unavailable (fallback mode), all agents default to NORMAL — behavioral gating degrades gracefully to structural-only checks. This is consistent with the principle that AIRS is an optimization layer, not a security dependency (security.md §9.2).

**Step 4: Protocol type check.** If the channel has a registered `ChannelProtocol`, the kernel checks that `message_type` is in the valid set for this direction. This is a linear scan of a small array (~10 cycles for typical protocol sizes of 5-15 types). Channels without a registered protocol (e.g., POSIX pipes) skip this step.

### 9.2 Priority Inheritance Across IPC

When Interactive-class Agent A calls Normal-class Space Service B, the scheduler donates A's time slice to B (scheduler.md §4.2). But B still runs at B's scheduling class — Normal. If B needs to wait on Idle-class compression thread C, A is transitively blocked by an Idle-class thread. This is classic priority inversion.

**Resolution: scheduling context inheritance.** When an `IpcCall` crosses scheduling classes, the kernel temporarily elevates the receiver to the caller's class for the duration of that request:

```rust
unsafe fn ipc_direct_switch(sender: &mut Thread, receiver: &mut Thread, message: &RawMessage) {
    // ... existing copy and switch logic ...

    // Priority inheritance: receiver inherits caller's scheduling context.
    // Saved and restored on IpcReply.
    receiver.sched.inherited_class = Some(sender.sched.class);
    receiver.sched.inherited_priority = Some(sender.sched.priority);
    receiver.sched.inherited_deadline = sender.sched.deadline;

    // If receiver is in a lower class, temporarily elevate
    if receiver.sched.class < sender.sched.class {
        receiver.sched.effective_class = sender.sched.class;
        receiver.sched.effective_priority = sender.sched.priority;
    }
}

// On IpcReply: restore receiver's original scheduling context.
// All three inherited fields must be cleared to prevent stale state.
unsafe fn ipc_reply_switch(replier: &mut Thread, caller: &mut Thread) {
    replier.sched.effective_class = replier.sched.class;          // restore
    replier.sched.effective_priority = replier.sched.priority;    // restore
    replier.sched.inherited_class = None;
    replier.sched.inherited_priority = None;
    replier.sched.inherited_deadline = None;
}
```

**Deadline propagation.** When RT-class Compositor (16.6ms deadline) calls Normal-class Space Service to load a texture, the deadline propagates. Space Service inherits the compositor's deadline for this request, ensuring it runs with RT priority until it replies. This prevents frame drops caused by Space Service being preempted by less-urgent Normal work.

**Transitive inheritance.** If Service B, while handling A's elevated request, calls Service C, the inheritance chain propagates: C inherits A's original scheduling context. The chain is bounded by IPC call depth (typically 1-2 levels; the kernel enforces a maximum depth of 8 to prevent runaway chains).

This is the seL4 MCS (Mixed Criticality System) approach, adapted for AIOS's four scheduling classes. The key property: **no thread ever blocks a higher-priority thread's IPC chain.**

### 9.3 Optimizations

- **Direct thread switch:** When Agent A calls Service B, the kernel switches directly from A's thread to B's thread. No scheduler invocation, no runqueue manipulation.
- **Register-based small messages:** Messages ≤ 64 bytes are passed in registers (x0-x7), avoiding memory copy entirely.
- **Channel caching:** Channel metadata is cached in the kernel's per-process structure. No hash table lookup on the hot path.
- **Batch operations:** The SDK batches small IPC calls when possible (e.g., reading 10 small objects → one IPC with batch read).

-----

## 10. Design Principles

1. **Synchronous by default.** Async adds complexity. Use synchronous IPC for all request/reply patterns. Use notifications for events.
2. **Zero-copy for large data.** Shared memory for anything over 256 bytes. Never copy megabytes through the kernel.
3. **Capabilities are first-class.** The IPC system carries capabilities alongside data. Services receive capabilities, not just requests.
4. **Minimal kernel surface.** 31 syscalls (§3.4). Every syscall is fuzz-tested. Less surface = fewer bugs.
5. **Audit everything.** All IPC is logged at the metadata level. Content logging is opt-in.
6. **POSIX is a library.** The POSIX translation layer is userspace code, not kernel code. The kernel only knows AIOS syscalls.

-----

## 11. Implementation Order

```
Phase 3a:  Syscall handler + basic IPC (send/recv)
           IpcCall with mandatory timeout
           IpcReply (capabilityless reply)
           IpcCancel (request cancellation)
           Peer death signaling (EPIPE on endpoint death)
           → processes can communicate reliably

Phase 3b:  Channel management + capability transfer
           Channel protocol registration (message type validation)
           Per-process kernel resource limits (KernelResourceLimits)
           Channel statistics (ChannelStats)
           Backpressure (EAGAIN on full queue)
           → secure, bounded, observable IPC

Phase 3c:  Shared memory manager
           Reference-counted lifecycle (SharedMemoryRegion)
           Cleanup on process death
           Per-channel bounded shared region arrays
           → zero-copy transfers with no leaks

Phase 3c':  Ring buffer channels + lightweight notifications
            → high-frequency bulk channels (AIRS directives, streaming)

Phase 3d:  Service manager + service discovery
           Multi-client service model (IpcSelect event loop)
           Service restart/reconnection protocol
           → fault-tolerant service infrastructure

Phase 3e:  Notification channels (bounded subscribers)
           → async event delivery

Phase 3f:  IPC performance optimization
           Priority inheritance across IPC (§9.2)
           Deadline propagation
           Direct thread switch (already designed)
           → sub-5μs round-trip, no priority inversion

Phase 8:   AI-native IPC (requires AIRS)
           Context-adaptive IPC scheduling (§13.4)
           Inference-aware batching (§13.3)
           Behavioral gating integration (security.md §10.3)
           Provenance-carrying IPC (§13.5)
           → IPC that understands agents

Phase 10:  Agent IPC extensions (requires Agent SDK)
           Intent-aware IPC routing (§13.1)
           Predictive channel warming (§13.2)
           Semantic capability activation (§13.6)
           → IPC that anticipates agents

Phase 15:  POSIX syscall translation layer + shim caching
           → BSD tools work fast
```

-----

## 12. Modern Kernel Comparison and Gap Analysis

This section compares the AIOS IPC design against state-of-the-art techniques from modern kernels (seL4, Linux 6.x, Fuchsia/Zircon, Hubris, Redox) and identifies gaps with concrete recommendations.

### 12.1 What AIOS Gets Right

**Direct thread switching (Section 9.3).** When Agent A calls Service B, the kernel switches directly from A's thread to B's thread — no scheduler invocation, no runqueue manipulation. This is the core L4 technique that enables sub-microsecond kernel overhead. The ~415 cycle / ~0.2 μs kernel overhead is competitive with seL4 on ARM.

**Register-based small messages (Section 9.3).** Messages ≤ 64 bytes are passed in registers (x0-x7), avoiding memory copy entirely. This matches seL4's approach. Combined with direct switching, the fast path for small synchronous IPC is as good as it gets.

**Capability caching per-channel (Section 4.2).** Channel capabilities are checked at creation, not per-message. This avoids a capability table lookup on the hot path. seL4 does the same with endpoint capabilities.

**Zero-copy shared memory (Section 4.4).** Large data transfers use shared memory regions with pointer exchange instead of kernel-mediated copy. The 10 μs round-trip for a 1 MB transfer (versus 500 μs with copy) is correct.

### 12.2 Gaps and Recommendations

#### Gap 1: No shared-memory ring buffer channel — RESOLVED

**Resolution.** `RingChannelCreate` and `RingChannelDestroy` integrated into the syscall table (§3.1). See §5.2 for streaming inference use case.

#### Gap 2: Heavyweight notification channels — RESOLVED

**Resolution.** Lightweight notification primitives (`NotificationCreate`, `NotificationSignal`, `NotificationWait`) integrated into the syscall table (§3.1). These use seL4-style single-word bitmap signals (~10 cycles). Application-level notification channels (§6) remain for rich typed events.

#### Gap 3: No `IpcReply` syscall — RESOLVED

**Resolution.** `IpcReply` integrated into the syscall table (§3.1). No channel capability required on the reply path — the kernel tracks the caller. Saves ~30 cycles per round-trip.

#### Gap 4: Kernel-enforced protocol types — RESOLVED

**Problem.** Messages were "untyped byte buffers" at the kernel level. Type safety was SDK-only.

**Resolution.** `ChannelProtocol` integrated into §4.1 with bounded fixed-size arrays (not `Vec`/`HashMap`). The kernel checks `message_type` against the channel's registered protocol before delivery. Returns `EPROTO` on mismatch. Protocol validation adds ~10 cycles to the fast path (§9.1 step 4). Channels without a registered protocol (e.g., POSIX pipes) skip the check.

Full FIDL-style typed marshaling (generated code, wire format validation) can be added later as a Phase 3 enhancement.

#### Gap 5: Kernel heap bounds and per-process resource limits — RESOLVED

**Problem.** Channel contained `Vec<SharedMemoryId>`, implying unbounded heap allocation. No per-process limits on kernel object creation.

**Resolution.** Both issues are integrated:
- §4.1: `Channel.shared_regions` is now `[Option<SharedMemoryId>; MAX_SHARED_REGIONS_PER_CHANNEL]` (fixed-size array, no heap)
- §4.3: `RawMessage` uses fixed-size arrays for capabilities and shared memory
- §3.1: `ProcessCreate` requires `KernelResourceLimits`
- §3.3: `KernelResourceLimits` struct with per-trust-level defaults
- Kernel heap bounded by `sum(per-process limits) * per-object size`

#### Gap 6: POSIX translation performance

**Problem.** Every POSIX `read(fd, buf, len)` becomes an IPC round-trip to the Space Service (~5 μs). Native Linux cached `read()` is ~0.2-0.5 μs. That's 10-25x slower. BSD tools (grep, find, cc) perform thousands of small reads — the IPC overhead dominates. These tools use POSIX syscalls directly; they don't use the AIOS SDK and can't benefit from SDK-level batching (Section 9.3).

**Modern precedent.** POSIX shim layers in other microkernels (QNX, Fuchsia, Redox) use userspace caching to amortize IPC cost. QNX's resource managers maintain per-client read buffers. Fuchsia's fdio library caches directory entries.

**Recommendation.** The POSIX translation library (musl libc shim) should include:

1. **Read-ahead buffer.** On `read()`, request 64 KB from the Space Service even if the caller asked for 4 KB. Cache the remainder in the shim. Subsequent `read()` calls are satisfied from the buffer with no IPC. This converts sequential-read workloads from one IPC per read to one IPC per 64 KB.

2. **Vnode cache.** Cache `stat()` results and directory listings in the shim. `stat()` on the same path within a TTL window returns the cached result. This eliminates IPC for repeated `stat()` calls (extremely common in build tools and shell operations).

3. **Batched readdir.** On `opendir()` + `readdir()`, fetch the entire directory listing in one IPC call and iterate locally. This converts O(n) IPC calls to O(1) for directory traversal.

4. **Write coalescing.** Buffer small `write()` calls and flush to the Space Service on `fsync()`, `close()`, or when the buffer is full. This is standard stdio behavior but should be in the POSIX shim for all file descriptors, not just buffered streams.

These optimizations are invisible to POSIX callers and maintain correctness (cache invalidation via notification channels when objects change). Implementation belongs in Phase 15 alongside the POSIX translation layer.

### 12.3 Summary

| Technique | Source | Status | Priority |
|---|---|---|---|
| **Already done** | | | |
| Direct thread switch | seL4, L4 | **Done** | — |
| Register-based messages ≤ 64B | seL4 | **Done** | — |
| Capability caching per-channel | seL4 | **Done** | — |
| Zero-copy shared memory | All microkernels | **Done** | — |
| **Adopted from modern kernels (§12.2)** | | | |
| `IpcReply` syscall | seL4 | **Integrated** (§3.1) | High |
| Shared-memory ring buffer channels | Linux io_uring | **Integrated** (§3.1 `RingChannelCreate`) | High |
| Lightweight notification (bitmap) | seL4 | **Integrated** (§3.1 `NotificationSignal`/`Wait`) | Medium |
| Kernel-enforced protocol types | Fuchsia FIDL | **Integrated** (§4.1 `ChannelProtocol`) | Medium |
| Per-process kernel resource limits | Hubris, general | **Integrated** (§3.3) | Medium |
| POSIX shim caching | QNX, Fuchsia | **Specified** (§12.2) | High |
| **Reliability and correctness (this revision)** | | | |
| Mandatory `IpcCall` timeout | QNX, general | **Integrated** (§3.1) | Critical |
| Peer death signaling (`EPIPE`) | Fuchsia, POSIX | **Integrated** (§4.1) | Critical |
| `IpcCancel` syscall | Fuchsia | **Integrated** (§3.1) | High |
| Service restart/reconnection protocol | QNX, Fuchsia | **Integrated** (§5.5) | Critical |
| Priority inheritance across IPC | seL4 MCS | **Integrated** (§9.2) | High |
| Shared memory lifecycle (refcount) | All kernels | **Integrated** (§4.5) | High |
| Flow control / backpressure (EAGAIN) | POSIX, general | **Integrated** (§4.1) | Medium |
| Multi-client service model (IpcSelect loop) | QNX, general | **Integrated** (§5.4) | Medium |
| AIRS kernel-copy (no shared memory) | Novel | **Integrated** (§5.2) | Critical |
| AIRS streaming via ring buffer | Novel | **Integrated** (§5.2) | High |
| Bounded kernel structs (no Vec in hot path) | Hubris | **Integrated** (§4.1, §4.3, §6) | Medium |
| Channel statistics / metrics | General observability | **Integrated** (§4.1) | Low |
| **AI-native IPC (§13 — unique to AIOS)** | | | |
| Intent-aware IPC routing | Novel | **Specified** (§13.1) | High |
| Predictive channel warming | Novel | **Specified** (§13.2) | Medium |
| Inference-aware batching | Novel | **Specified** (§13.3) | High |
| Context-adaptive IPC scheduling | Novel | **Specified** (§13.4) | Medium |
| Provenance-carrying IPC | Novel | **Specified** (§13.5) | High |
| Semantic capability activation (not negotiation) | Novel | **Specified** (§13.6) | Medium |

-----

## 13. AI-Native IPC: What Only AIOS Can Do

Every IPC technique in §12 exists in at least one other kernel. This section describes capabilities that are unique to AIOS — possible only because the kernel has an integrated AI runtime (AIRS) with semantic understanding of agents, tasks, and user intent. No existing OS kernel has these capabilities, and they cannot be retrofitted onto Linux, seL4, or Fuchsia without an equivalent AI runtime.

### 13.0 Kernel Independence Guarantee

**AIRS is advisory. The kernel is authoritative.** This is the inviolable architectural constraint for every feature in this section. It extends security.md §9.2 ("Resource Intelligence as Optimization, Not Security") to AI-native IPC:

```
┌──────────────────────────────────────────────────────────────────┐
│  AIRS CAN:                                                        │
│    ├── Suggest IPC routes (§13.1)                                │
│    ├── Predict future channel needs (§13.2)                      │
│    ├── Recommend batch parameters (§13.3)                        │
│    ├── Publish context hints for scheduling (§13.4)              │
│    └── Screen data provenance for taint (§13.5)                  │
│                                                                   │
│  AIRS CANNOT:                                                     │
│    ├── Grant, create, or expand capabilities (NEVER)             │
│    ├── Bypass kernel capability checks on any IPC path           │
│    ├── Route an agent to a service the agent has no cap for      │
│    ├── Override kernel resource limits or blast radius policies   │
│    ├── Suppress provenance tags or audit logging                 │
│    └── Prevent the kernel from falling back to static behavior   │
│                                                                   │
│  IF AIRS IS DISABLED OR COMPROMISED:                              │
│    ├── Intent routing: returns ENOTSUP, agent uses direct IPC    │
│    ├── Predictive warming: no pre-warming, cold-start latency    │
│    ├── Inference batching: no batching, sequential processing    │
│    ├── Context scheduling: all agents run at their static class  │
│    ├── Provenance taint screening: tags still propagated by      │
│    │   kernel, but AIRS-based screening disabled                 │
│    └── System is SLOWER but EQUALLY SECURE                       │
└──────────────────────────────────────────────────────────────────┘
```

Every subsection below includes a **"Damage ceiling"** analysis: the worst-case outcome if AIRS is compromised or an agent attempts to exploit the feature. If the damage ceiling for any feature exceeds "degraded performance / wrong optimization," that feature violates this guarantee and must be redesigned or removed.

### 13.1 Intent-Aware IPC Routing

**Problem in every other kernel.** In Linux, Fuchsia, or QNX, when an agent needs a service, it must know *which* service to call and *which* protocol to use. The agent developer hard-codes the service endpoint. If the service changes, the agent breaks. If a better service becomes available (a local LLM that can answer a question instead of an expensive cloud API), the agent doesn't know.

**What AIOS can do.** AIRS understands the *intent* behind IPC calls, not just the protocol. When an agent sends an IPC message, the kernel can optionally route it based on semantic intent rather than fixed endpoint:

```rust
pub enum AirsRequest {
    // ...existing variants...

    /// Intent-routed request. The agent describes what it needs;
    /// AIRS determines which service (or combination of services)
    /// can fulfill it.
    IntentRoute {
        intent_buf: *const u8,
        intent_len: usize,
        /// Constraints: latency budget, privacy level, cost budget.
        constraints: IntentConstraints,
    },
}

pub struct IntentConstraints {
    /// Maximum acceptable latency (ms). 0 = no constraint.
    max_latency_ms: u32,
    /// Data must not leave device? (true = local-only resolution)
    local_only: bool,
    /// Maximum capability scope required.
    max_trust_level: u8,
}
```

**Example flows:**
```
Agent: "summarize this document" + local_only=true
  → AIRS routes to local inference engine (AIRS §Infer)
  → No network, no cloud, no data exfiltration possible

Agent: "translate this text to Japanese" + local_only=false
  → AIRS checks: is a translation model loaded? If yes, route locally.
  → If not, route to cloud translation API via Network Translation Module.
  → Agent code is identical in both cases.

Agent: "find similar images in my photos"
  → AIRS routes to: semantic search (local embedding) → Space Service query
  → Composes two service calls into one logical operation
  → Agent doesn't need to know the implementation pipeline
```

**Security.** Intent routing passes through all five enforcement levels (§8.3). The agent's capabilities still constrain what services it can reach. AIRS can route, but cannot grant capabilities the agent doesn't hold. If AIRS routes to a service the agent has no capability for, the kernel returns EPERM — same as a direct call. Intent routing is an optimization for developer ergonomics and adaptability — it does not bypass security.

**Damage ceiling if AIRS compromised:** Misrouting — an agent's request goes to the wrong service. The wrong service either (a) rejects it (wrong protocol/capability) or (b) processes it incorrectly (wrong answer). **No capability breach, no data leak.** The agent still only reaches services it has capabilities for.

**Agent exploitation vector:** An agent crafts a misleading intent description to be routed to a service it shouldn't access. **Blocked by kernel:** the agent's capability set doesn't change based on the intent description. AIRS can choose among services the agent *already* has access to — it cannot route to services outside the agent's capability set.

**Fallback:** If AIRS is unavailable, `IntentRoute` returns `ENOTSUP`. Agent uses direct IPC with explicit service endpoints. All existing direct IPC paths remain available.

**Why this is future-oriented.** As agent ecosystems grow, hard-coded service endpoints become brittle. Intent routing means agents written today work with services that don't exist yet, as long as the new service can fulfill the intent. This is how operating systems should work when the application layer is autonomous agents rather than deterministic programs.

### 13.2 Predictive Channel Warming

**Problem in every other kernel.** IPC channels are demand-driven — a channel is established when the agent first needs it. The first call to a cold service incurs setup latency: channel creation, capability transfer, TLB warm-up, service wake-up. On AIOS hardware (Raspberry Pi, SD-card storage), cold-start can add 10-50ms.

**What AIOS can do.** AIRS has a behavioral model of every agent (security.md §2, Layer 3). It knows which services each agent typically calls and in what order. The Context Engine knows the user's current activity context. Combined, the kernel can predict future IPC needs:

```
Context Engine: user is switching from browsing to coding
  → AIRS predicts: code agent will need Space Storage (project files),
     semantic search (code navigation), inference (code completion)
  → Kernel pre-warms:
     1. Establishes channels to Space Service for the project space
     2. Prefetches project directory listings into page cache
     3. Warms AIRS inference engine with code-completion KV cache
     4. Pre-maps shared memory regions for anticipated large reads
  → When the code agent actually starts, cold-start latency is ~0
```

**Implementation.** Predictive warming is triggered by AIRS publishing a `WarmingHint` to the Service Manager via the AIRS directive channel (ring buffer):

```rust
pub struct WarmingHint {
    /// Agent that will likely need these services.
    predicted_agent: AgentId,
    /// Services to pre-warm, in predicted order of use.
    services: [ServiceId; MAX_WARMING_SERVICES],
    service_count: u8,
    /// Confidence score (0.0-1.0). Below threshold, hint is ignored.
    confidence: f32,
    /// Time horizon: when is the access expected? (ms from now)
    horizon_ms: u32,
}

const MAX_WARMING_SERVICES: usize = 8;
```

**Degradation.** If the prediction is wrong, nothing bad happens — channels that aren't used are cleaned up by the idle timeout. The warming work is speculative and cancellable. This is the scheduling equivalent of CPU branch prediction: predict the common path, execute speculatively, discard if wrong.

**Damage ceiling if AIRS compromised:** Wasted resources — channels pre-established that nobody uses, pages prefetched that nobody reads. **No capability breach.** Pre-warming still requires the agent to hold the relevant capabilities. The kernel validates capabilities during channel creation regardless of whether the request came from the Service Manager (normal) or from an AIRS warming hint. A compromised AIRS cannot pre-warm channels to services the agent has no access to.

**Agent exploitation vector:** An agent behaves in patterns designed to trigger warming for channels it wants but doesn't have capabilities for. **Blocked by kernel:** warming hints are issued by AIRS (Trust Level 1), not by agents. Agents cannot publish `WarmingHint`. An agent's behavior can influence AIRS's prediction, but the kernel validates capabilities on every channel creation regardless.

**Fallback:** If AIRS is unavailable, no warming hints are issued. Channels are established on first use (cold-start). Performance degrades; security is unchanged.

### 13.3 Inference-Aware IPC Batching

**Problem in every other kernel.** When multiple agents all need LLM inference simultaneously (code completion, email summarization, search indexing), each agent submits an independent `IpcCall` to AIRS. AIRS processes them sequentially. GPU/NPU throughput is wasted — modern inference hardware is optimized for batch processing, where processing 8 requests takes barely longer than processing 1.

**What AIOS can do.** The kernel can transparently batch independent inference requests at the IPC level:

```
Without batching:
  Agent A: IpcCall(AIRS, Infer{prompt_A}) → waits
  AIRS: processes A → replies to A
  Agent B: IpcCall(AIRS, Infer{prompt_B}) → waits
  AIRS: processes B → replies to B
  Total: 2 × inference_latency

With kernel-mediated batching:
  Agent A: IpcCall(AIRS, Infer{prompt_A}) → waits
  Agent B: IpcCall(AIRS, Infer{prompt_B}) → waits
  Kernel: sees two pending IpcCalls to AIRS.Infer within batch window
  Kernel: delivers both as a batch to AIRS
  AIRS: processes A and B in parallel (batch inference)
  AIRS: IpcReply to A, IpcReply to B
  Total: ~1 × inference_latency (batch overhead is marginal)
```

**Batch window.** The kernel holds pending `Infer` requests for a configurable batch window (default: 5ms). If multiple requests arrive within the window, they are delivered as a batch. If only one arrives, it's delivered immediately after the window expires. The window is adaptive — AIRS adjusts it based on current inference throughput and queue depth.

```rust
/// AIRS notifies kernel of optimal batch parameters via ring buffer.
pub struct BatchConfig {
    /// Maximum time to hold requests for batching (ms). 0 = no batching.
    batch_window_ms: u16,
    /// Maximum batch size. Limited by KV cache memory.
    max_batch_size: u8,
    /// Current queue depth. Kernel uses this to decide whether to batch.
    current_queue_depth: u8,
}
```

**Why this matters.** On a device with a GPU or NPU, batching can double or triple inference throughput without any change to agent code. Agents submit individual requests; the kernel and AIRS collaborate to batch them. This is transparent optimization at the system level — exactly what a kernel should do.

**Damage ceiling if AIRS compromised:** A compromised AIRS sets `batch_window_ms` to a large value → all inference requests are delayed. Or sets `max_batch_size` to 1 → no batching, wasted throughput. **Damage is DoS (slower inference), not data breach.** The kernel caps `batch_window_ms` at a hard maximum (e.g., 50ms) regardless of what AIRS requests. Individual agents' `IpcCall` timeouts bound their maximum wait.

**Agent exploitation vector:** An agent floods inference requests to manipulate batch composition (e.g., include adversarial prompts in a batch with other agents' prompts). **Blocked by kernel:** each agent's requests are independent. AIRS processes them in the same batch but with separate KV caches and separate security screening. Batching is a scheduling optimization for the compute hardware — it does not merge or mix agents' data.

**Fallback:** If AIRS is unavailable, `BatchConfig` is ignored. Requests are delivered to AIRS sequentially as they arrive. No batching, no delay. Throughput is lower; correctness is unchanged.

### 13.4 Context-Adaptive IPC Scheduling

**Problem in every other kernel.** IPC priority is static — determined by the caller's scheduling class and the channel's configuration. A background indexing agent has the same IPC behavior whether the user is actively working or the device is idle on a desk.

**What AIOS can do.** The Context Engine continuously publishes the user's activity context (scheduler.md §8.1): work mode, play mode, idle, focus, multi-tasking. The kernel can modulate IPC scheduling based on context:

```
Context: FOCUS (user deeply engaged in one agent)
  → Foreground agent's IPC: promoted to Interactive class
  → Background agents' IPC: demoted to Idle class
  → AIRS inference for foreground: priority boost
  → AIRS inference for background: deferred until next batch window

Context: IDLE (device on desk, screen off)
  → All IPC: Normal class (no priority distinctions)
  → Background indexing, behavioral analysis, space compaction: unrestricted
  → Batch coalescing window increased (less time-sensitive)

Context: MEDIA (user watching video, listening to music)
  → Audio/video pipeline IPC: promoted to RT class
  → Other IPC: Normal class
  → Inference requests: deferred (avoid competing for memory bandwidth)
```

**Integration with behavioral gating (§9.1, step 3).** The `behavioral_state` byte already modulates IPC per-process. Context-adaptive scheduling extends this to modulate IPC system-wide based on what the user is doing. The two are complementary: behavioral gating catches anomalous agents; context scheduling optimizes normal agents.

**Damage ceiling if AIRS compromised:** A compromised Context Engine publishes wrong context hints → IPC priorities are wrong. The foreground agent runs at Idle priority (sluggish UI), or background agents run at Interactive priority (wasted CPU). **Damage is quality-of-service degradation, not security breach.** The kernel enforces hard bounds: context hints can promote an agent at most one scheduling class above its static assignment, and can never promote to RT class (RT is reserved for compositor and audio, set at process creation, not by context hints).

**Agent exploitation vector:** Not applicable — agents cannot publish context hints. Only AIRS (Trust Level 1) publishes `ContextHint` to the scheduler. An agent cannot spoof the context to get itself promoted.

**Fallback:** If AIRS is unavailable, no context hints are published. All agents run at their statically assigned scheduling class. The scheduler already works without context hints (scheduler.md §8.1). Performance is less adaptive; security is unchanged.

### 13.5 Provenance-Carrying IPC

**Problem in every other kernel.** IPC messages are opaque — the kernel knows source, destination, and metadata, but not the data's origin or history. If an agent reads data from Space A, transforms it, and sends it to Space B, the provenance chain is broken — Space B's version history shows the agent as the author, but not that the data originated from Space A.

**What AIOS can do.** IPC messages carry provenance metadata that tracks the chain of custody through the system:

```rust
/// Provenance tag attached to every IPC message at the kernel level.
/// Zero overhead in the fast path — the tag is in the message header,
/// not a separate allocation.
pub struct ProvenanceTag {
    /// Hash of the provenance chain up to this point.
    /// Each IPC hop extends the chain: new_hash = H(prev_hash || source || dest || timestamp).
    chain_hash: [u8; 32],
    /// Origin: where did this data first enter the system?
    origin: ProvenanceOrigin,
    /// Number of IPC hops since origin.
    hop_count: u8,
    /// Highest trust level that touched this data. Once data passes
    /// through a Trust Level 4 agent (web content), it's permanently
    /// tagged as untrusted-origin, regardless of subsequent processing.
    max_trust_exposure: u8,
}

pub enum ProvenanceOrigin {
    /// Created by an agent (agent_id, timestamp)
    Agent(AgentId, Timestamp),
    /// Loaded from a space (space_id, object_id, version)
    Space(SpaceId, ObjectId, Hash),
    /// Received from network (peer_id, connection_id)
    Network(PeerId, ConnectionId),
    /// Generated by AIRS inference (request_id)
    Inference(RequestId),
    /// User input (input_event_id)
    UserInput(EventId),
}
```

**Security application: taint tracking.** If data originates from a web page (Trust Level 4), the `max_trust_exposure` field is set to 4. Every subsequent IPC message carrying derivatives of this data inherits the taint. When this data reaches AIRS for intent verification, AIRS knows it originated from untrusted web content and applies stricter screening (security.md §2.5, Layer 5). This is automatic, kernel-enforced taint propagation — no agent cooperation required.

**Audit application.** The provenance chain hash creates a Merkle chain of all IPC hops. The audit system (security.md §7) can reconstruct exactly how data flowed through the system after the fact. "This file in Space B was created by Agent X, using data from Space A that was fetched from the network by Tab Agent Y" — the full chain is cryptographically verifiable.

**Developer application.** Agent developers can inspect the provenance tag of received data to make trust decisions. An email agent receiving an attachment can check: did this come from a known space (trusted), or was it downloaded from a web page by a tab agent (untrusted)? The tag is available in the SDK as a first-class field on every received message.

**Damage ceiling if AIRS compromised:** Provenance tags are **kernel-maintained** — AIRS reads them but does not write them. The kernel computes `chain_hash`, sets `origin`, increments `hop_count`, and updates `max_trust_exposure` on every IPC hop. AIRS uses provenance tags for screening decisions (Layer 5), but a compromised AIRS cannot forge, suppress, or modify tags. If AIRS screening is disabled, tags still propagate — they just aren't acted on by Layer 5. **No damage to provenance integrity.** AIRS screening of tainted data degrades (less adversarial defense), but the tags themselves are correct.

**Agent exploitation vector:** An agent cannot modify its outgoing provenance tags — the kernel writes them. An agent receiving data with `max_trust_exposure: 4` (web content) cannot "launder" the taint by forwarding it through itself — the kernel preserves the maximum trust exposure across hops. The taint is monotonic: once data touches a low-trust agent, it stays tainted forever.

**Fallback:** Provenance tags propagate regardless of AIRS availability (kernel-enforced). Without AIRS, Layer 5 screening is disabled — tainted data still flows but isn't screened for adversarial content. Tags remain available to services and agents for their own trust decisions.

### 13.6 Semantic Capability Activation

~~**Previous design (REMOVED): Semantic Capability Negotiation.**~~ An earlier version of this section proposed that AIRS could *grant* capabilities to agents based on conversation context. This was removed because it violates the Kernel Independence Guarantee (§13.0): **AIRS cannot grant, create, or expand capabilities.** If AIRS were compromised, it could grant any capability to any agent — making AIRS a security dependency, not an optimization layer. This is the one §13 feature that failed the damage ceiling test.

**Corrected design: Semantic Capability Activation.** AIRS does not grant new capabilities. Instead, it activates *dormant capabilities* that the user already approved.

**How it works.** At install time, the user approves the agent's manifest, which declares the maximum capability set:

```
Agent "Photo Collage" manifest:
  Capabilities requested:
    - ReadSpace("photos")          ← user approves at install
    - ReadSpace("documents")       ← user approves at install
    - WriteSpace("creations")      ← user approves at install
    - UseInference                  ← user approves at install
```

Traditionally, all approved capabilities are active immediately and permanently. The agent holds `ReadSpace("photos")` from the moment it's installed until it's uninstalled — whether or not it's actually working on photos.

With semantic activation, approved capabilities start **dormant** and are activated just-in-time:

```
Install time:
  Kernel creates tokens for all approved capabilities, but marks them DORMANT.
  Agent cannot use dormant capabilities — IPC returns ECAP_DORMANT.

Runtime — agent starts a task:
  User: "make a collage of my vacation photos"
  AIRS: task requires ReadSpace("photos") + WriteSpace("creations") + UseInference
  AIRS sends ActivationRequest to kernel via ring buffer:
    { agent: collage_agent,
      capabilities: [ReadSpace("photos"), WriteSpace("creations"), UseInference],
      scope_narrowing: ReadSpace("photos") → ReadSpace("photos/vacation/"),
      ttl: Duration::minutes(30),
      conversation_context_hash: H(conversation) }
  Kernel validates:
    1. Every requested capability is in the agent's APPROVED manifest (user-approved)
    2. Every scope narrowing is a strict subset (attenuation, not expansion)
    3. TTL does not exceed MAX_CAPABILITY_TTL for the agent's trust level
    4. The agent is not in SUSPENDED behavioral state
  Kernel activates the capabilities with the narrowed scope and TTL.

Task complete:
  AIRS sends DeactivationRequest → kernel returns capabilities to DORMANT.
  Or: TTL expires → kernel automatically deactivates.
```

**What AIRS can do:** Activate a subset of user-approved capabilities, narrow their scope, and set a short TTL.

**What AIRS cannot do:** Grant capabilities the user didn't approve. Expand scope beyond the manifest. Activate capabilities for agents in SUSPENDED state. Override the kernel's TTL limits.

**Damage ceiling if AIRS compromised:** A compromised AIRS activates ALL of an agent's manifest-approved capabilities with maximum scope and maximum TTL. This is equivalent to the traditional model (all capabilities active at install). **The damage ceiling is: the system behaves like a traditional capability OS.** No capability is granted that the user didn't already approve. The semantic activation layer degrades to always-on — worse than designed, but no worse than the baseline.

**Agent exploitation vector:** An agent crafts task descriptions designed to trick AIRS into activating capabilities the agent wants. AIRS may activate `ReadSpace("photos")` when the agent's real intent is data exfiltration. **Bounded by:** (a) only manifest-approved capabilities can activate, (b) behavioral monitoring (Layer 3) detects anomalous access patterns after activation, (c) short TTLs limit exposure window, (d) all activations are logged in the provenance chain.

**Fallback:** If AIRS is unavailable, two options (configurable by user):
1. **Conservative (default):** All capabilities remain dormant. Agent cannot start new tasks until AIRS recovers. Existing active capabilities continue until TTL expires.
2. **Permissive:** All manifest-approved capabilities activate with maximum scope. System behaves like a traditional capability OS. Less secure but functional.

**Why this is better than the status quo.** In a traditional capability OS (seL4, Fuchsia), agents hold all their permissions all the time. A compromised agent has full access to everything in its manifest from the moment it's compromised. With semantic activation, the exposure window is limited to the current task's scope and duration. A compromised photo agent during a "vacation collage" task can read `photos/vacation/` for 30 minutes — not all photos forever.

### 13.7 What Makes These Novel

| Capability | Why no existing OS can do this | AIOS enabler | Damage ceiling if AIRS compromised |
|---|---|---|---|
| Intent-aware routing | Requires semantic understanding of requests | AIRS inference engine | Misrouting (wrong answer, not capability breach) |
| Predictive warming | Requires behavioral model + context awareness | AIRS behavioral monitoring + Context Engine | Wasted resources (no capability breach) |
| Inference batching | Requires kernel awareness of ML batch semantics | AIRS co-designed with kernel IPC | Slower inference (DoS, not data breach) |
| Context-adaptive scheduling | Requires continuous user activity inference | Context Engine (AIRS subsystem) | Wrong priorities (QoS degradation) |
| Provenance-carrying IPC | Could be done without AI, but *taint-based screening* requires ML | AIRS adversarial defense (Layer 5) | Taint screening disabled (tags still propagate) |
| Semantic capability activation | Requires understanding *why* an agent needs access | AIRS intent verification (Layer 1) + conversation context | All manifest caps active = traditional OS behavior |

**Every feature degrades to "traditional OS behavior" when AIRS fails.** None of the §13 features create new attack surfaces that don't exist in a traditional capability OS. They are strictly additive — they make the system better when AIRS works, and no worse when AIRS doesn't.

These capabilities are why AIOS exists as a new OS rather than a Linux distribution. They require co-design between the kernel's IPC layer and the AI runtime — something that cannot be achieved by adding an AI service on top of an existing kernel. The kernel must understand inference latency, batch semantics, behavioral baselines, and conversation context at the IPC scheduling level. That is only possible when the AI runtime is a first-class kernel citizen. But at no point does the kernel *trust* AIRS — it validates every hint, caps every parameter, and falls back to static behavior if AIRS is unavailable. The relationship is: AIRS advises, the kernel decides.
