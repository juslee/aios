---
author: jl + claude
date: 2026-10-29
tags: [kernel, security, ipc, memory, sched, smp, tooling]
status: final
---

# Lesson: Capability lifetime: wait tokens, kill points, and measuring a PR on a kernel that still crashes

The capability-lifetime PR (#266) put generation-numbered ids, derived access tokens, the region cascade, process exit and reaping, and a wake token on every waiting path into one branch, `claude/cap-lifetime`. The decisions, the design, the implementation changes and the soak results are in [the capability-lifetime ADR](../decisions/2026-09-24-jl-capability-lifetime.md): see its "Review notes" (notes on the implementation PR and the soak pair) and "Evidence limits". The per-step detail is in [the working plan's last version](https://github.com/juslee/aios/blob/0bc9fe9e3d1a80af9c85b32c7e35165589f60236/docs/knowledge/plans/2026-09-24-jl-capability-lifetime-plan.md). The lessons below apply beyond this PR.

## 1. Arm before you publish any waker record, and loop on the condition

**What happened.** Every lost wakeup the PR closed (#189 window 2, crash-fix N2 and N3, #186 item 4) had the same shape. The waiter published a record a waker could find: a `TIMEOUT_QUEUE` entry, a notification deadline, `pending_caller`, `waiting_receiver`, a waiter record or a `PROCESS_WAITERS` entry. It reached `block_current` only after that. A wake in between found the waiter still Running, `unblock` skipped it, and the waiter then blocked with no waker left. Timeouts and deadlines count as records too: a timeout that fires between its own insertion and the arm loses the wake the same way.

**How to apply.** Arm (`prepare_block`) inside or before the critical section that publishes the first record. After arming, end every pass in `block_current` or `cancel_block`. When `block_current` returns, re-check the condition under its own lock and go round again. Results come from state, not from the wake: a stale wake costs one pass, not a wrong result. The rules are in `prepare_block`'s doc comment, and `block_current` asserts the arm from the `ipc_call` step on.

## 2. A thread's own tid must be read with IRQs masked, or on a pinned thread

**What happened.** `current_thread_id()` reads `core_id` and then `CURRENT_THREAD[core]` with IRQs on. A migration between the two reads names another CPU's thread, and an arm with that tid leaves a token on the wrong thread.

**How to apply.** Read the tid once per wait through `sched::current_tid()`, which masks IRQs across both reads, or run the code on a thread pinned to one CPU. Mask through `daif::with_irqs_masked`, which nests, not with a new inline `DAIFClr` site: the four helpers this PR added (`current_tid`, `current_thread_and_pid`, `kill_process_threads`, `thread_probe`) add none, so the crash-fix DAIF inventory did not grow.

## 3. An id layout needs a strict malformed range

**What happened.** The new ids carry a slot in bits 0–7 and a generation in bits 8–31, and generation 0 is never issued, so no valid id is below 256. The boot tests' out-of-range sentinels (raw 128 and `u32::MAX`) land in the malformed slot range (128–255 for `ChannelId`), so they stayed invalid without any edit to the tests.

**How to apply.** When a layout gains fields, keep a value range that no valid id can reach, so the old sentinels stay invalid and no caller needs to change them. Check every site that builds an id by hand in the id types' own module, where a private field does not make the compiler flag it.

## 4. An EL1 reader of user memory needs a borrow

**What happened.** A force-unmap removes a region from user page tables, but every thread runs at EL1 and can still reach the frames through the direct map. Freeing the frames while the bench read them through `region_dmap_addr` would have been a use-after-free that no page fault reports.

**How to apply.** Kernel code reads region memory only under a `RegionBorrow`. A destroy while a borrow is held unmaps and bumps the generation at once, and the frames are freed at the last `region_release`.

## 5. At EL1, holding no lock does not make a kill point safe

**What happened.** A thread can be switched off as Dead after releasing a lock but before a wake, frame free or publish that it still owes ("owed work"). It then never finishes the work, and exit step C cannot recover it, because the victim has already taken the channel or the waiter record. Once Dead is terminal, a Dead-marked thread switched away inside a `spin::Mutex` critical section never releases the lock either. For a victim that was Runnable at the kill, the old pick had hidden this by writing Running over Dead.

**How to apply.** Until crash-fix step 8 adds the preempt count and meets the owed-work requirement, exit only processes whose other threads cannot be Running or Runnable at the kill. Making a state terminal can turn a hidden revival into a lost lock, so list the paths that used to revive.

## 6. On a kernel that still crashes, judge each check on its own and attribute every failure

**What happened.** The branch was built on a kernel with known pre-existing faults: H1 register corruption on the IRQ path (PC-zero reports, wild pointers), H3 lock re-entry from IRQ exit, and a bad-free PANIC in gpu mode. Most per-step gate boots died, and the soak pairs had few CLEAN boots: text 13/20 on `main` and 9/20 on the branch, and gpu on the CI runner 0/20 in both arms.

- The self-test lines print about 6 s in, before the bench, so a step's checks were judged on any boot that printed them. A boot that died first was re-run on the same sha and noted. Lines from a pinned thread were never waived, and a boot was never re-run just to get a luckier result.
- The text pair's gate 1 (one-sided Fisher on CLEAN) passed at p = 0.17. Gate 3 attributed every non-CLEAN branch-arm boot. Three of them fit no pre-registered signature, and a read-only source diagnose traced all three to H1: the IRQ stub returns with the live `ELR_EL1`/`SPSR_EL1`. The owner ruled them H1 and added the signature to the table.
- In gpu mode on the CI runner, no boot was CLEAN even on `main`, so gate 1 could not fail there. Only attribution carried information.
- The branch raised exposure without adding a cause: more CPU-2 spinners, a longer teardown, and the bench pinned to CPU 0. That showed as more H3 re-entries (PANIC-LOCK 1 vs 4 in the CI gpu pair) and more first faults on CPUs 1 and 3.

**How to apply.** Pre-register the attribution signatures before the pair runs. Send any boot that fits none of them to a read-only diagnose and then to the owner, and never read it as noise. Record that a recurrence after the fix step (crash-fix step 3 for H1) would mean a branch bug. A CLEAN-rate gate with a near-zero base rate is not a gate: report it, and rely on attribution.

## 7. A dying boot loses log lines it had already produced

**What happened.** Step 14's first four boots died before `Call-loop` printed, and the missing line looked like a step-14 failure. Only CPU 0's tick drains the log rings to the UART (`arch/aarch64/timer.rs`, every 4th tick), so lines that CPU 2 had produced but CPU 0 had not yet drained die with the boot. Separately, UART output from several CPUs is unlocked, so a line can be torn by another CPU's output. A torn line can be heavy: one `EPIPE` line was spread through a whole tripwire dump.

**How to apply.** In a boot that dies, a line that never printed carries no information. Re-run the boot, and do not read the missing line as a failure. Before counting a line as missing, read the log by hand for a torn copy. A soak `run-NN.log` is binary-detected and sits under gitignored `target/`, so search it by explicit path or with `command grep -a`.

## 8. A hung bench holds tripwire lines back

**What happened.** `bench.rs` sets `CONSOLE_BUSY` (`tripwire::set_console_busy(true)`) when it starts printing its Gate 1 block, and clears it only on its early-failure exit and at the end of the block. A bench that hangs in between therefore leaves it set, and CPU 0's heartbeat tripwire lines stay held back for the rest of the boot. This was seen in 5 of 13 boots.

**How to apply.** Read `hbdefer == hb_count` in a tripwire line as the "bench hung" signature, not as a lock problem.

## 9. Plan host-quiet windows and test budgets with the host in mind

**What happened.**

- The local quiet-mode gpu pair was deferred three times on 2026-10-28. Host load ran from 12 to 158, mostly iCloud `bird` and `fileproviderd`, with VS Code's `rg` and a CodeQL Java process at other times.
- The owner took the CI `soak-matrix` run as the gpu pair for now. It uses its own baseline (x86 TCG, 90 s boots) and is not pooled with the local arm64 pairs. The local pair is still owed.
- CI's Miri job ran past its 6 h cap after the host models gained more tests. The 28 pure-logic model tests contain no `unsafe` and now skip under Miri (`#[cfg_attr(miri, ignore = …)]`), and they still run in `just test`.

**How to apply.** Before asking for a quiet window, check the host's load sources and ask the owner to pause them. Keep a CI substitute ready, and record which baseline it uses. Keep pure-logic model tests out of Miri: Miri checks `unsafe`, and they have none.

## 10. Writer areas shape commits, and append-only catalogues avoid schema bumps

**What happened.** Under rule 11, kernel-dev writes `shared/` and worker writes `tools/`. So a step that changed the tripwire catalogue in `shared/` and the soak goldens in `tools/` had to land as two commits, pushed together so that no pushed head failed CI's `tools` job. Steps 9 and 11 did this. The schema went to `v=2` only once, at step 11, because the earlier catalogue changes only appended keys or trailing indices. A trailing index or key that a line lacks reads 0, so `v=1` lines kept decoding.

**How to apply.** When a change crosses writer areas, plan it as consecutive commits and fast-forward them together. Order catalogue changes so that appends come first and every shift lands in one bump. Keep a frozen copy of the old catalogue (`tools/src/cmd/soak/schema_v1.rs`) for old logs.

## 11. A whole-range simplify pass and a security scan find real defects

**What happened.**

- The simplifier pass over the PR range found that `unblock`'s empty-slot branch unmasked IRQs with `THREAD_TABLE` held. The bug predated the PR and was fixed in 5be44df.
- Audit round 1 found that `ipc_cancel` had no kernel-side test. The Cancel-loop test was added.
- A security scan found that the Kit `KernelCapabilitySystem::grant` skips the 64-slot foreign reserve. It is dead code that nothing constructs, but it would become a capability-table exhaustion path as soon as it gained a caller.

**How to apply.** Run the simplifier over the whole PR range, not only over the last step. Treat a dead trait implementation that bypasses a security check as a latent bug: record it, and give it the check, or route it through `share_decision`, before it gets a caller.
