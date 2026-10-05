# IPC Kit

**Layer:** Kernel | **Crate:** `aios_ipc` | **Architecture:** [`docs/kernel/ipc.md`](../../kernel/ipc.md)

## 1. Overview

IPC Kit provides capability-gated message passing between agents. In a microkernel, every
service interaction is an IPC message — reading a file, playing audio, querying a database.
IPC Kit is the communication backbone that makes AIOS's agent-based architecture possible.

Application developers rarely use IPC Kit directly. Higher-level Kits (Storage Kit, Audio Kit,
Network Kit) wrap IPC calls into typed APIs. You reach for IPC Kit when you need to build a
service that other agents call into, implement a custom protocol between cooperating agents,
or use notifications and multi-wait for event-driven architectures.

AIOS IPC targets **sub-5-microsecond round-trip latency**. The key optimizations: synchronous
call/reply semantics (no queue management on the fast path), direct thread switching (the
kernel switches directly from caller to callee without going through the scheduler), inline
message payloads (256 bytes without allocation), and capability transfer as a first-class
operation.

## 2. Core Traits

```rust
use aios_ipc::{
    Channel, ChannelId, MessageRing, RawMessage,
    Notification, NotificationId,
    IpcSelect, SelectEntry, SelectKind,
    SharedMemoryRegion, SharedMemoryId,
};
use aios_capability::CapabilityHandle;

/// A bidirectional message channel between two agents.
///
/// Channels are the primary IPC mechanism. Each channel has a 16-slot
/// ring buffer for messages. The kernel validates capabilities on every
/// send/receive — there is no "open channel, then send freely" pattern.
pub struct Channel {
    pub id: ChannelId,
    /// The ring buffer backing this channel.
    ring: MessageRing,
    /// Capability that created this channel (for cascade revocation).
    creation_cap: CapabilityHandle,
}

/// Lock-free ring buffer for IPC messages.
///
/// Each slot holds a RawMessage (272 bytes: sender + 256-byte payload + length).
/// 16 slots per channel = 4,352 bytes per channel.
pub struct MessageRing {
    messages: [RawMessage; 16],
    head: AtomicU32,
    tail: AtomicU32,
}

/// Synchronous call: send a message and block until the server replies.
///
/// This is the primary IPC pattern. The kernel performs a direct thread
/// switch — the caller is suspended and the server thread is resumed
/// immediately, without going through the scheduler. The reply switch
/// is equally fast.
///
/// Timeout is mandatory to prevent deadlocks.
pub fn ipc_call(
    channel: ChannelId,
    request: &[u8],
    reply_buf: &mut [u8],
    timeout: Duration,
) -> Result<usize, IpcKitError>;

/// Wait for an incoming message on a channel.
///
/// Servers use this to receive client requests. Returns the message
/// payload and the caller's identity (for reply routing).
pub fn ipc_recv(
    channel: ChannelId,
    buf: &mut [u8],
    timeout: Duration,
) -> Result<(usize, CallerId), IpcKitError>;

/// Reply to the last received call.
///
/// No capability check required — the kernel tracks which caller is
/// waiting for a reply on this channel. Can only be called once per
/// received message.
pub fn ipc_reply(reply: &[u8]) -> Result<(), IpcKitError>;

/// Asynchronous send: post a message without waiting for a reply.
///
/// Useful for fire-and-forget notifications or event streams.
pub fn ipc_send(
    channel: ChannelId,
    message: &[u8],
) -> Result<(), IpcKitError>;

/// Lightweight notification object (seL4-style bitmap signals).
///
/// No message body — just a single-word bitmap where each bit is a signal.
/// Atomic OR to signal, masked wait to receive. ~10 cycles per signal.
pub struct Notification {
    pub id: NotificationId,
    /// The notification word — each bit is an independent signal.
    word: AtomicU64,
}

impl Notification {
    /// Signal specific bits (atomic OR into the notification word).
    /// A missing notification is `InvalidArgument` (the kernel's EINVAL).
    pub fn signal(&self, bits: u64) -> Result<(), IpcKitError>;

    /// Wait until any bit in mask is set. Returns the matched bits
    /// and atomically clears them.
    pub fn wait(&self, mask: u64, timeout: Duration) -> Result<u64, IpcKitError>;
}

/// Multi-wait on channels and notifications simultaneously.
///
/// Servers that handle multiple event sources use IpcSelect to wait on
/// up to 8 sources at once. Returns the index of the first ready source.
pub struct IpcSelect;

impl IpcSelect {
    /// Wait on multiple sources. Returns (ready_index, matched_bits).
    /// matched_bits is non-zero only for notification entries.
    pub fn wait(
        entries: &[SelectEntry],
        timeout: Duration,
    ) -> Result<(usize, u64), IpcKitError>;
}

/// An entry in an IpcSelect wait set.
pub struct SelectEntry {
    pub kind: SelectKind,
}

pub enum SelectKind {
    /// Wait for a message on a channel.
    Channel(ChannelId),
    /// Wait for bits on a notification.
    Notification { id: NotificationId, mask: u64 },
}

/// Zero-copy shared memory region between agents.
///
/// For payloads larger than 256 bytes (the inline message limit),
/// agents allocate a shared memory region and pass its ID over IPC.
/// W^X enforcement: a region cannot be both writable and executable.
pub struct SharedMemoryRegion {
    pub id: SharedMemoryId,
    pub size: usize,
    pub flags: MemoryFlags,
}

impl SharedMemoryRegion {
    /// Create a new shared memory region.
    pub fn create(size: usize, flags: MemoryFlags) -> Result<Self, IpcKitError>;

    /// Map the region into another agent's address space.
    pub fn share_with(&self, agent: AgentId, flags: MemoryFlags) -> Result<(), IpcKitError>;

    /// Get a pointer to the mapped region.
    pub fn as_ptr(&self) -> *const u8;
    pub fn as_mut_ptr(&self) -> *mut u8;

    /// Unmap and destroy the region.
    pub fn destroy(self) -> Result<(), IpcKitError>;
}
```

