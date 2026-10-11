---
author: jl + claude
date: 2026-10-11
tags: [kernel, smp, sched]
status: final
---

# Research: Crash-fix step 200, A/B pair results and attribution

Task T8 of crash-fix step 200 (#200): the interleaved A/B pair of `main` against the step's branch, read against the gates pre-registered on 2026-10-11 (owner answers Q1 A, Q2 A, Q12 A, Q14 A) and the step's prediction table. The step makes every CPU put its timer PPI in Group 1, so CPUs 1–3 take timer IRQs for the first time. See the [crash-fix ADR](../decisions/2026-09-22-jl-crash-fix-preemption-and-fp.md) for the hypotheses (H1, H3, N2, N4, N5), [observability.md §6.5](../../kernel/observability.md) for every key, and [B1's note](2026-10-10-jl-crash-fix-1b-n2-baseline.md) for the pre-step signatures. The gate text and the prediction table live in the step's working plan (`docs/knowledge/plans/2026-10-10-jl-crash-fix-200-gic-groups-plan.md`, "A/B acceptance"), which T9 distils and deletes; the parts this note applies are restated below.

**Verdict as pre-registered: the step does not pass yet.** Items 1–4 pass or are reported as the plan says. Item 5 leaves 2 arm-B boots unattributed (gpu runs 04 and 09, the bad-free PANIC), and the plan's rule is that "any unattributed signature in arm B fails the step, whatever the guard says, and goes to the owner". The same PANIC occurs in arm A at the same rate (gpu runs 04 and 06, 2 of 10 in each arm). The owner questions are at the end.

## Conditions

- **Arms:** A is `main` 030b3b7 (`/Users/juslee/dev/aios-030b3b7`, kernel ELF sha256 prefix `2b646f6de2ae54a7`); B is the branch tip 82caf24 (`.claude/worktrees/crash-200`, `690350af4860ed7f`). Both on nightly-2026-10-11, `rustc 1.101.0-nightly (6f5da406b 2026-10-10)`; every arm contains 7167d40.
- **Runs:** `just soak --arm A --arm B runs=20 secs=75` (text, 40 boots) and `… runs=10 secs=75 mode=gpu` (20 boots), 2026-10-11, harness 030b3b7, report-only by step 1a's D5, arm order rotated each round, fresh data disk per boot. QEMU 11.1.2 (sha256 `7299ef807db462f9`), Homebrew edk2 firmware (sha256 `47765fe344818cbc`). Host: Darwin 27.2.0 arm64, 10 CPUs. The harness warned that the parent `.cargo/config.toml` joins arm B's build.
- **Logs:** `target/soak/20261011-t8-200-text` and `target/soak/20261011-t8-200-gpu`, each with `summary.md`, `boots.tsv`, `arms.tsv` and `arm-A/`, `arm-B/` `run-NN.log`.
- **Extraction:** the parser contract of observability.md §6.5, as in B1: per boot, the last `[tripwire]` line whose key count matches its `n=`; a missing key is 0; CR stripped. An independent re-parse of all 60 logs matched the `tw_line` column of `boots.tsv` in 60 of 60 boots. Logs interleave lines from several CPUs, so a report's fields can be split across lines; the line numbers below are those of the `run-NN.log` after `tr -d '\r'`.

## Pair reports

| Mode | Arm | Boots | CLEAN | CLEAN rate (Wilson 95%) | Non-CLEAN classes | load1 mean / max |
| --- | --- | --- | --- | --- | --- | --- |
| text | A | 20 | 15 | 75% (53–89%) | PANIC-LOCK 2, EXCEPTION 1, WEDGE-ALIVE 1, DEGRADED 1 | 14.51 / 27.03 |
| text | B | 20 | 13 | 65% (43–82%) | PCZERO 5, EXCEPTION 2 | 14.62 / 31.00 |
| gpu | A | 10 | 1 | 10% (2–40%) | EXCEPTION 7, PANIC 2 | 23.83 / 59.34 |
| gpu | B | 10 | 0 | 0% (0–28%) | EXCEPTION 7, PANIC 2, PANIC-LOCK 1 | 26.42 / 58.93 |

- **Regression guard** (one-sided Fisher, fewer CLEAN in B): text 15/20 against 13/20, p = 0.366, passes; gpu 1/10 against 0/10, p = 0.5, passes (informational, Q12 A).
- **Markers:** text PCZERO 0/20 against 5/20, p (more in B) = 0.0236, two-sided 0.0471: `new`. No other marker in either mode; no `removed` marker. Text PANIC-LOCK 2 → 0 (p fewer in B 0.244), WEDGE-ALIVE 1 → 0 and DEGRADED 1 → 0 (0.5 each) are unmarked. gpu PANIC-LOCK 0 → 1 (p more in B 0.5) is unmarked.
- **Load:** text means 1% apart, gpu 11% apart; both comparable under the 25% redo rule. No INCONCLUSIVE boot in any cell.
- **Gate 1 IPC** (recorded, not gated): text A 7.93 us over 15 CLEAN boots, G1PASS 13 of 20; text B 9.15 us over 13, G1PASS 9 of 20. gpu A 11.00 us (1 boot), gpu B no CLEAN boot; G1PASS 0 of 10 in both.
- The full per-arm tripwire table is each `summary.md`'s "Tripwire per arm"; the rows the prediction table names are below.

## Mechanical fix check ("What gates" item 1)

Rule: in arm B, every boot whose last complete tripwire line has `tick[0]` > 0 also has `tick[1..3]` > 0 in that line; in arm A, every boot with a complete line has `tick[1..3]` = 0.

| Cell | Boots | In scope | Out of scope | `tick[1..3]` all > 0 | `tick[1..3]` all = 0 | Result |
| --- | --- | --- | --- | --- | --- | --- |
| text A | 20 | 20 | 0 | 0 | 20 | pass |
| text B | 20 | 20 | 0 | 20 | 0 | pass |
| gpu A | 10 | 10 | 0 | 0 | 10 | pass |
| gpu B | 10 | 10 | 0 | 10 | 0 | pass |

- **Out of scope:** none in any cell, so no boot is listed. Every boot has at least 2 complete tripwire lines. In text B run-08 the CPU 2 `src=exc` line is split by another CPU's output and fails the completeness test; its last complete line is CPU 0's `src=exc` (t=506, `tick=506,523,243,523`).
- **`[smp]` lines:** arm B prints `ppi=30 ppi_grp1=1 ppi_en=1 ppi_ok=1` on all four CPUs in all 30 boots (120 of 120 lines) and no `setup failed` line. Arm A prints four `[smp]` lines per boot without the fields, as expected for `main`.
- **Item 1 passes.** It asks only for `tick` > 0, so a CPU that ticked and then halted also passes: text B run-01 CPU 1 (11029 against 43001), run-08 CPU 2 (243 against 506), run-10 CPU 2 (417 against 7002), run-18 CPU 2 (42502 against 44001). In every CLEAN text-B boot `tick[1..3]` is within 0.1% of `tick[0]` (44002–44034 against 44001).

## Prediction table, observed

Means are over each arm's boots unless marked CLEAN. "n/s" is boots with a non-zero value / sum, as in `summary.md`. Text unless marked gpu.

| Key / class | Expected in B | Observed (2026-10-11) | Matches |
| --- | --- | --- | --- |
| `tick[1..3]` | close to `tick[0]` | CLEAN B means 44020, 44020, 44014 against 44001; > 0 in 30/30 B boots; 0 in 30/30 A boots | yes |
| `irqsw[1..3]` | > 0, mostly CPUs 1–2 | text B 20/13029, 20/13428, 2/4 (CLEAN means 804, 803, 0.15); gpu B 6/11, 5/678, 0/0 | yes |
| `irqsw0[1..3]` | about 1 each | 1, 1, 1 in 30/30 B boots; `irqsw0[0]` (no row) also 1 in 15/20 text and 8/10 gpu B boots | yes |
| `pcphys` | > 0, about the IRQ-path restores on CPUs 1–3 | text B 20/26422; CLEAN mean 1606, against CLEAN mean `irqsw[1]+[2]+[3]` 1608; gpu B 6/684 | yes |
| `elrmm`, `spsrmm` [1..3] | > 0 | 0 in all 13 CLEAN text B boots. Non-zero only in failing boots: text `elrmm[1]` 2/6, `[2]` 4/355 (352 of it in run-19, where CPU 2 ran on), `spsrmm[1]` 1/5, `[2]` 2/2; gpu `elrmm[1]` 6/6, `[2]` 1/1, `spsrmm[1]` 3/3. `elrmm[0]` stays at 1 in CLEAN boots of both arms | only in failing boots |
| `n4`, `nest`, `insched`, `n1` | can be > 0 | 0 in all 60 boots | not seen |
| `lb` | higher | A 20/121 (CLEAN mean 5.00), B 20/344 (CLEAN mean 20.69) | yes |
| `starved[Normal]` | lower, not 0 | A 17/153 (CLEAN mean 9.00), B 18/101 (CLEAN mean 6.08, range 3–8) | yes |
| `starved[Idle]` | 4 | A 17/68, B 18/72 (4 per boot) | yes |
| `ctbusy`, `xdir`, `xrep` | higher | `ctbusy` A 2/2, B 16/31; `xrep` A 0, B 18/18; `xdir` 0 in both | `xdir` no |
| `ubmove[to]` | either way | A 17/34 (2 per CLEAN boot), B 0 in all 20 | fell to 0 |
| `lkph`, `lkpho`, `lkphrun`, `kind=ph` | can be > 0 | 0 in every B boot; A `lkpho[TIMEOUT_QUEUE]` 2/2 (runs 01, 20) | not seen in B |
| `scanstall` | can be > 0 | B 2/33: run-01 27 (CPU 1 halted), run-10 6 (CPU 2 halted); 0 in CLEAN boots | yes |
| `irqsw[0]` per tick | may fall | rose: A 50420/765856 = 0.0658, B 65940/743785 = 0.0887 (CLEAN 0.0699 → 0.0886) | no |
| PCZERO, EXCEPTION | may rise, first reports on CPUs 1–3 | A 0 + 1, B 5 + 2 (PCZERO `new`, p = 0.0236); first reports CPU 1 ×1, CPU 2 ×3, CPU 0 ×3; all H1 | yes |
| N5 | possible | 0 in both arms and modes: every CPU 0 EXCEPTION has ELR 0 or a kernel VA and FAR 0 or a small value | not seen |
| PANIC-LOCK on CPUs 1–3 | possible | text 0; gpu 1 (B run-07, CPU 2, `irq-exit`) | yes, once |
| WEDGE-STUCK, WEDGE-ALIVE | either way | A 0 + 1 (run-04, N2), B 0 + 0 | down |
| gpu classes | about the same | A 1 CLEAN, 7 EXCEPTION, 2 PANIC; B 0 CLEAN, 7 EXCEPTION, 2 PANIC, 1 PANIC-LOCK | yes |
| `Timeout test: unexpected result -6` | unchanged | A 20/20 text, 10/10 gpu; B 20/20 text, 9/10 gpu (B run-07 died at t=2) | yes |
| `Server: started`, `Caller: got` (#240) | > 0 in most B boots | text B 20/20 boots each (96 `Caller: got` lines; run-19 has 1); A 0; gpu B 0 | yes (text) |
| `unauthorized ChannelCreate -> EPERM` | > 0 in most boots | text B 20/20; A 0; gpu 0 | yes (text) |
| `Thread A on core` | > 0 in most boots | text B 20/20 (98 lines; run-17 has 3); gpu B 1/10 (run-09); A 0 | yes (text) |
| Service echo pair | client lines in most boots | text B: `Echo server: started` 20/20, `Echo client: got` 19 (run-05: `ipc_call failed with -1`), `process_exit` of pid 7 20/20, `EPIPE after service death` 18 by strict grep (run-08 torn by interleaving, run-19 prints `error -12`); gpu 0 | yes (text) |
| IPC keys (`ubrun`, `ubrbl`, `ubdead`, `ubnone`, `n2`, `latereply`) | can rise | all 0 in 20/20 text B boots; A `ubrbl[reply]` 2/2, `n2[rblk]` 1/1, `latereply` 1/1 | no |
| `badchan` | unchanged | 3,2,1 in 19/20 text B boots (run-19 4,2,1) and 20/20 A | yes |
| `qbad` (no row) | — | A 0; B 2/37 (run-05 CLEAN 30, run-10 7) | not predicted |

In gpu mode CPU 0's first fatal report comes at t = 18–19 in 9 of 10 arm-B boots (run-07 ends at t = 2 on CPU 2), so most gpu counters are cut off there (6 of B's 10 last complete lines were printed by CPU 1 or CPU 2 after CPU 0's report).

## Newly live self-tests (boot check 7, Q10)

Boots with at least one match (total lines in brackets). `Server: started` is counted with `(^|[^-])Server: started, channel`, so `PI-Server` does not match.

| Line | text A | text B | gpu A | gpu B |
| --- | --- | --- | --- | --- |
| `Thread A on core` | 0 | 20 (98) | 0 | 1 (5) |
| `Server: started, channel` | 0 | 20 (20) | 0 | 0 |
| `Caller: got` | 0 | 20 (96) | 0 | 0 |
| `Caller: ipc_call failed` | 0 | 0 | 0 | 0 |
| `unauthorized ChannelCreate -> EPERM` | 0 | 20 (20) | 0 | 0 |
| `Echo server: started` | 0 | 20 (20) | 0 | 0 |
| `Echo client: got` | 0 | 19 (19) | 0 | 0 |
| `Echo client: ipc_call failed` | 0 | 1 (1) | 0 | 0 |
| `Echo client: EPIPE after service death` | 0 | 18 (18) | 0 | 0 |
| `Timeout test: unexpected result -6` | 20 | 20 | 10 | 9 |

- **Why they run (Q10).** The self-tests run in arm B because CPUs 1–2 now take a timer tick, not because of a second change: the branch's kernel changes are the per-CPU PPI setup and its report (`gic.rs`, `smp.rs`, `shared/src/gic.rs`; plan T1–T4), and no self-test, IPC or scheduler code changed. On `main` the balancer moves ipc-server (tid 8), thread A (tid 4) and cap-denied (tid 13) onto CPUs 1–2, where they starve behind test threads B and C, which never yield; arm A prints none of the lines above in any of its 30 boots, as in B1's 32.
- **gpu mode** says nothing about the self-tests: a first fatal report at t = 2–19 ends every arm-B gpu boot before they run; only run-09 printed thread A.
- **Non-fatal anomalies** (Q14 A: reported in these counts only, no class change): text B run-05 (CLEAN) logs `Echo client: ipc_call failed with -1` at 13.68 s, then completes its `process_exit` checks; run-17 prints thread A's iterations 0–2 only (lines 222–233; tid 4 is migrated CPU 1 → CPU 0 at line 234 and CPU 0 → CPU 3 at line 302, and prints nothing more); run-19 prints `Caller: got … iter=0` (line 221) and no later iteration or failure, and the echo client prints `error -12 after service death (expected EPIPE)` (line 240). None of these is a first fatal report.

## Attribution of every non-CLEAN arm-B boot ("What gates" item 5)

Signatures as pre-registered: **H1** PCZERO, or an EXCEPTION at a corrupt PC or register, with `elrmm` incrementing on the faulting CPU; **H3** PANIC-LOCK, `lock re-entry:` with `ctx=irq` or `irq-exit`; **N2** a WEDGE with `n2[rblk]`, `ubrbl[reply]` and `nowaker`; **N4** `n4`; **N5** an EXCEPTION on CPU 0 with EC 0x21 or 0x25 and ELR or FAR in [0x4000_0000, 0x8000_0000); **newly live self-test** the first fatal report lies in thread A, the echo IPC pair, cap-denied, the service echo pair or `process_exit`; **unattributed** anything else. The `elrmm` "before" value is the last `src=hb` line before the fault (an absent key is 0), the "after" value the faulting CPU's `src=exc` or `src=panic` line. Thread ids are slot numbers: 0–3 idle, 4–7 test threads A–D, 8/9 ipc-server/caller, 10 ipc-timeout, 13 cap-denied, 14/15 the service echo pair, 16 and above threads created later (in text mode the Interactive bench threads). No boot carries N2, N4 or N5's signature (`n2`, `ubrbl`, `nowaker`, `n4`, `nest` 0 in every line below; no ELR or FAR in the N5 range).

### Text, arm B (7 boots)

| Run | Class | Fatal CPU | Thread (`ctx`) | Signature | Key evidence |
| --- | --- | --- | --- | --- | --- |
| 01 | PCZERO | 1 | tid 5 (B) | H1 | Line 299 `EXCEPTION[CPU 1]` ESR 0x8600000d, EC 0x21, ELR 0, FAR 0. `elrmm[1]` 0 (hb t=11001, line 298) → 1 (`src=exc` t=11006, line 303), with `irqsw[1]` 202 → 203: the one IRQ-path switch in between is the one with the mismatch. B never migrated. CPU 1 `tick` frozen at 11029, `scanstall` 27; the only fatal report |
| 08 | EXCEPTION | 2 | tid 4 (A) | H1; by the letter also newly live self-test (see below) | Line 233 ESR 0x96000021 (EC 0x25, alignment fault, read), FAR 0x1021, ELR 0xffff0000000d4f2c = `ServiceName::matches`+0xc (`ldr x2, [x0, #0x20]`, arm-B ELF), so x0 = 0x1001: a corrupt register. The PC is the echo client's (tid 15) `service_lookup` after `process_exit(pid 7)`, run on thread A's stack (sp 0xC0 below its block top). `elrmm[2]` 0 (hb t=1, line 142) → 1 (`src=exc` t=232, line 253, interleaved to 256), `irqsw[2]` = 5. Second report line 269: CPU 0 PCZERO, tid 18, `elrmm[0]` 0 → 1 |
| 10 | EXCEPTION (PC zero in substance) | 2 | tid 6 (C) | H1 | Line 238 `EXCEPTION[CPU 2]: ESR=0x000000008600000d`, the rest (EC 0x21, FAR 0, line 254 `ELR=0x0000000000000000`) scattered through lines 239–254 by CPU 2's drained lines, so the classifier's ELR look-ahead (`exwin`, `tools/src/cmd/soak/classify.rs`) missed it. `elrmm[2]` and `spsrmm[2]` 0 (line 142) → 1 (`src=exc` t=409, line 261). CPU 2 `tick` frozen at 417. Second report lines 292–296: CPU 0 PCZERO, tid 16, `elrmm[0]` 1 → 2 |
| 13 | PCZERO | 0 | tid 16 | H1 | Line 297 ESR 0x86000006, ELR 0, FAR 0. `elrmm[0]` 1 (hb t=11001, line 296) → 2 (`src=exc` t=11002, line 301), `irqsw[0]` 982 → 983 |
| 17 | PCZERO | 0 | tid 16 | H1 | Line 325, ELR 0, FAR 0. `elrmm[0]` 1 (hb t=23001, line 324) → 2 (`src=exc` t=23002, line 329), `irqsw[0]` 2023 → 2024 |
| 18 | PCZERO | 2 | tid 7 (D) | H1 | Line 372 ESR 0x8600000d, ELR 0, FAR 0. `elrmm[2]` 0 (hb t=42001, line 371) → 1 (`src=exc` t=42480, lines 376–377, split by CPU 1's `migrated tid=7 from CPU 3 to CPU 2` at 71.687 s). `irqsw[2]` reads 752 in both lines. CPU 2 `tick` frozen at 42502; the boot ran its 75 s |
| 19 | PCZERO | 0 | tid 18 | H1 | Line 342 ESR 0x86000006, ELR 0, FAR 0. `elrmm[0]` 0 in every hb line (last t=43257, line 341: `elrmm=0,5,352,0`) → 1 (`src=exc` t=43258, line 346), `irqsw[0]` 3855 → 3856. CPU 2's `elrmm` climbed from 11 to 352 over 42 s without a fault on CPU 2 |

- **The PCZERO `new` marker (item 4).** Its 5 boots are runs 01 (CPU 1), 13, 17, 19 (CPU 0) and 18 (CPU 2), all H1; their first fatal lines are in the table. With run-10, 6 of the 7 failing text-B boots are PC-zero faults. The CPU 0 ones match the PCZERO boots on `main` before the step (8 of the 9 PCZERO boots on disk read `elrmm` = 2 on CPU 0, one reads 1; B1 text run-10 is the same report, `ctx: cpu=0 tid=16`). The CPU 1 and CPU 2 ones are the same mechanism newly reachable, because before the step CPUs 1–3 made no IRQ-path switch.
- **Run-08 overlap.** Its first report names thread A (`ctx: … tid=4`), one of the Q14 threads, and its PC is the service echo client's; it also carries H1's signature (a corrupt register with `elrmm` incrementing on the faulting CPU). The plan's Q14 text defines the self-test bucket for boots carrying none of H1–N5's signatures, so this note reads it as H1. Either reading attributes it; whether it also gets its own issue is T8 question 3.
- The `boots.tsv` detail for run-08 says "3 fatal reports"; the log has 2 EXCEPTION headers (lines 233, 269) and no PANIC.

### gpu, arm B (10 boots)

In every one of the 9 boots whose first report is on CPU 0, that report's tripwire line reads `irqsw=1,0,0,0`: CPU 0 had made its first IRQ-path switch, and CPUs 1–3 none, when it faulted or panicked. The last `src=hb` line before each is t=1 (line 158) and carries no `elrmm` key.

| Run | Class | Fatal CPU | Thread | Signature | Key evidence |
| --- | --- | --- | --- | --- | --- |
| 01 | EXCEPTION | 0 | tid 16 | H1 | Line 223 ESR 0x96000046 (EC 0x25), FAR 0xd, ELR 0xffff00000008a28c; `elrmm[0]` 0 → 1 (line 227). Only fatal report |
| 02 | EXCEPTION | 0 | tid 16 | H1 | FAR 0x12, ELR 0xffff00000009d50c; `elrmm[0]` 0 → 1 (line 227). Then CPU 1 tid 5 PCZERO (line 228) |
| 03 | EXCEPTION | 0 | tid 16 | H1 | FAR 0xc, ELR 0x…08a28c; `elrmm[0]` 0 → 1. Then CPU 1 tid 5 EC 0x25 FAR 0xffff000235efef60 |
| 04 | PANIC | 0 | tid 16 | **unattributed** | Lines 223–225: `panicked at kernel/src/compositor/service.rs:450:9` / `[mm] BUG: free_pages(0x1500, 18446462598733788976) — address not in any pool`, `ctx=thread irq_was=on`; `src=panic` t=19 `irqsw=1,0,0,0 elrmm=1,0,0,0` (line 226). Not PCZERO, EXCEPTION or PANIC-LOCK, not in a Q14 thread |
| 05 | EXCEPTION | 0 | tid 16 | H1 | FAR 0x9, ELR 0x…08a28c; `elrmm[0]` 0 → 1. Then CPU 1 tid 5 EC 0x25 FAR 0xffff0000002da010 |
| 06 | EXCEPTION | 0 | tid 16 | H1 | FAR 0x3, ELR 0x…08a28c; `elrmm[0]` 0 → 1. Then CPU 1 tid 5 EC 0x25 FAR 0xfff7000000f68c08 |
| 07 | PANIC-LOCK | 2 | tid 10 (ipc-timeout) | H3 | Lines 159–161: `lock re-entry: THREAD_TABLE on CPU 2 ctx=irq-exit holder=? holder_irqs=on tid=? gen=2`, `irq_elr=0xffff0000000ba5a0`; `src=panic` t=2 `tick=2,2,3,2 irqsw=0,0,0,0` (line 162). See the `holder=` check |
| 08 | EXCEPTION | 0 | tid 16 | H1 | FAR 0xb, ELR 0x…08a28c; `elrmm[0]` 0 → 1. Then CPU 1 tid 5 PCZERO |
| 09 | PANIC | 0 | tid 16 | **unattributed** | `free_pages(0x1500, 18446462598733784816)` at `service.rs:450:9`, `ctx=thread`; `src=panic` t=19 `irqsw=1,0,0,0 elrmm=1,0,0,0` (line 226) |
| 10 | EXCEPTION | 0 | tid 16 | H1 | FAR 0xd, ELR 0x…08a28c; `elrmm[0]` 0 → 1. Then CPU 1 tid 5 PCZERO and CPU 2 tid 6 PCZERO (`irqsw[2]` 674, `tick[2]` 36464) |

The seven CPU 0 EXCEPTIONs are B1's gpu H1 form (a data abort at a near-null FAR right after CPU 0's first IRQ-path switch, `elrmm=1`); arm A's gpu runs 02, 03, 05 and 10 fault at the same ELR 0xffff00000008a28c in tid 16. The CPU 1 and CPU 2 second reports are H1 on the secondaries after CPU 0 died.

### Arm A, for comparison (not gated)

| Mode | Run | Class | Fatal CPU | Thread | Reading | Key evidence |
| --- | --- | --- | --- | --- | --- | --- |
| text | 01 | EXCEPTION | 0 | tid 18 | H1 | Line 227 ESR 0x96000061 (EC 0x25), FAR 0x81a, ELR 0xffff0000000bacd4 = `stlr w22, [x9]` with x9 = x19 + 0x808 in `IrqSpinLock<TIMEOUT_QUEUE>::lock_contended` (x19 = 0x12; B1's text H1 form); `elrmm[0]` 0 (line 177) → 1 (line 231) |
| text | 02 | PANIC-LOCK | 0 | tid 16 | H3 | Lines 227–228 `lock re-entry: THREAD_TABLE on CPU 0 ctx=irq-exit holder=kernel/src/cap/mod.rs:39 … gen=374459` |
| text | 04 | WEDGE-ALIVE | none | — | N2 | No fatal report; line 341 (t=44257) `n2=0,1,0,0`, `ubrbl[reply]=1`, `nowaker=1` |
| text | 15 | PANIC-LOCK | 0 | tid 16 | H3 | Lines 227–228, same form, `gen=361176`; `elrmm=0,0,0,0` |
| text | 20 | DEGRADED | none | — | not fatal | Gate 1 completed with 9999 of 10000 IPC iterations (line 231); line 330 `latereply=1`, `ubrbl[reply]=1` |
| gpu | 01 | EXCEPTION | 0 | tid 16 | H1 form | FAR 0xffe0e0e0, ELR 0xffff00000009e674, `elrmm=1,0,0,0`, `irqsw=1,0,0,0` |
| gpu | 02, 03, 05, 10 | EXCEPTION | 0 | tid 16 | H1 form | ELR 0xffff00000008a28c, FAR 0xb, 0xd, 0x3, 0xb; `elrmm=1,0,0,0`, `irqsw=1,0,0,0` |
| gpu | 07 | EXCEPTION | 0 | tid 16 | H1 form | FAR 0xfd8, ELR 0xffff00000008b180, `irqsw=2,0,0,0`, `elrmm=1,0,0,0` |
| gpu | 08 | EXCEPTION | 0 | tid 19 | H1 form | FAR 0, ELR 0xffff00000009cc0c, t=556, `irqsw=4,0,0,0`, `elrmm=1,0,0,0` |
| gpu | 04, 06 | PANIC | 0 | tid 16 | bad-free (as B 04, 09) | `service.rs:450:9`, `free_pages(0x320, 144115188075855872)` and `free_pages(0x1500, 18446462598733787592)`, `ctx=thread`, `irqsw=1,0,0,0`, `elrmm=1,0,0,0` |

The bad-free PANIC is known before this step: the crash-fix ADR's evidence note (2026-10-10, gpu boot gate at 78fa514) records it at `compositor/service.rs:450` with `irqsw=1` and `elrmm=1,0,0,0`, calls it "the first runtime link between H1 and this PANIC", and leaves its attribution to steps 3 and 4. Its rate is 2 of 10 in each arm here (p more in B = 0.709).

## `holder=` check (boot check 5, read in arm B)

- **Text arm B:** no `[tripwire-ev]` or `lock re-entry:` line in any of the 20 boots, so nothing to read.
- **gpu arm B:** one qualifying event, run-07: `lock re-entry: THREAD_TABLE on CPU 2 ctx=irq-exit holder=? holder_irqs=on tid=? gen=2`, followed by three interleaved `[tripwire-ev] kind=stuck` lines (cpu=1, cpu=3, cpu=0; `owner_cpu=2`, `owner_gen=2`), all `holder=?`. The check as written ("prints a readable `holder=<file>:<line>`") **is not met** for this event.
- **What `?` means in the code:** `put_tid` prints `?` for `TID_NONE` and a null site prints `?` (`kernel/src/sync/irq_spin_lock.rs:660-680`); `holder_snapshot` returns `(TID_NONE, null)` when the lock word changed or the holder fields were not yet written (`:303-317`); `hold` writes `tid` and `kva_of(site)` only after the CAS (`:290-301`). So the line reports holder fields that were unset, not a garbled address from `kva_of`. At `gen=2` and tick 2 on CPU 2, an IRQ landing between the holder's CAS and its field stores fits that reading; it was not traced further. The run-07 tid (10, ipc-timeout) is not a Q14 thread.
- **Not exercised:** no arm-B line prints a holder recorded on CPUs 1–3, so `kva_of`'s physical-alias branch produced no printed output in this pair (T8 question 2).
- Arm A's `kind=stuck cpu=3` lines (text runs 02 and 15, line 231) name CPU 3 only as the printing CPU, with the owner on CPU 0 and a readable `holder=kernel/src/cap/mod.rs:39`; they are the pre-step H3 form.

## N2 after the step (B2, Q6)

Arm B's text boots read 0 for every N2 key: `n2` (rpre, rblk, vpre, vblk), `ubrun` and `ubrbl[reply]`, `latereply` and `nowaker` in 20 of 20; `ctbusy` is 16/31 and there is no WEDGE. Arm A has one N2 WEDGE (run-04: `n2[rblk]`, `ubrbl[reply]`, `nowaker` 1 each), plus `ubrbl[reply]` and `latereply` in run-20. Under Q6 A this arm B is the B2 baseline only if the merged `main` kernel ELF equals arm B's (`690350af4860ed7f`); otherwise Q6 B's separate soak runs.

## Gate decisions, as pre-registered

1. **Fix, mechanically: pass.** All 60 boots in scope; arm B `tick[1..3]` > 0 in 30/30, arm A 0 in 30/30.
2. **Regression guard: text pass** (15/20 against 13/20, p = 0.366). gpu informational (1/10 against 0/10, p = 0.5).
3. **`removed`:** no marker; nothing claimed.
4. **`new`:** text PCZERO (0/20 against 5/20, p = 0.0236), reported with every boot's first fatal line and attribution above: all 5 H1.
5. **Attribution: 15 of 17 non-CLEAN arm-B boots attributed** (H1 14: text 7, gpu 7; H3 1: gpu run-07). **2 unattributed:** gpu runs 04 and 09, the bad-free PANIC at `compositor/service.rs:450`. The same PANIC is in arm A's gpu runs 04 and 06 (2 of 10 against 2 of 10, with the same site, CPU 0, tid 16 and `irqsw=1,0,0,0 elrmm=1,0,0,0`).

**Step result under the pre-registered rule: not passed; to the owner.** The step passes when item 1 passes and either the text guard passes or every extra non-CLEAN boot is attributed, and "any unattributed signature in arm B fails the step, whatever the guard says, and goes to the owner". Items 1 and 2 pass; item 5's two unattributed boots decide it.

## Owner questions

1. **The bad-free PANIC (gpu B runs 04 and 09).** It is not one of item 5's signatures: it is a PANIC, not a PCZERO, EXCEPTION or PANIC-LOCK, and not in a Q14 thread. Facts that bear on it: it occurs at the same rate in arm A (runs 04, 06); in both arm-B boots CPUs 1–3 had made no IRQ-path switch when CPU 0 panicked (`irqsw=1,0,0,0`); the ADR links it to H1 (`elrmm=1`) and leaves it to steps 3 and 4. Options: (A) rule it pre-existing exposure, record it with the lower baseline Q2 A asks for, and let the step proceed to T9; (B) hold the step as item 5 reads until the bad-free PANIC is attributed by steps 3 and 4; (C) another ruling.
2. **The `holder=` check.** gpu B run-07's only CPU 1–3 event prints `holder=? tid=?`, which by the code means the holder fields were unset, not a garbled `kva_of` value; and no arm-B line exercised `kva_of`'s physical-alias output. Accept the check as "no holder recorded, nothing garbled", or require a targeted check of `kva_of` on a secondary before the PR?
3. **Text B run-08.** Read as H1, but its first report also names thread A, a Q14 thread. Q14 A says each distinct self-test signature gets its own issue. File one for the `ServiceName::matches` alignment fault (ELR 0xffff0000000d4f2c, FAR 0x1021), or treat the boot as H1 only?
