//! Focus management — keyboard focus, pointer focus, and Alt+Tab history.
//!
//! Per docs/platform/compositor/input.md §7.2:
//!   * Keyboard focus is set by user action (click, Alt+Tab) and triggers
//!     `FocusChanged` IPC events to the gaining and losing surfaces.
//!   * Pointer focus follows the cursor (via hit-test) and is internal-only —
//!     no IPC notification.
//!   * Focus history is the most-recently-used order used by Alt+Tab cycling.
//!
//! The `FocusManager` is a leaf mutex — never held while issuing IPC calls
//! or while holding `SURFACE_TABLE`/`WINDOW_Z_ORDER`. Callers snapshot the
//! affected surface ids under the lock, then drop the lock before sending
//! `FocusChanged` events.
//
// Mutators wired by Step 20 (input router routes click → set_keyboard_focus)
// and Step 22 (Alt+Tab cycles focus history). The kernel-side IPC dispatch
// (Step 20g) calls `surface_destroyed` when a surface is torn down so that
// focus state stays consistent.
#![allow(dead_code)]

use shared::compositor::SurfaceId;
use spin::Mutex;

/// Maximum entries in the most-recently-used focus history.
///
/// Per input.md §7.2 — a 16-entry ring covers Alt+Tab cycling for the
/// floating-window desktop comfortably.
pub const FOCUS_HISTORY_CAPACITY: usize = 16;

// ---------------------------------------------------------------------------
// Focus history container
// ---------------------------------------------------------------------------

/// A bounded most-recently-used list of `SurfaceId`s.
///
/// The most-recently-focused id is always at index 0; older entries trail
/// behind. `touch(id)` moves an existing entry (or pushes a new one) to the
/// front; `remove(id)` deletes an entry (used when a surface is destroyed).
/// When the list is full, `touch` evicts the LRU entry at the back.
#[derive(Clone, Copy)]
pub struct FocusHistory {
    entries: [SurfaceId; FOCUS_HISTORY_CAPACITY],
    len: usize,
}

impl FocusHistory {
    pub const fn new() -> Self {
        Self {
            entries: [SurfaceId::NONE; FOCUS_HISTORY_CAPACITY],
            len: 0,
        }
    }

    /// Mark `id` as the most-recently-used. If `id` was already in the
    /// history it moves to the front; otherwise it is inserted, evicting
    /// the LRU entry when at capacity. `SurfaceId::NONE` is rejected.
    pub fn touch(&mut self, id: SurfaceId) {
        if id.is_none() {
            return;
        }
        // If already present, move it to the front by shifting everything
        // ahead of it back by one slot.
        if let Some(pos) = self.entries[..self.len].iter().position(|&s| s == id) {
            for i in (1..=pos).rev() {
                self.entries[i] = self.entries[i - 1];
            }
            self.entries[0] = id;
            return;
        }
        // Insert as MRU; shift all existing entries back one. Drop the LRU
        // entry if we'd overflow the capacity.
        let new_len = (self.len + 1).min(FOCUS_HISTORY_CAPACITY);
        let shift_end = new_len.saturating_sub(1);
        for i in (1..=shift_end).rev() {
            self.entries[i] = self.entries[i - 1];
        }
        self.entries[0] = id;
        self.len = new_len;
        // Clear the slot just past `len` if we shrank conceptually (no-op
        // on overflow since we reused slot[len-1]).
        if self.len < FOCUS_HISTORY_CAPACITY {
            self.entries[self.len] = SurfaceId::NONE;
        }
    }

    /// Remove `id` from the history (called when a surface is destroyed).
    /// No-op if `id` is not present.
    pub fn remove(&mut self, id: SurfaceId) {
        if let Some(pos) = self.entries[..self.len].iter().position(|&s| s == id) {
            for i in pos..self.len - 1 {
                self.entries[i] = self.entries[i + 1];
            }
            self.len -= 1;
            self.entries[self.len] = SurfaceId::NONE;
        }
    }

    /// Most-recently-used surface id, or `None` if the history is empty.
    pub fn most_recent(&self) -> Option<SurfaceId> {
        if self.len == 0 {
            None
        } else {
            Some(self.entries[0])
        }
    }

    /// The id at position `n` from MRU (0 = most recent).
    pub fn nth(&self, n: usize) -> Option<SurfaceId> {
        if n >= self.len {
            None
        } else {
            Some(self.entries[n])
        }
    }

    /// Returns the next id to focus when Alt+Tab is pressed — the second
    /// most recent surface (i.e. the previously-focused one). Returns
    /// `None` if there is only zero or one entry.
    pub fn next_alt_tab(&self) -> Option<SurfaceId> {
        self.nth(1)
    }

