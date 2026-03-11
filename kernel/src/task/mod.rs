//! Thread and process data structures for the scheduler and IPC subsystem.
//!
//! Defines `Thread`, `ThreadContext`, `SchedEntity`, and supporting types.
//! Per scheduler.md §3–4.

pub mod process;

use crate::mm::buddy::PAGE_SIZE;
use crate::smp::MAX_CORES;
use spin::Mutex;

// Re-export shared types used throughout the kernel.
pub use shared::{CpuSet, SchedulerClass, ThreadId, ThreadState};

// ---------------------------------------------------------------------------
// Scheduling entity (scheduler.md §3.3)
// ---------------------------------------------------------------------------

/// Per-thread scheduling metadata used by the scheduler for all decisions.
#[allow(dead_code)]
pub struct SchedEntity {
    /// Unique thread identifier.
    pub thread_id: ThreadId,
    /// Owning agent (None for kernel threads).
    pub agent_id: Option<u64>,
    /// Scheduling class (RT, Interactive, Normal, Idle).
    pub class: SchedulerClass,
    /// Priority within class (0 = lowest, 255 = highest).
    pub priority: u8,
    /// Absolute deadline (for RT class).
    pub deadline: Option<u64>,
    /// Virtual runtime for weighted fair queuing (Normal class).
    pub vruntime: u64,
    /// Remaining time in current time slice (nanoseconds).
    pub time_slice_remaining: u64,
    /// Effective scheduling class (base overridden by priority inheritance).
    pub effective_class: SchedulerClass,
    /// Effective priority (base overridden by priority inheritance).
    pub effective_priority: u8,
    /// Inherited class from priority inheritance during IPC (ipc.md §9.2).
    pub inherited_class: Option<SchedulerClass>,
    /// Inherited priority from priority inheritance during IPC.
    pub inherited_priority: Option<u8>,
    /// Inherited deadline from priority inheritance during IPC.
    pub inherited_deadline: Option<u64>,
    /// CPU affinity mask.
    pub affinity: CpuSet,
    /// Thread execution state.
    pub state: ThreadState,
}

// ---------------------------------------------------------------------------
// Hardware context (scheduler.md §4.1, repr(C) for assembly access)
// ---------------------------------------------------------------------------

/// CPU register state saved/restored on context switch.
///
/// Layout is `repr(C)` so assembly can access fields by fixed byte offsets:
///   gp_regs[0]  = offset 0x000 (x0)
///   gp_regs[30] = offset 0x0F0 (x30/LR)
///   sp          = offset 0x0F8
///   pc          = offset 0x100 (ELR_EL1)
///   pstate      = offset 0x108 (SPSR_EL1)
///   ttbr0       = offset 0x110
///   timer_cval  = offset 0x118
///   timer_ctl   = offset 0x120
///
/// Total: 31×8 + 6×8 = 296 bytes.
#[repr(C)]
pub struct ThreadContext {
    /// General-purpose registers x0–x30.
    pub gp_regs: [u64; 31],
    /// Stack pointer (SP_EL0 for user threads, SP_EL1 for kernel threads).
    pub sp: u64,
    /// Program counter (saved in ELR_EL1 on exception entry).
    pub pc: u64,
    /// Processor state (saved in SPSR_EL1 on exception entry).
    pub pstate: u64,
    /// User page table base register (TTBR0_EL1).
    pub ttbr0: u64,
    /// Timer comparator value (CNTP_CVAL_EL0), saved across context switch.
    pub timer_cval: u64,
    /// Timer control register (CNTP_CTL_EL0), saved across context switch.
    pub timer_ctl: u64,
}

// Verify layout assumptions for assembly compatibility.
const _: () = assert!(core::mem::size_of::<ThreadContext>() == 296);

impl ThreadContext {
    #[allow(dead_code)]
    const ZERO: Self = Self {
        gp_regs: [0; 31],
        sp: 0,
        pc: 0,
        pstate: 0,
        ttbr0: 0,
        timer_cval: 0,
        timer_ctl: 0,
    };
}

// ---------------------------------------------------------------------------
// FP/NEON context (scheduler.md §4.1, lazy save via CPACR_EL1)
// ---------------------------------------------------------------------------

