//! ARM Generic Timer driver.
//!
//! Configures the physical timer (CNTP) for a 1 ms scheduler tick.
//! Per hal.md §4.2.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use shared::tripwire::Key;

use crate::arch::aarch64::gic::InterruptController;
use crate::observability::tripwire;

/// ARM Generic Timer state.
#[allow(dead_code)]
pub struct Timer {
    frequency_hz: u64,
    tick_interval: u64,
    timer_irq: u32,
}

/// Read CNTFRQ_EL0 (timer frequency).
#[inline(always)]
pub fn read_cntfrq() -> u64 {
    let val: u64;
    // SAFETY: CNTFRQ_EL0 is always readable at EL1.
    unsafe { core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) val) };
    val
}

/// Read CNTVCT_EL0 (virtual timer count).
#[inline(always)]
pub fn read_counter() -> u64 {
    let val: u64;
    // SAFETY: CNTVCT_EL0 is always readable at EL1.
    unsafe { core::arch::asm!("mrs {}, CNTVCT_EL0", out(reg) val) };
    val
}

/// Initialize the ARM Generic Timer and register its interrupt in the GIC.
///
/// Programs a 1 ms tick on the physical timer (CNTP) and enables PPI `irq`
/// in the GIC. The timer fires but interrupts remain masked at PSTATE level
/// until the scheduler unmasks them (Phase 3).
pub fn init_generic_timer(irq: u32, ic: &InterruptController) -> Timer {
    let frequency_hz = read_cntfrq();
    assert!(
        frequency_hz > 0,
        "CNTFRQ_EL0 is zero — timer not configured"
    );

    let tick_interval = frequency_hz / 1000; // 1 ms tick

    // SAFETY: System register writes for timer configuration at EL1.
    unsafe {
        // Program physical timer compare value for 1 ms
        core::arch::asm!("msr CNTP_TVAL_EL0, {}", in(reg) tick_interval);

        // Enable physical timer: CNTP_CTL_EL0 bit 0 = ENABLE, bit 1 = IMASK (0 = not masked)
        core::arch::asm!("msr CNTP_CTL_EL0, {}", in(reg) 1u64);

        core::arch::asm!("isb");
    }

    // Enable the timer PPI in the GIC
    ic.enable_irq(irq);

    Timer {
        frequency_hz,
        tick_interval,
        timer_irq: irq,
    }
}

#[allow(dead_code)]
impl Timer {
    /// Current counter value.
    pub fn now(&self) -> u64 {
        read_counter()
    }

    /// Timer frequency in Hz.
    pub fn frequency(&self) -> u64 {
        self.frequency_hz
    }

    /// Timer interrupt INTID.
    pub fn irq(&self) -> u32 {
        self.timer_irq
    }

    /// Ticks per scheduler interval (1 ms).
    pub fn tick_interval(&self) -> u64 {
        self.tick_interval
    }

    /// Set the next timer deadline (ticks from now).
    pub fn set_next_deadline(&self, ticks: u64) {
        // SAFETY: CNTP_TVAL_EL0 write is safe at EL1.
        // ISB ensures ISTATUS is cleared before any subsequent EOIR.
        unsafe {
            core::arch::asm!("msr CNTP_TVAL_EL0, {}", in(reg) ticks);
            core::arch::asm!("isb");
        }
    }
}

// ---------------------------------------------------------------------------
// Timer tick infrastructure (Phase 3 M10 Step 4)
// ---------------------------------------------------------------------------

/// Tick interval in timer counts (set during init, read by all cores).
static TICK_INTERVAL: AtomicU64 = AtomicU64::new(0);

/// System tick counter: advanced once per timer tick on CPU 0 only, so it
/// counts CPU 0's ticks (1 ms apart nominally; fewer per second when CPU 0's
/// ticks run late). Every CPU's own tick count is the tripwire `tick` key.
pub static TICK_COUNT: AtomicU64 = AtomicU64::new(0);

/// Preemption needed flag (checked by scheduler return path in M11).
pub static NEED_RESCHED: AtomicBool = AtomicBool::new(false);

/// Store the tick interval for use by the tick handler and secondary cores.
/// Called once during boot CPU timer init.
pub fn set_tick_interval(interval: u64) {
    TICK_INTERVAL.store(interval, Ordering::Relaxed);
}

/// Initialize the timer on a secondary core.
///
/// Programs CNTP_TVAL_EL0 and enables the physical timer. Uses the
/// tick interval stored by the boot CPU during init.
pub fn init_timer_secondary() {
    let interval = TICK_INTERVAL.load(Ordering::Relaxed);
    if interval == 0 {
        return;
    }

    // SAFETY: CNTP_TVAL_EL0 and CNTP_CTL_EL0 are per-core timer registers,
    // safe to write at EL1. ISB ensures the writes take effect before the
    // timer can fire.
    unsafe {
        core::arch::asm!("msr CNTP_TVAL_EL0, {}", in(reg) interval);
        core::arch::asm!("msr CNTP_CTL_EL0, {}", in(reg) 1u64);
        core::arch::asm!("isb");
    }
}

