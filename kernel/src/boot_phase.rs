//! Early boot phase tracking and boot timing.
//!
//! Provides the `EarlyBootPhase` enum matching boot.md §3.1 and timing
//! functions that read the ARM Generic Timer counter registers directly.

use core::sync::atomic::{AtomicU64, Ordering};

/// Boot phases from entry point through full initialization.
/// Matches boot.md §3.1 EarlyBootPhase enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u32)]
#[allow(dead_code)]
pub enum EarlyBootPhase {
    EntryPoint = 0,
    ExceptionVectors = 1,
    DeviceTreeParsed = 2,
    UartReady = 3,
    InterruptsReady = 4,
    TimerReady = 5,
    MmuEnabled = 6,
    PageAllocatorReady = 7,
    HeapReady = 8,
    LogRingsReady = 9,
    RngReady = 10,
    KaslrApplied = 11,
    CapabilityManagerReady = 12,
    IpcReady = 13,
    AuditLogReady = 14,
    ProcessManagerReady = 15,
    ProvenanceReady = 16,
    Complete = 17,
}

static BOOT_START_TICKS: AtomicU64 = AtomicU64::new(0);
static CURRENT_PHASE: AtomicU64 = AtomicU64::new(0);

/// Read the ARM Generic Timer virtual count register.
#[inline(always)]
fn read_cntvct() -> u64 {
    let val: u64;
    // SAFETY: CNTVCT_EL0 is always readable at EL1 without configuration.
    unsafe { core::arch::asm!("mrs {}, CNTVCT_EL0", out(reg) val) };
    val
}

/// Read the ARM Generic Timer frequency register.
#[inline(always)]
fn read_cntfrq() -> u64 {
    let val: u64;
    // SAFETY: CNTFRQ_EL0 is always readable at EL1.
    unsafe { core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) val) };
    val
}

/// Initialize boot timing. Call once at kernel entry.
pub fn init_boot_timing() {
    BOOT_START_TICKS.store(read_cntvct(), Ordering::Relaxed);
}

/// Elapsed milliseconds since boot timing was initialized.
pub fn boot_elapsed_ms() -> u64 {
    let freq = read_cntfrq();
    if freq == 0 {
        return 0;
    }
    let elapsed_ticks = read_cntvct() - BOOT_START_TICKS.load(Ordering::Relaxed);
    (elapsed_ticks * 1000) / freq
}

/// Advance to a new boot phase. Prints the transition to UART if past UartReady.
pub fn advance_boot_phase(phase: EarlyBootPhase) {
    CURRENT_PHASE.store(phase as u64, Ordering::Relaxed);

    // Only print if we're past UartReady (UART is initialized).
    // Use println! directly here (not klog!) since LogRingsReady may not
    // have been reached yet, and the log macros need the boot phase check.
    if phase >= EarlyBootPhase::UartReady {
        crate::kinfo!(Boot, "{:?} — {}ms", phase, boot_elapsed_ms());
    }
}

/// Get the current boot phase.
#[allow(dead_code)]
pub fn current_boot_phase() -> EarlyBootPhase {
    let val = CURRENT_PHASE.load(Ordering::Relaxed) as u32;
    // SAFETY: All values 0..=17 are valid EarlyBootPhase variants.
    if val <= 17 {
        // SAFETY: repr(u32) enum with contiguous values 0..=17.
        unsafe { core::mem::transmute::<u32, EarlyBootPhase>(val) }
    } else {
        EarlyBootPhase::EntryPoint
    }
}
