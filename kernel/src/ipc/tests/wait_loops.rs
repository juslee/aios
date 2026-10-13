//! Loop tests of the converted waits (capability-lifetime ADR §9, the loop
//! contract), run from the ipc-timeout thread (pid 1, pinned to CPU 2)
//! before the Timeout test, with the wake helper (`wake_helper`) acting on
//! the waits from the same CPU.
//!
//! Outcome-only (owner, 2026-10-11, Q8): each case checks which wake source
//! fired (the result the wait returned, and for a sleep, a receive or a call
//! whether its backstop fired), the order of events (the helper's stale wake
//! landed inside the wait, which the helper guarantees by waiting for the
//! wait's record, a timed-out wait did not return before its deadline, and
//! a cancelled call returned before it) and the state left behind (no
//! waiter record, registration, deadline or timeout entry). No elapsed tick
//! is read as a pass band. Every wait uses a 2000-tick timeout, a backstop
//! expires 2000 ticks after its wait's deadline, and every helper command
//! is due 5 ticks after it is posted, so crowding on CPU 2 can delay a case
//! but not change its outcome.

use core::sync::atomic::Ordering;

use crate::arch::aarch64::timer::TICK_COUNT;
use crate::ipc::channel::{ipc_call, ipc_recv, ipc_send};
use crate::ipc::notify::{
    arm_deadline, clear_deadline, deadline_of, has_waiter_record, notification_create,
    notification_destroy, notification_wait,
};
use crate::ipc::select::ipc_select;
use crate::ipc::timeout::{has_reply_slot, has_timeout_entry, sleep_ticks};
use crate::ipc::{channel_create, channel_destroy, channel_mut, DeadlineArm, CHANNEL_TABLE};
use crate::sched;
use crate::syscall::IpcError;
use crate::task::ThreadId;
use shared::{Capability, ChannelId, NotificationId, SelectEntry, SelectKind, MAX_MESSAGE_SIZE};

use super::wake_helper::{self, Command};
use super::TEST_PID;

/// Every loop-test wait's timeout.
const LOOP_TICKS: u64 = 2000;
/// Ticks after a wait's own deadline at which its backstop expires.
const BACKSTOP_TICKS: u64 = 2000;
/// Ticks after the post at which a helper command acts.
const DUE_TICKS: u64 = 5;
/// The notification bits the tests signal and wait for.
const MASK: u64 = 0b101;

/// The tick count now.
fn now() -> u64 {
    TICK_COUNT.load(Ordering::Relaxed)
}

/// The failure, if any, in the state a wait of `me` left behind: a waiter
/// record on any notification, or a published deadline.
fn leftover(me: ThreadId) -> Option<&'static str> {
    if has_waiter_record(me) {
        Some("record left")
    } else if deadline_of(me) != u64::MAX {
        Some("deadline left")
    } else {
        None
    }
}

/// Whether channel `ch` names a waiting receiver (`Some(tid)` for that
/// receiver, `None` for any).
fn receiver_is(ch: ChannelId, want: Option<ThreadId>) -> bool {
    let mut table = CHANNEL_TABLE.lock();
    channel_mut(&mut table, ch).is_ok_and(|c| c.waiting_receiver == want)
}

/// Whether channel `ch` names a pending caller (`Some(tid)` for that
/// caller, `None` for none).
fn caller_is(ch: ChannelId, want: Option<ThreadId>) -> bool {
    let mut table = CHANNEL_TABLE.lock();
    channel_mut(&mut table, ch).is_ok_and(|c| c.pending_caller.map(|p| p.tid) == want)
}

/// Log `<test> test: as expected`, or `<test> test: (<case>) <failure>`.
fn report(test: &str, failure: Option<(&str, &str)>) {
    match failure {
        None => crate::kinfo!(Ipc, "{} test: as expected", test),
        Some((case, what)) => crate::kwarn!(Ipc, "{} test: ({}) {}", test, case, what),
    }
}

