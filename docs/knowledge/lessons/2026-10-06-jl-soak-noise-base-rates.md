---
author: jl + claude
date: 2026-10-06
tags: [boot, sched, smp, tooling]
status: final
---

# Lesson: Know the boot noise and the base rates before blaming a change

## What happened

Boot logs from QEMU on the local host carry lines that look like failures but are not, and the failure classes that `aios soak` reports have non-zero rates on `main` itself. Reviewers and implementers repeatedly blamed a change for one of these. The facts below were re-checked on 2026-10-10 (Homebrew QEMU 11.1.2, edk2 `edk2-aarch64-code.fd`, `main` at af59149 for the logs).

**Firmware noise before the stub.** `Error: Image at 000BF... start failed: <status>` lines and `ArmTrngLib could not be correctly initialized.` come from edk2 driver dispatch, before `BdsDxe:` and the AIOS stub.

- They appear with no disk attached (the firmware then drops to its shell), so they have nothing to do with AIOS or a boot option.
- With `-cpu cortex-a72` a no-disk, no-ramfb boot prints five of them: `00000001`, two `Not Found`, `Unsupported`, `Aborted`. With `-cpu max` the `00000001` and `Aborted` lines are gone. `ArmTrngLib could not be correctly initialized.` stays under both, so it is not tied to the CPU model. An earlier note tied it to the same root as the two vanishing lines; that was wrong and is dropped.
- A boot with ramfb prints one `Not Found`; without ramfb (`just run-gpu`) it prints one more, because the ramfb DXE driver finds no device.
- The stub itself warns `No EFI_RNG_PROTOCOL (rng_seed will be zero)` on cortex-a72 (`acquire_rng_seed` in `uefi-stub/src/main.rs`). The only effect is a zero `rng_seed`; the KASLR slide is computed but not applied yet (`.claude/CLAUDE.md`, KASLR), so nothing visible depends on it.

Only lines from `uefi-stub` onward matter for an acceptance check.

**Kernel self-test warnings in every boot.** Every boot of `main` prints `pid=1: denied ChannelAccess(N)`, `Timeout test: unexpected result -6` and `Destroy test: unexpected result Err(-6)` (seen in the af59149 logs). `channel_create` (`kernel/src/ipc/mod.rs`) checks `ChannelCreate` and creates the channel, but grants its creator no `ChannelAccess`, so the self-test's `ipc_call` and `ipc_recv` on it return EPERM (-6). The capability-lifetime ADR (`docs/knowledge/decisions/2026-09-24-jl-capability-lifetime.md`) decides the fix; until it lands these lines are expected.

**Failure rates on `main`.** There is no recent clean baseline on disk. What exists:

| Run | Boots | Result |
| --- | --- | --- |
| Run 167, text, kernel at f0b4169 (2026-09-22), host load1 mean 9 to 24 | 20 | 12 CLEAN, 5 WEDGE (all heartbeat stuck at tick 0), 3 PCZERO (ticks 3000, 29000, 38000) |
| Run 167, gpu, same kernel | 10 | 0 CLEAN, 5 PANIC, 2 EXCEPTION, 2 WEDGE, 1 PCZERO |
| Three small text runs at 212df62 (2026-09-29), host load1 mean 83 to 224 | 16 | 2 CLEAN, 8 PCZERO, 5 WEDGE, 1 PANIC |
| One text run at af59149 (2026-10-10), load1 mean 193 | 2 | 1 WEDGE (tick 0), 1 EXCEPTION |

The kernel has changed a lot since f0b4169 (log rings, IPC and syscall hardening, the I/D cache sync, the boot-stack move), and the later runs were taken on a host running other soaks. Treat the run-167 rates as the shape of the problem, not as the current rate. Refresh them with a host-exclusive `just soak runs=20` before using a number in a gate. The boot race behind the tick-0 WEDGE and the PC=0 abort is the one in `docs/knowledge/decisions/2026-09-22-jl-crash-fix-preemption-and-fp.md`.

