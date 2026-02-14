# AIOS IPC and Syscall Interface

## Deep Technical Architecture

**Parent document:** [architecture.md](../project/architecture.md)
**Related:** [compositor.md](../platform/compositor.md) — Compositor protocol, [subsystem-framework.md](../platform/subsystem-framework.md) — Subsystem sessions

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

    /// Send a message and wait for reply (synchronous)
    IpcCall {
        channel: ChannelId,
        send_buf: *const u8,
        send_len: usize,
        recv_buf: *mut u8,
        recv_len: usize,
    },

    /// Send a message without waiting for reply (asynchronous)
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

    /// Wait for a message on any of multiple channels
    IpcSelect {
        channels: *const ChannelId,
        channel_count: usize,
        recv_buf: *mut u8,
        recv_len: usize,
        timeout: Option<Duration>,
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

    /// Spawn a new process
    ProcessCreate {
        image: ContentHash,            // content-addressed executable
        capabilities: *const CapabilityTokenId,
        cap_count: usize,
        args: *const u8,
        args_len: usize,
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

  Return:
  x0:  result (0 = success, negative = error code)
  x1:  secondary return value (e.g., bytes transferred)
```

### 3.3 Syscall Count

Total: ~20 syscalls. Compare with Linux (~450) or even seL4 (~12). AIOS targets the sweet spot: enough for a full-featured OS, few enough that every syscall can be audited and fuzz-tested exhaustively.

-----

## 4. IPC Design

### 4.1 Channels

IPC channels are the communication primitive. A channel is a bidirectional pipe between two endpoints:

```rust
pub struct Channel {
    id: ChannelId,
    endpoint_a: ProcessId,
    endpoint_b: ProcessId,
    capability: ChannelCapability,
    message_queue: RingBuffer<Message>,
    shared_regions: Vec<SharedMemoryId>,
    audit: bool,                        // log all messages?
}

pub struct ChannelFlags {
    /// Maximum message size
    max_message: usize,
    /// Queue depth (for async sends)
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
  │ ←── IpcReply(channel, reply)   │
  │ (agent thread resumes)         │
  │                                │
```

**Why synchronous:** Synchronous IPC is simpler, debuggable, and avoids the complexity of asynchronous callback chains. For a microkernel where every system call is an IPC, synchronous is the proven approach (seL4, L4, QNX all use synchronous IPC).

**Latency target:** < 5 microseconds round-trip. This requires:
- No memory allocation in the IPC path
- No context switch overhead (direct thread switch from sender to receiver)
- Message copied once (sender buffer → receiver buffer) or zero-copy via shared memory
- Capability validation cached per-channel (checked at creation, not per-message)

### 4.3 Message Format

Messages are untyped byte buffers at the kernel level. The SDK provides typed wrappers:

```rust
/// Kernel-level message (untyped)
pub struct RawMessage {
    data: *const u8,
    len: usize,
    capabilities: Vec<CapabilityTokenId>,  // capability transfer
    shared_memory: Vec<SharedMemoryId>,    // shared region transfer
}

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

### 4.5 Capability Transfer

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

### 5.1 Space Service Protocol

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
}

pub enum SpaceReply {
    Object { content: SharedMemoryId, metadata: ObjectMetadata },
    ObjectId { id: ObjectId },
    ObjectList { objects: Vec<ObjectSummary> },
    VersionList { versions: Vec<VersionSummary> },
    SpaceList { spaces: Vec<SpaceSummary> },
    Ok,
    Error(SpaceError),
}
```

### 5.2 AIRS Service Protocol

```rust
pub enum AirsRequest {
    /// Request inference
    Infer {
        prompt: SharedMemoryId,
        parameters: InferenceParameters,
        priority: InferencePriority,
    },
    /// Generate embedding
    Embed {
        content: SharedMemoryId,
    },
    /// Search semantically
    SemanticSearch {
        query: String,
        space: SpaceId,
        limit: u32,
    },
    /// Register a tool
    ToolRegister {
        name: String,
        schema: ToolSchema,
    },
}

pub enum AirsReply {
    /// Streaming: each token delivered as separate reply
    Token { text: String, finished: bool },
    /// Embedding vector
    Embedding { vector: SharedMemoryId },
    /// Search results
    SearchResults { objects: Vec<(ObjectId, f32)> },
    Ok,
    Error(AirsError),
}
```

### 5.3 Compositor Protocol

See [compositor.md](../platform/compositor.md) for the full protocol. The compositor uses the same IPC channel pattern as all other services.

-----

## 6. Notification Mechanism

For asynchronous events (device hotplug, file system changes, attention items), services use **notification channels** — one-way channels where the service can push events to interested agents:

```rust
pub struct NotificationChannel {
    id: ChannelId,
    subscribers: Vec<ProcessId>,
    filter: NotificationFilter,
}

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

The IPC system enforces capabilities at two levels:
1. **Channel capability:** Does this agent have the right to use this channel? (Checked at IpcCall)
2. **Service capability:** Does this agent have the right to perform this operation? (Checked by the service)

Level 1 is in the kernel. Level 2 is in the service. Both must pass.

-----

## 9. Performance Design

### 9.1 Fast Path

The IPC fast path (synchronous call/reply, small message, no capability transfer):
```
1. SVC trap to kernel                    ~20 cycles
2. Validate syscall + parameters          ~30 cycles
3. Find destination thread                ~10 cycles (direct lookup)
4. Copy message (≤ 256 bytes in-line)     ~50 cycles
5. Switch to destination thread           ~100 cycles (TTBR swap if needed)
6. Service processes request              (variable)
7. Reply: same path in reverse            ~210 cycles
                                          ─────────
Total kernel overhead:                    ~420 cycles (~0.2 μs at 2 GHz)
```

### 9.2 Optimizations

- **Direct thread switch:** When Agent A calls Service B, the kernel switches directly from A's thread to B's thread. No scheduler invocation, no runqueue manipulation.
- **Register-based small messages:** Messages ≤ 64 bytes are passed in registers (x0-x7), avoiding memory copy entirely.
- **Channel caching:** Channel metadata is cached in the kernel's per-process structure. No hash table lookup on the hot path.
- **Batch operations:** The SDK batches small IPC calls when possible (e.g., reading 10 small objects → one IPC with batch read).

-----

## 10. Design Principles

1. **Synchronous by default.** Async adds complexity. Use synchronous IPC for all request/reply patterns. Use notifications for events.
2. **Zero-copy for large data.** Shared memory for anything over 256 bytes. Never copy megabytes through the kernel.
3. **Capabilities are first-class.** The IPC system carries capabilities alongside data. Services receive capabilities, not just requests.
4. **Minimal kernel surface.** ~20 syscalls. Every syscall is fuzz-tested. Less surface = fewer bugs.
5. **Audit everything.** All IPC is logged at the metadata level. Content logging is opt-in.
6. **POSIX is a library.** The POSIX translation layer is userspace code, not kernel code. The kernel only knows AIOS syscalls.

-----

## 11. Implementation Order

```
Phase 3a:  Syscall handler + basic IPC (send/recv)  → processes can communicate
Phase 3b:  Channel management + capability transfer  → secure IPC
Phase 3c:  Shared memory manager                     → zero-copy transfers
Phase 3d:  Service manager + service discovery       → services register and are findable
Phase 3e:  Notification channels                     → async event delivery
Phase 3f:  IPC performance optimization              → sub-5μs round-trip
Phase 15:  POSIX syscall translation layer           → BSD tools work
```

-----

## 12. Modern Kernel Comparison and Gap Analysis

This section compares the AIOS IPC design against state-of-the-art techniques from modern kernels (seL4, Linux 6.x, Fuchsia/Zircon, Hubris, Redox) and identifies gaps with concrete recommendations.

### 12.1 What AIOS Gets Right

**Direct thread switching (Section 9.2).** When Agent A calls Service B, the kernel switches directly from A's thread to B's thread — no scheduler invocation, no runqueue manipulation. This is the core L4 technique that enables sub-microsecond kernel overhead. The ~420 cycle / ~0.2 μs kernel overhead is competitive with seL4 on ARM.

**Register-based small messages (Section 9.2).** Messages ≤ 64 bytes are passed in registers (x0-x7), avoiding memory copy entirely. This matches seL4's approach. Combined with direct switching, the fast path for small synchronous IPC is as good as it gets.

**Capability caching per-channel (Section 4.2).** Channel capabilities are checked at creation, not per-message. This avoids a capability table lookup on the hot path. seL4 does the same with endpoint capabilities.

**Zero-copy shared memory (Section 4.4).** Large data transfers use shared memory regions with pointer exchange instead of kernel-mediated copy. The 10 μs round-trip for a 1 MB transfer (versus 500 μs with copy) is correct.

### 12.2 Gaps and Recommendations

#### Gap 1: No shared-memory ring buffer channel

**Problem.** The channel's `RingBuffer<Message>` (Section 4.1) is internal to the kernel, not a shared-memory ring buffer visible to userspace. Every IPC message — including high-frequency, low-priority traffic like AIRS resource directives, agent telemetry, and fire-and-forget hints — requires an SVC trap. For channels with thousands of messages per second, the syscall overhead accumulates.

**Modern precedent.** Linux's io_uring uses shared-memory submission and completion queues. Userspace writes to the submission queue; the kernel reads it on its own schedule. No syscall per operation. This achieves millions of I/O operations per second with near-zero kernel entry overhead.

**Recommendation.** Add a `ChannelCreateRing` variant that creates a shared-memory ring buffer channel:

```rust
/// Ring buffer channel: shared-memory submission/completion queues.
/// No SVC trap per message. Kernel polls the submission queue.
/// Suitable for: AIRS directives, agent hints, telemetry, metrics.
RingChannelCreate {
    submission_queue_size: u32,  // entries (power of 2)
    completion_queue_size: u32,
    entry_size: u32,            // max bytes per entry
    flags: RingChannelFlags,
},
```

The kernel maps the ring buffer into both address spaces. The producer writes entries and advances the tail pointer (atomic). The consumer reads entries and advances the head pointer (atomic). A lightweight notification (see Gap 2) wakes the consumer when new entries arrive.

**Use cases:**
- AIRS resource directive channel (high volume, batchable, kernel polls at tick rate)
- Agent hint channel (fire-and-forget, no reply needed)
- Telemetry/metrics channel (periodic, batchable)
- Audit event batching (reduce per-event syscall overhead)

**Not suitable for:** Request/reply IPC (use synchronous `IpcCall`), capability transfers (require kernel mediation), security-sensitive operations (need per-message validation).

#### Gap 2: Heavyweight notification channels

**Problem.** Section 6 defines notifications as full IPC channels with subscriber lists, filters, and rich typed payloads (`SpaceChanged`, `DeviceEvent`, `AttentionItem`). This is appropriate for application-level events but too expensive for kernel-level signals like memory pressure transitions (`Normal → Low → Critical → OOM`), ring buffer wakeups, or fallback mode triggers.

**Modern precedent.** seL4's notification objects are a single machine word (bitmap) that can be set and waited on atomically. Setting a notification bit is a single atomic OR — no message allocation, no queue, no serialization. Orders of magnitude cheaper than a full IPC message.

**Recommendation.** Add a lightweight notification primitive alongside the existing notification channels:

```rust
/// Lightweight notification: single-word bitmap, atomic set/wait.
/// No message body. Each bit position is a signal.
/// Suitable for: pressure transitions, wakeups, mode changes.
pub struct LightNotification {
    id: NotificationId,
    word: AtomicU64,  // 64 signal bits
}

/// Syscalls:
NotificationCreate {},
NotificationSignal { id: NotificationId, bits: u64 },  // atomic OR
NotificationWait { id: NotificationId, mask: u64 },    // block until any bit in mask is set
```

**Bit assignments (example for kernel → AIRS channel):**
- Bit 0: Memory pressure changed
- Bit 1: New entries in ring buffer
- Bit 2: Fallback mode transition requested
- Bit 3: Agent spawned/exited
- Bits 4-63: Reserved

This pairs naturally with ring buffer channels: the producer signals bit 1 after writing entries; the consumer waits on bit 1 and processes the batch.

#### Gap 3: No `IpcReply` syscall

**Problem.** The syscall table has `IpcCall`, `IpcSend`, `IpcRecv` — but no dedicated `IpcReply`. Services presumably reply using `IpcSend` on the same channel. This requires a capability check on the reply path (the service must hold the channel capability to send). It also creates a confused-deputy risk: a service could accidentally reply on the wrong channel.

**Modern precedent.** seL4 has a separate `Reply` syscall that requires no capability. The kernel tracks "who called me" implicitly — a reply always goes back to the last caller. This eliminates one capability check per round-trip and makes it structurally impossible to reply to the wrong endpoint.

**Recommendation.** Add `IpcReply` as a syscall:

```rust
/// Reply to the last IpcCall received on this channel.
/// No capability required — the kernel knows the caller.
/// Can only be used once per received IpcCall (enforced by kernel).
IpcReply {
    reply_buf: *const u8,
    reply_len: usize,
},
```

This saves ~30 cycles per round-trip (skipped capability validation) and prevents misrouted replies. The fast path cycle count drops from ~420 to ~390.

#### Gap 4: Kernel-enforced protocol types

**Problem.** Messages are "untyped byte buffers" at the kernel level (Section 4.3). Type safety comes from `TypedMessage<T>` in the SDK — voluntary, not enforced. A malicious or buggy agent can send raw bytes that don't match the expected type. The service must defensively deserialize and handle all malformed input.

**Modern precedent.** Fuchsia's FIDL (Fuchsia Interface Definition Language) generates marshaling code from interface definitions. The kernel validates that messages conform to the registered protocol before delivery. This prevents protocol confusion attacks at the kernel level.

**Recommendation.** At minimum, enforce a message type tag per-channel:

```rust
pub struct ChannelProtocol {
    /// Valid message_type values for this channel direction (A→B).
    /// Kernel rejects messages with unregistered types.
    valid_types_a_to_b: Vec<u32>,
    /// Valid message_type values for the reverse direction (B→A).
    valid_types_b_to_a: Vec<u32>,
    /// Maximum message size per type (optional).
    max_size_per_type: HashMap<u32, usize>,
}
```

The Service Manager registers valid protocol types when creating channels. The kernel checks `message_type` against the channel's protocol before delivery. This is a lightweight check (one lookup in a small set) that catches protocol confusion without the full complexity of FIDL-style validation.

Full FIDL-style typed marshaling (generated code, wire format validation) can be added later as a Phase 3 enhancement.

#### Gap 5: Kernel heap bounds and per-process resource limits

**Problem.** Channel contains `Vec<SharedMemoryId>` (Section 4.1), implying heap allocation. No per-process limits on kernel object creation are specified. A malicious process creating thousands of channels, shared memory regions, or pending messages can exhaust the kernel heap.

**Modern precedent.** Hubris (Oxide Computer's embedded Rust kernel) does zero heap allocation after boot — all resources are statically declared. This is extreme for a general-purpose OS, but the principle of bounded kernel resources is sound.

**Recommendation.** Add per-process kernel resource limits:

```rust
pub struct KernelResourceLimits {
    max_channels: u32,              // default: 64
    max_shared_regions: u32,        // default: 32
    max_pending_messages: u32,      // default: 256
    max_notification_subscriptions: u32, // default: 16
    max_child_processes: u32,       // default: 8
}
```

These limits are set at process creation (derived from the agent's trust level and blast radius policy) and enforced by the kernel. The kernel's total heap usage is bounded by: `sum(per-process limits) * per-object size`. This provides a hard ceiling that prevents kernel OOM from any userspace action.

Additionally, replace `Vec<SharedMemoryId>` in the Channel struct with a fixed-size array:

```rust
pub struct Channel {
    // ...
    shared_regions: [Option<SharedMemoryId>; MAX_SHARED_REGIONS_PER_CHANNEL],
    // ...
}
```

This eliminates dynamic allocation in the channel hot path entirely.

#### Gap 6: POSIX translation performance

**Problem.** Every POSIX `read(fd, buf, len)` becomes an IPC round-trip to the Space Service (~5 μs). Native Linux cached `read()` is ~0.2-0.5 μs. That's 10-25x slower. BSD tools (grep, find, cc) perform thousands of small reads — the IPC overhead dominates. These tools use POSIX syscalls directly; they don't use the AIOS SDK and can't benefit from SDK-level batching (Section 9.2).

**Modern precedent.** POSIX shim layers in other microkernels (QNX, Fuchsia, Redox) use userspace caching to amortize IPC cost. QNX's resource managers maintain per-client read buffers. Fuchsia's fdio library caches directory entries.

**Recommendation.** The POSIX translation library (musl libc shim) should include:

1. **Read-ahead buffer.** On `read()`, request 64 KB from the Space Service even if the caller asked for 4 KB. Cache the remainder in the shim. Subsequent `read()` calls are satisfied from the buffer with no IPC. This converts sequential-read workloads from one IPC per read to one IPC per 64 KB.

2. **Vnode cache.** Cache `stat()` results and directory listings in the shim. `stat()` on the same path within a TTL window returns the cached result. This eliminates IPC for repeated `stat()` calls (extremely common in build tools and shell operations).

3. **Batched readdir.** On `opendir()` + `readdir()`, fetch the entire directory listing in one IPC call and iterate locally. This converts O(n) IPC calls to O(1) for directory traversal.

4. **Write coalescing.** Buffer small `write()` calls and flush to the Space Service on `fsync()`, `close()`, or when the buffer is full. This is standard stdio behavior but should be in the POSIX shim for all file descriptors, not just buffered streams.

These optimizations are invisible to POSIX callers and maintain correctness (cache invalidation via notification channels when objects change). Implementation belongs in Phase 15 alongside the POSIX translation layer.

### 12.3 Revised Implementation Order

```
Phase 3a:  Syscall handler + basic IPC (send/recv)       → processes can communicate
Phase 3b:  Channel management + capability transfer       → secure IPC
Phase 3b':  Add IpcReply syscall                          → cheaper reply path
Phase 3c:  Shared memory manager                          → zero-copy transfers
Phase 3c':  Ring buffer channels                          → high-frequency bulk channel
Phase 3d:  Service manager + service discovery            → services register and are findable
Phase 3d':  Channel protocol registration                 → kernel-enforced message types
Phase 3e:  Notification channels + light notifications    → async events + cheap signals
Phase 3f:  IPC performance optimization                   → sub-5μs round-trip
Phase 3f':  Per-process kernel resource limits             → bounded kernel heap
Phase 15:  POSIX translation layer + shim caching         → BSD tools work fast
```

### 12.4 Summary

| Technique | Source | Status | Priority |
|---|---|---|---|
| Direct thread switch | seL4, L4 | **Done** | — |
| Register-based messages ≤ 64B | seL4 | **Done** | — |
| Capability caching per-channel | seL4 | **Done** | — |
| Zero-copy shared memory | All microkernels | **Done** | — |
| `IpcReply` syscall | seL4 | **Add** | High (fast path improvement) |
| Shared-memory ring buffer channels | Linux io_uring | **Add** | High (AIRS directive channel) |
| Lightweight notification (bitmap) | seL4 | **Add** | Medium (pairs with ring buffers) |
| Kernel-enforced protocol types | Fuchsia FIDL | **Add** | Medium (security hardening) |
| Per-process kernel resource limits | Hubris, general | **Add** | Medium (DoS prevention) |
| POSIX shim caching (readahead, vnode, batched readdir) | QNX, Fuchsia | **Add** | High (user-facing performance) |
