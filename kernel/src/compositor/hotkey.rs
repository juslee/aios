//! System hotkeys — Alt+Tab, Alt+F4, Super.
//!
//! Per docs/platform/compositor/input.md §7.3, the compositor consumes
//! system hotkeys before any surface receives them. The matching helper
//! and action table are deliberately small for M25; the broader pipeline
//! (agent-registered hotkeys, secure input mode) lands in later phases.
//!
//! M26 Step 26 adds bare-Super edge detection separately — the
//! `SYSTEM_HOTKEYS` table matches `(key, modifiers)` exactly on the press
//! event, but Super-only requires release-edge detection to avoid
//! firing on Super+anything-else combos. See `super_key_edge_detector`.
//
// `match_hotkey` is consulted by the input pipeline's `HotkeyFilter`
// (Step 20). Step 22 fills in the table contents.
#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, Ordering};

use shared::input::{InputEvent, KeyCode, KeyState, Modifiers};

// ---------------------------------------------------------------------------
// Hotkey actions
// ---------------------------------------------------------------------------

/// Discrete actions a system hotkey may trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    /// Cycle keyboard focus to the next surface in the focus history.
    SwitchWindow,
    /// Send `CloseRequested` to the currently focused surface.
    CloseWindow,
    /// Toggle the workspace surface visibility (M26).
    ShowWorkspace,
}

/// A hotkey binding — key + modifier combo paired with an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyBinding {
    pub key: KeyCode,
    pub modifiers: Modifiers,
    pub action: HotkeyAction,
}

/// System hotkey table.
///
/// Static `const` so M25 has no agent-registration path — system hotkeys
/// cannot be overridden. Entries are matched on key+modifiers in
/// `match_hotkey`; the first match wins.
///
/// `Modifiers(0)` would match a key press with NO modifiers, which is
/// almost never what we want for system hotkeys. Bare-Super is therefore
/// not in the table — workspace toggling lands in M26 alongside the
/// Workspace surface.
pub const SYSTEM_HOTKEYS: &[HotkeyBinding] = &[
    HotkeyBinding {
        key: KeyCode::Tab,
        modifiers: Modifiers(Modifiers::ALT),
        action: HotkeyAction::SwitchWindow,
    },
    HotkeyBinding {
        key: KeyCode::F4,
        modifiers: Modifiers(Modifiers::ALT),
        action: HotkeyAction::CloseWindow,
    },
];

// ---------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------

/// Returns the action triggered by `(key, modifiers)` on press, or `None`
/// if no binding matches. Releases and repeats never trigger hotkeys —
/// only the initial press.
pub fn match_hotkey(key: KeyCode, modifiers: Modifiers, state: KeyState) -> Option<HotkeyAction> {
    if !matches!(state, KeyState::Pressed) {
        return None;
    }
    SYSTEM_HOTKEYS
        .iter()
        .find(|b| b.key == key && b.modifiers.0 == modifiers.0)
        .map(|b| b.action)
}

/// Apply a matched hotkey action. Step 22 implements the action bodies.
pub fn apply(action: HotkeyAction) {
    match action {
        HotkeyAction::SwitchWindow => apply_switch_window(),
        HotkeyAction::CloseWindow => apply_close_window(),
        HotkeyAction::ShowWorkspace => apply_show_workspace(),
    }
}

/// Cycle keyboard focus to the next surface in the focus history (Alt+Tab).
///
/// Snapshots the FocusManager target, drops the lock, then issues the
/// focus change via the canonical `set_keyboard_focus_safe` entry point
/// so shell surfaces are refused (defensive — `FocusHistory` should
/// never contain them, but Step 27 makes the guard explicit at every
/// focus mutation site). Also raises the new focus to the top of its
/// layer in the z-order list.
fn apply_switch_window() {
    let target = {
        let fm = super::focus::FOCUS_MANAGER.lock();
        fm.alt_tab_target()
    };
    let _ = super::input_route::set_keyboard_focus_safe(target);
    if let Some(id) = target {
        let mut z = super::window::WINDOW_Z_ORDER.lock();
        z.raise_to_top(id);
    }
}

