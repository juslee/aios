# AIOS Memory Management

## Deep Technical Architecture

**Parent document:** [architecture.md](../project/architecture.md)
**Related:** [ipc.md](./ipc.md) — IPC shared memory, [airs.md](../intelligence/airs.md) — Model memory and KV caches, [development-plan.md](../project/development-plan.md) — Phase 2

-----

## 1. Overview

The AIOS memory subsystem has a harder job than a traditional OS memory manager. It must handle the usual work — physical page allocation, virtual address spaces, kernel heap — but also manage multi-gigabyte AI model weights on devices with as little as 2 GB of total RAM. A conventional OS would page out inactive memory to disk. AIOS cannot do that for model weights — swapping 4 GB of model data would make inference unusable. The memory subsystem must be aware of what memory contains and why it matters.

The memory subsystem manages four concerns simultaneously:

1. **Traditional OS memory** — page allocator, virtual memory, kernel heap, per-process address spaces
2. **AI model memory** — large pinned regions for model weights, paged KV caches, embedding stores
3. **Per-agent isolation** — each agent gets its own address space with enforced memory limits
4. **Memory pressure on constrained devices** — 8 GB recommended minimum, 4 GB supported with constraints, 2 GB degraded mode, with a model that wants most of the RAM

The target hardware is Raspberry Pi 4/5 (aarch64, 2–8 GB RAM). Every design decision is made with this constraint in mind.

### Hardware Tier Classification

| RAM | Tier | Experience | Local AI | Notes |
|---|---|---|---|---|
| 2 GB | **Degraded** | Basic OS, 1-2 browser tabs, limited agents | Cloud inference only (no local model fits alongside OS) | Not recommended for the full AIOS experience |
| 4 GB | **Constrained** | Full OS, browser, agents | Small models only (1-3B Q4), limited KV cache | Functional but tight; model switching is slow on SD |
| 8 GB | **Recommended** | Full OS, browser, many agents | 8B Q4_K_M model + embedding model simultaneously | The target for the "AI-native OS" promise |
| 16 GB+ | **Comfortable** | Everything with headroom | 8B Q5_K_M/Q6_K + multiple specialist models | Future Pi hardware or alternative SBCs |

**8 GB is the recommended minimum** for users who want the advertised AI-native experience. The model pool gets 4 GB on an 8 GB device, which fits a quantized 8B model with room for KV caches and embedding stores. At 4 GB, the model pool is only 2 GB — enough for a 3B model but not the 8B models that deliver meaningfully better reasoning. At 2 GB, there is no model pool (0 MB — see §2.4); AIOS falls back to cloud inference via the Network Translation Module.

**Cloud inference fallback (2 GB devices):** When local inference is not viable, AIRS routes inference requests through the NTM to a configured cloud endpoint. The model pool is released to the user pool, giving agents and the browser more room. The system is fully functional — just slower (network latency) and dependent on connectivity. The user is informed at first boot: "This device has 2 GB RAM. AI features will use cloud processing. For local AI, 8 GB RAM is recommended."

-----

## 2. Physical Memory Manager

### 2.1 Bootstrap

At boot, UEFI hands the kernel a memory map — an array of `EFI_MEMORY_DESCRIPTOR` entries describing every region of physical memory. The kernel walks this array and classifies each region:

```
UEFI Memory Map (example, 4 GB device):

0x0000_0000 - 0x0000_0FFF   Reserved (ARM exception vectors)
0x0000_1000 - 0x0007_FFFF   Conventional (usable)
0x0008_0000 - 0x001F_FFFF   Loader Code (kernel image, reclaimable after boot)
0x0020_0000 - 0x3FFF_FFFF   Conventional (usable — bulk of RAM)
0x4000_0000 - 0x4000_FFFF   ACPI Reclaim
0xFE00_0000 - 0xFEFF_FFFF   MMIO (device registers)
0xFF80_0000 - 0xFFFF_FFFF   Reserved (firmware)
```

The kernel builds its initial free list from `Conventional` regions. `Loader Code` and `Loader Data` regions are reclaimed after early boot completes. `MMIO` and `Reserved` regions are never touched by the allocator.

```rust
// ── Kernel-internal types used throughout this document ──────────────
//
// The following types are referenced in code blocks below but defined
// elsewhere in the kernel or are opaque kernel primitives:
//
//   PhysicalAddress    — newtype wrapper around usize (defined in §3.2)
//   PhysicalFrame      — single physical page frame, identified by PFN (§2.2)
//   VirtualAddress     — newtype wrapper around usize (§3.2)
//   PageTableEntry     — 64-bit aarch64 PTE with W^X helpers (§3.2)
//   PageTable          — 512-entry page table (§3.2)
//   AddressSpace       — per-process virtual address space (§3.2)
//   BuddyAllocator     — physical page allocator, orders 0–10 (§2.2)
//   MemoryRegion       — UEFI memory map entry (§2.1, below)
//   SlabCache          — fixed-size object cache with per-CPU magazines (§4.1)
//   FaultError         — error enum for page fault outcomes (§10.5)
//   FaultType          — read / write / execute classification of a fault
//   PteState           — decoded non-valid PTE state (§10.5)
//   Vma                — alias for VmRegion (§3.2)
//   SharedMemoryId     — opaque handle for shared memory regions (§7)
//   MappedFile         — file-backed mapping descriptor
//   PageType           — page classification for MGLRU reclaim (§9)
//   FrameRefCount      — per-frame atomic reference counter (§4.2)
//   Process            — kernel process descriptor (see scheduler.md)
//   Pool               — memory pool discriminant: Kernel/User/Model/Dma (§2.4)

/// Physical memory region from UEFI memory map
pub struct MemoryRegion {
    pub base: PhysicalAddress,
    pub size: usize,
    pub kind: MemoryType,
}

/// Classification of physical memory (canonical definition — see also boot.md §3).
/// Names match UEFI memory descriptor types.
pub enum MemoryType {
    /// Usable RAM — available for allocation
    Conventional,
    /// Boot loader code — reclaimable after boot
    LoaderCode,
    /// Boot loader data — reclaimable after boot
    LoaderData,
    /// UEFI boot services code — reclaimable after ExitBootServices
    BootServicesCode,
    /// UEFI boot services data — reclaimable after ExitBootServices
    BootServicesData,
    /// UEFI runtime services code — must preserve
    RuntimeServicesCode,
    /// UEFI runtime services data — must preserve
    RuntimeServicesData,
    /// Firmware reserved — never touch
    Reserved,
    /// ACPI tables — reclaimable after parsing
    AcpiReclaimable,
    /// ACPI NVS — must preserve
    AcpiNvs,
    /// MMIO — device registers, never allocatable
    MemoryMappedIO,
    /// Boot info struct — reclaimable after early boot
    BootInfo,
    /// Kernel image — text/data/bss loaded by bootloader
    KernelImage,
    /// Initial RAM filesystem
    Initramfs,
}
```

### 2.2 Buddy Allocator

Physical page allocation uses a classic buddy system. Simple, well-understood, O(log n) allocation and free, and it naturally coalesces free regions to provide large contiguous blocks when needed.

```
Order   Page Count   Block Size   Use Case
─────   ──────────   ──────────   ────────
  0          1          4 KB      Single page (page tables, small allocs)
  1          2          8 KB      —
  2          4         16 KB      —
  3          8         32 KB      —
  4         16         64 KB      Medium THP (agent heaps, KV cache blocks)
  5         32        128 KB      —
  6         64        256 KB      —
  7        128        512 KB      —
  8        256          1 MB      —
  9        512          2 MB      Huge page (model weights)
 10       1024          4 MB      Maximum contiguous allocation
```

**Multi-size Transparent Huge Pages (THP):**

AIOS uses three page sizes on aarch64, matched to workload characteristics:

```
Page Size   Order   TLB Entries for 4 GB   Primary Use
─────────   ─────   ────────────────────   ───────────
  4 KB        0       1,048,576             Page tables, small allocs, fine-grained mapping
 64 KB        4          65,536             Agent heaps, KV cache blocks, shared memory
  2 MB        9           2,048             Model weights (pinned, read-only)
```

The 64 KB medium page (order 4) is the key innovation from Linux 6.8+ multi-size THP. It fills the gap between 4 KB (too small for bulk data, high TLB pressure) and 2 MB (too large for dynamic allocations, causes internal fragmentation). On the Cortex-A76 TLB (~1280 entries), 64 KB pages cover 80 MB per TLB set — a 16x improvement over 4 KB pages without requiring the 2 MB contiguous regions that model memory needs.

**Where medium pages are used:**

| Allocation Type | Page Size | Rationale |
|---|---|---|
| Agent heap (> 64 KB) | 64 KB | Reduces TLB misses for heap-heavy agents. Transparent: agent sees contiguous virtual memory. |
| KV cache blocks | 64 KB | Each KV block is 1 MB (16 × 64 KB frames). Medium pages reduce TLB entries per block from 256 to 16. |
| Shared memory regions | 64 KB | IPC shared buffers are typically 64 KB–1 MB. Medium pages avoid TLB thrashing during zero-copy transfers. |
| Page tables, slab caches | 4 KB | Fine-grained allocation where 64 KB would waste memory. |
| Model weights | 2 MB | Huge contiguous regions (2-8 GB). 2 MB pages minimize TLB entries for multi-GB models. |

**Transparent promotion:** When an agent's heap grows beyond 64 KB of contiguous virtual address space, the kernel transparently promotes backing pages from 4 KB to 64 KB if a contiguous order-4 region is available from the buddy allocator. The agent's PTEs are updated atomically. If a 64 KB region is not available (fragmentation), the allocation falls back to 4 KB pages — correctness is never compromised, only performance.

**Splitting under pressure:** When memory pressure reaches Critical and the reclaimer needs individual 4 KB pages, 64 KB medium pages can be split back into 16 × 4 KB pages. Only cold medium pages (generation 3 in MGLRU) are split. This ensures that THP benefits persist during normal operation and degrade gracefully under pressure.

Each order maintains a free list. Allocation splits larger blocks when needed; freeing merges adjacent buddies back together.

```rust
/// A single physical page frame
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PhysicalFrame {
    /// Physical frame number (address >> 12)
    pub pfn: usize,
}

impl PhysicalFrame {
    pub fn address(&self) -> PhysicalAddress {
        PhysicalAddress(self.pfn << 12)
    }

    /// Convert to a typed pointer via the direct-map region.
    pub fn as_ptr<T>(&self) -> *const T {
        (DIRECT_MAP_BASE + self.pfn * PAGE_SIZE) as *const T
    }

    /// Convert to a mutable typed pointer via the direct-map region.
    pub fn as_mut_ptr<T>(&self) -> *mut T {
        (DIRECT_MAP_BASE + self.pfn * PAGE_SIZE) as *mut T
    }

    pub fn from_address(addr: PhysicalAddress) -> Self {
        Self { pfn: addr.0 >> 12 }
    }
}

/// Buddy allocator for physical memory
pub struct BuddyAllocator {
    /// Free list per order (0..=MAX_ORDER)
    free_lists: [FreeList; MAX_ORDER + 1],
    /// Bitmap tracking allocated/free state per page
    bitmap: Bitmap,
    /// Base physical address of managed region
    base: PhysicalAddress,
    /// Total pages managed
    total_pages: usize,
    /// Free pages remaining
    free_pages: AtomicUsize,
}

const MAX_ORDER: usize = 10; // 4 MB max contiguous

impl BuddyAllocator {
    /// Allocate 2^order contiguous pages
    pub fn alloc(&mut self, order: u32) -> Option<PhysicalFrame> {
        // Try the requested order first
        if let Some(frame) = self.free_lists[order as usize].pop() {
            self.free_pages.fetch_sub(1 << order, Ordering::Relaxed);
            return Some(frame);
        }
        // Split a larger block
        for higher in (order + 1)..=(MAX_ORDER as u32) {
            if let Some(frame) = self.free_lists[higher as usize].pop() {
                // Split down to requested order, putting buddies on free lists
                self.split(frame, higher, order);
                self.free_pages.fetch_sub(1 << order, Ordering::Relaxed);
                return Some(frame);
            }
        }
        None // out of memory
    }

    /// Free 2^order contiguous pages, merging buddies
    pub fn free(&mut self, frame: PhysicalFrame, order: u32) {
        let mut current = frame;
        let mut current_order = order;

        // Merge with buddy if buddy is also free
        while current_order < MAX_ORDER as u32 {
            let buddy = self.buddy_of(current, current_order);
            if !self.bitmap.is_free(buddy, current_order) {
                break;
            }
            self.free_lists[current_order as usize].remove(buddy);
            current = PhysicalFrame {
                pfn: core::cmp::min(current.pfn, buddy.pfn),
            };
            current_order += 1;
        }

        self.free_lists[current_order as usize].push(current);
        self.free_pages.fetch_add(1 << order, Ordering::Relaxed);
    }
}
```

### 2.3 Frame Allocator Interface

The `FrameAllocator` wraps the buddy allocator and provides the primary API for the rest of the kernel:

```rust
pub struct FrameAllocator {
    buddy: BuddyAllocator,
    pools: PagePools,
    stats: AllocatorStats,
}

pub struct AllocatorStats {
    pub total_pages: usize,
    pub free_pages: usize,
    pub kernel_pages: usize,
    pub user_pages: usize,
    pub model_pages: usize,
    pub dma_pages: usize,
}

impl FrameAllocator {
    /// Allocate a single page from the specified pool
    pub fn alloc_page(&self, pool: Pool) -> Option<PhysicalFrame> {
        self.pools.alloc(pool, 0)
    }

    /// Allocate 2^order contiguous pages from the specified pool
    pub fn alloc_pages(&self, pool: Pool, order: u32) -> Option<PhysicalFrame> {
        self.pools.alloc(pool, order)
    }

    /// Free pages back to their pool
    pub fn free_pages(&self, frame: PhysicalFrame, order: u32) {
        self.pools.free(frame, order)
    }

    /// Current memory pressure level.
    /// Computed from the user pool only — the model pool is statically
    /// allocated and excluded from pressure calculations (§8).
    pub fn pressure(&self) -> MemoryPressure {
        let user_free = self.pools.user.free_pages.load(Ordering::Relaxed);
        let user_total = self.pools.user.total_pages;
        let free_pct = (user_free * 100) / user_total;
        match free_pct {
            21..=100 => MemoryPressure::Normal,
            11..=20  => MemoryPressure::Low,
            5..=10   => MemoryPressure::Critical,
            _        => MemoryPressure::Oom,
        }
    }
}
```

### 2.4 Page Pools

Physical memory is divided into pools at boot based on total RAM. Each pool reserves a region of physical memory for a specific purpose. This prevents one subsystem from starving another.

```
┌─────────────────────────────────────────────────────────┐
│                    Physical Memory Layout                 │
│                                                          │
│  ┌──────────┐  ┌───────────────────┐  ┌──────────────┐ │
│  │  Kernel   │  │   Model (pinned)   │  │    User      │ │
│  │  Pool     │  │   Pool             │  │    Pool      │ │
│  │           │  │                    │  │              │ │
│  │  page     │  │  model weights     │  │  agent       │ │
│  │  tables,  │  │  KV caches         │  │  heaps,      │ │
│  │  kernel   │  │  embedding stores  │  │  stacks,     │ │
│  │  heap,    │  │                    │  │  shared mem  │ │
│  │  slab     │  │  2 MB huge pages   │  │              │ │
│  │  caches   │  │  pinned, never     │  │  4 KB pages  │ │
│  │           │  │  swapped           │  │  demand-     │ │
│  │  4 KB     │  │                    │  │  paged       │ │
│  │  pages    │  │                    │  │              │ │
│  └──────────┘  └───────────────────┘  └──────────────┘ │
│  ┌──────────┐  ┌───────────────────┐                    │
│  │   DMA    │  │   Reserved         │                    │
│  │   Pool   │  │   (firmware, MMIO) │                    │
│  │          │  │                    │                    │
│  │  contig  │  │                    │                    │
│  │  aligned │  │                    │                    │
│  └──────────┘  └───────────────────┘                    │
└─────────────────────────────────────────────────────────┘
```

Pool sizing is determined at boot based on detected RAM:

```
Total RAM   Kernel    Model     User      DMA       Reserved    Tier
─────────   ──────    ──────    ──────    ──────    ────────    ────
  2 GB      128 MB    0 MB*     1.75 GB   64 MB     64 MB      Degraded
  4 GB      256 MB    2 GB      1.5 GB    128 MB    128 MB     Constrained
  8 GB      256 MB    4 GB      3.5 GB    128 MB    128 MB     Recommended
 16 GB      256 MB    8 GB      7.5 GB    128 MB    128 MB     Comfortable

*2 GB devices: model pool is 0 — cloud inference only. The full 1.75 GB
 (after kernel/DMA/reserved) is available to the user pool, giving agents
 and the browser more breathing room than the previous 768 MB.
```

**AIRS resource orchestration scope:** AIRS resource directives can only adjust the boundary between the **Model Pool** and **User Pool**. The Kernel Pool, DMA Pool, and Reserved regions are fixed at boot and are not subject to AIRS directives:

| Pool | AIRS Can Resize? | Reason |
|---|---|---|
| Kernel | No | Kernel data structures only. Fixed at boot. |
| Model | Yes (boundary with User) | AIRS grows model pool when loading models, shrinks when evicting. Constrained by `security_floor` (§12.2). |
| User | Yes (boundary with Model) | Inverse of model pool adjustments. Per-agent limits still enforced by blast radius (Layer 8). |
| DMA | No | Fixed at boot. Device I/O buffers require physically contiguous, stable pages. Not addressable by AIRS directives. |
| Reserved | No | Firmware/MMIO regions. Hardware-defined, immutable. |

