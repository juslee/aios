---
author: jl + claude
date: 2026-10-11
tags: [kernel, smp, sched]
status: final
---

# Research: Crash-fix step 1b, A/B pair (af59149 vs e211d6d)

Step 1b (#238, e211d6d) added the detect-only tripwires and the `IrqSpinLock` re-entry detector. Its acceptance is an interleaved A/B against `main` at its merge base (af59149), which had to wait for step 1a's interleave mode (`just soak --arm`, #246). This note gives that pair's results. Arm A is af59149 and arm B is e211d6d. See the [crash-fix ADR](../decisions/2026-09-22-jl-crash-fix-preemption-and-fp.md) for the hypotheses (H1, H3, N2), step 1b's acceptance and the errata that restate its decision rules. See [B1's single-arm baseline](2026-10-10-jl-crash-fix-1b-n2-baseline.md) for the counters on the step-1b branch and [observability.md §6.5](../../kernel/observability.md) for every tripwire key. Step 200's pair, which ran in the same session, is in [its own note](2026-10-11-jl-crash-fix-200-ab.md).

## Conditions

- **Arms:** detached checkouts `/Users/juslee/dev/aios-af59149` (A, kernel ELF sha256 prefix `8a9a2094e3dda6de`) and `/Users/juslee/dev/aios-e211d6d` (B, `62519fa0670768ce`). Both pin nightly-2026-10-09 (`rustc 1.101.0-nightly (a30aa9064 2026-10-08)`) and contain 7167d40 (#196).
- **Instrument:** QEMU 11.1.2 (Homebrew, binary sha256 prefix `7299ef807db462f9`), Homebrew edk2 firmware (`edk2-aarch64-code.fd`, sha256 prefix `47765fe344818cbc`), one ESP image per arm, a fresh data disk per boot, 75 s boots with a 15 s stall limit. Harness `aios soak` at 030b3b7. The arm order rotated each round.
- **Order:** the owner's run-order answer (Q7, option A): step 1b's pair first, then step 200's. The text pair ran about 10:40–11:30 (+07) and the first gpu pair about 11:31–11:55, both before step 200's pair (about 12:04–13:21). The gpu redo ran about 13:31–13:56, after it. Step 200's arms are separate checkouts, so only the host's load is shared.
- **Host:** Apple Silicon Mac, 10 CPUs, shared with other sessions. iCloud and Spotlight load could not be controlled. Every run passed the start check (load1 at or below 10 before the builds: 9.67, 8.90 and 8.99).

| Run | Boots per arm | Logs | load1 mean A / B | load1 max A / B | Load rule (25%) |
| --- | --- | --- | --- | --- | --- |
| text | 20 | `target/soak/20261011-t8-1b-text` | 16.31 / 16.44 | 40.77 / 32.71 | 1% apart, comparable |
| gpu | 10 | `target/soak/20261011-t8-1b-gpu` | 20.59 / 15.17 | 42.69 / 29.30 | 36% apart, harness says redo |
| gpu redo | 10 | `target/soak/20261011-t8-1b-gpu-redo` | 11.11 / 14.01 | 14.06 / 19.01 | 26% apart, harness says redo again |

The owner accepted both gpu runs as they stand on 2026-10-11 and closed the soak window. Pooled, CLEAN is 2/20 in each arm. At a CLEAN rate near 10%, a 10-boot gpu regression guard cannot fail: B would need 0 CLEAN against at least 4 of 10 in A (one-sided p 0.043). Class differences within a gpu run still carry the load caveat.

The runs did not pass `--combine WEDGE-STUCK+PANIC-LOCK`, so the combined rows below were computed for this note. They use the harness's test (Fisher's exact test on conclusive boots) and reproduce its p values for the single-class rows.

## Classes and pair tests

From each run's `summary.md`. No boot was INCONCLUSIVE or DEGRADED, and no class got a `removed` or `new` marker in any run.

| Class | text A | text B | gpu A | gpu B | redo A | redo B |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| PCZERO | 0 | 0 | 0 | 0 | 1 | 1 |
| PANIC-LOCK | 0 | 2 | 0 | 1 | 0 | 0 |
| PANIC | 0 | 0 | 3 | 1 | 2 | 2 |
| EXCEPTION | 1 | 1 | 4 | 7 | 4 | 5 |
| WEDGE-STUCK | 4 | 0 | 2 | 0 | 1 | 0 |
| WEDGE-ALIVE | 0 | 0 | 0 | 1 | 1 | 0 |
| CLEAN | 15 | 17 | 1 | 0 | 1 | 2 |
| CLEAN rate (Wilson 95%) | 75% (53–89%) | 85% (64–95%) | 10% (2–40%) | 0% (0–28%) | 10% (2–40%) | 20% (6–51%) |

Regression guard (fewer CLEAN in B, one-sided): text 15/20 vs 17/20, p 0.882; gpu 1/10 vs 0/10, p 0.5; redo 1/10 vs 2/10, p 0.895. All three pass.

| Row | Run | A | B | p fewer in B | p more in B | p two-sided |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| WEDGE-STUCK | text | 4/20 | 0/20 | 0.053 | 1 | 0.106 |
| PANIC-LOCK | text | 0/20 | 2/20 | 1 | 0.244 | 0.487 |
| EXCEPTION | text | 1/20 | 1/20 | 0.756 | 0.756 | 1 |
| WEDGE-STUCK + PANIC-LOCK | text | 4/20 | 2/20 | 0.331 | 0.909 | 0.661 |
| EXCEPTION | gpu | 4/10 | 7/10 | 0.965 | 0.185 | 0.37 |
| PANIC | gpu | 3/10 | 1/10 | 0.291 | 0.957 | 0.582 |
| WEDGE-STUCK + PANIC-LOCK | gpu | 2/10 | 1/10 | 0.5 | 0.895 | 1 |
| WEDGE-STUCK + PANIC-LOCK | gpu redo | 1/10 | 0/10 | 0.5 | 1 | 1 |

The other per-class rows have p ≥ 0.5 in every direction (see each `summary.md`). Gate 1 IPC is a recorded marker, never gated on. Text: mean round trip 4.73 µs over A's 15 CLEAN boots and 8.41 µs over B's 17, with G1PASS in 15 of 20 and 11 of 20. That is the detect-only lock's accepted Gate 1 cost (ADR erratum "The detect-only lock costs Gate 1 time").

## Step 1b's acceptance

The ADR ("Steps", step 1b, "Acceptance") sets three conditions:

1. **No new class except PANIC-LOCK.** Met in text and in the gpu redo. In the first gpu run, B has one WEDGE-ALIVE against none in A (p 0.5, no `new` marker). That boot is a harness false positive, not a kernel hang: Gate 1 finished with both gates PASS, but interleaved UART output tore the "=== Gate 1 Complete ===" line, so the G1DONE marker never matched. Arm A has a WEDGE-ALIVE of its own in the redo.
2. **WEDGE-STUCK + PANIC-LOCK in B not different from WEDGE-STUCK in A** (two-sided p ≥ 0.05). Met: p 0.661 (text), 1 (gpu) and 1 (redo). At these counts the test has little power. It shows only that no difference was detected, not that the two rates are equal.
3. **Every non-CLEAN boot has a tripwire line or a fatal dump.** Met: the harness counts 0 B boots without one in all three runs.

## Per-boot attribution

Every non-CLEAN boot was read from its log with CR stripped. ELRs were symbolised with `nm` or lldb against each arm's ELF, whose sha matches the summary: `/Users/juslee/dev/aios-af59149/target/aarch64-unknown-none/debug/kernel` and `/Users/juslee/dev/aios-e211d6d/target/aarch64-unknown-none/debug/kernel`. Arm A has no tripwire line, no lock detector and no `#[track_caller]` on `free_pages`, so its attributions rest on signature, timing and the matching arm-B boots: they are probable, not proven. Its panic handler also predates N8 and leaves IRQs on, so an arm-A PANIC boot runs on to tick 45000 and its panic text names no CPU. Both arms predate step 200, so only CPU 0 takes timer IRQs.

In the `free_pages(addr, order)` panics, `addr` is the first value. In every one, `addr` (0x1500 or 0x320) is in no pool and `order` is garbage: a kernel-image VA such as `0xffff0000000de5c0`, or 2^57.

### Text, arm A

| Run | Class | Signature | Key evidence |
| --- | --- | --- | --- |
| 01 | WEDGE-STUCK | H3, probable (pre-detector form) | Heartbeat stuck at tick 0 (`hb_count`=1). The log ends at "Bench main: server ready, starting IPC benchmark", with no PANIC or EXCEPTION. |
| 03 | WEDGE-STUCK | H3, probable | Same shape as 01. |
| 07 | WEDGE-STUCK | H3, probable | Same, but the log ends at the "=== Gate 1 Benchmark ===" header. The later bench lines are probably still in the log ring that CPU 0's tick drains. |
| 13 | WEDGE-STUCK | H3, probable | Same shape as 01. |
| 09 | EXCEPTION, CPU 0 | H1, probable | Data abort, ESR 0x96000006, FAR 0x100, ELR 0xffff0000000aa340 = `ipc_recv`+1304, the CHANNEL_TABLE `spin::Mutex::lock` CAS (`channel.rs:248`). The lock-pointer register held ~0. B's run 01 faults in the same function and phase. |

All five stop at about 6.6 s, at the Gate 1 bench start, with heartbeat tick 0 the only one. H3 rather than N2: in an N2 wedge the heartbeat stays alive (B1). H3 rather than an H1 hijack: a hijacked bench thread also leaves the heartbeat alive. B's two PANIC-LOCK boots die at the same point. Without a detector, the lock and its holder cannot be named.

### Text, arm B

| Run | Class | Signature | Key evidence |
| --- | --- | --- | --- |
| 01 | EXCEPTION, CPU 0 | H1 | Data abort, ESR 0x96000006 (level-2 translation fault), FAR 0xef9, ELR 0xffff0000000b1248 (`ipc_recv`+1536, in `SpinMutex::lock`). Thread level (tid=18, irq=0 sched=0), 0.1 s after "starting IPC benchmark". The `src=exc` line (t=514) has `elrmm`=1, `spsrmm`=1 and `irqsw`=3 on CPU 0. Every other detector is 0. |
| 03 | PANIC-LOCK, CPU 0 | H3 | `scheduler.rs:196:38`: "lock re-entry: THREAD_TABLE on CPU 0 ctx=irq-exit holder=? holder_irqs=on tid=? gen=334048". `[panic]` tid=16, `irq_elr` 0xffff0000000c397c, t=6.748. A `[tripwire-ev] kind=stuck` line shows CPU 3 waiting on THREAD_TABLE owned by CPU 0 at the same generation (`lktry[THREAD_TABLE]`=1). `elrmm`=0, `n2`=0, `nowaker`=0. |
| 14 | PANIC-LOCK, CPU 0 | H3 | Same site and text, gen=356286, the same tid=16 and `irq_elr`, t=6.719, the same `kind=stuck` event from CPU 3. `elrmm`=0. |

### gpu, arm A

| Run | Class | Signature | Key evidence |
| --- | --- | --- | --- |
| 02 | WEDGE-STUCK | H3, probable (pre-detector form) | Heartbeat stops at tick 0 right after the bench starts. The last line is a CPU 0 IRQ-path "Load balance: migrated tid=4 from CPU 1 to CPU 0" at 6.604. |
| 03 | EXCEPTION, CPU 0 | H1, probable | Data abort, ESR 0x96000046, FAR 0x8, ELR 0xffff00000009a0bc = `compositor_loop`+0x334. It comes after "VirtIO-GPU resource 5 created" and before "display handoff complete", the site of B's gpu H1 faults. |
| 04 | EXCEPTION, CPU 0 | H1-class (corrupt PC) | PC alignment fault (EC 0x22), ELR = FAR = 0xffff0001440b6493, an odd direct-map address. It comes late, after tick 12000 (about 22.9 s). |
| 05 | WEDGE-STUCK | H3, probable | Heartbeat stops at tick 0. The log ends at the bench banner, before "Channel 11 created". |
| 06 | EXCEPTION, CPU 0 | H1-class | Data abort, ESR 0x96000045, FAR 0xffe0e0e0 (a pixel colour used as a pointer), ELR 0xffff0000000b4c98 = `gpu_transfer_to_host`+0xdc, before the bench. B's run 03 has the same FAR. |
| 07 | PANIC | bad-free | `frame.rs:51:9`: `free_pages(0x1500, 0xffff0000000de5c0)`, then two "VirtIO-GPU error response 0x1203" lines. That is the `release_buffer` sequence B names as `compositor/service.rs:450`. |
| 08 | PANIC | bad-free | Same, `order` 0xffff0000000dd680. |
| 09 | PANIC | bad-free | Same, `order` 0xffff0000000dd680. |
| 10 | EXCEPTION, CPU 0 | H1, probable | FAR 0xd at the same ELR as 03 (`compositor_loop`+0x334). |

### gpu, arm B

| Run | Class | Signature | Key evidence |
| --- | --- | --- | --- |
| 01 | EXCEPTION, CPU 0 | H1 | ELR 0xffff00000009d154, `compositor_loop`'s framebuffer fill (`str w9,[x10],#4`, with x10 from callee-saved x20). FAR 0x7, ESR 0x96000046. tid=16, irq=0 sched=0. `src=exc` t=18: `irqsw`=1, `elrmm`=1, `spsrmm`=1. |
| 02 | EXCEPTION, CPU 0 | H1 | Same ELR, FAR 0x9. t=22, `irqsw`=1, `elrmm`=1. `lkoxp`=2 (contention only). |
| 03 | EXCEPTION, CPU 0 | H1 | ELR 0xffff0000000bbe54 (`gpu_transfer_to_host`+0xdc), FAR 0xffe0e0e0, ESR 0x96000045. tid=16. t=23, `irqsw`=1, `elrmm`=1. |
| 04 | PANIC-LOCK, CPU 0 | H3 | `scheduler.rs:196:38`: "lock re-entry: THREAD_TABLE on CPU 0 ctx=irq-exit holder=kernel/src/cap/mod.rs:39 holder_irqs=on tid=19 gen=472952". `irq_elr` 0xffff0000000c3a08 is in `cap::process_of_thread`. t=579, after the bench started. `irqsw`=4, `elrmm`=0. A `kind=stuck` line shows CPU 3 waiting on THREAD_TABLE owned by CPU 0. |
| 05 | EXCEPTION, CPU 0 | H1 | ELR 0x9d154, FAR 0x6. t=22, `irqsw`=1, `elrmm`=1. |
| 06 | WEDGE-ALIVE | other: harness false positive | Gate 1 finished (avg=6 µs, IPC PASS, context switch PASS). The torn "=== Gate 1 Co[ 6.741222] ... mplete ===" line missed the G1DONE marker. The heartbeat reached tick 45000. `g1_elrmm`=0, and `elrmm`=1 by t=1001 on a thread that kept running. |
| 07 | PANIC, CPU 0 | bad-free (H1 downstream) | `compositor/service.rs:450:9`: `free_pages(0x1500, 0xffff0000000eacf8)`, then GPU error 0x1203. tid=16 in `release_buffer`. `src=panic` t=19: `irqsw`=1, `elrmm`=1, `spsrmm`=1, the same counters as the H1 faults. |
| 08 | EXCEPTION, CPU 0 | H1 | ELR 0x9d154, FAR 0x2. t=22, `irqsw`=1, `elrmm`=1. |
| 09 | EXCEPTION, CPU 0 | H1 | ELR 0x9d154, FAR 0x6. t=18, `irqsw`=1, `elrmm`=1. |
| 10 | EXCEPTION, CPU 0 | H1 | ELR 0x9d154, FAR 0xc. t=18, `irqsw`=1, `elrmm`=1. |

In 01–05 and 07–10, `n2`, `ub*` (apart from `ubmove`) and `nowaker` are 0.

### gpu redo, arm A

| Run | Class | Signature | Key evidence |
| --- | --- | --- | --- |
| 01 | EXCEPTION, CPU 0 | H1-class (corrupt PC) | Instruction abort (EC 0x21), ELR = FAR = 0xffff0001440b64bc, a direct-map RW+XN address. After the bench start, at tick 3000. |
| 02 | EXCEPTION, CPU 0 | H1-class (corrupt PC) | PC alignment fault (EC 0x22), ELR = FAR = 0xffff0001440b60ef, in the same page. At tick 13000. |
| 03 | PANIC | other: garbage pointer | `virtio_gpu.rs:383:32`: the `read_volatile` of `used_virt + 2` is misaligned, so `used_virt` was garbage. It comes during display handoff, right after resource 5's backing. Gate 1 never completed. H1 or H5 is plausible, not shown. |
| 04 | EXCEPTION, CPU 0 | H1, probable | FAR 0xa, ELR 0xffff00000009a0bc. lldb: `compositor_loop` > `display_handoff` (`service.rs:429`) > `fill_buffer` (`:495`) > `slice::fill` through a garbage `fb_virt`. At tick 0. |
| 05 | PCZERO, CPU 0 | H1, probable | Instruction abort at ELR = FAR = 0, about 60 ms after Gate 1 completed (PASS, avg=5 µs). |
| 06 | WEDGE-STUCK | H3 candidate, unproven | Everything stops after CPU 0's IRQ-path "Load balance: migrated tid=4 CPU1->CPU0" at 6.658, just after the bench start. No heartbeat after tick 0. |
| 08 | PANIC | bad-free | `frame.rs:51:9`: `free_pages(0x1500, 0xffff0000000deb40)`, then GPU error 0x1203. |
| 09 | EXCEPTION, CPU 0 | H1, probable | FAR 0xc at the same ELR as 04, the same `fill_buffer` path. At tick 0. |
| 10 | WEDGE-ALIVE | undetermined | Heartbeat to 44000, "bench started" the last bench line, no fatal. Only 2 migrations of tid 4 and 8 after the bench start, against 6–8 in clean boots, so the bench chain stopped. N2 and an H1 hijack fit; with CPU 0 still ticking, same-CPU re-entry on CPU 0 does not. Arm A has no `n2` or `nowaker` to decide. |

### gpu redo, arm B

| Run | Class | Signature | Key evidence |
| --- | --- | --- | --- |
| 01 | PANIC, CPU 0 | bad-free (H1 downstream) | `service.rs:450:9`: `free_pages(0x1500, 0xffff0000000eb378)`, then GPU error 0x1203 right after "display handoff complete". tid=16. `src=panic` t=23: `irqsw`=1, `elrmm`=1, `spsrmm`=1. |
| 02 | EXCEPTION, CPU 0 | H1 | Data abort, FAR 0xd, ELR 0x9d154 (`compositor_loop`+0x33c), after resource 5 is allocated. tid=16, irq=0 sched=0. t=18, `irqsw`=1, `elrmm`=1. |
| 03 | PCZERO, CPU 0 | H1 | Instruction abort at ELR = FAR = 0 in bench thread tid=19, at tick about 44327, after G1PASS and G1DONE. `g1_elrmm`=0. `elrmm` was 1 at heartbeat 44001 and 2 in the `src=exc` line (`irqsw` 2983 to 3007), so one more mismatching IRQ-path switch on CPU 0 came just before the fault. |
| 04 | EXCEPTION, CPU 0 | H1 | FAR 0x7, ELR 0x9d154. t=18, `irqsw`=1, `elrmm`=1. |
| 05 | EXCEPTION, CPU 0 | H1 | FAR 0x882, ELR 0xffff0000000baab4 (`submit_command`+0x17c, called by the compositor). t=26, `irqsw`=1, `elrmm`=1. |
| 07 | EXCEPTION, CPU 0 | H1 | FAR 0xe, ELR 0x9d154. t=22, `irqsw`=1, `elrmm`=1. |
| 08 | EXCEPTION, CPU 0 | H1 | FAR 0xffe0e0e0, ELR 0xffff0000000bbe54 (`gpu_transfer_to_host`+0xdc). t=19, `irqsw`=1, `elrmm`=1. |
| 10 | PANIC, CPU 0 | bad-free (H1 downstream) | `service.rs:450:9`: `free_pages(0x320, 0x200000000000000)`. tid=16. `src=panic` t=26: `irqsw`=1, `elrmm`=1, `spsrmm`=1. |

In all 8, the lock keys (`lkself`, `lktry`, `lkph`, `lkphrun`, `lkstk`), `n2`, `ub*` (apart from `ubmove`: run 03 has `ubmove[to]`=2, as do the CLEAN boots 06 and 09), `nowaker`, `n4`, `tpidrbad`, `pcnull`, `pcphys`, `pcother` and `spbad` are 0. The CLEAN boots 06 and 09 have `elrmm`=0 at the `src=g1` line (tick about 613) and 1 by tick about 1001.

### Tally

| Signature | text A | text B | gpu A | gpu B | redo A | redo B |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| H3, detector-proven (PANIC-LOCK, `ctx=irq-exit`, `holder_irqs=on`) | – | 2 | – | 1 | – | 0 |
| H3, probable (heartbeat stopped at tick 0) | 4 | 0 | 2 | 0 | 1 | 0 |
| H1 (B: `elrmm` on the fatal line; A: signature only) | 1 | 1 | 4 | 7 | 5 | 6 |
| bad-free PANIC (B: same counters as H1) | 0 | 0 | 3 | 1 | 1 | 2 |
| other or undetermined | 0 | 0 | 0 | 1 | 2 | 0 |
| N2 | 0 | 0 | 0 | 0 | 0 | 0 |

## What the pair says about step 1b

Step 1b is detect-only. It changes no scheduling except the near-no-op bench-masking removal (F6), so the expected effect is relabelling: failures keep their rate and gain names. That is what the pair shows.

- **Text: WEDGE-STUCK 4 → 0 and PANIC-LOCK 0 → 2.** The stalls that wedged A at the bench start with no output now panic in B with a named lock, context and generation: `lock re-entry: THREAD_TABLE ... ctx=irq-exit holder_irqs=on`. The combined rows (4/20 vs 2/20, p 0.661) detect no change in the rate of the underlying failure, which is not the same as showing the rates are equal. That A's four wedges are the same failure is inferred from timing, not proven.
- **gpu: the same relabelling at smaller counts.** WEDGE-STUCK goes 2 → 0 and 1 → 0, and PANIC-LOCK 0 → 1 and 0 → 0. The bad-free panics now print their caller (`compositor/service.rs:450` instead of `frame.rs:51`). A PANIC on CPU 0 now stops the heartbeat (N8) instead of running on to tick 45000. EXCEPTION 4 → 7 in the first gpu run is within noise (p 0.185, loads 36% apart) and is 4 → 5 in the redo.
- **No CLEAN regression detected.** Text 15/20 vs 17/20 (p 0.882), gpu 1/10 vs 0/10 (p 0.5), redo 1/10 vs 2/10 (p 0.895). The Gate 1 latency marker drops (G1PASS 15 → 11 of 20 in text) by the accepted lock cost. It is recorded, not gated.
- **What is not significant.** No class difference reaches p < 0.05 in any run. The closest, text WEDGE-STUCK 4 → 0 (one-sided p 0.053), just misses the `removed` marker; the ADR's statistics bullet predicted about 0.056 for 4 → 0 at 30 boots. PANIC-LOCK 0 → 2 alone gives p 0.244. These counts cannot show that step 1b changed any rate, and step 1b is not meant to. What the pair establishes is attribution: every B failure has a tripwire line or fatal dump, and most name their mechanism.
- **N2 did not recur.** B1 saw 3 N2 wedges in 20 text boots of the step-1b branch (c065b0c). B has none in 20 text boots here, with `n2`, `ubrbl[reply]` and `nowaker` 0 in every boot. B1 was a different session and is not an arm of this pair, so no comparison is drawn (were it one, 3/20 vs 0/20 would give one-sided p 0.115).

## What this means for the ADR

This note does not edit the ADR. The pair supplies what its "Pending acceptance soak" bullets under "Step 1b decisions" wait for: the H1 and H3 verdicts and step 1b's acceptance.

- **H3: not refuted, and shown at runtime.** The restated rule (erratum "Re-entry is decided in every context") refutes H3 only on 3 or more WEDGE-STUCK with no PANIC-LOCK or `kind=self` line in `ctx=irq` or `irq-exit`. Arm B has 0 WEDGE-STUCK and 3 PANIC-LOCKs with `ctx=irq-exit holder_irqs=on` (text 03 and 14, gpu 04), all THREAD_TABLE on CPU 0 at the bench start. gpu 04 names the holder, `cap/mod.rs:39` (`process_of_thread`), one of the thread-level holders the ADR's H3 row lists. No boot has a `kind=self` line (`ev_self_irq`=0 everywhere). This is H3 evidence before step 2, and step 2 (IRQ-class locks that mask IRQs) targets exactly this mechanism.
- **H1: the verdict waits for the owner's restatement** (erratum "The H1 rule cannot be applied as written"). Both candidates can be computed from the columns, leaving out boots whose first fatal report is a PANIC (any PANIC or PANIC-LOCK):
  - *(a) Count only mismatches before the `src=g1` line.* For a boot that died before `src=g1`, that is its last line's `elrmm`. H1 is live: 8 of the pair's 26 such B boots have a mismatch (text 1 of 18, gpu 7 of 8), and 13 of 34 with the redo (redo 5 of 8).
  - *(b) Require `elrmm` > 1.* No boot of the pair qualifies (0 of 26), so the rule as written would refute H1. The redo adds one (B 03, the PCZERO with `elrmm`=2): 1 of 34.
  - Under (a) the B arms separate cleanly. All 21 B boots that reached `src=g1` have `g1_elrmm`=0, over a window (to tick about 613) that covers the ticks the early boots died in. All 13 B EXCEPTIONs that died before `src=g1` (text 01, gpu 01–03, 05, 08–10, redo 02, 04, 05, 07, 08) have `elrmm`=1. The gpu ones have `irqsw`=1, so the fault follows the first IRQ-path switch, whose saved ELR and SPSR did not match. The 3 bad-free panics show the same counters, and the 3 PANIC-LOCKs have `elrmm`=0. Rule (b) counts a single corrupting switch as no evidence, so it would read the dominant gpu failure as unrelated to H1. That fits (a) better, but choosing the rule is the owner's decision. The counters show that a mismatching switch came before each fault. They do not show the mechanism of the corruption.
- **#200 and CPU 0.** Every fatal report that names a CPU, in both arms, is on CPU 0, the only CPU that takes timer IRQs before step 200. The H1 and H3 evidence here therefore covers CPU 0's IRQ path only. Step 200's pair, in the note named above, adds an arm in which CPUs 1–3 tick as well.

## Files

- Summaries and per-boot TSVs: `/Users/juslee/dev/aios/target/soak/20261011-t8-1b-text/`, `.../20261011-t8-1b-gpu/` and `.../20261011-t8-1b-gpu-redo/` (`summary.md`, `boots.tsv`, `arms.tsv`, `arm-A/run-NN.log`, `arm-B/run-NN.log`).
- ELFs used for symbolisation: `/Users/juslee/dev/aios-af59149/target/aarch64-unknown-none/debug/kernel` and `/Users/juslee/dev/aios-e211d6d/target/aarch64-unknown-none/debug/kernel`.
