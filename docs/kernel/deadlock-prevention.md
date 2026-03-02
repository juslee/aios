# AIOS Deadlock Prevention Architecture

**Parent document:** [architecture.md](../project/architecture.md)
**Related:** [ipc.md](./ipc.md) — IPC timeouts (§3.1), synchronous call-reply (§4.2), priority inheritance (§9.2) | [scheduler.md](./scheduler.md) — Lock ordering (§9.1), preemption model (§10.3), async priority inheritance (§13.4) | [memory.md](./memory.md) — Per-CPU magazine allocator (§4.1), kernel singletons (§4.2) | [security.md](../security/security.md) — Capability model

-----

## 1. Overview

Deadlocks occur when two or more threads each hold a resource the other needs, creating a circular wait. Traditional operating systems are plagued by deadlocks because they rely on coarse-grained locking and allow arbitrary resource acquisition orders. AIOS eliminates deadlocks through a layered strategy: **structural prevention** (making deadlocks impossible by design), **timeout-based detection** (bounding the cost when prevention alone is insufficient), and **lock-free fast paths** (avoiding locks entirely in the hottest code paths).

This document catalogs every deadlock prevention mechanism in the AIOS kernel, explains why each works, and describes how they compose into a system where deadlocks are a non-issue rather than a constant threat.

-----

## 2. The Four Coffman Conditions

A deadlock requires all four conditions simultaneously (Coffman et al., 1971):

1. **Mutual exclusion** — a resource can only be held by one thread
2. **Hold and wait** — a thread holds one resource while waiting for another
3. **No preemption** — resources cannot be forcibly taken from a thread
4. **Circular wait** — a cycle exists in the resource dependency graph

AIOS breaks one or more of these conditions at every level of the system, supplemented by liveness mechanisms that bound delays even when structural prevention alone is insufficient. The table below summarizes which condition each mechanism targets:

| Mechanism | Breaks | This doc | Subsystem source |
|---|---|---|---|
| Lock ordering (CPU ID) | Circular wait | §3 | [scheduler.md §9.1](./scheduler.md) |
| Mandatory IPC timeouts | Circular wait (bounded) | §4 | [ipc.md §3.1](./ipc.md) |
| Priority inheritance† | *(liveness)* | §5 | [ipc.md §9.2](./ipc.md), [scheduler.md §4.2](./scheduler.md) |
| Lock-free per-CPU magazines | Mutual exclusion | §6 | [memory.md §4.1](./memory.md) |
| Capability-based resource model | Circular wait (graph constraint) | §7 | [ipc.md §4.1](./ipc.md), [security.md](../security/security.md) |
| Synchronous IPC (no callback chains) | Circular wait | §8 | [ipc.md §4.2](./ipc.md) |
| Preemptive kernel | No preemption | §9 | [scheduler.md §10.3](./scheduler.md) |
| Wait-Die / Wound-Wait | Circular wait | §10 | Future — resource arbitration layer |

†Priority inheritance does not break a Coffman condition directly. It prevents unbounded priority inversion — a liveness hazard where a high-priority thread is indefinitely delayed by lower-priority work (§5.1). It is included here because unbounded priority inversion is operationally indistinguishable from deadlock.

-----

## 3. Lock Ordering in the Scheduler

### 3.1 The Problem

The load balancer migrates threads between CPUs to maintain even utilization. Migrating a thread from CPU A to CPU B requires locking both run queues. If CPU A's balancer tries to pull from CPU B while CPU B's balancer simultaneously pulls from CPU A, a classic ABBA deadlock occurs.

### 3.2 The Solution: Ascending CPU ID Order

AIOS enforces a global lock ordering: **run queue locks are always acquired in ascending CPU ID order**. This breaks the circular wait condition — two CPUs can never form a cycle because both will attempt to lock the lower-numbered CPU first.

```rust
impl Scheduler {
    /// Migrate a thread from src_cpu to dst_cpu.
    /// Locks are acquired in CPU ID order to prevent deadlock.
    fn migrate(&mut self, thread: &SchedEntity, src: CpuId, dst: CpuId) {
        let (first, second) = if src.0 < dst.0 {
            (&self.run_queues[src.0], &self.run_queues[dst.0])
        } else {
            (&self.run_queues[dst.0], &self.run_queues[src.0])
        };
        first.lock.lock();
        second.lock.lock();
        // ... perform migration ...
        second.lock.unlock();
        first.lock.unlock();
    }
}
```

