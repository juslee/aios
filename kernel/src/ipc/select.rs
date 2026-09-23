//! IpcSelect: multi-wait on channels and notifications.
//!
//! A thread can wait on up to MAX_SELECT_ENTRIES sources (channels or
//! notifications). If any source is ready, returns immediately. Otherwise
//! blocks with BlockedSelect state until any source fires.
//! Per ipc.md §5.

use crate::sched;
use crate::task::{ThreadId, ThreadState, MAX_THREADS};
use shared::{SelectEntry, SelectKind, MAX_SELECT_ENTRIES};
use spin::Mutex;

/// Per-thread select registration: what sources a BlockedSelect thread is
/// waiting on, and which one fired.
pub(super) struct SelectWaiter {
    #[allow(dead_code)]
    pub(super) tid: ThreadId,
    pub(super) entries: [Option<SelectEntry>; MAX_SELECT_ENTRIES],
    pub(super) entry_count: usize,
    /// Set by the waker to indicate which entry became ready.
    pub(super) ready_index: Option<usize>,
    /// For notification wakes: the matched bits.
    pub(super) ready_bits: u64,
}

// ---------------------------------------------------------------------------
// Global table
// ---------------------------------------------------------------------------

/// Per-thread select waiters. Lock ordering: after NOTIFICATION_TABLE,
/// after CHANNEL_TABLE (per deadlock-prevention §3).
pub(super) static SELECT_WAITERS: Mutex<[Option<SelectWaiter>; MAX_THREADS]> =
    Mutex::new([const { None }; MAX_THREADS]);

// ---------------------------------------------------------------------------
// API
// ---------------------------------------------------------------------------

