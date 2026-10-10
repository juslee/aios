//! Tripwire self-test (crash-fix step 1b, task T-self): an end-to-end proof
//! of the PANIC-LOCK path. Built only with the `tripwire-selftest` feature,
//! which is off by default and in neither soak arm.
//!
//! One kernel thread, pinned to CPU 0 (the only CPU that takes timer IRQs
//! until #200 is fixed), yields for [`SETTLE_TICKS`], then takes
//! `THREAD_TABLE.lock()` with IRQs on and busy-waits for [`HOLD_TICKS`]
//! ticks while holding it. The first timer IRQ on CPU 0 that blocks on
//! THREAD_TABLE finds the lock word stamped with its own interrupted stream
//! and calls `reentry_panic`. Expected output, in order:
//!
//! - `PANIC: panicked at` one of the three blocking THREAD_TABLE sites on the
//!   tick path: `schedule()` (through `check_preemption`), `unblock()`
//!   (through `check_timeouts` → `wake_with_error`) or the balancer's
//!   affinity check in `try_load_balance`;
//! - on the next line, `lock re-entry: THREAD_TABLE on CPU 0 ctx=irq-exit`
//!   (or `ctx=irq`) `holder=kernel/src/sync/selftest.rs:<line>
//!   holder_irqs=on …`;
//! - `[panic] cpu=0 … irq_elr=0x…`, with `irq_elr` inside [`hold_loop`];
//! - `[tripwire] v=1 src=panic …` with `lktry` for THREAD_TABLE ≥ 1, from
//!   `sched::timer_tick`'s `try_lock`, which runs before all three sites.
//!
//! If no panic arrives within [`HOLD_TICKS`], the thread releases the lock,
//! prints a `[selftest]` failure line and parks.

use core::fmt::Write;
use core::sync::atomic::Ordering;

use crate::arch::aarch64::timer::TICK_COUNT;
use crate::arch::aarch64::uart::UartWriter;
use crate::sched;
use crate::task::{CpuSet, SchedulerClass, Thread, ThreadId, THREAD_TABLE};

/// Ticks the thread yields before it takes the lock, so that the scheduler,
/// the other threads and the first drains are running. Below the bench's
/// 500-tick settle, so the bench result block does not overlap the panic.
const SETTLE_TICKS: u64 = 200;

/// Ticks the thread holds THREAD_TABLE with IRQs on. The first CPU 0 tick
/// already re-enters (every tick sets `NEED_RESCHED`, so `check_preemption`
/// calls `schedule()`); 3 leaves margin for a tick that is delayed.
const HOLD_TICKS: u64 = 3;

/// Create the self-test thread and queue it on CPU 0. Call during boot,
/// before `sched::start()`.
pub fn init() {
    let stack_phys = sched::alloc_kernel_stack();
    let stack_virt_top = sched::phys_to_virt(stack_phys) + sched::STACK_SIZE;

    let mut thread = Thread::new_kernel(
        ThreadId(0xC00),
        b"tw-selftest\0\0\0\0\0",
        selftest_entry as *const () as usize,
        stack_phys,
    );
    // Interactive, like the bench threads: CPU 0 picks classes in strict
    // priority order, and after Gate 1 the Interactive bench main spins
    // there for good, so a Normal thread pinned to CPU 0 would never run.
    thread.sched.class = SchedulerClass::Interactive;
    thread.sched.effective_class = SchedulerClass::Interactive;
    // CPU 0 only: CPUs 1-3 take no timer IRQs (#200), so a hold there would
    // never be re-entered.
    thread.sched.affinity = CpuSet::single(0);
    thread.context.sp = stack_virt_top as u64;

    let idx = sched::allocate_thread(thread).expect("thread table full for tripwire selftest");
    sched::enqueue_on_cpu(0, ThreadId(idx as u32), SchedulerClass::Interactive);
    crate::kinfo!(Sched, "Tripwire selftest thread created");
}

/// Self-test thread entry.
fn selftest_entry() -> ! {
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit, which is permitted at
    // EL1 where kernel threads run. The scheduler and the timer are
    // initialised before any thread is dispatched. Unmasking earlier would
    // let a tick preempt code that requires IRQs masked; nothing here does.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    let start = TICK_COUNT.load(Ordering::Relaxed);
    while TICK_COUNT.load(Ordering::Relaxed).wrapping_sub(start) < SETTLE_TICKS {
        sched::thread_yield();
    }

    let mut w = UartWriter;
    let _ = writeln!(
        w,
        "[selftest] tripwire: cpu={} taking THREAD_TABLE with IRQs on for {} ticks",
        crate::observability::tripwire::cpu_here(),
        HOLD_TICKS
    );

    // `lock()` is `#[track_caller]`: the holder site recorded in the lock,
    // and printed in the re-entry message, is this line.
    let guard = THREAD_TABLE.lock();
    hold_loop();
    drop(guard);

    let _ = writeln!(
        w,
        "[selftest] tripwire: FAIL, no lock re-entry after {} ticks",
        HOLD_TICKS
    );
    loop {
        // SAFETY: wfe is a hint instruction, safe at EL1. IRQs are on, so a
        // tick still wakes this CPU and can switch away from the thread.
        unsafe { core::arch::asm!("wfe") };
    }
}

/// Busy-wait for [`HOLD_TICKS`] ticks of `TICK_COUNT`. A separate symbol so
/// that the `[panic]` line's `irq_elr` can be matched against its address
/// range in the ELF.
#[inline(never)]
fn hold_loop() {
    let start = TICK_COUNT.load(Ordering::Relaxed);
    while TICK_COUNT.load(Ordering::Relaxed).wrapping_sub(start) < HOLD_TICKS {
        core::hint::spin_loop();
    }
}
