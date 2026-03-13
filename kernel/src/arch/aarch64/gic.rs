//! GICv3 interrupt controller driver.
//!
//! Initializes the GICv3 distributor, redistributor, and CPU interface
//! for the boot CPU. Per hal.md §4.1.

/// GICv3 interrupt controller state.
pub struct InterruptController {
    gicd_base: usize,
    gicr_base: usize,
}

// GIC Distributor register offsets
const GICD_CTLR: usize = 0x0000;
const GICD_TYPER: usize = 0x0004;
const GICD_ISENABLER: usize = 0x0100;

// GICD_CTLR bits
const GICD_CTLR_ARE_NS: u32 = 1 << 4;
const GICD_CTLR_ENABLE_GRP1_NS: u32 = 1 << 1;
const GICD_CTLR_RWP: u32 = 1 << 31;

// GIC Redistributor register offsets (within each 128 KiB frame)
const GICR_WAKER: usize = 0x0014;
const GICR_ISENABLER0: usize = 0x0100 + 0x10000; // SGI frame is at +64KiB

// GICR_WAKER bits
const GICR_WAKER_PROCESSOR_SLEEP: u32 = 1 << 1;
const GICR_WAKER_CHILDREN_ASLEEP: u32 = 1 << 2;

/// Read a 32-bit MMIO register.
#[inline(always)]
unsafe fn mmio_read32(addr: usize) -> u32 {
    core::ptr::read_volatile(addr as *const u32)
}

/// Write a 32-bit MMIO register.
#[inline(always)]
unsafe fn mmio_write32(addr: usize, val: u32) {
    core::ptr::write_volatile(addr as *mut u32, val);
}

/// Initialize GICv3 distributor, redistributor (core 0), and CPU interface.
pub fn init_gicv3(gicd_base: usize, gicr_base: usize) -> InterruptController {
    // SAFETY: GIC MMIO addresses are provided by DTB and validated by QEMU.
    // All register accesses are to well-defined GICv3 registers.
    unsafe {
        // --- Distributor init ---
        // Enable ARE (affinity routing) and Group 1 NS interrupts
        let ctlr = mmio_read32(gicd_base + GICD_CTLR);
        mmio_write32(
            gicd_base + GICD_CTLR,
            ctlr | GICD_CTLR_ARE_NS | GICD_CTLR_ENABLE_GRP1_NS,
        );

        // Wait for RWP (Register Write Pending) to clear
        let mut timeout = 1_000_000u32;
        while mmio_read32(gicd_base + GICD_CTLR) & GICD_CTLR_RWP != 0 {
            timeout -= 1;
            if timeout == 0 {
                panic!("GICv3 distributor RWP timeout");
            }
        }

        // Read GICD_TYPER to get ITLinesNumber
        let typer = mmio_read32(gicd_base + GICD_TYPER);
        let _it_lines = (typer & 0x1F) + 1; // Number of 32-interrupt groups

        // --- Redistributor init (core 0) ---
        // Core 0's redistributor is at gicr_base + 0
        // Clear ProcessorSleep to wake the redistributor
        let waker = mmio_read32(gicr_base + GICR_WAKER);

        if waker & GICR_WAKER_CHILDREN_ASLEEP != 0 {
            // Redistributor is asleep — wake it
            mmio_write32(gicr_base + GICR_WAKER, waker & !GICR_WAKER_PROCESSOR_SLEEP);

            // Wait for ChildrenAsleep to clear
            timeout = 1_000_000;
            while mmio_read32(gicr_base + GICR_WAKER) & GICR_WAKER_CHILDREN_ASLEEP != 0 {
                timeout -= 1;
                if timeout == 0 {
                    panic!("GICv3 redistributor wake timeout");
                }
            }
        }

        // --- CPU interface init (system registers) ---
        // Enable system register interface (ICC_SRE_EL1.SRE = 1)
        let sre: u64;
        core::arch::asm!("mrs {}, ICC_SRE_EL1", out(reg) sre);
        core::arch::asm!("msr ICC_SRE_EL1, {}", in(reg) sre | 1);
        core::arch::asm!("isb");

        // Set priority mask to allow all priorities (ICC_PMR_EL1 = 0xFF)
        core::arch::asm!("msr ICC_PMR_EL1, {}", in(reg) 0xFFu64);

        // Enable Group 1 interrupts (ICC_IGRPEN1_EL1 = 1)
        core::arch::asm!("msr ICC_IGRPEN1_EL1, {}", in(reg) 1u64);
        core::arch::asm!("isb");
    }

    InterruptController {
        gicd_base,
        gicr_base,
    }
}

