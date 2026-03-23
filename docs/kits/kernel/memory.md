# Memory Kit

**Layer:** Kernel | **Crate:** `aios_memory` | **Architecture:** [`docs/kernel/memory.md`](../../kernel/memory.md) + 5 sub-docs

## 1. Overview

Memory Kit is the lowest-level Kit in AIOS — every other Kit depends on it. It provides
physical page allocation, virtual address space management, kernel heap services, and
zero-copy shared memory between agents. Application developers rarely interact with Memory
Kit directly; higher-level Kits (Storage Kit, Compute Kit, IPC Kit) handle memory on their
behalf. You reach for Memory Kit when you need to allocate shared memory regions for
zero-copy data transfer, manage memory-mapped buffers for hardware access, or query memory
pressure to adapt your agent's behavior.

Kit authors use Memory Kit extensively. Every Kit that manages buffers, caches, or
hardware-mapped regions uses the frame allocator, slab allocator, or shared memory APIs.
Memory Kit enforces W^X (write XOR execute) on every mapping — no page is both writable
and executable, ever.

AIOS targets devices with as little as 2 GB RAM, so memory is always scarce. The kernel
partitions physical memory into four pools — kernel, user, model (for AI inference), and
DMA — and enforces per-agent budgets. Memory Kit exposes pressure signals so agents can
adapt (release caches, reduce quality) before the OOM killer intervenes.

## 2. Core Traits

### Frame Allocator

```rust
use aios_memory::frame::{FrameAllocator, PhysFrame, Pool};

/// Pool-aware physical page allocator.
///
/// Allocates 4 KiB physical frames from one of four pools:
/// Kernel, User, Model, or DMA. Each pool is backed by a buddy
/// allocator (orders 0-10, 4 KiB to 4 MiB).
pub trait FrameAllocator {
    /// Allocate a physical frame from the specified pool.
    fn alloc_frame(&self, pool: Pool) -> Result<PhysFrame, MemoryError>;

    /// Free a physical frame back to its pool.
    fn free_frame(&self, frame: PhysFrame);

    /// Query memory pressure for a pool (0.0 = empty, 1.0 = full).
    fn pressure(&self, pool: Pool) -> f32;
}

/// Physical memory pool classification.
pub enum Pool {
    /// Kernel-internal allocations (page tables, stacks, metadata).
    Kernel,
    /// Per-agent heap and stack pages.
    User,
    /// AI model weights, KV caches, embedding stores.
    Model,
    /// DMA buffers for device drivers (contiguous, uncacheable).
    Dma,
}
```

### Address Space

```rust
use aios_memory::address_space::{AddressSpace, VirtAddr, Mapping, PagePermissions};

/// Per-agent virtual address space backed by a 4-level page table.
///
/// Each agent gets its own AddressSpace with an ASID (Address Space
/// Identifier) for TLB isolation. The kernel switches TTBR0 when
/// scheduling a different agent.
pub trait AddressSpace {
    /// Map a virtual address range to physical frames.
    /// Enforces W^X: the caller cannot set both WRITE and EXECUTE.
    fn map(
        &mut self,
        vaddr: VirtAddr,
        frames: &[PhysFrame],
        perms: PagePermissions,
    ) -> Result<(), MemoryError>;

    /// Unmap a virtual address range and return the freed frames.
    fn unmap(&mut self, vaddr: VirtAddr, pages: usize) -> Result<Vec<PhysFrame>, MemoryError>;

    /// Change permissions on an existing mapping.
    /// Cannot add EXECUTE to a WRITE page (W^X invariant).
    fn protect(
        &mut self,
        vaddr: VirtAddr,
        pages: usize,
        perms: PagePermissions,
    ) -> Result<(), MemoryError>;

    /// Query the current mapping at a virtual address.
    fn query(&self, vaddr: VirtAddr) -> Option<Mapping>;
}

/// Page permissions with W^X enforcement.
pub struct PagePermissions {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
    pub user_accessible: bool,
}
```

### Shared Memory

