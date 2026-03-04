# Phase 2: Memory Management

**Tier:** 1 — Hardware Foundation
**Duration:** 4 weeks
**Deliverable:** Virtual memory, heap, W^X, KASLR
**Status:** Planned
**Prerequisites:** Phase 1 (Boot and First Pixels)
**Unlocks:** Phase 3 (IPC & Capability System)

-----

## Objective

Build the full memory management subsystem on top of Phase 1's early boot allocators and identity-mapped page tables. Phase 1 established a bump allocator and minimal MMU configuration to get the kernel running. Phase 2 replaces these with production-grade components: a buddy allocator with pool partitioning, 4-level page tables with W^X enforcement, an ASID-tagged TLB scheme, KASLR, a slab allocator for kernel objects, and a typed kernel heap API.

By the end of this phase, the kernel manages physical memory through a buddy allocator partitioned into kernel/user/model/DMA pools, maps all kernel memory through proper TTBR1 page tables with W^X enforcement, randomizes its base address via KASLR, and provides a working `kalloc`/`kfree` heap. A test that allocates, writes, reads back, and frees memory through the heap confirms end-to-end correctness. Per-agent address spaces (TTBR0 switching) are also functional, preparing the ground for process isolation in Phase 3.

-----

## Architecture References

| Topic | Document | Relevant Sections |
|---|---|---|
| Physical memory manager & buddy allocator | [memory.md](../kernel/memory.md) | §2 Physical Memory Manager; §2.1 Bootstrap; §2.2 Buddy Allocator; §2.3 Frame Allocator Interface |
| Page pools | [memory.md](../kernel/memory.md) | §2.4 Page Pools |
| Virtual memory & address space layout | [memory.md](../kernel/memory.md) | §3 Virtual Memory Manager; §3.1 Address Space Layout; §3.2 Page Tables |
| KASLR | [memory.md](../kernel/memory.md) | §3.3 KASLR |
| TLB management & ASIDs | [memory.md](../kernel/memory.md) | §3.4 TLB Management |
| Slab allocator & kernel heap | [memory.md](../kernel/memory.md) | §4 Kernel Heap; §4.1 Slab Allocator; §4.2 Kernel Allocation API |
| Per-agent memory & address spaces | [memory.md](../kernel/memory.md) | §5.1 Agent Address Spaces; §5.2 Memory Accounting |
| W^X enforcement | [memory.md](../kernel/memory.md) | §9.1 W^X (Write XOR Execute) |
| Guard pages | [memory.md](../kernel/memory.md) | §9.5 Guard Pages |
| Implementation order | [memory.md](../kernel/memory.md) | §13 Implementation Order |
| BootInfo and memory map handoff | [boot.md](../kernel/boot.md) | §2.2 BootInfo struct; §3.3 Steps 3–9 |
| Security model (W^X, PAC, BTI) | [security.md](../security/security.md) | Memory isolation sections |

-----

## Milestones

Milestones are numbered continuously across all phases. Phase 1 used M4–M6; Phase 2 continues with M7–M9.

| Milestone | Steps | Target | Observable result |
|---|---|---|---|
| **M7 — Physical memory manager** | 1–3 | End of week 1 | Buddy allocator initialised from UEFI memory map; pool stats printed to UART |
| **M8 — Virtual memory & KASLR** | 4–7 | End of week 2 | Kernel mapped at randomised TTBR1 base with W^X; ASID allocator functional |
| **M9 — Kernel heap & per-agent address spaces** | 8–11 | End of week 4 | `kalloc`/`kfree` working; TTBR0 switching tested; CI passes |

-----

## Milestone 7 — Physical Memory Manager (End of Week 1)

*Goal: Replace the Phase 1 bump allocator with a buddy allocator partitioned into page pools. Boot log prints pool statistics.*

-----

### Step 1: Buddy Allocator Core

**What:** Implement the buddy allocator with orders 0–10 (4 KB – 4 MB), supporting `alloc(order)` and `free(frame, order)` with buddy merging.

