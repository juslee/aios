#![no_std]

pub mod boot;
pub mod cap;
pub mod collections;
pub mod ipc;
pub mod kaslr;
pub mod memory;
pub mod observability;
pub mod sched;
pub mod storage;
pub mod syscall;

/// Physical address type alias.
pub type PhysAddr = u64;

/// Virtual address type alias.
pub type VirtAddr = u64;

/// Magic number for BootInfo validation: "AIOSBOOT" as u64.
pub const BOOTINFO_MAGIC: u64 = 0x41494F53_424F4F54;

// Re-export commonly used types at crate root for ergonomic imports.
pub use boot::{BootInfo, EarlyBootPhase, MemoryDescriptor, MemoryType, PixelFormat};
pub use cap::{
    Capability, CapabilityHandle, CapabilityTable, CapabilityToken, CapabilityTokenId,
    MAX_CAPS_PER_PROCESS,
};
pub use collections::{FixedQueue, RingBuffer};
pub use ipc::{
    validate_user_va, ChannelId, EndpointState, NotificationId, RawMessage, SelectEntry,
    SelectKind, ServiceName, ServiceState, SharedMemoryId, DEFAULT_TIMEOUT_TICKS, MAX_CHANNELS,
    MAX_INHERITANCE_DEPTH, MAX_MESSAGE_SIZE, MAX_NOTIFICATIONS, MAX_SELECT_ENTRIES, MAX_SERVICES,
    MAX_SERVICE_NAME_LEN, MAX_SHARED_MAPPINGS, MAX_SHARED_REGIONS, MAX_WAITERS_PER_NOTIFICATION,
    RING_CAPACITY, USER_VA_LIMIT,
};
pub use kaslr::{compute_slide_from_entropy, KaslrConfig};
pub use memory::{
    buddy_of, order_for_pages, ticks_to_ns, BenchStats, MemoryPressure, Pool, PoolConfig,
};
pub use observability::{timestamp_to_secs_micros, LogEntry, LogLevel, Subsystem};
pub use sched::{
    default_slice, CpuSet, KernelResourceLimits, ProcessId, SchedulerClass, ThreadId, ThreadState,
    IDLE_SLICE_NS, INTERACTIVE_SLICE_NS, NORMAL_SLICE_NS, RT_SLICE_NS,
};
pub use storage::{
    BlockId, BlockLocation, ContentHash, ContentType, SecurityZone, SpaceId, StorageError,
    StorageTier, Timestamp, BLOCK_SIZE, SECTOR_SIZE, SUPERBLOCK_MAGIC,
};
pub use syscall::{IpcError, Syscall, SYSCALL_COUNT};
