//! IPC channel operations: call, recv, reply, send, cancel.
//!
//! Implements the core IPC syscall handlers. Phase 3 kernel threads
//! invoke these via direct function calls (not SVC). The SVC dispatch
//! path is wired in parallel for future EL0 user threads.

use core::sync::atomic::Ordering;

use crate::arch::aarch64::timer::TICK_COUNT;
use crate::observability::metrics::METRICS;
use crate::sched;
use crate::syscall::IpcError;
use crate::task::{ThreadId, ThreadState};
use shared::{ChannelId, EndpointState, RawMessage, MAX_MESSAGE_SIZE};

use super::timeout::{
    clear_timeout, get_wakeup_error, wake_with_error, ReplySlot, TimeoutEntry, REPLY_SLOTS,
    TIMEOUT_QUEUE,
};
use super::{direct, CHANNEL_TABLE};

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
    let caller_tid = match super::current_thread_id() {
        Some(t) => t,
        None => return IpcError::Eperm as i64,
    };

    // Capability enforcement: check ChannelAccess (fail-closed).
    match crate::cap::process_of_thread(caller_tid) {
        Some(pid) => {
            if let Err(e) = crate::cap::check_channel_access(pid, channel) {
                return e;
            }
        }
        None => return IpcError::Eperm as i64,
    }

    // Build message.
    let mut msg = RawMessage::EMPTY;
    msg.sender = caller_tid;
    msg.len = send_buf.len();
    msg.data[..send_buf.len()].copy_from_slice(send_buf);

    // Enqueue message and register as pending caller.
    // Also check for direct switch opportunity (receiver already waiting).
    let direct_switch_target: Option<ThreadId>;
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

        // Check for direct switch: is a receiver already waiting?
        direct_switch_target = ch.waiting_receiver.take();

        if let Some(recv_tid) = direct_switch_target {
            // Receiver found — we'll attempt direct switch below.
            // Don't unblock via scheduler; direct switch is faster.
            // Clear the receiver's timeout first (they're being woken by message delivery).
            clear_timeout(recv_tid);
            drop(table);

            // Register reply buffer BEFORE direct switch (we'll be blocked).
            {
                let mut slots = REPLY_SLOTS.lock();
                slots[caller_tid.0 as usize] = Some(ReplySlot {
                    buf: recv_buf.as_mut_ptr(),
                    buf_len: recv_buf.len(),
                    bytes_written: 0,
                });
            }

            // Register timeout (even with direct switch, the receiver
            // might not reply in time).
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

            // Attempt direct switch — bypasses scheduler entirely.
            if direct::try_direct_switch(caller_tid, recv_tid) {
                // We've been restored after the reply. Continue below
                // to check result (same as scheduler-woken path).
            } else {
                // Direct switch failed — fall back to scheduler path.
                sched::unblock(recv_tid);
                sched::block_current(ThreadState::BlockedIpc {
                    channel: channel.0 as u64,
                });
            }
        } else {
            // No receiver waiting — use scheduler path.
            drop(table);

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
        }
    }

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
    let receiver_tid = match super::current_thread_id() {
        Some(t) => t,
        None => return Err(IpcError::Eperm as i64),
    };

    // Capability enforcement: check ChannelAccess (fail-closed).
    let pid = crate::cap::process_of_thread(receiver_tid).ok_or(IpcError::Eperm as i64)?;
    crate::cap::check_channel_access(pid, channel)?;

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
/// Uses direct switch when the caller is on the same CPU (fast path).
pub fn ipc_reply(channel: ChannelId, reply_buf: &[u8]) -> i64 {
    if reply_buf.len() > MAX_MESSAGE_SIZE {
        return IpcError::Enospc as i64;
    }

    let caller_tid;
    let replier_tid = match super::current_thread_id() {
        Some(t) => t,
        None => return IpcError::Eperm as i64,
    };

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

    // Clear the caller's timeout — the reply arrived, so the timeout must
    // not fire later and spuriously fail the call with ETIMEDOUT.
    clear_timeout(caller_tid);

    // Try direct switch back to caller (fast path).
    // This bypasses the scheduler — replier switches directly to caller.
    if direct::try_reply_switch(replier_tid, caller_tid) {
        // Replier was saved and will be resumed by scheduler later.
        // The caller has already been restored and is running.
        return 0;
    }

    // Fallback: unblock via scheduler (caller on different CPU, etc.)
    sched::unblock(caller_tid);

    0
}

// ---------------------------------------------------------------------------
// IpcSend — asynchronous send (fire and forget)
// ---------------------------------------------------------------------------

/// Send a message without waiting for reply.
///
/// Returns 0 on success, negative error on failure.
pub fn ipc_send(channel: ChannelId, send_buf: &[u8]) -> i64 {
    if send_buf.len() > MAX_MESSAGE_SIZE {
        return IpcError::Enospc as i64;
    }

    let sender_tid = match super::current_thread_id() {
        Some(t) => t,
        None => return IpcError::Eperm as i64,
    };

    // Capability enforcement: check ChannelAccess (fail-closed).
    match crate::cap::process_of_thread(sender_tid) {
        Some(pid) => {
            if let Err(e) = crate::cap::check_channel_access(pid, channel) {
                return e;
            }
        }
        None => return IpcError::Eperm as i64,
    }

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

    // Wake receiver if waiting — clear their timeout first.
    if let Some(recv_tid) = ch.waiting_receiver.take() {
        drop(table);
        clear_timeout(recv_tid);
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
pub fn ipc_cancel(channel: ChannelId) -> i64 {
    // Capability enforcement: check ChannelAccess (fail-closed).
    match crate::cap::current_process_id() {
        Some(pid) => {
            if let Err(e) = crate::cap::check_channel_access(pid, channel) {
                return e;
            }
        }
        None => return IpcError::Eperm as i64,
    }

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
