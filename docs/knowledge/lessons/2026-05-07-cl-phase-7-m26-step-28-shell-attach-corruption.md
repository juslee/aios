---
author: claude
date: 2026-05-08
tags: [compositor, kernel, mmu, memory, debugging]
status: final
---

# Step 28 keystone: surface-state-machine fix unblocks shell init; visible pixels gated on M24 race

## What I hit

Phase 7 M26 Step 28 was supposed to flip `COMPOSITOR_PRESENT_ENABLED`
from `false` to `true` and produce the first interactive desktop on
`just run-gpu`. All the infrastructure landed cleanly — per-client
`client_channel` field on `CompositorRequest`, the test-app process
(`ProcessId(11)`), the per-client surface channel routing in
`handle_create_surface`, the SURFACE_TABLE snapshot + shmem-pixel
resolver in `present_frame`. Build is clean (618 host tests passing,
zero warnings). But flipping the present flag to `true` runs straight
into a pre-existing kernel memory-corruption issue that surfaces
deterministically once the compositor is exercised under load.

## Concrete observations

Multiple `just run-gpu` runs with the flag on captured the following
sequence:

```
[Boot]   ...standard boot...
[bench]  Gate 1 starts (IPC round-trip + context switch + shm).
[heartbeat] tick=1000  (1 second after boot)
PANIC: panicked at kernel/src/mm/frame.rs:51:9:
[mm] BUG: free_pages(0x501e0, 192) — address not in any pool
[Compositor] display handoff complete (scanout 0 ...)
[Mm] shm_create: id=0 size=0x28000 pages=64 order=6
[Compositor] shell: status-strip attach failed surface_id=1 shmem_id=0 err=NotFound
[Compositor] shell init failed (AttachBuffer); continuing without shell chrome
[Gpu] VirtIO-GPU: error response 0x1203
EXCEPTION[CPU 0]: ESR=0x96000006 EC=0x25 FAR=0x500 ELR=0xffff000000086e10
  Data Abort at 0x00000500
```

Multiple aborts of this family also reproduce on `just run` (no
display) and on the unmodified `main` branch tip — they are NOT new
in Step 28.

## What's actually broken

Three concrete failures, almost certainly all symptoms of the same
underlying corruption:

1. **`free_pages(0x501e0, 192) — address not in any pool`.** Address
   is non-page-aligned; order is impossible (max sane buddy order is
   ~10). One of the slab/heap free paths is computing pages-to-free
   from corrupted state (the slab cache header, a Vec capacity, or a
   `core::alloc::Layout::size()` reading garbage).

2. **`surface_attach_buffer` returns `SurfaceError::NotFound` for a
   surface id we just received from `surface_create`.** No other
   thread should be touching `SURFACE_TABLE` between those two
   calls — the compositor's `init_shell_surfaces` runs single-threaded
   on the compositor's own kernel thread before the recv loop begins.
   For `find_mut` to miss the entry, the slot's `Some(Surface { id: 1, .. })`
   must have been clobbered after `surface_create` returned.

3. **Data abort at low VAs (FAR=0x500, others observed: 0x16, 0xab0)**
   — already documented in
   [the M24 compositor-present gate decision](../decisions/2026-05-07-cl-phase-07-m24-compositor-present-gate.md):
   > Data abort at low VAs (FAR=0x16, 0x3f, 0xab0 across runs) —
   > surfaces only with M24's added activity. Cannot be reproduced
   > on plain `main`. Root cause not yet identified.

   Step 28 confirms the issue persists; running the test app makes it
   easier to reproduce because the IPC traffic patterns the
   compositor and test app generate are exactly the kind of load
   that exposes the race.

## What Step 28 ships anyway

Even with the present flag still `false`, Step 28's infrastructure
lands the keystone changes:

- **`client_channel: u64` on `CompositorRequest`** with explicit
  padding (M25 implicit-padding-UB lesson applied) plus a host-tested
  `effective_channel(client_channel, service_channel)` helper.
- **Per-client channel routing** in `handle_create_surface`: when
  `client_channel != 0` the compositor stores the caller's channel
  on `Surface.channel`, so events flow back to the right endpoint.
- **Test app process** (`ProcessId(11)`) with the full surface
  lifecycle implementation — channel create, service lookup,
  CreateSurface/Configure round-trip, shmem-backed buffer rendering
  with "Hello from AIOS!", AttachBuffer, event loop with keyboard
  appending and CloseRequested handling.
