---
author: jl + claude
date: 2026-10-11
tags: [kernel, smp, platform, tooling]
status: final
---

# Lesson: Making every CPU take timer IRQs, and measuring a fix that exposes known defects (crash-fix step 200)

Crash-fix step 200 (#200) makes every CPU set up the timer PPI on its own redistributor: Group 1, priority, level check, enable, then a readback. Before it, CPUs 1–3 never took a timer IRQ. The change and the owner's rulings are in the [crash-fix ADR](../decisions/2026-09-22-jl-crash-fix-preemption-and-fp.md) ("Step 200" under "Steps"). The register sequence and the spec sections it was checked against are in [hal.md §4.1](../../kernel/hal.md) and `shared/src/gic.rs`. The A/B pair, the per-boot attribution and the observed predictions are in [the step's A/B research note](../research/2026-10-11-jl-crash-fix-200-ab.md). How #200 was found is in [step 1b's lessons §6](2026-10-10-jl-crash-fix-1b-lessons.md). The lessons below apply beyond step 200.

## 1. Firmware sets per-CPU GIC state on the boot CPU only, and the boot CPU's state belongs to firmware too

**What happened.** A QEMU monitor read on `main` (QEMU 11.1.2, Homebrew edk2 stable202408) gave the redistributor state before the step:

- CPU 0: `GICR_IGROUPR0` = 0xffffffff, and the `IPRIORITYR7` word was 0x80808080 (edk2's default priority 0x80);
- CPUs 1–3: `IGROUPR0` = 0 and priority 0, both QEMU reset values;
- all four CPUs: `GICR_ISENABLER0` bit 30 = 1. The kernel's enable write landed, and only the group was wrong.

CPU 0's PPI fired only because edk2 had put it in Group 1. A priority of 0 on CPUs 1–3 would also have broken the step's readback, because a register that reads as zero (RAZ) also returns 0, so a 0 readback cannot tell the two apart.

Two spec claims the plan had written from memory were wrong. The primary text (IHI 0069H.b) settled both:

- Clearing `GICR_WAKER.ProcessorSleep` while `ChildrenAsleep` is 0 is UNPREDICTABLE, so the wake has to be a bounded state machine (`waker_step`), not an unconditional write.
- "Below `ICC_PMR_EL1` = 0xFF" admits 0xF0. That is the lowest supported priority with 4 implemented bits, and the GIC always masks it.

**Why it matters.** A per-CPU setup that relies on firmware for any field works on the CPU that firmware touched and fails silently on the others. A readback of a value that a RAZ register also returns proves nothing.

**How to apply.**

- On every CPU, CPU 0 included, write every per-CPU field the kernel depends on (group, priority, enable) and read each one back. Check the trigger mode, which may not be programmable, instead of writing it.
- Choose values that make the readback meaningful: a non-zero priority with its low four bits clear (exact in every priority view), below the lowest supported priority. Under `GICD_CTLR.DS` = 0, check the group only through the priority readback, because there `IGROUPR0` is RAZ/WI to Non-secure writes.
- For a measured step, write the value the boot CPU already holds (here 0x80), so the boot CPU's registers stay byte-identical and the pair measures one change. Confirm it with a register diff between the arms (section 3).
- Check GIC behaviour against the primary spec before coding, not from memory. Record the revision and section numbers next to the constants.
- CI runs a different firmware (edk2 2025.11 on Ubuntu 26.04) whose boot-CPU state nobody has read. The `[smp]` readback fields in CI logs are the first reading.

## 2. A PPI in the wrong group is silent: the CPU never ticks, and nothing faults

**What happened.** The kernel enables Group 1 only (`ICC_IGRPEN1_EL1`, `GICD_CTLR.EnableGrp1`). The step's first `.claude/CLAUDE.md` fact and hal.md said a Group 0 PPI would be signalled as FIQ into the `b .` vectors. The first independent audit round corrected that: with Group 0 disabled, the PPI is never signalled at all. T3's fault injection showed it. A local build that skipped the `IGROUPR0` write on CPU 2 booted CLEAN. CPU 2's `[smp]` line read `ppi_grp1=0 ppi_ok=0`, and `tick[2]` stayed 0 in all 45 heartbeat lines while the other three CPUs ticked.

**Why it matters.** The fault leaves no class change, no exception and no hang, so a soak classifier cannot see it. #200 went unnoticed until step 1b added a per-CPU `tick` counter. A thread that was moved to a non-ticking CPU behind a thread that never yields simply never ran: #240's echo IPC pair, cap-denied, test thread A and the service echo pair printed nothing in 32 of 32 logged boots.

**How to apply.**

- When a CPU's `tick` column stays 0 while CPU 0's grows, check the PPI's group first, then its enable bit.
- Prove that a readback check reports the failure it exists for: inject the fault in a temporary build and confirm the field, the error line and the counter all show it on that CPU only. Do not commit the injected build.
- When one CPU starts taking IRQs, expect code to run that no logged boot has run. Predict which threads that is, from the balancer's migration lines, before the pair runs.

## 3. Scripting the QEMU monitor on macOS

**What happened.** No recipe exposes a scriptable monitor, and `-nographic`'s multiplexed monitor (Ctrl-A C) works only interactively. A unix-socket monitor driven by macOS `nc -U -w 2` works. Every reply arrives, but the monitor's readline echoes each typed character with cursor escapes, and every line ends in CR. Stripping the CRs and keeping only the `xp` value lines and the version line leaves a 30-line `mon.txt`, which two kernels can be diffed on.

```sh
D=<new empty dir under target/>
qemu-system-aarch64 <just run's arguments> -monitor unix:"$D/mon.sock",server,nowait \
  < /dev/null > "$D/serial.log" 2>&1 &
QPID=$!
# wait with an until-loop until: [ "$(grep -c 'src=hb' "$D/serial.log")" -ge 2 ]
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

Per CPU, the list is `GICR_CTLR`, `GICR_WAKER`, `IGROUPR0`, `IGRPMODR0`, `ISENABLER0`, `IPRIORITYR7` and `ICFGR1`, then `GICD_CTLR` and the QEMU version. Between `main` and the step's branch, the diff was exactly six lines: `IGROUPR0` and the INTID 30 priority byte on each of CPUs 1–3, with CPU 0 unchanged.

**How to apply.** Use the block above for any register check (`xp` reads physical addresses), changing only the address list. Kill only your own QEMU (`$QPID`). Diff the monitor outputs of the two arms so the expected change, and nothing else, shows up.

## 4. Check for a quiet host and balanced arm loads before trusting a pair

**What happened.**

- The development boots ran at load1 8 to 23.5 on 10 CPUs.
- In the step's own pair, the gpu arms averaged load1 23.8 and 26.4 (maximum about 59), and the text arms 14.5 and 14.6.
- Step 1b's gpu pair ran first (Q7 A), and both of its runs broke the 25% "redo the pair" rule: their arm load means were 36% apart, then 26% apart. The redo, run with the host unchanged, broke the rule again. Both runs read 2/20 against 2/20 CLEAN. The owner accepted them as they were, with no third run, and closed the quiet-host window.
- Part of the load on this Mac came from the OS rather than from other sessions. Until 2026-10-10 the checkout lived in the iCloud-synced `~/Documents`, where `fileproviderd` and Spotlight indexing loaded the host during soaks ([1a lessons §8](2026-10-10-jl-crash-fix-1a-lessons.md) saw the same) and iCloud left " 2" duplicate files inside `.git`. The repository now lives at `/Users/juslee/dev/aios`, outside iCloud.

**Why it matters.** Load inflates hangs and timings, so a pair whose arms ran under different load compares the host rather than the kernels. A redo under the same conditions only repeats the imbalance and spends another host-exclusive hour.

**How to apply.**

- Before a host-exclusive soak, read load1 and the top processes (other sessions' `cargo` and QEMU, `fileproviderd`, `mds_stores`), and coordinate with every other session ([1b lessons §5](2026-10-10-jl-crash-fix-1b-lessons.md)).
- Keep the checkout and its `target/` out of iCloud-synced folders.
- When the redo rule fires, find and remove the load source before re-running. If another run cannot change the decision (here, equal CLEAN counts), ask the owner to accept the run as it is, and record the ruling with the load figures.

## 5. A fix that exposes known defects needs attribution rules registered in advance, and still needs owner rulings

**What happened.** Giving CPUs 1–3 a tick made H1, H3, N4 and N5 reachable on three more CPUs, so the plan expected non-CLEAN boots to rise. Before the pair, the owner registered the gates in advance (Q1 A, Q2 A, Q12 A, Q14 A): a mechanical check that the fix took (`tick[1..3]` > 0), a per-mode regression guard, and a signature for every non-CLEAN boot of the new arm, with "unattributed" sending the step to the owner. The pair then produced:

- a new marker: PCZERO went from 0/20 to 5/20 in text (p = 0.0236), all with H1's signature, two of them first reported on CPUs 1 and 2. This is the same mechanism, newly reachable;
- two gpu boots with a bad-free PANIC that matched no registered signature, although arm A had the same PANIC at the same rate (2 of 10 each) and CPUs 1–3 had made no IRQ-path switch when it fired;
- one boot (text B run-08) that fitted two buckets: H1's corrupt register, and a first report naming a newly live self-test's thread;
- a check (`holder=` on CPUs 1–3) whose trigger occurred only once, with unset holder fields, so it was neither passed nor failed as written.

The pre-registered verdict was "not passed", and T8 stopped. The owner ruled the PANIC pre-existing exposure, the run-08 boot H1 only, and the `holder=` check met but not exercised.

**Why it matters.** Without signatures registered in advance, every exposed defect would have been argued after the fact. Even with them, three outcomes fell outside rules written before the data existed. The regression guard alone could not decide the step: at 20 text boots per arm its power was 0.35, and the gpu guard could not fail at all.

**How to apply.**

- For a step that turns on a code path, register in advance: the mechanical "it took" check, the guard and its real power per mode, and a signature for every known hypothesis.
- Also register three rules the step-200 rules lacked:
  - a "pre-existing in the old arm" bucket (same site, same CPU and thread, a rate not higher, the new path not involved);
  - which bucket wins when a boot fits two;
  - a "not exercised" outcome for a check whose trigger may not occur.
- When a boot still falls outside the rules, stop and send it to the owner with the evidence and a recommendation. Do not reinterpret the rule. Record the ruling in the research note and the ADR.
- Read every non-CLEAN boot of the new arm, whatever the guard's p-value. A low-power guard that passes is weak evidence that nothing regressed.

## 6. Audit independence and agent capabilities

**What happened.**

- T7's `/audit-loop` ran in a workflow agent. Workflow agents cannot spawn subagents, so one agent ran the doc, code and security audits itself: 4 rounds, 5 fixes, then 1, then two clean rounds.
- The lead then ran an independent loop with separate auditors. Its first three rounds found 3, 1 and 1 more should-fix findings, then a clean round. One finding was a factual error in `.claude/CLAUDE.md` and hal.md (the Group 0 FIQ claim, section 2). Another was a code comment pointing into the working plan, which would have dangled once the plan was deleted.
- Doc-writer agents have only Read, Write, Edit, Grep and Glob (`.claude/agents/doc-writer.md`), so they cannot run `docs-check`, `rg` or the gates on their own edits.

**Why it matters.** Two clean rounds from the agent that wrote and fixed the text are one reviewer's view twice, not two independent views. The single-agent clean signal missed five findings.

**How to apply.**

- Count a round as independent only when the doc, code and security audits run in separate agents that did not write the change. When the implementer runs in a workflow, the lead launches those auditors.
- Give documentation tasks that must pass `docs-check` or `just check` to an agent with Bash, or run the gates in the lead after a doc-writer's edit.
- Never point code comments at a working plan. Point them at the architecture doc that outlives the plan.
