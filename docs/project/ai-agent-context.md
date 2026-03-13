# AI Agent Context Guide

**Purpose**: Context-loading checklist for AI coding agents before writing AIOS kernel code.

**When to load**: At agent spawn, before any implementation work. This guide prescribes the minimum reading and behavioral rules for producing correct kernel code.

**Not for humans**: For the human-readable developer guide, see [developer-guide.md](./developer-guide.md).

**Related**:

- [CLAUDE.md](../../CLAUDE.md) -- Code conventions, quality gates, technical facts
- [developer-guide.md](./developer-guide.md) -- Human-readable kernel developer guide
- [deadlock-prevention.md](../kernel/deadlock-prevention.md) -- Lock ordering rules

-----

## 1. Required Reading Order

Before writing code for any phase step, read these documents in order:

### Mandatory (every task)

1. **Phase doc** (`docs/phases/NN-phase-name.md`) -- Read the specific step you are implementing. Note the acceptance criteria -- this is your done condition.
2. **CLAUDE.md** -- Read these sections:
   - Code Conventions (Rust, Assembly, Architecture-Specific)
   - Unsafe Documentation Standard (three-line SAFETY format)
   - Key Technical Facts (addresses, offsets, constants)
   - File Placement (where to put new files)
3. **Developer guide §2** ([developer-guide.md §2](./developer-guide.md#2-aios-kernel-patterns)) -- The four unsafe patterns (MMIO, page tables, SPSC rings, system registers) and three error handling patterns.
4. **Developer guide §4** ([developer-guide.md §4](./developer-guide.md#4-common-pitfalls)) -- All seven pitfalls. These represent real bugs discovered during Phases 1-3.
5. **Deadlock prevention** ([deadlock-prevention.md](../kernel/deadlock-prevention.md)) -- Lock ordering rules. Violating lock order causes deadlocks that are extremely difficult to debug.

### Task-specific

6. **Architecture docs** -- Read the documents listed in the phase doc's "Architecture References" table. These are the source of truth for register offsets, struct fields, and memory addresses.
7. **Existing code** -- Before creating new files, read the existing code in the same directory. Match its patterns, naming, and style exactly.

### What to skip

- Developer guide §1 (Rust Competency Model) -- written for humans, not agents
- Developer guide §5-6 (Build/Debug workflow) -- you execute commands, not read about them
- Developer guide §10 (Glossary) -- reference only if you encounter unfamiliar terms

### Task decomposition guidance

Research on LLM code generation (deepSURF, SACTOR) shows that agents produce safer code when complex tasks are broken into focused sub-tasks with type definitions provided upfront. When implementing a multi-step phase step:

1. Read all relevant type definitions and struct layouts first
2. Implement one function at a time, verifying each compiles before moving on
3. Write the SAFETY comment *before* writing the unsafe block -- this forces you to think about invariants first
4. Run `cargo build` after each function, not just at the end

-----

## 2. Pattern Quick-Reference

When implementing kernel code, use these established patterns:

### Core patterns (Phase 0-2)

| Task | Pattern | Reference File |
|---|---|---|
| Read/write hardware register | `mmio_read32()`/`mmio_write32()` volatile helpers | `arch/aarch64/uart.rs:102-110` |
| Read ARM system register | `asm!("mrs {}, REG", out(reg) val)` with SAFETY | `arch/aarch64/timer.rs:19-25` |
| Write ARM system register | `asm!("msr REG, {}",  in(reg) val)` + ISB if needed | `arch/aarch64/timer.rs:51-58` |
| New static shared across cores | `AtomicT` with appropriate ordering | `smp.rs:34` (PRINT_TURN) |
| Write-once boot-time static | `UnsafeCell` + `unsafe impl Sync` | `arch/aarch64/mmu.rs:32-39` |
| Per-core data structure | Array indexed by `current_core_id()` | `observability/mod.rs:101` (LOG_RINGS) |
| New module in kernel | `pub mod name;` in parent + file with `//!` doc comment | `arch/aarch64/mod.rs` |
| Shared type (kernel + stub) | Define in `shared/src/`, import in kernel with `pub use shared::` | `observability/mod.rs:16` |
| Error from syscall handler | Return `Err(IpcError::Variant as i64)` | `ipc/mod.rs` (channel_create) |
| Unrecoverable error | `kerror!(Subsys, "msg"); halt()` OR `panic!("msg")` | `main.rs:53-64` |
| Feature-gated code | `#[cfg(feature = "feature-name")] { ... }` | `observability/trace.rs:6` |
| Structured logging | `kinfo!(Subsys, "fmt {}", arg)` | `main.rs:37` |
| TLB invalidation (boot) | `tlbi vmalle1` + `dsb nsh` (non-broadcast) | `arch/aarch64/mmu.rs` |
| TLB invalidation (runtime) | `tlbi vmalle1is` + `dsb ish` (broadcast) | `mm/kmap.rs` |
| Compile-time size check | `const _: () = assert!(size_of::<T>() == N);` | `arch/aarch64/trap.rs:35` |

### Phase 3-4 patterns

| Task | Pattern | Reference File |
|---|---|---|
| Lock ordering enforcement | Acquire in order per CLAUDE.md: PROCESS_TABLE > CHANNEL_TABLE > ... | `docs/kernel/deadlock-prevention.md` |
| IRQ masking before spinlock | `asm!("msr DAIFSet, #0x2")` → lock → work → unlock → unmask | `sched/scheduler.rs:67-76` |
| Direct IPC (kernel threads) | Call `ipc_call()` directly -- NOT via SVC (SVC is for future EL0) | `ipc/channel.rs:1-5` (module doc) |
| Capability check before op | `check_channel_create(pid)` / `check_channel_access(pid, ch)` | `cap/mod.rs:58-108` |
| Service registration | `service_register(name, pid, channel)` → name uniqueness check | `service/mod.rs:48-83` |
| Lock-free audit logging | `audit_log(pid, event)` -- AtomicUsize ring head, Mutex ring body | `service/mod.rs:153-171` |
| VirtIO-blk I/O | `read_sector(sector, buf)` / `write_sector(sector, buf)` polled | `drivers/virtio_blk.rs:112-132` |
| Crash-safe block write | WAL append → data write → WAL commit | `storage/block_engine.rs:1-6` (module doc) |
| Direct-map phys→virt | Add `DIRECT_MAP_BASE + phys` after TTBR1 enabled | `mm/slab.rs` (`convert_to_direct_map`) |
| Cascade revocation | Drop PROCESS_TABLE lock *before* walking CHANNEL_TABLE | `cap/mod.rs:197-236` |

-----

## 3. Anti-Patterns

Things that agents commonly get wrong. Violating any of these will fail code review.

### Never invent hardware constants

```text
WRONG: Guessing a register offset
const GICD_CTLR: usize = 0x000;  // "I think this is right"

RIGHT: Read from architecture doc
// GICv3 GICD_CTLR offset (hal.md §4.1)
const GICD_CTLR: usize = 0x000;
```

If you don't know a register offset, address, or constant -- read the architecture doc or `CLAUDE.md` Key Technical Facts. Never guess.

### Never use spin::Mutex on Non-Cacheable memory

Post-Phase 2 M8, the TTBR0 identity map uses WB cacheable (Attr3), so `spin::Mutex` works on identity-mapped RAM. However, if you explicitly map memory as Non-Cacheable (device MMIO, framebuffer), spinlocks will HANG there. For NC regions, use `AtomicBool` with `load(Acquire)`/`store(Release)` only. See developer-guide.md §4.1.

### Never leave TODO comments

AIOS convention: no TODO comments in code. If a feature is incomplete, either:

- Complete it fully in this step, OR
- Mark the function/module `#[allow(dead_code)]` if it will be wired in a later phase step

### Never create files in wrong directories

Follow CLAUDE.md File Placement rules exactly:

- aarch64-specific code → `kernel/src/arch/aarch64/`
- Memory management → `kernel/src/mm/`
- Platform abstraction → `kernel/src/platform/`
- Shared types → `shared/src/`
- Phase docs → `docs/phases/` (flat, no subdirs)

### Never skip ISB after MSR writes

Always add `isb` after writing to: `VBAR_EL1`, `SCTLR_EL1`, `TCR_EL1`, `TTBR0_EL1`, `TTBR1_EL1`, `CPACR_EL1`. See developer-guide.md §4.4.

### Never omit the SAFETY comment

Every `unsafe` block requires the three-line format:

```rust
// SAFETY: <what invariant makes this safe>
// <who maintains the invariant>
// <what happens if violated>
unsafe { ... }
```

From Fuchsia's unsafe encapsulation convention: when creating a safe wrapper around unsafe code, also document:

- **Preconditions**: what must be true for callers (e.g., "addr must be 4KiB-aligned")
- **Failure mode**: what happens if preconditions are violated (abort, data corruption, etc.)
- **Aliasing rules**: for types with `*mut` / `UnsafeCell` fields, explain the aliasing/mutation invariant

### Never map a page as RWX

W^X policy: pages are writable OR executable, never both. See developer-guide.md §4.5.

### Never acquire locks out of order

The full lock ordering is defined in CLAUDE.md Key Technical Facts. The canonical order is:

```text
PROCESS_TABLE > SHARED_REGION_TABLE > NOTIFICATION_TABLE > CHANNEL_TABLE
  > SELECT_WAITERS > BLOCK_ENGINE > VIRTIO_BLK
```

Violating this causes deadlocks under contention. When you need two locks, always acquire the earlier one first. See [deadlock-prevention.md](../kernel/deadlock-prevention.md).

### Never hold spinlocks with IRQs enabled in scheduler code

Timer IRQ fires every 1ms. If a timer IRQ hits while the current core holds `THREAD_TABLE` or `RUN_QUEUES`, `timer_tick()` will try to re-acquire and deadlock. Pattern:

```rust
// SAFETY: DAIFSet #0x2 sets the IRQ mask bit. Safe at EL1.
unsafe { core::arch::asm!("msr DAIFSet, #0x2") };  // mask IRQs
let table = THREAD_TABLE.lock();
// ... do work ...
drop(table);
// SAFETY: DAIFClr #0x2 clears the IRQ mask bit.
unsafe { core::arch::asm!("msr DAIFClr, #0x2") };  // unmask IRQs
```

Reference: `sched/scheduler.rs:67-76`.

### Never use SVC from EL1 kernel threads

Phase 3 kernel threads run at EL1. They invoke IPC via direct function calls (`ipc_call()`, `ipc_recv()`, etc.), NOT via `svc #0`. The SVC dispatch path (`trap.rs` lower_el_sync_handler) is wired for future EL0 user threads only. Calling SVC from EL1 would trap into the "same-EL" exception handler, which is a different vector entry.

### Never allocate in interrupt handlers

The slab and buddy allocators may block or deadlock under contention. Use pre-allocated buffers for IRQ-context paths. The timer tick handler and GIC IRQ handler must never call `alloc::` functions.

### Never confuse physical and virtual addresses

After Phase 2, the kernel runs with TTBR1 virtual addresses. DMA hardware requires physical addresses. `PhysAddr` and `VirtAddr` newtypes in `shared/src/lib.rs` prevent accidental mixing.

```text
WRONG: mmio_write32(0x0900_0000, val)  // physical addr, but MMU is on
RIGHT: mmio_write32(MMIO_BASE + 0x0900_0000, val)  // virtual via MMIO map

WRONG: submit_request(blk, virt_addr)  // DMA needs physical
RIGHT: submit_request(blk, phys_addr)  // hardware accesses physical memory
```

### Common LLM mistakes in kernel code

Research (SACTOR, SafeTrans, C2SaferRust, deepSURF) identifies these as the most frequent LLM failures in systems code:

1. **Pointer arithmetic errors** -- wrong stride, off-by-one in page table indexing, base+offset confusion
2. **Address space confusion** -- physical vs virtual vs direct-map addresses (see above)
3. **Lifetime mismatches** -- returning references into stack-allocated buffers, especially in unsafe code
4. **Incomplete implementations** -- token limit causes truncated output; verify every function has a closing brace
5. **Silent size regressions** -- changing a `repr(C)` struct without updating compile-time size assertions

-----

## 4. Verification Checklist

Before marking any step complete, verify ALL of these:

### Build verification

- [ ] `cargo build --target aarch64-unknown-none` -- zero warnings
- [ ] `just check` passes (fmt + clippy + build)
- [ ] No new clippy warnings introduced

### Code quality

- [ ] Every `unsafe` block has a `// SAFETY:` comment (three-line format)
- [ ] Every new file has a `//!` module-level doc comment
- [ ] No TODO comments in code
- [ ] Naming follows conventions: `snake_case` functions, `CamelCase` types, `SCREAMING_SNAKE` constants
- [ ] New constants reference their source: `// (hal.md §4.3)` or similar

### Architecture compliance

- [ ] No W^X violations (no page mapped as RW+X)
- [ ] All MMIO access uses volatile read/write
- [ ] ISB after all MSR writes to instruction-affecting registers
- [ ] Correct TLB invalidation strategy (local-only during boot, broadcast after all cores online)
- [ ] Addresses and offsets match CLAUDE.md Key Technical Facts
- [ ] Lock acquisition follows CLAUDE.md lock ordering
- [ ] Capability checks precede all privileged operations (IPC, shmem, channel create)
- [ ] No allocation in interrupt context (timer tick, GIC IRQ handler)
- [ ] PhysAddr/VirtAddr types used correctly (no raw usize for addresses crossing domains)

### Phase acceptance

- [ ] Step's acceptance criteria met (run the exact command from the phase doc)
- [ ] If QEMU output expected, it matches the documented strings

-----

## 5. Commit Protocol

After completing a step:

1. Stage only the files you created or modified
2. Commit with format: `Phase N MK: Step X -- <description>`
3. Push immediately -- do not batch steps
4. Report completion to team-lead with summary of files created/modified

-----

## 6. Subsystem Quick-Reference

Comprehensive per-subsystem guide for agents working on Phase 3+ code. For each subsystem: key APIs, all gotchas, lock ordering, and common mistakes.

### 6.1 Scheduler

**Files**: `kernel/src/sched/scheduler.rs`, `kernel/src/sched/init.rs`, `kernel/src/sched/mod.rs`

**Key APIs**:

```rust
// Enter the scheduler loop on a CPU (never returns).
// Called from kernel_main (boot CPU) and secondary_main.
pub fn enter_scheduler() -> !            // scheduler.rs:47

// Core scheduling function. Must be called with IRQs masked.
// Picks next thread from run queue and context-switches to it.
fn schedule()                            // scheduler.rs:153

// Block the current thread with the given new state.
// Masks IRQs internally, updates state, calls schedule().
pub fn block_current(new_state: ThreadState)  // scheduler.rs:327

// Unblock a thread by ID. Safe to call from IRQ context.
// Saves/restores DAIF to work from any context.
pub fn unblock(tid: ThreadId)            // scheduler.rs:358

// Timer tick handler (called from IRQ at 1 kHz).
// Uses try_lock to avoid deadlock from same-core re-entrancy.
pub fn timer_tick()                      // scheduler.rs:114
```

**Gotchas**:

- **IN_SCHEDULER re-entrancy guard** (scheduler.rs:157): `IN_SCHEDULER[cpu].swap(true, Acquire)` prevents recursive schedule() calls. If a timer IRQ fires while schedule() is running on the same core, timer_tick() sees the guard is set and returns immediately. Forgetting this guard causes stack overflow from recursive scheduling.

- **Context switch return ambiguity** (scheduler.rs:263-272): After `save_context()` returns, the thread might be the *original* thread being resumed later (not the thread that just saved). The code must re-read the CPU ID and CURRENT_THREAD to determine which case it is. Do not cache CPU ID across a save_context call.

- **Lock release before restore_context** (scheduler.rs:247): The THREAD_TABLE lock MUST be dropped before calling `restore_context()`. If the lock is held during restore, a timer IRQ on the newly-running thread will deadlock trying to reacquire.

- **timer_tick() uses try_lock** (scheduler.rs:114-141): Because timer_tick() runs from IRQ context, it cannot block. It uses `try_lock()` on both THREAD_TABLE and RUN_QUEUES. If either is held (by the same core's schedule()), it returns without acting. This is correct; the pending NEED_RESCHED flag will be picked up on the next schedule() call.

**Lock ordering**:

```text
THREAD_TABLE (global) → CURRENT_THREAD[cpu] (per-CPU) → RUN_QUEUES[cpu]
```

Always lock THREAD_TABLE first. CURRENT_THREAD locks are nested inside THREAD_TABLE critical sections. timer_tick() uses try_lock on both to avoid deadlock from same-core IRQ.

**Common mistakes**:

```text
WRONG: Calling schedule() with IRQs unmasked
RIGHT: Mask IRQs first with asm!("msr DAIFSet, #0x2"), then call schedule()

WRONG: Caching cpu_id across a save_context() call
RIGHT: Re-read core_id() after save_context returns (you might be on a different CPU)

WRONG: Holding THREAD_TABLE lock while calling restore_context()
RIGHT: Drop the lock, then call restore_context()
```

### 6.2 IPC

**Files**: `kernel/src/ipc/channel.rs`, `kernel/src/ipc/direct.rs`, `kernel/src/ipc/timeout.rs`, `kernel/src/ipc/notify.rs`, `kernel/src/ipc/select.rs`, `kernel/src/ipc/shmem.rs`

**Key APIs**:

```rust
// Synchronous IPC call: send request and block for reply.
// Returns bytes received on success, or negative error code.
pub fn ipc_call(
    channel: ChannelId,
    send_buf: &[u8],
    recv_buf: &mut [u8],
    timeout_ticks: u64,
) -> i64                                 // channel.rs:33

// Receive a message from a channel. Blocks if no message pending.
// Returns (bytes_received, sender_tid) on success.
pub fn ipc_recv(
    channel: ChannelId,
    recv_buf: &mut [u8],
    timeout_ticks: u64,
) -> Result<(usize, ThreadId), i64>     // channel.rs:232

// Reply to a pending caller. No capability check required (spec §9.1).
pub fn ipc_reply(
    channel: ChannelId,
    reply_buf: &[u8],
) -> i64                                 // channel.rs:355

// Non-blocking send (fire and forget).
pub fn ipc_send(
    channel: ChannelId,
    send_buf: &[u8],
) -> i64                                 // channel.rs:421

// Create a new IPC channel. Requires ChannelCreate capability.
pub fn channel_create(
    creator: ThreadId,
) -> Result<ChannelId, i64>              // ipc/mod.rs:149

// Multi-wait on channels + notifications (select/poll).
pub fn ipc_select(
    entries: &[SelectEntry],
    timeout_ticks: u64,
) -> Result<(usize, u64), i64>          // select.rs:44
```

**Gotchas**:

- **Capability check placement** (channel.rs:49-57): Every IPC operation checks capabilities *before* locking CHANNEL_TABLE. This prevents state corruption on unauthorized access. Cap check order: resolve thread → find process → check capability → proceed.

- **Only one pending caller per channel** (channel.rs:81): A channel supports at most one outstanding call (single-request model). A second `ipc_call()` to a channel with a pending caller returns `Eagain`. This is architectural, not a bug.

- **Direct switch conditions** (channel.rs:132-144): Direct switch (bypassing the scheduler) succeeds only if: both caller and receiver threads exist, receiver is in `BlockedIpc` state, and both are on the same CPU. On failure, falls back to scheduler path. The receiver must be unblocked *after* dropping the CHANNEL_TABLE lock to avoid select waker inconsistency.

- **Reply buffer lifetime** (channel.rs:107-114): The `ReplySlot` holds a pointer to the caller's recv_buf. The receiver copies directly into this buffer during `ipc_reply()`. The buffer must remain valid for the entire IPC round-trip. Do not use stack-allocated reply buffers that go out of scope before the reply arrives.

- **Timeout registration before blocking** (channel.rs:118-126): Timeouts are registered in the TIMEOUT_QUEUE *before* the thread blocks. If direct switch fails and falls through to the scheduler, the timeout is still active. On timeout expiry, the timeout queue wakes the thread with an error code.

- **ipc_reply() needs no capability** (channel.rs:355): Per ipc.md §9.1, replying to a pending caller does not require a capability check. The caller already proved authorization when it initiated the call.

**Lock ordering**:

```text
CHANNEL_TABLE → TIMEOUT_QUEUE → SELECT_WAITERS → sched::RUN_QUEUES
```

Drop CHANNEL_TABLE before calling `sched::unblock()` to prevent circular wait with the scheduler.

**Common mistakes**:

```text
WRONG: Locking CHANNEL_TABLE and then calling sched::unblock() (holds table lock)
RIGHT: Drop CHANNEL_TABLE lock, then call sched::unblock(tid)

WRONG: Using SVC to invoke ipc_call() from a kernel (EL1) thread
RIGHT: Call ipc_call() directly as a function call (SVC is for EL0 only)

WRONG: Using a stack-local buffer as recv_buf for ipc_call(), then returning
RIGHT: Use a buffer with lifetime that spans the entire blocking period
```

### 6.3 Capabilities

**Files**: `kernel/src/cap/mod.rs`

**Key APIs**:

```rust
// Check ChannelCreate capability. Returns authorizing token ID.
pub fn check_channel_create(
    pid: ProcessId,
) -> Result<CapabilityTokenId, i64>      // cap/mod.rs:58

// Check ChannelAccess(channel_id) capability. Fail-closed.
pub fn check_channel_access(
    pid: ProcessId,
    channel: ChannelId,
) -> Result<(), i64>                     // cap/mod.rs:85

// Grant a capability token to a process.
pub fn grant_to_process(
    pid: ProcessId,
    cap: Capability,
    delegatable: bool,
) -> Result<CapabilityHandle, i64>       // cap/mod.rs:169

// Revoke a token (and all children) from a process.
// Cascades: destroys channels created under the revoked capability.
pub fn revoke_in_process(
    pid: ProcessId,
    token_id: CapabilityTokenId,
)                                        // cap/mod.rs:197
```

**Gotchas**:

- **Token expiry** (cap/mod.rs:59, 86): All enforcement checks pass `TICK_COUNT.load()` as "now" to capability table methods. Tokens can have an optional `expires_at_tick`. An expired token fails the check even if present. Ensure tick counting is accurate across subsystems.

- **Cascade revocation lock ordering** (cap/mod.rs:214-236): `revoke_in_process()` first locks PROCESS_TABLE to mark tokens revoked, then *drops* PROCESS_TABLE, then locks CHANNEL_TABLE to destroy channels. This lock-drop-relock pattern is deliberate to maintain the PROCESS_TABLE > CHANNEL_TABLE ordering.

- **Process exit implicit revocation** (cap/mod.rs:61-68): All cap checks validate the process exists in PROCESS_TABLE. When a process exits (slot becomes `None`), all its capabilities are implicitly void. No explicit revocation needed.

- **CapabilityTable is per-process, max 256** (shared/src/cap.rs): Each process has `[Option<CapabilityToken>; 256]`. Handle allocation is O(n) scan for `None` slot. Don't assume constant-time allocation.

**Lock ordering**:

```text
PROCESS_TABLE → CHANNEL_TABLE (never reverse)
```

When checking caps before IPC, PROCESS_TABLE is locked first. Cascade revocation drops PROCESS_TABLE before CHANNEL_TABLE walk to avoid inversion.

### 6.4 Services

**Files**: `kernel/src/service/mod.rs`

**Key APIs**:

```rust
// Register a service by name. Checks name uniqueness.
pub fn service_register(
    name: &[u8],
    pid: ProcessId,
    channel: ChannelId,
) -> Result<(), i64>                     // service/mod.rs:48

// Look up a service by name. Returns (pid, channel) if Running.
pub fn service_lookup(
    name: &[u8],
) -> Option<(ProcessId, ChannelId)>      // service/mod.rs:86

// Mark services owned by a process as Dead.
pub fn service_on_death(
    pid: ProcessId,
)                                        // service/mod.rs:97

// Append event to the audit ring.
pub fn audit_log(
    pid: ProcessId,
    event: &[u8],
)                                        // service/mod.rs:156
```

**Gotchas**:

- **Dead entries are never removed** (service/mod.rs:97-116): When a process exits, `service_on_death()` marks its entries as `Dead` but doesn't remove them. Dead entries remain in the registry permanently. If MAX_SERVICES (16) slots fill with dead entries, new registrations fail with `Enospc`.

- **Name collision with dead entries** (service/mod.rs:53-57): The duplicate check scans ALL entries (including Dead ones). You cannot re-register a name that exists in a Dead state. This prevents name recycling without explicit cleanup.

- **Lock drop before audit_log** (service/mod.rs:105-112): `service_on_death()` drops the SERVICE_MANAGER lock before calling `audit_log()`. This prevents potential lock ordering issues if audit_log ever needs another lock.

- **Audit ring contention** (service/mod.rs:153-171): AUDIT_HEAD is atomic (lock-free increment), but AUDIT_RING itself is behind a Mutex. Concurrent audit writes from multiple cores will contend on this mutex. Acceptable for diagnostics, not for high-frequency logging.

**Lock ordering**:

```text
SERVICE_MANAGER (standalone — never acquired while holding PROCESS_TABLE or CHANNEL_TABLE)
```

### 6.5 VirtIO-blk Driver

**Files**: `kernel/src/drivers/virtio_blk.rs`

**Key APIs**:

```rust
// Probe for a VirtIO-blk device and initialize it.
// Strategy: DTB probe → brute-force MMIO scan fallback.
pub fn init(dt: &DeviceTree) -> bool     // virtio_blk.rs:81

// Read a single 512-byte sector.
pub fn read_sector(
    sector: u64,
    buf: &mut [u8; 512],
) -> Result<(), StorageError>            // virtio_blk.rs:113

// Write a single 512-byte sector.
pub fn write_sector(
    sector: u64,
    buf: &[u8; 512],
) -> Result<(), StorageError>            // virtio_blk.rs:123

// Device capacity in sectors, or 0 if no device.
pub fn capacity_sectors() -> u64         // virtio_blk.rs:135
```

**Gotchas**:

- **Legacy v1 MMIO only** (virtio_blk.rs:7-8): The driver checks `version == 1` during probe. Modern v2 devices are rejected. There is no FEATURES_OK negotiation step. Don't try to add v2 support without understanding the full VirtIO spec §3.1 device initialization sequence.

- **Polled I/O with busy-wait** (virtio_blk.rs:20): `POLL_TIMEOUT = 10_000_000` iterations. Each read/write is fully synchronous. No IRQ-based completion. The driver spins checking the used ring. Don't use this in latency-sensitive paths.

- **DMA pool allocation**: Request buffers are allocated from the DMA pool (`Pool::Dma`), not the kernel pool. DMA memory must be physically contiguous and the device accesses it via physical addresses. Using kernel pool memory would cause cache coherency issues or mapping failures.

- **Single request buffer** (virtio_blk.rs:29-48): The driver holds one VirtioBlk instance with pre-allocated `req_phys`/`req_virt` buffers. There is no request queuing -- each I/O is atomic and exclusive. The VIRTIO_BLK mutex serializes all access.

- **Probe address conversion** (virtio_blk.rs:81-110): Physical MMIO addresses are converted to virtual via `MMIO_BASE + phys_base`. The driver always works with virtual addresses after init. Don't pass raw physical MMIO addresses after Phase 2.

**Lock ordering**:

```text
BLOCK_ENGINE > VIRTIO_BLK (BlockEngine calls read/write_sector inside its lock)
```

Never call VirtIO operations while holding CHANNEL_TABLE or other kernel locks.

**Common mistakes**:

```text
WRONG: Passing virtual address to DMA descriptor (hardware reads physical)
RIGHT: Use req_phys for descriptor physical address, req_virt for CPU access

WRONG: Allocating request buffer from Pool::Kernel
RIGHT: Use Pool::Dma for DMA-accessible physically contiguous memory

WRONG: Calling read_sector() from interrupt context (mutex may deadlock)
RIGHT: Only call from thread context with IRQs enabled
```

### 6.6 Block Engine

**Files**: `kernel/src/storage/block_engine.rs`

**Key APIs**:

```rust
// Write a block (crash-safe): WAL append → data write → WAL commit.
// Returns content hash and block location on success.
pub fn write_block(
    &mut self,
    data: &[u8],
) -> Result<(ContentHash, BlockLocation), StorageError>

// Read a block by location. Verifies CRC-32C integrity on read.
pub fn read_block(
    &mut self,
    location: &BlockLocation,
    buf: &mut [u8],
) -> Result<(), StorageError>

// Format a new disk: write superblock, initialize WAL.
pub fn format(total_sectors: u64) -> Result<(), StorageError>

// Initialize from existing disk: read and validate superblock.
pub fn init() -> Result<(), StorageError>

// Compute CRC-32C checksum.
pub fn crc32c(data: &[u8]) -> u32        // block_engine.rs:44
```

**Gotchas**:

- **Crash safety via WAL** (block_engine.rs:1-6): Write path is: append WAL entry (uncommitted) → write data sectors → mark WAL entry committed. On crash before commit, entry is discarded during recovery. On crash after commit, entry is replayed.

- **CRC-32C offset precision** (block_engine.rs:69-137): The superblock checksum covers a specific byte range of the repr(C) layout. Off-by-one in the checksum range causes validation failures on read. When modifying the Superblock struct, always update the checksum computation range and add a compile-time size assertion.

- **read_unaligned for superblock**: Superblock is assembled from an array of 512-byte sector reads into a byte buffer. The resulting pointer is not guaranteed to be aligned to Superblock's alignment. Must use `ptr::read_unaligned()` -- `ptr::read()` would cause UB on misaligned access.

- **Superblock write is not atomic**: Superblock spans 8 sectors. Write is a loop of 8 `write_sector()` calls. A crash mid-write produces a partially updated superblock. Recovery checks `is_valid()` (magic + version + checksum), so partial writes fail validation gracefully.

**Lock ordering**:

```text
BLOCK_ENGINE (global) → VIRTIO_BLK (driver)
```

BlockEngine holds its lock during all write_block/read_block/recover operations. Never hold CHANNEL_TABLE or other kernel locks while calling BlockEngine functions.

### 6.7 WAL & MemTable

**Files**: `kernel/src/storage/wal.rs`, `kernel/src/storage/lsm.rs`

**Key APIs**:

```rust
// WAL: append a new entry (uncommitted).
// Returns (sequence_number, logical_index).
pub fn append(
    &mut self,
    block_id: ContentHash,
    data_offset: u64,
    data_size: u32,
) -> Result<(u64, u64), StorageError>    // wal.rs:118

// WAL: mark entry at index as committed (O(1)).
pub fn commit_at(
    &mut self,
    index: u64,
) -> Result<(), StorageError>            // wal.rs:151

// WAL: find and commit by sequence number (O(n) scan).
pub fn commit(
    &mut self,
    sequence_number: u64,
) -> Result<(), StorageError>            // wal.rs:160

// MemTable: insert or dedup (refcount bump).
// Returns true if new insertion, false if dedup.
pub fn insert(
    &mut self,
    key: ContentHash,
    location: BlockLocation,
) -> Result<bool, StorageError>          // lsm.rs:73

// MemTable: lookup by content hash (O(log n)).
pub fn get(
    &self,
    key: &ContentHash,
) -> Option<&MemTableEntry>              // lsm.rs:59
```

**Gotchas**:

- **WAL entry size is exactly 64 bytes** (wal.rs:39): `const _: () = assert!(size_of::<WalEntry>() == WAL_ENTRY_SIZE);`. The `repr(C)` layout must stay at 64 bytes for sector alignment (8 entries per 512-byte sector). Adding fields breaks the on-disk format.

- **WAL checksum covers bytes 0..56** (wal.rs:42-47): The last 8 bytes (checksum + pad2) are excluded. `compute_checksum()` must remain in sync with the layout. Changing field order breaks the checksum.

- **Uncommitted entries discarded on recovery** (wal.rs:5): Entries with `committed == 0` are skipped during replay. This is the crash-safety mechanism. Data blocks corresponding to uncommitted entries remain on disk but are not indexed in the MemTable.

- **MemTable capacity is a hard limit** (lsm.rs:84-86): 65536 entries maximum. When full, `insert()` returns `MemTableFull` even for dedup operations. Recovery requires flushing the MemTable to a persistent SSTable (future L0 flush).

- **Dedup via refcount** (lsm.rs:78-82): If the same ContentHash (SHA-256 of data) is inserted twice, the existing entry's refcount is incremented instead of creating a duplicate. This is content-addressed deduplication.

- **Binary search key is [u8; 32]** (lsm.rs:112): Comparison is lexicographic byte-by-byte (`entry.key.0.cmp(&key.0)`). Changing the hash algorithm requires updating both the hash size and the comparison.

**Lock ordering**:

Both WAL and MemTable have no internal locks. They are accessed exclusively through the outer `BLOCK_ENGINE` mutex. Never access them directly without holding BlockEngine's lock.

-----

## 7. Team & Agent Workflow

AIOS uses an autonomous agent team for development. Understanding the workflow helps agents collaborate correctly.

### Agent roles

| Agent | Role | Spawned by |
|---|---|---|
| **team-lead** | Orchestrates phases, manages tasks, commits, creates PRs | User or `/build-team` |
| **kernel-dev** | Implements Rust/asm code per phase doc steps | team-lead |
| **doc-writer** | Generates phase docs from architecture docs | team-lead |
| **code-reviewer** | Runs quality gates, reviews code conventions | team-lead |
| **verifier** | Boots QEMU, validates acceptance criteria | team-lead |
| **doc-auditor** | Validates docs on every change, loops until clean | Hook (auto) or team-lead |

### How kernel-dev receives work

1. team-lead reads the phase doc and creates a TodoWrite task list
2. team-lead spawns kernel-dev with a specific step number and context
3. kernel-dev reads the phase doc step, relevant architecture docs, and this guide
4. kernel-dev implements the step, runs acceptance criteria
5. kernel-dev reports completion back to team-lead

### Post-implementation audit loop (mandatory)

Before any PR is created, three audits must pass with zero issues:

1. **Doc audit** -- Cross-reference errors, terminology, technical accuracy in all modified docs
2. **Code review** -- Convention compliance, unsafe documentation, W^X, naming, dead code
3. **Security/bug review** -- Logic errors, address confusion (virt vs phys), PTE bit correctness, race conditions

Fix all genuine issues, commit, and re-run all three audits. Repeat until a full round returns 0 issues across all three categories.

### Post-PR review workflow

1. Push branch, create PR to `main`
2. Wait 3-5 minutes for Copilot/automated reviewer comments
3. Read each comment, fix the issue in code
4. Reply to the comment explaining the fix
5. Resolve the conversation
6. Push the fix commit
7. Repeat until all comments are resolved

### Branch naming

```text
claude/phase-N-MK-name      -- milestone implementations
claude/phase-N-docs          -- phase doc generation
claude/docs-update-*         -- architecture doc updates
```
