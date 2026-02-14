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
