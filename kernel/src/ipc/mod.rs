//! IPC channels and synchronous call/reply.
//!
//! Provides channel creation/destruction, synchronous IpcCall/IpcRecv/IpcReply,
//! asynchronous IpcSend, IpcCancel, and mandatory timeouts.
//! Per ipc.md §3–4, §9.1.
//!
//! Phase 3 kernel threads invoke IPC via direct function calls (not SVC).
//! The SVC dispatch path is wired in parallel for future EL0 user threads.

mod channel;
pub mod direct;
pub mod notify;
pub mod select;
pub mod shmem;
mod tests;
mod timeout;

use crate::syscall::IpcError;
use crate::task::ThreadId;
use spin::Mutex;

// Re-export IPC types from shared crate.
pub use shared::{
    ChannelId, EndpointState, RawMessage, DEFAULT_TIMEOUT_TICKS, MAX_CHANNELS, MAX_MESSAGE_SIZE,
    RING_CAPACITY,
};

// Re-export channel operations so callers see the same public API.
pub use channel::{ipc_call, ipc_cancel, ipc_recv, ipc_reply, ipc_send};
pub(crate) use tests::channel_create_unchecked;
pub use tests::init;
pub(crate) use timeout::wake_with_error;
pub use timeout::{check_timeouts, current_thread_id, sleep_ticks};

// ---------------------------------------------------------------------------
// Message ring buffer
// ---------------------------------------------------------------------------

/// Fixed-capacity ring buffer for IPC messages.
pub(crate) struct MessageRing {
    entries: [RawMessage; RING_CAPACITY],
    head: usize,
    tail: usize,
    len: usize,
}

