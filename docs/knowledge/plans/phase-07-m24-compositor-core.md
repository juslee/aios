---
author: claude
date: 2026-05-07
tags: [compositor, gpu, ipc, sched, drivers]
status: in-progress
phase: 7
milestone: M24
---

# Plan: Phase 7 M24 — Compositor Core

## Approach

M24 stands up the AIOS compositor as a **system service** following the established GPU Service pattern (`kernel/src/gpu/service.rs`). The compositor takes ownership of the display from the GPU Service mid-boot and becomes the single producer of display frames. M23 already wired the input subsystem and `EarlyBootPhase::CompositorReady = 19`; M24 fills in the actual compositor.

**Architecture decisions baked in (per phase doc + ADRs):**

- Compositor is a **system service**, not a Kit. Apps reach pixels through Compute Kit (`GpuSurface`) and through the IPC protocol defined here. (Custom-Core principle, ADR 2026-03-22.)
- Layer 1 only: floating windows, no AIRS, no smart layout. (Three-Interaction-Layers ADR.)
- Compositor → GPU: **direct VirtIO-GPU driver access** (same trust level, no IPC round-trip). Phase doc §Decision Points.
- Scene model: **flat z-ordered surface list** (not the full SceneNode tree from rendering.md §5.1). Adequate for ≤32 surfaces.
- Pixel format: **`Xrgb8888` opaque** for shell + test surfaces in M24. Premultiplied alpha math is wired but exercised by M25 decorations / cursor.
- Capabilities: **flat enum variants** (consistent with existing `GpuMmioAccess`, `DisplayControl`); rich `DisplayCapability` deferred to Phase 18.

**Key gaps found during exploration:**

- `shared/src/compositor.rs` does not exist — entire shared protocol must be created.
- `Subsystem::COUNT` is currently 15 with `Input = 14` as the last variant. Adding `Compositor = 15` brings COUNT to 16; the `name()` match, `subsystem_count` test, `subsystem_repr_values` test, and `subsystem_names_are_5_chars` test all enumerate every variant — every one must be updated.
- Capabilities `CompositorCreateSurface`, `CompositorFullscreen`, `CompositorOverlay`, `CompositorInputAccess` must be added to `Capability` enum; `permits()` and `can_attenuate_to()` need new arms; cap unit tests exhaustively enumerate.
- `gpu_release_test_frame()` already exists; `swap_buffers()` already exists. There is no equivalent "release double buffers" function on the GPU Service — display handoff has to allocate the compositor's buffers, swap scanout, then let GPU Service tear down its old buffers via a graceful exit signal (`COMPOSITOR_ACTIVE`).
- `EarlyBootPhase::CompositorReady = 19` was added in M23 but never advanced to. M24 wires the `advance_boot_phase(CompositorReady)` call after `init_compositor()`.

**Shared crate refactoring (end of milestone):**

Step 16 of the phase doc IS the shared-crate step. Pure data types live in `shared/src/compositor.rs`:

- `SurfaceId(u64)`, `SurfaceState`, `SurfaceLayer`, `SurfaceTitle`, `SurfaceContentType`, `DamageRegion`, `CompositorRequest`, `CompositorEvent` — pure repr(C) data, no hardware deps. All host-testable.
- `SurfaceTable` (`[Option<Surface>; 32]` with monotonic id allocator) is a candidate for shared if/when the surface-row layout stabilizes. M24 keeps `Surface` itself in `kernel/src/compositor/surface.rs` because it embeds `ChannelId` and `ProcessId` and references kernel-side resources; we test the pure pieces from the shared crate.
- `DamageTracker` data structure (Step 13) is also pure — define in `shared/src/compositor.rs` so it is host-testable.

**M23 doc hygiene piggyback:** Before starting M24 implementation, fix the `[ ]` checkboxes in `docs/phases/07-window-compositor-and-shell.md` for M23's tasks (Steps 1–7). PR #144 merged the code but never updated the checkbox state. Roll this into the M24 PR as a separate commit (`Phase 7 M23: phase doc checkbox cleanup`) since splitting it would just be churn.

## Progress

- [ ] Step 0 (M24 prep): M23 phase doc cleanup
  - [ ] 0a: tick `[ ]` → `[x]` for all M23 tasks (Steps 1–7) in `docs/phases/07-window-compositor-and-shell.md`
  - [ ] 0b: leave Status field as "Planned" (still mid-phase) but note progress in CLAUDE.md memory
  - [ ] 0c: Verify: `git diff` shows only checkbox flips
