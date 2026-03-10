# Phase 3: IPC & Capability System

**Tier:** 1 — Hardware Foundation
**Duration:** 6 weeks
**Deliverable:** Synchronous IPC with < 10 μs round-trip, capability-enforced channels, scheduler with 4 scheduling classes, kernel observability, service manager
**Status:** Planned
**Prerequisites:** Phase 2 (Memory Management)
**Unlocks:** Phase 4 (Block Storage & Object Store), Phase 28 (Composable Capability Profiles)

-----

## Objective

Build the IPC subsystem, scheduler, capability system, and kernel observability infrastructure on top of Phase 2's memory management. Phase 2 established buddy allocators with page pools, 4-level page tables with W^X enforcement, slab allocator with per-CPU magazines, a typed kernel heap (`kalloc`/`kfree`), and per-agent address spaces with TTBR0 switching. Phase 3 uses these to implement the microkernel's core communication and scheduling mechanisms.

The IPC subsystem provides synchronous call-reply messaging with mandatory timeouts, zero-copy shared memory transfers, and a direct-switch fast path that bypasses the scheduler when caller and receiver are on the same channel. The scheduler implements 4 scheduling classes (Real-Time, Interactive, Normal, Idle) with per-CPU run queues, priority inheritance across IPC boundaries, and a periodic load balancer. The capability system enforces access control on every channel operation through unforgeable tokens with attenuation and revocation. Kernel observability replaces raw `println!()` with per-core structured logging, sharded metric counters, and compile-time-switchable trace points.

By the end of this phase, two kernel threads perform IPC ping-pong with measured round-trip latency < 10 μs (target < 5 μs), a minimal service manager spawns test services, and Gate 1 (Kernel Architecture viability) data is printed to UART.

-----

## Architecture References

| Topic | Document | Relevant Sections |
|---|---|---|
| IPC design, channels, messages, syscalls | [ipc.md](../kernel/ipc.md) | §2 Architecture; §3 Syscall Interface; §4 IPC Design; §6 Notifications; §8 Security; §9 Performance Design |
| Synchronous IPC and zero-copy transfers | [ipc.md](../kernel/ipc.md) | §4.2 Synchronous IPC; §4.4 Zero-Copy Transfers; §4.5 Shared Memory Lifecycle |
| Capability transfer via IPC | [ipc.md](../kernel/ipc.md) | §4.6 Capability Transfer; §8.3 Capability Enforcement |
| Kernel resource limits | [ipc.md](../kernel/ipc.md) | §3.3 Kernel Resource Limits |
| Service protocol and restart | [ipc.md](../kernel/ipc.md) | §5.4 Multi-Client Service Model; §5.5 Service Restart |
| IPC fast path and priority inheritance | [ipc.md](../kernel/ipc.md) | §9.1 Fast Path; §9.2 Priority Inheritance Across IPC |
| Scheduler architecture and classes | [scheduler.md](../kernel/scheduler.md) | §3 Architecture; §3.1 Scheduling Classes; §3.2 Scheduler Architecture; §3.3 SchedEntity |
| Context switch and IPC direct switch | [scheduler.md](../kernel/scheduler.md) | §4 Context Switch; §4.1 Save/Restore; §4.2 IPC Direct Switch; §4.3 Latency Budget |
| Timer, preemption, time slices | [scheduler.md](../kernel/scheduler.md) | §10 Timer and Preemption; §10.2 Time Slices; §10.3 Preemption Model |
| Load balancing | [scheduler.md](../kernel/scheduler.md) | §9 Multi-Core Load Balancing; §9.1 Strategy |
| Deadlock prevention layers | [deadlock-prevention.md](../kernel/deadlock-prevention.md) | §3 Lock Ordering; §4 Mandatory IPC Timeouts; §5 Priority Inheritance; §7 Capability-Based Resource Model; §8 Synchronous IPC |
| Structured logging | [observability.md](../kernel/observability.md) | §2 Structured Logging; §2.4 Log Entry; §2.5 Per-Core Ring Buffer; §2.6 Logging Macros; §2.7 UART Drain |
| Metric counters | [observability.md](../kernel/observability.md) | §3 Metric Counters; §3.2 Counter; §3.3 Gauge; §3.4 Histogram; §3.5 Kernel Metrics Registry |
| Trace points | [observability.md](../kernel/observability.md) | §4 Trace Points; §4.2 Trace Events; §4.4 Per-Core Trace Ring |
| Capability token lifecycle | [security.md](../security/security.md) | §2.2 Layer 2: Capability Check; §3 Capability System Internals; §3.1 Capability Token Lifecycle; §3.2 Kernel Capability Table; §3.3 Attenuation |
| Agent address spaces and shared memory | [memory.md](../kernel/memory.md) | §5.1 Agent Address Spaces; §7 Shared Memory; §9.5 Guard Pages |
| Memory hardening | [fuzzing-and-hardening.md](../security/fuzzing-and-hardening.md) | §3.3 Memory Hardening |

-----

## Milestones

Milestones are numbered continuously across all phases. Phase 2 used M7–M9; Phase 3 continues with M10–M12.