/// Floating-point and NEON register state. Saved lazily — only allocated
/// and saved when a thread actually uses FP/NEON instructions.
///
/// 16-byte alignment required for `stp q0, q1, [x0]` instructions.
#[repr(C, align(16))]
pub struct FpContext {
    /// NEON/FP registers v0–v31 (128-bit each).
    pub vregs: [u128; 32],
    /// Floating-point control register.
    pub fpcr: u32,
    /// Floating-point status register.
    pub fpsr: u32,
}

// 32×16 (vregs) + 4 (fpcr) + 4 (fpsr) = 520, padded to 528 by align(16).
const _: () = assert!(core::mem::size_of::<FpContext>() == 528);

// ---------------------------------------------------------------------------
// Thread (scheduler.md §3, §4)
// ---------------------------------------------------------------------------

/// Maximum threads system-wide.
pub const MAX_THREADS: usize = 64;

/// A kernel or user thread.
#[allow(dead_code)]
pub struct Thread {
    /// Scheduling metadata.
    pub sched: SchedEntity,
    /// CPU register context (saved/restored on switch).
    pub context: ThreadContext,
    /// FP/NEON context (allocated lazily on first FP use).
    pub fp_context: Option<FpContext>,
    /// Priority inheritance depth — tracks how many IPC call levels deep
    /// the inheritance chain is. Bounded to MAX_INHERITANCE_DEPTH (8).
    /// See ipc.md §9.2 for transitive inheritance design.
    pub inheritance_depth: u8,
    /// Owning process (for capability lookups). None for unassigned threads.
    pub owner_pid: Option<shared::ProcessId>,
    /// Physical address of the thread's stack base.
    pub stack_phys: usize,
    /// Human-readable name (for debugging).
    pub name: [u8; 16],
}

impl Thread {
    /// Create a new kernel thread.
    ///
    /// Sets up the context so that when this thread is first switched to,
    /// execution begins at `entry_fn` with the stack at `stack_phys + 4*PAGE_SIZE`
    /// (top of a 16 KiB stack, growing downward).
    ///
    /// PSTATE = 0x3C5: EL1h, DAIF all masked (the thread unmasks as needed).
    /// TTBR0 = 0 (kernel threads don't have a user address space).
    pub fn new_kernel(id: ThreadId, name: &[u8], entry_fn: usize, stack_phys: usize) -> Self {
        let mut name_buf = [0u8; 16];
        let copy_len = name.len().min(16);
        name_buf[..copy_len].copy_from_slice(&name[..copy_len]);

        Self {
            sched: SchedEntity {
                thread_id: id,
                agent_id: None,
                class: SchedulerClass::Normal,
                priority: 128,
                deadline: None,
                vruntime: 0,
                time_slice_remaining: 10_000_000, // 10ms default slice
                effective_class: SchedulerClass::Normal,
                effective_priority: 128,
                inherited_class: None,
                inherited_priority: None,
                inherited_deadline: None,
                affinity: CpuSet::all(),
                state: ThreadState::Runnable,
            },
            context: ThreadContext {
                gp_regs: [0; 31],
                sp: (stack_phys + 4 * PAGE_SIZE) as u64,
                pc: entry_fn as u64,
                pstate: 0x3C5, // EL1h, DAIF masked
                ttbr0: 0,
                timer_cval: 0,
                timer_ctl: 0,
            },
            fp_context: None,
            inheritance_depth: 0,
            owner_pid: None,
            stack_phys,
            name: name_buf,
        }
    }
}

// ---------------------------------------------------------------------------
// Global thread table
// ---------------------------------------------------------------------------

/// System-wide thread table. BSS-allocated via `Option<Thread>`.
///
/// Protected by a spinlock. In Phase 3 M11, individual thread access
/// will be optimized with per-thread locks or lock-free techniques.
pub static THREAD_TABLE: Mutex<[Option<Thread>; MAX_THREADS]> = {
    const NONE: Option<Thread> = None;
    Mutex::new([NONE; MAX_THREADS])
};

// ---------------------------------------------------------------------------
// Per-CPU current thread tracking
// ---------------------------------------------------------------------------

/// Per-CPU currently running thread ID. Used by the scheduler to know
/// which thread is active on each core without locking the thread table.
pub static CURRENT_THREAD: [Mutex<Option<ThreadId>>; MAX_CORES] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const NONE: Mutex<Option<ThreadId>> = Mutex::new(None);
    [NONE; MAX_CORES]
};