/// Log `<test> test: as expected`, or `<test> test: <failure>`, for a test
/// of one case.
fn report_one(test: &str, failure: Option<&str>) {
    match failure {
        None => crate::kinfo!(Ipc, "{} test: as expected", test),
        Some(what) => crate::kwarn!(Ipc, "{} test: {}", test, what),
    }
}

/// Run `wait`, a wait of `me` that times out after [`LOOP_TICKS`] and
/// publishes a notification deadline, with a stale wake at +5
/// (`Command::Stale` without `clear_timeout`). `wait` returns whether it
/// timed out (ETIMEDOUT). The failure, if any: not ETIMEDOUT, returned
/// before its deadline, or the stale wake's ([`wake_helper::stale_failure`]).
fn stale_then_timeout(me: ThreadId, wait: impl FnOnce() -> bool) -> Option<&'static str> {
    let stale = Command::Stale {
        target: me,
        clear_timeout: false,
    };
    if !wake_helper::post(stale, DUE_TICKS) {
        return Some("helper busy");
    }
    // The wait's own deadline is at or after this one.
    let deadline = now().saturating_add(LOOP_TICKS);
    let timed_out = wait();
    let returned = now();
    let helper = wake_helper::finish();
    if !timed_out {
        return Some("not ETIMEDOUT");
    }
    if returned < deadline {
        return Some("early");
    }
    wake_helper::stale_failure(helper, false)
}

/// How a wait run by [`backstopped`] ended.
struct Backstopped<R> {
    /// What the wait returned.
    result: R,
    /// Whether it returned before its own deadline.
    early: bool,
    /// Whether the backstop fired first: the wake the case expects was lost.
    backstop_fired: bool,
    /// The helper command's outcome, from [`wake_helper::finish`].
    helper: Option<wake_helper::Outcome>,
}

/// The stale wake of [`backstopped`]'s timed-out cases: one that first
/// removes `me`'s TIMEOUT_QUEUE entry (`Command::Stale` with
/// `clear_timeout`), so only the wait's re-insert of its entry can end it on
/// time.
fn stale_clear(me: ThreadId) -> Command {
    Command::Stale {
        target: me,
        clear_timeout: true,
    }
}

/// Run `wait`, a wait of `me` that times out after [`LOOP_TICKS`] through
/// its TIMEOUT_QUEUE entry, with the helper running `cmd` at +5: a stale
/// wake ([`stale_clear`]) for a case that expects the timeout, or the event
/// a case expects to end the wait.
///
/// A backstop deadline (`arm_deadline`), [`BACKSTOP_TICKS`] after the
/// wait's, is published first: an independent wake source, so a wait whose
/// wake was lost returns instead of hanging. It fired if
/// `NOTIFY_DEADLINES[me]` no longer holds it when the wait returns. It is
/// cleared afterwards; one that fired has already woken the wait. `Err`
/// names a setup failure.
fn backstopped<R>(
    me: ThreadId,
    cmd: Command,
    wait: impl FnOnce() -> R,
) -> Result<Backstopped<R>, &'static str> {
    // The wait's own deadline is at or after this one.
    let deadline = now().saturating_add(LOOP_TICKS);
    let backstop = deadline.saturating_add(BACKSTOP_TICKS);
    if arm_deadline(me, Some(backstop)) == DeadlineArm::Expired {
        return Err("backstop expired");
    }
    if !wake_helper::post(cmd, DUE_TICKS) {
        clear_deadline(me);
        return Err("helper busy");
    }
    let result = wait();
    let early = now() < deadline;
    let backstop_fired = deadline_of(me) != backstop;
    clear_deadline(me);
    let helper = wake_helper::finish();
    Ok(Backstopped {
        result,
        early,
        backstop_fired,
        helper,
    })
}

