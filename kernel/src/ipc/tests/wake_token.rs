//! Wake-token self-test (capability-lifetime ADR §8), run from the
//! ipc-timeout thread before the Timeout test: a wake that lands between a
//! thread's arm and its block is kept as a token, and the block consumes it
//! and returns at once instead of sleeping.
//!
//! Deterministic and single-threaded: the thread wakes itself. Its tid comes
//! from the ipc-timeout thread, which is pinned to CPU 2, so the tid read
//! cannot name another CPU's thread (ADR §8's own-tid rule). The self-wakes
//! count under `WakeSource::SelfTest`, so they never inflate a real source's
//! outcome counts.

use core::sync::atomic::Ordering;

use crate::arch::aarch64::daif;
use crate::arch::aarch64::timer::TICK_COUNT;
use crate::sched::{self, wake};
use crate::syscall::IpcError;
use crate::task::{ThreadId, ThreadState};
use shared::tripwire::WakeSource;
use shared::{deadline_after, WaitState};

use super::super::timeout::{get_wakeup_error, TimeoutEntry, TIMEOUT_QUEUE};

/// Ticks until the safety entry fires. Long enough that crowding on CPU 2
/// between the self-wake and `block_current` cannot let it fire while a
/// consumed token is still on its way; a consumed token returns at once, so
/// the entry costs these ticks only when the token is lost.
const SAFETY_TICKS: u64 = 2000;

/// Run the test in the ipc-timeout thread, whose tid is `me`.
///
/// 1. `unblock(me)` without arming leaves the word Idle (a stale wake).
/// 2. `prepare_block(me)`, then `unblock(me)`: the word is Woken.
///    `cancel_block(me)` returns `true` and leaves it Idle.
/// 3. `prepare_block(me)`, `unblock(me)`, then publish a 2000-tick safety
///    entry in TIMEOUT_QUEUE (ETIMEDOUT) and `block_current(BlockedIpc)`.
/// 4. Clean up under TIMEOUT_QUEUE: remove the entry if it is still there,
///    and record whether it was. If it is gone it fired, and its
///    `wake_with_error` set this thread's ETIMEDOUT code, which is taken back
///    with `get_wakeup_error`.
///
/// The verdict is outcome-only: the token was consumed if the safety entry
/// was still queued when step 4 removed it and the word is Idle. A consumed
/// token returns from `block_current` without calling `schedule()`; a lost
/// one can come back only through the entry's own wake, which removes the
/// entry. No elapsed tick is read.
///
/// Every TIMEOUT_QUEUE and WAKEUP_ERRORS hold runs inside
/// `daif::with_irqs_masked`: both are IRQ-class locks that the IRQ path
/// takes, and a tick landing inside an unmasked hold would make this
/// thread's own IRQ re-enter it. The two are never held together.
///
/// Logs `Wake-token test: early wake as expected`, `Wake-token test: token
/// lost` when the safety entry fired, or a warning naming the step that read
/// the wrong word.
pub(super) fn wake_token_test(me: ThreadId) {
    // 1. A wake with no arm is stale: the word stays Idle.
    sched::unblock(me, WakeSource::SelfTest);
    let word = wake::wait_state(me);
    if word != WaitState::Idle {
        crate::kwarn!(Ipc, "Wake-token test: stale wake left {:?}", word);
        wake::cancel_block(me);
        return;
    }

    // 2. A wake of an armed thread leaves the token, and cancel takes it.
    wake::prepare_block(me);
    sched::unblock(me, WakeSource::SelfTest);
    let word = wake::wait_state(me);
    let had_token = wake::cancel_block(me);
    let after = wake::wait_state(me);
    if word != WaitState::Woken || !had_token || after != WaitState::Idle {
        crate::kwarn!(
            Ipc,
            "Wake-token test: cancel read {:?} {} {:?}",
            word,
            had_token,
            after
        );
        return;
    }

    // 3. Arm, wake, publish the safety entry after the arm (rule 1), block.
    wake::prepare_block(me);
    sched::unblock(me, WakeSource::SelfTest);
    daif::with_irqs_masked(|| {
        let now = TICK_COUNT.load(Ordering::Relaxed);
        let deadline = deadline_after(now, SAFETY_TICKS).unwrap_or(u64::MAX);
        TIMEOUT_QUEUE.lock()[me.0 as usize] = Some(TimeoutEntry {
            tid: me,
            wake_at_tick: deadline,
            error_code: IpcError::Etimedout as i64,
        });
    });
    sched::block_current(ThreadState::BlockedIpc { channel: u64::MAX });

    // 4. Clean up on either path. check_timeouts calls wake_with_error only
    // after it drops TIMEOUT_QUEUE, so after a fired entry the code can land
    // after this take, as a late ETIMEDOUT that the thread's next ipc_call or
    // ipc_recv exit discards.
    let still_queued =
        daif::with_irqs_masked(|| TIMEOUT_QUEUE.lock()[me.0 as usize].take().is_some());
    if !still_queued {
        daif::with_irqs_masked(|| get_wakeup_error(me));
    }
    let word = wake::wait_state(me);

    if still_queued && word == WaitState::Idle {
        crate::kinfo!(Ipc, "Wake-token test: early wake as expected");
    } else if !still_queued {
        crate::kwarn!(Ipc, "Wake-token test: token lost");
    } else {
        crate::kwarn!(Ipc, "Wake-token test: block left {:?}", word);
    }
}
