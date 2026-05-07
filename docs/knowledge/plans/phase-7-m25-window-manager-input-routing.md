---
author: claude
date: 2026-05-07
tags: [compositor, input, window-manager, focus, hotkey]
status: in-progress
phase: 7
milestone: 25
---

# Plan: Phase 7 M25 — Window Manager & Input Routing

## Approach

M25 turns M24's headless surface table into an interactive desktop. M24 delivered the
service skeleton, surface lifecycle primitives (`surface_create`, `surface_attach_buffer`,
`surface_destroy`, `surface_resize`, `surface_set_layer`), the opaque/premultiplied blit
primitives, the damage tracker, and the compose-and-present scaffold gated behind
`COMPOSITOR_PRESENT_ENABLED`. None of those entry points are reached today — the service
loop receives IPC messages and acks them with an empty reply.

M25 wires the parts that make the desktop usable:

1. Window decorations (compositor-rendered title bars and focus borders) so the user can
   see and click windows.
2. Hit-testing and a software cursor so pointer input maps to surfaces.
3. Focus management with separate keyboard/pointer focus and a 16-entry MRU history.
4. The input pipeline (coalesce → hotkey filter → focus router → IPC delivery), which is
   the first piece that actually consumes the input queue produced in M23.
5. Move/resize via title-bar drag and edge-grabs, plus close-button handling.
6. System hotkeys (Alt+Tab, Alt+F4, Super) that are consumed before any surface sees them.
7. Shared-crate refactoring of the pure data types (`HitZone`, hotkey matching, etc.) so
   they can be unit-tested host-side.

**Key gaps found during exploration:**
- The compositor service loop currently calls `ipc_reply(ch, &[])` on every received
  message — no `CompositorRequest` decoding. Without IPC dispatch the M25 input routing
  cannot reach a surface (no surface was ever created). Step 20 must add minimal
  CreateSurface/AttachBuffer/DestroySurface/Resize/SetLayer dispatch so the input router
  has surfaces to route to.
- `KeyCode` already has `RightShift/RightCtrl/RightAlt/RightSuper` but the kernel input
  module's `update_modifiers` covers them already — confirmed during exploration.
- The `Modifiers::SUPER` bit and `KEY_LEFTMETA`/`KEY_RIGHTMETA` are wired end-to-end.
- Existing `Capability` already has the four compositor variants needed for M25; no new
  capabilities required. Per the phase doc Step 9 was an M24 task.

**Shared crate refactoring (Step 23):** The pure data types introduced by Steps 17–22 —
`HitZone`, `WindowDecoration` constants, hotkey matching, focus history container,
input filter `FilterResult` — should live in `shared/src/compositor.rs` (or a new
`shared/src/compositor_wm.rs` module) so they can be unit-tested without booting QEMU.
The kernel-side state (FocusManager mutex, hotkey table, drag state machine) stays in
`kernel/src/compositor/`.

## Progress

- [ ] Step 17: Window manager — floating layout and decorations
  - [ ] 17a: Add `HitZone` enum and `WindowDecoration` constants to `shared/src/compositor.rs`
  - [ ] 17b: Create `kernel/src/compositor/window.rs` with `WindowDecoration` rendering helpers
  - [ ] 17c: Add `render_title_bar(&Surface, &mut [u32], stride, fb_width, fb_height, focused)` — fills bg, blits title text via spleen-font, draws close-button "X" glyph
  - [ ] 17d: Add `render_focus_indicator(&Surface, &mut [u32], stride, fb_width, fb_height, focused)` — colored 1px border (blue when focused, gray otherwise)
  - [ ] 17e: Add `WINDOW_Z_ORDER: Mutex<FixedQueue<SurfaceId, MAX_SURFACES>>` (or similar `[Option<SurfaceId>; MAX_SURFACES]` array) tracking insertion order
  - [ ] 17f: Add `raise_to_top(SurfaceId)` and `default_position(width, height, sequence)` (centered with cascading offset)
  - [ ] 17g: Verify: `just check` + `just test`
