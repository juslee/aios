//! IPC channels and synchronous call/reply.
//!
//! Provides channel creation/destruction, synchronous IpcCall/IpcRecv/IpcReply,
//! asynchronous IpcSend, IpcCancel, and mandatory timeouts.
//! Per ipc.md §3–4, §9.1.
//!
//! Phase 3 kernel threads invoke IPC via direct function calls (not SVC).
//! The SVC dispatch path is wired in parallel for future EL0 user threads.

use core::sync::atomic::Ordering;

use crate::arch::aarch64::timer::TICK_COUNT;
use crate::observability::metrics::METRICS;
use crate::sched;
use crate::syscall::IpcError;
use crate::task::{ThreadId, ThreadState, MAX_THREADS};
use spin::Mutex;

// Re-export IPC types from shared crate.
pub use shared::{
    ChannelId, EndpointState, RawMessage, DEFAULT_TIMEOUT_TICKS, MAX_CHANNELS, MAX_MESSAGE_SIZE,
    RING_CAPACITY,
};

// ---------------------------------------------------------------------------
// Message ring buffer
// ---------------------------------------------------------------------------

/// Fixed-capacity ring buffer for IPC messages.
struct MessageRing {
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
struct Channel {
    id: ChannelId,
    state_a: EndpointState,
    state_b: EndpointState,
    /// Owner thread of endpoint A (creator).
    owner_a: ThreadId,
    /// Owner thread of endpoint B (peer).
    owner_b: Option<ThreadId>,
    /// Message ring buffer (requests and async sends).
    ring: MessageRing,
    /// Thread currently blocked in ipc_recv() on this channel, if any.
    waiting_receiver: Option<ThreadId>,
    /// Thread currently blocked in ipc_call() waiting for a reply.
    /// The receiver uses this to deliver the reply.
    pending_caller: Option<ThreadId>,
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
        }
    }
}

// ---------------------------------------------------------------------------
// Global channel table
// ---------------------------------------------------------------------------

static CHANNEL_TABLE: Mutex<[Option<Channel>; MAX_CHANNELS]> = {
    const NONE: Option<Channel> = None;
    Mutex::new([NONE; MAX_CHANNELS])
};

// ---------------------------------------------------------------------------
// Timeout queue
// ---------------------------------------------------------------------------

/// Entry in the timeout queue — tracks a thread's IPC deadline.
struct TimeoutEntry {
    tid: ThreadId,
    wake_at_tick: u64,
    error_code: i64,
}

static TIMEOUT_QUEUE: Mutex<[Option<TimeoutEntry>; MAX_THREADS]> = {
    const NONE: Option<TimeoutEntry> = None;
    Mutex::new([NONE; MAX_THREADS])
};

/// Reply buffer: when a caller blocks in ipc_call(), it registers where
/// the reply should be written. Protected by the CHANNEL_TABLE lock
/// (reply is written while the lock is held in ipc_reply).
///
/// Per-thread: index = thread table index. Only one outstanding ipc_call
/// per thread is possible (thread is blocked).
struct ReplySlot {
    /// Pointer to the caller's reply buffer (kernel VA).
    buf: *mut u8,
    /// Maximum reply buffer size.
    buf_len: usize,
    /// Actual bytes written by the replier (set by ipc_reply).
    bytes_written: usize,
}

// SAFETY: ReplySlot contains a *mut u8 pointing to a blocked thread's kernel
// stack buffer. The thread is blocked (cannot run) while the slot is active,
// so the pointer is stable and exclusively accessed via REPLY_SLOTS lock.
unsafe impl Send for ReplySlot {}
unsafe impl Sync for ReplySlot {}

static REPLY_SLOTS: Mutex<[Option<ReplySlot>; MAX_THREADS]> = {
    const NONE: Option<ReplySlot> = None;
    Mutex::new([NONE; MAX_THREADS])
};

// ---------------------------------------------------------------------------
// Channel create / destroy
// ---------------------------------------------------------------------------