## 3. Usage Patterns

### Building a service (server side)

```rust
use aios_ipc::{Channel, ipc_recv, ipc_reply, IpcSelect, SelectEntry, SelectKind};
use aios_app::AgentContext;

fn run_echo_service(ctx: &AgentContext) -> Result<(), AppError> {
    let channel = Channel::create(ctx.capability(Capability::ChannelCreate)?)?;
    ctx.register_service("com.example.echo", channel.id)?;

    let mut buf = [0u8; 256];
    loop {
        // Wait for incoming requests
        let (len, _caller) = ipc_recv(channel.id, &mut buf, Duration::MAX)?;

        // Echo the message back
        ipc_reply(&buf[..len])?;
    }
}
```

### Calling a service (client side)

```rust
use aios_ipc::ipc_call;
use aios_app::AgentContext;

fn call_echo(ctx: &AgentContext, message: &[u8]) -> Result<Vec<u8>, AppError> {
    let echo_channel = ctx.lookup_service("com.example.echo")?;
    let mut reply = vec![0u8; 256];
    let len = ipc_call(echo_channel, message, &mut reply, Duration::secs(5))?;
    reply.truncate(len);
    Ok(reply)
}
```

### Event-driven server with IpcSelect

```rust
use aios_ipc::{IpcSelect, SelectEntry, SelectKind, Notification};

fn event_loop(
    client_channel: ChannelId,
    timer_notify: &Notification,
    shutdown_notify: &Notification,
) -> Result<(), AppError> {
    let entries = [
        SelectEntry { kind: SelectKind::Channel(client_channel) },
        SelectEntry { kind: SelectKind::Notification {
            id: timer_notify.id,
            mask: 0x1, // bit 0 = timer tick
        }},
        SelectEntry { kind: SelectKind::Notification {
            id: shutdown_notify.id,
            mask: 0x1, // bit 0 = shutdown signal
        }},
    ];

    loop {
        let (ready_idx, bits) = IpcSelect::wait(&entries, Duration::secs(30))?;
        match ready_idx {
            0 => handle_client_request(client_channel)?,
            1 => handle_timer_tick()?,
            2 => {
                handle_shutdown();
                return Ok(());
            }
            _ => unreachable!(),
        }
    }
}
```