- [ ] Step 8: Compositor shared types
  - [ ] 8a: create `shared/src/compositor.rs`, add `pub mod compositor;` to `shared/src/lib.rs`
  - [ ] 8b: define `SurfaceId(pub u64)` (`Debug, Clone, Copy, PartialEq, Eq, Hash`)
  - [ ] 8c: define `SurfaceState` enum (`Created`, `Configured`, `Active`, `Suspended`, `Destroyed`) with `is_terminal()` + `can_transition_to()` helpers
  - [ ] 8d: define `SurfaceLayer` enum (repr(u8) for ordering: `Background = 0`, `Normal`, `TopLevel`, `Overlay`, `Panel`)
  - [ ] 8e: define `SurfaceTitle { bytes: [u8; 64], len: u8 }` with `from_bytes()` truncating UTF-8 char-boundary-safely
  - [ ] 8f: define `SurfaceContentType` enum: `Document`, `Terminal`, `Browser`, `Game`, `Settings`, `SystemUI`, `Generic`
  - [ ] 8g: define `DamageRegion` enum: `Rect { x, y, width, height }`, `FullSurface`, `Empty`
  - [ ] 8h: define `CompositorCommand` enum (u32 wire IDs) + `CompositorRequest` repr(C) ≤256 B with discriminant + payload union
  - [ ] 8i: define `CompositorEvent` repr(C) ≤256 B (Configure, FocusChanged, CloseRequested, BufferReleased, FramePresented, Input)
  - [ ] 8j: add compile-time `const _: () = assert!(size_of::<CompositorRequest>() <= MAX_MESSAGE_SIZE)` for both
  - [ ] 8k: extend `Subsystem` to add `Compositor = 15`; bump `COUNT` to 16; update `name()` to "Comp "; update both `subsystem_count`, `subsystem_repr_values`, `subsystem_names_are_5_chars`, `subsystem_name_content` tests
  - [ ] 8l: Verify: `cargo test -p shared` passes; `just check` zero warnings
- [ ] Step 9: Compositor capability types
  - [ ] 9a: add `CompositorCreateSurface`, `CompositorFullscreen`, `CompositorOverlay`, `CompositorInputAccess` variants to `Capability` enum in `shared/src/cap.rs`
  - [ ] 9b: extend `permits()` with identity matches for each
  - [ ] 9c: extend `can_attenuate_to()` (defaults to `permits()` via `_ => self.permits(other)`, but explicitly note no attenuation chain in M24)
  - [ ] 9d: extend cap unit tests (`permits_*`, `can_attenuate_to_*` if exhaustive)
  - [ ] 9e: Verify: `cargo test -p shared` + `just check`
- [ ] Step 10: Compositor service process
  - [ ] 10a: create `kernel/src/compositor/mod.rs` with `pub mod service;` `pub mod surface;` `pub mod render;`
  - [ ] 10b: add `pub mod compositor;` to `kernel/src/main.rs`
  - [ ] 10c: implement `init_compositor()` in `service.rs` — mirror `gpu::service::init_gpu_service()`: PROCESS_TABLE slot 10 (name="compositor"), grant caps (`CompositorCreateSurface`, `GpuMmioAccess`, `ChannelCreate`, `DebugPrint`, `ChannelAccess(ch)`), `channel_create_unchecked(ThreadId(0xA10))`, `service_register(b"compositor", ...)`, spawn thread `compositor_entry` (Interactive, CpuSet::all())
  - [ ] 10d: implement `compositor_entry()`: unmask IRQs (`DAIFClr #0x2`), call `compositor_loop()` (stub for now — real loop in Step 14)
  - [ ] 10e: define `static COMPOSITOR_CHANNEL: Mutex<Option<ChannelId>> = Mutex::new(None);`
  - [ ] 10f: add `compositor::service::init_compositor()` call in `kernel/src/main.rs` after `input::init()` (gated on `display_info().is_some()`)
  - [ ] 10g: document lock ordering at SURFACE_TABLE / DAMAGE / FRAME_PACER declaration sites with a unified comment block
  - [ ] 10h: Verify: `just run-input` boots; UART shows `[Compositor] started, channel=N`
