#![no_std]

pub mod boot;
pub mod collections;
pub mod ipc;
pub mod memory;
pub mod sched;

/// Physical address type alias.
pub type PhysAddr = u64;

/// Virtual address type alias.
pub type VirtAddr = u64;

/// Magic number for BootInfo validation: "AIOSBOOT" as u64.
pub const BOOTINFO_MAGIC: u64 = 0x41494F53_424F4F54;

// Re-export commonly used types at crate root for ergonomic imports.
pub use boot::{BootInfo, MemoryDescriptor, MemoryType, PixelFormat};
pub use collections::{FixedQueue, RingBuffer};
pub use ipc::{validate_user_va, USER_VA_LIMIT};
pub use memory::{buddy_of, MemoryPressure, Pool, PoolConfig};
pub use sched::{
    default_slice, CpuSet, KernelResourceLimits, ProcessId, SchedulerClass, ThreadId, ThreadState,
    IDLE_SLICE_NS, INTERACTIVE_SLICE_NS, NORMAL_SLICE_NS, RT_SLICE_NS,
};