```rust
pub enum Pool {
    /// Kernel data structures, page tables, slab caches
    Kernel,
    /// Agent heaps, stacks, shared memory regions
    User,
    /// Model weights, KV caches, embedding stores (pinned, huge pages)
    Model,
    /// Physically contiguous for device I/O
    Dma,
}

pub struct PagePools {
    kernel: BuddyAllocator,
    user: BuddyAllocator,
    model: BuddyAllocator,
    dma: BuddyAllocator,
}

/// Pool size configuration, computed at boot from detected RAM.
/// Reserved memory (firmware tables, MMIO) is tracked explicitly
/// to ensure the arithmetic in init() accounts for all RAM.
struct PoolConfig {
    kernel: usize,
    model: usize,
    user: usize,
    dma: usize,
    reserved: usize,
}

impl PagePools {
    /// Initialize pools based on total RAM
    pub fn init(total_ram: usize, regions: &[MemoryRegion]) -> Self {
        let config = match total_ram {
            // Degraded tier: no model pool, cloud inference only
            // All available RAM goes to user pool for agents/browser
            r if r <= 2 * GB => PoolConfig {
                kernel: 128 * MB,
                model: 0,
                user: (r - 128 * MB - 64 * MB - 64 * MB),
                dma: 64 * MB,
                reserved: 64 * MB,
            },
            // Constrained tier: small model pool (1-3B models)
            r if r <= 4 * GB => PoolConfig {
                kernel: 256 * MB,
                model: 2 * GB,
                user: (r - 256 * MB - 2 * GB - 128 * MB - 128 * MB),
                dma: 128 * MB,
                reserved: 128 * MB,
            },
            // Recommended tier: full model pool (8B Q4 models)
            r if r <= 8 * GB => PoolConfig {
                kernel: 256 * MB,
                model: 4 * GB,
                user: (r - 256 * MB - 4 * GB - 128 * MB - 128 * MB),
                dma: 128 * MB,
                reserved: 128 * MB,
            },
            // Comfortable tier: large model pool (8B Q5/Q6 + specialists)
            r => PoolConfig {
                kernel: 256 * MB,
                model: 8 * GB,
                user: (r - 256 * MB - 8 * GB - 128 * MB - 128 * MB),
                dma: 128 * MB,
                reserved: 128 * MB,
            },
        };
        Self::partition(regions, config)
    }
}
```

The model pool is the largest allocation on devices with 4 GB+ RAM. This is intentional — AIRS model weights dominate memory usage on target hardware. On a 4 GB device, the 2 GB model pool fits smaller models (1-3B at Q4) or heavily quantized variants of larger models. On 8 GB devices, the 4 GB model pool fits an 8B Q4_K_M model with room for KV caches.

**2 GB devices are the exception:** the model pool is zero. No local model fits alongside a running OS in 2 GB. Instead of allocating 1 GB for a model that would be too small to be useful, that memory goes to the user pool (1.75 GB total), giving agents and the browser substantially more headroom. AIRS falls back to cloud inference via the NTM.

-----

## 3. Virtual Memory Manager

### 3.1 Address Space Layout (aarch64)

ARM64 with 48-bit virtual addresses provides 256 TB of virtual address space, split between kernel (upper half, TTBR1) and user (lower half, TTBR0):

```
┌────────────────────────────────────────────────────────────┐
│                    Virtual Address Space                      │
│                     (48-bit, 256 TB)                         │
│                                                              │
│  0xFFFF_FFFF_FFFF_FFFF ┌────────────────────────────────┐  │
│                         │ Per-CPU data, temp mappings     │  │
│  0xFFFF_FF00_0000_0000 ├────────────────────────────────┤  │
│                         │         (gap)                   │  │
│  0xFFFF_0010_0000_0000 ├────────────────────────────────┤  │
│                         │ MMIO regions                    │  │
│  0xFFFF_0002_0000_0000 ├────────────────────────────────┤  │
│                         │ Physical memory direct map      │  │
│  0xFFFF_0001_0000_0000 ├────────────────────────────────┤  │
│                         │ Kernel heap                     │  │
│  0xFFFF_0000_4000_0000 ├────────────────────────────────┤  │
│    TTBR1                │ Kernel data (.data, .bss)       │  │
│    (kernel)             │ Kernel text (.text, read-only)  │  │
│  0xFFFF_0000_0000_0000 ├════════════════════════════════┤  │
│                         │                                 │  │
│                         │   Non-canonical address hole    │  │
│                         │   (inaccessible — hardware      │  │
│                         │    enforced gap)                 │  │
│                         │                                 │  │
│  0x0000_8000_0000_0000 ├════════════════════════════════┤  │
│  0x0000_7FFF_FFFF_F000 ├────────────────────────────────┤  │
│                         │ Stack (grows down)              │  │
│  0x0000_7FFF_FFC0_0000 ├────────────────────────────────┤  │
│                         │         (gap)                   │  │
│  0x0000_0010_0000_0000 ├────────────────────────────────┤  │
│                         │ Memory-mapped spaces            │  │
│  0x0000_0001_0000_0000 ├────────────────────────────────┤  │
│                         │ Shared memory (IPC regions)     │  │
│  0x0000_0000_1000_0000 ├────────────────────────────────┤  │
│    TTBR0                │ Agent heap (grows up)           │  │
│    (user, per-agent)    │ Agent data (.data, .bss)        │  │
│  0x0000_0000_0040_1000 ├────────────────────────────────┤  │
│                         │ Guard page (4 KB, unmapped)     │  │
│  0x0000_0000_0040_0000 ├────────────────────────────────┤  │
│                         │ Agent text (.text, read-only)   │  │
│  0x0000_0000_0010_0000 ├────────────────────────────────┤  │
│                         │ Guard page (unmapped)           │  │
│  0x0000_0000_0000_0000 └────────────────────────────────┘  │
└────────────────────────────────────────────────────────────┘
```

**Kernel space (TTBR1)** is identical across all processes. The same physical page tables back the upper-half mapping for every address space. Kernel code, data, heap, the physical memory direct map, and MMIO regions are always accessible when executing in EL1.

**User space (TTBR0)** is unique per agent. Each agent has its own page table tree rooted at TTBR0. When the scheduler switches from one agent to another, it writes the new agent's TTBR0 value. The kernel space stays mapped.

### 3.2 Page Tables

ARM64 with a 4 KB granule uses 4-level page tables. Each level indexes 9 bits of the virtual address:

```
48-bit Virtual Address:
┌───────┬───────┬───────┬───────┬────────────┐
│  PGD  │  PUD  │  PMD  │  PTE  │   Offset   │
│[47:39]│[38:30]│[29:21]│[20:12]│  [11:0]    │
│ 9 bits│ 9 bits│ 9 bits│ 9 bits│  12 bits   │
└───┬───┴───┬───┴───┬───┴───┬───┴────────────┘
    │       │       │       │
    ▼       ▼       ▼       ▼
   PGD     PUD     PMD     PTE → Physical Frame
  table    table   table   table
  (512     (512    (512    (512
  entries) entries)entries)entries)

For 2 MB huge pages (model memory):
┌───────┬───────┬───────┬─────────────────────┐
│  PGD  │  PUD  │  PMD  │      Offset          │
│[47:39]│[38:30]│[29:21]│     [20:0]           │
│ 9 bits│ 9 bits│ 9 bits│     21 bits          │
└───────┴───────┴───┬───┴─────────────────────┘
                     │
                     ▼
                    PMD entry points directly to
                    2 MB physical block (no PTE level)
```

```rust
/// Newtype wrappers for address types
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct VirtualAddress(pub usize);

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysicalAddress(pub usize);

/// Page table entry (64 bits on aarch64)
#[repr(transparent)]
#[derive(Copy, Clone)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    // Bit positions in the aarch64 page table entry
    const VALID: u64       = 1 << 0;   // Entry is valid
    const TABLE: u64       = 1 << 1;   // Points to next-level table (not block)
    const ATTR_IDX: u64    = 0b111 << 2; // Memory attribute index (MAIR)
    const NS: u64          = 1 << 5;   // Non-secure
    const AP_RO: u64       = 1 << 7;   // Read-only
    const AP_USER: u64     = 1 << 6;   // User accessible (EL0)
    const SH_INNER: u64    = 0b11 << 8; // Inner shareable
    const AF: u64          = 1 << 10;  // Access flag
    const NG: u64          = 1 << 11;  // Not global (uses ASID)
    const PXN: u64         = 1 << 53;  // Privileged execute-never
    const UXN: u64         = 1 << 54;  // Unprivileged execute-never
    const DIRTY: u64       = 1 << 55;  // Software: dirty
    const COW: u64         = 1 << 56;  // Software: copy-on-write

    pub fn is_valid(&self) -> bool { self.0 & Self::VALID != 0 }
    pub fn is_writable(&self) -> bool { self.0 & Self::AP_RO == 0 }
    pub fn is_executable(&self) -> bool { self.0 & Self::UXN == 0 }
    pub fn is_user(&self) -> bool { self.0 & Self::AP_USER != 0 }
    pub fn is_dirty(&self) -> bool { self.0 & Self::DIRTY != 0 }
    pub fn is_cow(&self) -> bool { self.0 & Self::COW != 0 }

    pub fn frame(&self) -> PhysicalFrame {
        PhysicalFrame::from_address(PhysicalAddress(
            (self.0 & 0x0000_FFFF_FFFF_F000) as usize
        ))
    }

    /// W^X enforcement: setting writable clears executable, and vice versa
    pub fn set_writable(&mut self) {
        self.0 &= !Self::AP_RO;     // clear read-only → writable
        self.0 |= Self::UXN;        // set execute-never → not executable
        self.0 |= Self::PXN;
    }

    pub fn set_executable(&mut self) {
        self.0 |= Self::AP_RO;      // set read-only → not writable
        self.0 &= !Self::UXN;       // clear execute-never → executable
    }

    /// Replace the physical frame address in this PTE.
    pub fn set_frame(&mut self, frame: PhysicalFrame) {
        self.0 = (self.0 & !0x0000_FFFF_FFFF_F000)
            | (frame.address().0 as u64 & 0x0000_FFFF_FFFF_F000);
    }

    /// Clear the COW software bit (page is now exclusively owned).
    pub fn clear_cow(&mut self) {
        self.0 &= !Self::COW;
    }
}

/// A page table (512 entries, 4 KB)
#[repr(C, align(4096))]
pub struct PageTable {
    entries: [PageTableEntry; 512],
}

/// Complete address space for a process
pub struct AddressSpace {
    /// Root page table (PGD) physical address — loaded into TTBR0
    pgd: PhysicalFrame,
    /// ASID for this address space
    asid: Asid,
    /// Virtual memory regions tracked for this space
    regions: BTreeMap<VirtualAddress, VmRegion>,
    /// Memory statistics
    stats: MemoryStats,
}

/// Describes a contiguous virtual memory region
pub struct VmRegion {
    pub start: VirtualAddress,
    pub size: usize,
    pub flags: VmFlags,
    pub kind: VmRegionKind,
}

bitflags::bitflags! {
    pub struct VmFlags: u32 {
        const READ     = 0b0001;
        const WRITE    = 0b0010;
        const EXECUTE  = 0b0100;
        const USER     = 0b1000;
        const SHARED   = 0b0001_0000;
        const PINNED   = 0b0010_0000;
        const HUGE     = 0b0100_0000;  // 2 MB pages
        const NO_DUMP  = 0b1000_0000;  // Excluded from core dumps and zram compression (cryptographic keys)
    }
}

pub enum VmRegionKind {
    /// Agent code section
    Text,
    /// Agent data section
    Data,
    /// Agent heap (grows up via brk/mmap)
    Heap,
    /// Agent stack (grows down)
    Stack,
    /// Shared memory (IPC)
    SharedMemory { region_id: SharedMemoryId },
    /// Memory-mapped space object
    MappedObject { object_id: ObjectId },
    /// Guard page (unmapped, triggers fault)
    Guard,
}

/// Alias used in COW and fault-handling code (§5.4).
type Vma = VmRegion;

impl AddressSpace {
    /// Look up the PTE for a virtual address by walking the four-level page table.
    /// Returns a reference to the leaf PTE, or FaultError::InvalidAddress if
    /// any intermediate table is missing (no auto-population).
    pub fn lookup_pte(&self, addr: VirtualAddress) -> Result<&PageTableEntry, FaultError> {
        let l0_idx = (addr.0 >> 39) & 0x1FF;
        let l1_idx = (addr.0 >> 30) & 0x1FF;
        let l2_idx = (addr.0 >> 21) & 0x1FF;
        let l3_idx = (addr.0 >> 12) & 0x1FF;

        let l0 = &self.pgd;
        let l1 = l0.entry(l0_idx).table().ok_or(FaultError::InvalidAddress)?;
        let l2 = l1.entry(l1_idx).table().ok_or(FaultError::InvalidAddress)?;
        // L2 entry may be a 2 MB block (huge page) — return it directly
        if l2.entry(l2_idx).is_block() {
            return Ok(l2.entry_ref(l2_idx));
        }
        let l3 = l2.entry(l2_idx).table().ok_or(FaultError::InvalidAddress)?;
        Ok(l3.entry_ref(l3_idx))
    }

    /// Mutable variant of lookup_pte. Walks the four-level page table
    /// and returns a mutable reference to the leaf PTE. Used by update_pte()
    /// and COW fault handling (§5.4) to modify PTEs in place.
    pub fn lookup_pte_mut(&mut self, addr: VirtualAddress) -> Result<&mut PageTableEntry, FaultError> {
        let l0_idx = (addr.0 >> 39) & 0x1FF;
        let l1_idx = (addr.0 >> 30) & 0x1FF;
        let l2_idx = (addr.0 >> 21) & 0x1FF;
        let l3_idx = (addr.0 >> 12) & 0x1FF;

        let l0 = &mut self.pgd;
        let l1 = l0.entry_mut(l0_idx).table_mut().ok_or(FaultError::InvalidAddress)?;
        let l2 = l1.entry_mut(l1_idx).table_mut().ok_or(FaultError::InvalidAddress)?;
        if l2.entry(l2_idx).is_block() {
            return Ok(l2.entry_mut(l2_idx));
        }
        let l3 = l2.entry_mut(l2_idx).table_mut().ok_or(FaultError::InvalidAddress)?;
        Ok(l3.entry_mut(l3_idx))
    }

    /// Overwrite the PTE for a virtual address. Caller must ensure the
    /// intermediate tables already exist (see map_page for auto-population).
    /// Issues a TLB invalidation for the affected VA after the write.
    pub fn update_pte(&mut self, addr: VirtualAddress, pte: PageTableEntry) {
        // Walk to the leaf entry and overwrite it
        let leaf = self.lookup_pte_mut(addr).expect("PTE must exist for update");
        *leaf = pte;
        // Single-entry TLBI for this ASID + VA
        tlb_invalidate_page(self.asid, addr);
    }

    /// Find the VmRegion (VMA) containing `addr`, if any.
    /// VmRegions are stored in an interval tree sorted by base address.
    pub fn find_vma(&self, addr: VirtualAddress) -> Option<&VmRegion> {
        self.regions.find_containing(addr)
    }

    /// Walk the page table and return the PTE (may be invalid/encoded).
    /// Unlike lookup_pte, this does not require the PTE to be valid —
    /// it returns whatever bits are stored, including swap/compressed
    /// encodings (see §10.5 for PteState decoding).
    pub fn walk_page_table(&self, addr: VirtualAddress) -> Result<PageTableEntry, FaultError> {
        self.lookup_pte(addr).copied()
    }

    /// Install a mapping: allocate intermediate page tables as needed, write the
    /// leaf PTE with the given frame and permissions. Enforces W^X — the perms
    /// argument must not set both WRITE and EXECUTE. Panics if W^X is violated.
    pub fn map_page(&mut self, addr: VirtualAddress, frame: PhysicalFrame, perms: VmFlags) {
        assert!(!perms.contains(VmFlags::WRITE | VmFlags::EXECUTE), "W^X violation");
        // Ensure L0→L1→L2→L3 tables exist, allocating from frame allocator as needed
        let l3_table = self.ensure_table_path(addr);
        let l3_idx = (addr.0 >> 12) & 0x1FF;
        l3_table.set_entry(l3_idx, PageTableEntry::page(frame, perms));
        tlb_invalidate_page(self.asid, addr);
    }
}
```

**W^X enforcement** is built into the page table entry API. The `set_writable()` method automatically clears the executable bit. The `set_executable()` method automatically sets read-only. No page is ever both writable and executable. This is enforced at the lowest level — there is no API to create a writable+executable mapping.

### 3.3 KASLR

The kernel base address is randomized at boot to defeat return-oriented programming (ROP) attacks that rely on known kernel addresses.

```
Boot sequence:
1. UEFI loads kernel ELF into a temporary address
2. Kernel early init reads random seed:
   - Preferred: UEFI RNG protocol (EFI_RNG_PROTOCOL)
   - Fallback: device tree /chosen/rng-seed property
   - Last resort: ARM generic counter (weak entropy)
3. Compute slide: random value & ~(2MB - 1) within 0..128 MB range
4. Relocate kernel to: base_address + slide
5. Fixup all kernel pointers (PIC — position-independent code)
6. Set up TTBR1 page tables at randomized base
```

```rust
pub struct KaslrConfig {
    /// Minimum kernel base address
    pub base: VirtualAddress,
    /// Alignment of the slide (2 MB — must be huge page aligned)
    pub alignment: usize,
    /// Range of possible slides
    pub slide_range: usize,
    /// Actual slide chosen at boot
    pub slide: usize,
}

impl KaslrConfig {
    pub fn default() -> Self {
        Self {
            base: VirtualAddress(0xFFFF_0000_0000_0000),
            alignment: 2 * MB,
            slide_range: 128 * MB,
            slide: 0, // computed at boot
        }
    }

    pub fn compute_slide(&mut self, entropy: u64) {
        let steps = self.slide_range / self.alignment;
        let step = (entropy as usize) % steps;
        self.slide = step * self.alignment;
    }

    pub fn kernel_base(&self) -> VirtualAddress {
        VirtualAddress(self.base.0 + self.slide)
    }
}
```

The slide range provides 64 possible positions at 2 MB alignment within a 128 MB window (unidirectional, starting from the base address) — enough to thwart automated attacks while keeping kernel virtual memory layout predictable for debugging.

### 3.4 TLB Management

The TLB (Translation Lookaside Buffer) caches virtual-to-physical translations. Without care, switching between address spaces requires a full TLB flush, which destroys performance. AIOS avoids this by using ASIDs.

**ASID (Address Space Identifier):** Each process gets a unique 16-bit ASID. TLB entries are tagged with the ASID. On context switch, the kernel writes the new ASID into TTBR0 — TLB entries from other ASIDs are ignored automatically, without flushing.

```rust
pub struct AsidAllocator {
    /// Current generation (incremented when ASID space wraps)
    generation: u64,
    /// Next ASID to allocate
    next: u16,
    /// Maximum ASID value (hardware-dependent, typically 65535)
    max: u16,
    /// Map from ASID to owning process
    owners: [Option<ProcessId>; 65536],
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Asid {
    pub value: u16,
    pub generation: u64,
}

impl AsidAllocator {
    /// Allocate an ASID for a new process
    pub fn alloc(&mut self) -> Asid {
        if self.next > self.max {
            // All ASIDs used — start new generation
            // This requires a full TLB flush (once per 65536 processes)
            self.generation += 1;
            self.next = 1; // ASID 0 is reserved for kernel
            self.owners = [None; 65536];
            tlbi_all(); // flush entire TLB — rare operation
        }
        let asid = Asid {
            value: self.next,
            generation: self.generation,
        };
        self.next += 1;
        asid
    }

    /// Check if an ASID is still valid (same generation)
    pub fn is_valid(&self, asid: Asid) -> bool {
        asid.generation == self.generation
    }
}
```

