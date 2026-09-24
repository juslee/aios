---
author: jl + claude
date: 2026-09-22
tags: [kernel, sched, smp, ipc, security]
status: final
---

# ADR: Boot-crash fix — preemption, lock discipline and FP/NEON policy

The owner decided issue #164 on 2026-09-22: 1C (typed lock classes) and 2B (hard-float kernel with eager FP/SIMD save). See "Decision". Implementation is tracked by #164; the PR #149 merge gate is issue #165. The code was read at `main` f0b4169. `main` has since moved to 33c6b3d through four commits that touch no kernel code: dd05a1e (#170), bdbdded (#172) and 33c6b3d (#173) change only `.claude/settings.json`, and e98e1ad (#161) moves `rust-toolchain.toml` to nightly-2026-09-22. This branch is based on e98e1ad. Every `path:line` citation below therefore still holds at e98e1ad, except the `developer-guide.md` citations, which the 2026-09-24 amendment moved to their lines at b07d7e4. Every code claim comes from reading the source, and none has been confirmed at runtime. Claims marked *(likely)* or *(inference)* were not traced end to end. The fatal-report lines quoted below were read from the soak logs in `target/soak/167`. Nothing was built or booted while this ADR was written. On 2026-09-24 the owner amended the delivery plan (#185, #189); see the amendment under "Review notes".

-----

## Context

### Soak evidence

There are two measurements of `main`. Both used `scripts/soak-qemu.sh` on the local macOS host (QEMU TCG, 75 s per boot; run 167 used QEMU 11.1.1). They boot the dev-profile ELF (`justfile:5`, `:20-21`, `:32`), which is built at opt-level 1 with debug assertions (`Cargo.toml:13-14`).

| Run | Arm | Mode | Boots | CLEAN | WEDGE (stuck / alive) | PCZERO | PANIC | EXCEPTION |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline, ELF 59e095c0 | main | text | 20 | 12 (60%, Wilson 38.7–78.1%) | 6 | 2 | 0 | 0 |
| | main | gpu | 10 | 3 (30%, Wilson 10.8–60.3%) | 3 | 2 | 1 | 1 |
| `target/soak/167`, interleaved | main f0b4169 (ELF d9988962) | text | 20 | 12 | 5 (5 / 0) | 3 | 0 | 0 |
| | main | gpu | 10 | 0 | 2 (1 / 1) | 1 | 5 | 2 |
| | #161, nightly-2026-09-22 | text | 20 | 11 | 5 (4 / 1) | 4 | 0 | 0 |
| | #161 | gpu | 10 | 2 | 0 | 2 | 4 | 2 |

