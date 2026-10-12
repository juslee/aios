---
author: claude
date: 2026-05-08
tags: [compositor, shell, ipc, architecture, phase-7]
status: final
---

# M26 architectural choices: shell-internal surfaces and per-client channels

## Context

Phase 7 M26 introduced three shell surfaces (Status Strip, Taskbar,
Workspace) and a test-app process that drives the first real IPC
client through the compositor's surface lifecycle. Two architectural
decisions in this milestone deserve preservation because they will
shape Phase 8+ work (real EL0 clients, agent surfaces).

## Decision 1: Shell surfaces are compositor-internal

The platform architecture (`docs/platform/compositor/protocol.md` §3.1)
treats every surface as an IPC-bound entity that follows the same
Created → Configured → Active lifecycle. A literal reading would have
spawned three additional processes for the three shell surfaces.

We chose instead to make the shells **compositor-internal**:
`owner_pid == ProcessId(10)` (the compositor's own process), `channel
== COMPOSITOR_CHANNEL` (the well-known service channel). M25's
`is_self_channel` predicate keeps any IPC events directed at them
from re-entering the compositor's own recv ring.

**Why this is right for Layer 1**:

- Three extra processes for three pixel-pushing surfaces with no
  separable lifecycle is overhead with no Layer-1 benefit.
- The shells render their own pixels by writing into shmem regions
  directly via the direct-map VA — no IPC round-trip to "submit"
  themselves.
- The cleanest enforcement of "shell surfaces never receive keyboard
  focus" is a kernel-side predicate, not an IPC capability check.

**Canonical predicate**: `Surface::is_shell()` returns
`owner_pid == ProcessId(10) && layer == SurfaceLayer::Panel`. The
Workspace is `Normal` layer (per phase doc) and is owned by
ProcessId(10) but is *not* shell — that's intentional, the Workspace
is a hidden-by-default home view, not a chrome panel. The Workspace
participates in normal stacking and can be raised by Super-toggle.

**Where the predicate is used**:

- `taskbar.rs::collect_entries` — filters shell out of the window list
- `input_route::set_keyboard_focus_safe` — refuses focus on shells
- `input_route::deliver_to_surface` — routes pointer to
  `shell::route_pointer` for shells, normal IPC otherwise
- `route_event_with_shell` (in shared) — drops keyboard events
  targeted at shells

**When this becomes wrong**: when shell surfaces need their own
process for crash isolation (Phase 18+ when "compositor must survive
shell bugs"). At that point the shells move out of ProcessId(10) and
gain real per-client channels — but the *protocol shape* stays the
same; only `Surface.owner_pid` changes.

## Decision 2: Per-client channels via `CompositorRequest.client_channel`

M25 stored the well-known compositor channel as every surface's
`Surface.channel` because per-client channels weren't wired yet.
That meant compositor-issued events round-tripped onto the
compositor's own recv ring, where they were misinterpreted as
requests and logged as warnings on every keypress (the M25
self-channel lesson).

M26 needed real per-client channels for the test app. Two designs
were considered:

1. **IPC-layer sender introspection**: `ipc_recv` returns the sender
   thread/process ID. The compositor reads this and looks up the
   sender's "default" channel from a registry shared with the IPC
   subsystem.
2. **Field-on-request**: add `client_channel: u64` to
   `CompositorRequest::CreateSurface`. The client passes its own
   receive endpoint inline; the compositor stores it as
   `Surface.channel`.

We chose **(2)**. Reasoning:

- Local to the compositor protocol — no cross-subsystem registry.
- Matches the existing "everything inline" pattern of
  `CompositorRequest` (`SurfaceTitle`, damage region, etc., are all
  carried in-band).
- A client can host multiple surfaces with multiple channels by
  passing a different `client_channel` per CreateSurface (future
  multi-window apps).
- Falls back gracefully: when `client_channel == 0`, the compositor
  uses the well-known service channel — preserving M25's
  `is_self_channel` suppression for shell-internal callers.

**Implementation**:

```rust
// shared/src/compositor.rs
pub const fn effective_channel(client_channel: u64, service_channel: ChannelId)
    -> ChannelId
{
    if client_channel == 0 {
        service_channel
    } else {
        ChannelId(client_channel)
    }
}
```

Used by `handle_create_surface` to derive the surface's stored
channel. Host-tested.

The `CompositorRequest` size grew by 8 bytes plus 4 bytes of explicit
padding (per the M25 implicit-padding-UB lesson) — total still well
within `MAX_MESSAGE_SIZE`.

## Decision 3: Pure transition functions in `shared`, kernel-side wrappers drive them

M25 established the FOCUS_MANAGER pattern: every focus operation
returns a `FocusChange` value so the caller can drop the lock before
issuing IPC. M26 generalized this into a recurring shape — pure
transition functions extracted to `shared/src/compositor.rs`,
kernel-side wrappers driving them from atomics or mutexes.

**Examples that landed**:

- `super_edge_step(state, is_super_key, is_press, is_release) ->
  (next_state, action)` — bare-Super rising-edge detector. Kernel
  wrapper in `compositor::hotkey` reads/writes two `AtomicBool`s.
- `route_event_with_shell(event, kbd_focus, pointer_hit, is_shell)
  -> RouteTarget` — input router. Kernel passes
  `surface::is_shell_id` as the predicate.
- `taskbar_pointer_action(layout, entry_ids, x, y) ->
  Option<TaskbarPointerAction>` — taskbar click resolver.
- `should_redraw_shell(needs_first_render, snapshot_changed) ->
  bool` — shell tick fast-path predicate.
- `workspace_render_mode(visible, unavailable, count) ->
  WorkspaceRenderMode` — workspace render dispatch.
- `effective_channel(client, service) -> ChannelId` — see Decision 2.
- `format_hhmm`, `format_hhmmss`, `format_percent_2digits`,
  `format_u32_left4`, `format_frame_window_summary` — pure
  formatters.

**Why this works**:

- Host-testable: ~89 new tests in M26 alone, none requiring kernel
  boot. The pure functions are exercised across boundary cases that
  would be expensive to reproduce on QEMU.
- Lock-discipline-friendly: the wrapper acquires its mutex, reads
  state into a local, drops the mutex, calls the pure function,
  applies the result. No mutex held across IPC, no surprise
  reentrance.
- Forces the design through the shared crate: any kernel-side state
  machine that grows complex enough to warrant extraction has to
  define its inputs and outputs cleanly first.

**When NOT to do this**: when the operation has irreducible
side-effects (allocation, IPC, mutex acquisition) or when the
"transition" is really just direct field-mutation under a lock. A
pure function for a single-field write is overkill.

## How these decisions compose

Together they make M26's shell layer a clean Layer-1 implementation:

- Shell surfaces are kernel-internal but ride the same surface
  protocol as future EL0 clients.
- Per-client channels enable real IPC with no compromise to the
  shell's self-internal short-circuit.
- Pure transition functions document the design through their type
  signatures and let host tests exercise edge cases without QEMU.

Phase 8's first EL0 client process should slot in as another
`client_channel`-passing CreateSurface caller with no protocol
changes. The shell layer continues to work unchanged.