**TLB invalidation operations used by AIOS:**

| Operation | aarch64 Instruction | When Used |
|---|---|---|
| Invalidate single page | `TLBI VAE1IS, <Xt>` | Page remapped or unmapped |
| Invalidate by ASID | `TLBI ASIDE1IS, <Xt>` | Process terminated |
| Invalidate all | `TLBI VMALLE1IS` | ASID generation wraparound |

Single-page and ASID invalidations include the `IS` (Inner Shareable) suffix to broadcast to all cores on multi-core devices like the Pi 4/5.

-----

## 4. Kernel Heap

### 4.1 Slab Allocator

The kernel needs to allocate variable-sized objects frequently — page table pages, IPC message buffers, capability tokens, process descriptors. A raw buddy allocator wastes memory on small allocations (allocating 64 bytes wastes 4032 bytes of a 4 KB page). The slab allocator solves this.

```
Slab Allocator Architecture:

┌─────────────────────────────────────────────────────┐
│              Slab Allocator                           │
│                                                      │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐ │
│  │ Cache: IPC   │  │ Cache: Cap   │  │ Cache: PTE  │ │
│  │ Message      │  │ Token        │  │ Page         │ │
│  │ (64 bytes)   │  │ (128 bytes)  │  │ (4096 bytes) │ │
│  │              │  │              │  │              │ │
│  │ ┌────┐ free  │  │ ┌────┐ free │  │ ┌────┐ free │ │
│  │ │slab│──→    │  │ │slab│──→   │  │ │slab│──→   │ │
│  │ ├────┤       │  │ ├────┤      │  │ ├────┤      │ │
│  │ │slab│ full  │  │ │slab│ full │  │ │slab│ full │ │
│  │ ├────┤       │  │ ├────┤      │  │ ├────┤      │ │
│  │ │slab│partial│  │ │slab│      │  │ │slab│      │ │
│  │ └────┘       │  │ └────┘      │  │ └────┘      │ │
│  └─────────────┘  └─────────────┘  └─────────────┘ │
│                                                      │
│  Per-CPU Magazine Layer (lock-free fast path)        │
│  ┌─────────┐ ┌─────────┐ ┌─────────┐               │
│  │ CPU 0   │ │ CPU 1   │ │ CPU 2   │ ...           │
│  │ loaded  │ │ loaded  │ │ loaded  │               │
│  │ prev    │ │ prev    │ │ prev    │               │
│  └─────────┘ └─────────┘ └─────────┘               │
│                    │                                 │
│                    ▼                                 │
│           Buddy Allocator (for slab backing pages)   │
└─────────────────────────────────────────────────────┘
```

```rust
/// A slab cache for fixed-size objects
pub struct SlabCache {
    /// Object size (rounded up to alignment)
    object_size: usize,
    /// Objects per slab (per backing page)
    objects_per_slab: usize,
    /// List of slabs: partial (has free slots), full, empty
    partial: LinkedList<Slab>,
    full: LinkedList<Slab>,
    empty: LinkedList<Slab>,
    /// Per-CPU magazine for lock-free fast path (simplified: single magazine shown)
    magazine: Magazine,
    /// Name for debugging
    name: &'static str,
}

/// A single slab (backed by one or more physical pages)
pub struct Slab {
    /// Backing pages
    page: PhysicalFrame,
    /// Free object bitmap
    free_bitmap: Bitmap,
    /// Number of allocated objects
    allocated: usize,
    /// Total slots
    capacity: usize,
}

/// Per-CPU magazine — lock-free fast path for alloc/free
pub struct Magazine {
    /// Current magazine (array of free object pointers)
    current: MagazineRound,
    /// Previous magazine (swap when loaded is empty)
    prev: MagazineRound,
}

pub struct MagazineRound {
    objects: [*mut u8; MAGAZINE_SIZE],
    count: usize,
}

const MAGAZINE_SIZE: usize = 32;

impl SlabCache {
    /// Create a new slab cache for objects of `size` bytes.
    /// Allocates one initial slab (one physical page) and fills
    /// the per-CPU magazine for the boot CPU.
    pub fn new(name: &'static str, size: usize, fa: &FrameAllocator) -> Self {
        let aligned_size = size.next_power_of_two().max(8); // minimum 8-byte alignment
        let objects_per_slab = PAGE_SIZE / aligned_size;
        let initial_page = fa.alloc_pages(Pool::Kernel, 0).expect("slab init: OOM");
        // Carve the page into a freelist of fixed-size objects
        let initial_slab = Self::build_slab(initial_page, aligned_size, objects_per_slab);
        Self { name, object_size: aligned_size, objects_per_slab,
               partial: LinkedList::from(initial_slab), full: LinkedList::new(),
               empty: LinkedList::new(), magazine: Magazine::empty() }
    }

    /// Allocate one object from this cache.
    /// Fast path: pop from per-CPU magazine (no lock, no atomic).
    /// Slow path: refill magazine from shared freelist (lock required).
    pub fn alloc(&mut self) -> *mut u8 {
        // Fast path: per-CPU magazine
        if let Some(ptr) = self.magazine.current.pop() {
            return ptr;
        }
        // Swap current ↔ prev magazine
        core::mem::swap(&mut self.magazine.current, &mut self.magazine.prev);
        if let Some(ptr) = self.magazine.current.pop() {
            return ptr;
        }
        // Slow path: refill magazine from shared freelist
        self.refill_magazine();
        self.magazine.current.pop().expect("slab: refill failed — OOM")
    }

    /// Return an object to this cache.
    /// Fast path: push to per-CPU magazine. If magazine is full,
    /// swap with prev and push. If both full, flush prev to freelist.
    pub fn free(&mut self, ptr: *mut u8) {
        if self.magazine.current.push(ptr) { return; }
        core::mem::swap(&mut self.magazine.current, &mut self.magazine.prev);
        if self.magazine.current.push(ptr) { return; }
        // Both magazines full — flush prev to shared freelist, then push
        self.flush_magazine(&mut self.magazine.prev);
        self.magazine.current.push(ptr);
    }

    /// Grow the cache by allocating a new backing slab from the frame allocator.
    /// Called when the shared freelist is empty and a magazine refill is needed.
    pub fn grow(&mut self, fa: &FrameAllocator) {
        let page = fa.alloc_pages(Pool::Kernel, 0).expect("slab grow: OOM");
        let new_slab = Self::build_slab(page, self.object_size, self.objects_per_slab);
        self.partial.push_back(new_slab);
    }
}

/// Top-level slab allocator managing all caches
pub struct SlabAllocator {
    caches: [SlabCache; NUM_CACHES],
}

impl SlabAllocator {
    /// Allocate `size` bytes with `align` alignment.
    /// Finds the smallest cache whose object size >= requested size.
    /// Returns null if no cache fits (caller falls back to buddy allocator
    /// for allocations larger than the biggest slab cache).
    pub fn alloc(&mut self, size: usize, align: usize) -> *mut u8 {
        let effective_size = size.max(align);
        for cache in &self.caches {
            if cache.object_size >= effective_size {
                return cache.alloc();
            }
        }
        core::ptr::null_mut() // too large for slab — caller uses buddy allocator
    }

    /// Free a previously allocated pointer of the given `size`.
    /// Routes to the correct slab cache based on size.
    pub fn free(&mut self, ptr: *mut u8, size: usize) {
        for cache in &self.caches {
            if cache.object_size >= size {
                cache.free(ptr);
                return;
            }
        }
        panic!("slab free: size {} exceeds all caches — was this allocated from buddy?", size);
    }

    /// Standard caches created at boot
    pub fn init(frame_allocator: &FrameAllocator) -> Self {
        Self {
            caches: [
                SlabCache::new("ipc_message", 64, frame_allocator),
                SlabCache::new("capability_token", 128, frame_allocator),
                SlabCache::new("channel", 256, frame_allocator),
                SlabCache::new("process_descriptor", 512, frame_allocator),
                SlabCache::new("vm_region", 128, frame_allocator),
                SlabCache::new("page_table", 4096, frame_allocator),
            ],
        }
    }
}
```

The per-CPU magazine layer eliminates lock contention on the allocation hot path. Each CPU maintains a small array of pre-allocated objects. Allocating takes an object from the local magazine — no locks, no atomic operations, just a decrement and a pointer load. Only when the magazine is empty does the CPU need to access the shared slab (which requires a lock).

### 4.2 Kernel Allocation API

The kernel provides a typed allocation interface built on top of the slab and buddy allocators:

```rust
// ── Kernel global singletons (initialized once during boot) ─────────
//
// These are module-level statics accessed throughout the kernel.
// Each is protected by a spin-lock or is inherently lock-free.

/// Physical page allocator — partitioned into Kernel/User/Model/DMA pools (§2.4).
/// FrameAllocator wraps PagePools and provides alloc_page/free_pages (§2.3).
static FRAME_ALLOCATOR: FrameAllocator = /* initialized at boot from PagePools::init() */;

/// Per-frame reference counts for COW and shared mappings (§5.4).
static FRAME_REFCOUNT: FrameRefCount = /* initialized at boot, one atomic counter per PFN */;

/// Slab allocator for small fixed-size kernel objects (§4.1).
static SLAB_ALLOCATOR: SlabAllocator = /* initialized by SlabAllocator::init() at boot */;

/// Queue of frames awaiting asynchronous zeroing by the page-zero thread.
static ZERO_QUEUE: PageZeroQueue = /* initialized at boot */;

/// Typed kernel allocation — uses slab cache if size matches, buddy otherwise
pub fn kalloc<T>() -> *mut T {
    let size = core::mem::size_of::<T>();
    let align = core::mem::align_of::<T>();
    let ptr = SLAB_ALLOCATOR.alloc(size, align);
    if ptr.is_null() {
        panic!("kernel allocation failed: OOM for {} bytes", size);
    }
    ptr as *mut T
}

pub fn kfree<T>(ptr: *mut T) {
    let size = core::mem::size_of::<T>();
    SLAB_ALLOCATOR.free(ptr as *mut u8, size);
}

/// Page-granularity allocation (delegates to buddy allocator)
pub fn alloc_pages(order: u32) -> Option<PhysicalFrame> {
    FRAME_ALLOCATOR.alloc_pages(Pool::Kernel, order)
}

pub fn free_pages(frame: PhysicalFrame, order: u32) {
    FRAME_ALLOCATOR.free_pages(frame, order);
}

/// Contiguous physical memory for DMA
pub fn alloc_contiguous(size: usize) -> Option<PhysicalFrame> {
    let order = size.next_power_of_two().trailing_zeros();
    FRAME_ALLOCATOR.alloc_pages(Pool::Dma, order)
}

/// Zero a page asynchronously (page zeroing thread picks this up)
pub fn zero_page_async(frame: PhysicalFrame) {
    ZERO_QUEUE.push(frame);
}
```

Kernel allocation failure in core data paths (page table allocation during process creation, IPC buffer allocation) is a fatal condition. The kernel must always reserve enough memory in the kernel pool to service its own needs. This is why the kernel pool is sized generously (128–256 MB) and is separate from the user pool.

-----

## 5. Per-Agent Memory Management

### 5.1 Agent Address Spaces

Each agent gets its own address space — a unique TTBR0 page table tree. No two agents share virtual-to-physical mappings except through explicit shared memory regions.

```
Agent "research-assistant"              Agent "code-editor"
┌─────────────────────────┐            ┌─────────────────────────┐
│  TTBR0: 0x1A2B_0000     │            │  TTBR0: 0x3C4D_0000     │
│  ASID: 42                │            │  ASID: 43                │
│                          │            │                          │
│  0x0040_0000  data  (RW-)│            │  0x0040_0000  data  (RW-)│
│  0x0080_0000  data  (RW-)│            │  0x0080_0000  data  (RW-)│
│  0x0100_0000  heap  (RW-)│            │  0x0100_0000  heap  (RW-)│
│       ...                │            │       ...                │
│  0x1_0000_0000 shm  (RW-)│──┐         │  0x1_0000_0000 shm  (RW-)│──┐
│       ...                │  │         │       ...                │  │
│  0x7FFF_FFC0_0000 stack  │  │         │  0x7FFF_FFC0_0000 stack  │  │
└─────────────────────────┘  │         └─────────────────────────┘  │
                              │                                      │
                              │    ┌───────────────────┐             │
                              └───→│  Shared Memory     │←────────────┘
                                   │  Region #17        │
                                   │  Physical: 0x5000  │
                                   │  Size: 64 KB       │
                                   │  Refcount: 2       │
                                   └───────────────────┘
```

When the kernel creates an agent process, it:
1. Allocates a PGD page from the kernel pool
2. Copies the kernel portion (TTBR1 entries are the same for all processes)
3. Creates the initial user-space mappings: text, data, heap, stack
4. Assigns an ASID
5. Records the memory limit from the agent manifest (or system default)

```rust
/// Agent process — memory-relevant fields shown here.
/// Full struct includes additional fields for capabilities, IPC channels,
/// CPU quota, space mounts, and manifest (see architecture.md §6.3).
pub struct AgentProcess {
    pub pid: ProcessId,
    pub agent_id: AgentId,
    pub capabilities: CapabilitySet,
    pub address_space: AddressSpace,
    pub memory_limit: usize,           // max RSS in bytes
    pub memory_stats: AgentMemoryStats,
    pub cpu_quota: CpuQuota,
    pub ipc_channels: Vec<ChannelId>,
    pub space_access: Vec<SpaceMount>,
    pub manifest: AgentManifest,
    /// Agent priority from manifest (§8.1).
    /// Used by OOM scorer (§8.1) and thrash detector (§10.6) for victim selection.
    pub priority: AgentPriority,
    /// Whether this agent is currently suspended (e.g., by thrash detector).
    pub suspended: bool,
}

impl AgentProcess {
    pub fn priority(&self) -> AgentPriority { self.priority }
    pub fn is_suspended(&self) -> bool { self.suspended }
}
```

### 5.2 Memory Accounting

Every page allocated to an agent is tracked. The kernel maintains per-agent statistics and enforces limits:

```rust
pub struct AgentMemoryStats {
    /// Resident Set Size — physical pages currently mapped
    pub rss: usize,
    /// Virtual size — total virtual address range mapped
    pub virtual_size: usize,
    /// Private pages — pages owned exclusively by this agent
    pub private_pages: usize,
    /// Shared pages — pages in shared memory regions
    pub shared_pages: usize,
    /// Peak RSS (high-water mark)
    pub peak_rss: usize,
    /// Page faults (total)
    pub page_faults: u64,
    /// Page faults (major — required disk I/O)
    pub major_faults: u64,
    /// Memory limit for this agent
    pub limit: usize,
}

impl AgentMemoryStats {
    /// Check if the agent has exceeded its memory limit
    pub fn is_over_limit(&self) -> bool {
        self.rss > self.limit
    }

    /// Remaining budget before limit
    pub fn remaining(&self) -> usize {
        self.limit.saturating_sub(self.rss)
    }

    /// Major fault rate (faults requiring disk I/O) over the last sampling window.
    /// Used by ThrashDetector to identify agents causing excessive paging.
    /// The sampling window is 1 second, updated on each major fault.
    pub fn major_faults_per_sec(&self) -> f64 {
        let elapsed = Timestamp::now().as_millis() - self.last_sample_time.as_millis();
        if elapsed == 0 { return 0.0; }
        (self.major_faults_in_window as f64) / (elapsed as f64 / 1000.0)
    }
}
```

**Shared page accounting:** When a shared memory region is mapped into two agents, each agent is charged for half the pages. This prevents agents from evading memory limits by hiding allocations in shared regions. The formula: `charged = shared_region_size / participant_count`. If one agent unmaps, the remaining agent absorbs the full cost.

**Model memory is not charged to agents.** Model weights, KV caches, and embedding stores live in the model pool. They are system infrastructure managed by AIRS. Charging model memory to agents would be meaningless — no single agent "owns" the model, and the memory would instantly blow past any reasonable agent limit.

**Accounting is visible.** Per-agent memory stats are exposed through the Inspector and agent cards in the GUI. Users can see exactly how much memory each agent uses.

### 5.3 Memory Limit Enforcement

When an agent's RSS exceeds its memory limit, the kernel does not silently kill it. The enforcement sequence:

```
1. Agent's RSS crosses memory limit
     ↓
2. Kernel sets agent state to Suspended (scheduler.md §3.3 ThreadState::Suspended)
   (agent threads stop executing, no data loss)
     ↓
3. Kernel sends notification to Attention Manager:
   "Agent 'research-assistant' exceeded its 4 MB memory limit (current: 5.2 MB)"
     ↓
4. Attention Manager notifies user with options:
   a) Increase limit (to suggested value based on agent behavior)
   b) Terminate agent (state saved to space best-effort)
   c) Terminate other agents to free memory
     ↓
5. User chooses — or if no response within 30 seconds,
   agent remains paused until user acts
```

The agent is never silently killed except in OOM conditions (section 8). Pausing preserves the agent's state so it can resume if the user increases the limit.

### 5.4 Copy-on-Write

AIOS rarely forks processes (agents are typically spawned fresh from manifests), but COW is used in two cases:

1. **POSIX fork()** — BSD tools call fork(). The child gets a COW copy of the parent's address space. Pages are marked read-only with the COW software bit set. On write, the page fault handler allocates a new page, copies the content, and maps the new page as writable.

2. **Flow object transfer** — when an agent sends a large object through Flow, the kernel maps the object's pages into the receiver's address space with COW semantics. If the receiver only reads the data, no copy occurs. If the receiver writes, it gets a private copy.

