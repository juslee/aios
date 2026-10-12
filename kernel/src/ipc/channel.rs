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
use shared::tripwire::{Key, WakeSource};
use shared::{deadline_after, ChannelId, EndpointState, RawMessage, MAX_MESSAGE_SIZE};

use super::timeout::{
    arm_timeout, clear_own_timeout, clear_timeout, get_wakeup_error, next_call_seq, ReplySlot,
    REPLY_SLOTS,
};
use super::{channel_mut, direct, DeadlineArm, PendingCall, CHANNEL_TABLE};

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

/// What a pass after the first found in `me`'s reply slot.
enum ReplyRead {
    /// The reply is written: its length. The slot has been taken.
    Done(usize),
    /// No reply yet, and whether a canceller has marked the call cancelled.
    Waiting { cancelled: bool },
}

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
///
/// The caller's tid comes from `sched::current_tid()` (EPERM with no current
/// thread). The first pass writes the reply slot (`REPLY_SLOTS[me]`, with
/// `done` and `cancelled` clear; not a waker record, so it comes before the
/// arm), then, in one CHANNEL_TABLE critical section, fails on a lookup
/// error, a Dead endpoint (EPIPE), another pending caller (EAGAIN) or a full
/// ring (ENOSPC), with the slot taken back and nothing armed, or queues the
/// request, arms (`wake::prepare_block`), publishes itself as the channel's
/// `pending_caller` and takes the waiting receiver. After the release it
/// publishes its timeout entry (`timeout::arm_timeout`), clears the
/// receiver's entry and wakes it, by a direct switch when it can (the
/// caller resumes once the reply switches back to it) and otherwise by
/// `unblock`, and blocks. A deadline that had already passed at the entry
/// still wakes the receiver, but cancels the arm and does not block.
///
/// The call's sequence number (`timeout::next_call_seq`) goes into both the
/// reply slot and the `pending_caller` entry (`PendingCall`), so a replier
/// or canceller that claimed an earlier call of this thread finds a `seq`
/// mismatch and leaves this call's slot alone.
///
/// Every later pass arms, then reads the reply slot (REPLY_SLOTS): a written
/// reply ends the call. Then, under CHANNEL_TABLE, a destroyed channel or a
/// Dead endpoint ends it with EPIPE, and a claimed call (`pending_caller` no
/// longer this call) that the slot showed cancelled ends it with
/// ECANCELED; a claimed call that is not cancelled has a reply in flight and
/// keeps waiting. Then the deadline: ETIMEDOUT once it has passed, otherwise
/// the timeout entry is re-inserted if a waker removed it, and the pass
/// blocks. The canceller claims before it sets `cancelled`, and the pass
/// reads `cancelled` before the claim, so a cancel landing between the two
/// reads leaves a token and is seen on the next pass. A stale wake costs one
/// pass, never an early return.
///
/// The exit clears `pending_caller` if it is still this call and takes the
/// reply slot (a written reply wins over a concurrent timeout, cancel or
/// destroy), both skipped after a pass read the reply, which leaves neither
/// behind; then it removes the timeout entry, takes and discards the thread's
/// `WAKEUP_ERRORS` code once (it decides nothing, capability-lifetime ADR
/// §9), and cancels the arm. The first pass's failures return before the
/// arm and take nothing.
pub fn ipc_call(
    channel: ChannelId,
    send_buf: &[u8],
    recv_buf: &mut [u8],
    timeout_ticks: u64,
) -> i64 {
    if send_buf.len() > MAX_MESSAGE_SIZE {
        return IpcError::Enospc as i64;
    }

    let Some(me) = sched::current_tid() else {
        return IpcError::Eperm as i64;
    };

    // Capability enforcement: check ChannelAccess (fail-closed).
    let Some(pid) = crate::cap::process_of_thread(me) else {
        return IpcError::Eperm as i64;
    };
    if let Err(e) = crate::cap::check_channel_access(pid, channel) {
        return e;
    }

    let mut msg = RawMessage::EMPTY;
    msg.sender = me;
    msg.len = send_buf.len();
    msg.data[..send_buf.len()].copy_from_slice(send_buf);

    // --- Pass 1 ---
    // This call's sequence number, carried by its reply slot and its
    // pending_caller entry.
    let seq = next_call_seq(me);
    // The reply slot first: a replier that takes pending_caller must find it.
    post_reply_slot(me, seq, recv_buf);
    let (deadline, receiver) = match call_publish(me, seq, channel, msg, timeout_ticks) {
        Ok(published) => published,
        Err(code) => {
            take_reply_slot(me);
            return code;
        }
    };

    #[cfg(feature = "kernel-metrics")]
    METRICS.ipc_call.inc();

    // Publish the timeout entry after the arm (ADR §8 rule 1). The tripwire
    // N2 phase becomes armed inside the TIMEOUT_QUEUE hold.
    let arm = arm_timeout(
        me,
        deadline,
        IpcError::Etimedout as i64,
        Some(WaitSide::Call),
    );
    let blocked_on = ThreadState::BlockedIpc {
        channel: u64::from(channel.raw()),
    };
    if let Some(recv_tid) = receiver {
        // The receiver is woken by the queued request: clear its timeout
        // first. The result stays with this wake until its unblock fallback
        // classifies it (tripwire N2 counters).
        let recv_clear = clear_timeout(recv_tid);
        tripwire::note_clear(recv_clear);
        // A direct switch blocks this thread, so it needs a published
        // deadline (or none at all). It fails, and the scheduler path
        // follows, when the receiver is not blocked in ipc_recv (an armed
        // receiver, or one in ipc_select) or when this thread holds a token.
        let switched = arm == DeadlineArm::Armed && direct::try_direct_switch(me, recv_tid);
        if !switched {
            let outcome = sched::unblock(recv_tid, WakeSource::Call);
            tripwire::note_send_wake(outcome, u64::from(channel.raw()), recv_clear);
            if arm == DeadlineArm::Armed {
                sched::block_current(blocked_on);
            }
        }
    } else if arm == DeadlineArm::Armed {
        sched::block_current(blocked_on);
    }
    if arm == DeadlineArm::Expired {
        // The deadline had passed: no entry was published, and the pass did
        // not block. The next pass re-arms before it re-checks the reply,
        // the claim and the deadline, so cancelling loses no wake.
        wake::cancel_block(me);
    }

    // --- Passes 2 and later ---
    let result = loop {
        wake::prepare_block(me);
        let cancelled = match reply_read(me) {
            ReplyRead::Done(len) => break Ok(len),
            ReplyRead::Waiting { cancelled } => cancelled,
        };
        match call_claimed(me, seq, channel) {
            None => break Err(IpcError::Epipe as i64),
            Some(true) if cancelled => break Err(IpcError::Ecanceled as i64),
            // Unclaimed, or claimed with a reply in flight: keep waiting.
            Some(_) => {}
        }
        let arm = arm_timeout(
            me,
            deadline,
            IpcError::Etimedout as i64,
            Some(WaitSide::Call),
        );
        if arm == DeadlineArm::Expired {
            break Err(IpcError::Etimedout as i64);
        }
        sched::block_current(blocked_on);
    };

    // --- Exit: every exit comes from a pass that armed ---
    // The call's tripwire N2 phase ends here.
    tripwire::wait_ended(WaitSide::Call, me);
    // A reply read by a pass needs neither hold below: reply_read took the
    // slot, and the replier that marked it done had taken this call's
    // pending_caller entry, which only this call's call_publish ever wrote.
    let result = match result {
        Ok(len) => Ok(len),
        Err(code) => {
            {
                let mut table = CHANNEL_TABLE.lock();
                if let Ok(ch) = channel_mut(&mut table, channel) {
                    if ch.pending_caller == Some(PendingCall { tid: me, seq }) {
                        ch.pending_caller = None;
                    }
                }
            }
            // A written reply wins over a concurrent timeout, cancel or
            // destroy.
            match take_reply_slot(me) {
                Some(slot) if slot.done => Ok(slot.bytes_written),
                _ => Err(code),
            }
        }
    };
    clear_own_timeout(me);
    // Taken once per wait and discarded: it decides nothing (ADR §9).
    daif::with_irqs_masked(|| get_wakeup_error(me));
    wake::cancel_block(me);

    #[cfg(feature = "kernel-metrics")]
    if result == Err(IpcError::Etimedout as i64) {
        METRICS.ipc_timeout.inc();
    }
    match result {
        Ok(len) => len as i64,
        Err(code) => code,
    }
}