| Milestone | Steps | Target | Observable result |
|---|---|---|---|
| **M10 — Kernel Observability & Process Model** | 1–4 | End of week 2 | Structured logging to UART; thread create/destroy; SVC trap dispatches syscalls; timer tick at 1 kHz |
| **M11 — Scheduler & IPC Core** | 5–8 | End of week 4 | 4-class scheduler across 4 cores; IpcCall/IpcReply round-trip measured; direct switch; capability enforcement |
| **M12 — Shared Memory, Service Manager & Gate 1** | 9–12 | End of week 6 | Shared memory lifecycle; notifications; service manager spawns test service; IPC benchmark < 10 μs; Gate 1 data |

-----

## Milestone 10 — Kernel Observability & Process Model (End of Week 2)

*Goal: Replace `println!()` with structured per-core logging, build the Thread/Process model, wire the SVC exception handler for syscall dispatch, and enable the 1 kHz timer tick for preemption.*

-----

### Step 1: Structured Logging Infrastructure

**What:** Implement per-core LogRing, LogEntry, log levels, subsystem tags, and `klog!`/`kinfo!`/`kwarn!`/`kerror!` macros. Replace all existing `println!()` calls in the kernel with structured logging macros. Add UART drain function.

**Tasks:**
- [ ] Create `kernel/src/observability/mod.rs` — `LogLevel` enum (Trace/Debug/Info/Warn/Error/Fatal), `Subsystem` enum (Boot/Mm/Sched/Ipc/Cap/Irq/Timer/Uart/Gic/Mmu/Smp/Storage/Audit), `LogEntry` struct (64 bytes: timestamp, core_id, level, subsystem, flags, msg_len, message[48])
- [ ] Implement `LogRing` — 256 entries (16 KiB per core), single-producer/single-consumer ring buffer with `head`/`tail` `AtomicU32`. Static `LOG_RINGS: [LogRing; MAX_CORES]` array (observability.md §2.5)
- [ ] Implement `log_impl(level, subsystem, args)` — writes to current core's ring buffer; reads `MPIDR_EL1` for core ID, `CNTVCT_EL0` for timestamp
- [ ] Implement `klog!`, `kinfo!`, `kwarn!`, `kerror!`, `kdebug!`, `ktrace!` macros with compile-time level filtering via `cfg(feature = "log-level-*")` (observability.md §2.6)
- [ ] Early boot fallback: before `LogRingsReady` phase, `klog!` writes directly to UART (observability.md §2.8)
- [ ] Add `LogRingsReady` variant to `EarlyBootPhase` enum in `boot_phase.rs`
- [ ] Implement UART drain function: reads all per-core rings round-robin, formats `[secs.micros] [core] LEVEL Subsys Message`, writes to UART (observability.md §2.7)
- [ ] Replace all `println!("[boot]..."` / `println!("[mm]..."` calls in `main.rs`, `smp.rs`, `mm/*.rs` with appropriate `kinfo!`/`kwarn!`/`kerror!` calls
- [ ] Create `kernel/src/observability/metrics.rs` — `Counter` (per-core sharded `AtomicU64`), `Gauge` (`AtomicI64`), `Histogram<N>` (fixed-bucket), `KernelMetrics` BSS registry with initial counters (mm_page_alloc, mm_page_free, mm_slab_alloc, mm_slab_free, irq_total, irq_timer) (observability.md §3.2–3.5). **Note:** observability.md §8 schedules `Histogram` for Phase 4; this phase pulls it forward because IPC round-trip and context switch latency histograms are needed for Gate 1 benchmarking.
- [ ] Feature gate: `cfg(feature = "kernel-metrics")` — when disabled, Counter/Gauge/Histogram become zero-sized no-ops (observability.md §3.6)
- [ ] Create `kernel/src/observability/trace.rs` — `TraceEvent` enum, `TraceRecord` (32 bytes), `TraceRing` (4096 entries = 128 KiB/core), `trace_point!` macro. Feature-gated: `cfg(feature = "kernel-tracing")`, off by default in release (observability.md §4.2–4.5). **Note:** observability.md §8 schedules trace infrastructure for Phase 4; this phase pulls it forward because the scheduler and IPC direct switch are difficult to debug without trace points. The feature gate ensures zero cost when disabled.

**Key reference:** [observability.md §2–4](../kernel/observability.md) — Structured Logging, Metric Counters, Trace Points

**Acceptance:** `just run` produces UART output in the structured format:
```
[   0.003142] [0] INFO  Boot  Boot sequence starting...
[   0.004501] [0] INFO  Mm    Pool init: 32768 pages in Kernel
```
No remaining raw `println!()` calls in kernel source (except panic handler and macro definitions). `just check` passes.

-----

### Step 2: Thread and Process Control Structures

**What:** Define Thread, ThreadContext, FpContext, SchedEntity, ProcessControl, and ThreadState. Implement kernel thread creation and tracking. No scheduling yet — threads are created and tracked but not dispatched.