```rust
/// Handle a page fault on a COW page.
/// Called from the page fault dispatcher (§5.5) with the faulting address,
/// the original frame from the PTE, the owning process, and the VMA.
fn handle_cow_fault(
    fault_addr: VirtualAddress,
    original_frame: PhysicalFrame,
    process: &mut Process,
    vma: &Vma,
) -> Result<(), FaultError> {
    let addr_space = &mut process.address_space;
    let pte = addr_space.lookup_pte(fault_addr)?;

    if !pte.is_cow() {
        return Err(FaultError::AccessViolation);
    }

    let old_frame = original_frame;
    let new_frame = FRAME_ALLOCATOR
        .alloc_page(Pool::User)
        .ok_or(FaultError::OutOfMemory)?;

    // Copy page content
    unsafe {
        core::ptr::copy_nonoverlapping(
            old_frame.as_ptr::<u8>(),
            new_frame.as_mut_ptr::<u8>(),
            PAGE_SIZE,
        );
    }

    // Update PTE: new frame, writable, no longer COW
    let mut new_pte = *pte;
    new_pte.set_frame(new_frame);
    new_pte.set_writable();
    new_pte.clear_cow();
    addr_space.update_pte(fault_addr, new_pte);

    // Decrement refcount on old frame; free if zero
    if FRAME_REFCOUNT.decrement(old_frame) == 0 {
        FRAME_ALLOCATOR.free_pages(old_frame, 0);
    }

    Ok(())
}
```

-----

## 6. Model Memory (AIRS)

### 6.1 The Problem

On target hardware, AI model memory dominates everything else:

```
Memory budget on a 4 GB Raspberry Pi 5:

Total RAM:                              4096 MB
  - Kernel:                              256 MB
  - Reserved (firmware tables, MMIO):    128 MB
  - DMA pool:                            128 MB
  - User pool (OS services, agents,     1536 MB
    heap, browser, headroom):
  ─────────────────────────────────────────────
  Available for model:              2048 MB (2 GB)

Llama 3.1 8B at Q4_K_M:               ~4500 MB  ← does not fit
Llama 3.1 8B at Q3_K_S:               ~3200 MB  ← does not fit
Phi-3 Mini 3.8B at Q4_K_M:            ~2300 MB  ← does not fit
Phi-3 Mini 3.8B at Q4_K_M + KV cache: ~2700 MB  ← does not fit
TinyLlama 1.1B at Q4_K_M:             ~700 MB   ← fits
Phi-2 2.7B at Q4_K_M:                 ~1800 MB  ← fits

On a 2 GB device (model pool is 0 — see §2.4):
  Available for model:                  0 MB (cloud inference only)
  All 1.75 GB (after kernel/DMA/reserved) is user pool
```

The model IS the memory problem. Traditional OS memory management — where everything is fungible and swappable — does not work here. Model weights must stay in RAM. Swapping 3 GB of model data to an SD card would take tens of seconds and make inference unusable.

### 6.2 Model Memory Region

Model weights are loaded into the model pool — a dedicated region of physical memory that is pinned (never paged out), uses 2 MB huge pages (to reduce TLB pressure), and is mapped read-only into the AIRS process.

```rust
/// A loaded model's memory region
pub struct ModelMemoryRegion {
    /// Physical frames backing this model (2 MB huge pages)
    frames: Vec<PhysicalFrame>,
    /// Total size in bytes
    size: usize,
    /// Reference count (multiple sessions can share weights)
    refcount: AtomicUsize,
    /// Model identity
    model_id: ModelId,
    /// Virtual address mapped in AIRS process
    vaddr: VirtualAddress,
}

/// Mapping configuration for model memory
pub struct ModelMapping {
    /// Physical base
    phys_base: PhysicalAddress,
    /// Virtual base in AIRS address space
    virt_base: VirtualAddress,
    /// Size (2 MB aligned)
    size: usize,
    /// Flags: read-only, shared, pinned, huge pages
    flags: VmFlags,
}

impl ModelMapping {
    pub fn new(region: &ModelMemoryRegion) -> Self {
        Self {
            phys_base: region.frames[0].address(),
            virt_base: region.vaddr,
            size: region.size,
            flags: VmFlags::READ | VmFlags::SHARED | VmFlags::PINNED | VmFlags::HUGE,
        }
    }
}
```

**Why huge pages for models:** A 4 GB model mapped with 4 KB pages requires 1,048,576 page table entries and the same number of TLB entries. The TLB on a Cortex-A76 (Pi 5) has ~1280 entries — hopeless. With 2 MB huge pages, the same model needs only 2048 TLB entries. Still more than the TLB can hold, but the miss rate is dramatically lower because each entry covers 512x more memory.

**Why pinned:** Model weights are read-only after loading. They are never written, so they are never dirty, so there is nothing to write back to disk. Evicting them from RAM saves nothing — they would just need to be reloaded from storage. Pinning prevents the page reclamation system from touching model memory.

**Reference counting:** When multiple inference sessions (conversation bar, space indexer, intent verifier) all use the same model, they share the same physical memory region. The refcount tracks how many sessions hold a reference. The model is evicted only when the refcount drops to zero AND memory pressure requires it.

#### 6.2.1 Model Page Pinning and Inference Safety

Model pages are pinned for their **entire resident lifetime**, not just during active inference:

```rust
impl ModelMemoryRegion {
    /// Model frames are pinned from the moment they are loaded
    /// until the model is explicitly evicted. They are NEVER on
    /// the free list. Page reclamation cannot touch them.
    ///
    /// Pinning invariants:
    /// 1. All frames have VmFlags::PINNED set at allocation time
    /// 2. Pinned frames are excluded from the page reclaimer's scan
    /// 3. The model pool pressure calculation ignores pinned pages
    ///    (model pool pressure = 0 when all pages are model weights)
    /// 4. Dynamic pool resizing (§12.2) cannot reclaim pinned frames
    ///    — it only moves the pool boundary using FREE pages
    /// 5. Eviction requires refcount == 0 (no active sessions)
    ///    AND an explicit eviction decision by AIRS or the kernel
    ///
    /// During active inference, model pages are accessed read-only.
    /// Since they are pinned, mapped read-only, and shared:
    /// - No page fault can occur (pages are always resident)
    /// - No eviction can occur (pinned frames are never reclaimed)
    /// - No corruption can occur (read-only mapping, W^X enforced)
    /// - No concurrent modification is possible (no writer exists)
    pub fn is_safe_for_inference(&self) -> bool {
        self.frames.iter().all(|f| f.flags().contains(VmFlags::PINNED))
            && self.refcount.load(Ordering::Acquire) > 0
    }
}
```

**Inference-critical invariant:** An LLM inference session that begins with a loaded model will never observe model page eviction, corruption, or stale data, regardless of concurrent memory pressure on the user pool. This is guaranteed by three properties:
1. Model pages are pinned (never reclaimable by the page reclaimer)
2. Model pages are read-only (no writer can modify weights during inference)
3. Model eviction requires refcount == 0 (impossible while any session is active)

**Interaction with dynamic pool resizing (§12.2):** When AIRS resource orchestration resizes the model pool / user pool boundary, only FREE pages participate. Model weight pages have nonzero refcounts and the `PINNED` flag — they are never on the free list and cannot be moved by pool boundary adjustment. The `security_floor` invariant (§12.2) additionally prevents the pool from shrinking below the primary model's footprint while security services are active.

### 6.3 KV Cache Management

KV caches are the per-session cost of maintaining conversation context. Unlike model weights (which are static and shared), KV caches are dynamic, per-session, and can grow large:

```
KV cache size ≈ 2 × num_layers × head_dim × num_kv_heads × context_length × sizeof(f16)

Llama 3.1 8B:
  32 layers × 128 head_dim × 8 kv_heads × 8192 context = ~1 GB at f16
  With Q8 quantization: ~512 MB
  With Q4 quantization: ~256 MB
```

AIOS uses **PagedAttention** — a technique pioneered by vLLM that manages KV caches as non-contiguous fixed-size blocks mapped through a block table, analogous to virtual memory page tables. Traditional KV cache allocation pre-reserves contiguous memory for the maximum context length, wasting 60-80% of model pool memory on empty slots. PagedAttention allocates blocks on demand as tokens are generated, reducing waste to under 4%.

```rust
/// KV cache for a single inference session, using PagedAttention.
/// Blocks are allocated on demand — no pre-reservation of max context.
pub struct KvCache {
    /// Session owning this cache
    pub session: SessionId,
    /// Block table: logical block index → physical block.
    /// Analogous to a page table mapping virtual → physical pages.
    /// Grows dynamically as context length increases.
    pub block_table: Vec<Option<KvBlockId>>,
    /// Current context length (tokens stored)
    pub context_length: u32,
    /// Maximum context length (model limit)
    pub max_context: u32,
    /// Total bytes currently allocated (not reserved)
    pub allocated_bytes: usize,
    /// Last time this cache was used
    pub last_used: Timestamp,
    /// Priority for eviction ordering
    pub priority: CachePriority,
    /// Prefix sharing: if this cache shares a prefix with another session,
    /// the shared blocks are COW (copy-on-write) — only divergent blocks
    /// are independently allocated.
    pub shared_prefix: Option<SharedPrefix>,
}

/// Fixed-size block in the KV cache.
/// Each block holds KV data for a fixed number of token positions.
pub struct KvCacheBlock {
    /// Unique block ID in the model pool
    id: KvBlockId,
    /// Physical frame(s) backing this block (may use medium 64 KB pages)
    frames: [PhysicalFrame; FRAMES_PER_KV_BLOCK],
    /// Number of token positions stored in this block
    tokens_stored: u32,
    /// Capacity: token positions per block
    tokens_capacity: u32,
    /// Reference count: >1 when shared via prefix caching
    refcount: AtomicU32,
}

/// Prefix sharing between sessions with common system prompts or context.
/// When two sessions share the first N tokens (e.g., same system prompt),
/// their KV blocks for those tokens are shared via COW.
pub struct SharedPrefix {
    /// Source session whose blocks we share
    source: SessionId,
    /// Number of shared blocks (from block 0 to shared_blocks-1)
    shared_blocks: u32,
    /// Shared blocks are read-only. If this session modifies a shared
    /// block (e.g., due to positional encoding differences), it COWs:
    /// allocate a new block, copy data, update block_table entry.
}

pub enum CachePriority {
    /// User actively waiting (conversation bar)
    Interactive,
    /// System service (intent verification, context engine)
    System,
    /// Background work (space indexing)
    Background,
}

/// Block sizing: 16 token positions per block at the default.
/// For Llama 3.1 8B (32 layers, 8 KV heads, 128 head_dim, Q8):
///   Per-token KV size = 2 × 32 × 8 × 128 × 1 byte = 64 KB
///   Block size = 16 tokens × 64 KB = 1 MB per block
/// Backed by 64 KB medium THP pages for efficient TLB usage.
const KV_TOKENS_PER_BLOCK: u32 = 16;
const KV_BLOCK_SIZE: usize = 1 * MB; // 1 MB blocks (model-dependent)
const KV_MEDIUM_PAGE_SIZE: usize = 64 * KB; // 64 KB medium THP
const FRAMES_PER_KV_BLOCK: usize = KV_BLOCK_SIZE / KV_MEDIUM_PAGE_SIZE; // 16 medium pages per block
```

**PagedAttention memory savings:**

```
Scenario: 4 concurrent sessions, 8K max context, 8B model (Q8 KV)

Traditional (contiguous pre-allocation):
  Per session: 8192 tokens × 64 KB/token = 512 MB reserved
  4 sessions: 2048 MB reserved
  Actual usage (avg 2K tokens used): 512 MB
  Waste: 1536 MB (75%)

PagedAttention (on-demand blocks):
  Per session: only allocated blocks for actual tokens
  4 sessions at avg 2K tokens: 4 × 128 MB = 512 MB allocated
  Waste: < 20 MB (partially-filled last blocks)
  Savings: 1516 MB freed for other use

On an 8 GB device with 4 GB model pool:
  Traditional: 2 GB KV + 2.5 GB model weights = exceeds pool
  PagedAttention: 512 MB KV + 2.5 GB model weights = 3 GB, fits with 1 GB headroom
```

**Prefix caching — cross-session KV sharing:**

When multiple sessions use the same system prompt (common: conversation bar, intent verifier, and behavioral monitor all share AIOS system prompts), their KV cache blocks for those tokens are identical. PagedAttention enables sharing:

```
Session A (conversation bar):  [system prompt KV | user context A KV]
Session B (intent verifier):   [system prompt KV | user context B KV]
Session C (behavioral monitor):[system prompt KV | user context C KV]

Without prefix sharing:  3 × 200 tokens × 64 KB = 38.4 MB for system prompts
With prefix sharing:     1 × 200 tokens × 64 KB = 12.8 MB (shared via COW)
Savings: 25.6 MB — significant when model pool is 2-4 GB
```

**KV cache eviction** follows priority ordering when the model pool is under pressure. **Important:** KV cache "eviction" means **deallocation back to the model pool free list** — not MGLRU-based page reclamation. KV cache blocks live in the pinned model pool, which is excluded from MGLRU tracking entirely. MGLRU governs user pool pages (agent heaps, page cache, shared memory). The KV cache eviction policy below is a separate, AIRS-driven mechanism:

```
Eviction order (first evicted → last evicted):
1. Background session KV caches (space indexing, metadata generation)
2. System session KV caches (intent verifier, behavioral monitor)
3. Idle interactive session KV caches (conversation bar idle > 5 min)
4. Active interactive session KV caches (never evicted — inference fails instead)

Within a priority level, partially-filled blocks are evicted first (least
tokens stored = least re-computation cost to reconstruct).
```

**DAMON integration for KV caches:** While MGLRU does not track model pool pages, DAMON (§10.9) can still monitor access patterns on KV cache memory regions. DAMON detects when a KV cache transitions from active (inference in progress) to idle (session waiting), enabling AIRS to make proactive eviction decisions. DAMON reports access frequency; AIRS decides whether to evict; the kernel executes the deallocation. The feedback path is: DAMON → AIRS resource orchestration → kernel KV cache eviction → model pool free list.

When a KV cache is evicted, the session's conversation history is still in a space object. The cache can be reconstructed by re-processing the conversation — slower than keeping it in RAM, but not data-losing. With prefix caching, reconstruction is faster: only the session-specific suffix needs recomputation; the shared prefix blocks may still be resident from another session.

### 6.4 Model Loading and Eviction

Models are loaded from space storage into the model pool. AIOS uses **userfaultfd-based lazy loading** — a technique that enables inference to begin before the entire model is resident in RAM. Instead of blocking until all model pages are faulted in, the kernel registers a userfaultfd handler that loads pages on demand with intelligent prefetch, allowing the first inference to start within seconds even for multi-GB models on SD card storage.

```
Model loading flow (userfaultfd lazy loading):

1. AIRS requests model load: model_id = "phi-3-mini-q4"
     ↓
2. Kernel allocates virtual address range in model pool (2 MB huge page aligned)
   — physical pages NOT yet allocated (lazy)
     ↓
3. Register userfaultfd handler for the model region:
   - Handler knows the GGUF file layout (tensor offsets)
   - Handler reads from space storage on page fault
     ↓
4. AIRS maps the region read-only into its address space
     ↓
5. Inference can start IMMEDIATELY:
   - First token access faults in the embedding layer weights (~50 MB)
   - Subsequent layers are faulted in as inference progresses
   - Prefetch thread reads ahead: if layer N is accessed, prefetch layers N+1, N+2
     ↓
6. Background prefetch continues loading remaining layers:
   - Prioritizes layers in inference order (embedding → attention → FFN)
   - Uses low-priority I/O to avoid blocking active inference faults
     ↓
7. After full warmup (all pages resident), inference runs at full speed
   - userfaultfd handler is detached (no further overhead)
   - Pages are pinned with VmFlags::PINNED
```

```rust
/// Lazy model loader using userfaultfd
pub struct LazyModelLoader {
    /// userfaultfd file descriptor for this model region
    uffd: UserfaultFd,
    /// Model region virtual address range
    region: VirtualRange,
    /// GGUF file handle in space storage
    gguf_file: SpaceObjectHandle,
    /// Tensor layout: maps virtual offset → GGUF file offset
    tensor_map: Vec<TensorMapping>,
    /// Prefetch state
    prefetch: PrefetchState,
    /// Pages loaded so far
    pages_loaded: AtomicUsize,
    /// Total pages needed
    pages_total: usize,
}

pub struct TensorMapping {
    /// Virtual offset within model region
    vaddr_offset: usize,
    /// Offset within GGUF file
    file_offset: usize,
    /// Size in bytes
    size: usize,
    /// Layer index (for prefetch ordering)
    layer: u32,
}

pub struct PrefetchState {
    /// Last layer accessed by inference
    last_accessed_layer: AtomicU32,
    /// Prefetch window: how many layers ahead to read
    window: u32,           // default: 2 layers ahead
    /// Prefetch thread handle
    thread: Option<JoinHandle<()>>,
}

impl LazyModelLoader {
    /// Handle a page fault in the model region
    fn handle_fault(&self, addr: VirtualAddress) -> Result<(), FaultError> {
        let offset = addr.0 - self.region.start.0;
        let tensor = self.tensor_map.iter()
            .find(|t| offset >= t.vaddr_offset && offset < t.vaddr_offset + t.size)
            .ok_or(FaultError::UnmappedRegion)?;

        // Read the faulted page from space storage
        let page_offset = offset & !(PAGE_SIZE_2MB - 1); // 2 MB aligned
        let file_offset = tensor.file_offset + (page_offset - tensor.vaddr_offset);
        let data = self.gguf_file.read_at(file_offset, PAGE_SIZE_2MB)?;

        // Install the page via userfaultfd UFFDIO_COPY
        self.uffd.copy(addr, &data)?;
        self.pages_loaded.fetch_add(1, Ordering::Relaxed);

        // Signal prefetch thread: advance if needed
        self.prefetch.last_accessed_layer.store(tensor.layer, Ordering::Relaxed);

        Ok(())
    }
}
```

**Why userfaultfd instead of plain demand paging?** Standard demand paging (mmap + page fault) works but has no awareness of model structure. A page fault in the middle of a tensor triggers a single 4 KB/2 MB read. With userfaultfd, the fault handler knows the GGUF layout — it can prefetch entire tensors and prioritize layers that inference will access next. On SD card storage where sequential reads are 10x faster than random reads, this prefetch intelligence reduces model loading time by 40-60%.

**First-token latency improvement:**

```
Loading a 4.5 GB model (Llama 3.1 8B Q4_K_M) from SD card:

Traditional (load all, then start):
  Load time: 45 seconds (100 MB/s sequential read)
  First token: 45 seconds

userfaultfd lazy loading:
  Embedding layer fault-in: ~2 seconds (first ~100 MB)
  First token: ~3 seconds (embedding + first attention layer)
  Full warmup (background): ~45 seconds

Time to first token: 3s vs 45s (15x faster)
```

