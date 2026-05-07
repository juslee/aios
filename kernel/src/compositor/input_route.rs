//! Compositor input routing pipeline.
//!
//! Per docs/platform/compositor/input.md §7.1, every input event traverses a
//! six-stage pipeline before reaching a surface. M25 implements four of those
//! stages — coalescing, hotkey filtering, focus routing, and IPC delivery —
//! sufficient for keyboard + pointer input. Gesture recognition (touch) and
//! the device-driver stage land in later phases.
//!
//! The pipeline is driven by the compositor service loop: each iteration it
//! drains the kernel input queue, runs every popped event through
//! `route_event`, and dispatches the result. Side effects on focus and
//! drag state happen here; actual frame rendering follows in the same loop
//! tick.
//
// Wired up by Step 20g (compositor_loop pulls events from INPUT_QUEUE and
// calls `drain_and_route`). Step 22 fills in the hotkey table consumed by
// `HotkeyFilter`.
#![allow(dead_code)]

use shared::compositor::{
    hit_zone, CompositorEvent, HitZone, ResizeEdge, SurfaceId, WindowDecoration,
};
use shared::input::{InputEvent, MouseButton};

use super::cursor;
use super::focus::FOCUS_MANAGER;
use super::hotkey;
use super::surface::SURFACE_TABLE;
use super::window::{outer_rect, WINDOW_Z_ORDER};

// ---------------------------------------------------------------------------
// Filter primitives
// ---------------------------------------------------------------------------

/// Outcome of an input filter applied to a single event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterResult {
    /// Pass the event to the next stage unchanged.
    Pass,
    /// The filter consumed the event — do not forward.
    Consume,
    /// Replace the event with a transformed version.
    Transform(InputEvent),
}

/// Pipeline stage trait — each stage decides whether the event continues,
/// is consumed, or is transformed.
pub trait InputFilter {
    fn filter(&mut self, event: &InputEvent) -> FilterResult;
}

// ---------------------------------------------------------------------------
// Stage 1: pointer coalescing
// ---------------------------------------------------------------------------

/// Successive pointer-motion events are merged into one — only the latest
/// position is delivered. Pointer events that carry a button transition
/// flush the coalesced motion first and then deliver the transition
/// standalone.
pub struct PointerCoalescer {
    pending: Option<(u32, u32)>,
}

impl PointerCoalescer {
    pub const fn new() -> Self {
        Self { pending: None }
    }

    /// Returns `true` and the coalesced position when there is a pending
    /// motion to flush.
    pub fn take_pending(&mut self) -> Option<InputEvent> {
        self.pending.take().map(|(x, y)| InputEvent::Pointer {
            x,
            y,
            button: None,
            state: None,
        })
    }
}

impl Default for PointerCoalescer {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Stage 2: hotkey filter (table consulted in Step 22)
// ---------------------------------------------------------------------------

/// Consults the system hotkey table; consumes matched events so they never
/// reach a surface. Step 22 populates the hotkey table; this stage is
/// always wired in the pipeline so adding hotkeys takes no further plumbing.
pub struct HotkeyFilter;

impl HotkeyFilter {
    pub const fn new() -> Self {
        Self
    }

    fn handle(&self, event: &InputEvent) -> FilterResult {
        if let InputEvent::Keyboard {
            key,
            state,
            modifiers,
        } = event
        {
            if let Some(action) = hotkey::match_hotkey(*key, *modifiers, *state) {
                hotkey::apply(action);
                return FilterResult::Consume;
            }
        }
        FilterResult::Pass
    }
}

impl Default for HotkeyFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl InputFilter for HotkeyFilter {
    fn filter(&mut self, event: &InputEvent) -> FilterResult {
        self.handle(event)
    }
}

// ---------------------------------------------------------------------------
// Stage 3: focus router — pick a target surface
// ---------------------------------------------------------------------------

/// Where a routed event should be delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteTarget {
    /// Deliver to the surface (its IPC channel).
    Surface(SurfaceId),
    /// Pointer landed on decoration that the compositor handles itself —
    /// the move/resize/close-button machinery (Step 21).
    Decoration { surface: SurfaceId, zone: HitZone },
    /// No target — the event is dropped (e.g. pointer over empty desktop).
    None,
}

/// Resolve the target for `event` given the current focus and z-order
/// state. Pure logic given the shared inputs — split out so it can be
/// tested without locking globals.
pub fn route_event(
    event: &InputEvent,
    keyboard_focus: Option<SurfaceId>,
    pointer_hit: Option<(SurfaceId, HitZone)>,
) -> RouteTarget {
    match event {
        InputEvent::Keyboard { .. } => match keyboard_focus {
            Some(id) => RouteTarget::Surface(id),
            None => RouteTarget::None,
        },
        InputEvent::Pointer { .. } => match pointer_hit {
            Some((id, HitZone::Content)) => RouteTarget::Surface(id),
            Some((id, zone)) => RouteTarget::Decoration { surface: id, zone },
            None => RouteTarget::None,
        },
    }
}

