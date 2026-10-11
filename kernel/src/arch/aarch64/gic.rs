//! GICv3 interrupt controller driver.
//!
//! Initializes the GICv3 distributor (once, on the boot CPU), and on every
//! CPU its own redistributor, its CPU interface and its timer PPI. Per
//! hal.md §4.1.
//!
//! # Per-CPU timer PPI setup (crash-fix step 200, #200)
//!
//! Every CPU configures the timer PPI on its own redistributor and relies on
//! no firmware state: [`setup_timer_ppi`] wakes the redistributor, disables
//! the PPI and waits for `GICR_CTLR.RWP`, writes `GICR_IGROUPR0` so that it
//! is in Group 1 (the only group the kernel enables), writes its priority,
//! checks that it is level-sensitive, enables it, and reads the result back
//! into a [`PpiStatus`]. The boot CPU runs it from
//! [`InterruptController::enable_irq`]'s PPI branch on its redistributor;
//! CPUs 1-3 run it from [`init_gicv3_secondary`] on theirs, so the two paths
//! cannot drift apart. The register layout and the readback rules are in
//! [`shared::gic`], with host tests.
//!
//! The boot CPU publishes two GIC-wide facts before PSCI `CPU_ON`: the timer
//! PPI's INTID (from the DTB, never a literal) and `GICD_CTLR.DS`. CPUs 1-3
//! read both in [`init_gicv3_secondary`], every CPU passes `DS` to the
//! readback check, and [`irq_handler_el1`] dispatches the INTID as the tick.

use core::cell::Cell;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use shared::gic::{
    security_disabled, waker_step, GicError, PpiConfig, PpiReadback, PpiStatus, WakerStep,
    GICR_CTLR, GICR_CTLR_RWP, GICR_ICENABLER0, GICR_ICFGR1, GICR_IGROUPR0, GICR_ISENABLER0,
    GICR_SGI_BASE_OFFSET, GICR_WAKER, IGROUPR0_ALL_GROUP1, TIMER_PPI_PRIORITY,
};

use crate::observability::tripwire;

/// GICv3 interrupt controller state.
pub struct InterruptController {
    gicd_base: usize,
    gicr_base: usize,
    /// The boot CPU's timer PPI setup result, set by [`Self::enable_irq`]'s
    /// PPI branch. `None` until the timer PPI is enabled.
    timer_ppi_status: Cell<Option<PpiStatus>>,
}

// GIC Distributor register offsets
const GICD_CTLR: usize = 0x0000;
const GICD_TYPER: usize = 0x0004;
const GICD_ISENABLER: usize = 0x0100;

// GICD_CTLR bits
const GICD_CTLR_ARE_NS: u32 = 1 << 4;
const GICD_CTLR_ENABLE_GRP1_NS: u32 = 1 << 1;
const GICD_CTLR_RWP: u32 = 1 << 31;

/// Distance between consecutive redistributors (RD_base + SGI_base, two
/// 64 KiB frames each, GICv3 without VLPIs).
const GICR_FRAME_STRIDE: usize = 0x2_0000;

/// Iteration bound of every GIC register poll (distributor RWP, redistributor
/// wake, redistributor RWP).
const GIC_POLL_LIMIT: u32 = 1_000_000;

/// An INTID that `ICC_IAR1_EL1` never returns for a real interrupt (1023,
/// spurious): the value of [`TIMER_PPI_INTID`] until the boot CPU sets up
/// the timer PPI, so the IRQ handler's compare can never match it.
const INTID_NONE: u32 = 1023;

/// The timer PPI's INTID (the DTB's non-secure physical timer, 30 on QEMU
/// virt). Written once by the boot CPU in [`InterruptController::enable_irq`]
/// before PSCI `CPU_ON`; read by [`init_gicv3_secondary`] and, one relaxed
/// load per IRQ, by [`irq_handler_el1`].
static TIMER_PPI_INTID: AtomicU32 = AtomicU32::new(INTID_NONE);

/// `GICD_CTLR.DS` as the boot CPU read it in [`init_gicv3`], before PSCI
/// `CPU_ON`. A distributor bit, the same for every CPU, so one read serves
/// all; every CPU's readback check takes it.
static SECURITY_DISABLED: AtomicBool = AtomicBool::new(false);