### Zero-copy bulk transfer via shared memory

```rust
use aios_ipc::{SharedMemoryRegion, ipc_call};

fn send_large_payload(
    channel: ChannelId,
    data: &[u8],
) -> Result<(), AppError> {
    // Allocate shared memory for the payload
    let region = SharedMemoryRegion::create(
        data.len(),
        MemoryFlags { read: true, write: true, execute: false },
    )?;

    // Copy data into the shared region
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), region.as_mut_ptr(), data.len());
    }

    // Send the region ID over IPC (fits in the 256-byte inline message)
    let msg = SharedMemoryMessage {
        region_id: region.id,
        offset: 0,
        length: data.len(),
    };
    let mut reply = [0u8; 64];
    ipc_call(channel, &msg.serialize(), &mut reply, Duration::secs(10))?;

    // Clean up
    region.destroy()?;
    Ok(())
}
```

## 4. Integration Examples

### With App Kit — message dispatch loop

```rust
use aios_app::{Application, MessageLoop};
use aios_ipc::{ipc_recv, ipc_reply};

/// App Kit wraps IPC Kit into a higher-level dispatch loop.
/// Agents implement handlers for typed messages rather than
/// raw byte buffers.
impl MessageLoop for MyApp {
    fn dispatch(&mut self, msg: TypedMessage) -> Result<TypedMessage, AppError> {
        match msg {
            TypedMessage::CreateNote(req) => {
                let note = self.create_note(req)?;
                Ok(TypedMessage::NoteCreated(note))
            }
            TypedMessage::SearchNotes(query) => {
                let results = self.search(query)?;
                Ok(TypedMessage::SearchResults(results))
            }
            _ => Err(AppError::UnknownMessage),
        }
    }
}
```

### With Capability Kit — capability transfer over IPC

```rust
use aios_ipc::Channel;
use aios_capability::{CapabilityHandle, AttenuationSpec};

/// Transfer a capability to another agent over an IPC channel.
/// The kernel mediates the transfer — the sending agent cannot
/// fabricate capabilities.
fn grant_storage_access(
    channel: &Channel,
    storage_cap: CapabilityHandle,
) -> Result<(), AppError> {
    channel.transfer_capability(storage_cap)?;
    Ok(())
}
```

## 5. Capability Requirements

| Capability | What It Gates | Default Grant |
| --- | --- | --- |
| `ChannelCreate` | Creating new IPC channels | Granted to all agents |
| `ChannelAccess(id)` | Sending/receiving on a specific channel | Per-channel, on creation |
| `SharedMemoryCreate` | Creating shared memory regions | Granted to all agents |
| `SharedMemoryAccess(id)` | Mapping a specific region (`shmem_map`) | Granted to the creator on creation; to another agent by the `SharedMemoryShare` syscall |

Two operations are syscalls, not capabilities of their own. `SharedMemoryShare` lets a region's creator grant `SharedMemoryAccess(id)` to another agent (the target design requires both agents' consent). `CapabilityTransfer` moves a token over a channel and requires a delegatable token.

### Agent manifest example

```toml
[agent]
name = "com.example.echo-service"
version = "1.0.0"

[capabilities.required]
channel_create = true     # Must be able to create channels to serve clients

[capabilities.optional]
shared_memory = true      # Large payload support (graceful degradation without)
```

## 6. Error Handling

