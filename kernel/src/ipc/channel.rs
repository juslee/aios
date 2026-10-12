//! IPC channel operations: call, recv, reply, send, cancel.
//!
//! Implements the core IPC syscall handlers. Phase 3 kernel threads
//! invoke these via direct function calls (not SVC). The SVC dispatch
//! path is wired in parallel for future EL0 user threads.

use core::sync::atomic::Ordering;

use crate::arch::aarch64::daif;
use crate::arch::aarch64::timer::TICK_COUNT;
use crate::observability::metrics::METRICS;
use crate::observability::tripwire::{self, WaitSide};
use crate::sched::{self, wake};
use crate::syscall::IpcError;
use crate::task::{ThreadId, ThreadState};
use shared::tripwire::WakeSource;
use shared::{deadline_after, ChannelId, EndpointState, RawMessage, MAX_MESSAGE_SIZE};

use super::timeout::{
    arm_timeout, clear_own_timeout, clear_timeout, get_wakeup_error, wake_with_error, ReplySlot,
    REPLY_SLOTS, TIMEOUT_QUEUE,
};
use super::{channel_mut, direct, DeadlineArm, CHANNEL_TABLE};

// ---------------------------------------------------------------------------
// Deadline shared by IpcCall and IpcRecv
// ---------------------------------------------------------------------------

/// The deadline of a wait for `timeout_ticks` that starts now, by
/// [`deadline_after`]: the current tick plus `timeout_ticks`, or `None` for
/// `u64::MAX` and for a sum that would reach `u64::MAX` (a tick never
/// reached). `ipc_call` and `ipc_recv` handle 0 before this.
fn deadline_from_now(timeout_ticks: u64) -> Option<u64> {
    deadline_after(TICK_COUNT.load(Ordering::Relaxed), timeout_ticks)
}

// ---------------------------------------------------------------------------
// IpcCall — send request and block for reply (synchronous)
// ---------------------------------------------------------------------------