**Tasks:**
- [ ] Create `kernel/src/mm/mod.rs` — memory management module root
- [ ] Create `kernel/src/mm/buddy.rs` — `BuddyAllocator` struct with free lists per order and a bitmap tracking allocated/free state (memory.md §2.2)
- [ ] Implement `alloc(order)` — try requested order, split from larger blocks if needed
- [ ] Implement `free(frame, order)` — merge with buddy if buddy is free, coalescing up to `MAX_ORDER`
- [ ] Implement `buddy_of(frame, order)` — XOR-based buddy address computation
- [ ] Add unit tests (host target) for alloc/free/split/merge sequences

**Key reference:** [memory.md §2.2](../kernel/memory.md) — Buddy Allocator

**Acceptance:** `just test` passes buddy allocator unit tests (alloc returns valid frames, free+realloc returns same frame, split/merge works across all orders).

-----

### Step 2: Page Pools and Frame Allocator

**What:** Partition physical memory into kernel/user/model/DMA pools based on detected RAM, and wrap with the `FrameAllocator` interface.

**Tasks:**
- [ ] Create `kernel/src/mm/pools.rs` — `PagePools` struct with four `BuddyAllocator` instances (memory.md §2.4)
- [ ] Implement `PoolConfig::from_total_ram(total)` — compute pool sizes per the table in memory.md §2.4 (2 GB/4 GB/8 GB/16 GB tiers)
- [ ] Create `kernel/src/mm/frame.rs` — `FrameAllocator` wrapping `PagePools` with `alloc_page`, `alloc_pages`, `free_pages`, and `pressure()` (memory.md §2.3)
- [ ] Implement `MemoryPressure` enum (Normal/Low/Critical/Oom) with thresholds from memory.md §2.3

**Key reference:** [memory.md §2.3–2.4](../kernel/memory.md) — Frame Allocator Interface, Page Pools

**Acceptance:** `just test` passes pool sizing tests (verify correct KB/MB values for each RAM tier). `FrameAllocator::pressure()` returns `Normal` after init with sufficient free pages.

-----

### Step 3: Bootstrap from UEFI Memory Map