```rust
/// Errors returned by IPC Kit operations (`shared/src/kits/ipc.rs`).
pub enum IpcKitError {
    /// The channel does not exist or was destroyed.
    /// Recovery: re-lookup the service.
    InvalidChannel { id: ChannelId },

    /// The message ring is full (16 slots).
    /// Recovery: back off and retry, or use shared memory for bulk transfer.
    ChannelFull { id: ChannelId, capacity: usize },

    /// The operation timed out (mandatory timeout expired).
    /// Recovery: the service may be overloaded — retry with backoff.
    Timeout { elapsed_ticks: u64 },

    /// The operation was cancelled (by IpcCancel or process exit).
    /// Recovery: clean up partial state, optionally retry.
    Cancelled,

    /// The agent does not hold the required capability.
    /// Recovery: request the capability or degrade gracefully.
    CapabilityDenied { required: Capability },

    /// A shared memory operation failed: the region does not exist, the
    /// caller has no mapping of it, it is already mapped, or the calling
    /// thread has no process.
    /// Recovery: verify the region ID and the mapping state.
    SharedMemoryError { reason: &'static str },

    /// The message exceeds the 256-byte inline limit.
    /// Recovery: use SharedMemoryRegion for large payloads.
    MessageTooLarge { size: usize, max: usize },

    /// No reply was received (server did not call ipc_reply).
    /// Recovery: the service may have crashed — check service health.
    NoReply,

    /// The operation would have to block: a non-blocking receive found no
    /// message, or another thread is already receiving on or calling
    /// through the channel.
    /// Recovery: retry later, or wait with a timeout.
    WouldBlock,

    /// The agent is SUSPENDED by the behavioral gate. It may still hold
    /// every capability; requesting more does not help.
    /// Recovery: none from the agent; the gate is lifted by AIRS or the user.
    Suspended,

    /// An argument is invalid (an out-of-range id, an undefined flag bit,
    /// a W^X request, a bad buffer or an unknown select entry kind), or the
    /// caller's state does not allow the call: no current thread or
    /// process, or a reply with no pending call.
    /// Recovery: for an argument error, fix the call; retrying unchanged
    /// fails the same way. A reply with no pending call can succeed once a
    /// call arrives.
    InvalidArgument { reason: &'static str },

    /// A table, queue or memory pool is full.
    /// Recovery: release objects the agent no longer needs, or retry later.
    ResourceExhausted { reason: &'static str },

    /// The operation is not available.
    Unsupported,

    /// The channel or shared memory region the call names is gone: it was
    /// never created or has been destroyed, or a channel endpoint is dead.
    /// Recovery: re-lookup the service or the region; retrying the same id
    /// fails the same way.
    ObjectGone,
}
```

### Mapping kernel error codes

The kernel returns the syscall-level codes of [`docs/kernel/ipc.md` §3.1](../../kernel/ipc.md) (`IpcError`, negative `i64`). The Kit decodes them through one table, `IpcKitError::from_code` in `shared/src/kits/ipc.rs`: `IpcError::try_from(i64)`, then `From<IpcError>`. The kernel's `KernelIpc` and a future EL0 Kit share it, so the two cannot drift. The table maps each code to the least specific variant that is correct for every kernel path returning it. `EPERM` and `ENOSPC` have no such variant, so their default is wrong for some paths, which their rows below name; a syscall on such a path with no Kit wrapper decodes to the wrong variant. `EPROTO` is a deliberate exception: its only kernel path today, `ipc_reply` with no pending call, would fit `InvalidArgument`, but the default stays `NoReply` so that `NoReply` survives the round trip through `EPROTO`, and `reply` overrides it. Each wrapper then overrides with what it knows (the lesson in `docs/knowledge/lessons/2026-03-24-cl-phase-5-m17-lossy-error-conversions.md`).

