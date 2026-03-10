//! SMP secondary core bringup and minimal scheduler stub.
//!
//! Manages secondary core startup via PSCI, per-core stacks,
//! and provides a minimal Scheduler for Phase 1 core tracking.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Maximum supported CPU cores.
pub const MAX_CORES: usize = 8;

// ── Per-core stack pointers (written by boot CPU, read by boot.S) ───
// UnsafeCell + unsafe impl Sync pattern (same as mmu.rs RawPageTable).
// Written once by boot CPU before PSCI CPU_ON, then read by each
// secondary core in _secondary_entry.

#[repr(C)]
struct StackPointers {
    stacks: UnsafeCell<[u64; MAX_CORES]>,
}

// SAFETY: Written once by boot CPU with DSB SY barrier before secondaries
// read. No concurrent writes after initialization.
unsafe impl Sync for StackPointers {}

#[no_mangle]
static SECONDARY_STACKS: StackPointers = StackPointers {
    stacks: UnsafeCell::new([0; MAX_CORES]),
};

/// Serializes secondary core printing. Core N waits for PRINT_TURN == N
/// before printing, then stores N+1. Uses only load(Acquire)/store(Release)
/// — no exclusive load/store pairs — which is safe on NC memory.
static PRINT_TURN: AtomicUsize = AtomicUsize::new(1);

/// Number of online CPUs. Only written by boot CPU after collecting results.
static ONLINE_CPUS: AtomicUsize = AtomicUsize::new(1);

/// GIC redistributor base address (set by boot CPU before SMP bringup).
static GICR_BASE: AtomicUsize = AtomicUsize::new(0);

// ── Per-core info and Scheduler stub ────────────────────────────────

/// Per-core state tracked by the Scheduler.
#[allow(dead_code)]
pub struct CoreInfo {
    pub mpidr: u64,
    pub stack_base: usize,
    pub stack_size: usize,
    pub online: AtomicBool,
}

impl CoreInfo {
    #[allow(dead_code)]
    const fn new() -> Self {
        Self {
            mpidr: 0,
            stack_base: 0,
            stack_size: 0,
            online: AtomicBool::new(false),
        }
    }
}

/// Minimal Scheduler stub for Phase 1.
/// Tracks per-core state (stack, MPIDR, online status).
/// Full scheduling classes (RT, Interactive, Normal, Idle) are Phase 3.
#[allow(dead_code)]
pub struct Scheduler {
    cores: [CoreInfo; MAX_CORES],
    core_count: usize,
}