impl MessageRing {
    const fn new() -> Self {
        Self {
            entries: [const { RawMessage::EMPTY }; RING_CAPACITY],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    fn push(&mut self, msg: RawMessage) -> bool {
        if self.len >= RING_CAPACITY {
            return false;
        }
        self.entries[self.tail] = msg;
        self.tail = (self.tail + 1) % RING_CAPACITY;
        self.len += 1;
        true
    }

    fn pop(&mut self) -> Option<RawMessage> {
        if self.len == 0 {
            return None;
        }
        let msg = self.entries[self.head].clone();
        self.head = (self.head + 1) % RING_CAPACITY;
        self.len -= 1;
        Some(msg)
    }

    #[allow(dead_code)]
    fn is_empty(&self) -> bool {
        self.len == 0
    }
}

// ---------------------------------------------------------------------------
// Channel
// ---------------------------------------------------------------------------

/// Bidirectional IPC channel between two endpoints.
///
/// endpoint_a is the "creator" side, endpoint_b is the "peer" side.
/// Messages flow in both directions via a single ring buffer.
/// For synchronous IPC, the ring holds the request; the reply is
/// delivered directly to the blocked caller's reply buffer.
pub(crate) struct Channel {
    #[allow(dead_code)]
    pub(crate) id: ChannelId,
    pub(crate) state_a: EndpointState,
    pub(crate) state_b: EndpointState,
    /// Owner thread of endpoint A (creator).
    pub(crate) owner_a: ThreadId,
    /// Owner thread of endpoint B (peer).
    pub(crate) owner_b: Option<ThreadId>,
    /// Message ring buffer (requests and async sends).
    pub(crate) ring: MessageRing,
    /// Thread currently blocked in ipc_recv() on this channel, if any.
    pub(crate) waiting_receiver: Option<ThreadId>,
    /// Thread currently blocked in ipc_call() waiting for a reply.
    /// The receiver uses this to deliver the reply.
    pub(crate) pending_caller: Option<ThreadId>,
    /// Capability token that authorized this channel's creation.
    /// Used for cascade revocation: revoking this token destroys the channel.
    pub(crate) creation_cap: Option<shared::CapabilityTokenId>,
}

impl Channel {
    fn new(id: ChannelId, owner_a: ThreadId) -> Self {
        Self {
            id,
            state_a: EndpointState::Active,
            state_b: EndpointState::Active,
            owner_a,
            owner_b: None,
            ring: MessageRing::new(),
            waiting_receiver: None,
            pending_caller: None,
            creation_cap: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Global channel table
// ---------------------------------------------------------------------------

/// Backing array of `CHANNEL_TABLE`, indexed by `ChannelId::index()`.
type ChannelTable = [Option<Channel>; MAX_CHANNELS];

pub(crate) static CHANNEL_TABLE: Mutex<ChannelTable> = {
    const NONE: Option<Channel> = None;
    Mutex::new([NONE; MAX_CHANNELS])
};

/// Range-checked access to a channel's slot in a locked `CHANNEL_TABLE`.
///
/// Returns `Err(EINVAL)` when `id` is `>= MAX_CHANNELS`. No channel can exist
/// at such an id, so a raw index would run past the end of the table and
/// panic the kernel. Every channel lookup goes through this function.
fn channel_slot_mut(table: &mut ChannelTable, id: ChannelId) -> Result<&mut Option<Channel>, i64> {
    match id.index() {
        // `index()` only returns ids below MAX_CHANNELS, the length of the
        // table array, so this indexing cannot panic.
        Some(idx) => Ok(&mut table[idx]),
        None => Err(IpcError::Einval as i64),
    }
}

/// Look up a live channel in a locked `CHANNEL_TABLE`.
///
/// Returns `Err(EINVAL)` when `id` is out of range (see `channel_slot_mut`)
/// and `Err(EPIPE)` when the slot is empty, i.e. the channel was destroyed or
/// never created.
fn channel_mut(table: &mut ChannelTable, id: ChannelId) -> Result<&mut Channel, i64> {
    channel_slot_mut(table, id)?
        .as_mut()
        .ok_or(IpcError::Epipe as i64)
}

// ---------------------------------------------------------------------------
// Channel create / destroy
// ---------------------------------------------------------------------------

/// Create a new IPC channel. Returns channel_id.
///
/// The creator thread owns endpoint A. Endpoint B can be assigned to
/// another thread via `channel_set_peer()`.
///
/// Requires `Capability::ChannelCreate` on the creator's process.
pub fn channel_create(creator: ThreadId) -> Result<ChannelId, i64> {
    // Capability enforcement: check ChannelCreate (fail-closed).
    // Returns the authorizing token ID for cascade revocation tracking.
    let pid = crate::cap::process_of_thread(creator).ok_or(IpcError::Eperm as i64)?;
    let auth_token = crate::cap::check_channel_create(pid)?;

    let mut table = CHANNEL_TABLE.lock();
    // Find a free slot.
    let slot = table.iter().position(|s| s.is_none());
    let idx = match slot {
        Some(i) => i,
        None => return Err(IpcError::Enospc as i64),
    };

    let id = ChannelId(idx as u32);
    let mut ch = Channel::new(id, creator);
    ch.creation_cap = Some(auth_token);
    table[idx] = Some(ch);
    crate::kinfo!(Ipc, "Channel {} created by thread {}", idx, creator.0);
    Ok(id)
}

/// Set the peer (endpoint B) owner of a channel.
pub fn channel_set_peer(channel: ChannelId, peer: ThreadId) -> Result<(), i64> {
    let mut table = CHANNEL_TABLE.lock();
    let ch = channel_mut(&mut table, channel)?;
    ch.owner_b = Some(peer);
    Ok(())
}

/// Destroy a channel. Any thread blocked on this channel is woken with EPIPE.
///
/// Requires `Capability::ChannelAccess(channel)` on the caller's process.
pub fn channel_destroy(channel: ChannelId) -> Result<(), i64> {
    // Capability enforcement: check ChannelAccess (fail-closed).
    let pid = crate::cap::current_process_id().ok_or(IpcError::Eperm as i64)?;
    crate::cap::check_channel_access(pid, channel)?;

    channel_destroy_unchecked(channel)
}

/// Internal channel destruction — bypasses capability checks.
/// Used by cascade revocation (kernel-initiated teardown).
pub(crate) fn channel_destroy_unchecked(channel: ChannelId) -> Result<(), i64> {
    let mut table = CHANNEL_TABLE.lock();
    let ch = channel_slot_mut(&mut table, channel)?
        .take()
        .ok_or(IpcError::Epipe as i64)?;

    // Wake any blocked threads with EPIPE (both receiver and caller).
    let wake_recv = ch.waiting_receiver;
    let wake_caller = ch.pending_caller;
    drop(table);
    if let Some(recv_tid) = wake_recv {
        timeout::wake_with_error(recv_tid, IpcError::Epipe as i64);
    }
    if let Some(caller_tid) = wake_caller {
        timeout::wake_with_error(caller_tid, IpcError::Epipe as i64);
    }

    crate::kinfo!(Ipc, "Channel {} destroyed", channel.0);
    Ok(())
}

// ---------------------------------------------------------------------------
// IPC Kit trait implementations
// ---------------------------------------------------------------------------

use shared::kits::ipc::{self as ipc_kit, IpcKitError};
use shared::{Capability, SelectEntry, SelectKind};

/// Kernel-side implementation of the IPC Kit traits.
///
/// A zero-sized unit struct that delegates to the global IPC subsystem
/// (CHANNEL_TABLE, NOTIFICATION_TABLE, SHARED_REGION_TABLE, etc.).
///
/// Errors go through the one errno table, `IpcKitError::from_code`, and each
/// wrapper then overrides what it knows better (docs/kits/kernel/ipc.md §6):
/// the real channel id, the capability the kernel actually checked, and what
/// an errno means for its operation. Where the kernel checks no capability,
/// no wrapper reports `CapabilityDenied`.
#[allow(dead_code)]
pub struct KernelIpc;

/// Kit error for a failed operation on channel `id`.
///
/// A channel operation's only capability check is `ChannelAccess(id)`
/// (`channel_destroy`, `ipc_send`, `ipc_recv`, `ipc_call`), so its EPERM
/// names that capability; EINVAL (an out-of-range id) and EPIPE (destroyed or
/// dead) both mean channel `id` does not exist. Everything else, EAGAIN
/// included (`WouldBlock`), comes from the errno table.
#[allow(dead_code)] // used only by KernelIpc, which nothing constructs yet
fn channel_kit_err(id: ChannelId, code: i64) -> IpcKitError {
    match IpcError::try_from(code) {
        Ok(IpcError::Eperm) => IpcKitError::CapabilityDenied {
            required: Capability::ChannelAccess(id),
        },
        Ok(IpcError::Einval | IpcError::Epipe) => IpcKitError::InvalidChannel { id },
        _ => IpcKitError::from_code(code),
    }
}

/// Kit error for a failed shared memory operation.
///
/// `checked` is the capability the kernel checks for the operation, or
/// `None` for `shared_memory_unmap`, which checks none: there EPERM means the
/// caller has no mapping of the region. EPIPE means the region does not
/// exist, not a channel.
#[allow(dead_code)] // used only by KernelIpc, which nothing constructs yet
fn shm_kit_err(code: i64, checked: Option<Capability>) -> IpcKitError {
    match IpcError::try_from(code) {
        Ok(IpcError::Eperm) => match checked {
            Some(required) => IpcKitError::CapabilityDenied { required },
            None => IpcKitError::SharedMemoryError {
                reason: "not mapped",
            },
        },
        Ok(IpcError::Epipe) => IpcKitError::SharedMemoryError {
            reason: "region not found",
        },
        _ => IpcKitError::from_code(code),
    }
}

/// The payload of `msg`, or `MessageTooLarge` if its `len` exceeds the
/// inline limit (slicing `data` by such a `len` would panic).
#[allow(dead_code)] // used only by KernelIpc, which nothing constructs yet
fn msg_payload(msg: &RawMessage) -> Result<&[u8], IpcKitError> {
    msg.data.get(..msg.len).ok_or(IpcKitError::MessageTooLarge {
        size: msg.len,
        max: MAX_MESSAGE_SIZE,
    })
}

/// The first channel entry in `entries` whose `ChannelAccess` the calling
/// thread's process does not hold, looked up without logging a denial (the
/// `ipc_select` call that failed already logged it).
#[allow(dead_code)] // used only by KernelIpc, which nothing constructs yet
fn first_denied_channel(entries: &[SelectEntry]) -> Option<ChannelId> {
    let now = crate::arch::aarch64::timer::TICK_COUNT.load(core::sync::atomic::Ordering::Relaxed);
    let pid = crate::cap::current_process_id();
    let table = crate::task::process::PROCESS_TABLE.lock();
    let proc = pid.and_then(|p| crate::task::process::process_ref(&table, p).ok());
    entries.iter().find_map(|entry| match entry.kind {
        SelectKind::Channel(ch)
            if !proc.is_some_and(|p| {
                p.cap_table
                    .has_capability(&Capability::ChannelAccess(ch), now)
            }) =>
        {
            Some(ch)
        }
        _ => None,
    })
}

impl ipc_kit::ChannelOps for KernelIpc {
    fn channel_create(&mut self) -> Result<ChannelId, IpcKitError> {
        let tid = current_thread_id().ok_or(IpcKitError::CapabilityDenied {
            required: Capability::ChannelCreate,
        })?;
        channel_create(tid).map_err(|code| match IpcError::try_from(code) {
            Ok(IpcError::Eperm) => IpcKitError::CapabilityDenied {
                required: Capability::ChannelCreate,
            },
            // ENOSPC from channel_create is a full channel table; there is no
            // channel yet, so ChannelFull (a full ring) does not apply.
            Ok(IpcError::Enospc) => IpcKitError::ResourceExhausted {
                reason: "channel table full",
            },
            _ => IpcKitError::from_code(code),
        })
    }

    fn channel_destroy(&mut self, id: ChannelId) -> Result<(), IpcKitError> {
        channel_destroy(id).map_err(|code| channel_kit_err(id, code))
    }

    fn send(&self, id: ChannelId, msg: &RawMessage) -> Result<(), IpcKitError> {
        let code = ipc_send(id, msg_payload(msg)?);
        if code >= 0 {
            return Ok(());
        }
        Err(match IpcError::try_from(code) {
            // ipc_send returns EAGAIN when the ring is full.
            Ok(IpcError::Eagain) => IpcKitError::ChannelFull {
                id,
                capacity: RING_CAPACITY,
            },
            _ => channel_kit_err(id, code),
        })
    }

    fn recv(&self, id: ChannelId, timeout_ticks: u64) -> Result<RawMessage, IpcKitError> {
        let mut buf = [0u8; MAX_MESSAGE_SIZE];
        // EAGAIN here is an empty non-blocking poll or a receiver already
        // waiting: WouldBlock, from the table, not ChannelFull.
        let (bytes, sender) =
            ipc_recv(id, &mut buf, timeout_ticks).map_err(|code| channel_kit_err(id, code))?;
        let mut msg = RawMessage::EMPTY;
        msg.sender = sender;
        msg.len = bytes;
        msg.data[..bytes].copy_from_slice(&buf[..bytes]);
        Ok(msg)
    }

    fn call(
        &self,
        id: ChannelId,
        request: &RawMessage,
        timeout_ticks: u64,
    ) -> Result<RawMessage, IpcKitError> {
        let mut recv_buf = [0u8; MAX_MESSAGE_SIZE];
        let code = ipc_call(id, msg_payload(request)?, &mut recv_buf, timeout_ticks);
        if code < 0 {
            return Err(match IpcError::try_from(code) {
                // The request fits (msg_payload checked it), so ENOSPC from
                // ipc_call is a full ring. Its EAGAIN is another caller
                // already pending on the channel: WouldBlock, from the table.
                Ok(IpcError::Enospc) => IpcKitError::ChannelFull {
                    id,
                    capacity: RING_CAPACITY,
                },
                _ => channel_kit_err(id, code),
            });
        }
        let bytes = code as usize;
        let mut msg = RawMessage::EMPTY;
        msg.len = bytes;
        msg.data[..bytes].copy_from_slice(&recv_buf[..bytes]);
        Ok(msg)
    }

    fn reply(&self, id: ChannelId, msg: &RawMessage) -> Result<(), IpcKitError> {
        let code = ipc_reply(id, msg_payload(msg)?);
        if code >= 0 {
            return Ok(());
        }
        Err(match IpcError::try_from(code) {
            // ipc_reply checks no capability (ipc.md §9.1); its only EPERM is
            // a caller with no current thread.
            Ok(IpcError::Eperm) => IpcKitError::InvalidArgument {
                reason: "no current thread",
            },
            // ipc_reply's EPROTO is a reply on a channel with no pending
            // call. NoReply, the table's default, is the caller's side.
            Ok(IpcError::Eproto) => IpcKitError::InvalidArgument {
                reason: "no pending call",
            },
            _ => channel_kit_err(id, code),
        })
    }
}

impl ipc_kit::NotificationOps for KernelIpc {
    fn notification_create(&mut self) -> Result<shared::NotificationId, IpcKitError> {
        // notification_create checks no capability, so a caller with no
        // process is not a capability denial.
        let pid = crate::cap::current_process_id().ok_or(IpcKitError::InvalidArgument {
            reason: "no current process",
        })?;
        notify::notification_create(pid).map_err(|code| match IpcError::try_from(code) {
            // notification_create's only ENOMEM is a full notification table.
            Ok(IpcError::Enomem) => IpcKitError::ResourceExhausted {
                reason: "notification table full",
            },
            _ => IpcKitError::from_code(code),
        })
    }

    fn signal(&self, id: shared::NotificationId, bits: u64) -> Result<(), IpcKitError> {
        notify::notification_signal(id, bits).map_err(IpcKitError::from_code)
    }

    fn wait(
        &self,
        id: shared::NotificationId,
        mask: u64,
        timeout_ticks: u64,
    ) -> Result<u64, IpcKitError> {
        notify::notification_wait(id, mask, timeout_ticks).map_err(IpcKitError::from_code)
    }
}

impl ipc_kit::SelectOps for KernelIpc {
    fn select(
        &self,
        entries: &[SelectEntry],
        timeout_ticks: u64,
    ) -> Result<(usize, u64), IpcKitError> {
        select::ipc_select(entries, timeout_ticks).map_err(|code| match IpcError::try_from(code) {
            // ipc_select checks ChannelAccess for every channel entry and
            // reports only EPERM; name the channel the caller lacks.
            Ok(IpcError::Eperm) => match first_denied_channel(entries) {
                Some(ch) => IpcKitError::CapabilityDenied {
                    required: Capability::ChannelAccess(ch),
                },
                None => IpcKitError::from_code(code),
            },
            _ => IpcKitError::from_code(code),
        })
    }
}

impl ipc_kit::SharedMemoryOps for KernelIpc {
    fn shmem_create(
        &mut self,
        size: usize,
        flags: u64,
    ) -> Result<shared::SharedMemoryId, IpcKitError> {
        let pid = crate::cap::current_process_id().ok_or(IpcKitError::CapabilityDenied {
            required: Capability::SharedMemoryCreate,
        })?;
        let vm_flags = crate::mm::pgtable::VmFlags::from_caller_bits(flags).map_err(|_| {
            IpcKitError::InvalidArgument {
                reason: "undefined flag bits",
            }
        })?;
        shmem::shared_memory_create(pid, size, vm_flags)
            .map_err(|code| shm_kit_err(code, Some(Capability::SharedMemoryCreate)))
    }

    fn shmem_map(
        &mut self,
        id: shared::SharedMemoryId,
        _vaddr: shared::VirtAddr,
        flags: u64,
    ) -> Result<(), IpcKitError> {
        // shared_memory_map checks SharedMemoryAccess(id).
        let required = Capability::SharedMemoryAccess(id.0);
        let pid =
            crate::cap::current_process_id().ok_or(IpcKitError::CapabilityDenied { required })?;
        let vm_flags = crate::mm::pgtable::VmFlags::from_caller_bits(flags).map_err(|_| {
            IpcKitError::InvalidArgument {
                reason: "undefined flag bits",
            }
        })?;
        shmem::shared_memory_map(pid, id, vm_flags)
            .map(|_va| ())
            .map_err(|code| shm_kit_err(code, Some(required)))
    }

    fn shmem_unmap(&mut self, id: shared::SharedMemoryId) -> Result<(), IpcKitError> {
        // shared_memory_unmap checks no capability.
        let pid = crate::cap::current_process_id().ok_or(IpcKitError::SharedMemoryError {
            reason: "no current process",
        })?;
        shmem::shared_memory_unmap(pid, id).map_err(|code| shm_kit_err(code, None))
    }

    fn shmem_destroy(&mut self, id: shared::SharedMemoryId) -> Result<(), IpcKitError> {
        // No dedicated kernel destroy function exists. Walk the region's
        // mappings and unmap each one. The last unmap frees backing pages.
        let pids_to_unmap: alloc::vec::Vec<crate::task::process::ProcessId> = {
            let idx = id.0 as usize;
            if idx >= shared::MAX_SHARED_REGIONS {
                return Err(IpcKitError::SharedMemoryError {
                    reason: "invalid region id",
                });
            }
            let table = shmem::SHARED_REGION_TABLE.lock();
            match &table[idx] {
                Some(region) => region
                    .mappings
                    .iter()
                    .filter_map(|m| m.as_ref().map(|mapping| mapping.pid))
                    .collect(),
                None => {
                    return Err(IpcKitError::SharedMemoryError {
                        reason: "region not found",
                    })
                }
            }
        };

        let mut first_err: Option<IpcKitError> = None;
        for pid in pids_to_unmap {
            if let Err(code) = shmem::shared_memory_unmap(pid, id) {
                if first_err.is_none() {
                    first_err = Some(shm_kit_err(code, None));
                }
            }
        }

        if let Some(err) = first_err {
            Err(err)
        } else {
            Ok(())
        }
    }
}
