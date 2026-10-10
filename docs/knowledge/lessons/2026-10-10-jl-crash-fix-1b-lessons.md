---
author: jl + claude
date: 2026-10-10
tags: [kernel, sched, smp, platform, tooling]
status: final
---

# Lesson: Instrumenting the IRQ path under QEMU TCG (crash-fix step 1b)

Crash-fix step 1b added detect-only tripwires to the scheduler, IPC and IRQ paths: a stamped IRQ-class lock, per-CPU counters, heartbeat scans and fatal dumps. The [crash-fix ADR](../decisions/2026-09-22-jl-crash-fix-preemption-and-fp.md) holds the decisions ("Step 1b decisions, 2026-10-10" and the 2026-10-09 errata under "Review notes"), [observability.md §6.5](../../kernel/observability.md) the line format, and [the B1 research note](../research/2026-10-10-jl-crash-fix-1b-n2-baseline.md) the baseline data. These are the lessons that apply beyond step 1b.

## 1. Under TCG, code inside a critical section sets the race window

**What happened.** The first build of the detect-only lock moved the holder bookkeeping after its CAS into an out-of-line call. That put about 11 translation-block starts inside `current_thread_id`'s `CURRENT_THREAD` hold, where `spin::Mutex` had 1, and about 12 inside `current_process_id`'s `THREAD_TABLE` hold, where it had about 3. That build gave a PANIC-LOCK (the H3 re-entry, caught) in 6 of 7 text boots. After the in-hold path was cut back to `spin::Mutex`'s translation-block starts, it fell to 3 of 11 (one-sided Fisher p ≈ 0.015 against the 9 of 11 across the first two builds).

**Why.** QEMU TCG takes a pending interrupt only where a translation block starts, and every taken branch, call or return starts one. The number of IRQ windows inside a hold, not its length in instructions, sets how often an IRQ lands in it. The extra starts came from three places:
- an out-of-line helper;
- a `Result<guard, u64>` from the CAS that opt-level 1 tested again after the CAS;
- `#[inline]` rather than `#[inline(always)]`, which left `lock` out of line at some sites and put a `ret` inside the hold.

One more came from the source, not the lock. In edition 2021 a block's tail temporaries live to the end of the statement, so `{ *LOCK.lock() }?` ran the `?` test inside the hold. The old one-store release was hoisted above that branch, but the new three-store release was not.

**Rule.** When you change a lock, or any code that runs inside an IRQs-on critical section, count the translation-block starts per hold at every inlined site from `llvm-objdump`, and keep them equal to the baseline. Keep the in-hold path straight-line: inline always, an `Option` rather than a `Result` from the CAS, and a branch-free re-stamp. Split a guard expression from the `?` that follows it. Otherwise the instrumentation changes the failure rate it is there to measure.

## 2. Under TCG, an MPIDR_EL1 read costs two helper calls; take a hot-path CPU id from TPIDR_EL1

**What happened.** The lock's stamp read the CPU id from MPIDR_EL1: 4 reads and 1 DAIF read per `lock()`, about 135 MPIDR reads per Gate 1 IPC round trip (33 lock acquisitions). The round trip went from an average of 4–7 µs to 10–13 µs, and Gate 1's `IPC < 10 us` failed in most boots. Two interleaved ablation arms placed the cost:
- `g`: the contended path cut to a bare spin, stamps kept. It matched the full lock.
- `fg`: the stamps removed as well. It matched the old `spin::Mutex`.

So the stamp reads were the whole cost.

**Why.** TCG emulates each MPIDR_EL1 read with two C helper calls and a full guest-register spill, while a TPIDR_EL1 read is one inline load. Writing MPIDR Aff0 to TPIDR_EL1 once per CPU in `boot.S` and stamping from it recovered about 2.8 µs in a paired experiment. Why a few system-register reads cost microseconds per round trip was not measured.

**Rule.** A per-operation CPU id on a hot path comes from TPIDR_EL1 (set in `boot.S` before any Rust code), with a count-only cross-check against MPIDR (`tpidrbad`). Step 8's `ThreadInfo` will take TPIDR_EL1 over, so the CPU id must move first (the ADR's erratum "TPIDR_EL1 is in use"). Before optimising a TCG cost, find it with interleaved ablation arms, as above.

## 3. The IRQ path saves no V registers: keep wide copies and popcounts off it, and check with a differential listing

