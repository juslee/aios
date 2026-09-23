//! IpcSelect capability self-test (#179), run from the ipc-timeout thread.

use crate::ipc::channel::{ipc_recv, ipc_send};
use crate::ipc::select::{ipc_select, SELECT_WAITERS};
use crate::ipc::{channel_mut, CHANNEL_TABLE};
use crate::syscall::IpcError;
use crate::task::ThreadId;
use shared::{Capability, ChannelId, RawMessage, SelectEntry, SelectKind, MAX_CHANNELS};

use super::channel_create_unchecked;

/// Short timeout for every select below. A regression that lets a select
/// block fails its check with ETIMEDOUT (or at worst stalls this thread
/// before the success line prints).
const SELECT_TIMEOUT_TICKS: u64 = 10;

/// IpcSelect capability test: select needs ChannelAccess for every channel
/// in the set, like ipc_recv, and must reject a set with any inaccessible
/// channel before it observes or registers on any source.
///
/// Runs in the ipc-timeout thread (process 1). The three channels come from
/// channel_create_unchecked; creating a channel grants no ChannelAccess, so
/// process 1 is granted ChannelAccess to `owned_a` and `owned_b` only. The
/// channels stay allocated for the rest of the boot, like the other test
/// channels, so the grants never point at a reused channel slot.
///
/// Each case pins one ordering regression:
/// - `denied` first holds a queued message, so a check that ran after the
///   non-blocking scan (or checked only the last entry) would report it
///   ready instead of returning EPERM.
/// - With `denied` empty again, the mixed set puts the accessible channel
///   first, so a check made while registering would leave a waiter behind.
///
/// The three EPERM cases each log one expected `denied ChannelAccess`
/// warning from the capability check.
pub(super) fn select_cap_test(my_tid: ThreadId) {
    let pid = match crate::cap::process_of_thread(my_tid) {
        Some(p) => p,
        None => {
            crate::kwarn!(Ipc, "Select-cap test: no owning process");
            return;
        }
    };

    let owned_a = channel_create_unchecked(my_tid);
    let owned_b = channel_create_unchecked(my_tid);
    let denied = channel_create_unchecked(my_tid);
    for ch in [owned_a, owned_b] {
        if crate::cap::grant_to_process(pid, Capability::ChannelAccess(ch), false).is_err() {
            crate::kwarn!(Ipc, "Select-cap test: grant failed");
            return;
        }
    }

    let chan = |id: ChannelId| SelectEntry {
        kind: SelectKind::Channel(id),
    };
    let eperm = Err(IpcError::Eperm as i64);
    let einval = Err(IpcError::Einval as i64);
    let mut passed = true;

    // Make `denied` ready. Process 1 cannot ipc_send to it, so the message
    // goes straight onto the ring.
    let queued = {
        let mut table = CHANNEL_TABLE.lock();
        channel_mut(&mut table, denied).is_ok_and(|ch| {
            ch.ring.push(RawMessage {
                sender: my_tid,
                ..RawMessage::EMPTY
            })
        })
    };
    if !queued {
        passed = false;
        crate::kwarn!(Ipc, "Select-cap test: queue failed");
    }

    // A ready inaccessible channel, alone and ahead of an accessible one.
    let denied_only = ipc_select(&[chan(denied)], SELECT_TIMEOUT_TICKS);
    let denied_first = ipc_select(&[chan(denied), chan(owned_a)], SELECT_TIMEOUT_TICKS);
    if denied_only != eperm || denied_first != eperm {
        passed = false;
        crate::kwarn!(
            Ipc,
            "Select-cap: denied -> {:?} {:?}",
            denied_only,
            denied_first
        );
    }

    // Drop the queued message so `denied` is empty for the rest of the test.
    {
        let mut table = CHANNEL_TABLE.lock();
        if let Ok(ch) = channel_mut(&mut table, denied) {
            ch.ring.pop();
        }
    }

    // A mixed set with nothing ready. The accessible channel comes first, so
    // a check made while registering would already have claimed its
    // waiting_receiver slot (or the SELECT_WAITERS entry) before failing.
    let mixed = ipc_select(&[chan(owned_a), chan(denied)], SELECT_TIMEOUT_TICKS);
    let on_channel = {
        let mut table = CHANNEL_TABLE.lock();
        channel_mut(&mut table, owned_a).is_ok_and(|ch| ch.waiting_receiver.is_some())
    };
    let in_waiters = SELECT_WAITERS
        .lock()
        .get(my_tid.0 as usize)
        .is_some_and(Option::is_some);
    let waiter_left = on_channel || in_waiters;
    if mixed != eperm || waiter_left {
        passed = false;
        crate::kwarn!(
            Ipc,
            "Select-cap: mixed -> {:?} waiter={}",
            mixed,
            waiter_left
        );
    }

    // A set the caller fully owns: a message on owned_b makes entry 1 ready.
    // Drain it afterwards so the channel is left empty.
    let send_result = ipc_send(owned_b, b"SELECT");
    let owned = ipc_select(&[chan(owned_a), chan(owned_b)], SELECT_TIMEOUT_TICKS);
    let mut buf = [0u8; 64];
    let drained = ipc_recv(owned_b, &mut buf, 0).is_ok();
    if send_result != 0 || owned != Ok((1, 0)) || !drained {
        passed = false;
        crate::kwarn!(
            Ipc,
            "Select-cap: owned -> {} {:?} {}",
            send_result,
            owned,
            drained
        );
    }

    // An out-of-range id is EINVAL, whether the rest of the set is
    // accessible or not: the range check runs before any capability check.
    let bad_id = ChannelId(MAX_CHANNELS as u32);
    let bad_owned = ipc_select(&[chan(owned_a), chan(bad_id)], SELECT_TIMEOUT_TICKS);
    let bad_denied = ipc_select(&[chan(denied), chan(bad_id)], SELECT_TIMEOUT_TICKS);
    if bad_owned != einval || bad_denied != einval {
        passed = false;
        crate::kwarn!(
            Ipc,
            "Select-cap: bad id -> {:?} {:?}",
            bad_owned,
            bad_denied
        );
    }

    if passed {
        crate::kinfo!(Ipc, "Select-cap test: EPERM/EINVAL as expected");
    }
}