    /// Iterate from MRU to LRU.
    pub fn iter(&self) -> impl Iterator<Item = SurfaceId> + '_ {
        self.entries[..self.len].iter().copied()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Default for FocusHistory {
    fn default() -> Self {
        Self::new()
    }
}

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
/// Lock ordering: leaf — never held while sending IPC events, locking
/// `SURFACE_TABLE`, `WINDOW_Z_ORDER`, or `DRAG_STATE`. Every public
/// operation returns enough information for the caller to drop this
/// lock before performing follow-up work.
pub static FOCUS_MANAGER: Mutex<FocusManager> = Mutex::new(FocusManager::new());

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_history_starts_empty() {
        let h = FocusHistory::new();
        assert!(h.is_empty());
        assert!(h.most_recent().is_none());
    }

    #[test]
    fn focus_history_touch_promotes_existing() {
        let mut h = FocusHistory::new();
        h.touch(SurfaceId(1));
        h.touch(SurfaceId(2));
        h.touch(SurfaceId(3));
        h.touch(SurfaceId(1)); // promote 1 back to MRU
        assert_eq!(h.most_recent(), Some(SurfaceId(1)));
        assert_eq!(h.nth(1), Some(SurfaceId(3)));
        assert_eq!(h.nth(2), Some(SurfaceId(2)));
        assert_eq!(h.len(), 3);
    }

    #[test]
    fn focus_history_evicts_at_capacity() {
        let mut h = FocusHistory::new();
        for i in 1..=FOCUS_HISTORY_CAPACITY as u64 + 1 {
            h.touch(SurfaceId(i));
        }
        assert_eq!(h.len(), FOCUS_HISTORY_CAPACITY);
        // The first inserted (id=1) should be evicted.
        assert!(!h.iter().any(|s| s == SurfaceId(1)));
        // Most recent is the last inserted.
        assert_eq!(
            h.most_recent(),
            Some(SurfaceId(FOCUS_HISTORY_CAPACITY as u64 + 1))
        );
    }

    #[test]
    fn focus_history_remove_present_id() {
        let mut h = FocusHistory::new();
        h.touch(SurfaceId(1));
        h.touch(SurfaceId(2));
        h.remove(SurfaceId(1));
        assert_eq!(h.len(), 1);
        assert_eq!(h.most_recent(), Some(SurfaceId(2)));
    }

    #[test]
    fn focus_history_remove_missing_id_is_noop() {
        let mut h = FocusHistory::new();
        h.touch(SurfaceId(1));
        h.remove(SurfaceId(99));
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn set_keyboard_focus_returns_lost_and_gained() {
        let mut fm = FocusManager::new();
        let change = fm.set_keyboard_focus(Some(SurfaceId(1)));
        assert_eq!(
            change,
            FocusChange {
                lost: None,
                gained: Some(SurfaceId(1))
            }
        );
        let change = fm.set_keyboard_focus(Some(SurfaceId(2)));
        assert_eq!(
            change,
            FocusChange {
                lost: Some(SurfaceId(1)),
                gained: Some(SurfaceId(2))
            }
        );
    }

    #[test]
    fn set_keyboard_focus_idempotent_returns_no_change() {
        let mut fm = FocusManager::new();
        fm.set_keyboard_focus(Some(SurfaceId(1)));
        let change = fm.set_keyboard_focus(Some(SurfaceId(1)));
        assert_eq!(change.lost, None);
        assert_eq!(change.gained, None);
    }

    #[test]
    fn alt_tab_target_skips_current_focus() {
        let mut fm = FocusManager::new();
        fm.set_keyboard_focus(Some(SurfaceId(1)));
        fm.set_keyboard_focus(Some(SurfaceId(2)));
        // History MRU: 2, 1. Current = 2 → alt+tab target = 1.
        assert_eq!(fm.alt_tab_target(), Some(SurfaceId(1)));
    }

    #[test]
    fn surface_destroyed_clears_focus() {
        let mut fm = FocusManager::new();
        fm.set_keyboard_focus(Some(SurfaceId(1)));
        let change = fm.surface_destroyed(SurfaceId(1));
        assert_eq!(change.lost, Some(SurfaceId(1)));
        assert_eq!(fm.keyboard_focus(), None);
        assert!(fm.history().is_empty());
    }

    #[test]
    fn surface_destroyed_purges_pointer_focus() {
        let mut fm = FocusManager::new();
        fm.set_pointer_focus(Some(SurfaceId(7)));
        fm.surface_destroyed(SurfaceId(7));
        assert_eq!(fm.pointer_focus(), None);
    }
}