/// Run `case` on a channel from `channel_create` in pid 1 (minting pid 1's
/// `ChannelAccess`), then destroy the channel and revoke the minted token.
fn on_new_channel(
    me: ThreadId,
    case: fn(ThreadId, ChannelId) -> Option<&'static str>,
) -> Option<&'static str> {
    let Ok((ch, _access)) = channel_create(me) else {
        return Some("create failed");
    };
    let failure = case(me, ch);
    let _ = channel_destroy(ch);
    super::stale_id::revoke_all(TEST_PID, Capability::ChannelAccess(ch));
    failure
}

/// `Notify-loop test`, two cases on one notification:
///
/// - (a) `notification_wait(n, MASK, 2000)` with a stale wake at +5
///   (`Command::Stale`, after the wait's deadline is published): ETIMEDOUT,
///   returned at or past its deadline, the stale wake recorded, no waiter
///   record and no deadline left.
/// - (b) the same wait with `Signal(n, MASK)` at +5: `Ok(MASK)`, nothing
///   left behind. ETIMEDOUT means the signal's wake was lost.
///
/// Logs `Notify-loop test: as expected`, or a line naming the case and the
/// check that failed.
pub(super) fn notify_loop_test(me: ThreadId) {
    let n = match notification_create(TEST_PID) {
        Ok(n) => n,
        Err(e) => {
            crate::kwarn!(Ipc, "Notify-loop test: create failed: {}", e);
            return;
        }
    };
    let failure = notify_case_a(me, n)
        .map(|f| ("a", f))
        .or_else(|| notify_case_b(me, n).map(|f| ("b", f)));
    notification_destroy(n);
    wake_helper::reclaim();
    report("Notify-loop", failure);
}

/// Notify-loop case (a): a stale wake, then the deadline.
fn notify_case_a(me: ThreadId, n: NotificationId) -> Option<&'static str> {
    stale_then_timeout(me, || {
        notification_wait(n, MASK, LOOP_TICKS) == Err(IpcError::Etimedout as i64)
    })
    .or_else(|| leftover(me))
}

/// Notify-loop case (b): a signal inside the wait.
fn notify_case_b(me: ThreadId, n: NotificationId) -> Option<&'static str> {
    if !wake_helper::post(Command::Signal { n, bits: MASK }, DUE_TICKS) {
        return Some("helper busy");
    }
    let result = notification_wait(n, MASK, LOOP_TICKS);
    let helper = wake_helper::finish();
    match result {
        Ok(bits) if bits == MASK => {}
        Err(e) if e == IpcError::Etimedout as i64 => return Some("ETIMEDOUT"),
        _ => return Some("wrong result"),
    }
    if helper.is_none_or(|o| o.code != 0) {
        return Some("signal failed");
    }
    leftover(me)
}

/// The channels and notification the Select-loop cases share.
#[derive(Clone, Copy)]
struct SelectSet {
    n1: NotificationId,
    ch1: ChannelId,
    ch2: ChannelId,
}

impl SelectSet {
    /// The `[n1, ch1]` set of cases (a)-(c).
    fn pair(&self) -> [SelectEntry; 2] {
        [
            SelectEntry {
                kind: SelectKind::Notification(self.n1, MASK),
            },
            SelectEntry {
                kind: SelectKind::Channel(self.ch1),
            },
        ]
    }

    /// The failure, if any, in what a select of `me` on the pair left
    /// behind: a registration (a `Select` record or `ch1`'s receiver slot)
    /// or a deadline.
    fn leftover(&self, me: ThreadId) -> Option<&'static str> {
        if !receiver_is(self.ch1, None) {
            return Some("receiver left");
        }
        leftover(me)
    }
}

