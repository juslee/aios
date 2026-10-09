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

- [x] S1: `FixedQueue::iter()` / `contains()` (shared, host tests)
- [x] S2: `shared/src/lock.rs`: owner stamp, `classify`, `StampedLock`, `LockClass` (host tests, Miri)
- [x] S3: `shared/src/tripwire.rs`: key catalogue, `WakeSource`, `CpuCounters`, line writer, `classify_pc`, scan classification, two strikes (host tests)
- [x] K1: Bench DAIF masking removed
- [x] K2: Tripwire runtime, per-CPU ticks, heartbeat and g1 lines
- [x] K3: Dispatch bookkeeping: switch generation, `CURRENT_TID`, `IRQ_CTX`, last CPU, `schedule(origin)`
- [x] K4: IRQ frame ELR/SPSR snapshot and mismatch counter (192-byte frame)
- [x] K5: `IrqSpinLock` (detect-only) on the 9 IRQ-shared statics
- [x] K5b: stamp CPU id from `TPIDR_EL1` (variant T), its cross-checks, the Gate 1 zero-iteration guard, and the run-08 evidence
- [x] Merge: `main` `c6f5511` merged (`docs-check` lock-order ported to Rust; boot deferred)
- [x] Merge219: `main` `5838ad2` merged (#219 LogRing drop-on-full, continuation entries and `daif.rs`; #220 `aios hook` subcommands; boot deferred, load 234–333)
- [x] K6: Wake attribution and the N2 counters (`unblock`/`wake_with_error`/`try_wake_select` sources, call phase; boot deferred, load 408; review fix 1 (`ctbusy` not counted on the `to` path): boot deferred, load 135)
- [x] K7: Restore-site PC/SP checks (N5) (boot deferred, load 135 at the start and 91 at the gate)
- [x] K8: Heartbeat scans: orphan, no-waker, starved (two strikes, wake-pending markers; boot deferred, load 73 at the start and 201–215 at the gate)
- [x] MergeR4: `main` `1a5c363` merged (nightly-2026-10-09; #228 `pyre` classes and the git-history check.py oracle; #230 Tools R4, `just soak` is the Rust `aios soak`; boot deferred, load 50 at the start and 71 at the gate)
- [x] K9: Fatal dumps (panic handler masks IRQs first) and `#[track_caller]` on the free paths (boot deferred, load 28 at the start and 78 at the gate)
- [x] K10: `[smp]` VBAR/TTBR0 lines (booted at load 21–28: text and gpu); review fixes: §6.5 log-order note (docs only)
- [x] T-self: `tripwire-selftest` feature: end-to-end PANIC-LOCK proof (not in either soak arm) (booted at load 10–17: the feature boot, plus default text and gpu)
- [x] V1: V-register listing: differential gate over the IRQ call graph (no repository file; passes, no contamination; no boot needed)
- [x] D1: Docs sweep and ADR errata (docs only, no boot; `/audit-loop` left to the lead)
- [ ] B1: Single-arm N2 baseline soak on the final 1b head (owner gate; host-exclusive, coordinated with team-build, the other session, formerly aios-b9)
  - **Owner decision before B1 (K5 review 2):** accept the K5 lock's cost for B1 and the A/B soak (the Gate 1 IPC round trip about doubles, and Gate 1's `IPC < 10 us` fails in most boots), or first cut the stamp reads that cause it. Evidence and the measured split: Issues Encountered, K5 (review 2). **Decided 2026-10-06 (owner): accept the cost.** B1 measures the N2 counters, not IPC, and both A/B arms carry the same lock.

Every task: one commit `Crash fix step 1b: <description>`, `just check` with zero warnings, `just test`, and for kernel tasks one text and one gpu boot (`just soak runs=1 secs=75 report_only=1`, then `mode=gpu`) with the task's boot acceptance from Design §3.

## Design (revision 2)


**Baseline:** worktree `crash-1b`, branch `claude/crash-fix-step-1b-tripwires`, at `main` `b07d7e4`. The kernel code there is identical to `aa1f128` apart from comment text, so every `aa1f128` line number below still holds. `rust-toolchain.toml` at `b07d7e4` pins `nightly-2026-09-24`, which reports `rustc 1.100.0-nightly (6eeff9a52 2026-09-23)`.

**Baseline update (2026-10-06, after K5b):** merged `main` `c6f5511` into the branch. The toolchain is now `nightly-2026-10-06`, which reports `rustc 1.101.0-nightly (ea137335b 2026-10-05)`. The project memory moved to `.claude/CLAUDE.md`, and `scripts/docs/check.py` is now the Rust `aios docs-check` (`tools/src/cmd/docs_check/`). Kernel line numbers below may have drifted; locate sites by content.

**Baseline update (2026-10-06, after the first merge):** merged `main` `5838ad2`: #219 (`4ddabea`: `arch/aarch64/daif.rs`, LogRing drop-on-full with 96-byte messages in a head entry plus a continuation, pid-owned channel endpoints and `process_exit`'s pid walk) and #220 (`aios hook` subcommands, tools only). The toolchain is unchanged. The interaction with the tripwires and the line drift for K6 and K9 are under Decisions Made (Merge219).

**Baseline update (2026-10-09, after K8):** merged `main` `1a5c363`. The toolchain is now `nightly-2026-10-09`, which reports `rustc 1.101.0-nightly (a30aa9064 2026-10-08)`. `scripts/soak-qemu.sh` is gone: `just soak` runs the Rust `aios soak` (`tools/src/cmd/soak/`, #230), with the same arguments and output layout, so the soak commands below are unchanged. Details under Decisions Made (MergeR4).

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
pub fn classify(observed: u64, me: OwnerStamp, v: &impl CpuView) -> Contention; // ignores IRQS_ON
//   me = the waiter's own stamp, taken before `observed` was loaded (the stamp its failed CAS used)
//   now = v.switch_gen(owner cpu), read once, after the word load
//   waiter still in its generation: owner cpu == me.cpu && me.gen == now
//   Reentry:          waiter still in its generation && owner gen == now (so owner == me)
//   PreemptedHolder:  waiter still in its generation && owner gen != now
//   OtherCpu:         otherwise, owner gen == now   (count only)
//   OtherCpuSwitched: otherwise, owner gen != now
pub trait CpuView { fn cpu(&self) -> u8; fn switch_gen(&self, cpu: u8) -> u64; } // fresh reads, see S2 review
pub fn read_stamp(v: &impl CpuView) -> (u8, u64); // cpu, gen[cpu], cpu again; retry until equal
pub struct StampedLock<T> { word: AtomicU64, data: UnsafeCell<T> }
pub enum LockClass { ThreadTable, CurrentThread, RunQueues, WakeupErrors, TimeoutQueue,
                     NotifyDeadlines, NotificationTable, SelectWaiters, BootLog } // COUNT=9
pub const TID_NONE: u32 = u32::MAX;
```
One CAS takes the word from 0 to the stamp, and the unlock stores 0 with Release. This is the same read-modify-write and orderings as spin 0.12.3 (`mutex/spin.rs:233-240`), with the same test-and-test-and-set spin, `core::hint::spin_loop()`, and no fairness. The Reentry decision uses the word, the waiter's own stamp and `SWITCH_GEN[stamp cpu]` read after the word load. A waiter with IRQs on can be moved between taking its stamp and classifying, so its CPU alone proves nothing (S2 review). The cross-CPU `switch_gen` read in the other arms only splits a count.

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
- *IRQs on:* `read_stamp`, then CAS, then store the fields, then `read_stamp` again and `restamp` if it differs. A stale stamp, the holder's or the waiter's, can only err towards PreemptedHolder, OtherCpu or OtherCpuSwitched, never towards Reentry.
- *Waiting with IRQs on:* take a fresh stamp before each word load that is classified, and pass it to `classify`.

**`lock()` (`#[track_caller]`):**
```text
s = stamp_now(); if weak CAS ok → set holder fields → (IRQs on: re-stamp) → guard
loop:
  w = owner_word(); if w == 0 → retry CAS
  match classify(w, s, v):                            // s: the stamp taken before this w was loaded
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

**Soundness statement, documented on the type (L4, S2 review):** Reentry means the word holds the waiter's own stamp `(c, g)` and `SWITCH_GEN[c]`, read after the word load, is still g. The test is free of false positives because:
- (a) every `restore_context` on CPU c is preceded, on c with IRQs masked, by `note_dispatch` bumping `SWITCH_GEN[c]`, with c read from MPIDR (5 restores ← 4 commits: `scheduler.rs:84→94`, `:244→280/286`, `direct.rs:153→186`, `:285→312`);
- (b) the dispatching stream releases every guard it took after that bump before `restore_context`. Those are the temporaries at `scheduler.rs:267`, `direct.rs:178`, `:290` (`enqueue_on_cpu`) and `:305`;
- (c) a thread's execution is ordered across a switch: whatever the CPU that switched it out did first, including the bump in (a), happens before the thread's next instruction on any CPU. The scheduler needs this anyway to resume a thread from its saved context. A thread resumed before its context was saved (the missing `on_cpu` handshake, ADR H2/F4) breaks this and every other invariant with it.

Under (c), a waiter that was switched out or moved after stamping sees `SWITCH_GEN[c]` changed, so the post-load check rules out another thread that now runs on the waiter's old CPU. A waiter whose own generation has ended gets the other-CPU verdicts. Guards held by a preempted thread across a switch are expected; they classify PreemptedHolder, OtherCpu or OtherCpuSwitched.

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

**N2 phase protocol (SCAN-2, UBRUN-REPLY-AMBIGUOUS, S3 review).** Each thread writes only its own slots, with plain Release stores. The phase values come from `shared::tripwire::wait_phase(step, timed)`: step 1 (`PHASE_PUBLISHED`) or 2 (`PHASE_ARMED`), with the flag `PHASE_UNTIMED` (0x80) set when the wait has no timeout. A call is timed when `timeout_ticks > 0`, and a receive when `timeout_ticks < u64::MAX`.

| Store | Where |
|---|---|
| `CALL_CHAN = channel`, then `CALL_PHASE = wait_phase(1, timed)` | `channel.rs:91`, right after `pending_caller = Some` |
| `CALL_PHASE = wait_phase(2, timed)` | **Inside** the TIMEOUT_QUEUE critical section at `:120-125` and `:162-167`, before the guard drops; or at the same point when `timeout_ticks == 0` (untimed). Timed phase 2 therefore means "my timeout is registered". A replier whose `clear_timeout` acquired TIMEOUT_QUEUE after that registration also sees phase 2. |
| `CALL_PHASE = 0` | `:180` |
| `RECV_CHAN`/`RECV_PHASE = wait_phase(1, timed)` | `channel.rs:275` |
| `RECV_PHASE = wait_phase(2, timed)` | Inside `:281-286`, or after `:276` when `timeout_ticks == u64::MAX` (untimed) |
| `RECV_PHASE = 0` | `:294` |

**Phase alone is not enough (S3 review).** Two interleavings leave a timed waiter at phase 2 although it heals:
- *Late reply after a timeout wake:* `check_timeouts` removes the entry and `wake_with_error` makes the caller Runnable. Its phase stays 2 until it runs to `:180`, and `pending_caller` stays set until about `:204`, so a reply in that window takes the caller, finds no entry to clear, fails `try_reply_switch` (the caller is not BlockedIpc) and skips it. The caller returns ETIMEDOUT.
- *Early reply seen armed:* the reply's `clear_timeout` (`:392`) runs before the caller registers, and the caller stores phase 2 before the reply's `unblock` (`:403`). The registered timeout heals the caller.
- The receive side has the same two cases: the receiver's phase stays 2 until `:294`, and `ipc_send` clears at `:457` before `unblock` at `:460`.

Only a skipped wake that leaves **no timeout** is a wedge precursor: the waker's own `clear_timeout` returned `Removed`, or the wait is untimed (then phase 1 is a wedge precursor too, since nothing registers later). `clear_timeout` returns `ClearResult { Removed, Absent, Busy }`, and the reply, send and call wakers pass their own result into the classifier.

**Classification** (`classify_reply(outcome, channel, clear)`, `classify_send(outcome, channel, clear)`), with `bump()` in thread context:
- `ipc_reply` fallback (`:403`), on outcome skip (Running or Runnable) with `CALL_CHAN == channel`:
  - untimed, phase 1 or 2 → `n2[1]` (`rblk`);
  - timed, phase 2, `clear == Removed` → `n2[1]` (`rblk`: the reply removed the registered timeout and the caller will block with no waker. **This is the exact N2 wedge precursor**);
  - timed, phase 1 → `n2[0]` (`rpre`: reply before the caller's timeout registration; the caller heals with ETIMEDOUT later);
  - timed, phase 2, `clear` Absent or Busy → `latereply` (the timeout fired already, was registered after the reply's clear, or was left by a busy clear; it heals the caller).
- `ipc_reply` fallback, on outcome skip with phase 0 or `CALL_CHAN ≠ channel` → `latereply` (the caller had already left the call, usually woken by its timeout).
- `ipc_reply` fallback, on outcome woke with step ≠ 2 or chan ≠ channel → `misrep` (the reply woke a thread blocked in some other wait).
- `try_reply_switch`, after validation at `:243`: under THREAD_TABLE, `classify_reply` with `UnblockKind::Woke`, the caller's phase and the reply's `ClearResult`; step ≠ 2 or `CALL_CHAN ≠ channel` → `misrep` (`bump_masked`). The caller is Blocked there, so its phase is stable.
- `ipc_send` (`:460`) and `ipc_call` (`:139`) fallback, on outcome skip with `RECV_CHAN == channel`: untimed phase 1 or 2, or timed phase 2 with `clear == Removed` → `n2[3]` (`vblk`: the receive-side analogue; the receiver blocks with no waker); timed phase 1 → `n2[2]` (`vpre`); timed phase 2 with `clear` Absent or Busy → not counted (the timeout heals the receiver). Receive-side misdirected wakes are not counted (see §6 SCAN-2).
- `ClearResult::Busy` also bumps `ctbusy` (a stale timeout left behind), at the reply, send and call wakers and in `wake_with_error` for every source except `to`: `check_timeouts` has already taken the entry it wakes, so a busy clear there leaves nothing behind.

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
| Per-CPU | `tick`, `irqsw`, `irqsw0`, `nest`, `elrmm`, `spsrmm`, `n4`, `insched`, `rsthold`, `tpidrbad` (K5b) |
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
- A flag is **confirmed** when the thread is flagged in two consecutive completed scans of that kind, with `LAST_RUN` unchanged, **and** every online CPU whose per-CPU `tick` has ever advanced has advanced by ≥ 100 since the first strike (owner, 2026-09-28: CPU 0 only on this kernel, all four once #200 is fixed). If not, bump `scanstall` and keep the first strike. This covers a vCPU stalled mid-window on a ticking CPU. Stall detection for a CPU that never ticks is a documented residual.
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
  - The waiter side, exhaustive (S2 review): the waiter is also moved 0–2 times before its word load and before `classify`'s generation read, against holder stamps for every generation of either CPU, including the thread that now runs on the CPU the waiter left. No false Reentry, no missed Reentry for a waiter that never moved, and exact other-CPU verdicts.
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
  - `classify_reply`/`classify_send`: every (outcome, phase step, timed, `ClearResult`, chan match) cell, plus the named kernel interleavings (S3 review).
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
- **Boot acceptance:** `irqsw` > 0 on CPU 0, and on every CPU that takes timer IRQs (CPUs 1–3 take none until #200 is fixed, so their `irqsw` reads 0 on this kernel); `nest`, `insched`, `n4` and `irqsw0` expected 0 or small.
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
- **Review checklist (the `shared::lock` contract, S2 review):**
  - `CpuView::cpu` reads MPIDR with an `asm!` that uses none of `pure`, `nomem` or `readonly`. It is not `exceptions::core_id()`, which is `nomem`. Without those options the `asm!` is a compiler barrier, which keeps the CPU, generation, CPU reads of `read_stamp` fresh and in order.
  - `CpuView::switch_gen` is a fresh atomic load of `SWITCH_GEN[cpu]`.
  - `classify(w, s, v)` gets the waiter's own stamp `s`, taken before `w` was loaded: the stamp of the CAS that failed, or with IRQs on a new `read_stamp` after each spin, before the next word load.
  - The `lkself` compare needs the waiter's own tid. `CURRENT_TID[s.cpu()]` is that tid only while `s` is current, so re-check `SWITCH_GEN[s.cpu()]` after reading it, or skip the compare. After an OtherCpu or OtherCpuSwitched verdict, `s` may already be stale.

**K5b. Stamp CPU id from `TPIDR_EL1` (variant T), Gate 1 guard, run-08 evidence** (owner, 2026-09-29; depends on K5)
- **Why:** under QEMU TCG each `MPIDR_EL1` read is two C helper calls with a full guest-register spill, while a `TPIDR_EL1` read is one inline load. K5's stamp does about 135 MPIDR reads per Gate 1 round trip (33 lock acquisitions). Variant T recovered about 2.8 us of K5's ~6.5 us IPC cost in the 2026-09-29 paired experiment, with Gate 1 passing 9/10 against head's 2/4; TW gave no measurable extra gain. Evidence: the worktree's `target/dig/a1/` (anatomy, microbenchmarks, plugin counts) and `target/dig/a2/report/` (paired rounds).
- **Change (base it on `target/dig/a1/diffs/T.diff`):**
  - `boot.S`: write `TPIDR_EL1 = MPIDR_EL1 & 0xff` once per CPU before any Rust code, on the boot CPU path and the secondary entry path.
  - `observability/tripwire.rs`: `cpu_tpidr()`, with the same contract as `cpu_here()` (fresh read, compiler barrier, no `pure`/`nomem`/`readonly`).
  - `sync/irq_spin_lock.rs`: the stamp's CPU reads (`read_stamp`'s double read, the re-stamp) use `cpu_tpidr()`. Every S2 soundness rule stays; the in-hold instructions change only in the system-register operand, so translation-block parity with K4 (K5 review 1) must still hold — recheck it.
  - Cross-checks that `TPIDR_EL1` matches `MPIDR_EL1` Aff0, count-only (never panic, detect-only): at `kernel_main` entry, at `secondary_main` entry, and in `note_dispatch`. A new tripwire key, for example `tpidrbad`, expected 0.
- **Gate 1 zero-iteration guard:** `bench.rs:82-86` returns an average of 0 when `iterations == 0`, and the Gate 1 line (`:433-438`) then prints PASS. Make a zero-iteration run report FAIL (or `n/a`), and note it in the plan; seen in two plugin boots.
- **run-08 evidence (K5 review 3's should-fix):** add a K5 Issues Encountered entry for `target/soak/20260928-145827-text/run-08.log`: the two `[tripwire-ev] kind=stuck lock=TIMEOUT_QUEUE ... owner_gen=272869 holder_tid=?` lines, `lktph[TIMEOUT_QUEUE]` rising by 1 per tick from tick 576 (`:196`), `lkstk`=2, `elrmm` 15→826 against `irqsw` 65→1557, and the 2026-09-29 deep-dive reading: H1 resumed the lock holder at the other bench thread's PC, and none of K6–K8's scans would have named it.
- **Boot acceptance:** text and gpu boots; IPC avg in the text boot recorded next to K5's; `tpidrbad` = 0; no new class; the hazard grep empty.
- **Docs:** CLAUDE.md Key Technical Facts: "`TPIDR_EL1` = MPIDR Aff0, written by boot.S on every CPU; the IRQ-class lock stamps with it".

**K6. Wake attribution and N2 counters** (depends on S3 and K3; **not** on K5)
- **Files:**
  - `scheduler.rs` (`unblock(tid, src) -> UnblockOutcome`, outcome counters, phase snapshot, `WAKE_PENDING` clear);
  - `timeout.rs` (`wake_with_error` src, `clear_timeout -> ClearResult` + `ctbusy`, marker at `:97`);
  - `select.rs:293`;
  - the 18 call sites;
  - `channel.rs`: `CALL_*`/`RECV_*` phase stores at `:91`, `:120-125`, `:162-167`, `:180`, `:275`, `:281-286`, `:294`, with the values from `wait_phase(step, timed)`; markers at `:94`, `:368`, `:455`, `:493`; `classify_reply` at `:403`; `classify_send` at `:139`, `:460`;
  - **the waker's own `ClearResult` must reach the classifier (S3 review):** keep the result of `clear_timeout` at `:100` until the `:139` fallback (across `try_direct_switch`), at `:392` until `try_reply_switch` and the `:403` fallback, and at `:457` until the `:460` fallback. `try_reply_switch` therefore also takes the `ClearResult`;
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
- **Boot acceptance:** `pcother` = 0 in CLEAN boots. `pcphys` counts IRQ-path switches on CPUs 1–3, which take no IRQs until #200 is fixed, so `pcphys` = 0 is expected on this kernel.

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
- ADR amendment for #200 (owner, 2026-09-28): CPUs 1–3 take no timer IRQs because `init_gicv3_secondary` never sets `GICR_IGROUPR0`, so the H1 IRQ-path switch occurs only on CPU 0 (fits every PCZERO and EXCEPTION in run 167), N5 cannot fire, N7's timeouts-on-every-CPU reasoning does not hold, and a thread moved to CPU 1–3 runs until it yields or blocks. The fix is its own crash-fix step, the first behaviour change after step 1a, A/B-measured; the N2 baseline (B1) is re-taken after it.
- CLAUDE.md: Workspace Layout; Key Facts (192-B frame, `IrqSpinLock` detect-only, `CURRENT_TID` invariant, IRQ-path address rule §2.11); a note that the NC limitation does not apply to kernel statics after boot.
- Rules 01 and 05; `kernel-dev.md`; `developer-guide.md` (test counts `:1579`, `:1631`, `:1724`; panic pattern); `deadlock-prevention.md`; `observability.md`.
- The three places that still describe the IRQ-masked bench loop K1 removed:
  - `developer-guide.md:1785` (soak heartbeat rule 1): rewrite. It explains a `tick=0` heartbeat stuck after the bench header as "the IRQ-masked loop hanging on CPU 0".
  - `development-plan.md:247` (Gate 1 benchmark methodology): add a note, do not rewrite. "IRQs masked during measurement" records how the Phase 3 figure was taken; say that from step 1b on the bench runs with IRQs on, and that the masking only ever covered the first call (`direct.rs:192`).
  - The `tick == 0` classifier comment, which says the bench "runs an IRQ-masked IPC loop": rewrite. It was `scripts/soak-qemu.sh:360-362`; #230 deleted the script, and the comment now lives in its Rust port, `tools/src/cmd/soak/classify.rs` (the `self.tick == 0.0` branch of the classifier, `:521-522` at `1a5c363`). Editing it touches `tools/`, so D1 then runs the three tools gates. The ADR's `soak-qemu.sh` line-drift erratum no longer depends on it (MergeR4).
- An errata block in the crash-fix ADR:
  - the PANIC-LOCK two-line form;
  - `frame.rs:51` → `[mm] BUG: free_pages(`;
  - the 15-source unblock list;
  - SELECT_WAITERS not a waker, NOTIFY_DEADLINES scoped, sleep = `BlockedIpc{MAX}`;
  - transit windows (`:386`; add `:192-244`, `:73-84` and creation; drop `:174-177`), and the waker-in-transit split (`wakefl`);
  - N2 measured by `n2[rblk]`/`n2[vblk]` (phase counters plus the waker's own `ClearResult`, §2.4), and `ubrun[reply]` also counting late replies;
  - `channel.rs` line shifts (`278→275`, `293→290`, `361→355`, `374→368`, `398→392`, `409→403`);
  - `channel.rs:249` gone (→ `badchan`, 3 sites);
  - the ELR compare placement;
  - "post-#161 / nightly-09-22" → nightly-2026-09-24 (`6eeff9a52`);
  - `cargo objdump` → `llvm-objdump`;
  - `soak-qemu.sh` line drift (`:205-206` → `:208-209`, `:356-383` → `:359-386`, `:598-615` → `:614-631`, `:636` → `:652`, `:677` → `:693`, `:688` → `:704`), stated against the script's last blob (`9689334`, unchanged from `aa1f128` until #230 deleted it; the `aios soak` sources cite it as "the blob at `212df62`"), and a note that the script is now the Rust `aios soak` (#230), so the ADR's `soak-qemu.sh` line citations name that blob, not a live file;
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
     - **Measured (K5 review 2):** the IPC round trip goes from avg 4–7 us (min 3.0–5.0 us) with K4's `spin::Mutex` to avg 10–13 us (min 7.0–11.0 us) with the K5 lock, in interleaved boots at load 7–13. The context switch goes from avg 1–4 us to 4–8 us.
     - Gate 1's `IPC < 10 us` passes in 17 of 18 K1–K4 boots and in 1 of 14 K5 boots. `Context switch < 20 us` still passes.
     - Nearly all of it is the stamp reads: per `lock()`, 1 DAIF and 4 MPIDR reads, 4 loads and a re-stamp store (`whoami`, `current_stamp_or`). The holder fields, the 64-bit word and the contended-path bookkeeping cost nothing measurable.
     - Accepting this or cutting the stamp reads is an owner decision before B1 (Progress).
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
   - **`rblk`/`vblk` need the waker's `ClearResult` (S3 review).** Phase 2 alone also covers a caller its timeout already woke (phase and `pending_caller` are cleared only when it runs) and a reply whose clear ran before the caller registered. Both heal, so a timed skip at phase 2 counts as `rblk`/`vblk` only when the waker's own clear returned `Removed`. Untimed waits (call timeout 0, recv `u64::MAX`; no in-kernel site uses them today, but the Kit and syscall paths pass caller values) count at phase 1 or 2, because no timeout can heal them.
   - Residuals of that rule: an early reply seen after the caller armed counts as `latereply`, not `rpre`, because it cannot be told apart from a late reply. A stale entry left by an earlier `Busy` clear (`ctbusy`) can make an already-woken caller's entry look `Removed` and give a false `rblk`; `ctbusy` > 0 flags that. If the timeout's own wake is also skipped (the entry was taken by `check_timeouts` just before the reply's clear), the reply counts `latereply` and the wedge shows as `ubrun[to]`/`ubrbl[to]` and `nowaker`.

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
  - `n2[rblk]` or `n2[vblk]` > 0 is a direct observation of the N2 race: a skipped wake that left the waiter no timeout (the waker's own `clear_timeout` removed it, or the wait is untimed). Timed skips at phase 2 whose timeout the waker did not remove are not counted there (§4.13).
  - `nowaker` > 0 is the resulting wedge.
  - `wakefl` separates starved wakers.
  - `latereply` separates reply skips that heal by the caller's timeout (mostly late replies) from N2.

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
  - `clear_timeout` now returns `ClearResult`, used for `ctbusy` (a timeout left live, so the caller heals) and, since the S3 review, passed into `classify_reply`/`classify_send` (§2.4).
  - The reviewer's `ubrun_live` key is not added, because `n2[rblk]` carries the same information more precisely.

## Issues Encountered

- S1: `just check` lints only the `aarch64-unknown-none` build, so test modules are never linted. `cargo clippy -p shared --tests -- -D warnings` on the host already fails on `main` in other modules' tests (`too_many_arguments`, `assertions_on_constants`, `needless_range_loop`, …). None of the failures is in `collections.rs`. S2/S3 should check their own files with that command, not expect it to exit 0.
- S2: none. Host clippy on the tests reports 0 findings in `lock.rs`. Miri passes the 16 lock tests, and the threaded test also passes under `-Zmiri-many-seeds=0..16`. Two mutations each fail the model tests: Reentry ignoring the generation, and `read_stamp` without its second CPU read. A host run of the threaded test takes about 20,000 contended snapshots, about 28% of them with a published tid, and none is inconsistent. Host tests: 564 → 580.
- S2 (review): with 25 iterations per thread under Miri, `just miri` (CI's Miri gate, one default seed) missed three ordering regressions in the threaded test. Removing the `fence(Acquire)` in `consistent_snapshot`, making `restamp` a Relaxed store, and making the test's holder-field store Relaxed all passed on the default seed. Only `-Zmiri-many-seeds` caught them.
- S2 (review 2): `classify(observed, cpu, v)` could report a false Reentry for a waiter with IRQs on. In the failing sequence, the waiter reads CPU 0 and is then moved to CPU 1. CPU 0's new thread takes the lock, and the waiter matches that thread's current stamp against its stale CPU. No test moved the waiter after its CPU read. The fix is in `shared` (see Decisions Made). Three mutations were run against the new tests:
  - The old rule (CPU match plus current owner generation) fails the new exhaustive waiter test, the table and the regression test.
  - A frozen waiter stamp without the post-load check fails the same three. The reviewer's stale `(0, 101)` case is now its own test, and it fails too.
  - `read_stamp` without its second CPU read still fails three tests.
- S2 (review 2): the exhaustive waiter test (2 × 7⁴ move sequences) takes about 40 s under Miri and 0.01 s on the host. The model is safe, single-threaded code, so Miri adds no checks there. Under `cfg(miri)`, each position uses at most one move (2 × 3⁴ sequences, about 2.4 s), following the threaded test's `cfg(miri)` sizing.
- S3: `u64::count_ones` compiles to NEON (`fmov d, x` / `cnt v.8b` / `addv`) on `aarch64-unknown-none`, which has no FEAT_CSSC. `EdgeUpdate`'s counts used it on the scan path. A throwaway no_std staticlib in the scratchpad instantiated every IRQ-path generic (`write_line` with a volatile UART sink, `CpuCounters<8>::value`, `TwoStrike<8>::scan`, `EdgeCounter`, `classify_*`, masks, `put_dec`/`put_hex`) at opt-level 1, and `llvm-objdump` found the V-register sites. The fix is `count_tids`, which uses a 16-entry nibble table. The re-check at opt-level 1 (43 functions) and opt-level 3 (12) finds no V-register, NEON, `memcpy`/`memset` or `blr` site in any tripwire function. **For K8 and all kernel IRQ-path code: never call `count_ones`.** V1 would catch it, but only at the end.
- S3: the `Full` line is longer than §4.3's estimate of about 700 B. With 4 CPUs and every value 0 it is 913 B (about 4.3 ms of UART at 4.7 µs/B), and 1829 B when every value has 5 digits. `MAX_LINE_LEN` (8 CPUs, every value `u64::MAX`) is 6039 B. `g1` prints once, and panic/exc lines are post-fatal, so only the timing estimates change.
- S3: the worst-case re-entry message is `NOTIFICATION_TABLE` at 153 bytes, not `CURRENT_THREAD[7]` at 152, because the scalar name is one character longer. The test takes the maximum over all 9 classes. The longest `kernel/src` path today is `kernel/src/arch/aarch64/exceptions.rs` (37 bytes). A second test walks `kernel/src` and re-checks the bound against the live tree; it is ignored under Miri, because Miri isolates the file system.
- S3: rendering the lines under Miri took about 88 s: 24 full renders in the hazard test and 60 single-key renders. That code is safe and single-threaded, so it is sized down under `cfg(miri)`: one line per mode, and one key per `Width`. The module's Miri tests now take about 37 s serially. `just miri` passes 621 tests with 1 ignored (the file-system walk) in 83 s of wall time. Host tests: 583 → 622.
- S3 (review): the §2.4 N2 table classified on (kind, phase, chan) alone, so two healing interleavings counted as `rblk` (and `vblk` on the receive side). In the first, a late reply reaches a caller its timeout already woke: the phase stays 2 and `pending_caller` stays set until the caller runs. In the second, the reply's `clear_timeout` runs before the caller registers, and the caller arms before the reply's `unblock`. Both were checked against `channel.rs`, `timeout.rs` and `direct.rs`. Untimed waits were also wrong: `rpre`/`vpre` counted them as healing. Process exit and channel destroy take `pending_caller`/`waiting_receiver` exclusively, so once a reply or send holds the reference, an untimed waiter has no other waker. Four mutations each fail the new tests: ignoring `clear` (the old rule), dropping the untimed arm, treating `Busy` as removed, and a `misrep` check blind to the untimed flag. A scratch staticlib (opt-level 1 and 3) shows no V-register, NEON, `memcpy`/`memset` or `blr` site in `classify_reply`, `classify_send`, `skip_fate` or `wait_phase`. Host tests: 622 → 624. `just miri`: 623 pass, 1 ignored.
- K1: three places still describe the IRQ-masked bench loop, and D1 must fix them (listed under D1 in §3). `docs/project/developer-guide.md:1785` (soak heartbeat rule 1) explains a `tick=0` heartbeat stuck after the bench header as "the IRQ-masked loop hanging on CPU 0". `docs/project/development-plan.md:247` (Gate 1 benchmark methodology) says "IRQs masked during measurement". `scripts/soak-qemu.sh:360-362` (the classifier's `tick == 0` comment) says the bench "runs an IRQ-masked IPC loop on the CPU that runs it". The development plan records how the Phase 3 figure was taken, so D1 should add a note there rather than rewrite it. The masking only ever covered the first call anyway (`direct.rs:192`).
- K1: boots at load average 15–45. Text: PCZERO at heartbeat tick 10000, after `=== Gate 1 Complete ===`, with `IPC round-trip (same core): avg=10 us, p99=82 us, … (10000 iters)`. Three gpu boots (`target/soak/20260927-221647-gpu`, `-221817-gpu`, `-221948-gpu`): each is the known `frame.rs:51` PANIC (`free_pages(0x1500, 0xffff0000000d7570…d8570)`), fired right after heartbeat tick 0, during display handoff. The bench still ran after it and reached K1's sites: the header, then `Bench main: server ready, starting IPC benchmark`, so the server had passed its ready store (where the removed `DAIFSet` ran) and bench main had entered the IPC phase. It then stalled, with no round-trip line. All three boots then hit a second fatal report, `EXCEPTION[CPU 0]: ESR=0x86000005 EC=0x21 FAR=ELR=0x2_0000_0000` (instruction abort, level-1 translation fault), at heartbeat ticks 1000, 28000 and 27000. That signature is in no run-167 log and not in the ADR. The PANIC and the stall are `main`'s (K1 (review) below). No `[tripwire` lines exist yet, so the hazard grep is empty.
- K1 (review): the K1 note first said the gpu boots panicked before the bench header and never reached K1's code. The logs show otherwise, and the note above is corrected. The post-panic bench stall is `main`'s. All 8 `frame.rs:51` boots in run 167 (5 `main`, 3 PR #161) started the bench, logged `server ready` and never printed a round-trip line. So did the one `frame.rs:51` boot in the reviewer's 6-boot gpu soak of `c1ce1a2` (the same kernel as `main` `7167d40`; logs in the session scratchpad, `soak-main-gpu/`). The post-panic CPU 0 exception is less clear-cut:
  - The `c1ce1a2` boot hit ESR 0x96000006, FAR 0, ELR `0xffff000000095bd4`. In a rebuilt `c1ce1a2` ELF that is `ldaxrb w8, [x20]` in `bench_main_entry`: the `SAMPLE_BUF` lock in the inlined measurement loop, with x20 zero instead of `SAMPLE_BUF`'s address. Run 167 has one post-panic exception in its 8 (`pr161-gpu-r2/run-04`: ESR 0x96000006, FAR 0, ELR `0xffff0000000a06cc`, at tick 46000). The 5 `main` boots ran on to tick 53000–54000 with none.
  - The fix-round gpu boot (`target/soak/20260928-061425-gpu`) was again `frame.rs:51` (`0xffff0000000d7170`), `server ready`, a stall, then `ELR=FAR=0x2_0000_0000` at tick 2000. That makes 4 of 4 K1 gpu boots with this exact signature, against 1 of 6 `main`-kernel `frame.rs:51` boots with any post-panic exception, at a different ELR.
  - So K1 probably changes the post-panic fallout. It removes only two `msr DAIFSet`, so it cannot branch to 0x2_0000_0000 itself. With IRQs on, the bench threads keep being preempted after the panic. That likely steers a pre-existing context corruption into this form: the `c1ce1a2` boot also resumed a bench thread with a corrupted callee-saved register.
  - The fix-round text boot (`target/soak/20260928-061250-text`, load 24) met K1's acceptance: `IPC round-trip (same core): avg=4 us, p99=9 us, … (10000 iters)` and `=== Gate 1 Complete ===`, then PCZERO at tick 23000 (a `main` signature). Both hazard greps are empty (no `[tripwire` lines yet).
- K2: **CPUs 1–3 take no timer IRQs on this host.** Every K2 line shows `tick=N,0,0,0`. Two checks independent of the counter confirm it:
  - A QEMU boot of the K2 image with `-d int` logged 1069 IRQ exceptions, all on CPU 0 and none on CPUs 1–3 after they unmasked IRQs in `enter_scheduler`.
  - The QEMU monitor (`xp`) at heartbeat 2000 shows the cause. `GICR_IGROUPR0` is `0xffffffff` on CPU 0 and `0x00000000` on CPUs 1–3. `GICR_ISENABLER0` and `GICR_ISPENDR0` have bit 30 set on all four. edk2 set up only CPU 0's redistributor, and `init_gicv3_secondary` (`gic.rs:112-154`) enables PPI 30 but never sets its group. PPI 30 therefore stays in Group 0, which the kernel never enables (it sets only `ICC_IGRPEN1_EL1` and `GICD_CTLR.EnableGrp1NS`), so the timer interrupt stays pending on CPUs 1–3 for the whole boot.
  - Host: QEMU 11.1.1 with Homebrew's `edk2-aarch64-code.fd`. CI's Ubuntu QEMU and edk2 are not checked.
  - Not fixed (detect-only). What it means for the rest of the plan:
    - K2's acceptance "all four `tick` values > 0" cannot hold here (see Decisions Made).
    - On this host, CPUs 1–3 never run `timer_tick_handler`, `check_timeouts`, the balancer or `check_preemption` from IRQ context, and the timer never preempts their threads. They switch only by yielding or blocking. The physical-alias IRQ path (§2.11) and IRQ-context switches do not occur on them, so K3's "`irqsw` > 0 on all CPUs" and K7's "`pcphys` > 0 on CPUs 1–3" will fail here too.
    - K8's two-strike gate needs every online CPU's `tick` to advance by ≥ 100, so it would never confirm a flag while CPUs 1–3 stay at 0. K8 needs a decision (for example, gate on the CPUs whose `tick` advances) before it is written.
    - The ADR hypotheses that need IRQs on CPUs 1–3 cannot fire on this host. Check whether the ADR's evidence came from a QEMU or firmware that groups the secondaries' PPIs.
- K2: print cost (CNTVCT ticks), from the `twc` deltas of the two boots below:
  - A 95-byte `hb` line costs a median of 29,000 (text) and 28,125 (gpu), about 0.46 ms or 4.9 µs per byte, as §4.3's model predicts. The minimum is 22,750 (text) and 21,313 (gpu).
  - The first `hb` line (`t=1`, 76 bytes, right after the boot's log drain) costs 129,562 (text) and 118,312 (gpu).
  - The `g1` `Full` line (922 bytes) costs 210,312 (text) and 172,562 (gpu), which is 2.8–3.4 ms.
  - **K8's bound `twmax` < 187,500 would fail on the text boot because of the `g1` line**, since `twmax` is the maximum over all lines. K8's bound should apply to `hb` lines, or K8 should record the `g1` cost separately.
- K2: boots. Text `target/soak/20260928-063511-text` (load 15) and gpu `target/soak/20260928-064257-gpu` (load 5) are both CLEAN. Each has 46 heartbeats, and each heartbeat is followed by exactly one `src=hb` line before the next. Each has one `src=g1` line after `=== Gate 1 Complete ===` (text 4 lines later, gpu on the next line), `hbdefer=0` on every line, no `src=hb` line inside the bench block, and all 47 lines complete (token count equals `n`). The hazard grep is empty. IPC round-trip avg is 5 us in both. The gpu log has a `drain_logs` line splitting a bench line (`[bench] Shared[   6.774872] [0] INFO  Mm …`). `CONSOLE_BUSY` holds back only tripwire lines, so that interleave is `main`'s.
- K2: `llvm-objdump` of the K2 ELF finds no V-register, NEON, `memcpy`/`memset` or `blr` site in `print_line`, `bump_masked`, `end_of_tick`, `write_line::<UartSink, …>`, `print_line::{closure#0}` or `timer_tick_handler`.
- K3: in the K3 ELF, LLVM drops every store to `CURRENT_TID`, `LAST_RUN`, `WAKE_PENDING` and `WAKE_AT`. Nothing reads them yet, so they are write-only globals; `note_dispatch` keeps only a discarded `ldr xzr` of `TICK_COUNT`. The stores come back when K5, K8 and K9 add readers, so V1's per-symbol counts for `note_dispatch`, `note_repick` and `reset_thread_stamps` will grow then. `SWITCH_GEN`, `IRQ_CTX` and `LAST_CPU` are already read, and their stores are in the ELF.
- K3: `llvm-objdump` of the K3 ELF finds no V-register, NEON, `memcpy`/`memset` or `blr` site in `note_dispatch`, `note_repick`, `irq_enter`, `irq_preempt_check`, `irq_leave`, `irq_ctx`, `set_irq_ctx`, `schedule`, `enter_scheduler`, `try_direct_switch`, `try_reply_switch` or `irq_handler_el1`. `irq_handler_el1` calls `irq_enter` before the IAR read and tail-calls `irq_leave` on both returns.
- K3: boots (four, because two failed early; the hazard grep is empty in all four):
  - text `target/soak/20260928-073606-text` (load 9): PCZERO (`EXCEPTION[CPU 0]`, ELR=FAR=0) right after `Bench main: server ready, starting IPC benchmark`, before the first round-trip line and before heartbeat 1000, so only the `t=1` line printed. PCZERO is a `main` class, but every run-167 `main` PCZERO came after `g1`. `main` ran the first bench call with IRQs masked until K1, so a PCZERO inside the bench may be K1 exposing the same IRQ-path switch there; one boot cannot tell.
  - text `target/soak/20260928-073759-text` (load 15): CLEAN. `g1` at `t=594`: `irqsw=5,0,0,0`, and `irqsw0`, `nest`, `n4` and `insched` are 0 on every CPU; `xdir`, `xrep`, `xnever`, `n1` and `badtid` are 0. The last `hb` line (`t=45001`) has `irqsw=2391,0,0,0`. 46 heartbeats, 46 `src=hb` lines; IPC round-trip avg 8 us.
  - gpu `target/soak/20260928-073931-gpu` (load 12): WEDGE, "heartbeat alive but the Gate 1 bench never completed" (a `main` signature: run 167 `main-gpu-r1` run 05). The bench stalled after `server ready`, with no round-trip line. CPU 0 kept committing IRQ-path switches through the hang (`irqsw` 38 at `t=1257`, 2475 at `t=44257`, about 55 per second). `hbdefer` equals the line count: the hung bench never clears `CONSOLE_BUSY`, so every `hb` line waits the full 256 ticks (K2's cap, as designed).
  - gpu `target/soak/20260928-074112-gpu` (load 14): PCZERO at heartbeat 3000, after `=== Gate 1 Complete ===` (a `main` signature: run 167 `main-gpu-r2` run 02, at 4000). `g1` at `t=628`: `irqsw=5,0,0,0`, the other K3 keys 0.
  - `xdir`, `xrep` and `xnever` stay 0: the bench's direct and reply switches are same-core, and a direct or reply target is always BlockedIpc, so it has been dispatched before.
- K3 (review 1): boots after the `n1` origin fix (three, because the first two failed; the hazard grep is empty in all three):
  - text `target/soak/20260928-075548-text` (load 11): PCZERO at heartbeat 6000, after `g1`, the `main` signature (`EXCEPTION[CPU 0]`, EC=0x21, ELR=FAR=0). `g1` at `t=580`: `irqsw=6,0,0,0`, the other K3 keys 0.
  - gpu `target/soak/20260928-075715-gpu` (load 10): PCZERO at heartbeat 1000, after `g1`, same signature. `g1` at `t=637`: `irqsw=12,0,0,0` and `irqsw0=1,0,0,0`: one IRQ-path switch on CPU 0 had no current thread to save. The other K3 keys are 0.
  - text `target/soak/20260928-075841-text` (load 12): CLEAN. `g1` at `t=593`: `irqsw=4,0,0,0`, the other K3 keys 0. The last `hb` line (`t=45001`) has `irqsw=2096,0,0,0`. 46 heartbeats, 46 `src=hb` lines; IPC round-trip avg 5 us.
- K4: **`elrmm` = `spsrmm` = 1 on CPU 0 in every boot that ran past `t=1001`** (5 of 5). Each is 0 at `g1` (`t` ≈ 600), 1 by `t=1001`, and never rises after, while `irqsw` reaches 1600–1985. `nest` is 0, and only the unreached lower-EL stubs write ELR_EL1/SPSR_EL1. So once per boot, an IRQ return on CPU 0 erets with another stream's ELR and SPSR: H1's mechanism, seen directly.
  - One reading fits the counts but is unproven. After Gate 1, CPU 0 holds three Interactive threads (`bench.rs:490-545`). The balancer moves only Normal threads, so all three stay on CPU 0.
    - bench-main sits in `loop { wfe }` (`bench.rs:455`).
    - bench-server reaches its own `loop { wfe }` (`:196`) at most one 100-tick `ipc_recv` timeout after `BENCH_SERVER_EXIT`, which is stored right after `request_g1_line`.
    - bench-yield starts each turn with a fresh 10 ms slice and yields within microseconds. So it is never switched out on the IRQ path, and it always resumes at thread level, never through `irq_frame_check` after a switch.
    - So bench-main and bench-server are the only pair that resume through the stub after a switch. In the K4 ELF both loops are `wfe; b`, at different addresses (`bench_main_entry+0x810`, `bench_server_entry+0x98`), and neither reads a register or the flags. The first IRQ-path resume of one after the other took an IRQ erets it into the other's loop, and nothing breaks. From then on both threads share a PC and an SPSR, so no later mismatch shows. That fits 0 at `g1` and 1 by `t=1001`.
    - The idle thread is not in the pair. `RunQueue::pick_next` (`sched/mod.rs:65-76`) takes the Idle class last, and after the server exits none of the three bench threads ever blocks, so CPU 0 never picks its idle thread. (The idle loop, `sched/init.rs:125-139`, also loads `NEED_RESCHED` and calls `thread_yield`, so it is not register-free.)
  - For D1 and the owner: §5's rule "H1 refuted if `elrmm` = 0 in all 30 boots" cannot be met on this kernel. `main` (`b07d7e4`) has the same bench tail and the same GPR-only stub restore, so the same swap happens there, but only the 1b arm counts it. The restated rule could count only mismatches before `g1`, or require `elrmm` > 1. Either way it should leave out boots whose first fatal report is a PANIC, as the ADR does for later symptoms (N8). The K4 (review 1) gpu PANIC boot below has a mismatch before `g1`.
- K4: boots. The hazard grep is empty in all of them.
  - gpu `target/soak/20260928-081501-gpu` (load 7): CLEAN. 46 heartbeats, 46 `src=hb` lines, and all 47 lines complete. `g1` has `elrmm=0,0,0,0 spsrmm=0,0,0,0`.
  - text, 8 boots: 3 CLEAN, 4 WEDGE ("heartbeat stuck at tick 0 after the Gate 1 bench started"), and 1 PCZERO at tick 16000, after `g1`. The runs are `target/soak/20260928-081329-text`, `-081709-text`, `-081903-text` and `-082121-text` (3 runs), plus 2 in the session scratchpad.
  - The first three text boots were WEDGEs in a row. So 3 boots of the K3 head (`c619738`, built from `git archive` in the scratchpad) were interleaved with the last 2 K4 boots. K3 gave CLEAN, PCZERO (tick 9000) and CLEAN; K4 gave CLEAN and CLEAN.
  - Tick-0 WEDGE is a `main` signature (5–6 of 20 in the #167 and #168 `main` soaks). With these counts, K4's rate cannot be told apart from `main`'s; the A/B soak will decide.
- K4: `llvm-objdump` of the K4 ELF: the stub is §2.8's sequence. `irq_frame_check` is `mrs`, `cmp`, and `bl`/`b` to `bump_masked`, and saves only x19 and x30. There is no V-register, NEON, `memcpy`/`memset` or `blr` site.
- K4 (review 1): boots of the unchanged K4 ELF (sha256 `590fbe383bd033fa`, the same as the K4 boots), host load 8–32. The hazard grep is empty in all four. Every class is a `main` class.
  - text `target/soak/20260928-085340-text` (load 8): PCZERO at tick 40000, after `g1`. `g1` at `t=592` has `irqsw=5,0,0,0`, `elrmm=0,0,0,0` and `spsrmm=0,0,0,0`. `elrmm` = `spsrmm` = 1 on CPU 0 from `t=1001` to the last line (`t=40001`, `irqsw=1559`). This is a sixth boot that fits the K4 pattern.
  - gpu `target/soak/20260928-085516-gpu` (load 32): PANIC, `main`'s `frame.rs:51` before the bench header. The bench then stalled after `server ready`, so there is no `g1` line. The `ELR=FAR=0x2_0000_0000` exception at tick 27000 comes after the PANIC, as in K1 (review). The `hb` lines have `elrmm=1,0,0,0` from `t=1257`, but `spsrmm` stays 0, and `irqsw0=1,0,0,0`. This mismatch comes before any `g1` and has no SPSR mismatch, so the bench-main/bench-server reading does not cover it. It is PANIC-boot fallout by the ADR's rule. It is recorded because a "before `g1`" H1 rule would count it.
  - gpu `target/soak/20260928-085739-gpu` (load 21): WEDGE, heartbeat stuck at tick 0 after the Gate 1 bench started.
  - gpu `target/soak/20260928-085904-gpu` (load 21): EXCEPTION at tick 0, `EC=0x25 ESR=0x96000006 FAR=0x882`. The ELR is `ldrh w11, [x9, #0x2]` in `VirtioGpu::submit_command`'s used-ring poll, with the ring pointer loaded as 0x880. This is the class of the ADR's `main gpu r2 #03`: a data abort at tick 0 from a valid kernel PC through a near-null pointer.
  - No gpu boot in this round reached `g1`. K4's gpu CLEAN boot of the same ELF (`20260928-081501-gpu`) shows `elrmm`/`spsrmm` in its `g1` line.

- K5: **PANIC-LOCK from IRQ context in 9 of 11 text boots, all at or just after the bench start.** Every one is `ctx=irq-exit holder_irqs=on`, the §5 H3 evidence form. The interrupted thread holds THREAD_TABLE (`process_of_thread`, `cap/mod.rs:39`, reached from the bench's IPC capability checks) or CURRENT_THREAD[0] (`current_thread_id`, `timeout.rs:130`) with IRQs on. CPU 0's timer IRQ then reaches `schedule()`, which blocks on the same lock (`scheduler.rs:193` THREAD_TABLE, `:189` CURRENT_THREAD). An example message: `lock re-entry: CURRENT_THREAD[0] on CPU 0 ctx=irq-exit holder=kernel/src/ipc/timeout.rs:130 holder_irqs=on tid=16 gen=254506`.
  - On `main` the same interleaving spins forever with IRQs masked. That is the tick-0 WEDGE "heartbeat stuck at tick 0 after the Gate 1 bench started" (K4: 4 of 8 text boots).
  - Two seconds later, CPU 3 prints `kind=stuck lock=THREAD_TABLE owner_cpu=0 holder_running=0`. So the holder is CPU 0's stream, frozen in the panic handler.
  - 3 of the 9 print `holder=? tid=?`: the IRQ landed between the holder's CAS and its field stores, the §2.1 residual window. K9's `irq_elr` will name that PC.
- K5: **The first K5 build widened every IRQs-on hold window, and the H3 hit rate follows the window.** QEMU TCG takes a pending IRQ only where a translation block starts, and every branch or call inside a critical section starts one. In the first build (ELF `ac38b5c19f887704`), the holder bookkeeping after the CAS was the out-of-line `note_acquired` (a call, the IRQs-on `read_stamp` loop, a bounds branch). That meant about 11 TB starts inside the CURRENT_THREAD hold of `current_thread_id` (K4: 1) and about 12 inside the THREAD_TABLE hold of `current_process_id` (K4: about 3). That build gave 6 PANIC-LOCK in 7 text boots. The revised build (Decisions Made, ELF `5becf0f733c6460b`) has 3 and about 5, and gave 3 in 4.
  - 9 of 11 against K4's 4 of 8 is not significant at these counts (one-sided Fisher p ≈ 0.17), but it points the way the mechanism predicts.
  - The one extra TB start left, right after the CAS, is where `try_lock_weak`'s `Result` is re-tested (opt-level 1 does not thread it). It is in `shared::lock`'s codegen, not the kernel lock. *(Fixed by K5 review 1: `shared::lock` now returns `Option`.)*
  - **For the owner and B1:** most 1b text boots now end in PANIC-LOCK at the bench start. The B1 N2 baseline will therefore have few text boots that run the IPC phase, and the A/B soak's WEDGE-STUCK + PANIC-LOCK comparison against `main` includes this widening. *(Superseded by K5 review 1, below: the hold windows are back to K4's, and PANIC-LOCK fell to 3 of 11.)*
- K5: `lkoxp[THREAD_TABLE]` reaches about 200,000 by `g1` in both CLEAN boots. It counts mostly the dispatch window, not migrated holders. All 4 commit sites hold THREAD_TABLE across `note_dispatch`'s generation bump, so a waiter on another CPU in that window sees OtherCpuSwitched although the holder is running. B1 and D1 should read `lkoxp` that way.
- K5: gpu boots, one per build. Both stalled after `server ready`; the hung bench never clears `CONSOLE_BUSY`, so `hbdefer` equals the line count. Both then took an EXCEPTION at heartbeat 13000 with a PC in the direct map: `ELR=FAR=0xffff0001440b67ce`, EC=0x22 (misaligned), then `0xffff0001440b68e4`, EC=0x21 (XN). Run 167 has the same pattern: `pr161-gpu-r1/run-05` (a stall after `server ready`, then EC=0x22 at `0xffff0001440b6845`, tick 15000) and `main-gpu-r1/run-04` (EC=0x22 at `0xffff0001440b5f16`). So it is not a new signature.
- K5: the CLEAN text boots, one per build (`target/soak/20260928-093655-text` run 02 and `-094515-text` run 02), meet K5's acceptance:
  - `rsthold=0,0,0,0`;
  - no `lock re-entry:` and no `[tripwire-ev]` line;
  - `lktry[TIMEOUT_QUEUE]=1` in the first (a near-miss);
  - `hbdefer=0`;
  - 46 heartbeats with 46 `src=hb` lines;
  - a `g1` `Full` line with `n=65`;
  - IPC round-trip avg 10 us and 9 us, about twice K4's (K5 (review 2) below).
  In all 13 K5 logs the hazard grep is empty, every `[tripwire]` line is complete, and `rsthold` is 0.
- K5: `llvm-objdump` of the final ELF: the only new V-register or `memset`/`memcpy` site is `bl memset` in `reentry_panic` (its `BufSink`; exempt, halt-terminal). The per-symbol V-site counts equal K4's for `irq_handler_el1`, `timer_tick_handler`, `drain_logs` (3 in both), `sched::timer_tick`, `unblock`, `check_timeouts`, `wake_with_error`, `try_load_balance`, `check_notification_timeouts`, `irq_frame_check`, `end_of_tick`, `note_dispatch` and the switch functions. Every `irq_spin_lock` symbol, `lock_contended::<T>` and `irq_lock_words::<..>` has 0, and there is no `blr`.
- K5: layout. `.text` grows by 25 KiB (the inlined lock paths). `.data` grows by 60 KiB, because the 9 statics moved from `.bss` to `.data`: `HolderFields::tid` starts at `TID_NONE`, and the lock id is non-zero. `__kernel_end` is `0xFFFF_0000_002C_C000` (K4: `…2C_5000`), inside the boot TTBR1's 4 × 2 MiB.
- K5 (review 1): **the reviewer's TB-start count is right, and the fix restores K4's.** A TB start inside a hold is counted as 1 + the branches, calls and returns executed from the CAS's `cbnz` to the releasing `stlr`, on the usual path. `current_thread_id` (`timeout.rs:130`): K4 1, K5 3, now 1. `process_of_thread` (`cap/mod.rs:39`, inlined into its callers in K4): K4 3, K5 5, now 3. The CURRENT_THREAD hold of `current_process_id` (`cap/mod.rs:50`), on the same bench path: K4 1, K5 4, now 1. The lock's own fast path is now `ldaxr; cbnz; stxr; cbnz`, then straight-line code to the release, as `spin::Mutex`'s was.
  - A sweep of every inlined IRQ-class lock site (call sites named by lldb, K4 ELF against this one) finds no other hold that the lock itself widens. Three TIMEOUT_QUEUE holds keep one extra TB start, at `channel.rs:183`, `:281` and `:296`: `process_of_thread` is no longer inlined into `ipc_call`/`ipc_recv`, so its `tid < 64` test no longer proves the index in range, and the `tq[tid]` bounds check stays inside the hold. The IRQ path only try-locks TIMEOUT_QUEUE, so this can raise `lktry` and `check_timeouts` skips, never cause a PANIC-LOCK. `schedule()`'s RUN_QUEUES enqueue (`scheduler.rs:202`) has 4 against 3, with IRQs masked. Both go in the PR's §4.5 list.
  - The cost the reviewer named is real: an IRQ at the one TB start after the CAS runs before the holder-field stores, so a short hold's PANIC-LOCK prints `holder=? tid=?` (`target/soak/20260928-134816-text` run 01). K9's `irq_elr` gives the PC.
- K5 (review 1): boots of ELF `128cbb0e` (load 13–55 from another session): 11 text boots gave 3 PANIC, 4 PCZERO, 2 WEDGE, 1 EXCEPTION and 1 CLEAN; 1 gpu boot gave a WEDGE.
  - All 3 PANICs are the H3 PANIC-LOCK at the bench start, `ctx=irq-exit holder_irqs=on`: THREAD_TABLE with `holder=kernel/src/cap/mod.rs:39` twice, CURRENT_THREAD[0] with `holder=?` once. That is 3 of 11, against K5's 9 of 11 (one-sided Fisher p ≈ 0.015) and K4's 4 of 8 tick-0 WEDGEs.
  - The 4 PCZEROs come after the bench start. Each boot that ran past `t=1001` shows `elrmm=spsrmm=1,0,0,0`, K4's pattern. K5's PANIC-LOCKs had censored this class.
  - The EXCEPTION (`20260928-135549-text` run 04) is `EC=0x25 ESR=0x96000006 FAR=0x5a8`, a read at bench start from `ipc_reply+444`: REPLY_SLOTS's `spin::Mutex` spin (`channel.rs:376`), with the lock pointer register holding 0x5a8. REPLY_SLOTS is not an IRQ-class lock and no K5 code runs there. It has the shape of run 167's `main` gpu r2 #03 (`EC=0x25`, `FAR=0xa`, a valid kernel PC, tick 0), so it is not a new class. It is the first text-mode one on this branch; fewer PANIC-LOCKs at the bench start leave more boots to reach it.
  - The 2 text WEDGEs and the gpu WEDGE have heartbeats alive and the bench stalled after `server ready`, with `hbdefer` equal to the line count: the known heartbeat-alive hang.
  - The CLEAN boot (`20260928-140136-text` run 03) meets K5's acceptance: no `lock re-entry:` and no `[tripwire-ev]` line, a `g1` `Full` line with `n=65` and `rsthold=0,0,0,0`, `lktry[TIMEOUT_QUEUE]=2`, `hbdefer=0`, and 53 heartbeats with 53 `src=hb` lines. IPC avg 19 us at load 13 (K5 (review 2) below).
  - In all 12 logs the hazard grep is empty, every `[tripwire]` line is complete, and `rsthold` is 0.
- K5 (review 1): `llvm-objdump` of the new ELF: every symbol's V-register, `memset`/`memcpy` and `blr` count equals the K5 ELF's (the only site on the lock path is still `reentry_panic`'s `bl memset`). `.text` is 26.8 KiB above K4 (K5: 25 KiB), and `__kernel_end` is `0xFFFF_0000_002C_D000`, inside the boot TTBR1's 4 × 2 MiB.
- K5 (review 2): **the K5 lock about doubles the Gate 1 IPC round trip, and Gate 1's `IPC < 10 us` now fails in most boots.** The K5 entries above gave IPC figures with no baseline. From every soak log on this host that has a round-trip line (avg and min in us; load is the boot's 1-minute average):
  - `main`, run 167 (`f0b4169`): 16 boots (15 text, 1 gpu), avg 3–6, min 2.0–4.0, all PASS, load 6.5–24.
  - K1–K4 (ELFs `e3822af6` to `590fbe38`): 13 boots (9 text, 4 gpu), avg 4–10 (4–7 at load 5–12), min 2.0–5.0, 12 PASS. The one FAIL is K1's avg 10 at load 33.
  - K5 (ELFs `ac38b5c1`, `5becf0f7`, `128cbb0e`): 9 text boots (the 6 above, this round's gate boot, and 2 from `20260928-145827-text`), avg 9–13 with one 19 at load 12, min 6.0–11.0, 1 PASS (avg 9). No K5 gpu boot reached the round trip.
  - The minimum is the steadiest signal, because load inflates avg and p99 far more than the best round trip. Every K5 minimum but one is 6,992 ns or more; every `main` and K1–K4 minimum is 4,992 ns or less.
  - The context switch moves the same way: avg 1–4 us and min 1.0–2.0 us before K5, avg 4–8 us and min 3.0–5.0 us with it. Its 20 us Gate 1 limit still passes.
- K5 (review 2): **where the cost comes from, measured.** Four kernels, 8 rounds, one short text boot per kernel per round, interleaved (`secs=30`, load 7–16). The kernels are scratch builds of `git archive` trees in the session scratchpad, not committed:
  - `head`: this commit (its `.text` is byte-identical to `128cbb0e`'s);
  - `k4`: K4 (`43fc8d5`);
  - `g`: `head` with `lock_contended` cut to spin-and-retry (no classify, snapshot, `bump` or event line), stamps unchanged;
  - `fg`: `g` with a constant never-current stamp and `TID_NONE` from an inlined `whoami`, and no re-stamp in `hold`. Its fast path is K4's plus the two holder-field stores after the CAS, their two clears before the release, and a 64-bit word.
  - Results (IPC avg; IPC min; Gate 1 IPC; context-switch avg):
    - `head`, 5 figures: 10, 10, 13, 13, 13; 7.0–11.0; 0 PASS; 4–8.
    - `g`, 4 figures: 11, 11, 14, 15; 6.0–11.0; 0 PASS; 4–8.
    - `fg`, 4 figures: 5, 6, 6, 8; 3.0–4.0; 4 PASS; 2–9.
    - `k4`, 5 figures: 4, 5, 6, 6, 7; 3.0–5.0; 5 PASS; 1–4.
  - `g` matches `head` and `fg` matches `k4`. Nearly the whole cost is the stamp reads on the fast path:
    - `whoami`: a call, 1 DAIF and 2 MPIDR reads, 2 `SWITCH_GEN` loads, a `CURRENT_TID` load and about 7 branches with the `ret`;
    - the re-stamp inside the hold (`current_stamp_or`): 2 MPIDR reads, 1 load and 1 `stlr`.
  - The contended-path bookkeeping and the holder fields cost nothing these boots resolve, although `lkoxp[THREAD_TABLE]` reaches 41,000–250,000 by `g1`. Not measured: why a few system-register reads per `lock()` cost microseconds per round trip under TCG, and how many IRQ-class `lock()` calls one round trip makes.
  - The other 14 boots stopped before the round trip, each in a way its kernel shows elsewhere: tick-0 WEDGE in `k4`, `g` and `fg` (their contended path spins where `head` panics), 2 PANIC-LOCK in `head`, the EC=0x25 near-null abort at bench start in `head`, `g` and `fg`, and one heartbeat-alive bench stall in `fg`. Classes from 30-second boots say nothing about rates.
- K5 (review 2): **a PC equal to an owner stamp.** The first attempt at the measurement above, discarded because its `fg` build had a bug, included a `head` boot that stalled after `server ready` with its heartbeat alive, then took `EXCEPTION[CPU 0]: ESR=0x8a000000 EC=0x22 FAR=ELR=0xc000000000034f51` at tick 9000 (load 11), with no earlier PANIC. That value decodes as an `IrqSpinLock` word: HELD, IRQS_ON, CPU 0, generation 216,913, in the range of CPU 0's generation at the bench (the re-entry messages show 159,928–264,124). So a branch or return most likely took a lock word as its target. The 1b lock words are the only data known to hold values of this shape, so the signature is new in value. It belongs to the family of EC=0x22 jumps to data values after a bench stall (the direct-map PCs in K5's gpu boots and run 167's gpu logs). **The log was deleted with that attempt's output directory, so only the classifier's line above survives.** Eight more `head` text boots (`20260928-145827-text`, `secs=40`) did not repeat it: 3 PANIC-LOCK, 2 EC=0x25 aborts (`ipc_recv`, FAR 0x12 and 0x679), 2 CLEAN and 1 heartbeat-alive stall. By K1 (review)'s rule for `0x2_0000_0000`, a new signature in a boot with no earlier PANIC must be explained before 1b merges.
- K5 (review 2): the survivor data point the same way as the IPC figures, but load confounds them. Of the 7 text boots of the current ELF that got past tick 0 in 75-second soaks (K5 review 1 and this round's gate boot), 2 are CLEAN, 3 are PCZERO after `g1` and 2 are heartbeat-alive bench stalls. `main` run 167 has 12 CLEAN of 15 (one-sided Fisher p ≈ 0.03; 0.014 for review 1's 1 of 6), but at load 6.5–24 against K5's 10–55. None of run 167's 20 text boots is a heartbeat-alive stall; K5 has 2 in 12 such boots.
- K5 (review 2): gates. `just check` clean; `just test` 624 passed. Text boot `20260928-145402-text` (ELF `128cbb0e`, load 13): CLEAN, 54 heartbeats with 54 `src=hb` lines, `g1` with `n=65`, `rsthold=0,0,0,0` and `hbdefer=0`, no `lock re-entry:` and no `[tripwire-ev]` line, IPC avg 12 us and min 9.0 us (Gate 1 IPC FAIL), `elrmm=1,0,0,0` after `g1` (K4's pattern). Gpu boot `20260928-145534-gpu` (load 11): `main`'s `frame.rs:51` PANIC after tick 0, the bench stalled after `server ready`, and the post-panic `ELR=FAR=0x2_0000_0000` exception at tick 44000 (K1 (review)); 45 heartbeats with 45 `src=hb` lines, every line after the first held back the full 256 ticks (`hbdefer=44`). The hazard grep is empty and every `[tripwire]` line is complete in both.
- K5 (review 3, recorded in K5b): **run-08 (`target/soak/20260928-145827-text/run-08.log`, ELF `128cbb0e`, load 18) is a TIMEOUT_QUEUE holder that never released.** The classifier calls it WEDGE ("heartbeat alive but the Gate 1 bench never completed"): the bench stalled after `server ready`, with no round-trip line, no `g1` line and 25 heartbeats.
  - `lktph[TIMEOUT_QUEUE]` is 682 at `t=1257` (log line 196) and rises by exactly 1000 per heartbeat, to 23,682 at `t=24257`. So from about tick 576, every CPU 0 tick's `check_timeouts` try-lock failed against a CPU-0 stamp of an ended generation (PreemptedHolder), and no IPC timeout fired on CPU 0 again.
  - Log lines 198-199, before the `t=2257` line: `[tripwire-ev] kind=stuck cpu=0 lock=TIMEOUT_QUEUE idx=- ctx=thread owner_cpu=0 owner_gen=272869 holder_tid=? cur_tid=16 holder_running=? holder=?`, and the same with `cur_tid=18`: two thread-context waiters on CPU 0 spinning in `lock()`, bench server (16) and bench main (18; it created channel 9), which `bench.rs` allocates in that order with bench yield between them. `lkstk[TIMEOUT_QUEUE]`=2 from then on, and `lkpho[TIMEOUT_QUEUE]` is already 1 at `t=1257`. The word never changes, so the snapshot is consistent and the `?` fields are really empty: the holder never stored its fields. That fits a holder stopped at the one translation-block start right after its CAS (K5 review 1).
  - `elrmm` climbs 15 -> 826 while `irqsw` climbs 65 -> 1557: 811 of CPU 0's 1492 IRQ-path switches in that span returned with a changed ELR, where K4's pattern is one per boot. `spsrmm` goes 1 -> 2.
  - The 2026-09-29 deep-dive reading (inferred, not reproduced): H1 resumed the lock holder at the other bench thread's PC, so it ran that thread's code and never reached its field stores or its release. After that, two streams trade PCs on every IRQ-path switch, which fits the `elrmm` climb.
  - None of K6-K8's scans would have named it. The holder is Running or queued Runnable, never Blocked, so it is neither an orphan nor a no-waker thread, and it keeps running, so it does not starve. K7's restore-site check sees a valid kernel-text PC. Only the lock counters (`lktph`, `lkstk`, the `stuck` lines) and `elrmm` show it. Owner decision 2026-09-29: no extra H1 tripwires in 1b.
- K5b: parity with K5 (and so with K4's in-hold translation-block counts, K5 review 1), from `llvm-objdump -d` of the K5 ELF (`128cbb0e`) and the K5b ELF (`b1bf5edf`), compared per function after `TPIDR_EL1` is treated as `MPIDR_EL1` and addresses and `.llvm.*` suffixes are normalised. 701 of 707 common functions have the same mnemonics in the same order and the same branch operands. The rest differ only in data offsets and in key-index or slot-count immediates (the new key). The 6 that differ structurally are the ones K5b edits: `_start` and `_secondary_entry` (+3 each, the TPIDR write), `kernel_main` (+1, the `check_tpidr` call), `secondary_main` (+2), `note_dispatch` (+20, the cross-check) and `bench_main_entry` (+8, the guard). The hold paths in `current_thread_id`, `process_of_thread` and `current_process_id` differ from K5 only in the two `mrs` operands. No V-register, `memset`/`memcpy` or `blr` site in `check_tpidr`, `note_dispatch`, `whoami` or `secondary_main`. The layout is unchanged: `__kernel_end` is still `0xFFFF_0000_002C_D000`.
- K5b: boots, ELF `b1bf5edf`, host load 50-157 (other sessions' work). The hazard grep is empty and every `[tripwire]` line is complete in all 5. `tpidrbad` is 0 on every CPU in every line: 0 in both `g1` `Full` lines, and absent (0) from every `NonZero` line, including each boot's `t=1` line, which prints after `kernel_main`'s and the secondaries' checks.
  - Gate text `20260929-190618-text` (load 157): PANIC-LOCK at the bench start, `lock re-entry: CURRENT_THREAD[0] on CPU 0 ctx=irq-exit holder=? holder_irqs=on tid=? gen=244123` (the K5 H3 form).
  - Gate gpu `20260929-190754-gpu` (load 144): `main`'s `frame.rs:51` PANIC after tick 0, the bench stalled after `server ready`, then the post-panic `ELR=FAR=0x2_0000_0000` exception after heartbeat 2000 (K1 (review)).
  - Three extra text boots, `20260929-190926-text` (`secs=40`), to reach the round trip. Run 01 (load 91): PANIC-LOCK `THREAD_TABLE ... holder=kernel/src/cap/mod.rs:39 holder_irqs=on`, and 2 s later CPU 3's `kind=stuck` line (K5's pattern). Runs 02 and 03 CLEAN: `g1` with `n=66`, `rsthold=0,0,0,0`, `tpidrbad=0,0,0,0` and `hbdefer=0`; 25 heartbeats and 25 `src=hb` lines; `elrmm=spsrmm=1,0,0,0` after `g1` (K4's pattern).
  - **IPC, next to K5's:** run 02 (load 63) avg 9 us, min 4,992 ns, Gate 1 IPC PASS; run 03 (load 50) avg 12 us, min 8,000 ns, FAIL. Context switch avg 4 and 6 us. K5 gave avg 9-13 us, min 6.0-11.0 us, at load 10-55 (K5 (review 2)). Run 02's minimum is in the K1-K4 range (4,992 ns or less), and no K5 boot had one. Two boots at load 50-63 cannot measure the gain; the paired experiment (`target/dig/a2/report/`, T about 2.8 us below K5) is the evidence.
  - No new class: PANIC-LOCK at the bench start, `frame.rs:51` with its post-panic exception, and CLEAN are all earlier K5 or `main` classes.
- K5b (review 1): boots of the comment-only fix (same code as `6df832d`, host load 22-41), none past tick 0: text `20260929-200735-text` and `20260929-201140-text` are PCZERO (`EXCEPTION[CPU 0]`, EC=0x21, ELR=FAR=0) right after `server ready`, the `main` class K3 met (`20260928-073606-text`); gpu `20260929-200928-gpu` is `frame.rs:51` at tick 0 with its post-panic exception, as in the K5b gate. The hazard grep is empty, and `tpidrbad` is absent (0) from each boot's `t=1` line. No new class.

- K6: boot deferred. Host load average was 408 at the gate (rule: above 30, do not boot), from other sessions' work. Still owed in a quiet window: one text and one gpu boot with K6's acceptance (`ubrun`/`ubrbl` present; `n2` and `misrep` 0 in CLEAN boots, any non-zero value recorded; classes not new; the hazard grep empty) and the per-site `badchan` self-test baseline (cap and slot from the ipc tests, select from the select_cap tests), recorded here.
- K6: V-register check of the K6 ELF (`llvm-objdump -d -l`, sysroot `nightly-2026-10-06`): no V-register, `memcpy`/`memset` or `blr` site in any new tripwire function (`note_unblock`, `note_unblock_target`, `mark_wake_pending`, `clear_wake_pending`, `note_reply_switch`, `note_reply_wake`, `note_send_wake`, `wait_*`) or in `unblock`, `clear_timeout`, `wake_with_error`, `check_timeouts`, `check_notification_timeouts`, `try_wake_select`, `try_load_balance` and the switch functions. Every V site in the changed files is on a line K6 did not touch (`RawMessage` builds and ring push/pop in `channel.rs`, `notification_destroy`'s `take()` at `notify.rs:282`, `allocate_thread`'s slot fill). V1 still runs the full differential gate.
- K7: boot deferred. The host load average was 135 at the start and 91 at the gate (rule: above 30, do not boot), from other sessions' work. Still owed in a quiet window: one text and one gpu boot with K7's acceptance (`pcother`, `pcnull` and `spbad` 0 in CLEAN boots; `pcphys` 0, since CPUs 1-3 take no timer IRQs (#200); classes not new; the hazard grep empty).
- K7: `llvm-objdump` of the K7 ELF (sysroot `nightly-2026-10-06`): no V-register, `memcpy`/`memset` or `blr` site in `check_restore`, `enter_scheduler`, `schedule`, `try_direct_switch` or `try_reply_switch`. `check_restore` loads `sp` and `pc` with one GPR `ldp`. Its raw-pointer read carries the dev build's null and alignment checks (`panic_null_pointer_dereference`, `panic_misaligned_pointer_dereference`); every caller passes `&thread.context` from THREAD_TABLE, so they cannot fire, and `assert_valid_ctx`'s reads of the same fields already have them. LLVM tail-merges `schedule()`'s two restore paths into one `check_restore` / `assert_valid_ctx` / `note_restore` / `restore_context` sequence. `kernel_main` stores `TEXT_LO` = `0xFFFF_0000_0008_0000` and `TEXT_HI` = `0xFFFF_0000_000D_A000`, both VAs.
- K8: gates. `just check` has 0 warnings and `just test` passes 678 (one new `TwoStrike` test). `cargo miri test -p shared --lib two_strike` passes 8, and host clippy on the shared tests reports nothing in `tripwire.rs`. The branch's own `aios docs-check` (`AIOS_TOOLS_BIN`) shows the same 4 new findings as K7, all D1's (the developer-guide test count now reads 612 against 678). Boot deferred: load 73 at the start and 201–215 at the gate (rule: above 30, do not boot). The lead's deferred boot should check K8's acceptance with K2's caveat: the `twmax` < 187,500 bound applies to `hb` lines, since the `g1` `Full` line alone costs about 210,000.
- K8: `llvm-objdump` of the K8 ELF finds no V-register, NEON, `memcpy`/`memset`/`memmove`, `blr` or panic call in `run_scans`, `scan_a`, `scan_b`, `confirm`, `scan_note_queued`, `scan_note_currents`, `scan_note_slot`, `scan_note_waker`, `note_scan_hold2`, `scan_lock_busy`, `held_by_own_stream`, `sched::scan_snapshot`/`scan_queue_level`/`scan_thread_table`, the four `ipc::scan` functions, `end_of_tick` or `TwoStrike<8>::scan`. The first build had `panic_const_add_overflow` calls in `scan_a`, `scan_thread_table`, `scan_deadlines` and `TwoStrike::scan`: `enumerate()`'s counter is overflow-checked in the dev build. They are now `0..n` ranges with `get`, and the calls are gone. `try_load_balance` keeps `main`'s own overflow and bounds-check calls; K8 changes only its `saturating_add`.
- K9: gates. `just check` has 0 warnings and `just test` passes 678. The branch's own `aios docs-check` (`AIOS_TOOLS_BIN`) shows the same 4 new findings, all D1's. Boot deferred: load 28 at the start and 78 at the gate (rule: above 30, do not boot). Owed in a quiet window: one text and one gpu boot; K9's acceptance is "CLEAN boots unchanged" plus the hazard grep. A gpu boot that hits `main`'s bad-free PANIC should now print `[mm] BUG: free_pages(` at the caller of `free_dma_pages`/`release_buffer`, not `frame.rs:51`, followed by `[panic] cpu=… ctx=thread…` and a `src=panic` line.
- K9: `llvm-objdump` of the K9 ELF: no V-register site in `drain_logs` (the tick's path; its body is the new private `drain_rings`, inlined), `print_panic_report`, `print_exception_ctx` (`in_scheduler` inlined) or `sync_exception_handler`. The sync vector slot is the 13 instructions §2.7 names, with `add x3, sp, #0x30` and `mrs x4, SPSR_EL1` before the `bl`.
- K10: gates. `just check` has 0 warnings and `just test` passes 678. The branch's own `aios docs-check` (`AIOS_TOOLS_BIN`) shows the same 4 new findings, all D1's. Boots at load 21 (text, `20261009-212720-text`) and 24 (gpu, `20261009-212849-gpu`); the hazard grep is empty in both. Both print the four `[smp]` lines: CPUs 1–3 `vbar=0x0000000040081000` (CPU 0's `0xffff000000081000` minus `VIRT_PHYS_OFFSET`, the LMA of `.text.rvectors`) `ttbr0=0x00000000402ad000 vbar_kva=0 ttbr0_idmap=1`, and CPU 0 `vbar=0xffff000000081000 ttbr0=0x000200004400b000 vbar_kva=1 ttbr0_idmap=0` (ASID 2, address space B, matching `TTBR0 switch: ASID 1 -> ASID 2`). Text is PCZERO after heartbeat 6000 (EC=0x21, ELR=FAR=0), `main`'s class. Gpu is EXCEPTION at the bench start after `server ready` (EC=0x25, FAR=0x419, ELR in `check_channel_access`'s PROCESS_TABLE spin, `ldrb w10, [x8, #0x418]` with x8=1) with `elrmm=spsrmm=1,0,0,0` and `irqsw=2`: a corrupted register across an IRQ-path switch, the H1 family. No new class; K10's lines print at boot only, long before either.
- K10 review fixes: the §6.5 "SMP lines" text gave execution order, but the serial log shows the direct `[smp]` lines above the earlier ring-buffered `Core N online` / `4 CPUs online` / `TTBR0 switch` entries (`20261009-212720-text` lines 50-65). Added a log-order note to §6.5 and the K10 Decisions bullet. The reviewer's "correlate by the bracketed timestamp" wording was not used because `[smp]` lines have no timestamp; the note says to match by content. Docs only, so no boot; `just check` 0 warnings, `just test` 678 passed.
- T-self: gates. `just check` has 0 warnings and `just test` passes 678; `cargo build` and `cargo clippy -D warnings` with `--features tripwire-selftest` are clean too. Feature boot at load 13 (`target/soak/20261009-220509-text`; ELF and log copied to `target/dig/tself/`), with the ESP assembled by hand as `justfile`'s `disk` recipe does and `just soak runs=1 secs=40 --no-build report_only=1`. Every L5 item holds: `PANIC: panicked at kernel/src/sched/scheduler.rs:196:38:` (`schedule()`'s `THREAD_TABLE.lock()`, §3's `:168` before the drift), next line `lock re-entry: THREAD_TABLE on CPU 0 ctx=irq-exit holder=kernel/src/sync/selftest.rs:94 holder_irqs=on tid=19 gen=96512`, then `[panic] cpu=0 tid=19 ctx=irq-exit irq_was=off t=6.142418 irq_elr=0xffff000000087c84`, the `isb` of `hold_loop`'s spin (`0xffff000000087c6c`–`0x…c98`), and `src=panic` with `lktry=1,0,…` (THREAD_TABLE). The summary classifies PANIC with both lines in `first_fatal`. One `[tripwire-ev] kind=stuck cpu=3 lock=THREAD_TABLE ctx=thread-off … holder_tid=19 holder_running=0` follows: CPU 3 waits on the lock the halted CPU 0 still holds, the expected post-panic fallout. The hazard grep is empty.
- T-self: the first feature boot (`20261009-220356-text`) stayed CLEAN with no `[selftest]` line: the thread was Normal, pinned to CPU 0, and starved there (`starved=4,10,0,0`), because CPU 0 picks classes in strict priority and the Interactive bench main spins on it for good after Gate 1. Made the thread Interactive, as the bench threads are (Decisions Made, T-self).
- T-self: default-build boots at load 10 and 9. Text (`20261009-220609-text`): PCZERO after heartbeat 45000 (EC=0x21, ELR=FAR=0), `main`'s class, no `selftest` line, `src=g1` present. Gpu (`20261009-220731-gpu`): EXCEPTION at tick 22 (EC=0x25, FAR=0x10, ELR in `compositor_loop`'s fill store, tid 16) with `irqsw=1` and `elrmm=spsrmm=1,0,0,0`: a corrupted register across an IRQ-path switch, the H1 family K10's gpu boot also hit. No new class; the feature is compiled out of both. The hazard grep is empty in both.
- V1: the IRQ path already clobbers V registers on `main`, independent of 1b. `<shared::observability::LogMessageBuf>::new` zero-fills with `movi v0.2d, #0` and six `str q0` (`shared/src/observability.rs:265`), and `::entries` has `stp q0, q0` (`:177`). Both are reached through `log_impl`: from `try_load_balance` (the `kinfo!` at `sched/init.rs:274` on every IRQ-context migration) and from `assert_valid_ctx` in `schedule()`. #219 brought `LogMessageBuf`; at `b07d7e4`, `log_impl` itself had 3 V sites on the same path. `memset` (10 V sites, `compiler-builtins` `impls.rs:350-351`) is reached through `drain_logs`, but only its misaligned-head and 8–15-byte-tail paths use V registers, and the drain's 160-byte aligned zero-fill takes neither. This is H5 evidence for the ADR (step 4), not a 1b defect, and V1 counts it as parity.
- D1: gates. `just check` has 0 warnings, `just test` passes 678, and the tools gates pass (`cargo fmt --check -p aios-tools`, `cargo clippy -p aios-tools --all-targets -- -D warnings`, `AIOS_REQUIRE_CHECK_PY=1 cargo test -p aios-tools`), since D1 edits a comment in `tools/src/cmd/soak/classify.rs`. The branch's own `aios docs-check` (`AIOS_TOOLS_BIN`) shows 1 new finding, the working plan; the developer-guide test count and the two layout entries are fixed. Plain `just docs-check` runs the main checkout's `aios`, which lacks the `lock_order.rs` port, and adds 15 stale `lock-order` findings for the IRQ-class statics (Merge), plus the plan.

## Decisions Made

- Owner, 2026-10-06:
  - **K5 lock cost:** accepted for B1 and the A/B soak; the remaining stamp reads are not cut first. B1 measures the N2 counters, not IPC, and both A/B arms carry the same lock.
  - **Merge gate:** the lock-word PC (EC=0x22, PC equal to a lock word) stays recorded as *inferred*, because its log was lost, and 1b's merge does not wait for a reproduction. 1b is detect-only instrumentation that would capture a recurrence; H1 already stands on run-08's TIMEOUT_QUEUE holder (log kept); the H1 fix step's interleaved A/B soak is the real test. (This was the assistant's recommendation on 2026-09-29; the owner confirmed it on 2026-10-06.)
- Owner, 2026-09-29, after the K5 deep dive:
  - Apply variant T (task K5b) and the Gate 1 zero-iteration guard, then continue K6–K10 as planned, scans included; no extra H1 tripwires.
  - The lock-word PC signature (EC=0x22 with ELR equal to an `IrqSpinLock` word, log lost) is explained by inference: H1 resumes a thread at a foreign ELR and a stale epilogue slot holding a lock-word copy becomes the return address. The PR and the ADR errata record it as inferred; merge does not wait for a reproduction. D1 adds a decoding note so a recurrence is recognised on sight (HELD bit, IRQS_ON bit, CPU, generation).
  - Rate-settling soak, base vs K4 vs the merging kernel (T), about 30 boots per arm, interleaved: both on a CI `workflow_dispatch` matrix (its own baseline, never mixed with local numbers) and later on a clean local host.
  - #198 (boot stack outside every PT_LOAD) is ruled out as the `frame.rs:51` cause: the kernel allocator already excluded the stack, and BootInfo is at 0xbcb40000 in all 101 logged boots.
- Owner, 2026-09-28 (#200): CPUs 1–3 never take timer IRQs on this kernel (found by K2's `tick` counter; confirmed by QEMU `-d int`, the GIC state and the code). Step 1b stays detect-only and does not fix it; B1 measures the kernel as it is; the GIC fix is its own later step, and N2 is re-baselined after it. K8's two strikes gate only on CPUs whose `tick` has ever advanced; K3's and K7's acceptance items that need IRQs on CPUs 1–3 are amended above.
- S1: `iter()` returns a named `FixedQueueIter` (queue reference + logical position, two words) that yields copies. It is not an `impl Iterator` over two chained slices, which would be four words. `next()` uses `wrapping_*` index arithmetic and `buf.get()`, so the dev build adds no overflow-check or bounds-check panic paths. It skips a `None` slot rather than stopping, so `FusedIterator` holds unconditionally. `ExactSizeIterator` relies on the structural invariant that every slot in the live range is `Some`. `contains(&T)` needs `T: PartialEq` (as `VecDeque::contains` does). For K8: the iterator is 16 bytes, so consume it in a `for` loop in `RunQueue::for_each`, which lets SROA keep it in registers, and never store it or pass it by value; V1 checks for NEON. `FixedQueueIter` is not re-exported at the crate root (`shared::collections::FixedQueueIter`). Host tests: 559 → 564; Miri runs the 23 collections tests cleanly.
- S2: The pre-release hook is a type parameter stored in the lock, `StampedLock<T, H: PreRelease = ()>`. It is not a function pointer (V1 bans `blr`) and not a wrapper guard (16 bytes). The guard stays one pointer, and drop order runs `pre_release` before `store(0, Release)`. **For K5:** put `holder_site`/`holder_tid` in the hook type (`StampedLock<T, HolderFields>`) and keep `class`/`index` on the wrapper. This changes §2.1's field layout, not its protocol.
- S2: The orderings behind the L1 snapshot:
  - holder fields are stored with `Release` right after the CAS, and cleared in `pre_release`;
  - `restamp` is a `Release` store (a Relaxed one would end the CAS's release sequence);
  - `consistent_snapshot(|stamp, hook| ..)` loads the word with `Acquire`, runs the closure, issues `fence(Acquire)`, reloads the word, and returns `None` if the lock was free or the two words differ.
  Together these rule out both an earlier holder's fields and a later holder's.
- S2: `OwnerStamp` wraps `NonZeroU64`, so `Option<OwnerStamp>` is 8 bytes. `new` is therefore a non-`const` fn (`NonZeroU64 | u64`) with no `unsafe` and no panic path. Compile-time `const _` asserts, checked on the kernel target too: the guard is 8 bytes and `Result<guard, u64>` is 16 (returned in x0/x1).
- S2: `try_lock`/`try_lock_weak` return `Err(observed word)`, so the slow path can classify without another load. *(Superseded by K5 review 1: they return `Option`.)* `Err(0)` from the weak CAS is a spurious failure. Guard operations are associated functions (`StampedGuard::restamp(&g, s)`) so they never shadow the data type's methods. The guard is `Sync` only for `T: Sync`.
- S2: Added `SCALAR_INDEX = 0xFF` (§2.1's index for non-array locks) beside `LockClass`, so that S3 and K5 share one constant. There is no `lock()` spin loop in shared: the slow path's counters, CNTVCT reads and prints are K5's. `lock.rs` denies `clippy::arithmetic_side_effects` and `indexing_slicing` outside tests.
- S2 (review): the Miri `ITERS` of the threaded test is now 400, not 25. The test's doc comment names the orderings it guards and says to re-run the mutations if `ITERS` changes. `just miri` stays unchanged. Running every shared test with `-Zmiri-many-seeds` would repeat the whole suite 16 times, while this change costs one test about 5 s. Six mutations were checked against the committed file on the default seed, and each one fails:
  - `Acquire` CAS → Relaxed, and release `store(0)` → Relaxed: both give a Miri data race on `*g += 1`;
  - first word load → Relaxed, no `fence(Acquire)`, `restamp` → Relaxed, and holder-field store → Relaxed: all give "fields of another holder".
  With explicit seeds 0–9, the fence, `restamp` and first-load mutations fail on every seed, and the field-store mutation fails on 8 of the 10. The unmodified test passes `-Zmiri-many-seeds=0..16`. `just miri`: 580 tests pass in about 90 s.
- S2 (review 2): took the reviewer's Option 1. The details:
  - `classify(observed, me: OwnerStamp, v)` takes the waiter's own stamp as an `OwnerStamp`: one register, and the same stamp its failed CAS used. The reviewer's `(u8, u64)` tuple would be 16 bytes. The Reentry test is exactly Option 1: the owner's CPU is `me`'s, and both generations equal `v.switch_gen(me.cpu)`, read once after the word load.
  - A waiter whose own generation has ended (`me.gen != now`) gets OtherCpu or OtherCpuSwitched, chosen by the owner's generation, instead of PreemptedHolder. Its stamped CPU no longer says where it runs. In the reviewer's sequence this turns the false Reentry into OtherCpu, which is the truth: the holder runs on CPU 0. It also limits K5's PreemptedHolder arm (`lkph`, and the `kind=self` compare against `CURRENT_TID[cpu]`) to waiters that were still in their generation at the check.
  - Added rule (c) to the soundness statement (module doc and §2.1): a thread's execution is ordered across a switch. The bump that ended its generation happens before its next instruction. Both the waiter-side argument and the holder-side `read_stamp` argument need it. The missing `on_cpu` handshake (ADR H2/F4: a thread published before its save completes) breaks it, and with it the thread's own program order. The module doc says so.
  - `classify` has a `compiler_fence(Acquire)` before its generation read, so the compiler keeps that read after the caller's word load. An IRQ between the two is exactly the case the check exists for. IRQs are taken precisely, so hardware reordering cannot matter, and the fence emits no instruction.
  - `read_stamp` has no fence: a compiler fence orders only memory accesses and cannot hold a `nomem` `asm!` in place. The `CpuView` contract carries the requirement instead (the reviewer's second item, accepted as written): `cpu()` is a fresh read and not `pure`, `nomem` or `readonly`, and `switch_gen()` is a fresh atomic load. K5's review checklist repeats it, together with the `lkself` tid caveat.
  - Not taken: Option 2 (re-read the CPU and generation around `classify`, and retry on a change). It is also sound if it compares generations, but it adds reads and a retry loop. Option 1 needs one generation read, as before.
- S3: the line writer, with its API for K2:
  - The call is `write_line(sink, src, mode, cpu, t, ncpu, |key, idx| value)`. The prefix values are scalar arguments.
  - `twc`, `twn` and `twmax` are catalogue keys, always printed, and come through the same closure.
  - The writers return nothing. `BufSink` counts its own bytes, and K2's UART sink can count too.
  - `ncpu` is clamped to `1..=MAX_CPUS`.
  - Iterated tables (`KEYS`, `SLOT_BASE`) are `static`, not `const`, so a loop never copies them to the stack.
- S3: every key has `CpuCounters` slots: 202 per row, one 1664-byte row per CPU, which is 13 KiB for 8 CPUs.
  - `value(key, idx)` reads row `idx` for a per-CPU key. Otherwise it takes the maximum over rows for a gauge and the sum for a counter.
  - `twmax` counts as a gauge because it is a maximum. The gauges are `*_now`, `scanhold1`, `scanhold2` and `twmax`.
  - Writers are `add`, `store` and `store_max` (load, compare, store). None of them is a read-modify-write.
- S3: `MAX_CPUS = 8` and `MASK_TIDS = 64` are shared copies of the kernel's `MAX_CORES` and `MAX_THREADS`. K2 and K8 should const-assert that they are equal.
- S3: `UnblockOutcome.kind` is a `#[repr(u8)] UnblockKind` rather than a raw `u8`. The struct is still 16 bytes (const-asserted).
  - `UnblockKind::of(state)` encodes `unblock`'s current decision. Blocked Ipc, Notification, Select and ProcessWait give `Woke`. Dead, Suspended, BlockedTimer and BlockedIo give `Revived` (`ubdead`). The skips and `NoThread` complete the set.
  - The phase constants are `PHASE_IDLE`, `PHASE_PUBLISHED` and `PHASE_ARMED` (0, 1, 2).
- S3: `classify_reply` treats `Revived` like `Woke`, since both make the thread Runnable. Either is `misrep` unless the thread was armed in this call. `NoThread` is not counted, because `unblock`'s `ubnone`/`badtid` already cover it.
  - Verdicts are `WakeVerdict::{NotCounted, N2(N2Kind), LateReply, Misrep}`, with `key() -> (Key, idx)`.
  - `try_reply_switch`'s check is `classify_reply` with `UnblockKind::Woke` and the caller's phase.
- S3: `classify_pc(pc, TextLayout { lo, hi }, virt_phys_offset)` takes the offset as an argument, so shared keeps no copy of `mmu::VIRT_PHYS_OFFSET`. `TextLayout` is 16 bytes, passed in two registers.
  - `Null` means exactly 0. A misaligned PC is `Other` wherever it points.
  - `Phys` means aligned and inside `[lo, hi)` minus the offset.
- S3: the plan names `qbad` without a rule. It is now "queued and not Runnable".
  - `classify_slot(state, flags, now, last_run)` takes a `SlotState` built with `SlotState::of(Option<&ThreadState>)` and a `SlotFlags(u8)` bitset.
  - A Runnable or Blocked thread that is current is `Clear`.
  - Scan A may call it before the waker flags exist and use only `Orphan`, `Starved` and `QueuedBad`.
- S3: `mask_set` returns `MaskSet::{New, Dup, BadTid}`, so `dupq`/`dupcur` come from the same call that builds the mask.
  - `TID_NONE` is `BadTid`, so the caller filters it first.
  - `pop_lowest` walks set bits, and `count_tids` counts them without NEON.
- S3: `TwoStrike<CPUS>` lives in atomics, so it can be a static. It keeps `armed`, a `LAST_RUN` snapshot per thread and a `tick` snapshot per CPU, all updated at each completed scan of its kind.
  - The tick gate is evaluated only when a confirmation is pending.
  - A stall changes nothing: the first strikes and snapshots are kept, and no new strike is added. The caller bumps `scanstall` and leaves the `EdgeCounter` alone.
  - A thread flagged every scan stays confirmed, and `EdgeCounter` counts it once.
- S3: `write_reentry_msg(sink, &ReentryReport { class, index, cpu, ctx, holder: Option<HolderSite>, owner: OwnerStamp, tid })` prints `holder_irqs` and `gen` from the observed stamp. The panic path is exempt from V1, so a struct argument is fine there. `Ctx::from_raw(IRQ_CTX[cpu], daif_i)` gives the label. The `IRQ_CTX_*` values (0, 1, 2) are shared constants for K3.
- S3 (review): took the reviewer's fix. `classify_reply(outcome, channel, clear)` and `classify_send(outcome, channel, clear)` take the waker's own `ClearResult`. This supersedes the S3 bullet above that fixed the phase constants at 0, 1 and 2. The details:
  - **Timed-ness goes in the phase byte:** a flag `PHASE_UNTIMED = 0x80` is set with both steps, and `wait_phase(step, timed)` builds the values. The reviewer's `PHASE_ARMED_UNTIMED = 3` alone would miss an untimed wait at phase 1, which is also a wedge precursor. A separate `timed: bool` would need another per-thread array and a second store that is not read together with the phase. With the flag, one byte, snapshot under THREAD_TABLE, carries both.
  - **Rule:** a skipped wake of a waiter in this wait is `rblk`/`vblk` when no timeout is left (untimed at step 1 or 2, or timed at step 2 with `Removed`), and `rpre`/`vpre` when a timed waiter is at step 1. A timed waiter at step 2 with `Absent` or `Busy` heals: the reply counts `latereply` and the send nothing. `misrep` compares the step, ignoring the flag. A private `SkipFate` enum shared by both classifiers encodes it.
  - **Bucket for timed step 2 without `Removed`:** `latereply` rather than `rpre` or a new key. That cell is mostly late replies (the reviewer's first case). The early-reply-seen-armed case cannot be told apart there, and a new key would change schema v1 for a count that means "heals" either way. The residual is stated in §4.13.
  - **For K6:** keep each waker's `ClearResult` until its classifier runs (`:100` → `:139`, `:392` → `try_reply_switch` and `:403`, `:457` → `:460`), and store phases with `wait_phase` (§2.4, §3 K6).
- K1 (review): accepted both review items; no code changed.
  - The stale `soak-qemu.sh:360-362` comment is listed for D1, not fixed in K1. K1's file is `bench.rs`, and D1 already owns the ADR's `soak-qemu.sh` line-drift erratum, which a longer comment would shift.
  - The `ELR=FAR=0x2_0000_0000` exception is recorded, not treated as a K1 regression. In every boot that shows it, the first fatal report is `main`'s `frame.rs:51` PANIC, and the ADR (N8) treats later symptoms in a PANIC boot as fallout. Two follow-ups:
    - K9's exception report (SP, TTBR0_EL1, VBAR_EL1, current thread) should name the thread that branches there.
    - If the signature appears in a boot with no earlier PANIC, it is a new signature, and it must be explained before 1b merges.
- K2: the acceptance "one `src=g1` line with all four `tick` values > 0" is amended: CPU 0's `tick` is > 0, and CPUs 1–3 show the timer IRQs they actually take, which is 0 on this host (Issues Encountered). The counter is right, as two separate checks show. Fixing the GIC grouping would change scheduling, which detect-only 1b must not do, so it is reported, not fixed.
- K2: no `held_by_stream` stub. `end_of_tick` defers on `CONSOLE_BUSY` alone, and K5 adds `|| held_by_stream(0)` at that test. Until K5 no IRQ-class lock carries a stamp, so a stub could only ever return false.
- K2: the module exposes functions, not public statics: `note_heartbeat()`, `request_g1_line()`, `set_console_busy(bool)`, `end_of_tick()`, `print_line(src, mode)`, `bump_masked(key, idx)` and `bump(key, idx)`. A per-CPU key takes idx 0, and both bumps pick the row from MPIDR.
  - `bump` has no caller yet and carries `#[expect(dead_code, reason = …)]`. Its first caller (K5 or K6) must delete the attribute, because an unfulfilled `expect` fails `just check`.
  - `cpu_here()` reads MPIDR without `nomem`, so in `bump` the read stays after the `msr DAIFSet`.
  - `bump_masked`'s `debug_assert!` has no message, so its failing branch calls `core::panicking::panic(&str)` and builds no `fmt::Arguments` on the stack.
- K2: at most one line prints per tick, the `hb` line before `g1`. `DEFERRED_TICKS` counts consecutive held-back ticks. After 256 of them the next busy tick prints anyway and bumps `hbdefer`, once per forced line.
- K2: `end_of_tick` is the last statement of `timer_tick_handler` (step 10, after the metrics step 9), which satisfies "after step 8".
- K2: the timer comments now say that `TICK_COUNT` advances on CPU 0 only, list all ten handler steps, and correct the drain comment. The old one claimed that draining every 4th tick kept the handler within 1 ms, but `DRAIN_BATCH_SIZE` is 16.
- K3: the signature is `note_dispatch(cpu, tid, site: DispatchSite) -> u8`. §2.3's `origin` is the commit site (`Enter`, `Schedule`, `Direct`, `Reply`). `note_dispatch` counts `xdir`/`xrep`/`xnever` itself, comparing the old `LAST_CPU` with the MPIDR CPU, not with the caller's `cpu`, which N4 can make stale. It still returns the old `LAST_CPU` for K7. `irqsw`/`irqsw0` are counted in `schedule()` at the commit, because only it knows its `Origin` (`Irq`, `Yield`, `Block`) and whether it saved a current context (`irqsw0` when `old_ctx_ptr` is null).
- K3: `note_dispatch` has `bump_masked`'s `debug_assert!` on DAIF.I. An out-of-range `tid` counts `badtid`, skips the per-thread stamps and returns `LAST_CPU_NEVER`; every commit site has already indexed THREAD_TABLE with that tid.
- K3: `WAKE_AT` is reset to 0 with the other stamps in `allocate_thread`, which is its only use until K6. §2.3's reset list omits it; resetting it with `WAKE_PENDING` keeps the pair consistent.
- K3: `IRQ_CTX`: `irq_enter` is the first call in `irq_handler_el1` (before the IAR read, so the spurious path is labelled too), `irq_preempt_check` comes before `check_preemption`, and `irq_leave` after it and on the spurious return. All three read the CPU with the tripwire's `cpu_here()` (no `nomem`), not `exceptions::core_id()`. `schedule()` saves `IRQ_CTX[cpu]` right after its re-entrancy guard. The one-store accessors `irq_ctx`/`set_irq_ctx` are `#[inline(never)]` too, like every new IRQ-path helper.
- K3: the `gic.rs` comment now says what the IRQ entry frame lacks: ELR_EL1 and SPSR_EL1 stay in the system registers across a switch in `check_preemption`, so the stub's `eret` uses whatever the last exception on the resuming CPU left there (H1).
- K3 (review 1): `n1` counts only when `origin == Origin::Irq`. Only there is a Runnable current thread the N1 orphan: a direct-switch receiver that is current and not queued. With `Origin::Block` it is F4(3). A waker on another CPU saw the Blocked state between `block_current`'s store and `schedule()`'s `THREAD_TABLE` lock. Either `unblock` queued the thread, or `try_direct_switch` is already running it there. `Origin::Yield` never reaches the branch, because `thread_yield` stores Running. Counting all origins would have left B1 unable to attribute a non-zero `n1`. Schema v1 is unchanged, and so is every scheduling decision; the `Key::N1` doc now says "on the IRQ return path". Open for D1 and the owner: whether F4(3) gets its own key in a later schema.
- K4: `irq_frame_check` sits in `exceptions.rs`, beside the stub. It reads ELR_EL1 and SPSR_EL1 itself, through the private `read_elr_el1` and `read_spsr_el1`, and counts with `bump_masked`. That is safe because IRQs are always masked there. Exception entry sets PSTATE.I, and every dispatch that can resume an IRQ-path thread runs masked (`schedule()`, `enter_scheduler`, and the direct and reply switches), so the DAIF.I `debug_assert!` holds.
- K4: also fixed K3's `gic.rs` comment ("That frame holds x0-x18, x29 and x30 only"), which the 192-byte frame made wrong. That file is outside §3 K4's list; the change is to one comment.
- K4: the CLAUDE.md fact is "EL1 IRQ entry frame (1b)" under Boot invariants, after the syscall ABI.
- K4 (review 1): accepted the review item. It is a plan-text fix only, and no code changed.
  - The old reading paired bench-main with the idle thread. But `pick_next` takes Idle last, and after `g1` CPU 0 always has a runnable Interactive bench thread, so its idle thread never runs.
  - The new reading names bench-main and bench-server. Each claim was checked against the code: the balancer moves only Normal threads; `schedule(Origin::Irq)` keeps a Running thread whose slice has time left, so bench-yield's turns, far shorter than 10 ms, never end on the IRQ path; and the K4 ELF shows both tail loops as `wfe; b`. `main`'s bench tail is the same.
  - Also corrected the H1 note: "a rate compared between arms" cannot work for `elrmm`, because `main` has no counter. The note now suggests counting mismatches before `g1`, or `elrmm` > 1, in boots whose first fatal report is not a PANIC. This round's gpu PANIC boot has a mismatch before `g1`.

- K5: the in-critical-section bookkeeping is inline and straight-line. *(The re-stamp is superseded by K5 review 1, below.)* This deviates from two things in §2.1/§2.5: "all new IRQ-path helpers `#[inline(never)]`", and "read_stamp again and restamp if it differs". The TB-start finding (Issues Encountered) is the reason. `hold()` stores the two holder fields (`Release`, as before), then runs `stamp_is_current`. That check compares MPIDR with the stamp's CPU and `SWITCH_GEN[stamp cpu]` with its generation, in one compare with 0, with no bounds branch (`tripwire::switch_gen_masked`, index masked by `MAX_CORES - 1`). Only on a mismatch does the cold, out-of-line `fresh_stamp` run the plan's `read_stamp`, followed by the re-stamp. The outcome is the plan's: re-stamp exactly when the stamp no longer names the running stream. The check is also sound as a currency test. A switch away bumps the generation it reads (rule (a)), rule (c) makes that bump visible wherever the holder runs next, and a switch back bumps it again. With IRQs masked it is always true. V1's rules are by source line, so inlining hides nothing from it. Every helper outside the critical section stays `#[inline(never)]`.
- K5: the lock is `IrqSpinLock<T> { inner: StampedLock<T, HolderFields>, id: LockId { class, index } }`. The holder fields live in the hook (S2's decision), and the plan's `class`/`index` fields are one 2-byte `LockId`, so the non-generic helpers take one register for them. `holder_site` stays an `AtomicPtr<Location>`, normalised with `map_addr` (`kva_of`). Per-CPU arrays use `[const { IrqSpinLock::new(..) }; MAX_CORES]` plus a `const fn set_index` loop, as §2.1 describes; `const RQ`/`const NONE` and their `allow`s are gone.
- K5: the stamp and the thread id are read together. `whoami()` does `read_stamp`, then `CURRENT_TID[cpu]`, then `SWITCH_GEN[cpu]` again, and retries if that moved. So the tid is the calling stream's own thread (the interrupted thread's, in an IRQ). The holder stores it in `holder_tid`, and a waiter compares it for `lkself`. This settles the fourth K5 review-checklist item: a thread's tid does not change when it moves, so the waiter's tid is right after its stamp goes stale, and the compare never reads `CURRENT_TID[s.cpu]` after `s` has ended.
- K5: `for_each_lock_word` reaches the statics through per-module `irq_lock_words(&mut f)` accessors (`sched`, `ipc` with `timeout` and `notify`, `observability`). THREAD_TABLE and CURRENT_THREAD are reached directly. The alternative, `pub(crate)` statics, would expose `pub(super)` element types (`TimeoutEntry`, `SelectWaiter`) and trip `private_interfaces`. That makes 23 words: 1 + 8 + 8 + 6.
- K5: details of the event line. `idx=-` for a scalar static. `ctx` is the `Ctx` label, not a number, as in the re-entry message. `holder_running` scans the online CPUs other than the waiter's. The line prints with IRQs masked (saved and restored), so that this CPU's own tick cannot split it. `kind=self` and `kind=stuck` print in any context; `kind=ph` prints only with IRQs masked (§2.1). The stuck check reads CNTVCT every 1024 spins against 2 × CNTFRQ.
- K5: `try_lock_quiet` has `#[expect(dead_code)]` until K8, K2's pattern; its first caller must delete the attribute. `tripwire::bump` has its first callers and has lost its `expect`. `cpu_here`, `read_daif`, `DAIF_I` and `UartSink` became `pub(crate)`, and `switch_gen`, `switch_gen_masked` and `current_tid` are new `#[inline(always)]` single-load accessors.
- K5: `scripts/docs/check.py`'s lock-order scan now recognises `IrqSpinLock<` statics as well as `Mutex<` ones. The file is outside K5's list. Without the change, the conversion adds 17 stale or unknown lock findings for names that are still locks. With it, `just docs-check` shows the same 4 new findings as K4, all of them D1's. `docs/kernel/observability.md` §6.5 also gained the `held_by_stream` deferral and a short `[tripwire-ev]` paragraph, because K5 adds both. D1 still owns that doc's sweep.
- K5 (review 1): took the reviewer's option (b), restoring K4's hold windows, rather than (a), accepting the widening. Option (a) needs the owner and leaves B1 with few text boots that reach the IPC phase.
  - (i) `StampedLock::try_lock` and `try_lock_weak` return `Option<guard>`, with no observed word. The `Result<guard, u64>` was what opt-level 1 re-tested after the CAS. After a failed CAS, `lock_contended` and `try_lock`'s counting load `owner_word()`. The stamp was taken before that load, as `classify` requires. The one visible difference: a `try_lock` failure whose holder releases before that load now counts nothing.
  - (ii) The re-stamp is branch-free and always stores. `current_stamp_or` reads MPIDR, `switch_gen_masked`, then MPIDR again. It keeps the new pair if the two CPUs agree (a `csel` via `core::hint::select_unpredictable`) and the `whoami` stamp if not. Either one names the holder's current generation or an ended one, which is all the re-entry verdict needs. With IRQs masked the store rewrites the same word. `stamp_is_current` and `fresh_stamp` are gone. A move inside those three instructions keeps a possibly stale stamp, where `fresh_stamp` retried; that can only hide a re-entry.
  - Also, for the same reason: `lock`, `try_lock` and `try_lock_quiet` are `#[inline(always)]`. With `#[inline]`, CURRENT_THREAD's `lock` stayed out of line in `enter_scheduler`, and TIMEOUT_QUEUE's in `ipc_call`/`ipc_recv`, which put a `ret` inside the hold. `spin::Mutex::lock` was inlined at every site in K4.
  - Also: `current_process_id` (`cap/mod.rs`, outside K5's list) splits `{ *CURRENT_THREAD[cpu].lock() }?` into two statements. In edition 2021 a block's tail temporaries live to the end of the statement, so the `?` test ran inside the hold. K4's compiler hoisted its one-store release above that branch, but it does not hoist the three-store release. Behaviour is unchanged.
  - Review item 2 accepted: `site_ref`'s SAFETY comment names `IrqSpinLock::hold` as the writer.
  - Gates: `just check` clean, `just test` 624 passed, and Miri on the 19 `lock::` tests is clean. The threaded Miri test still fails on the default seed when `try_lock_weak`'s `Acquire` is weakened to `Relaxed` (data race).
- K5 (review 2): accepted the review item, with no code change. Its figures check out against the soak logs; `lkoxp[THREAD_TABLE]` reaches 250,000 in the first K5 build, and the Gate 1 FAILs span all three K5 ELFs. The plan now records the IPC cost and its baseline (Issues Encountered), states the measured size in §4.5 for the PR, and puts the choice under B1 in Progress as an owner decision: accept the cost for B1 and the A/B soak, or cut the stamp reads first.
  - Also ran the split the review names as a reduction's first step, so that the owner decides with it: the stamp reads are the cost, and the contended path and holder fields are not (Issues Encountered). The two variants are measurement arms, not candidate fixes. `g` loses the re-entry panic, the contention counters and the event lines; `fg` also loses the stamps.
  - Not done: any reduction. That is the owner's call, and a reduction must keep the in-hold TB-start counts that `2cb77eb` restored. The measurement does not separate `whoami` from the in-hold re-stamp (`current_stamp_or`, 2 MPIDR reads). Dropping the re-stamp alone would be the next arm to boot. A stale holder stamp can only hide a re-entry, never invent one (§2.1).
  - The PC-equals-stamp exception (Issues Encountered) is reported, not diagnosed: its log is lost, and 8 more boots did not repeat it.
- K5b: the change is `target/dig/a1/diffs/T.diff`. boot.S writes `TPIDR_EL1 = MPIDR_EL1 & 0xff` just before each branch into Rust. `tripwire::cpu_tpidr()` has `cpu_here()`'s `asm!` options. `KernelView::cpu` (both CPU reads of `read_stamp`, so `whoami`) and `current_stamp_or` (the in-hold re-stamp) use it. Every other `cpu_here()` stays on MPIDR, because none of them makes a stamp: the counter rows, `note_dispatch`'s `SWITCH_GEN` index (rule (a) is stated in MPIDR terms), `note_restore`, the event lines and `holder_running`. The module doc's rule (a) now says why a TPIDR stamp names the same CPU, and what a wrong TPIDR_EL1 would do (stamps naming another CPU's generation could match a waiter there).
- K5b: `tpidrbad` is a per-CPU key after `rsthold`: schema v1 grows to 61 keys, and a `Full` line now has `n=66`. The golden line and the width and slot tests are updated, and the host test count is unchanged (624). Step 1a has not consumed the golden line, so the version stays `v=1`. It is per CPU, like `n4` (the other CPU-id mismatch counter), so a bad value names its CPU.
- K5b: the cross-check compares the whole TPIDR_EL1 value with `u64::from(MPIDR Aff0)`, not just the low byte the stamp uses, so any other writer shows. `check_tpidr()` is the first statement of `kernel_main` and `secondary_main`, before their first IRQ-class lock (`kinfo!`'s `BOOT_LOG` try-lock), and counts with `bump`, which masks for the update whatever the entry DAIF. `note_dispatch` repeats it with `add_row`, IRQs masked. Count only: nothing panics or changes.
- K5b: Gate 1 zero-iteration guard. `BenchResult::passes_below_us(limit)` is `iterations > 0 && avg_us() < limit`, and both Gate 1 verdicts use it. A run with no successful `ipc_call` (every call returned an error, so `record` never ran) now prints `FAIL`; the result line is unchanged and still shows `(0 iters)`. Two plugin boots of the dig printed `IPC round-trip ... (0 iters)` then `Gate 1: IPC < 10 us: PASS` (`target/dig/a1/runs/T-c1/serial.log:230`, `target/dig/a1/runs/k5b/serial.log:230`). The soak harness matches only `Gate 1: IPC < 10 us: *PASS` (`G1PASS`), so nothing parses the new verdict differently.
- K5b, for D1: the crash-fix ADR says TPIDR_EL1 is unused (`:69`), and step 8 (Decision 1C) plans to point TPIDR_EL1 at a per-thread `ThreadInfo`. That step must first move the stamp's CPU id (by then step 2 has replaced this lock) or keep the CPU id elsewhere; `tpidrbad` would count the clash. Rule 01's strict boot order should gain "set TPIDR_EL1" before the branch to `kernel_main`. D1 owns both.
- K5b (review 1): accepted, comment text only. `shared/src/lock.rs`'s `CpuView` contract said the kernel's CPU id comes from MPIDR (`:203`, `:209`) and the soundness text said a masked stamp comes "from MPIDR and `SWITCH_GEN`" (`:62`, `:257`). They now say the kernel reads TPIDR_EL1, which boot.S sets to the MPIDR Aff0 that rule (a)'s bump uses (`tripwire::cpu_tpidr`), that the barrier requirement is on that `asm!`, and "from the CPU id and `SWITCH_GEN`". Rule (a)'s "c read from MPIDR" (`:46`) stays: `note_dispatch` still indexes `SWITCH_GEN` by MPIDR. The tests do not change (624 pass), and neither does the code: every loadable section (`.text.boot`, both vector sections, `.text`, `.rodata`, `.data`) and every section address and size is byte-identical to a build of `6df832d`. Only the debug info differs (the moved line numbers), so the soak's ELF hash changes (`5638684a`).
  - For D1: the shared-crate ADR (`2026-05-07-cl-phase-7-m25-shared-crate-for-host-tested-kernel-types.md:91`) still describes `CpuView` as "MPIDR and the per-CPU switch generation"; it should say the CPU id (TPIDR_EL1). Left to D1's ADR errata, outside this review's scope.

- Merge (`main` `c6f5511`): V1 must now build both ELFs with `nightly-2026-10-06` (`rustc 1.101.0-nightly (ea137335b 2026-10-05)`), not `nightly-2026-09-24`, and the §5 A/B protocol's toolchain changes with it. The `main` arm is the merged `main` (`c6f5511` or later), not `b07d7e4`: a 09-24 build of `b07d7e4` against a 10-06 build of the 1b head would compare two compilers.
- Merge: `scripts/docs/check.py` was deleted on `main` (R1, #207) and modified here in `56c4bf4`. Resolved by deleting it and porting `56c4bf4`'s change to `tools/src/cmd/docs_check/checks/lock_order.rs`: the static-type regex is `\b(?:Mutex|IrqSpinLock)\s*<`, and the description reads "production Mutex/IrqSpinLock statics vs deadlock-prevention.md §3.3-3.4 and CLAUDE.md" (`model.rs`). The finding messages ("not a Mutex static") stay as `56c4bf4` left them.
- Merge: parity. The R1 parity goldens were recorded from check.py at `33c6b3d`, which defines no `IrqSpinLock` static, so no recorded finding changes; only the `lock-order` line of `list-checks` differs. That golden (`real/list-checks.golden`) and the `CHECK_PY_LIST_CHECKS` constant now hold check.py's output at `56c4bf4` (verified byte for byte with `git show 56c4bf4:scripts/docs/check.py --list-checks`), and `fixture.rs`/`docs_check_parity.rs` say so. A new test, `lock_order_counts_irq_spin_lock_statics` (`tools/tests/checks_code.rs`), pins the `IrqSpinLock` case with expectations recorded from check.py at `56c4bf4` (plain, array and path-qualified statics count; `MyIrqSpinLock<` and `IrqSpinLockGuard<` do not). A differential on the merged tree (check.py at `56c4bf4` with `.claude/CLAUDE.md` copied to the root, against this branch's `aios`) gives the same 16 `lock-order` findings.
- Merge: `just docs-check` runs the **main checkout's** `aios` through `.claude/hooks/aios`, which does not have the port until this branch merges; it reports the 9 IRQ-class statics as stale (17 new `lock-order` findings). Run the branch's own build with `AIOS_TOOLS_BIN=<worktree>/target/dig/tools/release/aios just docs-check`: 4 new findings, all present before the merge (developer-guide test count 594 vs 659, layout missing `shared/src/lock.rs` and `shared/src/tripwire.rs`, the working plan). D1 owns the first three.
- Merge, for K6: `main`'s #211 adds no `unblock`, `wake_with_error`, `try_wake_select` or `schedule` caller, so K6's site list is unchanged. It does add boot-time ipc self-tests (`ipc/tests/kit_errors.rs`, `syscall_args.rs`) that take the channel rejection paths (`CapabilityDenied`, `InvalidChannel`), so K6 records the per-site `badchan` self-test baseline on the merged kernel, not from earlier boots.
- Merge: `main`'s #209 moved the boot stack into a `.stack (NOLOAD)` section in the RW PT_LOAD segment, and `linker.ld` now asserts the image fits the boot TTBR1's 4 x 2 MB window, which covers §4.7's "check the image end" at link time.
- Merge219: merged `main` `5838ad2` (#219, then #220) into the branch. Two files conflicted, and neither conflict touched code.
  - `timer.rs`: the drain comment only. The resolution keeps the branch's text (single consumer, CPU 0 only, IRQs masked) and adds #219's cost (16 lines plus dropped-message lines and pending entries, ~130-character joined lines, ~11 ms each at 115200 baud). The tripwire hooks auto-merged unchanged: `bump_masked(Key::Tick)` after the rearm, `note_heartbeat()` after the heartbeat `writeln!`, and `end_of_tick()` as the last step on CPU 0.
  - `observability/mod.rs`: the `use` list only, and both sides are kept. `BOOT_LOG` is still an `IrqSpinLock` reached by `irq_lock_words`, and #219's `log_impl` pushes inside `with_irqs_masked`.
  - `bench.rs` and `ipc/mod.rs` auto-merged with disjoint hunks: #219's pid arguments to `channel_create_unchecked`/`channel_set_peer` (`ProcessId(8)`), and the branch's `CONSOLE_BUSY`/`G1_PENDING` and `irq_lock_words`.
- Merge219: does tripwire IRQ-path code now reach a LogRing push or a lock? No, so no code changed. `with_irqs_masked` has one caller, `log_impl`, and wraps only the lock-free `LOG_RINGS[core].push`. Neither `observability/tripwire.rs` nor `sync/` calls `klog!` or `log_impl`. The IRQ-context pushes that exist are `main`'s, as before the merge: the balancer's `kinfo!` (`sched/init.rs:265`, holding `RUN_QUEUES[first]` and `[second]`) and the `pc=0` check's `kerror!` before its `drain_logs` and panic (`scheduler.rs`). In IRQ context DAIF.I is already set, so the helper saves, sets the bit again and restores the same value.
- Merge219: does #219's masking change what the K5 stamps or the counters assume? No.
  - No lock is taken inside a `with_irqs_masked` window, so no `IrqSpinLock` acquisition sees DAIF.I set there. The `IRQS_ON` bit, the `thread-off` label, `held_by_stream` and `rsthold` mean what they did.
  - A thread that logs while it holds an IRQ-class lock with IRQs on masks only for the push and restores IRQs on. Its stamp's `IRQS_ON = 1` stays true outside that window, and no IRQ can run inside it to classify against the stamp.
  - No counter is bumped inside the window. `bump` (save, mask, restore) and `bump_masked` keep their contract (§2.2).
- Merge219: `tripwire::bump` does what `with_irqs_masked` does. It masks, then clears the mask only if DAIF.I was clear on entry, which is equivalent to restoring the saved DAIF. The merge does not fold one into the other, because it fixes only conflicts and `bump` must stay a non-generic `#[inline(never)]` symbol for V1. D1 decides whether `bump`'s body becomes `with_irqs_masked(|| add_row(cpu_here(), key, idx, 1))`, keeping `#[inline(never)]` on `bump`. K9's panic handler cannot use the helper: it masks for good, never restores, and keeps `irq_was`.
- Merge219, for K9: `drain_logs` is now at `observability/mod.rs:355`, and the panic handler is at `main.rs:397`. #219 changed what a call prints.
  - Its local `drained` counts joined lines: a head plus its continuation is one line, and dropped-message report lines are not counted.
  - It can end above `DRAIN_BATCH_SIZE`, because pending entries print past the limit. K9's bounded panic loop must therefore continue while the return is `>= DRAIN_BATCH_SIZE`, not `== DRAIN_BATCH_SIZE`.
  - The bound `LOG_RING_SIZE * MAX_CORES / DRAIN_BATCH_SIZE` (256 × 8 / 16 = 128) still holds, because every counted line pops at least one entry.
  - `LogRing`'s `unsafe impl Sync` text now documents the overlapping-drain gap and its callers (the tick, boot flushes, and the `pc=0` check on any CPU). `CPU0_DRAINING` closes only CPU 0's panic-against-tick overlap, so K9 adds the panic drain to that text's caller list.
- Merge219, for K6: the site set is unchanged, because #219 adds no `unblock`, `wake_with_error`, `try_wake_select` or `schedule` caller. Lines moved:
  - `process_exit` now matches endpoints by pid (`owner_a`/`owner_b: ProcessId`). Its `epipe_wakeups` stores (plan `:192`/`:198`) are at `task/process.rs:164` and `:170`, `wake_with_error` (`pexit`, plan `:209`) at `:181`, and the `pwait` `unblock` (plan `:223`) at `:195`.
  - `channel_destroy_unchecked`: the `wake_recv`/`wake_caller` reads (plan `ipc/mod.rs:227-228`) are at `:241-242`, and the two `wake_with_error` calls (`chdestroy`, plan `:231`/`:234`) are at `:245`/`:248`.
  - For B1 and the A/B soak: the pid walk also reaches channels created for label tids (0x700, 0x201, ...), so `pexit` counts can differ from earlier boots. Both arms built from the merged `main` have it.
- Merge219, for V1 and the §5 A/B protocol: #219 changed code that CPU 0's tick runs (`drain_logs` with `next_log_line` and continuation joining) and that the balancer reaches in IRQ context (`log_impl` with `LogMessageBuf` and `with_irqs_masked`). The `main` arm must therefore be `5838ad2` or later, not `c6f5511`.
- Merge219: gates. `just check` has 0 warnings and `just test` passes 677. The tools gates pass (fmt, clippy `-D warnings`, `cargo test -p aios-tools`), because #220 and this branch's `lock_order.rs` port both touch `tools/`. The branch's own `aios docs-check` (`AIOS_TOOLS_BIN`) shows the same 4 new findings as the first merge, all D1's: developer-guide test count (now 612 vs 677), the layout entries for `shared/src/lock.rs` and `shared/src/tripwire.rs`, and the working plan. Boot deferred: load 234 at the start and 333 at the gate (rule: above 30, do not boot).

- K6: `try_reply_switch(replier, caller, channel, clear)` gains the channel and the reply's `ClearResult`, and classifies the wake once the caller is validated (`misrep` only, since a validated caller is Blocked). `try_direct_switch` gets no `channel` parameter, unlike §2.4: nothing would read it, because receive-side misdirected wakes are out of scope (§6 SCAN-2), and an unused parameter is noise. Both switch functions count `badtid` in their bounds-check branch.
- K6: `UnblockOutcome` comes back through the caller's result slot (`x8`, sret), not in `x0`/`x1` as §2.4 says: that is the Rust ABI for `{u8, u8, u64}` here. `unblock` hands its own result pointer to `note_unblock`, so the outcome is written once, in place, with no copy and no V register (Issues Encountered, K6).
- K6: an out-of-range `tid` in `unblock` counts `badtid` and `ubnone[src]` and returns, where `main` panicked on the index (§2.4's `ubnone`). An empty slot returns as before, and its guard still drops after the DAIF restore, as on `main`.
- K6: the `WAKE_PENDING` marker records "a wake is in flight", and the source in it is the best guess at the marking site: the notification signal marks `sig` even for a select-blocked waiter that `try_wake_select` then wakes as `selsig`, and the deadline scan marks `nto` before it knows whether the waiter is in a notification or a select wait. The exact source is in the outcome counters.
- K6: two marker sites beyond §2.4's list, both inside its rule ("wherever a waker removes the target's last reference", cleared where the wake is abandoned). `notification_destroy` marks `ndestroy`, because taking the notification removes its waiters' last reference, as at the listed sites. `check_notification_timeouts`'s `_` arm (the deadline belongs to a thread no longer in a notification or select wait) clears the marker like the contended `continue`: no `unblock` follows, and a marker left behind would make K8 read that thread's later no-waker block as `wakefl`.
- K6: `clear_timeout` tests `is_some()` and then stores `None`, as `main` stored `None`, rather than calling `take()`, which would move the 24-byte entry out on the IRQ path (`check_timeouts` → `wake_with_error` → `clear_timeout`).
- K6 (review fix, supersedes the first K6 rule that every busy clear counted `ctbusy`): `clear_timeout` only returns its `ClearResult`. Its callers count `ctbusy` through `tripwire::note_clear` (`#[inline(never)]`, `bump`, safe in thread and IRQ context): the reply, send and call wakers, and `wake_with_error` for every source except `WakeSource::To`. On the `to` path `check_timeouts` has already set the entry to `None` before it drops TIMEOUT_QUEUE, so a busy clear there only means another CPU held the queue. Counting it let every `sleep_ticks` expiry on CPU 0 add `ctbusy` in CLEAN boots, which is not the stale entry §2.4, §4.5 and §4.13 read it as. `note_clear` disassembles to a compare and a tail call to `bump`, with no V register.
- K6: the balancer counts `lb` only when its `push_back` succeeds. A failed push (a full Normal queue; the thread is lost, as on `main`) counts `enqfull`, which matches that key's definition. `RunQueue::enqueue` counts `enqfull` with `bump`, because its callers include thread-context paths.
- K6: phase stores: `wait_published` stores the channel (Relaxed), then the phase (Release); `wait_armed` and `wait_ended` store only the phase (Release); readers load the phase (Acquire), then the channel. The armed store sits inside the TIMEOUT_QUEUE guard's scope right after the entry is written, or in an `else` branch for an untimed wait. `ipc_call`'s phase ends at "Woken up", before its timeout clear; `ipc_recv`'s right after `block_current` returns.

- K7: `tripwire::check_restore(ctx, stack_phys, old_last_cpu)` (`#[inline(never)]`, IRQs masked, count only) runs right before `assert_valid_ctx` at `enter_scheduler` and both `schedule()` restores, and right before `note_restore` in `try_direct_switch` and `try_reply_switch`. Each site keeps `note_dispatch`'s return (the old `LAST_CPU`) and reads the thread's `stack_phys` under THREAD_TABLE, beside its context pointer. The PC goes through `classify_pc` against `TEXT_LO`/`TEXT_HI` with `mmu::VIRT_PHYS_OFFSET`. The SP range is `[DIRECT_MAP_BASE + stack_phys, + STACK_SIZE]`, inclusive at the top, because a fresh context's `sp` is the stack's top. All arithmetic wraps.
- K7: `capture_text_layout()` is called right after `check_tpidr()` at the top of `kernel_main`, which runs at its VA from its first instruction, well before `smp::bring_secondaries_online`. It takes `&raw const __text_start`/`__text_end` (no `unsafe` needed) with its own `extern` declaration, as `kmap` has.
- K7 (deviation): the exemption cannot be written with `STACK_SIZE`. `Thread::new_kernel`'s physical default `sp` is `stack_phys + 4 * PAGE_SIZE` (16 KiB), while `STACK_SIZE` is 8 pages (32 KiB). So `task/mod.rs` (outside K7's file list) gains `NEW_KERNEL_SP_OFFSET = 4 * PAGE_SIZE`, which `new_kernel` now uses and the check compares with, so the two cannot drift. Behaviour is unchanged. Also fixed `new_kernel`'s doc, which called that point "the top of a 16 KiB stack". Every kernel thread creator replaces the default with the virtual top before the thread first runs, so the exemption covers only a creator that forgets to.
- K8 (deviation, owner decision 2026-09-28): `TwoStrike::scan` in `shared/src/tripwire.rs` (outside K8's file list) now gates only on online CPUs whose `tick` reads non-zero, i.e. has ever advanced. As S3 wrote it, the gate needed every online CPU to advance by 100, so with CPUs 1-3 at 0 (#200) no flag could ever confirm. A CPU that starts ticking gates from then on, and one that stops after ticking keeps gating. The existing pending-confirmation test now lets CPUs tick; a new test covers CPUs that never tick, one that starts late, one that stops, and no CPU ticking at all.
- K8: scan B runs only after a completed scan A in the same heartbeat. Its candidates are the threads scan A saw in `BlockedIpc`, `BlockedNotification` or `BlockedSelect`, and §2.6 already makes NOTIFICATION_TABLE depend on scan A; "phase 2 is independent of phase 1" is read as "takes its own locks, after phase 1 released all of its". When scan A is skipped, scan B is not attempted, and neither `scanb` nor `skipb` counts.
- K8: scan A classifies each slot under THREAD_TABLE (`tripwire::scan_note_slot`, `classify_slot` with `QUEUED`/`CURRENT` from the masks already built), rather than keeping a `SCAN_RUNNABLE` mask and classifying after the release: the slot's `SlotState` is then never stored. The accumulators are `SCAN_QUEUED`, `SCAN_DUPQ`, `SCAN_CLASS[4]`, `SCAN_CURRENT`, `SCAN_DUPCUR`, the per-scan flag masks `SCAN_ORPHAN`, `SCAN_STARVED`, `SCAN_QBAD`, the scan-B candidate masks `SCAN_BLOCKED_IPC`/`_NOTIF`/`_SELECT`, and scan B's `SCAN_TIMEOUT`, `SCAN_CHANREF`, `SCAN_NDL`, `SCAN_NOTIF`. `dupq`, `dupcur` and `qbad` add the popcount of their mask once per completed scan (single-scan checks, not edge-counted). `starved[c]` takes the class of the queue the thread was found in.
- K8: `scanhold1` is timed around `sched::scan_snapshot`, from the first try-lock to the last release, and recorded only for completed scans. `scanhold2` is the longest single-table hold in scan B, timed from each acquisition to its release.
- K8: `skipaself` comes from `sync::held_by_own_stream(word)`: after a failed `try_lock_quiet`, the word is checked against CPU 0's current stamp. `held_by_stream` now shares the stamp test (`is_stream_stamp`); its behaviour is unchanged. `try_lock_quiet` lost its `#[expect(dead_code)]`. A failed scan B lock counts `skipb` alone: CHANNEL_TABLE is a `spin::Mutex` with no stamp to test.
- K8: `ipc/scan.rs` is the accessor module (`scan_wakers`). NOTIFY_DEADLINES became `pub(super)` for it. CHANNEL_TABLE is taken with `spin::Mutex::try_lock` and the IRQ-class tables with `try_lock_quiet`, each alone. SELECT_WAITERS is not read (§2.6: not a waker). A deadline of `u64::MAX` means none.
- K8: scan B reads each candidate's `WAKE_PENDING` with `Acquire` after all four tables, and classifies with `classify_slot(state, flags, 0, 0)` (the time arguments only matter for Runnable slots). A current blocked thread is clear, as in S3.

- MergeR4: merged `main` `1a5c363` (#221–#230; the toolchain moves to `nightly-2026-10-09`). Three files conflicted, all in `tools/`: `checks/lock_order.rs`, `tests/common/fixture.rs` and `tests/docs_check_parity.rs`. `lock_order.rs` keeps #228's `pyre::compile` for every pattern and this branch's `LOCK_TYPE_RE`, now `pyre::compile(r"\b(?:Mutex|IrqSpinLock)\s*<")` (`\s` becomes Python's class, as for the other patterns), and the module doc combines #228's class text with the `IrqSpinLock` note. The two test files take #228's text, plus the `56c4bf4` note.
- MergeR4: the check.py oracle. #228 (with #206) brought back a differential test that materialises check.py from git history (`33c6b3d`'s blob), patches it with `check-py-claude-md.patch` and runs it next to `aios` on every golden case and on the live checkout. That check.py predates `56c4bf4`, so it would print the old `lock-order` description (the `real/list-checks` golden holds `56c4bf4`'s) and, on the live checkout, report all 9 IRQ-class statics as stale. Fix: a second oracle patch, `tools/tests/fixtures/docs-check/check-py-irq-spin-lock.patch` (`fixture::CHECK_PY_IRQ_SPIN_LOCK`), which is `56c4bf4`'s check.py diff, applied after the migration patch. The patched result is exactly `56c4bf4`'s check.py plus the migration (checked with `diff`). A patch rather than `56c4bf4`'s blob, because `56c4bf4` exists only on this branch and a squash merge leaves it out of `main`'s history, where CI's oracle reads. With `AIOS_REQUIRE_CHECK_PY=1` and CPython 3.14.3, `differential_against_check_py` compares 70 cases and 4 live modes with 0 differences, and the ignored `record_goldens_from_check_py` rewrites every golden byte for byte: **no golden changed.**
- MergeR4: `just soak` is now `aios soak` (#230; `scripts/soak-qemu.sh` deleted). Checked against the plan's commands: it takes `runs=`, `secs=`, `mode=gpu`, `out=`, `report_only=1` and `--no-build` in any order before a log file (`soak::parse`), and writes `target/soak/<YYYYmmdd-HHMMSS>-<mode>/run-NN.log` (two-digit index for under 100 runs), each ending in a `[soak] meta` line, plus `summary.tsv` whose 22 columns keep the script's order (column 3 `class`, column 9 `load1`, column 22 `log`). So the boot gate, T-self's and B1's commands and B1's extraction are unchanged. One difference to note for B1: `just soak` is `[no-cd]`, so a relative `out=` resolves against the directory `just` is invoked in, and the soak boots the checkout that contains it; run B1 from the worktree root. The `[tripwire]` lines and the PANIC next-line form are the kernel's, so the hazard grep and T-self's acceptance do not change.
- MergeR4, for D1: the `tick == 0` comment to rewrite is now in `tools/src/cmd/soak/classify.rs`, two lines, and the ADR's `soak-qemu.sh` line-drift erratum is stated against the deleted script's last blob (D1 entry updated). The plan's other `soak-qemu.sh:N` citations (§1, §4) are to that same blob.
- MergeR4, for step 1a: §Approach says 1a waits for the R4 soak port; R4 is now on `main`, so 1a (the classes, the interleave mode) is written against `tools/src/cmd/soak/`, not the script. Out of 1b's scope; recorded for the hand-off.
- MergeR4, for K6–K10: `main`'s kernel changes since `5838ad2` (#224, #226: `ipc/mod.rs`, `task/process.rs`, `service/mod.rs`, `syscall/user.rs` and the ipc self-tests) add no `unblock`, `wake_with_error`, `try_wake_select` or `schedule` caller and no `spin::Mutex` or `IrqSpinLock` static, and touch no IRQ-path file.
- MergeR4, for V1 and the §5 A/B protocol: both ELFs are now built with `nightly-2026-10-09` (`a30aa9064`), and the `main` arm is `1a5c363` or later.
- MergeR4: gates. `just check` has 0 warnings and `just test` passes 678. Tools gates pass: `cargo fmt --check -p aios-tools`, `cargo clippy -p aios-tools --all-targets -- -D warnings`, `cargo test -p aios-tools` (with `AIOS_REQUIRE_CHECK_PY=1`). The branch's own `aios docs-check` (`AIOS_TOOLS_BIN`) shows the same 4 new findings, all D1's (developer-guide test count now 612 vs 678, the two layout entries, the working plan). Boot deferred: load 50 at the start and 71 at the gate.
- K9 (deviation): `CPU0_DRAINING` is two flags, keyed on the IRQ mask at the drain's entry. `CPU0_DRAINING_MASKED` brackets a drain that started on CPU 0 with IRQs masked (the tick, the `pc=0` check, the panic drain), which nothing can interrupt or move. `CPU0_DRAINING_OPEN` brackets one that started on CPU 0 with IRQs on (boot flushes, `storage`), and is cleared wherever that drain ends. With one flag, as §2.7 wrote it, a tick that drains inside a boot flush clears the flag the flush set, and a panic in a later tick of the same flush would then drain over it; a save-and-restore flag instead sticks at `true` when a thread drain migrates off CPU 0, turning the panic drain off for good. Known gap: two threads draining on CPU 0 in turn, the first preempted inside its drain, can clear `OPEN` under the first; that only lets a panic drain overlap it.
- K9: the bounded loop is `observability::drain_logs_after_panic()`, so that `DRAIN_BATCH_SIZE`, `LOG_RING_SIZE` and the flags stay private there; it returns at once off CPU 0 or while `cpu0_draining()`, and the panic handler calls it unconditionally. It stops when a call prints fewer than `DRAIN_BATCH_SIZE` lines (the Merge219 `>=` rule), at most `PANIC_DRAIN_CALLS` = 128 calls. `drain_logs` returns the lines its body (`drain_rings`) counted; the 16 existing callers ignore it. The `LogRing` `unsafe impl Sync` text and the `DRAIN_BATCH_SIZE` doc now list the panic drain.
- K9: `ctx=` on the `[panic]` line is the label (`thread`, `thread-off`, `irq`, `irq-exit`), as the `[tripwire-ev]` lines and the PANIC-LOCK message print it, from `IRQ_CTX[cpu]` and the DAIF.I the handler found; §2.7's `ctx=N` is read as that label. `t=` is CNTVCT_EL0 as `secs.micros` (6-digit micros). `irq_elr=` prints only for `irq`/`irq-exit`. The exception's `  ctx:` line prints `irq=` as the raw `IRQ_CTX` value (0, 1, 2) and `sched=` as 0 or 1, as §2.7 wrote them.
- K9: `PANICKING` is in `main.rs` beside the handler, indexed by `cpu_here().min(MAX_CORES - 1)` like the other per-CPU tables. The handler's DAIF read and mask are one `asm!` (`mrs` then `msr DAIFSet, #0x2`), not `with_irqs_masked`, which would restore the mask (Merge219).
- K9: the optional `#[track_caller]` on `compositor::release_buffer` is taken: it is a funnel for three `free_dma_pages` calls (`compositor/service.rs:422`, `:450`, `:451`), so the assertion names which one. The chain is `release_buffer` → `free_dma_pages` → `FrameAllocator::free_pages`. `free_frame`, `free_user_pages` and the slab's direct `fa.free_pages` are not `#[track_caller]`, so their assertion location is their own call line in `frame.rs`/`slab.rs`, no longer `frame.rs:51`.
- K9: `read_elr_el1` in `exceptions.rs` became `pub(crate)` for the `[panic]` line; `read_ttbr0_el1` is new and `pub` (K10 reuses it).
- K9: docs. Rule 01 and `.claude/agents/kernel-dev.md` now say the panic handler masks IRQs first and takes no lock; `developer-guide.md` Pattern 2 shows the new order and describes the `[panic]`, `regs:`, `ctx:` and `src=panic|exc` lines and the `#[track_caller]` chain.
- K10: `smp::print_cpu_line(cpu)` prints the line with `println!` (thread level at boot, never the IRQ path, so `core::fmt` is allowed). `vbar_kva` is `VBAR_EL1 >= KERNEL_BASE`; `ttbr0_idmap` compares `TTBR0_EL1 & TTBR_BADDR_MASK` (new `mmu.rs` const, BADDR bits [47:1], so the ASID and CnP drop out) with `ttbr0_l0_addr()`, which loses its `#[allow(dead_code)]`. Secondaries print it inside the `PRINT_TURN` window right after `kinfo!("Core N online")`; CPU 0 prints after `Address space switching verified`, inside the step-10 block, while address space B is installed. That is execution order: the `Core N online`, `4 CPUs online` and `TTBR0 switch` lines are ring-buffered (`LogRingsReady` is set before `bring_secondaries_online`) and reach the UART only at the next drain, so the direct `[smp]` lines appear above them in the serial log. `observability.md` §6.5 gains an "SMP lines" paragraph with the SMP-timeout interleave note.
- T-self: the self-test is `kernel/src/sync/selftest.rs` (§3 names no file), behind `mod selftest` in `sync/mod.rs` and a call after `bench::init()` in `kernel_main`, both `#[cfg(feature = "tripwire-selftest")]`. One thread, `CpuSet::single(0)` because CPUs 1–3 take no timer IRQs (#200), yields for 200 ticks, prints a `[selftest]` line, takes `THREAD_TABLE.lock()` with IRQs on, and spins in `hold_loop` (`#[inline(never)]`, so `irq_elr` can be matched to its address range) for 3 ticks. Every tick sets `NEED_RESCHED`, so the first CPU 0 tick re-enters in `schedule()`; if no panic comes, it releases, prints a `[selftest] … FAIL` line and parks in `wfe`.
- T-self (deviation): the thread is Interactive, not Normal, so that it runs on CPU 0 alongside the bench threads (Issues Encountered, T-self). No `justfile` recipe: §3's build command, the hand-assembled ESP and `--no-build` are enough for a test that is never in a soak arm.
- V1 (deviation, baseline): the task names `b07d7e4` as the baseline, and MergeR4 says the `main` arm must be `1a5c363` or later. Both arms were built, and the gate is judged against `1a5c363`, the merge base of the 1b diff. Every ELF was built with the branch's pinned `nightly-2026-10-09` (`RUSTUP_TOOLCHAIN` overrode `b07d7e4`'s own 09-24 pin), from temporary worktrees under `target/dig/v1/` that were removed afterwards, using `env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS -u CARGO_TARGET_DIR -u CARGO_BUILD_TARGET just disk`. Builds had 0 warnings. `strings -a` shows `rustc version 1.101.0-nightly (a30aa9064 2026-10-08)` in all three ELFs (`b07d7e4`, `1a5c363` and head `eefb177`).
- V1 (method): `target/dig/v1/v1.py` parses sysroot `llvm-objdump -d -l -C`. It builds R by following `b`/`bl`/`b.cond`/`cbz`/`cbnz`/`tbz`/`tbnz` targets that land in another function, starting from `irq_el1_entry`. It does not follow `blr`/`br`, and it stops at `reentry_panic` and `rust_begin_unwind`. A V site is any operand that names `v`/`q`/`d`/`s`/`h`/`b` registers. Its source line is the innermost `-l` line. For rule 1, head lines in files the diff touches are mapped to base lines through `git diff -U0`. Lines the 1b diff (`1a5c363..HEAD`) adds or changes, and every line of the four new files, count as 1b lines.
- V1 (result vs `1a5c363`): |R| 54 → 128. R has 26 V sites in both ELFs: `LogMessageBuf::new` 7, `::entries` 9, `memset` 10. **Rule 1: 0 parity violations. Rule 2: 0 V sites on 1b lines anywhere in the ELF. Rule 3: the only `memset`/`memcpy`/`memmove` call from a 1b line is `reentry_panic`'s `bl memset` (`shared/src/tripwire.rs:903`, its `BufSink`), which is exempt as halt-terminal and is not in R.** The only external callee reachable only in the head graph is `core::panicking::panic_null_pointer_dereference` (K7's `check_restore`, halt-terminal, 0 V). The other 74 head-only functions are 1b functions or kernel/shared functions with 0 V sites. Rule 4: `irq_handler_el1`, `timer_tick_handler`, `drain_logs`, `sched::scheduler::timer_tick`, `schedule`, `unblock`, `check_timeouts`, `try_load_balance` and `check_notification_timeouts` have 0 V sites in both ELFs. `wake_with_error` and `irq_frame_check` are symbols only in head, with 0. `check_preemption` has no symbol in either ELF, because it is inlined into `irq_handler_el1`. The `blr`/`br` sites in R are `restore_context`'s `br x1` and five in `core::fmt::write`, and all are in `main` too. No `blr`/`br` sits on a 1b line anywhere in the ELF. **No contamination, so no code change.**
- V1 (result vs `b07d7e4`): 16 rule-1 "new" sites, all in `LogMessageBuf::new`/`::entries` (`shared/src/observability.rs:265`, `:177`). That code came with #219 on `main`, and the 1b diff does not touch it. At `b07d7e4`, `log_impl`'s 3 V sites fill the same role. Rules 2 and 3 give the same results as against `1a5c363`. Comparing with a pre-#219 baseline therefore attributes #219's code to 1b, which is what MergeR4 predicted.
- V1 (H5 verdict input, `--debug-inlined-funcs=ascii`): on this toolchain at opt-level 1, none of `check_timeouts`, `ipc_call`, `ipc_recv`, `ipc_send`, `gpu_release_test_frame` (with `remove_buffer` inlined) or `MessageRing::push`/`pop` has a V-register instruction of its own. `check_timeouts`' 1 KiB zero-fill is a scalar `str xzr` loop. The `GpuBufferHandle` take (`virtio_gpu.rs:975`) and the `RawMessage` copies call `memcpy`, which has 0 V sites. The `RawMessage` zero-inits (`channel.rs:62`, `:485` in head) call `memset`, which uses V registers only on misaligned or short-tail paths. The ADR's expected H5 sites are therefore refuted for the zero-fill and the copies, and the live IRQ-path V use is `LogMessageBuf` (Issues Encountered, V1). Listings are in `target/dig/v1/`: `vreg-by-symbol.txt`, the per-site TSVs `sites-*-vs-head.tsv`, and `h5-*.txt`. `aes::backends::aarch64_aes` and `polyval` are not in R in either ELF.
- D1: the ADR gets one "Amendment and errata, 2026-10-09" note at the top of "Review notes", as the 2026-09-24 amendment did, plus four edits in the body: an intro pointer, a "#200" row after step 1b in "Order and why" (the capability-lifetime ADR leaves placing the step to D1), a bullet on its place, and "Costs" at 11 PRs and about 14–15 soak hours (seven pairs). The row moves every later ADR line down by one, which the note states; the body text keeps its e98e1ad citations.
- D1 (deviation): the V-register erratum states what V1 did, `nightly-2026-10-09` (`a30aa9064`) against `main` `1a5c363`, not §3's "nightly-2026-09-24 (`6eeff9a52`)", which the Merge and MergeR4 decisions superseded. It also records V1's H5 input, because the ADR's step 1b bullet says the H5 verdict is updated from the listing.
- D1 (deviation): the `channel.rs` erratum is a mapping from the ADR's e98e1ad lines to aa1f128's (#175 moved them), not a correction: at e98e1ad the ADR's lines are right.
- D1: the H1 rule ("`elrmm` = 0 in all 30 boots") is recorded as not applicable, with K4's two candidate restatements, and left to the owner before the A/B soak is read; whether F4 path 3 gets its own key is left to a later schema. Neither is decided here.
- D1: Merge219's open question, whether `tripwire::bump`'s body becomes `with_irqs_masked(...)`: no. It would change IRQ-path code after V1 gated it, so V1 and the boot gate would have to run again, for no change in behaviour; `bump` already saves and restores the mask the same way.
- D1 (scope): beyond §3's list, the "For D1" items from K5b and K5b (review 1) (rule 01's boot order gains the `TPIDR_EL1` write; the shared-crate ADR's `CpuView` says the CPU id comes from TPIDR_EL1), and two docs that described the boot and drain paths 1b changed: `docs/kernel/boot/kernel.md` (boot.S step 8 and the secondary sequence set `TPIDR_EL1`) and `observability.md` §2.7 (the panic drain). `.claude/CLAUDE.md` also gains a #200 fact next to the timer PPI, since the tick-based gates depend on it. `observability.md` §6.5 gains the `panic`/`exc` rows, a key-group table (with the `lkoxp` reading from K5 and the `elrmm` caveat from K4), the re-entry panic and lock-word decoding, the self-test feature and the measured line cost (K2's `twmax` caveat).
- D1 (review 1): two fixes. First, the ADR's `soak-qemu.sh` drift erratum: #192's first hunk (`@@ -117,8 +117,11`) shifts every line from old `:126` on, so "lines before `:185` did not move" was wrong. It now reads "before `:126`", which keeps the ADR's `:21`, `:35-39` and `:67-72`. Old `:598` (`esp=...`) is new `:614`, not `:613` (`:613` is the `mktemp` line, old `:597`), so the erratum and §3's D1 list now give `:614-631`. Second, `observability.md` §6.5 now uses this branch's boots for the `g1` line cost: 942–950 bytes, `twmax` on the first `hb` line after Gate 1 between 195,313 and 328,000. Over all saved boots, including K2's 922-byte lines, the range is 172,562–330,125. The doc gives "about 940 bytes, 190,000–330,000 ticks (3.0–5.3 ms)". The reviewer's suggested lower bound of 220,000 is not used, because two of this branch's text boots measure 195,313 and 239,375 (`target/soak/20261009-220356-text`, `-220609-text`). The plan's K2 entries stay as K2's own record.

## Lessons Learned

(to be filled during implementation)