**Tasks:**
- [ ] Create `kernel/src/task/mod.rs` — `Thread` struct with `ThreadId`, `ThreadContext` (GP regs x0–x30, SP_EL0, ELR_EL1, SPSR_EL1, TTBR0, per-thread timer state), `FpContext` (v0–v31, FPCR, FPSR) (scheduler.md §4.1)
- [ ] Define `ThreadState` enum: `Runnable`, `Running`, `BlockedIpc { channel: ChannelId }`, `BlockedTimer { wake_at: Timestamp }`, `BlockedIo`, `Suspended`, `Dead` (scheduler.md §3.3)
- [ ] Define `SchedEntity` — `thread_id`, `agent_id`, `class` (`SchedulerClass` enum: RealTime/Interactive/Normal/Idle), `priority` (u8), `deadline` (Option), `cpu_quota`, `vruntime` (u64), `time_slice_remaining`, `effective_class`/`effective_priority`, `inherited_class`/`inherited_priority`/`inherited_deadline`, `affinity` (CpuSet), `state` (scheduler.md §3.3)
- [ ] Create `kernel/src/task/process.rs` — `ProcessControl` struct: `pid` (ProcessId), `address_space` (UserAddressSpace from Phase 2 `uspace.rs`), `capability_table`, `resource_limits` (KernelResourceLimits), `threads` list (ipc.md §3.3)
- [ ] Define `KernelResourceLimits` — `max_channels`, `max_shared_regions`, `max_pending_messages`, `max_notification_subscriptions`, `max_child_processes` with trust-level defaults (ipc.md §3.3)
- [ ] Implement `Thread::new_kernel(entry_fn, stack_page)` — creates a kernel thread with initial context (ELR = entry, SPSR = EL1h, SP = stack top, TTBR0 = kernel)
- [ ] Static `PROCESS_TABLE` and `THREAD_TABLE` (bounded slab-backed arrays)
- [ ] Unit tests in `shared/` for `KernelResourceLimits` trust-level defaults, `ThreadState` transitions

**Key reference:** [scheduler.md §3.3](../kernel/scheduler.md) — SchedEntity; [scheduler.md §4.1](../kernel/scheduler.md) — ThreadContext; [ipc.md §3.3](../kernel/ipc.md) — Kernel Resource Limits

**Acceptance:** `just test` passes thread/process structure tests. `just check` passes with zero warnings. `Thread::new_kernel` compiles and produces a valid `ThreadContext`.

-----

### Step 3: Syscall Dispatch (SVC Handler)

**What:** Wire the "Lower EL using AArch64 — Synchronous" exception vector entry to a syscall dispatcher. Define the syscall number table and error codes. Implement register save/restore for the EL0-to-EL1 transition. Initially, only `DebugPrint` is functional.

**Tasks:**
- [ ] Create `kernel/src/arch/aarch64/trap.rs` — `TrapFrame` struct (x0–x30, SP_EL0, ELR_EL1, SPSR_EL1)
- [ ] Modify exception vector table in `exceptions.rs`: the "Lower EL using AArch64 — Synchronous" entry saves all GP registers to a `TrapFrame` on the kernel stack, reads `ESR_EL1` to determine exception class (EC), dispatches to `svc_handler` if EC == 0x15 (SVC from AArch64)
- [ ] Create `kernel/src/syscall/mod.rs` — `Syscall` enum (numeric IDs: 0=IpcCall, 1=IpcSend, 2=IpcRecv, 3=IpcReply, 4=IpcCancel, 5=IpcSelect, 6=ChannelCreate, 7=ChannelDestroy, ..., 30=DebugPrint) and `IpcError` repr(i32) (ETIMEDOUT=-1 through ECAP_DORMANT=-10) (ipc.md §3.1–3.2)
- [ ] Implement `syscall_dispatch(tf: &mut TrapFrame)`: reads syscall number from `x8`, arguments from `x0`–`x5`, dispatches to handler, writes return value to `tf.x[0]` (ipc.md §3.2)
- [ ] Implement `DebugPrint` handler: validates user pointer (must be in TTBR0 range), copies message to kernel buffer, writes to UART via `klog!`
- [ ] Implement stub handlers for all other syscalls returning `ENOTSUP` (-9)
- [ ] Create `kernel/src/arch/aarch64/context_switch.S` — `save_context` and `restore_context` assembly routines: save/restore callee-saved registers (x19–x30), SP_EL0, ELR_EL1, SPSR_EL1 for kernel-to-kernel switch; full frame for user-to-kernel (scheduler.md §4.1)

**Key reference:** [ipc.md §3.1–3.2](../kernel/ipc.md) — Syscall Table, Syscall ABI; [scheduler.md §4.1](../kernel/scheduler.md) — Context save/restore

**Acceptance:** `just run` with a test that triggers `SVC #0` from EL1 with x8=30 (DebugPrint) prints the message to UART via structured logging. Exception does not hang. `just check` passes.

-----

### Step 4: Timer Tick and Preemption Support

**What:** Wire the ARM Generic Timer to fire IRQs at 1 kHz. The IRQ handler updates tick counters, calls the UART log drain, and sets the `need_resched` flag on the current thread. Implement `TimeGet`, `TimeSleep`, and `TimerSet` syscalls.

