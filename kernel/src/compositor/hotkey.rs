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

/// System hotkey table. Static `const` so M25 has no agent-registration
/// path — system hotkeys cannot be overridden.
///
/// Step 22 populates this table.
pub const SYSTEM_HOTKEYS: &[HotkeyBinding] = &[];

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

fn apply_switch_window() {
    // Step 22 wires this to FocusManager.alt_tab_target().
}

fn apply_close_window() {
    // Step 22 wires this to send CloseRequested via IPC.
}

fn apply_show_workspace() {
    // M26 introduces the Workspace surface — this is a placeholder.
}
