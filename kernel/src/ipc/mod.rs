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

pub(crate) static CHANNEL_TABLE: Mutex<[Option<Channel>; MAX_CHANNELS]> = {
    const NONE: Option<Channel> = None;
    Mutex::new([NONE; MAX_CHANNELS])
};

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
    let ch = match &mut table[channel.0 as usize] {
        Some(c) => c,
        None => return Err(IpcError::Epipe as i64),
    };
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
    let idx = channel.0 as usize;
    let ch = match table[idx].take() {
        Some(c) => c,
        None => return Err(IpcError::Epipe as i64),
    };

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

    crate::kinfo!(Ipc, "Channel {} destroyed", idx);
    Ok(())
}

// ---------------------------------------------------------------------------
// IPC Kit trait implementations
// ---------------------------------------------------------------------------

use shared::kits::ipc::{self as ipc_kit, IpcKitError};

/// Kernel-side implementation of the IPC Kit traits.
///
/// A zero-sized unit struct that delegates to the global IPC subsystem
/// (CHANNEL_TABLE, NOTIFICATION_TABLE, SHARED_REGION_TABLE, etc.).
#[allow(dead_code)]
pub struct KernelIpc;

/// Convert a raw i64 error code to an IpcKitError.
#[allow(dead_code)]
fn i64_to_kit_err(code: i64) -> IpcKitError {
    // Try to interpret as a known IpcError discriminant.
    match code {
        x if x == IpcError::Etimedout as i64 => IpcKitError::Timeout { elapsed_ticks: 0 },
        x if x == IpcError::Epipe as i64 => IpcKitError::InvalidChannel { id: ChannelId(0) },
        x if x == IpcError::Eagain as i64 => IpcKitError::ChannelFull {
            id: ChannelId(0),
            capacity: RING_CAPACITY,
        },
        x if x == IpcError::Ecanceled as i64 => IpcKitError::Cancelled,
        x if x == IpcError::Eacces as i64 || x == IpcError::Eperm as i64 => {
            IpcKitError::CapabilityDenied {
                required: shared::Capability::ChannelCreate,
            }
        }
        x if x == IpcError::Enospc as i64 => IpcKitError::MessageTooLarge {
            size: 0,
            max: MAX_MESSAGE_SIZE,
        },
        x if x == IpcError::Eproto as i64 => IpcKitError::NoReply,
        x if x == IpcError::Enomem as i64 => IpcKitError::SharedMemoryError {
            reason: "out of memory",
        },
        x if x == IpcError::Eexist as i64 => IpcKitError::SharedMemoryError {
            reason: "already exists",
        },
        x if x == IpcError::Einval as i64 => IpcKitError::InvalidChannel { id: ChannelId(0) },
        _ => IpcKitError::SharedMemoryError {
            reason: "unknown error",
        },
    }
}

impl ipc_kit::ChannelOps for KernelIpc {
    fn channel_create(&mut self) -> Result<ChannelId, IpcKitError> {
        let tid = current_thread_id().ok_or(IpcKitError::CapabilityDenied {
            required: shared::Capability::ChannelCreate,
        })?;
        channel_create(tid).map_err(|code| {
            // Enospc from channel_create means "table full", not "message too large".
            if code == IpcError::Enospc as i64 {
                IpcKitError::ChannelFull {
                    id: ChannelId(0),
                    capacity: MAX_CHANNELS,
                }
            } else {
                i64_to_kit_err(code)
            }
        })
    }

    fn channel_destroy(&mut self, id: ChannelId) -> Result<(), IpcKitError> {
        channel_destroy(id).map_err(i64_to_kit_err)
    }

    fn send(&self, id: ChannelId, msg: &RawMessage) -> Result<(), IpcKitError> {
        let code = ipc_send(id, &msg.data[..msg.len]);
        if code < 0 {
            Err(i64_to_kit_err(code))
        } else {
            Ok(())
        }
    }

    fn recv(&self, id: ChannelId, timeout_ticks: u64) -> Result<RawMessage, IpcKitError> {
        let mut buf = [0u8; MAX_MESSAGE_SIZE];
        let (bytes, sender) = ipc_recv(id, &mut buf, timeout_ticks).map_err(i64_to_kit_err)?;
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
        let code = ipc_call(
            id,
            &request.data[..request.len],
            &mut recv_buf,
            timeout_ticks,
        );
        if code < 0 {
            Err(i64_to_kit_err(code))
        } else {
            let bytes = code as usize;
            let mut msg = RawMessage::EMPTY;
            msg.len = bytes;
            msg.data[..bytes].copy_from_slice(&recv_buf[..bytes]);
            Ok(msg)
        }
    }