/// Timer tick handler. Called from `irq_handler_el1` on PPI 30, on each CPU
/// that takes that interrupt, with IRQs masked.
///
/// 1. Rearm the timer (before EOIR, so the level-sensitive PPI does not fire
///    again at once), then count the tick in this CPU's tripwire `tick`.
/// 2. CPU 0 only: advance `TICK_COUNT`; every 1000th tick, print the
///    heartbeat and mark its tripwire line due.
/// 3. CPU 0 only, every 4th tick: drain the log rings to the UART.
/// 4. Scheduler tick: charge the current thread's time slice.
/// 5. Expire IPC timeouts.
/// 6. Every 4th tick: load balance.
/// 7. CPU 0 only, every 16th tick: signal input polling.
/// 8. Set `NEED_RESCHED`, which `check_preemption` reads after EOIR.
/// 9. Count the IRQ in the metrics (`kernel-metrics`).
/// 10. CPU 0 only: print a due tripwire line (`tripwire::end_of_tick`), last,
///     so the steps above keep their timing.
///
/// MUST NOT call klog! — this is called from IRQ context and drain_logs()
/// would deadlock or cause re-entrancy issues.
pub fn timer_tick_handler() {
    // 1. Rearm timer.
    // ISB after TVAL write ensures ISTATUS is cleared before irq_handler_el1
    // writes EOIR. PPI 30 is level-sensitive — without the barrier, the GIC
    // may still see the interrupt asserted and immediately re-pend it after
    // EOIR, causing an infinite IRQ loop.
    let interval = TICK_INTERVAL.load(Ordering::Relaxed);
    if interval > 0 {
        // SAFETY: CNTP_TVAL_EL0 is a per-core timer register, safe at EL1.
        // ISB ensures the timer state update (ISTATUS clear) is visible
        // to the GIC before the handler returns and EOIR is written.
        unsafe {
            core::arch::asm!("msr CNTP_TVAL_EL0, {}", in(reg) interval);
            core::arch::asm!("isb");
        }
    }
    // Count this CPU's tick (IRQ context, so IRQs are masked).
    tripwire::bump_masked(Key::Tick, 0);

    // 2-3. CPU 0 only: advance the system tick counter, drain the log rings
    // and print the heartbeat. TICK_COUNT is system-wide, so only one core
    // advances it. The log rings have a single consumer, so the tick drains
    // them on CPU 0 only. Draining runs on every 4th tick, straight to the
    // UART with IRQs masked. One call prints DRAIN_BATCH_SIZE (16) lines,
    // plus one dropped-messages line per ring and any entries a missing
    // continuation left pending (observability/mod.rs). A line joins a head
    // entry with its continuation, so it can reach ~130 characters: ~11 ms on
    // a 115200-baud PL011, so a full batch holds this handler for well over
    // its 1 ms tick interval and a log burst delays CPU 0's following ticks.
    // That cost is a known limit: the rate limit bounds how often a drain
    // runs and the batch size how much one call prints; neither fits a full
    // batch in one tick.
    let cpu = crate::observability::current_core_id().min(crate::smp::MAX_CORES - 1);
    if cpu == 0 {
        let tick = TICK_COUNT.fetch_add(1, Ordering::Relaxed);
        if tick.is_multiple_of(4) {
            crate::observability::drain_logs();
        }
        // Heartbeat every 1000 ticks (1s) to verify timer is alive.
        if tick.is_multiple_of(1000) {
            use core::fmt::Write;
            let mut w = crate::arch::aarch64::uart::UartWriter;
            let _ = writeln!(w, "[heartbeat] tick={}", tick);
            tripwire::note_heartbeat();
        }
    }

    // 4. Scheduler tick: decrement current thread's time slice.
    crate::sched::timer_tick(cpu);

    // 5. Check IPC timeouts (uses try_lock — safe from IRQ context).
    crate::ipc::check_timeouts();

    // 6. Load balance every 4 ticks.
    let tick = TICK_COUNT.load(Ordering::Relaxed);
    if tick.is_multiple_of(4) {
        crate::sched::try_load_balance();
    }

    // 7. Signal input polling every 16ms (CPU 0 only).
    if cpu == 0 && tick.is_multiple_of(16) {
        crate::input::set_poll_due();
    }

    // 8. Signal preemption needed.
    NEED_RESCHED.store(true, Ordering::Release);

    // 9. Increment IRQ metrics.
    #[cfg(feature = "kernel-metrics")]
    {
        crate::observability::metrics::METRICS.irq_total.inc();
        crate::observability::metrics::METRICS.irq_timer.inc();
    }

    // 10. CPU 0 only: print a due tripwire line (heartbeat or Gate 1).
    if cpu == 0 {
        tripwire::end_of_tick();
    }
}