/// Returns true when the pointer event represents a button transition
/// (click or release), as opposed to bare motion. Coalescing only
/// collapses motion-only events.
pub fn is_pointer_transition(event: &InputEvent) -> bool {
    matches!(
        event,
        InputEvent::Pointer {
            button: Some(_),
            state: Some(_),
            ..
        }
    )
}

// ---------------------------------------------------------------------------
// Pipeline driver
// ---------------------------------------------------------------------------

/// Drain the kernel input queue and route every event through the
/// compositor pipeline. Called once per compositor loop iteration.
pub fn drain_and_route() {
    let mut hotkeys = HotkeyFilter::new();
    let mut coalescer = PointerCoalescer::new();
    let deco = WindowDecoration::DEFAULT;

    while let Some(event) = crate::input::pop_event() {
        // Stage 1: coalescing for motion-only pointer events.
        match &event {
            InputEvent::Pointer {
                x,
                y,
                button,
                state,
            } => {
                cursor::set_position(*x as i32, *y as i32);
                if button.is_none() && state.is_none() {
                    // Motion only — coalesce and continue.
                    coalescer.pending = Some((*x, *y));
                    continue;
                } else {
                    // Transition: flush any pending motion first, then
                    // deliver the transition standalone.
                    if let Some(pending) = coalescer.take_pending() {
                        run_through_stages(&pending, &mut hotkeys, &deco);
                    }
                    run_through_stages(&event, &mut hotkeys, &deco);
                }
            }
            InputEvent::Keyboard { .. } => {
                // Keyboard events: flush pending pointer first so order is
                // preserved on the wire (motion happens before keypress).
                if let Some(pending) = coalescer.take_pending() {
                    run_through_stages(&pending, &mut hotkeys, &deco);
                }
                run_through_stages(&event, &mut hotkeys, &deco);
            }
        }
    }

    // After draining, flush any pending coalesced motion as the last event.
    if let Some(pending) = coalescer.take_pending() {
        run_through_stages(&pending, &mut hotkeys, &deco);
    }
}

/// Run an event through stages 2–4 (hotkey filter → focus router →
/// delivery). Stage 1 (coalescing) is handled by the caller because it
/// requires lookahead into the queue.
fn run_through_stages(event: &InputEvent, hotkeys: &mut HotkeyFilter, deco: &WindowDecoration) {
    let event = match hotkeys.filter(event) {
        FilterResult::Pass => *event,
        FilterResult::Consume => return,
        FilterResult::Transform(replacement) => replacement,
    };

    let (kbd_focus, pointer_hit) = snapshot_focus_state(&event, deco);

    // For pointer motion, also update pointer focus (no IPC).
    if matches!(event, InputEvent::Pointer { .. }) {
        let mut fm = FOCUS_MANAGER.lock();
        fm.set_pointer_focus(pointer_hit.map(|(id, _)| id));
    }

    let target = route_event(&event, kbd_focus, pointer_hit);
    match target {
        RouteTarget::Surface(id) => deliver_to_surface(id, &event),
        RouteTarget::Decoration { surface, zone } => {
            super::window::handle_decoration_event(surface, zone, &event)
        }
        RouteTarget::None => {}
    }
}

/// Snapshot the current keyboard focus and the pointer-hit result for a
/// given event, dropping the focus and z-order locks before the snapshot
/// is consumed by the routing decision. This keeps the FOCUS_MANAGER lock
/// scope tight and avoids holding it across IPC.
fn snapshot_focus_state(
    event: &InputEvent,
    deco: &WindowDecoration,
) -> (Option<SurfaceId>, Option<(SurfaceId, HitZone)>) {
    let kbd_focus = FOCUS_MANAGER.lock().keyboard_focus();
    let pointer_hit = match event {
        InputEvent::Pointer { x, y, .. } => topmost_at(*x as i32, *y as i32, deco),
        InputEvent::Keyboard { .. } => None,
    };
    (kbd_focus, pointer_hit)
}

/// Walk the z-order top-down and return the topmost surface (and zone)
/// containing `(px, py)`. Acquires both `WINDOW_Z_ORDER` and `SURFACE_TABLE`
/// briefly; never holds either across IPC.
fn topmost_at(px: i32, py: i32, deco: &WindowDecoration) -> Option<(SurfaceId, HitZone)> {
    let z = WINDOW_Z_ORDER.lock();
    let table = SURFACE_TABLE.lock();
    for id in z.iter_top_down() {
        if id.is_none() {
            continue;
        }
        let surface = match table.iter().find_map(|s| {
            s.as_ref()
                .filter(|surf| surf.id == id && surf.state.is_visible())
                .copied()
        }) {
            Some(s) => s,
            None => continue,
        };
        let (x, y, w, h) = outer_rect(&surface, deco);
        if let Some(zone) = hit_zone(px, py, x, y, w, h, deco) {
            return Some((surface.id, zone));
        }
    }
    None
}