/// `Select-loop test`. Channels from `channel_create` in pid 1 (each mints
/// pid 1's `ChannelAccess`), a notification from `notification_create`;
/// all are destroyed afterwards, and the minted tokens revoked.
///
/// - (a) `select([n1, ch1], 2000)` with a stale wake at +5: ETIMEDOUT,
///   returned at or past its deadline, the stale wake recorded, no
///   registration and no deadline left.
/// - (b) the same select with `Signal(n1, MASK)` at +5: `Ok((0, MASK))`.
/// - (c) the same select with `Send(ch1)` at +5: `Ok((1, 0))`; then
///   `ipc_recv` drains ch1.
/// - (d) the helper blocks in `RecvBlock(ch2, 2000)`; the driver yields
///   until the helper is ch2's waiting receiver (at most 32 yields), then
///   `select([ch2], 2000)` must return EAGAIN without
///   blocking, and `ipc_send(ch2)` releases the helper, whose receive must
///   return `Ok`.
///
/// In (b) and (c) ETIMEDOUT means the event's wake was lost. Logs
/// `Select-loop test: as expected`, or a line naming the case and the check
/// that failed, for example `Select-loop test: (d) ETIMEDOUT`.
pub(super) fn select_loop_test(me: ThreadId) {
    let ch1 = channel_create(me).ok().map(|(ch, _access)| ch);
    let ch2 = channel_create(me).ok().map(|(ch, _access)| ch);
    let n1 = notification_create(TEST_PID).ok();
    let failure = match (ch1, ch2, n1) {
        (Some(ch1), Some(ch2), Some(n1)) => {
            let set = SelectSet { n1, ch1, ch2 };
            select_case_a(me, &set)
                .map(|f| ("a", f))
                .or_else(|| select_case_b(me, &set).map(|f| ("b", f)))
                .or_else(|| select_case_c(me, &set).map(|f| ("c", f)))
                .or_else(|| select_case_d(me, &set).map(|f| ("d", f)))
        }
        _ => Some(("setup", "create failed")),
    };
    for ch in [ch1, ch2].into_iter().flatten() {
        let _ = channel_destroy(ch);
        super::stale_id::revoke_all(TEST_PID, Capability::ChannelAccess(ch));
    }
    if let Some(n1) = n1 {
        notification_destroy(n1);
    }
    // A case that failed with the helper still in its command: the destroys
    // above ended it.
    wake_helper::reclaim();
    report("Select-loop", failure);
}

/// Select-loop case (a): a stale wake, then the deadline.
fn select_case_a(me: ThreadId, set: &SelectSet) -> Option<&'static str> {
    stale_then_timeout(me, || {
        ipc_select(&set.pair(), LOOP_TICKS) == Err(IpcError::Etimedout as i64)
    })
    .or_else(|| set.leftover(me))
}

/// Select-loop case (b): a signal of n1 inside the select.
fn select_case_b(me: ThreadId, set: &SelectSet) -> Option<&'static str> {
    let signal = Command::Signal {
        n: set.n1,
        bits: MASK,
    };
    if !wake_helper::post(signal, DUE_TICKS) {
        return Some("helper busy");
    }
    let result = ipc_select(&set.pair(), LOOP_TICKS);
    let helper = wake_helper::finish();
    match result {
        Ok((0, bits)) if bits == MASK => {}
        Err(e) if e == IpcError::Etimedout as i64 => return Some("ETIMEDOUT"),
        _ => return Some("wrong result"),
    }
    if helper.is_none_or(|o| o.code != 0) {
        return Some("signal failed");
    }
    set.leftover(me)
}

/// Select-loop case (c): a message on ch1 inside the select.
fn select_case_c(me: ThreadId, set: &SelectSet) -> Option<&'static str> {
    if !wake_helper::post(Command::Send { ch: set.ch1 }, DUE_TICKS) {
        return Some("helper busy");
    }
    let result = ipc_select(&set.pair(), LOOP_TICKS);
    let helper = wake_helper::finish();
    let mut buf = [0u8; MAX_MESSAGE_SIZE];
    let drained = ipc_recv(set.ch1, &mut buf, 0).is_ok();
    match result {
        Ok((1, 0)) => {}
        Err(e) if e == IpcError::Etimedout as i64 => return Some("ETIMEDOUT"),
        _ => return Some("wrong result"),
    }
    if helper.is_none_or(|o| o.code != 0) {
        return Some("send failed");
    }
    if !drained {
        return Some("drain failed");
    }
    set.leftover(me)
}

