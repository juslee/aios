---
author: claude
date: 2026-05-07
tags: [compositor, shell, status-strip, taskbar, workspace, ipc, phase-7]
status: in-progress
phase: 7
milestone: 26
---

# Plan: Phase 7 M26 — Desktop Shell

## Context

Why this milestone matters: Phase 7 promises a Layer 1 desktop where the user can see and use AIOS without any AI infrastructure. M24 brought up the compositor service and software composition; M25 wired floating windows, decorations, focus, input routing, and IPC dispatch (with placeholder per-surface channels). M26 is where a user actually sees a desktop — Status Strip with time/memory/cores, a Taskbar that lists windows, a Workspace home view — and where the **first real client process** (`ProcessId(11)` "test-app") drives the IPC surface lifecycle end-to-end. This is the milestone that proves the IPC stack works for an external client and unblocks flipping `COMPOSITOR_PRESENT_ENABLED` to `true`.

Intended outcome at M26 end: `just run-gpu` boots to a static desktop with three shell surfaces visible, a 400×300 test-app window showing "Hello from AIOS!" that responds to keyboard input, Alt+Tab/Alt+F4/Super hotkeys functional, and frame timing logged.

## Approach

Three shell surfaces (Status Strip, Taskbar, Workspace) are **compositor-internal** per the phase doc (Step 24 explicitly says "not a separate process"). They are owned by `ProcessId(10)` (the compositor itself) and registered in `SURFACE_TABLE` like any other surface, but they have no separate process and no per-client IPC channel — the compositor renders into their backing buffers directly. This is a Layer 1 simplification of the architecture (compositor.md §3.1 treats all surfaces as IPC-bound), justified because the alternative — spawning three new processes per shell surface — adds complexity with no Layer 1 benefit.

The **test app (Step 28)** is the opposite — it is a real separate process (`ProcessId(11)`) using the full IPC surface lifecycle. This is what actually validates that IPC dispatch works end-to-end after M25's placeholder channels were superseded. The test app gets a per-client channel and passes it to the compositor inside `CompositorRequest::CreateSurface`, so the compositor can route events back to it instead of self-sending.

### Key gaps found during exploration

- **Per-client channels**: `Surface.channel` today always stores the well-known compositor channel (M25 placeholder). M26 needs the test app to create its own channel and pass it via `CreateSurface`. The M25 `is_self_channel` predicate becomes a no-op for real clients but must remain in place for shell surfaces (which still self-route or, better, never deliver IPC events at all).
- **CompositorRequest.client_channel field**: `CompositorRequest::CreateSurface` does not carry a sender channel today. Add a `client_channel: u64` field (or use `reserved` slot if present). Verify size assertion still holds. The M25 padding-UB lesson means we must declare this carefully.
- **`COMPOSITOR_PRESENT_ENABLED` flag**: still `false` post-M25. M26 is the milestone that flips it `true` — gated by the test app actually rendering. This unblocks visible composition output.
- **`storage::space_list()`** exists at `kernel/src/storage/space.rs:64` — ready for Workspace.
- **No `kernel/src/compositor/shell/`** directory yet — created fresh in this milestone with `mod.rs`, `status_strip.rs`, `taskbar.rs`, `workspace.rs`, `test_app.rs`.
- **Memory % calculation**: Walk pools (`Pool::Kernel`/`User`/`Dma`/`Reserved`/`Model`) summing free vs total pages. Use `crate::mm::pools` API; if no aggregate API exists, add one in shared/internal.
- **CPU %**: Scheduler exposes no per-core utilization metric today. Phase doc says "if available, else N/A" — render `CPU: N/A` until Phase 25 adds metrics.
- **Super→ShowWorkspace hotkey** is registered as placeholder in M25 (logs a `kinfo!` line). M26 wires the actual toggle action.
- **`run-compositor` justfile recipe** is M27 work. M26 testing uses `just run-gpu`.

### Shared crate refactoring (end of milestone)

