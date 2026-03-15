# AIOS Memory Management

## Deep Technical Architecture

**Parent document:** [architecture.md](../project/architecture.md)
**Related:** [ipc.md](./ipc.md) — IPC shared memory, [airs.md](../intelligence/airs.md) — Model memory and KV caches, [development-plan.md](../project/development-plan.md) — Phase 2, [deadlock-prevention.md](./deadlock-prevention.md) — Deadlock prevention architecture (lock ordering §3, contention-reducing allocator §6)

-----

## Document Map

This document was split for navigability. Each sub-document preserves the original section numbers for cross-reference stability.

| Document | Sections | Content |
|---|---|---|
| **This file** | §1, §14 | Overview and implementation order |
| [physical.md](./memory/physical.md) | §2, §4 | Buddy allocator, page pools, frame allocator, slab allocator, kernel heap |
| [virtual.md](./memory/virtual.md) | §3, §5, §7 | Page tables, KASLR, TLB/ASID, per-agent address spaces, COW, shared memory |
| [ai.md](./memory/ai.md) | §6 | Model memory regions, PagedAttention KV caches, model loading/eviction |
| [reclamation.md](./memory/reclamation.md) | §8, §10, §12 | Memory pressure, OOM, MGLRU, zram, swap, DAMON, future scaling |
| [hardening.md](./memory/hardening.md) | §9, §11, §13 | W^X, PAC, BTI, MTE, guard pages, Spectre mitigations, performance, future directions |

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

**8 GB is the recommended minimum** for users who want the advertised AI-native experience. The model pool gets 4 GB on an 8 GB device, which fits a quantized 8B model with room for KV caches and embedding stores. At 4 GB, the model pool is only 2 GB — enough for a 3B model but not the 8B models that deliver meaningfully better reasoning. At 2 GB, there is no model pool (0 MB — see [§2.4](./memory/physical.md)); AIOS falls back to cloud inference via the Network Translation Module.

**Cloud inference fallback (2 GB devices):** When local inference is not viable, AIRS routes inference requests through the NTM to a configured cloud endpoint. The model pool is released to the user pool, giving agents and the browser more room. The system is fully functional — just slower (network latency) and dependent on connectivity. The user is informed at first boot: "This device has 2 GB RAM. AI features will use cloud processing. For local AI, 8 GB RAM is recommended."

-----

## 14. Implementation Order

Memory management spans several development phases:

```text
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

Phase 9 — AIRS Inference Engine:
  ├── Model memory pool (huge pages, pinned)
  ├── Model loading via userfaultfd lazy loader (§6.4)
  ├── PagedAttention KV cache with block tables (§6.3)
  ├── KV prefix caching (cross-session sharing, COW)
  └── KV cache eviction policy

Phase 17 — Security Architecture:
  ├── PAC (pointer authentication) enabled for kernel + agents
  ├── BTI (branch target identification) enforcement
  ├── MTE (memory tagging) for agent heap allocations
  └── MTE for kernel heap allocations (synchronous mode)

Phase 21 — Performance and Optimization:
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

Phase 22 — POSIX Compatibility:
  ├── mmap() / munmap() translation to AIOS syscalls
  ├── fork() with COW semantics
  ├── brk() / sbrk() for musl libc heap
  └── /proc/self/maps emulation
```

-----

## Cross-Reference Index

Quick lookup for commonly referenced sections across the memory sub-documents:

| Reference | Location | Topic |
|---|---|---|
| §2.2 BuddyAllocator | [physical.md](./memory/physical.md) | Buddy allocator struct, orders 0-10 |
| §2.3 FrameAllocator | [physical.md](./memory/physical.md) | Frame allocator API, pool routing |
| §2.4 PagePools | [physical.md](./memory/physical.md) | Pool sizing by RAM tier |
| §3.2 PageTableEntry | [virtual.md](./memory/virtual.md) | PTE bits, W^X API, AddressSpace |
| §3.3 KASLR | [virtual.md](./memory/virtual.md) | Kernel base randomization |
| §3.4 TLB/ASID | [virtual.md](./memory/virtual.md) | ASID allocator, TLB invalidation |
| §4.1 SlabAllocator | [physical.md](./memory/physical.md) | 5 size classes, magazine layer |
| §5.3 Memory Limit Enforcement | [virtual.md](./memory/virtual.md) | Per-agent enforcement |
| §5.4 COW | [virtual.md](./memory/virtual.md) | Copy-on-write fault handling |
| §6.2 ModelMemoryRegion | [ai.md](./memory/ai.md) | Pinned model weight regions |
| §6.3 PagedAttention | [ai.md](./memory/ai.md) | KV cache block tables |
| §6.4 userfaultfd loading | [ai.md](./memory/ai.md) | Lazy model loading |
| §7.1 SharedMemoryRegion | [virtual.md](./memory/virtual.md) | Zero-copy IPC |
| §8.1 MemoryPressure | [reclamation.md](./memory/reclamation.md) | Pressure levels and PSI |
| §8.2 OOM killer | [reclamation.md](./memory/reclamation.md) | Priority-based victim selection |
| §9.1 W^X | [hardening.md](./memory/hardening.md) | Write XOR Execute enforcement |
| §9.4 MTE | [hardening.md](./memory/hardening.md) | Memory Tagging Extension |
| §9.5 Guard pages | [hardening.md](./memory/hardening.md) | Stack overflow protection |
| §10.2 MGLRU | [reclamation.md](./memory/reclamation.md) | 4-generation page aging |
| §10.3 zram | [reclamation.md](./memory/reclamation.md) | Compressed memory |
| §10.5 Page faults | [reclamation.md](./memory/reclamation.md) | Compressed/swapped page handling |
| §10.9 DAMON | [reclamation.md](./memory/reclamation.md) | Access pattern monitoring |
| §11.1 TLB efficiency | [hardening.md](./memory/hardening.md) | FEAT_CONTPTE, FEAT_TLBIRANGE |
| §12.2 Dynamic model pool | [reclamation.md](./memory/reclamation.md) | Runtime pool resizing |
| §13 Future directions | [hardening.md](./memory/hardening.md) | Research-informed improvements |
