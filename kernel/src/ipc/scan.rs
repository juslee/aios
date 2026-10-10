//! Heartbeat scan B: who can still wake a blocked thread (crash-fix step 1b).
//!
//! Read-only accessors for the no-waker scan in `observability::tripwire`.
//! Each waker table is try-locked alone, its references are handed to the
//! tripwire's scan-B accumulators, and the lock is dropped before the next
//! table is tried:
//!
//! - CHANNEL_TABLE: each channel's `waiting_receiver` and `pending_caller`
//!   (`chan_ref`);
//! - TIMEOUT_QUEUE: each registered IPC timeout or sleep (`timeout`);
//! - NOTIFY_DEADLINES: each notification or select deadline (`ndl`);
//! - NOTIFICATION_TABLE: each notification's waiters (`notif_ref`), only when
//!   scan A saw a thread blocked on a notification or in a select.
//!
//! SELECT_WAITERS is not a waker (a signal or message reaches a select waiter
//! through the notification's waiter list or the channel's receiver slot), so
//! it is not read.
//!
//! CPU 0's timer IRQ only. Every lock is a try-lock that counts nothing: the
//! IRQ-class tables use `try_lock_quiet`, and CHANNEL_TABLE, a `spin::Mutex`
//! that thread code holds with IRQs on, uses `spin::Mutex::try_lock`. Taking
//! CHANNEL_TABLE from the tick handler is the documented exception to the
//! IRQ-class rule (deadlock-prevention.md §3.3): a try-lock cannot wait on a
//! holder that CPU 0 itself interrupted. A busy table ends the scan. The
//! longest single hold goes to the tripwire's `scanhold2`.

#![deny(clippy::arithmetic_side_effects, clippy::indexing_slicing)]

use crate::arch::aarch64::timer;
use crate::observability::tripwire::{self, ScanWaker};
use crate::task::MAX_THREADS;

/// Phase 2 of the heartbeat scans: record every waker reference in the four
/// tables. Returns `false` as soon as a table is busy (the scan is then
/// incomplete and the tripwire counts `skipb`); `need_notif` is whether
/// NOTIFICATION_TABLE must be read.
#[inline(never)]
pub(crate) fn scan_wakers(need_notif: bool) -> bool {
    scan_channels() && scan_timeouts() && scan_deadlines() && (!need_notif || scan_notifications())
}

/// CHANNEL_TABLE: pending callers and waiting receivers.
#[inline(never)]
fn scan_channels() -> bool {
    let Some(table) = super::CHANNEL_TABLE.try_lock() else {
        return false;
    };
    let start = timer::read_counter();
    for channel in table.iter().flatten() {
        if let Some(tid) = channel.waiting_receiver {
            tripwire::scan_note_waker(ScanWaker::ChanRef, tid.0);
        }
        if let Some(tid) = channel.pending_caller {
            tripwire::scan_note_waker(ScanWaker::ChanRef, tid.0);
        }
    }
    drop(table);
    tripwire::note_scan_hold2(timer::read_counter().wrapping_sub(start));
    true
}

/// TIMEOUT_QUEUE: registered IPC timeouts and sleeps.
#[inline(never)]
fn scan_timeouts() -> bool {
    let Some(queue) = super::timeout::TIMEOUT_QUEUE.try_lock_quiet() else {
        return false;
    };
    let start = timer::read_counter();
    for entry in queue.iter().flatten() {
        tripwire::scan_note_waker(ScanWaker::Timeout, entry.tid.0);
    }
    drop(queue);
    tripwire::note_scan_hold2(timer::read_counter().wrapping_sub(start));
    true
}

/// NOTIFY_DEADLINES: notification and select deadlines (`u64::MAX` = none).
#[inline(never)]
fn scan_deadlines() -> bool {
    let Some(deadlines) = super::notify::NOTIFY_DEADLINES.try_lock_quiet() else {
        return false;
    };
    let start = timer::read_counter();
    // A range, not `enumerate()`, whose counter carries an overflow-check
    // panic in the dev build.
    for slot in 0..MAX_THREADS {
        if deadlines
            .get(slot)
            .is_some_and(|deadline| *deadline != u64::MAX)
        {
            tripwire::scan_note_waker(ScanWaker::Deadline, slot as u32);
        }
    }
    drop(deadlines);
    tripwire::note_scan_hold2(timer::read_counter().wrapping_sub(start));
    true
}

/// NOTIFICATION_TABLE: every notification's registered waiters.
#[inline(never)]
fn scan_notifications() -> bool {
    let Some(table) = super::notify::NOTIFICATION_TABLE.try_lock_quiet() else {
        return false;
    };
    let start = timer::read_counter();
    for notification in table.iter().flatten() {
        for waiter in notification.waiters.iter().flatten() {
            tripwire::scan_note_waker(ScanWaker::NotifRef, waiter.tid.0);
        }
    }
    drop(table);
    tripwire::note_scan_hold2(timer::read_counter().wrapping_sub(start));
    true
}
