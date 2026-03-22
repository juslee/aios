# Memory Kit

**Layer:** Kernel | **Architecture:** `docs/kernel/memory.md` + 5 sub-docs

## Purpose

Buffer allocation, address space management, zero-copy sharing, and memory-mapped regions. The foundation Kit — no Kit dependencies.

## Key APIs

| Trait / API | Description |
|---|---|
| `FrameAllocator` | Pool-aware physical page allocation and deallocation |
| `BuddyAllocator` | Orders 0-10 (4 KiB - 4 MiB), bitmap coalescing, poison fill |
| `SlabAllocator` | 5 size classes (64-4096B), magazine layer, red zones |
| `PageTableEntry` | 4-level page tables (PGD/PUD/PMD/PTE), W^X enforcement |
| `AddressSpace` | Per-agent virtual address space management |
| `UserAddressSpace` | User-space mapping via TTBR0, ASID tracking |
| `SharedMemoryRegion` | Zero-copy shared buffers between agents, W^X enforced |

## Consumers

All other Kits — Memory Kit is the foundation everything builds on.

## Implementation Phase

Phase 2 (physical memory, virtual memory, slab allocator) — **implemented**.
