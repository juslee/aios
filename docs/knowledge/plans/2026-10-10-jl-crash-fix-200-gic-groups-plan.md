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

The kernel change is small. On every CPU, including CPU 0, the kernel configures the timer PPI on its own redistributor instead of relying on firmware: it puts the PPI in Group 1, sets its priority, checks its trigger mode, enables it, and reads the result back. The measurement is the larger part, because the fix turns on timer preemption, timeouts and load balancing on three more CPUs. That exposes them to defects that today only CPU 0 can hit (H1, H3, N4, N5), so CLEAN can fall. It also lets self-tests run that no logged boot has run: #240's echo IPC pair, cap-denied, test thread A and the service echo pair with its `process_exit`. The balancer moves them onto CPUs 1–2 in every boot, and today they starve there only because those CPUs never tick ("Facts checked"). The step fixes #240's immediate cause as a side effect of the one change, and the plan has to predict that, not deny it (Q10). The regression guard therefore needs owner rulings before the pair runs: what a failure means (Q2), how the guard is pre-registered, given that each mode is its own, smaller test (Q12), and how a failure in a newly live self-test is attributed (Q14).

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
- **Which threads starve on CPUs 1–2 today.** `allocate_thread` takes the first free slot (`sched/mod.rs:237-247`), so in creation order the slots are: 0–3 idle and 4–7 test threads A–D (`sched/init.rs:33-95`); then, from `ipc/tests/mod.rs`, ipc-server 8 (queued on CPU 0, `:172`), ipc-caller 9 (CPU 1, `:193`), ipc-timeout 10 (CPU 2, `:214`), pi-server 11 and pi-caller 12 (CPU 3) and cap-denied 13 (CPU 0, `:297`); then `service::init`'s echo-server 14 (CPU 0, `service/mod.rs:238`) and echo-client 15 (CPU 1, `:259`); then the Interactive bench threads on CPU 0. Every Normal one has all-CPU affinity. In all 32 boots of B1 (`target/soak/pr238-20261010-094339-b1-{text,gpu}`, 20 + 10) and `pr239-20261010-115625-text` (2), the balancer migrates tid 4 (A) and tid 8 from CPU 0 to CPU 1, and tid 13 from CPU 0 to CPU 2. tid 8's first migration comes after B has printed `Thread B on core 1 iter=4` and entered its `wfe` loop as CPU 1's current thread in 32 of 32 boots (run-01: 5.797 s, then 5.811 s); tid 13's comes after C's `iter=4` on CPU 2 in the 28 boots that print it. None of these threads prints in any of the 32 boots: no `Thread A`, no `Server: started` (counted with `(^|[^-])Server: started, channel`, as #240 does, so `PI-Server` does not match), no `Caller:` line, no `unauthorized ChannelCreate`, no `Echo server: started`. echo-client prints `started` and `found service` in all 32 and nothing after, not even the 5 s timeout error, although it goes on to `process_exit(pid 7)` whichever way its call ends (`service/mod.rs:410-455`). With a tick on CPUs 1–2, `timer_tick` runs out B's and C's slices and `schedule()` re-queues them (`scheduler.rs:198-205`), so the threads queued behind them run.
- Every `[tripwire]` line in `target/soak` reads `tick=N,0,0,0` and `irqsw=N,0,0,0` (evidence reader: 2,322 lines, all pr238-*, fix217-* and pr239-* runs, B1 included). One example is `pr238-20261010-094339-b1-text/run-01.log:324`: `tick=45001,0,0,0 irqsw=3182,0,0,0`.
- Step 1a merged as #246 (f3b039b) and provides `aios soak --arm DIR --arm DIR` (2–4 arms; always report-only, so `report_only=1` with `--arm` is a usage error; arm order rotated each round, A B, B A, …; version checks: every arm contains 7167d40, one firmware file, one `rust-toolchain.toml` channel and, after the builds, one `rustc --version` unless `--allow-mixed-toolchains`; QEMU and firmware sha256 re-checked around every boot; load refusal above the CPU count unless `--ignore-load`), the 9 classes, the tripwire columns, the pair report (one-sided Fisher on conclusive boots, one test per run with no pooling across runs; regression guard, per-class rows, `removed` and `new` markers, a Bonferroni note with more than one pair, the 25% load-mean "redo the pair" rule) and `--combine`, plus `--fail-on-regression`. `arms.tsv` records each arm's kernel ELF sha256 (first 16 hex digits). Its working plan was deleted at merge: "its D4" below means one Fisher test per run and "its D5" means an `--arm` soak is always report-only; both are now in `aios soak --help`, [developer guide §5.6](../../project/developer-guide.md) and the crash-fix ADR's "Step 1a delivered" and "Step 1a design decisions" notes (2026-10-10). The parser accepts T8's form `just soak --arm DIR --arm DIR runs=20 secs=75` (and `mode=gpu`). Both T8 arm paths sit under `.claude/worktrees/` inside the main checkout, so the harness warns that the parent `.cargo/config.toml` joins their builds (identical to the worktree's own on 2026-10-11); 1a's lessons note §8 recommends arm worktrees outside any checkout (`git worktree add --detach ../aios-<sha> <sha>`) for arm A.
- `cargo run -q -p aios-tools -- docs-check` on this branch before the plan reported no new drift.
- Every QEMU command line in the repo is `-machine virt,gic-version=3`, with no `secure=on`: `justfile:43`, `:58`, `:73`, `:90`, `:107` and `:124`, and the soak runner (`tools/src/cmd/soak/runner.rs:154`, plus the container command lines at `:763` and `:769` that CI and the soak matrix boot). Local, CI and soak boots therefore run the same GIC model, and `GICD_CTLR.DS` = 1 on all of them (next list). `DS` = 0 arises only on real GICv3 hardware whose Secure firmware (TF-A) runs below the kernel, and this step boots no such machine.

**Reported by the research readers or from the specs, not re-checked here** (T0 checks the first two and `DS` on the live machine; T1 checks the spec items against IHI 0069):

- QEMU's GICv3 reset state with `secure=off` (`-machine virt,gic-version=3`, `justfile:43`): `GICD_CTLR.DS` = 1, `GICR_IGROUPR0` = 0, `GICR_IGRPMODR0` = 0, every `GICR_IPRIORITYR` byte = 0, PPIs level, SGIs edge, and `ICC_PMR_EL1` and `ICC_IGRPEN0/1_EL1` = 0. QEMU signals a Group 0 interrupt as FIQ (`hw/intc/arm_gicv3*.c`). `DS` comes from the GIC model's `security_extn`, which `virt` sets only for `secure=on`, so it does not depend on the firmware image. The ADR's monitor read of `IGROUPR0` = 0xffffffff on CPU 0 agrees: edk2's Non-secure write took effect, which happens only when `DS` = 1.
- edk2's `ArmGicV3Dxe` writes `GICR_IGROUPR0` = 0xffffffff when `DS` = 1, `ICC_BPR1_EL1` = 7 and `ICC_PMR_EL1` = 0xff, all on the calling CPU's redistributor (CPU 0) only.
- `GICR_IGRPMODR0` is RAZ/WI when `DS` = 1. When `DS` = 0, `GICR_IGROUPR0` and `GICR_IGRPMODR0` are RAZ/WI to Non-secure accesses. So from Non-secure EL1, `IGRPMODR0` reads 0 under either value of `DS`, and a check on it can never fail.
- `GICD_CTLR` bit 6 is `DS` (RAO/WI) in the `DS` = 1 view, and RES0 in the Non-secure view when `DS` = 0. A Non-secure read of bit 6 is therefore 1 exactly when `DS` = 1.
- Priority from Non-secure EL1: the priority of a Secure interrupt (Group 0 or Secure Group 1) is RAZ/WI. Under `DS` = 0 a Group 1 Non-secure interrupt's priority uses the Non-secure view: a write of `v` stores `(v >> 1) | 0x80`, and a read returns the stored value shifted left by one. An implementation has at least 4 priority bits (at least 5 with two Security states; QEMU implements 5), so a priority whose low four bits are 0 reads back exactly as written in every view.
- `GICR_WAKER` is RAZ/WI to Non-secure accesses when `DS` = 0 (written from memory of IHI 0069; T1 confirmed it in IHI 0069H.b §12.11.42). Row 1's wake loop over `shared::gic::waker_step` is correct either way, because a RAZ register reads (`ProcessorSleep`, `ChildrenAsleep`) = (0, 0), which the loop treats as done.

-----

## The change

### Registers, per CPU, on its own redistributor

Offsets are from IHI 0069 (Arm GICv3 and GICv4 Architecture Specification), chapter 12, register descriptions. RD_base is the CPU's redistributor frame, and SGI_base = RD_base + 0x10000. T1 records the revision and section numbers it was checked against in `shared/src/gic.rs` and in hal.md §4.1. The CPU-interface registers are from DDI 0487 (Arm ARM).

| # | Register (offset) | Action | Why |
| --- | --- | --- | --- |
| 1 | `GICR_WAKER` (RD_base + 0x014) | One bounded loop over `shared::gic::waker_step` (amended by T1, IHI 0069H.b §12.11.42): read; (0,0) done; (1,1) write the value with `ProcessorSleep` (bit 1) cleared, keeping the other bits (bits 0 and 31 are IMPLEMENTATION DEFINED); (0,1) or (1,0) read again. Pairs are (`ProcessorSleep`, `ChildrenAsleep` (bit 2)) | IHI 0069H.b §12.11.42: after power-up software clears `ProcessorSleep` and waits for `ChildrenAsleep` = 0, and "Changing ProcessorSleep from 1 to 0 when ChildrenAsleep is not 1 results in UNPREDICTABLE behavior", so the write waits out (1,0) (entering sleep) instead of being unconditional. Today's guard (`if ChildrenAsleep`) returns without waiting in (0,1), a wake in progress, and leaves (1,0) asleep |
| 2 | `GICR_ICENABLER0` (SGI_base + 0x180), then `GICR_CTLR.RWP` (RD_base + 0x000, bit 3) | Write `1 << intid`, then poll RWP until it reads 0, with a bound | The PPI is reconfigured while it is disabled. RWP tracks the effect of `ICENABLER0` writes |
| 3 | `GICR_IGROUPR0` (SGI_base + 0x080) | Write 0xffffffff, so every SGI and PPI is in Group 1 (Q4), then read it back. The readback is checked only when `DS` = 1. Under `DS` = 0 the register is RAZ/WI to this write, the group is Secure firmware's to set, and the group flag reads `unchecked`, not failed | Group 1 non-secure is the only group the kernel enables (`ICC_IGRPEN1_EL1`, `GICD_CTLR.EnableGrp1`). The value matches what edk2 leaves on CPU 0. The write is kept under `DS` = 0 too: it is ignored there, and Linux's `gic_cpu_init` writes it the same way on every platform |
| 4 | `GICR_IGRPMODR0` (SGI_base + 0xD00) | Not accessed | Modifier and group (0,1) is G1NS, but from Non-secure EL1 the register reads 0 under either value of `DS` ("Reported", above), so a check could never fail. Under `DS` = 0, a PPI that Secure firmware left in Group 0 or Secure Group 1 shows up in row 5 instead: its priority is RAZ/WI to Non-secure accesses, so the readback is 0 and the priority flag fails |
| 5 | `GICR_IPRIORITYR<n>` (SGI_base + 0x400 + intid, byte access) | Write one priority byte, the same on every CPU, then read it back | QEMU resets the byte to 0 on CPUs 1–3, and on CPU 0 it holds whatever edk2 left. T0 reads CPU 0's value, and the step writes that value on every CPU, so CPU 0's state does not change. The value must be non-zero (so a RAZ readback cannot pass), below `ICC_PMR_EL1` (0xFF), and have its low four bits 0 (so the readback is exact in every priority view). If T0's byte breaks any of these rules (0 included), the step picks no value of its own: the reading is recorded under "Issues Encountered" and goes to the owner as Q13 before T1. Writing a substitute such as 0xA0 on CPU 0 would break boot check 3's byte-identical rule. edk2's `ArmGicV3Dxe` is reported to write a default priority (`ARM_GIC_DEFAULT_PRIORITY`, 0x80) to every interrupt at init, which meets all three rules; that is from memory and not checked, so T0's reading decides |
| 6 | `GICR_ICFGR1` (SGI_base + 0xC04) | Read only: bit `2 * (intid - 16) + 1` must be 0 (level) | The timer PPI is level-sensitive. Changing the trigger of an enabled interrupt is UNPREDICTABLE, and whether a PPI's trigger is programmable is IMPLEMENTATION DEFINED, so the step checks the bit and does not write it |
| 7 | `GICR_ISENABLER0` (SGI_base + 0x100) | Write `1 << intid`, then read it back | Enable |
| 8 | `ICC_SRE_EL1`, `ICC_PMR_EL1`, `ICC_IGRPEN1_EL1` | Unchanged: SRE = 1 and ISB, PMR = 0xFF, IGRPEN1 = 1 and ISB | Already correct on both paths. `ICC_CTLR_EL1.EOImode` stays at its reset value 0, because the handler writes only `ICC_EOIR1_EL1`. `ICC_BPR1_EL1` is not written, because nothing nests (handlers run with PSTATE.I set) |

