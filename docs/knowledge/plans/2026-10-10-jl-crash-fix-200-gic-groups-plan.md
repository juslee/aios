---
author: jl + claude
date: 2026-10-10
tags: [kernel, smp, sched]
status: in-progress
phase: crash-fix
milestone: step-200
---

# Plan: Crash fix step 200 — every CPU puts its timer PPI in Group 1 (#200)

## Approach

CPUs 1–3 never take a timer IRQ (#200). The owner made the fix its own crash-fix step on 2026-09-28: the first behaviour change after step 1a, sequenced and A/B-measured like the other steps, with N2 re-baselined after it ([crash-fix ADR](../decisions/2026-09-22-jl-crash-fix-preemption-and-fp.md), "Order and why" row #200 and its bullet; the 2026-10-09 amendment's #200 bullet). This plan covers the code change, the boot checks, the A/B acceptance, the re-baselines it triggers and the docs it makes stale.

The kernel change is small. On every CPU, including CPU 0, the kernel configures the timer PPI on its own redistributor instead of relying on firmware: it puts the PPI in Group 1, sets its priority, checks its trigger mode, enables it, and reads the result back. The measurement is the larger part, because the fix turns on timer preemption, timeouts and load balancing on three more CPUs. That exposes them to defects that today only CPU 0 can hit (H1, H3, N4, N5), so CLEAN can fall. The regression guard therefore needs an owner ruling before the pair runs (Q2).

Branch `claude/crash-fix-step-200-gic-groups`, worktree `.claude/worktrees/crash-200`, from `main` 353f619 (step 1b is e211d6d, #238; #239 is 353f619). Commits are named `Crash fix step 200: <description>`.

**Facts checked for this plan (2026-10-10, read-only, no boots).**

- `init_gicv3_secondary` (`kernel/src/arch/aarch64/gic.rs:114-156`) does the following: wakes the redistributor, sets `ICC_SRE_EL1.SRE`, writes `ICC_PMR_EL1` = 0xFF and `ICC_IGRPEN1_EL1` = 1, and writes `1 << 30` to `GICR_ISENABLER0` (`:152-154`). It never writes `GICR_IGROUPR0`. The only redistributor offsets that `gic.rs` defines are `GICR_WAKER` and `GICR_ISENABLER0` (`:24-26`).
- `init_gicv3` (CPU 0, `gic.rs:45-108`) does not write `GICR_IGROUPR0` either. CPU 0's PPI is enabled by `init_generic_timer` → `ic.enable_irq(dt.timer_ppi)` (`timer.rs:65`). `enable_irq` writes only core 0's frame for any INTID below 32 (`gic.rs:168-171`). So CPU 0's PPI 30 sits in Group 1 only because edk2 put it there.
- Both wake paths write `ProcessorSleep = 0` only when `ChildrenAsleep` is already set (`gic.rs:75`, `:123`). On a timeout the secondary path breaks out of the loop silently (`:133-135`).
- The secondary path finds its frame as `gicr_base + core_id * 0x20000` (`gic.rs:116`). `core_id` is the PSCI context id, which is the DTB CPU index (`smp.rs:151-153`).
- `secondary_main` initialises the GIC (`smp.rs:227-228`) before it installs the kmap TTBR1 (`:236`). It reaches `GICR_BASE`, a physical address, through the boot identity TTBR0. A UART print or a panic at that point would fault, because the UART is reached through TTBR1 (comment at `smp.rs:230-233`). So a GIC init failure on a secondary can only be recorded at that point and reported after the kmap install.
- `init_timer_secondary` arms CNTP (`smp.rs:251`) long before `enter_scheduler` unmasks IRQs (`scheduler.rs:63`). Once the group is fixed, each secondary takes the already-pending level PPI as soon as it unmasks. That happens before it has a current thread, so `schedule(Origin::Irq)` dispatches with no thread to save and counts `irqsw0` (`scheduler.rs:286-293`).
- `irq_handler_el1` matches the literal INTID 30 (`gic.rs:250-254`), while CPU 0 enables `dt.timer_ppi` (`dtb.rs:139-154`). On QEMU virt both are 30.
- The FIQ vectors are `b .` (`exceptions.rs:70`, `:83`), and only IRQ is ever unmasked (`DAIFClr, #0x2`). Enabling Group 0 would therefore route the timer to FIQ and halt the CPU. It is not a fix.
- The tick path (`timer.rs:199-232`): `timer_tick`, `check_timeouts`, `try_load_balance` (when CPU 0's `TICK_COUNT % 4 == 0`) and `NEED_RESCHED` run on every CPU that takes the IRQ. Only `TICK_COUNT`, the log drain, the heartbeat, input polling and the tripwire print are CPU-0-only.
- Test threads A–D have all-CPU affinity (`sched/init.rs:81`), and after 5 iterations they end in a `wfe` loop that never yields (`:169-188`). The balancer migrates only from the Normal queue (`sched/init.rs:259-284`).
- Every `[tripwire]` line in `target/soak` reads `tick=N,0,0,0` and `irqsw=N,0,0,0` (evidence reader: 2,322 lines, all pr238-*, fix217-* and pr239-* runs, B1 included). One example is `pr238-20261010-094339-b1-text/run-01.log:324`: `tick=45001,0,0,0 irqsw=3182,0,0,0`.
- Step 1a (`claude/crash-fix-step-1a-soak-classes`, in progress at 7509824) provides `aios soak --arm DIR --arm DIR` (report-only, rotation ABBA…, version checks, load refusal unless `--ignore-load`), the 9 classes, the tripwire columns, the pair report with regression guard, `removed` and `new` markers, and `--combine` (its plan, D4 and D5).
- `cargo run -q -p aios-tools -- docs-check` on this branch before the plan reported no new drift.

**Reported by the research readers, not re-checked here** (T0 checks the first two on the live machine):

- QEMU's GICv3 reset state with `secure=off` (`-machine virt,gic-version=3`, `justfile:43`): `GICD_CTLR.DS` = 1, `GICR_IGROUPR0` = 0, `GICR_IGRPMODR0` = 0, every `GICR_IPRIORITYR` byte = 0, PPIs level, SGIs edge, and `ICC_PMR_EL1` and `ICC_IGRPEN0/1_EL1` = 0. QEMU signals a Group 0 interrupt as FIQ (`hw/intc/arm_gicv3*.c`).
- edk2's `ArmGicV3Dxe` writes `GICR_IGROUPR0` = 0xffffffff when `DS` = 1, `ICC_BPR1_EL1` = 7 and `ICC_PMR_EL1` = 0xff, all on the calling CPU's redistributor (CPU 0) only.
- `GICR_IGRPMODR0` is RAZ/WI when `DS` = 1. When `DS` = 0, `GICR_IGROUPR0` is RAZ/WI to Non-secure accesses.
- `GICR_WAKER` may be Secure-only when `DS` = 0. This comes from memory of IHI 0069 and was not checked against the spec text. The unconditional wake sequence below is correct either way, because a RAZ register reads `ChildrenAsleep` = 0.

-----

## The change

### Registers, per CPU, on its own redistributor

Offsets are from IHI 0069 (Arm GICv3 and GICv4 Architecture Specification), chapter 12, register descriptions. RD_base is the CPU's redistributor frame, and SGI_base = RD_base + 0x10000. T1 records the revision and section numbers it was checked against in `shared/src/gic.rs` and in hal.md §4.1. The CPU-interface registers are from DDI 0487 (Arm ARM).

| # | Register (offset) | Action | Why |
| --- | --- | --- | --- |
| 1 | `GICR_WAKER` (RD_base + 0x014) | Read-modify-write: clear `ProcessorSleep` (bit 1) unconditionally, keeping the other bits (bits 0 and 31 are IMPLEMENTATION DEFINED). Then poll until `ChildrenAsleep` (bit 2) reads 0, with a bound | IHI 0069, redistributor power management: software clears `ProcessorSleep` and then waits for `ChildrenAsleep` = 0. Today's guard (`if ChildrenAsleep`) skips a wake that is in progress, where `ProcessorSleep` = 1 and `ChildrenAsleep` = 0 |
| 2 | `GICR_ICENABLER0` (SGI_base + 0x180), then `GICR_CTLR.RWP` (RD_base + 0x000, bit 3) | Write `1 << intid`, then poll RWP until it reads 0, with a bound | The PPI is reconfigured while it is disabled. RWP tracks the effect of `ICENABLER0` writes |
| 3 | `GICR_IGROUPR0` (SGI_base + 0x080) | Write 0xffffffff, so every SGI and PPI is in Group 1 (Q4), then read it back | Group 1 non-secure (with `IGRPMODR0` = 0) is the only group the kernel enables (`ICC_IGRPEN1_EL1`, `GICD_CTLR.EnableGrp1`). The value matches what edk2 leaves on CPU 0 |
| 4 | `GICR_IGRPMODR0` (SGI_base + 0xD00) | Read only: bit `intid` must be 0 | Modifier and group (0,1) is G1NS. It is RAZ/WI under `DS` = 1, and under `DS` = 0 only Secure software can write it |
| 5 | `GICR_IPRIORITYR<n>` (SGI_base + 0x400 + intid, byte access) | Write one priority byte, the same on every CPU, then read it back | QEMU resets the byte to 0 on CPUs 1–3, and on CPU 0 it holds whatever edk2 left. T0 reads CPU 0's value, and the step writes that value so CPU 0's state does not change. If edk2 left 0, the step uses 0xA0 (Linux's `GICD_INT_DEF_PRI`). Either way the value must be below `ICC_PMR_EL1` (0xFF) |
| 6 | `GICR_ICFGR1` (SGI_base + 0xC04) | Read only: bit `2 * (intid - 16) + 1` must be 0 (level) | The timer PPI is level-sensitive. Changing the trigger of an enabled interrupt is UNPREDICTABLE, and whether a PPI's trigger is programmable is IMPLEMENTATION DEFINED, so the step checks the bit and does not write it |
| 7 | `GICR_ISENABLER0` (SGI_base + 0x100) | Write `1 << intid`, then read it back | Enable |
| 8 | `ICC_SRE_EL1`, `ICC_PMR_EL1`, `ICC_IGRPEN1_EL1` | Unchanged: SRE = 1 and ISB, PMR = 0xFF, IGRPEN1 = 1 and ISB | Already correct on both paths. `ICC_CTLR_EL1.EOImode` stays at its reset value 0, because the handler writes only `ICC_EOIR1_EL1`. `ICC_BPR1_EL1` is not written, because nothing nests (handlers run with PSTATE.I set) |

The global prerequisite, `GICD_CTLR.EnableGrp1` (bit 1, named `EnableGrp1NS` in the `DS` = 0 view; `GICD_CTLR_ENABLE_GRP1_NS` at `gic.rs:21`), is already set by CPU 0 before `CPU_ON`, and the step does not change it. Group 0 (`ICC_IGRPEN0_EL1`, `GICD_CTLR.EnableGrp0`) stays disabled (see "Facts checked" on the FIQ vectors).

The INTID is `dt.timer_ppi`, never the literal 30. CPU 0 stores it in a static next to `GICR_BASE` before `CPU_ON`, the secondaries read it, and `irq_handler_el1` matches against it (Q4 B).

### Where it lives: `arch/aarch64/gic.rs`, not the Platform trait

- **gic.rs** holds the register-level code: a single per-CPU function, used by `init_gicv3` (CPU 0, through `enable_irq`'s PPI branch) and by `init_gicv3_secondary`, so the two paths cannot drift apart again. hal.md says the arch driver programs registers and that per-CPU differences are "handled internally" by `InterruptController` (hal.md §4.1, table row "Per-CPU config").
- **Not the Platform trait.** Neither the current trait (`init_uart`, `init_interrupts`, `init_timer`) nor hal.md's target trait has a per-CPU or secondary-init method, and `secondary_main` already calls `gic::init_gicv3_secondary` directly (`smp.rs:228`). A per-CPU trait method changes the HAL design, and only GICv2 support (`gic_v2.rs`, planned) needs it. That belongs to the BSP work, not to a crash-fix step that must stay one behaviour change for its A/B (rule: no design creep inside a measured step).
- **shared/** holds the pure logic (next section), following the ADR's "Rules for every step PR" and the M25 shared-crate ADR.

### Host-testable logic in `shared/src/gic.rs`

The step adds a new `no_std` module with no hardware access:

- the register offsets and bit positions from the table above, as constants;
- `PpiConfig::new(intid, priority) -> Result<PpiConfig, GicError>`, which rejects INTIDs outside 16–31 and a priority at or above the PMR, and gives the enable mask, the priority byte offset, and the `ICFGR1` and `IGRPMODR0` bit positions;
- `PpiReadback { igroupr0, igrpmodr0, isenabler0, priority, icfgr1 }` and `PpiConfig::check(&PpiReadback) -> PpiStatus`: one flag per property (group 1, modifier 0, enabled, priority as written, level), plus `wake_ok` and `rwp_ok` from the bounded polls. `PpiStatus::ok()` is true only when every flag is;
- `waker_wake_value(waker) -> u32` (the read-modify-write value) and `waker_awake(waker) -> bool`;
- with Q4 C only: `find_redistributor(frames, mpidr_aff)`, which walks `GICR_TYPER` values (affinity in bits [63:32], `VLPIS` in bit 1 giving a stride of 0x20000 or 0x40000, `Last` in bit 4).

The tests cover:

- bit positions for INTID 16, 30 and 31; INTIDs 15 and 32 rejected; a priority of 0xFF rejected;
- `check` on the edk2 CPU 0 state (0xffffffff, enabled, level), which is `ok`;
- `check` on QEMU's reset state for CPUs 1–3 (`IGROUPR0` = 0), which fails the group flag;
- `check` on `DS` = 0 RAZ/WI (`IGROUPR0` reads 0 after the write), which fails the group flag;
- `check` with `IGRPMODR0` bit 30 set (G1S), which fails the modifier flag;
- `check` with an edge-triggered `ICFGR1`, which fails the level flag;
- `waker_wake_value` keeping bits 0 and 31;
- `waker_awake` for the four `ProcessorSleep`/`ChildrenAsleep` combinations.

Miri runs over these in CI (`ci.yml`).

### Reporting

- `init_gicv3_secondary` returns a `PpiStatus` and never prints. `secondary_main` passes it to `print_cpu_line` after the kmap TTBR1 install, inside its `PRINT_TURN` window. CPU 0 keeps its status in `InterruptController`, and `kernel_main` prints it on CPU 0's `[smp]` line.
- The `[smp]` line gains four fields: `ppi=<intid> ppi_grp1=0|1 ppi_en=0|1 ppi_ok=0|1`. For example: `[smp] cpu=1 vbar=0x0000000040081000 ttbr0=0x00000000402ad000 vbar_kva=0 ttbr0_idmap=1 ppi=30 ppi_grp1=1 ppi_en=1 ppi_ok=1`. Step 1a's harness does not parse `[smp]` lines (its non-goals), so the classifier and its goldens are unaffected. CI soak logs then show CI's GIC state, which no one has checked so far.
- When `ppi_ok=0`, a `kerror!(Smp, "cpu {} timer PPI {} setup failed: …")` names the failed flags. Whether the CPU then halts is Q5; the recommendation is to report and continue.
- No tripwire schema change. The step changes behaviour, not measurement, so the two arms' tripwire lines keep the same keys and step 1a's parser reads both.

-----

## Boot checks (development, before the A/B)

A failure in the first three is a bug in the step. The rest describe the kernel and are recorded in the PR. Logs come from `just run` (text mode), up to 3 boots. `main`'s text CLEAN rate is 55% (B1), so a single non-CLEAN boot is not a failure by itself; the A/B decides that.

1. **Per-CPU ticks.** In every `[tripwire] v=1 src=hb` line after the first, all four `tick` values are greater than 0, and each one grows between consecutive heartbeat lines. Today the line reads `tick=N,0,0,0`.
2. **`[smp]` lines.** Four lines, each with `ppi=30 ppi_grp1=1 ppi_en=1 ppi_ok=1`, and no `timer PPI … setup failed` line. CPUs 1–3 still read `vbar_kva=0 ttbr0_idmap=1`, and CPU 0 reads `vbar_kva=1 ttbr0_idmap=0`: N5 stays as it is (Q3).
3. **Register state, QEMU monitor** (`-monitor` on a socket, `xp /1wx`), on a boot of the branch, compared with T0's table for `main`:
   - CPUs 1–3 now match CPU 0 in `GICR_IGROUPR0`, bit 30 of `ISENABLER0`, the priority byte, `ICFGR1` and `IGRPMODR0`;
   - CPU 0's values are byte-identical to `main`'s. CPU 0 is not meant to change.

   The addresses come from `GICR_BASE` 0x080A_0000: CPU k's SGI_base is 0x080B_0000 + k × 0x20000. On CPU 0: `IGROUPR0` 0x080B_0080, `ISENABLER0` 0x080B_0100, `IPRIORITYR7` 0x080B_041C, `ICFGR1` 0x080B_0C04, `IGRPMODR0` 0x080B_0D00.
4. **`irqsw`.** In a boot that completes Gate 1, the sum of `irqsw` over CPUs 1–3 is above 0, most likely on CPUs 1 and 2, where the `wfe`-looping B and C now rotate with the migrated Normal threads. `irqsw0` reads about 1 on each of CPUs 1–3, from the PPI already pending when `enter_scheduler` unmasks.
5. **No new hazard lines in a CLEAN boot:**
   - no `PANIC`, no `EXCEPTION[CPU n]` and no `lock re-entry:`;
   - no `[tripwire-ev] kind=self`, and no `kind=stuck` line naming a CPU from 1 to 3;
   - `pcphys`, `elrmm[1..3]` and `n4` are recorded and not judged. They are expected to become non-zero (see the prediction table).
6. **Gates (rule 02):**
   - `just check` and `just test`, with zero warnings;
   - `cargo build --target aarch64-unknown-none`;
   - `llvm-objdump -h` on the kernel ELF, run from the toolchain sysroot the way step 1b ran it, with sections at their usual addresses;
   - `just run`: EL = 1, core 0, `4 CPUs online`.

-----

## A/B acceptance

**Prerequisites:**

- step 1a has merged, because the pair needs its interleave mode, classes and tripwire columns;
- this branch has merged that `main`;
- step 1b's own A/B soak has run, or the owner orders otherwise (Q7).

**Arms:**

- A (previous) is `main` at this branch's merge base, checked out in its own worktree, for example `.claude/worktrees/crash-200-base`;
- B (new) is this branch's tip.

Both arms pin one toolchain and boot one QEMU binary and one firmware image; step 1a's preflight refuses anything else. Both arms carry 1b's detect-only lock, scan A (#236) and the same tripwire schema, so none of these confounds the pair.

**Commands** (attended and host-exclusive, about 75–85 minutes in all, with the owner's go-ahead):

```sh
just soak --arm /Users/juslee/dev/aios/.claude/worktrees/crash-200-base \
          --arm /Users/juslee/dev/aios/.claude/worktrees/crash-200 runs=20 secs=75
just soak --arm /Users/juslee/dev/aios/.claude/worktrees/crash-200-base \
          --arm /Users/juslee/dev/aios/.claude/worktrees/crash-200 runs=10 secs=75 mode=gpu
```

Both commands are report-only by step 1a's D5. The load rule is enforced at the start (a refusal above the CPU count), and the report gives each arm's mean and maximum load. If the arm means differ by more than 25%, the pair is redone. The agent stops only its own QEMU PID.

**What gates (proposed; Q1 and Q2 decide):**

1. **The fix itself, mechanically.** In arm B, every boot with a complete tripwire line has `tick` > 0 on all four CPUs. In arm A, every such boot has `tick` = 0 on CPUs 1–3. This is read from step 1a's per-CPU `tick` columns.
2. **The regression guard.** It fails if CLEAN is lower in B with one-sided Fisher p < 0.05, per mode, as step 1a's pair report computes it. If it fails, Q2's ruling applies.
3. **"Class removed".** No class is a target of this step, so a `removed` marker is reported and not claimed.
4. **The `new` marker.** A class with 0 boots in A and p < 0.05 for more in B is reported, together with the first fatal line of every boot in that class.

**Expected movement.** These are predictions, recorded so the PR can compare them with the results; none of them gates. "Today" is B1 (c065b0c) and the tripwire logs since.

| Key / class | Today | Expected in B | Why |
| --- | --- | --- | --- |
| `tick[1..3]` | 0 | close to `tick[0]` (about 45,000 in a 75 s CLEAN text boot) | The fix |
| `irqsw[1..3]` | 0 | > 0, mostly CPUs 1–2; CPU 3 may stay low | B and C never yield, so only the tick moves them off. On CPU 3 the Interactive PI-Caller keeps being repicked |
| `irqsw0[1..3]` | 0 | about 1 each | The PPI is already pending at `enter_scheduler`'s unmask |
| `pcphys` | 0 in all 82 Full lines | > 0, roughly the number of restores of threads switched out on CPUs 1–3's IRQ path | VBAR is still physical on CPUs 1–3 (N5, step 5). It stops being a harm signal, and only a restore on CPU 0 is fatal |
| `elrmm`, `spsrmm` [1..3] | 0 | > 0 | H1 on CPUs 1–3: two threads that both left through the IRQ stub resume with each other's ELR. Today every PCZERO boot has `elrmm` = 2 on CPU 0 |
| `n4`, `nest`, `insched`, `n1` [1..3] | 0 | can be > 0 | These paths become reachable on CPUs 1–3 |
| `lb` | 5 per CLEAN boot | higher | `try_load_balance` runs on every CPU whose tick lands in a `TICK_COUNT % 4 == 0` window |
| `starved[Normal]` | 9 at the end of every CLEAN text boot (99 over 11) | lower, not 0 | Threads migrated into CPUs 1–2 now get turns. CPU 3's Normal threads stay starved behind the Interactive PI-Caller's yield loop (`ipc/tests/mod.rs:769-771`), and CPU 0's behind the bench loops (N10, independent of #200) |
| `starved[Idle]` | 4 | 4 | Every CPU always has a non-idle Runnable thread |
| `ubmove[to]`, `ctbusy`, `xdir`, `xrep` | 2, 2/2, … | higher | `check_timeouts` runs on four CPUs (N7's premise restored) |
| `lkph`, `lkpho`, `lkphrun`, `kind=ph` | 0 in all 30 B1 boots | can be > 0 | Lock-holder preemption on CPUs 1–3: `IrqSpinLock` does not mask IRQs |
| `scanstall` | 0 | can be > 0 | All four CPUs now gate the two-strike confirmation. A CPU that halts or spins with IRQs masked freezes its `tick`, which withholds confirmations, so a WEDGE can lose its `nowaker` attribution |
| `irqsw[0]` per tick | — | may fall | `NEED_RESCHED` is one global flag that any CPU's `schedule()` clears (H4) |
| PCZERO, EXCEPTION | text 1 + 2 of 20 | may rise, with first fatal reports on CPUs 1–3 | H1 on CPUs 1–3 |
| EXCEPTION on CPU 0 with EC = 0x21 and ELR = FAR in [0x4008_0000, physical text end) | never seen | possible, new signature | N5: a thread preempted on CPU k is migrated to CPU 0 and resumed at a physical-alias PC under CPU 0's user TTBR0. B and C (Normal, all-CPU affinity) are the likely carriers |
| PANIC-LOCK with `on CPU 1\|2\|3 ctx=irq\|irq-exit` | impossible today | possible | H3 on CPUs 1–3: `schedule` and the balancer's `THREAD_TABLE.lock()` run on their IRQ path |
| WEDGE-STUCK, WEDGE-ALIVE | text 3 of 20 (all N2) | either way | N2's window is open to preemption on CPUs 1–3 (up). Some stalls may now end as PANIC-LOCK (down). Candidate (d) of WEDGE-ALIVE becomes reachable |
| gpu classes | 1 CLEAN, 9 EXCEPTION | about the same | Censored by CPU 0's `compositor_loop` EXCEPTION at the first IRQ-path switch (9 of 10) |
| `Timeout test: unexpected result -6` | every boot | unchanged | EPERM from a capability check, not a timer effect |
| `Server: started` (#240) | 0 in every boot | most likely 0 | The echo pair is Normal and queued on CPU 0, behind the Interactive spinner. CPU 0 already ticks, so #200 does not explain it. The PR reports the count per arm (Q10) |

**Report.** The PR carries the following:

- step 1a's pair report for each mode;
- the per-arm tripwire table;
- the table above with its observed column filled in;
- for every boot in arm B that is not CLEAN: its class, the CPU of its first fatal report, and which hypothesis's signature it carries (H1, H3, N2, N4 or N5), or "unattributed".

The results go into a research note, `docs/knowledge/research/2026-10-1x-jl-crash-fix-200-ab.md`, in B1's format.

-----

## Re-baselined after merge

- **N2 (B2).** The ADR re-baselines N2 after this step and before the capability-lifetime PR converts the blocking paths. That re-baseline reads `n2` (rpre, rblk, vpre, vblk), `ubrun` and `ubrbl[reply]`, `latereply`, `ctbusy` and `nowaker`. Whether arm B of this pair counts as that baseline is Q6.
- **CI.** CI's 26.04 baseline is 3 soak runs on `main`, re-run after a baseline-shifting PR (crash-fix ADR, the 26.04 baseline bullet at `:535`; the capability-lifetime ADR states the re-run for its own PR). This step shifts every baseline (Q8).
- **Step 5's N5 baseline.** Step 5's acceptance, "the PC-outside-kernel-VA counter goes from its baseline value to 0", reads `pcphys` from a post-#200 kernel. Arm B gives the first value.
- **Step 6b.** Its line "cross-CPU unblock from `check_timeouts` reads 0 for Interactive threads" only means something from this step onward.
- **The capability-lifetime branch** (`claude/cap-lifetime`) is rebased after this step merges, as that ADR's rebase schedule requires. Both arms of its pair include this step.
- **No earlier figure is compared with a later arm** (no instrument freeze). B1 stays a historical record.

-----

## Docs this makes stale

| Doc | What changes | Task |
| --- | --- | --- |
| Crash-fix ADR, 2026-10-09 amendment, #200 bullet and its sub-bullets (H1 CPU-0-only, N5 cannot fire, N7's premise, H3 and candidate (d)) | A dated note: fixed by step 200 (#PR); the sub-bullets describe the kernel before it. The erratum stays as history. The "Docs to update" table gains a row for this step. The owner's rulings on Q1–Q3 go into a "Step 200" entry under "Steps" | T6 |
| Capability-lifetime ADR: "today CPUs 1–3 take none" and "on CPU 0 today and on every CPU from #200's fix step" (§7 exit safe point and off-CPU wait, the interim rule, "Risks", the #200 note) | A dated note that these now hold on every CPU; the text stays in place | T6 |
| `.claude/CLAUDE.md` Key Technical Facts, "Timer IRQs on CPUs 1-3" (`:54-56`) | Rewritten: every CPU configures its own timer PPI (Group 1, priority, level check, enable, readback) and never relies on firmware. The "IRQ-path address values (1b)" fact (`:83-89`) stays true, and its physical-alias PCs are now live on CPUs 1–3 until step 5. `.claude/` is protected: the lead makes this edit through the permission prompt, not an unattended agent | T6 (lead) |
| `docs/kernel/observability.md` §6.5: "Counters" paragraph (`:879`), "Two strikes" bullet (`:902`), "SMP lines" (`:928-934`) | Remove "Today CPUs 1–3 take no timer IRQs". The tick-0 gate exemption stays as the rule for a CPU that never ticked, without citing #200 as the current state. Document the four new `[smp]` fields | T3, T6 |
| `shared/src/tripwire.rs`: `TwoStrike` doc (`:1655-1661`) and the comment in `two_strike_gate_skips_cpus_that_never_ticked` (`:3297`) | Reword as the general never-ticked-CPU case. The test stays | T4 |
| `kernel/src/sync/selftest.rs` (`:5-6`, `:61-62`) | The reason for the CPU 0 pin becomes "the hold must be re-entered by the same CPU's tick" rather than "the only CPU that takes timer IRQs" | T4 |
| `kernel/src/arch/aarch64/gic.rs` module doc ("for the boot CPU") | Describe the per-CPU PPI setup | T2 |
| `docs/kernel/hal.md` §4.1 (`InterruptController`, "Per-CPU config" row) | The per-CPU PPI sequence and the rule "never rely on firmware for group or priority" | T6 |
| `docs/kernel/boot/kernel.md` secondary sequence, step 6 (`:235`) | "Init GIC redistributor + CPU interface" gains "put the timer PPI in Group 1" | T6 |
| `docs/phases/01-boot-and-first-pixels.md:170` ("enable Group 1 SGIs", ticked, never done) and `docs/phases/03-ipc-and-capability-system.md:144` (secondary timer IRQs, ticked, false until now) | A dated note on each, naming `GICR_IGROUPR0` and this step | T6 |
| `docs/knowledge/research/2026-10-10-jl-crash-fix-1b-n2-baseline.md` item 5 (`:52`) | A dated correction: only part of `starved[Normal]` is #200. CPU 3's Normal threads starve behind the Interactive PI-Caller, and CPU 0's behind the bench (N10). It also points to the B2 note | T6 |
| `docs/knowledge/lessons/2026-10-10-jl-crash-fix-1b-lessons.md` §6 | No change (history). Its rule is what this step implements | — |
| #240 | A comment with the per-arm `Server: started` counts and the conclusion (Q10) | T8 |

-----

## Tasks

Each task is one commit, `Crash fix step 200: <description>`, with the rule 02 gates (`just check`, `just test`) passing before it. Tasks that change the kernel also boot it (`just run`).

- [ ] **T0. Live GIC state on `main` (no commit; results go into this plan).**
  - On a QEMU monitor boot of 353f619, record for each CPU: `GICR_WAKER`, `IGROUPR0`, `IGRPMODR0`, `ISENABLER0`, `IPRIORITYR7`, `ICFGR1` and `GICR_CTLR`, plus `GICD_CTLR` (`DS`, `EnableGrp0`, `EnableGrp1`, `ARE`).
  - Choose the priority from CPU 0's byte at offset 0x41E.
  - Note the QEMU version and the firmware path and sha256. Note CI's QEMU and edk2 versions from the latest `ci.yml` soak job log.
  - Check: the table is in "Issues Encountered" below.
- [ ] **T1. Pure logic in `shared/src/gic.rs`, with host tests** (the section above; Q4 decides whether `find_redistributor` is included).
  - Check: `just test`, and Miri on the new tests (`cargo +nightly miri test -p shared gic`).
- [ ] **T2. One per-CPU PPI setup in `gic.rs`, used by CPU 0 and CPUs 1–3:**
  - the unconditional WAKER sequence on both paths (CPU 0 still panics on timeout, and a secondary records the timeout in its status);
  - disable, RWP, group, priority, level check, enable and readback, all through `shared::gic`;
  - `enable_irq`'s PPI branch calls the setup and is documented as boot-CPU-only;
  - the timer INTID is stored before `CPU_ON` and used by `init_gicv3_secondary` and `irq_handler_el1`;
  - a `// SAFETY:` comment in rule 06's form on every new unsafe block.
  - Check: `just check`, `just test`, and `just run` passing boot checks 1, 2 and 4. Boot check 2 shows the new fields only from T3 on; until then, ticks alone.
- [ ] **T3. Report:**
  - `PpiStatus` reaches `print_cpu_line` on every CPU; the four `[smp]` fields; the `kerror!` on failure;
  - observability.md "SMP lines";
  - a fault-injection check: a temporary local build that skips the `IGROUPR0` write on CPU 2 prints `ppi_grp1=0 ppi_ok=0` and the error line on CPU 2 only, and `tick[2]` stays 0. The build is not committed, and the result is recorded in this plan.
  - Check: boot check 2.
- [ ] **T4. Code comments made stale:** `shared/src/tripwire.rs` (`TwoStrike` doc and test comment) and `kernel/src/sync/selftest.rs`.
  - Check: `just check`, `just test`.
- [ ] **T5. Development boots and the register check:**
  - boot checks 1–6 on up to 3 `just run` boots, plus boot check 3 against T0's table;
  - `llvm-objdump -h`.
  - Results into this plan.
- [ ] **T6. Docs:**
  - the stale-docs table above, except the T3/T4 rows and #240;
  - the ADR's dated notes, with no rulings invented: Q1–Q3 are recorded only once the owner has answered;
  - `.claude/CLAUDE.md`, by the lead, through the permission prompt;
  - `cargo run -q -p aios-tools -- docs-check`, with no new drift except this plan's working-plan finding.
- [ ] **T7. `/audit-loop`** (doc, code and security/bug review) until a clean round. The review covers in particular: virtual against physical addresses (GICR is reached physically through TTBR0 before the kmap install), the bounded polls, readback on `DS` = 0, the PPI disable window, and that nothing new runs on the IRQ path except the INTID compare.
- [ ] **T8. Merge `main` after step 1a lands, then run the A/B pair:**
  - re-run the gates;
  - with the owner's go-ahead, run the pair as above;
  - write the research note, comment on #240 and fill in the expected-movement table.
  - If Q2's ruling fails the step, stop and report to the owner.
- [ ] **T9. Distil and hand off:**
  - lessons and decisions into `docs/knowledge/`, then delete this plan;
  - push the branch (`git push -u origin claude/crash-fix-step-200-gic-groups`), open the PR and run `/review-pr-comments`;
  - hand off to the owner for `/merge-and-cleanup`. After the merge come the re-baselines above (B2 per Q6, CI per Q8).

**Keeping the branch current.** The step cannot merge before step 1a, because its A/B needs 1a's harness. T1–T7 go ahead now on 353f619. Each time `main` moves (step 1a's merge, other PRs), the branch merges `main`, as step 1a's branch did at 810c28f, and re-runs the gates. Rebasing would rewrite commits that may already be pushed. The pair (T8) runs only after the last such merge before the PR: arm A must be the branch's merge base. If `main` later moves again, the pair is re-run only when the kernel ELF of either arm changes. Step 1a's report records each arm's ELF sha256; a docs-only or tools-only change leaves both ELFs, and so the pair, valid.

-----

## Dependencies & Risks

- **Depends on:**
  - step 1a merged, for the pair;
  - step 1b merged (done, e211d6d);
  - B1 taken (done, 2026-10-10);
  - the owner's answers to Q1–Q3 before the pair is run, and to Q4 and Q5 before T1 and T2.
- **Risk: the regression guard fails because of exposure, not the fix.** The guard catches a drop from 60% to 30% CLEAN about two times in three at 30 boots per arm, and 9 of B1's 20 text boots already end non-CLEAN with only CPU 0 ticking (H1 3, N2 3, H3 3). Mitigation: Q2's ruling in advance, and per-boot attribution in the report.
- **Risk: N5 goes live** (Q3). A thread preempted on CPU k and migrated to CPU 0 faults there. Mitigation: the signature (EC 0x21, physical ELR = FAR, CPU 0, preceded by a `pcphys` increment) is distinct and attributable, and step 5 removes it.
- **Risk: CPU 0's GIC state changes by accident.** Rewriting the priority or `IGROUPR0` on CPU 0 with other values than edk2's would add a second difference to the pair. Mitigation: T0 and boot check 3 require CPU 0's registers to be byte-identical.
- **Risk: `DS` = 0 (CI firmware, `secure=on`, real hardware).** Non-secure `IGROUPR0` writes are ignored there. Mitigation: the readback reports `ppi_grp1=0`, visible in CI logs. The step does not try Secure-only registers.
- **Risk: the first IRQ on each secondary arrives inside `enter_scheduler`'s unmask window.** The PPI is pending as soon as IRQs are unmasked, so the first IRQ dispatches with no current thread (`irqsw0`). CPU 0 takes this path too, but on secondaries it becomes deterministic. Mitigation: counted (`irqsw0`). Moving `init_timer_secondary` later is not part of this step.
- **Risk: the IRQ handler's INTID compare reads a static.** That adds one relaxed load to the IRQ path. It allocates nothing and uses no V registers. T7 reviews it.
- **Risk: scope creep into steps 2, 5, 6a and 6b** (the balancer from four CPUs, the global `NEED_RESCHED`, VBAR). Mitigation: Q3 and Q9 keep them out, so the pair measures one change.
- **Not in scope, noted:** the platform reader reports that Pi 5 (BCM2712) has a GIC-400 (GICv2), while bsp.md, drivers.md, platforms.md, model.md, firmware.md, testing.md and hal.md call it GICv3. This plan has not verified that. If it holds, `gic.rs` serves QEMU only, and both Pi boards need `gic_v2.rs`. The lead files a separate issue; this step does not touch those docs.

-----

## Open questions for the owner

**Q1. What does this step's acceptance gate on, beyond the regression guard?** The ADR says only "A/B-measured like the other steps".
- A. The regression guard, plus the mechanical fix check: every arm-B boot with a complete tripwire line has `tick` > 0 on all four CPUs. Everything else (`irqsw`, `starved`, `pcphys`, classes) is reported, not gated.
- B. A, plus `irqsw[1..3]` > 0 and a fall in `starved[Normal]` as gates.
- C. The regression guard only.
- Recommendation: A. B gates on predictions that depend on test-thread placement (CPU 3 may legitimately stay low). C does not check that the fix took effect.

**Q2. What happens if the regression guard fails (CLEAN lower in arm B, p < 0.05)?** That is plausible, because the fix makes H1, H3, N4 and N5 reachable on three more CPUs. The ADR has no rule for a fix that exposes known defects.
- A. Attributed exposure passes. The step merges if every extra non-CLEAN boot in arm B carries a known hypothesis's signature (H1, H3, N2, N4 or N5) and no unattributed signature appears. The ADR records the ruling and the lower baseline. Later steps (2, 3, 5, 6a) are measured against it.
- B. The guard holds as written. The step waits until steps 2, 3 and 5 have removed the exposed defects, which reverses the owner's 2026-09-28 order.
- C. Merge behind a default-off feature.
- Recommendation: A. The ADR already expects this step to shift every baseline. B undoes the decision to measure every later step with all CPUs ticking. C leaves a second kernel configuration, which the no-legacy rule rules out.

**Q3. Does N5 stay live until step 5?** CPUs 1–3 keep their physical VBAR, so a preempted thread migrated to CPU 0 can fault there.
- A. Leave it to step 5 (F7), as the ADR orders. `pcphys` and any N5 EXCEPTION become step 5's baseline.
- B. Pull F7's first bullet (`secondary_main` calls `install_vector_table`) into this step.
- Recommendation: A. It keeps the pair to one change and gives step 5's acceptance a non-zero baseline. The cost is that N5 EXCEPTIONs may count against Q2.

**Q4. How wide is the GIC change?**
- A. Minimal: OR bit 30 into `GICR_IGROUPR0` on CPUs 1–3 only.
- B. One per-CPU PPI setup for CPU 0 and CPUs 1–3:
  - unconditional WAKER, disable and RWP, `IGROUPR0` = 0xffffffff, the priority written, the level and modifier checked, enable, readback;
  - the INTID from the DTB in the setup and in the handler;
  - the pure logic in `shared/`.
- C. B plus a `GICR_TYPER` affinity walk instead of `core_id × 0x20000`, and per-CPU init moved behind `InterruptController`.
- Recommendation: B. It follows the lesson's rule ("never relies on firmware state") on every CPU, and on QEMU it leaves CPU 0's state unchanged. C's walk and HAL change make no difference on QEMU and belong to the GICv2 BSP work.

**Q5. What does a CPU do when its readback fails (`ppi_ok=0`)?**
- A. Report and continue: the `[smp]` fields plus a `kerror!`. The CPU runs cooperatively, as today, and its `tick` stays 0.
- B. Halt that CPU, in a `wfe` loop, after printing.
- C. Panic.
- Recommendation: A. The failure is visible in every log (CI included), and the kernel behaves exactly as it does today. B and C turn a firmware difference (`DS` = 0) into a boot failure.

**Q6. Is arm B of this pair the post-#200 N2 baseline (B2)?**
- A. Yes, if the merged `main` kernel ELF is identical to arm B's (step 1a records the sha256). Arm B's 20 text and 10 gpu boots have B1's shape.
- B. A separate single-arm soak of post-merge `main`, in B1's form (`just soak runs=20 secs=75 report_only=1`, then gpu with `runs=10`), about 40 minutes.
- Recommendation: A, with B as the fallback when the ELF differs. Interleaved boots are the same instrument, and the cost is saved.

**Q7. In which order do step 1b's pending A/B soak (af59149 against e211d6d) and this step's pair run?** Both wait for step 1a, and both are host-exclusive.
- A. 1b's pair first, then this one.
- B. This step's pair first.
- C. One three-arm interleave (af59149, e211d6d, this branch), about 115–125 minutes, reading the A–B and B–C pairs. Step 1a prints a Bonferroni note.
- Recommendation: A. 1b's H1 and H3 verdicts are about pre-#200 kernels and decide the ADR revisions before step 2. Running them first keeps each pair's question separate. C saves about 35 minutes, but the ADR states every soak as a pair and adds a multiplicity question.

**Q8. Are CI's 3 baseline soak runs on `main` re-run after this step merges?**
- A. Yes, 3 `workflow_dispatch` runs on post-merge `main`.
- B. No.
- Recommendation: A. This step shifts every baseline, and CI's arms are their own instrument.

**Q9. Does this step leave the other timer-path behaviour on CPUs 1–3 as it is?** That covers `try_load_balance` from every CPU (with a blocking `THREAD_TABLE.lock()` and `kinfo!` in IRQ context) and the global `NEED_RESCHED` that any CPU's `schedule()` clears (H4).
- A. Leave both. Steps 2 and 6a own them, and the pair measures the GIC change alone.
- B. Restrict the balancer to CPU 0's tick in this step.
- Recommendation: A. B changes behaviour a second way and hides the H3 and H5 exposure that steps 2 and 4 must measure.

**Q10. How does #240 (the echo IPC pair never runs) relate to this step?**
- A. This PR reports the per-arm `Server: started` counts and comments on #240. Fixing #240 is not part of this step.
- B. Fix #240 here.
- Recommendation: A. The echo pair is queued on CPU 0, which already ticks, so #200 is unlikely to be the cause, and a fix would be a second change in the pair.

**Q11. The step's name.** The ADR's rule is `claude/crash-fix-step-<n>-<name>` with commits `Crash fix step <n>: …`, but #200's step has no number.
- A. Keep `step-200` (branch `claude/crash-fix-step-200-gic-groups`, commits `Crash fix step 200: …`), named after the issue.
- B. Renumber the series.
- Recommendation: A. The ADR already refers to it as "#200's fix step", and renumbering would invalidate every later step's references.

-----

## Issues Encountered

(to be filled during implementation; T0's register table goes here)

## Decisions Made

(to be filled during implementation)

## Lessons Learned

(to be filled during implementation)