- [ ] Step 18: Pointer hit-testing and software cursor
  - [ ] 18a: Add `cursor.rs` with `CURSOR_ARROW: [u32; 16*16]` const sprite (RGBA arrow with black outline + white fill, transparent elsewhere)
  - [ ] 18b: Add `hit_test(x: i32, y: i32, surfaces: &[Surface], z_order: &[SurfaceId]) -> Option<(SurfaceId, HitZone)>` that walks z-order top-to-bottom; per-surface check against decoration zones (TitleBar, CloseButton, ResizeBorder*, Content)
  - [ ] 18c: Add `render_cursor(buffer, stride, width, height, x, y)` — alpha-composite the sprite via `blit_alpha_premultiplied` (last operation before present)
  - [ ] 18d: Maintain `CURSOR_POS: Mutex<(i32, i32)>` updated from `InputEvent::Pointer`
  - [ ] 18e: Verify: `just check` + `just test`
- [ ] Step 19: Focus management
  - [ ] 19a: Create `kernel/src/compositor/focus.rs` with `FocusManager { keyboard_focus, pointer_focus, focus_history: FixedQueue<SurfaceId, 16> }`
  - [ ] 19b: Add `set_keyboard_focus(SurfaceId)` — emits `FocusChanged` to losing + gaining surfaces via IPC, pushes to history, raises in z-order
  - [ ] 19c: Add `set_pointer_focus(Option<SurfaceId>)` — internal-only, no IPC event
  - [ ] 19d: Add `surface_destroyed(SurfaceId)` — purge from history, clear focus if matched
  - [ ] 19e: Static `FOCUS_MANAGER: Mutex<FocusManager>` with documented lock ordering position (above SURFACE_TABLE? — see Step 17 ordering decision)
  - [ ] 19f: Verify: `just check` + `just test`
- [ ] Step 20: Input routing pipeline + IPC dispatch
  - [ ] 20a: Create `kernel/src/compositor/input_route.rs` with `FilterResult { Pass, Consume, Transform(InputEvent) }`
  - [ ] 20b: Implement event coalescing — collect `InputEvent::Pointer` and merge into latest position before routing
  - [ ] 20c: Implement hotkey filter (skeleton only — full table in Step 22): always-pass for keys that don't match
  - [ ] 20d: Implement focus router — keyboard → keyboard_focus surface; pointer → hit-test result, updates pointer_focus
  - [ ] 20e: Implement IPC delivery — for each routed event, call `ipc_call` with `CompositorEvent::input(surface, &event)` to the surface's channel (best-effort, drop on full)
  - [ ] 20f: Wire pipeline into compositor main loop: drain `crate::input::pop_event()` each iteration, run through pipeline
  - [ ] 20g: Add minimal IPC dispatch: decode `CompositorRequest` from received messages, call `surface_create/_attach_buffer/_destroy/_resize/_set_layer`, send `Configure` event back via `ipc_reply`, register/deregister with z-order list and focus manager on create/destroy
  - [ ] 20h: Verify: `just check` + `just test`
- [ ] Step 21: Window move and resize
  - [ ] 21a: Add `DragState` enum (Idle, Moving { surface, start_pointer, start_window_pos }, Resizing { surface, edge: HitZone, start_pointer, start_dims }) in `kernel/src/compositor/window.rs`
  - [ ] 21b: On pointer-down on TitleBar: enter Moving; on pointer-down on ResizeBorder*: enter Resizing
  - [ ] 21c: On pointer-move during Moving: update surface x/y, mark damaged
  - [ ] 21d: On pointer-move during Resizing: clamp to MIN_WINDOW_SIZE (200x100), call `surface_resize`, send `Configure` event
  - [ ] 21e: On pointer-up: exit drag state
  - [ ] 21f: On click on CloseButton: send `CloseRequested` event to surface
  - [ ] 21g: Verify: `just check` + `just test`
- [ ] Step 22: Alt+Tab and system hotkeys
  - [ ] 22a: Create `kernel/src/compositor/hotkey.rs` with `HotkeyAction { SwitchWindow, CloseWindow, ShowWorkspace }`
  - [ ] 22b: Add `KeyCombo { key: KeyCode, modifiers: Modifiers }` matching helper (move to shared if convenient)
  - [ ] 22c: Define static `SYSTEM_HOTKEYS: &[(KeyCombo, HotkeyAction)]` const table — Alt+Tab, Alt+F4, Super
  - [ ] 22d: Add `match_hotkey(key, modifiers, state)` returning `Option<HotkeyAction>` — only on Pressed, ignored on repeat
  - [ ] 22e: Implement `apply_hotkey(action)` — Alt+Tab cycles `focus_history`, Alt+F4 sends CloseRequested to current focus, Super logs "ShowWorkspace TODO" (actual workspace surface lands in M26)
  - [ ] 22f: Wire into Step 20's hotkey filter — return `FilterResult::Consume` on match
  - [ ] 22g: Verify: `just check` + `just test`