/// Perform IpcSelect: wait on multiple sources, return the index of the
/// first ready source. `entries` is a slice of SelectEntry.
///
/// Every channel entry requires `Capability::ChannelAccess(id)`, the same
/// check `ipc_recv` makes. The whole set is validated before the thread is
/// registered as a waiter on any source, so a rejected call leaves no
/// partial registration: `EINVAL` if any id is out of range, otherwise
/// `EPERM` if the caller lacks access to any channel in the set.
///
/// Returns `(ready_index, matched_bits)` on success.
/// `matched_bits` is non-zero only for notification entries.
pub fn ipc_select(entries: &[SelectEntry], timeout_ticks: u64) -> Result<(usize, u64), i64> {
    use crate::arch::aarch64::timer::TICK_COUNT;
    use crate::syscall::IpcError;

    let my_tid = crate::ipc::current_thread_id().ok_or(IpcError::Einval as i64)?;

    if entries.is_empty() || entries.len() > MAX_SELECT_ENTRIES {
        return Err(IpcError::Einval as i64);
    }

    // Validate all entry IDs are within bounds to prevent kernel panics.
    for entry in entries {
        match entry.kind {
            SelectKind::Channel(ch_id) => {
                if ch_id.index().is_none() {
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
    // (fail-closed). This runs before the scan and the registration below
    // take CHANNEL_TABLE or SELECT_WAITERS. check_channel_access takes
    // PROCESS_TABLE, which ranks above both, so neither may be held here.
    check_channel_entries(my_tid, entries)?;

    // --- Non-blocking scan: check each entry ---
    if let Some((idx, bits)) = scan_entries(entries) {
        return Ok((idx, bits));
    }

    // --- Blocking path ---
    let deadline = if timeout_ticks == u64::MAX {
        u64::MAX
    } else {
        TICK_COUNT
            .load(core::sync::atomic::Ordering::Relaxed)
            .saturating_add(timeout_ticks)
    };

    // Register as select waiter.
    {
        let mut waiters = SELECT_WAITERS.lock();
        let mut sw = SelectWaiter {
            tid: my_tid,
            entries: [None; MAX_SELECT_ENTRIES],
            entry_count: entries.len(),
            ready_index: None,
            ready_bits: 0,
        };
        for (i, e) in entries.iter().enumerate() {
            sw.entries[i] = Some(*e);
        }
        waiters[my_tid.0 as usize] = Some(sw);
    }

    // Register on each source's waiter list.
    register_on_sources(my_tid, entries);

    // Store deadline for timeout checking.
    super::notify::set_select_deadline(my_tid, deadline);

    // Block.
    sched::block_current(ThreadState::BlockedSelect);

    // Woken up — retrieve result.
    let (ready_index, ready_bits) = {
        let mut waiters = SELECT_WAITERS.lock();
        let result = if let Some(sw) = &waiters[my_tid.0 as usize] {
            (sw.ready_index, sw.ready_bits)
        } else {
            (None, 0)
        };
        // Clean up our registration.
        waiters[my_tid.0 as usize] = None;
        result
    };

    // Clean up from all source waiter lists.
    unregister_from_sources(my_tid, entries);

    match ready_index {
        Some(idx) => Ok((idx, ready_bits)),
        None => Err(IpcError::Etimedout as i64),
    }
}

// ---------------------------------------------------------------------------
// Capability check
// ---------------------------------------------------------------------------

/// Check that the process owning `tid` holds `ChannelAccess` for every
/// channel entry in `entries`. Returns the first failure.
///
/// Each id goes through `cap::check_channel_access`, the path the other IPC
/// entry points use, so a denial counts toward `ipc_cap_denied` and logs the
/// same warning. Notification entries are not capability-checked, matching
/// `notify::notification_wait`. A set without channel entries skips the
/// process lookup.
fn check_channel_entries(tid: ThreadId, entries: &[SelectEntry]) -> Result<(), i64> {
    use crate::syscall::IpcError;

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
// Non-blocking scan
// ---------------------------------------------------------------------------

/// Scan all entries for a ready source. Returns `(index, matched_bits)`.
fn scan_entries(entries: &[SelectEntry]) -> Option<(usize, u64)> {
    for (i, entry) in entries.iter().enumerate() {
        match entry.kind {
            SelectKind::Channel(ch_id) => {
                // Check if the channel has a pending message.
                let mut table = super::CHANNEL_TABLE.lock();
                if let Ok(ch) = super::channel_mut(&mut table, ch_id) {
                    if ch.ring.len > 0 {
                        return Some((i, 0));
                    }
                }
                // Lock dropped here.
            }
            SelectKind::Notification(nid, mask) => {
                // Check if notification has matching bits.
                let table = super::notify::NOTIFICATION_TABLE.lock();
                if let Some(notif) = &table[nid.0 as usize] {
                    let current = notif.word.load(core::sync::atomic::Ordering::Acquire);
                    let matched = current & mask;
                    if matched != 0 {
                        // Clear the matched bits.
                        notif
                            .word
                            .fetch_and(!matched, core::sync::atomic::Ordering::AcqRel);
                        return Some((i, matched));
                    }
                }
                // Lock dropped here.
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Source registration
// ---------------------------------------------------------------------------

/// Register the select-waiting thread on each source's waiter list.
fn register_on_sources(tid: ThreadId, entries: &[SelectEntry]) {
    for entry in entries {
        match entry.kind {
            SelectKind::Channel(ch_id) => {
                // For channels, set waiting_receiver if not already set.
                // The channel's ipc_call/ipc_send code checks waiting_receiver.
                let mut table = super::CHANNEL_TABLE.lock();
                if let Ok(ch) = super::channel_mut(&mut table, ch_id) {
                    if ch.waiting_receiver.is_none() {
                        ch.waiting_receiver = Some(tid);
                    }
                }
            }
            SelectKind::Notification(nid, mask) => {
                let mut table = super::notify::NOTIFICATION_TABLE.lock();
                if let Some(notif) = &mut table[nid.0 as usize] {
                    // Add to notification's waiter list.
                    for slot in notif.waiters.iter_mut() {
                        if slot.is_none() {
                            *slot = Some(super::notify::waiter_new(tid, mask));
                            break;
                        }
                    }
                }
            }
        }
    }
}

/// Remove the select-waiting thread from each source's waiter list.
fn unregister_from_sources(tid: ThreadId, entries: &[SelectEntry]) {
    for entry in entries {
        match entry.kind {
            SelectKind::Channel(ch_id) => {
                let mut table = super::CHANNEL_TABLE.lock();
                if let Ok(ch) = super::channel_mut(&mut table, ch_id) {
                    if ch.waiting_receiver == Some(tid) {
                        ch.waiting_receiver = None;
                    }
                }
            }
            SelectKind::Notification(nid, _mask) => {
                let mut table = super::notify::NOTIFICATION_TABLE.lock();
                if let Some(notif) = &mut table[nid.0 as usize] {
                    for slot in notif.waiters.iter_mut() {
                        if let Some(w) = slot {
                            if w.tid == tid {
                                *slot = None;
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Wake helpers (called from signal/send paths)
// ---------------------------------------------------------------------------

/// Check if a thread is select-blocked and wake it via the select path.
/// Called from ipc_call/ipc_send when a message is enqueued for a receiver,
/// and from notification_signal when bits match.
///
/// Returns true if the thread was select-woken (caller should NOT do
/// separate unblock).
pub fn try_wake_select(tid: ThreadId, source_kind: SelectKind, bits: u64) -> bool {
    // Check thread state first (cheap — avoids SELECT_WAITERS lock if not select-blocked).
    let is_select = {
        let table = crate::task::THREAD_TABLE.lock();
        if let Some(thread) = &table[tid.0 as usize] {
            matches!(thread.sched.state, ThreadState::BlockedSelect)
        } else {
            false
        }
    };

    if !is_select {
        return false;
    }

    let mut waiters = SELECT_WAITERS.lock();
    let sw = match &mut waiters[tid.0 as usize] {
        Some(sw) => sw,
        None => return false,
    };

    // Find the matching entry index.
    for i in 0..sw.entry_count {
        if let Some(entry) = &sw.entries[i] {
            let matches = match (&entry.kind, &source_kind) {
                (SelectKind::Channel(a), SelectKind::Channel(b)) => a.0 == b.0,
                (SelectKind::Notification(a, _), SelectKind::Notification(b, _)) => a.0 == b.0,
                _ => false,
            };
            if matches {
                sw.ready_index = Some(i);
                sw.ready_bits = bits;
                drop(waiters);
                sched::unblock(tid);
                return true;
            }
        }
    }

    false
}

/// Set the select-ready metadata for a thread without unblocking it.
/// Used by ipc_call's direct-switch path where unblock happens separately.
/// No-op if the thread is not select-blocked.
pub fn set_select_ready(tid: ThreadId, source_kind: SelectKind, bits: u64) {
    let mut waiters = SELECT_WAITERS.lock();
    let sw = match &mut waiters[tid.0 as usize] {
        Some(sw) => sw,
        None => return,
    };

    for i in 0..sw.entry_count {
        if let Some(entry) = &sw.entries[i] {
            let matches = match (&entry.kind, &source_kind) {
                (SelectKind::Channel(a), SelectKind::Channel(b)) => a.0 == b.0,
                (SelectKind::Notification(a, _), SelectKind::Notification(b, _)) => a.0 == b.0,
                _ => false,
            };
            if matches {
                sw.ready_index = Some(i);
                sw.ready_bits = bits;
                return;
            }
        }
    }
}