*Source: [scheduler.md §9.1 — Load Balancer Strategy](./scheduler.md) (see lock ordering implementation)*

### 3.3 Why This Works

Lock ordering is a total order over all lockable resources. Any acyclic total order prevents circular wait. CPU IDs are natural, unique, and immutable — they require no runtime bookkeeping. The cost is zero: a single integer comparison before acquiring the second lock.

### 3.4 Scope

This ordering applies to per-CPU run queues. Any future per-CPU lock must follow the same ascending-ID convention.

-----

## 4. Mandatory IPC Timeouts

### 4.1 The Problem

In a microkernel, every system operation is an IPC message. An agent that calls a service and blocks indefinitely is indistinguishable from a deadlocked thread. If Service A calls Service B and Service B calls Service A (directly or transitively through a chain), both block forever.

### 4.2 The Solution: No Unbounded Waits

Every `IpcCall` in AIOS **requires** a timeout. There is no API to block indefinitely on a synchronous IPC call:

```rust
IpcCall {
    channel: ChannelId,
    send_buf: *const u8,
    send_len: usize,
    recv_buf: *mut u8,
    recv_len: usize,
    timeout: Duration,  // mandatory — no default, no "infinite"
},
```

When the timeout elapses, the kernel returns `ETIMEDOUT` and cleans up the pending call state. The caller can retry, fall back, or propagate the error.

*Source: [ipc.md §3.1 — Syscall Table](./ipc.md) (mandatory timeout field), [ipc.md §4.2 — Synchronous IPC](./ipc.md) (call-reply pattern)*

### 4.3 Complementary: IpcCancel

Agents can also explicitly cancel a pending call via `IpcCancel`, which returns `ECANCELED` to the blocked caller. The kernel uses this during process teardown to release all pending IPC state for a dying process — preventing zombie dependencies.

### 4.4 Why This Works

Mandatory timeouts break the **circular wait** condition with a time bound. Even if a circular dependency forms, the cycle is broken within the shortest timeout in the chain — the timed-out caller releases its wait edge, collapsing the cycle. This converts a permanent deadlock into a transient timeout error.

### 4.5 Design Trade-off

Timeouts mean callers must handle `ETIMEDOUT`. This is intentional — AIOS treats unresponsive services as a fault to be handled, not a state to be tolerated. The SDK provides retry helpers with exponential backoff for the common case.

-----

## 5. Priority Inheritance Across IPC

### 5.1 The Problem

Priority inversion is a deadlock-adjacent hazard. When a high-priority Interactive thread calls a Normal-priority service, and that service is preempted by medium-priority work, the high-priority thread is effectively blocked by medium-priority work — indefinitely in the worst case.

### 5.2 The Solution: Scheduling Context Donation

When an `IpcCall` crosses scheduling classes, the kernel temporarily elevates the receiver to the caller's scheduling class:

```rust
unsafe fn ipc_direct_switch(sender: &mut Thread, receiver: &mut Thread, message: &RawMessage) {
    // ... existing copy and switch logic ...

    // Priority inheritance: receiver inherits caller's scheduling context.
    // Saved and restored on IpcReply.
    receiver.sched.inherited_class = Some(sender.sched.class);
    receiver.sched.inherited_priority = Some(sender.sched.priority);
    receiver.sched.inherited_deadline = sender.sched.deadline;

    // If receiver is in a lower class, temporarily elevate
    if receiver.sched.class < sender.sched.class {
        receiver.sched.effective_class = sender.sched.class;
        receiver.sched.effective_priority = sender.sched.priority;
    }
}
```

On `IpcReply`, the receiver's original scheduling context is restored. This is transitive — if Service B calls Service C while holding A's inherited priority, C also inherits A's priority.

*Source: [ipc.md §9.2 — Priority Inheritance Across IPC](./ipc.md) (scheduling context donation code), [scheduler.md §4.2 — IPC Direct Switch](./scheduler.md) (fast-path priority fields)*

### 5.3 Async Tasks

The kernel's async executor applies the same principle. When a high-priority scheduler thread blocks waiting for an async task's result, the async task's priority is temporarily boosted:

