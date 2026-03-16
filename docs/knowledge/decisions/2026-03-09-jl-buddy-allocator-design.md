---
author: Justin Lee
date: 2026-03-09
tags: [kernel, memory]
status: final
---

# ADR: Buddy allocator over bitmap for physical memory

## Context

Phase 2 needed a physical page allocator. Two main approaches: bitmap allocator
(one bit per page) or buddy allocator (split/coalesce powers of two).

## Options Considered

### Option A: Bitmap allocator

- Pros: Simple implementation, O(n) scan, constant memory overhead
- Cons: External fragmentation, no large contiguous allocations, slow for large blocks

### Option B: Buddy allocator

- Pros: O(log n) alloc/free, natural coalescing prevents fragmentation, supports
  large contiguous allocations (up to 4 MiB at order 10), well-understood algorithm
- Cons: Internal fragmentation (rounds up to power of two), more complex implementation

## Decision

Buddy allocator (Option B). The coalescing property is critical for DMA buffers
and future large page support. Orders 0-10 (4 KiB to 4 MiB).

Implementation details:
- Bitmap-based coalescing (check buddy bit to decide merge)
- Poison fill on free (0xDEAD_DEAD) for use-after-free detection
- 4 separate pool instances: kernel (128 MB), user (remainder), model (0), DMA (64 MB)
- ~522K free pages on QEMU 2G configuration

## Consequences

- Slightly more memory overhead than bitmap (buddy metadata)
- Internal fragmentation for non-power-of-two allocations (mitigated by slab allocator on top)
- Excellent large-block allocation performance for DMA and framebuffers