This matters enormously for user experience. When the user opens the conversation bar, they expect a response in seconds, not a 45-second wait for model loading. Lazy loading with userfaultfd makes AIRS responsive immediately — the first few layers are enough to begin generating tokens. Inference quality is identical; only the first few tokens have slightly higher latency (page faults during layer traversal). By the time the user reads the first sentence, the full model is resident.

```rust
/// Policy for model eviction when pool is full
pub struct ModelEvictionPolicy {
    /// Currently loaded models ordered by last use time
    loaded: Vec<LoadedModel>,
}

pub struct LoadedModel {
    pub model_id: ModelId,
    pub region: ModelMemoryRegion,
    pub last_used: Timestamp,
    pub active_sessions: usize,
}

impl ModelEvictionPolicy {
    /// Select a model to evict (returns None if no model can be evicted)
    pub fn select_victim(&self) -> Option<ModelId> {
        // Never evict a model with active interactive sessions
        // Prefer evicting models with zero sessions
        // Among those, evict least recently used
        self.loaded.iter()
            .filter(|m| m.active_sessions == 0)
            .min_by_key(|m| m.last_used)
            .map(|m| m.model_id)
    }
}
```

**On 2 GB devices:** No local model is loaded. The model pool is zero. All inference is routed to cloud endpoints via the NTM. This eliminates the memory pressure that model weights would cause on a 2 GB system.

**On 4 GB devices:** Only one small model (1-3B at Q4) fits at a time. Model switching requires full eviction and reload — an operation that takes several seconds from SD card storage. AIRS avoids unnecessary model switches by routing all task types to the single loaded model.

**On 8 GB devices:** A large model (8B Q4) and an embedding model can coexist simultaneously. Model switching is rare. The model pool has enough headroom for generous KV caches.

-----

## 7. Shared Memory and IPC

### 7.1 Shared Memory Regions

Shared memory enables zero-copy IPC. When an agent needs to transfer large data to a service (or to another agent), it writes the data into a shared memory region and sends the region ID over the IPC channel. The receiver maps the same physical pages into its own address space.

```rust
/// A shared memory region managed by the kernel.
/// Canonical definition — must match ipc.md §4.5.
pub struct SharedMemoryRegion {
    pub id: SharedMemoryId,
    /// Physical frames backing this region (contiguous page range)
    pub physical_pages: PageRange,
    /// Reference count: incremented on map, decremented on unmap or
    /// process death. Physical pages freed when count reaches 0.
    pub ref_count: AtomicU32,
    /// The process that created this region
    pub creator: ProcessId,
    /// Maximum permissions granted at creation time
    pub max_flags: MemoryFlags,
    /// Capability required to access
    pub capability: CapabilityTokenId,
    /// Per-mapping permissions (bounded; MAX_SHARED_MAPPINGS = 8)
    pub mappings: [Option<SharedMapping>; MAX_SHARED_MAPPINGS],
}

pub struct SharedMapping {
    pub process: ProcessId,
    pub vaddr: VirtualAddress,
    pub flags: VmFlags,  // may be more restrictive than max_flags
}
```

Creation flow:

```
Agent A wants to share 1 MB with Agent B:

1. Agent A: syscall SharedMemoryCreate { size: 1 MB }
   → Kernel allocates frames from user pool
   → Kernel maps into Agent A at 0x1_0000_0000
   → Returns SharedMemoryId and CapabilityTokenId

2. Agent A: writes data to shared region (direct memory access)

3. Agent A: syscall SharedMemoryShare { region, channel_to_B, flags: READ }
   → Kernel verifies A holds the capability
   → Kernel creates a read-only mapping capability for B
   → Transfers capability to B over the IPC channel

4. Agent B: syscall SharedMemoryMap { region, flags: READ }
   → Kernel verifies B holds the received capability
   → Kernel maps the SAME physical frames into B at 0x1_0000_0000
   → B can now read the data directly — no copy

5. When done: either agent calls SharedMemoryUnmap
   → Kernel unmaps from that agent's address space
   → When all mappings removed, frames freed
```

Both agents access the same physical memory. The kernel enforces that the receiver's mapping flags are at most as permissive as what the sender granted. If the sender shares as read-only, the receiver cannot write.

**Stability during pool boundary resizing:** Shared memory regions are allocated from the user pool. When AIRS resource orchestration resizes the model pool / user pool boundary (see [airs.md §10](../intelligence/airs.md), [security.md §9](../security/security.md)), shared memory physical frames are **never reclaimed or relocated**:

```rust
impl DynamicModelPool {
    /// When shrinking the user pool (growing model pool), the kernel
    /// can only reclaim FREE pages from the user pool. Pages that are:
    ///   - Mapped by any agent (including shared memory)
    ///   - Pinned for DMA
    ///   - Part of an active page cache entry
    /// are NOT eligible for reclamation. The kernel moves the pool
    /// boundary only as far as free pages allow.
    ///
    /// Shared memory frames have refcount >= 2 (multiple mappers).
    /// They are never on the free list. Pool resizing cannot touch them.
    pub fn shrink_user_pool(&self, target_delta: usize) -> usize {
        let mut reclaimed = 0;
        for frame in self.user_pool.free_list() {
            if reclaimed >= target_delta { break; }
            // Only FREE frames — never mapped, pinned, or shared
            self.transfer_to_model_pool(frame);
            reclaimed += frame.size;
        }
        reclaimed  // may be less than target_delta if not enough free frames
    }
}
```

Pool boundary resizing operates exclusively on the **free page list**. Shared memory regions are backed by physical frames that are mapped into at least one (usually two or more) agent address spaces. These frames have nonzero reference counts and are never on the free list. There is no mechanism by which pool resizing can fragment, relocate, or reclaim shared memory. The physical frames backing shared regions are stable for their entire lifetime, regardless of pool boundary movement.

If AIRS requests a pool resize larger than the available free pages, the resize is partially fulfilled — the boundary moves as far as free pages allow, and the remaining shortfall is logged as a resource pressure event. This prevents pool resizing from evicting active mappings to satisfy the request.

### 7.2 Memory-Mapped Space Objects

Space objects can be memory-mapped into an agent's address space, avoiding the overhead of IPC read calls for large objects (images, documents, model files):

```rust
/// Memory-map a space object into the calling agent's address space
pub fn map_space_object(
    space: SpaceId,
    object: ObjectId,
    flags: VmFlags,
) -> Result<VirtualAddress, MapError> {
    // 1. Verify agent holds ReadSpace(space) capability
    // 2. Resolve object to physical storage blocks
    // 3. Create VmRegion of kind MappedObject
    // 4. Map pages (demand-paged — not loaded until accessed)
    // 5. Return virtual address
}
```

Immutable objects (most space content) are mapped read-only and shared across any agents that map them — same physical pages, multiple virtual mappings. If an agent needs to modify the content, it gets a COW mapping: reads see the shared pages, writes trigger a page fault that allocates private copies.

#### 7.2.1 Page Fault Re-Verification

When an agent accesses a mapped space object page that is not currently resident (evicted during memory pressure, or not yet demand-paged), the page fault handler **re-verifies the agent's capability** before loading the page:

```rust
/// Page fault handler for MappedObject regions.
/// Called by the kernel when an agent accesses a non-resident page
/// in a VmRegion of kind MappedObject.
fn handle_mapped_object_fault(
    agent: AgentId,
    region: &VmRegion,
    fault_addr: VirtualAddress,
) -> Result<PhysicalFrame, FaultError> {
    let space = region.source_space();
    let object = region.source_object();

    // Step 1: Re-verify capability.
    // The agent may have had its ReadSpace token revoked since the
    // initial map_space_object() call. A revoked capability means
    // the agent no longer has the right to read this data.
    if !capability_table.check(agent, Capability::ReadSpace(space)) {
        // Capability revoked — unmap the entire region.
        // Agent receives SIGSEGV (or AIOS equivalent).
        unmap_region(agent, region);
        return Err(FaultError::CapabilityRevoked);
    }

    // Step 2: Load page through Space Storage read path.
    // This includes checksum verification and decryption.
    let frame = space_storage.read_page(space, object, fault_addr.page_offset())?;

    // Step 3: Map the frame into the agent's address space.
    map_page(agent, fault_addr, frame, region.flags());

    Ok(frame)
}
```

**Why re-verify on every fault:** The initial `map_space_object()` call establishes a virtual mapping, but pages are demand-loaded. Between the initial map and a page fault, the agent's capability may have been revoked (user removed the agent's access via Inspector, capability expired, cascade revocation from parent token). Without re-verification, a revoked agent could continue reading data from pages that happen to fault in — the mapping itself would be a stale privilege.

**When capability is revoked:** If the check fails, the kernel unmaps the entire `MappedObject` region from the agent's address space. The agent receives a fault signal. The provenance chain records the denied access. This is the same behavior as a denied `SpaceRead` syscall — just triggered by a page fault instead of an explicit read.

**Performance:** The capability re-check is O(1) in the kernel `CapabilityTable` — a hash lookup, not an IPC round-trip. It adds ~50 ns to a page fault that already costs ~100 μs (SD card read) or ~5 μs (NVMe). The security cost is negligible.

-----

## 8. Memory Pressure and OOM

### 8.1 Memory Pressure Levels

The frame allocator continuously tracks free page counts across all pools. Pressure levels are computed from the user pool (model pool is pinned and excluded from pressure calculations):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPressure {
    /// > 20% free pages in user pool — normal operation
    Normal,
    /// 11-20% free — start background reclamation
    Low,
    /// 5-10% free — aggressive reclamation, suspend background agents
    Critical,
    /// < 5% free — OOM killer engages
    Oom,
}
```

```
Pressure response table:

Level     Free %    Actions
────────  ──────    ──────────────────────────────────────────────────
Normal    > 20%     None — system operates normally

Low       11-20%    - Reclaim clean page cache pages
                    - Compress inactive agent pages (zram)
                    - Notify AIRS to evict background KV caches
                    - Zero-page thread paused (save CPU)

Critical  5-10%     - Suspend all background agents
                    - Evict ALL non-interactive KV caches
                    - Compress all idle session pages
                    - Notify user: "System low on memory"

OOM       < 5%      - OOM killer selects victim agent
                    - Notify user before killing
                    - Save victim state to space (best effort)
                    - Kill victim, reclaim all its pages
```

### 8.2 OOM Killer

When physical memory is exhausted and reclamation has failed, the OOM killer terminates an agent to reclaim memory. The selection algorithm is priority-based:

```rust
pub struct OomPolicy {
    /// Agents that must never be killed
    protected: Vec<AgentId>,
    /// Priority ordering for kill selection
    priority: OomPriority,
}

pub enum OomPriority {
    /// Kill the agent using the most memory with the lowest priority
    LowestPriorityLargestMemory,
}

/// Agent scheduling/OOM priority level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentPriority {
    /// Kernel-critical services (compositor, service manager)
    Critical,
    /// Core OS services (space storage, network)
    System,
    /// User-facing agents with active sessions
    Normal,
    /// Inactive or suspended agents
    Background,
}

/// Protected agents (never killed by OOM):
/// - Kernel threads
/// - Service Manager
/// - Compositor
/// - Conversation bar service
/// - Space Storage service
/// - AIRS core (model memory is in a separate pool anyway)

impl OomPolicy {
    pub fn select_victim(&self, agents: &[AgentProcess]) -> Option<ProcessId> {
        agents.iter()
            .filter(|a| !self.protected.contains(&a.agent_id))
            .max_by_key(|a| self.score(a))
            .map(|a| a.pid)
    }

    /// Higher score = more likely to be killed
    fn score(&self, agent: &AgentProcess) -> u64 {
        let memory_score = agent.memory_stats.rss as u64;
        let priority_multiplier = match agent.priority() {
            AgentPriority::Background => 4,
            AgentPriority::Normal     => 2,
            AgentPriority::System     => 1,
            AgentPriority::Critical   => 0, // never killed
        };
        memory_score * priority_multiplier
    }
}
```

**OOM kill sequence:**

```
1. OOM condition detected (free pages < 5%)
     ↓
2. OOM killer selects victim: lowest priority × largest memory
     ↓
3. Notification sent to user:
   "Low memory. Terminating 'research-assistant' (using 12 MB).
    Agent state will be saved."
     ↓
4. Agent receives SIGTERM-equivalent (5 second grace period)
     ↓
5. Agent state saved to space (conversation history, partial work)
     ↓
6. After 5 seconds (or agent exits): force terminate
     ↓
7. All agent pages reclaimed immediately
     ↓