**gpu boots: a PANIC does not end the boot.** On `main`, gpu soak boots often hit `PANIC: panicked at kernel/src/mm/frame.rs:51:9` (`[mm] BUG: free_pages(...)`) right after heartbeat tick 0, during display handoff. The panic handler (`kernel/src/main.rs`) prints and halts the panicking CPU with IRQs still on, so the other CPUs keep running. In run 167's `main-gpu-r1`, three of five boots print `[bench] === Gate 1 Benchmark ===` and `Bench main: server ready, starting IPC benchmark` after the PANIC line; the bench then stalls. An early report said these boots "never reached the bench code"; a reviewer caught it from the line order.

**CPUs 1 to 3 take no timer IRQs.** Crash-fix step 1b found a per-CPU tick counter reading `tick=N,0,0,0` on every line. Two checks that do not depend on the counter agree: QEMU's `-d int` logged about a thousand IRQ exceptions on CPU 0 and none on CPUs 1 to 3, and the QEMU monitor showed `GICR_IGROUPR0` = 0xffffffff on CPU 0 and 0 on CPUs 1 to 3, with PPI 30 enabled and pending on all four. The cause is in the code at this commit: `init_gicv3_secondary` (`kernel/src/arch/aarch64/gic.rs`) enables PPI 30 but never writes the group, the kernel never enables Group 0, and edk2 groups only CPU 0's redistributor. The timer handler (`kernel/src/arch/aarch64/timer.rs`) runs the scheduler tick, `check_timeouts` and the load balancer on whichever CPU takes the IRQ, so with no IRQs on CPUs 1 to 3 they run on CPU 0 only; the tick counter and the heartbeat are CPU 0's by design. The capability-lifetime ADR records the owner decision that the fix is its own crash-fix step (#200). It was seen on Homebrew edk2 only; CI's Ubuntu QEMU and edk2 were not checked.

## Why it happened

Noise has no label. The boot race produces WEDGE, PCZERO and PANIC at a base rate that a small sample cannot tell from a regression, and a lock-order or preemption change moves the rate in either direction. Post-panic output looks like a second bug.

## What we learned

- A single WEDGE or PCZERO boot, or a streak of two or three, is not evidence against a change.
- The warnings above and the edk2 lines appear in every boot; their absence would be the surprise.
- Everything printed after a PANIC on a gpu boot is fallout from a still-running system. It is not "unreached code".
- Any acceptance criterion that needs timer IRQs on CPUs 1 to 3 cannot pass on this host until #200 lands.

## How to avoid next time

- Classify logs with `aios soak --classify LOG...` (or `just soak`); the classes and their precedence are in `aios soak --help`.
- Before calling a boot a regression, run the previous head through the same interleaved soak (see the A/B lesson, `2026-10-06-jl-ab-soak-method.md`).
- When describing a gpu `frame.rs:51` boot, grep the log for the bench header, `server ready`, the round-trip lines and any later `EXCEPTION[`, and report them with their line order.
- Before writing an acceptance criterion that needs ticks on CPUs 1 to 3, look at a fresh log's per-CPU tick values. To read the GIC from a scratch run, start QEMU with `-monitor tcp:127.0.0.1:PORT,server,nowait`: a unix socket path in a deep scratch directory exceeds macOS's 104-byte `sun_path` limit. Then `xp /1wx <address>` reads `GICR_IGROUPR0` for CPU n at the redistributor base (0x080A_0000 on QEMU virt) + n * 0x20000 + 0x10000 (the SGI frame) + 0x80.
- Do not fix the GIC group inside a detect-only step; it changes scheduling. Report it.
- Kernel log messages longer than 48 bytes now take a continuation entry (up to 96 bytes in all, `LOG_MSG_CAPACITY` and `LOG_CHAIN_CAPACITY` in `shared/src/observability.rs`), so an older note that self-test messages are cut at 48 bytes no longer holds.