/// Initialize GICv3 redistributor and CPU interface for a secondary core.
///
/// The distributor is already initialized by the boot CPU. Each secondary
/// core needs its own redistributor wakeup and CPU interface enable.
pub fn init_gicv3_secondary(gicr_base: usize, core_id: usize) {
    // Each redistributor frame is 128 KiB (0x20000) — RD_base + SGI_base.
    let redist_base = gicr_base + core_id * 0x20000;

    // SAFETY: GIC MMIO addresses are derived from DTB-provided base + core offset.
    // All register accesses are to well-defined GICv3 registers.
    unsafe {
        // Wake this core's redistributor.
        let waker = mmio_read32(redist_base + GICR_WAKER);
        if waker & GICR_WAKER_CHILDREN_ASLEEP != 0 {
            mmio_write32(
                redist_base + GICR_WAKER,
                waker & !GICR_WAKER_PROCESSOR_SLEEP,
            );

            let mut timeout = 1_000_000u32;
            while mmio_read32(redist_base + GICR_WAKER) & GICR_WAKER_CHILDREN_ASLEEP != 0 {
                timeout -= 1;
                if timeout == 0 {
                    // Can't panic cleanly on secondary — just break and continue.
                    break;
                }
            }
        }

        // Enable system register interface (ICC_SRE_EL1.SRE = 1).
        let sre: u64;
        core::arch::asm!("mrs {}, ICC_SRE_EL1", out(reg) sre);
        core::arch::asm!("msr ICC_SRE_EL1, {}", in(reg) sre | 1);
        core::arch::asm!("isb");

        // Set priority mask to allow all priorities.
        core::arch::asm!("msr ICC_PMR_EL1, {}", in(reg) 0xFFu64);

        // Enable Group 1 interrupts.
        core::arch::asm!("msr ICC_IGRPEN1_EL1, {}", in(reg) 1u64);
        core::arch::asm!("isb");

        // Enable timer PPI 30 (NS Physical Timer) in GICR_ISENABLER0.
        let bit = 1u32 << 30;
        mmio_write32(redist_base + GICR_ISENABLER0, bit);
    }
}

#[allow(dead_code)]
impl InterruptController {
    /// Enable an interrupt by INTID.
    ///
    /// For PPIs (INTID 16-31): writes to GICR_ISENABLER0 in the SGI frame.
    /// For SPIs (INTID 32+): writes to GICD_ISENABLER[n].
    pub fn enable_irq(&self, irq: u32) {
        let bit = 1u32 << (irq % 32);
        // SAFETY: GIC register writes at validated MMIO addresses.
        unsafe {
            if irq < 32 {
                // PPI/SGI: use redistributor SGI frame (core 0)
                let reg = self.gicr_base + GICR_ISENABLER0;
                mmio_write32(reg, bit);
            } else {
                // SPI: use distributor
                let reg_offset = GICD_ISENABLER + ((irq / 32) as usize) * 4;
                mmio_write32(self.gicd_base + reg_offset, bit);
            }
        }
    }

    /// Acknowledge an interrupt (read ICC_IAR1_EL1).
    pub fn acknowledge(&self) -> u32 {
        let irq: u64;
        // SAFETY: Reading ICC_IAR1_EL1 is safe at EL1.
        unsafe { core::arch::asm!("mrs {}, ICC_IAR1_EL1", out(reg) irq) };
        irq as u32
    }

    /// Signal end of interrupt (write ICC_EOIR1_EL1).
    pub fn end_of_interrupt(&self, irq: u32) {
        // SAFETY: Writing ICC_EOIR1_EL1 is safe at EL1.
        unsafe { core::arch::asm!("msr ICC_EOIR1_EL1, {}", in(reg) irq as u64) };
    }

    /// Get the distributor base address.
    pub fn gicd_base(&self) -> usize {
        self.gicd_base
    }

    /// Get the redistributor base address.
    pub fn gicr_base(&self) -> usize {
        self.gicr_base
    }

    /// Update base addresses after MMU enable (physical → virtual).
    pub fn update_bases(&mut self, gicd: usize, gicr: usize) {
        self.gicd_base = gicd;
        self.gicr_base = gicr;
    }
}

// ---------------------------------------------------------------------------
// Standalone IRQ handler (called from assembly, no instance needed)
// ---------------------------------------------------------------------------

/// Read ICC_IAR1_EL1 (acknowledge interrupt, get INTID).
#[inline(always)]
fn read_icc_iar1_el1() -> u32 {
    let val: u64;
    // SAFETY: ICC_IAR1_EL1 is readable at EL1.
    unsafe { core::arch::asm!("mrs {}, ICC_IAR1_EL1", out(reg) val) };
    val as u32
}

/// Write ICC_EOIR1_EL1 (signal end of interrupt).
#[inline(always)]
fn write_icc_eoir1_el1(intid: u32) {
    // SAFETY: ICC_EOIR1_EL1 is writable at EL1.
    unsafe { core::arch::asm!("msr ICC_EOIR1_EL1, {}", in(reg) intid as u64) };
}

/// Top-level IRQ handler called from the exception vector assembly stubs.
///
/// Reads the interrupt ID, dispatches to the appropriate handler,
/// and signals end of interrupt. Does not require an InterruptController
/// instance — uses direct system register access.
#[no_mangle]
extern "C" fn irq_handler_el1() {
    let intid = read_icc_iar1_el1();

    // Spurious interrupt (no pending IRQ).
    if intid >= 1020 {
        return;
    }

    match intid {
        30 => {
            // PPI 30: NS Physical Timer interrupt.
            crate::arch::aarch64::timer::timer_tick_handler();
        }
        _ => {
            // Unknown IRQ — increment spurious counter if metrics enabled.
            #[cfg(feature = "kernel-metrics")]
            crate::observability::metrics::METRICS.irq_spurious.inc();
        }
    }

    write_icc_eoir1_el1(intid);

    // Check if preemption is needed after handling the IRQ.
    // This enables timer-driven preemption: when NEED_RESCHED is set by
    // the timer tick, schedule() runs before eret returns to the
    // interrupted thread. Each thread's stack preserves the IRQ entry
    // frame, so context-switch + unwind works correctly.
    crate::sched::check_preemption();
}