/// Select-loop case (d): ch2's receiver slot is taken, so the select fails
/// EAGAIN (#186 item 2) after its arm, without blocking. The driver waits
/// for the helper to be ch2's waiting receiver; the helper's receive armed
/// before it took the slot, so the send that releases it is kept as a token
/// if the helper has not blocked yet.
fn select_case_d(me: ThreadId, set: &SelectSet) -> Option<&'static str> {
    let Some(helper) = wake_helper::helper_tid() else {
        return Some("no helper");
    };
    let recv = Command::RecvBlock {
        ch: set.ch2,
        timeout: LOOP_TICKS,
    };
    if !wake_helper::post(recv, 0) {
        return Some("helper busy");
    }
    let receiving = || receiver_is(set.ch2, Some(helper));
    let mut yields = 0;
    while !receiving() && yields < wake_helper::MAX_POLLS {
        sched::thread_yield();
        yields += 1;
    }
    if !receiving() {
        // The caller destroys ch2 next, which ends a receive that started.
        return Some("helper not receiving");
    }

    let entries = [SelectEntry {
        kind: SelectKind::Channel(set.ch2),
    }];
    let result = ipc_select(&entries, LOOP_TICKS);
    let sent = ipc_send(set.ch2, b"RELEASE") == 0;
    let helper_done = wake_helper::finish();
    match result {
        Err(e) if e == IpcError::Eagain as i64 => {}
        Err(e) if e == IpcError::Etimedout as i64 => return Some("ETIMEDOUT"),
        _ => return Some("wrong result"),
    }
    if !sent {
        return Some("send failed");
    }
    if helper_done.is_none_or(|o| o.code < 0) {
        return Some("helper receive failed");
    }
    leftover(me)
}

/// `Sleep-loop test`: `sleep_ticks(2000)` run [`backstopped`], whose stale
/// wake is a stale reply that clears the sleeper's entry and wakes it.
///
/// Pass: the sleep returned with the backstop not fired, so the sleep's own
/// re-inserted entry ended it; at or past its deadline; the helper removed
/// an entry; and no TIMEOUT_QUEUE entry is left. Logs `Sleep-loop test: as
/// expected`, or `Sleep-loop test: entry lost` (the backstop fired first),
/// `Sleep-loop test: early` (returned before its deadline) or a line naming
/// the helper's or the leftover's failure.
pub(super) fn sleep_loop_test(me: ThreadId) {
    let failure = sleep_case(me);
    wake_helper::reclaim();
    report_one("Sleep-loop", failure);
}

/// The Sleep-loop case: the failure, if any.
fn sleep_case(me: ThreadId) -> Option<&'static str> {
    let end = match backstopped(me, stale_clear(me), || sleep_ticks(LOOP_TICKS)) {
        Ok(end) => end,
        Err(failure) => return Some(failure),
    };
    if end.backstop_fired {
        return Some("entry lost");
    }
    if end.early {
        return Some("early");
    }
    wake_helper::stale_failure(end.helper, true)
        .or_else(|| has_timeout_entry(me).then_some("entry left"))
}

/// `Recv-loop test`: `ipc_recv(ch, 2000)` on a pid-1 channel with no sender
/// ([`on_new_channel`]), run [`backstopped`], whose stale wake is a stale
/// sender that clears the receiver's entry and wakes it.
///
/// Pass: ETIMEDOUT with the backstop not fired (the receive's own
/// re-inserted entry ended it), at or past its deadline; the helper removed
/// an entry; and afterwards the channel's `waiting_receiver` is `None` and no
/// TIMEOUT_QUEUE entry is left. Logs `Recv-loop test: as expected`, or
/// `Recv-loop test: entry lost` (the backstop fired first), `Recv-loop test:
/// early` (returned before its deadline), the wrong result by name, or a line
/// naming the helper's or the leftover's failure.
pub(super) fn recv_loop_test(me: ThreadId) {
    let failure = on_new_channel(me, recv_case);
    // A case that failed with the helper still in its command: the receive
    // it waited for has returned, so the command is abandoned.
    wake_helper::reclaim();
    report_one("Recv-loop", failure);
}