    fn reply(&self, id: ChannelId, msg: &RawMessage) -> Result<(), IpcKitError> {
        let code = ipc_reply(id, &msg.data[..msg.len]);
        if code < 0 {
            Err(i64_to_kit_err(code))
        } else {
            Ok(())
        }
    }
}

impl ipc_kit::NotificationOps for KernelIpc {
    fn notification_create(&mut self) -> Result<shared::NotificationId, IpcKitError> {
        let pid = crate::cap::current_process_id().ok_or(IpcKitError::CapabilityDenied {
            required: shared::Capability::ChannelCreate,
        })?;
        notify::notification_create(pid).map_err(i64_to_kit_err)
    }

    fn signal(&self, id: shared::NotificationId, bits: u64) -> Result<(), IpcKitError> {
        notify::notification_signal(id, bits);
        Ok(())
    }

    fn wait(
        &self,
        id: shared::NotificationId,
        mask: u64,
        timeout_ticks: u64,
    ) -> Result<u64, IpcKitError> {
        notify::notification_wait(id, mask, timeout_ticks).map_err(i64_to_kit_err)
    }
}

impl ipc_kit::SelectOps for KernelIpc {
    fn select(
        &self,
        entries: &[shared::SelectEntry],
        timeout_ticks: u64,
    ) -> Result<(usize, u64), IpcKitError> {
        select::ipc_select(entries, timeout_ticks).map_err(i64_to_kit_err)
    }
}

impl ipc_kit::SharedMemoryOps for KernelIpc {
    fn shmem_create(
        &mut self,
        size: usize,
        flags: u64,
    ) -> Result<shared::SharedMemoryId, IpcKitError> {
        let pid = crate::cap::current_process_id().ok_or(IpcKitError::CapabilityDenied {
            required: shared::Capability::SharedMemoryCreate,
        })?;
        let vm_flags = crate::mm::pgtable::VmFlags::from_bits(flags as u32);
        shmem::shared_memory_create(pid, size, vm_flags).map_err(i64_to_kit_err)
    }

    fn shmem_map(
        &mut self,
        id: shared::SharedMemoryId,
        _vaddr: shared::VirtAddr,
        flags: u64,
    ) -> Result<(), IpcKitError> {
        let pid = crate::cap::current_process_id().ok_or(IpcKitError::CapabilityDenied {
            required: shared::Capability::SharedMemoryCreate,
        })?;
        let vm_flags = crate::mm::pgtable::VmFlags::from_bits(flags as u32);
        shmem::shared_memory_map(pid, id, vm_flags)
            .map(|_va| ())
            .map_err(i64_to_kit_err)
    }

    fn shmem_unmap(&mut self, id: shared::SharedMemoryId) -> Result<(), IpcKitError> {
        let pid = crate::cap::current_process_id().ok_or(IpcKitError::CapabilityDenied {
            required: shared::Capability::SharedMemoryCreate,
        })?;
        shmem::shared_memory_unmap(pid, id).map_err(i64_to_kit_err)
    }

    fn shmem_destroy(&mut self, id: shared::SharedMemoryId) -> Result<(), IpcKitError> {
        // No dedicated kernel destroy function exists. Walk the region's
        // mappings and unmap each one. The last unmap frees backing pages.
        let pids_to_unmap: alloc::vec::Vec<crate::task::process::ProcessId> = {
            let table = shmem::SHARED_REGION_TABLE.lock();
            let idx = id.0 as usize;
            match &table[idx] {
                Some(region) => region
                    .mappings
                    .iter()
                    .filter_map(|m| m.as_ref().map(|mapping| mapping.pid))
                    .collect(),
                None => {
                    return Err(IpcKitError::InvalidChannel {
                        id: ChannelId(id.0),
                    })
                }
            }
        };

        for pid in pids_to_unmap {
            let _ = shmem::shared_memory_unmap(pid, id);
        }
        Ok(())
    }
}