| Code | Default variant | Wrapper overrides |
|---|---|---|
| `EPERM` (missing capability or right) | `CapabilityDenied` (placeholder capability) | Every wrapper names the capability the kernel checks: `ChannelCreate` (`channel_create`), `ChannelAccess(id)` (`channel_destroy`, `send`, `recv`, `call`; `select` names the first channel entry the caller lacks), `SharedMemoryCreate` (`shmem_create`), `SharedMemoryAccess(id)` (`shmem_map`). Where the kernel checks none: `reply` → `InvalidArgument` (no current thread), `notification_create` → `InvalidArgument` (no current process), `shmem_unmap` → `SharedMemoryError` (not mapped). The default is wrong wherever the `EPERM` is not a missing capability: every syscall that checks no capability returns `EPERM` when the caller has no current thread or no process (`IpcReply`, `NotificationCreate`, `MemoryMap`, `MemoryUnmap`, `CapabilityAttenuate`, `CapabilityRevoke`, `CapabilityList`, `ProcessExit`, `ProcessWait`, `AuditLog` and `SharedMemoryShare`), and so does an unmap of a region the caller has not mapped (`MemoryUnmap` of a shared window address, or the in-kernel `shared_memory_unmap` behind `shmem_unmap`), `SharedMemoryShare` from a caller that is not the region's creator or to a target pid whose slot holds no process, and `ProcessWait` for a child pid whose slot holds no process or after a wake that finds no exit code ([`docs/kernel/ipc.md` §3.2](../../kernel/ipc.md)). Of these, only `reply`, `notification_create` and `shmem_unmap` have Kit wrappers; nothing overrides the other syscalls' `EPERM` |
| `EACCES` (SUSPENDED) | `Suspended` | — |
| `EINVAL` | `InvalidArgument` | Channel wrappers → `InvalidChannel { id }` (an out-of-range id) |
| `EPIPE` (the named object is gone) | `ObjectGone` | Channel wrappers → `InvalidChannel { id }` with the real `id`; shared memory wrappers → `SharedMemoryError` (region not found). The default names no id, so it is correct on the channel paths and on the region paths (`SharedMemoryMap`, `SharedMemoryShare`, `MemoryUnmap` of a shared window address and the in-kernel `shared_memory_unmap` behind `shmem_unmap`). `SharedMemoryShare` has no Kit wrapper, so a plain decode of its `EPIPE` reads as `ObjectGone` |
| `EAGAIN` | `WouldBlock` | `send` → `ChannelFull { id }` (its EAGAIN is a full ring) |
| `ENOSPC` | `ResourceExhausted` | The default is wrong wherever `ENOSPC` rejects a request above a fixed limit, where releasing objects or retrying cannot help: a payload above `MAX_MESSAGE_SIZE` (`IpcSend`, `IpcCall`, `IpcReply`), an `IpcCall` or `IpcRecv` receive length above `MAX_MESSAGE_SIZE`, a `DebugPrint` above 256 bytes and an `AuditLog` event above 48 bytes. `send`, `call`, `reply` → `MessageTooLarge` for an oversized payload (checked before the call); `recv` and `call` receive into a `MAX_MESSAGE_SIZE` buffer, so they never pass a longer receive length. `DebugPrint` and `AuditLog` have no Kit wrapper, so a plain decode of their fixed-limit `ENOSPC` reads as `ResourceExhausted`. (`MemoryMap` above 64 pages and `SharedMemoryCreate` above 4 MiB return `EINVAL`, so their `ENOSPC` is only a full table, [`docs/kernel/ipc.md` §4.7](../../kernel/ipc.md).) `call` → `ChannelFull { id }` for a full ring; `channel_create` → `ResourceExhausted` ("channel table full") |
| `ENOMEM` | `ResourceExhausted` | `notification_create` → "notification table full" |
| `ETIMEDOUT` | `Timeout` | — |
| `ECANCELED` | `Cancelled` | — |
| `EPROTO` | `NoReply` | `reply` → `InvalidArgument` (no call is pending on the channel). That is the only kernel path that returns `EPROTO` today, the replier's side, where `InvalidArgument` is correct. The default is kept as `NoReply` by choice, so that `NoReply` survives the round trip, and `reply`'s override corrects it on that path |
| `ENOTSUP` | `Unsupported` | — |
| `ECAPDORMANT` | `CapabilityDenied` | — |
| `EEXIST` | `SharedMemoryError` (only `shmem_map` returns it) | — |