Pure logic to move/extend in `shared/src/compositor.rs`:
- Status Strip text formatting helpers (tick→HH:MM, percent→2-char string, integer formatting helpers)
- Taskbar entry layout/truncation logic
- `route_event` extension for Panel-layer surfaces (shell surfaces never receive keyboard events even when clicked)
- Damage-region equality helpers used by Step 29 optimization
- `CompositorRequest::CreateSurface` `client_channel` field plumbing

Non-shareable (kernel-only): the actual surface buffer allocation, hotkey global state, frame-pacing service loop changes.

## Progress

- [ ] Step 24: Status Strip surface
  - [ ] 24a: Create `kernel/src/compositor/shell/mod.rs` with `pub mod status_strip; pub mod taskbar; pub mod workspace;` and a `init_shell_surfaces()` entry point called from `compositor::service::init_compositor()` after display handoff
  - [ ] 24b: Create `kernel/src/compositor/shell/status_strip.rs` defining `StatusStrip { surface_id, last_render_tick, cached_time, cached_mem_pct, cached_cpu_pct, cached_cores }`
  - [ ] 24c: In `init_shell_surfaces`: call `surface::create_internal(width=display_w, height=32, title="status-strip", content_type=SystemUI, layer=Panel, owner_pid=ProcessId(10), channel=COMPOSITOR_CHANNEL)` to allocate a SurfaceId; allocate a kernel-pool buffer (use `mm::pools::alloc_pages(Pool::Kernel, ...)` → write into ARGB8888); call `surface_attach_buffer` to bind. Position at y=0
  - [ ] 24d: Implement `pub fn render(strip: &mut StatusStrip, buf: &mut [u32], width, height, now_ticks)` that draws `AIOS  HH:MM  CPU: N/A  MEM: NN%  CORES: N` using `compositor::text::draw_text_clipped`
  - [ ] 24e: Time format: convert TICK_COUNT (62.5MHz / 1ms tick) to HH:MM; helper in `shared::compositor::format_hhmm(elapsed_ms: u64) -> [u8; 5]`
  - [ ] 24f: Memory %: iterate frame allocator pools, sum `total_pages` and `total_free_pages`, compute `100*(total-free)/total`. Add aggregate helper in `kernel/src/mm/pools.rs` if missing
  - [ ] 24g: Cores: read `crate::smp::cpu_count()` (or equivalent); fallback to constant `4` if not exposed
  - [ ] 24h: Damage: only mark surface damaged when `cached_*` values change between ticks
  - [ ] 24i: Hook a per-second update into compositor service loop: tick-aligned call to `shell::status_strip::tick(now_ms)` that re-renders if values changed
  - [ ] 24j: Verify: `just check` zero warnings, `just test` passes, `just run-gpu` shows top bar with time/mem/cores updating

- [ ] Step 25: Taskbar surface
  - [ ] 25a: Create `kernel/src/compositor/shell/taskbar.rs` defining `Taskbar { surface_id, entries: [TaskbarEntry; MAX_TASKBAR_ENTRIES], focused_index }`
  - [ ] 25b: Allocate Panel-layer surface at y=display_h-40, height=40
  - [ ] 25c: Implement `pub fn render(tb: &Taskbar, buf: &mut [u32], focus_id: Option<SurfaceId>)` — draws horizontal list of non-shell, non-test-app-internal surface titles, highlighted entry has different background color
  - [ ] 25d: Workspace button on far left: a 40×40 cell containing `[W]` glyph; reserves hit-zone for click handling
  - [ ] 25e: Surface count display on far right: `N windows`
  - [ ] 25f: Damage: only redraw when surface list mutates (track via `SURFACE_TABLE` generation counter or hash) or focus changes
  - [ ] 25g: Filter shell surfaces from the list: ignore surfaces with `owner_pid == ProcessId(10)` AND `layer == Panel`
  - [ ] 25h: Verify: `just check` + `just test` + `just run-gpu` shows bottom bar with test-app entry once Step 28 lands