/// `ipc_call`'s first-pass critical section, under CHANNEL_TABLE: fail on a
/// lookup error, a Dead endpoint (EPIPE), another pending caller (EAGAIN)
/// or a full ring (ENOSPC), before any arm. Otherwise queue `msg`, arm
/// (`wake::prepare_block`, ADR §8 rule 1), publish `me` and the call's `seq`
/// as the pending call and take the waiting receiver, whose wake is then in
/// flight. Returns the call's deadline, counted from the queued request
/// (`None`: no timeout, for 0 as for a value whose deadline would reach
/// `u64::MAX`), and the receiver to wake.
fn call_publish(
    me: ThreadId,
    seq: u32,
    channel: ChannelId,
    msg: RawMessage,
    timeout_ticks: u64,
) -> Result<(Option<u64>, Option<ThreadId>), i64> {
    let mut table = CHANNEL_TABLE.lock();
    let ch = channel_mut(&mut table, channel)?;
    if ch.state_a == EndpointState::Dead || ch.state_b == EndpointState::Dead {
        return Err(IpcError::Epipe as i64);
    }
    // Only one pending caller per channel at a time.
    if ch.pending_caller.is_some() {
        return Err(IpcError::Eagain as i64);
    }
    if !ch.ring.push(msg) {
        return Err(IpcError::Enospc as i64);
    }
    // 0 is no timeout for ipc_call (capability-lifetime ADR §9), not a poll.
    let deadline = if timeout_ticks == 0 {
        None
    } else {
        deadline_from_now(timeout_ticks)
    };
    wake::prepare_block(me);
    ch.pending_caller = Some(PendingCall { tid: me, seq });
    // Tripwire N2 phase: published, timeout not yet registered.
    tripwire::wait_published(
        WaitSide::Call,
        me,
        u64::from(channel.raw()),
        deadline.is_some(),
    );
    let receiver = ch.waiting_receiver.take();
    if let Some(recv_tid) = receiver {
        // The receiver's wake is in flight until the direct switch or the
        // unblock lands.
        tripwire::mark_wake_pending(recv_tid, WakeSource::Call);
    }
    Ok((deadline, receiver))
}