/// The timer PPI's INTID as the boot CPU published it ([`TIMER_PPI_INTID`]),
/// for the `[smp]` lines. Thread level; 1023 before the boot CPU sets up the
/// timer PPI.
pub fn timer_ppi_intid() -> u32 {
    TIMER_PPI_INTID.load(Ordering::Relaxed)
}

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

/// Read an 8-bit MMIO register.
#[inline(always)]
unsafe fn mmio_read8(addr: usize) -> u8 {
    core::ptr::read_volatile(addr as *const u8)
}

/// Write an 8-bit MMIO register.
#[inline(always)]
unsafe fn mmio_write8(addr: usize, val: u8) {
    core::ptr::write_volatile(addr as *mut u8, val);
}

/// Wake the redistributor at `rd_base`: one bounded loop over
/// [`waker_step`]. Returns true when `GICR_WAKER` reads awake within
/// [`GIC_POLL_LIMIT`] reads.
///
/// No formatting, UART access or reachable panic path, so CPUs 1-3 can run
/// it before their kmap TTBR1 install. The only panic in the dev-profile
/// build is core's pointer-alignment precondition on the `u32` MMIO
/// accesses, which a 64 KiB-aligned redistributor frame cannot trip.
///
/// # Safety
///
/// `rd_base` must be a redistributor's RD_base, mapped as device memory in
/// the current translation regime.
unsafe fn wake_redistributor(rd_base: usize) -> bool {
    let waker_addr = rd_base.wrapping_add(GICR_WAKER);
    for _ in 0..GIC_POLL_LIMIT {
        // SAFETY: waker_addr is GICR_WAKER in the RD_base frame the caller
        // guarantees is mapped device memory (this function's contract). A
        // wrong rd_base would fault (synchronous abort) or hit another
        // device's register.
        let waker = unsafe { mmio_read32(waker_addr) };
        match waker_step(waker) {
            WakerStep::Done => return true,
            WakerStep::Wait => core::hint::spin_loop(),
            // SAFETY: as for the read above; the value keeps every bit as
            // read except ProcessorSleep, and is written only when
            // ChildrenAsleep reads 1 (IHI 0069H.b §12.11.42).
            WakerStep::Write(value) => unsafe { mmio_write32(waker_addr, value) },
        }
    }
    false
}

/// Configure and check the timer PPI on the redistributor at `rd_base`:
/// wake, disable and wait for `GICR_CTLR.RWP`, Group 1, priority, level
/// check, enable, readback (plan rows 1-7; [`shared::gic`] holds the
/// offsets and the rules).
///
/// No formatting, UART access or reachable panic path, so CPUs 1-3 can run
/// it before their kmap TTBR1 install. The dev-profile build keeps panic
/// calls that cannot fire: core's pointer-alignment precondition on the
/// `u32` MMIO accesses (a redistributor frame is 64 KiB aligned) and the
/// shift and subtract overflow checks in [`PpiConfig`]'s helpers
/// ([`PpiConfig::new`] admits only INTIDs 16-31). A wake or RWP poll that
/// runs out sets its flag to `Fail` and the sequence carries on, so the
/// status names every property that failed.
///
/// # Safety
///
/// `rd_base` must be the calling CPU's own redistributor RD_base, with its
/// SGI_base frame following it, both mapped as device memory in the current
/// translation regime. The caller must have IRQs masked at PSTATE, because
/// the PPI is disabled while it is reconfigured.
unsafe fn setup_timer_ppi(rd_base: usize, cfg: PpiConfig, ds: bool) -> PpiStatus {
    let sgi_base = rd_base.wrapping_add(GICR_SGI_BASE_OFFSET);
    let mask = cfg.enable_mask();

    // SAFETY: rd_base is this CPU's own RD_base, mapped as device memory
    // (this function's contract, kept by both callers). A wrong rd_base would
    // fault (synchronous abort) or wake another CPU's redistributor.
    let wake_ok = unsafe { wake_redistributor(rd_base) };

    // SAFETY: every address below is a GICR register in the RD_base or
    // SGI_base frame that the caller guarantees is this CPU's own and mapped
    // as device memory; the offsets are IHI 0069H.b's (shared::gic). Wrong
    // bases would fault (synchronous abort) or write another CPU's or
    // device's registers. IRQs are masked, so no IRQ is taken while the PPI
    // is disabled.
    unsafe {
        // Disable the PPI and wait for the disable to take effect: it is
        // reconfigured only while disabled.
        mmio_write32(sgi_base.wrapping_add(GICR_ICENABLER0), mask);
        let ctlr_addr = rd_base.wrapping_add(GICR_CTLR);
        let mut rwp_ok = false;
        for _ in 0..GIC_POLL_LIMIT {
            if mmio_read32(ctlr_addr) & GICR_CTLR_RWP == 0 {
                rwp_ok = true;
                break;
            }
            core::hint::spin_loop();
        }

        // Every SGI and PPI in Group 1 (RAZ/WI to this write when DS = 0).
        let igroupr0_addr = sgi_base.wrapping_add(GICR_IGROUPR0);
        mmio_write32(igroupr0_addr, IGROUPR0_ALL_GROUP1);
        let igroupr0 = mmio_read32(igroupr0_addr);

        // The priority byte (IPRIORITYR is byte-accessible).
        let priority_addr = sgi_base.wrapping_add(cfg.priority_byte_offset());
        mmio_write8(priority_addr, cfg.priority());
        let priority = mmio_read8(priority_addr);

        // The trigger mode is only read: changing it is UNPREDICTABLE while
        // enabled and IMPLEMENTATION DEFINED for a PPI.
        let icfgr1 = mmio_read32(sgi_base.wrapping_add(GICR_ICFGR1));

        // Enable.
        let isenabler0_addr = sgi_base.wrapping_add(GICR_ISENABLER0);
        mmio_write32(isenabler0_addr, mask);
        let isenabler0 = mmio_read32(isenabler0_addr);

        cfg.check(
            &PpiReadback {
                wake_ok,
                rwp_ok,
                igroupr0,
                isenabler0,
                priority,
                icfgr1,
            },
            ds,
        )
    }
}