- [ ] Step 26: Workspace surface
  - [ ] 26a: Create `kernel/src/compositor/shell/workspace.rs` defining `Workspace { surface_id, visible: bool }`
  - [ ] 26b: Allocate Normal-layer surface (NOT Panel — phase doc says "behind other Normal-layer windows but above Background"). Width≈800, height≈600, centered
  - [ ] 26c: `pub fn render(ws: &Workspace, buf: &mut [u32])`: draw "AIOS" title centered at top, list of system spaces from `storage::space_list()`, uptime line `"Uptime: NN:NN:NN"`
  - [ ] 26d: Visibility toggle: store `visible` flag, only insert into composition when `true`. When toggled off, send Suspended state
  - [ ] 26e: Wire Super hotkey: in `compositor::hotkey::apply_show_workspace()`, replace the M25 placeholder log with `shell::workspace::toggle_visibility()`
  - [ ] 26f: Register Super (no other modifiers) in `SYSTEM_HOTKEYS` — but since M25 noted modifier-on-same-event ambiguity, gate on Super press-edge detection (track previous Super state, fire on rising edge)
  - [ ] 26g: Verify: `just check` + `just test`; `just run-gpu` Super key shows Workspace; spaces list visible

- [ ] Step 27: Shell input integration
  - [ ] 27a: Extend `shared::compositor::route_event` so shell surfaces (those with `owner_pid==ProcessId(10)` AND `layer==Panel`) never receive keyboard events even when they have keyboard focus (they shouldn't ever get keyboard focus, but defensive)
  - [ ] 27b: Modify `compositor::focus::FocusManager::set_keyboard_focus` to refuse to set focus to a shell surface; clicking a shell surface keeps existing keyboard focus
  - [ ] 27c: In compositor service loop's pointer-event handler: when click lands on Taskbar entry, call `focus::set_focus(target_surface_id)` and raise to top of Z-order
  - [ ] 27d: When click lands on Workspace button in Taskbar, call `shell::workspace::toggle_visibility()`
  - [ ] 27e: Status Strip is non-interactive in M26 — clicks pass through (no hit-zone handling)
  - [ ] 27f: Verify: `just check` + `just test`; clicking taskbar entries switches focus; clicking [W] toggles workspace

- [ ] Step 28: Test application surface
  - [ ] 28a: Add `client_channel: u64` field to `CompositorRequest` in `shared/src/compositor.rs`. Verify `repr(C)` size assertion (no implicit padding — apply M25 lesson). Update all encoders/decoders. Update host tests
  - [ ] 28b: In `compositor::service::handle_create_surface`, store `req.client_channel` (or fall back to caller's well-known channel if 0) into `Surface.channel`. Shell surface helpers pass 0 → preserves M25 self-channel suppression
  - [ ] 28c: Create `kernel/src/compositor/test_app.rs`. Define `pub fn spawn_test_app()`: allocate `PROCESS_TABLE[11]` with name="test-app", grant minimal caps (ChannelCreate, ChannelAccess, SharedMemoryCreate, SharedMemoryAccess), allocate kernel stack, create thread with entry `test_app_entry`
  - [ ] 28d: Implement `test_app_entry()`: (1) `channel_create()` for own per-client channel, (2) `service_lookup(b"compositor")` to get compositor's well-known channel, (3) build `CompositorRequest::CreateSurface { width: 400, height: 300, title: "test-app", layer: Normal, client_channel: my_channel.0 }`, (4) `ipc_call(comp_ch, &req)` → expect `Configure` event, (5) `shmem_create(400*300*4)`, write solid background + "Hello from AIOS!" using a copy of compositor::text helper, (6) `ipc_send(comp_ch, AttachBuffer)`, (7) loop on `ipc_recv(my_channel, ...)` decoding `CompositorEvent::Input` and appending typed chars
  - [ ] 28e: Re-render and re-attach buffer on each text change (back-buffer pattern: ping-pong two shmem buffers)
  - [ ] 28f: Handle `CompositorEvent::CloseRequested` by calling `DestroySurface` then exiting thread
  - [ ] 28g: Call `spawn_test_app()` from `kernel/src/main.rs` after compositor init
  - [ ] 28h: **Flip `COMPOSITOR_PRESENT_ENABLED` to `true`** — only after the test app reliably renders. If the M24 data abort recurs, root-cause before flipping. Document gating evidence in plan
  - [ ] 28i: Advance boot phase to `EarlyBootPhase::CompositorReady` after test app surface is Active
  - [ ] 28j: Verify: `just run-gpu` — visible test-app window, Hello text, type into window appends chars, Alt+Tab focuses workspace, Alt+F4 closes test app

- [ ] Step 29: Shell rendering optimization
  - [ ] 29a: Status Strip damage: emit `DamageRegion::Empty` when none of (cached_time, cached_mem_pct, cached_cpu_pct, cached_cores) changed
  - [ ] 29b: Taskbar damage: track `last_focus_id` and a hash of surface-list contents; emit `Empty` when both stable
  - [ ] 29c: Workspace damage: emit `Empty` while invisible; full damage on toggle; otherwise hash uptime+spaces and rate-limit redraw to once per second
  - [ ] 29d: Profile: add per-frame composition timer (already partly present); log average over 60 frames
  - [ ] 29e: Static-desktop check: with no input for 5s, frame-time stat should report 0 composes
  - [ ] 29f: Verify: `just run-gpu` UART log shows `[Compositor] avg compose: <5ms / 60 frames`; idle period reports `composes=0`

- [ ] Step 30: Shell shared types and unit tests
  - [ ] 30a: Move pure logic to `shared/src/compositor.rs`: `format_hhmm`, `format_percent_2digits`, taskbar entry truncation function, taskbar entry layout (`compute_entry_x_positions`)
  - [ ] 30b: Add tests: `format_hhmm` at 0/59999/3600000/86399999/864000000ms boundaries; truncation at title length 1/8/64; layout with 0/1/N entries; damage-tracking transitions; Workspace toggle Suspended↔Active
  - [ ] 30c: Add CompositorRequest size + roundtrip tests including new `client_channel` field
  - [ ] 30d: Target ≥10 new tests; total kernel + shared count ≥553 (was 543 post-M25)
  - [ ] 30e: Verify: `just check` + `just test`

- [ ] Step 31 (milestone close): Update docs and audit
  - [ ] 31a: Check off M26 tasks in phase doc; add post-implementation notes for Steps 24–30
  - [ ] 31b: Update `CLAUDE.md` Workspace Layout (new `compositor/shell/` tree, `compositor/test_app.rs`), Key Technical Facts (CompositorRequest size if changed, COMPOSITOR_PRESENT_ENABLED now true, ProcessId(11) reserved for test-app), Architecture Doc Map, lock ordering if a new mutex is introduced (Workspace.visible would be one — likely guarded by SURFACE_TABLE so no new entry)
  - [ ] 31c: Update `docs/project/developer-guide.md` test counts, file sizes, new patterns (shell surface internal-ownership pattern)
  - [ ] 31d: Run `/audit-loop` recursively until 0 issues
  - [ ] 31e: Verify: `just check` + `just test` + `just run-gpu`

## Code Structure Decisions

- **Shell as compositor-internal `SURFACE_TABLE` entries (not separate processes)**: Three rejected alternatives — (a) spawn one process per shell surface (3× overhead, no Layer 1 benefit), (b) bypass `SURFACE_TABLE` and render shells in a separate path (forks the rendering pipeline, breaks z-order semantics), (c) stuff shells into a single magic SurfaceId reserved for shell (loses per-shell damage). Sticking with the existing surface protocol but using `owner_pid: ProcessId(10)` as the "internal" marker is the smallest delta.

- **Per-client channels via `client_channel` field on `CreateSurface`**: Alternative was IPC-layer sender introspection (compositor reads `ipc_recv` sender tag, looks up channel in a registry). Rejected because it requires an additional process→channel registry shared with IPC subsystem. Field-on-request is local to the compositor protocol and matches the shape of `SurfaceTitle` already passed inline. Apply M25 implicit-padding lesson — declare the field explicitly, no holes.

- **Shell surfaces never get keyboard focus, even on click**: Architecture (input.md §7.2) says "Shell surfaces do NOT take keyboard focus." Implementing this as a *guard in `set_keyboard_focus`* (refuse) plus `route_event` (no keyboard delivery) instead of leaving it to convention. Defense-in-depth.

- **Super hotkey rising-edge detection**: M25 deferred this because bare-Super carries `Modifiers::SUPER` on the same event. Track previous Super state in a `SuperKeyState` static (or extend `FocusManager`); fire on rising edge only. Avoids triggering the toggle on every key press while Super is held.

- **`COMPOSITOR_PRESENT_ENABLED` flip is its own sub-step (28h)**: Not the first thing in M26 — only flip *after* test app proves it renders. If a residual M24 race surfaces, this is the single point where it shows up. Keep the flag and the verification together so reverting is a one-line change if needed.

- **Static spawning for test app**: The test app process runs forever; no destruction path. This is a deliberate Layer 1 simplification — proper agent lifecycle is Phase 17.

## Dependencies & Risks

- **Depends on**: M24 compositor service running and dispatching `CompositorRequest`; M25 surface lifecycle, decorations, hit-test, focus, hotkey infrastructure; M23 input pipeline; storage `space_list()`; spleen 8×16 font feature; SMP cpu_count.
- **Risk: residual data abort when `COMPOSITOR_PRESENT_ENABLED=true`**. M24 deferred the flip because of an unresolved low-VA data abort under post-handoff IPC pressure. Mitigation: enable the flag *after* test app stabilizes; if it reproduces, root-cause via UART trace + `just run` boot log diff with/without flag, do not skip with a workaround. Document fix in lessons.
- **Risk: `client_channel` field breaks size invariant**. `CompositorRequest` already exceeds 256 bytes per the explore report (912B); the size assertion in shared crate currently checks `≤912` or whatever was last set. Adding 8 bytes might tip a boundary. Mitigation: pre-compute new size, update assertion deliberately, document in commit.
- **Risk: Super press-edge logic interferes with composed key combos** (Super+Tab in future, etc.). Mitigation: only the rising-edge of bare-Super (no other modifiers) triggers; Super+anything-else falls through to normal hotkey table.
- **Risk: shell surface owner-pid filter is brittle** — Taskbar uses `owner_pid==ProcessId(10) && layer==Panel` as the "is shell" predicate. If anyone else is given ProcessId(10), this breaks. Mitigation: encapsulate in a `Surface::is_shell()` helper; document as a contract.
- **Risk: storage `space_list()` returns Vec which requires alloc** — confirm we have global allocator at this point in boot (we do, since heap is up since M5). No changes needed but verify no `no_std`/no-alloc context.

## Verification

End-to-end test plan (run inside the worktree):

1. `just check` — must be zero warnings.
2. `just test` — host-side tests, target ≥553 total (≥10 new in M26).
3. `just run-gpu` — graphical boot. Expected UART output:
   - `[Boot] CompositorReady` phase transition
   - `[Compositor] avg compose: Xms / 60 frames` (X<5ms)
   - `[TestApp] surface created id=N`
   - No `[Compositor] watchdog: frame >100ms` warnings
4. Visual check inside QEMU window:
   - Top bar: `AIOS  HH:MM  CPU: N/A  MEM: NN%  CORES: 4`
   - Bottom bar: `[W] | test-app | 1 windows`
   - 400×300 test-app window centered with "Hello from AIOS!" text
   - Type chars → appended to displayed text
   - `Alt+Tab` switches focus between test-app and workspace (after it's been shown)
   - `Super` toggles workspace visibility
   - `Alt+F4` closes the focused window (test-app exits its surface)
5. `just run` (text-only) — must still boot to UART without compositor display, no regressions.
6. Audit loop returns 0 issues across doc/code/security categories.
