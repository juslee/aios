---
author: claude
date: 2026-05-07
tags: [kernel, observability, smp, debugging]
status: final
---

# `drain_logs` runs only on CPU 0 — silence after a CPU 0 panic

## What happened

During M24 implementation, a fresh kernel-side service (the compositor) appeared
to "go silent" after its first `kinfo!` call. The compositor logged
`"Compositor: started, channel=5"` and then no further `kinfo!`, `kerror!`,
`println!`, or even raw `putc` output reached the UART, despite the thread
remaining alive and other threads continuing to run.

The mystery resolved when I added a deliberate `panic!()` on the compositor
thread. Even the panic message did not appear. I then:

1. Tested `main` (no compositor changes) — no panic surfaces, boot completes.
2. Tested with the compositor wired in — a separate thread (the IPC bench)
   panicked at `cap/mod.rs:86` on a torn `Thread.owner_pid` read.

The panic ran via `panic_handler` → `halt()` (`wfe` loop on the panicking core).
That core was CPU 0. Looking at `kernel/src/arch/aarch64/timer.rs` lines
176–187:

```rust
if cpu == 0 {
    let tick = TICK_COUNT.fetch_add(1, Ordering::Relaxed);
    if tick.is_multiple_of(4) {
        crate::observability::drain_logs();
    }
    ...
}
```

`drain_logs()` is called **only from CPU 0's timer tick**. When CPU 0 halted
from the unrelated bench panic, log drain stopped entirely. Any kinfo entries
pushed to per-core ring buffers after that point sat in the rings forever; the
synchronous UART writes from `panic_handler` and `println!` *did* happen, but
they raced with the existing deadlock and may not have flushed before
`halt()` parked the core in `wfe`.

## Why it matters

This made debugging look impossible: the symptoms pointed at the compositor
thread (logs stop after one entry, thread appears wedged), but the actual
cause was a totally separate thread on the same CPU.

## Fix / mitigation

For the immediate M24 work:

- Added bounds checks to all four `cap/mod.rs` `check_*` functions so torn
  reads return `Eperm` instead of panicking. The cap functions now match the
  shape of their already-correct `None`-slot fallthroughs.
- Added a divisor-zero guard in `virtio_input.rs::poll_*` that surfaced under
  the same conditions.

For the underlying observability gap (still open):

- `drain_logs` should run on every core's timer tick, not just CPU 0. The
  SPSC contract on `LogRing` is per-core (single producer, single consumer),
  so a "round-robin across all rings on every tick" would still be safe — the
  consumer is `drain_logs` itself, called from one core's timer at a time.
  Even simpler: each core drains its own ring on its own tick.

This redesign is out of scope for M24 (it would be Phase 25 perf work) but
should be tracked.

## How to apply this lesson

When kernel logging "goes silent" on a thread but other threads keep running,
**check whether CPU 0 has halted**. Symptoms include:

- Per-core ring entries from any thread on CPU 0 stop appearing in stdout
- Other CPUs continue logging (they push to their own rings; CPU 0's drain
  pulls from all rings, so when CPU 0 dies all per-core logging dies)
- Panic messages may appear partially (synchronous UART) but eventually stop

When this happens, the fix is to find the **other** thread that panicked on
CPU 0 — the visible "silent" thread is usually a victim, not the cause.