**What:** Walk the `BootInfo` memory map (populated by Phase 1's UEFI stub) to initialise the buddy allocator and page pools.

**Tasks:**
- [ ] Create `kernel/src/mm/init.rs` — `init_memory(boot_info: &BootInfo)` entry point
- [ ] Walk `BootInfo` memory map regions (via `memory_map_addr`, `memory_map_count`, `memory_map_entry_size`): classify each as Conventional, LoaderCode, Reserved, MMIO, etc. (memory.md §2.1)
- [ ] Feed Conventional regions into the buddy allocator as initial free pages
- [ ] Mark LoaderCode/LoaderData regions as reclaimable (add to free list after early boot completes)
- [ ] Partition the buddy allocator's free pages into pools per `PoolConfig`
- [ ] Print pool statistics to UART: total RAM, per-pool sizes, free pages
- [ ] Replace Phase 1's bump allocator calls with `FrameAllocator` calls in existing kernel code

**Note:** The `BootInfo` memory map is passed as three fields: `memory_map_addr` (physical address of the `MemoryDescriptor` array), `memory_map_count` (number of entries), and `memory_map_entry_size` (bytes per entry). These are populated by Phase 1's UEFI stub. Phase 2 walks this array to bootstrap the buddy allocator and page pools.

**Key reference:** [memory.md §2.1](../kernel/memory.md) — Bootstrap; [boot.md §2.2](../kernel/boot.md) — BootInfo struct

**Acceptance:** `just run` prints pool statistics to UART:
```
[mm] Physical memory: <N> MB total
[mm] Pools: kernel=<X> MB, user=<Y> MB, model=<Z> MB, dma=<W> MB
[mm] Free pages: <F> / <T>
```
Values are consistent with the QEMU `-m 2G` configuration.

-----

## Milestone 8 — Virtual Memory & KASLR (End of Week 2)

*Goal: Full 4-level page tables with W^X, KASLR, ASID allocator, and TLB management replace Phase 1's identity mapping.*

-----

### Step 4: Page Table Infrastructure

**What:** Implement 4-level page table (PGD/PUD/PMD/PTE) data structures with W^X enforcement built into the PTE API.

**Tasks:**
- [ ] Create `kernel/src/mm/pgtable.rs` — `PageTable` (512 entries, 4 KB aligned), `PageTableEntry` with all aarch64 bit fields (memory.md §3.2)
- [ ] Implement PTE helpers: `is_valid`, `is_writable`, `is_executable`, `frame`, `set_writable` (clears exec), `set_executable` (sets read-only) — W^X enforced at API level
- [ ] Implement `AddressSpace` struct: PGD physical frame, ASID, VmRegion BTreeMap, MemoryStats (memory.md §3.2)
- [ ] Implement `AddressSpace::map_page(addr, frame, perms)` — walks/allocates intermediate tables, writes leaf PTE, asserts W^X
- [ ] Implement `AddressSpace::lookup_pte(addr)` — 4-level walk returning leaf PTE reference
- [ ] Implement `AddressSpace::unmap_page(addr)` — clears PTE, issues TLB invalidation
- [ ] Add `VmRegion`, `VmFlags` (with W^X constraint), and `VmRegionKind` types (memory.md §3.2)

**Key reference:** [memory.md §3.2](../kernel/memory.md) — Page Tables

**Acceptance:** `just test` passes page table unit tests: map_page creates valid PTE, W^X assertion fires on WRITE|EXECUTE, lookup_pte returns correct frame after mapping.

-----

### Step 5: Kernel Address Space and Direct Map

**What:** Build the kernel's TTBR1 page table tree: map kernel text (RX), data/BSS (RW), physical memory direct map (RW), and MMIO regions (RW device).

**Tasks:**
- [ ] Create `kernel/src/mm/kmap.rs` — `init_kernel_address_space()` builds the TTBR1 mapping
- [ ] Map kernel text section: read-only + executable (R-X)
- [ ] Map kernel data/BSS sections: read-write + no-execute (RW-)
- [ ] Map physical memory direct map at `0xFFFF_0001_0000_0000` — identity of all RAM, read-write + no-execute (memory.md §3.1)
- [ ] Map MMIO regions (UART, GIC, etc.) at `0xFFFF_0002_0000_0000` with device memory attributes (nGnRnE)
- [ ] Switch from Phase 1's identity/early page tables to the new TTBR1 tables: write `TTBR1_EL1`, issue `TLBI VMALLE1IS`, `DSB ISH`, `ISB`
- [ ] Verify kernel continues executing after the switch (UART still works)

**Note:** The direct map allows the kernel to access any physical address by adding `DIRECT_MAP_BASE`. This is how `PhysicalFrame::as_ptr()` works (memory.md §2.2).

**Key reference:** [memory.md §3.1](../kernel/memory.md) — Address Space Layout

**Acceptance:** `just run` — kernel prints to UART after switching to new TTBR1 page tables. `cargo objdump -- -h` shows kernel text section is mapped at virtual address `0xFFFF_0000_*`.

-----

### Step 6: KASLR

**What:** Randomize the kernel base address at boot using entropy from UEFI RNG, DTB, or ARM generic counter.

**Tasks:**
- [ ] Create `kernel/src/mm/kaslr.rs` — `KaslrConfig` struct (memory.md §3.3)
- [ ] Implement `compute_slide(entropy)` — 2 MB aligned slide within 0..128 MB range (64 possible positions)
- [ ] Read entropy source: try UEFI RNG (from BootInfo), fall back to DTB `/chosen/rng-seed`, last resort ARM generic counter (`CNTPCT_EL0`)
- [ ] Apply slide to kernel mapping before setting up TTBR1 page tables (integrate with Step 5)
- [ ] Print actual kernel base to UART for verification: `[kaslr] Kernel base: 0x<addr> (slide: 0x<slide>)`

**Key reference:** [memory.md §3.3](../kernel/memory.md) — KASLR

**Acceptance:** `just run` prints KASLR base address. Two consecutive boots show different slide values (non-deterministic). Kernel functions correctly at the randomised address.

-----

### Step 7: ASID Allocator and TLB Management

**What:** Implement 16-bit ASID allocation and TLB invalidation primitives needed for per-agent address spaces.

**Tasks:**
- [ ] Create `kernel/src/mm/asid.rs` — `AsidAllocator` with generation tracking (memory.md §3.4)
- [ ] Implement `alloc()` — returns `Asid { value, generation }`, wraps with full TLB flush at generation boundary
- [ ] Implement `is_valid(asid)` — checks generation match
- [ ] Create `kernel/src/mm/tlb.rs` — TLB invalidation wrappers:
  - `tlb_invalidate_page(asid, va)` → `TLBI VAE1IS`
  - `tlb_invalidate_asid(asid)` → `TLBI ASIDE1IS`
  - `tlbi_all()` → `TLBI VMALLE1IS`
- [ ] All invalidations include `DSB ISH` + `ISB` barriers
- [ ] Wire `tlb_invalidate_page` into `AddressSpace::update_pte` and `unmap_page`

**Key reference:** [memory.md §3.4](../kernel/memory.md) — TLB Management

**Acceptance:** `just test` passes ASID allocator tests (sequential alloc returns unique values, generation wraps correctly). TLB inline assembly compiles for aarch64 target.

-----

## Milestone 9 — Kernel Heap & Per-Agent Address Spaces (End of Week 4)

*Goal: Slab allocator, `kalloc`/`kfree`, per-agent TTBR0 switching, guard pages, and memory accounting. CI passes all gates.*

-----

### Step 8: Slab Allocator

**What:** Implement the slab allocator with per-CPU magazines for lock-free fast-path allocation of fixed-size kernel objects.

**Tasks:**
- [ ] Create `kernel/src/mm/slab.rs` — `SlabCache` and `SlabAllocator` (memory.md §4.1)
- [ ] Implement `SlabCache::new(name, size, fa)` — allocates one backing page, carves into freelist
- [ ] Implement `SlabCache::alloc()` — fast path: pop from magazine; slow path: refill from shared slab
- [ ] Implement `SlabCache::free(ptr)` — fast path: push to magazine; overflow: flush to shared slab
- [ ] Implement `Magazine` layer — `MagazineRound` with `MAGAZINE_SIZE = 32` object slots, current/prev swap
- [ ] Implement `SlabAllocator::init(fa)` — create standard caches: 64, 128, 256, 512, 4096 bytes (memory.md §4.1)
- [ ] Implement `SlabAllocator::alloc(size, align)` and `free(ptr, size)` — route to smallest fitting cache

**Key reference:** [memory.md §4.1](../kernel/memory.md) — Slab Allocator

**Acceptance:** `just test` passes slab allocator tests: alloc returns non-null aligned pointers, free + realloc cycle works, magazine fast path avoids slab lock.

-----

### Step 9: Kernel Heap API (`kalloc`/`kfree`)

**What:** Wire the slab and buddy allocators into the kernel-wide `kalloc<T>()`/`kfree<T>()` typed allocation API, and implement `#[global_allocator]` for `alloc` crate usage.

**Tasks:**
- [ ] Create `kernel/src/mm/heap.rs` — `kalloc<T>()` and `kfree<T>(ptr)` functions (memory.md §4.2)
- [ ] `kalloc`: use slab allocator for sizes ≤ largest cache; fall back to buddy allocator for larger allocations
- [ ] `kfree`: route to slab or buddy based on size
- [ ] Implement `GlobalAlloc` trait on a `KernelAllocator` struct — delegates to `kalloc`/`kfree` — enables `alloc::boxed::Box`, `alloc::vec::Vec`, etc.
- [ ] Register as `#[global_allocator]`
- [ ] Print heap ready message: `[mm] Kernel heap ready (slab caches: 64, 128, 256, 512, 4096)`
- [ ] Test: allocate a `Box<[u8; 1024]>`, write pattern, read back, drop — verify no panic

**Key reference:** [memory.md §4.2](../kernel/memory.md) — Kernel Allocation API

**Acceptance:** `just run` prints heap ready message. A kernel-mode allocation test (write + readback) succeeds without panic. `just check` passes with zero warnings.

-----

### Step 10: Per-Agent Address Spaces and TTBR0 Switching

**What:** Implement user-space address space creation (TTBR0) with ASID tagging, and a context-switch function that swaps TTBR0.

**Tasks:**
- [ ] Create `kernel/src/mm/uspace.rs` — `create_user_address_space()` → allocates PGD, assigns ASID, copies kernel mappings (TTBR1 entries) into upper half
- [ ] Implement `switch_address_space(new_as: &AddressSpace)` — writes new TTBR0 with ASID, issues appropriate barriers (no full TLB flush needed thanks to ASIDs)
- [ ] Implement guard pages: map a 4 KB unmapped page below stack and above text (memory.md §9.5) — access triggers synchronous fault
- [ ] Implement basic `MemoryStats` tracking per address space: pages allocated, peak usage
- [ ] Test: create two address spaces, switch between them, verify each can access its own mapping but not the other's

**Note:** Full process creation and scheduling are Phase 3. This step establishes the MMU mechanics that Phase 3 builds on.

**Key reference:** [memory.md §5.1](../kernel/memory.md) — Agent Address Spaces; [memory.md §9.5](../kernel/memory.md) — Guard Pages

**Acceptance:** `just run` prints:
```
[mm] Address space A created (ASID=1)
[mm] Address space B created (ASID=2)
[mm] TTBR0 switch: ASID 1 -> ASID 2
```
Guard page access triggers synchronous exception (caught by exception handler from Phase 0).

-----

### Step 11: Integration and CI

**What:** Wire all memory subsystem components into the boot sequence, run full quality gates, update CLAUDE.md.

**Tasks:**
- [ ] Integrate memory init into boot sequence: after Phase 1's early boot → call `init_memory(boot_info)` → buddy init → pool partition → KASLR → TTBR1 switch → slab init → heap ready
- [ ] Print complete boot memory summary to UART
- [ ] Verify `just check` (fmt + clippy + build) passes with zero warnings
- [ ] Verify `just test` passes all unit tests (buddy, pools, slab, page tables, ASID)
- [ ] Verify `just run` shows complete boot log through heap ready
- [ ] Update CLAUDE.md: add `kernel/src/mm/` to Workspace Layout, add new constants to Key Technical Facts

**Key reference:** [memory.md §13](../kernel/memory.md) — Implementation Order (Phase 2 items)

**Acceptance:** All quality gates pass:
```
just check   → zero warnings
just test    → all pass
just run     → boot log shows: pool stats, KASLR base, heap ready, address space test
```

-----

## Decision Points

| Decision | When | Options | Impact |
|---|---|---|---|
| KASLR entropy source | Step 6 | UEFI RNG vs DTB rng-seed vs counter | Weak entropy (counter only) is acceptable for QEMU; real hardware needs UEFI RNG or DTB seed |
| Slab cache sizes | Step 8 | Fixed set (64–4096) vs dynamic | Fixed set is simpler and sufficient for Phase 2; dynamic caches can be added later if profiling shows waste |
| Global allocator registration | Step 9 | Register immediately vs defer to Phase 3 | Registering now enables `alloc` crate usage in Phase 2; simplifies Phase 3 |
| Guard page placement | Step 10 | Stack-only vs stack+heap+text | Full guard pages (below stack, above text) catch more bugs; minimal performance cost |

-----

## Phase Completion Criteria

- [ ] Buddy allocator with orders 0–10, split/merge, O(log n) alloc/free
- [ ] Page pools partitioned by RAM tier (kernel/user/model/DMA)
- [ ] 4-level page tables (PGD/PUD/PMD/PTE) with W^X enforcement at API level
- [ ] Kernel mapped through TTBR1 with correct permissions: text=R-X, data=RW-, MMIO=device
- [ ] Physical memory direct map at `0xFFFF_0001_0000_0000`
- [ ] KASLR functional: different base address on each boot
- [ ] ASID allocator with generation tracking
- [ ] TLB invalidation primitives (`TLBI VAE1IS`, `TLBI ASIDE1IS`, `TLBI VMALLE1IS`)
- [ ] Slab allocator with per-CPU magazines and standard kernel caches
- [ ] `kalloc`/`kfree` and `#[global_allocator]` working
- [ ] Per-agent address spaces with TTBR0 switching
- [ ] Guard pages trigger synchronous exceptions
- [ ] `just check` — zero warnings
- [ ] `just test` — all unit tests pass
- [ ] `just run` — complete boot log through heap ready with pool stats and KASLR base