```rust
impl KernelExecutor {
    /// Boost an async task's priority because a high-priority thread is waiting for it.
    pub fn boost_priority(&mut self, task_id: AsyncTaskId, waiter_priority: Priority) {
        if let Some(task) = self.tasks.get_mut(&task_id) {
            // Lower numerical value = higher priority (Priority(0) is highest)
            task.priority = task.priority.min(waiter_priority);
            // Re-sort ready queue if the task is ready
        }
    }
}
```

*Source: [scheduler.md §13.4 — Priority Inheritance for Async Tasks](./scheduler.md) (boost_priority implementation)*

### 5.4 Why This Works

Priority inheritance prevents the unbounded blocking that makes priority inversion equivalent to deadlock. The high-priority thread's wait is bounded by the time the service needs to process the request (at the caller's priority level), not by unrelated medium-priority work.

-----

## 6. Lock-Free Fast Paths in the Memory Allocator

### 6.1 The Problem

Memory allocation is the most frequent kernel operation. If every allocation requires a global lock, contention between CPUs creates both performance bottlenecks and deadlock risk (an interrupt handler allocating memory while the interrupted code holds the allocator lock).

### 6.2 The Solution: Per-CPU Magazines

The slab allocator uses a **per-CPU magazine layer** that requires no locks for the common case:

```
Per-CPU Magazine Layer (lock-free fast path)
┌─────────┐ ┌─────────┐ ┌─────────┐
│ CPU 0   │ │ CPU 1   │ │ CPU 2   │ ...
│ current │ │ current │ │ current │
│ prev    │ │ prev    │ │ prev    │
└─────────┘ └─────────┘ └─────────┘
```

Each CPU maintains a small array of pre-allocated objects. Allocating takes an object from the local magazine — no locks, no atomic operations, just a decrement and a pointer load. Only when the magazine is empty does the CPU access the shared slab (which requires a lock).

*Source: [memory.md §4.1 — Slab Allocator](./memory.md) (per-CPU magazine architecture and lock-free fast path)*

### 6.3 Global Singletons

Kernel global allocators are each protected independently:

```rust
// Each is protected by a spin-lock or is inherently lock-free.
static FRAME_ALLOCATOR: FrameAllocator = /* ... */;
static FRAME_REFCOUNT: FrameRefCount = /* atomic counters, lock-free */;
static SLAB_ALLOCATOR: SlabAllocator = /* per-CPU magazines + locked slabs */;
static ZERO_QUEUE: PageZeroQueue = /* ... */;
```

*Source: [memory.md §4.2 — Kernel Allocation API](./memory.md) (global singleton declarations)*

### 6.4 Why This Works

Lock-free fast paths break the **mutual exclusion** condition for the common case. Since each CPU operates on its own magazine, there is no shared resource to contend over. The rare slow path (refilling an empty magazine from the shared slab) uses fine-grained locks with minimal hold times, subject to the system-wide spinlock budget (< 1 μs target — see §9.2).

-----

## 7. Capability-Based Resource Model

### 7.1 The Problem

Traditional OSes use ambient authority — any thread can attempt to open any file, connect to any service, or acquire any resource. This leads to complex locking because any thread might compete for any resource at any time.

### 7.2 The Solution: Unforgeable Capability Tokens

In AIOS, access to any resource requires a capability token. Channels are created with specific capabilities and cannot be used without them. This constrains the resource dependency graph:

- Agents typically communicate through channels pre-established at boot by the Service Manager
- Runtime channel creation requires explicit `ChannelCreate` capability and is subject to per-process limits
- Capability transfer requires explicit kernel mediation

For the common case — agent-to-service communication — the set of possible resource dependencies is **known at boot time** when the Service Manager creates channels (ipc.md §4.1). Processes with `ChannelCreate` capability can create channels at runtime, but the kernel enforces per-process channel limits (`max_channels` in `KernelResourceLimits`), and circular dependencies between services can be detected in the capability topology.

*Source: [ipc.md §4.1 — Channels](./ipc.md) (ChannelCapability struct), [ipc.md §4.6 — Capability Transfer](./ipc.md) (move semantics), [security.md](../security/security.md) (capability model overview)*

### 7.3 Why This Works

Capabilities restrict the dependency graph. When services cannot arbitrarily call each other, circular wait becomes a design error that is visible in the capability topology — not a runtime surprise that emerges under load.

-----

## 8. Synchronous IPC Eliminates Callback Cycles

### 8.1 The Problem

Asynchronous message-passing systems can create subtle deadlocks through callback chains: A sends to B, B's callback sends to C, C's callback sends to A, and all message queues fill up — deadlock through backpressure.

