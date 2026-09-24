---
author: jl + claude
date: 2026-09-24
tags: [kernel, security, ipc, memory, sched]
status: final
---

# ADR: Capability lifetime — derived tokens, generation-numbered ids, region cascade, process exit and wake tokens

The owner decided #163 and #185 on 2026-09-23, and answered this ADR's open questions in two sets on 2026-09-24 (#185, #189). This ADR records every decision and the design for implementing them in one PR. The code was read at `main` b640789. `main` is now b07d7e4: since b640789 it changes scripts, documentation, the toolchain (#193) and single comment lines in place in `kernel/` and `shared/` (#194), so every `path:line` below holds. Nothing was built or booted. Implementation choices that the decisions leave open are made here and marked **(ADR choice)**. Per-site change lists, evidence tables, per-step checks, the test plan and the review log are in the companion working plan, `docs/knowledge/plans/2026-09-24-jl-capability-lifetime-plan.md`, which lives only on the implementation branch `claude/cap-lifetime` (its first commit) until the implementation PR, and is deleted there before that PR is ready (step 16). First-set answer 3 and second-set answers 2 and 3 change the crash-fix series; [the crash-fix ADR](2026-09-22-jl-crash-fix-preemption-and-fp.md) carries an amendment note for them. "Crash-fix" below means that ADR, and a bare `:NNN` after a crash-fix reference is a line in it. `shared/`, `docs/` and `CLAUDE.md` paths are from the repository root; other code paths are relative to `kernel/src/`, and a bare file name is unique there.

-----

## Context

**#163.** `channel_create` (`kernel/src/ipc/mod.rs:177-197`) checks `ChannelCreate` but grants no `ChannelAccess(new_id)`, so the creator cannot call, receive on or destroy its own channel. The Phase 3 timeout/destroy self-test (`kernel/src/ipc/tests/mod.rs:446-486`) has got EPERM on every boot since M11. Services work only because boot code grants access ambiently with `cap::grant_to_process`. Separately, `Capability::can_attenuate_to` (`shared/src/cap.rs:110`, `:112`, `:114`) lets any holder of `ChannelCreate`, `SharedMemoryCreate` or `GpuBufferCreate` attenuate to `…Access(any id)`, and SVC 15 exposes this (`kernel/src/syscall/mod.rs:337-350`). That is harmless while every thread runs at EL1 and a privilege escalation once EL0 exists.

**#185.** Capabilities outlive the objects and processes they name. Ids are bare slot indices (`shared/src/ipc.rs:46-70`), freed slots are reused lowest-first (`ipc/mod.rs:184-191`, `ipc/shmem.rs:136-145`), and `permits` compares the bare `u32` (`shared/src/cap.rs:79-85`). There are four forms:

1. Freed shared-memory regions keep their `SharedMemoryAccess` tokens.
2. Destroyed channels keep their `ChannelAccess` tokens.
3. `process_exit` (`task/process.rs:125-226`) leaves the process slot and its `cap_table` live, and the scheduler can move a Dead thread back to Running.
4. `shared_memory_share` checks the creator under `SHARED_REGION_TABLE` and grants only after dropping that lock (`shmem.rs:421-445`).

None is exploitable while all threads run at EL1 and `ProcessCreate` and `CapabilityTransfer` return ENOTSUP. All become exploitable once EL0 processes exist.

**Related.** #189: `process_wait` has no parent check, one overwritable waiter slot, two lost-wakeup windows, an `i32::MIN` sentinel clash and exit codes in the errno register. #186: `ipc_select` drops lookup errors, leaves wake results behind, and can miss an event between its scan and its registration. Crash-fix N2 and N3 (`:98-99`) are the same register-then-block gap in `ipc_call`, `ipc_recv`, `sleep_ticks` and notification waits. #187 (`ThreadId` has no generation) and #188 (EL0 syscall hardening) are touched here but not fixed.

-----

## Decisions

### Owner, 2026-09-23 (#163, #185)

1. **#163: derived token.** `channel_create` keeps `ChannelCreate` and `ChannelAccess` as distinct types and tokens. It atomically mints `ChannelAccess(new_id)` for the creator as a child of the authorising `ChannelCreate` token and returns it. Revoking `ChannelCreate` cascades to the tokens it minted. Peers get access only by delegation (the Service Manager, or `CapabilityTransfer` once implemented), never by ambient `grant_to_process`. The `can_attenuate_to` arms `ChannelCreate → ChannelAccess(any)`, `SharedMemoryCreate → SharedMemoryAccess(any)` and `GpuBufferCreate → GpuBufferAccess(any)` are removed. Expected side effect: the Phase 3 timeout/destroy self-tests stop getting EPERM.
2. **#185 option A: generation in ids.** `SharedMemoryId` and `ChannelId` carry slot + generation, with the generation in the high bits. The slot's generation is bumped on every free. Lookups reject a mismatch, so stale tokens stop matching with no cross-process sweep, and form 4 closes.
3. **#185 process lifecycle, confirmed as proposed.** `ProcessControl` gets an `Alive`/`Exited` state. `process_exit` revokes the `cap_table` under `PROCESS_TABLE` and then runs the channel cascade. `process_wait` reaps the slot once the exit code is collected. Process creation resets `EXIT_CODES` and `PROCESS_WAITERS` for the slot before publishing it. The scheduler never moves a Dead thread back to Runnable or Running (`thread_yield`, `block_current`, `unblock`), and `sys_process_exit` switches away instead of returning. Before process slots are reused, `ProcessId` gets a generation, or reuse is refused while anything references the pid.

### Adopted from a comment on #185, 2026-09-23

This comment is not headed as an owner decision. Its notes were adopted:

