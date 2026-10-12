//! Loop tests of the converted waits (capability-lifetime ADR §9, the loop
//! contract), run from the ipc-timeout thread (pid 1, pinned to CPU 2)
//! before the Timeout test, with the wake helper (`wake_helper`) acting on
//! the waits from the same CPU.
//!
//! Outcome-only (owner, 2026-10-11, Q8): each case checks which wake source
//! fired (the result the wait returned, and for a sleep whether its backstop
//! fired), the order of events (the helper's stale wake landed inside the
//! wait, which the helper guarantees by waiting for the wait's record, and a
//! timed-out wait did not return before its deadline) and the state left
//! behind (no waiter record, registration, deadline or timeout entry). No
//! elapsed tick is read as a pass band. Every wait uses a 2000-tick timeout,
//! a backstop expires 2000 ticks after its wait's deadline, and every helper
//! command is due 5 ticks after it is posted, so crowding on CPU 2 can delay
//! a case but not change its outcome.

use core::sync::atomic::Ordering;

use crate::arch::aarch64::timer::TICK_COUNT;
use crate::ipc::channel::{ipc_recv, ipc_send};
use crate::ipc::notify::{
    arm_deadline, clear_deadline, deadline_of, has_waiter_record, notification_create,
    notification_destroy, notification_wait,
};
use crate::ipc::select::ipc_select;
use crate::ipc::timeout::{has_timeout_entry, sleep_ticks};
use crate::ipc::{channel_create, channel_destroy, channel_mut, DeadlineArm, CHANNEL_TABLE};
use crate::sched;
use crate::syscall::IpcError;
use crate::task::{ThreadId, ThreadState};
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

/// Log `<test> test: as expected`, or `<test> test: (<case>) <failure>`.
fn report(test: &str, failure: Option<(&str, &str)>) {
    match failure {
        None => crate::kinfo!(Ipc, "{} test: as expected", test),
        Some((case, what)) => crate::kwarn!(Ipc, "{} test: ({}) {}", test, case, what),
    }
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
    let stale = Command::Stale {
        target: me,
        clear_timeout: false,
    };
    if !wake_helper::post(stale, DUE_TICKS) {
        return Some("helper busy");
    }
    // The wait's own deadline is at or after this one.
    let deadline = now().saturating_add(LOOP_TICKS);
    let result = notification_wait(n, MASK, LOOP_TICKS);
    let returned = now();
    let helper = wake_helper::finish();
    if result != Err(IpcError::Etimedout as i64) {
        return Some("not ETIMEDOUT");
    }
    if returned < deadline {
        return Some("early");
    }
    wake_helper::stale_failure(helper, false).or_else(|| leftover(me))
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
///   until the helper is ch2's waiting receiver and blocked (at most 32
///   yields), then `select([ch2], 2000)` must return EAGAIN without
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
    let stale = Command::Stale {
        target: me,
        clear_timeout: false,
    };
    if !wake_helper::post(stale, DUE_TICKS) {
        return Some("helper busy");
    }
    let deadline = now().saturating_add(LOOP_TICKS);
    let result = ipc_select(&set.pair(), LOOP_TICKS);
    let returned = now();
    let helper = wake_helper::finish();
    if result != Err(IpcError::Etimedout as i64) {
        return Some("not ETIMEDOUT");
    }
    if returned < deadline {
        return Some("early");
    }
    wake_helper::stale_failure(helper, false).or_else(|| set.leftover(me))
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
/// for the helper to be blocked in its receive, not only published as the
/// receiver: `ipc_recv` is not converted until step 13, and a send that
/// found the helper published but not yet blocked would skip it.
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
    let receiving = || {
        receiver_is(set.ch2, Some(helper))
            && matches!(
                sched::thread_probe(helper, false).state,
                Some(ThreadState::BlockedIpc { .. })
            )
    };
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

/// `Sleep-loop test`: `sleep_ticks(2000)` with a stale wake at +5 that first
/// removes the sleep's TIMEOUT_QUEUE entry (`Command::Stale` with
/// `clear_timeout`: a stale reply that clears the sleeper's entry and wakes
/// it), so only the sleep's re-insert of its entry can end it on time.
///
/// A backstop deadline (`arm_deadline`), 2000 ticks after the sleep's, is
/// published first: an independent wake source, so a sleep whose entry was
/// lost returns instead of hanging. Pass: the sleep returned with the
/// backstop not fired (`NOTIFY_DEADLINES[me]` still holds it), so the sleep's
/// own re-inserted entry ended it; at or past its deadline; the helper
/// removed an entry; and no TIMEOUT_QUEUE entry is left. Logs
/// `Sleep-loop test: as expected`, or `Sleep-loop test: entry lost` (the
/// backstop fired first), `Sleep-loop test: early` (returned before its
/// deadline) or a line naming the helper's or the leftover's failure. The
/// backstop is cleared afterwards; one that fired has already woken the
/// sleep.
pub(super) fn sleep_loop_test(me: ThreadId) {
    let failure = sleep_case(me);
    wake_helper::reclaim();
    match failure {
        None => crate::kinfo!(Ipc, "Sleep-loop test: as expected"),
        Some(what) => crate::kwarn!(Ipc, "Sleep-loop test: {}", what),
    }
}

/// The Sleep-loop case: the failure, if any.
fn sleep_case(me: ThreadId) -> Option<&'static str> {
    // The sleep's own deadline is at or after this one.
    let deadline = now().saturating_add(LOOP_TICKS);
    let backstop = deadline.saturating_add(BACKSTOP_TICKS);
    if arm_deadline(me, Some(backstop)) == DeadlineArm::Expired {
        return Some("backstop expired");
    }
    let stale = Command::Stale {
        target: me,
        clear_timeout: true,
    };
    if !wake_helper::post(stale, DUE_TICKS) {
        clear_deadline(me);
        return Some("helper busy");
    }
    sleep_ticks(LOOP_TICKS);
    let returned = now();
    let backstop_fired = deadline_of(me) != backstop;
    clear_deadline(me);
    let helper = wake_helper::finish();
    if backstop_fired {
        return Some("entry lost");
    }
    if returned < deadline {
        return Some("early");
    }
    wake_helper::stale_failure(helper, true)
        .or_else(|| has_timeout_entry(me).then_some("entry left"))
}
