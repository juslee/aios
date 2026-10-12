//! IpcSelect: multi-wait on channels and notifications.
//!
//! A thread can wait on up to MAX_SELECT_ENTRIES sources (channels or
//! notifications). If any source is ready, returns immediately. Otherwise
//! blocks with BlockedSelect state until any source fires.
//! Per ipc.md §5.
//!
//! The sources are authoritative (capability-lifetime ADR §9): a select
//! registers on each source itself, as a channel's `waiting_receiver` and as
//! a `Select` waiter record on a notification, and the source's own waker
//! (`ipc_send`, `ipc_call`'s fallback, `notification_signal`) claims the
//! registration and calls `unblock`. The select then re-scans. It arms its
//! wake token before each pass, so a wake that lands between a registration
//! and the block is kept, and checks and registers each source in one
//! critical section, so no event falls between the check and the
//! registration.

use crate::arch::aarch64::daif;
use crate::arch::aarch64::timer::TICK_COUNT;
use crate::sched::{self, wake};
use crate::syscall::IpcError;
use crate::task::{ThreadId, ThreadState};
use shared::tripwire::{BadchanSite, Key};
use shared::{deadline_after, SelectEntry, SelectKind, MAX_SELECT_ENTRIES};

use super::{channel_mut, notify, timeout, DeadlineArm, CHANNEL_TABLE};

// ---------------------------------------------------------------------------
// API
// ---------------------------------------------------------------------------

/// Perform IpcSelect: wait on multiple sources, return the index of the
/// lowest-index ready source. `entries` is a slice of SelectEntry.
///
/// Every channel entry requires `Capability::ChannelAccess(id)`, the same
/// check `ipc_recv` makes. The whole set is validated before the
/// non-blocking scan and before the thread is registered on any source, so a
/// rejected call observes nothing and leaves no registration: `EINVAL` if
/// any id is out of range, otherwise `EPERM` if the caller lacks access to
/// any channel in the set (or, for a set with a channel entry, if the
/// calling thread has no owning process). `EPERM` also when the CPU has no
/// current thread.
///
/// Then the fast scan, which arms and publishes nothing, takes each entry in
/// index order under its own lock: a channel lookup error (EINVAL for a
/// malformed slot, EPIPE for a destroyed or stale id) or a Dead endpoint
/// (EPIPE) returns at once, a queued message is ready, and a notification
/// returns EINVAL for an empty slot or consumes the bits of its mask.
///
/// Otherwise the loop: each pass arms (`wake::prepare_block`), then checks
/// and registers each entry in index order, one critical section per source.
/// A channel fails as in the scan, is ready with a queued message, and is
/// otherwise registered as the channel's `waiting_receiver`, or fails EAGAIN
/// if another thread holds that slot. A notification fails EINVAL for an
/// empty slot, is ready when bits of its mask are set (consumed), and
/// otherwise keeps one `Select` record for this thread whose mask is the OR
/// of every entry naming it, or fails ENOMEM when its waiter list is full.
/// The first entry that is ready or fails ends the pass. Otherwise the pass
/// publishes the deadline (`timeout_ticks` from the call, by
/// `shared::deadline_after`; ETIMEDOUT if it has passed) and blocks.
///
/// Every exit of the loop removes the thread's registrations, clears its
/// deadline, takes and discards its `WAKEUP_ERRORS` code (destroy and exit
/// write EPIPE there for a `waiting_receiver`; the result comes from the
/// channel's state), and cancels the arm. The result comes from the sources'
/// state and the wait's own deadline, never from which wake arrived.
///
/// Returns `(ready_index, matched_bits)` on success.
/// `matched_bits` is non-zero only for notification entries.
pub fn ipc_select(entries: &[SelectEntry], timeout_ticks: u64) -> Result<(usize, u64), i64> {
    let me = sched::current_tid().ok_or(IpcError::Eperm as i64)?;

    if entries.is_empty() || entries.len() > MAX_SELECT_ENTRIES {
        return Err(IpcError::Einval as i64);
    }

    // Validate all entry IDs are within bounds to prevent kernel panics.
    for entry in entries {
        match entry.kind {
            SelectKind::Channel(ch_id) => {
                if ch_id.index().is_none() {
                    crate::observability::tripwire::bump(Key::Badchan, BadchanSite::Select.index());
                    return Err(IpcError::Einval as i64);
                }
            }
            SelectKind::Notification(nid, _) => {
                if nid.0 as usize >= shared::MAX_NOTIFICATIONS {
                    return Err(IpcError::Einval as i64);
                }
            }
        }
    }

    // Capability enforcement: ChannelAccess for every channel in the set
    // (fail-closed). It must run before the non-blocking scan, which reports
    // whether a channel has a pending message and consumes notification bits:
    // scanning first would leak traffic on channels the caller cannot access.
    // Lock order agrees: the scan and the registration below take
    // CHANNEL_TABLE or NOTIFICATION_TABLE, while the check takes THREAD_TABLE
    // (process_of_thread) and then PROCESS_TABLE (check_channel_access) one
    // at a time; both rank above both of them, so neither may be held here.
    check_channel_entries(me, entries)?;

    // --- Fast scan: no arm, no registration ---
    if let Some(ready) = fast_scan(entries)? {
        return Ok(ready);
    }

    // --- Loop: arm, check and register, then publish the deadline ---
    let deadline = deadline_after(
        TICK_COUNT.load(core::sync::atomic::Ordering::Relaxed),
        timeout_ticks,
    );
    let result = loop {
        // Arm before the per-source pass (ADR §8 rule 1).
        wake::prepare_block(me);
        if let Some(result) = register_pass(me, entries) {
            break result;
        }
        if notify::arm_deadline(me, deadline) == DeadlineArm::Expired {
            break Err(IpcError::Etimedout as i64);
        }
        sched::block_current(ThreadState::BlockedSelect);
    };

    // --- Exit: every loop exit armed in its pass ---
    unregister(me, entries);
    notify::clear_deadline(me);
    // Taken once per wait and discarded: it decides nothing (ADR §9).
    daif::with_irqs_masked(|| timeout::get_wakeup_error(me));
    wake::cancel_block(me);
    result
}

