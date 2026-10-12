//! Dead self-test (capability-lifetime ADR §7), run from the ipc-timeout
//! thread: a thread that ends itself with `sched::exit_current()` is Dead
//! for good. No queue holds it, a deliberate `unblock` does not revive it,
//! and it stays Dead across later switches.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::sched::{self, ThreadProbe};
use crate::task::{CpuSet, SchedulerClass, Thread, ThreadId, ThreadState};

use super::TEST_PID;

/// The helper thread's table index, or `NO_HELPER` until [`init`] creates
/// it.
static HELPER_TID: AtomicU32 = AtomicU32::new(NO_HELPER);

/// [`HELPER_TID`] before the helper exists.
const NO_HELPER: u32 = u32::MAX;

/// The CPU the helper and the ipc-timeout thread are pinned to.
const PINNED_CPU: usize = 2;

/// Yields the test waits, at most, for the helper to have run and exited.
/// The Normal queue is FIFO, so each yield lets every Normal thread queued
/// ahead on CPU 2 run once before the poll runs again.
const MAX_POLLS: usize = 32;

/// Create the helper thread: process 1, pinned to CPU 2 and queued there,
/// so the ipc-timeout thread probes it from the CPU it runs on. Called from
/// `tests::init` before `enter_scheduler`.
pub(super) fn init() {
    let stack_phys = sched::alloc_kernel_stack();
    let stack_virt_top = sched::phys_to_virt(stack_phys) + sched::STACK_SIZE;

    let mut thread = Thread::new_kernel(
        ThreadId(0x203),
        b"dead-helper\0\0\0\0\0",
        dead_helper_entry as *const () as usize,
        stack_phys,
    );
    thread.sched.class = SchedulerClass::Normal;
    thread.sched.effective_class = SchedulerClass::Normal;
    thread.sched.affinity = CpuSet::single(PINNED_CPU);
    thread.context.sp = stack_virt_top as u64;
    thread.owner_pid = Some(TEST_PID);

    let idx = sched::allocate_thread(thread).expect("thread table full for dead helper");
    HELPER_TID.store(idx as u32, Ordering::Release);
    sched::enqueue_on_cpu(PINNED_CPU, ThreadId(idx as u32), SchedulerClass::Normal);
}

/// The helper: ends itself the first time it runs. IRQs stay masked as
/// `enter_scheduler` or `schedule` left them; `exit_current` masks anyway.
fn dead_helper_entry() -> ! {
    sched::exit_current()
}

/// Run the test in the ipc-timeout thread.
///
/// 1. Poll `thread_probe(helper, false)`, one yield between polls, until the
///    helper reads Dead, at most [`MAX_POLLS`] yields.
/// 2. `thread_probe(helper, true)` unblocks it (`WakeSource::SelfTest`,
///    counted under `ubdead`) and expects it still Dead and in no run queue.
/// 3. Yield twice, so CPU 2 schedules again, and expect it still Dead.
///
/// Logs `Dead test: terminal as expected`, or a warning naming the step
/// that failed and what the probe read.
pub(super) fn dead_thread_test() {
    let raw = HELPER_TID.load(Ordering::Acquire);
    if raw == NO_HELPER {
        crate::kwarn!(Ipc, "Dead test: no helper thread");
        return;
    }
    let helper = ThreadId(raw);

    let mut polled = sched::thread_probe(helper, false);
    let mut yields = 0;
    while !is_dead(&polled) && yields < MAX_POLLS {
        sched::thread_yield();
        yields += 1;
        polled = sched::thread_probe(helper, false);
    }
    if !is_dead(&polled) {
        crate::kwarn!(
            Ipc,
            "Dead test: not Dead after {} yields: {:?}",
            yields,
            polled.state
        );
        return;
    }

    let woken = sched::thread_probe(helper, true);
    if !is_dead(&woken) || woken.queued {
        crate::kwarn!(
            Ipc,
            "Dead test: after unblock: {:?} queued={}",
            woken.state,
            woken.queued
        );
        return;
    }

    sched::thread_yield();
    sched::thread_yield();
    let after = sched::thread_probe(helper, false);
    if !is_dead(&after) {
        crate::kwarn!(Ipc, "Dead test: left Dead: {:?}", after.state);
        return;
    }

    crate::kinfo!(Ipc, "Dead test: terminal as expected");
}

/// Whether the probe read the thread as Dead.
fn is_dead(probe: &ThreadProbe) -> bool {
    probe.state == Some(ThreadState::Dead)
}