**Tasks:**
- [ ] Wire GICv3 IRQ handler in exception vector table: "Current EL with SP_ELx — IRQ" entry saves minimal state, reads IAR, dispatches to `irq_handler`
- [ ] `irq_handler` in `gic.rs`: reads IAR, if INTID == 30 (PPI for EL1 physical timer), calls `timer_tick_handler`, writes EOIR
- [ ] `timer_tick_handler` in `timer.rs`: rearm timer for next 1 ms tick (62500 counts at 62.5 MHz), increment global tick counter, call `observability::drain_logs()`, set `need_resched` flag on current thread's `SchedEntity`
- [ ] Implement `TimeGet` syscall: reads `CNTVCT_EL0`, returns monotonic nanoseconds
- [ ] Implement `TimeSleep` syscall: computes `wake_at = now + duration`, sets thread state to `BlockedTimer`, triggers reschedule (stub: immediately returns until scheduler is wired in Step 5)
- [ ] Implement `TimerSet` syscall: sets a one-shot or repeating timer that wakes `IpcSelect`; stub returns `ENOTSUP` until `IpcSelect` is wired in Step 10 (ipc.md §3.1)
- [ ] Metric instrumentation: increment `KernelMetrics.irq_timer` on every tick
- [ ] Enable timer interrupts on boot CPU: unmask `DAIF.I` after VBAR and GIC init
- [ ] Enable timer interrupts on secondary cores via `init_gicv3_secondary` (existing from Phase 1)

**Key reference:** [scheduler.md §10](../kernel/scheduler.md) — Timer and Preemption; [observability.md §2.7](../kernel/observability.md) — UART Drain; [ipc.md §3.1](../kernel/ipc.md) — TimeGet, TimeSleep, TimerSet

**Acceptance:** `just run` shows periodic structured log drain output from the timer tick (log entries appear at regular intervals, not just at boot). Timer tick counter increments visibly in log output. `just check` passes.

-----

## Milestone 11 — Scheduler & IPC Core (End of Week 4)

*Goal: Full 4-class scheduler with context switching across all cores, IPC channels with synchronous call/reply, direct switch fast path, and capability enforcement on every channel operation.*

-----

### Step 5: Scheduler — Run Queues and Context Switch

**What:** Implement the 4-class per-CPU run queue structure and context switching. Kernel threads can be created, enqueued, and scheduled across all 4 cores. The idle loop on each core becomes a proper idle thread.

**Tasks:**
- [ ] Create `kernel/src/sched/mod.rs` — `Scheduler` struct: per-CPU `RunQueue` array, `nr_cpus`, `SchedulerConfig` (tick_hz=1000, interactive_slice=4ms, normal_slice=10ms, idle_slice=50ms, balance_interval=4ms) (scheduler.md §3.2)
- [ ] Implement `RunQueue` with class-specific queues: `rt_queue` (sorted by deadline), `interactive_queue` (priority list with round-robin), `normal_queue` (sorted by vruntime), `idle_queue` (FIFO). Use slab-allocated intrusive containers (scheduler.md §3.1–3.2)
- [ ] Implement `schedule()`: called from timer tick and voluntary yield points. Picks next thread from highest-priority non-empty class. Saves current thread context via `save_context`, restores next thread context via `restore_context` (scheduler.md §4.1)
- [ ] Wire context switch assembly (`context_switch.S`): full register save/restore (x0–x30, SP_EL0, ELR_EL1, SPSR_EL1), TTBR0 switch with ASID (DSB SY → MSR TTBR0_EL1 → TLBI VMALLE1IS → DSB ISH → ISB), lazy FP save via CPACR_EL1 trap bit (scheduler.md §4.1–4.2)
- [ ] Convert secondary core idle loops (currently `wfe` in `smp.rs`) to proper idle threads that call `schedule()` when woken
- [ ] Timer tick handler calls `schedule()` when `need_resched` is set
- [ ] Implement `thread_yield()` — current thread voluntarily yields, calls `schedule()`
- [ ] Lock ordering: per-CPU run queue locks acquired in ascending CPU ID order (deadlock-prevention.md §3)
- [ ] Metric instrumentation: increment `KernelMetrics.sched_context_switch` on every switch; record latency in `sched_switch_latency_ns` histogram
- [ ] Trace instrumentation: `trace_point!(SchedSwitch { prev_tid, next_tid, prev_state })` (observability.md §4.2)

**Key reference:** [scheduler.md §3–4](../kernel/scheduler.md) — Architecture, Context Switch; [deadlock-prevention.md §3](../kernel/deadlock-prevention.md) — Lock Ordering

**Acceptance:** `just run` shows multiple kernel threads running across all 4 cores (UART output from different cores interleaved via structured logging). Context switch counter is non-zero and incrementing. `just check` passes.

-----

### Step 6: IPC Channels and Synchronous Call/Reply

**What:** Implement Channel, `ChannelCreate`/`ChannelDestroy` syscalls, `IpcCall` with mandatory timeout, `IpcSend` (async fire-and-forget), `IpcRecv`, `IpcReply`, and `IpcCancel`. Message queue is a fixed-size ring buffer. No capability enforcement yet — that comes in Step 8.

