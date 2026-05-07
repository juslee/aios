//! System hotkeys — Alt+Tab, Alt+F4, Super.
//!
//! Per docs/platform/compositor/input.md §7.3, the compositor consumes
//! system hotkeys before any surface receives them. The matching helper
//! and action table are deliberately small for M25; the broader pipeline
//! (agent-registered hotkeys, secure input mode) lands in later phases.
//
// `match_hotkey` is consulted by the input pipeline's `HotkeyFilter`
// (Step 20). Step 22 fills in the table contents.
#![allow(dead_code)]

use shared::input::{KeyCode, KeyState, Modifiers};

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
/// focus change so we never hold FOCUS_MANAGER across IPC. Also raises
/// the new focus to the top of its layer in the z-order list.
fn apply_switch_window() {
    let (target, change) = {
        let mut fm = super::focus::FOCUS_MANAGER.lock();
        let target = fm.alt_tab_target();
        let change = fm.set_keyboard_focus(target);
        (target, change)
    };
    if let Some(id) = target {
        let mut z = super::window::WINDOW_Z_ORDER.lock();
        z.raise_to_top(id);
    }
    super::input_route::notify_focus_change(change);
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
    // M26 introduces the Workspace surface — this is a placeholder.
    crate::kinfo!(Compositor, "Hotkey: ShowWorkspace (M26 placeholder)");
}