/// The Recv-loop case on channel `ch`: the failure, if any.
fn recv_case(me: ThreadId, ch: ChannelId) -> Option<&'static str> {
    let receive = || {
        let mut buf = [0u8; MAX_MESSAGE_SIZE];
        ipc_recv(ch, &mut buf, LOOP_TICKS)
    };
    let end = match backstopped(me, stale_clear(me), receive) {
        Ok(end) => end,
        Err(failure) => return Some(failure),
    };
    if end.backstop_fired {
        return Some("entry lost");
    }
    match end.result {
        Err(e) if e == IpcError::Etimedout as i64 => {}
        Ok(_) => return Some("Ok"),
        Err(e) if e == IpcError::Eagain as i64 => return Some("EAGAIN"),
        Err(e) if e == IpcError::Epipe as i64 => return Some("EPIPE"),
        Err(_) => return Some("wrong error"),
    }
    if end.early {
        return Some("early");
    }
    wake_helper::stale_failure(end.helper, true)
        .or_else(|| (!receiver_is(ch, None)).then_some("receiver left"))
        .or_else(|| has_timeout_entry(me).then_some("entry left"))
}

/// `Call-loop test`: `ipc_call(ch, …, 2000)` on a pid-1 channel with no
/// server ([`on_new_channel`]), run [`backstopped`], whose stale wake is a
/// stale replier that clears the caller's entry and wakes it.
///
/// Pass: ETIMEDOUT with the backstop not fired (the call's own re-inserted
/// entry ended it), at or past its deadline (before the loop, the stale wake
/// returned 0 bytes); the helper removed an entry; and afterwards the
/// channel's `pending_caller` and this thread's reply slot are `None` and no
/// TIMEOUT_QUEUE entry is left. The request no server took is drained with
/// `ipc_recv(ch, 0)`. Logs `Call-loop test: as expected`, or `Call-loop
/// test: entry lost` (the backstop fired first), `Call-loop test: early`
/// (returned before its deadline), the wrong result by name, or a line
/// naming the helper's, the leftover's or the drain's failure.
pub(super) fn call_loop_test(me: ThreadId) {
    let failure = on_new_channel(me, call_case);
    // A case that failed with the helper still in its command: the call it
    // waited for has returned, so the command is abandoned.
    wake_helper::reclaim();
    report_one("Call-loop", failure);
}

/// The Call-loop case on channel `ch`: the failure, if any.
fn call_case(me: ThreadId, ch: ChannelId) -> Option<&'static str> {
    let call = || {
        let mut reply = [0u8; MAX_MESSAGE_SIZE];
        ipc_call(ch, b"CALL-LOOP", &mut reply, LOOP_TICKS)
    };
    let end = match backstopped(me, stale_clear(me), call) {
        Ok(end) => end,
        Err(failure) => return Some(failure),
    };
    // The request is still queued: no server took it. The poll pops it
    // before any arm, so the drain takes no WAKEUP_ERRORS code.
    let mut buf = [0u8; MAX_MESSAGE_SIZE];
    let drained = ipc_recv(ch, &mut buf, 0).is_ok();
    if end.backstop_fired {
        return Some("entry lost");
    }
    match end.result {
        e if e == IpcError::Etimedout as i64 => {}
        n if n >= 0 => return Some("Ok"),
        e if e == IpcError::Eagain as i64 => return Some("EAGAIN"),
        e if e == IpcError::Epipe as i64 => return Some("EPIPE"),
        e if e == IpcError::Ecanceled as i64 => return Some("ECANCELED"),
        _ => return Some("wrong error"),
    }
    if end.early {
        return Some("early");
    }
    wake_helper::stale_failure(end.helper, true)
        .or_else(|| (!caller_is(ch, None)).then_some("caller left"))
        .or_else(|| has_reply_slot(me).then_some("reply slot left"))
        .or_else(|| has_timeout_entry(me).then_some("entry left"))
        .or_else(|| (!drained).then_some("drain failed"))
}