- [ ] Step 11: Display handoff from GPU Service
  - [ ] 11a: define `pub static COMPOSITOR_ACTIVE: AtomicBool = AtomicBool::new(false);` in `kernel/src/compositor/mod.rs`
  - [ ] 11b: implement `display_handoff(state)` in `compositor/service.rs`: get `display_info()`, allocate two DMA framebuffers via `virtio_gpu::gpu_allocate_framebuffer(w, h)`, fill front with AIOS blue (`AIOS_BLUE_B8G8R8A8`), `gpu_set_scanout(0, front, &full_rect)`, `gpu_transfer_to_host`, `gpu_resource_flush`, set `COMPOSITOR_ACTIVE.store(true, Release)`
  - [ ] 11c: store handles in compositor service state (CompositorState struct: `front_buffer`, `back_buffer`, `display`)
  - [ ] 11d: GPU Service must check `COMPOSITOR_ACTIVE` before continuing display work — simplest: GPU Service still allocates its boot buffers as before (renders boot log), but the moment the compositor's `init_double_buffering()`-equivalent runs and calls `set_scanout` to a fresh resource, the scanout is owned by the compositor. GPU Service buffers are no longer scanned out but remain allocated for later release. Verify no flicker/black frame.
  - [ ] 11e: log handoff completion: `[Compositor] display handoff: scanout 0 = compositor front buffer (resource={})`
  - [ ] 11f: Verify: `just run-input` shows AIOS-blue desktop after compositor handoff (no flicker)
- [ ] Step 12: Surface lifecycle management
  - [ ] 12a: create `kernel/src/compositor/surface.rs`
  - [ ] 12b: define `Surface { id, state, layer, title, content_type, x, y, width, height, shmem_id, owner_pid, channel, damaged }`
  - [ ] 12c: define `MAX_SURFACES: usize = 32`; `static SURFACE_TABLE: Mutex<[Option<Surface>; MAX_SURFACES]>` with lock-ordering comment
  - [ ] 12d: define `static NEXT_SURFACE_ID: AtomicU64 = AtomicU64::new(1);`
  - [ ] 12e: implement `surface_create(owner_pid, channel, width, height, title, content_type, layer) -> Result<SurfaceId, CompositorError>`: capability check, allocate slot, allocate ID, insert state=Created, return ID. Caller (the IPC dispatcher) sends the `Configure` event back via `ipc_reply` or as a follow-up event.
  - [ ] 12f: implement `surface_attach_buffer(id, shmem_id, damage, owner_pid)`: validate ownership, set `shmem_id`, mark `damaged=true`, transition `Created/Configured → Active`
  - [ ] 12g: implement `surface_destroy(id, owner_pid)`: set state=Destroyed, clear slot
  - [ ] 12h: implement `surface_resize(id, width, height, owner_pid)`: update size, mark damaged, return new dimensions for caller to send Configure
  - [ ] 12i: implement `surface_set_layer(id, layer, owner_pid)`: update layer, mark damaged
  - [ ] 12j: state machine validation helper — only allow valid transitions (Created → Configured → Active, Active ↔ Suspended, * → Destroyed)
  - [ ] 12k: Verify: `just check` + `just test`
- [ ] Step 13: Software compositor — flat z-order blitting
  - [ ] 13a: create `kernel/src/compositor/render.rs`
  - [ ] 13b: implement `compose_frame(surfaces: &[&Surface], comp_buffer: &mut [u32], stride_px: u32, width: u32, height: u32)`: clear damaged background regions to AIOS blue (or full-clear on first frame), iterate surfaces sorted by `(layer, insertion_order)`, blit each into composition buffer
  - [ ] 13c: implement `blit_opaque(src: &[u32], src_w, src_h, dst: &mut [u32], dst_x, dst_y, dst_stride_px, dst_w, dst_h)`: clip to dst bounds, copy row-by-row (`copy_from_slice`)
  - [ ] 13d: implement `blit_alpha_premultiplied(...)`: per-pixel blend using premultiplied alpha formula `out = src + dst * (1 - src_alpha)` for ARGB8888 pixels (8-bit channels, integer math)
  - [ ] 13e: define `DamageTracker` in `shared/src/compositor.rs`: per-surface dirty flag (the `damaged: bool` on `Surface`) + screen-space accumulator `[Option<DamageRect>; 16]` with `union(rect)` and `clear()`
  - [ ] 13f: pixel format note: composition buffer is host-side `Bgra` (matches VirtIO-GPU `B8G8R8A8Unorm`); store as `u32` little-endian. AIOS_BLUE constant already encoded correctly in `shared::gpu`.
  - [ ] 13g: Verify: unit test `blit_opaque_centered`, `blit_opaque_clipped_left_edge`, `blit_alpha_blend_50pct` in `shared/src/compositor.rs`