On GitHub (ubuntu-24.04, x86 TCG, 90 s per boot) one run had 0/5 CLEAN (4 WEDGE, 1 EXCEPTION) and a later run had 2/5. The M26 branch (PR #149) failed 10/10 in an earlier manual repro, and `COMPOSITOR_PRESENT_ENABLED=false` did not avoid it.

- The baseline run's logs are no longer on disk. `target/soak` holds only run 167, so the baseline's fatal reports cannot be re-read.
- Run 167 was the A/B soak for #161, which merged after it. The #161 arm is therefore the current `main` baseline. The two arms do not differ detectably: text 12 against 11 CLEAN, gpu 0 against 2.
- The gpu CLEAN rate is poorly determined. It was 3/10 in the baseline and 0/10 and 2/10 in run 167, which pools to 5/30.
- Host load was high and uneven during run 167. The 1-minute load at each boot's start ranged from 6.5 to 50 on a 10-CPU host (`summary.tsv` column 9).

What each class looks like:

- **WEDGE, heartbeat stuck.** The CPU 0 heartbeat stays at `tick=0`. In text mode all 9 such boots in run 167 stopped after the bench header, which prints 500 ticks after the bench thread first runs (`kernel/src/bench.rs:354-362`). So CPU 0 stopped taking ticks between tick 500 and tick 1000, while the bench was running.
- **WEDGE, heartbeat alive, bench incomplete.** CPU 0 heartbeats keep coming, up to tick 79000 on GitHub, but the bench never completes. After the last bench line come only heartbeats and "Load balance: migrated tid=N from CPU 0 to CPU k" lines (`kernel/src/sched/init.rs:265-271`). This was the GitHub signature. It also occurs locally: 2 of the 60 boots in run 167. They stop at different points:
  - main gpu r1 #05 stops, as on GitHub, after "Bench main: server ready, starting IPC benchmark" (`bench.rs:394`), at about 7 s.
  - #161 text r4 #02 completes the IPC benchmark and prints its result line (10000 iterations, `bench.rs:400-409`). It then stops before the context-switch result line (`bench.rs:416-425`).
- **PCZERO.** An EL1 instruction abort with ELR=0 and ESR 0x86000006 (EC 0x21, level-2 translation fault). All 10 in run 167 were on CPU 0.
- **PANIC and EXCEPTION.** These occurred in gpu mode only. The reports are in the table below.

Fatal reports on disk (`target/soak/167`, the `first_fatal` column of each `summary.tsv`):

| Boots | Report | Reading |
| --- | --- | --- |
| 8 of 20 gpu boots: main gpu r1 #01–#03, r2 #01, #05; #161 gpu r2 #01, #03, #04 | `PANIC: panicked at kernel/src/mm/frame.rs:51:9: [mm] BUG: free_pages(0x1500, N)`, with N from 0xFFFF_0000_000F_5990 to 0xFFFF_0000_000F_6B10 (main) and from 0xFFFF_0000_000D_3DA0 to 0xFFFF_0000_000D_4720 (#161). Every one fires right after heartbeat tick 0, after "Compositor: display handoff complete" | The address argument is 0x1500 in all 8. The `order` argument is a kernel-image address (the VMA base is 0xFFFF_0000_0008_0000). It moves by up to 4.5 KiB between boots and shifts with the build, so it is a return address or a boot-stack address (the 128 KiB boot stack is in the image, `kernel/src/arch/aarch64/linker.ld:67-74`), not an order. Each of the 8 logs also has two "VirtIO-GPU: error response 0x1203" lines (`VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID`, `shared/src/gpu.rs:45`), and no other gpu boot has them. So one release path (detach backing, unref, free) ran with a `GpuBufferHandle` whose `resource_id`, `fb_phys` and `order` were all wrong. No CLEAN gpu boot runs a release path after the handoff. The call site is not identified, because the panic prints no CPU, thread or backtrace |
| #161 gpu r1 #03 | `PANIC: panicked at kernel/src/ipc/channel.rs:249:29: index out of bounds: the len is 128 but the index is 1141767816` | 1141767816 is 0x440D_FE88, the low 32 bits of a direct-map pointer (`DIRECT_MAP_BASE` + physical 0x440D_FE88). It was used as the `ChannelId` in `ipc_recv` |
| main gpu r1 #04; #161 gpu r1 #05 | `EXCEPTION[CPU 0]: ESR=0x8a000000 EC=0x22 FAR=ELR=0xffff0001440b5f16` and `…0xffff0001440b6845` | PC alignment fault. The CPU branched to a misaligned direct-map data address (physical 0x440B_5F16 and 0x440B_6845), in the same 1 MiB physical region as the `ChannelId` above (0x440D_FE88) |
| main gpu r2 #03 | `EC=0x25 ESR=0x96000046 FAR=0xa ELR=0xffff0000000a2830` | A write to address 0xa (a null pointer plus 10) from a valid kernel PC, at tick 0 |
| #161 gpu r2 #05 | `EC=0x25 ESR=0x96000045 FAR=0xffe0e0e0 ELR=0xffff0000000a88e8` | A write to 0x0000_0000_FFE0_E0E0, which is `BOOT_LOG_FG`, the boot-log text colour (`shared/src/gpu.rs:371`). A 32-bit pixel value was used as an address, at tick 0 |
| 10 PCZERO boots | `EC=0x21 ESR=0x86000006 FAR=0 ELR=0` | As described above |

All 16 `EXCEPTION[CPU n]` lines in run 167 report CPU 0. None has an ELR or FAR in 0x4000_0000–0x7FFF_FFFF, which is where a physical-alias PC would land (N5). The PANIC boots ran on after the panic, because the panic handler leaves IRQs on (N8): the eight `frame.rs:51` boots to heartbeat tick 46000–54000, and the `channel.rs:249` boot to tick 36000.

**Classifier caveat.** CLEAN needs only heartbeat progress and "=== Gate 1 Complete ===" (`scripts/soak-qemu.sh:67-72`, `:205-206`). It does not need every IPC call to have succeeded. The bench counts only successful calls (`bench.rs:65-66`, `:241-246`) and prints that count (`bench.rs:402`). A boot that lost the bench server (N1 below) still finishes, late and with few samples, and is counted CLEAN. The CLEAN rates above are therefore upper estimates.

### What the code does today

1. **IRQ entry.** The EL1 IRQ entry pushes x0–x18, x29, x30 and an xzr pad (176 bytes) onto the interrupted thread's stack (`kernel/src/arch/aarch64/exceptions.rs:145-170`). It does not save ELR_EL1, SPSR_EL1, x19–x28 or FP/SIMD state. The lower-EL entries do save a full 272-byte `TrapFrame` including ELR and SPSR (`kernel/src/arch/aarch64/trap.rs:23-35`), but nothing reaches them, because no code enters EL0 (`CLAUDE.md:66`).
2. **Switching inside the IRQ.** `irq_handler_el1` acknowledges the interrupt, runs the timer handler, writes EOIR, then calls `check_preemption()` → `schedule()` before `eret` (`kernel/src/arch/aarch64/gic.rs:235-262`). The comment at `gic.rs:257-261` assumes the stack frame is enough to resume, but that frame has no ELR or SPSR.
3. **Global reschedule flag.** Every tick on every CPU sets the one global `NEED_RESCHED` (`kernel/src/arch/aarch64/timer.rs:115`, `:208`), and any CPU's `schedule()` clears it (`kernel/src/sched/scheduler.rs:162`). As a result `schedule()` runs from IRQ context on almost every tick.
4. **Context switch.** `save_context` and `restore_context` store and load only x19–x30 and SP, then `br` to the saved LR (`kernel/src/arch/aarch64/context_switch.S:23-56`).
   - `ThreadContext.pc` holds the resume LR. `save_context` stores x30 there (`context_switch.S:37`), `restore_context` branches to it (`:54-56`), and `assert_valid_ctx` checks it (`scheduler.rs:22-36`). `pstate`, `ttbr0`, `timer_cval` and `timer_ctl` are never applied (`kernel/src/task/mod.rs:77-84`).
   - A new thread starts directly at its entry function, with `gp_regs` all zero (`task/mod.rs:183-191`). Nothing runs between `restore_context` and the entry function.
   - `fp_context` is always `None` (`task/mod.rs:139`, `:192`), and no switch path saves V registers.
   - TPIDR_EL0 and TPIDR_EL1 are not used anywhere (searched: not found).
5. **Publish before save.** `schedule()` marks the next thread Running and sets `CURRENT_THREAD` while it holds `THREAD_TABLE`. It then drops the lock and saves and restores through raw pointers (`scheduler.rs:217-287`). No `on_cpu` or "context saved" flag exists (searched: not found).
6. **Locks and IRQ masking.**
   - All 39 static locks are `spin::Mutex` 0.12 (`kernel/Cargo.toml:12`), which neither masks IRQs nor disables preemption. No preempt count or IRQ-save primitive exists (searched: not found).
   - IRQ masking is done with hand-written `msr DAIFSet/DAIFClr`, in 41 asm sites across 10 files.
   - `thread_yield`, `block_current`, `try_direct_switch` and `try_reply_switch` unmask IRQs unconditionally on exit (`scheduler.rs:318`, `:347`; `kernel/src/ipc/direct.rs:71`, `:79`, `:91`, `:192`, `:220`, `:227`, `:239`, `:317`). Only `unblock` restores the previous state (`scheduler.rs:358-415`).
7. **Work done in the timer IRQ.** The handler runs `check_timeouts` on every CPU on every tick (`timer.rs:194`), plus notification timeouts and the load balancer (every 4 ticks, on every CPU). On CPU 0 it also drains the log and prints the heartbeat (`timer.rs:176-200`). Only CPU 0 advances `TICK_COUNT` (`timer.rs:176-178`), so a stall on CPU 0 freezes every IPC timeout in the system.
8. **Scheduling policy.** `pick_next` is strict class priority: RT, then Interactive, then Normal, then Idle (`kernel/src/sched/mod.rs:65-76`). `unblock` queues the woken thread on the CPU that runs `unblock` whenever affinity allows (`scheduler.rs:398-407`). Every service and bench thread has `CpuSet::all()` (`bench.rs:498`, `:519`, `:540`; `kernel/src/gpu/service.rs:619`; `kernel/src/compositor/service.rs:619`; `kernel/src/input/mod.rs:332`).
9. **Panic handler.** It prints `PANIC: <info>` and halts in a `wfe` loop, without masking IRQs and without printing the CPU or thread (`kernel/src/main.rs:383-397`).
10. **Floating point.**
    - The kernel builds for the hard-float `aarch64-unknown-none` target (`justfile:3`; `rust-toolchain.toml:3`), and FP/SIMD is left untrapped at EL1 on every CPU (`kernel/src/arch/aarch64/boot.S:83-89`, `:280-285`).
    - `kernel/` and `shared/` contain no `f32` or `f64` (searched: 0 matches).
    - A research pass disassembled an older debug ELF, not a soak binary. It found NEON in `memset`, `core::fmt::Formatter::pad_integral` and `log_impl`, all reachable from the timer IRQ through `drain_logs`, the heartbeat `writeln!` (`timer.rs:180`, `:186`) and the load balancer's `kinfo!`. #161 has since changed the toolchain, so that disassembly must be redone on the current ELF (step 1b).

### Hypotheses: verdicts

| # | Hypothesis | Verdict | Key evidence | Explains |
| --- | --- | --- | --- | --- |
| H1 | The IRQ frame lacks ELR/SPSR, but the IRQ path can switch threads | **Confirmed** (static) | `exceptions.rs:145-170`; `gic.rs:262`; `scheduler.rs:258`, `:280` | PCZERO; the bench thread silently becoming a copy of another loop (the heartbeat-alive hang); plausibly the PANIC and EXCEPTION reports, whose values are data and stack pointers where a PC, a `ChannelId` or a free address should be |
| H2 | No `on_cpu` handshake | **Confirmed, and wider than stated.** The load-balancer correlation looks like a symptom, not the cause | Four paths publish a thread before its save completes (F4 below). The balancer moves only `normal`-class threads (`init.rs:254`), and the bench threads are Interactive (`bench.rs:496-541`). They reach other CPUs through `unblock` instead (N7) | Two CPUs running on one stack once caller and server sit on different CPUs |
| H3 | `spin::Mutex` masks neither IRQs nor preemption | **Confirmed.** The main mechanism is the IRQ path re-entering a lock on the same CPU. Lock-holder preemption is secondary | The IRQ path blocking-locks `CURRENT_THREAD` and `THREAD_TABLE` (`scheduler.rs:164`, `:168`), `THREAD_TABLE` (`init.rs:257`), and `WAKEUP_ERRORS` → `THREAD_TABLE` → `RUN_QUEUES` (`kernel/src/ipc/timeout.rs:133`; `scheduler.rs:371`, `:407`). Thread code holds the same locks with IRQs on (`timeout.rs:118`, `:145`; `kernel/src/cap/mod.rs:39`, `:50`) | Heartbeat-stuck WEDGE |
| H4 | `NEED_RESCHED` is global | **True, but not a crash cause** | `timer.rs:115`, `:208`; `scheduler.rs:162` | Widens the H1 and H3 windows, because `schedule()` runs from the IRQ on every tick |
| H5 | No FP/NEON save on a hard-float kernel | **Confirmed defect. Live candidate for PANIC and EXCEPTION, not ranked below H1** | No V-register save at IRQ entry (`exceptions.rs:147-157`) or at any switch (`context_switch.S:24-56`; `task/mod.rs:139`, `:192`). `check_timeouts` zero-fills a 1 KiB `[(ThreadId, i64); 64]` array (`timeout.rs:89`) on every tick on every CPU where its `try_lock` succeeds (`:82`). Thread code copies `Copy` structs of 16 bytes or more by value, such as `GpuBufferHandle` (`shared/src/gpu.rs:331-351`), taken out of an `Option` (`kernel/src/drivers/virtio_gpu.rs:975`) or returned by value (`gpu/service.rs:79`) before `free_dma_pages(handle.fb_phys, handle.order)` (`virtio_gpu.rs:982`; `gpu/service.rs:293`, `:312`, `:418`, `:439`, `:445`; `compositor/service.rs:510`). That the zero-fill and these copies use V registers on the current toolchain is expected but unconfirmed (step 1b objdump) | A tick between the loads and stores of a copy that goes through several q registers corrupts the whole copy, and a switch hands one thread's V state to the next. This could produce the `frame.rs:51` PANIC. The zero-fill alone would leave zeros, and none of the corrupted values on disk is zero, so the reports fit a wrong frame (H1, H2) at least as naturally. Step 3 against step 4 on the `frame.rs:51` PANIC (8 of 20 gpu boots) decides |

New findings, not in the original list:

| # | Finding | Evidence | Effect |
| --- | --- | --- | --- |
| N1 | `try_direct_switch` sets the receiver Runnable, not Running, and never queues it. If a tick lands while the receiver runs, `schedule()` takes its "blocked or dead" branch. The receiver is then Runnable, in no queue and current nowhere, and `unblock` ignores Runnable threads | `direct.rs:135`; `scheduler.rs:182-184`, `:375-383` | Two variants. (a) The orphaning tick lands before `ipc_reply` takes the caller: each later call times out after 1000 ticks (`bench.rs:241`), and once 16 requests are unconsumed the ring is full (`shared/src/ipc.rs:15`; `kernel/src/ipc/channel.rs:86-88`). The degraded run is counted CLEAN. (b) The tick lands after `ipc_reply` has taken `pending_caller` (`channel.rs:374`) and cleared the caller's timeout (`channel.rs:398`), but before `try_reply_switch` masks IRQs (`direct.rs:210`). The caller then stays BlockedIpc with no timeout entry and no waker, and the orphaned server never replies. Heartbeats continue and the bench never finishes: the heartbeat-alive signature, with no cross-CPU step. A Runnable-orphan scan misses (b), because the caller is Blocked |
| N2 | Reply before block. `ipc_call` publishes `pending_caller` and its timeout (`channel.rs:91`, `:147-176`) before `block_current` sets BlockedIpc (`scheduler.rs:338`). A replier on another CPU clears the timeout (`channel.rs:398`) and sees the caller still Running, so `try_reply_switch` fails (`direct.rs:234-241`). The fallback `unblock` then does nothing (`channel.rs:409`; `scheduler.rs:375-383`). The caller blocks with no waker and no timeout | as cited | Heartbeats continue but the bench never finishes |
| N3 | The same register-then-block gap exists in `ipc_recv` (`channel.rs:278` vs `:293`) and in `sleep_ticks` (`timeout.rs:176-184`). A notification deadline is cleared and then dropped when `THREAD_TABLE.try_lock` fails (`kernel/src/ipc/notify.rs:317`, `:321-323`) | as cited | IPC stalls until the timeout fires; notification waits can be lost for good |
| N4 | Per-CPU ids are read in preemptible code. `direct.rs:55` and `:206` read `core_id` before masking IRQs (`:59`, `:210`) and later write `CURRENT_THREAD[cpu]` and `RUN_QUEUES[cpu]` (`:153`, `:285`, `:290`). `current_thread_id` reads `core_id` and then takes the lock (`timeout.rs:117-118`), and so do `cap::current_process_id` (`cap/mod.rs:49-50`) and the dead-thread exits of the echo server, GPU service and compositor (`kernel/src/service/mod.rs:315-316`; `gpu/service.rs:178-179`; `compositor/service.rs:187-188`) | as cited | After a migration, another CPU's slot can be written |
| N5 | CPUs 1–3 install VBAR_EL1 with the MMU off, so it holds a physical address (`boot.S:287-291`). They never reinstall it: `install_vector_table` is called only from `kernel_main` (`main.rs:75`), never from `secondary_main` (`kernel/src/smp.rs:184-220`). Every IRQ on CPUs 1–3 therefore runs at physical-alias PCs, and `adrp` there yields physical addresses. A thread switched out inside such an IRQ saves a physical-alias PC and return addresses, and may hold physical-alias static pointers in x19–x28. CPU 0 leaves TTBR0 on user address space B after the boot test (`main.rs:271`), where none of those addresses are mapped. **Possible; not observed** | as cited; none of the 16 EXCEPTION lines in run 167 has a PC or FAR in 0x4000_0000–0x7FFF_FFFF | An instruction or data abort at a physical address when such a thread resumes on CPU 0. Kept as a latent defect, counted in step 1b and fixed in step 5 |
| N6 | The load balancer calls `kinfo!` from IRQ context (`init.rs:265`), which adds a second producer to the per-core single-producer log ring (`kernel/src/observability/mod.rs:62-64`). `timer.rs:150-151` says the IRQ path must not log | as cited | Garbled or lost log entries; no crash |
| N7 | Timeouts move threads between CPUs. `check_timeouts` runs on every CPU on every tick (`timer.rs:194`) over the single global `TIMEOUT_QUEUE` (`timeout.rs:25-28`, `:82`), and `unblock` queues the woken thread on the CPU that runs it (`scheduler.rs:398-407`). CPU 0 checks right after advancing `TICK_COUNT` (`timer.rs:178`, `:194`), so it usually processes an expiry. Another CPU processes it when CPU 0's `try_lock` fails or when CPU 0's handler is slow (log draining, the heartbeat). The same move happens on the `ipc_reply` fallback (`channel.rs:409`) and on the other wake paths. The bench server re-arms a 100-tick receive timeout on every `ipc_recv` (`bench.rs:188`), and every bench `ipc_call` arms 1000 ticks (`bench.rs:230`, `:241`) | as cited | After a gap of 100 ms or more in the bench's calls, which is more likely on a slow host, the server can land on another CPU. From then on every round trip is cross-CPU and exposed to N2, N4 and F4 path 4 (`try_reply_switch` restores a caller from another CPU with no CPU check, `direct.rs:231-243`, `:312`). This is the likely link between slow hosts and the heartbeat-alive WEDGE *(inference)* |
| N8 | The panic handler leaves IRQs on and halts in a `wfe` loop (`main.rs:383-397`). The timer IRQ keeps running on the panicking CPU, and `check_preemption` can switch the panicking thread out. That thread keeps any lock it holds, and the report names no CPU or thread | as cited; the 8 `frame.rs:51` PANIC boots ran on to heartbeat tick 46000–54000 | Later symptoms in a PANIC boot are fallout, and the report cannot be tied to a CPU or thread |
| N9 | CPUs 1–3 run for their whole life on the boot identity map as TTBR0 (`boot.S:310-314`). `secondary_main` and `install_kmap_ttbr1_secondary` change only TTBR1 (`smp.rs:184-220`; `kernel/src/mm/kmap.rs:32-49`). The map's RAM block at 0x4000_0000 is an executable 1 GiB block with no AP bits, so it is RWX (`kernel/src/arch/aarch64/mmu.rs:152-155`) | as cited | A W^X violation against rule 01 (`.claude/rules/01-code-conventions.md:21`). It is also why N5's physical-alias PCs execute on CPUs 1–3 instead of faulting |
| N10 | During the bench, Normal-class threads queued on CPU 0 do not run. `bench-yield` loops on `thread_yield` forever (`bench.rs:206-216`) and is queued on CPU 0 (`bench.rs:524`). Bench main and the server also spin on `thread_yield` while they wait (`bench.rs:164-170`, `:354-359`), and strict class priority always picks an Interactive thread first (`sched/mod.rs:65-76`). Only the balancer's migrations free a Normal thread (`init.rs:254`) | as cited. The starvation follows from the code; a starved thread holding a lock the bench needs has not been identified *(inference)* | A Normal thread that was preempted on CPU 0 while holding a lock the bench needs stalls the bench until the balancer moves it, with heartbeats alive. This is a fifth candidate for the heartbeat-alive WEDGE, and it matters for moving lock classes after the gate (Decision 1) |

### Likely mechanism for each failure class

| Class | Most likely mechanism | Supporting detail |
| --- | --- | --- |
| PCZERO (CPU 0) | H1, in four steps. (1) A thread switched out inside the IRQ path waits to run again. (2) Before it does, another IRQ on the same CPU loads a foreign address into ELR_EL1, often a function epilogue. (3) The first thread resumes and `eret`s to that foreign address. (4) The epilogue reloads x30 from a slot in the wrong stack frame and returns. A zero word in that slot gives PC=0: the zeroed `recv_buf` (`bench.rs:226`), the xzr pad (`exceptions.rs:157`), or the zero frame record that every new thread starts with (`task/mod.rs:184`) | Direct switches hand out zero time slices (`direct.rs:143`, `:276`), and `schedule()` does not refill a slice when it picks a thread (`scheduler.rs:235-237`). So bench threads are often preempted from the IRQ path. A tick that went pending while IRQs were masked is delivered right after the `DAIFClr` in `thread_yield`, `block_current` or `try_reply_switch`. ESR 0x86000006 is a level-2 translation fault, which fits CPU 0: its TTBR0 is user address space B, mapped with 4 KiB pages from `USER_DATA_BASE` (`main.rs:271`; `kernel/src/mm/uspace.rs:33`). On CPUs 1–3, VA 0 is a PXN device block (`mmu.rs:86-96`, `:151`), which would give a permission fault. *(likely)* |
| WEDGE, heartbeat stuck | H3. A tick arrives while thread code on CPU 0 holds `CURRENT_THREAD`, `THREAD_TABLE` or `WAKEUP_ERRORS` with IRQs on. The IRQ path then spins on the same lock forever with IRQs masked, and `TICK_COUNT` stops | The window is open whenever thread code on CPU 0 holds one of these locks with IRQs on: every `ipc_call`, `ipc_recv` and `ipc_send` takes `CURRENT_THREAD` in `current_thread_id` (`timeout.rs:117-118`) and `THREAD_TABLE` in `process_of_thread` (`cap/mod.rs:39`); a round trip takes `CURRENT_THREAD` at least 3 times and `THREAD_TABLE` twice (`channel.rs:44`, `:50`, `:237`, `:243`, `:361`). In gpu mode that starts at the first tick, because the GPU service, compositor and input threads run on CPU 0 (`main.rs:303-341`; `gpu/service.rs:624`; `compositor/service.rs:624`; `input/mod.rs:336`). In text mode all 9 heartbeat-stuck boots in run 167 stopped after the bench header, so the bench's IPC dominates there. A naive exposure estimate (several short holds per round trip over hundreds of ticks) predicts a higher WEDGE rate than the observed 20–30% of text-mode boots. Either TCG's interrupt delivery shrinks these windows, or H3 is not the whole story. Step 1b's named-lock detector decides, not this static argument |
| WEDGE, heartbeat alive, bench incomplete | One of five: (a) N2, a lost wakeup after a cross-CPU reply; (b) N1's variant (b), with no cross-CPU step; (c) H1 hijacking a bench thread into a stack-free loop (bench-yield's loop, `bench.rs:213-215`, or an idle `wfe` loop); (d) H3 on a secondary CPU k's `CURRENT_THREAD[k]`, which CPU 0 never takes; (e) N10, a starved lock holder. N7 raises the exposure for (a) and (d) on slow hosts | The balancer lines need `THREAD_TABLE` (`init.rs:257`), so a `THREAD_TABLE` deadlock is ruled out for this mode. Step 1b's counters separate the five: per-CPU tick counts (d), the orphan and blocked-with-no-waker scans (a, b), the ELR mismatch counter (c), the starvation scan (e), and the cross-CPU wake counts (N7) |
| PANIC (`frame.rs:51`, `channel.rs:249`), EXCEPTION | Register or stack corruption. The values on disk are direct-map pointers, a kernel-image address, a null-plus-offset and a pixel colour, found where a PC, a `ChannelId`, a store address or a free address should be. That fits resuming with another context's registers or frame: H1, or H2 (two CPUs on one stack). H5 is not excluded (see its row). N5 is not observed | The `frame.rs:51` PANIC recurs at one program point in 8 of 20 gpu boots, which makes it the best discriminator between steps 3 and 4. Step 1b adds `#[track_caller]` to `free_dma_pages` so the report names the call site |