- #189 is fixed in the same PR, because reaping in `process_wait` makes the missing parent check worse. The owner's first-set answer 3 later made this the owner's decision and extended it to window 2.
- `ipc_select` must propagate the new generation-mismatch error (#186 item 3).

### Owner answers, 2026-09-24, first set (#185, #189)

1. **Derived minting extends to `shared_memory_create`.** The creator's `SharedMemoryAccess` is minted as a child of the authorising `SharedMemoryCreate` token and returned. SVC 20 returns the handle in x1, and the Kit `shmem_create` signature changes.
2. **Revoking `SharedMemoryCreate` destroys the regions created under it**, as revoking `ChannelCreate` destroys channels. The cascade force-unmaps every mapper, frees the region and bumps its generation.
3. **#189 is fixed in this PR, including window 2**: the lost wakeup between registering in `PROCESS_WAITERS` and `block_current`. The wake token that the crash-fix series planned for step 6b (F5) is brought into this PR.

### Owner answers, 2026-09-24, second set (#185)

1. **Deferred free.** When a region is destroyed while a kernel `RegionBorrow` exists, the force-unmap and the generation bump happen at once, and the frames are freed when the last `region_release` runs (§5).
2. **Every blocking path converts in this PR**: `ipc_call`, `ipc_recv`, `sleep_ticks`, `notification_wait` and `ipc_select`, as well as `process_wait`. Crash-fix step 1b's N2 measurement constraint is resolved first (§10), and crash-fix step 6b's scope shrinks accordingly.
3. **The exit safe point is added to crash-fix step 8's scope**, recorded as an amendment note in the crash-fix ADR (§7).
4. **Format.** The ADR is condensed to about 400 lines. The detailed evidence and the per-site plan go in a companion working plan under `docs/knowledge/plans/` (see "Companion plan and crash-fix amendment").

Every open question is decided, so the status is `final`. **Delivery (owner plan):** this ADR, then one implementation PR.

-----

## Design

### 1. Id encoding (#185 A)

One layout serves `ChannelId` and `SharedMemoryId`, and later `ProcessId` **(ADR choice)**.

| Bits | Field | Values |
| --- | --- | --- |
| 0–7 | slot | `ChannelId`: 0–127 valid, 128–255 malformed. `SharedMemoryId`: 0–63 valid, 64–255 malformed |
| 8–31 | generation | 1 ..= 0xFF_FFFF issued. 0 is never issued; it marks a retired slot |

```rust
// shared/src/ipc.rs
pub const ID_SLOT_BITS: u32 = 8;
pub const ID_SLOT_MASK: u32 = (1 << ID_SLOT_BITS) - 1;      // 0xFF
pub const ID_GEN_MAX: u32 = (1 << (32 - ID_SLOT_BITS)) - 1; // 0xFF_FFFF
pub const ID_GEN_FIRST: u32 = 1;
pub const ID_GEN_RETIRED: u32 = 0;
// Strict: slot 255 is always malformed, so u32::MAX never decodes to a valid id.
const _: () = assert!(MAX_CHANNELS < 1 << ID_SLOT_BITS);
const _: () = assert!(MAX_SHARED_REGIONS < 1 << ID_SLOT_BITS);

/// Generation after a free. Saturation retires the slot for good.
pub const fn next_generation(g: u32) -> u32 {
    if g == ID_GEN_RETIRED || g >= ID_GEN_MAX { ID_GEN_RETIRED } else { g + 1 }
}
```

- **Why 8/24.** Every out-of-range value that the tests and sentinels use stays malformed: the `MAX_*` bounds in the #175/#177 EINVAL tests, the bench's `u32::MAX` sentinel (`bench.rs:145`), and `bad_pid.rs`'s pid 32 once `ProcessId` adopts the layout. Other raw values of 256 or more that are EINVAL today can decode to a valid slot and generation (256 is slot 0, generation 1). It gives 16,777,215 generations per slot. An exact 7-bit slot field would make every `u32` decode to a valid slot, and 16/16 gives only 65,535 generations.
- **Id types.** The tuple field becomes private **(ADR choice)**, so the compiler finds every site that builds an id from a slot number. Each type gets `new(slot, generation)` (kernel allocation only), `from_raw` (registers, wire structs, tests), `raw`, `generation`, and `index() -> Option<usize>`, which is `None` for a malformed slot and keeps the #175 contract. `Capability::SharedMemoryAccess(u32)` becomes `SharedMemoryAccess(SharedMemoryId)` **(ADR choice)**, so `permits` compares full ids with no change.
- **Where generations live.** Inside the existing mutex payloads: `ChannelSlot { generation, channel }` and `RegionSlot { generation, region, draining }` (§5). There is no new lock static and no separate atomic array, which would let a concurrent create read an old generation. Every slot starts at `ID_GEN_FIRST`.
- **Allocation** stays lowest-first **(ADR choice)** and skips retired slots and draining region slots. Each table has one free function that takes the object and advances the generation in one critical section: `channel_destroy_unchecked` (`ipc/mod.rs:220-239`) and `region_detach` (§5).
- **Wrap: retire (ADR choice).** A slot freed at `ID_GEN_MAX` becomes `ID_GEN_RETIRED` for the rest of the boot, and create returns ENOSPC once no usable slot is left. This fails closed: a stale token can never match again.
- **Lookup.** `channel_slot_mut` returns EINVAL for a malformed slot, and `channel_mut` returns EPIPE when the slot is empty or `ch.id != id` (the full `u32`). `channel_destroy_unchecked` makes the same identity check before its `take()` (`ipc/mod.rs:222-224`), so a stale destroy cannot tear down the channel now in that slot. A new `region_mut` in `shmem.rs` does the same for regions, treating a draining slot as empty. It serves map, unmap, share and the Kit `shmem_destroy`, which stops indexing the table directly (`ipc/mod.rs:469`). `ipc_select` propagates lookup errors instead of dropping them (`select.rs:192`, `:232`).
- **Failure order** for a channel or region op: EINVAL (malformed slot), then EPERM (no live token for the full id), then EPIPE (empty, draining or another generation). EPIPE keeps the "destroyed or never created" meaning of `channel_mut`, does not reveal that another process reused the slot, and needs no new errno (`IPC_ERROR_COUNT` stays 13).

### 2. Derived access tokens (#163; first-set answer 1)

`shared/src/cap.rs` gains these host-tested primitives:

```rust
/// Mint `capability` as a child of the live token `parent`. Kernel-only; ignores can_attenuate_to.
pub fn mint_child(&mut self, parent: CapabilityTokenId, capability: Capability,
                  holder: ProcessId, id: CapabilityTokenId, now_tick: u64)
    -> Result<CapabilityHandle, i64>;   // EPERM: parent missing, revoked or expired. ENOSPC: table full
pub fn free_slots(&self) -> usize;                        // empty slots; revoked tokens keep theirs
pub fn is_revoked(&self, id: CapabilityTokenId) -> bool;  // true for a missing id: cascades fail closed
pub fn revoke_all(&mut self) -> u32;
pub fn clear(&mut self);                                  // reset in place, for install and reap
```

The child gets `parent_token = Some(parent)`, the parent's `delegatable` and `expires_at_tick`, and `created_at_tick = now_tick`. Inheriting `delegatable` and expiry **(ADR choice)** never widens what the parent allows, and the creator needs `delegatable` to share (`docs/kernel/ipc.md` §4, §4.6).

`cap::create_with_access(pid, create, publish)` runs under one `PROCESS_TABLE` hold:

1. `process_mut(pid)`: EPERM unless the process is Alive.
2. `find_authorizing_token(&create, now)`: on a miss, log today's denial line and return EPERM.
3. `free_slots() >= 1`, else ENOSPC. Nothing has been published.
4. `publish(auth)` takes `CHANNEL_TABLE` or `SHARED_REGION_TABLE`, allocates a slot, inserts the object with `auth` as its `creation_cap`, releases, and returns the object and the access capability for the new full id. A full table returns ENOSPC and frees nothing. `publish` never takes `PROCESS_TABLE` and never wakes a thread.
5. `mint_child(auth, access, pid, new_token_id(), now)`. It cannot fail after step 3; an `Err` panics.

This closes today's check-then-insert window (`check_channel_create`, `cap/mod.rs:70-87`, releases `PROCESS_TABLE` before `ipc/mod.rs:183`): a concurrent revoke or exit runs wholly before step 1 or wholly after step 5.

- **`channel_create(creator: ThreadId) -> Result<(ChannelId, CapabilityHandle), i64>`** resolves the pid with `process_of_thread` first, as today, then calls the helper. `Channel` gains `creator: Option<ProcessId>`, `None` for kernel channels.
- **`shared_memory_create(pid, size, flags) -> Result<(SharedMemoryId, CapabilityHandle), i64>`.** `check_shared_memory_create` (`shmem.rs:108`) stays as a cheap pre-check before pages are allocated. Its token id is discarded, and the region records the token found under the hold, so a revoke between the two cannot leave a region that outlives its token. On `Err` the caller frees the pages once, outside `PROCESS_TABLE`. This replaces the parentless post-lock grant (`shmem.rs:162-174`). `SharedMemoryRegion.creation_cap` becomes a plain `CapabilityTokenId`.
- **Return ABI.** `ChannelCreate` (6): x0 is the channel id, a zero-extended `u32`; x1 is the `CapabilityHandle` of the minted `ChannelAccess`. `SharedMemoryCreate` (20): x0 is the region id and x1 the handle. On error x0 is the errno and x1 is not written. `sys_channel_create` and `sys_shared_memory_create` take `&mut TrapFrame`. The syscalls return handles because SVC 15 and 16 take handles. Kit `ChannelOps::channel_create` and `SharedMemoryOps::shmem_create` return `(id, CapabilityHandle)`.
- **Revocation (ADR choice).** Revoking a token through SVC 16 (`revoke_in_process`) or the Kit `revoke` revokes its same-table descendants (`CapabilityTable::revoke`). In the same `PROCESS_TABLE` hold it then runs the region cascade (§5) and the channel cascade. The channel cascade collects the full id of every channel with `creator == Some(pid)` whose `creation_cap` is now revoked (`is_revoked`) and destroys each with `channel_destroy_unchecked` after both locks are released, because destroying wakes threads. It replaces `revoke_channels_for_cap` (`cap/mod.rs:373-395`), which matched only the revoked token and missed channels created under a descendant `ChannelCreate`. `revoke_in_process` uses `process_mut` and returns EPERM for a pid that is not Alive. At exit, `revoke_all` replaces `revoke`, step A runs the region cascade in its `PROCESS_TABLE` hold, and the channel half runs in §6 step C instead.
- `channel_destroy` and the Kit destroy do not revoke the minted token. The generation makes it stale.

### 3. Attenuation and grants (#163)

- Delete `shared/src/cap.rs:110`, `:112` and `:114`. `can_attenuate_to` reduces to `permits`. SVC 15 types 1 and 3 work only from an Access token for the same full id, and return EPERM from a Create token.
- **Expiry clamp.** `attenuate` takes `now_tick`, returns EPERM when the parent has expired, and gives the child the earlier of the parent's and the requested expiry (`docs/security/model/capabilities.md` §3.3). Today an identity child of an expiring token can be made never to expire.
- **Kit `grant` (ADR choice)** refuses `ChannelAccess`, `SharedMemoryAccess` and `GpuBufferAccess` with `NotGranted`. Object-bound tokens come only from minting (§2) or delegation (§4).

### 4. Delegation and object-bound checks

**`shared_memory_share` becomes delegation**, in one `PROCESS_TABLE → SHARED_REGION_TABLE` hold:

1. EINVAL: a malformed region slot or `target_pid` out of range, checked before any lock.
2. EPERM: the caller's process is not Alive; it holds no live `SharedMemoryAccess(full id)`; or the target is another process and that token is not delegatable.
3. EPIPE if the slot is empty, draining or of another generation; EPERM if the caller is not the region's creator.
4. EPERM if the target is not Alive. `Ok` with no new token if the target already holds a live token for the full id (idempotent). ENOSPC if the target is another process and has `CAP_FOREIGN_RESERVE` or fewer free slots. Otherwise grant `SharedMemoryAccess(full id)` with `delegatable: false` and `parent_token = Some(caller's token)`.

The grant happens while both locks are held, so form 4 closes by the lock, with the generation as a second line. Steps 2 and 4 are one pure host-tested function, `share_decision` **(ADR choice)**.

- **Foreign-grant reserve (ADR choice).** `CAP_FOREIGN_RESERVE = 64` of the 256 slots. A token that another process causes to be granted into a table cannot use its last 64 free slots; the process's own mints and init-time grants can. Without it, share could fill a victim's table and permanently block its `channel_create`, which now needs a slot.
- **`shared_memory_map`** checks the token and inserts the mapping in one `PROCESS_TABLE → SHARED_REGION_TABLE` hold. EPERM for a caller that is not Alive or has no live token; EPIPE for a destroyed, draining or stale region, even when the caller's shared token is live.
- **Kit `shmem_destroy`** has no authorisation today. It now requires the caller to be the creator and to hold a live `SharedMemoryAccess(full id)`, checked in the same hold that calls `region_detach` (§5).
- **Compositor attach.** `handle_attach_buffer` (`compositor/service.rs:790-800`) calls `check_shared_memory_access(owner_pid, id)` before `surface_attach_buffer` and returns the zeroed error event on EPERM or EPIPE. Region ids are not secret.

### 5. Region cascade with deferred free (first-set answer 2; second-set answer 1)

A region is destroyed when its `creation_cap` is revoked in its creator's table: a revoke of that token or of any same-table ancestor. A process that holds two independent `SharedMemoryCreate` tokens loses only the regions of the revoked one. Expiry destroys nothing.

```rust
struct RegionSlot {
    generation: u32,
    region: Option<SharedMemoryRegion>, // SharedMemoryRegion gains kernel_borrows: u32
    draining: Option<DrainingFrames>,   // detached, frames still borrowed
}
struct DrainingFrames { id: SharedMemoryId, base_phys: usize, order: usize, borrows: u32 }
```

- **One free function.** `region_detach(slot)` runs with `SHARED_REGION_TABLE` held and is the only way a region leaves its slot. It drops the region together with its `mappings`, which force-unmaps every mapper at once, and advances the generation. With no kernel borrow it returns `(base_phys, order)`, and the caller frees the frames after releasing every lock. Otherwise it stores `DrainingFrames`, and the last `region_release` frees them. Its callers are the last unmap, exit cleanup, the cascade and the Kit destroy.
- **`revoke_regions_for_cap(pid, caps, out)`** runs inside the revoking `PROCESS_TABLE` hold with `SHARED_REGION_TABLE` nested. It detaches every region with `creator == pid` whose `creation_cap` is revoked in `caps`, skips draining slots, and leaves other creators' regions alone. It takes the table, not a token id, so descendants are covered. Region frames are always freed with no table lock held, so a region free never nests `FRAME_ALLOC` under either table.

**Kernel borrows (ADR choice).** EL1 reads region memory through the direct map, which a force-unmap cannot remove, so freeing frames that a kernel thread still reads would be a use-after-free. Kernel code dereferences a region's address only between `region_borrow` and `region_release`:

```rust
#[must_use]
pub struct RegionBorrow { id: SharedMemoryId, addr: usize, len: usize } // not Clone, not Copy
// RegionBorrow::addr(), len(), and is_live() (slot.region.id == id, under SHARED_REGION_TABLE)
/// EINVAL: malformed slot. EPIPE: empty, draining or stale. EPERM: `pid` has no mapping. ENOSPC: count overflow.
pub fn region_borrow(pid: ProcessId, id: SharedMemoryId) -> Result<RegionBorrow, i64>;
pub fn region_release(b: RegionBorrow); // the draining record's last release frees the frames, after the lock
```

They replace `region_dmap_addr` and `region_size` (`shmem.rs:623-641`), which are deleted. The release is explicit, not a `Drop`, because an implicit release inside a `SHARED_REGION_TABLE` hold would self-deadlock. A draining slot is never reallocated, so a release never meets a newer region. The only holder at boot is the bench (`bench.rs:280-341`), which becomes map → borrow → loops → release → unmap. PR #149's compositor must hold a borrow while a buffer is attached.

- **Creator exit destroys its regions**, including regions that live processes still map; their `map` and `unmap` calls then return EPIPE. This is the channel rule, and it replaces `docs/kernel/ipc.md:657-662` and `:848`. Memory that must outlive its producer is created by the longer-lived side or brokered by the Service Manager.
- **Revocation discipline.** A Create token is revoked only through `revoke_in_process`, the Kit `revoke` or exit, the paths that run both cascades. A direct `CapabilityTable::revoke` (today `bad_pid.rs:133-138`) would leave the region alive.
- **EL0.** Today the detach is the whole force-unmap, because `shared_memory_map` writes no page table (`shmem.rs:274-279`). Once it does, every `region_detach` caller and every non-final unmap must clear the mappers' TTBR0 PTEs and issue `tlbi vae1is` (or `tlbi aside1is`) and `dsb ish` before the frames are freed or drained, and the last unmap and exit step D then take `PROCESS_TABLE → SHARED_REGION_TABLE` instead of `SHARED_REGION_TABLE` alone.

### 6. Process lifecycle (#185; #189)

- **State (ADR choice).** `enum ProcessState { Empty, Alive, Exited { code: Option<i32> } }` lives in `shared/src/sched.rs`, with `may_become` for the four legal transitions: Empty → Alive, Alive → `Exited { None }`, `Exited { None }` → `Exited { Some }`, and `Exited { Some }` → Empty. The table becomes `[ProcessControl; MAX_PROCESSES]` with `state`, `parent: Option<ProcessId>` and `ever_used`, reset in place: a `ProcessControl` is about 18 KiB and must never move onto a 32 KiB kernel stack. `EXIT_CODES` is removed and the code lives in the state, which removes the `i32::MIN` sentinel (#189 d).
- **Accessors.** `process_mut` and `process_ref` return EPERM unless the slot is Alive, so every capability path fails closed for an exited pid. `process_install(pid, parent, name, limits)` returns EEXIST unless the slot is Empty and never used (reuse is refused until `ProcessId` has generations), resets it in place, clears `PROCESS_WAITERS[idx]`, and publishes Alive. The eight literal installs switch to it.
- **`process_exit(pid, code)`** runs in five steps, holding one lock at a time except the pairs named. The victims are every thread with `owner_pid == pid`, the caller included when it belongs to `pid`:
  - **A** (`PROCESS_TABLE`, `SHARED_REGION_TABLE` nested): EINVAL out of range; EPERM for pid 0 or an Empty slot; `Ok` if already exited. Otherwise set `Exited { code: None }`, `revoke_all`, and run the region cascade. Frames are freed after the release.
  - **B** (IRQs masked, `sched::kill_process_threads`): read the caller's tid after masking. In one `THREAD_TABLE` hold, mark every victim Dead and reset its wait word (§8), leaving out only the caller, and snapshot the victims for C and E.
  - **C** (channels and thread-keyed state, for every victim): under `CHANNEL_TABLE`, collect the channels with `creator == Some(pid)`, run the existing endpoint walk, and clear any `waiting_receiver` that names a victim. After the release, destroy the collected ids and deliver EPIPE. Then clear each victim's timeout (a blocking clear, not today's `try_lock`), `REPLY_SLOTS` entry, notification deadline, and notification and select waiter records. A `pending_caller` that names a victim stays until the server replies; the reply finds no reply slot, and its `unblock` of a Dead thread does nothing **(ADR choice)**.
  - **D:** `process_cleanup_shared_memory` removes the pid's mappings of other creators' regions, through `region_detach` when a last mapping goes; then `service_on_death`.
  - **E** (`PROCESS_TABLE → PROCESS_WAITERS`): publish `Exited { code: Some(code) }` last, copy the registered waiter and leave its entry set, and clear every entry that names a victim (#189 e). Release, then `unblock(waiter)`.
- **`process_wait(parent_tid, caller_pid, child)`** loops on `wait_step`, which runs under `PROCESS_TABLE → PROCESS_WAITERS`: EINVAL out of range; EPERM if the caller is not Alive; EPERM if the child is Empty or `child.parent != Some(caller_pid)` (#189 a); EEXIST if another thread is registered, before and after the exit (#189 b). On `Exited { code: Some(c) }` it clears the entry, reaps in place (`state = Empty`, `cap_table.clear()`, `address_space = None`, `parent = None`; `ever_used` stays set, so `process_install` refuses the slot) and returns `c`. Otherwise it calls `prepare_block`, registers, releases and returns `None`, and the caller blocks as `BlockedProcessWait`. The decision is a pure host-tested `process_wait_action` **(ADR choice)**. `process_wait` takes no `THREAD_TABLE` hold. It has no timeout **(ADR choice)**; `ipc.md:280-283`'s field is marked not implemented.
- **Syscalls.** `sched::current_thread_and_pid()` reads the caller's tid and pid in one IRQ-masked section. `sys_process_exit` calls `process_exit` and, on `Ok`, `sched::exit_current()`; on `Err` it returns the errno. `sys_process_wait` takes `&mut TrapFrame` and returns x0 = 0 and x1 = the exit code, sign-extended, or x0 = errno with x1 not written (#189 f).
- **Reapable is not off-CPU.** Reaping frees nothing today. Before `UserAddressSpace` gets a `Drop` or ASID release, reaping must wait for a per-process on-CPU count, which no crash-fix step delivers yet (it can build on step 6a's per-thread `on_cpu`).

### 7. Dead is terminal; the exit safe point (#185; second-set answer 3)

`ThreadState::may_become(next)` forbids leaving Dead and is host-tested over every pair. Every state writer is gated on it: `thread_yield` skips its Running write; `block_current` skips its write for a Dead thread (`AlreadyDead`, §8); `unblock` returns early; the pick in `schedule()` and `enter_scheduler` drops a picked Dead thread; `try_direct_switch` returns `false` for a Dead sender; `try_reply_switch` switches away from a Dead replier without making it Runnable or queueing it. `pub fn exit_current() -> !` blocks as Dead and replaces the three "mark self Dead, then yield" loops (`service/mod.rs:314-325`, `gpu/service.rs:177-188`, `compositor/service.rs:186-197`). `process_exit` never marks its caller Dead; the caller calls `exit_current()` next.

**The exit safe point.** A victim that is running on another CPU keeps running until its next switch. It can hold a lock that is never released, which crash-fix step 8's preempt count removes. It can also re-publish state that step C cleared: `waiting_receiver`, `pending_caller`, `REPLY_SLOTS`, a notification or select waiter record, a timeout or deadline, or an armed wait word. The owner added the fix to crash-fix step 8 (the crash-fix amendment of 2026-09-24): an exit-pending flag, set where B marks the thread Dead and checked at `irq_exit`, at syscall return, and on entry to `prepare_block` and `block_current`. A flagged thread leaves the CPU as Dead only where it holds no lock: at `irq_exit` where the context is preemptible, at syscall return, or on entry to `block_current`. `prepare_block` may be called inside the publishing critical section (rule 1, §8, allows inside or before it), so on entry to `prepare_block` the check does not arm and never leaves the CPU; the thread leaves at its next `block_current` or syscall return **(ADR choice)**. Step C is deferred until every victim has reached a safe point, so it also removes anything a victim published before then. **Step 8's added acceptance (ADR choice):** a boot self-test exits a process whose thread is running on another CPU; that thread reaches a safe point, and afterwards no waiter record of it remains. **Interim rule until step 8 lands:** `process_exit` is called only when no victim other than the caller can be on a CPU: none of another process's threads, and in a self-exit none of the caller's sibling threads. The lifecycle self-tests exit thread-less processes. The echo client's `process_exit(ProcessId(7), 0)` is a self-exit whose sibling, the pid-7 echo server, may be running on another CPU (`service/mod.rs:234`, `:255`, `:371`); it predates this PR, was never reached in run 167, and is the one known exception. No new caller of that kind is added.

### 8. Wake token (#189 window 2; crash-fix F5)

`static WAIT_STATE: [AtomicU8; MAX_THREADS]` in a new `kernel/src/sched/wake.rs` holds one `WaitState` per thread slot **(ADR choice)**. It is outside `THREAD_TABLE`, so a thread arms without a lock. It is not a lock, so it has no class or rank. Atomic read-modify-write is safe, because the array is WB-mapped `.bss` and threads run only after M8. `allocate_thread` resets the word. This is Linux's `set_current_state` / `try_to_wake_up` split.

| `WaitState` | Meaning |
| --- | --- |
| `Idle` | Not in a wait |
| `Armed` | The thread recorded "about to block" and may already be published as a waiter, but has not blocked |
| `Woken` | A waker found the thread Armed and not Blocked, and left the token |

- **Own tid.** `prepare_block` and `cancel_block` take the tid as an argument, because the kernel has no lock-free read of the current thread before crash-fix step 8. It must be the caller's own tid. `current_thread_id()` reads `core_id` and then `CURRENT_THREAD` with IRQs on (`timeout.rs:116-118`, crash-fix N4), so a migration between the two reads names another CPU's thread. Each converted site therefore reads `me` once per wait through `sched::current_tid()`, a new IRQ-masked read of `CURRENT_THREAD[core_id()]` **(ADR choice)**; SVC 25 uses `current_thread_and_pid()` (§6), and the self-tests run on threads pinned to one CPU.
- **Operations.** `prepare_block(tid)`: `swap(Armed)`, debug-asserting that the old value was Idle. `cancel_block(tid) -> bool`: `swap(Idle)`, returning whether a token was pending. `wake::reset(tid)`: `store(Idle)`, used by `allocate_thread`, exit B, `block_current(Dead)` and `AlreadyDead`. Every access is `Relaxed` **(ADR choice)**: wakers find waiters through a lock that the arm precedes, and the wake CAS and the consume swap both run inside `THREAD_TABLE` critical sections, so they are totally ordered.
- **`unblock`** applies `shared::wake_action(state, word)` inside its existing hold: Dead → `Dead` (nothing); Running or Runnable with Armed → `LeaveToken` (CAS to Woken, no enqueue); with Woken → `TokenPending`; with Idle → `Skip`; a Blocked state or Suspended → `Enqueue`, as today.
- **Block sites.** Only `block_current` and `try_direct_switch` set a thread Blocked, and both apply `shared::block_action(state, word, next)`: `AlreadyDead` (reset, `schedule()`); `next == Dead` (reset, write Dead: a token never makes `exit_current` return); `ConsumeToken` (the swap returned Woken: no write, no switch); `Sleep`. `try_direct_switch` returns `false` for a Dead or Woken sender inside its existing receiver check. The token adds no `DAIFClr` site, lock static or nesting edge.
- **Invariant.** A word that is not Idle belongs to a Running or Runnable thread between `prepare_block` and the `block_current` or `cancel_block` that ends its wait. There is one transient exception until crash-fix step 8's safe point: a victim still running after exit B can arm at a site that does not hold `PROCESS_TABLE`, and its word stays Armed until its next `block_current`, whose `AlreadyDead` branch resets it.
- **Rules for a waiting site.** They go in the `prepare_block` doc comment and replace `docs/project/developer-guide.md:2060`.
  1. Arm inside, or before, the critical section that publishes the first record any waker can use to find you: a `TIMEOUT_QUEUE` entry, a notification deadline, `pending_caller`, `waiting_receiver`, a notification or select waiter record, or a `PROCESS_WAITERS` entry.
  2. Once armed, call `block_current` or `cancel_block`.
  3. After `block_current` returns, re-check the condition under its own lock and loop. Each pass re-arms before it re-publishes a waker record.
- **`process_wait`** arms in `wait_step`'s hold, under the locks that E publishes under, which closes window 2: if E's `unblock` lands between the release and `block_current`, it leaves a token, and the next pass reaps. A stale wake costs one pass instead of returning EPERM.

### 9. Every waiting path arms (second-set answer 2)

Each site arms and gains its loop in the same step. The per-site passes, exits and tests are in the plan.

| Site | Arms | Blocks as | Exit, first match | Timeout |
| --- | --- | --- | --- | --- |
| `process_wait` | In `wait_step`'s hold | `BlockedProcessWait` | §6 | none |
| `notification_wait` | In the `NOTIFICATION_TABLE` hold, before its record | `BlockedNotification` | object gone (EINVAL); record fired; bits set; deadline | `NOTIFY_DEADLINES` |
| `ipc_select` | Before the per-source pass | `BlockedSelect` | first entry, by index, that is ready or fails; deadline | `NOTIFY_DEADLINES` |
| `sleep_ticks` | Before its entry | `BlockedTimer` (was `BlockedIpc { u64::MAX }`) | deadline | `TIMEOUT_QUEUE`, re-inserted |
| `ipc_recv` | In the `CHANNEL_TABLE` hold, before `waiting_receiver` | `BlockedIpc` | gone or Dead (EPIPE); message; receiver slot taken (EAGAIN); deadline | `TIMEOUT_QUEUE`, re-inserted |
| `ipc_call` | In the `CHANNEL_TABLE` hold, before `pending_caller`; its `REPLY_SLOTS` entry, with new `done` and `cancelled` flags, is written first | `BlockedIpc`, or a direct switch | reply `done`; gone or Dead (EPIPE); claimed and `cancelled` (ECANCELED); deadline | `TIMEOUT_QUEUE`, re-inserted |

- **Results come from state (ADR choice).** ETIMEDOUT means `TICK_COUNT >= deadline`. EPIPE means the channel is gone or Dead. ECANCELED means `ReplySlot.cancelled`, which `ipc_cancel` sets under `REPLY_SLOTS` after taking `pending_caller` and before its `unblock` (it no longer calls `wake_with_error`), honoured only once `pending_caller` has been claimed. `WAKEUP_ERRORS` decides nothing: a wait that `wake_with_error` can reach takes it once at exit and discards it. A stale wake or a stale error code costs one pass, not a wrong result. `ipc_call` and `ipc_recv` take `WAKEUP_ERRORS` once per wait, at exit, as today; `ipc_select` adds one take at exit; `ipc_cancel` drops its take. On a boot path only step 11's new `Select-loop` self-test reaches select's exit take: the select_cap self-test's six calls return at validation or at a scan that runs before the arm. So before crash-fix step 2 makes that IRQ-spun lock IRQ-class, thread-side holds of it (crash-fix H3) grow on a boot path only by the `Select-loop` test's exits, at most three per boot. In `ipc_call` a written reply wins over a concurrent timeout or destroy **(ADR choice)**. `timeout_ticks == 0` means no timeout for `ipc_call`, as the code does today (the doc comment at `channel.rs:30` is fixed), and a poll for `ipc_recv`; `u64::MAX` no longer overflows (`channel.rs:119`, `:161`).
- **Direct switch.** An armed receiver fails `try_direct_switch`'s BlockedIpc check, and an armed caller fails `try_reply_switch`'s. The fallback `unblock` then leaves the token. That is N2's fix.
- **Select and notifications (ADR choice).** After the arm, select checks and registers each source in one critical section, which closes #186 item 4. The sources are authoritative: a waker claims a registration and calls `unblock`, and the select re-scans. `SELECT_WAITERS`, `try_wake_select`, `set_select_ready` and `NOTIFY_RESULTS` are deleted. A notification waiter record gains a kind and a `fired` word: for a wait record the signaller moves the bits into `fired`; for a select record it removes the record without consuming the bits. A failed registration returns EAGAIN (receiver slot taken) or ENOMEM (no free waiter slot) (#186 item 2). A missing current thread returns EPERM (#186 item 5). Select consumes its `WAKEUP_ERRORS` value (#186 item 1). With §1's lookup errors (item 3), #186 closes.
- **Notification timeouts (ADR choice).** `check_notification_timeouts` collects the expired tids under `NOTIFY_DEADLINES.try_lock`, resets them, and calls `unblock` only after releasing the lock. It no longer tests thread state or cleans up waiter records, so no expiry is dropped (crash-fix F5's third bullet, `:180`), and `NOTIFY_DEADLINES` becomes the leaf that `deadlock-prevention.md` §3.3 already lists.
- **Guard.** From the `ipc_call` step on, every `block_current` caller except `exit_current` arms, and `block_current` debug-asserts that a block to any state other than Dead finds the word Armed or Woken. The assertion applies only when the current state is not Dead: `AlreadyDead` is decided first, because exit B resets a victim's word, and a victim that armed before B still calls `block_current`.
- **Every pass ends in `block_current` or `cancel_block`.** A pass that finds its deadline passed after arming, including `ipc_call`'s first pass, calls `cancel_block` before the next pass or the exit, so `prepare_block`'s Idle assertion holds (`just run` and the soak use the dev profile, with debug assertions on).

### 10. Measurement: crash-fix step 1b lands first (second-set answer 2)

Crash-fix step 1b counts "`unblock` skipping a Running or Runnable target, by caller" (`:420`) and scans for threads "blocked with no waker" (`:423`), and the series requires every mechanism's counter to be seen above 0 before its fix (`:539`). Converting the sites before 1b would fix N2 and N3 before any counter existed.

**Resolution (ADR choice): step 1b merges before this PR,** and both arms of this PR's soak pair carry 1b's counters. The before arm, post-1b `main`, counts N2 and N3 as skips by caller on unconverted waits. The after arm counts a wake that finds its target armed as `LeaveToken`. The 1,084-line draft's order (after 1a, before 1b) is dropped. Step 1b's content and acceptance do not change. A source that reads 0 in all 30 boots of the before arm is recorded as "not observed at n = 30"; its conversion then stands on the host model and its negative controls, and the result is recorded on #164. This PR extends 1b's instruments in the steps that change what they measure:

- `unblock`'s per-caller counter is split by `WakeAction`: `Enqueue`, `Skip`, `LeaveToken`, `TokenPending` and `Dead`. "Skipping a Running or Runnable target" then means `Skip` only. Deliberate self-test wakes count under their own caller, `self_test`.
- The blocked-with-no-waker scan adds `BlockedTimer` and `BlockedProcessWait` (a `PROCESS_WAITERS` entry counts as a waker), drops `SELECT_WAITERS` when it is deleted, and excludes Dead threads.
- **Wait-word count `bw`:** threads flagged in two consecutive heartbeat scans whose word is not Idle while their state is neither Running nor Runnable. Dead threads are left out until crash-fix step 8's safe point; from then on they are counted, and `bw` reads 0.
- After conversion a `Skip` finds the waiter outside an armed window, between passes or after its wait (for example after a timeout that raced the reply); its next pass re-checks after arming, so a `Skip` is stale, not lost. The blocked-with-no-waker scan is the lost-wakeup signal, so crash-fix `:497` becomes a reported count, not a gate, and step 6b passes on `:496` **(ADR choice)**.

### 11. Boot-time broker

- `channel_create_unchecked` moves from `ipc/tests/mod.rs:302-317` into `ipc/mod.rs`, shares the allocator, and sets `creation_cap` and `creator` to `None`. It stays for init code before `enter_scheduler` that names synthetic creator tids (`service/mod.rs:209`, `storage/space.rs:144`, `gpu/service.rs:592`, `compositor/service.rs:592`, `ipc/tests/mod.rs:142`, `:217`) and for the select_cap test's inaccessible channel. Their ambient `ChannelAccess` grants stay **(ADR choice)**, as an interim exception to owner decision 1 (#163: never by ambient `grant_to_process`): the kernel acts as the Service Manager until `CapabilityTransfer` exists. Kernel channels never cascade.
- Initial capability sets stay as `grant_to_process` calls at process setup. The bench's `bench.rs:365-375` runs in a pid-8 thread that holds `ChannelCreate` and switches to `channel_create`.
- **Rule** (doc comment on `grant_to_process`): at runtime, an object-bound token comes only from `create_with_access` or `shared_memory_share`. Ambient grants of them are allowed only in init code before `enter_scheduler` and in self-tests, the same interim exception **(ADR choice)**.

### 12. Lock order

- **`PROCESS_TABLE → CHANNEL_TABLE`** (create, channel cascade) and **`PROCESS_TABLE → SHARED_REGION_TABLE`** (create, map, share, Kit destroy, region cascade, exit A). Under one `PROCESS_TABLE` hold the two tables are taken one after the other, never nested. `CLAUDE.md:98` already allows both edges; these are the first paths that use them.
- **`PROCESS_TABLE → PROCESS_WAITERS`** (install, `wait_step`, exit E). `PROCESS_WAITERS` is never held across `unblock`.
- **`SHARED_REGION_TABLE` alone:** the borrow API, the last unmap and exit D. The borrow API is never called from IRQ context or under a lock ranked below `SHARED_REGION_TABLE`, and `region_release` never under `PROCESS_TABLE`. Region frames are freed after both table locks are released.
- **Four IRQ-masked helpers** in `sched/scheduler.rs` (`kill_process_threads`, `thread_probe`, `current_thread_and_pid`, `current_tid`) take `CURRENT_THREAD`, `THREAD_TABLE` and `RUN_QUEUES` one after the other, with `unblock`'s DAIF save/restore. They add eight inline DAIF sites: crash-fix's inventory (`:242`) goes from 41 to 49 in the same 10 files, and F6's list of 10 unconditional exits (`:186`) is unchanged.
- **Removed:** the statics `SELECT_WAITERS` and `NOTIFY_RESULTS`; the edges `CHANNEL_TABLE → SELECT_WAITERS` and `NOTIFY_DEADLINES → {THREAD_TABLE, NOTIFICATION_TABLE, SELECT_WAITERS, RUN_QUEUES}`. Crash-fix's IRQ-shared set (`:213`) shrinks from 9 statics to 7, and the nesting at `:219` is gone.
- The wake token adds no lock, edge, `DAIFClr` site or `THREAD_TABLE` hold with IRQs on. One such hold goes (`select.rs:296`), and the converted waits' IRQs-on `CURRENT_THREAD` read (`current_thread_id()`) becomes the masked `current_tid()` (`current_thread_and_pid()` for SVC 25).

-----

## Consequences

### ABI

- **Ids** are opaque: raw = `slot | generation << 8`, so no valid id is below 256. Syscall decoding does not change.
- **`ChannelCreate` (6), `SharedMemoryCreate` (20):** the minted handle comes back in x1.
- **`CapabilityAttenuate` (15):** Create → Access returns EPERM; an expired parent returns EPERM; the child's expiry is clamped to the parent's.
- **`CapabilityRevoke` (16):** revoking a Create token destroys the channels or regions created under it or under a same-table descendant. A caller that is not Alive gets EPERM.
- **`MemoryUnmap` (19) on a region's window VA, `SharedMemoryMap` (21):** EPIPE for a destroyed, draining or stale region. Map needs a live token and an Alive caller.
- **`SharedMemoryShare` (22):** needs a live token for the full id, delegatable unless the target is the caller. A repeat share is a no-op. ENOSPC at the foreign reserve.
- **`ProcessExit` (24)** does not return on success and destroys everything the process created. **`ProcessWait` (25)** is parent-only and reaps. x0 is the only input; on return x0 is 0 or an errno and x1 the exit code. A second waiter gets EEXIST, and a wait after the reap gets EPERM.
- **Blocking IPC** returns only on its condition, an error or its own deadline. `ipc_select` can return EAGAIN, ENOMEM, EPIPE or EINVAL and reports the lowest-index ready entry. `notification_wait` on a destroyed object returns EINVAL (today ETIMEDOUT).
- **Errnos:** none added. **Kit:** the create return types change; `grant` refuses object-bound capabilities; `shmem_destroy` needs the creator's live token and destroys the region even while others map it.

### Behaviour

- Stale ids fail with EPIPE and cannot touch the object now in the slot.
- **Lifetime create limit.** Each checked create mints a token, and neither revoked nor stale tokens free their slot, so a process makes at most about 256 checked creates in its lifetime. Pid 1 ends the boot at about 30 slots (at most about 44 if every stale-test retry is used), counted from the plan, far inside the 192 that the foreign reserve leaves.
- A creator's revoke or exit destroys its regions even while others map them, and a region that was never mapped is now freed. A draining slot stays out of use until its last release; a leaked borrow leaks one of 64 slots for the boot.
- Dead threads never run again, and no token is given to or consumed by one.
- The Timeout self-test really blocks for 100 ticks. Its thread is pinned to CPU 2, so `bench-yield` on CPU 0 (crash-fix N10) cannot starve it.
- **Cost.** One CAS in `unblock`, one swap per block, a load and a swap in `try_direct_switch`, one IRQ-masked `CURRENT_THREAD` read per wait in place of today's unmasked one, and per IPC round trip about two uncontended swaps and one `REPLY_SLOTS` check. Not measured; the soak pair records the Gate 1 average.
- Generations stop stale ids but are not secret; `ipc_reply` still has no capability check (`channel.rs:347`).

### Self-test and log changes

- `denied ChannelAccess` falls from 6 to 3 lines per boot (select_cap only). `Timeout test: ETIMEDOUT as expected` replaces `unexpected result -6` and prints after every other ipc-timeout test line. `Destroy test: EPIPE as expected` replaces `unexpected result Err(-6)`.
- New lines: `Stale-id test`, `Stale-shm test`, `Shm-cascade test`, `Create-ABI test`, `Share test`, `Dead test`, `Wake-token test`, seven `Lifecycle:` lines, and `Notify-loop`, `Select-loop`, `Sleep-loop`, `Recv-loop` and `Call-loop test: as expected`. Region logs: `shm_cascade: pid=N destroyed K regions`, `shm: region S.G draining (B borrows)`, `shm: region S.G drained` and `shm_destroy: region S.G`. Ids print as `slot.generation`. Every line stays under 48 bytes (#191). The plan lists the exact lines and counts.
- No soak classifier matches self-test lines, so the soak adds an expected-line tally.

### Docs to update (step 15)

| Doc | Change |
| --- | --- |
| `CLAUDE.md:98-107`, Key Technical Facts | Lock order (new nestings, `PROCESS_WAITERS`; `SELECT_WAITERS` removed from `:99`); capability enforcement (minting, share, cascades); new facts: id layout, Dead is terminal, process slots never reused, every waiting site arms and `block_current` asserts it, region memory only under a borrow |
| `docs/kernel/ipc.md` | Create ABIs; `ChannelId` encoding; `creation_cap` and `creator`; `ProcessExit`/`ProcessWait` (timeout not implemented); §4.5 and `:848`: a creator's death destroys its regions; `ipc_select` errors and lowest-index result; `notification_wait` EINVAL; `ipc_call` reply wins and 0 = no timeout; `SqEntry.channel_id` (`:1848`) must be `u32` |
| `docs/kernel/deadlock-prevention.md` | §3.3: the new nestings; `PROCESS_WAITERS` moves from the leaf and utility table (`:130`) into the primary hierarchy directly below `PROCESS_TABLE`, and stays a leaf lock: nothing is taken under it and it is never held across `unblock`; `SELECT_WAITERS` (`:87`, `:115`) and `NOTIFY_RESULTS` (`:132`) removed. §3.5: the new exit pattern |
| `docs/kernel/scheduler.md` | Dead is terminal; `exit_current`; the wake token; `sleep_ticks` uses `BlockedTimer` |
| `docs/kernel/memory/virtual.md`, `docs/kits/kernel/memory.md` | Create returns a handle; share rules and the reserve; destruction by revoke, exit or destroy; no `MemoryRevoked` event |
| `docs/security/model/capabilities.md`, `docs/kits/kernel/capability.md`, `docs/kits/kernel/ipc.md`, `shared/src/kits/ipc.rs:183-184` | No Create → Access; expiry clamp; minted children; cascades; Kit signatures, `grant` and `shmem_destroy` rules |
| `docs/project/developer-guide.md` | `:597` example; ipc and sched file trees; test totals; `:1792` expected warnings; `:2060` replaced by §8's rules; `:2061` keeps "Use `try_lock()` in IRQ context, never blocking lock", which crash-fix step 2 reverses (`:440`), and gains "call `unblock` only after releasing the lock" |
| `docs/project/ai-agent-context.md`, `docs/phases/03-ipc-and-capability-system.md:189`, `docs/phases/05-kit-foundation.md:248`, `docs/platform/posix.md:655-656`, `:705-720` | Scheduler API; create signatures; `ProcessWait` no longer serves `pthread_join` or `waitpid(-1)` |
| `kernel/src/ipc/channel.rs:30` | `timeout_ticks` doc comment (step 11 fixes the lock-order comment at `compositor/service.rs:547`) |

The crash-fix ADR is amended in the docs PR that carries this ADR, not in step 15.

### Not covered

- Capability-table slot reclamation, which needs handle generations. Cross-table revocation, a precondition for `CapabilityTransfer`, which otherwise must refuse Create-type capabilities.
- `NotificationId` reuse, `GpuBufferAccess` ids and `ThreadId` reuse (#187). `ProcessId` generations: reuse is refused until `ProcessCreate`.
- Reply-slot reuse, and a stale ECANCELED delivered after the caller's next call has been claimed. Both need a per-call sequence in `pending_caller` and `ReplySlot`. A replier killed between taking `pending_caller` and writing `done` leaves its caller waiting with no deadline (pre-existing).
- Killed borrowers' frames drain for the rest of the boot. Releasing them needs a per-process on-CPU count, which no crash-fix step delivers yet (it can build on step 6a's per-thread `on_cpu`), and per-mapping borrow counts, because borrows are counted per region. Reaping while threads are on a CPU, and address-space teardown.
- EL0 force-unmap (§5). `MemoryUnmap`'s private path can free region frames, which the last unmap, the cascade or the last release then frees again; #188 item 1's per-process allocation records must refuse any page inside a live or draining region. Mappers get no `MemoryRevoked` event.
- Kernel channels never cascade (#191). No `ProcessWait` timeout. POSIX `pthread_join` and `waitpid(-1)`.
- The exit safe point, until crash-fix step 8 (§7).

### Risks

- **PR #149.** Its compositor must hold a `RegionBorrow`, not a mapping, or a client's exit frees frames that compose still reads.
- **Conventions.** Arm-before-publish is mitigated by the rules, the host model's negative controls and, from step 14, `block_current`'s arm assertion. A new Blocked writer that bypasses `block_action` can lose a token. A missed `region_release` leaks a slot. A direct `CapabilityTable::revoke` of a Create token skips the cascades; step 7's grep guards it.
- **Stray tokens.** A missed `cancel_block` leaves the word Armed; a later stale wake leaves a token, and an unrelated wait returns early and loops once. `prepare_block`'s assertion catches it at the next arm in the dev profile only, and six sites now have arm-then-exit paths. Thread-slot reuse before `ThreadId` has a generation (#187) would let a stale tid leave a token on a new thread. `allocate_thread`'s reset and the loops bound both to one spurious pass.
- **Stack.** SVC 16 carries the 1 KiB region free list, `revoke()`'s 4 KiB array and the channel cascade's 1 KiB on a 32 KiB stack with no guard page (computed, not measured).
- **Brittle counts.** Acceptance log counts depend on each test's shape; a step that changes a test updates them in the same commit.

-----

## Implementation plan

One PR on `claude/cap-lifetime`, one commit per step, named `Cap lifetime step N: <description>`. The branch's first commit adds the plan (rule 04 step 4). Every commit passes `just check` (zero warnings) and `just test`. Boot checks grep one `just run` log for self-test lines, which print before the bench; a boot that wedges first, or logs `slot moved`, is re-run and noted. Every boot check also passes the plan's no-new-kwarn and Timeout-last checks. The plan gives each step's full change list and checks. DAIF counts are `rg -c 'msr DAIFClr' kernel/src/sched/scheduler.rs kernel/src/ipc/direct.rs`, read per file (rg does not print in argument order); at b640789 they are `scheduler.rs:7` and `direct.rs:8`.

| Step | Change | Acceptance |
| --- | --- | --- |
| 1 | Id layout and API; `SharedMemoryAccess(SharedMemoryId)` | `just test` id tests pass; `just run` prints the Bad-id, Bad-pid and Select-cap lines, and `grep -Ec 'Channel [0-9]+\.1 created'` is at least 1 |
| 2 | Per-slot generations, identity checks, `region_mut`, select propagates lookup errors; stale-id tests | `just run` prints `Stale-id test: EPIPE as expected` and `Stale-shm test: EPIPE as expected` |
| 3 | `mint_child`, `free_slots`, `is_revoked`, `revoke_all`, `clear`; the three arms removed; expiry clamp; Kit `grant` refusal | `just test`: the three denial tests, the clamp tests and the minting tests pass |
| 4 | ipc-timeout thread pinned to CPU 2; Timeout test moved last | `just run`: step-3 lines unchanged and the Timeout-last check passes |
| 5 | `create_with_access`; minting for channels and regions; x1 ABI; channel cascade; Create-ABI test | `just run` prints `Create-ABI test: x0/x1 as expected`, `Destroy test: EPIPE as expected` and `Timeout test: ETIMEDOUT as expected`; `grep -c 'denied ChannelAccess'` is 3 |
| 6 | Region cascade, `region_detach`, `RegionBorrow`, deferred free; Shm-cascade test | `rg -n -e region_dmap_addr -e region_size kernel/src` prints nothing; `just run` prints `Shm-cascade test: EPIPE as expected`, and `grep -c 'shm_cascade: pid=1 destroyed 1'` is 3 |
| 7 | Share as delegation; map in one hold; Kit destroy; compositor attach check; Share test | `just run` prints `Share test: delegation as expected`; `rg -n 'cap_table\.revoke\(' kernel/src` lists only `cap/mod.rs` |
| 8 | `may_become`, Dead guards, `exit_current`, `kill_process_threads`, `thread_probe`; Dead test | `just run` prints `Dead test: terminal as expected`; DAIF counts `scheduler.rs:9` and `direct.rs:8` |
| 9 | Wake token: `shared/` decisions and model, `sched/wake.rs`, `unblock`/`block_current`/`try_direct_switch`; 1b counter split; Wake-token test | `just test`: the model passes and its negative controls reach their lost-wakeup states; `just run` prints `Wake-token test: early wake as expected` |
| 10 | Process lifecycle: state, install, `process_exit` A–E, `wait_step`, syscalls; lifecycle tests | `just run` prints all seven `Lifecycle:` lines and no `PANIC`; DAIF counts `scheduler.rs:10` and `direct.rs:8` |
| 11 | `current_tid`; `notification_wait` and `ipc_select` arm; waiter records with `fired`; `check_notification_timeouts`; the four deletions; `SELECT_WAITERS` dropped from the lock-order comment at `compositor/service.rs:547` | `rg -n -e SELECT_WAITERS -e NOTIFY_RESULTS -e try_wake_select -e set_select_ready kernel/src` prints nothing; `just run` prints `Notify-loop test: as expected` and `Select-loop test: as expected`; DAIF counts `scheduler.rs:11` and `direct.rs:8` |
| 12 | `sleep_ticks` arms and blocks as `BlockedTimer` | `just run` prints `Sleep-loop test: as expected`; gpu mode prints `display handoff complete` |
| 13 | `ipc_recv` arms and loops | `just run` prints `Recv-loop test: as expected`; `[bench] IPC round-trip` reports 10000 iterations in a CLEAN boot |
| 14 | `ipc_call` arms and loops, `ReplySlot.done` and `cancelled`; `block_current` arm assertion | `just run` prints `Call-loop test: as expected`, and `Timeout test: ETIMEDOUT as expected` still prints last |
| 15 | Docs (table above) | `/audit-loop` reaches a clean round; `just docs-check --all` marks no new finding except `knowledge-hygiene plans-not-empty` for the plan |
| 16 | Distil and delete the plan: lessons, review notes and evidence limits into this ADR, which stays near 400 lines (second-set answer 4), plan references replaced by a permalink, `git rm` the plan | `ls docs/knowledge/plans` lists only `_template.md`; `just docs-check` exits 0 |

**Order.** Steps 1–2 precede step 5: landing #163 on bare ids would let a surviving `ChannelAccess` authorise the next channel in that slot. Step 3 precedes step 5 (`mint_child`, `free_slots`). Step 4 lands the scheduling change alone, so step 5's boot has one new variable. Step 6 needs step 5's `creation_cap` and precedes step 7. Steps 8, 9 and 10 go in that order: the token's Dead rows rely on step 8, and `process_wait` arms in step 10. Steps 11–14 convert the remaining sites from least to most used (boot self-tests only, gpu input, services and the bench server, the bench client), so the bench's IPC path changes last; steps 12–14's tests use step 11's notification timeout as a backstop. Step 16 comes last, before the PR is marked ready (rule 04 step 11).

**Soak.** Prerequisites: crash-fix steps 1a and 1b have merged. Run one interleaved pair under the crash-fix soak protocol, post-1b `main` against the branch tip, with 20 text-mode and 10 gpu-mode boots per arm, the load rule, and no other soak on the host.

- **Gates:** the regression guard (one-sided Fisher on CLEAN, failing at p < 0.05), and the expected-line tally: every CLEAN boot in the branch arm contains every expected line, and boots that log `slot moved` are counted separately.
- **Reported per arm:** 1b's per-caller `Skip` counts (and in the branch arm `LeaveToken` and `TokenPending`), the no-waker scan, `bw`, DEGRADED, WEDGE-ALIVE, WEDGE-STUCK and the Gate 1 IPC average. N2 and N3 are expected as skips from `ipc_reply`, `ipc_send`, the call fallback and `check_timeouts` in the before arm, and as `LeaveToken` in the branch arm. Class results are not gated here; the class gate is 6b's.
- CI's report-only soak does not gate. CI arms are unpaired, so the 3 baseline CI runs on `main` are re-run after this PR merges, before any later step's CI numbers are compared.

**Constraints on the crash-fix steps.** The crash-fix amendment points here for these.

- **1b:** merges before this PR, with its content and acceptance unchanged; this PR extends its counters (§10). The Timeout test's IRQ wake queues the pinned thread on CPU 2, its last CPU, so 1b's cross-CPU counter should record it 0 times.
- **2, 6a, 6b, 8:** their rewrites of `thread_yield`, `block_current`, `unblock`, `schedule` and `direct.rs` keep the host-tested `may_become`, `wake_action` and `block_action` intact.
- **2:** `with_this_cpu` replaces the IRQ-masked `CURRENT_THREAD` reads added here, `current_tid()` included. The eight new DAIF sites are in its scope (49 in total). This PR's step 8 deletes three of F6's per-CPU read sites (`:187`: `service/mod.rs:315-316`, `gpu/service.rs:178-179`, `compositor/service.rs:187-188`), leaving one read in `exit_current`. If this PR lands first, step 2 classifies 7 IRQ-shared statics, not 9; whichever lands second adjusts the classes and the §3.3 rebuild. No new lock static is added. EL0 SVCs enter with IRQs masked, and nothing on the paths of SVCs 16 and 24 unmasks them before they free region frames; F6 removes unmasks and adds none, so step 2 must not assume frame frees run with IRQs on.
- **6a:** `finish_switch` never queues a Dead previous thread. The `on_cpu` wait belongs to `unblock`'s `Enqueue` path only: `LeaveToken` and `TokenPending` neither restore nor queue, and waiting would deadlock on a self-wake. A `ConsumeToken` block does not switch. A per-process on-CPU count is the precondition for releasing killed borrowers and for reaping once `UserAddressSpace` has a `Drop`; it is not in 6a's scope, and a later step can build it on 6a's per-thread `on_cpu`.
- **6b:** keeps N1 (Dead counts as blocking in the requeue test; a `ConsumeToken` block never reaches `schedule()`), N7, and N10 if implicated. It needs 6a and this PR. Its acceptance (`:494-500`) is unchanged except `:497`, which an in-place edit makes a reported count; 6b passes on `:496` (§10).
- **8:** adds the exit safe point (§7), whose `prepare_block` check never leaves the CPU, and, once it exists, counts Dead threads in `bw`. Its added acceptance **(ADR choice)** is §7's boot self-test: a process whose thread is running on another CPU exits, and no waiter record of that thread remains. `WAIT_STATE` may move into `ThreadInfo`, and `prepare_block` and `cancel_block` then read the current tid there. The sleeping-while-atomic check at `block_current` entry runs before `block_action`, so a pending token cannot hide it. Region borrowers dereference with no lock held, so the bench loop stays preemptible.

**PR #149** (#165's pre-merge list): pid 11 is installed through `process_install`; the new create return types; pid 11's `SharedMemoryCreate` is granted delegatable, or its share to pid 10 returns EPERM; `test_app.rs`'s runtime ambient `ChannelAccess` grants break §11's rule, so the app's own channel comes from `channel_create` and its access to the compositor channel from the compositor's init-time broker grant; the test app's AttachBuffer passes §4's access check through the token minted with its buffer; the `client_channel == 0` and `shmem_id: 0` sentinels stay safe, because no valid id is below 256; the compositor maps a client's buffer, holds a `RegionBorrow` while it is attached, checks `is_live()` before each compose outside the compositor locks, and releases the borrow on detach, surface destroy, client exit or loss of liveness.

-----

## Alternatives considered

- **#185 B, revoke on free:** needs a draining tombstone and never frees token slots. **#185 C, bind to lineage:** touches every grant path. **#163 A, pure separate:** every channel would need a privileged minting broker. All rejected (#163, #185).
- **Revoking `SharedMemoryCreate` reaches only the tokens** (the first draft's default): replaced by the cascade (first-set answer 2).
- **Leaving #189's window 2 to crash-fix 6b** (the first draft's default): replaced by first-set answer 3.
- **Freeing a region's frames at once despite a kernel borrow**, or reading region memory only under `SHARED_REGION_TABLE`: rejected for deferred free (second-set answer 1).
- **Converting only `process_wait` and leaving five sites to crash-fix 6b** (the 1,084-line draft's default): the owner chose to convert every site (second-set answer 2).
- **A detect-only "step 0" in this PR** (the N2/N3 part of 1b, soaked against `main` before any conversion): not chosen. It duplicates 1b, adds a second soak pair, and would make 1b adopt counters designed here. The plan keeps its outline in case 1b stalls and the owner prefers it.
- **Keeping `SELECT_WAITERS` and deciding wakes by registration:** rejected; the sources are authoritative. **Clearing `WAKEUP_ERRORS` at wait entry** (#186): not adopted; results come from state, and the thread-side `WAKEUP_ERRORS` holds (crash-fix H3) grow only by the `Select-loop` self-test's exits (§9).
- **The wait word as a `Thread` field under `THREAD_TABLE`:** an IRQ-masked `THREAD_TABLE` hold at every arm and cancel, taken inside the publishing locks, which adds `THREAD_TABLE` nestings under `CHANNEL_TABLE`, `NOTIFICATION_TABLE` and `PROCESS_WAITERS`.
- **Reading `me` through `current_thread_and_pid()`:** a masked `THREAD_TABLE` hold per wait for a pid the sites do not use; `current_tid()` reads only `CURRENT_THREAD`.
- **The exit safe point as its own issue** (the 1,084-line draft's default): the owner added it to crash-fix step 8 (second-set answer 3).
- **Exit step E taking the waiter entry:** a second thread of the parent could then reap. **Resolving the caller's pid inside `process_wait`:** a new `THREAD_TABLE` hold with IRQs on.

-----

## Companion plan and crash-fix amendment

- **Plan:** `docs/knowledge/plans/2026-09-24-jl-capability-lifetime-plan.md` holds the evidence tables (file:line at b640789), the per-site change lists, the per-step checks, the test plan and soak procedure, the crash-fix interplay detail, the review log and the evidence limits. It is not part of the docs PR that carries this ADR, because `just docs-check` reports any file under `docs/knowledge/plans/` as drift (`plans-not-empty`). It is the first commit of `claude/cap-lifetime` (c97caa0, on `main` b07d7e4) and lives only on that branch until the implementation PR; the branch is rebased once this ADR merges and again after crash-fix steps 1a and 1b merge. Step 16 distils it into `docs/knowledge/lessons/` and into review notes and evidence limits on this ADR, replaces each mention of its path here with a permalink to its last version in the implementation PR's commits, and deletes it (rules 04 and 08).
- **Crash-fix amendment:** the note "Amendment, 2026-09-24 (#185, #189)" at the top of the crash-fix ADR's "Review notes", with one-line in-place edits that keep its line numbers, lands in the docs PR that carries this ADR. It records first-set answer 3's move of the wake token into this PR, second-set answer 2's move of the other five site conversions and its "resolved first" constraint, step 6b's remaining scope, and second-set answer 3's exit safe point in step 8. It marks this ADR's own choices as such. The crash-fix ADR stays `final`.

-----

## Review notes

**Audit round 1, 2026-09-24.**

- **fable, the interim exit rule names only "another process".** Adopted. §6's victims include the caller's sibling threads, and the one known exception, the echo client's `process_exit(ProcessId(7), 0)`, is a self-exit whose pid-7 sibling, the echo server, may be running on another CPU (`service/mod.rs:234`, `:255`, `:371`). SVC 24 always exits the caller's own process, so a multi-threaded caller would hit the same gap. §7 and the crash-fix amendment now say `process_exit` is called only when no victim other than the caller can be on a CPU.
- **fable, "`ipc_select` has no boot user".** Adopted, with one correction. The select_cap self-test calls `ipc_select` six times per boot, but each call returns at validation or at the scan before the arm, so none reaches the exit take of `WAKEUP_ERRORS`. Step 11's new `Select-loop` self-test does reach it. §9, the step order and "Alternatives considered" now say the thread-side H3 holds grow only by that test's exits.

**Audit round 2, 2026-09-24.**

- **fable, "§9 has no pre-arm scan, so the three-per-boot bound is unsupported".** Rejected: §9's table row and bullet place the arm before the check-and-register pass; they do not rule out the unarmed fast scan that comes first, which §9 names ("a scan that runs before the arm") and the plan's step 11 specifies. The select_cap `owned` call (`ipc/tests/select_cap.rs:131`) returns `Ok((1, 0))` from that scan, three calls return EPERM at `check_channel_entries` and two return EINVAL at the range check, so none reaches the exit take and the bound stands.

**Audit round 3, 2026-09-24.**

- **fable, "`(#186 item 2)` should be item 3".** Rejected: #186 has two numbered lists, "Findings" (1–5) and "Suggested fix" (1–6), and this ADR cites the Findings list. Finding 2 is the silently skipped registration (receiver slot taken, waiter list full), which EAGAIN and ENOMEM report; finding 3 is the empty or Dead channel, which §1's lookup errors report. Suggested-fix item 2 (clearing `WAKEUP_ERRORS` before publishing) is a different list's item, and "Alternatives considered" rejects it without contradiction.