/// Enable the GIC system register interface and Group 1 on the calling
/// CPU's interface: `ICC_SRE_EL1.SRE` = 1, `ICC_PMR_EL1` = 0xFF (every
/// priority), `ICC_IGRPEN1_EL1` = 1.
///
/// # Safety
///
/// EL1 only; the caller must have IRQs masked at PSTATE until it is ready to
/// take them.
unsafe fn init_cpu_interface() {
    // SAFETY: ICC_SRE_EL1, ICC_PMR_EL1 and ICC_IGRPEN1_EL1 are this CPU's own
    // GICv3 CPU interface registers, writable at EL1 (the caller's contract).
    // An ISB follows each write that later accesses depend on. Without SRE
    // the ICC_* accesses would trap as undefined.
    unsafe {
        let sre: u64;
        core::arch::asm!("mrs {}, ICC_SRE_EL1", out(reg) sre);
        core::arch::asm!("msr ICC_SRE_EL1, {}", in(reg) sre | 1);
        core::arch::asm!("isb");

        core::arch::asm!("msr ICC_PMR_EL1, {}", in(reg) 0xFFu64);

        core::arch::asm!("msr ICC_IGRPEN1_EL1, {}", in(reg) 1u64);
        core::arch::asm!("isb");
    }
}

/// Initialize GICv3 distributor, redistributor (core 0), and CPU interface.
///
/// Records `GICD_CTLR.DS` for every CPU's PPI readback check. The timer PPI
/// itself is set up later, by [`InterruptController::enable_irq`].
pub fn init_gicv3(gicd_base: usize, gicr_base: usize) -> InterruptController {
    // SAFETY: GIC MMIO addresses are provided by the DTB and reached through
    // the boot identity map's device block (init_mmu ran before this). All
    // register accesses are to well-defined GICv3 registers; a wrong base
    // would fault with a synchronous abort on the boot CPU. IRQs are masked
    // at PSTATE on the boot CPU throughout (firmware hands over with PSTATE.I
    // set after ExitBootServices, and nothing unmasks before kernel_main's
    // DAIFClr just ahead of enter_scheduler), as wake_redistributor and
    // init_cpu_interface require; unmasked, enabling Group 1 here could
    // deliver an IRQ before the vector table, timer and scheduler state exist.
    unsafe {
        // --- Distributor init ---
        // Enable ARE (affinity routing) and Group 1 NS interrupts
        let ctlr = mmio_read32(gicd_base + GICD_CTLR);
        SECURITY_DISABLED.store(security_disabled(ctlr), Ordering::Relaxed);
        mmio_write32(
            gicd_base + GICD_CTLR,
            ctlr | GICD_CTLR_ARE_NS | GICD_CTLR_ENABLE_GRP1_NS,
        );

        // Wait for RWP (Register Write Pending) to clear
        let mut timeout = GIC_POLL_LIMIT;
        while mmio_read32(gicd_base + GICD_CTLR) & GICD_CTLR_RWP != 0 {
            timeout -= 1;
            if timeout == 0 {
                panic!("GICv3 distributor RWP timeout");
            }
        }

        // Read GICD_TYPER to get ITLinesNumber
        let typer = mmio_read32(gicd_base + GICD_TYPER);
        let _it_lines = (typer & 0x1F) + 1; // Number of 32-interrupt groups

        // --- Redistributor init (core 0, at gicr_base + 0) ---
        if !wake_redistributor(gicr_base) {
            panic!("GICv3 redistributor wake timeout");
        }

        // --- CPU interface init (system registers) ---
        init_cpu_interface();
    }

    InterruptController {
        gicd_base,
        gicr_base,
        timer_ppi_status: Cell::new(None),
    }
}

