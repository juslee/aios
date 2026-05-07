---
author: claude
date: 2026-05-07
tags: [compositor, kernel, mmu, memory, debugging]
status: in-progress
---

# Step 28 keystone deferred: pre-existing memory corruption blocks visible output

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