**What happened.** The EL1 IRQ entry saves the general registers only (until crash-fix step 4), so any V-register use on the IRQ path clobbers the interrupted thread's FP/SIMD state (the ADR's H5). Step 1b found these sources:
- `u64::count_ones` compiles to NEON (`fmov d, x`; `cnt v.8b`; `addv`) on `aarch64-unknown-none`, which has no FEAT_CSSC. The scan code used it, and it now counts with a 16-entry nibble table.
- `main` already uses V registers on the IRQ path. `LogMessageBuf::new` zero-fills with `movi v0.2d` and `str q0`, and `::entries` has `stp q0, q0`. Both are reached through `log_impl` from the balancer's `kinfo!` and from `assert_valid_ctx` in `schedule()`. `memset` uses V registers on its misaligned-head and 8–15-byte-tail paths.
- Moving or zeroing an aggregate of 16 bytes or more can become a q-register copy or a `memset`/`memcpy` call. Step 1b kept these off the path:
  - `clear_timeout` stores `None` instead of `take()`, which would move a 24-byte entry;
  - `unblock` writes its 16-byte outcome in place through the return slot;
  - the 16-byte queue iterator is consumed in a `for` loop, so it stays in registers;
  - tables that a loop walks are `static`, not `const`, so a loop never copies one to the stack;
  - the scans keep their accumulators in static atomics and pass scalars, not stack structs.
- The dev build's overflow checks put panic calls on the IRQ path: an `enumerate()` counter in the scans did, until the loops became `0..n` ranges with `get`.

**Why.** At opt-level 1 the compiler chooses q-register copies and library calls for wide moves, and inlining moves new code into existing symbols, so a per-symbol check of new functions misses it. LLVM also drops stores to globals nothing reads yet, so a function's instruction count grows later, when a reader lands.