```rust
use aios_memory::shared::{SharedMemoryRegion, SharedMemoryId, MemoryFlags};

/// Zero-copy shared memory region between agents.
///
/// For payloads larger than the IPC inline limit (256 bytes), agents
/// allocate shared memory and pass the region ID over IPC. The kernel
/// enforces W^X on every mapping and tracks all agents with access
/// for cascade cleanup on process exit.
pub struct SharedMemoryRegion {
    pub id: SharedMemoryId,
    pub size: usize,
    pub flags: MemoryFlags,
}

impl SharedMemoryRegion {
    /// Create a new shared memory region backed by physical frames.
    pub fn create(size: usize, flags: MemoryFlags) -> Result<Self, MemoryError>;

    /// Map the region into another agent's address space.
    /// Both agents must consent (capability-gated).
    pub fn share_with(&self, agent: AgentId, flags: MemoryFlags) -> Result<(), MemoryError>;

    /// Get a pointer to the mapped region in this agent's address space.
    pub fn as_ptr(&self) -> *const u8;
    pub fn as_mut_ptr(&self) -> *mut u8;

    /// Unmap and destroy the region. All mappings in other agents
    /// are invalidated (they receive a MemoryRevoked event).
    pub fn destroy(self) -> Result<(), MemoryError>;
}

/// Memory region flags with W^X enforcement.
pub struct MemoryFlags {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}
```

### Memory Pressure

```rust
use aios_memory::pressure::{MemoryPressure, PressureLevel, PressureCallback};

/// Memory pressure monitoring for adaptive agents.
///
/// Agents register pressure callbacks to receive notifications when
/// memory is scarce. This lets agents release caches, reduce quality,
/// or defer work — avoiding the OOM killer.
pub trait MemoryPressure {
    /// Query current pressure level.
    fn current_level(&self) -> PressureLevel;

    /// Register a callback for pressure changes.
    fn on_pressure_change(&self, callback: PressureCallback) -> Result<(), MemoryError>;
}

/// Pressure levels (ascending severity).
pub enum PressureLevel {
    /// > 20% free — comfortable, no action needed.
    Normal,
    /// 11-20% free — release optional caches if convenient.
    Low,
    /// 5-10% free — aggressive reclamation, release everything possible.
    Critical,
    /// < 5% free — OOM killer imminent, last resort.
    Oom,
}
```

## 3. Usage Patterns

### Querying memory pressure (most common use case)

```rust
use aios_memory::pressure::{MemoryPressure, PressureLevel};

fn adapt_to_pressure(ctx: &AgentContext) -> Result<(), AppError> {
    let pressure = ctx.memory_pressure()?;
    match pressure.current_level() {
        PressureLevel::Normal => {
            // Full functionality — use caches freely
        }
        PressureLevel::Low => {
            // Release optional caches
            ctx.image_cache().evict_least_recent()?;
        }
        PressureLevel::Critical | PressureLevel::Oom => {
            // Emergency — drop all caches, show degraded UI
            ctx.image_cache().clear()?;
            ctx.show_toast("Low memory — some features reduced");
        }
    }
    Ok(())
}
```

### Zero-copy data transfer via shared memory

```rust
use aios_memory::shared::{SharedMemoryRegion, MemoryFlags};
use aios_ipc::ipc_call;

fn send_image_data(
    channel: ChannelId,
    pixels: &[u8],
) -> Result<(), AppError> {
    // Allocate shared memory for the pixel buffer
    let region = SharedMemoryRegion::create(
        pixels.len(),
        MemoryFlags { read: true, write: true, execute: false },
    )?;

    // Copy pixels into the shared region
    unsafe {
        core::ptr::copy_nonoverlapping(
            pixels.as_ptr(),
            region.as_mut_ptr(),
            pixels.len(),
        );
    }

    // Send the region ID over IPC (fits in 256-byte inline message)
    let msg = SharedMemoryMessage {
        region_id: region.id,
        offset: 0,
        length: pixels.len(),
    };
    let mut reply = [0u8; 64];
    ipc_call(channel, &msg.serialize(), &mut reply, Duration::secs(10))?;

    region.destroy()?;
    Ok(())
}
```