**Tasks:**
- [ ] Create `kernel/src/ipc/mod.rs` — `Channel` struct: `id` (ChannelId), `endpoint_a`/`endpoint_b` (ProcessId), `state_a`/`state_b` (EndpointState), `message_queue` (RingBuffer of RawMessage), `stats` (ChannelStatsData) (ipc.md §4.1)
- [ ] Implement `RingBuffer<RawMessage>` with fixed capacity (from `ChannelFlags.queue_depth`, default 64)
- [ ] Implement `RawMessage`: channel, message_type (u32), data pointer (`*const u8`) with length, capability and shared memory arrays (fixed-size, max 4 each) (ipc.md §4.3)
- [ ] Global `CHANNEL_TABLE`: bounded slab-allocated array of `Channel` objects
- [ ] `ChannelCreate` syscall: allocates Channel, returns `ChannelId` (ipc.md §3.1)
- [ ] `ChannelDestroy` syscall: marks endpoint as `Dead`, unblocks peer with `EPIPE` (ipc.md §4.1)
- [ ] `IpcCall` syscall: validates channel, copies message from user buffer to kernel `RawMessage`. If receiver is `BlockedIpc` on this channel, trigger direct switch (Step 7). Otherwise enqueue message, block sender with mandatory timeout. On timeout expiry, unblock sender with `ETIMEDOUT` (ipc.md §4.2; deadlock-prevention.md §4)
- [ ] `IpcRecv` syscall: if message in queue, dequeue and copy to user buffer, return. Otherwise block with timeout (ipc.md §4.2)
- [ ] `IpcReply` syscall: kernel tracks pending caller per channel, copies reply to caller's buffer, unblocks caller (ipc.md §4.2)
- [ ] `IpcSend` syscall: enqueue message without blocking for reply; returns `EAGAIN` if queue full (ipc.md §3.1, §4.2)
- [ ] `IpcCancel` syscall: if caller is blocked, unblock with `ECANCELED` (ipc.md §3.1)
- [ ] Implement `ChannelStats` syscall: copies `ChannelStatsData` for a given channel to user buffer (ipc.md §3.1)
- [ ] Stub `RingChannelCreate` and `RingChannelDestroy` syscalls: return `ENOTSUP` — ring buffer channels are deferred to a later phase when high-frequency streaming IPC is needed (ipc.md §3.1)
- [ ] Peer death cleanup: when process dies, all its channel endpoints set to `Dead`, all blocked peers unblocked with `EPIPE`
- [ ] Metric instrumentation: `KernelMetrics.ipc_call`, `ipc_send`, `ipc_recv`, `ipc_timeout`; update `ChannelStatsData` per operation

**Key reference:** [ipc.md §4.1–4.3](../kernel/ipc.md) — Channels, Synchronous IPC, Message Format; [deadlock-prevention.md §4](../kernel/deadlock-prevention.md) — Mandatory Timeouts

**Acceptance:** `just run` with two kernel threads performing `IpcCall`/`IpcReply` round-trip prints the measured latency to UART. Timeout test: an `IpcCall` to a channel with no receiver times out and returns `ETIMEDOUT`. `ChannelDestroy` on one endpoint causes peer's `IpcRecv` to return `EPIPE`. `just check` passes.

-----

### Step 7: IPC Direct Switch and Priority Inheritance

**What:** Implement the critical IPC fast path: when `IpcCall` finds the receiver already blocked in `IpcRecv` on the target channel, switch directly from sender to receiver without the scheduler. Implement priority inheritance across IPC.

**Tasks:**
- [ ] Create `kernel/src/ipc/direct.rs` — `ipc_direct_switch()`: copy message (A.send_buf → B.recv_buf), set sender state to `BlockedIpc`, donate time slice to receiver, save sender context, switch TTBR0 with ASID (no TLB flush needed), set CPACR trap bit for lazy FP, restore receiver context (ipc.md §9.1; scheduler.md §4.2)
- [ ] Implement `ipc_reply_switch()`: reverse direct switch on `IpcReply` — copy reply, save receiver, restore sender, restore original scheduling context
- [ ] Priority inheritance: when `IpcCall` crosses scheduling classes, receiver temporarily inherits sender's `effective_class`/`effective_priority`. On `IpcReply`, restore receiver's base class/priority. Fields: `inherited_class`, `inherited_priority`, `inherited_deadline` on `SchedEntity` (scheduler.md §4.2; deadlock-prevention.md §5)
- [ ] Register-based small messages: messages ≤ 64 bytes passed via `TrapFrame` registers (x0–x7) without memory copy (ipc.md §4.3)
- [ ] Transitive inheritance: if receiver (now elevated) makes another `IpcCall`, the chain propagates. Kernel enforces max inheritance depth of 8
- [ ] Metric instrumentation: `KernelMetrics.ipc_direct_switch` counter; record round-trip latency in `ipc_roundtrip_ns` histogram
- [ ] Trace instrumentation: `trace_point!(IpcDirectSwitch { from_tid, to_tid })` (observability.md §4.2)

**Key reference:** [ipc.md §9.1–9.2](../kernel/ipc.md) — Fast Path, Priority Inheritance; [scheduler.md §4.2](../kernel/scheduler.md) — IPC Direct Switch; [deadlock-prevention.md §5](../kernel/deadlock-prevention.md) — Priority Inheritance

**Acceptance:** `just run` with IPC ping-pong benchmark between two threads prints round-trip latency. Direct switch path should show < 5 μs round-trip (at 62.5 MHz timer resolution, approximately 312 ticks). Priority inheritance test: Interactive-class thread calls Normal-class service; service runs at Interactive effective priority during the call. `just check` passes.

-----

### Step 8: Capability System and IPC Enforcement

**What:** Implement `CapabilityToken`, `Capability` enum (Phase 3 subset), per-process `CapabilityTable`, and wire capability checks into every IPC channel operation. Implement `CapabilityTransfer`, `CapabilityAttenuate`, `CapabilityRevoke`, `CapabilityList` syscalls.