/// Write `me`'s reply slot for the call `seq` starting now: `recv_buf`,
/// nothing written, neither done nor cancelled.
fn post_reply_slot(me: ThreadId, seq: u32, recv_buf: &mut [u8]) {
    if let Some(entry) = REPLY_SLOTS.lock().get_mut(me.0 as usize) {
        *entry = Some(ReplySlot {
            buf: recv_buf.as_mut_ptr(),
            buf_len: recv_buf.len(),
            bytes_written: 0,
            done: false,
            cancelled: false,
            seq,
        });
    }
}

/// Take `me`'s reply slot back, if it is still there.
fn take_reply_slot(me: ThreadId) -> Option<ReplySlot> {
    REPLY_SLOTS
        .lock()
        .get_mut(me.0 as usize)
        .and_then(Option::take)
}

/// A later pass's read of `me`'s reply slot, under REPLY_SLOTS: a written
/// reply is taken, with its length; otherwise whether the call was
/// cancelled. A missing slot (process exit step C dropped a victim's) reads
/// as waiting and not cancelled: the victim is Dead, and its next
/// `block_current` drops it.
fn reply_read(me: ThreadId) -> ReplyRead {
    let mut slots = REPLY_SLOTS.lock();
    let Some(entry) = slots.get_mut(me.0 as usize) else {
        return ReplyRead::Waiting { cancelled: false };
    };
    match entry {
        Some(slot) if slot.done => {
            let len = slot.bytes_written;
            *entry = None;
            ReplyRead::Done(len)
        }
        Some(slot) => ReplyRead::Waiting {
            cancelled: slot.cancelled,
        },
        None => ReplyRead::Waiting { cancelled: false },
    }
}