- [ ] Step 23: Shared crate types and unit tests
  - [ ] 23a: Move `HitZone`, `KeyCombo`, `WindowDecoration` constants, MIN_WINDOW_SIZE constants to `shared/src/compositor.rs`
  - [ ] 23b: Add hit-test pure logic helper in shared (the geometric math without the table walk) so it's testable
  - [ ] 23c: Add 15+ unit tests: hit-test overlapping surfaces topmost wins; focus history ring buffer wraps at 16; raise-to-top z-order operation; KeyCombo matching for Alt+Tab/Alt+F4/Super; cursor-position clamping at boundaries; resize clamping to min size
  - [ ] 23d: Verify: `just check` + `just test`
- [ ] Final: Update CLAUDE.md, phase doc, developer-guide; dead code cleanup; run audit loop

## Code Structure Decisions

- **Decorations rendered by the compositor** (not the surface owner): the surface buffer
  contains client content only; the compositor draws title bar and borders on top during
  composition. Aligns with the phase doc Step 17 directive and prevents apps from
  spoofing focus indicators.
- **Z-order tracking via fixed array**, not a `Vec`: `[Option<SurfaceId>; MAX_SURFACES]`
  with a length counter. Matches the no-allocation discipline of the rest of the kernel,
  same pattern as `SURFACE_TABLE`.
- **Focus history via `shared::collections::FixedQueue<SurfaceId, 16>`** — already exists
  in the codebase, used by the input queue. Removing destroyed surfaces from a ring
  requires a `retain`-style scan; we accept O(N) retain because N=16.
- **Hit-zone is geometric pure logic**: lives in `shared/`, takes a surface rect plus
  decoration constants and returns the zone. Allows host-side unit tests to cover all
  edge cases (corners, narrow windows, off-screen pointer).
- **Drag state machine in window.rs** as a single global `DragState` mutex (the user
  drags one window at a time). Avoids per-surface state and keeps the click/move/release
  logic linear.
- **System hotkey table is `const`**: no agent-registration in M25 (deferred to a later
  phase per input.md §7.3). The table lives next to the matching helper.
- **Hotkey filter consumes before focus router**: matches the input.md §7.1 pipeline
  order. System hotkeys never reach a surface — guards against keystroke logging or
  focus-steal via shortcut interception.
- **Step 20 also adds IPC dispatch**: while the phase doc lists this implicitly under
  M24 Step 12 (surface lifecycle "send Configure event via IPC"), no caller actually
  wires it. M25 needs it for the input router to reach a surface; piggybacking onto
  Step 20 keeps the change scoped.
- **`COMPOSITOR_PRESENT_ENABLED` stays `false` for M25**: enabling the present path
  requires resolving the pre-existing data-abort race noted in M24's Step 14 commentary.
  Out of scope for M25; M26 (shell rendering) is a more natural place to flip it once
  shell surfaces give us deterministic damage to compose.

## Dependencies & Risks

- **Depends on**: M24 surface table, blit primitives, damage tracker, compositor
  service loop, input event queue (M23), `CompositorEvent::input()` builder, IPC
  primitives (`ipc_call`, `ipc_reply`).
- **Risk**: Holding `FOCUS_MANAGER` while sending an IPC event could deadlock if the
  IPC subsystem ever calls back into compositor code. Mitigation: snapshot the surface
  list under the lock, then drop the lock before issuing IPC calls. This matches the
  pattern in `gpu::service`.
- **Risk**: Routing input via `ipc_call` blocks the compositor on a slow client.
  Mitigation: use `ipc_call` with a short timeout (e.g., 1 tick = 1ms) and drop on
  timeout — input events are best-effort.
- **Risk**: Coalescing pointer events too aggressively could lose button-state
  transitions. Mitigation: only coalesce motion (`button == None && state == None`);
  any pointer event with a button transition is delivered standalone.
- **Risk**: Cursor sprite alpha-blending against an undrawn back buffer flickers if
  the present path is enabled. Mitigation: cursor render is always the last operation
  before present and operates on the back buffer directly, after surface composition.

## Issues Encountered

(to be filled during implementation)

## Decisions Made

(to be filled during implementation)

## Lessons Learned

(to be filled during implementation)
