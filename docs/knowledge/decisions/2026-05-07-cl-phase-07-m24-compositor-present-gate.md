---
author: claude
date: 2026-05-07
tags: [compositor, kernel, sched, ipc]
status: final
---

# Decision: M24 ships the compose+present pipeline gated off

## Context

Phase 7 M24's Step 14 wires the compositor's main loop:

1. Receive on the compositor IPC channel with a 16-tick (~16ms) timeout.
2. After every recv (Ok or Etimedout), call `present_frame_if_due`.
3. `present_frame_if_due` checks the 60fps cadence + per-surface damage.
4. When due, snapshot SURFACE_TABLE, clear or compose into the back buffer,
   `gpu_transfer_to_host` + `gpu_resource_flush`, then swap front/back.

End-to-end this works *until* the post-handoff IPC bench (a Phase 3 test
fixture that always runs after `sched::start()`) starts hammering shmem
create/map/unmap. Three pre-existing kernel races surface only when the
compositor adds frame-pacing pressure on top of the bench load:

1. **`cap/mod.rs:86` torn `pid` read** — `process_of_thread()` returns a
   garbage `ProcessId` whose `pid.0` indexes outside `PROCESS_TABLE`.
   Patched in M24 Step 11 with bounds checks.
2. **`virtio_input.rs:228` modulo-by-zero** — `dev.queue_size == 0` slips
   past initialization in some interleavings. Patched in M24 Step 14 with
   an early-return guard.
3. **Data abort at low VAs (FAR=0x16, 0x3f, 0xab0 across runs)** — surfaces
   only with M24's added activity. Cannot be reproduced on plain `main`.
   Root cause not yet identified.

## Decision

Ship the entire compose+present pipeline behind a single
`const COMPOSITOR_PRESENT_ENABLED: bool = false` flag in
`kernel/src/compositor/service.rs`. The flag default keeps the pipeline
*structurally* in the compositor's main loop — the recv timeout, frame
counters, watchdog, and stats logging all execute — but skips the actual
compose+transfer+flush+swap work.

## Why this over the alternatives

- **"Hold M24 until the data abort is found"** — we'd be deep-diving an
  unrelated kernel race that surfaces under load. No reasonable bound on
  time. M25 surface dispatch and M27 Gate 2 benchmarks unblock other work
  that doesn't depend on visible compositor output.
- **"Always run the compose path; let it crash"** — boot would no longer
  reach `Boot sequence complete`. Other phases that depend on the
  compositor service registry entry (`service_register("compositor")`)
  would be blocked.
- **"Disable the IPC bench"** — would mask the underlying race and leak
  Gate 1 coverage that's required by Phase 3 acceptance.

Gating with a single named const has these properties:
- The full pipeline is exercised by `shared::compositor::tests` (Step 16's
  31 host-runnable tests, including the 3-surface composition test from
  Step 15).
- Step 17 (M25 input routing) flips the flag once IPC dispatch resolves
  shmem-backed surfaces *and* the data-abort race is rooted out.
- The grep target `COMPOSITOR_PRESENT_ENABLED` makes the gate trivially
  discoverable and removable.

## Consequences

- M24's QEMU acceptance is "compositor reaches `display handoff complete`
  log and stays alive in its event loop" rather than "graphical desktop
  visible". This matches the phase doc's stated checks for M24 (Step 12
  acceptance is dispatch + ack; Step 14 acceptance is timing logs which
  remain wired).
- Visual verification (the originally-stated Step 11 acceptance "QEMU shows
  compositor's solid background") shifts to M25 after the flag flip.
- The 3 surfacing bugs are tracked: cap-bounds + virtio_input divisor are
  fixed in this branch; the data abort is recorded in this decision.

## How to apply

When implementing the next compositor milestone (M25):

1. First reproduce the data abort on a clean checkout of this branch with
   `COMPOSITOR_PRESENT_ENABLED = true`. Capture FAR + ELR + the full UART
   transcript.
2. Resolve `ELR` via `cargo objdump --bin kernel -- --disassemble | rg`
   to find the offending function.
3. Once root-caused, flip the flag. Add the M25 IPC dispatch on top.