**Tasks:**
- [ ] Create `kernel/src/cap/mod.rs` — `CapabilityToken`: `id` (CapabilityTokenId), `holder` (ProcessId), `capability` (Capability), `delegatable` (bool), `expiry` (Option<Timestamp>) (security.md §3.1). **Note:** security.md uses the field name `capability` with type `Capability`; this phase implements a Phase 3 subset of the full `Capability` enum.
- [ ] Define `Capability` enum (Phase 3 subset): `ChannelCreate`, `ChannelAccess(ChannelId)`, `SharedMemoryCreate`, `SharedMemoryAccess(SharedMemoryId)`, `SpawnAgent`, `DebugPrint`, plus future-reserved variants (`ReadSpace`, `WriteSpace`, `Network`, `Inference` — stubs for later phases) (security.md §2.2)
- [ ] Per-process `CapabilityTable`: fixed-size array `[Option<CapabilityToken>; 256]` per `ProcessControl` (security.md §3.2, `MAX_CAPS_PER_AGENT = 256`)
- [ ] Wire capability enforcement into IPC syscalls: `ChannelCreate` requires `Capability::ChannelCreate`; `IpcCall`/`IpcSend`/`IpcRecv` require `Capability::ChannelAccess(channel_id)`; `IpcReply` does NOT require a channel capability (kernel tracks the caller per ipc.md §3.1); `ChannelDestroy` requires `ChannelAccess`. Return `EPERM` (-6) on missing capability (ipc.md §8.3)
- [ ] `Channel.creation_capability` field: on `CapabilityRevoke`, kernel walks `CHANNEL_TABLE` and destroys channels whose `creation_capability` was revoked (ipc.md §4.6)
- [ ] `CapabilityTransfer` syscall: verify caller holds cap, verify `delegatable`, move or clone to receiver via channel (ipc.md §4.6; security.md §3.5)
- [ ] `CapabilityAttenuate` syscall: create new cap with subset scope from existing cap (e.g., reduce permissions, add expiry) (security.md §3.3)
- [ ] `CapabilityRevoke` syscall: remove cap from target's table, cascade to derived caps and channels (security.md §3.1)
- [ ] `CapabilityList` syscall: copy caller's capability table entries to user buffer
- [ ] Per-process resource limit enforcement: `ChannelCreate` checks `max_channels`, `SharedMemoryCreate` checks `max_shared_regions`, `IpcSend` checks `max_pending_messages` (ipc.md §3.3)
- [ ] Metric instrumentation: `KernelMetrics.ipc_cap_denied` counter

**Key reference:** [security.md §2.2, §3.1–3.5](../security/security.md) — Capability Check, Token Lifecycle, Kernel Table, Attenuation, Delegation; [ipc.md §4.6, §8.3](../kernel/ipc.md) — Capability Transfer, Enforcement

**Acceptance:** `just run` with test: thread without `ChannelCreate` capability calls `ChannelCreate`, gets `EPERM`. Thread with `ChannelAccess` capability performs `IpcCall` successfully. `CapabilityRevoke` on a channel's creation cap destroys the channel; peer gets `EPIPE`. `just check` passes.

-----

## Milestone 12 — Shared Memory, Service Manager & Gate 1 (End of Week 6)

*Goal: Complete shared memory lifecycle, notifications, a minimal service manager that spawns test services, and produce Gate 1 benchmark data (IPC round-trip < 10 μs, context switch < 20 μs).*

-----

### Step 9: Shared Memory Manager

**What:** Implement `SharedMemoryRegion`, `SharedMemoryCreate`/`SharedMemoryMap`/`SharedMemoryShare`/`MemoryMap`/`MemoryUnmap` syscalls, reference-counted shared memory lifecycle, and cleanup on process death.

**Tasks:**
- [ ] Create `kernel/src/ipc/shmem.rs` — `SharedMemoryRegion`: `id` (SharedMemoryId), `physical_pages` (PageRange from frame allocator, Pool::User), `ref_count` (AtomicU32), `creator` (ProcessId), `max_flags` (VmFlags), `mappings` array `[Option<SharedMapping>; 8]`, `capability` (CapabilityTokenId) (ipc.md §4.5)
- [ ] `SharedMapping`: `process` (ProcessId), `vaddr` (VirtualAddress), `flags` (VmFlags, must be subset of `max_flags`)
- [ ] Global `SHARED_REGION_TABLE`: bounded slab-allocated array
- [ ] `SharedMemoryCreate` syscall: allocates physical pages from Pool::User via frame allocator, creates region, `ref_count = 1`, capability check (`Capability::SharedMemoryCreate`) (ipc.md §4.5)
- [ ] `SharedMemoryMap` syscall: maps region pages into caller's address space via Phase 2 `uspace::map_user_page`, increments `ref_count`, records mapping (ipc.md §4.5)
- [ ] `SharedMemoryShare` syscall: sends region ID through channel (capability check: `Capability::SharedMemoryAccess` + `Capability::ChannelAccess`), with flags attenuation (read-only share supported via `flags` parameter that must be subset of `max_flags`) (ipc.md §4.5)
- [ ] `MemoryMap` syscall: allocates virtual memory in caller's address space with specified flags (ipc.md §3.1). Builds on Phase 2 `uspace::map_user_page`; W^X enforced
- [ ] `MemoryUnmap` for shared and private regions: unmaps from caller's page table, decrements `ref_count` for shared regions, frees pages if `ref_count` reaches 0 (ipc.md §3.1, §4.5; `MemoryUnmap` handles both private and shared mappings)
- [ ] Process death cleanup: iterate process's shared memory mappings, unmap all, decrement `ref_count`, free pages when 0
- [ ] W^X enforcement: shared memory cannot be mapped WRITE + EXECUTE simultaneously (memory.md §9.1)