The gpu mode queues more Interactive threads on CPU 0 (`gpu/service.rs:624`, `compositor/service.rs:624`, `input/mod.rs:336`). That raises exposure to H1, H3 and N1 alike, which fits the lower gpu CLEAN rate and M26's 10/10 failures. This is inference; the M26 branch was not read.

### What the architecture docs already say

The design already asks for most of direction D; the code has drifted from it:

- `docs/kernel/scheduler.md` §4.1 (lines 441-522) puts ELR (`pc`) and SPSR (`pstate`) in `ThreadContext`.
- §10.2 (line 1922) and `docs/kernel/hal.md` §8.3 (lines 1099-1118) set the reschedule flag only when the time slice expires, and switch at the next safe point.
- `scheduler.md` §10.3 (lines 1926-1951) takes spinlocks with `local_irq_disable() + spin_lock()`. `docs/kernel/deadlock-prevention.md` §9.2 (lines 395-406) repeats §10.3's list, in which spinlock critical sections run with preemption disabled.
- `deadlock-prevention.md` §12 rule 10 (line 513) says never spin on a lock from IRQ context. The code does.
- `scheduler.md` §3.3 (lines 340-343) makes the reschedule flag per thread (`preemption_flag`), not per CPU.
- `scheduler.md:472` specifies lazy FP save through a CPACR trap. It is not implemented, and it contradicts `docs/kernel/boot/kernel.md:95` ("Rust's codegen freely uses NEON"). Under Decision 2B the eager save replaces it (step 4), and `boot/kernel.md:95` stands.
- `docs/phases/03-ipc-and-capability-system.md` ticks as done several items the code does not have: the ELR/SPSR save (lines 123, 166), the direct-switch CPACR trap (line 212) and lazy FP (line 377).
- `docs/project/ai-agent-context.md:170-184` teaches the unconditional `DAIFSet`/`DAIFClr` pattern.
- `deadlock-prevention.md` §3.3 lists `RUN_QUEUES` and `NOTIFY_DEADLINES` as leaf locks (lines 127-137). In the code, `try_load_balance` takes `THREAD_TABLE` while holding `RUN_QUEUES` (`init.rs:237-257`), and `check_notification_timeouts` calls `unblock` while holding `NOTIFY_DEADLINES` (`notify.rs:310-349`).

In `docs/knowledge/`, no ADR, lesson or discussion covers IRQ masking, preemption at IRQ exit, lock classes or FP state. The one related note, [the 2026-03-05 spinlock-safety lesson](../lessons/2026-03-05-jl-smp-spinlock-safety.md), covers only the rule that spinlocks and atomic read-modify-write hang on Non-Cacheable memory. The new lock types must keep that rule: no RMW on NC memory before M8's WB upgrade.

-----

## The fixed foundation

Direction D is right in principle. These items are not up for decision. The list corrects D where the research disagrees with it and adds what D leaves out.

**F1. Full exception frame on EL1 IRQ entry.** Save x0–x30, ELR_EL1 and SPSR_EL1 in the existing 272-byte `TrapFrame` layout (`trap.rs:23-35`), and restore all of it before `eret`. This is required by H1. The frame then travels with the thread's stack, which makes a switch inside the IRQ path correct. Under Decision 2B, step 4 adds the 528-byte FP/SIMD area to this frame, for 800 bytes.

**F2. One preemption point: `irq_exit`.**

- `irq_handler_el1` no longer switches threads.
- After EOI, which already comes before any switch (`gic.rs:255`), the stub calls `irq_exit(frame)`.
- `irq_exit` calls `schedule()` only when the reschedule flag is set and the interrupted context is preemptible. Until the preempt count exists (step 8, Decision 1C), "preemptible" means only "not `IN_SCHEDULER` on this CPU" (`scheduler.rs:156-159`), so a holder of one of the 30 non-IRQ locks can still be preempted there, as today.
- IRQs stay masked across the switch, and the resumed thread returns through its own frame.
- Voluntary switches (`thread_yield`, `block_current`, the IPC direct switches) keep their entry points. F4 and F6 change their bodies.

**F3. Reschedule flag.** This corrects D.

- Making the flag per-CPU is cheap but changes little (H4). What matters is when it is set: only when the current thread's time slice expires, or when a wakeup targets this CPU with a higher class. Setting it on every tick (`timer.rs:208`) is the bug.
- Store it per thread, in the `ThreadInfo` block described under Decision 1. For the running thread that is the same as per-CPU. It also matches `scheduler.md` §3.3 and the inference preemption points (`scheduler.md:1087`).
- A per-CPU array is the interim (step 6a) until `ThreadInfo` exists (step 8).
- Also refill the time slice when a thread is picked with 0 left. Zero-slice donations (`direct.rs:143`, `:276`) are why the bench threads are preempted from the IRQ path so often.

**F4. `on_cpu` handshake on every path that publishes a thread, not only the load balancer.** Four paths make a thread visible to other CPUs before its context is saved:

1. `schedule()` queues a thread whose slice expired (`scheduler.rs:177`) before saving it (`:258`).
2. `try_reply_switch` queues the replier (`direct.rs:290`) before saving it (`:301`).
3. `block_current` sets the Blocked state (`scheduler.rs:338`). `unblock`, `try_direct_switch` and `try_reply_switch` on other CPUs can see that state before the save.
4. The direct and reply switches pick their target by state alone, with no check of CPU or save completion (`direct.rs:86-87`, `:234-235`), even though `direct.rs:197-201` documents a same-CPU requirement.

The mechanism:

