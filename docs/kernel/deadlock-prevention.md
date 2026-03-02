# AIOS Deadlock Prevention Architecture

**Parent document:** [architecture.md](../project/architecture.md)
**Related:** [scheduler.md](./scheduler.md) — Scheduler deep dive, [ipc.md](./ipc.md) — IPC and syscall interface, [memory.md](./memory.md) — Memory management

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

AIOS breaks one or more of these conditions at every level of the system. The table below summarizes which condition each mechanism targets:

| Mechanism | Breaks | Location |
|---|---|---|
| Lock ordering (CPU ID) | Circular wait | Scheduler (§3) |
| Mandatory IPC timeouts | Hold and wait | IPC subsystem (§4) |
| Priority inheritance | Hold and wait (transitive) | IPC + Scheduler (§5) |
| Lock-free per-CPU magazines | Mutual exclusion | Memory allocator (§6) |
| Capability-based resource model | Hold and wait | Kernel-wide (§7) |
| Synchronous IPC (no callback chains) | Circular wait | IPC subsystem (§8) |
| Preemptive kernel | No preemption | Scheduler (§9) |

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

*Source: [scheduler.md §9.1](./scheduler.md)*

### 3.3 Why This Works

Lock ordering is a total order over all lockable resources. Any acyclic total order prevents circular wait. CPU IDs are natural, unique, and immutable — they require no runtime bookkeeping. The cost is zero: a single integer comparison before acquiring the second lock.

### 3.4 Scope

This ordering applies to all per-CPU resources: run queues, per-CPU timer lists, and per-CPU statistics. Any future per-CPU lock must follow the same ascending-ID convention.

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

*Source: [ipc.md §3.1, §4.2](./ipc.md)*

### 4.3 Complementary: IpcCancel

Agents can also explicitly cancel a pending call via `IpcCancel`, which returns `ECANCELED` to the blocked caller. The kernel uses this during process teardown to release all pending IPC state for a dying process — preventing zombie dependencies.

### 4.4 Why This Works

Mandatory timeouts break the **hold and wait** condition with a time bound. Even if a circular dependency forms, the cycle is broken within the shortest timeout in the chain. This converts a permanent deadlock into a transient timeout error.

### 4.5 Design Trade-off

Timeouts mean callers must handle `ETIMEDOUT`. This is intentional — AIOS treats unresponsive services as a fault to be handled, not a state to be tolerated. The SDK provides retry helpers with exponential backoff for the common case.

-----

## 5. Priority Inheritance Across IPC

### 5.1 The Problem

Priority inversion is a deadlock-adjacent hazard. When a high-priority Interactive thread calls a Normal-priority service, and that service is preempted by medium-priority work, the high-priority thread is effectively blocked by medium-priority work — indefinitely in the worst case.

### 5.2 The Solution: Scheduling Context Donation

When an `IpcCall` crosses scheduling classes, the kernel temporarily elevates the receiver to the caller's scheduling class:

```rust
unsafe fn ipc_direct_switch(sender: &mut Thread, receiver: &mut Thread, msg: &RawMessage) {
    // Priority inheritance: receiver inherits caller's scheduling context.
    // Saved and restored on IpcReply.
    receiver.sched.inherited_class = Some(sender.sched.class);
    receiver.sched.inherited_priority = Some(sender.sched.priority);
    receiver.sched.inherited_deadline = sender.sched.deadline;
}
```

On `IpcReply`, the receiver's original scheduling context is restored. This is transitive — if Service B calls Service C while holding A's inherited priority, C also inherits A's priority.

*Source: [ipc.md §9.2](./ipc.md), [scheduler.md §4.2](./scheduler.md)*

### 5.3 Async Tasks

The kernel's async executor applies the same principle. When a high-priority scheduler thread blocks waiting for an async task's result, the async task's priority is temporarily boosted:

```rust
impl KernelExecutor {
    pub fn boost_priority(&mut self, task_id: AsyncTaskId, waiter_priority: Priority) {
        if let Some(task) = self.tasks.get_mut(&task_id) {
            if waiter_priority > task.priority {
                task.priority = waiter_priority;
                // Re-sort in the ready queue to reflect new priority
            }
        }
    }
}
```

