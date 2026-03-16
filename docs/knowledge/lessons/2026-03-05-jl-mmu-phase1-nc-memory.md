---
author: Justin Lee
date: 2026-03-05
tags: [kernel, mmu, boot, memory]
status: final
---

# Lesson: TLBI hangs and Non-Cacheable Normal memory pitfalls

## What happened

During Phase 1 M5 (MMU setup), `tlbi alle1` + `dsb sy` caused the system to hang
when SMP cores were parked. The TLBI broadcast waited for completion on ALL PEs,
but parked secondary cores never acknowledged — deadlock.

Additionally, edk2 leaves the MMU ON after ExitBootServices with specific
MAIR/TCR settings. Attempting to change MAIR or TCR while the MMU is enabled
is CONSTRAINED UNPREDICTABLE per the ARM Architecture Reference Manual.

## Why it happened

- `dsb sy` is a full-system barrier that waits for TLBI completion on every PE
- Parked secondary cores (in `wfe` loop) don't process TLBI maintenance operations
- edk2 MAIR=0xffbb4400, TCR T0SZ=20 — these are baked in while MMU is on

## What we learned

1. Use `tlbi vmalle1` + `dsb nsh` (non-shareable, local-only) during init when
   secondary cores are parked. Switch to `tlbi vmalle1is` + `dsb ish` (broadcast)
   only after all cores are running with cacheable memory.
2. Never modify MAIR/TCR while MMU is on — reuse edk2's attribute indices.
3. Phase 1 strategy: TTBR0-only swap with edk2-compatible page tables.
4. Pool init: compute min/max of all usable UEFI regions, partition contiguous range
   linearly. UEFI memory map has many small descriptors that tile contiguously.

## How to avoid next time

- Always check if secondary cores are running before using broadcast TLBI
- Treat edk2 MAIR/TCR as immutable after ExitBootServices
- Test TLBI sequences with `-smp 4` (multi-core), not just single-core