## 4. Integration Examples

### With IPC Kit — shared memory for large payloads

```rust
use aios_memory::shared::SharedMemoryRegion;
use aios_ipc::{ipc_recv, ipc_reply};

/// Server-side: receive a shared memory reference over IPC,
/// process the data, and reply.
fn handle_image_request(channel: ChannelId) -> Result<(), AppError> {
    let mut buf = [0u8; 256];
    let (len, _caller) = ipc_recv(channel, &mut buf, Duration::secs(30))?;

    let msg: SharedMemoryMessage = deserialize(&buf[..len])?;
    let region = SharedMemoryRegion::open(msg.region_id)?;

    // Read directly from the shared region — zero copy
    let data = unsafe {
        core::slice::from_raw_parts(region.as_ptr(), msg.length)
    };
    let result = process_image(data)?;

    ipc_reply(&result.serialize())?;
    Ok(())
}
```

### With Compute Kit — GPU buffer allocation

```rust
use aios_memory::shared::{SharedMemoryRegion, MemoryFlags};
use aios_compute::render::GpuRender;

/// Allocate a shared memory region for GPU texture upload.
/// The GPU driver maps the same physical pages — no copy needed.
fn upload_texture(
    renderer: &dyn GpuRender,
    pixels: &[u8],
    width: u32,
    height: u32,
) -> Result<GpuTexture, AppError> {
    let region = SharedMemoryRegion::create(
        pixels.len(),
        MemoryFlags { read: true, write: true, execute: false },
    )?;

    unsafe {
        core::ptr::copy_nonoverlapping(pixels.as_ptr(), region.as_mut_ptr(), pixels.len());
    }

    // GPU driver maps the shared region directly — zero copy to VRAM
    let texture = renderer.create_texture_from_shared(&region, width, height)?;
    Ok(texture)
}
```

## 5. Capability Requirements

| Capability | What It Gates | Default Grant |
| --- | --- | --- |
| `SharedMemoryCreate` | Creating shared memory regions | Granted to all agents |
| `SharedMemoryShare` | Mapping a region into another agent | Requires both agents' consent |
| `MemoryQuery` | Querying pressure levels and pool stats | Granted to all agents |
| `LargeAllocation` | Allocations above 1 MiB in the user pool | Prompt user on first use |

### Agent manifest example

```toml
[agent]
name = "com.example.image-processor"
version = "1.0.0"

[capabilities.required]
shared_memory = true       # Must be able to create shared regions for zero-copy I/O

[capabilities.optional]
large_allocation = true    # Can allocate large buffers (graceful degradation without)
```

## 6. Error Handling

```rust
/// Errors returned by Memory Kit operations.
pub enum MemoryError {
    /// No free frames in the requested pool.
    /// Recovery: register a pressure callback and wait, or reduce allocation size.
    OutOfMemory { pool: Pool, requested: usize, available: usize },

    /// The mapping would violate W^X (write XOR execute).
    /// Recovery: fix the permissions — this is a programming error.
    WxViolation,

    /// The shared memory region does not exist or was destroyed.
    /// Recovery: re-negotiate the region ID with the peer agent.
    InvalidRegion { id: SharedMemoryId },

    /// The agent does not hold the required capability.
    /// Recovery: request the capability or degrade gracefully.
    CapabilityDenied,

    /// The virtual address range is already mapped.
    /// Recovery: unmap the existing mapping first, or choose a different address.
    AlreadyMapped { vaddr: VirtAddr },

    /// The virtual address is not mapped.
    /// Recovery: map it first, or check the address.
    NotMapped { vaddr: VirtAddr },

    /// The allocation exceeds the agent's memory budget.
    /// Recovery: free unused memory or request a budget increase.
    BudgetExceeded { limit: usize, requested: usize },

    /// The maximum number of shared regions is reached (system-wide limit).
    /// Recovery: destroy unused regions and retry.
    TooManyRegions,
}
```

## 7. Platform & AI Availability