8. If still OOM: repeat from step 2 with next victim
```

The OOM killer is a last resort. The pressure-level system (section 8.1) catches most memory issues before OOM. In normal operation, background KV cache eviction and agent suspension provide enough reclamation to avoid killing anything.

-----

## 9. ARM Security Features

### 9.1 W^X (Write XOR Execute)

Every page in the system is either writable or executable, never both. This prevents the most common class of exploitation — injecting code into a writable buffer and then executing it.

**Implementation:** The `PageTableEntry` API enforces W^X at the lowest level. `set_writable()` clears the executable bit. `set_executable()` sets read-only. There is no `set_writable_and_executable()`.

**JIT compilation (SpiderMonkey in the browser):** JIT compilers generate machine code at runtime and need to write it to memory, then execute it. AIOS handles this with a two-step mapping:

```
1. JIT compiler allocates writable memory: mmap(RW-)
2. JIT compiler writes generated code to the pages
3. JIT compiler calls mprotect(R-X) — remap as executable, non-writable
4. JIT compiler cannot modify the code without another mprotect cycle
```

The kernel tracks mprotect transitions in the audit log. Frequent W→X transitions from a non-browser agent would be flagged by the behavioral monitor.

### 9.2 PAC (Pointer Authentication)

ARM Pointer Authentication adds a cryptographic signature to pointers stored in memory. Return addresses on the stack are signed on function entry and verified on function return. A corrupted return address (from a buffer overflow or ROP chain) fails verification and triggers a fault.

```
Function entry:           Function return:
  PACIASP                    AUTIASP
  (sign LR with key A,      (verify LR with key A,
   SP as context)             SP as context)
  STR LR, [SP, #-16]!       LDR LR, [SP], #16
  ...function body...        RET
```

**Per-process keys:** Each process gets its own PAC key, stored in system registers (`APIAKeyLo_EL1`, `APIAKeyHi_EL1`). The key is inaccessible from EL0 (userspace). An attacker who compromises one agent cannot forge pointers for another agent — the keys are different.

**Kernel PAC:** The kernel uses a separate key loaded at boot. Kernel function return addresses are PAC-protected.

### 9.3 BTI (Branch Target Identification)

ARM BTI marks valid indirect branch targets with a `BTI` instruction. Indirect branches (register jumps, function pointer calls) that land on a non-BTI instruction trigger a fault. This prevents Jump-Oriented Programming (JOP) attacks where an attacker chains together existing code snippets via indirect jumps.

```
Valid function entry point:
    BTI c                    ← valid target for indirect call (BLR)
    PACIASP
    ...

Invalid landing site:
    ADD X0, X1, X2           ← NOT a BTI instruction
    ...                         indirect branch here → fault
```

**Toolchain support:** The Rust compiler and LLVM toolchain emit BTI instructions for all function entries when the target supports it. The kernel sets the BTI enforcement bit in page table entries for executable pages.

### 9.4 MTE (Memory Tagging Extension)

MTE assigns a 4-bit tag to every 16-byte granule of memory and to every pointer. When a pointer is dereferenced, the hardware checks that the pointer's tag matches the memory's tag. A mismatch raises a fault — detecting use-after-free, buffer overflow, and other memory corruption bugs.

```
Memory tags (4 bits, stored in physical memory metadata):

  Address:  0x1000   0x1010   0x1020   0x1030   0x1040
  Tag:       [3]      [3]      [3]      [7]      [7]
              ▲                          ▲
              │                          │
         malloc(48) returns          malloc(32) returns
         ptr with tag 3             ptr with tag 7

  Access via ptr_tag_3 to 0x1030 → tag mismatch → fault
  (buffer overflow detected)

  After free(ptr_tag_3):
  Address:  0x1000   0x1010   0x1020   0x1030   0x1040
  Tag:       [11]     [11]     [11]     [7]      [7]
              ▲
              │
         tag randomized on free

  Access via stale ptr_tag_3 to 0x1000 → tag mismatch → fault
  (use-after-free detected)
```

**Probabilistic detection:** With 4 bits, there are 16 possible tags. A random tag collision (attacker guesses correctly) has a 1/16 probability. For security-critical allocations, the kernel re-tags on every free, making persistent exploits impractical.

```rust
/// MTE configuration for agent heap allocations
pub struct MteConfig {
    /// Enable MTE for this agent's heap
    pub enabled: bool,
    /// Synchronous (precise fault) or asynchronous (batched check)
    pub mode: MteMode,
}

pub enum MteMode {
    /// Fault immediately on tag mismatch — precise, slower
    Synchronous,
    /// Check asynchronously — less precise, faster
    Asynchronous,
}
```

**MTE is enabled for agent heap allocations starting in Phase 13 (Security Hardening).** Kernel heap allocations use MTE in synchronous mode for maximum safety. Agent heaps default to asynchronous mode for performance, with synchronous mode available for debugging.

### 9.5 Guard Pages

Guard pages are unmapped virtual memory regions placed between sensitive areas. Any access to a guard page triggers an immediate page fault, which the kernel handles as a clean error rather than allowing silent corruption.

```
Agent address space with guard pages:

0x0000_0000_0000_0000  ┌────────────────┐
                        │ GUARD (unmapped)│  ← NULL pointer dereference → fault
0x0000_0000_0010_0000  ├────────────────┤
                        │ Agent text      │
0x0000_0000_0040_0000  ├────────────────┤
                        │ GUARD           │  ← text/data boundary
0x0000_0000_0040_1000  ├────────────────┤
                        │ Agent data      │
                        │ Agent heap      │
                        │      ...        │
                        │ Heap top        │
0x0000_xxxx_xxxx_xxxx  ├────────────────┤
                        │ GUARD           │  ← heap/shared boundary
                        │      ...        │
0x0000_0001_0000_0000  ├────────────────┤
                        │ Shared memory   │
                        │      ...        │
                        ├────────────────┤
                        │ GUARD           │  ← shared/stack gap
                        │      ...        │
0x0000_7FFF_FFC0_0000  ├────────────────┤
                        │ Stack           │
                        │ (grows down)    │
                        │      ...        │
0x0000_7FFF_FFBF_F000  ├────────────────┤
                        │ GUARD           │  ← stack overflow → fault, not corruption
0x0000_7FFF_FFBF_E000  └────────────────┘
```

Stack overflow is the most common case. Without a guard page, a stack overflow silently writes into adjacent memory (heap or other data), causing corruption that may not be detected until much later. With a guard page, the overflow triggers an immediate, clean page fault. The kernel terminates the offending thread with a clear error message.

### 9.6 Speculative Execution Mitigations

The Cortex-A76 (Pi 5) is affected by Spectre variant 1 (bounds check bypass), variant 2 (branch target injection), and variant 4 (speculative store bypass). Model weights in the shared model pool are a high-value speculative side-channel target — a malicious agent could potentially leak model data through speculative execution. AIOS applies the following hardware and software mitigations:

```
Vulnerability     Mitigation                           Mechanism
─────────────     ──────────                           ─────────
Spectre v1        CSDB barriers after bounds checks    Compiler inserts CSDB (Consumption of
(bounds bypass)   in kernel syscall paths              Speculative Data Barrier) after array
                                                       index validation. Prevents speculative
                                                       loads past bounds checks.

Spectre v2        CSV2 (Cache Speculation Variant 2)   Cortex-A76 implements CSV2: branch
(branch target    hardware hardening + SMCCC           predictors are context-aware and do not
injection)        firmware interface                    use predictions from other contexts.
                                                       The kernel verifies CSV2 support at boot
                                                       via SMCCC and falls back to software
                                                       retpoline-equivalent if absent.

Spectre v4        SSBS (Speculative Store Bypass Safe) The kernel sets PSTATE.SSBS = 0 on
(store bypass)    bit in PSTATE on kernel entry        kernel entry (disabling speculative
                                                       store bypass for kernel code).
                                                       Agents run with SSBS = 1 (speculative
                                                       stores allowed — performance sensitive).

Meltdown          Not applicable on Cortex-A76         ARM's Cortex-A76 and later cores are
                                                       not affected by Meltdown (CVE-2017-5754).
                                                       Kernel/user page table isolation is NOT
                                                       required (unlike some x86 processors).
```

**Model pool side-channel hardening:** The model pool is mapped read-only into the AIRS address space with separate ASID. Speculative reads from agent address spaces cannot reach model pool pages because:
1. Agent PTEs do not contain model pool mappings (separate TTBR0 entries)
2. ASID tagging prevents speculative TLB hits across address spaces
3. CSV2 hardware prevents branch predictor poisoning across contexts

**Syscall boundary barriers:** Every syscall entry point inserts a speculation barrier (`SB` instruction) after validating arguments. This prevents speculative execution from progressing past argument validation with attacker-controlled values.

-----

## 10. Swap and Compression

### 10.1 Strategy

AIOS is designed to operate without swap under normal conditions. Swap to an SD card (the primary storage on Pi hardware) would add seconds of latency to page faults. The strategy is:

1. **Prefer no swap.** Size memory pools so that normal workloads fit in RAM.
2. **Compressed memory (zram) as first tier.** Inactive pages are compressed in-place, staying in RAM but occupying less space. Typical compression ratio: 2:1 to 3:1 for agent heap data.
3. **Disk swap as last resort.** Only if compressed memory is insufficient. Useful for heavy workloads on 2 GB devices.
4. **Model memory is never swapped or compressed.** It is pinned and excluded from reclamation.

```
Reclamation tiers:

Tier 1: Clean page cache (re-readable from storage)
  → Free immediately, no I/O needed on reclaim
  → Re-read from space storage if accessed again

Tier 2: Compressed memory (zram)
  → Compress inactive agent pages in RAM
  → ~50% memory savings, microsecond decompression
  → Good for agent heap data (often highly compressible)

Tier 3: Disk swap (if enabled)
  → Write compressed pages to swap partition
  → ~10ms read latency on SD card (slow, avoid if possible)
  → Only for 2 GB devices under heavy load
```

### 10.2 Multi-Generational LRU (MGLRU)

Every reclaimable page in the user pool is tracked in a **Multi-Generational LRU (MGLRU)** — an approach pioneered in Linux 6.1+ that replaces the traditional two-list active/inactive LRU with four age generations. MGLRU delivers dramatically better eviction decisions on memory-constrained devices: Android/ChromeOS benchmarks show 85% fewer low-memory kills and 18% less memory pressure stall time. On a device where 4-8 GB must serve both agents and an AI model pool, this precision matters.

**Why not two-list LRU?** The traditional two-list design has a fundamental resolution problem: a page is either "active" or "inactive" — two states for millions of pages. A page accessed once 100 ms ago and a page accessed continuously for the last 10 seconds both sit on the same active list. When reclamation needs candidates, it cannot distinguish them without expensive full-list scans. MGLRU solves this with multiple generations that provide finer age resolution without increasing scan overhead.

**Four-generation architecture:**

Pages age through four generations (0 = youngest, 3 = oldest). Each generation has a birth timestamp marking when pages were last promoted into it. The key hardware mechanism is the **Access flag** in aarch64 page table entries (PTE bit [10]). When the CPU accesses a page for the first time after the flag is cleared, it sets the flag automatically. The kernel clears these flags during aging scans to detect access patterns.

```rust
pub struct MglruList {
    /// Four generations of pages, indexed by generation number.
    /// Gen 0: youngest (recently accessed)
    /// Gen 3: oldest (best eviction candidates)
    generations: [Generation; NUM_GENERATIONS],
    /// Per-type folios for scanning efficiency
    types: [PageTypeList; NUM_PAGE_TYPES],
}

const NUM_GENERATIONS: usize = 4;
const NUM_PAGE_TYPES: usize = 3;

pub struct Generation {
    /// Pages in this generation
    pages: LinkedList<MglruEntry>,
    /// Page count
    count: usize,
    /// Timestamp when this generation was created (monotonic)
    birth: Timestamp,
}

pub struct MglruEntry {
    frame: PhysicalFrame,
    /// Page type for reclamation priority
    page_type: PageType,
    /// Current generation (0-3)
    gen: u8,
    /// Referenced since last aging scan?
    referenced: bool,
    /// Dirty (modified since last writeback)?
    dirty: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageType {
    /// File-backed page cache (clean: free immediately, dirty: write back first)
    PageCache,
    /// Anonymous page (agent heap, stack) — must compress or swap
    Anonymous,
    /// Shared memory region — only reclaimable if all mappers are idle
    Shared,
}
```

**Aging algorithm — generation advancement:**

The aging scan runs every 200 ms under normal pressure, 50 ms under critical. Instead of simply moving pages between two lists, MGLRU advances pages through generations based on access:

```
Aging scan (periodic, per-generation):
  For each page in generation N (scanning oldest generations first):
    1. Read PTE Access flag
    2. If Access flag SET:
         → Clear Access flag (start new observation window)
         → Promote page to generation 0 (youngest — it was just accessed)
    3. If Access flag CLEAR:
         → Page stays in its current generation
         → If this is generation 3 (oldest): page is a prime eviction candidate

Generation rotation (when generation 0 fills):
  1. Generation 3 is evicted (pages reclaimed or compressed)
  2. Generation 2 becomes generation 3
  3. Generation 1 becomes generation 2
  4. Generation 0 becomes generation 1
  5. A new empty generation 0 is created with current timestamp
```

**Why four generations?** Four generations provide the right balance between age resolution and overhead:

```
Generation   Meaning                      Typical Age     Action
──────────   ───────                      ───────────     ──────
    0        Just accessed / newly faulted  < 1 second     Protected — never reclaimed
    1        Accessed in recent past        1-5 seconds    Protected under normal pressure
    2        Not accessed recently          5-30 seconds   Candidate under Critical pressure
    3        Cold — no access for a while   > 30 seconds   First to be reclaimed
```

Two generations (the traditional design) cannot distinguish "accessed 1 second ago" from "accessed 10 seconds ago" — both are on the active list. Four generations separate them into gen 0 vs gen 1, enabling proportional reclamation under different pressure levels: Normal reclaims only gen 3, Low reclaims gen 3+2, Critical can dip into gen 1.

**Scan-resistant by design:** MGLRU is inherently resistant to scanning pollution. If an agent reads through a large file once, those pages enter generation 0 but are immediately aged to gen 1 on the next rotation (they won't be re-accessed). By the time they reach gen 3, they are the first candidates for eviction. Frequently-used pages keep getting promoted back to gen 0, staying safe. No special "second-chance" logic is needed — the generation structure handles it naturally.

```rust
impl MglruList {
    /// Minimum pages in gen 3 before we rotate
    const MIN_OLDEST_GEN_PAGES: usize = 128;
    /// Maximum age of gen 0 before rotation (milliseconds)
    const MAX_YOUNGEST_GEN_AGE_MS: u64 = 5000;

    /// Rotate generations: advance all gens by 1, evict oldest
    fn rotate(&mut self) -> Vec<PhysicalFrame> {
        // Collect gen 3 pages for reclamation
        let evicted: Vec<PhysicalFrame> = self.generations[3].pages
            .drain(..)
            .map(|entry| entry.frame)
            .collect();

        // Shift generations: 2→3, 1→2, 0→1
        self.generations[3] = core::mem::take(&mut self.generations[2]);
        self.generations[2] = core::mem::take(&mut self.generations[1]);
        self.generations[1] = core::mem::take(&mut self.generations[0]);

        // New empty gen 0
        self.generations[0] = Generation {
            pages: LinkedList::new(),
            count: 0,
            birth: Timestamp::now(),
        };

        // Update gen numbers in shifted entries
        for gen_idx in 1..NUM_GENERATIONS {
            for entry in self.generations[gen_idx].pages.iter_mut() {
                entry.gen = gen_idx as u8;
            }
        }

        evicted
    }

    /// Promote a page back to gen 0 (it was accessed)
    fn promote(&mut self, frame: PhysicalFrame, from_gen: u8) {
        self.generations[from_gen as usize].remove(frame);
        self.generations[0].push(MglruEntry {
            frame,
            page_type: frame.page_type(),
            gen: 0,
            referenced: true,
            dirty: frame.is_dirty(),
        });
    }

    /// Select pages for reclamation, respecting pressure level
    pub fn select_reclaim(
        &mut self,
        pressure: MemoryPressure,
        count: usize,
    ) -> Vec<PhysicalFrame> {
        // min_gen: the youngest generation we're willing to reclaim from.
        // Always start scanning from gen 3 (oldest/coldest) downward.
        // Under higher pressure, we dip into younger (warmer) generations.
        let min_gen = match pressure {
            MemoryPressure::Normal   => 3, // only oldest gen (gen 3)
            MemoryPressure::Low      => 3, // oldest gen, more aggressively
            MemoryPressure::Critical => 2, // dip into gen 2
            MemoryPressure::Oom      => 1, // everything except gen 0
        };

        let mut reclaimed = Vec::with_capacity(count);

        // Scan from oldest (gen 3) to youngest allowed (min_gen),
        // clean pages before dirty
        for gen in (min_gen..=3).rev() {
            // First pass: clean PageCache (free immediately, no I/O)
            for entry in self.generations[gen].pages.iter() {
                if reclaimed.len() >= count { break; }
                if !entry.dirty && entry.page_type == PageType::PageCache {
                    reclaimed.push(entry.frame);
                }
            }
            // Second pass: anonymous pages (must compress or swap)
            for entry in self.generations[gen].pages.iter() {
                if reclaimed.len() >= count { break; }
                if entry.page_type == PageType::Anonymous {
                    reclaimed.push(entry.frame);
                }
            }
        }

        reclaimed
    }
}
```

**Integration with DAMON (§10.9):** MGLRU's generation placement is enhanced by DAMON access pattern monitoring. While MGLRU relies on periodic PTE flag scans (point-in-time snapshots), DAMON provides continuous access frequency data. Pages that DAMON identifies as "cold" (low access frequency over sustained periods) can be aged more aggressively — moved directly to gen 2 or 3 instead of waiting for multiple rotation cycles. This is especially valuable for model memory management: DAMON detects when KV cache blocks transition from active inference to idle, enabling faster reclamation of background session caches.

**MGLRU performance characteristics on AIOS target hardware:**

```
Metric                           Two-list LRU    MGLRU       Improvement
──────                           ────────────    ─────       ───────────
Eviction accuracy (right page)   ~60%            ~85%        +42%
Scan overhead per aging cycle    O(active_list)  O(gen_0)    ~4x lower
Low-memory kills (8 GB, heavy)   ~12/hour        ~2/hour     85% fewer
Working set estimation error     ±30%            ±10%        3x more precise
```

These improvements come from generation-based age tracking: instead of a binary active/inactive classification, MGLRU maintains four distinct age cohorts. The kernel knows not just "was this page accessed recently?" but "how recently, relative to other pages?" This precision is critical on 4-8 GB devices where the difference between evicting the right page and the wrong page is the difference between smooth operation and an OOM kill.

### 10.3 Compressed Memory (zram)

zram provides the second tier of memory reclamation. Instead of writing inactive pages to a slow SD card, zram compresses them and stores the compressed data in a reserved region of RAM. A 4 KB page typically compresses to 1–2 KB (agent heap data — structs, strings, JSON — is highly compressible). This effectively doubles the amount of data that can reside in RAM without any I/O.

**Architecture:**

```
Before compression:                    After compression:

┌───────────┐                          ┌───────────┐
│ Page A     │  4 KB                   │ Page A     │  compressed: 1.2 KB ──┐
│ (inactive) │                         │ (freed)    │                       │
├───────────┤                          ├───────────┤    ┌─────────────────┐ │
│ Page B     │  4 KB                   │ Page B     │    │ zram pool       │ │
│ (inactive) │                         │ (freed)    │    │                 │ │
├───────────┤                          ├───────────┤    │ ┌─A─┬─B─┬─C─┐  │◄┘
│ Page C     │  4 KB                   │ Page C     │    │ │1.2│0.8│1.5│  │
│ (inactive) │                         │ (freed)    │    │ │KB │KB │KB │  │
├───────────┤                          ├───────────┤    │ └───┴───┴───┘  │
│ ...        │                         │ (free)     │    │ 3.5 KB used    │
│            │                         │ 12 KB free │    │ (was 12 KB)    │
└───────────┘                          └───────────┘    └─────────────────┘

12 KB occupied  →  3.5 KB occupied  (3.4:1 ratio, 8.5 KB freed)
```

**Compression algorithm selection:**

```rust
#[derive(Debug, Clone, Copy)]
pub enum CompressionAlgorithm {
    /// LZ4: ~500 MB/s compress, ~2 GB/s decompress on Cortex-A76
    /// Ratio: 2:1 to 2.5:1 typical. Best for latency-sensitive paths.
    Lz4,
    /// Zstd (level 1): ~300 MB/s compress, ~1 GB/s decompress
    /// Ratio: 2.5:1 to 3.5:1 typical. Better ratio at higher CPU cost.
    ZstdFast,
}
```

AIOS uses **LZ4 as the default** compression algorithm. The reasoning:

- **Decompression speed is critical.** When an agent accesses a compressed page, the page fault handler must decompress it before the agent can proceed. LZ4 decompresses at ~2 GB/s on Cortex-A76 — a 4 KB page decompresses in ~2 microseconds. The agent barely notices.
- **Compression speed matters too.** When the reclaimer compresses pages under memory pressure, every microsecond counts. LZ4 compresses at ~500 MB/s — roughly 8 microseconds per page.
- **The ratio tradeoff is acceptable.** LZ4's 2:1 to 2.5:1 ratio is worse than Zstd's 2.5:1 to 3.5:1, but the speed difference is large. On a 4-core Cortex-A76 where one core is running inference, CPU time is scarce. LZ4 minimizes CPU steal.

**When Zstd is used:** Under sustained Critical pressure (section 8.1), the reclaimer switches to ZstdFast for newly compressed pages. The reasoning: if we're already critically low on memory, squeezing an extra 20-40% ratio is worth the CPU cost. Pages that were already compressed with LZ4 are not recompressed — the benefit doesn't justify decompressing and recompressing every existing page.

```rust
pub struct ZramBackend {
    /// Compressed page storage (in RAM)
    compressed: HashMap<PhysicalFrame, CompressedPage>,
    /// Active compression algorithm (switches under pressure)
    algorithm: CompressionAlgorithm,
    /// Memory saved by compression (bytes_original - bytes_compressed)
    bytes_saved: usize,
    /// Total compressed bytes stored
    bytes_stored: usize,
    /// Maximum zram pool size (percentage of user pool)
    max_pool_bytes: usize,
    /// Compression statistics for monitoring
    stats: ZramStats,
}

pub struct ZramStats {
    /// Total pages compressed
    pages_compressed: u64,
    /// Total pages decompressed (on access fault)
    pages_decompressed: u64,
    /// Pages that didn't compress well (ratio < 1.5:1, stored uncompressed)
    pages_incompressible: u64,
    /// Average compression ratio (original / compressed)
    avg_ratio: f32,
    /// Total CPU time spent compressing (microseconds)
    compress_time_us: u64,
    /// Total CPU time spent decompressing (microseconds)
    decompress_time_us: u64,
}

pub struct CompressedPage {
    /// Compressed data (typically 1-2 KB for a 4 KB page)
    data: Vec<u8>,
    /// Original page's owner
    owner: ProcessId,
    /// Virtual address in owner's space
    vaddr: VirtualAddress,
    /// Algorithm used (needed for decompression)
    algorithm: CompressionAlgorithm,
    /// Original size (always PAGE_SIZE, but stored for validation)
    original_size: usize,
}
```

**Per-device zram pool sizing:**

The zram pool is capped at a percentage of the user pool. There's no benefit to allowing zram to consume all of RAM — at some point the overhead of compressed page metadata and fragmenting the remaining free memory is worse than just swapping to disk or killing an agent.

```
Device RAM   User Pool   zram Max (25%)   Effective Capacity
─────────    ─────────   ─────────────    ──────────────────
2 GB         1.75 GB     448 MB           1.75 GB user + ~900 MB virtual (2.5:1)
4 GB         1.5 GB      384 MB           1.5 GB user + ~750 MB virtual
8 GB         3.5 GB      896 MB           3.5 GB user + ~1.8 GB virtual
16 GB        7.5 GB      1.9 GB           7.5 GB user + ~3.8 GB virtual
```

**Incompressible page handling:** Not all data compresses well. Encrypted data, already-compressed media, and random bytes may compress to larger than the original. If a page compresses to more than 75% of its original size (ratio below 1.33:1), the reclaimer marks it as incompressible and does not store it in zram. These pages remain in their current MGLRU generation and will be swapped to disk (tier 3) if memory pressure continues. The `pages_incompressible` counter in `ZramStats` tracks how often this occurs — a high value suggests agents are working with encrypted or pre-compressed data, and the reclaimer should favor disk swap earlier.

### 10.4 Swap Device

Disk swap is tier 3 — the last resort before the OOM killer. It exists primarily for 2 GB devices where the user pool is small enough that even zram can't keep up with heavy workloads.

**Swap partition sizing and initialization:**

AIOS uses a dedicated swap partition rather than a swap file. A swap partition avoids filesystem overhead and provides predictable I/O patterns. The partition is created during installation and sized based on device RAM:

```
Device RAM   Swap Partition   Rationale
─────────    ──────────────   ─────────
2 GB         512 MB           Essential — user pool is only 1 GB
4 GB         256 MB           Safety net — rarely used if zram is effective
8 GB+        0 (disabled)     Not needed — zram provides sufficient expansion
```

On 8 GB+ devices, swap is **disabled by default**. The reasoning: on an 8 GB device the user pool is 3.5 GB, zram adds ~1.8 GB of virtual capacity, and the model pool already handles AI memory separately. Swap would only activate in pathological scenarios where the OOM killer is a better response. Disabling swap also eliminates SD card wear from swap I/O entirely on these devices.

```rust
pub struct SwapDevice {
    /// Block device path (e.g., /dev/mmcblk0p3 — third partition on SD card)
    device: BlockDeviceHandle,
    /// Total swap slots (one slot = one 4 KB page)
    total_slots: usize,
    /// Free slot bitmap
    free_bitmap: BitVec,
    /// Number of currently used slots
    used_slots: usize,
    /// I/O statistics
    stats: SwapStats,
    /// Throttle state for wear leveling
    throttle: SwapThrottle,
}

pub struct SwapStats {
    /// Total pages written to swap (swap-out)
    pages_out: u64,
    /// Total pages read from swap (swap-in, on page fault)
    pages_in: u64,
    /// Total bytes written to the device
    bytes_written: u64,
    /// Write errors (bad blocks, I/O failures)
    write_errors: u64,
    /// Average swap-in latency (microseconds)
    avg_swapin_latency_us: u64,
}

impl SwapDevice {
    /// Initialize swap from a dedicated partition
    pub fn init(device: BlockDeviceHandle, partition_size: usize) -> Result<Self, SwapError> {
        let total_slots = partition_size / PAGE_SIZE;

        // Write a swap header to the first page (magic number, version, slot count).
        // The header allows the kernel to validate the partition at boot and detect
        // corruption or a misidentified partition.
        let header = SwapHeader {
            magic: AIOS_SWAP_MAGIC,
            version: 1,
            total_slots: total_slots as u32,
            page_size: PAGE_SIZE as u32,
        };
        device.write_block(0, &header.to_bytes())?;

        let mut free_bitmap = BitVec::with_capacity(total_slots);
        free_bitmap.set_all(true);
        free_bitmap.set(0, false); // slot 0 is the header

        Ok(SwapDevice {
            device,
            total_slots,
            free_bitmap,
            used_slots: 0,
            stats: SwapStats::default(),
            throttle: SwapThrottle::new(),
        })
    }

    /// Write a page to swap. Returns the swap slot index.
    pub fn write_page(&mut self, frame: PhysicalFrame) -> Result<SwapSlot, SwapError> {
        // Check throttle — refuse if we've exceeded the write budget
        if self.throttle.is_throttled() {
            return Err(SwapError::Throttled);
        }

        let slot = self.free_bitmap.first_set()
            .ok_or(SwapError::Full)?;

        let offset = slot * PAGE_SIZE;
        self.device.write_block(offset, frame.as_slice())?;

        self.free_bitmap.set(slot, false);
        self.used_slots += 1;
        self.stats.pages_out += 1;
        self.stats.bytes_written += PAGE_SIZE as u64;
        self.throttle.record_write();

        Ok(SwapSlot(slot))
    }

    /// Read a page back from swap (called from page fault handler).
    pub fn read_page(&mut self, slot: SwapSlot) -> Result<PageData, SwapError> {
        let offset = slot.0 * PAGE_SIZE;
        let data = self.device.read_block(offset, PAGE_SIZE)?;

        self.free_bitmap.set(slot.0, true);
        self.used_slots -= 1;
        self.stats.pages_in += 1;

        Ok(data)
    }
}
```

### 10.5 Page Fault Handling for Compressed and Swapped Pages

When an agent accesses a page that has been compressed (zram) or swapped to disk, the CPU raises a page fault because the PTE has been invalidated. The page fault handler must detect the cause, retrieve the data, and make the page accessible again — all transparently to the agent.

**PTE encoding for compressed and swapped pages:**

When a page is reclaimed, its PTE is modified to indicate where the data went. The PTE's "valid" bit is cleared (causing a fault on access), and the remaining bits encode the location:

```
Valid PTE (page in physical RAM):
  ┌─────────────────────────────────────────────────────────┐
  │ Physical Frame Number (bits 47:12) │ Flags │ V=1        │
  └─────────────────────────────────────────────────────────┘

Compressed PTE (page in zram):
  ┌─────────────────────────────────────────────────────────┐
  │ zram index (bits 47:2)             │ Type=01 │ V=0      │
  └─────────────────────────────────────────────────────────┘

Swapped PTE (page on disk):
  ┌─────────────────────────────────────────────────────────┐
  │ Swap slot number (bits 47:2)       │ Type=10 │ V=0      │
  └─────────────────────────────────────────────────────────┘

Zero PTE (never accessed — demand-zero on first fault):
  ┌─────────────────────────────────────────────────────────┐
  │ 0000000000000000000000000000000000 │ Type=00 │ V=0      │
  └─────────────────────────────────────────────────────────┘
```

**PTE state classification and fault type:**

```rust
/// Decoded state of an invalid PTE (V=0). The encoding uses Type bits [1:0]
/// as shown in the PTE diagrams above; CopyOnWrite and FileBacked are
/// identified by additional software bits in the upper PTE word.
pub enum PteState {
    /// Type=00, all-zero — page has never been accessed (demand-zero).
    Zero,
    /// Type=01 — page was compressed into zram.
    Compressed { zram_index: usize },
    /// Type=10 — page was evicted to the swap device.
    Swapped { swap_slot: SwapSlot },
    /// Software COW bit set — page is a shared copy-on-write mapping.
    CopyOnWrite { original_frame: PhysicalFrame },
    /// Software file-backed bit — page is backed by a file in the page cache.
    FileBacked { file: MappedFile, offset: u64 },
    /// PTE is valid (V=1) — should not reach the non-present fault path.
    Present,
}

/// Classification of the memory access that triggered the fault.
pub enum FaultType {
    Read,
    Write,
    Execute,
}

/// Error outcomes for page fault resolution. Returned by the fault handler
/// and propagated to the process as a signal (SIGSEGV, SIGBUS) or to the
/// kernel for OOM handling.
pub enum FaultError {
    /// Access to an address not covered by any VMA — the process touched
    /// unmapped memory. Delivered as SIGSEGV to the faulting process.
    SegmentationFault,
    /// VMA exists but does not permit the attempted access type
    /// (e.g., write to a read-only mapping). Delivered as SIGSEGV.
    ProtectionFault,
    /// The capability that granted access to the underlying resource
    /// was revoked between mapping creation and fault resolution.
    CapabilityRevoked,
    /// No physical frames available and reclamation failed.
    OutOfMemory,
    /// Address has no VMA mapping (more specific than SegmentationFault
    /// for kernel-internal use — distinguishes "no VMA" from "VMA found
    /// but address outside its range").
    InvalidAddress,
    /// No VMA covers the faulting address in find_vma().
    UnmappedRegion,
    /// Swap device is required but not configured or not available.
    SwapDeviceMissing,
    /// PTE was in an unexpected state for the current fault path
    /// (e.g., valid PTE reaching the non-present handler).
    UnexpectedPteState,
    /// Generic write permission failure on a read-only address.
    AccessViolation,
}
```

**Page fault handler — full path:**

```rust
pub fn handle_page_fault(
    fault_addr: VirtualAddress,
    fault_type: FaultType,
    process: &mut Process,
) -> Result<(), FaultError> {
    let vma = process.address_space.find_vma(fault_addr)
        .ok_or(FaultError::SegmentationFault)?;

    // Permission check: is the access type valid for this VMA?
    if !vma.permits(fault_type) {
        return Err(FaultError::ProtectionFault);
    }

    let pte = process.address_space.walk_page_table(fault_addr)?;

    match pte.state() {
        // Case 1: Demand-zero page (first access to anonymous mapping)
        PteState::Zero => {
            let frame = alloc_zeroed_page()?;
            process.address_space.map_page(fault_addr, frame, vma.permissions());
            process.memory_stats.record_minor_fault();
            Ok(())
        }

        // Case 2: Compressed page in zram
        PteState::Compressed { zram_index } => {
            let frame = alloc_page()?;
            let compressed = RECLAIMER.lock().zram.decompress(zram_index)?;
            frame.copy_from(&compressed);
            process.address_space.map_page(fault_addr, frame, vma.permissions());
            process.memory_stats.record_minor_fault();
            // Page enters MGLRU generation 0 (youngest — just accessed)
            RECLAIMER.lock().mglru.insert_gen0(frame);
            Ok(())
        }

        // Case 3: Swapped page on disk
        PteState::Swapped { swap_slot } => {
            let frame = alloc_page()?;
            let data = RECLAIMER.lock().swap.as_mut()
                .ok_or(FaultError::SwapDeviceMissing)?
                .read_page(swap_slot)?;
            frame.copy_from(&data);
            process.address_space.map_page(fault_addr, frame, vma.permissions());
            process.memory_stats.record_major_fault();
            RECLAIMER.lock().mglru.insert_gen0(frame);
            Ok(())
        }

        // Case 4: COW page (handled in section 5.4)
        PteState::CopyOnWrite { original_frame } => {
            handle_cow_fault(fault_addr, original_frame, process, vma)
        }

        // Case 5: File-backed page (page cache miss)
        PteState::FileBacked { file, offset } => {
            let frame = alloc_page()?;
            file.read_page(offset, &mut frame)?;
            process.address_space.map_page(fault_addr, frame, vma.permissions());
            process.memory_stats.record_major_fault();
            RECLAIMER.lock().mglru.insert_gen0(frame);
            Ok(())
        }

        // Case 6: PTE is actually valid — should not reach this path
        PteState::Present => Err(FaultError::UnexpectedPteState),
    }
}
```

**Minor vs. major faults:**

- **Minor fault:** Data is still in RAM (demand-zero, zram, COW). Resolved in microseconds. No disk I/O.
- **Major fault:** Data must be read from disk (swap, file-backed page cache miss). Resolved in milliseconds. The faulting thread is blocked until I/O completes.

On 8 GB devices running normal workloads, virtually all faults are minor (demand-zero or COW). Major faults should be rare. If `major_faults` is climbing, the system is swapping — the memory pressure system (section 8) should already be responding.

**Readahead on swap-in:** When a page fault reads one page from swap, adjacent pages (in the agent's virtual address space) are likely to be needed soon. The swap-in path reads up to 8 contiguous swap slots in a single I/O operation, decompresses them, and maps them into the agent's address space. This amortizes the SD card's high seek latency across multiple pages. The readahead window is adaptive — it starts at 1 page and doubles on each sequential fault, resetting to 1 on a non-sequential fault.

```rust
pub struct SwapReadahead {
    /// Current readahead window (pages)
    window: usize,
    /// Maximum readahead window
    max_window: usize,          // default: 8 pages (32 KB)
    /// Last swap-in virtual address (for sequential detection)
    last_fault_addr: Option<VirtualAddress>,
}

impl SwapReadahead {
    pub fn compute_range(
        &mut self,
        fault_addr: VirtualAddress,
        swap_slot: SwapSlot,
    ) -> Range<SwapSlot> {
        if let Some(last) = self.last_fault_addr {
            if fault_addr == last + PAGE_SIZE {
                // Sequential access — grow window
                self.window = (self.window * 2).min(self.max_window);
            } else {
                // Random access — reset window
                self.window = 1;
            }
        }
        self.last_fault_addr = Some(fault_addr);

        let start = swap_slot;
        let end = SwapSlot(swap_slot.0 + self.window);
        start..end
    }
}
```

### 10.6 Swap Thrashing Prevention

Thrashing occurs when the system continuously swaps pages in and out — an agent accesses page A (swapped in), which evicts page B (swapped out), then the agent accesses page B (swapped in), which evicts page A (swapped out), and so on. The system spends all its time doing I/O and makes no forward progress. On an SD card with ~10 ms latency per I/O, thrashing is catastrophic.

**Detection:**

The kernel monitors the ratio of swap-in events to useful CPU cycles. Thrashing is detected when:

```rust
pub struct ThrashDetector {
    /// Swap-in events in the current window
    swapins_this_window: u64,
    /// Window duration (default: 1 second)
    window_duration: Duration,
    /// Threshold: swap-ins per second that indicates thrashing
    thrash_threshold: u64,       // default: 50 swap-ins/sec
    /// Consecutive windows above threshold before declaring thrash
    consecutive_above: u32,
    /// Threshold for consecutive windows
    consecutive_required: u32,   // default: 3 (3 seconds sustained)
    /// Current thrash state
    state: ThrashState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThrashState {
    /// Normal operation
    Normal,
    /// Elevated swap activity — monitoring
    Elevated,
    /// Thrashing detected — intervention required
    Thrashing,
}
```

**Response — escalating interventions:**

When thrashing is detected, the kernel does not simply continue reclaiming and swapping. It intervenes to break the thrash cycle:

```
ThrashState::Elevated (1-2 seconds of high swap activity):
  1. Increase zram pool cap by 10% (allow more compressed pages)
  2. Reduce aging scan interval to 50 ms (find cold pages faster)
  3. Notify memory pressure system: transition to Critical if not already

ThrashState::Thrashing (3+ seconds sustained):
  1. Identify the agent causing the most swap-ins (from per-process fault stats)
  2. SUSPEND that agent (stop its execution, freeze its pages in place)
  3. Notify user: "Agent 'research-assistant' suspended — system is low on memory.
     Close some agents or restart with more available memory."
  4. If multiple agents are thrashing, suspend in priority order (lowest first)
  5. Resume suspended agents only when memory pressure drops to Normal
```

**Why suspend instead of kill?** The OOM killer (section 8.2) is for when memory is truly exhausted. Thrashing is different — there may be enough total memory, but the working sets of running agents exceed physical RAM. Suspending one agent freezes its working set, allowing others to make progress. The suspended agent's state is preserved — it can resume later without losing work. Killing is permanent; suspending is reversible.

**Per-agent working set tracking:** The thrash detector maintains a per-agent swap-in counter. An agent that generates 80% of swap-ins while using 20% of memory is the likely thrash culprit — its working set doesn't fit. The suspension decision uses both swap-in frequency and agent priority:

```rust
impl ThrashDetector {
    fn select_suspend_candidate(&self, agents: &[AgentProcess]) -> Option<ProcessId> {
        agents.iter()
            .filter(|a| a.priority() != AgentPriority::Critical)
            .filter(|a| !a.is_suspended())
            .max_by(|a, b| {
                let score = |agent: &&AgentProcess| -> f64 {
                    let swapin_rate = agent.memory_stats.major_faults_per_sec();
                    let priority_weight: f64 = match agent.priority() {
                        AgentPriority::Background => 4.0,
                        AgentPriority::Normal     => 2.0,
                        AgentPriority::System     => 1.0,
                        AgentPriority::Critical   => 0.0,
                    };
                    swapin_rate * priority_weight
                };
                score(a).partial_cmp(&score(b)).unwrap_or(core::cmp::Ordering::Equal)
            })
            .map(|a| a.pid)
    }
}
```

### 10.7 SD Card Wear Mitigation

SD cards and eMMC storage use NAND flash, which has a limited number of write/erase cycles per cell:

```
Storage Type          Write Endurance         Practical Lifetime (swap use)
────────────          ───────────────         ────────────────────────────
Consumer SD (TLC)     ~1,000 P/E cycles       Destroyed in weeks of heavy swap
Industrial SD (pSLC)  ~30,000 P/E cycles      Months of sustained swap
eMMC (typical Pi 5)   ~3,000 P/E cycles       Weeks to months depending on load
NVMe (if available)   ~600 TBW (128 GB)       Years — not a concern
```

Heavy swap traffic on consumer SD cards is one of the fastest ways to destroy flash storage. A 512 MB swap partition with 50 pages/sec swap-out would write ~800 MB/hour — enough to cycle through every cell multiple times per day. AIOS addresses this with a write throttle and strict swap budgeting.

**Write throttle:**

```rust
pub struct SwapThrottle {
    /// Maximum bytes written per hour (rolling window)
    hourly_budget: u64,           // default: 200 MB/hour
    /// Maximum bytes written per day
    daily_budget: u64,            // default: 2 GB/day
    /// Bytes written in the current hour window
    hourly_written: u64,
    /// Bytes written in the current day window
    daily_written: u64,
    /// Window start timestamps
    hourly_start: Instant,
    daily_start: Instant,
    /// Throttle state
    throttled: bool,
}

impl SwapThrottle {
    pub fn new() -> Self {
        SwapThrottle {
            hourly_budget: 200 * 1024 * 1024,     // 200 MB/hour
            daily_budget: 2 * 1024 * 1024 * 1024,  // 2 GB/day
            hourly_written: 0,
            daily_written: 0,
            hourly_start: Instant::now(),
            daily_start: Instant::now(),
            throttled: false,
        }
    }

    pub fn record_write(&mut self) {
        self.hourly_written += PAGE_SIZE as u64;
        self.daily_written += PAGE_SIZE as u64;

        // Roll over windows
        if self.hourly_start.elapsed() > Duration::from_secs(3600) {
            self.hourly_written = 0;
            self.hourly_start = Instant::now();
        }
        if self.daily_start.elapsed() > Duration::from_secs(86400) {
            self.daily_written = 0;
            self.daily_start = Instant::now();
        }

        // Check budgets
        if self.hourly_written >= self.hourly_budget
            || self.daily_written >= self.daily_budget
        {
            self.throttled = true;
        }
    }

    pub fn is_throttled(&self) -> bool {
        self.throttled
    }
}
```

**What happens when the throttle engages?** The swap device refuses new writes. The reclaimer is limited to tier 1 (clean pages) and tier 2 (zram). If that's not enough, the memory pressure system escalates to Critical and eventually OOM. This is intentional: **it is better to kill an agent than to destroy the storage device.** The user is notified:

```
"Swap write limit reached (storage protection). Background agents may be
 suspended or terminated to free memory. Consider closing unused agents."
```

**Wear estimation and reporting:** At boot, the kernel reads the swap device's lifetime write counter (via eMMC SMART data or SD card status registers where available). AIOS estimates remaining device lifetime and exposes it through the system status API:

```
Storage health:
  Device: Samsung EVO 64 GB (SD)
  Estimated wear: 12% (based on total bytes written)
  Swap writes today: 340 MB / 2 GB budget
  Swap writes this hour: 45 MB / 200 MB budget
```

If estimated wear exceeds 80%, AIOS disables swap entirely and logs a warning recommending device replacement. The system continues to function — zram and the OOM killer handle memory pressure without disk swap.

### 10.8 Page Reclamation

The page reclaimer ties sections 10.2 through 10.7 together. It runs when memory pressure reaches `Low` or worse, uses MGLRU generation-based eviction to select candidates, and frees pages through the three-tier hierarchy:

```rust
pub struct PageReclaimer {
    /// Multi-Generational LRU of reclaimable pages (section 10.2)
    mglru: MglruList,
    /// Compressed memory backend (section 10.3)
    zram: ZramBackend,
    /// Swap device, if configured (section 10.4)
    swap: Option<SwapDevice>,
    /// Swap readahead state (section 10.5)
    readahead: SwapReadahead,
    /// Thrash detector (section 10.6)
    thrash_detector: ThrashDetector,
    /// DAMON access monitor (section 10.9) — optional, provides hints
    damon: Option<DamonMonitor>,
    /// Scan interval (adaptive based on pressure)
    scan_interval: Duration,
}

impl PageReclaimer {
    pub fn reclaim(&mut self, target_pages: usize, pressure: MemoryPressure) -> usize {
        let mut reclaimed = 0;

        // If DAMON is active, apply its cold-page hints to MGLRU
        // (age cold pages to older generations before selection)
        if let Some(ref damon) = self.damon {
            for cold_region in damon.cold_regions() {
                self.mglru.age_to_generation(cold_region, 3);
            }
        }

        // Select reclamation candidates from MGLRU based on pressure level
        let candidates = self.mglru.select_reclaim(pressure, target_pages);

        for frame in candidates {
            let entry = self.mglru.entry(frame);

            // Tier 1: clean page cache — free immediately, no I/O
            if !entry.dirty && entry.page_type == PageType::PageCache {
                self.free_clean_page(frame);
                reclaimed += 1;
                continue;
            }

            // Tier 2: compress dirty/anonymous pages into zram
            match self.zram.compress(frame) {
                Ok(_) => {
                    reclaimed += 1;
                    continue;
                }
                Err(ZramError::Full) => {} // fall through to tier 3
                Err(ZramError::Incompressible) => {} // fall through to tier 3
                Err(_) => break,
            }

            // Tier 3: swap to disk (last resort)
            if let Some(ref mut swap) = self.swap {
                match swap.write_page(frame) {
                    Ok(_) => {
                        reclaimed += 1;
                    }
                    Err(SwapError::Throttled) => break,
                    Err(SwapError::Full) => break,
                    Err(_) => break,
                }
            }
        }

        // Update thrash detector
        self.thrash_detector.update();

        reclaimed
    }

    fn free_clean_page(&self, frame: PhysicalFrame) {
        // Page is clean (unmodified page cache) — just free the frame.
        // If the file is accessed again, it will be re-read from storage.
        frame_allocator::free(frame);
    }
}
```

### 10.9 DAMON (Data Access Monitoring)

DAMON (Data Access MONitoring) provides continuous, low-overhead access pattern monitoring for the memory subsystem. Inspired by Linux 5.15+ DAMON, AIOS adapts this technique to serve two purposes: feeding MGLRU with precise access frequency data, and providing AIRS with memory usage intelligence for resource orchestration.

**Why DAMON in addition to PTE flag scanning?** MGLRU's PTE Access flag scan (§10.2) provides a binary signal: "was this page accessed since the last scan?" DAMON provides a richer signal: "how frequently is this region accessed, and what is its working set size?" This distinction matters for AI workloads where access patterns are predictable (layer-by-layer inference, sequential KV cache growth) and can be exploited for proactive reclamation.

```rust
/// DAMON monitors contiguous virtual address regions and samples
/// access patterns at configurable intervals.
pub struct DamonMonitor {
    /// Monitored regions (per-process)
    targets: Vec<DamonTarget>,
    /// Sampling interval (how often we check PTE flags)
    sample_interval: Duration,         // default: 5 ms
    /// Aggregation interval (how often we update statistics)
    aggregation_interval: Duration,    // default: 100 ms
    /// Minimum region size (don't split below this)
    min_region_size: usize,            // default: 64 KB (1 medium page)
    /// Maximum number of regions per target
    max_regions: usize,                // default: 1000
}

pub struct DamonTarget {
    /// Process being monitored
    pid: ProcessId,
    /// Monitored address regions with access statistics
    regions: Vec<DamonRegion>,
}

pub struct DamonRegion {
    /// Virtual address range
    start: VirtualAddress,
    end: VirtualAddress,
    /// Access frequency: accesses per aggregation interval
    /// Derived from PTE Access flag sampling
    access_frequency: u32,
    /// Age: aggregation intervals since last access
    age: u32,
    /// Classification based on frequency thresholds
    hotness: RegionHotness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionHotness {
    /// Accessed every sampling interval — actively in use
    Hot,
    /// Accessed occasionally — keep in RAM but low priority
    Warm,
    /// Not accessed for multiple aggregation intervals — eviction candidate
    Cold,
    /// Not accessed for extended period — proactive reclamation target
    Frozen,
}
```

**DAMON-MGLRU integration:**

```
DAMON aggregation cycle (every 100 ms):
  1. For each monitored region, count PTE Access flag set events
     across sample_interval samples (20 samples at 5 ms each)
  2. Compute access_frequency = flags_set / samples_taken
  3. Classify region:
     - Hot:    frequency > 50% → MGLRU gen 0 (protected)
     - Warm:   frequency 10-50% → MGLRU gen 1
     - Cold:   frequency < 10% → MGLRU gen 2-3 (eviction candidate)
     - Frozen: age > 30 intervals (3 seconds) → MGLRU gen 3 (force)
  4. For Cold/Frozen regions: hint MGLRU to age pages to older generation
     (bypass normal rotation cycle — proactive demotion)
```

**AI-workload-specific monitoring:**

DAMON is especially valuable for AIOS because AI workloads have predictable memory access patterns:

| Workload | Access Pattern | DAMON Action |
|---|---|---|
| Active inference | Sequential layer access, hot KV cache tail | Mark inference layers Hot, KV prefix Warm |
| Idle KV cache | No access after inference completes | Detect Cold within 3s, hint MGLRU for eviction |
| Model loading (userfaultfd) | Sequential large reads | Detect Hot during load, transition to Warm after |
| Agent heap | Irregular, varies by agent behavior | Adaptive region splitting to track working set |
| Embedding store | Burst access during search, then idle | Detect Frozen between searches, keep Hot during |

**DAMON overhead budget:** DAMON sampling adds ~0.3% CPU overhead (periodic PTE walks, ~5 ms intervals). On a 4-core Cortex-A76, this is negligible — less than 1 ms per 100 ms aggregation cycle. The overhead is constant regardless of RAM size because DAMON uses region-based sampling (random page within each region) rather than scanning every page.

**AIRS integration:** DAMON exposes per-process working set size and access frequency data to AIRS via the kernel's resource monitoring channel. AIRS uses this for resource orchestration decisions: if DAMON reports that an agent's working set is growing and approaching its memory limit, AIRS can proactively suggest model pool boundary adjustment (§12.2) before memory pressure triggers reactive reclamation. This is advisory-only — the kernel makes the actual decision.

-----

## 11. Performance Considerations

### 11.1 TLB Efficiency

TLB misses are expensive — each miss requires a 4-level page table walk (4 memory accesses). AIOS minimizes TLB misses through:

- **ASIDs:** Context switches do not flush the TLB. Entries from the previous process remain valid for that process's ASID.
- **Multi-size THP:** Three page sizes (4 KB, 64 KB, 2 MB) matched to workload. 64 KB medium pages for agent heaps and KV cache blocks reduce TLB entries by 16x vs 4 KB. 2 MB huge pages for model weights reduce entries by 512x.
- **TTBR1 global entries:** Kernel mappings are global (not tagged with an ASID), so they persist across all context switches.

### 11.2 Cache Awareness

The physical memory allocator is aware of cache geometry:

- **Cache line alignment:** Slab objects that are frequently accessed together are aligned to cache line boundaries (64 bytes on Cortex-A76).
- **Cache coloring:** The buddy allocator tracks page colors (physical address bits that determine cache set). Allocations for different agents prefer different colors to avoid cache thrashing. This matters most on the Pi 4 (1 MB L2 per cluster).

### 11.3 SIMD Alignment

Model memory is aligned for NEON/SVE SIMD operations:

- Model weight tensors are 16-byte aligned (NEON requirement)
- KV cache blocks are 64-byte aligned (cache line, avoids false sharing)
- Embedding vectors are 16-byte aligned (for vectorized distance computations)

The model pool allocator guarantees these alignments. 2 MB huge pages naturally satisfy all alignment requirements.

### 11.4 Page Zeroing

Freshly allocated pages must be zeroed before being given to userspace (security requirement — otherwise one agent could read another's freed data). Zeroing a 4 KB page takes ~2 microseconds. Doing it at allocation time adds latency to every page fault.

AIOS uses a background zero-page thread:

```
1. Pages freed → added to "dirty free list"
2. Zero-page thread (lowest priority) picks pages from dirty free list
3. Zeros page using NEON (DC ZVA for cache-line zeroing on aarch64)
4. Moves page to "clean free list"
5. Allocator serves from clean free list first
```

Under normal operation, the zero-page thread stays ahead of demand. Under heavy allocation load, the allocator falls back to synchronous zeroing (slower but correct).

-----

## 12. Future Memory Scaling

### 12.1 Hardware Trajectory

RAM on single-board computers and consumer devices is growing rapidly:

```
Year    Pi / SBC RAM          Consumer Device RAM    Model Sizes (local)
────    ────────────          ───────────────────    ───────────────────
2024    2-8 GB                8-16 GB                7-8B at Q4 (4.5 GB)
2025    4-16 GB (Pi 5 16GB)  16-32 GB               13B at Q4 (8 GB), 8B at F16 (16 GB)
2026+   8-32 GB (projected)  32-64 GB               70B at Q4 (40 GB), 13B at F16 (26 GB)
```

The memory subsystem is designed to scale with this trajectory. The pool-based architecture adapts automatically — larger total RAM means larger model and user pools, not a different architecture.

### 12.2 Dynamic Model Pool

On current hardware, the model pool is fixed at boot. As devices gain more RAM, a static allocation becomes wasteful — a 32 GB device doesn't need 16 GB pinned for models when no inference is running.

```rust
pub struct DynamicModelPool {
    /// Minimum model pool size (always reserved)
    /// Must be >= companion embedding model (~100 MB)
    minimum: usize,
    /// Security floor: minimum size when security models are active
    /// Must be >= primary model footprint + companion model
    /// The kernel enforces this floor — AIRS cannot resize below it
    /// while intent verification or behavioral monitoring are enabled.
    /// This prevents AIRS resource orchestration from inadvertently
    /// disabling its own security functions.
    security_floor: usize,
    /// Maximum model pool size (never exceed)
    maximum: usize,                     // cap at 50% of total RAM
    /// Current allocated size
    current: usize,
    /// Grow model pool on demand (steal from user pool)
    grow_on_demand: bool,
    /// Shrink model pool when idle (return to user pool)
    shrink_when_idle: bool,
    /// Idle timeout before shrinking
    idle_timeout: Duration,             // default: 10 minutes
    /// Whether security services (intent verification, behavioral
    /// monitoring) are currently active — gates the security_floor check
    security_services_active: bool,
}

impl DynamicModelPool {
    /// Kernel-enforced: AIRS cannot shrink below security_floor
    /// while security services are active. This prevents a compromised
    /// or confused resource orchestrator from starving security inference.
    pub fn validate_resize(&self, requested_size: usize) -> Result<usize> {
        let floor = if self.security_services_active {
            self.security_floor  // primary model + companion must fit
        } else {
            self.minimum         // only embedding model needs to fit
        };
        if requested_size < floor {
            return Err(PoolResizeError::BelowSecurityFloor {
                requested: requested_size,
                floor,
            });
        }
        Ok(requested_size.min(self.maximum))
    }
}
```

**Phase 14 optimization:** The model pool grows when AIRS loads a model (stealing pages from the user pool) and shrinks when the model is evicted (returning pages to the user pool). This eliminates the waste of pinning 4 GB for a model that may not be used for hours. The minimum reservation (enough for the embedding model) ensures Space Indexer can always operate.

**Security floor invariant:** The `security_floor` is distinct from `minimum`. The `minimum` guarantees the embedding model fits (~100 MB) — enough for Space Indexer. The `security_floor` guarantees the primary model fits alongside the companion — enough for intent verification, behavioral analysis, and adversarial defense. When AIRS security services are active (which is always during normal operation), the kernel refuses to shrink the model pool below `security_floor`. This prevents a compromised AIRS resource orchestrator from starving its own security functions — the damage ceiling remains denial of service against non-security tasks, never against security itself. See [security.md §9.6](../security/security.md).

**Huge page management:** Dynamic growth requires available 2 MB contiguous regions. The buddy allocator naturally maintains these through coalescing. If fragmentation prevents a 2 MB allocation, the kernel can compact the user pool (migrate pages, update PTEs) to create contiguous regions — a slow but rare operation.

### 12.3 Multi-Model Concurrency

With 16-32 GB, multiple models can be loaded simultaneously:

```
32 GB device:
  Kernel: 256 MB
  Model pool: 16 GB
    - Primary (13B Q4_K_M): 8 GB
    - Vision specialist (3B): 2 GB
    - Code specialist (7B Q4): 4.5 GB
    - Embedding model: 100 MB
    - KV caches: ~1.4 GB
  User pool: 15.5 GB
  DMA: 128 MB
  Reserved: 128 MB
```

AIRS can route tasks to the best specialist model without switching. Intent verification uses the primary model. Code generation uses the code specialist. Image understanding uses the vision model. All loaded, all available, zero switching latency.

### 12.4 Larger Context Windows

Larger RAM enables longer KV caches, which means longer context windows:

```
KV cache scaling (8B model, Q4 KV):
  8K context:    ~256 MB
  32K context:   ~1 GB
  128K context:  ~4 GB
  256K context:  ~8 GB (requires 16+ GB device)
```

On 8 GB devices, 8K-32K context is practical. On 16-32 GB devices, 128K+ context windows allow AIRS to maintain rich conversation history, process entire documents in a single pass, and keep system service context (intent verifier, behavioral monitor) for longer periods without cache eviction.

### 12.5 Design Principles for Forward Compatibility

1. **Pool boundaries are configuration, not architecture.** Changing pool sizes is a boot parameter change, not a code change.
2. **The buddy allocator scales to any RAM size.** MAX_ORDER can be increased if devices exceed the current 4 MB maximum contiguous allocation.
3. **Model memory management is model-size-agnostic.** The same userfaultfd lazy loading + huge page + PagedAttention KV caching works for a 500 MB model or a 40 GB model.
4. **Memory pressure thresholds are percentages, not absolute values.** "20% free" works at 2 GB and 32 GB alike.
5. **The OOM killer's priority scoring is RAM-independent.** It selects victims by relative memory × priority, not absolute thresholds.

-----

## 13. Implementation Order

Memory management spans several development phases:

```
Phase 1 — Boot and First Pixels:
  ├── Parse UEFI memory map
  ├── Early page allocator (simple bump allocator for boot)
  └── Identity-mapped page tables for early kernel

Phase 2 — Memory Management (primary phase):
  ├── Buddy allocator with split/merge
  ├── Page pools (kernel, user, model, DMA)
  ├── 4-level page tables (PGD/PUD/PMD/PTE)
  ├── W^X enforcement in page table API
  ├── KASLR (randomized kernel base)
  ├── ASID allocator and TLB management
  ├── Slab allocator with per-CPU magazines
  ├── Kernel heap (kalloc/kfree)
  ├── Per-process address spaces (TTBR0 switching)
  ├── Guard pages
  ├── Memory accounting per process
  └── Page fault handler (demand paging, COW)

Phase 3 — IPC and Capability System:
  ├── Shared memory regions (create, map, share)
  ├── Memory-mapped IPC (zero-copy transfers)
  └── Shared memory capability enforcement

Phase 8 — AIRS Core:
  ├── Model memory pool (huge pages, pinned)
  ├── Model loading via userfaultfd lazy loader (§6.4)
  ├── PagedAttention KV cache with block tables (§6.3)
  ├── KV prefix caching (cross-session sharing, COW)
  └── KV cache eviction policy

Phase 13 — Security Hardening:
  ├── PAC (pointer authentication) enabled for kernel + agents
  ├── BTI (branch target identification) enforcement
  ├── MTE (memory tagging) for agent heap allocations
  └── MTE for kernel heap allocations (synchronous mode)

Phase 14 — Performance and Optimization:
  ├── Background page zeroing thread
  ├── Cache coloring in buddy allocator
  ├── NEON-accelerated memory operations (memcpy, memset, zeroing)
  ├── Multi-size THP: 64 KB medium pages for agent heaps and KV caches (§2.2)
  ├── Multi-Generational LRU (MGLRU) with 4-generation aging (§10.2)
  ├── DAMON access pattern monitoring (§10.9)
  ├── zram compressed memory backend with LZ4/Zstd (§10.3)
  ├── Swap device initialization and slot management (§10.4)
  ├── Page fault paths for compressed/swapped pages (§10.5)
  ├── Swap readahead (adaptive sequential detection)
  ├── Thrash detection and agent suspension (§10.6)
  ├── SD card write throttle and wear monitoring (§10.7)
  ├── Page reclamation with MGLRU-driven three-tier hierarchy (§10.8)
  ├── Memory pressure monitoring
  └── OOM killer

Phase 15 — POSIX Compatibility:
  ├── mmap() / munmap() translation to AIOS syscalls
  ├── fork() with COW semantics
  ├── brk() / sbrk() for musl libc heap
  └── /proc/self/maps emulation
```

Phase 2 is on the critical path. Everything downstream — IPC, storage, GPU, compositor, AIRS — depends on having a working VMM. The buddy allocator and page table implementation must be correct and performant before any other kernel subsystem can function.