/// Send an `Input` event to the surface's IPC channel. Best-effort: the
/// send is non-blocking and dropped on a full ring or dead channel.
fn deliver_to_surface(id: SurfaceId, event: &InputEvent) {
    // Look up the surface's channel under SURFACE_TABLE; drop the lock
    // before issuing the IPC call so we don't hold SURFACE_TABLE across
    // the IPC subsystem.
    let channel = {
        let table = SURFACE_TABLE.lock();
        table
            .iter()
            .find_map(|s| s.as_ref().filter(|surf| surf.id == id).map(|s| s.channel))
    };
    let channel = match channel {
        Some(c) => c,
        None => return,
    };

    // Click on Content also raises the surface to the top and gives it
    // keyboard focus (input.md §7.2 — click sets focus).
    if let InputEvent::Pointer {
        button: Some(MouseButton::Left),
        state: Some(shared::input::ButtonState::Pressed),
        ..
    } = event
    {
        promote_to_focus(id);
    }

    let payload = CompositorEvent::input(id, event);
    let bytes: &[u8] = unsafe {
        // SAFETY: CompositorEvent is repr(C) Copy with no padding-trap
        // fields. We borrow its bytes for the duration of the IPC call.
        // The slice does not outlive `payload`.
        // Maintained by: payload is on this function's stack; the slice is
        // consumed synchronously by ipc_send before payload goes out of scope.
        // Violation: a longer-lived borrow would dangle when the stack frame
        // exits.
        core::slice::from_raw_parts(
            (&payload as *const CompositorEvent) as *const u8,
            core::mem::size_of::<CompositorEvent>(),
        )
    };

    let _ = crate::ipc::ipc_send(channel, bytes);
}

/// Click-to-focus side effect. Updates focus state and z-order.
fn promote_to_focus(id: SurfaceId) {
    let change = {
        let mut fm = FOCUS_MANAGER.lock();
        fm.set_keyboard_focus(Some(id))
    };
    {
        let mut z = WINDOW_Z_ORDER.lock();
        z.raise_to_top(id);
    }
    notify_focus_change(change);
}

/// Send `FocusChanged` IPC events to the gaining and losing surfaces.
/// Snapshots the channel ids under SURFACE_TABLE, drops the lock, then
/// issues `ipc_send` on each channel.
pub fn notify_focus_change(change: super::focus::FocusChange) {
    let (lost_channel, gained_channel) = {
        let table = SURFACE_TABLE.lock();
        let lookup = |id: Option<SurfaceId>| -> Option<shared::ipc::ChannelId> {
            id.and_then(|id| {
                table
                    .iter()
                    .find_map(|s| s.as_ref().filter(|surf| surf.id == id).map(|s| s.channel))
            })
        };
        (lookup(change.lost), lookup(change.gained))
    };

    if let (Some(id), Some(ch)) = (change.lost, lost_channel) {
        let event = CompositorEvent::focus_changed(id, false);
        send_event_bytes(ch, &event);
    }
    if let (Some(id), Some(ch)) = (change.gained, gained_channel) {
        let event = CompositorEvent::focus_changed(id, true);
        send_event_bytes(ch, &event);
    }
}

/// Internal helper: serialize a `CompositorEvent` to bytes and `ipc_send`
/// it on the given channel. Best-effort.
pub fn send_event_bytes(channel: shared::ipc::ChannelId, event: &CompositorEvent) {
    let bytes: &[u8] = unsafe {
        // SAFETY: CompositorEvent is repr(C) Copy. We borrow its bytes for
        // the duration of the synchronous ipc_send call.
        // Maintained by: caller's stack frame outlives the slice.
        // Violation: a longer-lived borrow would dangle.
        core::slice::from_raw_parts(
            (event as *const CompositorEvent) as *const u8,
            core::mem::size_of::<CompositorEvent>(),
        )
    };
    let _ = crate::ipc::ipc_send(channel, bytes);
}

// Helper used by the resize edge → cursor mapping (also referenced by
// Step 21 move/resize handlers).
#[allow(dead_code)]
pub const fn edge_is_corner(edge: ResizeEdge) -> bool {
    matches!(
        edge,
        ResizeEdge::NorthEast
            | ResizeEdge::NorthWest
            | ResizeEdge::SouthEast
            | ResizeEdge::SouthWest
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
//
// Pure routing logic (`route_event`, `is_pointer_transition`) is exercised
// by host-side tests in `shared::compositor` via Step 23. The kernel-side
// pipeline plumbing is verified by the build (lock ordering and trait
// implementations) and integration testing in M26.