- [ ] Step 14: Composition loop and frame pacing
  - [ ] 14a: implement `compositor_loop()` in `service.rs`: receive on COMPOSITOR_CHANNEL, dispatch to surface_create/attach/destroy/resize/set_layer, after each batch of IPC messages run a frame
  - [ ] 14b: frame pacing: track `last_frame_tick: u64`; if `TICK_COUNT - last_frame_tick < 16` (1 kHz tick = 1ms), defer composition; otherwise compose
  - [ ] 14c: composition step: take SURFACE_TABLE snapshot, sort by z-order, call `compose_frame`, then `gpu_transfer_to_host` + `gpu_resource_flush` on the back buffer, then swap (rebind scanout to back, swap front/back roles)
  - [ ] 14d: skip composition if no surface has `damaged=true` AND no full-redraw is pending
  - [ ] 14e: log frame stats every 60 frames: `[Compositor] avg frame=Nms, surfaces=N, damage=Nms`
  - [ ] 14f: watchdog: if a single frame composition exceeds 100ms, log warning `[Compositor] WARN: frame took {ms}ms (>100ms threshold)`
  - [ ] 14g: ensure compositor IPC dispatch path uses `ipc_recv` with a short timeout so the loop can run frames between messages (similar to GPU Service's `DEFAULT_TIMEOUT_TICKS`, but shorter — try `16` ticks = 16ms = one frame)
  - [ ] 14h: Verify: `just run-input` shows compositor frame stats; static screen → 0ms composition periodically logged
- [ ] Step 15: Multi-surface composition test
  - [ ] 15a: implement `compositor_test()` in `kernel/src/compositor/mod.rs` (kernel-side; not yet a separate process — that comes in M26)
  - [ ] 15b: directly insert 3 test Surface entries into SURFACE_TABLE bypassing IPC: background (Layer::Background, full-screen, dark gray 0xFF202020), window (Layer::Normal, 400×300 at (100,100), AIOS blue), overlay (Layer::Overlay, 200×50 at (200,400), yellow 0xFFFFD500)
  - [ ] 15c: allocate small backing kernel-side scratch buffers (since real shmem_id requires a process — for the in-kernel test we use raw `[u32; W*H]` slabs and pass them directly to `compose_frame`)
  - [ ] 15d: alternative: keep test purely as a `compose_frame` unit test in `render.rs` that builds the scene in stack slabs — this is simpler and host-testable. Use the kernel boot for visual verification only.
  - [ ] 15e: log `[Compositor] test: 3 surfaces composited, layers verified`
  - [ ] 15f: Verify: `just run-input` shows 3 colored rects at correct z-order on QEMU display
- [ ] Step 16: Shared crate compositor types and unit tests
  - [ ] 16a: add `#[cfg(test)] mod tests` to `shared/src/compositor.rs`
  - [ ] 16b: 15+ tests covering: `CompositorRequest` size ≤256, `CompositorEvent` size ≤256, SurfaceState transitions valid/invalid, SurfaceLayer ordering, SurfaceTitle truncation at 64 bytes, SurfaceTitle UTF-8 char-boundary safety, DamageRegion::Rect coordinates, SurfaceId uniqueness via NEXT_SURFACE_ID-style counter test, repr(C) compile-time asserts visible
  - [ ] 16c: Verify: `cargo test -p shared` reports 15+ new compositor tests
- [ ] Step 17: Doc updates + audit loop
  - [ ] 17a: CLAUDE.md — Workspace Layout (new `kernel/src/compositor/*` files, `shared/src/compositor.rs`); Key Technical Facts (Compositor ProcessId=10, MAX_SURFACES=32, CompositorReady boot phase, lock ordering update, Subsystem::COUNT=16)
  - [ ] 17b: README.md — Project Structure and Phase 7 status
  - [ ] 17c: docs/project/developer-guide.md — new modules, file sizes, test counts (target: 500+ tests)
  - [ ] 17d: docs/phases/07-window-compositor-and-shell.md — check off all M24 tasks (Steps 8–16)
  - [ ] 17e: dead code cleanup: `grep -r "#\[allow(dead_code)\]" kernel/src/ shared/src/` — remove any that are now used or are genuinely dead in M24 scope
  - [ ] 17f: run `/audit-loop` until 0 issues

## Code Structure Decisions

- **Compositor uses direct VirtIO-GPU driver calls, not IPC to GPU Service**: phase doc §Decision Points; both run as ProcessId(9)/(10) with same trust level; IPC overhead per frame would be ~4μs × 60Hz = noticeable. GPU Service still serves *third-party* GPU clients (test apps, etc.); compositor cuts the line for its own composition path only.
- **Flat surface array `[Option<Surface>; 32]`, not `Vec<Surface>`**: avoids OOM panic risk on heap pressure; matches the existing GPU Service `[Option<GpuBufferHandle>; 8]` and IPC `[Option<Channel>; 128]` pattern; 32 is generous for a Layer 1 desktop (typical user has ≤10 windows).
- **DAMAGE accumulator lives in compositor state, not per-surface globally**: the `damaged: bool` on `Surface` is a "needs recompositing" flag; the cross-frame screen-space damage union is owned by the composition loop in `compositor_loop`. This avoids adding another global `Mutex<DamageTracker>` to the lock graph.
- **Composition buffer pixel format `B8G8R8A8`** (matches VirtIO-GPU): we treat it as `u32` little-endian; the `AIOS_BLUE_B8G8R8A8` constant in `shared::gpu` already encodes correctly. No format conversion on the hot path.
- **Subsystem name "Comp "** (5 chars with trailing space): matches the 5-character padding convention enforced by the `subsystem_names_are_5_chars` test. Cannot use "Composit" (8) or "Cmp" (3 ≠ 5).
- **`Surface` keeps kernel-only fields (`channel`, `owner_pid`, `shmem_id`)**: it ties together IPC + capability + storage references that don't make sense in the host test environment. Pure data sub-structures (rects, ids, state machine) live in shared.
- **NEXT_SURFACE_ID is `AtomicU64` with `fetch_add(1, Relaxed)`**: monotonic IDs never reused; even at 1M surfaces/sec it wouldn't wrap in a century. No reuse simplifies "is this id still valid" reasoning.
- **CompositorEvent::Input wraps the existing `shared::input::InputEvent`** (added in M23): no parallel hierarchy. Re-exporting/aliasing in compositor.rs is fine.
- **No fences in M24**: the architecture's `AttachBuffer { fence: Option<FenceId> }` is a Phase 7 future. CPU-rendered surfaces (everything we render in M24) don't need acquire fences. Document this as an explicit deviation.
- **`SurfaceTitle::from_bytes` must respect UTF-8 boundaries**: a naïve `min(64, len)` truncation can split a multi-byte codepoint. Walk backward from the cut point until a char-boundary byte (`b & 0xC0 != 0x80`).

## Dependencies & Risks

- **Depends on**: M23 (input subsystem, EarlyBootPhase::CompositorReady), M22 (Compute Kit), M19/M20/M21 (GPU Service + VirtIO-GPU driver). All present on `main`.
- **Risk: DMA pool exhaustion** — GPU Service holds 2×4 MB front/back; compositor needs another 2×4 MB. ~16 MB out of 64 MB DMA pool. Acceptable.
- **Risk: scanout race during handoff** — if the GPU Service is mid-`gpu_set_scanout` and the compositor calls `gpu_set_scanout` concurrently, the VirtIO-GPU controlq mutex (`VIRTIO_GPU.lock()`) serializes them. After `COMPOSITOR_ACTIVE = true`, GPU Service stops issuing scanout commands.
- **Risk: lock ordering** — compositor introduces SURFACE_TABLE in the middle of the existing chain. Existing chain: `PROCESS_TABLE > SHARED_REGION_TABLE > NOTIFICATION_TABLE > CHANNEL_TABLE > SELECT_WAITERS > BLOCK_ENGINE > {VIRTIO_*}`. Insertion point: SURFACE_TABLE goes BELOW BLOCK_ENGINE and ABOVE the VirtIO leaves. New chain: `... > BLOCK_ENGINE > SURFACE_TABLE > {VIRTIO_BLK, VIRTIO_GPU, VIRTIO_INPUT}`. Must update CLAUDE.md "Lock ordering" line + add per-Mutex comments.
- **Risk: flicker during display handoff** — if back-buffer not pre-filled before scanout swap, user sees garbage frame. Mitigation: `display_handoff` fills front with AIOS blue *before* scanout rebind (matches GPU Service pattern at lines 449–464).
- **Risk: test surface allocations leak DMA** — Step 15 must not leak. Use kernel-pool/heap slabs, not DMA, for the in-kernel test (only the compositor's main composition buffer is DMA).
- **Risk: Subsystem::COUNT churn** — adding a variant touches 4 enumeration tests + the `name()` match + COUNT const. Easy to miss one. Mitigation: search for all `subsystem_` test names and update each.

## Issues Encountered

(to be filled during implementation)

## Decisions Made

(to be filled during implementation)

## Lessons Learned

(to be filled during implementation)