/// Initialize a secondary core's redistributor, timer PPI and CPU interface.
///
/// The distributor is already initialized by the boot CPU, which also
/// published the timer PPI's INTID and `GICD_CTLR.DS` before PSCI `CPU_ON`.
/// This CPU runs the same [`setup_timer_ppi`] as the boot CPU, on its own
/// redistributor, then enables its CPU interface.
///
/// `secondary_main` calls this before its kmap TTBR1 install: `gicr_base` is
/// the physical GICR base, reached through the boot identity TTBR0, and the
/// UART is not reachable yet. So this function has no formatting, UART
/// access or reachable panic path (the unreachable dev-profile panics are
/// [`setup_timer_ppi`]'s); it returns the setup status for the caller to report
/// once the UART is mapped. `Err` means the published INTID is not a valid
/// timer PPI configuration, and the redistributor was left untouched. The
/// boot CPU publishes only an INTID below 32 that validates (it panics
/// before `CPU_ON` on any other below 32), so `Err` arises only when the DTB
/// gave a timer INTID of 32 or above: [`InterruptController::enable_irq`]
/// takes that as an SPI and publishes nothing, this CPU reads 1023 and
/// returns `IntidNotPpi(1023)`, and `kernel_main` panics after SMP bringup
/// when it finds no boot CPU timer PPI status.
pub fn init_gicv3_secondary(gicr_base: usize, core_id: usize) -> Result<PpiStatus, GicError> {
    // Each redistributor is RD_base + SGI_base, 0x20000 apart, in CPU order
    // (core_id is the DTB CPU index). Wrapping arithmetic: no overflow
    // check, which could only panic.
    let rd_base = gicr_base.wrapping_add(core_id.wrapping_mul(GICR_FRAME_STRIDE));
    let ds = SECURITY_DISABLED.load(Ordering::Relaxed);
    let cfg = PpiConfig::new(TIMER_PPI_INTID.load(Ordering::Relaxed), TIMER_PPI_PRIORITY);

    let status = match cfg {
        // SAFETY: rd_base is this core's redistributor (DTB GICR base plus
        // its CPU index times the frame stride), mapped as device memory by
        // the boot identity TTBR0 that this core still runs on. IRQs are
        // masked: PSCI CPU_ON enters _secondary_entry with DAIF masked, and
        // nothing unmasks before enter_scheduler. A wrong frame would
        // configure another CPU's PPI.
        Ok(cfg) => Ok(unsafe { setup_timer_ppi(rd_base, cfg, ds) }),
        Err(e) => Err(e),
    };

    // SAFETY: secondary_main runs at EL1 with IRQs masked (PSCI CPU_ON entry
    // state) until enter_scheduler unmasks them, so enabling Group 1 here
    // cannot deliver an IRQ early. Unmasked, a pending timer PPI would be
    // taken before this core's kmap TTBR1 install and scheduler state exist.
    unsafe { init_cpu_interface() };

    status
}

