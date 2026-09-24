---
author: jl + claude
date: 2026-09-24
tags: [kernel, sched, smp, ipc]
status: in-progress
phase: crash-fix
milestone: step-1b
---

# Plan: Crash fix step 1b — kernel tripwires (detect-only)

## Approach

Step 1b of the [boot-crash fix ADR](../decisions/2026-09-22-jl-crash-fix-preemption-and-fp.md) adds detect-only instrumentation: counters, scans and fatal-dump context that let each later fix step show its mechanism firing before the fix and stopping after it. Nothing here changes scheduling behaviour, except removing the bench's own DAIF masking (F6) and one behaviour-neutral overflow guard in the balancer (Design §2.12).

**Owner decisions (2026-09-24).**

- Step 1b starts now; step 1a (the harness classes) waits for the R4 soak port to Rust, so the ADR's interleaved A/B acceptance soak for 1b waits too.
- Per [#185](https://github.com/juslee/aios/issues/185#issuecomment-5807013317), step 1b must land the N2 lost-wakeup counters and record a single-arm baseline soak with them (task B1) before the #163/#185 work converts the blocking paths.

**How this plan was made.** Eight parallel readers mapped every site at `aa1f128` (the kernel code at `b07d7e4` differs only in comment text). A lead engineer synthesised the design. Four adversarial reviewers (lock protocol, IRQ assembly, scans and deadlock, measurement validity) raised 27 findings (0 blockers, 8 major, 19 minor), all verified against the code and dispositioned in Design §6.

## Progress

- [ ] S1: `FixedQueue::iter()` / `contains()` (shared, host tests)
- [ ] S2: `shared/src/lock.rs`: owner stamp, `classify`, `StampedLock`, `LockClass` (host tests, Miri)
- [ ] S3: `shared/src/tripwire.rs`: key catalogue, `WakeSource`, `CpuCounters`, line writer, `classify_pc`, scan classification, two strikes (host tests)
- [ ] K1: Bench DAIF masking removed
- [ ] K2: Tripwire runtime, per-CPU ticks, heartbeat and g1 lines
- [ ] K3: Dispatch bookkeeping: switch generation, `CURRENT_TID`, `IRQ_CTX`, last CPU, `schedule(origin)`
- [ ] K4: IRQ frame ELR/SPSR snapshot and mismatch counter (192-byte frame)
- [ ] K5: `IrqSpinLock` (detect-only) on the 9 IRQ-shared statics
- [ ] K6: Wake attribution and the N2 counters (`unblock`/`wake_with_error`/`try_wake_select` sources, call phase)
- [ ] K7: Restore-site PC/SP checks (N5)
- [ ] K8: Heartbeat scans: orphan, no-waker, starved (two strikes, wake-pending markers)
- [ ] K9: Fatal dumps (panic handler masks IRQs first) and `#[track_caller]` on the free paths
- [ ] K10: `[smp]` VBAR/TTBR0 lines
- [ ] T-self: `tripwire-selftest` feature: end-to-end PANIC-LOCK proof (not in either soak arm)
- [ ] V1: V-register listing: differential gate over the IRQ call graph (no repository file)
- [ ] D1: Docs sweep and ADR errata
- [ ] B1: Single-arm N2 baseline soak on the final 1b head (owner gate; host-exclusive, coordinated with aios-b9)

Every task: one commit `Crash fix step 1b: <description>`, `just check` with zero warnings, `just test`, and for kernel tasks one text and one gpu boot (`just soak runs=1 secs=75 report_only=1`, then `mode=gpu`) with the task's boot acceptance from Design §3.

## Design (revision 2)


**Baseline:** worktree `crash-1b`, branch `claude/crash-fix-step-1b-tripwires`, at `main` `b07d7e4`. The kernel code there is identical to `aa1f128` apart from comment text, so every `aa1f128` line number below still holds. `rust-toolchain.toml` at `b07d7e4` pins `nightly-2026-09-24`, which reports `rustc 1.100.0-nightly (6eeff9a52 2026-09-23)`.

I read these ADR sections myself: F1–F8 (lines 137–207), Decision 1 (209–299), the soak protocol, "Rules for every step PR" (398–402), step 1b (414–434), and N2/N7 (lines 98, 103). I also re-read the code at every site this design depends on. None of the files on the IRQ path, the context switch, the timer, the panic handler or the exception handler changed since `f0b4169`. The drift is in `ipc/channel.rs`, `ipc/mod.rs`, `ipc/select.rs`, `task/process.rs`, `ipc/tests/` and `scripts/soak-qemu.sh`.

**Owner constraint (2026-09-24):** step 1b must land the N2 lost-wakeup counters and a single-arm N2 baseline soak on the 1b kernel (task B1), because that baseline gates other work. The counters required are "unblock skipping a Running target" by caller (above all from `ipc_reply`) and the blocked-with-no-waker scan. This revision also adds exact N2 phase counters (§2.4) so that the baseline can tell a real N2 from a late reply.

---

### 1. Scope check: each ADR step-1b bullet, where it lands now, and what changed

| ADR bullet | Current site | What is new, harder or different (evidence) |
|---|---|---|
| **Detect-only IRQ-class lock on the 9 statics** | THREAD_TABLE `task/mod.rs:209-212`; CURRENT_THREAD `task/mod.rs:220-224`; RUN_QUEUES `sched/mod.rs:88-92`; TIMEOUT_QUEUE `ipc/timeout.rs:25-28`; WAKEUP_ERRORS `ipc/timeout.rs:66`; NOTIFICATION_TABLE `ipc/notify.rs:57-58`; NOTIFY_DEADLINES `ipc/notify.rs:294`; SELECT_WAITERS `ipc/select.rs:32-33`; BOOT_LOG `observability/mod.rs:352` | **(a) The owner cannot live in a field beside `spin::Mutex`.** Either release order is wrong. Clearing the owner and then unlocking lets a tick in between see "held, no owner". Unlocking and then clearing lets another CPU's acquisition leave a stale owner behind. The owner stamp has to be the lock word itself (§2.1).<br>**(b) "IRQ-context acquisition" includes the preemption path.** The main blocking re-entries are `scheduler.rs:164` (CURRENT_THREAD) and `:168` (THREAD_TABLE), reached through `gic.rs:262` → `check_preemption` → `schedule()`. The hardirq work reaches blocking re-entries too: `scheduler.rs:371` through `check_timeouts` → `wake_with_error` → `unblock`, `init.rs:257` in the balancer, and `timeout.rs:133`. The test is therefore "owner = this CPU and generation unchanged", whatever the context.<br>**(c) Only THREAD_TABLE, CURRENT_THREAD and WAKEUP_ERRORS can reach the panic.** The IRQ path takes these blocking, and thread code holds them with IRQs on. RUN_QUEUES has no IRQs-on holder. The other five are only try-locked from IRQ context.<br>**(d)** ADR:213's site list omits `scheduler.rs:137`, `:208`, `:218`, `:244`, `:267`, `:371`, `:407` and `timeout.rs:155`. |
| **PANIC-LOCK prints `PANIC: lock re-entry: <LOCK> on CPU n`** | Panic handler `main.rs:390-397` | It cannot be one line. `writeln!("PANIC: {}", info)` prints `PANIC: panicked at <file:l:c>:` and puts the message on the next line (run-167 `main-gpu-r1/run-01.log:205-206`). The classifier attaches that next line (`soak-qemu.sh:185-188`). Step 1a matches `lock re-entry: ` in the attached message, and the ADR needs an erratum. |
| **`irq_spin_preempted_holder[lock]`** | lock slow path | Three extensions (§2.1):<br>• A holder switched out and resumed on the **same** CPU classifies PreemptedHolder, not Reentry.<br>• A holder preempted with IRQs on and resumed on **another** CPU keeps its stale stamp. An IRQ there that re-enters classifies "other CPU" (L2).<br>• PreemptedHolder does not mean the holder is off-CPU (L3).<br>All three need a consistent `holder_tid` snapshot compared with `CURRENT_TID`. The results are counted and printed as events; they never panic. |
| **ELR/SPSR mismatch counter "before `eret`"** | `exceptions.rs:145-170` (`irq_el1_entry`, 176 B frame) | The compare runs after `bl irq_handler_el1` returns and before the register restore at `:159`. The frame grows to 192 B. |
| **Per-CPU ticks** | `timer.rs:158-176` | Straightforward. `TICK_COUNT` advances only on CPU 0 (`timer.rs:178`); the comment at `:111` is wrong. |
| **IRQ-context switches** | commit `scheduler.rs:244-250`; the only IRQ caller is `:431` | `schedule(origin)` with 3 callers (`:314`, `:343`, `:431`). The no-current-thread dispatch from IRQ (`:281-287`) is counted separately. |
| **Cross-CPU direct and reply switches** | commits `direct.rs:152-153`, `:284-285` | Needs a new per-slot last-CPU stamp (§2.3). `cpu` at `direct.rs:55`/`:206` is read before the mask (N4). |
| **`unblock` skipping a Running or Runnable target, by caller** (owner-required N2 counter) | `scheduler.rs:375-383` | The caller list is incomplete. There are 10 `unblock` sites, 5 `wake_with_error` callers and 3 `try_wake_select` callers (§2.4). `ubrun[reply]` alone **cannot separate N2 from a late reply** to a caller its timeout has already woken (`timeout.rs:105` → `channel.rs:182-206`), so per-thread call/recv phase counters are added (§2.4). The `_ => {}` arm at `:384` revives Dead/Suspended/BlockedTimer/BlockedIo threads, and an empty slot returns at `:389-395`. Both are counted. |
| **`unblock` queueing off the last CPU** | `scheduler.rs:388`, `:398-407` | Uses the same last-CPU stamp. |
| **Saved PC or LR outside kernel VA (N5)** | restore sites `scheduler.rs:94`, `:280`, `:286`; `direct.rs:186`, `:312` | Check "inside `[__text_start, __text_end)` and 4-byte aligned". The observed bad PCs are in the direct map, which is inside kernel VA. **The bounds must be VAs captured once in `kernel_main`, not symbol addresses computed on the IRQ path**, which run at physical-alias PCs on CPUs 1–3 (§2.11). An LR check adds nothing (`context_switch.S:31`, `:37`). |
| **Heartbeat scans** (owner-required N2 no-waker scan) | `timer.rs:183-187` today; moved to the end of `timer_tick_handler` (§2.5) | Harder:<br>• `FixedQueue` has no iterator.<br>• There is no last-run tick.<br>• The ADR's waker rules are wrong in three ways: SELECT_WAITERS is not a waker, NOTIFY_DEADLINES counts only for Notification/Select, and sleep is `BlockedIpc{u64::MAX}`.<br>• The ADR's transit windows are incomplete.<br>• **Two strikes alone does not remove waker-in-transit windows**, because it keys on the flagged thread's `LAST_RUN` while the stream in transit is the waker. A per-target lock-free "wake pending" marker is needed (§2.6).<br>• CHANNEL_TABLE has no owner tracking. |
| **"Core N online" prints VBAR_EL1/TTBR0_EL1** | `smp.rs:205` | A separate direct-UART `[smp]` line inside the `PRINT_TURN` window, because `kinfo!` messages are cut to 48 bytes. CPU 0's line is printed after `main.rs:271`. |
| **Panic handler masks IRQs first (N8), prints CPU, thread, tripwire** | `main.rs:390-397` | It has no mask and no recursion guard. It must not lock CURRENT_THREAD, so it uses the lock-free per-CPU `CURRENT_TID` mirror. One final `drain_logs()` drains only 16 lines (`observability/mod.rs:252`, `:263`, `:266-267`), so the panic path loops (§2.7). |
| **Exception report adds SP, TTBR0, VBAR, thread** | stub `exceptions.rs:33-45`, handler `:277-318` | SP comes from the stub (`add x3, sp, #48`) and SPSR from `mrs x4`. The new fields go on lines after the Abort line. |
| **`free_dma_pages` and `free_pages` become `#[track_caller]`** | `frame.rs:49`, `:164-168` | Both are needed. The location key `frame.rs:51` then disappears, so step 1a and the ADR must key on `[mm] BUG: free_pages(`. |
| **V-register listing of the soak ELF** | objdump only | The toolchain is now `nightly-2026-09-24` (`6eeff9a52`, #193). Use sysroot `llvm-objdump`. **Gate the 1b arm differentially** (source-line parity over everything reachable from `irq_el1_entry`), not "0 sites in new symbols": at opt-level 1, new code inlines into existing symbols and can call `memset`/`memcpy` (§3 V1). |
| **Bench DAIF masking removed (F6)** | `bench.rs:178-179`, `:236-237`, `:249-250` | A near no-op. Keep the entry unmasks. |
| **Acceptance: every non-CLEAN boot has a tripwire line or fatal dump** | — | Cases that need care:<br>• The `channel.rs:249` PANIC signature can no longer occur (EINVAL at `ipc/mod.rs:152`, `cap/mod.rs:95-96`, `select.rs:66`), so `badchan` is a 3-wide key.<br>• A CPU 0 IRQ-context spin prints nothing after it starts, so there are event lines.<br>• Wedges before the first heartbeat, SError/FIQ `b .`, and a re-fault at the stub are known residuals. |

**Made unnecessary or easier by the current code:**
- The NC-memory rule does not constrain any of this. All statics are write-back from `boot.S:147-157`, and TTBR0 has been write-back since M8. BOOT_LOG's CAS already runs before `init_mmu`. New counters are still load/store only (§2.2).
- The ubuntu `timeout` fix is merged (#192).
- No code names `spin::MutexGuard` or uses `get_mut`/`into_inner`/`is_locked`/`force_unlock` on the 9 statics. A wrapper with `lock`, `try_lock`, `Deref` and `DerefMut` needs only declaration edits.

---

### 2. Design decisions

#### 2.1 The detect-only lock: `IrqSpinLock<T>`

**Name and place.** The kernel type is `kernel/src/sync/irq_spin_lock.rs` (new `mod sync;`). The exact core goes in `shared/src/lock.rs`, so that host tests and Miri cover it.

**Lock word = owner stamp** (in `shared::lock`):
```rust
// bit 63 HELD | bit 62 IRQS_ON (DAIF.I clear at acquisition) | bits 54..=61 cpu | bits 0..=53 switch gen
pub struct OwnerStamp(u64);                      // never 0
pub enum Contention { Free, OtherCpu, OtherCpuSwitched, Reentry, PreemptedHolder }
pub fn classify(observed: u64, cpu: u8, v: &impl CpuView) -> Contention; // ignores IRQS_ON
//   Reentry:          owner cpu == cpu && owner gen == v.switch_gen(cpu)
//   PreemptedHolder:  owner cpu == cpu && owner gen != v.switch_gen(cpu)
//   OtherCpu:         owner cpu != cpu && owner gen == v.switch_gen(owner cpu)   (Relaxed; count only)
//   OtherCpuSwitched: owner cpu != cpu && owner gen != v.switch_gen(owner cpu)
pub trait CpuView { fn cpu(&self) -> u8; fn switch_gen(&self, cpu: u8) -> u64; }
pub fn read_stamp(v: &impl CpuView) -> (u8, u64); // cpu, gen[cpu], cpu again; retry until equal
pub struct StampedLock<T> { word: AtomicU64, data: UnsafeCell<T> }
pub enum LockClass { ThreadTable, CurrentThread, RunQueues, WakeupErrors, TimeoutQueue,
                     NotifyDeadlines, NotificationTable, SelectWaiters, BootLog } // COUNT=9
pub const TID_NONE: u32 = u32::MAX;
```
One CAS takes the word from 0 to the stamp, and the unlock stores 0 with Release. This is the same read-modify-write and orderings as spin 0.12.3 (`mutex/spin.rs:233-240`), with the same test-and-test-and-set spin, `core::hint::spin_loop()`, and no fairness. The Reentry decision uses only the word and `SWITCH_GEN[this cpu]`. The cross-CPU `switch_gen` read in the other arms only splits a count.

**Kernel wrapper fields:**
- `inner: StampedLock<T>`
- `class: LockClass`
- `index: u8` (`0xFF` for scalars)
- `holder_site: AtomicPtr<Location<'static>>` (diagnostic)
- `holder_tid: AtomicU32` (diagnostic, initialised to `TID_NONE`)

**Diagnostic-field protocol (L1).**
- **Acquire:** the fields are stored **immediately after a successful CAS**, before the IRQs-on second `read_stamp`/`restamp`. `holder_tid` = `CURRENT_TID[mpidr_cpu]`, and `holder_site` = normalised `Location::caller()`.
- **Release:** the guard's `Drop` stores `holder_site = null` and `holder_tid = TID_NONE`, **then** `word.store(0, Release)`. A reader therefore never sees a previous holder's fields.
- **Reading:** a spinner takes a *consistent snapshot*: `w1 = word`, `tid = holder_tid`, `site = holder_site`, `w2 = word`. The snapshot is valid only if `w1 == w2 != 0`. Otherwise the fields print as `?`.
- **The residual window:** an IRQ that lands between the CAS and the field store, or between the clear and the release, sees `holder=?`/`tid=?` and still decides Reentry correctly. For those cases the `[panic]` line carries `irq_elr` (§2.7), which is the holder's exact PC.

**`CURRENT_TID[cpu]: AtomicU32`** is initialised to `TID_NONE`, never 0, because slot 0 is a real thread (`allocate_thread` fills from index 0, `sched/mod.rs:116-125`). `lkself` is counted only when `tid != TID_NONE`.

**No DAIF writes on the lock path in 1b.** Reading DAIF for IRQS_ON is allowed.

**Stamp accuracy without masking:**
- *IRQ context or IRQs masked:* read MPIDR Aff0, then `SWITCH_GEN[cpu]`.
- *IRQs on:* `read_stamp`, then CAS, then store the fields, then `read_stamp` again and `restamp` if it differs. A stale stamp can only err towards PreemptedHolder or OtherCpuSwitched, never towards Reentry.

**`lock()` (`#[track_caller]`):**
```text
s = stamp_now(); if weak CAS ok → set holder fields → (IRQs on: re-stamp) → guard
loop:
  w = owner_word(); if w == 0 → retry CAS
  match classify(w, cpu, gen):
    Reentry          → reentry_panic(self, w, ctx)      // #[cold] #[inline(never)] #[track_caller] -> !
    PreemptedHolder  → once per call: bump lkph (DAIF.I set) or lkpho (IRQs on) [class];
                       snap = consistent_holder();
                       if snap.tid == CURRENT_TID[cpu]            → bump lkself; print kind=self
                       elif snap.tid == CURRENT_TID[k], k≠cpu     → bump lkphrun (holder running on k; no line)
                       elif DAIF.I set                            → print kind=ph holder_running=none|?
    OtherCpuSwitched → once per call: bump lkoxp[class];
                       if snap.tid == CURRENT_TID[cpu] → bump lkself; print kind=self   (L2)
    OtherCpu         → nothing
  spin while owner_word() != 0 { spin_loop(); every 1024 iterations read CNTVCT:
       > 2 s since entry → once: bump lkstk[class]; print kind=stuck }
  (IRQs on: recompute the stamp, since the thread may have migrated while spinning)
```
- A `ph` line means "holder not running on this CPU and not found running elsewhere". It is **not** wedge evidence by itself (L3).
- A `self` line in ctx `irq`/`irq-exit` **is** a true same-stream deadlock, including the migrated-holder case that the stamp cannot see. It is count-only in 1b; step 1a treats it as H3 evidence (§5).
- `try_lock()` (`#[track_caller]`): one strong CAS. On failure, Reentry bumps `lktry[class]` and PreemptedHolder bumps `lktph[class]`. It never panics.
- `try_lock_quiet()` is for the heartbeat scans and never counts.

**Re-entry panics in every context; the context is a label.** The label comes from `IRQ_CTX[cpu]` (§2.3) and DAIF.I: `irq`, `irq-exit`, `thread-off` or `thread`. H3 evidence is `ctx ∈ {irq, irq-exit}` **and** `holder_irqs=on`. The ADR rule "≥3 WEDGE-STUCK and 0 PANIC-LOCK refutes H3" becomes "0 PANIC-LOCK with ctx ≠ thread*, and no `kind=self` event or `lkself` > 0 with ctx ≠ thread*" (§5).

**Panic message:** one line, at most 160 characters, lowercase keys, `?` for missing fields. The writer is `shared::tripwire::write_reentry_msg`, so the worst-case length is host-tested:
```
lock re-entry: CURRENT_THREAD[0] on CPU 0 ctx=irq-exit holder=kernel/src/ipc/timeout.rs:118 holder_irqs=on tid=12 gen=4711
```
`holder_site` is normalised before it is stored (§2.11).

**Const initialisation:**
- Use `[const { IrqSpinLock::new(..) }; MAX_CORES]` plus a `while i < MAX_CORES { a[i].index = i as u8 }` initializer block. The lock-protocol review verified this under `nightly-2026-09-23`/`-24` with `clippy -D warnings`.
- Drop the `const RQ`/`const NONE` items and their `allow`s.
- Remove `use spin::Mutex;` at `task/mod.rs:10`, `sched/mod.rs:16` and `select.rs:11`. Keep it at `timeout.rs:12` and `notify.rs:13`.

**Soundness statement, documented on the type (L4):** The Reentry test is free of false positives because:
- (a) every `restore_context` on CPU c is preceded, on c with IRQs masked, by `note_dispatch` bumping `SWITCH_GEN[c]`, with c read from MPIDR (5 restores ← 4 commits: `scheduler.rs:84→94`, `:244→280/286`, `direct.rs:153→186`, `:285→312`);
- (b) the dispatching stream releases every guard it took after that bump before `restore_context`. Those are the temporaries at `scheduler.rs:267`, `direct.rs:178`, `:290` (`enqueue_on_cpu`) and `:305`.

Guards held by a preempted thread across a switch are expected; they classify PreemptedHolder or OtherCpuSwitched.

**(b) is checked at run time, count-only:** `held_by_stream(cpu)` walks the 23 lock words through `for_each_lock_word(|w| …)`. That function takes references to the statics, which are only dereferenced, never compared or published. Any word stamped `(cpu, SWITCH_GEN[cpu])` found immediately before each of the 5 `restore_context` calls bumps `rsthold[cpu]`. Expected: 0. It is not a `debug_assert!`, because the soak build is a dev build and an assert would add a PANIC class.

#### 2.2 Counters: per-CPU, load/store only

- `shared::tripwire::CpuCounters<const CPUS: usize>`: rows of `[AtomicU64; SLOTS]`, `#[repr(align(64))]`. `add(cpu, slot, n)` is a Relaxed load + `wrapping_add` + store. The caller runs on `cpu` with IRQs masked. There is no `fetch_add`.
- `bump_masked(slot)` is for known-masked sites and has a `debug_assert!` on DAIF.I.
- `bump(slot)` is for sites that may run with IRQs on (lock slow path, `badchan`, reply/send fallbacks, `clear_timeout` busy): save DAIF, `DAIFSet #2`, bump, restore.
- Per-thread instrumentation arrays (`CALL_PHASE`, `WAKE_PENDING`, …) are single plain stores or loads, never read-modify-write.
- Do not use `metrics::Counter`. All arithmetic is `wrapping_*` or `saturating_*`, because the dev build has overflow checks.

#### 2.3 Dispatch bookkeeping, switch generation, IRQ context, last CPU

`tripwire::note_dispatch(tid, origin) -> u8` is called at the **4 `CURRENT_THREAD` commit sites** (`scheduler.rs:84`, `:244`; `direct.rs:153`, `:285`), right after the write, with THREAD_TABLE held and IRQs masked. It:

1. Reads `core_id()`. It bumps `n4` if that differs from the local `cpu` (`direct.rs:55`, `:206`).
2. Increments `SWITCH_GEN[mpidr_cpu]` (load + store; only that CPU writes it).
3. Stores `CURRENT_TID[cpu] = tid.0`, using the index `CURRENT_THREAD` used. **Invariant, documented here:** `CURRENT_TID` is written only at these 4 sites under THREAD_TABLE, so while THREAD_TABLE is held, `CURRENT_TID[k]` equals `*CURRENT_THREAD[k]` (SCAN-6).
4. Stores `IRQ_CTX[mpidr_cpu] = 0`.
5. Reads the old `LAST_CPU[tid]`, then stores `LAST_CPU[tid] = mpidr_cpu` and `LAST_RUN[tid] = TICK_COUNT`. **It returns the old `LAST_CPU`.** The direct and reply sites use it for `xdir`/`xrep`/`xnever`, and K7's restore-site check uses it for the never-dispatched exemption (K7-NEVER).
6. Stores `WAKE_PENDING[tid] = 0` (§2.6).

The same-thread re-pick at `scheduler.rs:210` also stamps `LAST_RUN`.

**Per-thread stamps** are static `[AtomicU8; MAX_THREADS]` and `[AtomicU64; MAX_THREADS]`, indexed by slot `tid.0`, accessed through `get()`, and reset in `allocate_thread`. The reset values are `LAST_CPU = NEVER (0xFF)`, `LAST_RUN = now`, `CALL_PHASE = RECV_PHASE = 0` and `WAKE_PENDING = 0`.

**`IRQ_CTX[cpu]: AtomicU8`** (0 thread, 1 irq, 2 irq-exit):
- `irq_handler_el1` sets 1 at entry (bumping `nest` if the old value was non-zero) and 2 before `check_preemption()`.
- It sets 0 after that call returns, on `core_id()` read again, and on the spurious return.
- `schedule()` saves it in a local and restores it in the resumed-as-old-thread branch (`:268-271`).

**`schedule(origin: Origin)`** with `Irq`, `Yield` and `Block`: `irqsw` or `irqsw0` at the commit. Exact counters in the same code:
- `n1` at `:182-185`;
- `insched` at `:157-158`/`:428-430`;
- `xdir`, `xrep`, `xnever` after validation (`direct.rs:95`, `:243`).

#### 2.4 `unblock` caller attribution and the N2 counters

**`WakeSource`** (`#[repr(u8)]`, `COUNT = 15`), in fixed order:

| # | name | site |
|---|---|---|
| 0 | `call` | `channel.rs:139` |
| 1 | `reply` | `channel.rs:403` |
| 2 | `send` | `channel.rs:460` |
| 3 | `selcall` | `try_wake_select` from `channel.rs:138` |
| 4 | `selsend` | `try_wake_select` from `channel.rs:459` |
| 5 | `selsig` | `try_wake_select` from `notify.rs:141` |
| 6 | `sig` | `notify.rs:146` |
| 7 | `ndestroy` | `notify.rs:282` |
| 8 | `nto` | `notify.rs:341` |
| 9 | `nsto` | `notify.rs:349` |
| 10 | `pwait` | `process.rs:223` |
| 11 | `to` | `wake_with_error` ← `timeout.rs:105` |
| 12 | `chdestroy` | `wake_with_error` ← `ipc/mod.rs:231`, `:234` |
| 13 | `pexit` | `wake_with_error` ← `process.rs:209` |
| 14 | `cancel` | `wake_with_error` ← `channel.rs:499` |

**Signatures that change** (18 compiler-checked call-site edits, plus 2 for the switch functions):
- `unblock(tid, src) -> UnblockOutcome`. The return is `{ kind: u8, phase: u8, chan: u64 }`, 16 bytes returned in x0/x1. Callers that ignore it compile unchanged, since it is not `#[must_use]`.
- `wake_with_error(tid, err, src)`.
- `try_wake_select(tid, kind, bits, src)`.
- `try_reply_switch(replier, caller, channel)` and `try_direct_switch(sender, receiver, channel)`. The new `channel` argument is instrumentation only.

**Outcomes counted per source** in `unblock` after `:369` (masked):
- `ubrun` (skip Running) and `ubrbl` (skip Runnable);
- `ubdead` (the `_ => {}` revive arm);
- `ubnone` (empty slot or `tid ≥ MAX_THREADS`: `badtid`, then return as today's index would not; see §4.5);
- `ubmove` (queued off `LAST_CPU`).

For `src ∈ {reply}`, `unblock` also snapshots `CALL_PHASE[tid]`/`CALL_CHAN[tid]` **under THREAD_TABLE** into the outcome. For `src ∈ {call, send}` it snapshots `RECV_PHASE`/`RECV_CHAN`. The skip-or-wake decision and the phase are then read at the same instant.

**N2 phase protocol (SCAN-2, UBRUN-REPLY-AMBIGUOUS).** Each thread writes only its own slots, with plain Release stores:

| Store | Where |
|---|---|
| `CALL_CHAN = channel`, then `CALL_PHASE = 1` | `channel.rs:91`, right after `pending_caller = Some` |
| `CALL_PHASE = 2` | **Inside** the TIMEOUT_QUEUE critical section at `:120-125` and `:162-167`, before the guard drops; or at the same point when `timeout_ticks == 0`. Phase 2 therefore means "my timeout is registered (or none was requested)". A replier whose `clear_timeout` acquired TIMEOUT_QUEUE after that registration also sees phase 2. |
| `CALL_PHASE = 0` | `:180` |
| `RECV_CHAN`/`RECV_PHASE = 1` | `channel.rs:275` |
| `RECV_PHASE = 2` | Inside `:281-286`, or after `:276` when `timeout_ticks == u64::MAX` |
| `RECV_PHASE = 0` | `:294` |

**Classification**, with `bump()` in thread context:
- `ipc_reply` fallback (`:403`), on outcome skip (Running or Runnable):
  - `CALL_CHAN == channel` and phase 1 → `n2[0]` (`rpre`: reply before the caller's timeout registration; the caller heals with ETIMEDOUT later);
  - `CALL_CHAN == channel` and phase 2 → `n2[1]` (`rblk`: the reply removed a registered timeout and the caller will block with no waker. **This is the exact N2 wedge precursor**);
  - otherwise → `latereply` (the caller had already left the call, usually woken by its timeout).
- `ipc_reply` fallback, on outcome woke with phase ≠ 2 or chan ≠ channel → `misrep` (the reply woke a thread blocked in some other wait).
- `try_reply_switch`, after validation at `:243`: under THREAD_TABLE, the caller's `CALL_PHASE ≠ 2` or `CALL_CHAN ≠ channel` → `misrep` (`bump_masked`). The caller is Blocked there, so its phase is stable.
- `ipc_send` (`:460`) and `ipc_call` (`:139`) fallback, on outcome skip with `RECV_CHAN == channel`: phase 1 → `n2[2]` (`vpre`), phase 2 → `n2[3]` (`vblk`: the receive-side analogue; the send removed the receiver's timeout and the receiver blocks with no waker). Receive-side misdirected wakes are not counted (see §6 SCAN-2).
- `clear_timeout` returns `ClearResult { Removed, Absent, Busy }`, and callers ignore it except for `Busy` → `ctbusy` (a stale timeout left behind).

`ubrun`/`ubrbl` by source stay as the ADR specifies. `n2`, `latereply` and `misrep` split them. Caveat for `src=to`: `ctbusy` > 0 explains some `ubrun`/`ubrbl[to]`.

**Wake-pending markers (SCAN-1).**
- **Set:** `WAKE_PENDING[tid] = src + 1` (plain store) wherever a waker removes the target's last reference: `channel.rs:94`, `:368`, `:455`, `:493`; `ipc/mod.rs:227-228`; `process.rs:192`/`:198` (only when stored in `epipe_wakeups`); `timeout.rs:97`; `notify.rs:124`, `:317`. `WAKE_AT[tid] = TICK_COUNT` is stored alongside.
- **Clear:** in `unblock` after `:369` on every outcome, in `note_dispatch`, and at `notify.rs:323`'s `continue`, where the deadline wake is abandoned (N3) and must not look "in flight" forever.

#### 2.5 The tripwire line

**Printer:** `putc` only, with no `core::fmt`, no locks, no local arrays or struct copies, and a closure-pull writer (`shared::tripwire::write_line(sink, mode, ncpu, |key, idx| value)`). All new IRQ-path helpers are `#[inline(never)]`, so V1 can attribute them per symbol.

**Format**, schema `v=1`, keys lowercase, values `^[0-9,]+$` (except `src`), keys in fixed order:
```
[tripwire] v=1 src=hb cpu=0 t=12000 ncpu=4 tick=12001,11890,11875,11902 irqsw=812,799,801,790 ... twc=812345 twn=12 twmax=94000 n=31
```

| Group | Keys |
|---|---|
| Prefix | `v`, `src` (`hb`/`g1`/`panic`/`exc`), `cpu`, `t`, `ncpu` |
| Per-CPU | `tick`, `irqsw`, `irqsw0`, `nest`, `elrmm`, `spsrmm`, `n4`, `insched`, `rsthold` |
| Global scalars | `xdir`, `xrep`, `xnever`, `n1`, `pcnull`, `pcphys`, `pcother`, `spbad`, `latereply`, `misrep`, `ctbusy`, `badtid` |
| N2, 4-wide | `n2` = `rpre`, `rblk`, `vpre`, `vblk` |
| 15-wide per source | `ubrun`, `ubrbl`, `ubdead`, `ubnone`, `ubmove` |
| 9-wide per lock | `lkph`, `lkpho`, `lkphrun`, `lkoxp`, `lkself`, `lktry`, `lktph`, `lkstk` |
| Scans, event counts | `scana`, `skipa`, `skipaself`, `scanb`, `skipb`, `scanstall`, `hbdefer`, `orphan`, `nowaker`, `wakefl`, `starved` (4), `dupcur`, `dupq`, `qbad` |
| Scans, gauges | `orphan_now`, `nowaker_now`, `wakefl_now`, `scanhold1`, `scanhold2` (max CNTVCT) |
| Miscellaneous | `lb`, `enqfull`, `badchan` (3: cap, select, slot) |
| Closing | `twc` (cumulative CNTVCT spent printing), `twn` (lines printed), `twmax` (max per line), `n` |

`n` is the number of `key=value` tokens before it, the prefix keys included.

**Parser contract (for step 1a and B1): use the last complete line.** A line is valid when its token count equals `n`. A missing key means 0. Most keys are cumulative, but the `*_now` and `scanhold*` keys are gauges. Omitting zero keys loses nothing only under a whole-line parser. A "last value per key" parser is wrong for gauges, so the §2.5 claim "every key is monotonic" is withdrawn.

**Modes:** `NonZero` at heartbeats, where keys that are all zero are omitted but the prefix, `twc`, `twn`, `twmax` and `n` always print. `Full` at `g1` and in fatal dumps.

**Where and when it prints (SCAN-PLACEMENT, SCAN-7, BENCH-INTERLEAVE):**
1. **Heartbeat:**
   - The `writeln!("[heartbeat] tick={}")` at `timer.rs:186` is unchanged and stays in place.
   - It now also sets `HB_PENDING`. The scans and the `NonZero` line run **at the end of `timer_tick_handler`, after step 8 (`:208`)**, on the first CPU-0 tick where `HB_PENDING` is set and both of these hold:
     - `CONSOLE_BUSY` is clear. `bench_main` sets and clears it around its result block (`bench.rs:362-450`).
     - `!held_by_stream(0)`, i.e. CPU 0's interrupted stream holds no stamped IRQ-class lock.
   - After 256 ticks of deferral it runs anyway and bumps `hbdefer`. That cap is below 1000, so each heartbeat still gets one `src=hb` line before the next heartbeat.
   - CPU 0's own `check_timeouts` and balancer therefore keep `main`'s timing.
   - CPU 0 holding CHANNEL_TABLE or PROCESS_TABLE (`spin::Mutex`, invisible to `held_by_stream`) is a residual; see §4.5.
2. **Gate 1:** `bench.rs:450` sets `G1_PENDING`, and the same end-of-handler point prints `src=g1` `Full` under the same deferral rule.
3. **Panic** and 4. **exception:** §2.7.

**Event line** (lock slow path only):
```
[tripwire-ev] kind=ph|stuck|self cpu=N lock=<name> idx=I ctx=N owner_cpu=K owner_gen=G holder_tid=T|? cur_tid=T|? holder_running=K|none|? holder=<file:line>|?
```
It is not sent through `kinfo!`.

#### 2.6 Heartbeat scans (CPU 0, IRQ context, `try_lock_quiet` only)

**Accumulators.** All masks and per-kind history live in CPU-0-only `static AtomicU64`s (Relaxed load/store): `SCAN_QUEUED`, `SCAN_DUPQ`, `SCAN_CLASS[4]`, `SCAN_CURRENT`, `SCAN_DUPCUR`, `SCAN_RUNNABLE`, `SCAN_TIMEOUT`, `SCAN_CHANREF`, `SCAN_NDL` and `SCAN_NOTIF`. Nothing is kept in a stack struct or in closure-captured locals (V1-GATE-INLINING). `classify_slot` takes scalars.

**tid conversions are checked everywhere (SCAN-3):**
- `if t < 64 { m |= 1 << t } else { bump badtid }`;
- `get()` instead of indexing;
- `#![deny(clippy::indexing_slicing, clippy::arithmetic_side_effects)]` on the shared scan module and the kernel scan functions;
- `TID_NONE` in `CURRENT_TID` means "no current" and is not `badtid`.

**Phase 1 (scans A and C).**
- Take all 8 RUN_QUEUES in ascending order through recursion (`fn rq_level(k)`, holding guard k while calling k+1), then THREAD_TABLE. This is the only order in the code (`init.rs:237-257`, `scheduler.rs:218-244`). All 8 are held together because the balancer moves a thread while holding both of its queues.
- **Read `CURRENT_TID[k]` for k < ncpu instead of try-locking `CURRENT_THREAD[k]` (SCAN-6).** Under THREAD_TABLE they are equal (§2.3).
- Record `scanhold1` = max CNTVCT from the first acquisition to the last release.

**Phase 2 (scan B).** Each table is try-locked alone, its mask built, and the lock dropped before the next. Record `scanhold2`.
- CHANNEL_TABLE: `chan_ref`;
- TIMEOUT_QUEUE: `timeout`;
- NOTIFY_DEADLINES: `ndl`;
- NOTIFICATION_TABLE: `notif_ref`, only if phase 1 saw a BlockedNotification or BlockedSelect thread.

Then the lock-free `WAKE_PENDING[t]` for each Blocked candidate.

**Classification** (`shared::tripwire::classify_slot`, pure):
- **Orphan:** Runnable, not queued and not current.
- **No waker** (non-current thread):
  - BlockedIpc: no `timeout` and no `chan_ref`;
  - BlockedNotification: no `ndl`, `notif_ref` or `timeout`;
  - BlockedSelect: no `ndl`, `chan_ref`, `notif_ref` or `timeout`;
  - BlockedProcessWait is excluded.
  - Then split: `WAKE_PENDING[t] != 0` → **`wakefl`** (a waker is in flight: it has taken the reference but not yet run `unblock`; this is starvation or N1(b), not a lost wakeup); otherwise → **`nowaker`**.
- **Starved:** queued, Runnable, `now.saturating_sub(LAST_RUN) > 1000`, by class.
- **Extras:** `dupcur`, `dupq`, `qbad`.

**Two strikes, per kind (SCAN-4).**
- Separate `(flag mask, LAST_RUN snapshot, per-CPU tick snapshot)` triples for A-orphan, A-starved, B-nowaker and B-wakefl, each updated only when its own phase completes.
- A flag is **confirmed** when the thread is flagged in two consecutive completed scans of that kind, with `LAST_RUN` unchanged, **and** every online CPU's per-CPU `tick` has advanced by ≥ 100 since the first strike. If not, bump `scanstall` and keep the first strike. This covers a vCPU stalled mid-window.
- Two strikes removes the scan-A transit windows (all masked, thread-side). Scan-B waker-in-transit windows are removed by the `WAKE_PENDING` split, not by two strikes.

**Counting semantics (SCAN-5).** Keep a `CONFIRMED` mask per kind. The event key (`orphan`, `nowaker`, `wakefl`, `starved[c]`) is bumped **only when a tid enters** the confirmed set, and the bit is cleared when the flag drops. The `*_now` gauges hold the current popcount.

**Skip counters:**
- `skipa` (busy);
- `skipaself` (a Reentry-class failure; rare, because the deferral rule already avoids CPU-0-held locks);
- `skipb`.

Phase 2 is independent of phase 1.

Taking CHANNEL_TABLE with `try_lock` from the tick handler is the ADR's documented exception to the class rule. Record it in `deadlock-prevention.md`.

#### 2.7 Fatal dumps and `#[track_caller]`

**Panic handler** (`main.rs:390-397`), in this order:
1. `mrs DAIF` (remember `irq_was`), then `msr DAIFSet, #0x2`.
2. Per-CPU `PANICKING[cpu]`: load then store. On re-entry, `halt()`.
3. **Unchanged** `writeln!("PANIC: {}", info)`.
4. `[panic] cpu=N tid=T|? ctx=N irq_was=on|off t=<secs.micros> irq_elr=0x…`. `irq_elr` is printed only when ctx ∈ {irq, irq-exit}: `ELR_EL1` still holds the interrupted PC, because no EL1 exception returns in between (the sync handler halts). Lowercase, and never `PANIC: `.
5. `[tripwire] v=1 src=panic … Full`.
6. **On CPU 0 only, and only if `CPU0_DRAINING` is clear (ASM-3):** loop `drain_logs()` while it returns `DRAIN_BATCH_SIZE`, bounded by `LOG_RING_SIZE * MAX_CORES / DRAIN_BATCH_SIZE` = 128 iterations (ASM-4). The flag is set and cleared inside `drain_logs` only when `core_id() == 0`. `drain_logs` gains a `usize` return (lines drained); existing callers ignore it.
7. `halt()`.

It takes no lock, makes no allocation and calls no `klog!`. The `fmt` in step 3 and in `drain_logs` is post-fatal and exempt from V1.

**Exception** (`exceptions.rs`):
- The stub gains `add x3, sp, #48` and `mrs x4, SPSR_EL1` (13 of 32 slot instructions).
- The handler signature becomes `(esr, far, elr, sp, spsr)`.
- The head and Abort lines are unchanged. Then it prints `  regs: sp=0x… spsr=0x… ttbr0=0x… vbar=0x…`, `  ctx: cpu=N tid=T irq=N sched=B`, `[tripwire] v=1 src=exc … Full`, and the `wfe` loop.
- Add `read_ttbr0_el1()` and `in_scheduler(cpu)`.

**`#[track_caller]` chain:**
- `IrqSpinLock::{lock, try_lock}` → `reentry_panic`;
- `FrameAllocator::free_pages` (`frame.rs:49`) and `free_dma_pages` (`frame.rs:164`);
- optionally `compositor::release_buffer` (`compositor/service.rs:501`).

#### 2.8 IRQ frame ELR/SPSR check

```asm
    stp x29, xzr, [sp, #-16]!     // :157 unchanged
    mrs x0, ELR_EL1
    mrs x1, SPSR_EL1
    stp x0,  x1,  [sp, #-16]!     // entry snapshot, 192-byte frame
    bl  irq_handler_el1
    ldp x0,  x1,  [sp], #16
    bl  irq_frame_check           // counts elrmm/spsrmm on core_id(); restores nothing
    ldp x29, xzr, [sp], #16       // :159-170 unchanged
```
- `irq_frame_check(entry_elr, entry_spsr)` is `#[no_mangle] extern "C"` and `#[inline(never)]`, with no locks and no logging. It compares register values only and never computes a symbol address (§2.11).
- The new pair sits below the pad. SP at the `bl`s is interrupted SP − 192 and − 176, both 16-byte aligned (SCTLR.SA=1).
- Σ`elrmm` ≤ Σ`irqsw` holds as a sum over CPUs.
- `lower_el_irq_entry` is unchanged.

#### 2.9 "Core N online" line

- In `secondary_main`, inside the `PRINT_TURN` window after `smp.rs:205`, print directly to the UART:
  ```
  [smp] cpu=1 vbar=0x0000000040081000 ttbr0=0x… vbar_kva=0 ttbr0_idmap=1
  ```
- For CPU 0, print the same line after `main.rs:271`.
- **Exception (ASM-5):** on the SMP-timeout path (`smp.rs:164-171`), CPU 0 stops waiting after 100 ms and resumes direct UART output (`main.rs:277`, its own `[smp]` line). A late secondary's line can then interleave. No parser depends on `[smp]`. K10's acceptance is "one `[smp]` line per CPU in `ONLINE_CPUS`", so an SMP timeout shows as a missing line.
- It only prints; the F7 asserts are step 5.

#### 2.10 Bench

- Delete `bench.rs:178-179`, `:236-237` and `:249-250`, and fix the comments at `:157-158`, `:175-178`, `:228` and `:233-236`.
- Add `CONSOLE_BUSY` set and clear around `:362-450` (§2.5).

#### 2.11 Address values on the IRQ path (ASM-1)

On CPUs 1–3, VBAR is installed with `adrp` while the MMU is off (`boot.S:287-291`), so the whole IRQ path (`irq_handler_el1`, `timer_tick_handler`, `check_preemption`, `schedule()`, `unblock`) runs at physical-alias PCs there. `adrp`-based accesses still work through the identity TTBR0. **Any address *value* that such code computes is physical.**

The rule: any address value computed on the IRQ path that is compared, stored or published across CPUs must either:
- come from a VA captured at thread level on CPU 0: `TEXT_LO`/`TEXT_HI` static `AtomicU64`s, set in `kernel_main` before `smp::init` from `&__text_start`/`&__text_end` (as `kmap.rs:234-235` reads them), and read by `classify_pc` on every CPU; or
- be normalised: `if v < KERNEL_VIRT { v + VIRT_PHYS_OFFSET }`. This applies to `Location::caller()` in `holder_site`.

Pointers that are only dereferenced (lock words, statics) need no change, and TTBR1 maps VAs on every CPU (`smp.rs:197`). S3 adds a host test that feeds `classify_pc` physical bounds and shows that a VA pc then classifies Other, which documents why the bounds must be VAs.

#### 2.12 Balancer overflow guard (SCAN-BALANCER-OVERFLOW)

`init.rs:226` becomes `max_depth <= min_depth.saturating_add(1)`.
- When at least one queue was read, the result is identical, since real depths are far below `usize::MAX`.
- When every online `try_lock` failed, `min_depth == usize::MAX` and `max_depth == 0`, so it returns early instead of panicking on `+ 1` in the dev build.

That all-failed case is reachable only while the phase-1 scan holds all 8 queues, so the guard stops the instrumentation from adding a PANIC signature. Record it in the PR as a behaviour-neutral guard.

---

### 3. Ordered task list

**Common rules for every task:**
- Worktree `crash-1b`, branch `claude/crash-fix-step-1b-tripwires`, one commit per task (`Crash fix step 1b: <desc>`).
- **Gates:** `just check` with zero warnings, and `just test`.
- **Boot gate** for kernel tasks:
  - `just soak runs=1 secs=75 report_only=1`
  - `just soak runs=1 secs=75 mode=gpu report_only=1`
- **Baseline greps on `run-01.log`:** `Boot +EL: 1`, `Core ID: 0`, and the heartbeat line shape.
- **Hazard check** (must be empty): `tr -d '\r' < run-01.log | grep -a '^\[tripwire' | grep -E 'ESR=|ELR=|EC=0x|FAR=|\[heartbeat\]|PANIC: '`.
- S1–S3 are host-only; the kernel tasks depend on them.

**S1. `FixedQueue::iter()` / `contains()`**
- **File:** `shared/src/collections.rs`.
- **Tests:** empty queue; FIFO order after wrap-around; full queue; `contains` after a pop.

**S2. `shared/src/lock.rs`**
- **Contents:** `OwnerStamp`, `classify` (5 outcomes, with a `CpuView` for the other CPU's generation), `read_stamp`, `StampedLock`/`StampedGuard` (`restamp`, weak and strong CAS, and a pre-release hook so the kernel can clear its holder fields before `store(0)`), `LockClass`, `TID_NONE`.
- **Tests:**
  - Codec round-trip.
  - The classify table, including OtherCpu versus OtherCpuSwitched and IRQS_ON ignored.
  - `read_stamp`, exhaustive for 2 CPUs × 0–2 switch events. No stamp can classify Reentry unless the thread is still on c with gen unchanged.
  - An interleaving model of "holder preempted, migrated to d, IRQ on d re-enters": it must classify OtherCpuSwitched, never Reentry.
  - The consistent-snapshot helper (`w1 == w2` rule).
  - A threaded test (4 threads, small N under `cfg(miri)`).
  - `LockClass` names.
- **Docs:** amend the M25 shared-crate ADR (atomics and `unsafe` for Miri-checked protocol types).

**S3. `shared/src/tripwire.rs`**
- **Contents:**
  - the `Key` catalogue with widths (Cpu, 1, Source 15, Lock 9, Class 4, N2 4, Badchan 3) and a gauge flag;
  - `WakeSource`, `UnblockOutcome`, `ClearResult`;
  - `CpuCounters`, the sinks, `put_dec`/`put_hex`;
  - `write_line` and `MAX_LINE_LEN`;
  - `write_reentry_msg`;
  - `classify_pc` with `TextLayout`;
  - checked `mask_set`/`mask_test`, `classify_slot` (scalar arguments), `is_starved`;
  - `TwoStrike` (per kind, with a tick-advance gate) and `EdgeCounter`;
  - `classify_reply` and `classify_send`, the §2.4 N2 table as a pure function.
- **Tests:**
  - `put_dec` against `format!`.
  - A **byte-exact golden `Full` line** (step 1a's fixture). `NonZero` omission and order. `n` equals the token count.
  - One `\n`. Names unique. No `[heartbeat]`/`ESR=`/`ELR=`/`EC=0x`/`FAR=`/`PANIC: `.
  - All values at `u64::MAX` fit `MAX_LINE_LEN`; an undersized sink sets `overflowed`.
  - The `write_reentry_msg` worst case (`CURRENT_THREAD[7]`, the longest `kernel/src` path + 5-digit line, `holder_irqs=off`, 2-digit tid, 17-digit gen) is ≤ 160 characters. `?` renders for NONE fields.
  - `classify_pc` on the real layout plus the ADR's observed values, and the physical-bounds test (§2.11).
  - `classify_slot` table, including `BlockedIpc{MAX}`, `last_run > now`, orphan-before-starved, and `wakefl` versus `nowaker`.
  - `mask_set` with tid = 63, 64, 0x8000_0000 and `u32::MAX`: no panic, and `badtid` counted.
  - `TwoStrike`: single; two in a row; three; flag–clear–flag; bits independent; **flag(B) → A-only scan with a dispatch between → flag(B) does not confirm**; the tick-advance gate withholds confirmation.
  - `EdgeCounter`: a persistent flag counts once; drop then re-flag counts twice; the gauge follows the popcount.
  - `classify_reply`/`classify_send`: every (outcome, phase, chan match) cell.
  - `CpuCounters` row isolation and wrap.

**K1. Bench DAIF removal** (independent)
- **File:** `bench.rs` (§2.10, the DAIF part).
- **Boot acceptance:** `[bench] === Gate 1 Complete ===` and an `IPC round-trip (same core): … (10000 iters)` line in text mode.

**K2. Tripwire runtime, per-CPU ticks, heartbeat and g1 lines**
- **Files:**
  - new `kernel/src/observability/tripwire.rs`: static `CpuCounters<MAX_CORES>`, `bump`/`bump_masked`, `UartSink` (`\n` → `\r\n`), `print_line`, `HB_PENDING`, `G1_PENDING`, `CONSOLE_BUSY`, `twc`/`twn`/`twmax`, and the deferral logic (§2.5; `held_by_stream` is stubbed to false until K5);
  - `timer.rs`: bump `tick` after the rearm; `HB_PENDING` after `:186`; `tripwire::end_of_tick()` after `:208` on CPU 0;
  - `bench.rs`: `G1_PENDING` at `:450`, and `CONSOLE_BUSY` around `:362-450`.
- **Boot acceptance:**
  - Every `[heartbeat] tick=N` is followed by exactly one `[tripwire] v=1 src=hb … n=K` before the next heartbeat.
  - One `src=g1` line after `=== Gate 1 Complete ===`, with all four `tick` values > 0.
  - `hbdefer` is small.
  - No `src=hb` line inside the bench result block.
- **Docs:** an `observability.md` "Tripwire line" section, stating the last-whole-line parser contract. Fix the comments at `timer.rs:111`, `:143-149` and `:172-175`.

**K3. Dispatch bookkeeping**
- **Files:**
  - `tripwire.rs`: `SWITCH_GEN`; `CURRENT_TID` (initialised to `TID_NONE`); `IRQ_CTX`; `LAST_CPU`, `LAST_RUN`, `WAKE_PENDING`, `WAKE_AT`; `note_dispatch` (returns the old `LAST_CPU`);
  - `scheduler.rs`: the commit calls, `schedule(origin)`, `n1`, `insched`, the `IRQ_CTX` save and restore;
  - `direct.rs`: commits, `n4`, `xdir`/`xrep`/`xnever` from the returned old `LAST_CPU`;
  - `gic.rs`: `IRQ_CTX` and `nest`;
  - `sched/mod.rs`: stamp reset in `allocate_thread`.
- **Boot acceptance:** `irqsw` > 0 on all CPUs; `nest`, `insched`, `n4` and `irqsw0` expected 0 or small.
- **Docs:** fix the `gic.rs:257-261` comment. CLAUDE.md Concurrency: "per-CPU switch generation bumped at the 4 `CURRENT_THREAD` commit sites; `CURRENT_TID` written only there, under THREAD_TABLE".

**K4. IRQ frame ELR/SPSR check**
- **Files:** `exceptions.rs` (§2.8, frame comment `:144`, stale `:6-8`).
- **Boot acceptance:** boots classify as before, and `elrmm`/`spsrmm` appear in `g1` lines.
- **Docs:** CLAUDE.md Key Technical Facts (192-B frame).

**K5. `IrqSpinLock` on the 9 statics** (depends on S2 and K3)
- **Files:**
  - `kernel/src/sync/{mod.rs, irq_spin_lock.rs}` (§2.1): holder-field protocol, the 5-arm slow path, `for_each_lock_word`, `held_by_stream`, and `rsthold` before the 5 `restore_context` calls;
  - the declarations at the 9 sites; removal of `use spin::Mutex`;
  - wire `held_by_stream(0)` into K2's deferral.
- **Boot acceptance:**
  - Classes are not new, and no `lock re-entry:` appears in CLEAN boots.
  - `rsthold` = 0.
  - `lktry` > 0 is allowed (the H3 near-miss rate).
  - `llvm-objdump -h` shows the expected layout, and `llvm-nm` shows the image end inside the boot TTBR1's 4×2 MiB.
- **Docs:** `deadlock-prevention.md` §3.3 note (detect-only type, and the heartbeat CHANNEL_TABLE try-lock exception); CLAUDE.md (Workspace Layout `sync/`, Concurrency); rule 05.

**K6. Wake attribution and N2 counters** (depends on S3 and K3; **not** on K5)
- **Files:**
  - `scheduler.rs` (`unblock(tid, src) -> UnblockOutcome`, outcome counters, phase snapshot, `WAKE_PENDING` clear);
  - `timeout.rs` (`wake_with_error` src, `clear_timeout -> ClearResult` + `ctbusy`, marker at `:97`);
  - `select.rs:293`;
  - the 18 call sites;
  - `channel.rs`: `CALL_*`/`RECV_*` phase stores at `:91`, `:120-125`, `:162-167`, `:180`, `:275`, `:281-286`, `:294`; markers at `:94`, `:368`, `:455`, `:493`; `classify_reply` at `:403`; `classify_send` at `:139`, `:460`;
  - `direct.rs` (`channel` parameter, `misrep` after `:243`);
  - `ipc/mod.rs:227-228`, `process.rs:192`/`:198`, `notify.rs:124`/`:317`/`:323` (markers);
  - `lb` at `init.rs:263-264`; `enqfull` at `sched/mod.rs:56-61`;
  - `badchan[0..3]` at `cap/mod.rs:95-96`, `select.rs:66` and `ipc/mod.rs:152`;
  - `badtid` in `unblock` and the switch functions.
- **Boot acceptance:**
  - `badchan` equals the self-test baseline per site. Record it (the ipc tests hit cap and slot; the select_cap tests hit select).
  - `ubrun`/`ubrbl` appear.
  - `n2`, `misrep` = 0 expected in CLEAN boots. Any non-zero value in a CLEAN boot is recorded, not failed.
  - Classes not new.
- **Docs:** none (D1 covers them).

**K7. Restore-site checks (N5)** (depends on S3, K2 and K3)
- **Files:**
  - `main.rs`: capture `TEXT_LO`/`TEXT_HI` at VA before `smp::init` (§2.11);
  - `scheduler.rs`: before `assert_valid_ctx` at `:88`, `:279`, `:285`;
  - `direct.rs:186`, `:312`.
- **Checks:**
  - `classify_pc(ctx.pc)` against the captured VAs.
  - SP within `[phys_to_virt(stack_phys), +STACK_SIZE]`.
  - A context whose **returned old `LAST_CPU`** is `NEVER` is exempt from the SP check if its `sp` is the physical default (`task/mod.rs:185`).
  - Count only.
- **Boot acceptance:** `pcphys` > 0 on CPUs 1–3 (IRQ-path switches); `pcother` = 0 in CLEAN boots.

**K8. Heartbeat scans** (depends on S1, S3, K3, K5 and K6)
- **Files:**
  - `sched/mod.rs`: `scan_snapshot` with recursive run-queue guards, `RunQueue::for_each`, and `CURRENT_TID` reads;
  - an `ipc` accessor module (`scan_wakers`);
  - `tripwire.rs`: the static accumulators, per-kind `TwoStrike`, `EdgeCounter`, `scanhold*`, and the call from `end_of_tick`;
  - `init.rs:226` (`saturating_add`, §2.12).
- **Boot acceptance:**
  - `scana` climbs about 1 per heartbeat, and `skipa` is small.
  - In CLEAN boots, `orphan` = `nowaker` = `dupcur` = `badtid` = 0. `wakefl` and `scanstall` are recorded.
  - `starved` shows the Idle class only.
  - `skipb` is non-zero during the bench.
  - **Timing:** `scanhold1` and `scanhold2` < 6250 (100 µs).
  - Record `twmax` and the measured maximum `NonZero` length L; pass if `twmax ≤ 1.5 × L × 4.7 µs × 62.5 MHz` (the UART model holds) and `twmax` < 187,500 (3 ms).
- **Docs:** `observability.md` scan definitions (edge counting, gauges, `wakefl`).

**K9. Fatal dumps and `#[track_caller]`** (depends on K2 and K3)
- **Files:**
  - `main.rs:390-397` (§2.7, including `irq_elr` and the bounded drain loop);
  - `observability/mod.rs` (`CPU0_DRAINING`, the `drain_logs` return value);
  - `exceptions.rs` (stub, handler signature, TTBR0 reader);
  - `sched/mod.rs` (`in_scheduler`);
  - `frame.rs:49`, `:164` (optionally `compositor/service.rs:501`).
- **Boot acceptance:** CLEAN boots unchanged. The fatal path is proven by T-self.
- **Docs:** rule 01 `:11`; `.claude/agents/kernel-dev.md:22`; `developer-guide.md:609-626`.

**K10. `[smp]` VBAR/TTBR0 lines** (independent)
- **Files:** `smp.rs:205-207`, `main.rs` after `:271`, `exceptions.rs` (`read_ttbr0_el1`), `mmu.rs:207`.
- **Boot acceptance:**
  - One `[smp] cpu=N` line per CPU in `ONLINE_CPUS`.
  - CPUs 1–3: `vbar_kva=0`, VBAR = the physical LMA of `.text.rvectors`, `ttbr0_idmap=1`.
  - CPU 0: a TTBR1 VBAR and address space B's TTBR0.
- **Docs:** `observability.md`, including the SMP-timeout interleave note.

**T-self (recommended, not in either soak arm).** A kernel feature `tripwire-selftest`, off by default. A thread takes `THREAD_TABLE.lock()` with IRQs on and busy-waits for 3 ticks.
- **Build and run:** `cargo build --target aarch64-unknown-none -p kernel --features tripwire-selftest`, assemble the ESP as in `justfile:32-38`, then `just soak runs=1 secs=40 --no-build report_only=1`.
- **Acceptance (L5):**
  - `PANIC: panicked at` one of `kernel/src/sched/scheduler.rs:168`, `kernel/src/sched/scheduler.rs:371` or `kernel/src/sched/init.rs:257`.
  - A next line `lock re-entry: THREAD_TABLE on CPU n ctx=irq-exit|irq holder=<selftest file:line> holder_irqs=on …`.
  - `[panic] cpu=n … irq_elr=0x…`, with `irq_elr` inside the selftest's busy loop.
  - `[tripwire] v=1 src=panic …` with `lktry` for THREAD_TABLE ≥ 1 (`scheduler.rs:124` runs before all three sites).
  - The summary classifies PANIC, with the message in `first_fatal`.

**V1. V-register listing, differential gate** (after all code; no repository file)
- **Build both ELFs** at `nightly-2026-09-24` with `env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS -u CARGO_TARGET_DIR -u CARGO_BUILD_TARGET just disk`: `main`@`b07d7e4` (kernel code equals `aa1f128`) and the 1b head. Confirm `strings -a $ELF | grep 'rustc version'` shows `1.100.0-nightly (6eeff9a52 2026-09-23)` for both.
- **Call graph:** from `llvm-objdump -d -l -C` (sysroot), collect the set R of functions reachable from `irq_el1_entry` by following `bl`/`b` targets. `blr` is not followed; the new code must contain no `blr`, which the listing checks.
- **Acceptance (ASM-2, V1-GATE-INLINING):**
  1. For every V-register site in R of the 1b ELF, its `-l` source line (file:line, innermost inline) also has a V-register site in R of the `main` ELF. This is **source-line parity**: no new V site on the IRQ path from any source line, inlined or not.
  2. No V-register site at all whose source line is in `sync/irq_spin_lock.rs`, `observability/tripwire.rs`, `shared/src/lock.rs` or `shared/src/tripwire.rs`, or on a line changed by the 1b diff.
  3. No `bl memset`/`memcpy`/`memmove` from those files or lines. Every external callee reachable only in the 1b graph is listed explicitly in `vreg-by-symbol.txt`.
  4. Per-symbol counts for `irq_handler_el1`, `timer_tick_handler`, `drain_logs`, `sched::timer_tick`, `check_preemption`, `schedule`, `unblock`, `check_timeouts`, `wake_with_error`, `try_load_balance`, `check_notification_timeouts` and `irq_frame_check` are reported for both ELFs; any increase must be explained by rule 1.
  - **Exempt:** functions that end in halt (`reentry_panic`, `rust_begin_unwind`/panic handler, `sync_exception_handler`).
- `--debug-inlined-funcs` for `check_timeouts` (`timeout.rs:89`), `gpu_release_test_frame`/`remove_buffer` and `MessageRing::push`/`pop`/`ipc_call`/`ipc_recv`/`ipc_send`, for the H5 verdict.
- `vreg-by-symbol.txt` and the parity diff go in the PR body; the per-site TSV is attached. Both are written outside the repository. Annotate `aes::backends::aarch64_aes` and `polyval` as runtime-gated.

**D1. Docs sweep and ADR errata**
- CLAUDE.md: Workspace Layout; Key Facts (192-B frame, `IrqSpinLock` detect-only, `CURRENT_TID` invariant, IRQ-path address rule §2.11); a note that the NC limitation does not apply to kernel statics after boot.
- Rules 01 and 05; `kernel-dev.md`; `developer-guide.md` (test counts `:1579`, `:1631`, `:1724`; panic pattern); `deadlock-prevention.md`; `observability.md`.
- An errata block in the crash-fix ADR:
  - the PANIC-LOCK two-line form;
  - `frame.rs:51` → `[mm] BUG: free_pages(`;
  - the 15-source unblock list;
  - SELECT_WAITERS not a waker, NOTIFY_DEADLINES scoped, sleep = `BlockedIpc{MAX}`;
  - transit windows (`:386`; add `:192-244`, `:73-84` and creation; drop `:174-177`), and the waker-in-transit split (`wakefl`);
  - N2 measured by `n2[rblk]`/`n2[vblk]` (phase counters), and `ubrun[reply]` also counting late replies;
  - `channel.rs` line shifts (`278→275`, `293→290`, `361→355`, `374→368`, `398→392`, `409→403`);
  - `channel.rs:249` gone (→ `badchan`, 3 sites);
  - the ELR compare placement;
  - "post-#161 / nightly-09-22" → nightly-2026-09-24 (`6eeff9a52`);
  - `cargo objdump` → `llvm-objdump`;
  - `soak-qemu.sh` line drift (`:205-206` → `:208-209`, `:356-383` → `:359-386`, `:598-615` → `:613-631`, `:636` → `:652`, `:677` → `:693`, `:688` → `:704`);
  - the H3 rule: ctx ≠ thread*, with `kind=self`/`lkself` counted as H3 evidence and `kind=ph` not wedge evidence by itself;
  - the post-panic behaviour change.
- Run `/audit-loop` until a clean round.

**B1. Single-arm N2 baseline soak** (last; on the final 1b head; owner gate)
- **What it is:** data collection only. It is not the ADR's interleaved A/B acceptance, which waits for step 1a. It makes no comparison with `main` and draws no Fisher inference.
- **Conditions:**
  - Host-exclusive under the ADR load rule: no other QEMU, build or soak on the host. **Coordinate with the other session before starting and after finishing.**
  - Record the commit, `load1` and firmware.
  - Run `just soak runs=20 secs=75 out=target/soak/<ts>-b1-text report_only=1`, then `just soak runs=10 secs=75 mode=gpu out=target/soak/<ts>-b1-gpu report_only=1`.
- **Extraction per `run-NN.log`** (script in the scratchpad, not the repository): the last valid line, all srcs allowed, since fatal boots end in `src=panic|exc`:
  ```sh
  tr -d '\r' < "$f" | grep -a -E '^\[tripwire\] v=1 src=(hb|g1|panic|exc) ' |
    awk '{ k=0; nv=-1
           for (i=2;i<=NF;i++) { split($i,a,"=")
             if (a[1]=="n") nv=a[2]; else if ($i ~ /^[a-z][a-z0-9_]*=[0-9a-z,]*$/) k++ }
           if (nv>=0 && k==nv) last=$0 }
         END { print last }'
  ```
  A missing key means 0. Array index 1 of `ubrun`/`ubrbl` is `reply`. Take `class` from `summary.tsv` column 3, joined on `log` (column 22).
- **Table per mode, split by class:**
  - boots, and boots with no valid line;
  - `ubrun[reply]` and `ubrbl[reply]` (boots > 0, sum);
  - `n2` (rpre, rblk, vpre, vblk), `latereply`, `misrep`, `ctbusy`;
  - `nowaker`, `wakefl`, `orphan`, `n1`, `scanstall`;
  - `lkph`/`lkself` for THREAD_TABLE, CURRENT_THREAD and WAKEUP_ERRORS;
  - `elrmm`.
- **Output:** the table and the per-boot TSV go in the PR (or a follow-up comment) and in `docs/knowledge/` as the baseline note. Do not draw A/B conclusions from them.

---

### 4. Risks and traps

1. **Panicking in the wrong place.**
   - Panic only on stamp Reentry. `lkself`, PreemptedHolder, OtherCpuSwitched and holder-field matches are count-only.
   - `try_lock` never panics, and scans use `try_lock_quiet`. `reentry_panic` is `#[cold]`, `#[inline(never)]` and `#[track_caller]`.
   - IRQS_ON is masked out of the classify comparison.
   - `rsthold` is a counter, not an assert.
   - No new panic sources from the instrumentation itself: checked tid conversions (SCAN-3), the balancer `saturating_add` (§2.12), and `wrapping`/`saturating` arithmetic.
2. **Printing from IRQ context.**
   - Only `putc` and atomics. CPU 0 is the only IRQ-context printer of `[tripwire]` lines.
   - Event lines are rare and have their own prefix.
   - Address values follow §2.11.
3. **UART cost.**
   - Using 4.7 µs/B, a `NonZero` line of about 400–500 B costs about 1.9–2.4 ms per heartbeat (~1000 ticks), and `Full` g1 (~700 B) prints once.
   - The line runs after CPU 0's `check_timeouts`/balancer (§2.5), but it still delays CPU 0's next tick by 1–2 ms. During that time other CPUs process expiries CPU 0 would have taken, which is the N7 factor.
   - `twc`/`twn`/`twmax` measure it. `drain_logs` (up to 16 lines every 4th tick) is larger.
   - Put the printer behind a feature before any real-PL011 use.
4. **V-register contamination (H5).**
   - Mitigations: static accumulators, scalar arguments, recursion for guards, the closure-pull printer, `#[inline(never)]` helpers, and no `core::fmt` on the IRQ path.
   - V1's differential source-line parity gate over the IRQ call graph enforces it, including inlining and `memset`/`memcpy`.
5. **Things that could change scheduling behaviour** (all listed in the PR):
   - **Lock fast path:** an MPIDR read, a DAIF read, a generation load, a second stamp read, the holder-field stores after acquire and **clears before release**, and the `#[track_caller]` argument. This changes the Gate 1 IPC figure.
   - **Phase 1 of the scan** holds all 8 RUN_QUEUES and THREAD_TABLE for ≤ 100 µs (measured by `scanhold1`). Effects:
     - other CPUs' `timer_tick` try-locks skip a slice decrement;
     - a balancer overlapping the ascending acquisition may decide on a **subset** of queues;
     - a fully overlapped balancer returns early (§2.12);
     - blocking takers (`unblock`, `schedule`) wait.
   - **Phase 2 of the scan** holds TIMEOUT_QUEUE, CHANNEL_TABLE, NOTIFY_DEADLINES and NOTIFICATION_TABLE briefly. Effects:
     - other CPUs' `clear_timeout` silently skips (`timeout.rs:155`), leaving a stale entry. That causes extra `ubrun`/`ubrbl[to]` and can cause a spurious ETIMEDOUT; `ctbusy` counts it.
     - `check_timeouts` on other CPUs skips its tick (`timeout.rs:82`).
     - `check_notification_timeouts` drops a due deadline when THREAD_TABLE is busy (`notify.rs:317` then `:321-323`, N3). This path is unreachable in soak today.
   - **The print** runs inside CPU 0's IRQ. The deferral rule skips ticks where CPU 0's interrupted stream holds a stamped IRQ-class lock. CHANNEL_TABLE/PROCESS_TABLE held by that stream (`cap/mod.rs:39`, `channel.rs:69`/`:362`/`:440`) is not visible, so CPUs 1–3 masked spins on those can be extended. This is the same exposure `drain_logs` has in `main`.
   - The `rsthold` walk (23 loads) before each `restore_context`, the `bump()` masked windows, and the `IRQ_CTX`/`note_dispatch`/phase/marker stores add nanoseconds and change no decisions.
   - `unblock`, `schedule`, `wake_with_error`, `try_wake_select`, `try_reply_switch` and `try_direct_switch` gain parameters or return values but no behaviour.
   - The balancer's `saturating_add` differs only where `main` would panic.
   - Do not fix anything the counters reveal.
   - The panic handler mask stops CPU 0's heartbeat after a CPU 0 panic; the panic path now drains every ring.
6. **SP alignment, frame layout, physical-alias execution.**
   - 192 and 176 are multiples of 16, and the xzr pad stays at its offset.
   - Code on CPUs 1–3's IRQ path may use `adrp`/`bl` freely for *accesses*. It must never compare or publish an address value it computes (§2.11).
7. **Const-initialising lock arrays.** The pattern is verified. `StampedLock` has no `Drop`; only the guard does. Check the image end against the boot TTBR1 window.
8. **Soak classifier regexes.**
   - No uppercase `ESR=`/`EC=`/`FAR=`/`ELR=`; `irq_elr=` is lowercase.
   - Never `[heartbeat]` or a second `PANIC: `.
   - Exception additions go after the Abort line.
   - PANIC-LOCK message ≤ 160 characters (host-tested).
   - No `kinfo!` for tripwire output.
9. **Harness key-set coupling.** S3's schema v1, its golden line and the **last-whole-line contract** are what 1a parses. A 12th awk field is absorbed into `C_I3` unless 1a adds a variable (`soak-qemu.sh:404`, `:414`), so 1a stores the whole last line in a side file plus selected columns.
10. **`frame.rs:51` key loss.** Key on `[mm] BUG: free_pages(`.
11. **Tick semantics.** `TICK_COUNT` advances only on CPU 0 (about 770/s under TCG). The per-CPU `tick` gate in two-strike covers stalls of CPUs 1–3. A CPU 0 stall freezes the scans anyway.
12. **Counter contract violations.** `bump_masked` at an unmasked site loses counts, and its `debug_assert!` would panic. Use it only at the listed sites.
13. **N2 phase caveats.**
   - A caller that re-enters `ipc_call` on the **same** channel after a timeout, with a reply from the previous call still in flight, classifies as N2 rather than `latereply`. No sequence number is kept, because the Channel struct shape stays unchanged. This is rare, and the PR states it.
   - Receive-side misdirected wakes are not counted.

---

### 5. What cannot be verified without step 1a, and what the PR must say

**Cannot be verified yet:**
- **All of the ADR's acceptance** (ADR:430-434): "no new class except PANIC-LOCK", WEDGE-STUCK + PANIC-LOCK against `main` (two-sided Fisher), and "every non-CLEAN boot has a tripwire line" as a harness column. The harness has no WEDGE-STUCK/ALIVE, DEGRADED or PANIC-LOCK classes, no tripwire columns, no interleave mode and no Fisher statistics. **For that A/B comparison**, two sequential non-interleaved batches (1b then `main`) break the protocol and must not be used. This restriction does not apply to B1, which is single-arm and descriptive.
- **Both decision rules:**
  - H3 is refuted if ≥3 WEDGE-STUCK and 0 PANIC-LOCK with ctx ≠ thread*, **and** no `kind=self` event or `lkself` > 0 with ctx ≠ thread* in the last or fatal line.
  - H1 is refuted if `elrmm` = 0 in all 30 boots.
  - These need 30 interleaved boots per arm.
- **The rare paths:** a real PANIC-LOCK, `ph`/`self` events, non-zero `elrmm`, confirmed orphans or no-waker threads, and the `#[track_caller]` GPU location. T-self proves the PANIC-LOCK path mechanically.
- A 1b-arm PANIC-LOCK is currently classified PANIC, with `lock re-entry: ` in `first_fatal`.

**N2 baseline (owner gate, single arm): task B1.**
- 20 text + 10 gpu boots of the final 1b head with the existing harness, host-exclusive and coordinated with the other session.
- Per-boot extraction of the last valid `[tripwire]` line (recipe in B1), tabulated per mode and class: `ubrun[reply]`, `ubrbl[reply]`, `n2`, `latereply`, `misrep`, `nowaker`, `wakefl`, `orphan`, `n1`.
- It is descriptive. It makes no claim about `main` and does not replace the interleaved acceptance. The design supports reading N2 wedges from it because they keep CPU 0's heartbeat alive (ADR N2), so the last `hb` line is current.
- **What each counter means:**
  - `n2[rblk]` or `n2[vblk]` > 0 is a direct observation of the N2 race.
  - `nowaker` > 0 is the resulting wedge.
  - `wakefl` separates starved wakers.
  - `latereply` separates benign late replies from N2.

**What single boots show (PR evidence):**
- one text and one gpu `runs=1` log with `src=hb` after every heartbeat, one `src=g1`, one `[smp]` line per online CPU, all `tick` > 0, and `rsthold` = 0;
- the hazard grep empty;
- the `llvm-objdump -h` layout;
- the V1 parity report;
- the T-self excerpt;
- `just check` and `just test` output;
- the B1 table.

**The PR description must state:**
1. "Acceptance soak pending. It depends on step 1a (classes, tripwire columns, interleave mode, Fisher) per ADR:361."
2. The pending protocol: interleaved, 20 text + 10 gpu per arm, 1b against `main`@`b07d7e4`, same load rule, QEMU, firmware and toolchain (`nightly-2026-09-24`).
3. The decision rules, with the ctx restriction, `lkself`/`kind=self` as H3 evidence, `kind=ph` not wedge evidence by itself, and the harness matching `lock re-entry: ` on the message line.
4. The schema-v1 key list, the golden line and the last-whole-line parser contract.
5. The known behaviour differences from `main` (§4.5 in full): lock fast-path overhead, the scan holds and their effects on `clear_timeout`/`check_timeouts`/the balancer, the print timing, the bench masking removal, `CONSOLE_BUSY`, post-panic masking and full drain, the 192-B frame, and the balancer `saturating_add`.
6. `frame.rs:51` → `[mm] BUG: free_pages(`; `channel.rs:249` → `badchan` (3 sites).
7. The B1 N2 baseline table, labelled single-arm and descriptive.
8. The PR is ready to merge as instrumentation. The H1 and H3 verdicts and any ADR revision wait for the soak.
9. The ADR errata that D1 applies.

Whether the user merges before or after the soak is their call through `/merge-and-cleanup`.

**Key files (worktree `/Users/juslee/Documents/workspace/juslee/aios/.claude/worktrees/crash-1b`):**
- `docs/knowledge/decisions/2026-09-22-jl-crash-fix-preemption-and-fp.md`
- `kernel/src/arch/aarch64/{exceptions.rs,gic.rs,timer.rs}`
- `kernel/src/sched/{scheduler.rs,mod.rs,init.rs}`
- `kernel/src/ipc/{direct.rs,channel.rs,timeout.rs,notify.rs,select.rs,mod.rs}`
- `kernel/src/cap/mod.rs`, `kernel/src/task/{mod.rs,process.rs}`
- `kernel/src/main.rs`, `kernel/src/smp.rs`, `kernel/src/bench.rs`, `kernel/src/mm/frame.rs`
- `kernel/src/observability/mod.rs`
- `shared/src/{lib.rs,collections.rs}`
- `scripts/soak-qemu.sh`

---

### 6. Review disposition

- **L1 — ACCEPTED.**
  - Holder fields are now stored immediately after the CAS and cleared in the guard's `Drop` before `store(0, Release)`.
  - `holder_tid` and `CURRENT_TID` start at `TID_NONE = u32::MAX`, because slot 0 is real (`sched/mod.rs:116-125`).
  - Readers take a consistent snapshot (word, fields, word again). NONE prints as `?`, and `lkself` requires a non-NONE tid.
  - For the residual window, the `[panic]` line prints `irq_elr` (`ELR_EL1`, still the interrupted PC in irq/irq-exit ctx) rather than reading the frame slot.
  - Changes: §2.1 and §2.7; S2/S3 tests.
- **L2 — ACCEPTED.**
  - Verified: WAKEUP_ERRORS is held with IRQs on at `timeout.rs:133`/`:145`, `schedule()` never takes it, and the balancer can migrate the preempted holder.
  - The fix splits the other-CPU outcome into `OtherCpu` and `OtherCpuSwitched` (`lkoxp`). The `lkself`/`kind=self` check runs in both preempted arms, and `owner_cpu`/`owner_gen` are on the event lines.
  - Step 1a and the ADR errata count `kind=self`/`lkself` with ctx ≠ thread* as H3 evidence (§2.1, §5, D1).
- **L3 — ACCEPTED.**
  - The PreemptedHolder arm checks the holder tid against `CURRENT_TID[k]` for k ≠ this CPU. If found, it bumps `lkphrun` and prints no line; otherwise the `ph` line carries `holder_running=none|?`.
  - The "near-certain wedge" comment is replaced. `kind=ph` is not wedge evidence by itself (§2.1, D1).
- **L4 — ACCEPTED, with a changed mechanism.**
  - The invariant is reworded as the (a)/(b) soundness statement (§2.1).
  - Rejected part: the debug-only *assert*, because the soak build is a dev build and an assert would add a PANIC class.
  - Replacement: a count-only `rsthold[cpu]`, which walks the 23 lock words for stamp `(cpu, SWITCH_GEN[cpu])` before each of the 5 `restore_context` calls. Expected 0.
- **L5 — ACCEPTED.** Verified that `check_timeouts` → `unblock` (`scheduler.rs:371`) and `try_load_balance` (`init.rs:257`) run before `check_preemption` (`timer.rs:194`, `:197-199`; `gic.rs:262`). T-self now accepts any of the three sites, with ctx ∈ {irq, irq-exit}.
- **L6 — ACCEPTED.**
  - Verified: `rust-toolchain.toml` = `nightly-2026-09-24`, `rustc 1.100.0-nightly (6eeff9a52 2026-09-23)`; `6bb1652a0` is `nightly-2026-09-23`.
  - V1 and D1 are updated. Both ELFs are built at 09-24, with the `main` arm at `b07d7e4`, whose kernel code equals `aa1f128`. The baseline header is updated.
- **ASM-1 — ACCEPTED.**
  - Verified: `boot.S:287-291` installs VBAR with `adrp` while the MMU is off, and `kmap.rs:234-235` takes `&__text_start` (`adrp`+`add` under the static relocation model).
  - The text bounds are now `TEXT_LO`/`TEXT_HI`, captured at VA in `kernel_main`.
  - New §2.11 states the general rule (normalise or pre-capture any compared or published address value), replacing §4.6's `adrp`-only rule. S3 gains a physical-bounds test.
- **ASM-2 — ACCEPTED.** Merged into the V1 redesign: a call graph from `irq_el1_entry`, source-line parity, explicit listing of new external callees, and a check that new code contains no `blr`.
- **ASM-3 — ACCEPTED.** Verified: `assert_valid_ctx` calls `drain_logs` on any CPU (`scheduler.rs:33`). The global `DRAINING` flag is replaced by `CPU0_DRAINING`, written only when `core_id() == 0`. The concurrent cross-CPU SPSC violation predates 1b and is noted.
- **ASM-4 — ACCEPTED.** Verified: `DRAIN_BATCH_SIZE = 16` with one shared counter (`observability/mod.rs:252`, `:263`, `:266-267`). `drain_logs` returns the lines drained, and the panic path loops, bounded at 128 iterations. The PR records this as a behaviour difference.
- **ASM-5 — ACCEPTED (docs and acceptance).** Verified: `smp.rs:164-171` breaks out after 100 ms. §2.9 states the exception, and K10 accepts "one line per CPU in `ONLINE_CPUS`".
- **SCAN-1 — ACCEPTED.**
  - Verified: `ipc_send` takes the receiver at `channel.rs:455` and clears its timeout at `:457` with IRQs on before `unblock`. The same shape appears at `:94`/`:100`, `:368`/`:392`, `process.rs:190`/`:196` and `ipc/mod.rs:222-231`.
  - Added `WAKE_PENDING`/`WAKE_AT` markers at the 10 reference-taking sites. They are cleared in `unblock` on every outcome, in `note_dispatch`, and at `notify.rs:323`'s abandoned-wake `continue`, which the reviewer did not list; without that clear an N3 victim would stay "in flight" forever.
  - Scan B splits `wakefl` from `nowaker`.
  - Two strikes now also needs every online CPU's `tick` to advance ≥ 100 (`scanstall`).
  - The "removes every transit window" claim is corrected in §2.6 and D1.
- **SCAN-2 — ACCEPTED, with changes.**
  - Verified: the late-reply path (`timeout.rs:105` → `channel.rs:368` → `direct.rs:234-241` → `scheduler.rs:376`), and that `try_reply_switch` accepts any `BlockedIpc` caller.
  - **Change 1:** phase 2 is stored *inside the TIMEOUT_QUEUE critical section* (`:120-125`, `:162-167`), not "before block". The reviewer's own evidence places the wedge window at `:168-174`, and storing under the lock makes phase 2 visible to a replier whose `clear_timeout` came after.
  - **Change 2:** the phase is snapshot inside `unblock` under THREAD_TABLE and returned in `UnblockOutcome`, so the skip decision and the phase are read together.
  - **Change 3:** `CALL_CHAN` is added, to separate "caller in a different call" from N2.
  - Result: `n2` (rpre, rblk, vpre, vblk), `latereply`, `misrep`.
  - Partly rejected: receive-side misdirected wakes. `select.rs:234` also registers `waiting_receiver`, so a select waiter would read as misdirected; this is out of scope for the owner's reply-focused gate. The residual same-channel re-call ambiguity is stated in §4.13.
- **SCAN-3 — ACCEPTED.** Checked tid-to-bit and tid-to-index conversions everywhere, `get()`, a `badtid` key, S3 tests with tid 64, 0x8000_0000 and `u32::MAX`, and clippy `indexing_slicing`/`arithmetic_side_effects` denied on the scan code.
- **SCAN-4 — ACCEPTED.** Separate (flag, `LAST_RUN`, tick) history per scan kind, updated only when that kind's phase completes, plus the S3 test flag(B) → A-only scan with a dispatch between → flag(B), which must not confirm.
- **SCAN-5 — ACCEPTED.** Edge counting through `CONFIRMED` masks, plus `*_now` gauges. This supersedes UART-BUDGET-TWC's suggested "count once per scan" wording.
- **SCAN-6 — ACCEPTED.** Verified: all 4 `CURRENT_THREAD` writers hold THREAD_TABLE (`scheduler.rs:79`/`:84`, `:218`/`:244`; `direct.rs:61`/`:153`, `:212`/`:285`), and `note_dispatch` writes `CURRENT_TID` there. Phase 1 reads `CURRENT_TID`, and the invariant is documented on `note_dispatch`.
- **SCAN-7 — ACCEPTED.**
  - Added `scanhold1`/`scanhold2` keys with a < 100 µs K8 bound, and listed the three effects (`clear_timeout` skip, `check_timeouts` skip, the N3 drop) in §4.5 and the PR.
  - The print is deferred while `held_by_stream(0)` (a stamped IRQ-class lock held by CPU 0's interrupted stream), capped at 256 ticks (`hbdefer`).
  - The `spin::Mutex` locks (CHANNEL_TABLE, PROCESS_TABLE) cannot be detected; that residual is stated.
- **V1-GATE-INLINING — ACCEPTED.** V1 is now the differential source-line parity gate over the IRQ call graph, with halt-terminal functions exempt. Scan accumulators moved to static atomics, `classify_slot` takes scalars, and new IRQ-path helpers are `#[inline(never)]`.
- **N2-BASELINE-UNADDRESSED — ACCEPTED, partly.**
  - Added the owner-constraint statement, the §5 "N2 baseline" subsection and task B1 with the exact extraction recipe (`\r`, valid-line check against `n=`, missing key = 0, `src=panic|exc` for fatal boots, `summary.tsv` columns 3 and 22).
  - Clarified that the "no sequential batches" rule applies only to the A/B acceptance.
  - K6 no longer depends on K5.
  - Rejected part: starting the baseline before K5. B1 measures the kernel that will merge, and a pre-K5 kernel has different lock code and scan-skip behaviour.
- **SCAN-BALANCER-OVERFLOW — ACCEPTED.** Verified: `init.rs:207`, `:211-213`, `:226`, with overflow checks on in the dev profile. Took the `saturating_add` fix (§2.12), which gives identical results whenever any queue was read. §4.5 now lists subset balancing. The per-queue snapshot alternative was rejected: it would add skew windows that two strikes absorbs only for dispatched threads.
- **SCAN-PLACEMENT-TIMEOUTS — ACCEPTED.** Verified the handler order (`timer.rs:178-208`). The scans and both prints now run at the end of `timer_tick_handler`, after step 8. TIMEOUT_QUEUE/NOTIFY_DEADLINES contention effects are added to §4.5 and the PR, and `ctbusy` counts the `clear_timeout` skips.
- **UART-BUDGET-TWC — ACCEPTED, partly.**
  - Verified: 300 B × 4.7 µs = 1.41 ms ≈ 88,000 ticks > 62,500.
  - `twc` is now cumulative, with `twn` and `twmax`. The K8 bound comes from the measured line length (≤ 3 ms, i.e. 187,500 ticks, plus the model check). The "every key monotonic" claim is withdrawn and replaced by the last-whole-line parser contract, and the §4.3 estimate is revised to 400–500 B.
  - Rejected part: "a flagged thread increments once per completed scan". SCAN-5's edge counting is used instead.
- **BENCH-INTERLEAVE — ACCEPTED.** `CONSOLE_BUSY` is set by `bench_main` around `bench.rs:362-450` (unlocked UART, `uart.rs:11-14`). CPU 0 defers the `NonZero` and g1 lines while it is set, within the same 256-tick cap.
- **K7-NEVER-EXEMPTION — ACCEPTED.** `note_dispatch` returns the old `LAST_CPU`, and the restore-site check uses that value.
- **BADCHAN-COVERAGE — ACCEPTED.** Verified: `cap/mod.rs:95-96` and `select.rs:66` reject first on the recv, send, call and select paths. `badchan` is now 3-wide (cap, select, slot), with a per-site self-test baseline recorded at K6.
- **UBRUN-REPLY-AMBIGUOUS — ACCEPTED, merged with SCAN-2.**
  - The phase counters (stored under the TIMEOUT_QUEUE lock) separate `n2[rblk]`, the exact wedge precursor, from `latereply`.
  - `clear_timeout` now returns `ClearResult`, used for `ctbusy` (a timeout left live, so the caller heals).
  - The reviewer's `ubrun_live` key is not added, because `n2[rblk]` carries the same information more precisely.

## Issues Encountered

(to be filled during implementation)

## Decisions Made

(to be filled during implementation)

## Lessons Learned

(to be filled during implementation)