### 8.2 The Solution: Synchronous Call-Reply

AIOS's primary IPC pattern is synchronous `IpcCall`/`IpcReply`. The caller blocks until the reply arrives. This means:

- A thread can only have **one outstanding IPC call** at a time
- The call graph is a tree (or chain), never a DAG with cycles
- Backpressure is automatic — a slow service slows its callers, not the whole system

```
Agent A ──IpcCall──→ Service B ──IpcCall──→ Service C
  (blocked)            (blocked)              (processing)
                                              IpcReply ──→ B
                       (resumes)
         IpcReply ──→ A
(resumes)
```

The asynchronous `IpcSend` (fire-and-forget) exists for notifications and telemetry, but it has explicit backpressure: when the queue is full, `IpcSend` returns `EAGAIN` — it never blocks the sender.

*Source: [ipc.md §4.2 — Synchronous IPC](./ipc.md) (call-reply diagram and backpressure semantics)*

### 8.3 Why This Works

Synchronous IPC creates a strict call chain, but by itself does not guarantee that blocked threads are lock-free. In AIOS we adopt a required coding rule: callers must not hold kernel or user-space locks, or other non-preemptible resources, across an `IpcCall`. Under this rule, a thread that is blocked on an `IpcCall` holds no locks and makes no further blocking calls — it simply waits. This breaks **circular wait** because a blocked thread cannot initiate the other half of a cycle.

-----

## 9. Fully Preemptive Kernel

### 9.1 The Problem

Non-preemptive kernels can deadlock when a thread holding a resource enters a long kernel code path and starves other threads waiting for that resource.

### 9.2 The Solution: Preempt Almost Everywhere

AIOS uses a fully preemptive kernel. User-space threads can be preempted at any instruction boundary. Kernel-mode code can be preempted at most points. Only four narrow regions disable preemption:

1. **Interrupt handler top halves** (< 10 μs)
2. **Spinlock critical sections** (< 1 μs target)
3. **Page table manipulation** (single-page atomic update)
4. **Context switch path** (inherently non-preemptible)

Everything else in the kernel is preemptible. A timer interrupt in preemptible kernel code immediately switches to a higher-priority thread.

*Source: [scheduler.md §10.3 — Preemption Model](./scheduler.md) (preemption-disabled regions list)*

### 9.3 Why This Works

Preemption breaks the **no preemption** condition. If a thread holds a resource too long, the scheduler can preempt it and run the waiting thread (especially with priority inheritance). The only non-preemptible regions are bounded to microseconds — too short for any practical deadlock.

-----

## 10. Wait-Die and Wound-Wait: Timestamp-Based Prevention

### 10.1 Background

Wait-Die and Wound-Wait (Rosenkrantz, Stearns, and Lewis, 1978) are classic deadlock prevention schemes from database concurrency control. Both assign each transaction (or thread) a **timestamp** and use age comparisons to decide whether to wait or abort — guaranteeing no circular wait can ever form.

| Scheme | Older requests resource held by younger | Younger requests resource held by older |
|---|---|---|
| **Wait-Die** | Older **waits** (it has priority) | Younger **dies** (aborted, restarts with same timestamp) |
| **Wound-Wait** | Older **wounds** younger (preempts it) | Younger **waits** (older will finish first) |

Both schemes break the **circular wait** condition: because age is a total order, a cycle of "A waits for B waits for A" is impossible — one side will always abort or be preempted.

### 10.2 How This Relates to AIOS

AIOS does not currently implement Wait-Die or Wound-Wait explicitly, but several of its mechanisms are functionally equivalent:

**Mandatory IPC timeouts (§4) approximate Wait-Die.** When a service call times out and the caller retries, the effect is similar to the "die and restart" behavior in Wait-Die — the younger/less-patient caller aborts its attempt and tries again. The difference is that AIOS uses wall-clock timeouts rather than age-based comparisons.

**Priority inheritance (§5) shares Wound-Wait's intuition but differs in mechanism.** In Wound-Wait, the older (higher-priority) transaction forces the younger holder to *abort* and release the resource. In AIOS's priority inheritance, the holder is *boosted* — its scheduling priority is elevated so it completes faster, but it is not aborted. Both ensure higher-priority work is not indefinitely blocked by lower-priority work, but priority inheritance achieves this through acceleration rather than preemption.

### 10.3 Where Wait-Die / Wound-Wait Could Add Value

