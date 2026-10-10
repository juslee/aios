//! Timeout-argument self-test (#217): IpcCall and IpcRecv through
//! `syscall_dispatch` with timeouts near `u64::MAX`, run by the PI pair
//! (`pi_caller_entry` / `pi_server_entry`, process 2, both started on CPU 3) after its three
//! round trips.

use shared::{ChannelId, Syscall, USER_VA_MIN};

use super::syscall_args::svc;
use crate::ipc::ipc_reply;

/// The timeouts each side passes, in order: `u64::MAX - 1`, which overflowed
/// `TICK_COUNT + timeout` once the tick count reached 2, then `u64::MAX`,
/// which overflowed IpcCall's add at any tick count above 0. Both now wait
/// without a deadline that can pass.
const TIMEOUTS: [u64; 2] = [u64::MAX - 1, u64::MAX];

/// Caller side: one IpcCall through `syscall_dispatch` per entry of
/// [`TIMEOUTS`], with the timeout in x5, on the PI channel `ch`.
///
/// Both buffers are empty at `USER_VA_MIN`, the only user buffers the
/// self-tests can pass while TTBR0 is not a process's own map
/// (`syscall/user.rs`), so a call that the server answered returns 0 and any
/// failure returns a negative errno. Before #217, a dev build panicked with
/// an add overflow in `ipc_call`, and a release build wrapped the deadline
/// into the past and returned ETIMEDOUT on the next tick. The server answers
/// both calls. If it fails to, neither call has a deadline that can pass, so
/// the call blocks this thread for good, and the missing success line and
/// the server's warning show it.
///
/// Logs one line: the success text, or the two results.
pub(super) fn call_side(ch: ChannelId) {
    let user = USER_VA_MIN as u64;
    let results = TIMEOUTS.map(|t| svc(Syscall::IpcCall, &[u64::from(ch.0), user, 0, user, 0, t]));
    // A log message keeps at most 96 bytes (two ring entries); longer text
    // is cut and marked with `~`, so keep these lines short.
    if results == [0, 0] {
        crate::kinfo!(Ipc, "Timeout-arg test: MAX-1/MAX calls replied");
    } else {
        crate::kwarn!(
            Ipc,
            "Timeout-arg test: call results {} {}",
            results[0],
            results[1]
        );
    }
}

/// Server side: for each entry of [`TIMEOUTS`], one IpcRecv through
/// `syscall_dispatch` with the timeout in x3, into an empty buffer at
/// `USER_VA_MIN`, then a reply to the call it received.
///
/// The server reaches its receive straight after its last in-kernel reply,
/// while the caller still logs that reply and yields, so the receive usually
/// blocks and registers its deadline, the path that overflowed before #217
/// for `u64::MAX - 1`. When the request is already queued, the receive
/// returns it without computing a deadline; the check passes either way.
/// Warns once per failed receive or reply; logs nothing on success, which the
/// caller reports.
pub(super) fn server_side(ch: ChannelId) {
    let user = USER_VA_MIN as u64;
    for (i, t) in TIMEOUTS.into_iter().enumerate() {
        let received = svc(Syscall::IpcRecv, &[u64::from(ch.0), user, 0, t]);
        if received != 0 {
            crate::kwarn!(Ipc, "Timeout-arg test: recv #{} got {}", i, received);
            continue;
        }
        let replied = ipc_reply(ch, b"REPLY:");
        if replied < 0 {
            crate::kwarn!(Ipc, "Timeout-arg test: reply got {}", replied);
        }
    }
}
