---
author: jl + claude
date: 2026-09-24
tags: [kernel, security, ipc, memory, sched]
status: in-progress
---

# Plan: capability lifetime implementation

This is the working plan for the one implementation PR of [the capability-lifetime ADR](../decisions/2026-09-24-jl-capability-lifetime.md) ("the ADR"). The ADR holds the decisions and the design. This plan holds the detail the PR works from: per-site change lists, evidence tables, per-step checks, the test plan, the crash-fix interplay, the review log and the evidence limits. It is ephemeral and never reaches `main`: it is the first commit of `claude/cap-lifetime` (rule 04 step 4), and before the implementation PR is marked ready, step 16 distils its lessons and decisions (§10) and deletes it (rule 04 step 11, rule 08). `just docs-check` reports any file here as `plans-not-empty` until then.

- "Crash-fix" means [the crash-fix ADR](../decisions/2026-09-22-jl-crash-fix-preemption-and-fp.md). A bare `:NNN` after a crash-fix reference is a line in that file.
- "The first draft" means the version of the ADR that the owner's first-set answers (2026-09-24, #185 and #189) answered. "The 1,084-line draft" means the in-progress version that the second-set answers (2026-09-24, #185) answered. Neither was committed.
- `shared/`, `docs/`, `scripts/` and `CLAUDE.md` paths are from the repository root; other code paths are relative to `kernel/src/`, and a bare file name is unique there (`test_app.rs` is on PR #149's branch). Line numbers are for b640789. `main` is now b07d7e4: #193 (toolchain) and #194 (single comment lines replaced in place in 30 `kernel/` and `shared/` files) moved no cited line. Nothing in this plan was built, tested or booted.
- Step numbers are the ADR's. The 1,084-line draft used 1, 2, 3, 3b, 4, 4b, 5, 6, 7, 8 and 9, and the wake-token design used 0 and 8b–8e. The map: 3b → 4, 4 → 5, 4b → 6, 5 → 7, 6 → 8, 7 → 9, 8 → 10, 8b → 11, 8c → 12, 8d → 13, 8e → 14, 9 → 15. Step 0 is dropped (§6.6). Step 16 (distil and delete) is new.

-----

## Progress

- [ ] Plan committed as the first commit of `claude/cap-lifetime`; branch rebased onto `main` after crash-fix steps 1a and 1b merge
- [ ] Step 1: id layout and API
- [ ] Step 2: per-slot generations and stale-id tests
- [ ] Step 3: capability primitives and attenuation
- [ ] Step 4: pin the ipc-timeout thread
- [ ] Step 5: derived minting
- [ ] Step 6: region cascade, borrows and deferred free
- [ ] Step 7: delegation and object-bound checks
- [ ] Step 8: Dead is terminal
- [ ] Step 9: the wake token
- [ ] Step 10: process lifecycle
- [ ] Step 11: `notification_wait`, `ipc_select` and their wakers
- [ ] Step 12: `sleep_ticks`
- [ ] Step 13: `ipc_recv`
- [ ] Step 14: `ipc_call`, `ipc_reply` and `ipc_cancel`
- [ ] Step 15: docs
- [ ] Soak pair (§5.3), counts recorded in the PR body and on #164
- [ ] Step 16: distil and delete this plan

-----

## 1. Conventions and shared checks

- Branch `claude/cap-lifetime`, in a worktree created from `main` once the docs PR that carries the ADR and the crash-fix amendment has merged. Its first commit adds this plan. Step 1 starts after crash-fix steps 1a and 1b have merged and the branch has been rebased onto `main`. One commit per step, named `Cap lifetime step N: <description>`, pushed once the step's checks pass.
- Every commit passes `just check` (zero warnings) and `just test`.
- **Boot checks** use `just run` and grep the UART log. A single boot on `main` can wedge (run 167: 36 of 60 boots printed `=== Gate 1 Complete ===`), so per-step boot checks grep only self-test lines, which print before the bench. A boot that wedges before them is re-run and the wedge is noted. A boot that logs `slot moved` is also re-run for its step's acceptance. Gate 1 is covered by the soak pair.
- Patterns that use `+` or alternation are run with `grep -E`.
- **Boot logs.** Each step saves its acceptance boot log as `target/cap-lifetime/step-N.log` (`target/` is gitignored); gpu-mode logs as `step-N-gpu.log`. The commands below use `LOG=target/cap-lifetime/step-N.log`.
- **No new kwarn.** Each step's normalised warning set must be a subset of the previous step's set plus an allowlist: `denied ChannelCreate` (not deterministic), `slot moved`, and any line the step's acceptance names as new. Lines the acceptance names as removed must be gone. The baseline is a boot of the PR's base commit; in recent boot logs that set is `denied ChannelAccess`, the two `unexpected result -N` lines and `No DTB`.

```bash
grep -a ' WARN ' "$LOG" | sed -E 's/^\[[ 0-9.]+\] \[[0-9]\] //; s/[0-9]+/N/g' | sort -u
```

- **Timeout-last check.** The Timeout line must come after every other ipc-timeout test line. The log carries a CPU tag but no thread tag, so line order is the only check available. Each step that adds a line adds its name to the pattern. The full pattern after step 14:

```bash
awk '/Timeout test:/ {t = NR} /(Destroy|Bad-id|Bad-pid|Select-cap|Stale-id|Stale-shm|Shm-cascade|Create-ABI|Share|Dead|Wake-token|Notify-loop|Select-loop|Sleep-loop|Recv-loop|Call-loop) test:|Lifecycle:/ {o = NR} END {exit !(t > o)}' "$LOG"
```

- **DAIF count.** `rg -c 'msr DAIFClr' kernel/src/sched/scheduler.rs kernel/src/ipc/direct.rs`, read per file, because rg does not print files in argument order: `scheduler.rs:7` and `direct.rs:8` at b640789, `scheduler.rs:9` after step 8, `scheduler.rs:10` after step 10 and `scheduler.rs:11` from step 11 on. `direct.rs` stays at 8 throughout.

-----

## 2. Evidence tables (b640789)

### 2.1 Sites that build or index ids from slot numbers

| Site | Today | After |
| --- | --- | --- |
| `ipc/mod.rs:191` | `ChannelId(idx as u32)` | `ChannelId::new(slot, generation)` |
| `ipc/tests/mod.rs:308` (`channel_create_unchecked`) | same | the allocator moves to `ipc/mod.rs`, shared with `channel_create` |
| `cap/mod.rs:382` (`revoke_channels_for_cap`) | `ChannelId(i as u32)` | replaced by the channel cascade, which uses the stored `ch.id` |
| `shmem.rs:145`, `:165` | `SharedMemoryId(idx as u32)`, `SharedMemoryAccess(idx as u32)` | `new()`, full id |
| `shmem.rs:260` | VA from `region_id.0` | VA from `index()`; the raw value would put the VA far outside the window |
| `shmem.rs:586` (`memory_unmap`) | `SharedMemoryId(region_idx)` | the slot's stored id, read under the lock |
| `shmem.rs:206`, `:219`, `:305`, `:310`, `:356`, `:410`, `:423`, `:624`, `:628`, `:636`, `:640` | raw `.0 as usize` | `region_mut` / `index()` |
| `ipc/mod.rs:469-475` (Kit `shmem_destroy`) | raw index; the bound check at `:469` would reject every id that carries a generation | `region_mut` |
| `select.rs:191`, `:231` | drops `channel_mut` errors | propagates them |
| `compositor/service.rs:793` (`handle_attach_buffer`) | `SharedMemoryId(req.shmem_id)` from the client's message | `from_raw()`, plus the access check |

`SharedMemoryAccess(SharedMemoryId)` also touches `shared/src/cap.rs:47`, `:83-85`; `cap/mod.rs:137` (`check_shared_memory_access` takes a `u32`) and `:144`; `syscall/mod.rs:346-349`; `shmem.rs:165`, `:443`; and `bad_pid.rs:84`, `:144`.

### 2.2 In-kernel holders of a region's direct-map address

From `rg 'region_dmap_addr|region_size|\.base_phys|DIRECT_MAP_BASE \+' kernel/src`:

| Holder | What it does | Exposure |
| --- | --- | --- |
| `bench_shmem_throughput` (`bench.rs:280-341`, pid 8) | `region_dmap_addr` (`:297`), then writes and reads 256 KiB (`SHM_SIZE`, `:31`) with no lock held, then unmaps (`:334`) | The only caller of `region_dmap_addr`. Its mapping keeps `ref_count` above 0, but a cascade would free the frames during the loops. Nothing at boot revokes pid 8's `SharedMemoryCreate` (`:482`) or exits pid 8 |
| `shared_memory_create` (`shmem.rs:128-133`) | Zeroes the frames through the direct map | Before the region is published; not a holder |
| Compositor (`surface.rs:67`, `:184`; `service.rs:793`) | Stores the `SharedMemoryId` only | No address. PR #149's `resolve_surface_pixels` is a holder |
| `region_size` (`shmem.rs:635-641`) | Size lookup | No caller |

No driver, GPU or storage code touches region frames. Their `DIRECT_MAP_BASE +` sites use pages the drivers own.

### 2.3 State writers gated by `may_become` (step 8)

| Site | Today | After |
| --- | --- | --- |
| `thread_yield`, `scheduler.rs:309` | writes Running unconditionally | skips the write if Dead; still calls `schedule()` |
| `block_current`, `:338` | writes `new_state` unconditionally | skips the write if Dead; from step 9 consumes a pending token instead of blocking |
| `unblock`, `:376` | returns early only for Running/Runnable | returns early for Dead; from step 9 leaves a token for an Armed Running/Runnable target |
| `schedule()` pick, `:210`, `:236` | sets Running with no check | re-picks while the picked thread is Dead, dropping it from the queue |
| `enter_scheduler`, `:82` | same | same filter |
| `try_direct_switch`, `ipc/direct.rs:142` | overwrites the current sender with BlockedIpc (later Running at `:266`) | returns false (slow path) if the current thread is Dead; from step 9 also if it holds a token |
| `try_reply_switch`, `ipc/direct.rs:275`, `:290` | makes the replier Runnable and enqueues it | skips both if Dead |

The owner named the first three. The others revive a Dead thread the same way: a thread killed while queued would be picked and set Running. Both direct switches already refuse a target that is not `BlockedIpc` (`direct.rs:84-94`, `:231-240`), so a Dead target takes the slow path.

### 2.4 Register-then-block sites today

None of them re-checks its condition in a loop.

| Site | Today | Converted in |
| --- | --- | --- |
| `process_wait` (`process.rs:233-268`) | One read of `EXIT_CODES`, EPERM on a spurious return (`:262-267`). Registers (`:251-254`), drops the lock, then blocks (`:257-259`) | step 10 |
| `notification_wait` (`notify.rs:159-255`) | A spurious return gives ETIMEDOUT (`:249-253`). The deadline is set (`:231`) after the waiter record (`:226`) | step 11 |
| `ipc_select` (`select.rs:52-148`) | `ready_index == None` gives ETIMEDOUT (`:144-147`). Publishes `SELECT_WAITERS` (`:104-117`), registers (`:120`), then sets the deadline (`:123`) | step 11 |
| `sleep_ticks` (`timeout.rs:167-185`) | A spurious return sleeps short. Blocks as `BlockedIpc { channel: u64::MAX }` (`:184`) | step 12 |
| `ipc_recv` (`channel.rs:232-336`) | EAGAIN on a wake with no message ("shouldn't happen", `:332-335`). The timeout (`:279-287`) comes after `waiting_receiver` (`:275`) | step 13 |
| `ipc_call` (`channel.rs:33-219`) | A spurious return reads `bytes_written == 0` as success and leaves `pending_caller` set. The reply slot and the timeout (`:106-126`, `:149-168`) come after `pending_caller` (`:91`), so a fast replier finds no slot and the reply is lost | step 14 |

### 2.5 Locks and IRQ state

- The crash-fix IRQ-shared set (`:213`) is 9 statics. The IRQ path blocking-locks `THREAD_TABLE`, `CURRENT_THREAD`, `RUN_QUEUES` and `WAKEUP_ERRORS`, and try-locks `TIMEOUT_QUEUE`, `NOTIFY_DEADLINES`, `NOTIFICATION_TABLE`, `SELECT_WAITERS` and `BOOT_LOG`. After step 11 it is 7: `NOTIFICATION_TABLE` leaves IRQ context and `SELECT_WAITERS` is deleted.
- Crash-fix `:219`: `NOTIFY_DEADLINES` is held across `THREAD_TABLE`, `NOTIFICATION_TABLE`, `SELECT_WAITERS` and `unblock`'s locks (`notify.rs:310-349`). Step 11 removes this nesting.
- 41 inline `DAIFSet`/`DAIFClr` sites in 10 files (crash-fix `:242`, confirmed by `rg -c 'msr DAIF(Set|Clr)'` at b07d7e4). Steps 8, 10 and 11 add eight, all in `scheduler.rs`, two per helper: `kill_process_threads` and `thread_probe` (step 8), `current_thread_and_pid` (step 10), `current_tid` (step 11).
- `current_thread_id()` (`timeout.rs:116-118`) reads `core_id()` and then locks `CURRENT_THREAD[cpu]` with IRQs on (crash-fix N4). Every blocking site gets its tid from it today (`channel.rs:44`, `:237`; `select.rs:56`; `notify.rs:166`; `timeout.rs:171`; `syscall/mod.rs:681`).
- `clear_timeout` (`timeout.rs:154-158`) uses `TIMEOUT_QUEUE.try_lock()` and does nothing when the lock is contended. `check_timeouts` try-locks it too (`timeout.rs:82`); every other site takes it with a blocking `lock()` in thread context (`channel.rs:120`, `:162`, `:183`, `:281`, `:296`; `timeout.rs:177`).
- EL0 SVCs reach `syscall_dispatch` with IRQs masked from exception entry (`trap.rs:70-71`; neither `trap.rs` nor `exceptions.rs` unmasks). The self-tests call `syscall_dispatch` from threads with IRQs on (`bad_pid.rs:55-67`).
- `CLAUDE.md:98` and `docs/kernel/deadlock-prevention.md` §3.3 already allow `PROCESS_TABLE → CHANNEL_TABLE` and `PROCESS_TABLE → SHARED_REGION_TABLE`; no code nests them today.

### 2.6 Run-167 facts used

- 60 boots of `main` and #161 at nightly-2026-09-22, before the Bad-id, Bad-pid and select_cap tests existed.
- In 60 of 60 boots the echo server never logged `started`, and the echo client logged nothing after `found service`.
- `denied ChannelCreate` appeared in 0 of 60 boots (pid 3's thread is Normal-class on CPU 0). `=== Gate 1 Complete ===` appeared in 36 of 60.
- In the one boot sampled, the Timeout test started at 5.890 s, 2 ms after `Bench threads created`. From about 5.9 s CPU 0 is held by the Interactive bench threads, and `bench-yield` loops forever (`bench.rs:505-524`, crash-fix N10). The echo client's timed-out call starves there in every boot.

-----

## 3. Per-step change lists and checks

### Step 1: id layout and API

- `shared/src/ipc.rs`: the layout constants with strict asserts, `next_generation`, private tuple fields, and `new`/`from_raw`/`raw`/`index`/`generation` on `ChannelId` and `SharedMemoryId`. `SharedMemoryId` gains the `index()` it lacks. `Capability::SharedMemoryAccess(SharedMemoryId)` (§2.1's second paragraph).
- Kernel: every id construction goes through the new API at `ID_GEN_FIRST` (§2.1), including `compositor/service.rs:793` (`from_raw`). Every index goes through `index()`. `revoke_channels_for_cap` and `memory_unmap` use the stored id. Logs print `slot.generation`. No per-slot state yet.
- **Checks.** `just test`: round-trip at generation 1 and at `ID_GEN_MAX`; `index()` is `None` for slots 128–255 (channel) and 64–255 (region) at any generation, and for `u32::MAX`; the same slot with a different generation compares unequal; `next_generation` goes to `RETIRED` and stays there; the #175 tests pass after the `raw()` rename. `just run`: `Bad-id test: EINVAL as expected`, `Bad-pid test: EINVAL as expected`, `Select-cap test: EPERM/EINVAL as expected`; `grep -c 'denied ChannelAccess'` is 6; `grep -Ec 'Channel [0-9]+\.1 created'` is at least 1.

### Step 2: per-slot generations and stale-id tests

- `ChannelSlot` (`ipc/mod.rs:135`) and `RegionSlot` (`shmem.rs:81`). The identity check goes into `channel_mut`, into `channel_destroy_unchecked` before its `take()` (`ipc/mod.rs:222-224`), and into a new `region_mut`. Bump and retire on free. Region VA from the slot. The Kit `shmem_destroy` goes through `region_mut`. Share checks `region.id` and grants the full id. `channel_create_unchecked` moves into `ipc/mod.rs`. `Channel.id` loses its `#[allow(dead_code)]` (`ipc/mod.rs:94`). `region_dmap_addr` and `region_size` return `None` on a mismatch until step 6 replaces them.
- **Why the destroy check matters.** `check_channel_access` passes a stale id when the caller holds the matching stale token. Without the check before `take()`, a stale destroy would tear down the channel now in that slot.
- **Select rollback.** `scan_entries` and `register_on_sources` (`select.rs:185-197`, `:225-236`) propagate lookup errors, and the scan returns its error before anything is registered. `ipc_select` publishes `SELECT_WAITERS[my_tid]` (`:104-117`) before registering (`:120`) and sets the deadline after both (`:123`). A registration failure therefore unregisters the sources already registered (`unregister_from_sources`, `:255`), sets `SELECT_WAITERS[my_tid] = None` and skips `set_select_deadline`, so `set_select_ready` (`channel.rs:103`) cannot write into a stale record. Step 11 replaces this code.
- **New `ipc/tests/stale_id.rs`**, run by the ipc-timeout thread (pid 1).
  - **Channel case.** `channel_create` mints nothing yet, so channels come from `channel_create_unchecked` plus a self-test `grant_to_process(ChannelAccess(id))` per id (the broker exemption; the `select_cap.rs:49-58` pattern). Create A, destroy A, create B. If `B.index() != A.index()`, another creator took the slot (the bench's creates, and from step 10 pid 14's probe): destroy B and create again, up to 8 attempts in all. If none reuses the slot, log `Stale-id test: slot moved` and stop. With A, `ipc_recv`, `ipc_send`, `ipc_reply`, `ipc_select` and `channel_destroy` return EPIPE, and B still answers `ipc_reply` with EPROTO. Destroy B and revoke every grant through `revoke_in_process`.
  - **Shm case.** On entry, check that pid 1 holds no live `SharedMemoryCreate`. Grant a temporary `SharedMemoryCreate` T. Create R1, map, unmap (which frees R1; its token stays). Create R2, retrying as for channels (a wrong-slot R2 is freed by map and unmap). Assert the same slot and a different id. `map(R1)` and `share(R1, self)` return EPIPE, and `map(R2)` succeeds. Unmap R2. Revoke the temporary tokens through `revoke_in_process`.
  - Allocation is lowest-first, so a destroy and a create in one thread usually reuse the slot. It is not guaranteed on SMP, which is why the tests assert the reuse and retry.
- **Checks.** `just run`: `Stale-id test: EPIPE as expected`, `Stale-shm test: EPIPE as expected`; step-1 lines unchanged; `denied ChannelAccess` still 6; no new kwarn.

### Step 3: capability primitives and attenuation

- `shared/src/cap.rs`: delete `:110`, `:112` and `:114` and rewrite the doc comment at `:102-106`. `attenuate` takes `now_tick`: it finds the parent through `get()` (`:186-192`), which skips revoked tokens but not expired ones, so it now returns EPERM for an expired parent, and sets the child's expiry to the earlier of the parent's and the requested one (`None` only when both are `None`). Add `mint_child`, `free_slots`, `is_revoked` (true for a missing id), `revoke_all` (one pass, no stack array; `revoke()` already puts 4 KiB on the stack at `:226`) and `clear`.
- Kernel: `sys_capability_attenuate` (`x2 == 0` means no expiry, `syscall/mod.rs:329`) and the Kit `attenuate` (`cap/mod.rs:330-344`) pass `now`. `KernelCapabilitySystem::grant` (`cap/mod.rs:259-290`) refuses the object-bound variants with `NotGranted`; today it mints a parentless, delegatable token for any capability and ignores `granted_by` (`:284`). Both Kit impls are `#[allow(dead_code)]`.
- Host tests: `:427-431`, `:434-436` and `:1031-1033` become denial tests. `:791-812` (`attenuate_channel_create_to_access_ok`) and `:843-862` (`attenuate_with_expiry`) would panic after the change and are rewritten on `ChannelAccess(x) → ChannelAccess(x)`. `:863-880` moves to an identity pair, so it still tests the revoked-parent path.
- **Checks.** `just test`: the three denial tests; an SVC-15-shaped `attenuate(ChannelCreate handle → ChannelAccess)` returns EPERM; identity attenuation with expiry passes; the child's expiry is the earlier of parent and request, and a `None` request under an expiring parent gets the parent's; an expired parent gives EPERM; `mint_child` sets parent, delegatable and expiry; revoking the parent revokes a child and a grandchild; a revoked, expired or missing parent gives EPERM; a full table gives ENOSPC; `free_slots` counts revoked tokens as occupied; the `revoke_all` count; `clear` leaves an empty table; `is_revoked(missing)` is true, `is_revoked(live)` false, and `is_revoked(child)` true after its parent is revoked. `just run`: step-2 lines unchanged.

### Step 4: pin the ipc-timeout thread

- The ipc-timeout thread is pinned to CPU 2 (`CpuSet::single(2)`) from creation, and the Timeout test moves to the end of `ipc_timeout_entry`. The call is still denied, so nothing blocks yet.
- **Why.** From step 5 the Timeout test really blocks for 100 ticks, and its wake comes from `check_timeouts` in timer-IRQ context. CPU 0 advances `TICK_COUNT` and runs `check_timeouts` in the same handler (`timer.rs:176-194`), so CPU 0 usually sees the expiry first, and `unblock` queues the woken thread on the waking CPU when affinity allows (`scheduler.rs:398-404`). Unpinned, the thread would starve on CPU 0 behind the bench (§2.6). Pinned, `unblock` takes its "first allowed CPU" branch. The result is one IRQ-context wake from another CPU per boot, on the crash-fix H3 lock path. It is not in N7's class: the thread cannot migrate, and CPU 2 is its last CPU. Moving the test last means a late wake can hide only its own line.
- **Checks.** `just run`: all step-3 lines unchanged; `Timeout test: unexpected result -6` still prints and passes the Timeout-last check; `denied ChannelAccess` still 6; no new kwarn.

### Step 5: derived minting

- `cap::create_with_access` (ADR §2). `channel_create` and `shared_memory_create` mint through it; `publish` never frees pages (today's slot search frees them itself, `shmem.rs:137-142`, so moving it unchanged would free twice). The region `publish` records the token found under the hold as `creation_cap: CapabilityTokenId`, now non-Option (`shmem.rs:68`, `:154`), and the pre-check's token id is discarded.
- SVC 6 and 20 return the handle in x1; `sys_channel_create` (`syscall/mod.rs:279`; dispatch at `:37`) and `sys_shared_memory_create` (`:488`; dispatch at `:50`) take `&mut TrapFrame`. On error x1 is not written, as for `IpcRecv` (`syscall/mod.rs:237-242`).
- `shared_memory_create`'s callers: `syscall/mod.rs:498`, the Kit `shmem_create` (`ipc/mod.rs:439`), `bench.rs:285` and `bad_pid.rs:45`. Kit signatures: `ChannelOps::channel_create` (`shared/src/kits/ipc.rs:113-114`), `SharedMemoryOps::shmem_create` (`:168`), and `KernelIpc`.
- The bench (`bench.rs:365-375`) switches to `channel_create` in its pid-8 thread, which holds `ChannelCreate` (`bench.rs:481`), and drops its ambient grant.
- The channel cascade in `revoke_in_process` (`cap/mod.rs:193-212`, now through `process_mut`) and the Kit `revoke` (`:291-328`), replacing `revoke_channels_for_cap`. The Kit `revoke`'s comment that `PROCESS_TABLE` must be dropped before `CHANNEL_TABLE` (`:321-323`) misstates the order and goes. The `grant_to_process` rule comment; the select_cap comment at `:21-25`.
- The stale-id channel case switches to `channel_create` and the minted handles.
- **New `ipc/tests/create_abi.rs`** drives `syscall_dispatch` with a synthetic `TrapFrame` (the `bad_pid.rs:55-67` pattern). `ChannelCreate`: x0 ≥ 256, and x1 names a live `ChannelAccess(x0)` in pid 1's table whose `parent_token` is pid 1's `ChannelCreate` token. `CapabilityAttenuate` type 1 from the `ChannelCreate` handle returns EPERM. The channel is then destroyed. The same for `SharedMemoryCreate` under a temporary grant: on entry pid 1 holds no live `SharedMemoryCreate`; the region's `creation_cap` equals both the granted token and the `parent_token` of the token that x1 names; the region is freed by map and unmap; the temporary grant is revoked through `revoke_in_process` before the test returns.
- **Checks.** `just run`: `Create-ABI test: x0/x1 as expected`, `Destroy test: EPIPE as expected`, `Bad-pid test: EINVAL as expected`, both stale lines, `Select-cap test: EPERM/EINVAL as expected`, and `Timeout test: ETIMEDOUT as expected`, which passes the Timeout-last check; `grep -c 'denied ChannelAccess'` is 3; no `unexpected result`; no new kwarn.

### Step 6: region cascade, borrows and deferred free

- `RegionSlot.draining` (`DrainingFrames`) and `SharedMemoryRegion.kernel_borrows`. `region_detach` becomes the single free function, used by the last unmap (`shmem.rs:353-357`) and `process_cleanup_shared_memory` (`:489-494`); frames are freed after the lock, as today (`:366-369`, `:502-505`).
- `RegionBorrow`, `region_borrow`, `region_release` and `is_live`; `region_dmap_addr` and `region_size` (`shmem.rs:623-641`) are deleted. The bench becomes map → borrow → loops → release → unmap, and its SAFETY comment (`bench.rs:307-308`) cites the borrow. A mismatch in `region_release` means a double release and panics. A missed release leaks the frames and keeps the slot out of use for the boot; that is a leak, not a use-after-free.
- `revoke_regions_for_cap` runs in `revoke_in_process` and the Kit `revoke`, in the same `PROCESS_TABLE` hold as step 5's channel collection. The lock sequence: under `PROCESS_TABLE`, `process_mut(pid)` then `cap_table.revoke(token)`; `SHARED_REGION_TABLE` nested for the region cascade, released; `CHANNEL_TABLE` nested for the channel collection, released; release `PROCESS_TABLE`; then, with no lock, free the collected frames, destroy the collected channels, and log. `FreeList` is `MAX_SHARED_REGIONS` `(phys, order)` pairs plus a count: 1 KiB, the same array `process_cleanup_shared_memory` uses (`shmem.rs:468`).
- **Logs** (each under 48 bytes): `shm_cascade: pid=N destroyed K regions`, printed only when K > 0; `shm: region S.G draining (B borrows)`; `shm: region S.G drained`; `shm_destroy: region S.G` (step 7).
- **Test isolation.** `find_authorizing_token` returns the first live match (`shared/src/cap.rs:202-216`), and the region records it as `creation_cap`. A temporary `SharedMemoryCreate` left live by an earlier test would be recorded instead of the current test's token, and revoking the test's token would destroy nothing. So from this step, every self-test grant is revoked through `revoke_in_process` before the test returns, and each shm test checks on entry that pid 1 holds no live `SharedMemoryCreate` (otherwise it logs a kwarn and stops) and, after each create, that `creation_cap` equals the token it granted.
- `bad_pid.rs`: the on-entry check; the create-token revoke moves after the shares and the map and goes through `revoke_in_process`, which destroys the region; `revoke_token` and the trailing `shared_memory_unmap` are removed; `revoke_region_access` stays for now, because it revokes the parentless self-share token; the comment "because region ids are reused" (`:22-27`) becomes "Revoking the create token destroys the region: no token or region outlives the test."
- The stale-shm case's final revoke goes through `revoke_in_process`.
- **New Shm-cascade case** in `ipc/tests/stale_id.rs` (pid 1). Grant a temporary `SharedMemoryCreate` T. Create R, map it, and borrow it as b. `revoke_in_process(1, T)`. Expect `shared_memory_unmap(1, R)` to return EPIPE and `b.is_live()` to be false. Grant T2, create R2, and expect `R2.index() != R.index()`; this holds on SMP, because no allocator takes a draining slot. `region_release(b)` logs `drained`. `revoke_in_process(1, T2)` destroys the never-mapped R2. Logs `Shm-cascade test: EPIPE as expected`.
- **Checks.** `just check`; `just test`; `rg -n -e region_dmap_addr -e region_size kernel/src` prints nothing. `just run`: `Shm-cascade test: EPIPE as expected`, `Bad-pid test: EINVAL as expected`, all step-5 lines unchanged; `grep -c 'shm_cascade: pid=1 destroyed 1'` is 3; `grep -c 'shm: region [0-9]*\.[0-9]* drained'` is 1; no `unexpected result`; no new kwarn. If the bench line prints, `[bench] Shared memory throughput` shows non-zero write and read.

### Step 7: delegation and object-bound checks

- `shared_memory_share` as delegation (ADR §4) with the pure `share_decision`; `shared_memory_map` checks and inserts in one hold (today it checks, releases and inserts later, `shmem.rs:201-270`); the Kit `shmem_destroy` authorises and calls `region_detach` in one `PROCESS_TABLE → SHARED_REGION_TABLE` hold, replacing the snapshot and per-pid unmap loop (`ipc/mod.rs:464-503`), and frees the frames after the release; the compositor attach check.
- Without the one-hold map, a thread of an exiting process still running on another CPU could insert a mapping after exit step D. The region's `ref_count` would then never reach 0.
- `bad_pid.rs`: the create grant (`:38`) becomes delegatable, so the minted access can be shared and `last_slot`'s EPERM still comes from the empty target slot; `revoke_region_access` is deleted (the self-share is idempotent and mints nothing).
- The stale-shm case changes its end: `revoke_in_process(T)` while R2 is mapped, then `share(R2, self)` returns EPERM (the token check comes before the region) and `shared_memory_unmap(R2)` returns EPIPE (the cascade freed R2).
- **New `ipc/tests/share.rs`** (pid 1, entry check as in step 6):
  - A non-delegatable temporary `SharedMemoryCreate` Tn and region Rn: `share(Rn, pid 2)` returns EPERM and `share(Rn, pid 1)` returns Ok. `revoke_in_process(1, Tn)` destroys Rn.
  - A delegatable Td and region Rd: `share(Rd, pid 2)` returns Ok twice, and pid 2 holds exactly one live `SharedMemoryAccess(Rd)`. `KernelIpc::shmem_destroy(Rd)` returns Ok. Pid 2's token and Td are revoked through `revoke_in_process`.
  - A temporary grant T2 to pid 2 and a region R2 created for pid 2: `KernelIpc::shmem_destroy(R2)` from pid 1 returns an error, and `revoke_in_process(2, T2)` destroys R2.
  - The compositor attach check has no boot caller on `main`; PR #149's test app exercises it.
- **Checks.** `just test`: `share_decision` at the reserve boundary (64 and 65 free slots), for the delegatable rule with the target equal to and different from the caller, and for idempotence. `just run`: `Share test: delegation as expected`, `Bad-pid test: EINVAL as expected`, `Stale-shm test: EPIPE as expected`; all step-6 lines unchanged except that `grep -c 'shm_cascade: pid=1 destroyed 1'` is 5; `grep -c 'shm_cascade: pid=2 destroyed 1'` is 1; `grep -c 'shm_destroy: region'` is 1; `rg -n 'cap_table\.revoke\(' kernel/src` lists only `cap/mod.rs`; no new kwarn; in gpu mode `display handoff complete` still prints.

### Step 8: Dead is terminal

- `ThreadState::may_become` with host tests over every state pair; the guards of §2.3; `exit_current`; the three self-Dead loops replaced.
- The existing `process_exit` walk (`process.rs:136-143`) moves into `sched::kill_process_threads`, which skips the calling thread, resolved with IRQs masked. It reads the caller from `CURRENT_THREAD[core_id()]` after masking, as `thread_yield` does (`scheduler.rs:296-305`): `current_thread_id()` reads `core_id` with IRQs on (`ipc/timeout.rs:116-118`, crash-fix N4), and a migration between its two reads would name another CPU's thread. Without the exclusion, the guards alone would make a self-exit permanent.
- `sched::thread_probe(tid, do_unblock)`: in one IRQ-masked section, optionally `unblock(tid)`, read the state under `THREAD_TABLE`, and check every online CPU's `RUN_QUEUES` with a new `RunQueue::contains`. Masking keeps a tick on this CPU from picking and dropping the thread between the `unblock` and the check.
- `exit_current()` calls `block_current(ThreadState::Dead)` and panics if that returns; it cannot, because every CPU has an idle thread (`sched/init.rs:29-32`). It replaces `service/mod.rs:314-325`, `gpu/service.rs:177-188` and `compositor/service.rs:186-197`, which `thread_yield` revives on every call today. They are reached only after EPIPE on the echo, GPU and compositor channels, which nothing destroys at boot (the exit walk skips synthetic owner tids, `process.rs:166-182`).
- **Echo server** (`service/mod.rs:300-311`): its receive loop ends on EPIPE (the channel is gone) or EPERM (its own process has exited) and then calls `exit_current()`. ETIMEDOUT continues silently. Any other error logs the existing kwarn and retries after `sched::thread_yield()`; ending there would leave pid 7 Alive with no server thread. The echo client, if it ever reaches its `process_exit(7)` (`service/mod.rs:371`), gets EPERM on its next `ipc_call`, logs `Echo client: EPERM after exit (expected)` and calls `exit_current()`.
- 1b instruments: `unblock`'s counter gains the `Dead` outcome, and deliberate self-test wakes count under a `self_test` caller (§6.2).
- **New `ipc/tests/dead_thread.rs`.** A pid-1 helper thread, created at init and pinned to CPU 2, calls `exit_current()` when it first runs. The ipc-timeout thread polls `thread_probe(tid, false)` until the state is Dead (bounded yields), then `thread_probe(tid, true)` expects Dead and in no queue, yields twice, and `thread_probe(tid, false)` still reads Dead.
- **Checks.** `just test` (the transition table). `just run` in text and gpu mode: `Dead test: terminal as expected`, step-7 lines unchanged, no new kwarn, `display handoff complete` in gpu mode. The DAIF count reports `scheduler.rs:9` and `direct.rs:8`.

### Step 9: the wake token

- `shared/src/sched.rs`: `WaitState`, `WakeAction`, `BlockAction`, `wake_action` and `block_action`, with host tests (§5.1). New `kernel/src/sched/wake.rs`: `WAIT_STATE`, `prepare_block`, `cancel_block`, `wait_state` and `reset`. `unblock`, `block_current` and `try_direct_switch` apply the decisions inside their existing exits; `block_current`'s `AlreadyDead` branch resets the word; `allocate_thread` (`sched/mod.rs:116-125`) resets the word before publishing the thread; `kill_process_threads` resets victims' words. The `prepare_block` doc comment carries the three rules and the own-tid rule (ADR §8): the tid comes from `current_tid()` (step 11), `current_thread_and_pid()` (step 10) or a thread pinned to one CPU, never from `current_thread_id()` on an unpinned thread.
- **Arm assertion order.** `block_current`'s later arm assertion (step 14) is checked after `block_action` has ruled out `AlreadyDead`: exit B resets a victim's word, and a victim that armed before B still reaches `block_current` with an Idle word.
- **Memory ordering, all `Relaxed`.** The protocol needs three things, and none needs a fence. (1) The waker sees Armed: it finds the waiter only through the publish lock, `prepare_block` is sequenced before that lock's release, and the waker reads the word after acquiring it. (2) A token is never lost against a block: the wake CAS and the consume swap both run under `THREAD_TABLE`, each in the critical section of the state check or write that goes with it, so they are totally ordered. (3) The thread's own swaps race safely with wakers: they are read-modify-writes on the same location as the waker's CAS, in one modification order. A future lock-free waker would need Release on the arm and Acquire on the lookup.
- **`unblock`** (`scheduler.rs:358-415`) replaces its early return (`:375-383`) with one decision under the same hold:

| Target state | Word | `WakeAction` | Effect |
| --- | --- | --- | --- |
| Dead | any | `Dead` | Return: no token, no enqueue |
| Running or Runnable | Armed | `LeaveToken` | CAS to Woken; no state change, no enqueue |
| Running or Runnable | Woken | `TokenPending` | Nothing |
| Running or Runnable | Idle | `Skip` | Nothing; the wake is stale |
| any Blocked state, or Suspended | Idle (invariant) | `Enqueue` | Runnable plus enqueue, as today (`:386-407`) |

- Runnable gets a token on purpose: an armed thread preempted by a tick is Runnable and queued (`scheduler.rs:170-177`), and a thread that `try_direct_switch` started is Runnable while it runs (`ipc/direct.rs:135`, crash-fix N1). The Dead row reuses the existing exit (`:376-383`), so no `DAIFClr` site is added.
- **`block_current`** (`:327-348`) applies `block_action` under its existing hold. `AlreadyDead`: write no state, consume nothing, reset the word, call `schedule()`. `next == Dead` (`exit_current`): reset the word and write Dead. `ConsumeToken` (the swap returned Woken): write nothing and do not call `schedule()`. `Sleep` (Armed or Idle): write `new_state` and call `schedule()`. Both leave through the existing exit (`:347`).
- **`try_direct_switch`** returns `false` when the sender is Dead or its word is Woken. The check goes into the receiver check and reuses its exit (`direct.rs:84-95`, exit `:88-93`), before any write, so `direct.rs` keeps its 8 `DAIFClr` sites. The word is only read there; otherwise the swap to Idle happens together with the BlockedIpc write at `:142`. The Woken branch becomes live in step 14. `try_reply_switch` is unchanged: an armed caller fails its BlockedIpc check (`direct.rs:231-243`).
- 1b instruments: the per-caller counter maps `wake_action` onto its outcomes, adding `LeaveToken` and `TokenPending`; `Skip` then means a Running or Runnable target whose word is Idle. The heartbeat scan adds `bw` (§6.2).
- **New `ipc/tests/wake_token.rs`**, run by the ipc-timeout thread before the Timeout test. Deterministic, one thread:
  1. `unblock(me)` without arming leaves `wait_state(me) == Idle`.
  2. `prepare_block(me)`, then `unblock(me)`: the word is Woken. `cancel_block(me)` returns `true` and leaves Idle.
  3. `prepare_block(me)`, `unblock(me)`, then publish a 50-tick safety entry in `TIMEOUT_QUEUE` (ETIMEDOUT), read `t0 = TICK_COUNT`, call `block_current(BlockedIpc { channel: u64::MAX })`, and read `t1` as soon as it returns. The token was consumed if `t1 - t0 < 50` and the word is Idle. A lost token can only come back through the 50-tick entry, which gives `t1 - t0 >= 50`.
  4. Clean up on either path: under `TIMEOUT_QUEUE.lock()`, remove the entry if still present. If it is gone, it fired, and `wake_with_error` (`timeout.rs:130-140`) left `WAKEUP_ERRORS[me] = ETIMEDOUT`; clear it with `get_wakeup_error(me)`.
  - The verdict comes from elapsed ticks, not from whether the entry remains; only a 50-tick preemption between reading `t0` and entering `block_current` can give a false failure. A lost token logs `Wake-token test: token lost`, not a hang. The IRQ side only try-locks `TIMEOUT_QUEUE` (`check_timeouts`, `timeout.rs:82`), so this lock cannot deadlock on the same CPU.
- **Checks.** `just test` passes the named tests, and both negative controls reach their lost-wakeup states. The DAIF count still reports `scheduler.rs:9` and `direct.rs:8`. `just run`: `Wake-token test: early wake as expected`, no `token lost`, all step-8 lines, no new kwarn.

### Step 10: process lifecycle

- `ProcessState` (with `may_become`) and `process_wait_action` in `shared/`.
- **Fixed-slot table.** `ProcessControl` is about 18 KiB, almost all of it the 256-token `CapabilityTable` (72 bytes per token, from the field types). Kernel stacks are 32 KiB with no guard page (`sched/mod.rs:127-148`), and SVC 25 reaches the reap path with a `TrapFrame` already on the stack. `take()` would move the whole block onto the stack, and a debug build can make more than one copy. So every slot holds a `ProcessControl`, const-initialised to `Empty`, and install and reap reset fields in place. There is no `Drop` impl in `kernel/` or `shared/`. `ever_used` is written only under `PROCESS_TABLE`.
- `EXIT_CODES` (`process.rs:109-113`) is removed. `sys_process_exit` passes `tf.x[0] as i32` (`syscall/mod.rs:662`), so today a caller can exit with `i32::MIN` and look as if it never exited.
- Accessors: `process_mut` and `process_ref` (`process.rs:77-95`) reject non-Alive slots through `deny_missing_process` (`cap/mod.rs:62`) with no warning; `process_slot_mut` returns the slot in any state, for lifecycle code only.
- `process_install`: EINVAL out of range; EEXIST if the slot is not Empty or `ever_used` is set; otherwise reset in place (`cap_table.clear()`, `thread_ids`, `name`, `resource_limits`, `address_space = None`, `parent`), clear `PROCESS_WAITERS[idx]`, set `ever_used` and `Alive`. The eight literal slot writes switch to it with `parent: None`: `ipc/tests/mod.rs:54`, `:84`, `:101`, `:118`; `service/mod.rs:193`; `bench.rs:472`; `gpu/service.rs:573`; `compositor/service.rs:563`. The lifecycle self-test adds pids 12–17.
- `process_exit` A–E as in the ADR §6, with these details:
  - The victims are every thread with `owner_pid == pid`, including the caller when it belongs to `pid`. B leaves the caller out only of the Dead write and the word reset; C and E clean up for every victim.
  - A frees the cascade's frames after releasing both locks and before B.
  - B takes `THREAD_TABLE` once: today `process.rs:137` and `:156` lock it with IRQs on, while the timer IRQ takes it through `check_timeouts → wake_with_error → unblock` (`timer.rs:194`, `ipc/timeout.rs:130`). B's masked section uses `unblock`'s DAIF save/restore, so a caller that entered masked stays masked.
  - C clears `waiting_receiver` wherever it names a victim, on every channel, so a replacement receiver does not get EAGAIN (`channel.rs:271-273`) forever. Per victim, each lock alone: `timeout::clear_timeout_blocking(tid)`, a new blocking `lock()` in thread context that returns whether it removed an entry (today's `clear_timeout` can skip under contention, and the entry would later fire at a dead slot); `REPLY_SLOTS[tid] = None`; `SELECT_WAITERS[tid] = None` (until step 11 deletes it) and the tid's `NOTIFY_DEADLINES` entry reset; the tid removed from every notification's waiter list. A `pending_caller` naming a victim stays; other callers get EAGAIN meanwhile (`channel.rs:81-82`), as behind a live caller.
  - E leaves the waiter entry set. If E took it, a second thread of the parent could pass the EEXIST check and reap, and the registered waiter would wake to an Empty slot and get EPERM. Only `wait_step` clears it. E also fixes two current bugs: `unblock` runs while `PROCESS_WAITERS` is held (`process.rs:220-225`), and the code is published first (`:133`), so a reaper could free the slot while cleanup keyed by the bare pid still runs.
- `process_wait(parent_tid, caller_pid, child)` as `wait_step` plus a blocking loop. The reap sets `state = Empty`, `cap_table.clear()`, `address_space = None` and `parent = None`, and leaves `ever_used` set, so `process_install` keeps refusing the slot. The lock-free fast path (`:238-241`) is removed. On the EPERM-for-a-non-Alive-caller path, `wait_step` also clears the child's `PROCESS_WAITERS` entry if it names `parent_tid`. Step A sets `Exited` under the same lock, so no thread of an exiting process arms in `wait_step` after A.
- `sched::current_thread_and_pid()`; `sys_process_exit` (`syscall/mod.rs:661-671`) and `sys_process_wait` (`:676-689`, now `&mut TrapFrame`, x1 output). With no process, `sys_process_exit` returns EPERM in x0; on `Err` the thread keeps running, so pid 0's threads get EPERM rather than being killed.
- Boot processes (pids 7–10) have no parent and no reaper; one that exits stays an Exited zombie with a fully revoked table.
- 1b instruments: the blocked-with-no-waker scan adds `BlockedProcessWait`, and a `PROCESS_WAITERS` entry counts as a waker.
- **New `ipc/tests/lifecycle.rs`**, run by the ipc-timeout thread before the Timeout test and after the stale tests:
  - **Thread-less.** Install pid 12 (parent 1). `process_exit(12, -6)` then `process_exit(12, 5)` both return Ok. SVC 25 through `syscall_dispatch` with x0 = 12 returns x0 = 0 and x1 = -6 sign-extended, without blocking (#189 f). A second `process_wait(me, 1, 12)` returns EPERM, and `process_install(12)` returns EEXIST. Install pid 13 with no parent, exit it, and `process_wait(me, 1, 13)` returns EPERM (#189 a). `process_exit(ProcessId(0), 0)` returns EPERM. Lines: `Lifecycle: reap as expected`, `Lifecycle: wait ABI as expected`.
  - **Self-exit.** Pid 14 (parent 1, `ChannelCreate` at init) has one thread, created at init and pinned to CPU 2, which yields until the ipc-timeout thread sets a start flag after the stale tests. Pid 17 (parent 14, no threads) is installed at init. Pid 14's thread calls `wait_step(me, 14, 17)`, which arms and registers, then `cancel_block(me)`. It creates a probe with `channel_create`, calls `process_exit(14, 7)`, checks that `ipc_call(probe)` returns EPERM and `ipc_reply(probe)` returns EPIPE (the cascade freed it; a live channel would give EPROTO), logs, calls `prepare_block(me)` and `unblock(me)`, sets a done flag (Release) and calls `exit_current()`. Arming before the flag means the ipc-timeout thread cannot read Idle before the arm; a `block_current(Dead)` that consumed the token would return and `exit_current` would panic. The ipc-timeout thread yields until the flag is set (Acquire), checks that pid 17's `PROCESS_WAITERS` entry is `None` (#189 e), gets `Ok(7)` from `process_wait(me, 1, 14)` without blocking, yields until that thread's word reads Idle (bounded), and runs `thread_probe(tid, true)`: Dead and in no queue. The direct probe matters: step 8's pick filter drops a Dead thread from the queue before it runs, so a broken Dead row that enqueued without writing Runnable would never reach `exit_current`. Lines: `Lifecycle: exit cascade as expected` (including these Dead-token checks), `Lifecycle: waiter cleared as expected`.
  - **Shm exit.** Pid 15 (parent 1, delegatable `SharedMemoryCreate`, no threads). Pid 1's thread creates R for pid 15, shares it from pid 15 to pid 1, maps it for pid 1, borrows it, then calls `process_exit(15, 0)`. `shared_memory_map(1, R)` returns EPIPE even though pid 1's token is live, `shared_memory_unmap(1, R)` returns EPIPE and `is_live()` is false. Release, then `process_wait(me, 1, 15)` returns `Ok(0)`, and pid 1's stale token is revoked through `revoke_in_process`. Line: `Lifecycle: shm exit as expected`.
  - **Window 2 and one waiter.** Pid 16 (parent 1, no threads). `wait_step(me, 1, 16)` returns `None` (armed, registered). `wait_step(other, 1, 16)`, where `other` is another pid-1 thread's tid, returns EEXIST before arming. The same thread calls `process_exit(16, i32::MIN)`; E reads the waiter, leaves the entry set, and calls `unblock` while this thread is Running and Armed. `wait_step(other, 1, 16)` returns EEXIST again. With the safety entry published, `block_current(BlockedProcessWait)` returns in under 50 ticks (checked and cleaned up as in the wake-token test). `wait_step(me, 1, 16)` returns `Some(i32::MIN)` (#189 d). Lines: `Lifecycle: one waiter as expected`, `Lifecycle: early wake as expected`.
  - Pids 12, 13, 16 and 17 get no capabilities.
- **Checks.** `just test`: process-state transitions and `process_wait_action` over every combination, including "T1 registered, child exits, T2 gets EEXIST, T1 reaps". `just run`: the seven `Lifecycle:` lines; `shm_cascade: pid=15 destroyed 1 regions`; `grep -c 'shm: region [0-9]*\.[0-9]* drained'` is 2; all step-9 lines; the Timeout-last check; the DAIF count reports `scheduler.rs:10` and `direct.rs:8`; no new kwarn and no `PANIC`.

### Steps 11–14: shared loop contract and helpers

Introduced in step 11 and used by steps 12–14.

- **Own tid:** the site reads `me` once per wait through `sched::current_tid()`, added in step 11: IRQs masked with `unblock`'s DAIF save/restore, then `CURRENT_THREAD[core_id()]`. It replaces the site's `current_thread_id()` call, whose two reads with IRQs on can name another CPU's thread (crash-fix N4); `prepare_block` on that tid would arm the wrong word. A missing current thread returns EPERM.
- **Pass 1:** arm inside, or before, the critical section that publishes the first waker record, then publish the timeout entry or deadline after the arm, then block. There is no check between the publish and the block, so a normal wake is a `LeaveToken`, an `Enqueue` or a direct switch, never a `Skip`.
- **Pass ≥ 2:** first `prepare_block(me)`. A condition that a waker writes under lock L may be checked in the same L critical section as the arm. Then evaluate the exits in the site's order, republish any record a waker removed, re-insert the timeout entry if it is missing, and block.
- **Fast path:** a read-only check with no arm may come before pass 1.
- **Every pass ends in `block_current` or `cancel_block`.** A pass that armed and then finds its deadline passed, a condition met or a failure calls `cancel_block(me)` before the next pass or the exit, so the next `prepare_block` finds Idle (its assertion is live in `just run` and the soak, which build the dev profile).
- **Exit:** remove own records and entries, take `WAKEUP_ERRORS` once and discard it if `wake_with_error` can reach the site, and then `cancel_block(me)` if this pass armed.
- **Result:** derived from object state and the wait's own deadline, never from which wake arrived.
- **Helpers.** `Deadline = Option<u64>`, computed with `saturating_add`. `timeout::arm_timeout(me, deadline, code) -> Armed | Expired` reads `TICK_COUNT` inside the `TIMEOUT_QUEUE` hold: `now >= deadline` gives Expired; otherwise it inserts the entry when the slot is `None`. Reading inside the hold orders it after any `check_timeouts` that removed the entry, so it never re-inserts an entry that already fired. `timeout::clear_own_timeout(me)` is `clear_timeout_blocking(me)` (step 10) and replaces `channel.rs:182-185` and `:295-298`. `notify::arm_deadline(me, deadline) -> Armed | Expired` and `notify::clear_deadline(me)` (sets `u64::MAX`).
- **Wake helper thread.** A new pid-1 thread, created at init and pinned to CPU 2. It polls an `AtomicU32` mailbox with `thread_yield` and runs one command at a given tick, then sets a done flag: `Stale { target, clear_timeout: bool }` (optionally `clear_timeout_blocking(target)`, recording whether it removed an entry, then `unblock(target)` under the `self_test` caller); `Signal { n, bits }`; `Send { ch }`; `RecvBlock { ch, t }`. A test that asks for `clear_timeout: true` logs `stale clear missed` as a failure if no entry was removed, because it would then not exercise the re-insert.
- **Backstop.** Steps 12–14's tests arm `notify::arm_deadline(me, now + 200)` as a backstop, which relies on step 11's unconditional `check_notification_timeouts`. The backstop is cleared afterwards; if it already fired, its late `unblock` is one more stale wake, which the next test tolerates.

### Step 11: `notification_wait`, `ipc_select` and their wakers

- **`sched::current_tid()`** in `scheduler.rs`, the fourth IRQ-masked helper (the own-tid rule above). Steps 11–14 switch each converted site to it.
- **Waiter record** (`notify.rs:21-29`): `Waiter { tid, mask, kind: WaitKind::{Wait, Select}, fired: u64 }`.
- **`notification_signal`** (`notify.rs:90-153`), under `NOTIFICATION_TABLE`: `fetch_or(bits)`. For a `Wait` record with `fired == 0`: `matched = word & mask`; if non-zero, `fetch_and(!matched)`, set `fired = matched` (the record stays), and collect the tid. For a `Select` record with `word & mask != 0`: remove the record (the claim) without consuming the bits, and collect the tid. Release, then `unblock` each collected tid. `NOTIFY_RESULTS` (`:62`) and the `try_wake_select` call (`:139-147`) are deleted. A signaller that runs after a waiter's own cleanup finds no record, so a completed wait cannot lose bits; the old path wrote the result after releasing the table lock.
- **`check_notification_timeouts`** (`notify.rs:308-357`): under `NOTIFY_DEADLINES.try_lock()`, put the expired tids in a `u64` bitmask and set their entries to `u64::MAX`. Release, then `unblock` each tid. The `THREAD_TABLE.try_lock`, the state `match` and the `NOTIFICATION_TABLE`/`SELECT_WAITERS` cleanup go. The bitmask avoids an IRQ-path stack array; the crash-fix H5 row names the zero-fill at `timeout.rs:89` as a V-register suspect. `unblock` now blocking-locks `THREAD_TABLE` from this path, as `check_timeouts → wake_with_error` already does; that exposure exists only while a notification or select deadline is pending, which at boot means only during the new self-tests.
- **`notification_destroy`** (`:258-286`) is unchanged. Each waiter finds the slot empty and returns EINVAL (today ETIMEDOUT). The function has no callers.
- **`notification_wait`** (`:159-255`): the id check and the fast path (`:172-189`) are unchanged; a missing current thread returns EPERM (#186 item 5). Each pass, under `NOTIFICATION_TABLE`: (1) slot empty → exit EINVAL; (2) own `Wait` record with `fired != 0` → take the bits, remove the record, exit `Ok(bits)`; (3) `word & mask != 0` → consume, exit `Ok`; (4) `prepare_block(me)`; (5) no own record → insert `Waiter { me, mask, Wait, 0 }`, or exit ENOMEM if no slot is free; (6) release; (7) `arm_deadline` returns Expired → under the lock remove own record, `Ok(bits)` if it had fired, otherwise ETIMEDOUT; (8) otherwise `block_current(BlockedNotification { notification })`. Exit: `clear_deadline(me)`, then `cancel_block` if this pass armed. A fired record wins over the timeout, because its bits were already consumed. `WAKEUP_ERRORS` is not used.
- **`ipc_select`** (`select.rs:52-148`): validation and `check_channel_entries` are unchanged; a missing current thread returns EPERM.
  - **Fast scan** (no arm), each entry in index order under its own lock. Channel: lookup error → return it; Dead → EPIPE; `ring.len > 0` → `Ok(i, 0)`. Notification: empty slot → EINVAL; `word & mask` → consume, `Ok(i, bits)`.
  - **Loop:** `prepare_block(me)`, then each entry in order, one critical section per source. Channel: lookup error or Dead → fail; `ring.len > 0` → ready(i, 0); otherwise `waiting_receiver`: `None` → `Some(me)`, `Some(me)` → keep, `Some(other)` → fail EAGAIN (#186 item 2). Notification: empty slot → fail EINVAL; `word & mask` → consume, ready(i, bits); otherwise ensure one own `Select` record per notification, whose mask is the OR of every entry naming it, or fail ENOMEM. The first entry that is ready or fails ends the pass. Otherwise `arm_deadline` Expired → ETIMEDOUT, else `block_current(BlockedSelect)`.
  - **Exit:** unregister every entry (under `CHANNEL_TABLE`, `waiting_receiver == me` → `None`; under `NOTIFICATION_TABLE`, remove own `Select` records); `clear_deadline(me)`; `get_wakeup_error(me)`, discarded (destroy and exit write EPIPE for a `waiting_receiver`, but the result comes from the channel's state; this closes #186 item 1); then `cancel_block` if this pass armed. This is select's only `WAKEUP_ERRORS` take, once per wait.
  - The result is the lowest-index ready entry, matching `docs/kernel/ipc.md:127`.
  - Deleted: `SELECT_WAITERS` (`:32`), `SelectWaiter`, `register_on_sources`, `scan_entries`, `try_wake_select` (`:293-333`), `set_select_ready` (`:338-359`), and step 2's `SELECT_WAITERS[my_tid] = None` rollback. Step 10's exit cleanup drops its `SELECT_WAITERS` line.
- **Channel wakers.** `ipc_send` (`channel.rs:454-462`): under `CHANNEL_TABLE`, push the message and take `waiting_receiver`; after the release, `clear_timeout(r)` and `unblock(r)`, with no select test. `ipc_call`: the `set_select_ready` call at `:103` goes, and the fallback at `:138-140` becomes a plain `unblock(r)`. An armed receiver or selector gets a `LeaveToken`; a blocked one gets an `Enqueue`.
- **#186 after this step:** item 1 (wake results left behind) closed; item 2 (a taken receiver slot or a full waiter list skipped silently) now EAGAIN or ENOMEM; item 3 (lookup errors) closed in step 2; item 4 (event between scan and registration) closed; item 5 (EINVAL instead of EPERM) closed. #186's "out of scope" delayed-ETIMEDOUT race becomes harmless, because ETIMEDOUT is validated against the wait's own deadline.
- `ipc/tests/select_cap.rs:4`, `:105-119` checks for no partial registration. Its `SELECT_WAITERS` check becomes: no `waiting_receiver == me` on the channel and no `Select` record.
- 1b instruments: the no-waker scan drops `SELECT_WAITERS` and keeps notification waiter records as wakers.
- **`Notify-loop test: as expected`.** (a) `notification_wait(n, m, 50)` with `Stale` at +5 → ETIMEDOUT, elapsed ≥ 50, no record left. (b) `Signal(n, m)` at +5 → `Ok(m)` with elapsed < 50. The stale wake goes through `unblock`; a `TIMEOUT_QUEUE` entry with `error_code` 0 at +5 also works, because `check_timeouts` wakes unconditionally.
- **`Select-loop test: as expected`.** Channels come from `channel_create` in pid 1 and notifications from `notification_create`; all are destroyed afterwards. (a) `select([n1, ch1], 50)` with `Stale` at +5 → ETIMEDOUT, elapsed ≥ 50, no registration left. (b) `Signal(n1)` at +5 → `Ok((0, bits))`; either interleaving gives the same result. (c) `Send(ch1)` at +5 → `Ok((1, 0))`, then `ipc_recv` drains ch1. (d) The helper runs `RecvBlock(ch2, 200)`; the driver yields until `ch2.waiting_receiver` is the helper, then `select([ch2])` returns EAGAIN at once, then `ipc_send(ch2)` releases the helper.
- **Host model** (extends step 9's): a notification site and a two-source select site, with the real waker, a timeout that fires once the deadline passes, a stale `unblock`, and preemption. It asserts that the waiter is never left Blocked with the condition set or the deadline passed and no wake pending, never returns ETIMEDOUT before the deadline, and never loses fired bits. Negative controls, each reaching its bad state: the old signal path (result written after releasing the table lock while the waiter removes its own record) → lost bits; the deadline set after the waiter record with no arm → lost wake; scan and registration in separate critical sections → lost event.
- **Checks.** `just test`: the model and its controls pass. `rg -n -e SELECT_WAITERS -e NOTIFY_RESULTS -e try_wake_select -e set_select_ready kernel/src` prints nothing. `just run`: both new lines; all step-10 lines unchanged; `Select-cap test: EPERM/EINVAL as expected` still prints; the Timeout-last pattern gains `Notify-loop|Select-loop`; the DAIF count reports `scheduler.rs:11` and `direct.rs:8`; no new kwarn.

### Step 12: `sleep_ticks` (`timeout.rs:167-185`)

- The only record is `TIMEOUT_QUEUE[me]` with `error_code` 0. The real waker is `check_timeouts`; stale wakers are any `clear_timeout` plus `unblock` from a stale reply or send record.
- **Loop:** `prepare_block(me)`; `arm_timeout(me, deadline, 0)` returns Expired → exit; otherwise `block_current(BlockedTimer { wake_at: deadline })`. **Exit:** `clear_own_timeout(me)` (a no-op if the entry fired), then `cancel_block`. `WAKEUP_ERRORS` is not taken.
- **State change.** `BlockedTimer` already exists (`shared/src/sched.rs:85`) but is unused. With it, `try_reply_switch` (`direct.rs:234-241`) and `try_direct_switch` no longer match a sleeper as `BlockedIpc`, so a stale reply gets an `Enqueue` and one extra pass instead of a direct restore. `may_become` covers Running → BlockedTimer.
- 1b instruments: the no-waker scan adds `BlockedTimer` (a `TIMEOUT_QUEUE` entry is its waker).
- **`Sleep-loop test: as expected`.** `sleep_ticks(50)`, with `Stale { clear_timeout: true }` at +5 (a stale reply that clears the sleeper's entry and wakes it) and the 200-tick backstop. Pass: elapsed in [50, 200). A return before 50 logs `early`; elapsed ≥ 200 logs `entry lost`.
- **Model:** the sleep site with the stale waker. Negative control: no re-insert after the stale clear leaves the waiter Blocked with no entry and no wake.
- **Checks.** The line prints. In gpu mode the input thread's 16-tick poll (`input/mod.rs:381`) still works: `display handoff complete` prints and there is no input kwarn. `rg -n 'channel: u64::MAX' kernel/src/ipc/timeout.rs` prints nothing. The Timeout-last pattern gains `Sleep-loop`.

### Step 13: `ipc_recv` (`channel.rs:232-336`)

- Records: `waiting_receiver` (in `CHANNEL_TABLE`) and `TIMEOUT_QUEUE[me]` with ETIMEDOUT. Wakers: `ipc_send`; `ipc_call` (takes `waiting_receiver`, `clear_timeout`, then a direct switch or the fallback `unblock`); destroy and exit (take the record and write EPIPE); `check_timeouts`.
- Deadline: `u64::MAX` → none; 0 → a non-blocking poll, as today.
- **Each pass, under `CHANNEL_TABLE`:** (1) lookup error → exit with it; Dead → exit EPIPE; (2) `pop` → exit `Ok(len, sender)`; (3) poll → exit EAGAIN; (4) `prepare_block(me)`, inside the hold and after the pop check, so the success path does not arm; (5) `waiting_receiver`: `None` → `Some(me)`, `Some(me)` → keep, `Some(other)` → exit EAGAIN; (6) release; (7) `arm_timeout` Expired → exit ETIMEDOUT; (8) otherwise `block_current(BlockedIpc { channel })`.
- **Exit:** under `CHANNEL_TABLE`, clear `waiting_receiver` if it is `me`; `clear_own_timeout(me)`; `get_wakeup_error(me)` once, discarded; `cancel_block` if this pass armed. Dead is still checked first (`:251`). The "shouldn't happen" EAGAIN (`:332-335`) disappears; EAGAIN now means only that another thread holds the receiver slot, so the echo server's retry path is for that case alone.
- **Direct switch.** A receiver that slept (BlockedIpc, word Idle) is switched to directly by `try_direct_switch`, and its next pass pops. An armed receiver fails that function's receiver check (`direct.rs:84-95`); the fallback `unblock` leaves a token, and the receiver's `block_current` consumes it and pops. N1 (the receiver left Runnable and in no queue) is unchanged and stays 6b's.
- **`Recv-loop test: as expected`.** On a pid-1 test channel with no sender: `ipc_recv(ch, 50)`, with `Stale { clear_timeout: true }` at +5 (a stale sender) and the backstop. Pass: ETIMEDOUT with elapsed in [50, 200), and `waiting_receiver` is `None` afterwards.
- **Model:** the recv site. Negative controls: the timeout entry published before the arm (today's order, `:279-287` after `:275`) → lost wake; no re-insert after a stale clear → hang.
- **Checks.** The line prints. The echo, GPU and compositor receive loops behave as before; in gpu mode `display handoff complete` prints. `[bench] IPC round-trip` reports 10000 iterations in a CLEAN boot; a wedged boot is re-run and noted. The Timeout-last pattern gains `Recv-loop`.

### Step 14: `ipc_call` (`channel.rs:33-219`), `ipc_reply` and `ipc_cancel`

- `REPLY_SLOTS[me]` gains `done: bool` and `cancelled: bool`. It is not a waker record, so it may, and must, be written before the arm. `pending_caller` is in `CHANNEL_TABLE`; `TIMEOUT_QUEUE[me]` holds ETIMEDOUT. Wakers: `ipc_reply` (takes `pending_caller`, writes the bytes and `done = true` under `REPLY_SLOTS`, `clear_timeout`, then `try_reply_switch` or `unblock`); `ipc_cancel` (takes `pending_caller`, then under `REPLY_SLOTS` sets `cancelled = true` if the caller's slot is present and not `done`, then `unblock`; it no longer calls `wake_with_error`, `channel.rs:499`); destroy and exit (EPIPE); `check_timeouts`.
- Deadline: 0 or `u64::MAX` → none (0 keeps today's behaviour and fixes the doc comment at `:30`; `u64::MAX` fixes the overflow at `:119` and `:161`); otherwise `now.saturating_add(t)`.
- **Pass 1:**
  1. Write `REPLY_SLOTS[me] = Some { buf, len, 0, done: false, cancelled: false }`.
  2. Under `CHANNEL_TABLE`: lookup error, Dead, `pending_caller` already set (EAGAIN) or a full ring (ENOSPC) → release, clear `REPLY_SLOTS[me]`, return; nothing is armed. Otherwise push the message, `prepare_block(me)`, set `pending_caller = Some(me)`, and `r = waiting_receiver.take()`.
  3. Release, then `arm_timeout(me, d)`.
  4. If `r` is set: `clear_timeout(r)`. If `try_direct_switch(me, r)` returns true, the caller has been restored; go to pass 2. Otherwise `unblock(r)`, then `block_current`.
  5. If `r` is not set: `block_current(BlockedIpc { channel })`.
  6. If `arm_timeout` returned Expired (a tiny timeout, such as `timeout_ticks = 1`), still wake `r` with a plain `unblock` if there is one, then `cancel_block(me)`, and go to pass 2 without blocking. The cancel loses no wake, because pass 2 re-arms before it re-checks `done`, the claim and the deadline; without it, pass 2's `prepare_block` finds Armed or Woken and its assertion panics.
- **Pass ≥ 2** (a pre-arm read of `done` is an allowed fast path after a switch): `prepare_block(me)`; under `REPLY_SLOTS`, read `done` and `cancelled`; `done` → exit Ok; under `CHANNEL_TABLE`, lookup error or Dead → exit EPIPE, and `claimed = pending_caller != Some(me)`; claimed and `cancelled` → exit ECANCELED (claimed without either means a reply is in flight: keep waiting); `arm_timeout` Expired → exit ETIMEDOUT; otherwise `block_current(BlockedIpc { channel })`. The canceller claims before it sets `cancelled`, and the caller reads `cancelled` before the claim, so a cancel that lands between the two reads is seen on the next pass through its `unblock`'s token.
- **Exit** (after the publish): under `CHANNEL_TABLE`, clear `pending_caller` if it is `me`; take the `REPLY_SLOTS` entry, and if it is `done` the result becomes `Ok(bytes)` (a written reply wins); `clear_own_timeout(me)`; `get_wakeup_error(me)` once, discarded, the wait's only take; `cancel_block` if this pass armed.
- **Replier and canceller.** `ipc_reply` (`:375-388`) sets `done` with `bytes_written`. `ipc_cancel` (`:476-501`) sets `cancelled` and calls `unblock`, as above. Nothing else changes.
- **Direct switch.** `try_direct_switch`'s Woken check goes live: it returns false for a Dead or Woken sender inside the receiver check (`:84-95`), and otherwise swaps the word to Idle with the BlockedIpc write at `:142`. `try_reply_switch` is unchanged; an armed caller fails its BlockedIpc check (`:234-241`) and the fallback `unblock` leaves the token (N2's fix).
- **Guard.** `block_current` and `try_direct_switch` debug-assert that a block to any state other than Dead finds the word Armed or Woken. In `block_current` the assertion runs after `block_action` has ruled out `AlreadyDead` (step 9's order note).
- **`Call-loop test: as expected`.** On a pid-1 channel with no server: `ipc_call(ch, …, 50)` with `Stale { clear_timeout: true }` at +5 (a stale replier) and the backstop. Pass: ETIMEDOUT with elapsed in [50, 200); today's code returns 0 bytes at +5. Afterwards `pending_caller` and `REPLY_SLOTS[me]` are `None`, and the request left in the ring is drained with `ipc_recv(ch, 0)`.
- **Model:** the call site, with the condition written under `REPLY_SLOTS` and the claim under `CHANNEL_TABLE`, the real replier, a canceller, the timeout, the stale replier and preemption, and the pass-1 Expired path with and without a concurrent reply. It asserts the site-level properties and that every `prepare_block` finds Idle. Negative controls: `REPLY_SLOTS` written after `pending_caller` (today's order) → lost reply; the arm after `pending_caller` → lost wake; no re-insert → hang; the Expired path without `cancel_block` → `prepare_block` finds Armed.
- **Checks.** The line prints; `Timeout test: ETIMEDOUT as expected` still prints last; the bench reports 10000 iterations in a CLEAN boot, and the IPC average is recorded; the echo client's EPERM path is unchanged; the Timeout-last pattern gains `Call-loop`; the DAIF count still reports `scheduler.rs:11` and `direct.rs:8`.

### Step 15: docs

- The ADR's docs table, detailed in §7.
- **Checks.** `/audit-loop` reaches a clean round, and `just docs-check --all` marks no new finding except `knowledge-hygiene plans-not-empty` for this plan (`scripts/docs/check.py:1358-1360`); it also checks test counts and lock order (`:787`, `:836`).

### Step 16: distil and delete this plan

- Run §10: lessons into `docs/knowledge/lessons/`; §8's not-adopted points and any implementation change from the ADR as one-line entries under a new `## Review notes` in the ADR; §9's limits that still hold under a new `## Evidence limits` in the ADR; "Issues Encountered", "Decisions Made" and "Lessons Learned" below, sorted into those three places.
- Replace each mention of this plan's path in the ADR (the intro and "Companion plan and crash-fix amendment") with a GitHub permalink to this file at the last commit that contains it. The implementation PR's commits stay reachable after the squash merge, and `check.py` skips URLs with a scheme, so the link survives the deletion.
- `git rm` this plan.
- **Checks.** `ls docs/knowledge/plans` lists only `_template.md`; `just docs-check` exits 0; `rg -n 'capability-lifetime-plan' docs` prints only permalinks.

-----

## 4. Why the order

- Steps 1–2 come before step 5. Landing #163 on bare ids would let pid 1's surviving `ChannelAccess(ch2)` from the Destroy test authorise whatever channel lands in ch2's slot next.
- Step 3 comes before step 5, which needs `mint_child` and `free_slots`.
- Step 4 comes before step 5, so the scheduling change lands and boots alone; if step 5's boot regresses, the capability change is the only new variable.
- Step 6 comes after step 5 (it needs `creation_cap` from minting, `is_revoked` from step 3 and `RegionSlot` from step 2) and before step 7, so the `bad_pid.rs` reorder and the stale-shm expectations are written once, against the cascade.
- Step 7 comes after steps 5 and 6: share-as-delegation needs the minted token, and the Kit destroy uses `region_detach`.
- Step 8 comes before step 9 (the token's Dead rows rely on its guards) and before step 10 (revoking pid 7's table without the guards leaves a killed thread able to be picked and to spin on EPERM). Step 9 comes before step 10, because `process_wait` arms.
- Steps 11–14 come after step 10, from least to most used: 11 has no boot user, 12 serves gpu input, 13 the services and the bench server, 14 the bench client. The bench's IPC path changes last. Each conversion arms a site and adds its loop in one commit, so no site ever arms without its re-check loop.
- Step 16 comes last, after the soak pair and before the PR is marked ready (rule 04 step 11), so that the PR's final docs-check passes and nothing it needs is deleted early.

-----

## 5. Test plan

### 5.1 Host tests (`shared/`; CI runs them under Miri)

- Ids: §3 step 1.
- Capabilities: §3 step 3; `share_decision` (step 7).
- `ThreadState::may_become` over every pair (step 8); `ProcessState::may_become` and `process_wait_action` over every combination (step 10).
- `wake_action` over every `ThreadState` and `WaitState`; `block_action` over those pairs and every `next`; every implied state change is allowed by `may_become` (step 9).
- **Protocol model** (step 9), enumerating every interleaving of the waiter (check, arm and register under a publish lock; an optional preemption, Running → Runnable; block) and the waker (publish the condition and read the waiter under that lock; `unblock`). It asserts that no final state leaves the waiter Blocked with the condition set and no enqueue. A second waker, a timeout that sets the waiter's error and then calls `unblock`, with the waiter publishing its timeout entry after arming: no final state leaves the waiter Blocked with its timeout spent and no enqueue. A victim that arms after it is marked Dead, and a victim that armed before exit B reset its word: in both, its word is Idle once its `block_current` has run, and `block_current`'s arm assertion (step 14) is not reached, because `AlreadyDead` is decided first. Negative controls: the model without the arm (today's `unblock`), and the two-waker model with the timeout entry published before the arm; each must reach its lost-wakeup state.
- Steps 11–14 extend the model per site (§3), each with its negative controls.

### 5.2 Boot self-test lines

| Line | Before | After | Step |
| --- | --- | --- | --- |
| `pid=N: denied ChannelAccess(…)` | 6 per boot (3 timeout/destroy, 3 select_cap) | 3 (select_cap only) | 5 |
| `Timeout test:` | `unexpected result -6`, first in the thread | `ETIMEDOUT as expected`, after every other ipc-timeout test line | 4, 5 |
| `Destroy test:` | `unexpected result Err(-6)` | `EPIPE as expected` | 5 |
| `Stale-id test:`, `Stale-shm test:` (`ipc/tests/stale_id.rs`) | — | `EPIPE as expected`; `slot moved` kwarn instead if 8 attempts never reuse the slot (tallied separately) | 2 |
| `Shm-cascade test:` (same file) | — | `EPIPE as expected` | 6 |
| `Create-ABI test:` (`ipc/tests/create_abi.rs`) | — | `x0/x1 as expected` | 5 |
| `Share test:` (`ipc/tests/share.rs`) | — | `delegation as expected` | 7 |
| `Dead test:` (`ipc/tests/dead_thread.rs`) | — | `terminal as expected` | 8 |
| `Wake-token test:` (`ipc/tests/wake_token.rs`) | — | `early wake as expected`; `token lost` on failure | 9 |
| `Lifecycle:` (`ipc/tests/lifecycle.rs`) | — | `reap`, `wait ABI`, `one waiter`, `exit cascade`, `waiter cleared`, `shm exit`, `early wake`, each `as expected` | 10 |
| `Notify-loop test:`, `Select-loop test:` | — | `as expected` | 11 |
| `Sleep-loop test:` | — | `as expected`; `early` or `entry lost` on failure | 12 |
| `Recv-loop test:` | — | `as expected` | 13 |
| `Call-loop test:` | — | `as expected` | 14 |
| `shm_cascade: pid=N destroyed K regions` | — | `pid=1 … 1 regions` 5 times (Bad-pid, Stale-shm, Shm-cascade twice, Share); `pid=2 … 1` once (Share); `pid=15 … 1` once | 6, 7, 10 |
| `shm: region S.G draining (B borrows)`, `shm: region S.G drained` | — | twice each, with 1 borrow (Shm-cascade, `Lifecycle: shm exit`) | 6, 10 |
| `pid=N: revoked token N` | from `revoke_in_process` only | one more per `revoke_in_process` call in the self-tests; not an acceptance count | 6 |
| Bad-pid's `shm_unmap: region=… freed (… pages) last_ref by pid=1` | printed | gone; the cascade frees that region | 6 |
| `shm_destroy: region S.G` | — | once (Share test) | 7 |
| Echo client after `process_exit(7)` | not reached (0 of 60 boots) | `Echo client: EPERM after exit (expected)` if reached; not an acceptance line | 8 |
| `Channel N created`, `Channel N destroyed`, `shm_create: id=N` | slot number | `slot.generation`, for example `Channel 2.1 created` | 1 |
| `pid=3: denied ChannelCreate` | not reached in run 167 | unchanged | — |
| `Bad-id test`, `Bad-pid test`, `Select-cap test` | — | unchanged | — |

Log messages are cut at 48 bytes (#191), so every new line stays under that. Self-test threads pinned to CPU 2: the ipc-timeout thread, the Dead-test helper, pid 14's thread and the wake helper.

**Pid 1's token slots at the end of step 14**, about 30: 3 initial grants (`ipc/tests/mod.rs:93-94`, `:147`), 2 select_cap grants (`select_cap.rs:53`), 9 minted `ChannelAccess` (Timeout, Destroy, Stale-id A and B, Create-ABI, Select-loop 2, Recv-loop, Call-loop), 8 minted `SharedMemoryAccess` (Stale-shm 2, Shm-cascade 2, Create-ABI, Bad-pid, Share 2), 7 temporary `SharedMemoryCreate` grants (Stale-shm, Shm-cascade 2, Create-ABI, Bad-pid, Share 2) and 1 foreign token shared by pid 15. If every stale-test retry is used, about 44. Both are far inside the 192 slots the reserve leaves. Counted from this plan, not measured.

### 5.3 Soak

- **Prerequisites:** crash-fix steps 1a (interleave mode, Fisher tests, `runs` input) and 1b have merged. There is no fallback to back-to-back runs.
- **The pair:** post-1b `main` against the branch tip, interleaved under the crash-fix soak protocol, 20 text-mode and 10 gpu-mode boots per arm (about 75–85 minutes), under the load rule and with no other soak on the host.
- **Gates:** the regression guard (one-sided Fisher on CLEAN, failing at p < 0.05); the expected-line tally below.
- **Expected-line tally.** The classifier ignores self-test lines (`scripts/soak-qemu.sh` matches only fatal reports, the heartbeat and the Gate markers), so CLEAN alone would not notice a missing one. For each arm, count the boots whose log contains each expected line (for example `grep -L '<line>' run-*.log` per line): `Destroy test`, `Bad-id test`, `Bad-pid test`, `Select-cap test`, `Stale-id test`, `Stale-shm test`, `Shm-cascade test`, `Create-ABI test`, `Share test`, `Dead test`, `Wake-token test: early wake`, the seven `Lifecycle` lines, `Notify-loop`, `Select-loop`, `Sleep-loop`, `Recv-loop`, `Call-loop` and `Timeout test: ETIMEDOUT`. Every CLEAN boot in the branch arm must contain all of them, except that a `slot moved` line stands in for its stale test's `EPIPE` line; those boots are counted separately.
- **Reported per arm:** 1b's per-caller `Skip` counts, and in the branch arm `LeaveToken` and `TokenPending`; the no-waker scan; `bw`; the scan-skipped count; DEGRADED, WEDGE-ALIVE and WEDGE-STUCK; the Gate 1 IPC average.
- **Expected:** in the `main` arm, N2 and N3 show as skips from `ipc_reply`, `ipc_send`, the `ipc_call` fallback and `check_timeouts`. In the branch arm they show as `LeaveToken`, and any `Skip` is benign: it found its target outside an armed window, between passes or after its wait, and the target's next pass re-checks after arming. A source that reads 0 in all 30 boots of the `main` arm is recorded as "not observed at n = 30"; its conversion then stands on the host model and its negative controls, and the result is recorded on #164. The no-waker scan can still flag N1 variant (b), which 6b fixes. Class results are not gated: WEDGE-STUCK censors them until crash-fix step 2, and the class gate is 6b's.
- Record the counts in the PR body and on #164.
- **CI.** The report-only CI soak (#168) does not gate. CI arms are unpaired, so the 3 baseline CI runs on `main` are re-run after this PR merges, before any later step's CI numbers are compared.

-----

## 6. Crash-fix interplay

### 6.1 The step-1b constraint and its resolution

- N2 (`:98`): `ipc_call` publishes `pending_caller` and its timeout before `block_current` sets BlockedIpc. A replier on another CPU clears the timeout, fails `try_reply_switch`, and its fallback `unblock` skips a caller that is still Running: "The caller blocks with no waker and no timeout." N3 (`:99`): the same gap in `ipc_recv` and `sleep_ticks`, and a notification deadline cleared and then dropped (`notify.rs:317`, `:321-323`).
- `:114`: the heartbeat-alive WEDGE has five candidate causes, with (a) = N2; "Step 1b's counters separate the five: … the orphan and blocked-with-no-waker scans (a, b) …".
- `:420`: 1b counts "`unblock` skipping a Running or Runnable target, by caller". `:423`: 1b scans for "blocked with no waker". `:430-434`: 1b is soaked interleaved against `main`; `:432`: WEDGE-STUCK must not differ between the arms; `:433`: every non-CLEAN boot must have a tripwire line.
- `:497`: 6b's acceptance requires "`unblock` skipping a Running target reads 0 from `ipc_reply`". `:539`: "Step 1b adds a detect-only counter for each mechanism, so the count is seen above 0 before its fix and at 0 after it." `:619`: "None has been confirmed at runtime; step 1b exists to do that."
- **Resolution (ADR §10):** step 1b merges first, and both arms of this PR's pair carry its counters. 1b's own pair (against `main`) and the `main` arm of this PR's pair both measure N2 and N3 on unconverted code; this PR's branch arm measures the fix. The comparison stays inside one pair, as "No instrument freeze" requires (`:388`).

### 6.2 Extensions to step 1b's instruments

1b's counters and tripwire keys are 1b's design; this PR extends them in the steps that change what they measure. The call sites whose outcomes must stay distinguishable by caller:

| Caller | Call site | Notes |
| --- | --- | --- |
| reply | `channel.rs:403` | |
| send | `channel.rs:460` | |
| call fallback | `channel.rs:139` | |
| timeout | `timeout.rs:105` (`wake_with_error`) | |
| destroy | `ipc/mod.rs:231`, `:234` | |
| exit EPIPE | `process.rs:209` | |
| cancel | `channel.rs:499` | `unblock` instead of `wake_with_error` from step 14 |
| select | `select.rs:326` | deleted in step 11 |
| signal | `notify.rs:146` | |
| notification timeout | `notify.rs:341`, `:349` | one site after step 11 |
| notification destroy | `notify.rs:282` | |
| process exit | `process.rs:223` | |
| `self_test` | test and probe calls | added in step 8 |

- **Outcomes.** Step 8 adds `Dead`. Step 9 maps `wake_action` onto the counter, adding `LeaveToken` and `TokenPending`; `Skip` then means a Running or Runnable target whose word is Idle.
- **Scans.** Step 8 excludes Dead threads from the orphan and no-waker scans. Step 10 adds `BlockedProcessWait` with `PROCESS_WAITERS` as a waker. Step 11 drops `SELECT_WAITERS` and keeps notification waiter records and non-`u64::MAX` `NOTIFY_DEADLINES` entries as wakers. Step 12 adds `BlockedTimer`.
- **`bw`** (step 9): a thread flagged in two consecutive scans whose wait word is not Idle while its state is neither Running nor Runnable (a Blocked state or Suspended, and Dead once counted). Dead threads are excluded until crash-fix step 8's safe point; from then on they are counted, and `bw` reads 0.
- **Tripwire keys** follow 1b's naming. If step 1a's parser uses a fixed key list, the steps that add keys add them to `scripts/soak-qemu.sh`.
- **No notification-drop counter.** No boot thread waits on a notification or a select, so a counter for the dropped deadline (`notify.rs:317`, `:321-323`) would read 0 by construction. Step 11's host model and its negative control are the evidence for that variant.
- The Timeout test's IRQ wake queues its pinned thread on CPU 2, its last CPU, so 1b's cross-CPU counter should record it 0 times. The region paths add no `unblock` calls.

### 6.3 Constraints on later crash-fix steps

- **Steps 2, 6a, 6b and 8** rewrite `thread_yield`, `block_current`, `unblock`, `schedule` and `direct.rs`. Those rewrites keep the host-tested `may_become`, `wake_action` and `block_action` intact; landing them first gives the later steps rules to preserve.
- **Step 2.**
  - `with_this_cpu` replaces the IRQ-masked `CURRENT_THREAD` reads added here (`kill_process_threads`, `current_thread_and_pid`, `current_tid`); `thread_probe`'s `RUN_QUEUES` reads are in scope too. `prepare_block` takes an explicit tid, but every converted site's `me` read is in step 2's scope: until then it goes through the masked `current_tid()`, because `current_thread_id()`'s unmasked two-read form (N4) could arm another thread's word.
  - The four helpers add eight inline DAIF sites; the inventory at `:242` becomes 49 in 10 files. They restore the previous IRQ state, so F6's list of 10 unconditional exits is unchanged. Step 8 of this PR deletes three of F6's per-CPU read sites (`:187`: `service/mod.rs:315-316`, `gpu/service.rs:178-179`, `compositor/service.rs:187-188`), and with them the step-2 conflict on `compositor/service.rs:187-188` noted for PR #149 (`:576`).
  - The IRQ-shared set is 7 statics after this PR, and the nesting at `:219` is gone. Whichever of this PR and step 2 lands second adjusts the classes and the §3.3 rebuild.
  - `THREAD_TABLE`'s type change does not touch the token code, because every wake and consume runs inside an existing `THREAD_TABLE` critical section.
  - No new lock static: `WAIT_STATE` is an atomic array, and the draining state lives in the `SHARED_REGION_TABLE` payload, so the lint (`:225`) is not triggered. `PROCESS_TABLE`, `SHARED_REGION_TABLE` and `FRAME_ALLOC` are not IRQ-class, and no region path runs in IRQ context.
  - EL0 SVCs 16 and 24 free region frames with IRQs masked until F6. That does not deadlock, because no lock on that path is taken in IRQ context, but step 2 must not assume frame frees run with IRQs on.
  - **H3 surface.** No wait gains a thread-context hold of `THREAD_TABLE`, `CURRENT_THREAD` or `RUN_QUEUES` with IRQs on; the converted waits' `CURRENT_THREAD` read (`current_thread_id()`) becomes masked. `WAKEUP_ERRORS` is taken once per `ipc_call` or `ipc_recv` wait, at exit, as today (ECANCELED moves to `ReplySlot.cancelled`, so `ipc_call`'s passes do not take it); `ipc_select` adds one take at exit and has no boot user; `ipc_cancel` drops its take. One `THREAD_TABLE` hold with IRQs on goes: `try_wake_select`'s read at `select.rs:296`, reached from `ipc_send` and the `ipc_call` fallback. `check_notification_timeouts` now blocking-locks `THREAD_TABLE` through `unblock`, as `check_timeouts` already does, only while a notification or select deadline is pending.
- **Step 6a.**
  - `finish_switch` must not queue a Dead previous thread.
  - In F4's restructured `unblock` (read the state, drop `THREAD_TABLE`, wait for `on_cpu`, re-take, re-check), the `on_cpu` wait belongs to the `Enqueue` path only. F4 waits only before a target is restored or queued (`:170`), and `LeaveToken` and `TokenPending` do neither. Waiting would also deadlock when the target is the waker itself, as in the self-tests' self-unblocks or a same-CPU IRQ wake. The token CAS stays in the final `THREAD_TABLE` section with the state re-check. A `ConsumeToken` block does not switch, so it never runs `finish_switch`.
  - A per-process on-CPU count is the precondition both for releasing a killed process's region borrows and for reaping once `UserAddressSpace` gets a `Drop`.
- **Step 6b.** It keeps N1 (`try_direct_switch` marks the receiver Running at `direct.rs:135`, and `schedule()` requeues on "was current and not blocking", counting Dead as blocking; a `ConsumeToken` block never reaches `schedule()`), N7 (`unblock` prefers the thread's last CPU) and N10 if 1b's starvation scan implicates it. It needs 6a and this PR. Its acceptance (`:494-500`) is unchanged and also re-verifies this PR, except `:497`, which the amendment makes a reported count: 6b passes on `:496`. It no longer converts sites, extends the host model, fixes the notification deadline (F5's third bullet, `:180`) or changes the select and notification wakers.
- **Step 8.**
  - The exit safe point (§6.4's amendment). A flagged thread leaves the CPU only where it holds no lock: at `irq_exit` where preemptible, at syscall return, or at `block_current` entry. At `prepare_block`, which runs inside the publishing lock, the check does not arm and never leaves the CPU. Once the safe point exists, `bw` counts Dead threads and reads 0.
  - `WAIT_STATE` may move into `ThreadInfo` next to `on_cpu`; `prepare_block` and `cancel_block` then drop their tid argument and read the current tid from `ThreadInfo`.
  - The sleeping-while-atomic check at `block_current` entry runs before `block_action`, so a pending token cannot hide a sleep attempted with preemption disabled.
  - Region borrowers dereference with no lock held, so the bench's 512 KiB loop and a future compose pass stay preemptible. The cascade's nested hold is a bounded scan of 64 slots.
- **Future EL0 teardown.** Its broadcast `tlbi vae1is`/`tlbi aside1is` plus `dsb ish` under `PROCESS_TABLE → SHARED_REGION_TABLE` is safe after the Phase 2 WB upgrade and must complete before frames are freed or drained.

### 6.4 The crash-fix amendment

- **Where:** the crash-fix ADR's `## Review notes` (line 631) is newest-first. The note goes at line 633, after the blank line 632 and before `**Owner decision, 2026-09-22 (#164).**`, followed by one blank line.
- **Why every other edit keeps its line count:** the ADR and this plan cite the crash-fix ADR by line number (`:98`–`:619`), and so do comments on #185 and #189. Every edit above line 631 replaces exactly one line. The only added lines are the note itself, after the last cited line. Do not add an `updated:` frontmatter field (it would shift every line by one).
- **When:** in the docs PR that carries the ADR, because it records owner decisions. This plan is not in that PR (§1).
- **Attribution:** the note credits the token's move out of 6b to first-set answer 3 (#185, #189), the other five sites and the "resolved first" constraint to second-set answer 2, and the safe point to second-set answer 3. It marks as *(capability-lifetime ADR choice)* the order "1b before this PR", the notification-deadline move, the L497 reading and step 8's added acceptance.
- **In-place edits (one line each):** L10 (intro), L179 (F5's N2/N3 bullet), L180 (F5's notification bullet), L367 (6b row), L369 (8 row), L375 (6a/6b attribution), L492 (6b heading), L497 (6b's `ipc_reply` skip line), L512 (8 heading), L516 (8 acceptance) and L613 (last paragraph of "Decision"). Each appends a pointer to the amendment or rewrites a table row in place, and all of them land with the note in the same docs PR.
- **Deliberately unchanged:** the frontmatter and `status: final`; L402 (the state machine still goes in `shared/`); L414-434 (step 1b; it lands first); L440 and L562 (step 2's replacement of the `try_lock()` rule, which this PR keeps and only appends to); L494-496 and L498-500; L514-515; L551 ("10 PRs" still holds); the decision tables and owner confirmations 1–5.
- **Optional:** stale citations not caused by the amendment. The ADR cites `developer-guide.md:2053` at L179 and `:2054` at L440, L562 and L576; they are now `:2060` and `:2061`. #192 delivered step 1a's `timeout` item (L410), so #169 (still open) no longer waits for step 1a (L585). They are left out of the planned edits because they are neither owner decisions nor consequences of #185 or #189, and the amendment's list of changed lines should match what the note records. Fix them in place, one line each, if an audit flags them.

### 6.5 Why the notification-deadline fix is here, not in 6b

Converting `notification_wait` needs `check_notification_timeouts` to wake an armed Running target, which today's state `match` ignores (`notify.rs:351-353`). The rewrite in step 11 calls `unblock` after releasing `NOTIFY_DEADLINES` and no longer takes `THREAD_TABLE.try_lock`, so the drop at `:321-323` disappears with it. A failed `NOTIFY_DEADLINES.try_lock` clears nothing and retries on the next tick. F5's third bullet is therefore delivered here, and 6b keeps only N1, N7 and N10.

### 6.6 Fallback if step 1b stalls: "step 0"

Not planned. If 1b stalls and the owner wants this PR first, the wake-token design's alternative applies:

- A first commit, "step 0", lands the N2/N3 part of 1b, detect-only, on unconverted code: `unblock` and `wake_with_error` take a `WakeSource` (the callers of §6.2), a `[[AtomicU32; OUTCOMES]; SOURCES]` counter array incremented with `Relaxed` `fetch_add` inside `unblock`'s existing branches (safe: `.bss`, WB, after M8), the no-waker scan in the CPU 0 heartbeat (`timer.rs:183-187`) with `try_lock` only and twice-in-a-row flagging, counters for the notification drop (`notify.rs:323`, `:351-353`), and a `[tripwire]` line with non-zero keys at each heartbeat and at `bench.rs:450`.
- **Pair B**, `main` against step 0, runs before the first conversion is pushed. It must pass the regression guard, `:432`'s inertness test (WEDGE-STUCK not different, two-sided p ≥ 0.05) and `:433`. A source that reads 0 in all 30 boots is recorded as "not observed at n = 30".
- The PR's own pair then becomes step 0 against the tip. 1b adopts step 0's counters and adds the rest (the detect-only IRQ-class lock, the ELR/SPSR check, per-CPU ticks, cross-CPU counts, the orphan and starvation scans, the fatal dumps and the objdump).
- Cost: a second host-exclusive soak of 75–85 minutes, and 1b adopts code designed here. That is why the ADR chose "1b first".

-----

## 7. Docs to update (step 15)

| Doc | Change |
| --- | --- |
| `CLAUDE.md:98-102` | Lock order: the create, map, share, Kit-destroy and region-cascade paths nest `PROCESS_TABLE` over `CHANNEL_TABLE`/`SHARED_REGION_TABLE`, one table lock at a time; add `PROCESS_TABLE > PROCESS_WAITERS`; remove `SELECT_WAITERS` (`:99`) |
| `CLAUDE.md:103-107` | `channel_create → ChannelCreate (mints ChannelAccess(new) as a child)`; the same for `shared_memory_create`; `shared_memory_share → live SharedMemoryAccess(full id)`; revoking `SharedMemoryCreate`, or the creator's exit, destroys its regions |
| `CLAUDE.md` Key Technical Facts | Id layout (slot bits 0–7, generation bits 8–31, first generation 1, 0 = retired); Dead is terminal; process slots are never reused; every waiting site arms with `prepare_block` before publishing any waker record, timeouts and deadlines included, passes its own tid read with IRQs masked (`current_tid()`), and `block_current` asserts it; `unblock` of an Armed Running/Runnable thread leaves a token that `block_current` consumes; Dead threads never take or consume one; EL1 code reads region memory only between `region_borrow` and `region_release`, and never releases under `PROCESS_TABLE` |
| `docs/kernel/ipc.md` | §3.1 `ChannelCreate` (`:140-143`): x0 id, x1 handle. `SharedMemoryCreate` (`:240-243`): id and handle. Flows at `:601` and `:615`: `SharedMemoryCreate(...) → (region_id, handle)`. `ChannelId` (`:336-340`): encoding. `creation_cap` (`:444-450`): `creator` and the minted child. `ProcessExit`/`ProcessWait` (`:275-283`): no return; parent only, reaping, code in x1; `timeout` not implemented. §4.5 struct comment (`:634-637`) and share flow; §4.5 process death (`:657-664`): the creator's death destroys its regions and removes every mapping; "EL1 kernel access only through a borrow". §5.5 `:848`. `ipc_select` (`:127`): errors EAGAIN, ENOMEM, EPIPE, EINVAL, and the lowest-index ready entry. `notification_wait` on a destroyed object: EINVAL. `ipc_call`: a written reply wins; `timeout_ticks = 0` means no timeout. Blocking calls return only on their condition, an error or their deadline. `SqEntry` `:1848`: `channel_id` must be `u32` |
| `docs/kernel/deadlock-prevention.md` | §3.3 table (`:81-91`, `ChannelTable` type; `:84` `SHARED_REGION_TABLE` nested under `PROCESS_TABLE` by create, map, share, destroy and the cascade; `:130` `PROCESS_WAITERS` is not a leaf under `PROCESS_TABLE`); drop the `SELECT_WAITERS` row (`:87`), its graph edge (`:115`) and `NOTIFY_RESULTS` (`:132`); the `NOTIFY_DEADLINES` leaf (`:133`) now matches the code. §3.5 (`:163`) the new `process_exit` pattern; `:167` `process_cleanup_shared_memory` uses `region_detach`, and the cascade detaches in the hold |
| `docs/kernel/scheduler.md` | Thread states (about `:399`): Dead is terminal; `exit_current`; the wake token (`WaitState`, `prepare_block`/`cancel_block`, `unblock`'s token rule, the two block sites); `sleep_ticks` uses `BlockedTimer` |
| `docs/kernel/memory/virtual.md` | `:681-720`: `SharedMemoryCreate` returns an id and a handle; share needs a live, delegatable token, is idempotent and respects the foreign reserve. `:726-728`: a region is also destroyed when its creator revokes `SharedMemoryCreate`, exits or destroys it |
| `docs/security/model/capabilities.md` | §3.3: no Create → Access attenuation; expiry clamp. §3.5: access minted as a child at creation; the cascade reaches only the creator's table; revoking `SharedMemoryCreate` destroys the regions created under it; the Kit `grant` refuses object-bound capabilities |
| `docs/kits/kernel/capability.md` | `:291` "child of the authorising `ChannelCreate`"; `:119-122` revoke destroys the channels and regions created under the token or a same-table descendant; the `grant` restriction |
| `docs/kits/kernel/memory.md` | `:127-128` "Both agents must consent": sharing needs no consent from the target, and the 64-slot reserve bounds it. `:135-137`: destroy, a revoke of the create token and the creator's exit invalidate every mapping, seen as EPIPE on the mapper's next call; no `MemoryRevoked` event |
| `docs/kits/kernel/ipc.md`, `shared/src/kits/ipc.rs:183-184` | `channel_create`/`shmem_create` signatures; `shmem_destroy` (`:164-165`): creator plus live token, destroys the region even while others map it |
| `docs/project/developer-guide.md` | `:597` example (old signature, `ChannelId(id as u32)`). `:1025-1037` ipc file tree: `stale_id.rs`, `create_abi.rs`, `share.rs`, `dead_thread.rs`, `wake_token.rs`, `lifecycle.rs` and the loop-test files, with line counts. `:1041-1046` sched file tree: `wake.rs` and the new `scheduler.rs` line count. Test totals `:1579`, `:1631`, `:1724` and the `cap` (`:1729`), `ipc` (`:1731`) and `sched` (`:1736`) rows. `:1792` expected warnings: drop the two `-6` lines, keep `denied ChannelAccess` (3 per boot), and do not list `denied ChannelCreate` as deterministic. `:2060`: the ADR's three rules for a waiting site. `:2061`: keep "Use `try_lock()` in IRQ context, never blocking lock", which crash-fix step 2 replaces (`:440`, `:562`), and append "; call `unblock` only after releasing the lock". New rule: region memory is read only between `region_borrow` and `region_release` |
| `docs/project/ai-agent-context.md` | `:272-302` scheduler files and API: `sched/wake.rs`, `prepare_block`/`cancel_block`, `exit_current`, the token rules, Dead is terminal. `:369-371` `channel_create` signature. Replace the `:457` gotcha "Process exit does not revoke" |
| `docs/phases/03-ipc-and-capability-system.md:189`, `docs/phases/05-kit-foundation.md:248` | `ChannelCreate` returns `ChannelId` and a handle; the Kit `shmem_create` returns `(SharedMemoryId, CapabilityHandle)` |
| `docs/platform/posix.md` | `:705-720` `pthread_join` calls `ProcessWait` with a thread id, and `:655-656` maps `waitpid(-1)` to it. `ProcessWait` now takes only a child pid and is parent-only: `pthread_join` needs a thread-join primitive, and `waitpid(-1)` is unsupported until a wait-any design exists |
| `kernel/src/compositor/service.rs:547`, `kernel/src/ipc/channel.rs:30` | Lock-order comment without `SELECT_WAITERS`; the `timeout_ticks` doc comment |

The crash-fix ADR is amended in the docs PR (§6.4), not in step 15.

-----

## 8. Review log

**First round** (the first draft; correctness, completeness and feasibility passes, checked against b640789 and run 167). Every point was adopted except these, in whole or in part:

- **Clearing `pending_caller` on exit and dropping the dead caller's queued request: not adopted.** Clearing it while the request is in the ring or being served would hand the server's reply to the next caller. The field stays until the reply, which is discarded, and `REPLY_SLOTS` is cleared. The proposed test ("a second caller gets past EAGAIN") contradicts that choice and was not added.
- **"Release `SHARED_REGION_TABLE`, then grant, which closes form 4 by the lock": corrected.** The last-unmap free path takes only `SHARED_REGION_TABLE`, so releasing it before the grant leaves the window open. The grant happens while both locks are held.
- **A per-process on-CPU count before reaping: deferred.** It needs crash-fix 6a's `finish_switch`, and reaping frees nothing today. Recorded as a precondition, also for releasing a killed process's borrows.
- **Fixing `MemoryUnmap`'s private path here: not adopted.** #188 item 1 tracks it with its own design.
- **The compositor's counted reference here: moved to PR #149.** No code on `main` reads attached pixels. The attach access check is adopted. The reference must be a `RegionBorrow`, not a mapping.
- **Kit `grant` minting as a child of a `granted_by` token:** the review's other option, refusing object-bound variants, was taken.
- **Pinning the echo server and client off CPU 0:** a dedicated lifecycle self-test was used instead, so boot tests do not depend on the echo path.

**Resolved by the owner's first-set answers (2026-09-24):** the region cascade (answer 2), the `shared_memory_create` extension (answer 1), and bringing the token forward for `process_wait` (answer 3); the first draft's defaults (a token-only revoke, and #189's window 2 left to crash-fix 6b) were dropped.

**Second round** (2026-09-24, on the revision that followed the first-set answers, which this round turned into the 1,084-line draft; correctness, crash-fix series and completeness passes, checked against aa1f128). Every blocking and important point was adopted, and so was every minor point, except these, in whole or in part:

- **`CLAUDE.md` line numbers: not changed.** At aa1f128 "Lock ordering (full, M25)" is line 98 and "Capability enforcement" line 103, so `:98-102` and `:103-107` stand.
- **A rule to check `WAKEUP_ERRORS` and the deadline before `block_current`: recorded as an optional fast path**, not a rule. With the arm before every waker record, a wake between the arm and the block always leaves a token, and the loop re-reads the condition. Rule 1's rewrite, the two-waker model and its negative control were adopted.
- **The Dead check "cannot detect a broken Dead row": adopted with a narrower reason.** A row that falls through today's `unblock` writes Runnable before it enqueues, so the thread runs and panics in `exit_current`. A row that enqueued without writing Runnable would not be caught, so the direct probe (`thread_probe`) was added.
- **`rg` with `\|`: fixed differently.** In a GFM table cell `\|` renders as a pipe, but agents copy the raw text. Commands use `-e` alternatives, and alternation patterns live outside tables.
- **The exit safe point: adopted; its home was left to the owner** (the 1,084-line draft's open question 3), then decided by the second-set answer 3.
- **Retrying the stale tests and tallying `slot moved`: both adopted**, together with gating pid 14's thread on a start flag set after the stale tests.

**Third revision** (2026-09-24, the owner's second-set answers; this condensation):

- Answer 1 confirmed deferred free; the 1,084-line draft's reading (ADR §5) stands.
- Answer 2 moved the five remaining sites into this PR. The wake-token all-sites design supplied steps 11–14 and the loop contract. Its ADR choices adopted: `SELECT_WAITERS`, `try_wake_select`, `set_select_ready` and `NOTIFY_RESULTS` deleted with sources authoritative; notification records with a kind and a `fired` word; `sleep_ticks` blocking as `BlockedTimer`; a written reply winning in `ipc_call`; EINVAL for a destroyed notification; EAGAIN/ENOMEM for failed select registrations and EPERM for a missing current thread; results derived from state, with `WAKEUP_ERRORS` still taken once per call or recv wait. Not adopted from #186: clearing `WAKEUP_ERRORS` at wait entry.
- **Two design inputs disagreed on the step-1b constraint.** The wake-token design proposed a detect-only "step 0" plus a pair against `main` inside this PR; the amendment design proposed that 1b merge first. The ADR takes "1b first" (no duplicated instrumentation, one soak pair, 1b unchanged) and keeps step 0 as the §6.6 fallback.
- **They also disagreed on the notification-deadline fix.** The amendment design left it in 6b; the wake-token design delivers it in step 11. The ADR delivers it here (§6.5), so 6b keeps N1, N7 and N10.
- **`:497` read through `:496`.** After conversion a timeout racing a reply gives a benign `Skip` from `ipc_reply`, so "reads 0" could fail with no lost wake. The amendment records the reading with an in-place edit of L497.
- Answer 3 put the exit safe point in crash-fix step 8, recorded by the amendment. The 1,084-line draft's "not in step 8's recorded scope" statements, its "until step 6b, no site arms" risks and its "inactive until 6b arms `ipc_call`" remarks were dropped. The wait-word scan the amendment's added acceptance uses is defined as `bw` (§6.2).
- Answer 4 condensed the ADR to about 400 lines and moved the detail here. Steps were renumbered 1–15.

**Fourth revision** (2026-09-24; fidelity, correctness and conventions passes on the condensed texts, checked against b07d7e4, the crash-fix ADR, `scripts/docs/check.py` and the #185 and #189 comments). Every blocking and important point was adopted, and so was every minor point. Adopted with a different mechanism or in part:

- **The converted sites' own tid: a new `current_tid()`**, not `current_thread_and_pid()`. Both are IRQ-masked; `current_tid()` takes only `CURRENT_THREAD`, where the other adds a `THREAD_TABLE` hold per wait for a pid the sites do not use. It adds two DAIF sites (49 in total, `scheduler.rs:11`).
- **ECANCELED:** the preferred `ReplySlot.cancelled` fix, so `WAKEUP_ERRORS` decides nothing and `ipc_call` takes it only at exit. The alternative, listing the extra takes, was not needed.
- **The exit safe point's `prepare_block` check:** the first option, "never leaves the CPU there"; the thread leaves at `block_current` entry or syscall return, and step C, deferred, removes what it published.
- **Plan placement:** the preferred fix. The plan is the implementation branch's first commit, the ADR names it as a code-span path, and step 16 swaps that for a permalink. Keeping it in the docs PR with a baselined `plans-not-empty` entry is left to the owner.
- **The L585 correction:** kept optional, with the reason now stated (§6.4); it is not caused by #185 or #189.
- **L497:** "read through `:496`" was not mechanical. It becomes "reported, not gated; 6b passes on `:496`", marked as this ADR's choice.
- **The "assistant wrote this comment" line:** replaced by what the record shows. The 2026-09-23 comment on #185 is by the owner's account and is not headed as an owner decision.

-----

## 9. Evidence limits

- Line numbers are from b640789 and were re-checked against b07d7e4. Since b640789, `main` changes scripts and documentation not cited at a shifted line (#182, #192), the toolchain (#193), and single comment lines in place in 30 `kernel/` and `shared/` files (#194), none of them a cited line. Nothing was built, tested or booted for the ADR or this plan.
- Boot-behaviour claims come from run 167 (§2.6), before the Bad-id, Bad-pid and select_cap tests existed. The kwarn baseline comes from ten recent boot logs of the #177 and #179 branches, not from a boot of b07d7e4.
- That CPU 0 usually runs the expiring `check_timeouts` first is inferred from `timer.rs:176-194`, not measured; step 1b's counters will measure it.
- The `ProcessControl` size and the SVC 16 stack figures are computed from the field types, not read from an ELF.
- The in-kernel holder list comes from one `rg` at b640789 (§2.2).
- The DAIF inventory comes from `rg -c` at b07d7e4; the counts after steps 8, 10 and 11 are projected.
- Pid 1's token-slot counts are counted from this plan, not measured.
- The host models cover one waiter with one waker, or one waker plus a timeout and a stale waker, with the decisions as pure functions. They do not model `spin::Mutex`, IRQ masking, `on_cpu` or several real wakers.
- The per-wait cost of the token and the loops is not measured; the soak pair records the Gate 1 average.
- Notification and select have no boot user, so their soak counters read 0 by construction; the host model and the boot self-tests are their only evidence.
- A replier killed between taking `pending_caller` and writing `done` leaves a caller with no deadline waiting until something else wakes it. Pre-existing and unchanged in kind.
- The effect on the soak CLEAN rate is unknown until the pair runs.

-----

## 10. Distillation (step 16, before the PR is marked ready)

- **Lessons** (`docs/knowledge/lessons/`): arm before publishing any waker record, timeouts and deadlines included, and loop on the condition; a timeout that fires between a record and the arm loses the wake. The tid given to `prepare_block` must be read with IRQs masked or on a pinned thread. Id layouts need a strict malformed range, so out-of-range sentinels stay invalid. EL1 readers of user memory need a borrow, because a force-unmap cannot remove the direct map. What the soak pair and the self-tests actually showed.
- **Decisions:** anything the implementation changed from the ADR goes into a review note on the ADR, not a new ADR, unless the owner decides otherwise.
- **Review notes and evidence limits.** Copy §8's not-adopted points as one-line entries under a new `## Review notes` in the ADR, and §9's limits that still hold under a new `## Evidence limits`, as the crash-fix ADR keeps them. Update the ADR's evidence where the soak or the boots contradicted it.
- **References.** Replace each mention of this plan's path in the ADR with a permalink to its last version (step 16), so the final ADR never points at a missing file.
- Sort "Issues Encountered", "Decisions Made" and "Lessons Learned" below into the three places above, then delete this plan.

-----

## Issues Encountered

(to be filled during implementation)

## Decisions Made

(to be filled during implementation)

## Lessons Learned

(to be filled during implementation)