/// `Cancel-loop test`: `ipc_call(ch, …, 2000)` on a pid-1 channel with no
/// server ([`on_new_channel`]), run [`backstopped`] with `ipc_cancel(ch)` by
/// the helper at +5 (`Command::Cancel`, once this thread is `ch`'s
/// `pending_caller`). The cancel claims the call (takes `pending_caller`,
/// the call's `PendingCall { tid, seq }`), marks this thread's reply slot
/// `cancelled` (its `seq` matches the claimed call's) and wakes it; the
/// call's next pass reads `cancelled` and the claim.
///
/// Pass: ECANCELED before the call's deadline, with the backstop not fired;
/// the helper's `ipc_cancel` returned 0; and afterwards the channel's
/// `pending_caller` and this thread's reply slot are `None` and no
/// TIMEOUT_QUEUE entry is left. The request the cancel left queued is
/// drained with `ipc_recv(ch, 0)`. Logs
/// `Cancel-loop test: as expected`, or `Cancel-loop test: wake lost` (the
/// cancel's wake was lost: the call returned at or past its deadline, or the
/// backstop fired), the wrong result by name, or a line naming the helper's,
/// the leftover's or the drain's failure.
///
/// The `seq` mismatch path of `ipc_reply` and `ipc_cancel` (the claimed
/// call's caller left it and posted its next call's slot between the claim
/// and the slot write) cannot be forced at boot: the claim and the write run
/// back to back in the replier or canceller, and only a preemption landing
/// between them would let the caller run. The host model
/// (`shared::sched`'s `call_model`) covers it.
pub(super) fn cancel_loop_test(me: ThreadId) {
    let failure = on_new_channel(me, cancel_case);
    // A case that failed with the helper still in its command: the call it
    // waited for has returned, so the command is abandoned.
    wake_helper::reclaim();
    report_one("Cancel-loop", failure);
}

/// The Cancel-loop case on channel `ch`: the failure, if any.
fn cancel_case(me: ThreadId, ch: ChannelId) -> Option<&'static str> {
    let call = || {
        let mut reply = [0u8; MAX_MESSAGE_SIZE];
        ipc_call(ch, b"CANCEL-LOOP", &mut reply, LOOP_TICKS)
    };
    let end = match backstopped(me, Command::Cancel { ch }, call) {
        Ok(end) => end,
        Err(failure) => return Some(failure),
    };
    // The request is still queued: the cancel claimed the call, not its
    // message. The poll pops it before any arm, so the drain takes no
    // WAKEUP_ERRORS code.
    let mut buf = [0u8; MAX_MESSAGE_SIZE];
    let drained = ipc_recv(ch, &mut buf, 0).is_ok();
    if end.backstop_fired {
        return Some("wake lost");
    }
    match end.result {
        e if e == IpcError::Ecanceled as i64 => {}
        n if n >= 0 => return Some("Ok"),
        e if e == IpcError::Etimedout as i64 => return Some("ETIMEDOUT"),
        e if e == IpcError::Eagain as i64 => return Some("EAGAIN"),
        e if e == IpcError::Epipe as i64 => return Some("EPIPE"),
        _ => return Some("wrong error"),
    }
    // ECANCELED at or past the deadline: the call's own timeout entry woke
    // it, not the cancel, and its next pass read the claim.
    if !end.early {
        return Some("wake lost");
    }
    match end.helper {
        None => return Some("helper not done"),
        Some(o) if o.acted_at.is_none() => return Some("cancel abandoned"),
        Some(o) if o.code != 0 => return Some("cancel failed"),
        Some(_) => {}
    }
    (!caller_is(ch, None))
        .then_some("caller left")
        .or_else(|| has_reply_slot(me).then_some("reply slot left"))
        .or_else(|| has_timeout_entry(me).then_some("entry left"))
        .or_else(|| (!drained).then_some("drain failed"))
}
