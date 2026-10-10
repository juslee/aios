---
author: jl + claude
date: 2026-10-10
tags: [kernel, sched, ipc, smp]
status: final
---

# Research: Crash-fix step 1b, B1 single-arm N2 baseline

Task B1 of crash-fix step 1b: one baseline soak of the final 1b kernel with the tripwire counters, before #163/#185 converts the blocking paths (owner constraint, #185). It is data collection only. It is not the ADR's interleaved A/B acceptance, which waits for step 1a, and it draws no comparison with `main`. See the [crash-fix ADR](../decisions/2026-09-22-jl-crash-fix-preemption-and-fp.md) for the hypotheses (H1, H3, N2) and [observability.md §6.5](../../kernel/observability.md) for every key.

## Conditions

- **Kernel:** branch `claude/crash-fix-step-1b-tripwires` at c065b0c (kernel ELF sha256 prefix `d2496ca5bf1745a2`, as the soak reports it), toolchain nightly-2026-10-09.
- **Runs:** `just soak runs=20 secs=75 report_only=1` (text) then `just soak runs=10 secs=75 mode=gpu report_only=1`, 2026-10-10 09:43–10:21 (+07), fresh data disk per boot. QEMU 11.1.2, Homebrew edk2 firmware.
- **Host:** Apple Silicon Mac, 10 CPUs. No other QEMU, build or test ran (the other session held its work). The 1-minute load was 5.1 at the start, 6.5 after the text arm and 18.6 at the end; iCloud's `fileproviderd` was still finishing the removal of the repo's old `~/Documents` copy.
- **Extraction:** the parser contract of [observability.md §6.5](../../kernel/observability.md). Per boot, the last tripwire line (`src=hb|g1|panic|exc`) whose key count matches its `n=`; a missing key is 0; `ubrun`/`ubrbl` index 1 is `reply`; class from the soak's `summary.tsv`. Every boot had a valid line.

## Classes

| Mode | Boots | CLEAN | WEDGE | PCZERO | EXCEPTION | PANIC |
| --- | --- | --- | --- | --- | --- | --- |
| text | 20 | 11 (55%, Wilson 95% 34–74%) | 3 | 1 | 2 | 3 |
| gpu | 10 | 1 (10%, 2–40%) | 0 | 0 | 9 | 0 |

## Counters by class

Cells are "boots with a non-zero value / sum over those boots". `lk*` columns are THREAD_TABLE (TT), CURRENT_THREAD (CT) and WAKEUP_ERRORS (WE). Every `lkph` and `lkself` cell and every `orphan`, `n1`, `scanstall`, `misrep` and `wakefl` cell was 0/0, so those columns are left out.

| Mode | Class | Boots | `ubrbl[reply]` | `n2` rblk | `nowaker` | `latereply` | `ctbusy` | `elrmm` | `irqsw` | `starved[Normal]` |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| text | CLEAN | 11 | 0/0 | 0/0 | 0/0 | 0/0 | 2/2 | 11/11 | 11/32810 | 11/99 |
| text | WEDGE | 3 | 3/3 | 3/3 | 3/3 | 0/0 | 0/0 | 0/0 | 3/52 | 3/27 |
| text | PCZERO | 1 | 0/0 | 0/0 | 0/0 | 0/0 | 1/1 | 1/2 | 1/540 | 1/9 |
| text | EXCEPTION | 2 | 1/1 | 0/0 | 0/0 | 1/1 | 1/1 | 2/2 | 2/36 | 0/0 |
| text | PANIC | 3 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 2/11 | 0/0 |
| gpu | CLEAN | 1 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 1/1 | 1/3100 | 1/9 |
| gpu | EXCEPTION | 9 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 9/9 | 9/9 | 0/0 |

