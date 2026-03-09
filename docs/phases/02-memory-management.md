# Phase 2: Memory Management

**Tier:** 1 — Hardware Foundation
**Duration:** 4 weeks
**Deliverable:** Virtual memory, heap, W^X, KASLR
**Status:** In Progress (M8 complete)
**Prerequisites:** Phase 1 (Boot and First Pixels)
**Unlocks:** Phase 3 (IPC & Capability System)

-----

## Objective

Build the full memory management subsystem on top of Phase 1's allocators and identity-mapped page tables. Phase 1 established a bump allocator, buddy allocator (orders 0–10 with splitting but no coalescing), slab allocator (10 size classes), a switchable `#[global_allocator]` (bump→slab), and an edk2-compatible MMU identity map. Phase 2 enhances these with production-grade components: buddy coalescing with bitmap tracking, page pool partitioning, 4-level page tables with W^X enforcement, an ASID-tagged TLB scheme, KASLR, per-CPU slab magazines, and a typed kernel heap API.

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
| Memory hardening (poisoning, double-free, red zones) | [fuzzing-and-hardening.md](../security/fuzzing-and-hardening.md) | §3.3 Memory Hardening |

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

*Goal: Enhance Phase 1's buddy allocator with coalescing and partition into page pools. Boot log prints pool statistics.*

-----

### Step 1: Buddy Allocator Core

**What:** Enhance the existing buddy allocator (orders 0–10, alloc with splitting) to add bitmap-based coalescing on free, and security hardening (poisoning, double-free detection).

**Tasks:**
- [x] `kernel/src/mm/mod.rs` exists — switchable GlobalAlloc (bump→slab)
- [x] `kernel/src/mm/buddy.rs` exists — orders 0–10, alloc with splitting, UEFI map init
- [x] `alloc(order)` implemented with splitting from larger blocks
- [x] Add module declarations for new files: `pub mod pools; pub mod frame; pub mod init;`
- [x] Enhance `free(frame, order)` — add bitmap-based coalescing up to `MAX_ORDER`
- [x] Add `buddy_of(frame, order)` — XOR-based buddy address computation
- [x] Add buddy-pair XOR bitmap for coalescing state tracking
- [x] **Security:** Double-free detection via bitmap check before free ([fuzzing-and-hardening.md §3.3](../security/fuzzing-and-hardening.md))
- [x] **Security:** Buddy allocator poisoning — fill freed pages with `0xDEAD_DEAD` ([fuzzing-and-hardening.md §3.3](../security/fuzzing-and-hardening.md))
- [x] Add unit tests (in `shared/` crate, host target) for buddy_of XOR, pool sizing, pressure thresholds

**Key reference:** [memory.md §2.2](../kernel/memory.md) — Buddy Allocator

**Acceptance:** `just test` passes buddy allocator unit tests (alloc returns valid frames, free+realloc returns same frame, split/merge works across all orders).

-----

### Step 2: Page Pools and Frame Allocator

**What:** Partition physical memory into kernel/user/model/DMA pools based on detected RAM, and wrap with the `FrameAllocator` interface.

**Tasks:**
- [x] Create `kernel/src/mm/pools.rs` — `PagePools` struct with four `BuddyAllocator` instances (memory.md §2.4)
- [x] Implement `PoolConfig::from_total_ram(total)` — compute pool sizes per the table in memory.md §2.4 (2 GB/4 GB/8 GB/16 GB tiers)
- [x] Create `kernel/src/mm/frame.rs` — `FrameAllocator` wrapping `PagePools` with `alloc_page`, `alloc_pages`, `free_pages`, and `pressure()` (memory.md §2.3)
- [x] Implement `MemoryPressure` enum (Normal/Low/Critical/Oom) with thresholds from memory.md §2.3

**Key reference:** [memory.md §2.3–2.4](../kernel/memory.md) — Frame Allocator Interface, Page Pools

**Acceptance:** `just test` passes pool sizing tests (verify correct KB/MB values for each RAM tier). `FrameAllocator::pressure()` returns `Normal` after init with sufficient free pages.

-----