/// Bring secondary cores online via PSCI CPU_ON.
///
/// Allocates per-core stacks from the buddy allocator, writes stack
/// pointers to SECONDARY_STACKS, then wakes each core with PSCI.
/// Returns a Scheduler with all core states populated.
pub fn bring_secondaries_online(dt: &crate::dtb::DeviceTree, gicr_base: usize) -> Scheduler {
    GICR_BASE.store(gicr_base, Ordering::Relaxed);

    let cpu_count = dt.cpu_count().min(MAX_CORES);
    #[allow(clippy::declare_interior_mutable_const)]
    const NEW_CORE: CoreInfo = CoreInfo::new();
    let mut sched = Scheduler {
        cores: [NEW_CORE; MAX_CORES],
        core_count: cpu_count,
    };

    // Boot CPU (core 0) is already online.
    sched.cores[0].mpidr = dt.cpu_mpidr(0);
    sched.cores[0].online.store(true, Ordering::Relaxed);

    if cpu_count <= 1 {
        return sched;
    }

    // Allocate stacks and populate SECONDARY_STACKS for each secondary core.
    for i in 1..cpu_count {
        let mpidr = dt.cpu_mpidr(i);
        // Allocate 16 KiB stack (buddy order 2 = 4 pages).
        // SAFETY: Identity map is active post-MMU init; buddy allocator is initialized.
        let stack_phys = {
            let mut guard = crate::mm::frame::FRAME_ALLOC.lock();
            if let Some(fa) = guard.as_mut() {
                // SAFETY: Identity map is active; frame allocator is initialized.
                unsafe { fa.alloc_pages(shared::Pool::Kernel, 2) }
            } else {
                // SAFETY: Fallback to legacy buddy if frame allocator not yet initialized.
                unsafe { crate::mm::buddy::BUDDY.lock().alloc_pages(2) }
            }
        }
        .expect("Failed to allocate secondary core stack");

        let stack_size = 4096 * 4; // 16 KiB
        let stack_top = stack_phys + stack_size;

        sched.cores[i].mpidr = mpidr;
        sched.cores[i].stack_base = stack_phys;
        sched.cores[i].stack_size = stack_size;

        // SAFETY: Single writer (boot CPU), secondaries not yet awake.
        unsafe {
            (*SECONDARY_STACKS.stacks.get())[i] = stack_top as u64;
        }
    }

    // DSB SY: ensure stack pointer writes are visible to secondary cores
    // before PSCI wakes them. Without this, a secondary core might read
    // a stale zero from its store buffer.
    // SAFETY: DSB SY is a barrier instruction, safe at EL1.
    unsafe { core::arch::asm!("dsb sy") };

    // Wake each secondary core via PSCI CPU_ON.
    extern "C" {
        static _secondary_entry: u8;
    }
    // With virtual linking, addr_of! returns a virtual address but PSCI CPU_ON
    // needs a physical entry point (secondary cores start with MMU off).
    let entry_virt = core::ptr::addr_of!(_secondary_entry) as u64;
    let entry_addr = crate::arch::aarch64::mmu::virt_to_phys(entry_virt);

    for i in 1..cpu_count {
        let mpidr = dt.cpu_mpidr(i);
        let ret = if dt.psci_hvc {
            crate::arch::aarch64::psci::cpu_on_hvc(mpidr, entry_addr, i as u64)
        } else {
            crate::arch::aarch64::psci::cpu_on_smc(mpidr, entry_addr, i as u64)
        };
        if ret != 0 {
            crate::kerror!(Smp, "PSCI CPU_ON core {} failed: {}", i, ret);
        }
    }

    // Wait for all secondaries to print (PRINT_TURN reaches cpu_count).
    // Uses only load(Acquire) — no exclusive pairs — safe on NC memory.
    let start = crate::boot_phase::boot_elapsed_ms();
    while PRINT_TURN.load(Ordering::Acquire) < cpu_count {
        if crate::boot_phase::boot_elapsed_ms() - start > 100 {
            crate::kwarn!(
                Smp,
                "SMP timeout: {}/{} cores online",
                PRINT_TURN.load(Ordering::Acquire),
                cpu_count
            );
            break;
        }
        core::hint::spin_loop();
    }

    let online = PRINT_TURN.load(Ordering::Acquire);
    ONLINE_CPUS.store(online, Ordering::Relaxed);
    crate::kinfo!(Smp, "{} CPUs online", online);
    sched
}

/// Entry point for secondary cores (called from boot.S _secondary_entry).
#[no_mangle]
pub extern "C" fn secondary_main(core_id: u64) -> ! {
    let core_id = core_id as usize;

    // Initialize this core's GIC redistributor and CPU interface.
    let gicr_base = GICR_BASE.load(Ordering::Relaxed);
    crate::arch::aarch64::gic::init_gicv3_secondary(gicr_base, core_id);

    // Wait for our turn to print (serializes UART output across cores).
    // Uses only load(Acquire) — no exclusive pairs — safe on NC memory.
    while PRINT_TURN.load(Ordering::Acquire) != core_id {
        core::hint::spin_loop();
    }

    crate::kinfo!(Smp, "Core {} online", core_id);

    // Signal next core's turn to print.
    PRINT_TURN.store(core_id + 1, Ordering::Release);

    // Enter idle loop — scheduler assigns work in Phase 3.
    loop {
        // SAFETY: WFE is a hint instruction, safe at EL1.
        unsafe { core::arch::asm!("wfe") };
    }
}