#[allow(dead_code)]
impl InterruptController {
    /// Enable an interrupt by INTID.
    ///
    /// For INTIDs below 32 (boot CPU only): the timer PPI, the only PPI the
    /// kernel enables (`init_generic_timer`). Publishes the INTID for
    /// [`init_gicv3_secondary`] and [`irq_handler_el1`], runs the per-CPU
    /// [`setup_timer_ppi`] on the boot CPU's redistributor and keeps its
    /// status ([`Self::timer_ppi_status`]). Panics, before any secondary
    /// starts, if an INTID below 32 is not a valid timer PPI configuration
    /// (0-15, which the DTB parser cannot produce). CPUs 1-3 set up their
    /// own redistributor in [`init_gicv3_secondary`].
    ///
    /// For SPIs (INTID 32+): writes to `GICD_ISENABLER<n>`. A timer INTID of
    /// 32 or above (a broken DTB) lands here too: nothing is published, and
    /// the failure surfaces after SMP bringup (see [`init_gicv3_secondary`]).
    pub fn enable_irq(&self, irq: u32) {
        if irq < 32 {
            let cfg = match PpiConfig::new(irq, TIMER_PPI_PRIORITY) {
                Ok(cfg) => cfg,
                Err(e) => panic!("timer PPI {}: invalid GIC configuration: {:?}", irq, e),
            };
            TIMER_PPI_INTID.store(irq, Ordering::Relaxed);
            let ds = SECURITY_DISABLED.load(Ordering::Relaxed);
            // SAFETY: gicr_base is the boot CPU's own redistributor (frame 0
            // of the DTB GICR region), mapped as device memory by the
            // identity map that init_mmu installed. This runs during boot on
            // the boot CPU with IRQs masked: firmware hands over with
            // PSTATE.I set (ExitBootServices), and nothing unmasks before
            // kernel_main's DAIFClr just ahead of enter_scheduler. A wrong
            // base would fault with a synchronous abort.
            let status = unsafe { setup_timer_ppi(self.gicr_base, cfg, ds) };
            self.timer_ppi_status.set(Some(status));
        } else {
            let bit = 1u32 << (irq % 32);
            let reg_offset = GICD_ISENABLER + ((irq / 32) as usize) * 4;
            // SAFETY: GICD_ISENABLER<n> of the DTB distributor, mapped as
            // device memory; a set-enable write only enables INTID irq. A
            // wrong base would fault with a synchronous abort.
            unsafe { mmio_write32(self.gicd_base + reg_offset, bit) };
        }
    }

    /// The boot CPU's timer PPI setup result, once [`Self::enable_irq`] has
    /// set up the PPI.
    pub fn timer_ppi_status(&self) -> Option<PpiStatus> {
        self.timer_ppi_status.get()
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
    // Label this CPU IRQ context for the tripwire (counts `nest` if it was
    // not in thread context).
    tripwire::irq_enter();

    let intid = read_icc_iar1_el1();

    // Spurious interrupt (no pending IRQ).
    if intid >= 1020 {
        tripwire::irq_leave();
        return;
    }

    // The timer PPI (the DTB's non-secure physical timer): the INTID the
    // boot CPU published before any CPU unmasked IRQs. One relaxed load.
    if intid == TIMER_PPI_INTID.load(Ordering::Relaxed) {
        crate::arch::aarch64::timer::timer_tick_handler();
    } else {
        // Unknown IRQ — increment spurious counter if metrics enabled.
        #[cfg(feature = "kernel-metrics")]
        crate::observability::metrics::METRICS.irq_spurious.inc();
    }

    write_icc_eoir1_el1(intid);

    // Timer-driven preemption: when the tick set NEED_RESCHED,
    // check_preemption() calls schedule() here, after EOIR and before the
    // entry stub's eret, and schedule() may switch to another thread. The
    // interrupted thread keeps its IRQ entry frame on its own stack and
    // comes back through this path when it is resumed, possibly on another
    // CPU. The stub restores x0-x18, x29 and x30 only. It keeps a copy of
    // ELR_EL1 and SPSR_EL1 from entry just to count a difference
    // (`irq_frame_check`: `elrmm`, `spsrmm`); the eret uses whatever the
    // last exception on the resuming CPU left in the system registers: the
    // interrupted thread's values only if no other exception was taken on
    // that CPU in between (crash-fix ADR, H1).
    tripwire::irq_preempt_check();
    crate::sched::check_preemption();

    // Back to thread context, on the CPU this thread returns on.
    tripwire::irq_leave();
}