Memory Kit is a kernel primitive — it runs on all AIOS platforms with identical behavior.
Pool sizes vary by hardware:

| Hardware | RAM | Model Pool | User Pool | Notes |
| --- | --- | --- | --- | --- |
| QEMU virt | 2 GB | 0 MB | 1,792 MB | Cloud inference fallback |
| Pi 4 (4 GB) | 4 GB | 2 GB | 1,664 MB | Small models (1-3B Q4) |
| Pi 4/5 (8 GB) | 8 GB | 4 GB | 3,712 MB | Recommended: 8B Q4_K_M |
| Apple Silicon | 16 GB+ | 8 GB+ | 7 GB+ | Multiple models simultaneously |

When AIRS is online, it enhances memory management with:

- **Predictive page reclamation**: AIRS learns which pages will be needed soon and
  pre-evicts cold pages before pressure builds, reducing OOM kills.
- **Model memory scheduling**: AIRS coordinates model loading and eviction across
  the model pool, keeping hot models resident and pre-loading models the user is
  likely to need based on context.
- **Anomaly detection**: the Behavioral Monitor flags agents with unusual memory
  patterns — sudden large allocations, memory not freed after use, or shared memory
  accessed in unexpected patterns.

Without AIRS, memory management works identically — the kernel just uses simpler
heuristics (LRU eviction, static thresholds) instead of learned predictions.

## For Kit Authors

### Allocating from the correct pool

```rust
use aios_memory::frame::{FrameAllocator, Pool};

/// Kit authors must allocate from the correct pool:
/// - Pool::Kernel for internal Kit metadata (page tables, queues)
/// - Pool::User for agent-visible allocations
/// - Pool::Model for AI model data (AIRS Kit only)
/// - Pool::Dma for device driver buffers (contiguous, uncacheable)
fn allocate_kit_buffer(allocator: &dyn FrameAllocator, pages: usize) -> Result<Vec<PhysFrame>, MemoryError> {
    let mut frames = Vec::with_capacity(pages);
    for _ in 0..pages {
        frames.push(allocator.alloc_frame(Pool::Kernel)?);
    }
    Ok(frames)
}
```

### Using the slab allocator for small objects

```rust
use aios_memory::slab::SlabAllocator;

/// For Kit-internal data structures smaller than 4 KiB, use the slab
/// allocator. It provides O(1) allocation with per-CPU magazines
/// (no contention) and red-zone detection for buffer overflows.
///
/// Size classes: 64, 128, 256, 512, 4096 bytes.
/// Objects smaller than 64 bytes are rounded up to 64.
fn allocate_ipc_message() -> *mut RawMessage {
    // The global allocator routes small allocations to the slab
    // automatically — Kit authors don't need to call slab directly.
    // Just use alloc::boxed::Box or alloc::vec::Vec.
    let msg = Box::new(RawMessage::default());
    Box::into_raw(msg)
}
```

### Responding to memory pressure

```rust
use aios_memory::pressure::{MemoryPressure, PressureLevel};

/// Kits that maintain internal caches should register for pressure
/// notifications and evict when the kernel signals pressure.
fn register_cache_eviction(cache: &mut KitCache) {
    aios_memory::pressure::on_pressure_change(|level| {
        match level {
            PressureLevel::Low => cache.evict_expired(),
            PressureLevel::Critical => cache.evict_lru(cache.len() / 2),
            PressureLevel::Oom => cache.clear(),
            _ => {}
        }
    });
}
```

## Cross-References

- [Memory Management Architecture](../../kernel/memory.md) — kernel implementation details
- [Physical Memory & Slab Allocator](../../kernel/memory/physical.md) — buddy allocator, page pools
- [Virtual Memory & Page Tables](../../kernel/memory/virtual.md) — per-agent address spaces, KASLR
- [IPC Kit](./ipc.md) — shared memory for large IPC payloads
- [Compute Kit](./compute.md) — GPU/accelerator buffer allocation
- [Capability Kit](./capability.md) — SharedMemoryCreate/Share capabilities