**Rule.**
- Never call `count_ones` (or another bit-count intrinsic) on the IRQ path. Pass scalars, keep wide state in statics, and write aggregates in place.
- Check early: a throwaway `no_std` staticlib in the scratchpad that instantiates every IRQ-path generic at opt-level 1 and 3, then `llvm-objdump` for V-register, NEON, `memcpy`/`memset`/`memmove`, `blr` and panic calls.
- Gate at the end with a differential listing (step 1b's V1). Use the toolchain sysroot's `llvm-objdump -d -l -C` (cargo-binutils is not installed). Collect the call graph from `irq_el1_entry` by following direct branches, not `blr`/`br`, and stop at halt-terminal functions. Then require source-line parity: every V-register site in the branch's graph sits on a source line that has one in the baseline's graph, and none sits on a line the branch added.
- Build the baseline at the branch's merge base with the same toolchain. A baseline from before a merged change (#219 here) charges that change's sites to the branch.

## 4. TCG timing cannot validate microsecond bounds set from hardware intuition

**What happened.** The design gave the heartbeat scan a 100 µs hold bound and the tripwire print a 3 ms bound, from a hardware-scale estimate and a UART model of 4.7 µs per byte. Measured, scan A held for 333–640 µs, already at load 9, and the print bound failed in its first boot: `twmax` is a maximum over all lines, and the single `g1` line costs 190,000–330,000 CNTVCT ticks (3–5 ms). The Gate 1 IPC average and p99 moved with host load from boot to boot. The minimum was the steadiest figure: every lock-build minimum but one was 6,992 ns or more, and every earlier one 4,992 ns or less.

**Why.** Under TCG, CNTVCT measures emulated time, which translation, helper calls and host load all inflate, so code that is microseconds on hardware is not microseconds in QEMU.

**Rule.**
- Under TCG, set a timing bound from a measured prototype boot, not from a model.
- State the bound per line kind or path, never on a key that takes a maximum over several kinds.
- Compare minima between the arms of an interleaved pair. Never gate on an absolute microsecond figure from one boot.
- When a bound fails, record the measured range and decide on it (step 1b's became #236) rather than tuning the bound.

## 5. Boot gates and soaks on a shared, loaded host

**What happened.**
- Other sessions kept the host's load high for days (up to 408), so the boot gates of seven tasks (four kernel tasks and three merges of `main`) were deferred under the rule "above 30, do not boot". One consolidated gate at the final head covered them.
- Single boots misled more than once. One PCZERO inside the bench looked like a regression of the bench change; one boot cannot tell. Three tick-0 WEDGEs in a row looked like a new defect, until boots interleaved with the previous head came out mostly CLEAN on both, and tick-0 WEDGE proved to be a `main` class (5–6 of 20 in earlier `main` soaks).
- Survivor comparisons were confounded by load: the lock builds booted at load 10–55, the `main` baseline at 6.5–24.
- The only log of a new signature (a PC equal to a lock word) was lost when its experiment's output directory was deleted.

**Why.** The classes vary from boot to boot, and load inflates both the hangs and the timings. One 30- or 75-second boot gives a class, never a rate.

**Rule.**
- Record the host's load with every boot. Above the threshold, defer the boot, list it as owed with its acceptance in the step's working plan or handoff, and run every owed boot in one quiet window before the PR.
- Never attribute a class from one boot or from sequential batches. Interleave the arms.
- Run a host-exclusive soak (a baseline or an A/B pair) only after coordinating with every other session on the host, and kill only your own QEMU process.
- Before deleting a soak or experiment directory, copy out any log that holds an unexplained signature.

## 6. A per-CPU counter found an invariant nobody had checked (#200)

**What happened.** The first per-CPU `tick` counter read `tick=N,0,0,0` in every boot: CPUs 1–3 never took a timer IRQ. Two independent checks confirmed it:
- QEMU `-d int` logged 1069 IRQ exceptions, all on CPU 0;
- the QEMU monitor (`xp`) showed `GICR_IGROUPR0` = 0xffffffff on CPU 0 and 0 on CPUs 1–3, with PPI 30 enabled and pending on all four.

The ADR's N5 and N7 reasoning assumed every CPU ticks, and the two-strike scan gate as first written ("every online CPU's `tick` advances by 100") could never confirm a flag on this kernel.

**Why.** edk2 configures only the boot CPU's redistributor (Homebrew edk2 on QEMU 11.1; CI's firmware was not checked). `init_gicv3_secondary` enables PPI 30 but never writes its group, so it stays in Group 0, which the kernel never enables (it enables Group 1 only).

**Rule.**
- A secondary CPU's GIC bring-up sets the group of every interrupt it enables. It never relies on firmware state, which covers the boot CPU only.
- Count per CPU, not only globally. When a counter contradicts the design, confirm it two independent ways (here `-d int` and the redistributor registers), then re-check every gate and hypothesis that assumed the old invariant. Also check whether earlier evidence came from a platform where the invariant held.

## 7. Miri's default seed misses memory-ordering bugs

**What happened.** The lock's threaded host test, with 25 iterations per thread under Miri, passed on CI's single default seed with three different ordering bugs planted: the snapshot's `fence(Acquire)` removed, the re-stamp made a `Relaxed` store, and the holder-field store made `Relaxed`. Only `-Zmiri-many-seeds` caught them. At 400 iterations, the default seed fails each of six ordering mutations.

**Why.** Miri explores one interleaving and one weak-memory outcome per seed, and a short test gives a race few chances to show.

**Rule.**
- Size a threaded Miri test so that the default seed fails each ordering mutation, and prove it by planting each one. The test's doc comment names the orderings it guards and says to re-run the mutations when its size changes.
- Run `-Zmiri-many-seeds` on the threaded test when its orderings change.
- Shrink exhaustive single-threaded model tests under `cfg(miri)`: they gain no checks from Miri, and one took 40 s there against 0.01 s on the host.
- `just check` does not lint test modules, and host `cargo clippy -p shared --tests` already fails on `main` in other modules. Run it and read the findings for your own files only.

## 8. A squash merge drops branch-only commits

**What happened.** The docs-check differential test materialises the old `check.py` from git history. This branch needed `check.py` as changed by a commit that exists only on the branch, and a squash merge leaves that commit out of `main`'s history, where CI reads the oracle. The fix was a committed patch file applied to the oracle (`tools/tests/fixtures/docs-check/check-py-irq-spin-lock.patch`), the technique in [the Rust port lesson](2026-09-24-jl-rust-port-parity-gotchas.md). The rate-settling soak the owner asked for has the same problem: one of its arms is a branch-only commit.

**Rule.** Anything a test, a CI job or a planned soak reads by commit must be on `main`'s history, or be carried as a file or a kept ref. Check this before the PR merges, because the merge deletes the branch. To run a PR's own `aios docs-check` rather than the main checkout's binary, see the same lesson (`AIOS_TOOLS_BIN`, or `cargo run -q -p aios-tools -- docs-check`).