/// A later pass's channel check, under CHANNEL_TABLE: `None` when the
/// channel is gone (destroyed, or its slot reused) or has a Dead endpoint,
/// the call's EPIPE; otherwise whether the call `seq` has been claimed, that
/// is, a replier or canceller has taken `pending_caller`.
fn call_claimed(me: ThreadId, seq: u32, channel: ChannelId) -> Option<bool> {
    let mut table = CHANNEL_TABLE.lock();
    let ch = channel_mut(&mut table, channel).ok()?;
    if ch.state_a == EndpointState::Dead || ch.state_b == EndpointState::Dead {
        return None;
    }
    Some(ch.pending_caller != Some(PendingCall { tid: me, seq }))
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
///
/// The replier's tid comes from `sched::current_tid()` (EPERM with no
/// current thread). It claims the call (takes `pending_caller`; EPROTO with
/// none), then, under REPLY_SLOTS, copies the reply into the caller's
/// buffer and marks the slot `done`, the condition the caller's next pass
/// reads, then clears the caller's timeout and wakes it. A caller that is
/// still armed (not yet blocked) fails the reply switch, and the fallback
/// `unblock` leaves it the token.
///
/// The write needs the slot's `seq` to equal the claimed call's. A slot that
/// is gone, or carries another `seq` (the caller left the claimed call and
/// started its next one between the claim and the write), gets nothing: the
/// late reply counts `latereply`, skips the timeout clear and the reply
/// switch, wakes the caller with a plain `unblock` (a stale wake costs it at
/// most one pass), and still returns 0.
pub fn ipc_reply(channel: ChannelId, reply_buf: &[u8]) -> i64 {
    if reply_buf.len() > MAX_MESSAGE_SIZE {
        return IpcError::Enospc as i64;
    }

    let Some(replier_tid) = sched::current_tid() else {
        return IpcError::Eperm as i64;
    };

    // Claim the call: take the pending caller and its call's seq.
    let PendingCall {
        tid: caller_tid,
        seq,
    } = {
        let mut table = CHANNEL_TABLE.lock();
        let ch = match channel_mut(&mut table, channel) {
            Ok(c) => c,
            Err(e) => return e,
        };

        let pending = match ch.pending_caller.take() {
            Some(p) => p,
            None => return IpcError::Eproto as i64,
        };
        // The caller's wake is in flight until the reply switch or the
        // unblock below lands.
        tripwire::mark_wake_pending(pending.tid, WakeSource::Reply);
        pending
    };

    // Copy the reply into the caller's reply buffer and mark it done, if the
    // slot is still the claimed call's. A caller that has already left the
    // call (its deadline passed after this claim) took its slot back, and
    // may have posted its next call's slot since, with another seq: either
    // way the reply is dropped.
    let delivered = {
        let mut slots = REPLY_SLOTS.lock();
        match slots
            .get_mut(caller_tid.0 as usize)
            .and_then(Option::as_mut)
        {
            Some(slot) if slot.seq == seq => {
                let copy_len = reply_buf.len().min(slot.buf_len);
                // SAFETY: The slot is the claimed call's, because its seq
                // matched, and the reply buffer pointer was provided by the
                // caller thread, which is still inside that ipc_call while
                // the slot exists: it holds the buffer's &mut borrow until its
                // exit takes the slot back under REPLY_SLOTS, held here, and
                // it never touches the buffer in between. So the buffer is
                // valid and not concurrently accessed. copy_len is bounded by
                // both the reply payload size and the buffer capacity.
                // Writing through a slot whose call had returned would
                // corrupt a dead stack frame, and writing into another call's
                // slot would hand that call this reply.
                unsafe {
                    core::ptr::copy_nonoverlapping(reply_buf.as_ptr(), slot.buf, copy_len);
                }
                slot.bytes_written = copy_len;
                slot.done = true;
                true
            }
            _ => false,
        }
    };

    if !delivered {
        // A late reply: the claimed call has ended. The caller's timeout
        // entry, if any, belongs to its next wait, so it is not cleared, and
        // the caller is not switched to. The unblock lands the wake marked at
        // the claim; a stale wake costs the caller at most one pass.
        tripwire::bump(Key::Latereply, 0);
        sched::unblock(caller_tid, WakeSource::Reply);
        return 0;
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

/// Cancel a pending IpcCall on a channel: the call returns ECANCELED, unless
/// its reply was written first. Returns 0, also when no call is pending.
///
/// The canceller's tid comes from `sched::current_tid()` (EPERM with no
/// current thread or no process). It claims the call (takes
/// `pending_caller`), then, under REPLY_SLOTS, marks the caller's slot
/// `cancelled` if it is present, still the claimed call's (its `seq`
/// matches; a caller that left the call and started its next one between
/// the claim and the mark has a slot with another `seq`) and not `done`,
/// then wakes the caller with `unblock`. The result comes from that state (capability-lifetime ADR §9),
/// so no error code is written: the caller's next pass reads `cancelled`
/// before it reads the claim, and a cancel landing between those reads
/// leaves a token.
pub fn ipc_cancel(channel: ChannelId) -> i64 {
    let Some(me) = sched::current_tid() else {
        return IpcError::Eperm as i64;
    };

    // Capability enforcement: check ChannelAccess (fail-closed).
    let Some(pid) = crate::cap::process_of_thread(me) else {
        return IpcError::Eperm as i64;
    };
    if let Err(e) = crate::cap::check_channel_access(pid, channel) {
        return e;
    }

    // Claim the call.
    let PendingCall {
        tid: caller_tid,
        seq,
    } = {
        let mut table = CHANNEL_TABLE.lock();
        let ch = match channel_mut(&mut table, channel) {
            Ok(c) => c,
            Err(e) => return e,
        };
        let Some(pending) = ch.pending_caller.take() else {
            return 0; // Nothing to cancel.
        };
        // The caller's wake is in flight until the unblock below lands.
        tripwire::mark_wake_pending(pending.tid, WakeSource::Cancel);
        pending
    };

    // Mark the call cancelled, after the claim, if the slot is still the
    // claimed call's. A written reply wins.
    if let Some(slot) = REPLY_SLOTS
        .lock()
        .get_mut(caller_tid.0 as usize)
        .and_then(Option::as_mut)
    {
        if slot.seq == seq && !slot.done {
            slot.cancelled = true;
        }
    }

    // Also on a late cancel: the unblock lands the wake marked at the claim,
    // and a stale wake costs the caller at most one pass.
    sched::unblock(caller_tid, WakeSource::Cancel);
    0
}
