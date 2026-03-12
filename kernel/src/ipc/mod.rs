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
#[allow(dead_code)]
pub(crate) struct Channel {
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