If AIOS ever needs to manage **contested shared resources** beyond IPC channels — for example, exclusive access to a hardware device, a shared memory region with write locks, or Space Storage write transactions that conflict — a timestamp-based scheme would provide stronger guarantees than timeouts alone:

```
Scenario: Agent A and Agent B both need exclusive access to
          resources R1 and R2 (in different orders).

With timeouts only:
  A locks R1, requests R2 (held by B) → waits up to timeout
  B locks R2, requests R1 (held by A) → waits up to timeout
  Both time out → both retry → possible livelock (repeated timeouts)

With Wait-Die (agents stamped at creation time):
  A (older) locks R1, requests R2 (held by B, younger) → A waits
  B (younger) locks R2, requests R1 (held by A, older) → B dies (aborts)
  B releases R2 → A acquires R2, completes → no deadlock, no livelock
```

The key advantage over pure timeouts: **Wait-Die and Wound-Wait guarantee progress**, while timeouts can lead to livelock if multiple threads repeatedly time out and retry in sync.

### 10.4 Design Considerations for AIOS

If adopted, the natural timestamp for AIOS would be the **agent creation time** (monotonic, unique, immutable) or the **IPC call sequence number** (for per-request ordering). The scheme would apply at the resource arbitration layer, not within the IPC syscall path itself:

```rust
/// Hypothetical Wait-Die resource arbitration
fn request_resource(requester: &Thread, holder: &Thread, resource: ResourceId) -> WaitOrDie {
    if requester.creation_timestamp < holder.creation_timestamp {
        // Older requester: allowed to wait (it won't cause a cycle)
        WaitOrDie::Wait
    } else {
        // Younger requester: abort and retry (prevents cycle formation)
        WaitOrDie::Die
    }
}
```

This would complement AIOS's existing defenses as a **Layer 8** — a safety net for resource contention patterns that aren't covered by lock ordering or IPC timeouts alone.

-----

## 11. Summary: Defense in Depth

No single mechanism prevents all deadlocks. AIOS layers multiple strategies so that each covers the gaps of the others:

```
Layer 1: Lock ordering           → no circular wait among kernel locks
Layer 2: Mandatory IPC timeouts  → no unbounded waits between services
Layer 3: Priority inheritance    → no priority inversion stalls
Layer 4: Lock-free fast paths    → no contention on hot paths
Layer 5: Capability restrictions → constrained dependency graph
Layer 6: Synchronous IPC         → no callback cycles
Layer 7: Preemptive kernel       → no indefinite resource holding
Layer 8: Wait-Die / Wound-Wait   → progress guarantee for contested resources (future)
```

Layers 1 and 4–7 prevent deadlocks structurally (making them impossible by construction). Layer 2 (timeouts) provides detection and recovery — bounding the cost when structural prevention alone is insufficient. Layer 3 (priority inheritance) is a liveness mechanism that prevents unbounded priority inversion from mimicking deadlock. The Wound-Wait scheme (§10) offers a path to **guaranteed progress** — eliminating the livelock risk that pure timeouts leave open. The system never hangs — it either completes the operation or reports a timeout error that the caller can handle.

-----

## 12. Guidance for Kernel Developers

When adding new kernel code that introduces locks or blocking operations:

1. **If you add a new per-CPU lock**, follow ascending CPU ID ordering (§3).
2. **If you add a new global lock**, document it in the lock hierarchy and ensure it does not create a cycle with existing locks.
3. **If you add blocking IPC**, always use `IpcCall` with a finite timeout. Never use `IpcRecv` with `timeout_ns: u64::MAX` in service code that holds resources.
4. **If you add a new allocator or cache**, consider a per-CPU magazine or lock-free design for the fast path (§6).
5. **If you add inter-service communication**, verify that the capability graph does not create a call cycle. If a cycle is architecturally necessary, ensure every call in the cycle has a timeout (§4).
6. **If your service handles IPC calls from higher-priority callers**, do not drop or ignore the inherited scheduling context. Complete the request promptly — the caller is blocked at your priority level (§5).
7. **Prefer synchronous `IpcCall`/`IpcReply`** for request-reply patterns. If asynchronous `IpcSend` is necessary, handle `EAGAIN` backpressure explicitly — never spin or block waiting for queue space (§8).
8. **Spinlock hold times must remain under 1 μs.** If your critical section might exceed this, restructure the code to do work outside the lock.