/// Create a new IPC channel. Returns (channel_id, endpoint_a, endpoint_b).
///
/// The creator thread owns endpoint A. Endpoint B can be assigned to
/// another thread via `channel_set_peer()`.
pub fn channel_create(creator: ThreadId) -> Result<ChannelId, i64> {
    let mut table = CHANNEL_TABLE.lock();
    // Find a free slot.
    let slot = table.iter().position(|s| s.is_none());
    let idx = match slot {
        Some(i) => i,
        None => return Err(IpcError::Enospc as i64),
    };

    let id = ChannelId(idx as u32);
    table[idx] = Some(Channel::new(id, creator));
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
pub fn channel_destroy(channel: ChannelId) -> Result<(), i64> {
    let mut table = CHANNEL_TABLE.lock();
    let idx = channel.0 as usize;
    let ch = match table[idx].take() {
        Some(c) => c,
        None => return Err(IpcError::Epipe as i64),
    };

    // Wake any blocked receiver with EPIPE.
    if let Some(recv_tid) = ch.waiting_receiver {
        drop(table);
        wake_with_error(recv_tid, IpcError::Epipe as i64);
    } else if let Some(caller_tid) = ch.pending_caller {
        drop(table);
        wake_with_error(caller_tid, IpcError::Epipe as i64);
    } else {
        drop(table);
    }

    crate::kinfo!(Ipc, "Channel {} destroyed", idx);
    Ok(())
}

// ---------------------------------------------------------------------------
// IpcCall — send request and block for reply (synchronous)
// ---------------------------------------------------------------------------

/// Synchronous IPC call: send a message and block until reply or timeout.
///
/// `send_buf`/`send_len`: request payload.
/// `recv_buf`/`recv_len`: reply buffer.
/// `timeout_ticks`: maximum ticks to wait (0 = non-blocking, use DEFAULT_TIMEOUT_TICKS for 5s).
///
/// Returns bytes received on success, or negative error code.
pub fn ipc_call(
    channel: ChannelId,
    send_buf: &[u8],
    recv_buf: &mut [u8],
    timeout_ticks: u64,
) -> i64 {
    if send_buf.len() > MAX_MESSAGE_SIZE {
        return IpcError::Enospc as i64;
    }

    // Get current thread ID.
    let caller_tid = match current_thread_id() {
        Some(t) => t,
        None => return IpcError::Eperm as i64,
    };

    // Build message.
    let mut msg = RawMessage::EMPTY;
    msg.sender = caller_tid;
    msg.len = send_buf.len();
    msg.data[..send_buf.len()].copy_from_slice(send_buf);

    // Enqueue message and register as pending caller.
    {
        let mut table = CHANNEL_TABLE.lock();
        let ch = match &mut table[channel.0 as usize] {
            Some(c) => c,
            None => return IpcError::Epipe as i64,
        };

        // Check endpoint is active.
        if ch.state_a == EndpointState::Dead || ch.state_b == EndpointState::Dead {
            return IpcError::Epipe as i64;
        }

        // Only one pending caller per channel at a time.
        if ch.pending_caller.is_some() {
            return IpcError::Eagain as i64;
        }

        // Enqueue the request message.
        if !ch.ring.push(msg) {
            return IpcError::Enospc as i64;
        }

        // Register as pending caller.
        ch.pending_caller = Some(caller_tid);

        // If a receiver is waiting, unblock it so it can process the message.
        if let Some(recv_tid) = ch.waiting_receiver.take() {
            drop(table);
            sched::unblock(recv_tid);
        } else {
            drop(table);
        }
    }

    // Register reply buffer.
    {
        let mut slots = REPLY_SLOTS.lock();
        slots[caller_tid.0 as usize] = Some(ReplySlot {
            buf: recv_buf.as_mut_ptr(),
            buf_len: recv_buf.len(),
            bytes_written: 0,
        });
    }

    // Register timeout.
    if timeout_ticks > 0 {
        let deadline = TICK_COUNT.load(Ordering::Relaxed) + timeout_ticks;
        let mut tq = TIMEOUT_QUEUE.lock();
        tq[caller_tid.0 as usize] = Some(TimeoutEntry {
            tid: caller_tid,
            wake_at_tick: deadline,
            error_code: IpcError::Etimedout as i64,
        });
    }

    #[cfg(feature = "kernel-metrics")]
    METRICS.ipc_call.inc();

    // Block until reply or timeout.
    sched::block_current(ThreadState::BlockedIpc {
        channel: channel.0 as u64,
    });

    // Woken up — check result.
    // Clear timeout entry.
    {
        let mut tq = TIMEOUT_QUEUE.lock();
        tq[caller_tid.0 as usize] = None;
    }

    // Read reply bytes written.
    let result = {
        let mut slots = REPLY_SLOTS.lock();
        if let Some(slot) = slots[caller_tid.0 as usize].take() {
            slot.bytes_written as i64
        } else {
            0
        }
    };

    // Check if we were woken due to error (timeout, EPIPE, cancel).
    // The wake_with_error function stores the error in the thread's
    // time_slice_remaining field (repurposed as error code when blocked).
    let error = get_wakeup_error(caller_tid);
    if error != 0 {
        // Clean up pending caller state.
        let mut table = CHANNEL_TABLE.lock();
        if let Some(ch) = &mut table[channel.0 as usize] {
            if ch.pending_caller == Some(caller_tid) {
                ch.pending_caller = None;
            }
        }

        #[cfg(feature = "kernel-metrics")]
        if error == IpcError::Etimedout as i64 {
            METRICS.ipc_timeout.inc();
        }

        return error;
    }

    result
}

// ---------------------------------------------------------------------------
// IpcRecv — wait for a message on a channel
// ---------------------------------------------------------------------------

/// Wait for a message on a channel.
///
/// `recv_buf`: buffer to receive message payload.
/// `timeout_ticks`: maximum ticks to wait (0 = non-blocking poll).
///
/// Returns (bytes_received, sender_tid) on success, or negative error.
/// The sender's ThreadId is returned so the receiver knows who to reply to.
pub fn ipc_recv(
    channel: ChannelId,
    recv_buf: &mut [u8],
    timeout_ticks: u64,
) -> Result<(usize, ThreadId), i64> {
    let receiver_tid = match current_thread_id() {
        Some(t) => t,
        None => return Err(IpcError::Eperm as i64),
    };

    // Try to dequeue a message.
    {
        let mut table = CHANNEL_TABLE.lock();
        let ch = match &mut table[channel.0 as usize] {
            Some(c) => c,
            None => return Err(IpcError::Epipe as i64),
        };

        if ch.state_a == EndpointState::Dead || ch.state_b == EndpointState::Dead {
            return Err(IpcError::Epipe as i64);
        }

        if let Some(msg) = ch.ring.pop() {
            let copy_len = msg.len.min(recv_buf.len());
            recv_buf[..copy_len].copy_from_slice(&msg.data[..copy_len]);

            #[cfg(feature = "kernel-metrics")]
            METRICS.ipc_recv.inc();

            return Ok((copy_len, msg.sender));
        }

        // No message — non-blocking poll?
        if timeout_ticks == 0 {
            return Err(IpcError::Eagain as i64);
        }

        // Register as waiting receiver.
        if ch.waiting_receiver.is_some() {
            // Only one receiver per channel.
            return Err(IpcError::Eagain as i64);
        }
        ch.waiting_receiver = Some(receiver_tid);
    }

    // Register timeout.
    if timeout_ticks < u64::MAX {
        let deadline = TICK_COUNT.load(Ordering::Relaxed) + timeout_ticks;
        let mut tq = TIMEOUT_QUEUE.lock();
        tq[receiver_tid.0 as usize] = Some(TimeoutEntry {
            tid: receiver_tid,
            wake_at_tick: deadline,
            error_code: IpcError::Etimedout as i64,
        });
    }

    // Block until message arrives or timeout.
    sched::block_current(ThreadState::BlockedIpc {
        channel: channel.0 as u64,
    });

    // Woken up — clear timeout.
    {
        let mut tq = TIMEOUT_QUEUE.lock();
        tq[receiver_tid.0 as usize] = None;
    }

    // Clear waiting_receiver.
    {
        let mut table = CHANNEL_TABLE.lock();
        if let Some(ch) = &mut table[channel.0 as usize] {
            if ch.waiting_receiver == Some(receiver_tid) {
                ch.waiting_receiver = None;
            }
        }
    }

    // Check for error wake.
    let error = get_wakeup_error(receiver_tid);
    if error != 0 {
        #[cfg(feature = "kernel-metrics")]
        if error == IpcError::Etimedout as i64 {
            METRICS.ipc_timeout.inc();
        }
        return Err(error);
    }

    // Retry dequeue (message was enqueued while we were blocked).
    let mut table = CHANNEL_TABLE.lock();
    let ch = match &mut table[channel.0 as usize] {
        Some(c) => c,
        None => return Err(IpcError::Epipe as i64),
    };

    if let Some(msg) = ch.ring.pop() {
        let copy_len = msg.len.min(recv_buf.len());
        recv_buf[..copy_len].copy_from_slice(&msg.data[..copy_len]);

        #[cfg(feature = "kernel-metrics")]
        METRICS.ipc_recv.inc();

        Ok((copy_len, msg.sender))
    } else {
        // Shouldn't happen — we were woken without error and no message.
        Err(IpcError::Eagain as i64)
    }
}

// ---------------------------------------------------------------------------
// IpcReply — reply to a pending caller
// ---------------------------------------------------------------------------

/// Reply to the thread that sent the last IpcCall on this channel.
///
/// `channel`: the channel the request was received on.
/// `reply_buf`: reply payload.
///
/// No capability check required (ipc.md §9.1).
pub fn ipc_reply(channel: ChannelId, reply_buf: &[u8]) -> i64 {
    if reply_buf.len() > MAX_MESSAGE_SIZE {
        return IpcError::Enospc as i64;
    }

    let caller_tid;

    // Find and clear the pending caller.
    {
        let mut table = CHANNEL_TABLE.lock();
        let ch = match &mut table[channel.0 as usize] {
            Some(c) => c,
            None => return IpcError::Epipe as i64,
        };

        caller_tid = match ch.pending_caller.take() {
            Some(t) => t,
            None => return IpcError::Eproto as i64,
        };
    }

    // Copy reply into caller's reply buffer.
    {
        let mut slots = REPLY_SLOTS.lock();
        if let Some(slot) = &mut slots[caller_tid.0 as usize] {
            let copy_len = reply_buf.len().min(slot.buf_len);
            // SAFETY: The reply buffer pointer was provided by the caller thread
            // from its kernel stack. The caller is blocked, so the buffer is valid
            // and not concurrently accessed. copy_len is bounded by both the reply
            // payload size and the buffer capacity.
            unsafe {
                core::ptr::copy_nonoverlapping(reply_buf.as_ptr(), slot.buf, copy_len);
            }
            slot.bytes_written = copy_len;
        }
    }

    // Unblock the caller.
    sched::unblock(caller_tid);

    0
}

// ---------------------------------------------------------------------------
// IpcSend — asynchronous send (fire and forget)
// ---------------------------------------------------------------------------

/// Send a message without waiting for reply.
///
/// Returns 0 on success, negative error on failure.
#[allow(dead_code)]
pub fn ipc_send(channel: ChannelId, send_buf: &[u8]) -> i64 {
    if send_buf.len() > MAX_MESSAGE_SIZE {
        return IpcError::Enospc as i64;
    }

    let sender_tid = match current_thread_id() {
        Some(t) => t,
        None => return IpcError::Eperm as i64,
    };

    let mut msg = RawMessage::EMPTY;
    msg.sender = sender_tid;
    msg.len = send_buf.len();
    msg.data[..send_buf.len()].copy_from_slice(send_buf);

    let mut table = CHANNEL_TABLE.lock();
    let ch = match &mut table[channel.0 as usize] {
        Some(c) => c,
        None => return IpcError::Epipe as i64,
    };

    if ch.state_a == EndpointState::Dead || ch.state_b == EndpointState::Dead {
        return IpcError::Epipe as i64;
    }

    if !ch.ring.push(msg) {
        return IpcError::Eagain as i64;
    }

    // Wake receiver if waiting.
    if let Some(recv_tid) = ch.waiting_receiver.take() {
        drop(table);
        sched::unblock(recv_tid);
    }

    #[cfg(feature = "kernel-metrics")]
    METRICS.ipc_send.inc();

    0
}

// ---------------------------------------------------------------------------
// IpcCancel — cancel a pending IpcCall
// ---------------------------------------------------------------------------

/// Cancel a pending IpcCall on a channel. The blocked caller is woken
/// with ECANCELED.
#[allow(dead_code)]
pub fn ipc_cancel(channel: ChannelId) -> i64 {
    let mut table = CHANNEL_TABLE.lock();
    let ch = match &mut table[channel.0 as usize] {
        Some(c) => c,
        None => return IpcError::Epipe as i64,
    };

    let caller_tid = match ch.pending_caller.take() {
        Some(t) => t,
        None => return 0, // Nothing to cancel.
    };

    drop(table);
    wake_with_error(caller_tid, IpcError::Ecanceled as i64);
    0
}

// ---------------------------------------------------------------------------
// Timeout checking — called from timer tick handler
// ---------------------------------------------------------------------------

/// Check for expired IPC timeouts. Called every tick from timer_tick_handler.
///
/// Scans the timeout queue (MAX_THREADS entries) and wakes any thread
/// whose deadline has passed. O(n) where n = MAX_THREADS = 64.
pub fn check_timeouts() {
    let now = TICK_COUNT.load(Ordering::Relaxed);

    // Use try_lock() because this is called from IRQ context.
    // If the lock is held (e.g., ipc_call is registering a timeout on this
    // core), skip this tick — the next tick will catch it.
    let mut tq = match TIMEOUT_QUEUE.try_lock() {
        Some(guard) => guard,
        None => return,
    };

    // Collect expired entries under the lock, then wake outside the lock
    // to avoid lock ordering issues (TIMEOUT_QUEUE → THREAD_TABLE).
    let mut expired: [(ThreadId, i64); MAX_THREADS] = [(ThreadId(0), 0); MAX_THREADS];
    let mut count = 0;

    for entry in tq.iter_mut() {
        if let Some(te) = entry {
            if now >= te.wake_at_tick {
                expired[count] = (te.tid, te.error_code);
                count += 1;
                *entry = None;
            }
        }
    }
    drop(tq);

    // Wake expired threads outside the lock.
    for &(tid, error) in expired[..count].iter() {
        wake_with_error(tid, error);
    }
}

// ---------------------------------------------------------------------------
// Helper: get current thread ID
// ---------------------------------------------------------------------------

pub fn current_thread_id() -> Option<ThreadId> {
    let cpu = crate::arch::aarch64::exceptions::core_id() as usize;
    *crate::task::CURRENT_THREAD[cpu].lock()
}

// ---------------------------------------------------------------------------
// Helper: wake a thread with an error code
// ---------------------------------------------------------------------------

/// Wake a blocked thread and signal an error condition.
///
/// We store the error code in a per-thread wakeup error slot so the
/// thread can check it after being unblocked.
fn wake_with_error(tid: ThreadId, error: i64) {
    // Store error in the wakeup error slot.
    {
        let mut errors = WAKEUP_ERRORS.lock();
        errors[tid.0 as usize] = error;
    }
    sched::unblock(tid);
}

/// Get and clear the wakeup error for a thread. Returns 0 if no error.
fn get_wakeup_error(tid: ThreadId) -> i64 {
    let mut errors = WAKEUP_ERRORS.lock();
    let error = errors[tid.0 as usize];
    errors[tid.0 as usize] = 0;
    error
}

/// Per-thread wakeup error codes. Set by wake_with_error(), read by
/// the woken thread to distinguish normal wakeup from error wakeup.
static WAKEUP_ERRORS: Mutex<[i64; MAX_THREADS]> = Mutex::new([0; MAX_THREADS]);

/// Sleep the current thread for the given number of ticks.
///
/// Uses the IPC timeout infrastructure to wake the thread after the deadline.
/// error_code=0 distinguishes sleep wake from error wake.
#[allow(dead_code)]
pub fn sleep_ticks(ticks: u64) {
    if ticks == 0 {
        return;
    }
    let tid = match current_thread_id() {
        Some(t) => t,
        None => return,
    };
    let deadline = TICK_COUNT.load(Ordering::Relaxed) + ticks;
    {
        let mut tq = TIMEOUT_QUEUE.lock();
        tq[tid.0 as usize] = Some(TimeoutEntry {
            tid,
            wake_at_tick: deadline,
            error_code: 0,
        });
    }
    sched::block_current(ThreadState::BlockedIpc { channel: u64::MAX });
}

// ---------------------------------------------------------------------------
// IPC test initialization
// ---------------------------------------------------------------------------

/// Channel ID shared between IPC test threads (set by init).
static TEST_CHANNEL: Mutex<Option<ChannelId>> = Mutex::new(None);

/// Initialize IPC test threads: a server and a caller that perform
/// synchronous IPC call/reply, plus a timeout test.
///
/// Called from main.rs after sched::init() but before enter_scheduler().
pub fn init() {
    use crate::task::{CpuSet, SchedulerClass, Thread};

    // Create a test channel.
    let caller_tid = ThreadId(0x200);
    let ch = channel_create(caller_tid).expect("Failed to create IPC test channel");
    let server_tid = ThreadId(0x201);
    channel_set_peer(ch, server_tid).expect("Failed to set IPC channel peer");
    *TEST_CHANNEL.lock() = Some(ch);

    // Create server thread (receives requests, sends replies).
    {
        let stack_phys = sched::alloc_kernel_stack();
        let stack_virt_top = sched::phys_to_virt(stack_phys) + sched::STACK_SIZE;

        let mut thread = Thread::new_kernel(
            server_tid,
            b"ipc-server\0\0\0\0\0\0",
            ipc_server_entry as *const () as usize,
            stack_phys,
        );
        thread.sched.class = SchedulerClass::Normal;
        thread.sched.effective_class = SchedulerClass::Normal;
        thread.sched.affinity = CpuSet::all();
        thread.context.sp = stack_virt_top as u64;

        let idx = sched::allocate_thread(thread).expect("thread table full for IPC server");
        // Enqueue on CPU 0.
        sched::enqueue_on_cpu(0, ThreadId(idx as u32), SchedulerClass::Normal);
    }

    // Create caller thread (sends requests, receives replies).
    {
        let stack_phys = sched::alloc_kernel_stack();
        let stack_virt_top = sched::phys_to_virt(stack_phys) + sched::STACK_SIZE;

        let mut thread = Thread::new_kernel(
            caller_tid,
            b"ipc-caller\0\0\0\0\0\0",
            ipc_caller_entry as *const () as usize,
            stack_phys,
        );
        thread.sched.class = SchedulerClass::Normal;
        thread.sched.effective_class = SchedulerClass::Normal;
        thread.sched.affinity = CpuSet::all();
        thread.context.sp = stack_virt_top as u64;

        let idx = sched::allocate_thread(thread).expect("thread table full for IPC caller");
        // Enqueue on CPU 1 to exercise cross-core IPC.
        sched::enqueue_on_cpu(1, ThreadId(idx as u32), SchedulerClass::Normal);
    }

    // Create timeout test thread (calls IpcCall with no server → timeout).
    {
        let stack_phys = sched::alloc_kernel_stack();
        let stack_virt_top = sched::phys_to_virt(stack_phys) + sched::STACK_SIZE;

        let mut thread = Thread::new_kernel(
            ThreadId(0x202),
            b"ipc-timeout\0\0\0\0\0",
            ipc_timeout_entry as *const () as usize,
            stack_phys,
        );
        thread.sched.class = SchedulerClass::Normal;
        thread.sched.effective_class = SchedulerClass::Normal;
        thread.sched.affinity = CpuSet::all();
        thread.context.sp = stack_virt_top as u64;

        let idx = sched::allocate_thread(thread).expect("thread table full for IPC timeout");
        sched::enqueue_on_cpu(2, ThreadId(idx as u32), SchedulerClass::Normal);
    }

    crate::kinfo!(Ipc, "IPC test threads created (server, caller, timeout)");
}

// ---------------------------------------------------------------------------
// IPC test thread entry points
// ---------------------------------------------------------------------------

/// IPC server thread: receives requests and sends replies.
fn ipc_server_entry() -> ! {
    // Unmask IRQs — enter_scheduler left them masked when it dispatched us.
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    let ch = loop {
        if let Some(ch) = *TEST_CHANNEL.lock() {
            break ch;
        }
        sched::thread_yield();
    };
    crate::kinfo!(Ipc, "Server: started, channel={}", ch.0);

    let mut recv_buf = [0u8; MAX_MESSAGE_SIZE];

    for i in 0..5u32 {
        match ipc_recv(ch, &mut recv_buf, DEFAULT_TIMEOUT_TICKS) {
            Ok((len, sender)) => {
                crate::kinfo!(
                    Ipc,
                    "Server: recv {} bytes from thread {} iter={}",
                    len,
                    sender.0,
                    i
                );

                // Echo back with "REPLY:" prefix.
                let mut reply = [0u8; MAX_MESSAGE_SIZE];
                let prefix = b"REPLY:";
                let reply_len = (prefix.len() + len).min(MAX_MESSAGE_SIZE);
                reply[..prefix.len()].copy_from_slice(prefix);
                let data_len = reply_len - prefix.len();
                reply[prefix.len()..reply_len].copy_from_slice(&recv_buf[..data_len]);

                let result = ipc_reply(ch, &reply[..reply_len]);
                if result < 0 {
                    crate::kwarn!(Ipc, "Server: reply failed with {}", result);
                }
            }
            Err(e) => {
                crate::kwarn!(Ipc, "Server: recv failed with {} iter={}", e, i);
            }
        }
    }

    // Keep yielding forever after test iterations.
    loop {
        sched::thread_yield();
    }
}

/// IPC caller thread: sends requests and receives replies.
fn ipc_caller_entry() -> ! {
    // Unmask IRQs — enter_scheduler left them masked when it dispatched us.
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    let ch = loop {
        if let Some(ch) = *TEST_CHANNEL.lock() {
            break ch;
        }
        sched::thread_yield();
    };
    for i in 0..5u32 {
        let msg = b"PING";
        let mut reply_buf = [0u8; MAX_MESSAGE_SIZE];

        let start = crate::arch::aarch64::timer::read_counter();
        let result = ipc_call(ch, msg, &mut reply_buf, DEFAULT_TIMEOUT_TICKS);
        let end = crate::arch::aarch64::timer::read_counter();

        if result >= 0 {
            let elapsed_ticks = end.wrapping_sub(start);
            // Convert to nanoseconds: ticks * 1_000_000_000 / 62_500_000 = ticks * 16
            let elapsed_ns = elapsed_ticks * 16;

            let reply_len = result as usize;
            let reply_str =
                core::str::from_utf8(&reply_buf[..reply_len]).unwrap_or("<invalid utf8>");
            crate::kinfo!(
                Ipc,
                "Caller: got '{}' in {} ns ({}us) iter={}",
                reply_str,
                elapsed_ns,
                elapsed_ns / 1000,
                i
            );

            #[cfg(feature = "kernel-metrics")]
            METRICS.ipc_roundtrip_ns.observe(elapsed_ns);
        } else {
            crate::kwarn!(Ipc, "Caller: ipc_call failed with {} iter={}", result, i);
        }

        sched::thread_yield();
    }

    loop {
        sched::thread_yield();
    }
}

/// IPC timeout test thread: calls IpcCall on a channel with no receiver.
fn ipc_timeout_entry() -> ! {
    // Unmask IRQs — enter_scheduler left them masked when it dispatched us.
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    // Create a channel with no server — timeout is guaranteed.
    let caller_tid = match current_thread_id() {
        Some(t) => t,
        None => loop {
            sched::thread_yield();
        },
    };

    let ch = match channel_create(caller_tid) {
        Ok(c) => c,
        Err(e) => {
            crate::kwarn!(Ipc, "Timeout test: channel_create failed: {}", e);
            loop {
                sched::thread_yield();
            }
        }
    };

    let msg = b"TIMEOUT_TEST";
    let mut reply_buf = [0u8; MAX_MESSAGE_SIZE];

    // Use a short timeout (100 ticks = 100ms).
    crate::kinfo!(Ipc, "Timeout test: calling with 100ms timeout...");
    let result = ipc_call(ch, msg, &mut reply_buf, 100);

    if result == IpcError::Etimedout as i64 {
        crate::kinfo!(Ipc, "Timeout test: ETIMEDOUT as expected");
    } else {
        crate::kwarn!(Ipc, "Timeout test: unexpected result {}", result);
    }

    // Test channel destroy → EPIPE.
    let ch2 = match channel_create(caller_tid) {
        Ok(c) => c,
        Err(_) => loop {
            sched::thread_yield();
        },
    };
    // Destroy channel, then try to recv — should get EPIPE.
    let _ = channel_destroy(ch2);
    let mut buf = [0u8; 64];
    let result = ipc_recv(ch2, &mut buf, 0);
    match result {
        Err(e) if e == IpcError::Epipe as i64 => {
            crate::kinfo!(Ipc, "Destroy test: EPIPE as expected");
        }
        _ => {
            crate::kwarn!(Ipc, "Destroy test: unexpected result {:?}", result);
        }
    }

    loop {
        sched::thread_yield();
    }
}