**Key reference:** [ipc.md §4.4–4.5](../kernel/ipc.md) — Zero-Copy Transfers, Shared Memory Lifecycle; [memory.md §7](../kernel/memory.md) — Shared Memory

**Acceptance:** `just run` with test: process A creates shared region, writes pattern, shares with process B via channel, B maps and reads same pattern. On A death, B's mapping remains valid (`ref_count > 0`). On B death, pages freed (`ref_count = 0`). `just check` passes.

-----

### Step 10: Lightweight Notifications

**What:** Implement lightweight notification objects (single-word bitmap signals). Implement `NotificationCreate`, `NotificationSignal`, `NotificationWait` syscalls and `IpcSelect` for multiplexing.

**Tasks:**
- [ ] Create `kernel/src/ipc/notify.rs` — `NotificationObject`: `id` (NotificationId), `word` (AtomicU64), `waiters` list (bounded array of waiting ThreadId + mask) (ipc.md §6)
- [ ] `NotificationCreate` syscall: allocates notification object, returns `NotificationId` (ipc.md §3.1)
- [ ] `NotificationSignal` syscall: atomic OR of bits into notification word (~10 cycles). If any waiter's mask matches, wake the waiter by enqueuing on run queue (ipc.md §6)
- [ ] `NotificationWait` syscall: if any bits in mask are set, return them and atomically clear. Otherwise block until bits are set or timeout (ipc.md §6)
- [ ] Implement `IpcSelect` syscall: wait on multiple channels and/or notifications simultaneously. Returns which channel/notification is ready. Uses a bounded wait set. Timer-based timeout (ipc.md §3.1)
- [ ] Notification-based wakeup integration with scheduler: signaling a notification that wakes a thread enqueues that thread on its CPU's run queue

**Key reference:** [ipc.md §6](../kernel/ipc.md) — Notification Mechanism; [ipc.md §3.1](../kernel/ipc.md) — IpcSelect

**Acceptance:** `just run` with test: thread A creates notification, thread B waits on it, A signals bits 0x05, B wakes and receives 0x05. `IpcSelect` test: thread waits on two channels and a notification, signal arrives on notification, `IpcSelect` returns. `just check` passes.

-----

### Step 11: Minimal Service Manager

**What:** Implement a kernel-internal service manager that can spawn processes (using Phase 2 user address spaces), distribute channels and capabilities, monitor service health, and perform basic load balancing.

**Tasks:**
- [ ] Create `kernel/src/service/mod.rs` — `ServiceManager`: service registry (name → ProcessId + ChannelId), service lifecycle tracking
- [ ] Implement `ProcessCreate` syscall: allocates `ProcessControl`, creates `UserAddressSpace` (Phase 2 `uspace.rs`), loads minimal test image (raw binary from known physical address), sets up initial thread with entry point, distributes capabilities per `KernelResourceLimits` (ipc.md §3.1)
- [ ] Implement `ProcessExit` syscall: marks process as dead, cleans up all channels (`EPIPE`), shared memory (unmap/deref), capabilities (revoke derived), threads (set `Dead`) (ipc.md §3.1)
- [ ] Implement `ProcessWait` syscall: block until child exits, return exit code (ipc.md §3.1)
- [ ] Service manager bootstrap: at end of kernel boot, service manager creates a "test service" process with a channel to the boot process. Test service enters `IpcRecv` loop, echoes messages (ipc.md §5.4)
- [ ] Implement `AuditLog` syscall: validates user pointer, copies event to kernel audit ring buffer, tags with caller's process ID and timestamp (ipc.md §3.1)
- [ ] Service restart detection: when a service process exits, service manager is notified, can recreate channels (ipc.md §5.5)
- [ ] Load balancer (basic): periodic 4 ms balance check, migrate threads from overloaded to underloaded CPUs using ascending CPU ID lock ordering (scheduler.md §9.1; deadlock-prevention.md §3)
- [ ] Trace instrumentation: `trace_point!(SchedMigrate { tid, from_core, to_core })` (observability.md §4.2)

**Key reference:** [ipc.md §5.4–5.5](../kernel/ipc.md) — Multi-Client Service Model, Service Restart; [scheduler.md §9](../kernel/scheduler.md) — Load Balancing; [deadlock-prevention.md §3](../kernel/deadlock-prevention.md) — Lock Ordering

**Acceptance:** `just run` shows service manager spawning test service, boot process sends `IpcCall` to test service, receives echo reply. Service exit triggers `EPIPE` on peer. Load balancer migrates threads (visible in klog output). `just check` passes.

-----

### Step 12: Gate 1 Benchmark and Integration

**What:** Run the Gate 1 benchmark suite: IPC round-trip latency, context switch latency, capability enforcement overhead. Print results to UART. Verify all Tier 1 success metrics. Run full quality gates.