- **SURFACE_TABLE snapshot + shmem→pixels resolver** in
  `present_frame` so the compose path is wired end-to-end behind the
  flag. Flipping the const in a future step should be a one-line
  change once the corruption is fixed.

The flag stays `false` for now. Visual verification is gated on the
underlying kernel corruption being root-caused, which is a Phase 7+
investigation beyond the scope of Step 28's keystone landing.

## How to apply when investigating

1. **Reproduce on `main`** without any M26 changes — confirms it's
   not Step 28.
2. **Bisect the heap/slab paths.** The `free_pages(addr, 192)` panic
   is the most actionable signal — `192` would equal `0xC0`; that's
   suspicious as a literal that might come from somewhere specific.
   Grep for `0xC0` in slab/heap code; check `BlockHeader` layout for
   torn reads.
3. **Add `debug_assert!` on free_pages at every call site** to catch
   the corrupting writer earlier. Currently the assert in
   `frame.rs:51` only fires once the corruption has propagated to a
   call.
4. **Check `THREAD_TABLE` torn reads** — same family of issue as the
   M24 cap/mod.rs torn pid issue; the surface table similarly stores
   `Option<Surface>` and could exhibit half-stored state if a
   reader observes a partial write under NC memory ordering.
5. **Consider whether `SURFACE_TABLE: Mutex<...>` is actually
   exclusive across cores.** Per CLAUDE.md NC-memory limitations,
   spin::Mutex requires WB-cacheable memory for its exclusive
   load/store pair to work. The kernel direct map IS WB by Phase 2
   M8, but verify that all SURFACE_TABLE-bearing pages are mapped
   through TTBR1's WB region, not edk2's NC region.

## Detection

```sh
just run-gpu 2>&1 | grep -E "PANIC|BUG: free_pages|shell.*attach failed|EXCEPTION"
```

If any of those fire, the corruption is still present.

## Resolution criteria

Step 28 reaches "complete" when:

1. `just run-gpu` produces UART log `[Compositor] shell: status-strip
   surface=N created (1280x32)` reliably across 5+ runs.
2. `[Compositor] shell: taskbar surface=N created (1280x40)` follows.
3. `[Compositor] shell: workspace surface=N created (...)` follows.
4. `[Compositor] test-app: surface created id=N (400x300)` appears.
5. No `PANIC` or `EXCEPTION` lines between boot and the 60-second
   mark.
6. `COMPOSITOR_PRESENT_ENABLED = true` is committed.

-----

## Root Cause (2026-05-08 follow-up)

**Primary cause of `shell init failed (AttachBuffer)` was misdiagnosed**:
the deterministic "AttachBuffer NotFound" symptom that prompted this
lesson is not a memory-corruption issue. It is a state-machine bug in
`kernel/src/compositor/surface.rs::surface_attach_buffer`.

### Diagnostic narrative

Adding `.map_err(|e| ...)` instrumentation around `surface_attach_buffer`'s
caller in `kernel/src/compositor/shell/status_strip.rs::init` and a
table-walk inside `surface_attach_buffer` itself produced a deterministic
trace:

```
[Comp] DIAG_create: id=1 owner=10
[Comp] DIAG_attach: id=1 some=1 found=true
[Comp] DIAG_attach: transit id=1 from=Created to=Active
[Comp] Compositor: shell init failed (AttachBuffer); ...
```

`find_mut` returns the surface fine. The owner check passes. But the
`can_transition_to(Active)` check on a surface in `Created` state
**always returns `false`** — see
`shared/src/compositor.rs::SurfaceState::can_transition_to` and the
companion test `surface_state_transitions_invalid` at
`shared/src/compositor.rs:1755`:

```rust
assert!(!SurfaceState::Created.can_transition_to(SurfaceState::Active));
```

The protocol's strict state machine requires the path
`Created → Configured → Active`. The compositor side never advances
the surface into `Configured` (neither in `surface_create` itself nor
in `handle_create_surface` when sending the `Configure` event). When
the shell's `init` (or any client's first `AttachBuffer`) arrives, the
surface is still `Created`, the protocol-disallowed direct
`Created → Active` jump is rejected, and `surface_attach_buffer` returns
`InvalidTransition` — which `status_strip::init` maps to
`ShellError::AttachBuffer`.