/// Send `CloseRequested` to the currently focused surface (Alt+F4).
fn apply_close_window() {
    let focus = {
        let fm = super::focus::FOCUS_MANAGER.lock();
        fm.keyboard_focus()
    };
    let id = match focus {
        Some(id) => id,
        None => return,
    };
    let channel = {
        let table = super::surface::SURFACE_TABLE.lock();
        table.iter().find_map(|s| {
            s.as_ref()
                .filter(|surf| surf.id == id)
                .map(|surf| surf.channel)
        })
    };
    if let Some(ch) = channel {
        let event = shared::compositor::CompositorEvent::close_requested(id);
        super::input_route::send_event_bytes(ch, &event);
    }
}

fn apply_show_workspace() {
    // M26 Step 26: toggle the Workspace home surface. The shell module
    // owns the visibility flag and the surface_set_visible call; this
    // hotkey applier just dispatches.
    super::shell::workspace::toggle_visibility();
}

// ---------------------------------------------------------------------------
// Bare-Super edge detector (M26 Step 26)
// ---------------------------------------------------------------------------
//
// Bare-Super is awkward because the `Modifiers::SUPER` bit is set on the
// same event that delivers the Super press itself, so a naive `match
// (KeyCode::Super, modifiers == SUPER)` would fire on every Super-something
// combo too. The pattern users expect is:
//
//   * tap-and-release Super alone           → toggle Workspace
//   * Super + (Tab | letter | …) combos     → never toggle Workspace
//
// The pure transition function lives in `shared::compositor::super_edge_step`
// (host-tested). This module wires it to two `AtomicBool`s so the
// kernel input pipeline stays lock-free and the host tests stay
// allocation-free.
//
// Atomics: leaf, no contention concern (input pipeline is single-threaded
// inside the compositor service loop). No lock-ordering implications.

static PREV_SUPER_PRESSED: AtomicBool = AtomicBool::new(false);
static SUPER_USED_IN_COMBO: AtomicBool = AtomicBool::new(false);

/// Returns `true` when `key` is one of the Super keys (Left or Right).
fn is_super_key(key: KeyCode) -> bool {
    matches!(key, KeyCode::LeftSuper | KeyCode::RightSuper)
}

/// Snapshot the persistent edge-detector state.
fn load_super_state() -> shared::compositor::SuperEdgeState {
    shared::compositor::SuperEdgeState {
        prev_super_pressed: PREV_SUPER_PRESSED.load(Ordering::Acquire),
        super_used_in_combo: SUPER_USED_IN_COMBO.load(Ordering::Acquire),
    }
}

/// Write back the persistent edge-detector state.
fn store_super_state(state: shared::compositor::SuperEdgeState) {
    PREV_SUPER_PRESSED.store(state.prev_super_pressed, Ordering::Release);
    SUPER_USED_IN_COMBO.store(state.super_used_in_combo, Ordering::Release);
}

/// Inspect a single keyboard event for the bare-Super press-tap pattern.
///
/// Delegates to `shared::compositor::super_edge_step` for the pure
/// transition logic, then writes the resulting state back to the
/// `AtomicBool`s. Returns `Some(ShowWorkspace)` on the release edge of a
/// bare-Super tap. Returns `None` for all other events. Pointer events
/// are ignored.
///
/// The caller (`HotkeyFilter`) should consume both Super press and Super
/// release events so they never reach client surfaces, regardless of
/// whether this function returned an action — Super is a system
/// shortcut and isn't forwarded.
pub fn super_key_edge_detector(event: &InputEvent) -> Option<HotkeyAction> {
    let (key, state) = match event {
        InputEvent::Keyboard { key, state, .. } => (*key, *state),
        InputEvent::Pointer { .. } => return None,
    };

    let is_press = matches!(state, KeyState::Pressed);
    let is_release = matches!(state, KeyState::Released);
    let prev = load_super_state();
    let (next, action) =
        shared::compositor::super_edge_step(prev, is_super_key(key), is_press, is_release);
    if next != prev {
        store_super_state(next);
    }
    action.map(|a| match a {
        shared::compositor::SuperEdgeAction::ShowWorkspace => HotkeyAction::ShowWorkspace,
    })
}

/// Returns true when this event is a Super key press or release that the
/// edge detector is responsible for. The `HotkeyFilter` consumes these
/// events from the input pipeline so they never reach surfaces.
pub fn is_super_event(event: &InputEvent) -> bool {
    matches!(
        event,
        InputEvent::Keyboard { key, .. } if is_super_key(*key)
    )
}