*Source: [scheduler.md §13.4](./scheduler.md)*

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
│ loaded  │ │ loaded  │ │ loaded  │
│ prev    │ │ prev    │ │ prev    │
└─────────┘ └─────────┘ └─────────┘
```

Each CPU maintains a small array of pre-allocated objects. Allocating takes an object from the local magazine — no locks, no atomic operations, just a decrement and a pointer load. Only when the magazine is empty does the CPU access the shared slab (which requires a lock).

*Source: [memory.md §4.1](./memory.md)*

### 6.3 Global Singletons

Kernel global allocators are each protected independently:

```rust
// Each is protected by a spin-lock or is inherently lock-free.
static FRAME_ALLOCATOR: FrameAllocator = /* ... */;
static FRAME_REFCOUNT: FrameRefCount = /* atomic counters, lock-free */;
static SLAB_ALLOCATOR: SlabAllocator = /* per-CPU magazines + locked slabs */;
static ZERO_QUEUE: PageZeroQueue = /* ... */;
```

*Source: [memory.md §4.2](./memory.md)*

### 6.4 Why This Works

Lock-free fast paths break the **mutual exclusion** condition for the common case. Since each CPU operates on its own magazine, there is no shared resource to contend over. The rare slow path (refilling an empty magazine from the shared slab) uses fine-grained locks with minimal hold times (< 1 us target).

-----

## 7. Capability-Based Resource Model

### 7.1 The Problem

Traditional OSes use ambient authority — any thread can attempt to open any file, connect to any service, or acquire any resource. This leads to complex locking because any thread might compete for any resource at any time.

### 7.2 The Solution: Unforgeable Capability Tokens

In AIOS, access to any resource requires a capability token. Channels are created with specific capabilities and cannot be used without them. This constrains the resource dependency graph:

- Agents can only communicate through pre-established channels
- Channels are created during service registration, not ad-hoc
- Capability transfer requires explicit kernel mediation

This means the set of possible resource dependencies is **statically known** at channel creation time, rather than being an arbitrary runtime graph. Circular dependencies between services can be detected at system design time.

*Source: [ipc.md §4.1](./ipc.md), [security.md](../security/security.md)*

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

*Source: [ipc.md §4.2](./ipc.md)*

### 8.3 Why This Works

Synchronous IPC creates a strict call chain. A thread that is blocked on an `IpcCall` holds no locks and makes no further calls — it simply waits. This breaks **circular wait** because a blocked thread cannot initiate the other half of a cycle.

-----

## 9. Fully Preemptive Kernel

### 9.1 The Problem

Non-preemptive kernels can deadlock when a thread holding a resource enters a long kernel code path and starves other threads waiting for that resource.

### 9.2 The Solution: Preempt Almost Everywhere

AIOS uses a fully preemptive kernel. User-space threads can be preempted at any instruction boundary. Kernel-mode code can be preempted at most points. Only four narrow regions disable preemption:

1. **Interrupt handler top halves** (< 10 us)
2. **Spinlock critical sections** (< 1 us target)
3. **Page table manipulation** (single-page atomic update)
4. **Context switch path** (inherently non-preemptible)

Everything else in the kernel is preemptible. A timer interrupt in preemptible kernel code immediately switches to a higher-priority thread.

*Source: [scheduler.md §10.3](./scheduler.md)*

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

**Priority inheritance (§5) approximates Wound-Wait.** When a high-priority thread calls a low-priority service, the service is "wounded" — its priority is forcibly elevated so it completes faster and releases the resource. This is the Wound-Wait intuition: the more important transaction preempts the less important one.

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

The existing layers (1–7) prevent deadlocks structurally. Where structural prevention is impractical (arbitrary inter-service call patterns), timeout-based detection provides a hard upper bound. The Wound-Wait scheme (§10) offers a path to **guaranteed progress** — eliminating the livelock risk that pure timeouts leave open. The system never hangs — it either completes the operation or reports a timeout error that the caller can handle.

-----

## 12. Guidance for Kernel Developers

When adding new kernel code that introduces locks or blocking operations:

1. **If you add a new per-CPU lock**, follow ascending CPU ID ordering (§3).
2. **If you add a new global lock**, document it in the lock hierarchy and ensure it does not create a cycle with existing locks.
3. **If you add blocking IPC**, always use `IpcCall` with a finite timeout. Never use `IpcRecv` with `timeout_ns: u64::MAX` in service code that holds resources.
4. **If you add a new allocator or cache**, consider a per-CPU magazine or lock-free design for the fast path (§6).
5. **If you add inter-service communication**, verify that the capability graph does not create a call cycle. If a cycle is architecturally necessary, ensure every call in the cycle has a timeout (§4).
6. **Spinlock hold times must remain under 1 us.** If your critical section might exceed this, restructure the code to do work outside the lock.
