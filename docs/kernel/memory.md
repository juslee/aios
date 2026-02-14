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

**8 GB is the recommended minimum** for users who want the advertised AI-native experience. The model pool gets 4 GB on an 8 GB device, which fits a quantized 8B model with room for KV caches and embedding stores. At 4 GB, the model pool is only 2 GB — enough for a 3B model but not the 8B models that deliver meaningfully better reasoning. At 2 GB, the 1 GB model pool cannot fit any model alongside a running OS; AIOS falls back to cloud inference via the Network Translation Module.

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
/// Physical memory region from UEFI memory map
pub struct MemoryRegion {
    pub base: PhysicalAddress,
    pub size: usize,
    pub kind: MemoryType,
}

/// Classification of physical memory
pub enum MemoryType {
    /// Usable RAM — available for allocation
    Conventional,
    /// Kernel code/data — reclaimable after boot
    LoaderCode,
    /// MMIO — device registers, never allocatable
    Mmio,
    /// ACPI tables — reclaimable after parsing
    AcpiReclaim,
    /// Firmware reserved — never touch
    Reserved,
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
  4         16         64 KB      Slab backing
  5         32        128 KB      —
  6         64        256 KB      —
  7        128        512 KB      —
  8        256          1 MB      —
  9        512          2 MB      Huge page (model memory)
 10       1024          4 MB      Maximum contiguous allocation
```

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
    pub fn alloc(&self, order: u32) -> Option<PhysicalFrame> {
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
    pub fn free(&self, frame: PhysicalFrame, order: u32) {
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

    /// Current memory pressure level
    pub fn pressure(&self) -> MemoryPressure {
        let free_pct = (self.stats.free_pages * 100) / self.stats.total_pages;
        match free_pct {
            21..=100 => MemoryPressure::Normal,
            11..=20  => MemoryPressure::Low,
            6..=10   => MemoryPressure::Critical,
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
            },
            // Constrained tier: small model pool (1-3B models)
            r if r <= 4 * GB => PoolConfig {
                kernel: 256 * MB,
                model: 2 * GB,
                user: (r - 256 * MB - 2 * GB - 128 * MB - 128 * MB),
                dma: 128 * MB,
            },
            // Recommended tier: full model pool (8B Q4 models)
            r if r <= 8 * GB => PoolConfig {
                kernel: 256 * MB,
                model: 4 * GB,
                user: (r - 256 * MB - 4 * GB - 128 * MB - 128 * MB),
                dma: 128 * MB,
            },
            // Comfortable tier: large model pool (8B Q5/Q6 + specialists)
            r => PoolConfig {
                kernel: 256 * MB,
                model: 8 * GB,
                user: (r - 256 * MB - 8 * GB - 128 * MB - 128 * MB),
                dma: 128 * MB,
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
3. Compute slide: random value & ~(2MB - 1) within ±128 MB range
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

The slide range provides 64 possible positions at 2 MB alignment within ±128 MB — enough to thwart automated attacks while keeping kernel virtual memory layout predictable for debugging.

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
    /// Per-CPU magazine for lock-free fast path
    magazines: PerCpu<Magazine>,
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
    /// Loaded magazine (array of free object pointers)
    loaded: MagazineRound,
    /// Previous magazine (swap when loaded is empty)
    prev: MagazineRound,
}

pub struct MagazineRound {
    objects: [*mut u8; MAGAZINE_SIZE],
    count: usize,
}

const MAGAZINE_SIZE: usize = 32;

/// Top-level slab allocator managing all caches
pub struct SlabAllocator {
    caches: [SlabCache; NUM_CACHES],
}

impl SlabAllocator {
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
│  0x0040_0000  text  (R-X)│            │  0x0040_0000  text  (R-X)│
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
pub struct AgentProcess {
    pub pid: ProcessId,
    pub agent_id: AgentId,
    pub address_space: AddressSpace,
    pub memory_limit: usize,           // max RSS in bytes
    pub memory_stats: AgentMemoryStats,
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
2. Kernel sets agent state to Paused
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
/// Handle a page fault on a COW page
fn handle_cow_fault(
    addr_space: &mut AddressSpace,
    fault_addr: VirtualAddress,
) -> Result<(), FaultError> {
    let pte = addr_space.lookup_pte(fault_addr)?;

    if !pte.is_cow() {
        return Err(FaultError::AccessViolation);
    }

    let old_frame = pte.frame();
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
  - Kernel + firmware reserved:          384 MB
  - DMA pool:                            128 MB
  - OS services (compositor, space       200 MB
    storage, network, etc.):
  - Agent overhead (10 agents × 4 MB):    40 MB
  - Free headroom:                       ~150 MB
  ─────────────────────────────────────────────
  Available for model:                  ~3200 MB

Llama 3.1 8B at Q4_K_M:               ~4500 MB  ← does not fit
Llama 3.1 8B at Q3_K_S:               ~3200 MB  ← barely fits
Phi-3 Mini 3.8B at Q4_K_M:            ~2300 MB  ← fits, some headroom
Phi-3 Mini 3.8B at Q4_K_M + KV cache: ~2700 MB  ← fits, tight

On a 2 GB device:
  Available for model:                  ~1100 MB
  Smallest usable model: ~1B at Q4     ~700 MB   ← fits, limited capability
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

### 6.3 KV Cache Management

KV caches are the per-session cost of maintaining conversation context. Unlike model weights (which are static and shared), KV caches are dynamic, per-session, and can grow large:

```
KV cache size ≈ 2 × num_layers × head_dim × num_kv_heads × context_length × sizeof(f16)

Llama 3.1 8B:
  32 layers × 128 head_dim × 8 kv_heads × 8192 context = ~1 GB at f16
  With Q8 quantization: ~512 MB
  With Q4 quantization: ~256 MB
```

AIOS uses paged attention — KV caches are stored as fixed-size blocks, not as one contiguous allocation. This allows flexible memory management without fragmentation:

```rust
/// KV cache for a single inference session
pub struct KvCache {
    /// Session owning this cache
    pub session: SessionId,
    /// Fixed-size blocks holding KV data
    pub blocks: Vec<KvCacheBlock>,
    /// Current context length (tokens stored)
    pub context_length: u32,
    /// Maximum context length (model limit)
    pub max_context: u32,
    /// Total bytes allocated
    pub allocated_bytes: usize,
    /// Last time this cache was used
    pub last_used: Timestamp,
    /// Priority for eviction ordering
    pub priority: CachePriority,
}

/// Fixed-size block in the KV cache (1 MB)
pub struct KvCacheBlock {
    /// Physical frame(s) backing this block
    frame: PhysicalFrame,
    /// Number of token positions stored
    tokens_stored: u32,
    /// Block index in the cache
    index: u32,
}

pub enum CachePriority {
    /// User actively waiting (conversation bar)
    Interactive,
    /// System service (intent verification, context engine)
    System,
    /// Background work (space indexing)
    Background,
}

const KV_BLOCK_SIZE: usize = 1 * MB; // 1 MB blocks
```

**KV cache eviction** follows priority ordering when the model pool is under pressure:

```
Eviction order (first evicted → last evicted):
1. Background session KV caches (space indexing, metadata generation)
2. System session KV caches (intent verifier, behavioral monitor)
3. Idle interactive session KV caches (conversation bar idle > 5 min)
4. Active interactive session KV caches (never evicted — inference fails instead)
```

When a KV cache is evicted, the session's conversation history is still in a space object. The cache can be reconstructed by re-processing the conversation — slower than keeping it in RAM, but not data-losing.

### 6.4 Model Loading and Eviction

Models are loaded from space storage into the model pool. AIOS uses memory-mapped I/O where possible:

```
Model loading flow:

1. AIRS requests model load: model_id = "phi-3-mini-q4"
     ↓
2. Kernel allocates model pool pages (2 MB huge pages)
     ↓
3. Map GGUF file from space storage:
   - If backed by block device: mmap directly (demand-page from disk)
   - If in object store: copy into model pool pages
     ↓
4. AIRS maps the region read-only into its address space
     ↓
5. Model weights are demand-paged:
   - First access to a page triggers a page fault
   - Kernel reads the page from storage into the model pool frame
   - Subsequent accesses hit RAM directly
     ↓
6. After warmup (all pages faulted in), inference runs at full speed
```

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
/// A shared memory region managed by the kernel
pub struct SharedMemoryRegion {
    pub id: SharedMemoryId,
    /// Physical frames backing this region
    pub frames: Vec<PhysicalFrame>,
    /// Total size
    pub size: usize,
    /// Agents currently mapping this region
    pub mappings: Vec<SharedMapping>,
    /// Capability required to access
    pub capability: CapabilityTokenId,
}

pub struct SharedMapping {
    pub process: ProcessId,
    pub vaddr: VirtualAddress,
    pub flags: VmFlags,  // may be read-only for some mappers
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

-----

## 8. Memory Pressure and OOM

### 8.1 Memory Pressure Levels

The frame allocator continuously tracks free page counts across all pools. Pressure levels are computed from the user pool (model pool is pinned and excluded from pressure calculations):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPressure {
    /// > 20% free pages in user pool — normal operation
    Normal,
    /// 10-20% free — start background reclamation
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

Low       10-20%    - Reclaim clean page cache pages
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

### 10.2 Page Reclamation

The page reclaimer runs when memory pressure reaches `Low` or worse:

```rust
pub struct PageReclaimer {
    /// LRU list of reclaimable pages
    lru: LruList<PhysicalFrame>,
    /// Compressed memory backend
    zram: ZramBackend,
    /// Swap device (if configured)
    swap: Option<SwapDevice>,
}

pub struct ZramBackend {
    /// Compressed page storage (in RAM)
    compressed: HashMap<PhysicalFrame, CompressedPage>,
    /// Compression algorithm (LZ4 for speed)
    algorithm: CompressionAlgorithm,
    /// Memory saved by compression
    bytes_saved: usize,
}

pub struct CompressedPage {
    /// Compressed data (typically 1-2 KB for a 4 KB page)
    data: Vec<u8>,
    /// Original page's owner
    owner: ProcessId,
    /// Virtual address in owner's space
    vaddr: VirtualAddress,
}

impl PageReclaimer {
    pub fn reclaim(&mut self, target_pages: usize) -> usize {
        let mut reclaimed = 0;

        // Tier 1: clean page cache
        while reclaimed < target_pages {
            if let Some(frame) = self.lru.pop_clean() {
                self.free_clean_page(frame);
                reclaimed += 1;
            } else {
                break;
            }
        }

        // Tier 2: compress dirty pages
        while reclaimed < target_pages {
            if let Some(frame) = self.lru.pop_inactive_dirty() {
                if let Ok(saved) = self.zram.compress(frame) {
                    reclaimed += 1;
                } else {
                    break; // zram full
                }
            } else {
                break;
            }
        }

        // Tier 3: swap to disk (last resort)
        if reclaimed < target_pages {
            if let Some(ref mut swap) = self.swap {
                while reclaimed < target_pages {
                    if let Some(frame) = self.lru.pop_any() {
                        if swap.write_page(frame).is_ok() {
                            reclaimed += 1;
                        } else {
                            break; // swap full
                        }
                    } else {
                        break;
                    }
                }
            }
        }

        reclaimed
    }
}
```

-----

## 11. Performance Considerations

### 11.1 TLB Efficiency

TLB misses are expensive — each miss requires a 4-level page table walk (4 memory accesses). AIOS minimizes TLB misses through:

- **ASIDs:** Context switches do not flush the TLB. Entries from the previous process remain valid for that process's ASID.
- **Huge pages for model memory:** 2 MB pages reduce TLB entries needed for models by 512x.
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
    minimum: usize,                     // enough for companion embedding model
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
}
```

**Phase 14 optimization:** The model pool grows when AIRS loads a model (stealing pages from the user pool) and shrinks when the model is evicted (returning pages to the user pool). This eliminates the waste of pinning 4 GB for a model that may not be used for hours. The minimum reservation (enough for the embedding model) ensures Space Indexer can always operate.

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
3. **Model memory management is model-size-agnostic.** The same mmap + huge page + LRU eviction works for a 500 MB model or a 40 GB model.
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
  ├── Model loading via memory-mapped I/O
  ├── KV cache block allocator
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
  ├── zram compressed memory backend
  ├── Page reclamation and pressure monitoring
  └── OOM killer

Phase 15 — POSIX Compatibility:
  ├── mmap() / munmap() translation to AIOS syscalls
  ├── fork() with COW semantics
  ├── brk() / sbrk() for musl libc heap
  └── /proc/self/maps emulation
```

Phase 2 is on the critical path. Everything downstream — IPC, storage, GPU, compositor, AIRS — depends on having a working VMM. The buddy allocator and page table implementation must be correct and performant before any other kernel subsystem can function.
