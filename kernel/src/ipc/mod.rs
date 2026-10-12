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
mod scan;
pub mod select;
pub mod shmem;
mod tests;
mod timeout;

use crate::syscall::IpcError;
use crate::task::process::ProcessId;
use crate::task::ThreadId;
use shared::tripwire::WakeSource;
use spin::Mutex;

// Re-export IPC types from shared crate.
pub use shared::{
    ChannelId, EndpointState, RawMessage, DEFAULT_TIMEOUT_TICKS, MAX_CHANNELS, MAX_MESSAGE_SIZE,
    RING_CAPACITY,
};

// Re-export channel operations so callers see the same public API.
pub use channel::{ipc_call, ipc_cancel, ipc_recv, ipc_reply, ipc_send};
pub(crate) use scan::scan_wakers;
pub use tests::init;
pub(crate) use timeout::wake_with_error;
pub use timeout::{check_timeouts, current_thread_id, sleep_ticks};

/// Visit the lock words of the IPC's IRQ-class locks (TIMEOUT_QUEUE,
/// WAKEUP_ERRORS, NOTIFY_DEADLINES, NOTIFICATION_TABLE), for
/// `sync::held_by_stream`.
pub(crate) fn irq_lock_words(f: &mut impl FnMut(u64)) {
    timeout::irq_lock_words(f);
    notify::irq_lock_words(f);
}

/// What arming a wait's deadline found (`notify::arm_deadline` for
/// NOTIFY_DEADLINES, `timeout::arm_timeout` for TIMEOUT_QUEUE): the deadline
/// is published, or it has already passed (`TICK_COUNT >= deadline`, read in
/// the publishing hold), so the wait must not block and its result is
/// ETIMEDOUT unless its condition was met first (a sleep simply ends).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DeadlineArm {
    /// Published (or the wait has no deadline): the wait may block.
    Armed,
    /// The deadline has passed; nothing was published.
    Expired,
}

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

/// A channel's pending `ipc_call`: the calling thread and the call's
/// sequence number (`timeout::next_call_seq`), the same `seq` its reply slot
/// carries. A claimer that takes it writes the slot only on a `seq` match,
/// so a claim on one call never lands in the same thread's next call.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct PendingCall {
    pub(crate) tid: ThreadId,
    pub(crate) seq: u32,
}

/// Bidirectional IPC channel between two endpoints.
///
/// endpoint_a is the "creator" side, endpoint_b is the "peer" side.
/// Messages flow in both directions via a single ring buffer.
/// For synchronous IPC, the ring holds the request; the reply is
/// delivered directly to the blocked caller's reply buffer.
pub(crate) struct Channel {
    /// The channel's full id (slot and generation), as issued at creation.
    pub(crate) id: ChannelId,
    pub(crate) state_a: EndpointState,
    pub(crate) state_b: EndpointState,
    /// Process that owns endpoint A (creator), recorded at creation.
    /// `process_exit` matches endpoints on this. A process, not a thread:
    /// kernel services create channels before their threads exist, for
    /// label tids (0x700, 0x201, ...) that name no thread-table slot, so a
    /// thread id could not be mapped back to its process at exit.
    pub(crate) owner_a: ProcessId,
    /// Process that owns endpoint B (peer), recorded by `channel_set_peer`.
    pub(crate) owner_b: Option<ProcessId>,
    /// Message ring buffer (requests and async sends).
    pub(crate) ring: MessageRing,
    /// Thread waiting in ipc_recv() or ipc_select() on this channel, if any:
    /// armed for its wait (`wake::prepare_block`) before it registers here,
    /// and blocked or about to block. A waker takes it, then wakes it.
    pub(crate) waiting_receiver: Option<ThreadId>,
    /// Thread waiting in ipc_call() for a reply to its queued request: armed
    /// for its wait (`wake::prepare_block`) before it registers here, with
    /// its reply slot already written. A replier or canceller claims the
    /// call by taking it, then writes the reply slot only if the slot's
    /// `seq` is the one it claimed, and wakes the caller.
    pub(crate) pending_caller: Option<PendingCall>,
    /// Process whose checked `channel_create` made this channel, or `None`
    /// for a kernel channel (`channel_create_unchecked`). The revoke cascade
    /// looks up `creation_cap` in this process's capability table. Not
    /// `owner_a`, which kernel channels also set and `process_exit` matches.
    pub(crate) creator: Option<ProcessId>,
    /// Token in `creator`'s table that authorised the create, or `None` for a
    /// kernel channel. Revoking it, or a same-table ancestor, destroys the
    /// channel; kernel channels never cascade.
    pub(crate) creation_cap: Option<shared::CapabilityTokenId>,
}

