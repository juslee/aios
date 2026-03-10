//! Process control structures.
//!
//! A process owns an address space and one or more threads. Each process
//! has kernel-enforced resource limits (ipc.md §3.3) that bound its
//! consumption of kernel objects.
//!
//! Per scheduler.md §3, ipc.md §3.3.

use super::ThreadId;
use crate::mm::uspace::UserAddressSpace;
use spin::Mutex;

// Re-export shared types.
pub use shared::{KernelResourceLimits, ProcessId};

// ---------------------------------------------------------------------------
// Process control block
// ---------------------------------------------------------------------------

/// Maximum threads per process.
const MAX_THREADS_PER_PROCESS: usize = 16;

/// Maximum processes system-wide.
pub const MAX_PROCESSES: usize = 32;

/// Process control block — owns an address space and tracks its threads.
pub struct ProcessControl {
    /// Process identifier.
    pub pid: ProcessId,
    /// User address space (None for kernel-only processes).
    pub address_space: Option<UserAddressSpace>,
    /// Kernel resource limits for this process.
    pub resource_limits: KernelResourceLimits,
    /// Thread IDs belonging to this process.
    pub thread_ids: [Option<ThreadId>; MAX_THREADS_PER_PROCESS],
    /// Human-readable name (for debugging).
    pub name: [u8; 32],
}

// ---------------------------------------------------------------------------
// Global process table
// ---------------------------------------------------------------------------

/// System-wide process table. BSS-allocated via `Option<ProcessControl>`.
pub static PROCESS_TABLE: Mutex<[Option<ProcessControl>; MAX_PROCESSES]> = {
    const NONE: Option<ProcessControl> = None;
    Mutex::new([NONE; MAX_PROCESSES])
};
