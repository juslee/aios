//! Focus management — keyboard focus, pointer focus, and Alt+Tab history.
//!
//! Per docs/platform/compositor/input.md §7.2:
//!   * Keyboard focus is set by user action (click, Alt+Tab) and triggers
//!     `FocusChanged` IPC events to the gaining and losing surfaces.
//!   * Pointer focus follows the cursor (via hit-test) and is internal-only —
//!     no IPC notification.
//!   * Focus history is the most-recently-used order used by Alt+Tab cycling.
//!
//! `FOCUS_MANAGER` is a leaf mutex — every public mutator returns the
//! affected surfaces (a `FocusChange`) so callers can drop the lock
//! before issuing IPC events or acquiring `SURFACE_TABLE` /
//! `WINDOW_Z_ORDER`. The lock is never held across `ipc_send` /
//! `ipc_reply`. The leaf-only invariant is enforced by inspection at
//! every call site (Step 20's `notify_focus_change`, Step 22's
//! `apply_switch_window`/`apply_close_window`).
//
// Mutators wired by Step 20 (input router routes click → set_keyboard_focus)
// and Step 22 (Alt+Tab cycles focus history). The kernel-side IPC dispatch
// (Step 20g) calls `surface_destroyed` when a surface is torn down so that
// focus state stays consistent.
#![allow(dead_code)]

use shared::compositor::{FocusHistory, SurfaceId};
use spin::Mutex;

// ---------------------------------------------------------------------------
// FocusManager
// ---------------------------------------------------------------------------

/// Compositor focus state.
///
/// Holds the keyboard focus, pointer focus, and the focus history ring.
/// All mutations go through the public methods so the invariants — focus
/// history is touched only on keyboard-focus changes; destroyed surfaces are
/// purged from both focus slots and the history — stay consistent.
#[derive(Clone, Copy)]
pub struct FocusManager {
    keyboard_focus: Option<SurfaceId>,
    pointer_focus: Option<SurfaceId>,
    focus_history: FocusHistory,
}

/// Result of a keyboard focus change. Returned to the caller so it can
/// drop the manager lock before issuing IPC events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusChange {
    /// Surface that just lost keyboard focus, if any.
    pub lost: Option<SurfaceId>,
    /// Surface that just gained keyboard focus, if any.
    pub gained: Option<SurfaceId>,
}

impl FocusManager {
    pub const fn new() -> Self {
        Self {
            keyboard_focus: None,
            pointer_focus: None,
            focus_history: FocusHistory::new(),
        }
    }

    /// Request keyboard focus for `id`. No-op if already focused.
    /// Updates focus history. Returns the surfaces that need a
    /// `FocusChanged` IPC notification.
    ///
    /// Pass `None` to clear keyboard focus (e.g., the focused surface was
    /// destroyed and there is no successor).
    pub fn set_keyboard_focus(&mut self, id: Option<SurfaceId>) -> FocusChange {
        if id == self.keyboard_focus {
            return FocusChange {
                lost: None,
                gained: None,
            };
        }
        let lost = self.keyboard_focus;
        self.keyboard_focus = id;
        if let Some(new) = id {
            self.focus_history.touch(new);
        }
        FocusChange { lost, gained: id }
    }

    /// Update pointer focus (internal — never emits IPC).
    pub fn set_pointer_focus(&mut self, id: Option<SurfaceId>) {
        self.pointer_focus = id;
    }

    /// Currently keyboard-focused surface, if any.
    pub fn keyboard_focus(&self) -> Option<SurfaceId> {
        self.keyboard_focus
    }

    /// Current pointer focus (surface under the cursor), if any.
    pub fn pointer_focus(&self) -> Option<SurfaceId> {
        self.pointer_focus
    }

    /// Read access to the focus history.
    pub fn history(&self) -> &FocusHistory {
        &self.focus_history
    }

    /// Compute the next Alt+Tab target — the most-recently-used surface
    /// that is *not* currently keyboard-focused.
    pub fn alt_tab_target(&self) -> Option<SurfaceId> {
        let current = self.keyboard_focus;
        self.focus_history.iter().find(|&id| Some(id) != current)
    }

    /// Notify the focus manager that a surface has been destroyed.
    /// Purges it from both focus slots and the history. Returns a
    /// `FocusChange` describing any required IPC follow-up (e.g., when
    /// the destroyed surface had keyboard focus, the manager will clear
    /// the slot — the caller may want to focus the next-MRU surface).
    pub fn surface_destroyed(&mut self, id: SurfaceId) -> FocusChange {
        self.focus_history.remove(id);
        if self.pointer_focus == Some(id) {
            self.pointer_focus = None;
        }
        if self.keyboard_focus == Some(id) {
            self.keyboard_focus = None;
            return FocusChange {
                lost: Some(id),
                gained: None,
            };
        }
        FocusChange {
            lost: None,
            gained: None,
        }
    }
}

impl Default for FocusManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Global focus state
// ---------------------------------------------------------------------------

/// System-wide focus state.
///
/// Lock ordering: leaf — every public operation returns a `FocusChange`
/// so the caller can drop this lock before sending IPC events or
/// acquiring `SURFACE_TABLE`, `WINDOW_Z_ORDER`, or `DRAG_STATE`. Never
/// nested inside any of those.
pub static FOCUS_MANAGER: Mutex<FocusManager> = Mutex::new(FocusManager::new());

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
//
// Pure-logic tests for `FocusHistory` and the `FocusManager` state
// machine live in `shared::compositor::tests` (Step 23) where the
// kernel-test target excludes them from the kernel build but they
// still execute under `just test` host-side.