- Each thread gets an atomic `on_cpu`, kept outside `THREAD_TABLE`: a static per-thread-slot array at first, moved into `ThreadInfo` in step 8. The CPU that switches to a thread sets it.
- `finish_switch()` runs on the incoming thread's stack once the outgoing save is complete, and clears the outgoing thread's `on_cpu` with Release ordering.
- **Every resume point runs `finish_switch()`, including a new thread's first run.** Today a new thread starts at its entry function (`task/mod.rs:186`), so nothing would clear the previous thread's `on_cpu`, and every later restore of that thread would wait forever. New threads therefore start in a `ret_from_switch` trampoline that calls `finish_switch()` and then the entry function. Under Decision 1C the trampoline also sets TPIDR_EL1 (step 8).
- **Where the wait happens.** Before a thread is restored or queued on another CPU, the waker waits with Acquire ordering until its `on_cpu` is 0. The waker must hold no lock that the switching CPU needs to finish its switch: `THREAD_TABLE`, `RUN_QUEUES` or `CURRENT_THREAD`. Linux's `try_to_wake_up` likewise waits on `on_cpu` before it takes the run-queue lock (prior art from the research pass, not re-read here). This matters because `unblock` holds `THREAD_TABLE` from `scheduler.rs:371` to `:396` and runs from IRQ context (`check_timeouts` → `wake_with_error` → `unblock`, `timeout.rs:105`, `:140`), while a thread that has just set Blocked still needs `THREAD_TABLE` (`scheduler.rs:168`, `:218`) before its save. A wait inside that critical section would deadlock both CPUs, with IRQs masked on the waiter. So `unblock` reads the state, drops `THREAD_TABLE`, waits, then re-takes the lock and re-checks.
- **The wait is bounded.** After a fixed number of iterations it panics with the thread id, so a stuck `on_cpu` becomes a named PANIC, not a WEDGE.
- The IPC direct switch does not wait; it falls back to the scheduler path.
- The load balancer skips threads whose `on_cpu` is set.
- `finish_switch()` also queues a preempted previous thread, so the running thread is never in a run queue (seL4's "Benno" invariant). That removes paths 1 and 2 at their source.

**F5. Wake/block protocol (new; N1–N3, N7).** Neither the frame nor `on_cpu` fixes these:

- **N1:** `try_direct_switch` marks the receiver Running. `schedule()` decides whether to queue the previous thread from "it was current and is not blocking", not from `state == Running`. This removes both N1 variants.
- **N2 and N3:** a thread records "about to block" before it publishes itself as a waiter (in `pending_caller`, `waiting_receiver`, a timeout or a notification waiter). A waker that finds its target not yet blocked leaves a wake token, and `block_current` consumes the token instead of sleeping. Linux does the same by calling `set_current_state` before checking the condition. This is the register-then-block rule already stated in `docs/project/developer-guide.md:2060`. Since the 2026-09-24 amendment, the capability-lifetime PR delivers this bullet at every blocking site, not step 6b (see "Review notes").
- **`notify.rs:317` and `:323`:** clear a deadline only after the wake succeeds. The issue goes away once that lock is IRQ-class and taken with a blocking lock. Since the 2026-09-24 amendment, the capability-lifetime PR delivers this bullet, by that ADR's choice: `check_notification_timeouts` calls `unblock` only after releasing `NOTIFY_DEADLINES` and no longer drops a cleared deadline (see "Review notes").
- **N7:** `unblock` prefers the thread's last CPU, recorded in its `SchedEntity` at switch time, unless affinity forbids it. Timeouts then no longer scatter Interactive threads across CPUs. The bench server's 100-tick receive timeout (`bench.rs:188`) is the knob that sets how often that happens on a slow host.
- **N10, only if step 1b's starvation scan implicates it:** `bench-yield` blocks until the context-switch benchmark starts and exits after it, so it stops monopolising CPU 0.

**F6. Nested IRQ masking and per-CPU reads (new).**

- Replace the unconditional `DAIFClr` exits with a restore of the previous state. That is 10 of the 15 `DAIFClr` sites in `scheduler.rs` and `direct.rs`: `scheduler.rs:318`, `:347` and `direct.rs:71`, `:79`, `:91`, `:192`, `:220`, `:227`, `:239`, `:317`. `unblock`'s 3 already restore. `enter_scheduler`'s 2 (`scheduler.rs:60` after `SCHED_READY`, `:102` in the idle wait) are legitimate unconditional unmasks and stay.
- Read `core_id` and `CURRENT_THREAD` only inside non-preemptible regions. The sites are `direct.rs:55`, `:206`; `timeout.rs:117-118`; `cap/mod.rs:49-50`; `service/mod.rs:315-316`; `gpu/service.rs:178-179`; `compositor/service.rs:187-188`. A type change alone does not fix `CURRENT_THREAD[cpu].lock()`, because the index is computed before the guard masks IRQs. The per-CPU arrays (`CURRENT_THREAD`, `RUN_QUEUES`) therefore get an API that reads `core_id` inside the IRQ-off section, such as `CURRENT_THREAD.with_this_cpu(|slot| ...)`. Every `CURRENT_THREAD` and `RUN_QUEUES[cpu]` site is in step 2's scope.
- The bench's own DAIF masking (`bench.rs:179`, `:237`, `:250`) is removed in step 1b, so every later pair runs the same workload. It is nearly a no-op already: the first IPC call unmasks IRQs again (`direct.rs:192`).

**F7. Address-space hygiene (new; N5, N9).**

- `secondary_main` calls `install_vector_table` (`exceptions.rs:253`) before anything unmasks IRQs on that CPU (`scheduler.rs:60`).
- Once every CPU's VBAR is virtual, retire the identity map on every CPU. Install an all-invalid TTBR0 table, as Linux does with its reserved page table, on CPU 0 (replacing address space B from `main.rs:271`) and on CPUs 1–3 (replacing the RWX identity map from `boot.S:310-314`). Do not use TCR_EL1.EPD0, because CLAUDE.md forbids changing TCR with the MMU on (`CLAUDE.md:72`). VA-0 fetches and physical-alias PCs then fault loudly on every CPU with the same ESR: a level-0 translation fault, 0x86000004 instead of 0x86000006. The classifier keys PCZERO on ELR or FAR being 0 (`soak-qemu.sh:35-39`), so the class is unchanged.
- Do **not** reinstall the boot identity map on CPU 0. Its RAM block is RWX (`mmu.rs:152-155`), which would break W^X and let physical-alias PCs execute silently.
- Extend `assert_valid_ctx` (`scheduler.rs:22-36`) to the direct-switch restores (`direct.rs:186`, `:312`) and to a check that the saved PC is a kernel VA.
- "Core N online" prints VBAR_EL1 and TTBR0_EL1, and asserts that VBAR is a TTBR1 VA and TTBR0 is the invalid table (N9's tripwire).

**F8. No logging from IRQ context (new; N6).** The load balancer counts migrations instead of calling `kinfo!`. Log draining on CPU 0 stays as it is (`timer.rs:169-181`).

**Considered and deferred:**

- Deriving time from `CNTPCT_EL0`, so that a CPU 0 stall no longer freezes every timeout.
- A softirq-style deferred context for timeouts and load balancing.

Both reduce the blast radius of a failure, but neither is needed to fix the classes above.

-----

## Decision 1: lock discipline

### Common to all options

- **The IRQ-shared set is 9 statics.** The IRQ path blocking-locks `THREAD_TABLE`, `CURRENT_THREAD[8]`, `RUN_QUEUES[8]` and `WAKEUP_ERRORS`. It try-locks `TIMEOUT_QUEUE`, `NOTIFY_DEADLINES`, `NOTIFICATION_TABLE`, `SELECT_WAITERS` and `BOOT_LOG` (`scheduler.rs:119`, `:124`, `:164`, `:168`, `:177`, `:193`; `init.rs:211`, `:237-257`; `timeout.rs:82`, `:133`; `notify.rs:310`, `:321`, `:329`, `:346`; `observability/mod.rs:393`). The other 30 statics are never taken from IRQ context. This comes from the research inventory; the listed sites were spot-checked.
- **Rule: a lock taken in IRQ context is never taken with IRQs enabled.** This is Linux's lockdep rule. Every option must enforce it for the 9 statics. Once it holds, the IRQ path can take those locks blocking, because any wait is on another CPU. The "`try_lock` and skip" rule (`deadlock-prevention.md:513`) and the events it drops (`notify.rs:317`, `:323`) then go away.
- **Rule: never take a non-IRQ lock while holding an IRQ-class lock.**
- **A total order for the 9 IRQ-class locks.** Blocking acquisitions in IRQ context are deadlock-free across CPUs only if these locks are always taken in one order. Step 2 turns today's `try_lock` skips into blocking waits, which is exactly when a hidden ABBA would start to deadlock. The nestings in the code today are:
  - `RUN_QUEUES` → `THREAD_TABLE` in the balancer (`init.rs:237-257`);
  - `THREAD_TABLE` → `CURRENT_THREAD` (`scheduler.rs:218-244`; `direct.rs:61-153`, `:212-285`);
  - `NOTIFY_DEADLINES` held across `THREAD_TABLE`, `NOTIFICATION_TABLE`, `SELECT_WAITERS` and `unblock`'s `THREAD_TABLE` and `RUN_QUEUES` (`notify.rs:310-349`).

  A candidate order is `NOTIFY_DEADLINES` > {`NOTIFICATION_TABLE`, `SELECT_WAITERS`} > `RUN_QUEUES` (ascending CPU) > `THREAD_TABLE` > `CURRENT_THREAD`, with `TIMEOUT_QUEUE`, `WAKEUP_ERRORS` and `BOOT_LOG` placed from their call sites. The step-2 PR derives the final order from the code, not from `CLAUDE.md:98-102`. That list puts the non-IRQ `CHANNEL_TABLE` below the IRQ-class `NOTIFICATION_TABLE`. Either that nesting does not occur in the code (check it), or `CHANNEL_TABLE` joins the IRQ class. A per-CPU held-rank check on the IRQ-class type arrives in step 2, not later, and stays on in release builds. The §3.3 table in `deadlock-prevention.md` is rebuilt in the same PR.
- **`IrqSpinLock` masks IRQs itself.** It saves DAIF through a per-CPU IRQ-off nesting counter and restores it on the outermost release. A check that "an IRQ-class lock is taken with IRQs enabled" could therefore never fire. The checks are the held-rank order (step 2) and, from step 8, the class checks on `SpinLock` (Decision 1C).
- **Rust guard pitfall.** A guard that stores its own saved DAIF breaks when guards are dropped out of order (Asterinas #1120 and #3896; the Rust-for-Linux `SpinLockIrq` discussion). Use a per-CPU IRQ-off nesting counter, which is safe to index while IRQs are off, or a closure API.
- **Switch paths.** `scheduler.rs:247` and `direct.rs:157` and `:287` drop `THREAD_TABLE` before save/restore. With an IRQ-restoring guard, that drop would unmask IRQs before the switch. Each switch therefore needs one explicit IRQ-off span that ends only when the resumed thread restores its own state.
- **New statics must choose a class.** Step 2 adds a `clippy.toml` with `disallowed-types = ["spin::Mutex"]` for `kernel/`, or an equivalent CI grep. The 30 existing declarations carry an explicit `allow` until step 8, so a new static, including PR #149's shell locks, fails lint until it picks a class.
- **Step 2 is the same under A, B and C:** the IRQ class for the 9 statics, plus F6 and F8. It is the heartbeat-stuck WEDGE fix. The decision changes only how the other 30 statics are handled (step 8).

**Blast radius** (counted at f0b4169):

- 39 static `Mutex` declarations.
- 239 `.lock()` and 13 `.try_lock()` call sites in `kernel/src`, tests and bench included.
- Named call sites of the four IRQ-spun locks, 52 in total:

  | Lock | `.lock()` | `.try_lock()` |
  | --- | --- | --- |
  | `THREAD_TABLE` | 21 | 2 |
  | `CURRENT_THREAD` | 15 | 1 |
  | `RUN_QUEUES` | 7 | 4 |
  | `WAKEUP_ERRORS` | 2 | 0 |

  The `RUN_QUEUES` row includes the two calls split across lines at `init.rs:53-55` and `:88-90`, and the balancer's iterator `try_lock` at `init.rs:211`. `RUN_QUEUES` is also reached through `enqueue_on_cpu` (`sched/mod.rs:95-97`). Grep-based counts miss multi-line calls, so the type change must be checked by the compiler, not by these counts.
- 41 inline `DAIFSet`/`DAIFClr` asm sites in 10 files, 21 of them in `scheduler.rs` and `direct.rs`.
- If the new lock types keep the `lock()`, `try_lock()` and `Deref` API, most call sites need only a type change at the declaration. The per-CPU arrays are the exception (F6).

### Decision 1 options

Option A is #164's 1A, and option B is #164's 1B. Option C is added here. The owner chose C (see "Decision"); A and B are kept as the analysis behind that choice.

| | **A: IRQ-off spinlocks everywhere** | **B: preempt count; IRQ-off only for the IRQ-shared set** | **C: Linux-style, class in the type** |
| --- | --- | --- | --- |
| Mechanism | Every spinlock saves DAIF, masks IRQs and restores DAIF. With preemption only at `irq_exit`, IRQs off means not preemptible, so no counter is needed | Every lock bumps a preempt count, and `irq_exit` switches only at 0. IRQs stay on under ordinary locks, and the 9 IRQ-shared statics also mask IRQs | Two types, chosen at declaration: `SpinLock` (preemption off) and `IrqSpinLock` (IRQs off and preemption off). A validator checks the class rules |
| Architecture docs | Match `scheduler.md` §10.3 and `deadlock-prevention.md` §9.2 as written | Amend §10.3 (owner approval, per #164) | Amend §10.3 to add a second class |
| New infrastructure | IRQ-off nesting counter; a blocking mutex (wait queue) for long holds | Per-thread `ThreadInfo` reached through TPIDR_EL1, which is unused today | `ThreadInfo` plus the validator |
| Statics reclassified | All 39 become IRQ-off; long-hold locks become a blocking mutex | 39 become preemption-off; 9 of them also mask IRQs | 9 `IrqSpinLock`, 30 `SpinLock` |
| IRQ latency | The longest hold of any lock | The longest hold of an IRQ-shared lock (scheduler paths, which are short) | Same as B |
| Long holds | `VIRTIO_GPU` is held (`kernel/src/drivers/virtio_gpu.rs:909`) across a poll loop of up to `POLL_TIMEOUT` = 10,000,000 iterations (`kernel/src/drivers/virtio_common.rs:13`; `virtio_gpu.rs:378-391`). Under A that poll runs with IRQs off | The same poll runs with preemption off, which blocks the scheduler on that CPU for the whole poll | Same as B |
| Lock-holder preemption | Impossible | Impossible (the count blocks it) | Impossible |
| IPC direct switch | Explicit IRQ-off span across the switch | Same, plus an assertion that the count is only the switch's own | Same as B, checked by the validator |
| Load balancer | Its blocking `THREAD_TABLE` lock in the IRQ (`init.rs:257`) becomes safe, waiting only on other CPUs. It still needs the `on_cpu` skip (F4) | Same, since `THREAD_TABLE` is IRQ-shared | Same |
| Main failure mode | A long IRQ-off hold on CPU 0 stops `TICK_COUNT`, which freezes every IPC timeout: the WEDGE symptom again | The IRQ-shared set is implicit, so missing one lock recreates today's WEDGE exactly. The count can also leak on early-return paths | Two types to choose between for every new static. The validator catches a wrong class at the first acquisition in the wrong context, in release builds too, because each check is a per-CPU counter read. C needs the most code |

Under every option, `VIRTIO_GPU` needs a sleeping lock or a poll that yields. Until then it breaks `deadlock-prevention.md` §12 rule 8 ("Spinlock hold times must remain under 1 μs", line 511). This is listed as a known violation, not as a cost of A alone.

Where the preempt count lives matters for B and C. A per-CPU counter indexed by `core_id` has a migration race on aarch64: a thread reads MPIDR, is preempted and moved, and then increments another CPU's counter. x86 Linux avoids this with a single `%gs`-relative instruction. arm64 Linux keeps `preempt_count` in `thread_info` for exactly this reason. B and C therefore need a per-thread count, or IRQs masked around every increment. The per-thread count goes in a `ThreadInfo` block kept outside `THREAD_TABLE` and pointed to by TPIDR_EL1.

- TPIDR_EL1 is set on every dispatch, by `restore_context` or the `ret_from_switch` trampoline (F4).
- `enter_scheduler` takes `RUN_QUEUES` and `THREAD_TABLE` on the per-CPU boot stack before any thread is dispatched (`scheduler.rs:74`, `:79`), and that context is never a thread. Each CPU therefore gets a static boot `ThreadInfo`, and `kernel_main` and `secondary_main` set TPIDR_EL1 to it before their first lock. Without it, the first lock on each CPU would dereference TPIDR_EL1 = 0.

**A in brief.**

- *Pros:* one lock class; matches the docs; no new per-thread state.
- *Cons:* the IRQ latency of every hold; a spinner waits with IRQs off.

**B in brief.**

- *Pros:* IRQs stay on under ordinary locks.
- *Cons:* the IRQ-safety rule stays a convention, as it is today in `scheduler.rs:116-118` and `deadlock-prevention.md:513`, and that convention was broken. The count must stay exact on every path.

**C in brief.**

- *Pros:* B's latency, with the class enforced by the type; the validator turns the rules into panics.
- *Cons:* two types to choose between for every new static; the most code.

### Recommendation (adopted): C, delivered as step 2 (common) and step 8 (after the crash-fix gate)

1. Step 2 fixes the heartbeat-stuck WEDGE under every option, so choosing now puts nothing at risk.
2. **Against A:** holding IRQs off across long holds on CPU 0 stops `TICK_COUNT` and every timeout, which trades a wedge for a stall. B and C only block preemption on that CPU.
3. **Against B:** the rule already existed in comments and was broken. C makes the class part of the type and checks it.
4. **`ThreadInfo` pays for itself.** It holds:
   - the thread id, so `current_thread_id` becomes lock-free. That removes at least 3 `CURRENT_THREAD` lock takes per IPC round trip (`channel.rs:44`, `:237`, `:361`) and closes the N4 race for good;
   - the preempt count;
   - the reschedule flag (F3);
   - `on_cpu` (F4), moved from its interim array.
5. **Keep F2's single preemption point.** At first, `preempt_enable` reaching 0 does not reschedule, so a reschedule can wait at most one 1 ms tick. Make it a reschedule point later only if latency measurements call for it.
6. **Step 8 comes after the crash-fix gate (step 7) and is not part of PR #149's gate.** No observed failure class maps to lock-holder preemption on the other 30 locks, and step 6a does not need `ThreadInfo`, because `on_cpu` starts in its own array. After step 2, no IRQ-off region takes one of the 30 locks, so preempting their holders cannot deadlock through IRQ context. It can still stall. With strict class priority (`sched/mod.rs:65-76`), a higher-class thread spinning on CPU c starves a preempted lower-class holder on the same CPU until the balancer moves it (N10). **Escalation rule:** if heartbeat-alive WEDGE persists after step 6b and step 1b's starvation scan is above 0, step 8 moves ahead of step 7.

The cost is amending `scheduler.md` §10.3 and `deadlock-prevention.md` §9.2 and §12. #164 required owner approval for that, and the owner's choice of 1C gives it.

-----

## Decision 2: FP/NEON policy

### Facts

- No `f32` or `f64` appears anywhere, so no kernel code depends on FP.
- On `aarch64-unknown-none`, neon is required by the ABI, and `-Ctarget-feature=-neon` is not allowed (rustc `target_features.rs`, per the research pass). The only way to build without FP is `aarch64-unknown-none-softfloat` (Tier 2, with `core` and `alloc`).
- The research pass found V registers in the prebuilt `libcore` and `compiler_builtins` (`memset`, `Formatter::pad_integral`) of an older nightly. Both run from the timer IRQ. Step 1b redoes this on the post-#161 ELF.
- No EL0 code exists (`CLAUDE.md:66`); every thread runs at EL1.
- The in-kernel AES-GCM and SHA-2 already run their software backends. The research pass found that `cpufeatures` (0.3.0, `src/aarch64.rs:175-183`) returns false on bare-metal aarch64 (`kernel/Cargo.toml:10-11`).
- Linux arm64 builds C with `-mgeneral-regs-only` and Rust for the softfloat target (since rustc 1.85). Zircon, Redox and Theseus also keep kernel code free of FP.
- **Option A reverses a Phase 0 decision.** `docs/phases/00-foundation-and-tooling.md:63` rejected `aarch64-unknown-none-softfloat` "because it cannot run NEON-optimized inference code". A's premise is that such code runs at EL0: AIRS is a "privileged userspace service" (`docs/intelligence/airs.md:30`). No phase in `docs/project/development-plan.md` introduces EL0 process execution (searched: not found). Phase 11, the AIRS Inference Engine (`development-plan.md:428`), is the first phase that needs it. So A carries a dependency: EL0 must exist before Phase 11, or A is revisited when Phase 11 is planned.

### Decision 2 options

Option A is #164's 2A with a correction, option B is #164's 2B, and option C is added here. The owner chose B, in the form its column now shows: the full 528 bytes at every thread switch as well as at every EL1 IRQ entry (see "Decision"). A and C are kept as the analysis behind that choice.

| | **A: softfloat kernel, EL0 FP later** | **B: hard-float, eager full save** | **C: hard-float + `kernel_neon_begin/end`** |
| --- | --- | --- | --- |
| Idea | The kernel never touches V registers. EL0 FP is handled when EL0 exists | Save q0–q31 plus FPCR/FPSR (528 B) at every EL1 IRQ entry, because the IRQ handler itself uses NEON, and the same 528 B at every thread switch. The draft saved only d8–d15 and FPCR (about 72 B) at the switch; the owner chose the full save | Use NEON only inside explicit regions that first save the owner's state |
| Viable here | Yes | Yes | **No** (see below) |
| Cost per IRQ | None | 528 B stored and loaded on every tick on every CPU. seL4 measured about 120 cycles for an FP save and restore on a Cortex-A35 (RFC-18) | — |
| Cost per voluntary switch | None today | 528 B stored and loaded (about 72 B in the draft's form). An IPC round trip makes two switches | — |
| With future EL0 | EL0 FP state survives syscalls and IRQs untouched; it is saved at switch-out, only for threads that use FP | Every syscall and IRQ from EL0 must save and restore all 528 B, because ordinary kernel paths dirty v0–v7 | — |
| Build changes | Target string in `justfile:3` (the ELF paths at `:5-6` follow from it), `.cargo/config.toml:1`, `rust-toolchain.toml:3` and `.github/workflows/ci.yml:22`, `:36`, `:50`, `:64`, `:81`, `:121`, plus the docs and agent files named at the end of "Docs to update" (unchanged under 2B). Add `--cfg aes_backend="soft"`, `polyval_backend="soft"` and `sha2_backend="soft"`: the crates' `#[target_feature(enable="aes")]` code paths are expected to fail to build on softfloat (inferred; a build will confirm) | None | None |
| Effect on CLAUDE.md | `:9` "hard-float ABI", `:10` target, `:58-59` FPU enable, `:152` toolchain targets; rule `01-code-conventions.md:16` and `:32` | `:9-10` and `:58-59` stay; add "every EL1 IRQ entry and every thread switch saves the full FP/SIMD state (528 B)" | — |
| Tripwire | Set CPACR_EL1.FPEN=0b00 at EL1, so any stray FP instruction traps with EC 0x07 and an exact ELR. Add a CI objdump gate: zero V-register instructions in the kernel ELF | A V-register clobber counter only; nothing structural stops a regression | — |
| Intelligence workloads | AIRS is a userspace service (`docs/intelligence/airs.md:30`, `:113`) built for its own hard-float EL0 target, so it is unaffected, provided EL0 arrives before Phase 11. In-kernel NEON (the NEON memops in `docs/kernel/memory.md:103`; page zeroing in `docs/kernel/memory/hardening.md` §11.4) would have to be hand-written asm regions. DC ZVA zeroing does not use NEON | Kernel Rust may use NEON freely | — |
| Main risk | A NEON-heavy service prototyped as an EL1 thread before EL0 exists cannot use NEON from Rust | The invariant is easy to break when a new entry path is added; the cost for AIOS is unmeasured | — |

**Why C is not viable.** A `kernel_neon_begin/end` discipline works only if the compiler is kept away from V registers outside the regions; Linux does that with `-mgeneral-regs-only`. `aarch64-unknown-none` has no such option, and the prebuilt `core` already uses V registers in code the IRQ can reach. C works only on top of A: a softfloat kernel with NEON regions written in asm, which is the Linux arm64 model.

**Correction to #164's 2A.** "Lazy EL0 FP" in the sense of `scheduler.md:472` leaves FP state in the registers across switches and saves it on the first trap. That is wrong on SMP: after a migration, the thread's live registers are on another CPU. When EL0 arrives, do this instead:

- trap the first FP use (FPEN=0b01) and allocate `FpContext` then;
- save eagerly at switch-out, for threads that use FP;
- restore on return to EL0.

This is the Linux arm64 model. seL4 moved to eager switching in RFC-18, citing information leaks and complexity, and CVE-2018-3665 (LazyFP) is the precedent for lazy restore leaking state.

This correction applies to 2A only. Under 2B, which the owner chose, every thread, EL0 threads included, has its full FP state saved at every switch. There is no first-use trap and no lazy restore.

### Recommendation (not taken): A, with B's saves as an interim (step 4)

- A removes the problem instead of paying for it on every IRQ, and it matches every surveyed kernel.
- It costs nothing at runtime today: no code uses FP, and the crypto already runs in software.
- It gives a deterministic tripwire (FPEN=0b00).
- The EL0 FP design waits until EL0 exists (YAGNI), with the Phase 11 dependency recorded above.
- H5 is a live candidate for the PANIC and EXCEPTION reports, so protection comes early. The draft had step 4 add B's saves as an interim (528 B at EL1 IRQ entry; d8–d15 and FPCR in `save_context`) right after the frame fix, and a later softfloat step switch the target and remove them.

The owner chose 2B instead. Step 4 is now the permanent FP step, with the full 528 B at the thread switch as well, and the softfloat step is gone (see "Decision").

-----

## Delivery plan

### Order and why

| Step | Change | Targets | Needs |
| --- | --- | --- | --- |
| **1a** Harness | Classifier subclasses, tripwire columns, interleave mode, statistics, load and version checks, a timeout check that works on ubuntu-26.04. No kernel change | Measurement | — |
| **1b** Tripwires | Detect-only counters and detectors; panic handler fix (N8); bench DAIF masking removed; V-register objdump | Measurement; decides H1 and H3 | 1a |
| **2** IRQ-class locks | `IrqSpinLock` for the 9 statics with the held-rank order; `with_this_cpu`; F6; F8; lint for new statics | Heartbeat-stuck WEDGE | 1b confirms H3 |
| **3** IRQ frame | F1 + F2 | PCZERO; the bench hijack; part of PANIC/EXCEPTION | 2 |
| **4** FP/SIMD save | Decision 2B: the full 528-byte FP/SIMD state saved at EL1 IRQ entry and at every thread switch | H5: its part of PANIC/EXCEPTION, and the defect itself | 3 |
| **5** Address spaces | F7 | N5, N9 | 1b |
| **6a** `on_cpu` | F4, including the `ret_from_switch` trampoline, plus F3 with a per-CPU flag | H2; cross-CPU corruption | 3 |
| **6b** Wake/block | F5 without the wake token: N1, N7, and N10 if implicated. The token, every blocking-site conversion and, by the capability-lifetime ADR's choice, the notification-deadline fix land in the capability-lifetime PR (2026-09-24 amendment) | Heartbeat-alive WEDGE; DEGRADED | 6a; the capability-lifetime PR |
| **7** Final gate | Remaining docs | All classes | 1a–6b |
| **8** Lock classes | The rest of Decision 1C; the exit safe point (2026-09-24 amendment) | Lock-holder preemption; per-CPU read races (for good); a killed thread running on after `process_exit` | 7, unless the escalation rule fires |

Why this order:

- **Step 2 comes before the frame fix.** WEDGE is the largest failure class on `main` across the three arms, 21 of 90 boots against 14 PCZERO: 9 of 30 in the baseline, 7 of 30 in run 167's `main` arm (6 of them heartbeat-stuck) and 5 of 30 in its #161 arm (4 stuck), where PCZERO has 6. Step 2 does not depend on F1 or F2: neither needs the new frame. Every class that ends a boot hides later ones, because the first failure decides the class. So while 20–30% of text-mode boots wedge at about 7 s, the PCZERO, PANIC and DEGRADED counts of every later comparison are censored and noisy. Removing WEDGE first makes each later comparison more sensitive. Step 2 proceeds only if step 1b's named-lock detector confirms H3.
- **Step 4 is separate from step 3** so that the `frame.rs:51` PANIC (8 of 20 gpu boots) can be attributed to H1 or H5. If the owner prefers speed over attribution, steps 3 and 4 can share one PR.
- **Steps 6a and 6b are separate** so that a fix of the heartbeat-alive WEDGE can be attributed to F4 or F5. Since the 2026-09-24 amendment, F5's wake token lands in the capability-lifetime PR, whose soak pair is attributed the same way (see "Review notes").
- **Step 8 is after the gate** (Decision 1 recommendation, item 6).

### Soak protocol (every soaked step)

- **Interleaved A/B.** Boot the previous step's kernel and this step's kernel alternately in the same host session, 20 text-mode and 10 gpu-mode boots per arm. That is 60 boots of 75 s, about 75–85 minutes of an otherwise idle host (the kill-after allows up to 10 s more per boot, `soak-qemu.sh:688`). Steps 3 and 4 use 20 gpu boots per arm, so the `frame.rs:51` comparison has some power; that is about 100 minutes. The harness boots one ESP image per invocation (`soak-qemu.sh:598-615`), so step 1a adds a mode that alternates two images per boot.
- **Classes (step 1a).** The harness has one WEDGE class and one PANIC class today, with the subtype only in free text (`soak-qemu.sh:21`, `:356-383`). Step 1a adds:
  - WEDGE-STUCK (heartbeat stuck or stopped) and WEDGE-ALIVE (heartbeat alive, bench incomplete);
  - DEGRADED: "=== Gate 1 Complete ===" printed but the IPC line reports fewer than `(10000 iters)` (`bench.rs:400-409`). CLEAN then requires 10000 iterations;
  - PANIC-LOCK: a panic from step 1b's lock detector, which prints `PANIC: lock re-entry: <LOCK> on CPU n`.

  The latency PASS stays a recorded marker only (`soak-qemu.sh:205`), because TCG timing depends on the host. Step 4 is the one exception: its acceptance compares the Gate 1 IPC figure between its two arms (see step 4 and "Gate 1").
- **Tripwire line.** Kernel counters are printed on one fixed line, `[tripwire] k=v k=v ...`, at "Gate 1 Complete", with each heartbeat, and in the panic and exception dumps. The harness puts the last value of each key into `summary.tsv` columns and reports them per arm.
- **No instrument freeze (owner decision, 2026-09-22).** Rule 04's Session Start Checklist stays in force during the series: `brew upgrade qemu just`, a nightly bump and `cargo update` may change QEMU, the Homebrew edk2 firmware (`justfile:8`), the toolchain and the dependencies between steps, and Renovate PRs (like #161 and #169) keep flowing. A freeze (pin QEMU; freeze `rust-toolchain.toml`, `Cargo.lock` and the CI runner image; hold Renovate) was considered and not taken. The consequence is that only within-pair comparisons are valid; any comparison across steps or with an older baseline must be re-measured as a fresh interleaved pair.

  The interleave mode refuses a pair whose arms report different QEMU versions (already recorded, `soak-qemu.sh:636`), firmware paths or toolchain channels. Interleaving protects each pair against drift, but not comparisons with older baselines.
- **Load rule.** The harness logs the 1-minute load per boot (`soak-qemu.sh:677`; `summary.tsv` column 9), and run 167 ranged from 6.5 to 50. Do not start a pair while the load is above the host's CPU count. Report each arm's mean and maximum load, and redo the pair if the arm means differ by more than 25%. Consider moving the text-mode pairs to a `workflow_dispatch` CI matrix to free the Mac. CI arms are a different instrument (x86 TCG) with their own baseline, so a pair must never mix a CI arm with a local one.
- **Statistics.** Every comparison is between the two interleaved arms of one pair, using a one-sided Fisher exact test. Raw differences are recorded but never gated on.
  - **Regression guard.** A step fails if CLEAN is lower in the new arm with p < 0.05. This replaces "CLEAN(new) ≥ CLEAN(prev)". That old rule fails a step with no effect about 41–45% of the time at 30 boots per arm and true CLEAN rates of 50–90%, because the two counts tie only about 10–17% of the time. It also passes a real 10-point regression about half the time. The Fisher guard has a false-fail rate of at most 5%. At 30 boots per arm it catches a drop from 60% to 30% about two times in three and a drop to 40% about one time in three (normal approximation). It guards against breaking things; it does not measure gains.
  - **"Class removed"** means 0 boots of the class in the new arm and Fisher p < 0.05. At 30 boots per arm that needs at least 5 boots of the class in the previous arm: 4 → 0 gives p ≈ 0.056, 5 → 0 gives p ≈ 0.026, and 9 → 0 gives p ≈ 0.001. A rarer class cannot be shown removed by one pair. Report it and pool the evidence across later pairs.
  - **Censoring.** The earliest failure decides a boot's class, so a class that ends boots early hides later ones. PCZERO counts on `main` are censored by WEDGE-STUCK.
  - **Absolute bounds.** 0 failures in n boots bounds the per-boot failure rate, one-sided at 95%, at 25.9% (n=10), 13.9% (20), 9.5% (30) and 4.9% (60).

### Rules for every step PR

- Each step is its own PR on a `claude/crash-fix-step-<n>-<name>` branch, with commits named `Crash fix step <n>: <description>`, as #168 did ("Crash fix step 0: QEMU boot soak harness").
- Each PR passes rule 02's gates (`just check`, `just test`, CI, `cargo objdump -- -h`) and `/audit-loop`, and updates the docs it makes stale.
- Pure logic goes in `shared/` with host unit tests: the IRQ-off nesting counter, the held-rank validator, the preempt count and the wake-token state machine. That follows [the M25 shared-crate ADR](2026-05-07-cl-phase-7-m25-shared-crate-for-host-tested-kernel-types.md), and CI runs Miri over it (`.github/workflows/ci.yml:73-85`). A soak is a weak tool for rare interleavings; unit tests cover the state machines.

### Steps

**Step 1a — harness only.**

- Classes and the tripwire columns as above; Fisher tests and per-arm load in the report.
- Interleave mode, with the version checks.
- A `timeout` check that accepts ubuntu-26.04's `timeout`, which today fails at setup (see #169).
- A `runs` input for `workflow_dispatch` (`ci.yml:8` has none), so that one commit can be soaked several times.
- **Acceptance:** `scripts/soak-qemu.sh --classify` on the run-167 logs reproduces every boot's class. The only changes are WEDGE split into STUCK and ALIVE, and CLEAN turning into DEGRADED where the IPC line reports fewer than 10000 iterations. No boots are needed.

**Step 1b — kernel tripwires, detect-only.** Nothing here changes scheduling except the bench-masking removal (F6), which is near no-op.

- **Detect-only IRQ-class lock type** on the 9 statics. It is step 2's `IrqSpinLock` with masking off. It records the owner CPU and that CPU's switch generation at each acquisition.
  - An IRQ-context acquisition of a lock held by this CPU, with the switch generation unchanged, panics as PANIC-LOCK with the lock's name.
  - If the generation has changed (the holder was switched out), it only counts `irq_spin_preempted_holder[lock]`. That case is lock-holder preemption, not re-entry, so it must not contaminate the H3 test.
- **ELR/SPSR mismatch counter.** The IRQ entry stores ELR_EL1 and SPSR_EL1 in 16 extra bytes of the frame and, before `eret`, counts differences from the live registers without restoring anything. A count above 0 shows that H1 fires.
- **Counts:** per-CPU ticks; IRQ-context switches; cross-CPU direct and reply switches; `unblock` skipping a Running or Runnable target, by caller; `unblock` queueing on a CPU other than the thread's last one, by caller (`check_timeouts`, `ipc_reply`, `ipc_send`, `try_wake_select`, notification timeouts); saved contexts whose PC or LR is outside the kernel VA range (N5).
- **Scans in the CPU 0 heartbeat**, with `try_lock` only. A scan is skipped and counted as skipped if any lock is contended. A `try_lock` that fails on a lock this CPU holds, with the switch generation unchanged, also counts as re-entry. A thread is counted only if a scan flags it twice in a row, because threads in transit are legitimately Runnable and unqueued for a moment (`scheduler.rs:174-177`, `:387-407`; `direct.rs:285-290`). The scans look for:
  - orphans: Runnable, not current and not queued;
  - blocked with no waker: BlockedIpc, BlockedNotification or BlockedSelect, with no `TIMEOUT_QUEUE` or `NOTIFY_DEADLINES` entry, and not referenced by any channel's `pending_caller` or `waiting_receiver`, by `SELECT_WAITERS` or by a notification waiter;
  - starved threads: queued and Runnable but not run for more than 1000 ticks, by class (N10).

  Taking non-IRQ locks with `try_lock` from the tick handler is a documented exception to the class rule, because `try_lock` never waits.
- **"Core N online"** prints VBAR_EL1 and TTBR0_EL1.
- **Fatal dumps.** The panic handler masks IRQs first (N8), then prints the CPU, the current thread and the tripwire line. The exception report adds SP, TTBR0_EL1, VBAR_EL1 and the current thread to ESR, ELR and FAR (`exceptions.rs:284-295`). `free_dma_pages` and `free_pages` become `#[track_caller]`, so the `frame.rs:51` report names its call site.
- **V-register listing.** A read-only `llvm-objdump` of the post-#161 soak ELF lists every V-register instruction site by symbol. It confirms or refutes that the zero-fill at `timeout.rs:89` and the `GpuBufferHandle` and `RawMessage` copies use V registers. The listing is archived in the PR, and the H5 verdict is updated from it.
- **Acceptance** (interleaved against `main`; both arms then become the new baseline):
  - No new class except PANIC-LOCK.
  - WEDGE-STUCK + PANIC-LOCK in the 1b arm is not different from WEDGE-STUCK in the `main` arm (two-sided Fisher p ≥ 0.05).
  - Every non-CLEAN boot has a tripwire line or fatal dump.
  - **Decisions:** if the 1b arm has 3 or more WEDGE-STUCK and 0 PANIC-LOCK, same-CPU re-entry (H3) is refuted as the WEDGE cause, and this ADR is revised before step 2. If the ELR mismatch counter is 0 in all 30 boots, H1 is refuted as a live mechanism, and this ADR is revised before step 3.

**Step 2 — IRQ-class locks** (the same under A, B and C).

- `IrqSpinLock` with masking on for the 9 statics, with the held-rank order and check. The IRQ path takes these locks blocking, replacing the `try_lock` skips.
- `with_this_cpu` for the per-CPU arrays; F6; F8; the lint for new statics.
- Rebuild `deadlock-prevention.md` §3.3; replace `developer-guide.md:2061` ("Use `try_lock()` in IRQ context, never blocking lock"), which this step reverses.
- **Acceptance:** WEDGE-STUCK and PANIC-LOCK are removed (0, Fisher p < 0.05). No rank-check panic. Regression guard. Gate 1 is recorded (see below).

**Step 3 — IRQ frame** (F1 + F2).

- The step-1b mismatch counter now counts mismatches that the restore corrected.
- **Acceptance:**
  - Primary: the mismatch counter is above 0 in at least one boot, which confirms the mechanism still fires, and PCZERO is 0 in the new arm, which shows it no longer escapes. PCZERO's Fisher p is reported; it is significant only if the previous arm has 5 or more.
  - PANIC and EXCEPTION counts are reported per report type, `frame.rs:51` separately.
  - Regression guard.

**Step 4 — FP/SIMD save** (Decision 2B; permanent).

- **What is saved.** q0–q31 (512 B), FPCR and FPSR, in the existing `FpContext` layout: `#[repr(C, align(16))]`, 528 bytes, with a size assertion (`kernel/src/task/mod.rs`). Every thread gets this, EL1 threads now and EL0 threads when they exist. There is no first-use trap and no lazy restore, and CPACR_EL1.FPEN stays at 0b11 as `boot.S` sets it (`boot.S:83-89`, `:280-285`).
- **Save point 1: the EL1 IRQ entry frame.**
  - `irq_el1_entry` stores the 528 bytes after step 3's 272-byte `TrapFrame`, at frame offset 272. The exit restores them before `eret`.
  - The frame grows from 272 to 800 bytes, on the interrupted thread's stack.
- **Save point 2: the per-thread context switch.**
  - `save_context` and `restore_context` store and load the same 528 bytes on every switch: `schedule()`, `thread_yield`, `block_current` and both IPC direct switches.
  - The save area is a new `FpContext` field in `ThreadContext` (`task/mod.rs:70-85`). It replaces the always-`None` `Thread::fp_context: Option<FpContext>` (`task/mod.rs:139`, `:192`).
  - With 8 bytes of padding after `timer_ctl`, the field sits at offset 0x130, and `ThreadContext` grows from 296 to 832 bytes. Its size assertion and the offsets `context_switch.S` uses change with it.
  - A new thread starts with the area zeroed (FPCR = 0).
- **Alignment: 16 bytes for every q-register `stp`/`ldp`.**
  - The FP area in the IRQ frame starts at offset 272, and 272 and 800 are both multiples of 16. SP therefore stays 16-byte aligned, and every q-register pair lands on a 16-byte boundary.
  - `FpContext`'s `align(16)` gives `ThreadContext`, and so `Thread`, 16-byte alignment. A compile-time assertion pins each offset the asm uses, as `trap.rs` and `task/mod.rs` already do for the sizes.
- **Counter:** IRQs whose handler changed any V register, found by comparing the saved and live registers before the restore. It stays as a permanent tripwire, because 2B has no structural guard.
- **Docs:** the `scheduler.md` FP text and the `CLAUDE.md` FP fact listed under "Docs to update".
- **Acceptance:**
  - The counter is above 0: the handler does clobber V state.
  - The `frame.rs:51` PANIC and EXCEPTION counts are compared with the step-3 arm; removal means Fisher p < 0.05. If the `frame.rs:51` PANIC disappears here but did not in step 3, H5 is confirmed as its cause.
  - **Gate 1 latency.** Take the mean of the per-boot IPC averages over each arm's CLEAN boots, per mode. The source is the bench output (`bench.rs:400-409`, and the `Gate 1: IPC < 10 us` line at `bench.rs:437-443`). The step-4 arm stays under the 10 μs threshold, and its mean is reported beside the step-3 arm's. Both arms run under the load rule.
    - If the step-3 arm is under 10 μs and the step-4 arm is not, the step fails, and Decision 2 is revisited (see "Decision").
    - If the step-3 arm is already at 10 μs or more, the Gate 1 escalation applies instead.
  - Regression guard.

**Step 5 — address spaces** (F7).

- **Acceptance:**
  - The step-1b PC/LR-outside-kernel-VA counter goes from its baseline value to 0. If it was 0 throughout, N5 is recorded as not observed.
  - "Core N online" shows a virtual VBAR and the invalid TTBR0.
  - No assertion fires in 30 boots.
  - Regression guard.

**Step 6a — `on_cpu`** (F4, including the `ret_from_switch` trampoline and the bounded wait, plus F3 with a per-CPU flag).

- **Acceptance:**
  - No `on_cpu` timeout panic.
  - Cross-CPU direct and reply switches read 0.
  - IRQ-context switches drop (F3).
  - Regression guard.
  - WEDGE-ALIVE and PANIC/EXCEPTION counts reported.

**Step 6b — wake/block** (F5 without the wake token: N1, N7, and N10 if implicated. The token, every blocking-site conversion and the notification-deadline fix land in the capability-lifetime PR; see the 2026-09-24 amendment under "Review notes").

- **Acceptance:**
  - DEGRADED = 0 and WEDGE-ALIVE = 0 in the new arm.
  - The orphan and blocked-with-no-waker scans read 0.
  - `unblock` skipping a Running target reads 0 from `ipc_reply`. Since the 2026-09-24 amendment this item is reported, not gated (a capability-lifetime ADR choice): step 6b passes on the line above, the blocked-with-no-waker scan reading 0.
  - Cross-CPU `unblock` from `check_timeouts` reads 0 for Interactive threads.
  - CI soak: all 5 boots CLEAN in each of 3 runs of the same commit (see "CI soak").
  - Regression guard.

**Step 7 — final gate.**

- Remaining doc updates. The results are recorded on #164.
- **Acceptance:**
  - Interleaved against a `main` arm, 20 text and 20 gpu boots per arm: CLEAN is higher in the new arm with Fisher p < 0.05 in each mode, and PCZERO, PANIC, EXCEPTION, WEDGE-STUCK, WEDGE-ALIVE and DEGRADED are all 0 in the new arm.
  - Then an unpaired, overnight absolute soak: at least 60 text boots and 30 gpu boots, all CLEAN. That bounds each mode's per-boot failure rate at 4.9% and 9.5%.
  - CI soak: all CLEAN in 3 runs.
  - `just docs-check` reports no new drift.
  - Decide whether to drop `report_only` and `continue-on-error`, so the CI soak becomes a merge gate.

**Step 8 — the rest of Decision 1C, and the exit safe point** (after step 7, not part of PR #149's gate, unless the escalation rule moves it). The exit safe point was added on 2026-09-24; its scope is in the amendment under "Review notes".

- `ThreadInfo` through TPIDR_EL1, with a per-CPU boot `ThreadInfo`; `SpinLock` for the 30 statics, with the release-build class checks; `on_cpu` and the reschedule flag moved into `ThreadInfo`; "sleeping while atomic" panics at `block_current` and `thread_yield`; panic on count underflow or overflow.
- A sleeping lock or a yielding poll for `VIRTIO_GPU`.
- **Acceptance:** no assertion fires in 30 boots; regression guard; the exit safe point's added acceptance (2026-09-24 amendment, "Review notes").

### Gate 1

Gate 1 (IPC < 10 μs) is a project gate. `development-plan.md` records it as passed (`:217-247`), and that result rules out the hybrid-kernel fallback (`:181`, `:231`, `:507`). Its methodology assumes IRQs masked during measurement (`:247`). This series departs from that plan, and says so:

- Gate 1 is re-measured and recorded at every step. It is an acceptance item only at step 4. There the eager FP save lands on the IPC path, in two switches per round trip, and the owner revisits 2B if it pushes the IPC average past 10 μs (see "Decision").
- The bench's masking is removed in step 1b. It was already ineffective after the first call (`bench.rs:237` vs `direct.rs:192`), so the recorded 4 μs was measured with IRQs mostly on.
- **Escalation:** if the IPC average is 10 μs or more on an idle host (load below the CPU count) at step 3 or later, the owner decides before step 7 whether Gate 1 is re-run or reopened.

### CI soak

- The CI soak is report-only: the job name says so (`ci.yml:88`), it runs with `continue-on-error: true` (`ci.yml:108`), and the harness gets `report_only=1` (`ci.yml:135`). Its check is green whenever the harness runs, whatever the boot classes. It is red only when setup fails, as on #169.
- Criteria are therefore class counts, read from `summary.md` (appended to the job summary, `ci.yml:136-140`) or the uploaded artifact.
- Repeated runs of one commit use "Re-run jobs" or the new `workflow_dispatch` input, not new pushes: a newer push to a non-`main` branch cancels a running soak (`ci.yml:102-104`).
- The 24.04 figures (0/5, 2/5) do not carry over to 26.04. After #169 merges, run the CI soak on `main` at least 3 times to set the 26.04 baseline before any step's CI numbers are compared.

-----

## Consequences

### Gains

- Every observed failure class has a candidate mechanism and a step that targets it. Step 1b adds a detect-only counter for each mechanism, so the count is seen above 0 before its fix and at 0 after it. An assertion that only lands with its fix cannot fail and would prove nothing.
- The kernel comes to match its own architecture docs (F1, F2, IRQ-class locks).
- The tripwires stay afterwards as assertions.

### Costs

- The IRQ entry frame grows from 176 bytes to 192 in step 1b (the detect-only ELR/SPSR slots), to 272 in step 3 and to 800 in step 4 (the 528-byte FP/SIMD area).
- From step 4, every IRQ and every thread switch also stores and loads 528 bytes, and `ThreadContext` grows from 296 to 832 bytes. Each IPC round trip makes two switches, which step 4's Gate 1 check measures.
- Under 2B, nothing structural guards the save. A new entry path that skips it corrupts FP state silently, and only the V-register counter would show it. An example is the lower-EL entries in `trap.rs` once EL0 exists.
- Lock operations gain a DAIF save and restore for IRQ-class locks, and a count increment and decrement for `SpinLock` from step 8.
- Gate 1's figures change (see "Gate 1").
- Every new static needs a class choice (1C), which the lint enforces.
- The series has 10 PRs (1a, 1b, 2, 3, 4, 5, 6a, 6b, 7, then 8). Nine of them, all but 1a, need a host-exclusive soak of 75–100 minutes, about 12.5–13.5 hours in total (six pairs of 75–85 minutes, and 100 minutes each for steps 3, 4 and 7). On top of that come the overnight absolute soak in step 7, any reruns required by the load rule, and the re-baselines. No builds can run on the Mac during a soak.

### Docs to update

| Document | Change | Step |
| --- | --- | --- |
| `CLAUDE.md` Key Technical Facts | Add a preemption/IRQ block: the IRQ frame (272 bytes from step 3, 800 from step 4: the `TrapFrame` plus the 528-byte FP/SIMD area), the single preemption point at `irq_exit`, the lock classes and the IRQ-shared set with its order, `on_cpu`/`finish_switch`, and when the reschedule flag is set. Add the FP fact: every EL1 IRQ entry and every thread switch saves the full FP/SIMD state (528 B). `:98-102`: lock order to include `THREAD_TABLE`, `RUN_QUEUES`, `CURRENT_THREAD`, `TIMEOUT_QUEUE`, `REPLY_SLOTS` and `WAKEUP_ERRORS` with their class. The hard-float ABI and target (`:9-10`) and the FPU enable sequence (`:58-59`) stay as they are | 2, 3, 4 |
| `.claude/rules/01-code-conventions.md` | `:11` panic handler masks IRQs first; add the lock-class rule and "no unconditional `DAIFClr`" | 1b, 2 |
| `docs/kernel/scheduler.md` | §3.3 reschedule-flag semantics (340-343); §4.1 saved state (ELR and SPSR); §4.2 direct switch: add the `on_cpu` check; §10.2 (1922); §10.3 lock classes | 3, 6a, 8 |
| `docs/kernel/scheduler.md`, FP text | The lazy FP save through a CPACR_EL1 trap is now wrong: the eager 528-byte save at EL1 IRQ entry and at every thread switch replaces it. Rewrite §4.1's "LAZY SAVE" diagram, its totals and the "Lazy FP save" paragraph (454, 467, 472), and the `FpContext` and `fp_context` comments (494, 511-512). Rewrite the lazy-FP CPACR trap in §4.2's direct switch (582-584). In §4.3's budget, the "with FP save/restore" line becomes the normal case. Also rewrite §6.4's CPACR sentence (1095) and "Lazy FP save/restore" in §14 (2407) | 4 |
| `docs/kernel/deadlock-prevention.md` | Rebuild the §3.3 table: `RUN_QUEUES` and `NOTIFY_DEADLINES` are not leaves; place `THREAD_TABLE`, `CURRENT_THREAD` and `WAKEUP_ERRORS`; add the M25 locks, a class column and the IRQ-class order. §9.2; §12 rule 10 becomes the class rule; rule 8 lists `VIRTIO_GPU` as a known violation | 2 |
| `docs/project/developer-guide.md` | `:2061` "`try_lock()` in IRQ context, never blocking lock" is reversed | 2 |
| `docs/kernel/hal.md` §8.3 | Reschedule flag set on slice expiry; `irq_exit` | 3 |
| `docs/project/ai-agent-context.md:170-184` | Replace the unconditional-unmask pattern | 2 |
| `docs/phases/03-ipc-and-capability-system.md` 123, 133, 166, 212, 377 | Un-tick or annotate the items that are not implemented, and point to this ADR. The lazy-FP items (166, 212, 377) point to step 4's eager save | 7 |
| `docs/project/development-plan.md:181`, `:217-247`, `:507` | Gate 1 re-measured; the departure recorded | 7 |

Under 2B these stay as they are: `CLAUDE.md:9-10`, `:58-59` and `:152`; rule 01 `:16` and `:32`; `docs/phases/00-foundation-and-tooling.md:63`; `docs/project/developer-guide.md:1901`; `docs/kernel/boot/kernel.md:95`; `docs/kernel/memory.md:103`; `docs/kernel/memory/hardening.md` §11.4; and the target strings in rule 02, the verify-phase skill, the agent files, `setup-dev-env.sh` and `README.md`. The draft changed them only for softfloat (2A).

### PR #149 (M26)

Issue #165 gates this PR on the crash fix. Its state on 2026-09-22: open, mergeable, all CI checks green, which says nothing about bootability.

- **Open owner decision.** #165's soak threshold is still unanswered: A, all CLEAN in 10 text + 10 gpu boots, or B, 20 + 20. All CLEAN in 10 boots bounds the failure rate only at 25.9% per mode, and 20 boots at 13.9%. This ADR suggests B, run interleaved against `main` with the same protocol.
- **Work still listed in #165:** rebase onto `main`; fix the clippy `needless_range_loop` at `kernel/src/compositor/shell/workspace.rs:557`; the doc fixes.
- **Expected conflicts.** #149 touches `CLAUDE.md`, `docs/project/developer-guide.md`, `kernel/src/main.rs` and `kernel/src/compositor/service.rs`. Steps 1b (`main.rs`, the panic handler), 2 (`compositor/service.rs:187-188`, `developer-guide.md:2061`), 2–4 (`CLAUDE.md`, see "Docs to update") and 5 (`main.rs:271`) edit the same files, so the merge after step 7 will conflict.
- **New statics.** The shell locks (`STATUS_STRIP`, `TASKBAR`, `WORKSPACE`, per the PR description) must take the new lock type. Step 2's lint makes an unclassed static fail after the merge. Under 1C, as decided, they are `SpinLock`, never taken from IRQ context. Add them to `deadlock-prevention.md` §3.3 and to the `CLAUDE.md` lock order.
- After step 7 is on `main`, merge `main` into `claude/phase-7-m26-desktop-shell` and soak at the chosen threshold.
- Then re-run the present-on experiment (`COMPOSITOR_PRESENT_ENABLED`, see [the M24 present-gate ADR](2026-05-07-cl-phase-07-m24-compositor-present-gate.md)) as a separate soak. The low-VA data aborts that the M24 ADR left unexplained plausibly come from the same corruption; run 167's `FAR=0xa` write fits that pattern. The M24 workarounds (the torn-read bounds check and the `virtio_input` modulo guard) stay as defensive checks, each with a counter.

### Other PRs (#161, #169, #170)

- **#170: merged** 2026-09-22 11:19 UTC as dd05a1e. It changes only `.claude/settings.json`. No action; this branch was fast-forwarded onto it, per rule 03.
- **#161: merged** 2026-09-22 12:10 UTC as e98e1ad. It changes only `rust-toolchain.toml` (nightly-2026-09-22). Its interleaved soak (`target/soak/167`) doubles as the local re-baseline (see "Soak evidence"). It changed codegen, so the V-register listing must be made from the post-#161 ELF (step 1b).
- **#169: open.** It moves the CI soak runner from `ubuntu-24.04` (`ci.yml:107`) to 26.04. On 26.04 the soak failed at setup with "soak-qemu: error: GNU timeout not found" (Actions run 35711014679). The harness required `timeout --version` to mention "GNU coreutils" (`soak-qemu.sh:475-476`, `:569-570` at e98e1ad), and 26.04's `timeout` does not: it is the uutils (Rust) coreutils Ubuntu now ships, as #192 verified. Merged before #192, CI would have produced no soak data at all. The runner also brings QEMU 10.2.1, against 11.1.1 on the Mac. Order: #192 (aa1f128, merged 2026-09-23) fixed the check, which delivers step 1a's `timeout` item; then #169 merges, then the CI soak runs on `main` at least 3 times to set the 26.04 baseline, all before step 1b's comparison. Renovate holds on the runner image and the toolchain follow (see "Fixed instrument").

-----

## Decision

**Decided by the owner on 2026-09-22 (issue #164): 1C and 2B.**

| Choice | Decided | Rejected | This ADR's recommendation |
| --- | --- | --- | --- |
| Decision 1: lock discipline | **C: Linux-style, class in the type.** `IrqSpinLock` (IRQs and preemption off) for the 9 IRQ-shared statics, `SpinLock` (preempt count) for the other 30, and a validator that checks the split. Step 2 delivers the IRQ class. Step 8 delivers the rest, after the crash-fix gate and outside PR #149's gate, unless the escalation rule moves it | A: IRQ-off spinlocks everywhere · B: preempt count, IRQ-off only for the IRQ-shared set | C: taken |
| Decision 2: FP/NEON policy | **B: hard-float kernel, eager full save.** The full FP/SIMD state (q0–q31, FPCR, FPSR; 528 B) is saved and restored at every EL1 IRQ entry and exit and at every thread switch, future EL0 threads included. Delivered by step 4 | A: softfloat kernel, EL0 FP later · C: hard-float with `kernel_neon_begin/end` (not viable on this target) | A, with B's saves as an interim: not taken |

The analysis of every option, the rejected ones included, stays under "Decision 1" and "Decision 2" above.

**Why 2B is acceptable.** It keeps the Phase 0 target decision (`docs/phases/00-foundation-and-tooling.md:63`), the hard-float ABI and the `CLAUDE.md` FPU-enable fact, and it adds no toolchain or target change. The price is the 528-byte save on every IRQ and every thread switch, two switches per IPC round trip, with a counter as the only tripwire. **Revisit** if the Gate 1 IPC latency budget (under 10 μs, `bench.rs`) regresses beyond its threshold in the soak's bench output. Step 4's acceptance checks this first, and 2A is the alternative on record.

**Owner confirmations (2026-09-22):**

1. **EL0 FP.** 2B applies to every exception entry. Syscalls and IRQs taken from EL0 (the lower-EL entries in `trap.rs`) also save and restore the full 528-byte FP/SIMD state eagerly.
2. **Gate 1 latency at step 4.** It is a gate only when both soak arms' mean load1 is below the load threshold. On a noisy host, re-run on a quiet host or in CI rather than fail the step.
3. **Architecture docs.** Choosing 1C approves amending `scheduler.md` §10.3 and `deadlock-prevention.md` §9.2 and §12. Each fix step amends the architecture text it changes, in that step's PR.

4. **#165 soak threshold: B.** PR #149 may merge after the crash fix only when 20 text + 20 gpu boots are all CLEAN.
5. **No instrument freeze.** Rule 04's session-start updates continue during the series; see "No instrument freeze" under the delivery plan.

Nothing is left open for the owner.

The fixed foundation (F1–F8) and the delivery order did not depend on either choice. The choices set the contents of step 4 (2B) and step 8 (1C). The 2026-09-24 amendment (#185, #189; see "Review notes") moves F5's wake token and the blocking-site conversions from step 6b to the capability-lifetime PR, which, by that ADR's choice, lands after step 1b, and adds the exit safe point to step 8.

-----

## Evidence limits

- Every mechanism above comes from reading the code. None has been confirmed at runtime; step 1b exists to do that.
- The fatal-report lines come from `target/soak/167`. The baseline run's logs (ELF 59e095c0) are no longer on disk. The ELR values in the reports have not been resolved to symbols, because that needs the matching ELF, and no tool was run on the ELFs.
- The FP disassembly was done by a research pass on an older debug ELF and toolchain. Whether the zero-fill at `timeout.rs:89` and the `GpuBufferHandle` copies use V registers on nightly-2026-09-22 is unconfirmed, pending step 1b's listing.
- Two claims about softfloat are inferred and need a build to confirm: that the crypto crates' hardware paths fail to build without the `*_backend="soft"` cfgs, and that the rustc target features behave as described. They bear only on the rejected option 2A.
- The cost of 2B's 528-byte saves on AIOS is unmeasured. The seL4 figure is from another kernel and core, and step 4's Gate 1 comparison is the first measurement.
- The heartbeat-alive WEDGE has five candidate mechanisms. Step 1b's counters are needed to pick between them.
- That ubuntu-26.04's `timeout` is the Rust coreutils is inferred from the error message, not checked.
- Prior-art citations (Linux, seL4, Zircon, Redox, Theseus, Asterinas) come from the research pass and were not re-read for this ADR.
- The PR #149 branch was not read beyond its description and file list.

-----

## Review notes

**Amendment, 2026-09-24 (#185, #189).**

On 2026-09-24 the owner answered the open questions of [the capability-lifetime ADR](2026-09-24-jl-capability-lifetime.md) in two sets. First-set answer 3 (#185, #189) pulled F5's wake token out of step 6b and into that ADR's PR, so that the PR closes #189's window 2 in `process_wait`. Second-set answer 2 (#185) extended the token to every other blocking site and required step 1b's N2 measurement constraint to be resolved first. Second-set answer 3 (#185) added the exit safe point to step 8. Items marked *(capability-lifetime ADR choice)* are that ADR's choices, not owner decisions. Decisions 1C and 2B and owner confirmations 1–5 stay as they are. The fixed foundation (F1–F8) keeps its content: F5's two bullets and the last paragraph of "Decision" gain only a pointer to this note.

- **The wake token moves to the capability-lifetime PR (first-set answer 3).** The plan gave step 6b the part of F5 that closes the register-then-block gaps. That PR now delivers:
  - the per-thread wait word and its state machine, host-tested in `shared/` (the "wake-token state machine" under "Rules for every step PR");
  - `prepare_block` and `cancel_block`, and the token rules in `unblock`, `block_current` and `try_direct_switch`;
  - arm, re-check and loop in `process_wait` (#189's window 2).
- **Every other blocking site converts in the same PR (second-set answer 2):** `ipc_call`, `ipc_recv`, `sleep_ticks`, `notification_wait` and `ipc_select`, which covers N2 and N3. With them the PR delivers F5's notification bullet *(capability-lifetime ADR choice)*. `check_notification_timeouts` clears expired deadlines under its `try_lock` and calls `unblock` only after releasing `NOTIFY_DEADLINES`, so no expiry is dropped (`notify.rs:317`, `:321-323`). `SELECT_WAITERS`, `try_wake_select`, `set_select_ready` and `NOTIFY_RESULTS` are deleted: a select registers on its sources, and wakers claim those registrations *(capability-lifetime ADR choice)*. The PR's rules for a waiting site replace the register-then-block rule that F5 cites (`developer-guide.md:2060`).
- **Step 6b keeps the rest of F5:**
  - N1: `try_direct_switch` marks the receiver Running, and `schedule()` requeues on "was current and not blocking". A Dead thread counts as blocking;
  - N7: `unblock` prefers the thread's last CPU;
  - N10, only if step 1b's starvation scan implicates it.

  Step 6b now needs the capability-lifetime PR as well as 6a. Its acceptance does not change, except that its `ipc_reply` skip line becomes a reported count, not a gate: step 6b passes on the scan line above it, the blocked-with-no-waker scan reading 0 *(capability-lifetime ADR choice)*. The reason: from that PR on, "`unblock` skipping a Running target" means the `Skip` outcome only, and token outcomes are counted separately. Once every site arms, a `Skip` finds the waiter outside an armed window, between passes or after its wait (for example after a timeout that raced the reply). Its next pass re-checks after arming, so a `Skip` is stale, not lost, and the blocked-with-no-waker scan is the lost-wakeup signal.
- **Step 1b lands first, so N2 is still seen before its fix.** "Gains" requires each mechanism's counter to be seen above 0 before its fix, and second-set answer 2 requires that constraint to be resolved first. To resolve it, step 1b merges before the capability-lifetime PR *(capability-lifetime ADR choice)*, and both arms of that PR's soak pair carry 1b's counters:
  - the before arm, post-1b `main`, counts N2 and N3 as `unblock` skips by caller (`ipc_reply`, `ipc_send`, the `ipc_call` fallback, `check_timeouts`) on waits that are not yet converted;
  - the after arm counts a wake that finds its target armed as `LeaveToken`, by caller.

  This is a comparison within one pair, as "No instrument freeze" requires. A source that reads 0 in all 30 boots of the before arm is recorded as "not observed at n = 30"; its conversion then stands on the host model and its negative controls, and the result is recorded on #164. The capability-lifetime ADR's 1,084-line draft placed that PR between steps 1a and 1b. That order is dropped. Step 1b's content and acceptance do not change; that PR extends 1b's counters and scans for the outcomes and states it adds. The PR's pair also reports WEDGE-ALIVE and DEGRADED per arm, so the heartbeat-alive WEDGE can still be attributed to the token (that PR), to F4 (6a) or to the rest of F5 (6b). The series still has 10 PRs. That PR's pair is one extra soak.
- **Step 2's inventory changes.** That PR takes `NOTIFICATION_TABLE` out of the IRQ path and deletes `SELECT_WAITERS`, as its select and notification redesign requires *(capability-lifetime ADR choice)*, so the IRQ-shared set under Decision 1 ("Common to all options") becomes 7 statics, and the `NOTIFY_DEADLINES` nesting listed there goes. It adds four IRQ-masked helpers with eight inline DAIF sites in `scheduler.rs`, 49 in total. One of them, `current_tid()`, is how every converted wait reads its own tid *(capability-lifetime ADR choice)*, because `prepare_block` must be given the caller's own tid and N4 makes today's unmasked read unsafe; `with_this_cpu` replaces it. The PR keeps "Use `try_lock()` in IRQ context, never blocking lock" in `developer-guide.md` (`:2061`) and only appends to it, so the replacement listed for step 2 still applies. Whichever of that PR and step 2 lands second adjusts the lock classes and the §3.3 rebuild.
- **Step 8 gains the exit safe point (second-set answer 3).** `process_exit` can mark a process's other threads Dead while one of them, a victim, is still running on another CPU. The victims include the caller's own sibling threads when a process exits itself. Step 8's preempt count keeps a victim on its CPU while it holds a `SpinLock`. It does not stop the victim from running on after that and re-publishing per-thread state that the exit has cleared: `waiting_receiver`, `pending_caller`, `REPLY_SLOTS`, a notification or select waiter record, a timeout or deadline, or an armed wait word. That is because `preempt_enable` reaching 0 does not reschedule (Decision 1 recommendation, item 5). Step 8's scope adds:
  - an exit-pending flag per thread, set where `process_exit` marks the thread Dead;
  - checks of the flag at `irq_exit`, at syscall return, and on entry to `prepare_block` and `block_current`. A flagged thread leaves the CPU as Dead, with its wait word reset, only where it holds no lock: at `irq_exit` where the context is preemptible, at syscall return, and on entry to `block_current`. `prepare_block` may be called inside the critical section that publishes a waiter record (that ADR's rule 1 allows inside or before it), so there the check does not arm and never leaves the CPU; the thread leaves at its next `block_current` or syscall return *(capability-lifetime ADR choice)*;
  - `process_exit`'s step C (channels and thread-keyed state, in the capability-lifetime ADR), deferred until every victim has reached a safe point, so that it also removes anything a victim published before then.

  The checks need to read the current thread without a lock, and step 8's `ThreadInfo` provides that read. The item builds on the capability-lifetime PR's `prepare_block` and `process_exit`. Step 8 comes after that PR in either order, because 6b needs the PR and the escalation rule acts only after 6b. If the escalation rule moves step 8 ahead of step 7, the safe point moves with it. Until step 8 lands, the capability-lifetime ADR's interim rule holds: `process_exit` is called only when no victim other than the caller can be on a CPU, which covers a self-exit's sibling threads as well as another process's threads, and no new caller of that kind is added. Step 8 lifts the rule.
  - **Added acceptance** *(capability-lifetime ADR choice)*: a boot self-test exits a process whose thread is running on another CPU. That thread reaches a safe point, and afterwards no waiter record of it remains. The capability-lifetime PR's wait-word count (`bw`: threads whose wait word is not Idle while their state is neither Running nor Runnable) leaves out Dead threads until the safe point exists. From step 8 it counts Dead threads too, and it reads 0.
- **Other constraints.** Steps 2, 6a, 6b and 8 rewrite `thread_yield`, `block_current`, `unblock`, `schedule` and `direct.rs`; those rewrites keep the capability-lifetime PR's host-tested `may_become`, `wake_action` and `block_action` intact. The capability-lifetime ADR lists the other constraints its PR places on steps 1b, 2, 6a, 6b and 8.
- What this amendment changes above. Each is an in-place edit, so citations of this ADR by line number still hold:
  - the intro;
  - F5's N2 and N3 bullet, and its notification bullet;
  - the 6b and 8 rows of "Order and why", and its bullet on steps 6a and 6b;
  - the step 6b heading, and step 6b's `ipc_reply` skip line;
  - the step 8 heading, and step 8's acceptance line;
  - the last paragraph of "Decision".

  Separately from this amendment, the same change updates the `developer-guide.md` citations in F5's N2 and N3 bullet, step 2, "Docs to update" and "Expected conflicts" to their lines at b07d7e4 (`:2053` to `:2060`, `:2054` to `:2061`, `:1894` to `:1901`); every other `path:line` citation stays at e98e1ad. The #169 bullet under "Other PRs" now records that #192 delivered step 1a's `timeout` check. The ADR stays `final`.

**Owner decision, 2026-09-22 (#164).**

- The owner chose 1C, as this ADR recommended. The owner chose 2B, a hard-float kernel with eager full save, not the recommended 2A.
- The owner's 2B saves the full 528 bytes at every thread switch as well as at every EL1 IRQ entry. The draft's B saved only d8–d15 and FPCR, about 72 B, at the switch.
- What this revision changes:
  - It records both decisions.
  - Step 4 becomes the permanent FP step, with both save points, 16-byte alignment and a Gate 1 latency check.
  - The softfloat step (the old step 7) is removed. The final gate is renumbered to step 7 and the lock classes to step 8.
  - The doc changes that only softfloat needed are dropped.
  - The ADR moves to `final`.

**Earlier revision.** It addressed two reviews (fable, 15 points; opus, 16 points). Each point was checked against the code at f0b4169, the `target/soak/167` logs and the GitHub state on 2026-09-22. Most were applied as proposed. These were rejected or changed:

- **fable, H5 re-rank: the PANIC fits H5 "at least as well as H1".** H5 is re-ranked as a live candidate, as asked. The claim itself is not adopted. The eight `frame.rs:51` reports on disk show a whole `GpuBufferHandle` wrong, with non-zero, pointer-like values. That fits a wrong frame at least as naturally as a torn copy. Step 3 against step 4 decides.
- **fable, H5: cite the objdump before keeping the verdict.** Not done. This revision was limited to read-only investigation while a soak ran on the host. Instead, no verdict relies on the missing objdump, and the listing is step 1b's job.
- **fable, H5: the 528-byte save in step 2 (the reviewed draft's numbering).** Moved early, but into its own step 4, right after the frame fix, so H1 and H5 can be told apart. Merging it into step 3's PR remains an owner option.
- **fable, N7: "moves the thread there with probability 3/4".** Rejected. CPU 0 runs `check_timeouts` right after advancing `TICK_COUNT`, in the same handler (`timer.rs:178`, `:194`), so it usually wins. Other CPUs win only when CPU 0's `try_lock` fails or its handler is slow. The rest of N7 is adopted.
- **fable, F7: set TCR_EL1.EPD0=1.** Rejected in favour of the other option in the same point, an all-invalid TTBR0 table, because `CLAUDE.md:72` forbids changing TCR with the MMU on.
- **fable, fatal reports: "no non-PCZERO EXCEPTION or PANIC report was findable".** Outdated. The soak was still running when that review was written. Run 167 now holds 9 PANIC boots and 4 EXCEPTION boots, and their first fatal reports are cited above. The resulting N5 verdict is "possible; not observed".
- **fable, the H3 window "open from the first tick".** Adopted for gpu mode only. The GPU service, compositor and input threads exist only in gpu mode (`main.rs:303-341`), and all 9 text-mode heartbeat-stuck boots in run 167 stopped after the bench header.
- **fable, reorder so step 4a comes first, and opus, move 4b off the critical path.** These two points ask for different orders. Both are adopted: step 2 (old 4a) comes first, and step 8 (old 4b) follows the gate. **opus's reason** is only partly adopted: "lock-holder preemption on those 30 cannot deadlock" holds for IRQ context. It is qualified, because strict class priority can starve a preempted lower-class holder (N10). Hence the escalation rule.
- **opus, rules for every step: "the 4a check can never fire if the lock masks IRQs itself", and fable, "keep the validator in release".** Both adopted, and reconciled: `IrqSpinLock` masks IRQs itself, and the checks that remain (held rank, and from step 8 the class checks) stay on in release.
- **opus, CI soak: "its check is therefore always green".** Partly wrong. The check is green whenever the harness runs, but setup failures make it red, as on #169. The rest is adopted.
- **opus, fixed instrument: `CLAUDE.md:13` and `justfile:7`.** The lines are `CLAUDE.md:15` and `justfile:8`; the substance is adopted.
- **opus, other PRs: rebase `claude/crash-fix-adr` onto dd05a1e.** Done as a fast-forward, which by then also brought in e98e1ad (#161), a toolchain-only commit.
