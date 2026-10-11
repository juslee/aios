---
author: jl + claude
date: 2026-10-06
tags: [kernel, sched, tooling]
status: final
---

# Lesson: Under QEMU TCG every branch inside an IRQs-on lock hold is a place the timer IRQ can land

## What happened

In crash-fix step 1b (task K5, 2026-09-28), adding instrumentation inside the IRQs-on hold of two kernel locks changed the very failure rate the step measures, although it changed no scheduling decision. The lock re-entry panic at the start of the bench hit 6 of 7, then 9 of 11, and after the fix below 3 of 11 text-mode boots, across successive builds. The previous task's build (K4) had 4 of 8 boots stuck at tick 0. The counts are small and were taken at different host loads; the mechanism is what matters.

## Why it happened

QEMU TCG takes a pending interrupt only at the start of a translation block (TB): `cpu_exec_loop` calls `cpu_handle_interrupt` between TBs, and a pending interrupt sets the exit request that the next TB entry checks (QEMU `accel/tcg/cpu-exec.c`, read at v11.1.0). Every conditional branch, call or return ends a TB. So each one inside a critical section is one more point where CPU 0's timer IRQ can arrive while the lock is held. For a lock that thread code holds with IRQs on, that is exactly the crash-fix H3 hazard: the IRQ path then needs the same lock (it spins forever on `main`; the step-1b IRQ-safe lock reports a `lock re-entry:` PANIC-LOCK instead).

In-hold TB starts on the usual path, measured per call site: `current_thread_id` 1 at the K4 baseline, about 11 in the first K5 build, 3 at the K5 commit, 1 after the review fix; `process_of_thread` 3, 5, 3; `current_process_id`'s `CURRENT_THREAD` hold 1, 4, 1. What restored K4 parity:

- A compare-and-swap that returns `Option<guard>` instead of `Result<guard, u64>` (at opt-level 1 LLVM re-tested the merged `Result` after the `stxr`).
- A branch-free restamp (`select_unpredictable` lowering to `csel`, then an unconditional `stlr`).
- `#[inline(always)]` on `lock` and `try_lock`: a non-inlined lock puts its `ret` inside the hold.
- Splitting `{ *lock() }?` into two statements. Edition 2021 keeps the guard alive through the `?` branch, and LLVM hoisted a one-store release above it but not a three-store release.

## What we learned

- Instrumentation placed inside an IRQs-on hold is not free under TCG, and the cost is in branches, not instructions.
- Holds taken with IRQs masked do not matter: no IRQ can land in them.
- Residual differences come from caller layout, for example a bounds proof lost when a callee stops being inlined.
- A system-register read is not a TB end under TCG in the case measured (K5b, 2026-09-29): `MPIDR` read as helper calls, `TPIDR` as an inline load, so swapping one for the other kept the TB counts.

The lock types named above are branch code from crash-fix step 1b, not on `main` at 368d99a; the technique applies to any lock held with IRQs on, `spin::Mutex` included.

## How to avoid next time

For code on the lock fast path, or between acquire and release of a lock that thread code holds with IRQs on, keep it inline and branch-free, and diff in-hold TB starts against the baseline ELF (build it from `git archive <baseline rev>` into a scratch directory).

A per-call-site sweep: in `llvm-objdump -d --no-show-raw-insn -C` output, find the acquire (`stxr`/`cbnz`, or `cas*`), walk to the releasing `stlr`, and count branches in between. Name the call site with `lldb -b <elf> -o "image lookup -a <addr>"`; it prints the function and source line (and the inline chain when one exists), and the sysroot has no `llvm-symbolizer`.

A faster whole-ELF check when a change should only swap an operand: disassemble both ELFs, split per function, normalise hex addresses of six digits or more, the `.llvm.<n>` suffixes and the swapped operand, then compare the mnemonic sequences and branch operands per function. In K5b, 701 of 707 functions were structurally identical and the other six were exactly the edited ones; the remaining differences were `adr`/`adrp`/`add` data offsets and key-index immediates.