**Tasks:**
- [ ] Create `kernel/src/bench.rs` — benchmark harness: runs N iterations, computes min/avg/max/p99 latency using `CNTVCT_EL0` (16 ns resolution at 62.5 MHz)
- [ ] IPC round-trip benchmark: two threads on same core, `IpcCall`/`IpcReply` ping-pong, 10000 iterations. Report in microseconds
- [ ] IPC cross-core benchmark: two threads on different cores, same `IpcCall`/`IpcReply` pattern
- [ ] Context switch benchmark: two Normal-class threads yield back and forth, measure switch time
- [ ] Direct switch benchmark: receiver already waiting, `IpcCall` triggers direct switch, measure end-to-end
- [ ] Capability overhead benchmark: `IpcCall` with vs without capability check, measure delta
- [ ] Shared memory throughput: create 1 MB region, write pattern, share, read, measure throughput
- [ ] Print all results to UART in structured format:
  ```
  [bench] IPC round-trip (same core):    avg=X.XX us, p99=X.XX us
  [bench] IPC round-trip (cross core):   avg=X.XX us, p99=X.XX us
  [bench] Context switch:                avg=X.XX us
  [bench] Direct switch:                 avg=X.XX us
  [bench] Capability overhead:           avg=XX ns
  [bench] Gate 1: IPC < 10 us:           PASS/FAIL
  [bench] Gate 1: Context switch < 20 us: PASS/FAIL
  ```
- [ ] Verify Gate 1 criteria: IPC round-trip < 10 μs (target < 5 μs), context switch < 20 μs (development-plan.md §5 Gate 1)
- [ ] Verify `just check` — zero warnings
- [ ] Verify `just test` — all unit tests pass
- [ ] Verify `just run` — complete boot log through benchmark results
- [ ] Update CLAUDE.md: Workspace Layout (add `kernel/src/observability/`, `kernel/src/task/`, `kernel/src/syscall/`, `kernel/src/sched/`, `kernel/src/ipc/`, `kernel/src/cap/`, `kernel/src/service/`, `kernel/src/bench.rs`), Key Technical Facts (IPC latency, context switch time, syscall count)

**Key reference:** [development-plan.md §5](../project/development-plan.md) — Gate 1 Decision; [ipc.md §9.1](../kernel/ipc.md) — Fast Path Budget; [scheduler.md §4.3](../kernel/scheduler.md) — Context Switch Latency Budget

**Acceptance:** All quality gates pass:
```
just check   → zero warnings
just test    → all pass
just run     → boot log shows: structured logging, scheduler running, IPC benchmark results, Gate 1 PASS
```

-----

## Decision Points

| Decision | When | Options | Impact |
|---|---|---|---|
| Run queue data structures | Step 5 | Sorted arrays vs intrusive red-black trees | Sorted arrays are simpler and sufficient for small thread counts; RB-trees scale better but add complexity. Can upgrade in Phase 14. |
| IPC message inline size | Step 6 | 64 bytes (register-only) vs 256 bytes (buffer copy) | 64-byte register path is fastest; 256-byte inline avoids shared memory for medium messages. Both are needed. |
| Capability table storage | Step 8 | Fixed array (256 slots per security.md §3.2) vs slab-backed growable | Fixed array is predictable (no heap allocation during cap operations); 256 matches security.md `MAX_CAPS_PER_AGENT`. |
| Shared memory page source | Step 9 | Pool::User vs Pool::Kernel | User pool is correct — shared memory is for agent data, not kernel structures. Kernel pool reserved for page tables and slab. |
| Load balancer frequency | Step 11 | 4 ms (every 4 ticks) vs adaptive | Fixed 4 ms is simple and matches architecture spec. Adaptive adds complexity for marginal gain at low core counts. |
| Gate 1 threshold | Step 12 | Strict (< 5 μs IPC) vs relaxed (< 10 μs) | Gate uses relaxed threshold (< 10 μs); target (< 5 μs) is for post-optimization in Phase 14. |

-----

## Phase Completion Criteria

- [ ] Structured per-core logging with `klog!` macros replaces all `println!()` in kernel
- [ ] Kernel metrics registry (Counter, Gauge, Histogram) with feature-gated zero-cost disable
- [ ] Compile-time-switchable trace points for scheduler and IPC events
- [ ] Thread and process data structures with `SchedEntity` and `KernelResourceLimits`
- [ ] SVC-based syscall dispatch with all 31 syscall numbers defined
- [ ] 1 kHz timer tick driving preemption and log drain
- [ ] 4-class scheduler (RT/Interactive/Normal/Idle) with per-CPU run queues
- [ ] Context switch with lazy FP save and TTBR0/ASID switching
- [ ] IPC channels with synchronous call-reply and mandatory timeouts
- [ ] IPC direct switch fast path (< 5 μs target round-trip)
- [ ] Priority inheritance across IPC boundaries with transitive support
- [ ] Capability tokens with scope, attenuation, revocation, and delegation
- [ ] Capability enforcement on every channel and shared memory operation
- [ ] Shared memory with reference-counted lifecycle and process death cleanup
- [ ] Lightweight notification objects with bitmap signaling
- [ ] `IpcSelect` for multiplexing channels and notifications
- [ ] Service manager spawns test service with IPC echo
- [ ] Load balancer with ascending CPU ID lock ordering
- [ ] Gate 1 data: IPC round-trip < 10 μs, context switch < 20 μs
- [ ] `just check` — zero warnings
- [ ] `just test` — all unit tests pass
- [ ] `just run` — complete boot log through Gate 1 benchmark PASS