`ubrun[reply]` and every other `n2` kind (rpre, vpre, vblk) were 0 in every boot. `scanhold1` peaked at 21,750–29,187 ticks and `twmax` at 147,063–871,250 ticks per class (owner-accepted K8 cost, #236).

## What the counters show

These are observations from one arm of 30 boots, not A/B results.

1. **Every WEDGE is an N2 reply lost wakeup.** Each of the 3 text WEDGE boots ends with exactly one `n2[rblk]`, one `ubrbl[reply]` and one `nowaker`; all 11 CLEAN boots have none of the three. The waiter was blocked in the reply phase, the reply's `unblock` found it not yet Blocked, and the scan then saw a blocked thread with no waker. This is the N2 mechanism the ADR predicted, now attributed by caller.
2. **Every PANIC is a PANIC-LOCK (H3).** The 3 text PANICs are `lock re-entry` at `irq-exit`: THREAD_TABLE twice (one holder `cap/mod.rs:39`) and CURRENT_THREAD[0] once. Before 1b these re-entries deadlocked or corrupted silently; the detect-only lock turns them into a named panic.
3. **EXCEPTIONs carry the H1 signature.**
   - gpu, 9 of 9: the same PC in `compositor_loop` (ELR `0xffff00000009d154`), a data abort at a near-null address (FAR 0x2–0xd), with `irqsw=1` and `elrmm=1`: each boot faults right after its first IRQ-path switch, which had an ELR mismatch.
   - text, 2 of 2: an alignment fault on `stlr w22, [x19, #0x808]` inside `IrqSpinLock<TIMEOUT_QUEUE>::lock_contended` (ELR `0xffff0000000c231c`, FAR 0x81a), so `x19` held 0x12 instead of the lock's address while `x26`, the lock word, was correct. A callee-saved register came back corrupted across a switch (`elrmm=1` in both): H1, with the lock's spin loop only as the place the thread was preempted.
4. **`elrmm=1` is common, not sufficient.** 11 of 11 text CLEAN boots also record one ELR mismatch, so a single mismatch does not by itself kill a boot; the gpu arm's mismatch on the very first IRQ-path switch does.
5. **Normal-class starvation in every CLEAN boot** (`starved[Normal]` 99 over 11 boots) is #200: CPUs 1–3 take no timer IRQ, so a Normal thread queued there waits for the running thread to yield.

## Data

The per-boot TSV (30 rows, the columns above plus `n2` rpre/vpre/vblk, `lkph`/`lkself` per lock, `scanhold1`, `twmax`):

```text
mode	class	log	src	ubrun_reply	ubrbl_reply	n2_rpre	n2_rblk	n2_vpre	n2_vblk	latereply	misrep	ctbusy	nowaker	wakefl	orphan	n1	scanstall	lkph_TT	lkph_CT	lkph_WE	lkself_TT	lkself_CT	lkself_WE	elrmm	irqsw	starved_normal	scanhold1	twmax
text	CLEAN	run-01.log	hb	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	1	3182	9	26750	189187
text	CLEAN	run-02.log	hb	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	1	3264	9	27375	185375
text	CLEAN	run-03.log	hb	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	1	3102	9	27437	190875
text	CLEAN	run-04.log	hb	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	1	2908	9	27063	226437
text	PANIC	run-05.log	panic	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	6	0	28375	142375
text	EXCEPTION	run-06.log	exc	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	1	19	0	28562	141437
text	PANIC	run-07.log	panic	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	5	0	28125	147063
text	EXCEPTION	run-08.log	exc	0	1	0	0	0	0	1	0	1	0	0	0	0	0	0	0	0	0	0	0	1	17	0	26437	147312
text	CLEAN	run-09.log	hb	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	1	2711	9	25625	183437
text	PCZERO	run-10.log	exc	0	0	0	0	0	0	0	0	1	0	0	0	0	0	0	0	0	0	0	0	2	540	9	29000	255812
text	PANIC	run-11.log	panic	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	29187	146875
text	WEDGE	run-12.log	hb	0	1	0	1	0	0	0	0	0	1	0	0	0	0	0	0	0	0	0	0	0	14	9	27188	142062
text	WEDGE	run-13.log	hb	0	1	0	1	0	0	0	0	0	1	0	0	0	0	0	0	0	0	0	0	0	21	9	26937	202875
text	CLEAN	run-14.log	hb	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	1	2793	9	26812	189000
text	CLEAN	run-15.log	hb	0	0	0	0	0	0	0	0	1	0	0	0	0	0	0	0	0	0	0	0	1	3234	9	26437	871250
text	CLEAN	run-16.log	hb	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	1	2936	9	26500	184812
text	CLEAN	run-17.log	hb	0	0	0	0	0	0	0	0	1	0	0	0	0	0	0	0	0	0	0	0	1	2858	9	26688	183750
text	CLEAN	run-18.log	hb	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	1	3064	9	20250	192750
text	CLEAN	run-19.log	hb	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	1	2758	9	26563	200937
text	WEDGE	run-20.log	hb	0	1	0	1	0	0	0	0	0	1	0	0	0	0	0	0	0	0	0	0	0	17	9	27125	153000
gpu	EXCEPTION	run-01.log	exc	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	1	1	0	26938	140312
gpu	EXCEPTION	run-02.log	exc	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	1	1	0	28000	157750
gpu	EXCEPTION	run-03.log	exc	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	1	1	0	26813	138063
gpu	EXCEPTION	run-04.log	exc	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	1	1	0	26938	143437
gpu	EXCEPTION	run-05.log	exc	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	1	1	0	0	137625
gpu	EXCEPTION	run-06.log	exc	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	1	1	0	28250	141500
gpu	EXCEPTION	run-07.log	exc	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	1	1	0	0	143000
gpu	CLEAN	run-08.log	hb	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	1	3100	9	21750	208750
gpu	EXCEPTION	run-09.log	exc	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	1	1	0	27812	139125
gpu	EXCEPTION	run-10.log	exc	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	0	1	1	0	0	133125
```