A wrapper whose calling thread has no process reports the variant it reports for the kernel's `EPERM`: `CapabilityDenied` for the capability the operation checks, `InvalidArgument` for `notification_create` and `SharedMemoryError` for `shmem_unmap`, which check none. The syscalls return `EPERM` for a caller with no process ([`docs/kernel/ipc.md` §3.2](../../kernel/ipc.md)), so an EL0 wrapper applies the `EPERM` overrides above. (`ipc_select` and `notification_wait` called with no current thread at all, which only in-kernel code can do, return `EINVAL`, so `select` and `wait` then report `InvalidArgument`.) `KernelIpc` reports the same variants, whether the kernel function returns `EPERM` or the wrapper finds no process itself. The reverse conversion, `From<IpcKitError> for IpcError`, maps `CapabilityDenied` to `EPERM`, `Suspended` to `EACCES`, `InvalidChannel` and `ObjectGone` to `EPIPE` and `SharedMemoryError` to `EINVAL`; `InvalidChannel`, `ChannelFull`, `MessageTooLarge` and `SharedMemoryError` do not survive a round trip, because their codes decode to the less specific `ObjectGone`, `WouldBlock`, `ResourceExhausted` and `InvalidArgument`.

## 7. Platform & AI Availability

IPC Kit is a kernel primitive — it runs on all AIOS platforms with identical behavior.
Channel latency and throughput vary by hardware (QEMU ~8 us round-trip, Pi 4 target
~3 us, Apple Silicon target ~1 us).

When AIRS is online, it enhances IPC with:

- **Channel health monitoring**: AIRS tracks per-channel latency distributions and
  alerts agents when a service they depend on is degrading.
- **Anomaly detection**: the Behavioral Monitor flags unusual IPC patterns — a
  suddenly chatty agent, unexpected channel creation, or capability transfer to
  an untrusted agent.
- **Smart routing**: for services with multiple instances (replicated servers),
  AIRS can suggest load-balancing across channels based on observed latency.

Without AIRS, IPC works identically — agents just don't get proactive health
warnings or routing suggestions.

## For Kit Authors

### Designing a Kit's IPC protocol

Every Kit that exposes services to other agents communicates over IPC. The pattern:

```rust
/// 1. Define your message types as a flat enum with serialization.
#[repr(u8)]
pub enum MyKitMessage {
    DoOperation = 1,
    QueryStatus = 2,
    Subscribe = 3,
}

/// Reply codes of your Kit's protocol (not kernel errnos).
#[repr(u8)]
pub enum MyKitReply {
    UnknownMessage = 0xFF,
}

/// 2. Your Kit's service loop receives raw IPC messages,
///    deserializes, validates capabilities, then dispatches.
fn service_loop(channel: ChannelId) {
    let mut buf = [0u8; 256];
    loop {
        let (len, caller) = ipc_recv(channel, &mut buf, Duration::MAX).unwrap();
        match buf[0] {
            1 => {
                // Validate capability, execute, reply
                let result = do_operation(&buf[1..len]);
                ipc_reply(&result).unwrap();
            }
            2 => {
                let status = query_status();
                ipc_reply(&status).unwrap();
            }
            _ => ipc_reply(&[MyKitReply::UnknownMessage as u8]).unwrap(),
        }
    }
}
```

### Direct switch optimization

When a client calls `ipc_call()` and the server is already blocked in `ipc_recv()`,
the kernel performs a **direct thread switch** — it saves the caller's context and
restores the server's context without touching the scheduler. This skips the run queue
entirely and saves ~2 us per round-trip. Kit authors get this optimization for free
when using synchronous call/reply.

### Priority inheritance

If a high-priority agent calls into a low-priority service, the kernel temporarily
boosts the service thread's priority to match the caller's. This prevents priority
inversion. The boost is transitive (bounded to 8 levels) and automatic — Kit authors
don't need to manage it.

## Cross-References

- [IPC & Syscall Architecture](../../kernel/ipc.md) — kernel implementation details
- [Capability Kit](./capability.md) — capability transfer over channels
- [Memory Kit](./memory.md) — SharedMemoryRegion internals
- [App Kit](../application/app.md) — high-level message dispatch loop
- [Deadlock Prevention](../../kernel/deadlock-prevention.md) — timeout and priority inheritance design