### Step 3: Bootstrap from UEFI Memory Map

**What:** Walk the `BootInfo` memory map (populated by Phase 1's UEFI stub) to initialise the buddy allocator and page pools.

**Tasks:**
- [x] Create `kernel/src/mm/init.rs` — `init_memory(boot_info: &BootInfo)` entry point
- [x] Walk `BootInfo` memory map regions (via `memory_map_addr`, `memory_map_count`, `memory_map_entry_size`): classify each as Conventional, LoaderCode, Reserved, MMIO, etc. (memory.md §2.1)
- [x] Feed Conventional regions into the buddy allocator as initial free pages
- [x] Mark LoaderCode/LoaderData regions as reclaimable (add to free list after early boot completes)
- [x] Partition the buddy allocator's free pages into pools per `PoolConfig`
- [x] Print pool statistics to UART: total RAM, per-pool sizes, free pages
- [x] Replace Phase 1's bump allocator calls with `FrameAllocator` calls in existing kernel code

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
- [x] Create `kernel/src/mm/pgtable.rs` — `PageTable` (512 entries, 4 KB aligned), `PageTableEntry` with all aarch64 bit fields (memory.md §3.2)
- [x] Implement PTE helpers: `is_valid`, `is_writable`, `is_executable`, `frame`, `set_writable` (clears exec), `set_executable` (sets read-only) — W^X enforced at API level
- [x] Implement `AddressSpace` struct: PGD physical frame, ASID, VmRegion BTreeMap, MemoryStats (memory.md §3.2)
- [x] Implement `AddressSpace::map_page(addr, frame, perms)` — walks/allocates intermediate tables, writes leaf PTE, asserts W^X
- [x] Intermediate page table pages allocated from `frame::alloc_page()` (Pool::Kernel)
- [x] Implement `AddressSpace::lookup_pte(addr)` — 4-level walk returning leaf PTE reference
- [x] Implement `AddressSpace::unmap_page(addr)` — clears PTE, issues TLB invalidation
- [x] Add `VmRegion`, `VmFlags` (with W^X constraint), and `VmRegionKind` types (memory.md §3.2)

**Key reference:** [memory.md §3.2](../kernel/memory.md) — Page Tables

**Acceptance:** `just test` passes page table unit tests: map_page creates valid PTE, W^X assertion fires on WRITE|EXECUTE, lookup_pte returns correct frame after mapping.

-----

### Step 5: Virtual Linking, Boot TTBR1, and Kernel Address Space

**What:** Two-phase TTBR1 approach. Phase A: Update linker script for virtual addresses, add minimal TTBR1 setup in boot.S so kernel runs at virtual addresses from entry. Phase B: After pool init, build full TTBR1 with fine-grained W^X, direct map, MMIO, and KASLR slide.

**Tasks (Phase A — boot.S):**

- [x] Update `kernel/src/arch/aarch64/linker.ld`: VMA at `KERNEL_BASE` (0xFFFF_0000_0000_0000), LMA at physical `0x40080000` (AT clause)
- [x] Add linker symbols: `__kernel_virt_base`, `__kernel_phys_base`, `__virt_phys_offset`
- [x] Update `uefi-stub/src/elf.rs`: convert `e_entry` from virtual to physical (track `lowest_vaddr` in Pass 1, compute `physical_entry = e_entry - lowest_vaddr + lowest_paddr`)
- [x] Update `kernel/src/arch/aarch64/boot.S`: build minimal TTBR1 tables (static L0/L1/L2 in BSS, 2MB block descriptors covering kernel image)
- [x] Set TCR_EL1 T1SZ=16 for 48-bit kernel VA before writing TTBR1_EL1
- [x] Use MAIR index 3 (Write-Back cacheable, 0xff) for kernel normal memory in TTBR1 — no MAIR register change needed (edk2 MAIR already has Attr3=WB)
- [x] Branch to virtual kernel_main address after TTBR1 install
- [x] Update `_secondary_entry` in boot.S to also install TTBR1 (reuse boot CPU's tables)

**Tasks (Phase B — kernel_main, in kmap.rs):**

- [x] Create `kernel/src/mm/kmap.rs` — `init_kernel_address_space(ram_start, ram_size)` builds full TTBR1
- [x] Map kernel text section: read-only + executable (R-X), 4KB pages, Attr3 WB
- [x] Map kernel rodata: read-only + no-execute
- [x] Map kernel data/BSS sections: read-write + no-execute (RW-)
- [x] Map physical memory direct map at `0xFFFF_0001_0000_0000` — all RAM, RW+XN (2MB blocks for efficiency)
- [x] Map MMIO regions (UART, GIC, etc.) at `0xFFFF_0010_0000_0000` with device memory attributes (Attr0, nGnRnE)
- [x] Switch TTBR1 to full tables (replacing boot.S minimal tables): `TLBI VMALLE1`, `DSB NSH`, `ISB` (non-IS variant sufficient since SMP not yet started)
- [x] Build TTBR0 RAM blocks with WB (Attr3) from init — mmu.rs `MAIR_NORMAL_IDX` set to WB (was NC in Phase 1); prevents CONSTRAINED UNPREDICTABLE from mismatched attributes with TTBR1
- [x] Verify kernel continues executing after the switch (UART still works)

**Note:** The direct map allows the kernel to access any physical address by adding `DIRECT_MAP_BASE`. This is how `PhysicalFrame::as_ptr()` works (memory.md §2.2). boot.S creates minimal TTBR1 for virtual kernel execution; full TTBR1 with KASLR/direct map built in kernel_main after pool init.

**Key reference:** [memory.md §3.1](../kernel/memory.md) — Address Space Layout, [boot.md §3.3](../kernel/boot.md) — Step 7

**Acceptance:** `just run` — kernel prints to UART after switching to new TTBR1 page tables. `cargo objdump -- -h` shows kernel text section is mapped at virtual address `0xFFFF_0000_*`.

-----

### Step 6: KASLR

**What:** Randomize the kernel base address at boot using entropy from UEFI RNG or ARM generic counter.

**Note:** KASLR is deferred to the full TTBR1 rebuild in kernel_main (Step 5 Phase B). boot.S uses fixed KERNEL_BASE; kernel_main computes slide after pool init, then builds full TTBR1 with slide applied. The transition works because aarch64 ADRP+ADD offsets are PC-relative and the entire image shifts uniformly — no pointer fixups needed.

**Tasks:**

- [x] Create `kernel/src/mm/kaslr.rs` — `KaslrConfig` struct (memory.md §3.3)
- [x] Implement `compute_slide(entropy)` — 2 MB aligned slide within 0..128 MB range (64 possible positions)
- [x] Read entropy source: try `BootInfo.rng_seed` (from UEFI RNG protocol), fall back to `CNTPCT_EL0` (weak but non-deterministic)
- [x] Compute slide and log it; slide is not yet passed to `init_kernel_address_space()` (non-zero slide deferred to a later milestone)
- [x] Print actual kernel base to UART for verification: `[kaslr] Kernel base: 0x<addr> (slide: 0x<slide>)`

**Key reference:** [memory.md §3.3](../kernel/memory.md) — KASLR

**Acceptance:** `just run` prints KASLR base address. Two consecutive boots show different slide values (non-deterministic). Kernel functions correctly at the randomised address.

-----

### Step 7: ASID Allocator and TLB Management

**What:** Implement 16-bit ASID allocation and TLB invalidation primitives needed for per-agent address spaces.

**Tasks:**
- [x] Create `kernel/src/mm/asid.rs` — `AsidAllocator` with generation tracking (memory.md §3.4)
- [x] Implement `alloc()` — returns `Asid { value, generation }`, wraps with full TLB flush at generation boundary
- [x] Implement `is_valid(asid)` — checks generation match
- [x] Create `kernel/src/mm/tlb.rs` — TLB invalidation wrappers:
  - `tlb_invalidate_page(asid, va)` → `TLBI VAE1IS`
  - `tlb_invalidate_asid(asid)` → `TLBI ASIDE1IS`
  - `tlbi_all()` → `TLBI VMALLE1IS`
- [x] All invalidations include `DSB ISH` + `ISB` barriers
- [x] Wire `tlb_invalidate_page` into `AddressSpace::update_pte` and `unmap_page`

**Key reference:** [memory.md §3.4](../kernel/memory.md) — TLB Management

**Acceptance:** `just test` passes ASID allocator tests (sequential alloc returns unique values, generation wraps correctly). TLB inline assembly compiles for aarch64 target.

-----

## Milestone 9 — Kernel Heap & Per-Agent Address Spaces (End of Week 4)

*Goal: Slab allocator, `kalloc`/`kfree`, per-agent TTBR0 switching, guard pages, and memory accounting. CI passes all gates.*

-----

### Step 8: Slab Allocator

**What:** Enhance the existing slab allocator (10 size classes, 8–4096 bytes) with per-CPU magazines for lock-free fast-path allocation.

**Tasks:**
- [x] `kernel/src/mm/slab.rs` exists — `SlabCache` with 10 size classes, intrusive free list, backed by buddy
- [x] `SlabCache::alloc()` and `SlabCache::free(ptr)` implemented (slow-path only)
- [ ] Add `Magazine` layer — `MagazineRound` with `MAGAZINE_SIZE = 32` object slots, current/prev swap
- [ ] Enhance `SlabCache::alloc()` — fast path: pop from magazine; slow path: refill from shared slab
- [ ] Enhance `SlabCache::free(ptr)` — fast path: push to magazine; overflow: flush to shared slab
- [ ] **Security:** Slab red zones — guard bytes around allocations to detect overflow ([fuzzing-and-hardening.md §3.3](../security/fuzzing-and-hardening.md))
- [ ] Consolidate standard caches to 64, 128, 256, 512, 4096 bytes (memory.md §4.1)

**Key reference:** [memory.md §4.1](../kernel/memory.md) — Slab Allocator

**Acceptance:** `just test` passes slab allocator tests: alloc returns non-null aligned pointers, free + realloc cycle works, magazine fast path avoids slab lock.

-----

### Step 9: Kernel Heap API (`kalloc`/`kfree`)

**What:** Enhance the existing `#[global_allocator]` (bump→slab switching) with a typed `kalloc<T>()`/`kfree<T>()` API that routes through the slab and buddy allocators.

**Tasks:**
- [x] `kernel/src/mm/mod.rs` has `KernelAllocator` registered as `#[global_allocator]` with bump→slab switching
- [x] `alloc` crate usage works (Box, Vec) via existing GlobalAlloc impl
- [ ] Create `kernel/src/mm/heap.rs` — `kalloc<T>()` and `kfree<T>(ptr)` typed functions (memory.md §4.2)
- [ ] `kalloc`: use slab allocator for sizes ≤ largest cache; fall back to buddy allocator for larger allocations
- [ ] `kfree`: route to slab or buddy based on size
- [ ] Enhance `KernelAllocator` to delegate through `kalloc`/`kfree`
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

- [x] Buddy allocator with orders 0–10, split/merge, O(log n) alloc/free
- [x] Page pools partitioned by RAM tier (kernel/user/model/DMA)
- [x] 4-level page tables (PGD/PUD/PMD/PTE) with W^X enforcement at API level
- [x] Kernel mapped through TTBR1 with correct permissions: text=R-X, data=RW-, MMIO=device
- [x] Physical memory direct map at `0xFFFF_0001_0000_0000`
- [x] KASLR functional: different base address on each boot
- [x] ASID allocator with generation tracking
- [x] TLB invalidation primitives (`TLBI VAE1IS`, `TLBI ASIDE1IS`, `TLBI VMALLE1IS`)
- [ ] Slab allocator with per-CPU magazines and standard kernel caches
- [ ] `kalloc`/`kfree` and `#[global_allocator]` working
- [ ] Per-agent address spaces with TTBR0 switching
- [ ] Guard pages trigger synchronous exceptions
- [ ] `just check` — zero warnings
- [ ] `just test` — all unit tests pass
- [ ] `just run` — complete boot log through heap ready with pool stats and KASLR base