/// Synchronous IPC call: send a message and block until reply or timeout.
///
/// `send_buf`/`send_len`: request payload.
/// `recv_buf`/`recv_len`: reply buffer.
/// `timeout_ticks`: maximum ticks to wait for the reply, from when the call
/// is queued. 0 and `u64::MAX` both mean no timeout: the call waits until a
/// reply, a cancel or the channel's destruction wakes it (capability-lifetime
/// ADR §9 keeps 0 unbounded), and so does any value whose deadline would
/// reach `u64::MAX`, so a value from a syscall register cannot overflow (#217). `DEFAULT_TIMEOUT_TICKS` is 5 s.
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
    // What registering the reply deadline found: Expired means it had
    // already passed, so no entry was published and the call does not block.
    let arm: DeadlineArm;
    {
        let mut table = CHANNEL_TABLE.lock();
        let ch = match channel_mut(&mut table, channel) {
            Ok(c) => c,
            Err(e) => return e,
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

        // The reply deadline counts from the queued request. 0 is no
        // timeout for ipc_call (capability-lifetime ADR §9), not a poll.
        let deadline = if timeout_ticks == 0 {
            None
        } else {
            deadline_from_now(timeout_ticks)
        };

        // Register as pending caller.
        ch.pending_caller = Some(caller_tid);
        // Tripwire N2 phase: published, timeout not yet registered.
        tripwire::wait_published(
            WaitSide::Call,
            caller_tid,
            u64::from(channel.raw()),
            deadline.is_some(),
        );

        // Check for direct switch: is a receiver already waiting?
        direct_switch_target = ch.waiting_receiver.take();

        if let Some(recv_tid) = direct_switch_target {
            // The receiver's wake is in flight until the direct switch or
            // the unblock below lands.
            tripwire::mark_wake_pending(recv_tid, WakeSource::Call);
            // Receiver found — we'll attempt direct switch below.
            // Don't unblock via scheduler; direct switch is faster.
            // Clear the receiver's timeout first (they're being woken by message delivery).
            // The result stays with this wake until its unblock fallback
            // classifies it (tripwire N2 counters). A receiver in ipc_select
            // took its own slot and re-scans when woken: it finds the queued
            // request ready.
            let recv_clear = clear_timeout(recv_tid);
            tripwire::note_clear(recv_clear);
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
            // might not reply in time). The tripwire N2 phase becomes armed
            // inside the TIMEOUT_QUEUE critical section.
            arm = arm_timeout(
                caller_tid,
                deadline,
                IpcError::Etimedout as i64,
                Some(WaitSide::Call),
            );

            #[cfg(feature = "kernel-metrics")]
            METRICS.ipc_call.inc();

            if arm == DeadlineArm::Expired {
                // The deadline has already passed and no entry was
                // published. A direct switch would block this thread with
                // nothing to time it out, so wake the receiver through the
                // scheduler and end the call below as its timeout would.
                // Otherwise attempt the direct switch, which bypasses the
                // scheduler entirely.
                let outcome = sched::unblock(recv_tid, WakeSource::Call);
                tripwire::note_send_wake(outcome, u64::from(channel.raw()), recv_clear);
            } else if direct::try_direct_switch(caller_tid, recv_tid) {
                // We've been restored after the reply. Continue below
                // to check result (same as scheduler-woken path).
            } else {
                // Direct switch failed (the receiver is not blocked in
                // ipc_recv, a select included) — fall back to the scheduler
                // path.
                let outcome = sched::unblock(recv_tid, WakeSource::Call);
                tripwire::note_send_wake(outcome, u64::from(channel.raw()), recv_clear);
                sched::block_current(ThreadState::BlockedIpc {
                    channel: u64::from(channel.raw()),
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

            // Register timeout. The tripwire N2 phase becomes armed inside
            // the TIMEOUT_QUEUE critical section.
            arm = arm_timeout(
                caller_tid,
                deadline,
                IpcError::Etimedout as i64,
                Some(WaitSide::Call),
            );

            #[cfg(feature = "kernel-metrics")]
            METRICS.ipc_call.inc();

            // Block until reply or timeout, unless the deadline has already
            // passed (no entry was published).
            if arm == DeadlineArm::Armed {
                sched::block_current(ThreadState::BlockedIpc {
                    channel: u64::from(channel.raw()),
                });
            }
        }
    }

    // Woken up — check result. The call's tripwire N2 phase ends here.
    tripwire::wait_ended(WaitSide::Call, caller_tid);
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
    // The wake_with_error function stores the error in the WAKEUP_ERRORS
    // table (indexed by ThreadId), retrieved via get_wakeup_error(). A call
    // whose deadline had passed at registration did not block: it takes the
    // code all the same, as its fired timeout would have, and ends with
    // ETIMEDOUT.
    let error = get_wakeup_error(caller_tid);
    let error = if arm == DeadlineArm::Expired {
        IpcError::Etimedout as i64
    } else {
        error
    };
    if error != 0 {
        // Clean up pending caller state.
        let mut table = CHANNEL_TABLE.lock();
        if let Ok(ch) = channel_mut(&mut table, channel) {
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

/// What one `ipc_recv` pass found in its CHANNEL_TABLE critical section.
enum RecvPass {
    /// The pass ended before its arm: a lookup error, a Dead endpoint
    /// (EPIPE), a message (popped and copied), or an empty poll (EAGAIN).
    Ended(Result<(usize, ThreadId), i64>),
    /// The pass armed, then found another thread in the receiver slot.
    Taken,
    /// The pass armed and is the channel's waiting receiver.
    Registered,
}

/// Wait for a message on a channel.
///
/// `recv_buf`: buffer to receive message payload.
/// `timeout_ticks`: maximum ticks to wait. 0 is a non-blocking poll and
/// `u64::MAX`, or any value whose deadline would reach `u64::MAX`, is no
/// timeout, so a value from a syscall register cannot overflow (#217).
///
/// Returns (bytes_received, sender_tid) on success, or negative error.
/// The sender's ThreadId is returned so the receiver knows who to reply to.
///
/// The caller's tid comes from `sched::current_tid()` (EPERM with no
/// current thread). Each pass runs one CHANNEL_TABLE critical section
/// (`recv_pass`): a lookup error or a Dead endpoint (EPIPE) ends the
/// receive, a queued message is popped and returned, and a poll with none
/// returns EAGAIN; otherwise the pass arms (`wake::prepare_block`) and
/// registers as the channel's `waiting_receiver`, or fails EAGAIN if another
/// thread holds that slot. After the release it publishes its timeout entry
/// (`timeout::arm_timeout`, ETIMEDOUT once the deadline has passed) and
/// blocks. A wake with no message (a stale sender whose `clear_timeout`
/// removed the entry, or any other stale `unblock`) costs one more pass,
/// which re-registers and re-inserts the entry, so the receive returns only
/// on a message, a failure or its own deadline.
///
/// A return from the first pass before its arm publishes nothing and takes
/// nothing. Every other exit clears the receiver slot if it is still this
/// thread's, removes the timeout entry, takes and discards the thread's
/// `WAKEUP_ERRORS` code once (destroy and exit write EPIPE there; the result
/// comes from the channel's state, capability-lifetime ADR §9), and cancels
/// the arm if its pass armed.
pub fn ipc_recv(
    channel: ChannelId,
    recv_buf: &mut [u8],
    timeout_ticks: u64,
) -> Result<(usize, ThreadId), i64> {
    let me = sched::current_tid().ok_or(IpcError::Eperm as i64)?;

    // Capability enforcement: check ChannelAccess (fail-closed).
    let pid = crate::cap::process_of_thread(me).ok_or(IpcError::Eperm as i64)?;
    crate::cap::check_channel_access(pid, channel)?;

    let poll = timeout_ticks == 0;
    let deadline = if poll {
        None
    } else {
        deadline_from_now(timeout_ticks)
    };

    let mut first = true;
    // Whether the pass that ends the loop armed (its exit cancels).
    let mut armed = false;
    let result = loop {
        match recv_pass(me, channel, recv_buf, poll, deadline.is_some()) {
            // Pass 1 before its arm: nothing published, nothing to take.
            RecvPass::Ended(result) if first => return result,
            RecvPass::Ended(result) => break result,
            RecvPass::Taken => {
                armed = true;
                break Err(IpcError::Eagain as i64);
            }
            RecvPass::Registered => {}
        }
        first = false;
        // Publish the timeout entry after the arm (ADR §8 rule 1). The
        // tripwire N2 phase becomes armed inside the TIMEOUT_QUEUE hold.
        let arm = arm_timeout(
            me,
            deadline,
            IpcError::Etimedout as i64,
            Some(WaitSide::Recv),
        );
        if arm == DeadlineArm::Expired {
            armed = true;
            break Err(IpcError::Etimedout as i64);
        }
        sched::block_current(ThreadState::BlockedIpc {
            channel: u64::from(channel.raw()),
        });
    };

    // --- Exit: the wait published a record in an earlier or this pass ---
    // The receive's tripwire N2 phase ends here.
    tripwire::wait_ended(WaitSide::Recv, me);
    {
        let mut table = CHANNEL_TABLE.lock();
        if let Ok(ch) = channel_mut(&mut table, channel) {
            if ch.waiting_receiver == Some(me) {
                ch.waiting_receiver = None;
            }
        }
    }
    clear_own_timeout(me);
    // Taken once per wait and discarded: it decides nothing (ADR §9).
    daif::with_irqs_masked(|| get_wakeup_error(me));
    if armed {
        wake::cancel_block(me);
    }

    #[cfg(feature = "kernel-metrics")]
    if matches!(result, Err(code) if code == IpcError::Etimedout as i64) {
        METRICS.ipc_timeout.inc();
    }
    result
}

/// One `ipc_recv` pass for `me`, in one CHANNEL_TABLE critical section: a
/// lookup error, a Dead endpoint, a queued message (popped into `recv_buf`)
/// or an empty `poll` ends it before the arm. Otherwise it arms
/// (`wake::prepare_block`, inside the hold and after the pop, so a receive
/// that finds a message never arms) and registers `me` as the channel's
/// `waiting_receiver`, keeping a registration of its own from an earlier
/// pass, or finds another receiver there. `timed` is the N2 phase's flag:
/// whether the wait has a deadline.
fn recv_pass(
    me: ThreadId,
    channel: ChannelId,
    recv_buf: &mut [u8],
    poll: bool,
    timed: bool,
) -> RecvPass {
    let mut table = CHANNEL_TABLE.lock();
    let ch = match channel_mut(&mut table, channel) {
        Ok(ch) => ch,
        Err(code) => return RecvPass::Ended(Err(code)),
    };
    if ch.state_a == EndpointState::Dead || ch.state_b == EndpointState::Dead {
        return RecvPass::Ended(Err(IpcError::Epipe as i64));
    }
    if let Some(msg) = ch.ring.pop() {
        let copy_len = msg.len.min(recv_buf.len());
        recv_buf[..copy_len].copy_from_slice(&msg.data[..copy_len]);

        #[cfg(feature = "kernel-metrics")]
        METRICS.ipc_recv.inc();

        return RecvPass::Ended(Ok((copy_len, msg.sender)));
    }
    if poll {
        return RecvPass::Ended(Err(IpcError::Eagain as i64));
    }

    // Arm before publishing the receiver slot (ADR §8 rule 1).
    wake::prepare_block(me);
    match ch.waiting_receiver {
        None => ch.waiting_receiver = Some(me),
        Some(tid) if tid == me => {}
        // Only one receiver per channel.
        Some(_) => return RecvPass::Taken,
    }
    // Tripwire N2 phase: published, timeout not yet registered.
    tripwire::wait_published(WaitSide::Recv, me, u64::from(channel.raw()), timed);
    RecvPass::Registered
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
        let ch = match channel_mut(&mut table, channel) {
            Ok(c) => c,
            Err(e) => return e,
        };

        caller_tid = match ch.pending_caller.take() {
            Some(t) => t,
            None => return IpcError::Eproto as i64,
        };
        // The caller's wake is in flight until the reply switch or the
        // unblock below lands.
        tripwire::mark_wake_pending(caller_tid, WakeSource::Reply);
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
    // not fire later and spuriously fail the call with ETIMEDOUT. The result
    // stays with this wake until the reply switch or the unblock fallback
    // classifies it (tripwire N2 counters).
    let clear = clear_timeout(caller_tid);
    tripwire::note_clear(clear);

    // Try direct switch back to caller (fast path).
    // This bypasses the scheduler — replier switches directly to caller.
    if direct::try_reply_switch(replier_tid, caller_tid, channel, clear) {
        // Replier was saved and will be resumed by scheduler later.
        // The caller has already been restored and is running.
        return 0;
    }

    // Fallback: unblock via scheduler (caller on different CPU, etc.)
    let outcome = sched::unblock(caller_tid, WakeSource::Reply);
    tripwire::note_reply_wake(outcome, u64::from(channel.raw()), clear);

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
    let ch = match channel_mut(&mut table, channel) {
        Ok(c) => c,
        Err(e) => return e,
    };

    if ch.state_a == EndpointState::Dead || ch.state_b == EndpointState::Dead {
        return IpcError::Epipe as i64;
    }

    if !ch.ring.push(msg) {
        return IpcError::Eagain as i64;
    }

    // Wake receiver if waiting — clear their timeout first. A receiver in
    // ipc_select took its own slot and re-scans when woken: it finds the
    // message ready.
    if let Some(recv_tid) = ch.waiting_receiver.take() {
        // The receiver's wake is in flight until the unblock below lands.
        tripwire::mark_wake_pending(recv_tid, WakeSource::Send);
        drop(table);
        // The result stays with this wake until its unblock fallback
        // classifies it (tripwire N2 counters).
        let clear = clear_timeout(recv_tid);
        tripwire::note_clear(clear);
        let outcome = sched::unblock(recv_tid, WakeSource::Send);
        tripwire::note_send_wake(outcome, u64::from(channel.raw()), clear);
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
    let ch = match channel_mut(&mut table, channel) {
        Ok(c) => c,
        Err(e) => return e,
    };

    let caller_tid = match ch.pending_caller.take() {
        Some(t) => t,
        None => return 0, // Nothing to cancel.
    };
    // The caller's wake is in flight until wake_with_error reaches unblock.
    tripwire::mark_wake_pending(caller_tid, WakeSource::Cancel);

    drop(table);
    wake_with_error(caller_tid, IpcError::Ecanceled as i64, WakeSource::Cancel);
    0
}