The global prerequisite, `GICD_CTLR.EnableGrp1` (bit 1, named `EnableGrp1NS` in the `DS` = 0 view; `GICD_CTLR_ENABLE_GRP1_NS` at `gic.rs:21`), is already set by CPU 0 before `CPU_ON`, and the step does not change it. Group 0 (`ICC_IGRPEN0_EL1`, `GICD_CTLR.EnableGrp0`) stays disabled (see "Facts checked" on the FIQ vectors).

The INTID is `dt.timer_ppi`, never the literal 30. CPU 0 stores it in a static next to `GICR_BASE` (`smp.rs:40`, `:83`) before `CPU_ON`, the secondaries read it, and `irq_handler_el1` matches against it (Q4 B).

`DS` is read once, on CPU 0: `init_gicv3` already reads `GICD_CTLR` (`gic.rs:51`), and it keeps bit 6 of that value. CPU 0 stores it next to the INTID before `CPU_ON`, and every CPU passes it to the readback check. `GICD_CTLR` is a distributor register, the same for every CPU, so one read serves all four.

### Where it lives: `arch/aarch64/gic.rs`, not the Platform trait

- **gic.rs** holds the register-level code: a single per-CPU function, used by `init_gicv3` (CPU 0, through `enable_irq`'s PPI branch) and by `init_gicv3_secondary`, so the two paths cannot drift apart again. hal.md says the arch driver programs registers and that per-CPU differences are "handled internally" by `InterruptController` (hal.md §4.1, table row "Per-CPU config").
- **Not the Platform trait.** Neither the current trait (`init_uart`, `init_interrupts`, `init_timer`) nor hal.md's target trait has a per-CPU or secondary-init method, and `secondary_main` already calls `gic::init_gicv3_secondary` directly (`smp.rs:228`). A per-CPU trait method changes the HAL design, and only GICv2 support (`gic_v2.rs`, planned) needs it. That belongs to the BSP work, not to a crash-fix step that must stay one behaviour change for its A/B (rule: no design creep inside a measured step).
- **shared/** holds the pure logic (next section), following the ADR's "Rules for every step PR" and the M25 shared-crate ADR.

### Host-testable logic in `shared/src/gic.rs`

The step adds a new `no_std` module with no hardware access:

- the register offsets and bit positions from the table above, as constants;
- `PpiConfig::new(intid, priority) -> Result<PpiConfig, GicError>`, which rejects INTIDs outside 16–31 and a priority that is 0, at or above `PRIORITY_LIMIT` (0xF0, the value `ICC_PMR_EL1` = 0xFF reads with 4 implemented bits; amended by T1), or has any of its low four bits set (row 5), and gives the enable mask, the priority byte offset and the `ICFGR1` bit position;
- `PpiReadback { wake_ok, rwp_ok, igroupr0, isenabler0, priority, icfgr1 }` and `PpiConfig::check(&PpiReadback, ds: bool) -> PpiStatus`: one flag per property (`wake`, `rwp` from the bounded polls; `group1`, `enabled`, `priority` as written, `level`). Each flag is `Pass`, `Fail` or `Unchecked`. The group flag is `Unchecked` when `ds` is false and checked otherwise; every other flag is always checked. `PpiStatus::ok()` is true when no flag is `Fail`;
- `waker_step(waker) -> WakerStep { Done, Wait, Write(u32) }` (row 1; replaces `waker_wake_value` and `waker_awake`, T1) and `security_disabled(gicd_ctlr) -> bool`;
- with Q4 C only: `find_redistributor(frames, mpidr_aff)`, which walks `GICR_TYPER` values (affinity in bits [63:32], `VLPIS` in bit 1 giving a stride of 0x20000 or 0x40000, `Last` in bit 4).

The tests cover:

- bit positions for INTID 16, 30 and 31; INTIDs 15 and 32 rejected; priorities 0, 0xFF and 0xA8 rejected, 0xA0 accepted;
- `check` with `ds` = 1 on the edk2 CPU 0 state (0xffffffff, enabled, level), which is `ok`;
- `check` with `ds` = 1 on QEMU's reset state for CPUs 1–3 (`IGROUPR0` = 0), which fails the group flag;
- `check` with `ds` = 0 on the RAZ/WI state (`IGROUPR0` reads 0 after the write, the rest as written): the group flag is `Unchecked` and `ok()` is true;
- `check` with `ds` = 0 and the priority reading 0 (Secure firmware left the PPI in a Secure group, so the write was ignored): the priority flag fails and `ok()` is false;
- `check` with an edge-triggered `ICFGR1`, which fails the level flag;
- `waker_step`'s `Write` value keeping bits 0 and 31 (T1);
- `waker_step` for the four `ProcessorSleep`/`ChildrenAsleep` combinations (T1).

Miri runs over these in CI (`ci.yml`).

### Reporting

- `init_gicv3_secondary` returns a `PpiStatus` and never prints. `secondary_main` passes it to `print_cpu_line` after the kmap TTBR1 install, inside its `PRINT_TURN` window. CPU 0 keeps its status in `InterruptController`, and `kernel_main` prints it on CPU 0's `[smp]` line.
- The `[smp]` line gains four fields: `ppi=<intid> ppi_grp1=0|1|- ppi_en=0|1 ppi_ok=0|1`, where `-` is the `Unchecked` group flag (`DS` = 0). For example: `[smp] cpu=1 vbar=0x0000000040081000 ttbr0=0x00000000402ad000 vbar_kva=0 ttbr0_idmap=1 ppi=30 ppi_grp1=1 ppi_en=1 ppi_ok=1`. Step 1a's harness does not parse `[smp]` lines (its non-goals), so the classifier and its goldens are unaffected. CI and soak boots run `DS` = 1 like local ones ("Facts checked"), so there the group flag is always checked. Their logs show whether CI's QEMU version and edk2 build leave the same state as the local ones, which no one has checked so far.
- When `ppi_ok=0`, a `kerror!(Smp, "cpu {} timer PPI {} setup failed: …")` names the failed flags. An `Unchecked` flag is not a failure and is not named. Whether the CPU then halts is Q5; the recommendation is to report and continue.
- No tripwire schema change. The step changes behaviour, not measurement, so the two arms' tripwire lines keep the same keys and step 1a's parser reads both.

-----

## Boot checks (development, before the A/B)

A failure in the first three is a bug in the step. The rest describe the kernel and are recorded in the PR. Logs come from `just run` (text mode), up to 3 boots. `main`'s text CLEAN rate is 55% (B1), so a single non-CLEAN boot is not a failure by itself; the A/B decides that.

1. **Per-CPU ticks.** In every `[tripwire] v=1 src=hb` line after the first, all four `tick` values are greater than 0, and each one grows between consecutive heartbeat lines. Today the line reads `tick=N,0,0,0`.
2. **`[smp]` lines.** Four lines, each with `ppi=30 ppi_grp1=1 ppi_en=1 ppi_ok=1`, and no `timer PPI … setup failed` line. CPUs 1–3 still read `vbar_kva=0 ttbr0_idmap=1`, and CPU 0 reads `vbar_kva=1 ttbr0_idmap=0`: N5 stays as it is (Q3).
3. **Register state, QEMU monitor.** T0's command, run unchanged in this worktree, writes `mon.txt`; compare it with T0's table for `main`:
   - CPUs 1–3 now match CPU 0 in `GICR_IGROUPR0`, bit 30 of `ISENABLER0`, the priority byte, `ICFGR1` and `IGRPMODR0`;
   - CPU 0's values are byte-identical to `main`'s. CPU 0 is not meant to change. Row 5 never writes a priority on CPU 0 other than the one T0 read; the only exception is an owner ruling under Q13 that changes CPU 0's byte, and that ruling then states the one expected difference here;
   - `GICD_CTLR` bit 6 (`DS`) reads 1.

   The addresses come from `GICR_BASE` 0x080A_0000: CPU k's RD_base is 0x080A_0000 + k × 0x20000 and its SGI_base is RD_base + 0x10000. On CPU 0: `GICR_CTLR` 0x080A_0000, `GICR_WAKER` 0x080A_0014, `IGROUPR0` 0x080B_0080, `ISENABLER0` 0x080B_0100, `IPRIORITYR7` 0x080B_041C (INTID 30 is bits [23:16] of that word), `ICFGR1` 0x080B_0C04, `IGRPMODR0` 0x080B_0D00. `GICD_CTLR` is 0x0800_0000.
4. **`irqsw`.** In a boot that completes Gate 1, the sum of `irqsw` over CPUs 1–3 is above 0, most likely on CPUs 1 and 2, where the `wfe`-looping B and C now rotate with the migrated Normal threads. `irqsw0` reads about 1 on each of CPUs 1–3, from the PPI already pending when `enter_scheduler` unmasks.
5. **No new hazard lines in a CLEAN boot:**
   - no `PANIC`, no `EXCEPTION[CPU n]` and no `lock re-entry:`;
   - no `[tripwire-ev] kind=self`, and no `kind=stuck` line naming a CPU from 1 to 3;
   - `pcphys`, `elrmm[1..3]` and `n4` are recorded and not judged. They are expected to become non-zero (see the prediction table);
   - every `[tripwire-ev]` or `lock re-entry:` line, in any boot, that names CPU 1, 2 or 3 as its CPU or `owner_cpu` prints a readable `holder=<file>:<line>`. The step makes the IRQ path on CPUs 1–3 run for the first time, and with it 1b's physical-alias normalisation (`kva_of`, `kernel/src/sync/irq_spin_lock.rs:437-453`, used at `:299`), which never fires on CPU 0. A garbled or missing holder there is a bug in 1b's normalisation, now live. If no such line occurs in these boots, the check is read in arm B's logs at T8.
6. **Gates (rule 02):**
   - `just check` and `just test`, with zero warnings;
   - `cargo build --target aarch64-unknown-none`;
   - `llvm-objdump -h` on the kernel ELF, run from the toolchain sysroot the way step 1b ran it, with sections at their usual addresses;
   - `just run`: EL = 1, core 0, `4 CPUs online`.
7. **Newly live self-tests** (recorded, not judged). Count per boot: `Thread A on core`, `Server: started` (with `(^|[^-])Server: started, channel`), `Caller: got`, `Caller: ipc_call failed`, `unauthorized ChannelCreate -> EPERM`, `Echo server: started`, `Echo client: got`, `Echo client: ipc_call failed` and `Echo client: EPIPE after service death`. All of them read 0 in the 32 logged boots ("Facts checked"), and most are expected to appear now (prediction table). A `PANIC` or `EXCEPTION` whose first report lies in one of these threads or in the `process_exit` path is recorded for Q14.

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

**What gates (proposed; Q1, Q2, Q12 and Q14 decide, and the answers are written into this plan before T8 runs the pair):**

1. **The fix itself, mechanically.** In arm B, every boot whose last complete tripwire line has `tick[0]` > 0 also has `tick[1..3]` > 0 in that line. In arm A, every boot with a complete tripwire line has `tick` = 0 on CPUs 1–3. This is read from step 1a's per-CPU `tick` columns.
   - Why the scope: no CPU takes a timer IRQ before the scheduler starts. CPU 0 unmasks after `sched::start` (`main.rs:371`, `:380`), and the secondaries unmask only once `SCHED_READY` is set (`scheduler.rs:55-63`), which `sched::start` does, with a `sev`, before CPU 0 unmasks (`sched/init.rs:108-112`). A boot that panics or faults earlier still prints a complete `src=panic` or `src=exc` line (`observability/tripwire.rs:353-375`), and it reads `tick=0,0,0,0` whatever the fix does. `tick[0]` > 0 shows that the scheduler started, so in scope are every `src=hb` and `src=g1` line plus the `src=panic` and `src=exc` lines taken after CPU 0 ticked.
   - Boots out of scope (no complete tripwire line, or a last complete line with `tick[0]` = 0) are listed in the report per arm, with their class and first fatal line, so none is silently dropped.
2. **The regression guard**, in the form Q12 pre-registers. It fails if CLEAN is lower in B with one-sided Fisher p < 0.05. Step 1a's pair report computes one test per run, on conclusive boots only (its D4), so the two commands give two tests: text at up to 20 boots per arm and gpu at up to 10. The ADR's sensitivity figure ("Statistics": a drop from 60% to 30% CLEAN caught about two times in three) assumes 30 boots per arm, and neither run has that. Exact one-sided Fisher power at α = 0.05:

   | Boots per arm | CLEAN in A → B | Power |
   | --- | --- | --- |
   | 30 | 60% → 30% (the ADR's figure) | 0.67 |
   | 20 | 60% → 30% | 0.46 |
   | 20 | 55% → 30% (from B1's text rate) | 0.35 |
   | 20 | 55% → 25% | 0.49 |
   | 10 | 10% → 0% (B1's gpu rate) | 0.01 |

   The gpu guard cannot fail at B1's baseline: 1 of 10 against 0 of 10 gives p = 0.50, and arm A needs at least 4 CLEAN of 10 before 0 of 10 in B reaches p < 0.05 (p = 0.043). Proposed (Q12 A): the text guard gates, and the gpu guard is reported as informational. The gpu boots still count in the per-boot attribution (Q2) and the `new` marker. If the guard fails, Q2's ruling applies.
3. **"Class removed".** No class is a target of this step, so a `removed` marker is reported and not claimed.
4. **The `new` marker.** A class with 0 boots in A and p < 0.05 for more in B is reported, together with the first fatal line of every boot in that class.

**Expected movement.** These are predictions, recorded so the PR can compare them with the results; none of them gates. "Today" is B1 (c065b0c) and the tripwire logs since.

| Key / class | Today | Expected in B | Why |
| --- | --- | --- | --- |
| `tick[1..3]` | 0 | close to `tick[0]` (about 45,000 in a 75 s CLEAN text boot) | The fix |
| `irqsw[1..3]` | 0 | > 0, mostly CPUs 1–2; CPU 3 may stay low | B and C never yield, so only the tick moves them off. On CPU 3 the Interactive PI-Caller keeps being repicked |
| `irqsw0[1..3]` | 0 | about 1 each | The PPI is already pending at `enter_scheduler`'s unmask |
| `pcphys` | 0 in all 82 Full lines | > 0, roughly the number of restores of threads switched out on CPUs 1–3's IRQ path | VBAR is still physical on CPUs 1–3 (N5, step 5). It stops being a harm signal, and only a restore on CPU 0 is fatal |
| `elrmm`, `spsrmm` [1..3] | 0 | > 0 | H1 on CPUs 1–3: two threads that both left through the IRQ stub resume with each other's ELR. Of the 9 PCZERO boots on disk with a `src=exc` line, 8 read `elrmm` = 2 on CPU 0 and one (`pr239-20261010-115625-text/run-01`) reads 1 |
| `n4`, `nest`, `insched`, `n1` [1..3] | 0 | can be > 0 | These paths become reachable on CPUs 1–3 |
| `lb` | 5 per CLEAN boot | higher | `try_load_balance` runs on every CPU whose tick lands in a `TICK_COUNT % 4 == 0` window |
| `starved[Normal]` | 9 at the end of every CLEAN text boot (99 over 11) | lower, not 0 | Threads migrated into CPUs 1–2 (A, ipc-server, cap-denied and others; "Facts checked") now get turns. CPU 3's Normal threads stay starved behind the Interactive PI-Caller's yield loop (`ipc/tests/mod.rs:769-771`), and CPU 0's behind the bench loops (N10, independent of #200) |
| `starved[Idle]` | 4 | 4 | Every CPU always has a non-idle Runnable thread |
| `ctbusy`, `xdir`, `xrep` | 2/2, … | higher | `check_timeouts` runs on four CPUs (N7's premise restored), and the newly live self-tests' calls and replies cross CPUs (rows below) |
| `ubmove[to]` | 2 per CLEAN text boot (B1 run-01: `ubmove=…,2,…` at index 11) | either way | Today's two are most likely the 5 s timeouts of ipc-caller and echo-client, which last ran on CPU 1 and are woken by CPU 0's tick (not traced per thread). If their servers now reply in time, those two go away, while `check_timeouts` on four CPUs adds other off-CPU timeout wakes |
| `lkph`, `lkpho`, `lkphrun`, `kind=ph` | 0 in all 30 B1 boots | can be > 0 | Lock-holder preemption on CPUs 1–3: `IrqSpinLock` does not mask IRQs |
| `scanstall` | 0 | can be > 0 | All four CPUs now gate the two-strike confirmation. A CPU that halts or spins with IRQs masked freezes its `tick`, which withholds confirmations, so a WEDGE can lose its `nowaker` attribution |
| `irqsw[0]` per tick | — | may fall | `NEED_RESCHED` is one global flag that any CPU's `schedule()` clears (H4) |
| PCZERO, EXCEPTION | text 1 + 2 of 20 | may rise, with first fatal reports on CPUs 1–3 | H1 on CPUs 1–3 |
| N5: EXCEPTION on CPU 0 with EC 0x21 (instruction abort) or 0x25 (data abort) and ELR or FAR in [0x4000_0000, 0x8000_0000) | never seen | possible, new signature | The ADR's N5 row (ADR `:101`): a thread switched out inside an IRQ on CPU k saves a physical-alias PC and return addresses, and may hold physical-alias static pointers in x19–x28. Migrated to CPU 0 and resumed under CPU 0's user TTBR0, it faults on the fetch (0x21) or on a load or store through such a pointer (0x25). The range is the ADR's run-167 check. B and C (Normal, all-CPU affinity) are the likely carriers |
| PANIC-LOCK with `on CPU 1\|2\|3 ctx=irq\|irq-exit` | impossible today | possible | H3 on CPUs 1–3: `schedule` and the balancer's `THREAD_TABLE.lock()` run on their IRQ path |
| WEDGE-STUCK, WEDGE-ALIVE | text 3 of 20 (all N2) | either way | N2's window is open to preemption on CPUs 1–3 (up). Some stalls may now end as PANIC-LOCK (down). Candidate (d) of WEDGE-ALIVE becomes reachable |
| gpu classes | 1 CLEAN, 9 EXCEPTION | about the same | Censored by CPU 0's `compositor_loop` EXCEPTION at the first IRQ-path switch (9 of 10) |
| `Timeout test: unexpected result -6` | every boot | unchanged | EPERM from a capability check, not a timer effect |
| `Server: started`, `Caller: got` (#240: the echo IPC self-test, tids 8 and 9) | 0 in all 32 logged boots | > 0 in most arm-B boots | tid 8 is migrated to CPU 1 in 32 of 32 boots, after B's `wfe` loop has started, and starves there only for lack of a tick ("Facts checked"). tid 9 most likely made its first `ipc_call` on CPU 1 before B's loop (no line marks it) and was woken by its 5 s timeout onto CPU 0's queue: in 12 of 20 B1 text boots it is later migrated from CPU 0 to CPU 2. With the tick, the server's `recv` takes the pending call and replies, or the caller logs `ipc_call failed` and its later calls are served. This is #200 itself, not a second change (Q10). In gpu boots tid 8 later bounces between CPUs 0, 1 and 3, and the compositor EXCEPTION censors most of them |
| `unauthorized ChannelCreate -> EPERM` (cap-denied, tid 13) | 0 in 32 | > 0 in most boots | Migrated to CPU 2, behind C's `wfe` loop, in 32 of 32 boots |
| `Thread A on core` (tid 4) | 0 in 32 | > 0 in most boots | Migrated to CPU 1 in 32 of 32 boots. A then ends in its own `wfe` loop, like B |
| Service echo pair (tids 14, 15): `Echo server: started`, `Echo client: got` or `ipc_call failed`, `… triggering process_exit for pid=7`, `EPIPE after service death` | client: `started` and `found service` in 32 of 32, nothing after; server: 0 | server lines in the boots where tid 14 reaches CPU 1 or 2; client lines in most boots | tid 14's first migration is to CPU 1 in 12 of 20 text and 10 of 10 gpu boots, to CPU 3 in 3 text boots (behind the Interactive PI-Caller), and none in 5. The client goes on to `process_exit(pid 7)` whether its call succeeds or times out, so the process-exit path (`pexit` wakes, channel death) runs in arm B for the first time in these logs |
| IPC keys these tests drive: `ubrun`, `ubrbl`, `ubdead`, `ubnone` [`call`, `reply`, `pexit`, `chdestroy`], `n2` (rpre, rblk, vpre, vblk), `latereply` | as today | can rise | Their calls and replies cross CPUs (servers on CPU 1 or 2, callers on CPUs 0–2) and end in a process exit. B2 (N2's re-baseline) inherits this traffic |
| `badchan` | 3,2,1 (B1 run-01) | unchanged by these tests | They use valid channel ids. The out-of-range ids come from ipc-timeout's bad-id, select and syscall-argument tests, which already run in 32 of 32 boots (`Bad-id test` in every B1 log) |

**Report.** The PR carries the following:

- step 1a's pair report for each mode;
- the per-arm tripwire table;
- the mechanical fix check ("What gates", item 1) per arm, with the boots outside its scope listed;
- the table above with its observed column filled in;
- per arm and mode, the counts of boot check 7's self-test lines, and a statement that the self-tests run because of the one change, not a second one (Q10);
- for every boot in arm B that is not CLEAN: its class, the CPU of its first fatal report, and which hypothesis's signature it carries (H1, H3, N2, N4 or N5, with N5's signature as in the table above), or "newly live self-test" as Q14 rules, or "unattributed";
- the gate decisions as Q12 pre-registered them, with the per-mode Fisher p-values and, for the gpu run, the note that its guard was informational.

The results go into a research note, `docs/knowledge/research/2026-10-1x-jl-crash-fix-200-ab.md`, in B1's format.

-----

## Re-baselined after merge

- **N2 (B2).** The ADR re-baselines N2 after this step and before the capability-lifetime PR converts the blocking paths. That re-baseline reads `n2` (rpre, rblk, vpre, vblk), `ubrun` and `ubrbl[reply]`, `latereply`, `ctbusy` and `nowaker`. Whether arm B of this pair counts as that baseline is Q6. B2 also carries the IPC traffic of the newly live self-tests (both echo pairs, cap-denied, the echo client's `process_exit`), which B1 never had, so a B1 → B2 difference in those keys is not preemption alone.
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
| #240 | A comment: the per-arm counts of boot check 7's self-test lines; the cause the logs show (tids 8, 9 and 13 starve on CPUs 1–2 for lack of a tick, which is #200); that this step resolves that cause as a side effect of its one change, as Q10 rules; and that CPU 0's Normal starvation behind the bench (N10), which the threads escape only because the balancer moves them, stays open. Closing #240 is proposed, not done, and is the owner's call | T8 |

-----

## Tasks

Each task is one commit, `Crash fix step 200: <description>`, with the rule 02 gates (`just check`, `just test`) passing before it. Tasks that change the kernel also boot it (`just run`).

- [x] **T0. Live GIC state on `main` (results in this plan; committed as the plan only).** Done 2026-10-11 on f3b039b at load 16.8: table under "Issues Encountered", T0.
  - On a QEMU monitor boot of 353f619 (f3b039b since the merge of 2026-10-11; see "Decisions Made", Merge), record for each CPU: `GICR_WAKER`, `IGROUPR0`, `IGRPMODR0`, `ISENABLER0`, `IPRIORITYR7`, `ICFGR1` and `GICR_CTLR`, plus `GICD_CTLR` (`DS` bit 6, `ARE` bit 4, `EnableGrp1` bit 1, `EnableGrp0` bit 0).
  - The boot uses `just run`'s QEMU command line (`justfile:41-53`) with a monitor socket added, because no recipe exposes a scriptable monitor. Run it from a checkout of f3b039b (the arm-A worktree `.claude/worktrees/crash-200-base`); boot check 3 runs the same lines unchanged in this worktree. `D` is a new empty directory under this worktree's gitignored `target/dig/200/` (T0 used `target/dig/200/gic-mon-f3b039b`).
  - macOS `nc -U -w 2` works (T0, 2026-10-11): every reply arrives, but the monitor's readline echoes each typed character with cursor escapes and every line ends in CR, so the pipe strips CR and keeps only the `xp` value lines and the version line. `mon.txt` then holds exactly 30 lines.

    ```sh
    just disk create-data-disk
    D=/Users/juslee/dev/aios/.claude/worktrees/crash-200/target/dig/200/gic-mon-<sha>; mkdir -p "$D"
    qemu-system-aarch64 -machine virt,gic-version=3 -cpu cortex-a72 -smp 4 -m 2G -nographic \
      -bios "${AIOS_EDK2_FW:-/opt/homebrew/share/qemu/edk2-aarch64-code.fd}" \
      -drive if=none,id=disk0,file=aios.img,format=raw -device virtio-blk-pci,drive=disk0 \
      -drive if=none,id=data0,file=data.img,format=raw -device virtio-blk-device,drive=data0 \
      -device ramfb -monitor unix:"$D/mon.sock",server,nowait \
      < /dev/null > "$D/serial.log" 2>&1 &
    QPID=$!
    # Wait with an until-loop (no foreground sleep) until this holds:
    #   [ "$(grep -c 'src=hb' "$D/serial.log")" -ge 2 ]
    { for k in 0 1 2 3; do
        rd=$((0x080A0000 + k * 0x20000)); sgi=$((rd + 0x10000))
        printf 'xp /1wx 0x%x\n' $rd $((rd + 0x14)) $((sgi + 0x80)) $((sgi + 0xD00)) \
          $((sgi + 0x100)) $((sgi + 0x41C)) $((sgi + 0xC04))
      done
      printf 'xp /1wx 0x8000000\ninfo version\n'
    } | nc -U -w 2 "$D/mon.sock" | tr -d '\r' \
      | grep -aE '^[0-9a-f]{8}: 0x[0-9a-f]{8}$|^[0-9]+\.[0-9]+\.[0-9]+' > "$D/mon.txt"
    kill "$QPID"
    ```

    Per CPU k, `mon.txt` lists `GICR_CTLR`, `GICR_WAKER`, `IGROUPR0`, `IGRPMODR0`, `ISENABLER0`, `IPRIORITYR7` and `ICFGR1` in that order, then `GICD_CTLR` and the QEMU version. The agent stops only its own QEMU (`$QPID`). If `nc -U` misbehaves on the host, T0 fixes the command here first, so that boot check 3 stays "run the command, read the output".
  - Choose the priority from CPU 0's byte at offset 0x41E (bits [23:16] of the `IPRIORITYR7` word), under row 5's rules. If the byte breaks any of them (0 included), record it and stop for Q13 before T1.
  - Note the QEMU version and the firmware path and sha256. Note CI's QEMU and edk2 versions from the latest `ci.yml` soak job log.
  - Check: the table is in "Issues Encountered" below.
- [x] **T1. Pure logic in `shared/src/gic.rs`, with host tests** (the section above; Q4 decides whether `find_redistributor` is included). Done 2026-10-11: 24 tests; Q4 B, so no `find_redistributor`; spec checked against IHI 0069H.b (Decisions Made, T1).
  - Check: `just test`, and Miri on the new tests (`cargo +nightly miri test -p shared gic`).
- [x] **T2. One per-CPU PPI setup in `gic.rs`, used by CPU 0 and CPUs 1–3:** Done 2026-10-11: one soak boot at load 15.9, CLEAN, boot checks 1 and 4 pass (Issues Encountered, T2; Decisions Made, T2).
  - the `GICR_WAKER` wake loop over `shared::gic::waker_step` on both paths, as row 1 describes (CPU 0 still panics on timeout, and a secondary records the timeout in its status as `wake_ok` false);
  - disable, RWP, group, priority, level check, enable and readback, all through `shared::gic`;
  - `enable_irq`'s PPI branch calls the setup and is documented as boot-CPU-only;
  - the timer INTID and `GICD_CTLR.DS` are stored before `CPU_ON`; the INTID is used by `init_gicv3_secondary` and `irq_handler_el1`, and `DS` by every CPU's readback check;
  - a `// SAFETY:` comment in rule 06's form on every new unsafe block.
  - Check: `just check`, `just test`, and `just run` passing boot checks 1, 2 and 4. Boot check 2 shows the new fields only from T3 on; until then, ticks alone.
- [x] **T3. Report:** Done 2026-10-11: two soak boots at load 10.2 and 23.5 pass boot check 2 (one PCZERO, one CLEAN); the fault-injection boot (load 8.1) passes (Issues Encountered, T3; Decisions Made, T3).
  - `PpiStatus` reaches `print_cpu_line` on every CPU; the four `[smp]` fields; the `kerror!` on failure;
  - observability.md "SMP lines";
  - a fault-injection check: a temporary local build that skips the `IGROUPR0` write on CPU 2 prints `ppi_grp1=0 ppi_ok=0` and the error line on CPU 2 only, and `tick[2]` stays 0. The build is not committed, and the result is recorded in this plan.
  - Check: boot check 2.
- [x] **T4. Code comments made stale:** `shared/src/tripwire.rs` (`TwoStrike` doc and test comment) and `kernel/src/sync/selftest.rs`. Done 2026-10-11: comments only, the test unchanged; no boot (no kernel code changed) (Decisions Made, T4).
  - Check: `just check`, `just test`.
- [x] **T5. Development boots and the register check:** Done 2026-10-11 on ab06b40 at load 8.0–10.0: three soak boots (1 CLEAN, 2 PCZERO with H1's signature on CPUs 1 and 2), the monitor read and `llvm-objdump -h` (Issues Encountered, T5).
  - boot checks 1–7 on up to 3 `just run` boots, plus boot check 3 against T0's table;
  - `llvm-objdump -h`.
  - Results into this plan.
- [x] **T6. Docs:** Done 2026-10-11: docs only, no boot (no kernel code changed); `docs-check` shows only the two findings this branch already had (Decisions Made, T6).
  - the stale-docs table above, except the T3/T4 rows and #240;
  - the ADR's dated notes, with no rulings invented: Q1–Q3 are recorded only once the owner has answered;
  - `.claude/CLAUDE.md`, by the lead, through the permission prompt;
  - `cargo run -q -p aios-tools -- docs-check`, with no new drift except this plan's working-plan finding.
- [ ] **T7. `/audit-loop`** (doc, code and security/bug review) until a clean round. The review covers in particular:
  - virtual against physical addresses (GICR is reached physically through TTBR0 before the kmap install);
  - the bounded polls;
  - the `DS`-dependent readback (group flag `Unchecked` under `DS` = 0, and the priority rules of row 5);
  - the PPI disable window;
  - that nothing new runs on the IRQ path except the INTID compare;
  - every address the IRQ path publishes across CPUs, which is now live on CPUs 1–3 at physical-alias PCs: the `kva_of` sites (holder `Location`s, `irq_spin_lock.rs:299`), `irq_elr` in the panic report (`observability/tripwire.rs:345-372`), and any other value computed on the IRQ path and stored, compared or printed by another CPU (the `.claude/CLAUDE.md` rule "IRQ-path address values (1b)"). Boot check 5's `holder=` check is the runtime half of this.
- [ ] **T8. Merge `main` after step 1a lands, then run the A/B pair:**
  - re-run the gates;
  - write the owner's Q1, Q2, Q12 and Q14 answers into "What gates" before the pair starts (pre-registration), and commit that;
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
  - the owner's answers to Q1–Q3, Q12 and Q14 before the pair is run, to Q4 and Q5 before T1 and T2, and to Q10 before T8 writes the #240 comment;
  - Q13 before T1, only if T0's priority byte breaks row 5's rules.
- **Coordination with team-build on #240.** The owner's instruction, relayed to this session on 2026-10-10: this step handles #240 when it is time. The team-fix lead tells team-build so that team-build stops its own #240 work and restarts it once this step completes; whatever #240 still needs then (CPU 0's Normal starvation, N10, or the test placement) is team-build's. The lead sends that message, not an agent of this plan.
- **Risk: self-tests run for the first time.** The two echo pairs, cap-denied, thread A and the echo client's `process_exit` have not run in any of the 32 logged boots ("Facts checked"), so a latent defect in them, or in the IPC and process-exit paths they drive, shows first in arm B. Such a boot carries none of H1, H3, N2, N4 or N5's signatures and would read "unattributed" under Q2 A. Mitigation: boot check 7 counts their lines in development, the prediction table expects them, Q14 decides how their failures are attributed before the pair, and the report lists them per boot.
- **Risk: the regression guard fails because of exposure, not the fix.** 9 of B1's 20 text boots already end non-CLEAN with only CPU 0 ticking (H1 3, N2 3, H3 3). Mitigation: Q2's ruling in advance, and per-boot attribution in the report.
- **Risk: the regression guard passes a real regression.** At 20 text boots per arm it catches a drop from B1's 55% to 30% CLEAN about one time in three (0.35), and the gpu guard cannot fail at B1's 1 of 10 ("What gates", item 2). A pass is weak evidence that nothing regressed. Mitigation: Q12 states this before the run, and Q2 A's per-boot attribution reads every non-CLEAN arm-B boot in both modes, whatever the p-value.
- **Risk: N5 goes live** (Q3). A thread switched out inside an IRQ on CPU k and migrated to CPU 0 faults there. Mitigation: the signature (CPU 0, EC 0x21 or 0x25, ELR or FAR in [0x4000_0000, 0x8000_0000), usually after a `pcphys` increment) is distinct and attributable, and step 5 removes it.
- **Risk: 1b's IRQ-path normalisation runs for the first time.** CPUs 1–3 have never taken an IRQ (every logged `tick` line reads 0 for them), so `kva_of`'s physical-alias branch and every other IRQ-path publish on CPUs 1–3 have never run. A defect there would show as an unreadable holder or a fault on the CPU that prints it. Mitigation: T7's review list and boot check 5's `holder=` check.
- **Risk: CPU 0's GIC state changes by accident.** Rewriting the priority or `IGROUPR0` on CPU 0 with other values than edk2's would add a second difference to the pair. Mitigation: row 5 writes on every CPU the priority T0 read on CPU 0, and a reading that breaks row 5's rules (0 included) goes to the owner (Q13) instead of being replaced. T0 and boot check 3 then require CPU 0's registers to be byte-identical, unless a Q13 ruling names the one CPU 0 change.
- **Risk: `DS` = 0 (real GICv3 hardware with Secure firmware).** Local, CI and soak boots all run `DS` = 1, because no QEMU command line in the repo sets `secure=on` ("Facts checked"), so no boot this step makes can reach this case. On such hardware Non-secure `IGROUPR0` writes are ignored and the register reads 0, so a group readback would report a failure on every CPU on every boot even when the firmware has already put the PPI in Group 1 Non-secure. Mitigation: the group flag is `Unchecked` under `DS` = 0 (`ppi_grp1=-`), and `ppi_ok` rests on the wake, RWP, priority, level and enable readbacks. A PPI that the firmware left in a Secure group still fails, through the priority readback (row 5). The step does not try Secure-only registers. Its host tests cover the `DS` = 0 cases; no boot does.
- **Risk: the first IRQ on each secondary arrives inside `enter_scheduler`'s unmask window.** The PPI is pending as soon as IRQs are unmasked, so the first IRQ dispatches with no current thread (`irqsw0`). CPU 0 takes this path too, but on secondaries it becomes deterministic. Mitigation: counted (`irqsw0`). Moving `init_timer_secondary` later is not part of this step.
- **Risk: the IRQ handler's INTID compare reads a static.** That adds one relaxed load to the IRQ path. It allocates nothing and uses no V registers. T7 reviews it.
- **Risk: scope creep into steps 2, 5, 6a and 6b** (the balancer from four CPUs, the global `NEED_RESCHED`, VBAR). Mitigation: Q3 and Q9 keep them out, so the pair measures one change.
- **Not in scope, noted:** the platform reader reports that Pi 5 (BCM2712) has a GIC-400 (GICv2), while bsp.md, drivers.md, platforms.md, model.md, firmware.md, testing.md and hal.md call it GICv3. This plan has not verified that. If it holds, `gic.rs` serves QEMU only, and both Pi boards need `gic_v2.rs`. The lead files a separate issue; this step does not touch those docs.

-----

## Open questions for the owner

**Answered 2026-10-10 16:29 (owner): every recommendation taken: Q1 A, Q2 A, Q3 A, Q4 B, Q5 A, Q6 A (B as the fallback), Q7 A, Q8 A, Q9 A, Q10 A, Q11 A, Q12 A, Q13 B, Q14 A.** The questions stay below as the record of the options.

**Q1. What does this step's acceptance gate on, beyond the regression guard?** The ADR says only "A/B-measured like the other steps".
- A. The regression guard, plus the mechanical fix check: every arm-B boot whose last complete tripwire line has `tick[0]` > 0 also has `tick[1..3]` > 0. A boot that dies before the scheduler starts reads `tick=0,0,0,0` whatever the fix does, so it is out of scope and listed in the report ("What gates", item 1). Everything else (`irqsw`, `starved`, `pcphys`, classes) is reported, not gated.
- B. A, plus `irqsw[1..3]` > 0 and a fall in `starved[Normal]` as gates.
- C. The regression guard only.
- Recommendation: A. B gates on predictions that depend on test-thread placement (CPU 3 may legitimately stay low). C does not check that the fix took effect.

**Q2. What happens if the regression guard fails (CLEAN lower in arm B, p < 0.05)?** That is plausible, because the fix makes H1, H3, N4 and N5 reachable on three more CPUs. The ADR has no rule for a fix that exposes known defects. The guard's real sensitivity is lower than the ADR's figure, because each mode is its own test (Q12): in text, at 20 boots per arm, a drop from B1's 55% to 30% CLEAN reaches p < 0.05 only about one time in three (0.35), and the gpu guard cannot fail at B1's 1 of 10. So a failure means a large drop, and a pass does not show that nothing regressed.
- A. Attributed exposure passes. The step merges if every extra non-CLEAN boot in arm B carries a known hypothesis's signature (H1, H3, N2, N4 or N5), or a newly live self-test's if Q14 A is chosen, and no unattributed signature appears. N5's signature is the ADR's: an EXCEPTION on CPU 0 with EC 0x21 or 0x25 and ELR or FAR in [0x4000_0000, 0x8000_0000). Attribution reads every non-CLEAN arm-B boot in both modes, whether or not the guard failed. The ADR records the ruling and the lower baseline. Later steps (2, 3, 5, 6a) are measured against it.
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
  - unconditional WAKER (amended by T1 to row 1's bounded `waker_step` loop; see Decisions Made), disable and RWP, `IGROUPR0` = 0xffffffff, the priority written, the level checked, enable, readback (the group readback only when `GICD_CTLR.DS` = 1);
  - the INTID from the DTB in the setup and in the handler;
  - the pure logic in `shared/`.
- C. B plus a `GICR_TYPER` affinity walk instead of `core_id × 0x20000`, and per-CPU init moved behind `InterruptController`.
- Recommendation: B. It follows the lesson's rule ("never relies on firmware state") on every CPU, and on QEMU it leaves CPU 0's state unchanged. C's walk and HAL change make no difference on QEMU and belong to the GICv2 BSP work.

**Q5. What does a CPU do when its readback fails (`ppi_ok=0`)?**
- A. Report and continue: the `[smp]` fields plus a `kerror!`. The CPU runs cooperatively, as today, and its `tick` stays 0.
- B. Halt that CPU, in a `wfe` loop, after printing.
- C. Panic.
- Recommendation: A. The failure is visible in every log (CI included), and the kernel behaves exactly as it does today. B and C turn a platform difference that no boot of this step can reach (real hardware, where the firmware owns more of the GIC) into a boot failure. `DS` = 0 alone does not fail the readback (the group flag is `Unchecked` there).

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

**Q10. How does this step handle #240 (the echo IPC pair never runs)?** The owner has said this step handles #240 when it is time (relayed 2026-10-10; "Dependencies & Risks"); what is open is how. The B1 logs make #200 the likely cause. The pair starts on CPU 0's queue behind the Interactive bench, but the balancer moves ipc-server (tid 8) to CPU 1 in 32 of 32 boots, after B's `wfe` loop has started, and there it starves only because CPU 1 never ticks; cap-denied (tid 13, to CPU 2), thread A and the service echo pair do the same ("Facts checked"). So the step will most likely make the pair run for the first time, as a side effect of its one change. CPU 0's own Normal starvation (N10) is untouched: the threads escape it only because the balancer moves them.
- A. Side effect, stated: the PR names the newly live self-tests, reports per-arm counts of their lines (boot check 7), and comments on #240 that this step resolves its immediate cause, proposing to close it if arm B shows `Server: started` and `Caller: got`. No #240-specific code. What remains (N10, test placement) goes back to team-build when it restarts.
- B. A, plus a #240-specific change here (the pair made Interactive, or queued off CPU 0). That is a second change in the pair: the self-tests would then run in arm B for two reasons, and the A/B could not attribute their traffic and failures to the GIC change.
- Recommendation: A. The fix already removes the cause the logs show, so the step can resolve #240 without a second change, and the pair still measures the GIC change alone.

**Q11. The step's name.** The ADR's rule is `claude/crash-fix-step-<n>-<name>` with commits `Crash fix step <n>: …`, but #200's step has no number.
- A. Keep `step-200` (branch `claude/crash-fix-step-200-gic-groups`, commits `Crash fix step 200: …`), named after the issue.
- B. Renumber the series.
- Recommendation: A. The ADR already refers to it as "#200's fix step", and renumbering would invalidate every later step's references.

**Q12. How is the regression guard pre-registered?** (Answered before T8 runs the pair, and written into "What gates".) Step 1a's pair report computes one Fisher test per run (its D4), so the two commands give a text test at up to 20 boots per arm and a gpu test at up to 10. The ADR's figure (60% → 30% CLEAN caught about two times in three) assumes 30 per arm. Exact one-sided Fisher power is in "What gates", item 2.
- A. Per mode. The text guard gates, at power 0.35 for 55% → 30% and 0.46 for 60% → 30%. The gpu guard is reported as informational: at B1's 1 of 10 it cannot reach p < 0.05.
- B. Pool CLEAN across the two runs by hand (30 per arm) and gate on the pool, still reporting per mode. B1's pooled rate is 12 of 30 (40%), so the ADR's 60% → 30% figure does not apply either: a drop from 40% to 20% is caught 0.42 of the time. The pool also mixes two modes with different rates, and gpu's CLEAN is censored by CPU 0's `compositor_loop` EXCEPTION.
- C. A, with the text run at `runs=30`: the ADR's 30 per arm in the mode that can carry it (0.67 for 60% → 30%). The pair grows from 60 to 80 boots, about 25 minutes more (about 100–115 minutes in all).
- Recommendation: A. It is the shape step 1b's own pair uses (20 text, 10 gpu; step 1a's plan), and Q2 A's attribution reads every non-CLEAN arm-B boot in both modes whatever the p-value, so the guard is not the only check on exposure. C if the owner wants the ADR's stated sensitivity in text.

**Q13. Asked only if T0's CPU 0 priority byte breaks row 5's rules (0, at or above 0xFF, or a low four bits not 0).** (Answered before T1.) Row 5 writes CPU 0's byte on every CPU so that CPU 0 does not change. A byte that breaks the rules cannot be written and checked that way: 0 cannot be told apart from a RAZ readback, and a byte with low bits set does not read back exactly in every priority view.
- A. Use a fixed byte (0xA0, Linux's `GICD_INT_DEF_PRI`) on every CPU, CPU 0 included. The priority stays checked on every CPU, but CPU 0's timer priority changes, which is a second difference in the pair. Boot check 3 and the risk "CPU 0's GIC state changes by accident" then name that byte as the one expected CPU 0 change.
- B. Keep CPU 0's byte as read and write it back unchanged on CPU 0, with its priority flag `Unchecked` there (as the `DS` = 0 group flag is); CPUs 1–3 use 0xA0. CPU 0 stays byte-identical, and the PPI's priority differs between CPU 0 and CPUs 1–3. Priority only orders interrupts on one CPU against each other and against `ICC_PMR_EL1` (0xFF), and the timer PPI is the only interrupt the kernel enables (`enable_irq`'s one caller is `timer.rs:65`), so the difference has no effect today.
- C. Stop the step and look at why edk2 left that byte before choosing.
- Recommendation: B. It keeps the pair to one change, and CPU 0's priority is the one firmware state the step leaves alone rather than relies on: CPU 0's PPI already fires today. A's change is just as inert, but it is a second register difference on CPU 0 that boot check 3 would have to excuse, and it rewrites firmware state that CPU 0 already runs correctly on. The case is not expected to arise (row 5's note on edk2's default priority).

**Q14. How is an arm-B failure in a newly live self-test attributed?** (Answered before T8 runs the pair, and written into "What gates".) The fix makes threads run that no logged boot has run: the echo IPC pair (#240), cap-denied, thread A, and the service echo pair with the echo client's `process_exit(pid 7)` ("Facts checked"). A fatal boot whose first report lies in one of them, or in the IPC or process-exit path they drive, carries none of H1, H3, N2, N4 or N5's signatures, so under Q2 A it reads "unattributed" and fails the step.
- A. Attribute it as exposure: such a boot is reported as "newly live self-test", with its first fatal line and the thread it names, and each distinct signature gets its own issue. It does not fail Q2 A. A self-test WARN that is not fatal (for example `Caller: ipc_call failed`) changes no class and is reported only in boot check 7's counts.
- B. Leave it unattributed: under Q2 A such a boot fails the step, and the step waits until the defect is fixed.
- Recommendation: A. A defect these paths show predates this step, which only lets them run, the same reasoning Q2 A applies to H1–N5. B would let the step's merge depend on IPC and process-lifecycle code that the step does not touch. The cost of A is that the step can merge while a newly found defect is open; the issue and the report keep it visible.

-----

## Issues Encountered

**T0. Live GIC state on `main` f3b039b (2026-10-11 07:17, load 16.8).** One monitor boot of the arm-A worktree `.claude/worktrees/crash-200-base` (f3b039b, built with `just disk create-data-disk`), read after the third `src=hb` line (`tick=3001,0,0,0` area; boot printed `4 CPUs online` and the four usual `[smp]` lines). Output: `target/dig/200/gic-mon-f3b039b/mon.txt` (gitignored).

- QEMU 11.1.2 (Homebrew `qemu 11.1.2`, `/opt/homebrew/bin/qemu-system-aarch64`).
- Firmware `/opt/homebrew/share/qemu/edk2-aarch64-code.fd` (the `just run` default, `AIOS_EDK2_FW` unset), sha256 `47765fe344818cbc464b1c14ae658fb4b854f5c2ceffa982411731eb4865594d`, banner `edk2-stable202408-prebuilt.qemu.org built at 16:28:50 on Sep 12 2024`.
- CI (latest `ci.yml` soak job, run 38096772215 on `main` f3b039b, 2026-10-10): runner image `ubuntu-26.04` 20260927.149.1, QEMU 10.2.1 (Debian `1:10.2.1+ds-1ubuntu3.2`), edk2 package `qemu-efi-aarch64` 2025.11-3ubuntu7.3, firmware `/usr/share/qemu-efi-aarch64/QEMU_EFI.fd`. CI boots no monitor, so its GIC state is not read; T3's `[smp]` fields in CI logs are the first reading.

| CPU | `GICR_CTLR` | `GICR_WAKER` | `IGROUPR0` | `IGRPMODR0` | `ISENABLER0` | `IPRIORITYR7` (INTID 30 byte) | `ICFGR1` (bit 29) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 0 | 0x00000002 | 0x00000000 | 0xffffffff | 0x00000000 | 0x40000000 | 0x80808080 (0x80) | 0x00000000 (0, level) |
| 1 | 0x00000002 | 0x00000000 | 0x00000000 | 0x00000000 | 0x40000000 | 0x00000000 (0x00) | 0x00000000 (0, level) |
| 2 | 0x00000002 | 0x00000000 | 0x00000000 | 0x00000000 | 0x40000000 | 0x00000000 (0x00) | 0x00000000 (0, level) |
| 3 | 0x00000002 | 0x00000000 | 0x00000000 | 0x00000000 | 0x40000000 | 0x00000000 (0x00) | 0x00000000 (0, level) |

- `GICD_CTLR` = 0x00000052: `DS` (bit 6) = 1, `ARE` (bit 4) = 1, `EnableGrp1` (bit 1) = 1, `EnableGrp0` (bit 0) = 0.
- `GICR_CTLR` = 0x2 on every CPU: `RWP` (bit 3) = 0 and `EnableLPIs` (bit 0) = 0; bit 1 is `CES` (Clear Enable Supported) by IHI 0069 as remembered, not re-checked (T1 checks it if `shared::gic` names it).
- `GICR_WAKER` = 0 on every CPU: `ProcessorSleep` = 0 and `ChildrenAsleep` = 0, so every redistributor is awake.
- `ISENABLER0` bit 30 is already 1 on CPUs 1–3: the kernel's enable write lands, and only the group is wrong.
- Against "Reported": all match. CPUs 1–3 `IGROUPR0` = 0 and CPU 0 = 0xffffffff; `DS` = 1; PPI 30 level on every CPU; `IGRPMODR0` = 0; CPUs 1–3 priority bytes 0 (QEMU reset). CPU 0's word 0x80808080 confirms the remembered edk2 default priority 0x80 (`ARM_GIC_DEFAULT_PRIORITY`) for INTIDs 28–31. No mismatch.
- CI runs a newer edk2 (2025.11) than the local one (stable202408), so CI's CPU 0 byte is not read. Writing 0x80 on CPU 0 there changes nothing if that edk2 keeps the 0x80 default; if it does not, the readback still passes (it checks the byte as written) and only CPU 0's byte-identity in CI is unverified. Boot check 3 runs locally, where it is verified.

- T1 review, round 1 (2026-10-11): `shared/src/gic.rs`'s module doc wrote bit ranges as `[30:3]`, `[7:4]`, `[7:3]` and `[30:5]`, which rustdoc reads as intra-doc links: `cargo doc -p shared --no-deps` gave 4 unresolved-link warnings (0 on `main`). Fixed by escaping the brackets; the command now prints none. No gate runs `cargo doc`, so `just check` did not catch it.

- T2 boot (2026-10-11 08:01, load 15.88 at start, 18.55 at end): `just soak runs=1 secs=75 report_only=1` on the T2 working tree (`b4ea05e-dirty`, kernel ELF sha256 `7a7b28bf8284bedb`), `target/soak/20261011-080148-text`: CLEAN, Gate 1 passed. Boot check 1: the first `src=hb` line reads `tick=1,3,3,3`, and in all 51 later ones each of the four values is above 0 and above its value in the line before (last `tick=51001,51028,51036,51117`). Boot check 4: last line `irqsw=4390,899,907,0` and `irqsw0=1,1,1,1`, so CPUs 1 and 2 switch on the IRQ path and CPU 3 does not, as the prediction table expects. Recorded, not judged: `pcphys=1804`, `elrmm=1,0,0,0`; no `PANIC`, `EXCEPTION`, `lock re-entry:` or `[tripwire-ev]` line. Newly live self-tests already print (`Thread A on core 2`, `Server: started, channel=0`, `Caller: got 'REPLY:PING'`); T5 counts them. The `[smp]` lines are unchanged until T3.

- T3 boots (2026-10-11): `just soak runs=1 secs=75 report_only=1` on the T3 working tree (`76b3c16-dirty`, kernel ELF sha256 `1b548a264cfcb0b2`), twice. Boot check 2 passes in both: four `[smp]` lines, each `ppi=30 ppi_grp1=1 ppi_en=1 ppi_ok=1`, CPUs 1–3 `vbar_kva=0 ttbr0_idmap=1`, CPU 0 `vbar_kva=1 ttbr0_idmap=0`, no `setup failed` line.
  - `target/soak/20261011-082615-text` (load 10.21 at start): PCZERO. `EXCEPTION[CPU 0]` EC 0x21, ELR 0, `ctx: cpu=0 tid=18 irq=0`, right after the echo client's `process_exit(pid 7)` on CPU 2; `src=exc` line `tick=574,588,587,587 irqsw=46,10,10,0 elrmm=1,0,0,0 spsrmm=1,0,0,0 pcphys=19`. H1's signature on CPU 0 (`elrmm` = 1, as in `pr239-20261010-115625-text/run-01`), a class B1 already has; not a new signature. Recorded for T5 and Q2/Q14 attribution, not judged here.
  - `target/soak/20261011-082928-text` (load 23.46 at start): CLEAN. Last line `tick=44001,44008,44012,43999 irqsw=3806,775,778,0 irqsw0=1,1,1,1 elrmm=1,0,0,0 pcphys=1551`; no `PANIC`, `EXCEPTION`, `lock re-entry:` or `[tripwire-ev]` line.
- T3 fault injection (2026-10-11, load 8.12 at start): a temporary local edit in `setup_timer_ppi` skipped the `GICR_IGROUPR0` write when `rd_base` = 0x080E_0000 (CPU 2's RD_base); kernel ELF sha256 `5433860e44095e30`, `target/soak/20261011-082748-text`, CLEAN. CPU 2's line reads `ppi=30 ppi_grp1=0 ppi_en=1 ppi_ok=0`, CPUs 0, 1 and 3 read `ppi_grp1=1 … ppi_ok=1`, and the one error line is `[2] ERROR Smp   cpu 2 timer PPI 30 setup failed: group1`. `tick[2]` is 0 in all 45 heartbeat lines (first `tick=1,3,0,2`, last `tick=44001,44012,0,44008`, `irqsw=3800,766,0,0`, `irqsw0=1,1,0,1`): the PPI stays in Group 0, which no CPU enables, and the CPU runs on as Q5 A describes. The edit was reverted before the commit; the rebuilt ELF's sha256 is `1b548a264cfcb0b2` again, identical to the boots above.

**T5. Development boots and the register check (2026-10-11, ab06b40, kernel ELF sha256 `1b548a264cfcb0b2`, the same ELF as T3's).** `just soak runs=3 secs=75 report_only=1`, `target/soak/20261011-083930-text`, load 9.97 at start, 7.34 at end, per-boot 1-min mean 8.85 (9.97, 8.57, 8.00): run-01 PCZERO, run-02 PCZERO, run-03 CLEAN. Then one monitor boot at load 7.1 (`target/dig/200/gic-mon-ab06b40`). Scratch output in `target/dig/200/t5/`.

- Boot check 1 (per-CPU ticks): 45 `src=hb` lines per boot; first lines `tick=1,3,3,3`, `1,3,3,3`, `1,2,2,2`. run-03: all four values above 0 and growing in all 44 later lines (last `tick=44001,44023,44018,44014`). run-01 and run-02: the same until the CPU that takes the PCZERO halts in the exception handler; from then on its `tick` is frozen (run-01 CPU 1 at 31930 from line 34, 12 lines; run-02 CPU 2 at 16128 from line 19, 27 lines) and the other three keep growing. The PPI setup took on every CPU in every boot; the frozen values are a halted CPU, not a CPU that never ticked (its `[smp]` line reads `ppi_ok=1`, and `tick` grew for 16 000 to 32 000 ticks first). Pass, with that reading.
- Boot check 2 (`[smp]` lines): pass in all three boots and the monitor boot: four lines, each `ppi=30 ppi_grp1=1 ppi_en=1 ppi_ok=1`; CPUs 1–3 `vbar_kva=0 ttbr0_idmap=1`, CPU 0 `vbar_kva=1 ttbr0_idmap=0`; no `setup failed` line.
- Boot check 3 (monitor, T0's block run unchanged, read after the second `src=hb` line, `mon.txt` 30 lines, QEMU 11.1.2): pass. `diff` against T0's `mon.txt` shows exactly six lines, all on CPUs 1–3: `IGROUPR0` 0x00000000 → 0xffffffff and `IPRIORITYR7` 0x00000000 → 0x00800000 (INTID 30's byte 0x80) on each. CPUs 1–3 now match CPU 0 in `IGROUPR0` (0xffffffff), `ISENABLER0` bit 30 (0x40000000), the INTID 30 priority byte (0x80), `ICFGR1` (0, level) and `IGRPMODR0` (0); `GICR_CTLR` 0x2 and `GICR_WAKER` 0 everywhere, as on `main`. CPU 0's seven registers are byte-identical to T0's (0x80808080 included). `GICD_CTLR` = 0x00000052, unchanged: `DS` = 1. The other three bytes of CPUs 1–3's `IPRIORITYR7` (INTIDs 28, 29, 31) stay 0, against 0x80 on CPU 0: the step writes only the timer PPI's byte (row 5), so this is expected, not a mismatch.
- Boot check 4 (`irqsw`): all three boots complete Gate 1. Last lines: run-01 `irqsw=3783,536,775,0`, run-02 `3786,771,281,2`, run-03 `3781,770,758,0`, so CPUs 1–3 sum to 1311, 1054 and 1528, all on CPUs 1–2 except run-02's 2 on CPU 3. `irqsw0=1,1,1,1` in every boot. Pass, as predicted.
- Boot check 5 (hazard lines): run-03 (CLEAN) has no `PANIC`, `EXCEPTION`, `lock re-entry:` or `[tripwire-ev]` line. Recorded, not judged: run-03 `pcphys=1526`, `elrmm=1,0,0,0`, `n4`, `nest`, `insched` absent (0); `scanstall` 0. No boot prints a `holder=` line (no `[tripwire-ev]` or `lock re-entry:` at all), so the `holder=` half of the check is read in arm B's logs at T8, as the check says.
- The two PCZERO boots (both carry H1's signature on a secondary, the prediction table's "first fatal reports on CPUs 1–3" row; not N5, which is CPU 0 with ELR or FAR in [0x4000_0000, 0x8000_0000); no new signature):
  - run-01: `EXCEPTION[CPU 1]` EC 0x21, ELR 0, FAR 0, `ctx: cpu=1 tid=5 irq=0 sched=0`, `spsr=0x80000345`, at about 55.67 s. tid 5 is test thread B: the balancer moved it CPU 1 → CPU 3 at 53.44 s and CPU 3 → CPU 1 at 55.674 s, a line interleaved with the exception report. `elrmm` goes from `1,0,0,0` (hb t=31001) to `1,1,0,0` in the `src=exc` line (t=31915; `pcphys=1096`, `irqsw=2737,536,562,0`).
  - run-02: `EXCEPTION[CPU 2]` EC 0x21, ELR 0, FAR 0, `ctx: cpu=2 tid=7 irq=0 sched=0`, at about 31.0 s. tid 7 is test thread D (it starts on CPU 3, where `irqsw[3]` = 2 shows it was switched out on the IRQ path); the balancer's `migrated tid=7 from CPU 3 to CPU 2` line is interleaved with the `src=exc` line. `elrmm` goes from `1,0,0,0` (t=16001) to `1,0,1,0` (t=16116; `pcphys=564`).
  - Both are a test thread that has just been migrated onto the faulting CPU and resumes there at PC 0, with that CPU's `elrmm` going 0 → 1 at the fault. Neither lies in a newly live self-test or the `process_exit` path (B and D ran before the step too), so Q14 does not apply. CPU 0 keeps ticking and both boots run their 75 s (`scanstall` 12 and 23 after the halt, as the prediction table's `scanstall` row expects). For T8's attribution: `elrmm` on a secondary alone does not mark a fault, because the monitor boot read `elrmm=1,1,1,0` at t=1001 with no exception.
  - With T3's two boots of the same ELF, that is 3 PCZERO (CPU 0 tid 18, CPU 1 tid 5, CPU 2 tid 7) and 2 CLEAN in 5 full boots of `1b548a264cfcb0b2` (T2's one CLEAN boot was an earlier ELF). Five boots say nothing about the rate; the A/B decides what it means.
- Boot check 6 (gates): `just check` exit 0, no warning or error line; `just test` exit 0 (712 passed, 0 failed), no warning; `cargo build --target aarch64-unknown-none` exit 0, no warning. `llvm-objdump -h` from the toolchain sysroot (`$(rustc --print sysroot)/lib/rustlib/aarch64-apple-darwin/bin/llvm-objdump`, nightly-2026-10-10) on `target/aarch64-unknown-none/debug/kernel`: `.text.boot` VMA 0xffff000000080000 / LMA 0x40080000, `.text.vectors` 0x…080800, `.text.rvectors` 0x…081000, `.text` 0x…081784; `.rodata` 0x…0e0000, `.data` 0x…121000, `.bss` 0x…276000, `.stack` (0x20000) 0x…2ba110, LMA = VMA − `VIRT_PHYS_OFFSET` throughout. Against arm A's ELF (`crash-200-base`, f3b039b) the first four are identical and `.rodata` to `.stack` sit 0x1000–0x2000 higher, page-aligned, from the larger `.text`: the usual layout. Each boot prints `EL: 1`, `Core ID: 0` and `4 CPUs online`.
- Boot check 7 (newly live self-tests), per boot, the same in all three: `Thread A on core` 5, `Server: started` (pattern `(^|[^-])Server: started, channel`) 1, `Caller: got` 5, `Caller: ipc_call failed` 0, `unauthorized ChannelCreate -> EPERM` 1, `Echo server: started` 1, `Echo client: got` 1, `Echo client: ipc_call failed` 0, `Echo client: EPIPE after service death` 1; the echo client's `process_exit` for pid 7 runs in every boot. No `PANIC` or `EXCEPTION` lies in these threads or in `process_exit`. `Timeout test: unexpected result -6` still prints once per boot (unchanged, as predicted).
- Gate 1's IPC line reads `FAIL` in run-01 and run-02 and `PASS` in run-03 (8 us); the 1b lessons record the IPC FAIL as expected since the lock landed, and the classifier does not count it.

**Plan review, round 1 (2026-10-10).** Four should-fix findings; each was checked against the code, the logs and the specs before the plan changed.

1. *`DS` = 0 makes the group readback a false failure, and CI is not a `DS` = 0 platform.* Confirmed: every QEMU command line in the repo omits `secure=on` (Facts checked), and QEMU's `DS` follows `secure=`, not the firmware. Fixed as proposed: CPU 0 reads `GICD_CTLR.DS` and stores it before `CPU_ON`; under `DS` = 0 the group flag is `Unchecked`; the risk, Reporting, Q5 and the host test changed with it. Two departures from the proposed fix:
   - The modifier (`IGRPMODR0`) check is dropped rather than reported `unchecked`. From Non-secure EL1 the register reads 0 under both values of `DS`, so the check could never fail. Its job under `DS` = 0 (catch a PPI the firmware left in a Secure group) is done by the priority readback, which reads 0 for a Secure interrupt. That only works if a written priority can never read back as 0 or come back changed, so row 5 now requires a non-zero priority with its low four bits clear, and T1 checks the priority-view rules against IHI 0069.
   - No new `[smp]` field for `DS`: `ppi_grp1=-` shows it.
2. *The guard's power is quoted for 30 boots per arm, but each mode is its own test.* Confirmed: step 1a's D4 computes one test per run, with no pooling across runs. The power figures were recomputed independently (exact one-sided Fisher, α = 0.05, by enumeration) and match the finding: 0.67, 0.46, 0.35, 0.49 and 0.01; 1 of 10 against 0 of 10 gives p = 0.50. Fixed: "What gates" item 2 carries the table, the two guard risks were split, Q2 states the real sensitivity, and the choice between per-mode and pooled gating is the new Q12. The pooled option's baseline is 40%, not the 60% the ADR's figure uses, so Q12 B gives its own figure (0.42).
3. *The N5 signature is narrower than the ADR's.* Confirmed: the ADR's N5 row (ADR `:101`) predicts instruction or data aborts and checked "ELR or FAR in 0x4000_0000–0x7FFF_FFFF". `kva_of` (`irq_spin_lock.rs:437-453`) adds `VIRT_PHYS_OFFSET` only below `KERNEL_BASE`, which never happens on CPU 0. All 2,323 `tick=` lines under `target/soak` read 0 for CPUs 1–3. Fixed: the prediction row, the risk, Q2 A and the report use the ADR's signature; T7's review list and boot check 5 cover the IRQ-path publishes that go live on CPUs 1–3.
4. *Boot check 3's monitor read is not runnable as written.* Confirmed in substance: no recipe takes extra arguments or passes `-monitor`, and no doc records step 1b's monitor command. One nuance: `just run`'s `-nographic` already multiplexes the monitor onto stdio (Ctrl-A C), but only interactively, which a verifier agent cannot script reliably. Fixed: T0 carries the full command (a monitor socket and an `xp` list), and boot check 3 runs it unchanged. No `justfile` recipe was added, to keep the step's diff to the kernel, `shared/` and docs.

**Plan review, round 2 (2026-10-10).** Three should-fix findings; each was checked against the code and the logs before the plan changed.

1. *The mechanical fix gate fails on a boot that dies before the scheduler starts.* Confirmed: CPU 0 unmasks at `main.rs:380`, after `sched::start` (`:371`); the secondaries unmask in `enter_scheduler` only once `SCHED_READY` is set (`scheduler.rs:55-63`, set with a `sev` at `sched/init.rs:108-112`); and `print_panic_report` prints a full `src=panic` line on any panic (`observability/tripwire.rs:353-375`), as `print_exception_ctx` does with `src=exc`. Fixed as proposed: "What gates" item 1 and Q1 A are scoped to boots whose last complete tripwire line has `tick[0]` > 0, and the report lists the boots outside that scope per arm. One residual case is left in scope on purpose: a secondary that halts before its own unmask while CPU 0 ticks would read `tick` 0 and fail the gate. That has never been seen, it is a fatal boot that Q2's attribution reads anyway, and its `[smp]` line (`ppi_ok`) tells "the fix did not take" from "the CPU died first".
2. *Row 5's "if edk2 left 0, use 0xA0" contradicts boot check 3's byte-identical rule.* Confirmed. Fixed by routing: any CPU 0 byte that breaks row 5's rules, 0 included, is recorded and goes to the owner as the new Q13 before T1, and the step writes no substitute on its own. Boot check 3 and the risk "CPU 0's GIC state changes by accident" now say that only a Q13 ruling can name a CPU 0 change. edk2's default priority (0x80) is quoted from memory and marked unchecked; T0's reading decides.
3. *"Every PCZERO boot has `elrmm` = 2 on CPU 0" is false.* Confirmed, with a different count from the finding's: `target/soak` holds 9 PCZERO boots (EC 0x21, ELR 0) with a `src=exc` line, not 4. The other five are `fix217-final-217/run-01`, `fix217-soak10-217/run-02` and `run-07`, `pr238-20261009-212720-text/run-01` and `pr238-20261009-220609-text/run-01`. Eight read `elrmm=2,0,0,0` and one, `pr239-20261010-115625-text/run-01`, reads `elrmm=1,0,0,0`. The row now says so; its prediction does not depend on it.

**Plan review, round 3 (2026-10-10).** One should-fix finding, checked against the code, the logs and #240 before the plan changed.

1. *The `Server: started` row says #200 does not explain #240; the logs say it does.* Confirmed. The thread-table order is as the finding gives it (`allocate_thread` first-free, `sched/mod.rs:237-247`; creation order in `sched/init.rs` and `ipc/tests/mod.rs`), and `Server: started` is printed at `ipc/tests/mod.rs:350`, before any IPC. Re-counted over the same logs: B1 text 20 and gpu 10, plus `pr239-20261010-115625-text` 2. In 32 of 32, tid 8's first migration is CPU 0 → CPU 1, after B's `iter=4` line, and tid 4 (0 → 1) and tid 13 (0 → 2) migrate in every boot; 0 of 32 print `Server: started` (with #240's pattern, which excludes `PI-Server`). Fixed as proposed: the row now expects `Server: started` and `Caller: got` > 0 in arm B, Q10's rationale names #200 as the likely cause and the step as resolving #240 as a side effect of its one change, the #240 comment in "Docs this makes stale" says so, and the Approach states it. Beyond the finding:
   - The same logs show more self-tests that never run: thread A (tid 4, no `Thread A` line in 32 of 32) and the service echo pair. echo-server (tid 14) never prints, and echo-client stops after `found service` in 32 of 32, although it goes on to `process_exit(pid 7)` even when its call fails. Both get prediction rows and boot check 7, so the process-exit path is expected to run in arm B too.
   - `badchan` is not expected to move with them: these tests pass valid channel ids, and the out-of-range ids come from ipc-timeout's tests, which already run in 32 of 32 (`Bad-id test`). The table says so instead of listing it as moving.
   - `ubmove[to]` was predicted "higher". Its 2 per CLEAN boot are most likely the 5 s timeouts of ipc-caller and echo-client, which the fix may replace with replies, so the row now reads "either way" (not traced per thread).
   - Newly live paths can fail in arm B with no H1–N5 signature, which Q2 A would read as "unattributed". That is a new owner question, Q14, wired into "What gates", Q2 A, the report, T8 and the dependencies, with a risk entry.
   - In gpu boots tid 8 later moves between CPUs 0, 1 and 3, and the compositor EXCEPTION censors most of them, so the gpu prediction is weaker; the row says so.
   - Coordination: per the owner's relayed instruction, the lead tells team-build that #240 is handled by this step when it is time, so team-build stops its own #240 work and restarts it after this step completes. Recorded under "Dependencies & Risks"; the message is the lead's to send, and this revision did not send it.

## Decisions Made

- Owner, 2026-10-10 16:29: every recommendation in "Open questions" taken (Q1 A, Q2 A, Q3 A, Q4 B, Q5 A, Q6 A with B as fallback, Q7 A, Q8 A, Q9 A, Q10 A, Q11 A, Q12 A, Q13 B, Q14 A).
- Merge (2026-10-11): the branch merged `main` f3b039b (#237 toolchain, #242 settings, #245 serde_json, #246 step 1a), a merge commit after the earlier d88002d. The new merge base f3b039b is arm A. Its `kernel/` and `shared/` sources are identical to 353f619's; only `rust-toolchain.toml` moved, to nightly-2026-10-10 (`rustc 1.101.0-nightly (32dba69d6 2026-10-09)` in this worktree). 353f619 pins nightly-2026-10-09, which 1a's preflight refuses next to this branch. T0 therefore reads the GIC state from a checkout of f3b039b instead of 353f619: GIC state is firmware state, and the kernel source is the same.
- Merge, Q7 A (facts only, nothing decided): step 1b's pair, af59149 against e211d6d, pins nightly-2026-10-09 in both arms. Both contain 7167d40, so 1a's preflight accepts them as a pair on their own. Next to a current arm (f3b039b or this branch, nightly-2026-10-10) the channel check refuses them unless `--allow-mixed-toolchains`, which would make it a toolchain-change soak; that matters only for a three-arm run like Q7 C. 1a's lessons note §8 records the same check refusing af59149 against 1a's own branch.
- T0, row 5: CPU 0's INTID-30 priority byte reads 0x80, which is non-zero, below `ICC_PMR_EL1` (0xFF) and has its low four bits 0. Row 5's rules hold, so Q13 B does not apply: **priority = 0x80 on every CPU**, CPU 0 included (a write of the value it already holds), and the priority flag is checked on every CPU. T1's `PpiConfig` tests and T2's setup use 0x80; CPU 0's registers stay byte-identical for boot check 3.
- T0, monitor command: the plan's `nc -U -w 2` pipe works on macOS, but the monitor echoes typed characters with cursor escapes and ends lines in CR, so the T0 block now pipes the replies through `tr -d '\r'` and a `grep` that keeps the 29 `xp` value lines and the version line. Boot check 3 runs that block unchanged.
- T1, source: the spec items were checked against the primary text, Arm IHI 0069H.b (GICv3.3/v4.2, ID041224, published 2024-04-16), fetched from developer.arm.com (documentation-service, `IHI0069H_b_gic_architecture_specification.pdf`) on 2026-10-11; text copy in the gitignored `target/dig/200/ihi0069hb.txt`. Sections: §12.10 (frames), §12.11.2 `GICR_CTLR` (RWP bit 3; bit 1 is `CES`, read-only, which confirms T0's note), §12.11.5 `ICENABLER0`, §12.11.8 `ICFGR1`, §12.11.12 `IGROUPR0`, §12.11.14 `IGRPMODR0`, §12.11.21 `IPRIORITYR`, §12.11.25 `ISENABLER0`, §12.11.42 `WAKER`, §12.9.4 `GICD_CTLR`, §4.8 (Table 4-9), §4.8.6, §4.8.7, §11.1. They are in `shared/src/gic.rs`'s module doc; hal.md §4.1 gets them in T6 (its row in "Docs this makes stale"). No Linux or QEMU source was needed.
- T1, confirmed as "Reported" states: `IGROUPR0` RAZ/WI to Non-secure under `DS` = 0; `IPRIORITYR` field of a Group 0 or Secure Group 1 interrupt RAZ/WI to Non-secure under `DS` = 0, and the Non-secure view `(v >> 1) | 0x80` stored, read back shifted left (§4.8.7); at least 4 priority bits with one Security state, 5 with two (§4.8, §4.8.7); `GICR_WAKER` RAZ/WI to Non-secure under `DS` = 0 (§12.11.42; the "from memory" bullet holds). `ISENABLER0` and `ICFGR1` bits of a Secure-group PPI are RAZ/WI too, so such a PPI also fails the enable flag (tested); its level flag passes.
- T1, corrections to "Reported": (a) `IGRPMODR0` under `DS` = 1 is RES0, which an implementation may make RAZ/WI, not RAZ/WI outright; row 4 still does not access it, so nothing changes. (b) In the Non-secure `GICD_CTLR` view under `DS` = 0, bit 6 is RES0, and the spec's RES0 definition says software "must not rely on the bit reading as 0". Linux's `gic_dist_security_disabled()` relies on the same read, and a misread fails safe: the group flag would be checked against a RAZ `IGROUPR0` and report `ppi_ok=0` (a visible false failure, never a false pass). Recorded on `security_disabled`; no design change.
- T1, deviation (row 1, WAKER): §12.11.42 says "Changing ProcessorSleep from 1 to 0 when ChildrenAsleep is not 1 results in UNPREDICTABLE behavior", so the plan's unconditional clear is wrong in the (`ProcessorSleep`, `ChildrenAsleep`) = (1, 0) state (redistributor entering sleep). Smallest correct change that keeps the intent (never skip or abandon a wake in progress): `waker_step(waker) -> WakerStep { Done, Wait, Write(u32) }` replaces `waker_wake_value` and `waker_awake`. T2 runs one bounded loop: read, then (0,0) done, (1,1) write the value with `ProcessorSleep` cleared and every other bit kept, (0,1) or (1,0) read again; the loop timing out sets `wake_ok` false. Row 1, the test list, the `GICR_WAKER` bullet under "Reported", T2's WAKER bullet and Q4 B's option text (annotated, the owner's wording kept) are amended in place (review round 2). On QEMU (`ChildrenAsleep` follows `ProcessorSleep` at once) and under `DS` = 0 (RAZ reads (0,0)) it behaves exactly like the unconditional clear.
- T1, deviation (row 5 priority bound): the plan's "below `ICC_PMR_EL1` (0xFF)" admits 0xF0, but `PMR` = 0xFF sets the lowest supported priority, which reads 0xF0 with 4 implemented bits, and "the GIC always masks an interrupt that has the lowest supported priority" (§4.8.6, Table 4-9). `PpiConfig::new` therefore rejects priorities at or above `PRIORITY_LIMIT` = 0xF0, so the accepted set is 0x10–0xE0 in steps of 16 (tested exhaustively). T0's 0x80 is in it.
- T1, Q13 B not built: T0 found CPU 0's byte 0x80, which meets every rule, so Q13 B does not apply and the priority flag is always checked. `PpiStatus`'s flags are a `Flag` enum with `Unchecked`, so an unchecked priority stays expressible if a later reading needs it, but no code path produces one (no-legacy rule: no unused mechanism). `TIMER_PPI_PRIORITY` = 0x80 lives in `shared::gic` for T2.
- T1, `PpiReadback` carries `wake_ok` and `rwp_ok` next to the register values, so `check` builds all six flags in one place.
- T1, docs: `docs/project/developer-guide.md` §5.4 test counts (and the gate table) updated (684 to 708, `gic` row 24) because `docs-check` flagged the count as new drift from this commit. `docs-check` also flags `.claude/CLAUDE.md` Workspace Layout not listing `shared/src/gic.rs`: that edit is the lead's (`.claude/` is protected), with T6's CLAUDE.md change.
- T2, statics in `gic.rs`, not `smp.rs`: `TIMER_PPI_INTID` (`AtomicU32`) and `SECURITY_DISABLED` (`AtomicBool`) live next to their readers (`init_gicv3_secondary`, `irq_handler_el1`, the readback check), not next to `GICR_BASE`. Both are written on CPU 0 long before `CPU_ON`: `DS` in `init_gicv3` from the `GICD_CTLR` read it already made, the INTID in `enable_irq`'s PPI branch (step 5b, from `init_generic_timer`'s `dt.timer_ppi`). `bring_secondaries_online`'s `dsb sy` and `CPU_ON` order them for the secondaries, as for `GICR_BASE`. The INTID starts at 1023 (spurious, filtered before the compare), so the handler can never match it before CPU 0 sets it.
- T2, `init_gicv3_secondary` returns `Result<PpiStatus, GicError>`, not `PpiStatus`: a secondary rebuilds the `PpiConfig` from the published INTID and cannot panic, so a rejected configuration is returned as `Err` with the redistributor left untouched (the CPU interface is still enabled). It is unreachable in practice: CPU 0 validates the same INTID in `enable_irq` and panics (a broken DTB) before any secondary starts. T3 reports both cases.
- T2, one per-CPU function `setup_timer_ppi(rd_base, cfg, ds) -> PpiStatus` covers rows 1–7 (wake included) on both paths, with no formatting, UART or reachable panic path (`wrapping_add` on the addresses). Checked (review round 1) on the ELF that `just run` and the soak boot, `target/aarch64-unknown-none/debug/kernel` (dev profile, opt-level 1, debug assertions on): `setup_timer_ppi` makes five calls into `shared::gic` (`PpiConfig::enable_mask`, `waker_step`, `PpiConfig::priority_byte_offset`, `PpiConfig::priority`, `PpiConfig::check`; small const fns are not inlined across crates at opt-level 1) and one to `core::panicking::panic_nounwind_fmt`, core's `read_volatile`/`write_volatile` alignment precondition on `rd_base` (`tst x23, #0x3` at entry), which a 64 KiB-aligned GICR frame cannot trip. Two of the callees keep debug overflow checks (`enable_mask`: shift overflow; `check`: shift and subtract overflow), unreachable because `PpiConfig::new` admits only INTIDs 16–31 and its fields are private. In `secondary_main` T2 adds only the calls to `PpiConfig::new` (no call or panic in its body) and `setup_timer_ppi`; its other panic calls (`panic_const_add_overflow` from `PRINT_TURN`'s `core_id + 1`, `unwrap_failed` on the fmt path) are in arm A's debug ELF too. `init_gicv3` still wakes CPU 0's redistributor first through the same `wake_redistributor` loop and panics on its timeout; the setup's own wake then reads (0,0) once and is done. A secondary now runs wake, PPI setup, then the CPU interface (Linux's `gic_cpu_init` order) instead of wake, CPU interface, enable; IRQs are masked throughout either way. The CPU-interface writes (row 8, unchanged) moved into one `init_cpu_interface` used by both paths.
- T2, CPU 0's status is kept in `InterruptController` as a `Cell<Option<PpiStatus>>`, because `enable_irq` takes `&self` and the `Platform::init_timer` signature passes `&InterruptController`; `timer_ppi_status()` reads it for T3. The controller is a local of `kernel_main`, never shared, so the `Cell` makes it `!Sync` at no cost.
- T2, the IRQ handler's `match intid { 30 => … }` became `if intid == TIMER_PPI_INTID.load(Relaxed)`: one `ldr` of the static, checked in the disassembly; nothing else on the IRQ path changed.
- T2 review round 1: the "never prints or panics" wording on `wake_redistributor`, `setup_timer_ppi` and `init_gicv3_secondary` was false for the dev ELF (the panic calls above); the doc comments now say "no formatting, UART access or reachable panic path" and name the unreachable panics. Doc-only fix. The optional `#[inline]` on `shared::gic`'s const fns was not taken: it would not change the property (the alignment precondition and the overflow checks stay, and the cross-crate calls are plain `bl`s into kernel text, which the boot TTBR1 maps before the kmap install), and it is outside the step's one change.
- T3, `print_cpu_line(cpu, ppi: Result<PpiStatus, GicError>)` prints the four fields and, when `ppi_ok=0`, the `kerror!`, so CPU 0 and CPUs 1–3 report through one function. The `Err` case (the published INTID is not a valid PPI, setup not run; unreachable, because CPU 0 panics on the same INTID before `CPU_ON`) prints `ppi_grp1=- ppi_en=- ppi_ok=0` and `… setup failed: not run, <error>`. That widens the plan's `ppi_en=0|1` to `-` for this one case, the smallest honest reading: neither register was read. Documented in observability.md "SMP lines" and the `print_cpu_line` doc.
- T3, CPU 0 passes `ic.timer_ppi_status()` with an `expect`: it is `None` only if `init_timer` never enabled the timer PPI, a kernel bug, and the panic fires on CPU 0 at thread level before the scheduler starts. No new `GicError` variant for "not set up" (it is not a configuration error).
- T3, the field values and the failed-flag names are pure logic in `shared::gic` with host tests (4 new, 28 in all; Miri passes on them): `Flag::field` (`1`/`0`/`-`), `PpiStatus::flags` and `PpiStatus::failed_flags`, a `Display` that names the `Fail` flags in check order and never an `Unchecked` one. The kernel adds `gic::timer_ppi_intid()` (a thread-level getter of the published INTID) for the `ppi=` field. Nothing on the IRQ path changed. Developer guide §5.4 counts 708 to 712.
- T4, comments only. `TwoStrike`'s doc now names the general never-ticked case (a CPU that has not unmasked IRQs yet, or whose timer PPI setup failed, `ppi_ok=0`), with the 2026-09-28 owner decision kept; the test comment in `two_strike_gate_skips_cpus_that_never_ticked` describes its own machine (only CPU 0 ticks) instead of citing #200, and the test is unchanged. `selftest.rs` gives two reasons for the CPU 0 pin, both still true after the fix: the re-entry panic needs a tick on the holder's own CPU (a blocking `lock()` from another CPU's tick only counts contention), and `SETTLE_TICKS`/`HOLD_TICKS` are timed in `TICK_COUNT`, which only CPU 0 advances (`timer.rs`, `if cpu == 0`). A grep of `kernel/src` and `shared/src` for other "no timer IRQ" / "only CPU 0 ticks" comments found none outside `gic.rs` (T2's).
- T5, boot check 1's reading: the check ("each one grows between consecutive heartbeat lines") is about whether the PPI setup took, which makes a miss a bug in the step. A CPU that ticked, then halted in the exception handler (run-01 CPU 1, run-02 CPU 2), freezes its `tick` from the fault on; T5 reads that as a pass for the setup and records the halt under boot check 5's PCZERO bullets, where Q2 A's attribution reads it. The same reading suits T8's mechanical gate, which uses `tick[1..3]` > 0, not growth. No third boot was taken: the three boots answer every check, and neither PCZERO carries a signature the prediction table lacks.
- T6, crash-fix ADR: no PR number exists yet and the ADR's step notes name branches ("Step 1a delivered": "branch `claude/crash-fix-step-1a-soak-classes`"), so every new note says "step 200 (branch `claude/crash-fix-step-200-gic-groups`)", not "(#PR)". "Step 200" sits under "Steps" after step 1b (16 lines) and records the change and all fourteen rulings of 2026-10-10, Q13 B as conditional and not applied (T0's 0x80); the lower baseline Q2 A asks for is left to T8's results. A new review note "Step 200, 2026-10-11" heads "Review notes" with the intro pointer and the line shifts (16 below step 1b's entry, 17 below "Docs to update"), as the 2026-10-09 note did for its own. Beyond the table's row: the "#200" row of "Order and why" and the 2026-09-24 amendment's two step-8 parentheticals ("today CPUs 1–3 take none") gain a short dated note, because they make the same claim. The step 1a note's #240 context bullet and the 1a lessons note are left as history.
- T6, capability-lifetime ADR: dated notes in place at every "CPUs 1–3 take no timer IRQs today" claim (§7's exit safe point, off-CPU wait and interim rule; "Not covered"; "Risks"; "Decisions", Owner 2026-09-28), an intro pointer, and a review note "Crash-fix step 200, 2026-10-11" (placed before the "#217" note, as #217's was placed before "#200"), which also gives the crash-fix line shifts for this ADR's bare crash-fix `:NNN` citations. No design text changes: the ADR already scoped everything to "from #200's fix step".
- T6, other docs: observability.md §6.5's Counters paragraph now says every CPU sets up its own PPI and only a `ppi_ok=0` CPU reads `tick` 0, with `pcphys` described without "stays 0"; the Two strikes bullet names the never-ticked cases (not unmasked yet, or `ppi_ok=0`), as T4 did for `TwoStrike`. hal.md §4.1 gains "Per-CPU timer PPI setup (GICv3)" (the six steps, the published INTID and `DS`, the pre-kmap constraint, Q5 A, the IHI 0069H.b sections T1 checked) and its struct listing gains `timer_ppi_status` with the boot-CPU-only PPI branch of `enable_irq`. boot/kernel.md's secondary step 6 and step 8 name the timer PPI setup and its `[smp]` report; beyond the table, its Step 5b "INTID 30" sentence now says the INTID comes from the DTB. Phase 01 (`:170`) and phase 03 (`:144`) gain a dated note on the ticked box. The B1 research note's item 5 gets the dated correction; B2 is described (Q6 A, B fallback) without a link, because its note does not exist until T8.
- T6, `docs-check` (`cargo run -q -p aios-tools -- docs-check`): 2 new findings, both present before T6: this plan (`knowledge-hygiene`) and `.claude/CLAUDE.md` Workspace Layout not listing `shared/src/gic.rs` (`layout`, the lead's edit with the "Timer IRQs on CPUs 1-3" fact). `just check` exit 0 with no warning; `just test` exit 0, 712 passed.

## Lessons Learned

(to be filled during implementation)
