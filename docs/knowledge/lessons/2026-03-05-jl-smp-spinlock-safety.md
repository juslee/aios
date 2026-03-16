---
author: Justin Lee
date: 2026-03-05
tags: [kernel, smp, memory]
status: final
---

# Lesson: Exclusive monitors and spinlock safety on Non-Cacheable memory

## What happened

During Phase 1 M6 (SMP bringup), `spin::Mutex` and any atomic read-modify-write
operations (`compare_exchange`, `swap`, `fetch_add`) caused the system to hang
under multi-core contention. These operations appeared to work in single-core
testing but deadlocked with 4 cores.

## Why it happened

Atomic RMW operations in ARM use exclusive load/store pairs (`ldaxr`/`stlxr`).
These require the **global exclusive monitor**, which only functions correctly on
**Inner Shareable + Cacheable** memory.

Phase 1 identity map uses Non-Cacheable Normal memory (edk2 MAIR Attr1=0x44).
The global exclusive monitor doesn't track NC memory — `stlxr` always fails,
creating an infinite retry loop (spinlock hang).

## What we learned

1. On NC memory, only use `load(Acquire)` (`ldar`) / `store(Release)` (`stlr`) for
   inter-core synchronization — these are plain loads/stores with ordering guarantees,
   not exclusive pairs.
2. Turn-based protocol works: `PRINT_TURN` atomic counter where core N waits for
   `load == N`, performs its work, then `store(N+1)`.
3. `spin::Mutex` and all Rust atomic RMW are safe ONLY after Phase 2 M8 upgrades
   TTBR0 RAM blocks to Write-Back cacheable (Attr3).
4. Secondary MMU enable sequence must be exact: MAIR → TCR → TTBR0 → ISB → DSB SY → SCTLR → ISB

## How to avoid next time

- Never use `spin::Mutex` or atomic RMW on Non-Cacheable memory
- Always verify memory attributes before introducing lock-based synchronization
- PSCI CPU_ON: use `clobber_abi("C")`, never `options(nomem)` — it clobbers registers