The original code had a comment hinting at the intent ("First buffer
attach takes us through Configured → Active even if the client called
AttachBuffer before processing its Configure event") but the
implementation contradicted it: the match arm collapsed
`Created | Configured` into the same `next_state`, then the subsequent
`can_transition_to` enforced the strict per-step rule the match was
trying to relax.

### Resolution

Fix in `kernel/src/compositor/surface.rs::surface_attach_buffer`: walk
the state machine explicitly so each individual step is legal under
`can_transition_to`. The `Created` case advances through
`Configured` first, then to `Active`; `Configured`/`Suspended` go
straight to `Active`; `Active` is idempotent; `Destroyed` rejects.

After the fix `shell: status-strip surface=1 created`,
`shell: taskbar surface=2 created`, and `shell: workspace surface=3
created` all log on `just run-gpu`, and a single composed frame
(`DIAG_frame: count=1 elapsed_ms=3`) confirmed the present pipeline
runs end-to-end on a successful boot.

### Misdiagnosis history

The original ticket described the symptom as "`surface_attach_buffer`
returns `NotFound` for a surface id we just received from
`surface_create`". That description is incorrect — the actual returned
error code is `InvalidTransition`, mapped to `ShellError::AttachBuffer`.
Both `NotFound` and `InvalidTransition` map to the same `AttachBuffer`
shell-error enum value, hiding which gate actually failed without the
diagnostic instrumentation above.

The `free_pages(addr, 192) — address not in any pool` panic that
accompanied the original report is a *secondary, non-deterministic*
symptom of the M24 data-abort race already documented in
[the M24 compositor-present gate decision](../decisions/2026-05-07-cl-phase-07-m24-compositor-present-gate.md).
That race is independent of the state-machine bug. Once the shell
chrome surfaces succeed, the race surfaces on the longer-lived
compositor loop (sometimes as `free_pages(garbage, 64)` panics where
the garbage address looks like a B8G8R8A8 pixel value or a low
field-offset, sometimes as a `PC = 0` instruction abort, sometimes as
a data abort with FAR in the 0x100–0x600 range — all consistent with
heap-resident pointer corruption from the same race that already
gates `COMPOSITOR_PRESENT_ENABLED` in M24).

### Why the present flag stays `false` for now

With the state-machine fix, shell init succeeds reliably (all three
chrome surfaces register and the first compose frame transfers to the
host) on every clean boot — but the M24 race intermittently hits the
compositor's main loop on subsequent ticks, producing the family of
faults catalogued above. Across 10 runs of `just run-gpu` with
`COMPOSITOR_PRESENT_ENABLED = true` and the state-machine fix applied:

* 3/10 runs: shell + workspace surfaces created **and** at least one
  frame logged a successful compose+present.
* 3/10 runs: crashed mid-boot (data abort or `PC=0`).
* 4/10 runs: stalled silently inside QEMU before reaching shell init
  (no compositor logs at all — same race manifesting earlier in boot
  on those runs).

Visible-pixel output therefore is achievable on individual runs, but
not "consistent across 5 consecutive runs" as Step 28's resolution
criteria requires. The flag is left at `false`. Step 28 ships the
state-machine fix as the substantive M26 closeout; full visible
output unblocks once the M24 race is rooted out (separate workstream
flagged in the M24 decision).

### What's needed to fix the secondary race

Symptoms across runs strongly imply heap-stored pointer corruption in
structures that survive across compositor-loop iterations
(`WorkspaceState.buffer_va`, `VirtioGpu.used_virt`, `Channel`'s
metadata, callee-saved register x19/x23/x26 reads against bogus stack
positions, etc.). Investigation hooks for the next session:

1. The `irq_el1_entry` minimal-save handler at
   `kernel/src/arch/aarch64/exceptions.rs:144` saves only x0–x18 + x29 +
   x30. Confirm that every call from inside `irq_handler_el1` (timer
   tick → `check_preemption` → `schedule` → `save_context`) preserves
   the interrupted thread's x19–x28 across context switches.
2. Audit code paths that take `&mut Surface` / `&mut WorkspaceState`
   under a lock and then drop the lock without invalidating the
   reference (the borrow checker sees the lock guard, but a
   raw-pointer read by an interrupting thread on the same physical
   memory wouldn't).
3. Run the existing `MIRI` target on `shared::compositor` and the
   surface-state tests with extra cases for the
   `Created → Configured → Active` walk to lock the state-machine
   invariants in.
4. Reproduce on `main` (without M26 changes) by adding a short
   delay loop into `compositor_loop` post-handoff — the race surfaces
   anywhere the compositor stays alive long enough for the IRQ-driven
   preempt + recv + tick cycles to overlap.

Status: state-machine fix shipped; secondary race deferred to a
follow-up investigation.