// ---------------------------------------------------------------------------
// Capability check
// ---------------------------------------------------------------------------

/// Check that the process owning `tid` holds `ChannelAccess` for every
/// channel entry in `entries`. Returns the first failure.
///
/// Each id goes through `cap::check_channel_access`, the path `ipc_call`,
/// `ipc_send`, `ipc_recv`, `ipc_cancel` and `channel_destroy` use, so a
/// denial counts toward `ipc_cap_denied` (with `kernel-metrics`) and logs the
/// same warning. Notification entries are not capability-checked, matching
/// `notify::notification_wait`. A set without channel entries skips the
/// process lookup.
fn check_channel_entries(tid: ThreadId, entries: &[SelectEntry]) -> Result<(), i64> {
    let mut channels = entries
        .iter()
        .filter_map(|entry| match entry.kind {
            SelectKind::Channel(ch_id) => Some(ch_id),
            SelectKind::Notification(..) => None,
        })
        .peekable();
    if channels.peek().is_none() {
        return Ok(());
    }

    let pid = crate::cap::process_of_thread(tid).ok_or(IpcError::Eperm as i64)?;
    channels.try_for_each(|ch_id| crate::cap::check_channel_access(pid, ch_id))
}

// ---------------------------------------------------------------------------
// Fast scan and the loop pass
// ---------------------------------------------------------------------------

/// The fast scan: each entry in index order under its own lock, publishing
/// nothing. Returns `(index, matched_bits)` for the first ready entry,
/// `None` if none is ready, or the first entry's failure.
fn fast_scan(entries: &[SelectEntry]) -> Result<Option<(usize, u64)>, i64> {
    for (i, entry) in entries.iter().enumerate() {
        match entry.kind {
            SelectKind::Channel(ch_id) => {
                let mut table = CHANNEL_TABLE.lock();
                let ch = channel_mut(&mut table, ch_id)?;
                if ch.has_dead_endpoint() {
                    return Err(IpcError::Epipe as i64);
                }
                if ch.ring.len > 0 {
                    return Ok(Some((i, 0)));
                }
            }
            SelectKind::Notification(nid, mask) => {
                if let Some(bits) = notify::poll(nid, mask)? {
                    return Ok(Some((i, bits)));
                }
            }
        }
    }
    Ok(None)
}

/// One loop pass for the armed thread `me`: check and register each entry in
/// index order, one critical section per source. `Some` with the pass's
/// result when an entry is ready or fails (the first one ends the pass),
/// `None` when every entry is registered.
fn register_pass(me: ThreadId, entries: &[SelectEntry]) -> Option<Result<(usize, u64), i64>> {
    for (i, entry) in entries.iter().enumerate() {
        match entry.kind {
            SelectKind::Channel(ch_id) => {
                let mut table = CHANNEL_TABLE.lock();
                let ch = match channel_mut(&mut table, ch_id) {
                    Ok(ch) => ch,
                    Err(code) => return Some(Err(code)),
                };
                if ch.has_dead_endpoint() {
                    return Some(Err(IpcError::Epipe as i64));
                }
                if ch.ring.len > 0 {
                    return Some(Ok((i, 0)));
                }
                match ch.waiting_receiver {
                    None => ch.waiting_receiver = Some(me),
                    Some(tid) if tid == me => {}
                    // Another receiver holds the slot (#186 item 2).
                    Some(_) => return Some(Err(IpcError::Eagain as i64)),
                }
            }
            SelectKind::Notification(nid, mask) => {
                // One record per notification, for every entry naming it.
                let record_mask = entries
                    .iter()
                    .filter_map(|e| match e.kind {
                        SelectKind::Notification(n, m) if n == nid => Some(m),
                        _ => None,
                    })
                    .fold(0, |acc, m| acc | m);
                match notify::select_register(nid, mask, record_mask, me) {
                    Ok(Some(bits)) => return Some(Ok((i, bits))),
                    Ok(None) => {}
                    Err(code) => return Some(Err(code)),
                }
            }
        }
    }
    None
}

/// Remove every registration of `me` on the sources of `entries`: under
/// CHANNEL_TABLE, a `waiting_receiver` that is `me`; under NOTIFICATION_TABLE
/// (IRQs masked), `me`'s `Select` records. A source a waker claimed holds no
/// registration of `me`, and a gone one holds none at all.
fn unregister(me: ThreadId, entries: &[SelectEntry]) {
    for entry in entries {
        match entry.kind {
            SelectKind::Channel(ch_id) => {
                let mut table = CHANNEL_TABLE.lock();
                if let Ok(ch) = channel_mut(&mut table, ch_id) {
                    if ch.waiting_receiver == Some(me) {
                        ch.waiting_receiver = None;
                    }
                }
            }
            SelectKind::Notification(nid, _) => notify::select_unregister(nid, me),
        }
    }
}