impl Channel {
    fn new(id: ChannelId, owner_a: ProcessId) -> Self {
        Self {
            id,
            state_a: EndpointState::Active,
            state_b: EndpointState::Active,
            owner_a,
            owner_b: None,
            ring: MessageRing::new(),
            waiting_receiver: None,
            pending_caller: None,
            creator: None,
            creation_cap: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Global channel table
// ---------------------------------------------------------------------------

/// One `CHANNEL_TABLE` slot: its channel, if any, and the generation that the
/// slot's channel carries, or that its next channel will carry.
///
/// A channel in the slot always has the id `ChannelId::new(slot, generation)`.
/// `free` is the only way a channel leaves its slot: it advances the
/// generation in the same `CHANNEL_TABLE` hold, so an id issued before the
/// free never matches the slot again. A slot freed at `ID_GEN_MAX` is retired
/// (`ID_GEN_RETIRED`) and never allocated again.
pub(crate) struct ChannelSlot {
    /// Set by `free` only; private to the IPC module.
    generation: u32,
    /// The channel in the slot. Walks of the table read and update it in
    /// place; only `channel_alloc` and `free` add or remove one.
    pub(crate) channel: Option<Channel>,
}

impl ChannelSlot {
    /// A never-used slot: empty, at the first generation.
    const EMPTY: Self = Self {
        generation: shared::ID_GEN_FIRST,
        channel: None,
    };

    /// Whether `channel_alloc` may put a new channel here: empty and not
    /// retired.
    fn is_free(&self) -> bool {
        self.channel.is_none() && self.generation != shared::ID_GEN_RETIRED
    }

    /// Take the channel out of the slot if it is the one `id` names, and
    /// advance the slot's generation (retiring it at `ID_GEN_MAX`). Returns
    /// `None`, changing nothing, when the slot is empty or holds another
    /// generation's channel.
    fn free(&mut self, id: ChannelId) -> Option<Channel> {
        if self.channel.as_ref()?.id != id {
            return None;
        }
        self.generation = shared::next_generation(self.generation);
        self.channel.take()
    }
}

/// Backing array of `CHANNEL_TABLE`, indexed by `ChannelId::index()`.
pub(crate) type ChannelTable = [ChannelSlot; MAX_CHANNELS];

pub(crate) static CHANNEL_TABLE: Mutex<ChannelTable> =
    Mutex::new([ChannelSlot::EMPTY; MAX_CHANNELS]);

/// Range-checked access to a channel's slot in a locked `CHANNEL_TABLE`.
///
/// Returns `Err(EINVAL)` when `id`'s slot is `>= MAX_CHANNELS` (malformed).
/// No channel can exist at such an id, so a raw index would run past the end
/// of the table and panic the kernel. Every channel lookup goes through this
/// function.
fn channel_slot_mut(table: &mut ChannelTable, id: ChannelId) -> Result<&mut ChannelSlot, i64> {
    match id.index() {
        // `index()` only returns ids below MAX_CHANNELS, the length of the
        // table array, so this indexing cannot panic.
        Some(idx) => Ok(&mut table[idx]),
        None => {
            crate::observability::tripwire::bump(
                shared::tripwire::Key::Badchan,
                shared::tripwire::BadchanSite::Slot.index(),
            );
            Err(IpcError::Einval as i64)
        }
    }
}

/// Look up the live channel `id` names in a locked `CHANNEL_TABLE`.
///
/// Returns `Err(EINVAL)` when `id`'s slot is malformed (see
/// `channel_slot_mut`) and `Err(EPIPE)` when the slot is empty or holds a
/// channel with another id (the full `u32`, generation included): the
/// channel was destroyed or never created, and a stale id never reaches the
/// channel now in its slot.
pub(crate) fn channel_mut(table: &mut ChannelTable, id: ChannelId) -> Result<&mut Channel, i64> {
    match &mut channel_slot_mut(table, id)?.channel {
        Some(ch) if ch.id == id => Ok(ch),
        _ => Err(IpcError::Epipe as i64),
    }
}

// ---------------------------------------------------------------------------
// Channel create / destroy
// ---------------------------------------------------------------------------

/// Put a new channel in the lowest free slot of `CHANNEL_TABLE`, at that
/// slot's generation. Endpoint A belongs to `owner`. `checked` is the token
/// that authorised a checked create, which also makes `owner` the channel's
/// `creator`; `None` makes a kernel channel, which never cascades.
///
/// Returns `Err(ENOSPC)` when no slot is free (each holds a channel or is
/// retired). The one allocator of `channel_create` and
/// `channel_create_unchecked`.
fn channel_alloc(
    owner: ProcessId,
    checked: Option<shared::CapabilityTokenId>,
) -> Result<ChannelId, i64> {
    let mut table = CHANNEL_TABLE.lock();
    let idx = table
        .iter()
        .position(ChannelSlot::is_free)
        .ok_or(IpcError::Enospc as i64)?;
    let slot = &mut table[idx];
    // A free slot is not retired, so its generation is one `new` accepts.
    let id = ChannelId::new(idx, slot.generation);
    let mut ch = Channel::new(id, owner);
    ch.creator = checked.map(|_| owner);
    ch.creation_cap = checked;
    slot.channel = Some(ch);
    Ok(id)
}

/// Create a new IPC channel. Returns its id and the handle of the creator's
/// `ChannelAccess(id)` token.
///
/// The creator thread's process owns endpoint A and is the channel's
/// `creator`. Endpoint B can be assigned to a process via
/// `channel_set_peer()`.
///
/// Requires `Capability::ChannelCreate` on the creator's process. The
/// channel is published and its `ChannelAccess` minted as a child of the
/// authorising token in one `PROCESS_TABLE` hold (`cap::create_with_access`),
/// so revoking that token revokes the access and destroys the channel.
/// Errors: EPERM for a thread with no process or a process without a live
/// `ChannelCreate`; ENOSPC when the process's capability table or the
/// channel table is full.
pub fn channel_create(creator: ThreadId) -> Result<(ChannelId, shared::CapabilityHandle), i64> {
    let pid = crate::cap::process_of_thread(creator).ok_or(IpcError::Eperm as i64)?;
    let (id, handle) = crate::cap::create_with_access(pid, Capability::ChannelCreate, |auth| {
        let id = channel_alloc(pid, Some(auth))?;
        Ok((id, Capability::ChannelAccess(id)))
    })?;
    crate::kinfo!(Ipc, "Channel {} created by thread {}", id, creator.0);
    Ok((id, handle))
}

/// Create a kernel channel without a capability check, minting no token,
/// with no `creator` and no `creation_cap`, so no revocation cascades to it.
/// The caller names the process that owns endpoint A: init code that runs
/// before the owning threads exist (before `enter_scheduler`), and
/// self-tests (ADR §11's interim exception).
///
/// # Panics
///
/// If the channel table has no free slot: these callers create a handful of
/// channels, so a full table is a kernel bug.
pub(crate) fn channel_create_unchecked(owner: ProcessId) -> ChannelId {
    let id = channel_alloc(owner, None).expect("channel table full");
    crate::kinfo!(
        Ipc,
        "Channel {} created (unchecked) for pid {}",
        id,
        owner.0
    );
    id
}

/// Set the process that owns the peer endpoint (B) of a channel.
pub fn channel_set_peer(channel: ChannelId, peer: ProcessId) -> Result<(), i64> {
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
///
/// Returns `Err(EINVAL)` for a malformed slot and `Err(EPIPE)` when the slot
/// is empty or holds a channel with another id. The identity check comes
/// before the take (`ChannelSlot::free`): `check_channel_access` passes a
/// stale id when the caller still holds the stale token, and a cascade can
/// name a channel destroyed since its walk, so without the check a stale
/// destroy would tear down the channel now in that slot.
pub(crate) fn channel_destroy_unchecked(channel: ChannelId) -> Result<(), i64> {
    let mut table = CHANNEL_TABLE.lock();
    let ch = channel_slot_mut(&mut table, channel)?
        .free(channel)
        .ok_or(IpcError::Epipe as i64)?;

    // Wake any blocked threads with EPIPE (both receiver and caller).
    let wake_recv = ch.waiting_receiver;
    let wake_caller = ch.pending_caller.map(|p| p.tid);
    // The channel held their last waiter references: their wakes are in
    // flight until wake_with_error reaches unblock.
    for tid in [wake_recv, wake_caller].into_iter().flatten() {
        crate::observability::tripwire::mark_wake_pending(tid, WakeSource::ChDestroy);
    }
    drop(table);
    if let Some(recv_tid) = wake_recv {
        timeout::wake_with_error(recv_tid, IpcError::Epipe as i64, WakeSource::ChDestroy);
    }
    if let Some(caller_tid) = wake_caller {
        timeout::wake_with_error(caller_tid, IpcError::Epipe as i64, WakeSource::ChDestroy);
    }

    crate::kinfo!(Ipc, "Channel {} destroyed", channel);
    Ok(())
}

/// Endpoint states (A, B) of `channel`, or `None` if the channel is gone
/// (its slot is empty or holds another generation's channel).
pub(crate) fn channel_endpoint_states(
    channel: ChannelId,
) -> Option<(EndpointState, EndpointState)> {
    let mut table = CHANNEL_TABLE.lock();
    channel_mut(&mut table, channel)
        .ok()
        .map(|ch| (ch.state_a, ch.state_b))
}

/// Process exit step C's per-victim cleanup of thread `tid`, which the exit
/// marked Dead (or which is the exiting caller): its timeout entry (a
/// blocking clear, so no entry fires later at the dead slot), its reply
/// slot, its notification deadline and its notification waiter records, a
/// `notification_wait`'s and an `ipc_select`'s alike (a select's channel
/// registrations are `waiting_receiver` records, which step C's channel walk
/// clears). Each lock is taken alone; the IRQ-class ones with IRQs masked.
pub(crate) fn forget_thread(tid: ThreadId) {
    timeout::clear_timeout_blocking(tid);
    timeout::clear_reply_slot(tid);
    notify::forget_waiter(tid);
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
/// no wrapper reports `CapabilityDenied`. The boot self-test
/// `ipc/tests/kit_errors.rs` checks every override a boot can reach and a
/// check can tell from the table's default; its doc comment lists the ones
/// it leaves out.
pub struct KernelIpc;

/// Kit error for a failed operation on channel `id`.
///
/// A channel operation's only capability check is `ChannelAccess(id)`
/// (`channel_destroy`, `ipc_send`, `ipc_recv`, `ipc_call`), so its EPERM
/// names that capability; EINVAL (an out-of-range id) and EPIPE (destroyed or
/// dead) both mean channel `id` does not exist. Everything else, EAGAIN
/// included (`WouldBlock`), comes from the errno table.
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
fn msg_payload(msg: &RawMessage) -> Result<&[u8], IpcKitError> {
    msg.data.get(..msg.len).ok_or(IpcKitError::MessageTooLarge {
        size: msg.len,
        max: MAX_MESSAGE_SIZE,
    })
}

/// The first channel entry in `entries` whose `ChannelAccess` the calling
/// thread's process does not hold, looked up without logging a denial (the
/// `ipc_select` call that failed already logged it).
///
/// The lookup runs after `ipc_select` has returned, so a `ChannelAccess`
/// grant can land in between and leave every channel entry held. The result
/// is then the first channel entry: `ipc_select` returns EPERM only for a set
/// with at least one channel entry (`select::check_channel_entries`), and the
/// caller lacked one of them when it checked. `None` means the set has no
/// channel entry at all.
fn first_denied_channel(entries: &[SelectEntry]) -> Option<ChannelId> {
    let now = crate::arch::aarch64::timer::TICK_COUNT.load(core::sync::atomic::Ordering::Relaxed);
    let pid = crate::cap::current_process_id();
    let table = crate::task::process::PROCESS_TABLE.lock();
    let proc = pid.and_then(|p| crate::task::process::process_ref(&table, p).ok());
    let mut channels = entries.iter().filter_map(|entry| match entry.kind {
        SelectKind::Channel(ch) => Some(ch),
        SelectKind::Notification(..) => None,
    });
    let first = channels.clone().next();
    channels
        .find(|&ch| {
            !proc.is_some_and(|p| {
                p.cap_table
                    .has_capability(&Capability::ChannelAccess(ch), now)
            })
        })
        .or(first)
}

impl ipc_kit::ChannelOps for KernelIpc {
    fn channel_create(&mut self) -> Result<(ChannelId, shared::CapabilityHandle), IpcKitError> {
        let tid = current_thread_id().ok_or(IpcKitError::CapabilityDenied {
            required: Capability::ChannelCreate,
        })?;
        channel_create(tid).map_err(|code| match IpcError::try_from(code) {
            Ok(IpcError::Eperm) => IpcKitError::CapabilityDenied {
                required: Capability::ChannelCreate,
            },
            // ENOSPC from channel_create is a full channel table or a full
            // capability table; there is no channel yet, so ChannelFull (a
            // full ring) does not apply.
            Ok(IpcError::Enospc) => IpcKitError::ResourceExhausted {
                reason: "channel or capability table full",
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
                // ipc_select checks no capability for a set without channel
                // entries, so its EPERM always comes with a channel entry and
                // this arm is not reached; it must not claim a capability.
                None => IpcKitError::InvalidArgument {
                    reason: "select set has no channel entry",
                },
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
    ) -> Result<(shared::SharedMemoryId, shared::CapabilityHandle), IpcKitError> {
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
        let required = Capability::SharedMemoryAccess(id);
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

    /// Destroy region `id` for the calling thread's process, which must be
    /// its creator and hold a live `SharedMemoryAccess(id)`
    /// (`shmem::shared_memory_destroy`: one authorising hold that detaches
    /// the region, force-unmapping every mapper).
    fn shmem_destroy(&mut self, id: shared::SharedMemoryId) -> Result<(), IpcKitError> {
        // shared_memory_destroy's EPERM is a caller with no process, no live
        // token or not the creator: the right it lacks is being the creator
        // with access, which no single capability names.
        let not_creator = IpcKitError::SharedMemoryError {
            reason: "not creator or no access",
        };
        let pid = crate::cap::current_process_id().ok_or_else(|| not_creator.clone())?;
        shmem::shared_memory_destroy(pid, id).map_err(|code| match IpcError::try_from(code) {
            Ok(IpcError::Einval) => IpcKitError::SharedMemoryError {
                reason: "invalid region id",
            },
            Ok(IpcError::Eperm) => not_creator,
            _ => shm_kit_err(code, None),
        })
    }
}
