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

// ---------------------------------------------------------------------------
// Process identity
// ---------------------------------------------------------------------------

/// Unique process identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessId(pub u32);

// ---------------------------------------------------------------------------
// Kernel resource limits (ipc.md §3.3)
// ---------------------------------------------------------------------------

/// Hard limits on kernel object creation per process.
///
/// Set at `ProcessCreate` and cannot be increased. A child process
/// cannot exceed its parent's limits (monotonic restriction).
#[derive(Debug, Clone, Copy)]
pub struct KernelResourceLimits {
    pub max_channels: u32,
    pub max_shared_regions: u32,
    pub max_pending_messages: u32,
    pub max_notification_subscriptions: u32,
    pub max_child_processes: u32,
}

impl KernelResourceLimits {
    /// Level 1 (System) trust level defaults.
    #[allow(dead_code)]
    pub const fn system() -> Self {
        Self {
            max_channels: 256,
            max_shared_regions: 128,
            max_pending_messages: 1024,
            max_notification_subscriptions: 64,
            max_child_processes: 32,
        }
    }

    /// Level 2 (Native) trust level defaults.
    #[allow(dead_code)]
    pub const fn native() -> Self {
        Self {
            max_channels: 128,
            max_shared_regions: 64,
            max_pending_messages: 512,
            max_notification_subscriptions: 32,
            max_child_processes: 16,
        }
    }

    /// Level 3 (Third-party) trust level defaults.
    #[allow(dead_code)]
    pub const fn third_party() -> Self {
        Self {
            max_channels: 64,
            max_shared_regions: 32,
            max_pending_messages: 256,
            max_notification_subscriptions: 16,
            max_child_processes: 8,
        }
    }

    /// Level 4 (Web) trust level defaults.
    #[allow(dead_code)]
    pub const fn web() -> Self {
        Self {
            max_channels: 16,
            max_shared_regions: 8,
            max_pending_messages: 64,
            max_notification_subscriptions: 4,
            max_child_processes: 0,
        }
    }
}

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
