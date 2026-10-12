//! Compositor protocol types shared between kernel and user-space.
//!
//! Per docs/platform/compositor/protocol.md §3.1–3.4. Phase 7 M24 ships the
//! Layer 1 subset: surface lifecycle, flat z-ordered surfaces, opaque blitting,
//! rect/full damage. Fences (§3.3), subsurfaces (§3.1), full hint set (§4),
//! and scene-graph diffs (rendering.md §5.1) are deferred.

use crate::input::InputEvent;
use crate::ipc::{ChannelId, SharedMemoryId, MAX_MESSAGE_SIZE};
use crate::sched::ProcessId;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of surfaces tracked by the compositor at once.
/// Layer 1 desktop never has more than ~10 windows; 32 leaves headroom.
pub const MAX_SURFACES: usize = 32;

/// Maximum bytes for a UTF-8 surface title before truncation.
pub const SURFACE_TITLE_MAX: usize = 64;

// ---------------------------------------------------------------------------
// Window decorations (M25)
// ---------------------------------------------------------------------------

/// Visual constants used by the compositor to render window chrome.
///
/// Decorations are rendered by the compositor on top of the client-supplied
/// surface buffer. Apps cannot draw inside these regions — they only see the
/// content rectangle (height − title bar; width − 2·border).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowDecoration {
    /// Title bar height in pixels.
    pub title_bar_height: u32,
    /// Outer border thickness in pixels (drawn on all four sides).
    pub border_width: u32,
    /// Width of the close-button glyph cell inside the title bar.
    pub close_button_width: u32,
    /// Hit-test margin in pixels for the resize border (extends inward from the edge).
    pub resize_margin: u32,
}

impl WindowDecoration {
    /// Default decoration metrics for Layer 1 floating windows.
    pub const DEFAULT: Self = Self {
        title_bar_height: 24,
        border_width: 1,
        close_button_width: 24,
        resize_margin: 8,
    };
}

/// Minimum interior dimensions a window may be resized to.
///
/// The decorated window may be larger by `2 * border_width` and
/// `border_width + title_bar_height` in width and height respectively, but the
/// content surface itself must not shrink below this size.
pub const MIN_WINDOW_WIDTH: u32 = 200;
pub const MIN_WINDOW_HEIGHT: u32 = 100;

// ---------------------------------------------------------------------------
// Hit-test zones (M25)
// ---------------------------------------------------------------------------

/// A region of a window that pointer events can land in.
///
/// `hit_zone()` (declared lower in this file) maps a pointer position to one
/// of these zones given the surface's geometry and decoration metrics. The
/// compositor's pointer-down handler dispatches by zone:
/// `TitleBar` → start drag; `ResizeBorder*` → start resize; `CloseButton` →
/// send `CloseRequested`; `Content` → forward to the surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitZone {
    /// Inside the title bar (drag-to-move).
    TitleBar,
    /// Inside the close-button glyph cell.
    CloseButton,
    /// Inside the surface content rectangle.
    Content,
    /// On a resize edge or corner. The variant identifies which.
    ResizeBorder(ResizeEdge),
}

/// Edge or corner identifier for `HitZone::ResizeBorder`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeEdge {
    North,
    South,
    East,
    West,
    NorthEast,
    NorthWest,
    SouthEast,
    SouthWest,
}

/// Compute which decoration zone a pointer at `(px, py)` lands in for a window
/// whose decorated outer rectangle is `(x, y, width, height)`.
///
/// `(width, height)` are the decorated outer dimensions — i.e. the content
/// surface plus the decoration metrics in `deco`. Returns `None` if the point
/// lies entirely outside the window (the caller should walk to the next
/// surface in z-order).
///
/// Resize zones take precedence over the title bar at the corners, and the
/// close-button cell takes precedence over the title bar inside the title bar
/// strip. Content covers the rest.
pub fn hit_zone(
    px: i32,
    py: i32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    deco: &WindowDecoration,
) -> Option<HitZone> {
    let w = width as i32;
    let h = height as i32;
    if w <= 0 || h <= 0 {
        return None;
    }
    let x_end = x.saturating_add(w);
    let y_end = y.saturating_add(h);
    if px < x || px >= x_end || py < y || py >= y_end {
        return None;
    }

    let rm = deco.resize_margin as i32;
    let on_left = px < x + rm;
    let on_right = px >= x_end - rm;
    let on_top = py < y + rm;
    let on_bottom = py >= y_end - rm;

    if on_top && on_left {
        return Some(HitZone::ResizeBorder(ResizeEdge::NorthWest));
    }
    if on_top && on_right {
        return Some(HitZone::ResizeBorder(ResizeEdge::NorthEast));
    }
    if on_bottom && on_left {
        return Some(HitZone::ResizeBorder(ResizeEdge::SouthWest));
    }
    if on_bottom && on_right {
        return Some(HitZone::ResizeBorder(ResizeEdge::SouthEast));
    }
    if on_top {
        return Some(HitZone::ResizeBorder(ResizeEdge::North));
    }
    if on_bottom {
        return Some(HitZone::ResizeBorder(ResizeEdge::South));
    }
    if on_left {
        return Some(HitZone::ResizeBorder(ResizeEdge::West));
    }
    if on_right {
        return Some(HitZone::ResizeBorder(ResizeEdge::East));
    }

    let title_bar_top = y + deco.border_width as i32;
    let title_bar_bottom = title_bar_top + deco.title_bar_height as i32;
    if py < title_bar_bottom {
        let close_left = x_end - deco.border_width as i32 - deco.close_button_width as i32;
        if px >= close_left {
            return Some(HitZone::CloseButton);
        }
        return Some(HitZone::TitleBar);
    }

    Some(HitZone::Content)
}

// ---------------------------------------------------------------------------
// Z-order tracking (M25)
// ---------------------------------------------------------------------------

/// Most-recently-focused-last z-order list within a single layer.
///
/// The compositor sorts surfaces at composition time by `(SurfaceLayer as
/// u8, position-in-this-list)`. `raise_to_top` removes an entry then
/// pushes it onto the end so the topmost surface is always at the back of
/// the array. The list stores `SurfaceId::NONE` in unused slots and never
/// reorders entries except via the public mutators.
///
/// Layer 1 desktop fits comfortably in `MAX_SURFACES` slots; the
/// container rejects `push` past capacity rather than panicking.
#[derive(Clone, Copy)]
pub struct ZOrder {
    entries: [SurfaceId; MAX_SURFACES],
    len: usize,
}

impl ZOrder {
    pub const fn new() -> Self {
        Self {
            entries: [SurfaceId::NONE; MAX_SURFACES],
            len: 0,
        }
    }

    /// Append a newly-created surface to the top of the stack. Returns
    /// `true` on success; `false` if the list is full or `id` is the
    /// NONE sentinel.
    pub fn push(&mut self, id: SurfaceId) -> bool {
        if self.len >= MAX_SURFACES || id.is_none() {
            return false;
        }
        self.entries[self.len] = id;
        self.len += 1;
        true
    }

    /// Remove `id` from the list (called when a surface is destroyed).
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

    /// Move `id` to the top of the stack — most-recently-focused last.
    /// No-op if `id` is not present.
    pub fn raise_to_top(&mut self, id: SurfaceId) {
        if self.entries[..self.len].contains(&id) {
            self.remove(id);
            self.push(id);
        }
    }

    /// Iterate from bottom of stack (oldest) to top (most-recent).
    pub fn iter(&self) -> impl Iterator<Item = SurfaceId> + '_ {
        self.entries[..self.len].iter().copied()
    }

    /// Iterate from top of stack (most-recent) to bottom — used by hit-testing.
    pub fn iter_top_down(&self) -> impl Iterator<Item = SurfaceId> + '_ {
        self.entries[..self.len].iter().rev().copied()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Default for ZOrder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Focus history (M25)
// ---------------------------------------------------------------------------

/// Maximum entries in the most-recently-used focus history.
///
/// Per docs/platform/compositor/input.md §7.2 — a 16-entry ring covers
/// Alt+Tab cycling for the floating-window desktop comfortably.
pub const FOCUS_HISTORY_CAPACITY: usize = 16;

/// A bounded most-recently-used list of `SurfaceId`s.
///
/// The most-recently-focused id is always at index 0; older entries
/// trail behind. `touch(id)` moves an existing entry (or inserts a new
/// one) to the front; `remove(id)` deletes an entry (used when a
/// surface is destroyed). At capacity, `touch` evicts the LRU entry.
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

    /// Mark `id` as the most-recently-used. `SurfaceId::NONE` is rejected.
    pub fn touch(&mut self, id: SurfaceId) {
        if id.is_none() {
            return;
        }
        if let Some(pos) = self.entries[..self.len].iter().position(|&s| s == id) {
            for i in (1..=pos).rev() {
                self.entries[i] = self.entries[i - 1];
            }
            self.entries[0] = id;
            return;
        }
        let new_len = (self.len + 1).min(FOCUS_HISTORY_CAPACITY);
        let shift_end = new_len.saturating_sub(1);
        for i in (1..=shift_end).rev() {
            self.entries[i] = self.entries[i - 1];
        }
        self.entries[0] = id;
        self.len = new_len;
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
// Input routing — pure decision helpers (M25)
// ---------------------------------------------------------------------------

/// Where a routed input event should be delivered.
///
/// Pure data — the kernel-side router builds this from focus state and
/// hit-test results, then dispatches accordingly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteTarget {
    /// Deliver to the surface (its IPC channel).
    Surface(SurfaceId),
    /// Pointer landed on a non-content decoration zone — handled by the
    /// compositor itself (move/resize/close-button).
    Decoration { surface: SurfaceId, zone: HitZone },
    /// No target — drop the event (e.g. pointer over empty desktop).
    None,
}

/// Decide where an `InputEvent` should be delivered.
///
/// Pure logic split out so it can be tested host-side without locking
/// any compositor globals. Pre-M26 callers can keep using this entry
/// point — it dispatches to `route_event_with_shell` with a "no shells"
/// predicate, preserving the original behaviour.
pub fn route_event(
    event: &crate::input::InputEvent,
    keyboard_focus: Option<SurfaceId>,
    pointer_hit: Option<(SurfaceId, HitZone)>,
) -> RouteTarget {
    route_event_with_shell(event, keyboard_focus, pointer_hit, |_| false)
}

/// Like `route_event`, but the caller supplies an `is_shell` predicate
/// that identifies compositor-internal shell surfaces (Status Strip,
/// Taskbar, Workspace).
///
/// M26 Step 27 invariant: shell surfaces never receive **keyboard**
/// events. Even if the focus manager somehow points at a shell
/// surface, this router collapses that to `RouteTarget::None`. Pointer
/// events targeted at a shell surface are still returned as
/// `RouteTarget::Surface(id)` — the kernel-side dispatcher branches on
/// `is_shell` again to route those into the shell's own pointer
/// handlers (Taskbar entries focus their corresponding window;
/// Workspace button toggles visibility; Status Strip drops).
pub fn route_event_with_shell<F>(
    event: &crate::input::InputEvent,
    keyboard_focus: Option<SurfaceId>,
    pointer_hit: Option<(SurfaceId, HitZone)>,
    is_shell: F,
) -> RouteTarget
where
    F: Fn(SurfaceId) -> bool,
{
    use crate::input::InputEvent;
    match event {
        InputEvent::Keyboard { .. } => match keyboard_focus {
            Some(id) if is_shell(id) => RouteTarget::None,
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

/// Clamp a candidate `(width, height)` resize to the minimum content
/// dimensions. Returns the adjusted dimensions.
pub fn clamp_window_size(width: u32, height: u32) -> (u32, u32) {
    (width.max(MIN_WINDOW_WIDTH), height.max(MIN_WINDOW_HEIGHT))
}

// ---------------------------------------------------------------------------
// Surface identity
// ---------------------------------------------------------------------------

/// Unique identifier for a compositor surface.
///
/// Assigned by the compositor on `CreateSurface`. Monotonically increasing,
/// never reused — even at 1M surfaces/sec the 64-bit space takes >500k years
/// to wrap, so reasoning about surface identity is straightforward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SurfaceId(pub u64);

impl SurfaceId {
    /// Sentinel value representing "no surface" — used in fields that may be absent.
    pub const NONE: Self = Self(0);

    /// First valid surface id (allocator starts here).
    pub const FIRST: Self = Self(1);

    /// Returns true if this id is the NONE sentinel.
    pub const fn is_none(self) -> bool {
        self.0 == 0
    }
}

// ---------------------------------------------------------------------------
// Surface state machine
// ---------------------------------------------------------------------------

/// Surface lifecycle state.
///
/// Surfaces progress linearly: Created → Configured → Active. `Suspended`
/// is a reversible detour from `Active`. `Destroyed` is terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SurfaceState {
    /// Surface allocated by the compositor; client has not received Configure yet.
    Created = 0,
    /// Compositor has sent Configure; client knows dimensions but no buffer attached.
    Configured = 1,
    /// Buffer attached and being composited each frame.
    Active = 2,
    /// Surface hidden (minimized / agent backgrounded). Resources retained, not composited.
    Suspended = 3,
    /// Surface torn down. Slot is released but the SurfaceId is never reused.
    Destroyed = 4,
}

impl SurfaceState {
    /// Returns true if no further transitions out of this state are possible.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Destroyed)
    }

    /// Returns true when the surface should appear in the composited frame.
    ///
    /// Layer 1 only renders surfaces that have an attached buffer
    /// (`Active`). `Configured` surfaces are still being set up by the
    /// client; `Created`/`Suspended`/`Destroyed` are not visible.
    pub const fn is_visible(self) -> bool {
        matches!(self, Self::Active)
    }

    /// Returns true if the state machine permits transitioning from `self` to `next`.
    ///
    /// Per protocol.md §3.1 state diagram. `Destroyed` from any non-terminal state
    /// is always allowed (cleanup path).
    pub const fn can_transition_to(self, next: Self) -> bool {
        match (self, next) {
            // Forward path.
            (Self::Created, Self::Configured) => true,
            (Self::Configured, Self::Active) => true,
            // Idempotent self-transition for Active (subsequent AttachBuffer calls).
            (Self::Active, Self::Active) => true,
            // Reversible suspend.
            (Self::Active, Self::Suspended) => true,
            (Self::Suspended, Self::Active) => true,
            // Cleanup from any non-terminal state.
            (Self::Created, Self::Destroyed) => true,
            (Self::Configured, Self::Destroyed) => true,
            (Self::Active, Self::Destroyed) => true,
            (Self::Suspended, Self::Destroyed) => true,
            // Anything else is invalid (skipping Configured, regressing Destroyed, etc.).
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Z-order layers
// ---------------------------------------------------------------------------

/// Compositing layer — controls z-order across surfaces.
///
/// Surfaces are sorted first by layer (Background to Panel), then by insertion
/// order within a layer. Numeric values are stable: higher = closer to viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum SurfaceLayer {
    /// Behind everything else — wallpaper, workspace background.
    Background = 0,
    /// Application windows. Default for client surfaces.
    Normal = 1,
    /// Foreground utility windows (always-on-top dialogs).
    TopLevel = 2,
    /// Floating overlays — notifications, menus, popovers.
    Overlay = 3,
    /// System chrome — taskbar, status strip. Locked to screen edges.
    Panel = 4,
}

// ---------------------------------------------------------------------------
// Surface metadata
// ---------------------------------------------------------------------------

/// UTF-8 surface title, fixed-capacity for IPC marshalling.
///
/// Always truncates at a UTF-8 character boundary so the stored bytes are
/// guaranteed to be valid UTF-8 (provided the input was valid UTF-8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct SurfaceTitle {
    /// Title bytes (zero-padded after `len`).
    pub bytes: [u8; SURFACE_TITLE_MAX],
    /// Number of valid bytes in `bytes`.
    pub len: u8,
}

impl SurfaceTitle {
    /// Empty title.
    pub const EMPTY: Self = Self {
        bytes: [0; SURFACE_TITLE_MAX],
        len: 0,
    };

    /// Construct a `SurfaceTitle` from a byte slice, truncating at a UTF-8
    /// character boundary if it exceeds `SURFACE_TITLE_MAX`.
    ///
    /// If `input` is not valid UTF-8 or contains an unfinished codepoint at the
    /// truncation point, the longest valid UTF-8 prefix that fits is stored.
    pub fn from_bytes(input: &[u8]) -> Self {
        let mut bytes = [0u8; SURFACE_TITLE_MAX];
        let mut cut = core::cmp::min(input.len(), SURFACE_TITLE_MAX);
        // Walk back to a UTF-8 char boundary. Continuation bytes have the
        // top two bits == 10; lead bytes do not. We never cross over a lead
        // byte's continuations.
        while cut > 0 && (input[cut - 1] & 0xC0) == 0x80 && {
            // Walk past contiguous continuation bytes to find the lead.
            let mut probe = cut - 1;
            while probe > 0 && (input[probe] & 0xC0) == 0x80 {
                probe -= 1;
            }
            // Decide whether the lead at `probe` plus its continuations fit
            // entirely within [0, cut). If not, we must truncate before `probe`.
            let lead = input[probe];
            let expected_len = if lead & 0x80 == 0 {
                1
            } else if lead & 0xE0 == 0xC0 {
                2
            } else if lead & 0xF0 == 0xE0 {
                3
            } else if lead & 0xF8 == 0xF0 {
                4
            } else {
                1 // invalid lead — treat as 1-byte to make progress
            };
            cut < probe + expected_len
        } {
            cut -= 1;
        }
        // Also handle the case where the byte at cut-1 is itself a lead with
        // outstanding continuations beyond `cut`.
        if cut > 0 {
            let lead = input[cut - 1];
            let expected_len = if lead & 0x80 == 0 {
                1
            } else if lead & 0xE0 == 0xC0 {
                2
            } else if lead & 0xF0 == 0xE0 {
                3
            } else if lead & 0xF8 == 0xF0 {
                4
            } else {
                1
            };
            if expected_len > 1 && cut < (cut - 1) + expected_len {
                cut -= 1;
            }
        }
        bytes[..cut].copy_from_slice(&input[..cut]);
        Self {
            bytes,
            len: cut as u8,
        }
    }

    /// View the title as a byte slice of length `self.len`.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }
}

/// Coarse content classification used by the compositor for layout hints.
///
/// Phase 7 M24: stored on each surface but not yet acted on. Layer 2 (smart
/// desktop) consumes this to drive context-aware layout in later phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SurfaceContentType {
    /// Text-heavy reading/editing surface.
    Document = 0,
    /// Monospace terminal emulator.
    Terminal = 1,
    /// Web content renderer.
    Browser = 2,
    /// Real-time interactive content (low-latency, fullscreen-friendly).
    Game = 3,
    /// System or application preferences.
    Settings = 4,
    /// System chrome (taskbar, status strip, launcher).
    SystemUI = 5,
    /// Unspecified / generic.
    Generic = 6,
}

impl SurfaceContentType {
    /// Convert from a raw u8 discriminant. Returns `None` on unknown values.
    pub const fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(Self::Document),
            1 => Some(Self::Terminal),
            2 => Some(Self::Browser),
            3 => Some(Self::Game),
            4 => Some(Self::Settings),
            5 => Some(Self::SystemUI),
            6 => Some(Self::Generic),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Damage tracking
// ---------------------------------------------------------------------------

/// Region of a surface buffer that changed since the last submission.
///
/// Phase 7 supports a single rectangle, full-surface, and empty. Multi-rect
/// damage lists are a Phase 25 optimization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageRegion {
    /// A rectangular subregion changed (surface-local pixel coordinates).
    Rect {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
    /// The entire surface changed (full repaint).
    FullSurface,
    /// Nothing changed since the last frame; compositor skips this surface.
    Empty,
}

impl DamageRegion {
    /// Returns true if this region encompasses any pixels.
    pub const fn has_damage(&self) -> bool {
        match self {
            Self::Empty => false,
            Self::Rect { width, height, .. } => *width > 0 && *height > 0,
            Self::FullSurface => true,
        }
    }
}

/// Screen-space damage accumulator used by the compositor during one frame.
///
/// Layer 1: a single bounding rect per output. Multi-rect dirty lists are
/// deferred. The accumulator unions every reported rect into one bounding box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DamageTracker {
    /// Inclusive damage bounds; `None` means no damage this frame.
    bounds: Option<DamageRect>,
    /// Whether a full-output redraw is pending (suppresses the bounding rect).
    full: bool,
}

/// Inclusive rectangle used by the screen-space damage accumulator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DamageRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl DamageTracker {
    /// Empty tracker — nothing damaged.
    pub const fn new() -> Self {
        Self {
            bounds: None,
            full: false,
        }
    }

    /// Returns true if any damage is recorded for this frame.
    pub const fn has_damage(&self) -> bool {
        self.full || self.bounds.is_some()
    }

    /// Returns true if a full-output redraw is pending.
    pub const fn is_full(&self) -> bool {
        self.full
    }

    /// Mark the entire output as damaged.
    pub fn mark_full(&mut self) {
        self.full = true;
        self.bounds = None;
    }

    /// Add a rectangular damage region (in output coordinates).
    ///
    /// Unioned with any previously added regions. No-op when `mark_full` was
    /// called earlier in the same frame.
    pub fn union(&mut self, rect: DamageRect) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        if self.full {
            return;
        }
        self.bounds = Some(match self.bounds {
            None => rect,
            Some(existing) => union_rect(existing, rect),
        });
    }

    /// Current bounding rect, if any. Returns `None` when `is_full()`.
    pub const fn bounds(&self) -> Option<DamageRect> {
        if self.full {
            None
        } else {
            self.bounds
        }
    }

    /// Reset to the empty state at the start of a new frame.
    pub fn clear(&mut self) {
        self.bounds = None;
        self.full = false;
    }
}

impl Default for DamageTracker {
    fn default() -> Self {
        Self::new()
    }
}

const fn union_rect(a: DamageRect, b: DamageRect) -> DamageRect {
    let x0 = if a.x < b.x { a.x } else { b.x };
    let y0 = if a.y < b.y { a.y } else { b.y };
    let a_x1 = a.x + a.width;
    let a_y1 = a.y + a.height;
    let b_x1 = b.x + b.width;
    let b_y1 = b.y + b.height;
    let x1 = if a_x1 > b_x1 { a_x1 } else { b_x1 };
    let y1 = if a_y1 > b_y1 { a_y1 } else { b_y1 };
    DamageRect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    }
}

// ---------------------------------------------------------------------------
// IPC protocol — request/response/event wire format
// ---------------------------------------------------------------------------

/// Compositor command discriminants for IPC requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum CompositorCommand {
    /// Allocate a new surface; compositor returns SurfaceId in the response.
    CreateSurface = 1,
    /// Attach a shared-memory buffer to an existing surface.
    AttachBuffer = 2,
    /// Tear down a surface and release any associated tracking.
    DestroySurface = 3,
    /// Request a new size — compositor responds with a Configure event.
    Resize = 4,
    /// Move a surface to a different z-order layer.
    SetLayer = 5,
}

impl CompositorCommand {
    /// Convert from a raw u32 discriminant.
    pub const fn from_u32(val: u32) -> Option<Self> {
        match val {
            1 => Some(Self::CreateSurface),
            2 => Some(Self::AttachBuffer),
            3 => Some(Self::DestroySurface),
            4 => Some(Self::Resize),
            5 => Some(Self::SetLayer),
            _ => None,
        }
    }
}

/// Compositor request — fixed-size, repr(C), serialized into RawMessage.data.
///
/// All fields are always present; unused fields are zeroed. Discriminate via
/// `command` and read only the fields relevant to that command. Explicit
/// `_pad_*` fields make every byte named so the entire struct is fully
/// initialized when serialized to bytes for IPC transport (no implicit
/// padding to leak uninitialized memory).
#[derive(Clone, Copy)]
#[repr(C)]
pub struct CompositorRequest {
    /// Command discriminant (matches `CompositorCommand`).
    pub command: u32,
    /// Explicit padding so `surface_id` lands at offset 8 (its natural u64 alignment).
    pub _pad_command: [u8; 4],
    /// SurfaceId.0 (for AttachBuffer, DestroySurface, Resize, SetLayer; 0 on CreateSurface).
    pub surface_id: u64,
    /// Surface width (CreateSurface, Resize).
    pub width: u32,
    /// Surface height (CreateSurface, Resize).
    pub height: u32,
    /// Initial layer (CreateSurface, SetLayer) — `SurfaceLayer as u8`.
    pub layer: u8,
    /// Initial content type (CreateSurface) — `SurfaceContentType as u8`.
    pub content_type: u8,
    /// Damage region tag (AttachBuffer): 0 = Empty, 1 = FullSurface, 2 = Rect.
    pub damage_tag: u8,
    /// Padding to align `title` to a 1-byte boundary (no-op; documents intent).
    pub _pad: u8,
    /// Title bytes (CreateSurface). Length carried in `title_len`.
    pub title: [u8; SURFACE_TITLE_MAX],
    /// Title byte count.
    pub title_len: u8,
    /// Padding to keep total size predictable.
    pub _pad2: [u8; 7],
    /// Shared memory region id (AttachBuffer).
    pub shmem_id: u32,
    /// Damage rect x (AttachBuffer when damage_tag=2).
    pub damage_x: u32,
    /// Damage rect y (AttachBuffer when damage_tag=2).
    pub damage_y: u32,
    /// Damage rect width (AttachBuffer when damage_tag=2).
    pub damage_w: u32,
    /// Damage rect height (AttachBuffer when damage_tag=2).
    pub damage_h: u32,
    /// Explicit padding so `client_channel` lands at offset 120 (its
    /// natural u64 alignment). Per M25's implicit-padding lesson, every
    /// byte of a `repr(C)` IPC struct must be named so serializing it
    /// to bytes never exposes uninitialized memory.
    pub _pad_damage: [u8; 4],
    /// M26 Step 28: per-client channel id. Set by `CreateSurface` to the
    /// caller-owned `ChannelId.0` so the compositor delivers events
    /// (Configure, Input, FocusChanged, BufferReleased, CloseRequested)
    /// directly to the client's receive endpoint instead of the
    /// well-known compositor channel. A value of `0` means "use the
    /// service channel" — preserves backward-compat for shell-internal
    /// surfaces that share the compositor's channel.
    pub client_channel: u64,
}

impl CompositorRequest {
    /// All-zero request (used as scratch buffer or builder base).
    pub const fn zeroed() -> Self {
        Self {
            command: 0,
            _pad_command: [0; 4],
            surface_id: 0,
            width: 0,
            height: 0,
            layer: 0,
            content_type: 0,
            damage_tag: 0,
            _pad: 0,
            title: [0; SURFACE_TITLE_MAX],
            title_len: 0,
            _pad2: [0; 7],
            shmem_id: 0,
            damage_x: 0,
            damage_y: 0,
            damage_w: 0,
            damage_h: 0,
            _pad_damage: [0; 4],
            client_channel: 0,
        }
    }

    /// Build a `CreateSurface` request.
    ///
    /// `client_channel` is the caller's per-client receive channel; the
    /// compositor stores it on the new surface and delivers events
    /// (Configure, Input, FocusChanged, …) to that channel. Pass `0` for
    /// shell-internal callers (Status Strip, Taskbar, Workspace) where
    /// the compositor is also the recipient — `0` is interpreted as
    /// "use the service channel" to preserve M25's self-channel
    /// suppression behaviour.
    pub fn create_surface(
        width: u32,
        height: u32,
        title: &[u8],
        layer: SurfaceLayer,
        content_type: SurfaceContentType,
        client_channel: u64,
    ) -> Self {
        let mut r = Self::zeroed();
        r.command = CompositorCommand::CreateSurface as u32;
        r.width = width;
        r.height = height;
        r.layer = layer as u8;
        r.content_type = content_type as u8;
        let cut = title.len().min(SURFACE_TITLE_MAX);
        r.title[..cut].copy_from_slice(&title[..cut]);
        r.title_len = cut as u8;
        r.client_channel = client_channel;
        r
    }

    /// Build an `AttachBuffer` request.
    pub fn attach_buffer(
        surface_id: SurfaceId,
        shmem_id: SharedMemoryId,
        damage: DamageRegion,
    ) -> Self {
        let mut r = Self::zeroed();
        r.command = CompositorCommand::AttachBuffer as u32;
        r.surface_id = surface_id.0;
        r.shmem_id = shmem_id.0;
        match damage {
            DamageRegion::Empty => {
                r.damage_tag = 0;
            }
            DamageRegion::FullSurface => {
                r.damage_tag = 1;
            }
            DamageRegion::Rect {
                x,
                y,
                width,
                height,
            } => {
                r.damage_tag = 2;
                r.damage_x = x;
                r.damage_y = y;
                r.damage_w = width;
                r.damage_h = height;
            }
        }
        r
    }

    /// Build a `DestroySurface` request.
    pub fn destroy_surface(surface_id: SurfaceId) -> Self {
        let mut r = Self::zeroed();
        r.command = CompositorCommand::DestroySurface as u32;
        r.surface_id = surface_id.0;
        r
    }

    /// Decode the damage region carried by this request (only meaningful for AttachBuffer).
    pub fn decode_damage(&self) -> DamageRegion {
        match self.damage_tag {
            0 => DamageRegion::Empty,
            1 => DamageRegion::FullSurface,
            2 => DamageRegion::Rect {
                x: self.damage_x,
                y: self.damage_y,
                width: self.damage_w,
                height: self.damage_h,
            },
            _ => DamageRegion::Empty,
        }
    }
}

/// Resolve the per-surface channel id from a `CompositorRequest`.
///
/// When `req.client_channel == 0` the request was built by a
/// shell-internal caller; the compositor stores the well-known service
/// channel on the surface, preserving M25's `is_self_channel`-suppression
/// behaviour. Otherwise the explicit per-client channel is used.
///
/// Pure function so the kernel-side IPC dispatcher and the host test
/// suite share the same logic.
pub const fn effective_channel(client_channel: u64, service_channel: ChannelId) -> ChannelId {
    if client_channel == 0 {
        service_channel
    } else {
        ChannelId(client_channel as u32)
    }
}

/// Event tag for compositor → client messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum CompositorEventTag {
    Configure = 1,
    FocusChanged = 2,
    CloseRequested = 3,
    BufferReleased = 4,
    FramePresented = 5,
    Input = 6,
}

impl CompositorEventTag {
    /// Convert from a raw u32 discriminant.
    pub const fn from_u32(val: u32) -> Option<Self> {
        match val {
            1 => Some(Self::Configure),
            2 => Some(Self::FocusChanged),
            3 => Some(Self::CloseRequested),
            4 => Some(Self::BufferReleased),
            5 => Some(Self::FramePresented),
            6 => Some(Self::Input),
            _ => None,
        }
    }
}

/// Compositor event — fixed-size, repr(C), sent from compositor to client via IPC.
///
/// The variant is identified by `tag` (a `CompositorEventTag`); only the
/// fields relevant to that variant are populated.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct CompositorEvent {
    /// Event variant tag.
    pub tag: u32,
    /// Explicit padding so `surface_id` lands at offset 8 (its natural u64
    /// alignment). Together with the other `_pad_*` fields below, this
    /// keeps the entire struct fully named so serializing it to bytes for
    /// IPC transport never exposes uninitialized memory.
    pub _pad_tag: [u8; 4],
    /// Surface id this event refers to (0 if not surface-bound).
    pub surface_id: u64,
    /// New surface width (Configure).
    pub width: u32,
    /// New surface height (Configure).
    pub height: u32,
    /// Display scale × 100 (Configure). 100 = 1.0×, 200 = 2.0×.
    pub scale_x100: u32,
    /// Focus state (FocusChanged): 0 = lost, 1 = gained.
    pub focused: u8,
    /// Padding for explicit alignment.
    pub _pad: [u8; 3],
    /// Released shared memory region id (BufferReleased).
    pub shmem_id: u32,
    /// Explicit padding so `timestamp_ticks` lands at its u64 alignment.
    pub _pad_shmem: [u8; 4],
    /// Frame presentation timestamp in 1 kHz timer ticks (FramePresented).
    pub timestamp_ticks: u64,
    /// Embedded input event (Input). Other variants leave this zeroed.
    pub input: InputEventBytes,
}

/// Wire-format byte buffer carrying a serialized `InputEvent`.
///
/// Decoded via `InputEventBytes::decode()`. Using a fixed byte buffer keeps
/// `CompositorEvent` `repr(C)` and avoids exposing `InputEvent`'s Rust enum
/// layout in the IPC protocol.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct InputEventBytes {
    pub tag: u32,
    pub key_or_buttons: u32,
    pub state_or_modifiers: u32,
    pub x: u32,
    pub y: u32,
    pub button_state: u32,
    pub _reserved: [u32; 2],
}

impl InputEventBytes {
    /// All-zero buffer.
    pub const fn zeroed() -> Self {
        Self {
            tag: 0,
            key_or_buttons: 0,
            state_or_modifiers: 0,
            x: 0,
            y: 0,
            button_state: 0,
            _reserved: [0; 2],
        }
    }

    /// Decode wire bytes back into an `InputEvent`.
    ///
    /// Returns `None` when the tag is unknown — the IPC framing
    /// already validated the byte length, but the embedded discriminants
    /// might still be garbage on a misframed message.
    pub fn decode(&self) -> Option<InputEvent> {
        use crate::input::{ButtonState, KeyState, Modifiers, MouseButton};
        match self.tag {
            1 => {
                let key = crate::input::KeyCode::from_evdev(self.key_or_buttons as u16);
                let state = KeyState::from_value(self.state_or_modifiers & 0xFF)?;
                let modifiers = Modifiers((self.state_or_modifiers >> 8) as u8);
                Some(InputEvent::Keyboard {
                    key,
                    state,
                    modifiers,
                })
            }
            2 => {
                let button = match self.key_or_buttons {
                    0 => None,
                    1 => Some(MouseButton::Left),
                    2 => Some(MouseButton::Right),
                    3 => Some(MouseButton::Middle),
                    _ => return None,
                };
                let bstate = match self.button_state {
                    0 => None,
                    1 => Some(ButtonState::Pressed),
                    2 => Some(ButtonState::Released),
                    _ => return None,
                };
                Some(InputEvent::Pointer {
                    x: self.x,
                    y: self.y,
                    button,
                    state: bstate,
                })
            }
            _ => None,
        }
    }

    /// Encode an `InputEvent` for IPC transport.
    ///
    /// Keyboard events store the evdev keycode (via `KeyCode::to_evdev`) so the
    /// wire format is a stable u16 rather than the Rust enum's discriminant.
    pub fn encode(event: &InputEvent) -> Self {
        let mut out = Self::zeroed();
        match event {
            InputEvent::Keyboard {
                key,
                state,
                modifiers,
            } => {
                out.tag = 1;
                out.key_or_buttons = key.to_evdev() as u32;
                out.state_or_modifiers = ((*state as u32) & 0xFF) | ((modifiers.0 as u32) << 8);
            }
            InputEvent::Pointer {
                x,
                y,
                button,
                state,
            } => {
                out.tag = 2;
                out.x = *x;
                out.y = *y;
                // Encode Option as 0 = None, button + 1 = Some(button).
                out.key_or_buttons = button.map(|b| b as u32 + 1).unwrap_or(0);
                out.button_state = state.map(|s| s as u32 + 1).unwrap_or(0);
            }
        }
        out
    }
}

impl CompositorEvent {
    /// All-zero event.
    pub const fn zeroed() -> Self {
        Self {
            tag: 0,
            _pad_tag: [0; 4],
            surface_id: 0,
            width: 0,
            height: 0,
            scale_x100: 0,
            focused: 0,
            _pad: [0; 3],
            shmem_id: 0,
            _pad_shmem: [0; 4],
            timestamp_ticks: 0,
            input: InputEventBytes::zeroed(),
        }
    }

    /// Build a Configure event.
    pub const fn configure(surface: SurfaceId, width: u32, height: u32, scale_x100: u32) -> Self {
        let mut e = Self::zeroed();
        e.tag = CompositorEventTag::Configure as u32;
        e.surface_id = surface.0;
        e.width = width;
        e.height = height;
        e.scale_x100 = scale_x100;
        e
    }

    /// Build a FocusChanged event.
    pub const fn focus_changed(surface: SurfaceId, focused: bool) -> Self {
        let mut e = Self::zeroed();
        e.tag = CompositorEventTag::FocusChanged as u32;
        e.surface_id = surface.0;
        e.focused = if focused { 1 } else { 0 };
        e
    }

    /// Build a CloseRequested event.
    pub const fn close_requested(surface: SurfaceId) -> Self {
        let mut e = Self::zeroed();
        e.tag = CompositorEventTag::CloseRequested as u32;
        e.surface_id = surface.0;
        e
    }

    /// Build a BufferReleased event.
    pub const fn buffer_released(surface: SurfaceId, shmem_id: SharedMemoryId) -> Self {
        let mut e = Self::zeroed();
        e.tag = CompositorEventTag::BufferReleased as u32;
        e.surface_id = surface.0;
        e.shmem_id = shmem_id.0;
        e
    }

    /// Build a FramePresented event.
    pub const fn frame_presented(surface: SurfaceId, timestamp_ticks: u64) -> Self {
        let mut e = Self::zeroed();
        e.tag = CompositorEventTag::FramePresented as u32;
        e.surface_id = surface.0;
        e.timestamp_ticks = timestamp_ticks;
        e
    }

    /// Build an Input event for delivery to a focused surface.
    pub fn input(surface: SurfaceId, event: &InputEvent) -> Self {
        let mut e = Self::zeroed();
        e.tag = CompositorEventTag::Input as u32;
        e.surface_id = surface.0;
        e.input = InputEventBytes::encode(event);
        e
    }
}

// ---------------------------------------------------------------------------
// Compositor-internal addressing
// ---------------------------------------------------------------------------

/// Owner reference attached to each surface — kernel side only.
///
/// Carries the originating process and the channel that delivers events
/// back to that process. Pure data so it lives in shared for host testing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceOwner {
    pub pid: ProcessId,
    pub channel: ChannelId,
}

// ---------------------------------------------------------------------------
// Shell text formatting helpers (M26 Step 24)
// ---------------------------------------------------------------------------

/// Format a millisecond-since-boot value as 5 ASCII bytes `HH:MM`.
///
/// Wraps modulo 24 hours, so any input maps onto a valid wall-clock-style
/// `HH:MM` string. The output is exactly 5 bytes (`b'0'..=b'9'` plus a
/// literal colon at index 2). Used by the Status Strip surface to display
/// the current time without pulling in `core::fmt`.
pub const fn format_hhmm(elapsed_ms: u64) -> [u8; 5] {
    const MS_PER_DAY: u64 = 24 * 60 * 60 * 1000;
    let wrapped = elapsed_ms % MS_PER_DAY;
    let total_minutes = wrapped / 60_000;
    let hours = (total_minutes / 60) as u32;
    let minutes = (total_minutes % 60) as u32;
    let mut out = [b'0'; 5];
    out[0] = b'0' + (hours / 10) as u8;
    out[1] = b'0' + (hours % 10) as u8;
    out[2] = b':';
    out[3] = b'0' + (minutes / 10) as u8;
    out[4] = b'0' + (minutes % 10) as u8;
    out
}

// ---------------------------------------------------------------------------
// Bare-Super edge detector (M26 Step 26)
// ---------------------------------------------------------------------------

/// Persistent state for `super_edge_step`.
///
/// Two flags suffice to recognize the bare-Super tap-and-release pattern:
///   * `prev_super_pressed` — `true` after a Super press, until release.
///   * `super_used_in_combo` — set when a non-Super key is pressed while
///     Super is held; cleared on the next Super press.
///
/// Pure data so the kernel can keep it in two `AtomicBool`s and the host
/// test suite can keep it in plain locals — `super_edge_step` reads it
/// by value and returns the next state along with any action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuperEdgeState {
    pub prev_super_pressed: bool,
    pub super_used_in_combo: bool,
}

impl SuperEdgeState {
    /// Initial state — Super not held, no combo recorded.
    pub const fn new() -> Self {
        Self {
            prev_super_pressed: false,
            super_used_in_combo: false,
        }
    }
}

impl Default for SuperEdgeState {
    fn default() -> Self {
        Self::new()
    }
}

/// Action emitted by `super_edge_step` on the bare-Super release edge.
///
/// Matches `HotkeyAction::ShowWorkspace` in `kernel/src/compositor/hotkey.rs`
/// in spirit; the shared crate doesn't depend on the hotkey enum, so we
/// just return a unit-style marker that the kernel maps over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuperEdgeAction {
    /// Bare-Super was tapped and released without any other key in between.
    ShowWorkspace,
}

/// Inspect a single keyboard event and update the edge-detection state.
///
/// Returns `(next_state, action)` — the updated state to store back into
/// the persistent statics and the action to apply (if any) on this
/// event. Pointer events should never reach this helper; the kernel
/// hotkey filter pre-filters by `InputEvent` variant.
///
/// Behavior table:
///
/// | Event                              | prev_pressed | combo  | Action          | Next prev | Next combo |
/// |------------------------------------|--------------|--------|-----------------|-----------|------------|
/// | Super press                        | (any)        | (any)  | None            | true      | false      |
/// | Super release, prev=true, combo=f  | true         | false  | ShowWorkspace   | false     | false      |
/// | Super release, prev=true, combo=t  | true         | true   | None            | false     | false      |
/// | Super release, prev=false          | false        | (any)  | None            | false     | (cleared)  |
/// | Other key press, Super held        | true         | (any)  | None            | true      | true       |
/// | Other key press, Super not held    | false        | (any)  | None            | false     | (unchanged)|
/// | Repeat / other release             | (any)        | (any)  | None            | (unchanged)| (unchanged)|
pub fn super_edge_step(
    state: SuperEdgeState,
    is_super_key: bool,
    is_press: bool,
    is_release: bool,
) -> (SuperEdgeState, Option<SuperEdgeAction>) {
    if is_super_key {
        if is_press {
            return (
                SuperEdgeState {
                    prev_super_pressed: true,
                    super_used_in_combo: false,
                },
                None,
            );
        }
        if is_release {
            let action = if state.prev_super_pressed && !state.super_used_in_combo {
                Some(SuperEdgeAction::ShowWorkspace)
            } else {
                None
            };
            return (
                SuperEdgeState {
                    prev_super_pressed: false,
                    super_used_in_combo: false,
                },
                action,
            );
        }
        // Repeat — leave state alone.
        return (state, None);
    }

    // Non-Super key: a press while Super is held marks this as a combo.
    if is_press && state.prev_super_pressed {
        return (
            SuperEdgeState {
                prev_super_pressed: true,
                super_used_in_combo: true,
            },
            None,
        );
    }
    (state, None)
}

/// Format a millisecond-since-boot value as 8 ASCII bytes `HH:MM:SS`.
///
/// Wraps modulo 24 hours (same convention as `format_hhmm`). The output
/// is exactly 8 bytes — six digits and two literal colons at indices 2
/// and 5. Used by the Workspace surface to display uptime.
pub const fn format_hhmmss(elapsed_ms: u64) -> [u8; 8] {
    const MS_PER_DAY: u64 = 24 * 60 * 60 * 1000;
    let wrapped = elapsed_ms % MS_PER_DAY;
    let total_seconds = wrapped / 1000;
    let hours = (total_seconds / 3600) as u32;
    let minutes = ((total_seconds % 3600) / 60) as u32;
    let seconds = (total_seconds % 60) as u32;
    let mut out = [b'0'; 8];
    out[0] = b'0' + (hours / 10) as u8;
    out[1] = b'0' + (hours % 10) as u8;
    out[2] = b':';
    out[3] = b'0' + (minutes / 10) as u8;
    out[4] = b'0' + (minutes % 10) as u8;
    out[5] = b':';
    out[6] = b'0' + (seconds / 10) as u8;
    out[7] = b'0' + (seconds % 10) as u8;
    out
}

/// Format an integer percent value (0..=99) as 2 ASCII digit bytes.
///
/// Values at or above 100 saturate to `99` so the result is always exactly
/// two digits. Used by the Status Strip for memory and CPU utilization
/// readouts where the trailing `%` glyph is rendered separately.
pub const fn format_percent_2digits(percent: u32) -> [u8; 2] {
    let clamped = if percent > 99 { 99 } else { percent };
    [b'0' + (clamped / 10) as u8, b'0' + (clamped % 10) as u8]
}

/// Format a small unsigned integer (0..=9999) as right-padded ASCII digits
/// inside a fixed-width 4-byte buffer (left-aligned, space-padded).
///
/// Used by the Status Strip core count display and similar bounded counters
/// where allocation-free integer formatting is required. Values above 9999
/// saturate to `9999`.
pub const fn format_u32_left4(value: u32) -> [u8; 4] {
    let v = if value > 9999 { 9999 } else { value };
    let mut out = [b' '; 4];
    if v >= 1000 {
        out[0] = b'0' + ((v / 1000) % 10) as u8;
        out[1] = b'0' + ((v / 100) % 10) as u8;
        out[2] = b'0' + ((v / 10) % 10) as u8;
        out[3] = b'0' + (v % 10) as u8;
    } else if v >= 100 {
        out[0] = b'0' + ((v / 100) % 10) as u8;
        out[1] = b'0' + ((v / 10) % 10) as u8;
        out[2] = b'0' + (v % 10) as u8;
    } else if v >= 10 {
        out[0] = b'0' + ((v / 10) % 10) as u8;
        out[1] = b'0' + (v % 10) as u8;
    } else {
        out[0] = b'0' + (v % 10) as u8;
    }
    out
}

// ---------------------------------------------------------------------------
// Taskbar layout (M26 Step 25)
// ---------------------------------------------------------------------------

/// Maximum number of taskbar entries laid out per frame. Layer 1 desktops
/// rarely show more than ~4 windows; 8 leaves headroom while keeping the
/// fixed-size array cheap to copy.
pub const MAX_TASKBAR_ENTRIES: usize = 8;

/// Width in pixels of the workspace button cell on the taskbar's left edge.
pub const TASKBAR_WORKSPACE_BUTTON_WIDTH: u32 = 40;

/// Default per-entry width before clipping. Each entry holds a truncated
/// surface title. Wide enough for ~24 8px glyphs after edge padding.
pub const TASKBAR_ENTRY_WIDTH: u32 = 200;

/// Reserved horizontal space for the right-anchored "N windows" count
/// readout. Wide enough for "8 windows" at the 8px cell width plus a small
/// margin on each side.
pub const TASKBAR_COUNT_RESERVED_WIDTH: u32 = 96;

/// One laid-out cell on the taskbar. Coordinates are in surface-local
/// pixels (origin at the taskbar's own top-left).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskbarCell {
    /// Left edge of the cell in surface-local pixels.
    pub x: i32,
    /// Cell width in pixels.
    pub width: u32,
}

/// Result of `compute_taskbar_layout` — fixed-capacity entry array plus
/// auxiliary cells for the workspace button and surface-count readout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskbarLayout {
    /// Cell occupied by the `[W]` workspace button.
    pub workspace_button: TaskbarCell,
    /// Per-entry cells. Only the first `visible_entries` slots are used.
    pub entries: [TaskbarCell; MAX_TASKBAR_ENTRIES],
    /// Number of `entries` slots that fit on this taskbar width.
    pub visible_entries: usize,
    /// Cell reserved for the right-anchored "N windows" count text.
    pub count_cell: TaskbarCell,
}

/// Lay out the taskbar's interactive cells for a given display width and
/// number of taskbar-eligible surfaces.
///
/// Pure function — no allocation, no dependence on kernel state — so it
/// can be unit-tested host-side. Callers that have more surfaces than
/// fit at the requested width receive a truncated layout (`visible_entries
/// < entry_count`); the remainder of the array is zeroed.
///
/// Layout (left → right):
///   * `workspace_button` at `x = 0`, fixed width
///     `TASKBAR_WORKSPACE_BUTTON_WIDTH`.
///   * Up to `MAX_TASKBAR_ENTRIES` entry cells starting at
///     `TASKBAR_WORKSPACE_BUTTON_WIDTH`, each `TASKBAR_ENTRY_WIDTH` wide.
///   * `count_cell` right-anchored at `display_width -
///     TASKBAR_COUNT_RESERVED_WIDTH`, fixed width
///     `TASKBAR_COUNT_RESERVED_WIDTH`.
pub const fn compute_taskbar_layout(display_width: u32, entry_count: usize) -> TaskbarLayout {
    let workspace_button = TaskbarCell {
        x: 0,
        width: TASKBAR_WORKSPACE_BUTTON_WIDTH,
    };

    // Right-anchored count cell: clamp at the workspace button's right edge
    // if the display is impossibly narrow so we never produce a negative x.
    let count_x = if display_width > TASKBAR_COUNT_RESERVED_WIDTH {
        display_width - TASKBAR_COUNT_RESERVED_WIDTH
    } else {
        TASKBAR_WORKSPACE_BUTTON_WIDTH
    };
    let count_cell = TaskbarCell {
        x: count_x as i32,
        width: TASKBAR_COUNT_RESERVED_WIDTH,
    };

    // Available space between the workspace button and the count cell.
    // Saturating: when the count cell sits at or before the workspace
    // button's right edge (extreme-narrow display), no entries fit.
    let entries_left = TASKBAR_WORKSPACE_BUTTON_WIDTH;
    let entries_right_limit = count_x;
    let available = entries_right_limit.saturating_sub(entries_left);
    let max_fit = (available / TASKBAR_ENTRY_WIDTH) as usize;

    // Visible entries: min(requested, fit, MAX).
    let mut visible = entry_count;
    if visible > max_fit {
        visible = max_fit;
    }
    if visible > MAX_TASKBAR_ENTRIES {
        visible = MAX_TASKBAR_ENTRIES;
    }

    let zero_cell = TaskbarCell { x: 0, width: 0 };
    let mut entries = [zero_cell; MAX_TASKBAR_ENTRIES];
    let mut i = 0;
    while i < visible {
        entries[i] = TaskbarCell {
            x: (entries_left + (i as u32) * TASKBAR_ENTRY_WIDTH) as i32,
            width: TASKBAR_ENTRY_WIDTH,
        };
        i += 1;
    }

    TaskbarLayout {
        workspace_button,
        entries,
        visible_entries: visible,
        count_cell,
    }
}

/// Truncate `title` to at most `max_chars` ASCII bytes for taskbar display.
///
/// Returns the longest prefix of `title` that fits in `max_chars` bytes.
/// Treats the title as opaque bytes (no UTF-8 boundary handling) — taskbar
/// glyph cells are 1 byte = 1 column at the spleen 8×16 cell width, so
/// the caller is responsible for passing ASCII-only titles. Non-ASCII
/// bytes are still returned as-is; the renderer's `?` fallback handles
/// them at the glyph level.
pub fn taskbar_entry_truncate(title: &[u8], max_chars: usize) -> &[u8] {
    let cut = if title.len() <= max_chars {
        title.len()
    } else {
        max_chars
    };
    &title[..cut]
}

/// Action a Taskbar pointer hit should produce.
///
/// `taskbar_pointer_action` resolves a click coordinate against a laid-out
/// `TaskbarLayout` plus the snapshot of currently-visible entries
/// (in the form of a parallel `SurfaceId` slice), returning what the
/// caller should do. Pure logic — host-tested, kernel-globals-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskbarPointerAction {
    /// User clicked the workspace button — toggle the Workspace surface.
    WorkspaceToggle,
    /// User clicked the Nth entry — focus that surface.
    FocusEntry(SurfaceId),
}

/// Resolve a pointer click on the Taskbar surface to an action.
///
/// `(local_x, local_y)` is the click position in **surface-local** pixel
/// coordinates (i.e., already translated by `(screen_x - surface.x,
/// screen_y - surface.y)`). `entry_ids[i]` corresponds to
/// `layout.entries[i]`; cells beyond `layout.visible_entries` and slots
/// beyond `entry_ids.len()` are ignored.
///
/// Returns `None` when the click missed every interactive cell (e.g.,
/// the count readout, dead space between cells, or out-of-bounds y).
/// The caller decides whether `None` means "consume silently" or
/// "forward to default handling" — Step 27 consumes silently because
/// the Taskbar is a Panel-layer surface and pass-through doesn't make
/// sense behind it.
pub fn taskbar_pointer_action(
    layout: &TaskbarLayout,
    entry_ids: &[SurfaceId],
    local_x: i32,
    local_y: i32,
) -> Option<TaskbarPointerAction> {
    // Reject clicks outside the surface's vertical band — defensive
    // against off-by-one routing where (local_y == surface.height) is
    // actually below the surface.
    if local_y < 0 {
        return None;
    }

    if cell_contains(&layout.workspace_button, local_x) {
        return Some(TaskbarPointerAction::WorkspaceToggle);
    }

    let visible = layout
        .visible_entries
        .min(MAX_TASKBAR_ENTRIES)
        .min(entry_ids.len());
    for (cell, id) in layout.entries.iter().zip(entry_ids.iter()).take(visible) {
        if cell_contains(cell, local_x) {
            if id.is_none() {
                return None;
            }
            return Some(TaskbarPointerAction::FocusEntry(*id));
        }
    }

    // count_cell is non-interactive in M26; clicks on it produce None.
    None
}

const fn cell_contains(cell: &TaskbarCell, x: i32) -> bool {
    x >= cell.x && x < cell.x + cell.width as i32
}

// ---------------------------------------------------------------------------
// Shell redraw decision (M26 Step 29)
// ---------------------------------------------------------------------------

/// Universal shell-surface redraw predicate.
///
/// Each shell surface (Status Strip, Taskbar, Workspace) caches its last
/// rendered snapshot and compares incoming snapshots against it on every
/// tick. If the snapshot is unchanged AND the surface has already
/// rendered at least one frame, the tick can short-circuit — no new
/// pixel writes, no `surface_attach_buffer` IPC, no `mark_damaged`.
/// `compose_frame` will then skip this surface entirely (its
/// `Surface::damaged` flag stays `false`), giving us the
/// "static desktop = 0 composes" property.
///
/// Pure function so the kernel-side tick code and host tests share the
/// same logic.
///
/// Returns `true` when the surface MUST redraw, `false` when the tick
/// can skip rendering and re-attaching.
pub const fn should_redraw_shell(needs_first_render: bool, snapshot_changed: bool) -> bool {
    needs_first_render || snapshot_changed
}

// ---------------------------------------------------------------------------
// Workspace spaces-list render decision (M26 Step 30)
// ---------------------------------------------------------------------------

/// Maximum number of space rows the Workspace renders.
///
/// Mirrors the kernel-side `kernel/src/compositor/shell/workspace.rs`
/// constant. Centralizing here lets host tests reason about the
/// truncation boundary without crossing the kernel-host divide.
pub const WORKSPACE_MAX_SPACES_RENDERED: u8 = 8;

/// What the Workspace renderer should draw for the spaces list given
/// the current snapshot. Captured as a small enum so the dispatch
/// (header → entries vs header → "(no spaces)") is a host-testable
/// pure function rather than a tangle of branches in the renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceRenderMode {
    /// Surface is hidden; renderer skips the spaces list entirely.
    Hidden,
    /// `space_list()` failed or returned zero entries — render
    /// `"(no spaces)"` placeholder.
    NoSpaces,
    /// Render `n` entries (always ≤ `WORKSPACE_MAX_SPACES_RENDERED`).
    WithSpaces(u8),
}

/// Decide what the Workspace renderer should draw given snapshot
/// inputs. Pure — host-testable, no kernel deps.
///
/// Truncates `raw_space_count` at `WORKSPACE_MAX_SPACES_RENDERED` so
/// the renderer's fixed-size snapshot array is always in bounds.
pub const fn workspace_render_mode(
    visible: bool,
    spaces_unavailable: bool,
    raw_space_count: u8,
) -> WorkspaceRenderMode {
    if !visible {
        return WorkspaceRenderMode::Hidden;
    }
    if spaces_unavailable || raw_space_count == 0 {
        return WorkspaceRenderMode::NoSpaces;
    }
    let capped = if raw_space_count > WORKSPACE_MAX_SPACES_RENDERED {
        WORKSPACE_MAX_SPACES_RENDERED
    } else {
        raw_space_count
    };
    WorkspaceRenderMode::WithSpaces(capped)
}

// ---------------------------------------------------------------------------
// Frame-window summary formatting (M26 Step 29)
// ---------------------------------------------------------------------------

/// Capacity of the buffer returned by `format_frame_window_summary`.
/// 64 bytes covers the worst-case payload — `frames=18446744073709551615
/// considered=18446744073709551615 idle=18446744073709551615/60
/// avg=18446744073709551615ms` is ~125 chars, but we cap displayed
/// values to 9999 (4 digits) so the actual layout is much smaller.
pub const FRAME_WINDOW_SUMMARY_BUF: usize = 64;

/// One window's worth of compositor stats — what gets logged every
/// `STATS_EVERY_FRAMES` (60) considered iterations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameWindow {
    /// Number of loop iterations past the 16ms cadence gate in this
    /// window. Equals `composes + idle` when the present flag is on;
    /// equals `idle + would-have-composed` when the flag is gated.
    pub considered: u64,
    /// Number of frames actually pushed through compose+present.
    /// Always 0 when `gated == true`.
    pub composes: u64,
    /// Number of frames that hit the cadence but had no damage (the
    /// fast idle path).
    pub idle: u64,
    /// Sum of compose+present durations in milliseconds for this
    /// window. Always 0 when `gated == true`.
    pub accum_ms: u64,
    /// `true` when `COMPOSITOR_PRESENT_ENABLED` was off for this
    /// window — the log line gets a `(gated)` prefix and `avg=` is
    /// suppressed because no presents ran.
    pub gated: bool,
}

impl FrameWindow {
    /// Empty / all-zero window — the initial state.
    pub const fn new(gated: bool) -> Self {
        Self {
            considered: 0,
            composes: 0,
            idle: 0,
            accum_ms: 0,
            gated,
        }
    }

    /// Average compose+present time per composed frame in milliseconds.
    /// Returns `0` when no frames composed in this window.
    pub const fn avg_compose_ms(&self) -> u64 {
        match self.accum_ms.checked_div(self.composes) {
            Some(avg) => avg,
            None => 0,
        }
    }
}

/// Format `window` into a fixed-size byte buffer suitable for `kinfo!`.
///
/// Layout when `gated == false`:
///   `frames=N considered=M idle=I/T avg=Ams`
///
/// Layout when `gated == true` (no compose ran):
///   `(gated) considered=M idle=I/T`
///
/// The returned slice is a prefix of `out` containing valid ASCII; the
/// caller passes it to `core::str::from_utf8_unchecked` (or the more
/// defensive `from_utf8(...).unwrap_or("")`) when feeding `kinfo!`.
/// Pure function with no allocation — host-testable.
pub fn format_frame_window_summary(
    window: &FrameWindow,
    window_size: u64,
    out: &mut [u8; FRAME_WINDOW_SUMMARY_BUF],
) -> usize {
    out.fill(0);
    let mut pos = 0usize;
    if window.gated {
        pos += copy_into(out, pos, b"(gated) considered=");
        pos += write_u64(out, pos, window.considered);
        pos += copy_into(out, pos, b" idle=");
        pos += write_u64(out, pos, window.idle);
        pos += copy_into(out, pos, b"/");
        pos += write_u64(out, pos, window_size);
    } else {
        pos += copy_into(out, pos, b"frames=");
        pos += write_u64(out, pos, window.composes);
        pos += copy_into(out, pos, b" considered=");
        pos += write_u64(out, pos, window.considered);
        pos += copy_into(out, pos, b" idle=");
        pos += write_u64(out, pos, window.idle);
        pos += copy_into(out, pos, b"/");
        pos += write_u64(out, pos, window_size);
        pos += copy_into(out, pos, b" avg=");
        pos += write_u64(out, pos, window.avg_compose_ms());
        pos += copy_into(out, pos, b"ms");
    }
    pos
}

/// Copy `src` into `dst[pos..]`, returning the number of bytes written.
/// Truncates silently if the destination is too small (only relevant
/// when the caller passed an undersized buffer; FRAME_WINDOW_SUMMARY_BUF
/// is sized to fit the worst case).
fn copy_into(dst: &mut [u8], pos: usize, src: &[u8]) -> usize {
    let avail = dst.len().saturating_sub(pos);
    let n = src.len().min(avail);
    dst[pos..pos + n].copy_from_slice(&src[..n]);
    n
}

/// Write `value` as decimal ASCII into `dst[pos..]`. Returns bytes
/// written. Saturates digit count at 20 (max u64 = 20 digits).
fn write_u64(dst: &mut [u8], pos: usize, value: u64) -> usize {
    let mut digits = [0u8; 20];
    let mut n = 0;
    let mut v = value;
    if v == 0 {
        digits[0] = b'0';
        n = 1;
    } else {
        while v > 0 && n < digits.len() {
            digits[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
    }
    let avail = dst.len().saturating_sub(pos);
    let to_copy = n.min(avail);
    // Reverse — digits[] is least-significant first.
    for i in 0..to_copy {
        dst[pos + i] = digits[n - 1 - i];
    }
    to_copy
}

// ---------------------------------------------------------------------------
// Compile-time invariants
// ---------------------------------------------------------------------------

const _: () = assert!(core::mem::size_of::<CompositorRequest>() <= MAX_MESSAGE_SIZE);
const _: () = assert!(core::mem::size_of::<CompositorEvent>() <= MAX_MESSAGE_SIZE);

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{ButtonState, KeyCode, KeyState, Modifiers, MouseButton};

    #[test]
    fn request_fits_in_message() {
        assert!(core::mem::size_of::<CompositorRequest>() <= MAX_MESSAGE_SIZE);
    }

    #[test]
    fn event_fits_in_message() {
        assert!(core::mem::size_of::<CompositorEvent>() <= MAX_MESSAGE_SIZE);
    }

    #[test]
    fn surface_state_terminal() {
        assert!(SurfaceState::Destroyed.is_terminal());
        assert!(!SurfaceState::Active.is_terminal());
        assert!(!SurfaceState::Created.is_terminal());
    }

    #[test]
    fn surface_state_transitions_forward_path() {
        assert!(SurfaceState::Created.can_transition_to(SurfaceState::Configured));
        assert!(SurfaceState::Configured.can_transition_to(SurfaceState::Active));
        assert!(SurfaceState::Active.can_transition_to(SurfaceState::Active));
    }

    #[test]
    fn surface_state_transitions_suspend() {
        assert!(SurfaceState::Active.can_transition_to(SurfaceState::Suspended));
        assert!(SurfaceState::Suspended.can_transition_to(SurfaceState::Active));
    }

    #[test]
    fn surface_state_transitions_destroy_from_anywhere() {
        for s in [
            SurfaceState::Created,
            SurfaceState::Configured,
            SurfaceState::Active,
            SurfaceState::Suspended,
        ] {
            assert!(s.can_transition_to(SurfaceState::Destroyed));
        }
    }

    #[test]
    fn surface_state_transitions_invalid() {
        // Skipping Configured.
        assert!(!SurfaceState::Created.can_transition_to(SurfaceState::Active));
        // Regressing from Destroyed.
        assert!(!SurfaceState::Destroyed.can_transition_to(SurfaceState::Active));
        // Regressing from Active to Configured.
        assert!(!SurfaceState::Active.can_transition_to(SurfaceState::Configured));
    }

    #[test]
    fn surface_layer_ordering() {
        assert!(SurfaceLayer::Background < SurfaceLayer::Normal);
        assert!(SurfaceLayer::Normal < SurfaceLayer::TopLevel);
        assert!(SurfaceLayer::TopLevel < SurfaceLayer::Overlay);
        assert!(SurfaceLayer::Overlay < SurfaceLayer::Panel);
    }

    #[test]
    fn surface_title_short_input() {
        let t = SurfaceTitle::from_bytes(b"hello");
        assert_eq!(t.len, 5);
        assert_eq!(t.as_bytes(), b"hello");
    }

    #[test]
    fn surface_title_truncation_ascii() {
        let long = [b'a'; 100];
        let t = SurfaceTitle::from_bytes(&long);
        assert_eq!(t.len as usize, SURFACE_TITLE_MAX);
        assert_eq!(t.as_bytes().len(), SURFACE_TITLE_MAX);
    }

    #[test]
    fn surface_title_utf8_boundary_safety() {
        // Construct: 60 ASCII bytes followed by a 4-byte UTF-8 emoji ("🎯" U+1F3AF).
        // SURFACE_TITLE_MAX is 64; the emoji starts at byte 60, would extend to byte 63
        // inclusive — that fits exactly. Add another byte past the limit.
        let mut input = [0u8; 100];
        for byte in input.iter_mut().take(60) {
            *byte = b'a';
        }
        let emoji = "🎯".as_bytes();
        input[60..60 + emoji.len()].copy_from_slice(emoji);
        // Now add 'b' starting at 64; that byte is past the limit.
        input[64] = b'b';
        let t = SurfaceTitle::from_bytes(&input[..65]);
        // The truncation must not split the emoji.
        let stored = t.as_bytes();
        // Either the emoji is fully present (len = 64) or fully absent (len = 60).
        assert!(stored.len() == 64 || stored.len() == 60);
        // If present, it must be valid UTF-8 (no torn codepoint).
        assert!(core::str::from_utf8(stored).is_ok());
    }

    #[test]
    fn surface_title_truncation_utf8_mid_codepoint() {
        // 62 ASCII bytes + 4-byte emoji = 66 bytes total. Truncation at 64 must
        // back up before the emoji to 62.
        let mut input = [0u8; 100];
        for byte in input.iter_mut().take(62) {
            *byte = b'a';
        }
        let emoji = "🎯".as_bytes();
        input[62..62 + emoji.len()].copy_from_slice(emoji);
        let t = SurfaceTitle::from_bytes(&input[..66]);
        // The emoji starts at 62 and extends to 65. SURFACE_TITLE_MAX = 64, so it
        // does not fit and the truncation must back up to 62.
        assert_eq!(t.len, 62);
        assert!(core::str::from_utf8(t.as_bytes()).is_ok());
    }

    #[test]
    fn surface_id_none_sentinel() {
        assert!(SurfaceId::NONE.is_none());
        assert!(!SurfaceId::FIRST.is_none());
    }

    #[test]
    fn damage_region_empty_has_no_damage() {
        assert!(!DamageRegion::Empty.has_damage());
    }

    #[test]
    fn damage_region_zero_size_rect_has_no_damage() {
        assert!(!DamageRegion::Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 5
        }
        .has_damage());
    }

    #[test]
    fn damage_region_full_has_damage() {
        assert!(DamageRegion::FullSurface.has_damage());
    }

    #[test]
    fn damage_region_rect_with_size_has_damage() {
        assert!(DamageRegion::Rect {
            x: 5,
            y: 5,
            width: 10,
            height: 10
        }
        .has_damage());
    }

    #[test]
    fn damage_tracker_starts_empty() {
        let t = DamageTracker::new();
        assert!(!t.has_damage());
        assert!(!t.is_full());
        assert!(t.bounds().is_none());
    }

    #[test]
    fn damage_tracker_union_bounds() {
        let mut t = DamageTracker::new();
        t.union(DamageRect {
            x: 10,
            y: 10,
            width: 20,
            height: 20,
        });
        t.union(DamageRect {
            x: 50,
            y: 5,
            width: 10,
            height: 10,
        });
        let b = t.bounds().expect("bounds set");
        // Bounding rect spans (10..60, 5..30).
        assert_eq!(b.x, 10);
        assert_eq!(b.y, 5);
        assert_eq!(b.width, 50);
        assert_eq!(b.height, 25);
    }

    #[test]
    fn damage_tracker_full_suppresses_bounds() {
        let mut t = DamageTracker::new();
        t.union(DamageRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        });
        t.mark_full();
        assert!(t.is_full());
        assert!(t.bounds().is_none());
        // Subsequent union is a no-op.
        t.union(DamageRect {
            x: 100,
            y: 100,
            width: 5,
            height: 5,
        });
        assert!(t.bounds().is_none());
    }

    #[test]
    fn damage_tracker_clear_resets() {
        let mut t = DamageTracker::new();
        t.mark_full();
        t.clear();
        assert!(!t.has_damage());
    }

    #[test]
    fn compositor_command_round_trip() {
        for c in [
            CompositorCommand::CreateSurface,
            CompositorCommand::AttachBuffer,
            CompositorCommand::DestroySurface,
            CompositorCommand::Resize,
            CompositorCommand::SetLayer,
        ] {
            assert_eq!(CompositorCommand::from_u32(c as u32), Some(c));
        }
        assert_eq!(CompositorCommand::from_u32(0), None);
        assert_eq!(CompositorCommand::from_u32(99), None);
    }

    #[test]
    fn compositor_event_tag_round_trip() {
        for t in [
            CompositorEventTag::Configure,
            CompositorEventTag::FocusChanged,
            CompositorEventTag::CloseRequested,
            CompositorEventTag::BufferReleased,
            CompositorEventTag::FramePresented,
            CompositorEventTag::Input,
        ] {
            assert_eq!(CompositorEventTag::from_u32(t as u32), Some(t));
        }
        assert_eq!(CompositorEventTag::from_u32(0), None);
    }

    #[test]
    fn surface_content_type_round_trip() {
        for t in [
            SurfaceContentType::Document,
            SurfaceContentType::Terminal,
            SurfaceContentType::Browser,
            SurfaceContentType::Game,
            SurfaceContentType::Settings,
            SurfaceContentType::SystemUI,
            SurfaceContentType::Generic,
        ] {
            assert_eq!(SurfaceContentType::from_u8(t as u8), Some(t));
        }
        assert_eq!(SurfaceContentType::from_u8(99), None);
    }

    #[test]
    fn compositor_request_decode_damage() {
        let mut r = CompositorRequest::zeroed();
        r.damage_tag = 0;
        assert_eq!(r.decode_damage(), DamageRegion::Empty);
        r.damage_tag = 1;
        assert_eq!(r.decode_damage(), DamageRegion::FullSurface);
        r.damage_tag = 2;
        r.damage_x = 5;
        r.damage_y = 10;
        r.damage_w = 100;
        r.damage_h = 50;
        assert_eq!(
            r.decode_damage(),
            DamageRegion::Rect {
                x: 5,
                y: 10,
                width: 100,
                height: 50
            }
        );
    }

    #[test]
    fn compositor_event_configure_constructor() {
        let e = CompositorEvent::configure(SurfaceId(7), 800, 600, 100);
        assert_eq!(e.tag, CompositorEventTag::Configure as u32);
        assert_eq!(e.surface_id, 7);
        assert_eq!(e.width, 800);
        assert_eq!(e.height, 600);
        assert_eq!(e.scale_x100, 100);
    }

    #[test]
    fn compositor_event_input_keyboard_round_trip() {
        let original = InputEvent::Keyboard {
            key: KeyCode::A,
            state: KeyState::Pressed,
            modifiers: Modifiers(Modifiers::SHIFT),
        };
        let e = CompositorEvent::input(SurfaceId(3), &original);
        assert_eq!(e.tag, CompositorEventTag::Input as u32);
        assert_eq!(e.surface_id, 3);
        assert_eq!(e.input.tag, 1);
        // Wire format carries evdev keycode (stable u16), not the Rust enum
        // discriminant. KeyCode::A maps to evdev KEY_A (30).
        assert_eq!(e.input.key_or_buttons, KeyCode::A.to_evdev() as u32);
        // state in low byte, modifiers in next byte.
        assert_eq!(e.input.state_or_modifiers & 0xFF, KeyState::Pressed as u32);
        assert_eq!(
            (e.input.state_or_modifiers >> 8) & 0xFF,
            Modifiers::SHIFT as u32
        );
    }

    // ---------------------------------------------------------------------
    // Multi-surface composition test (Phase 7 M24 Step 15)
    //
    // Exercises the same blit + z-order logic used by the kernel render
    // module against a stack-allocated framebuffer. The test creates three
    // surfaces at different layers and positions and verifies pixels at
    // representative coordinates after composition.
    // ---------------------------------------------------------------------

    /// Inline copy of the kernel's `blit_opaque` for host-side testing.
    /// Mirrors `kernel/src/compositor/render.rs::blit_opaque` byte-for-byte
    /// — any divergence here is a bug.
    fn host_blit_opaque(
        src: &[u32],
        src_w: u32,
        src_h: u32,
        dst: &mut [u32],
        dst_w: u32,
        dst_h: u32,
        dst_x: i32,
        dst_y: i32,
    ) -> Option<DamageRect> {
        if src_w == 0 || src_h == 0 {
            return None;
        }
        let dst_x_start = (dst_x as i64).max(0);
        let dst_y_start = (dst_y as i64).max(0);
        let dst_x_end = ((dst_x as i64) + src_w as i64).min(dst_w as i64);
        let dst_y_end = ((dst_y as i64) + src_h as i64).min(dst_h as i64);
        if dst_x_end <= dst_x_start || dst_y_end <= dst_y_start {
            return None;
        }
        let src_x = (dst_x_start - dst_x as i64) as usize;
        let src_y = (dst_y_start - dst_y as i64) as usize;
        let copy_w = (dst_x_end - dst_x_start) as usize;
        let copy_h = (dst_y_end - dst_y_start) as usize;
        for row in 0..copy_h {
            let s = (src_y + row) * src_w as usize + src_x;
            let d = (dst_y_start as usize + row) * dst_w as usize + dst_x_start as usize;
            dst[d..d + copy_w].copy_from_slice(&src[s..s + copy_w]);
        }
        Some(DamageRect {
            x: dst_x_start as u32,
            y: dst_y_start as u32,
            width: copy_w as u32,
            height: copy_h as u32,
        })
    }

    /// Test scene helper: a colored opaque rectangle.
    struct TestSurface {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        layer: SurfaceLayer,
        layer_seq: u64,
        color: u32,
    }

    fn make_pixels(width: u32, height: u32, color: u32) -> alloc::vec::Vec<u32> {
        alloc::vec![color; (width as usize) * (height as usize)]
    }

    #[test]
    fn multi_surface_composition_z_order() {
        // 64x32 framebuffer. Three surfaces:
        //   * background — 64x32 layer Background, dark gray
        //   * window     — 16x12 layer Normal at (10, 10), AIOS blue
        //   * overlay    — 8x6  layer Overlay at (20, 14), yellow
        const W: u32 = 64;
        const H: u32 = 32;
        const BG: u32 = 0xFF20_2020;
        const WINDOW: u32 = 0xFF5B_8CFF;
        const OVERLAY: u32 = 0xFFFF_D500;

        let mut dst: alloc::vec::Vec<u32> = alloc::vec![0; (W * H) as usize];
        let bg_pixels = make_pixels(W, H, BG);
        let window_pixels = make_pixels(16, 12, WINDOW);
        let overlay_pixels = make_pixels(8, 6, OVERLAY);

        // Build the scene unsorted to verify the sorting step.
        let mut scene = [
            TestSurface {
                x: 20,
                y: 14,
                width: 8,
                height: 6,
                layer: SurfaceLayer::Overlay,
                layer_seq: 3,
                color: OVERLAY,
            },
            TestSurface {
                x: 0,
                y: 0,
                width: W,
                height: H,
                layer: SurfaceLayer::Background,
                layer_seq: 1,
                color: BG,
            },
            TestSurface {
                x: 10,
                y: 10,
                width: 16,
                height: 12,
                layer: SurfaceLayer::Normal,
                layer_seq: 2,
                color: WINDOW,
            },
        ];

        // Sort by (layer, layer_seq) ascending — same key the kernel uses.
        scene.sort_by_key(|s| (s.layer as u8, s.layer_seq));

        let mut damage = DamageTracker::new();
        for surface in &scene {
            let pixels = match surface.color {
                BG => &bg_pixels,
                WINDOW => &window_pixels,
                OVERLAY => &overlay_pixels,
                _ => continue,
            };
            if let Some(rect) = host_blit_opaque(
                pixels,
                surface.width,
                surface.height,
                &mut dst,
                W,
                H,
                surface.x,
                surface.y,
            ) {
                damage.union(rect);
            }
        }

        // Verify damage tracker accumulated all three rectangles.
        let bounds = damage.bounds().expect("damage bounds set");
        assert_eq!(bounds.x, 0);
        assert_eq!(bounds.y, 0);
        assert_eq!(bounds.width, W);
        assert_eq!(bounds.height, H);

        // Pixel checks — corners and overlap regions.
        let pixel = |x: u32, y: u32| dst[(y * W + x) as usize];

        // Background fully visible at (0,0) and at (60, 30).
        assert_eq!(pixel(0, 0), BG, "background top-left");
        assert_eq!(pixel(60, 30), BG, "background bottom-right");

        // Window covers (10,10) through (25, 21).
        assert_eq!(pixel(10, 10), WINDOW, "window top-left");
        assert_eq!(pixel(15, 12), WINDOW, "window interior");

        // Overlay covers (20, 14) through (27, 19) — note this overlaps the
        // window region. Overlay z-orders above, so its color wins.
        assert_eq!(pixel(20, 14), OVERLAY, "overlay top-left (was window)");
        assert_eq!(pixel(25, 17), OVERLAY, "overlay interior");

        // Window x=10..26, y=10..22. Overlay x=20..28, y=14..20.
        // (28, 14): right of overlay AND right of window — must be BG.
        assert_eq!(pixel(28, 14), BG, "right-of-both edge falls to bg");
        // (24, 21): below overlay (overlay ends at y=20 exclusive) but
        // still inside window — must be WINDOW.
        assert_eq!(pixel(24, 21), WINDOW, "below-overlay still inside window");
    }

    #[test]
    fn blit_opaque_clips_left_edge() {
        let mut dst = alloc::vec![0u32; 16];
        let src = alloc::vec![0xFFFFFFFFu32; 4 * 1];
        // Blit a 4x1 source at dst_x = -2, dst_y = 0 into a 4x4 dst.
        let rect = host_blit_opaque(&src, 4, 1, &mut dst, 4, 4, -2, 0).expect("partial overlap");
        assert_eq!(rect.x, 0);
        assert_eq!(rect.y, 0);
        assert_eq!(rect.width, 2);
        assert_eq!(rect.height, 1);
        assert_eq!(dst[0], 0xFFFFFFFF);
        assert_eq!(dst[1], 0xFFFFFFFF);
        assert_eq!(dst[2], 0);
    }

    #[test]
    fn blit_opaque_off_screen_returns_none() {
        let mut dst = alloc::vec![0u32; 16];
        let src = alloc::vec![0xDEADBEEFu32; 4];
        // Source entirely off the right edge.
        assert!(host_blit_opaque(&src, 4, 1, &mut dst, 4, 4, 10, 0).is_none());
        // Source entirely off the top.
        assert!(host_blit_opaque(&src, 4, 1, &mut dst, 4, 4, 0, -5).is_none());
    }

    #[test]
    fn compositor_event_input_pointer_encoded() {
        let original = InputEvent::Pointer {
            x: 640,
            y: 400,
            button: Some(MouseButton::Left),
            state: Some(ButtonState::Pressed),
        };
        let e = CompositorEvent::input(SurfaceId(1), &original);
        assert_eq!(e.input.tag, 2);
        assert_eq!(e.input.x, 640);
        assert_eq!(e.input.y, 400);
        // Button is encoded as +1 to leave 0 for "no button".
        assert_eq!(e.input.key_or_buttons, MouseButton::Left as u32 + 1);
        assert_eq!(e.input.button_state, ButtonState::Pressed as u32 + 1);
    }

    // -----------------------------------------------------------------
    // M25 — hit-test geometry, z-order, focus history, route_event
    // -----------------------------------------------------------------

    fn deco() -> WindowDecoration {
        WindowDecoration::DEFAULT
    }

    #[test]
    fn surface_state_is_visible_only_for_active() {
        assert!(SurfaceState::Active.is_visible());
        assert!(!SurfaceState::Created.is_visible());
        assert!(!SurfaceState::Configured.is_visible());
        assert!(!SurfaceState::Suspended.is_visible());
        assert!(!SurfaceState::Destroyed.is_visible());
    }

    #[test]
    fn hit_zone_outside_returns_none() {
        let z = hit_zone(-1, -1, 0, 0, 100, 100, &deco());
        assert!(z.is_none());
        let z = hit_zone(100, 50, 0, 0, 100, 100, &deco()); // x_end exclusive
        assert!(z.is_none());
    }

    #[test]
    fn hit_zone_corners_are_resize_zones() {
        // 100x80 window at (10, 10), default decoration.
        let d = deco();
        let nw = hit_zone(10, 10, 10, 10, 100, 80, &d);
        assert_eq!(nw, Some(HitZone::ResizeBorder(ResizeEdge::NorthWest)));
        let ne = hit_zone(109, 10, 10, 10, 100, 80, &d);
        assert_eq!(ne, Some(HitZone::ResizeBorder(ResizeEdge::NorthEast)));
        let sw = hit_zone(10, 89, 10, 10, 100, 80, &d);
        assert_eq!(sw, Some(HitZone::ResizeBorder(ResizeEdge::SouthWest)));
        let se = hit_zone(109, 89, 10, 10, 100, 80, &d);
        assert_eq!(se, Some(HitZone::ResizeBorder(ResizeEdge::SouthEast)));
    }

    #[test]
    fn hit_zone_edges_distinguish_n_s_e_w() {
        let d = deco();
        // 200x120 window at (0,0); resize_margin=8 by default.
        assert_eq!(
            hit_zone(100, 2, 0, 0, 200, 120, &d),
            Some(HitZone::ResizeBorder(ResizeEdge::North))
        );
        assert_eq!(
            hit_zone(100, 117, 0, 0, 200, 120, &d),
            Some(HitZone::ResizeBorder(ResizeEdge::South))
        );
        assert_eq!(
            hit_zone(2, 60, 0, 0, 200, 120, &d),
            Some(HitZone::ResizeBorder(ResizeEdge::West))
        );
        assert_eq!(
            hit_zone(197, 60, 0, 0, 200, 120, &d),
            Some(HitZone::ResizeBorder(ResizeEdge::East))
        );
    }

    #[test]
    fn hit_zone_close_button_takes_precedence_over_title_bar() {
        let d = deco();
        // 200x120 window at (0,0). Title bar is in the top strip; close
        // button is the rightmost cell of the title bar (24 px wide).
        // Pointer at (190, 12): inside title bar, inside close button.
        let zone = hit_zone(190, 12, 0, 0, 200, 120, &d);
        assert_eq!(zone, Some(HitZone::CloseButton));
        // Pointer at (50, 12): inside title bar, NOT in close button.
        let zone = hit_zone(50, 12, 0, 0, 200, 120, &d);
        assert_eq!(zone, Some(HitZone::TitleBar));
    }

    #[test]
    fn hit_zone_content_when_not_decoration() {
        let d = deco();
        // 200x120 window at (0,0). Pointer at (100, 60): inside content.
        let zone = hit_zone(100, 60, 0, 0, 200, 120, &d);
        assert_eq!(zone, Some(HitZone::Content));
    }

    // ---- ZOrder ----

    #[test]
    fn z_order_push_iter_in_order() {
        let mut z = ZOrder::new();
        assert!(z.push(SurfaceId(1)));
        assert!(z.push(SurfaceId(2)));
        assert!(z.push(SurfaceId(3)));
        let collected: alloc::vec::Vec<_> = z.iter().collect();
        assert_eq!(
            collected,
            alloc::vec![SurfaceId(1), SurfaceId(2), SurfaceId(3)]
        );
    }

    #[test]
    fn z_order_iter_top_down_reverses() {
        let mut z = ZOrder::new();
        z.push(SurfaceId(1));
        z.push(SurfaceId(2));
        z.push(SurfaceId(3));
        let collected: alloc::vec::Vec<_> = z.iter_top_down().collect();
        assert_eq!(
            collected,
            alloc::vec![SurfaceId(3), SurfaceId(2), SurfaceId(1)]
        );
    }

    #[test]
    fn z_order_raise_to_top_moves_existing_entry() {
        let mut z = ZOrder::new();
        z.push(SurfaceId(1));
        z.push(SurfaceId(2));
        z.push(SurfaceId(3));
        z.raise_to_top(SurfaceId(1));
        let collected: alloc::vec::Vec<_> = z.iter().collect();
        assert_eq!(
            collected,
            alloc::vec![SurfaceId(2), SurfaceId(3), SurfaceId(1)]
        );
    }

    #[test]
    fn z_order_remove_existing() {
        let mut z = ZOrder::new();
        z.push(SurfaceId(1));
        z.push(SurfaceId(2));
        z.push(SurfaceId(3));
        z.remove(SurfaceId(2));
        let collected: alloc::vec::Vec<_> = z.iter().collect();
        assert_eq!(collected, alloc::vec![SurfaceId(1), SurfaceId(3)]);
        assert_eq!(z.len(), 2);
    }

    #[test]
    fn z_order_full_capacity_rejects_extra_push() {
        let mut z = ZOrder::new();
        for i in 1..=MAX_SURFACES as u64 {
            assert!(z.push(SurfaceId(i)));
        }
        assert!(!z.push(SurfaceId(999)));
    }

    #[test]
    fn z_order_rejects_none_id() {
        let mut z = ZOrder::new();
        assert!(!z.push(SurfaceId::NONE));
    }

    // ---- FocusHistory ----

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
        h.touch(SurfaceId(1));
        assert_eq!(h.most_recent(), Some(SurfaceId(1)));
        assert_eq!(h.nth(1), Some(SurfaceId(3)));
        assert_eq!(h.nth(2), Some(SurfaceId(2)));
        assert_eq!(h.len(), 3);
    }

    #[test]
    fn focus_history_evicts_lru_at_capacity() {
        let mut h = FocusHistory::new();
        for i in 1..=FOCUS_HISTORY_CAPACITY as u64 + 1 {
            h.touch(SurfaceId(i));
        }
        assert_eq!(h.len(), FOCUS_HISTORY_CAPACITY);
        assert!(!h.iter().any(|s| s == SurfaceId(1)));
        assert_eq!(
            h.most_recent(),
            Some(SurfaceId(FOCUS_HISTORY_CAPACITY as u64 + 1))
        );
    }

    #[test]
    fn focus_history_remove_purges_entry() {
        let mut h = FocusHistory::new();
        h.touch(SurfaceId(1));
        h.touch(SurfaceId(2));
        h.remove(SurfaceId(1));
        assert_eq!(h.len(), 1);
        assert_eq!(h.most_recent(), Some(SurfaceId(2)));
    }

    #[test]
    fn focus_history_rejects_none_id() {
        let mut h = FocusHistory::new();
        h.touch(SurfaceId::NONE);
        assert!(h.is_empty());
    }

    // ---- route_event ----

    #[test]
    fn route_event_keyboard_to_keyboard_focus() {
        let event = InputEvent::Keyboard {
            key: KeyCode::A,
            state: KeyState::Pressed,
            modifiers: Modifiers(0),
        };
        let target = route_event(&event, Some(SurfaceId(7)), None);
        assert_eq!(target, RouteTarget::Surface(SurfaceId(7)));
    }

    #[test]
    fn route_event_keyboard_no_focus_dropped() {
        let event = InputEvent::Keyboard {
            key: KeyCode::A,
            state: KeyState::Pressed,
            modifiers: Modifiers(0),
        };
        let target = route_event(&event, None, None);
        assert_eq!(target, RouteTarget::None);
    }

    #[test]
    fn route_event_pointer_content_to_surface() {
        let event = InputEvent::Pointer {
            x: 100,
            y: 100,
            button: None,
            state: None,
        };
        let hit = Some((SurfaceId(3), HitZone::Content));
        assert_eq!(
            route_event(&event, None, hit),
            RouteTarget::Surface(SurfaceId(3))
        );
    }

    #[test]
    fn route_event_pointer_decoration_routes_locally() {
        let event = InputEvent::Pointer {
            x: 100,
            y: 12,
            button: Some(MouseButton::Left),
            state: Some(ButtonState::Pressed),
        };
        let hit = Some((SurfaceId(3), HitZone::TitleBar));
        assert_eq!(
            route_event(&event, None, hit),
            RouteTarget::Decoration {
                surface: SurfaceId(3),
                zone: HitZone::TitleBar
            }
        );
    }

    #[test]
    fn route_event_pointer_no_hit_dropped() {
        let event = InputEvent::Pointer {
            x: 0,
            y: 0,
            button: None,
            state: None,
        };
        assert_eq!(route_event(&event, None, None), RouteTarget::None);
    }

    // ---- clamp_window_size ----

    #[test]
    fn clamp_window_size_enforces_minimum() {
        assert_eq!(
            clamp_window_size(50, 50),
            (MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT)
        );
        assert_eq!(clamp_window_size(800, 600), (800, 600));
    }

    #[test]
    fn clamp_window_size_at_exact_minimum_is_unchanged() {
        assert_eq!(
            clamp_window_size(MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT),
            (MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT)
        );
    }

    // ---- WindowDecoration::DEFAULT ----

    #[test]
    fn window_decoration_default_metrics() {
        let d = WindowDecoration::DEFAULT;
        assert_eq!(d.title_bar_height, 24);
        assert_eq!(d.border_width, 1);
        assert_eq!(d.close_button_width, 24);
        assert_eq!(d.resize_margin, 8);
    }

    // ---- Shell text formatting helpers (M26 Step 24) ----

    #[test]
    fn format_hhmm_at_zero_is_midnight() {
        assert_eq!(&format_hhmm(0), b"00:00");
    }

    #[test]
    fn format_hhmm_under_one_minute_stays_at_zero() {
        // 59_999 ms = 59.999 s — still inside the 00:00 minute.
        assert_eq!(&format_hhmm(59_999), b"00:00");
    }

    #[test]
    fn format_hhmm_one_hour_exact() {
        assert_eq!(&format_hhmm(60 * 60 * 1000), b"01:00");
    }

    #[test]
    fn format_hhmm_end_of_day() {
        // 23:59:59.999 — last millisecond before wrap.
        let ms = 24 * 60 * 60 * 1000 - 1;
        assert_eq!(&format_hhmm(ms), b"23:59");
    }

    #[test]
    fn format_hhmm_wraps_at_one_day() {
        // Exactly 24 h wraps back to 00:00.
        let ms = 24 * 60 * 60 * 1000;
        assert_eq!(&format_hhmm(ms), b"00:00");
    }

    #[test]
    fn format_hhmm_wraps_after_ten_days() {
        // 10 days plus 90 minutes — wrap should leave only 01:30.
        let ms = 10 * 24 * 60 * 60 * 1000 + 90 * 60 * 1000;
        assert_eq!(&format_hhmm(ms), b"01:30");
    }

    #[test]
    fn format_hhmmss_at_zero_is_midnight() {
        assert_eq!(&format_hhmmss(0), b"00:00:00");
    }

    #[test]
    fn format_hhmmss_one_second() {
        assert_eq!(&format_hhmmss(1000), b"00:00:01");
    }

    #[test]
    fn format_hhmmss_under_one_second_stays_at_zero() {
        // 999 ms still rounds down to 0 s.
        assert_eq!(&format_hhmmss(999), b"00:00:00");
    }

    #[test]
    fn format_hhmmss_one_minute_exact() {
        assert_eq!(&format_hhmmss(60_000), b"00:01:00");
    }

    #[test]
    fn format_hhmmss_one_hour_exact() {
        assert_eq!(&format_hhmmss(60 * 60 * 1000), b"01:00:00");
    }

    #[test]
    fn format_hhmmss_combo() {
        // 12h 34m 56s.
        let ms = (12 * 3600 + 34 * 60 + 56) * 1000;
        assert_eq!(&format_hhmmss(ms), b"12:34:56");
    }

    #[test]
    fn format_hhmmss_end_of_day() {
        // 23:59:59.999 — last millisecond before wrap.
        let ms = 24 * 60 * 60 * 1000 - 1;
        assert_eq!(&format_hhmmss(ms), b"23:59:59");
    }

    #[test]
    fn format_hhmmss_wraps_at_one_day() {
        let ms = 24 * 60 * 60 * 1000;
        assert_eq!(&format_hhmmss(ms), b"00:00:00");
    }

    #[test]
    fn format_percent_2digits_zero() {
        assert_eq!(&format_percent_2digits(0), b"00");
    }

    #[test]
    fn format_percent_2digits_single_digit_pads() {
        assert_eq!(&format_percent_2digits(7), b"07");
    }

    #[test]
    fn format_percent_2digits_two_digit() {
        assert_eq!(&format_percent_2digits(42), b"42");
    }

    #[test]
    fn format_percent_2digits_at_99() {
        assert_eq!(&format_percent_2digits(99), b"99");
    }

    #[test]
    fn format_percent_2digits_saturates_above_99() {
        assert_eq!(&format_percent_2digits(100), b"99");
        assert_eq!(&format_percent_2digits(u32::MAX), b"99");
    }

    #[test]
    fn format_u32_left4_single_digit() {
        assert_eq!(&format_u32_left4(4), b"4   ");
    }

    #[test]
    fn format_u32_left4_two_digits() {
        assert_eq!(&format_u32_left4(42), b"42  ");
    }

    #[test]
    fn format_u32_left4_three_digits() {
        assert_eq!(&format_u32_left4(987), b"987 ");
    }

    #[test]
    fn format_u32_left4_four_digits() {
        assert_eq!(&format_u32_left4(1234), b"1234");
    }

    #[test]
    fn format_u32_left4_saturates() {
        assert_eq!(&format_u32_left4(99_999), b"9999");
    }

    // ---- Taskbar layout (M26 Step 25) ----

    #[test]
    fn compute_taskbar_layout_workspace_button_always_first() {
        let layout = compute_taskbar_layout(1280, 0);
        assert_eq!(layout.workspace_button.x, 0);
        assert_eq!(
            layout.workspace_button.width,
            TASKBAR_WORKSPACE_BUTTON_WIDTH
        );
    }

    #[test]
    fn compute_taskbar_layout_count_cell_right_anchored() {
        let layout = compute_taskbar_layout(1280, 0);
        assert_eq!(
            layout.count_cell.x,
            (1280 - TASKBAR_COUNT_RESERVED_WIDTH) as i32
        );
        assert_eq!(layout.count_cell.width, TASKBAR_COUNT_RESERVED_WIDTH);
    }

    #[test]
    fn compute_taskbar_layout_zero_entries() {
        let layout = compute_taskbar_layout(1280, 0);
        assert_eq!(layout.visible_entries, 0);
        // Unused slots are zero-cells.
        assert_eq!(layout.entries[0], TaskbarCell { x: 0, width: 0 });
    }

    #[test]
    fn compute_taskbar_layout_one_entry_starts_after_workspace_button() {
        let layout = compute_taskbar_layout(1280, 1);
        assert_eq!(layout.visible_entries, 1);
        assert_eq!(layout.entries[0].x, TASKBAR_WORKSPACE_BUTTON_WIDTH as i32);
        assert_eq!(layout.entries[0].width, TASKBAR_ENTRY_WIDTH);
    }

    #[test]
    fn compute_taskbar_layout_packs_multiple_entries() {
        let layout = compute_taskbar_layout(1280, 4);
        assert_eq!(layout.visible_entries, 4);
        for (i, cell) in layout.entries.iter().take(4).enumerate() {
            let expected_x =
                TASKBAR_WORKSPACE_BUTTON_WIDTH as i32 + (i as i32) * TASKBAR_ENTRY_WIDTH as i32;
            assert_eq!(cell.x, expected_x);
            assert_eq!(cell.width, TASKBAR_ENTRY_WIDTH);
        }
    }

    #[test]
    fn compute_taskbar_layout_truncates_when_too_many() {
        // 1280 - 40 (button) - 96 (count) = 1144 → 5 entries fit at 200 wide.
        let layout = compute_taskbar_layout(1280, 12);
        assert_eq!(layout.visible_entries, 5);
    }

    #[test]
    fn compute_taskbar_layout_caps_at_max_entries() {
        // 4000 px is wide enough for 18 entries but the array max is 8.
        let layout = compute_taskbar_layout(4000, 100);
        assert_eq!(layout.visible_entries, MAX_TASKBAR_ENTRIES);
    }

    #[test]
    fn compute_taskbar_layout_narrow_display_drops_all_entries() {
        // Only just enough room for the workspace button + count cell.
        let layout = compute_taskbar_layout(
            TASKBAR_WORKSPACE_BUTTON_WIDTH + TASKBAR_COUNT_RESERVED_WIDTH,
            5,
        );
        assert_eq!(layout.visible_entries, 0);
    }

    #[test]
    fn compute_taskbar_layout_extreme_narrow_display_clamps_count_cell() {
        // Display narrower than the count cell — count_cell is parked at
        // the workspace button's right edge so x is never negative.
        let layout = compute_taskbar_layout(20, 3);
        assert_eq!(layout.visible_entries, 0);
        assert_eq!(layout.count_cell.x, TASKBAR_WORKSPACE_BUTTON_WIDTH as i32);
    }

    #[test]
    fn taskbar_entry_truncate_short_title_unchanged() {
        assert_eq!(taskbar_entry_truncate(b"app", 24), b"app");
    }

    #[test]
    fn taskbar_entry_truncate_exact_length() {
        assert_eq!(taskbar_entry_truncate(b"abcdef", 6), b"abcdef");
    }

    #[test]
    fn taskbar_entry_truncate_cuts_to_limit() {
        assert_eq!(
            taskbar_entry_truncate(b"a-very-long-window-title", 8),
            b"a-very-l"
        );
    }

    #[test]
    fn taskbar_entry_truncate_zero_max_chars() {
        assert_eq!(taskbar_entry_truncate(b"app", 0), b"");
    }

    #[test]
    fn taskbar_entry_truncate_empty_input() {
        assert_eq!(taskbar_entry_truncate(b"", 16), b"");
    }

    // ---- super_edge_step (M26 Step 26) ----

    /// Convenience: simulate a press of a Super key from a given state.
    fn super_press(state: SuperEdgeState) -> (SuperEdgeState, Option<SuperEdgeAction>) {
        super_edge_step(state, true, true, false)
    }

    /// Convenience: simulate a release of a Super key from a given state.
    fn super_release(state: SuperEdgeState) -> (SuperEdgeState, Option<SuperEdgeAction>) {
        super_edge_step(state, true, false, true)
    }

    /// Convenience: simulate a press of a non-Super key.
    fn other_press(state: SuperEdgeState) -> (SuperEdgeState, Option<SuperEdgeAction>) {
        super_edge_step(state, false, true, false)
    }

    /// Convenience: simulate a release of a non-Super key.
    fn other_release(state: SuperEdgeState) -> (SuperEdgeState, Option<SuperEdgeAction>) {
        super_edge_step(state, false, false, true)
    }

    #[test]
    fn super_edge_initial_state_no_action() {
        let s = SuperEdgeState::new();
        assert!(!s.prev_super_pressed);
        assert!(!s.super_used_in_combo);
    }

    #[test]
    fn super_edge_press_marks_held() {
        let (s, action) = super_press(SuperEdgeState::new());
        assert!(s.prev_super_pressed);
        assert!(!s.super_used_in_combo);
        assert_eq!(action, None);
    }

    #[test]
    fn super_edge_press_clears_combo_flag() {
        // Stale combo from a previous bad release path — a fresh press
        // must reset it so the next release can fire.
        let stale = SuperEdgeState {
            prev_super_pressed: false,
            super_used_in_combo: true,
        };
        let (s, _) = super_press(stale);
        assert!(s.prev_super_pressed);
        assert!(!s.super_used_in_combo);
    }

    #[test]
    fn super_edge_tap_release_fires_show_workspace() {
        let s = SuperEdgeState::new();
        let (s, _) = super_press(s);
        let (s, action) = super_release(s);
        assert_eq!(action, Some(SuperEdgeAction::ShowWorkspace));
        assert!(!s.prev_super_pressed);
        assert!(!s.super_used_in_combo);
    }

    #[test]
    fn super_edge_combo_press_then_release_does_not_fire() {
        // Super held → other key pressed → Super released ⇒ no action.
        let s = SuperEdgeState::new();
        let (s, _) = super_press(s);
        let (s, _) = other_press(s);
        assert!(s.super_used_in_combo);
        let (s, action) = super_release(s);
        assert_eq!(action, None);
        assert!(!s.prev_super_pressed);
        assert!(!s.super_used_in_combo);
    }

    #[test]
    fn super_edge_other_press_without_super_does_not_set_combo() {
        let (s, action) = other_press(SuperEdgeState::new());
        assert_eq!(action, None);
        assert!(!s.prev_super_pressed);
        assert!(!s.super_used_in_combo);
    }

    #[test]
    fn super_edge_other_release_is_a_noop() {
        let held = SuperEdgeState {
            prev_super_pressed: true,
            super_used_in_combo: false,
        };
        let (s, action) = other_release(held);
        assert_eq!(action, None);
        // Non-Super release leaves Super state untouched.
        assert_eq!(s, held);
    }

    #[test]
    fn super_edge_release_without_press_does_not_fire() {
        // Compositor came up with Super already physically released —
        // a stray release event must not phantom-toggle the workspace.
        let (s, action) = super_release(SuperEdgeState::new());
        assert_eq!(action, None);
        assert!(!s.prev_super_pressed);
        assert!(!s.super_used_in_combo);
    }

    #[test]
    fn super_edge_repeat_event_is_a_noop() {
        let held = SuperEdgeState {
            prev_super_pressed: true,
            super_used_in_combo: false,
        };
        // A Repeat event for the Super key — neither press nor release.
        let (s, action) = super_edge_step(held, true, false, false);
        assert_eq!(action, None);
        assert_eq!(s, held);
    }

    #[test]
    fn super_edge_two_taps_in_a_row_each_fire() {
        let s = SuperEdgeState::new();
        let (s, _) = super_press(s);
        let (s, action1) = super_release(s);
        assert_eq!(action1, Some(SuperEdgeAction::ShowWorkspace));
        let (s, _) = super_press(s);
        let (_, action2) = super_release(s);
        assert_eq!(action2, Some(SuperEdgeAction::ShowWorkspace));
    }

    #[test]
    fn super_edge_combo_then_clean_tap_fires() {
        // First Super+Tab combo (no fire), then a clean Super tap (fires).
        let s = SuperEdgeState::new();
        let (s, _) = super_press(s);
        let (s, _) = other_press(s);
        let (s, action_combo) = super_release(s);
        assert_eq!(action_combo, None);
        let (s, _) = super_press(s);
        let (_, action_clean) = super_release(s);
        assert_eq!(action_clean, Some(SuperEdgeAction::ShowWorkspace));
    }

    // ---- route_event_with_shell (M26 Step 27) ----

    #[test]
    fn route_event_keyboard_shell_focus_drops() {
        let event = InputEvent::Keyboard {
            key: KeyCode::A,
            state: KeyState::Pressed,
            modifiers: Modifiers(0),
        };
        // SurfaceId(99) is "the shell" for this test.
        let target =
            route_event_with_shell(&event, Some(SurfaceId(99)), None, |id| id == SurfaceId(99));
        assert_eq!(target, RouteTarget::None);
    }

    #[test]
    fn route_event_keyboard_non_shell_focus_routes() {
        let event = InputEvent::Keyboard {
            key: KeyCode::A,
            state: KeyState::Pressed,
            modifiers: Modifiers(0),
        };
        let target =
            route_event_with_shell(&event, Some(SurfaceId(7)), None, |id| id == SurfaceId(99));
        assert_eq!(target, RouteTarget::Surface(SurfaceId(7)));
    }

    #[test]
    fn route_event_pointer_on_shell_still_routes_to_surface() {
        // Pointer-on-shell goes to RouteTarget::Surface(id); the kernel
        // dispatcher branches on `is_shell` again to send it to the
        // shell pointer handlers. The shared router doesn't second-guess.
        let event = InputEvent::Pointer {
            x: 10,
            y: 10,
            button: Some(MouseButton::Left),
            state: Some(ButtonState::Pressed),
        };
        let hit = Some((SurfaceId(99), HitZone::Content));
        let target = route_event_with_shell(&event, None, hit, |id| id == SurfaceId(99));
        assert_eq!(target, RouteTarget::Surface(SurfaceId(99)));
    }

    #[test]
    fn route_event_default_predicate_unchanged_keyboard() {
        // Backward-compat: route_event() with no shell predicate behaves
        // exactly like before (delivers to keyboard focus).
        let event = InputEvent::Keyboard {
            key: KeyCode::A,
            state: KeyState::Pressed,
            modifiers: Modifiers(0),
        };
        assert_eq!(
            route_event(&event, Some(SurfaceId(7)), None),
            RouteTarget::Surface(SurfaceId(7))
        );
    }

    // ---- taskbar_pointer_action (M26 Step 27) ----

    #[test]
    fn taskbar_pointer_action_workspace_button() {
        // Click within the workspace button cell (x in [0, 40)).
        let layout = compute_taskbar_layout(1280, 0);
        let action = taskbar_pointer_action(&layout, &[], 8, 20);
        assert_eq!(action, Some(TaskbarPointerAction::WorkspaceToggle));
    }

    #[test]
    fn taskbar_pointer_action_workspace_button_right_edge() {
        // x = 39 is the last column inside the 40-px button.
        let layout = compute_taskbar_layout(1280, 0);
        let action = taskbar_pointer_action(&layout, &[], 39, 20);
        assert_eq!(action, Some(TaskbarPointerAction::WorkspaceToggle));
    }

    #[test]
    fn taskbar_pointer_action_workspace_button_just_past() {
        // x = 40 is the first column of the entry strip — not button.
        let layout = compute_taskbar_layout(1280, 1);
        let action = taskbar_pointer_action(&layout, &[SurfaceId(5)], 40, 20);
        assert_eq!(action, Some(TaskbarPointerAction::FocusEntry(SurfaceId(5))));
    }

    #[test]
    fn taskbar_pointer_action_first_entry() {
        let layout = compute_taskbar_layout(1280, 2);
        let entries = [SurfaceId(11), SurfaceId(12)];
        let action = taskbar_pointer_action(&layout, &entries, 100, 20);
        assert_eq!(
            action,
            Some(TaskbarPointerAction::FocusEntry(SurfaceId(11)))
        );
    }

    #[test]
    fn taskbar_pointer_action_second_entry() {
        let layout = compute_taskbar_layout(1280, 2);
        let entries = [SurfaceId(11), SurfaceId(12)];
        // First entry occupies x in [40, 240); second is [240, 440).
        let action = taskbar_pointer_action(&layout, &entries, 300, 20);
        assert_eq!(
            action,
            Some(TaskbarPointerAction::FocusEntry(SurfaceId(12)))
        );
    }

    #[test]
    fn taskbar_pointer_action_count_cell_returns_none() {
        // Count cell is non-interactive — clicks produce no action.
        let layout = compute_taskbar_layout(1280, 0);
        let click_x = layout.count_cell.x + 8;
        let action = taskbar_pointer_action(&layout, &[], click_x, 20);
        assert_eq!(action, None);
    }

    #[test]
    fn taskbar_pointer_action_negative_y_drops() {
        let layout = compute_taskbar_layout(1280, 1);
        let action = taskbar_pointer_action(&layout, &[SurfaceId(5)], 8, -1);
        assert_eq!(action, None);
    }

    #[test]
    fn taskbar_pointer_action_entry_beyond_visible_drops() {
        // Layout shows 2 entries, but the click lands on a cell that
        // would be entry index 3 — beyond visible_entries.
        let layout = compute_taskbar_layout(1280, 2);
        let entries = [SurfaceId(11), SurfaceId(12)];
        // Third entry would be at x in [440, 640) — well past visible.
        let action = taskbar_pointer_action(&layout, &entries, 500, 20);
        assert_eq!(action, None);
    }

    #[test]
    fn taskbar_pointer_action_none_id_drops() {
        // The snapshot's id slot is NONE (sentinel) — treat as no entry.
        let layout = compute_taskbar_layout(1280, 1);
        let action = taskbar_pointer_action(&layout, &[SurfaceId::NONE], 100, 20);
        assert_eq!(action, None);
    }

    #[test]
    fn taskbar_pointer_action_short_entry_slice_caps_visible() {
        // Layout claims 4 entries fit, but the caller only supplied 1
        // SurfaceId. Clicks on cells 2-4 must not index past the slice.
        let layout = compute_taskbar_layout(1280, 4);
        let entries = [SurfaceId(11)];
        // First entry cell — supplied → focus.
        assert_eq!(
            taskbar_pointer_action(&layout, &entries, 100, 20),
            Some(TaskbarPointerAction::FocusEntry(SurfaceId(11)))
        );
        // Second entry cell — out of supplied slice → None (no panic).
        assert_eq!(taskbar_pointer_action(&layout, &entries, 300, 20), None);
    }

    // ---- CompositorRequest::create_surface and effective_channel (M26 Step 28) ----

    #[test]
    fn compositor_request_size_under_message_limit() {
        // Adding `client_channel + _pad_damage` raises the size from the
        // pre-Step-28 footprint, but it must still fit in MAX_MESSAGE_SIZE.
        assert!(core::mem::size_of::<CompositorRequest>() <= MAX_MESSAGE_SIZE);
    }

    #[test]
    fn compositor_request_size_is_eight_byte_aligned() {
        // The struct's natural alignment is 8 (from u64 fields). Total
        // size must be a multiple of 8 so an array of requests has no
        // implicit gaps between elements.
        assert_eq!(core::mem::size_of::<CompositorRequest>() % 8, 0);
    }

    #[test]
    fn create_surface_factory_populates_fields() {
        let req = CompositorRequest::create_surface(
            400,
            300,
            b"test-app",
            SurfaceLayer::Normal,
            SurfaceContentType::Generic,
            42,
        );
        assert_eq!(req.command, CompositorCommand::CreateSurface as u32);
        assert_eq!(req.width, 400);
        assert_eq!(req.height, 300);
        assert_eq!(req.layer, SurfaceLayer::Normal as u8);
        assert_eq!(req.content_type, SurfaceContentType::Generic as u8);
        assert_eq!(&req.title[..req.title_len as usize], b"test-app");
        assert_eq!(req.client_channel, 42);
    }

    #[test]
    fn create_surface_factory_truncates_overlong_title() {
        let long = [b'a'; SURFACE_TITLE_MAX + 16];
        let req = CompositorRequest::create_surface(
            100,
            100,
            &long,
            SurfaceLayer::Normal,
            SurfaceContentType::Generic,
            0,
        );
        assert_eq!(req.title_len as usize, SURFACE_TITLE_MAX);
    }

    #[test]
    fn create_surface_factory_zero_channel_for_shell() {
        let req = CompositorRequest::create_surface(
            1280,
            32,
            b"status-strip",
            SurfaceLayer::Panel,
            SurfaceContentType::SystemUI,
            0,
        );
        assert_eq!(req.client_channel, 0);
    }

    #[test]
    fn attach_buffer_factory_full_damage() {
        let req = CompositorRequest::attach_buffer(
            SurfaceId(7),
            SharedMemoryId(3),
            DamageRegion::FullSurface,
        );
        assert_eq!(req.command, CompositorCommand::AttachBuffer as u32);
        assert_eq!(req.surface_id, 7);
        assert_eq!(req.shmem_id, 3);
        assert_eq!(req.damage_tag, 1);
    }

    #[test]
    fn attach_buffer_factory_rect_damage_round_trips() {
        let region = DamageRegion::Rect {
            x: 5,
            y: 10,
            width: 100,
            height: 50,
        };
        let req = CompositorRequest::attach_buffer(SurfaceId(7), SharedMemoryId(3), region);
        assert_eq!(req.damage_tag, 2);
        assert_eq!(req.decode_damage(), region);
    }

    #[test]
    fn destroy_surface_factory() {
        let req = CompositorRequest::destroy_surface(SurfaceId(9));
        assert_eq!(req.command, CompositorCommand::DestroySurface as u32);
        assert_eq!(req.surface_id, 9);
    }

    #[test]
    fn effective_channel_zero_falls_back_to_service() {
        let svc = ChannelId(1);
        assert_eq!(effective_channel(0, svc), svc);
    }

    #[test]
    fn effective_channel_nonzero_returns_client_channel() {
        let svc = ChannelId(1);
        assert_eq!(effective_channel(7, svc), ChannelId(7));
    }

    #[test]
    fn effective_channel_truncates_high_bits() {
        // Upper 32 bits of `client_channel` are reserved for future
        // endpoint encoding; the current ChannelId is u32 so we narrow.
        let svc = ChannelId(1);
        assert_eq!(
            effective_channel(0xFFFF_FFFF_0000_002A, svc),
            ChannelId(0x0000_002A)
        );
    }

    #[test]
    fn compositor_request_round_trip_via_bytes() {
        // Serialize → deserialize round-trip — exercises the M25
        // padding-UB rule: every byte of the struct must be named so
        // the bytes view is fully initialized.
        let original = CompositorRequest::create_surface(
            400,
            300,
            b"hello",
            SurfaceLayer::Normal,
            SurfaceContentType::Generic,
            123,
        );
        let bytes: &[u8] = unsafe {
            core::slice::from_raw_parts(
                (&original as *const CompositorRequest) as *const u8,
                core::mem::size_of::<CompositorRequest>(),
            )
        };
        let mut copy = CompositorRequest::zeroed();
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (&mut copy as *mut CompositorRequest) as *mut u8,
                bytes.len(),
            );
        }
        assert_eq!(copy.command, original.command);
        assert_eq!(copy.width, original.width);
        assert_eq!(copy.client_channel, original.client_channel);
        assert_eq!(&copy.title[..copy.title_len as usize], b"hello");
    }

    // ---- Shell redraw decision (M26 Step 29) ----

    #[test]
    fn should_redraw_shell_first_render_forces_yes() {
        // First render must always paint, even if the snapshot didn't
        // change (the cache is the all-zero default vs a real snapshot).
        assert!(should_redraw_shell(true, false));
        assert!(should_redraw_shell(true, true));
    }

    #[test]
    fn should_redraw_shell_unchanged_skips() {
        // Steady state: cache matches new snapshot → skip.
        assert!(!should_redraw_shell(false, false));
    }

    #[test]
    fn should_redraw_shell_changed_redraws() {
        // Cache differs from new snapshot → redraw.
        assert!(should_redraw_shell(false, true));
    }

    // ---- Workspace render mode (M26 Step 30) ----

    #[test]
    fn workspace_render_mode_hidden_dominates() {
        // Even with spaces available, hidden short-circuits.
        assert_eq!(
            workspace_render_mode(false, false, 3),
            WorkspaceRenderMode::Hidden
        );
        assert_eq!(
            workspace_render_mode(false, true, 0),
            WorkspaceRenderMode::Hidden
        );
    }

    #[test]
    fn workspace_render_mode_unavailable_renders_no_spaces() {
        // space_list() failed.
        assert_eq!(
            workspace_render_mode(true, true, 0),
            WorkspaceRenderMode::NoSpaces
        );
        // Even if a count survived, unavailable wins.
        assert_eq!(
            workspace_render_mode(true, true, 5),
            WorkspaceRenderMode::NoSpaces
        );
    }

    #[test]
    fn workspace_render_mode_zero_spaces_renders_placeholder() {
        // Storage returned an empty list.
        assert_eq!(
            workspace_render_mode(true, false, 0),
            WorkspaceRenderMode::NoSpaces
        );
    }

    #[test]
    fn workspace_render_mode_with_spaces_under_cap() {
        assert_eq!(
            workspace_render_mode(true, false, 1),
            WorkspaceRenderMode::WithSpaces(1)
        );
        assert_eq!(
            workspace_render_mode(true, false, 4),
            WorkspaceRenderMode::WithSpaces(4)
        );
    }

    #[test]
    fn workspace_render_mode_caps_at_max() {
        // Right at the cap.
        assert_eq!(
            workspace_render_mode(true, false, WORKSPACE_MAX_SPACES_RENDERED),
            WorkspaceRenderMode::WithSpaces(WORKSPACE_MAX_SPACES_RENDERED)
        );
        // Over the cap — truncated.
        assert_eq!(
            workspace_render_mode(true, false, WORKSPACE_MAX_SPACES_RENDERED + 1),
            WorkspaceRenderMode::WithSpaces(WORKSPACE_MAX_SPACES_RENDERED)
        );
        // Way over the cap — still truncated.
        assert_eq!(
            workspace_render_mode(true, false, 255),
            WorkspaceRenderMode::WithSpaces(WORKSPACE_MAX_SPACES_RENDERED)
        );
    }

    // ---- Frame window summary (M26 Step 29) ----

    fn summary_bytes(window: &FrameWindow, window_size: u64) -> alloc::string::String {
        let mut buf = [0u8; FRAME_WINDOW_SUMMARY_BUF];
        let n = format_frame_window_summary(window, window_size, &mut buf);
        alloc::string::String::from_utf8(buf[..n].to_vec()).expect("valid utf8")
    }

    #[test]
    fn frame_window_summary_active_with_composes() {
        let w = FrameWindow {
            considered: 60,
            composes: 12,
            idle: 48,
            accum_ms: 24,
            gated: false,
        };
        assert_eq!(
            summary_bytes(&w, 60),
            "frames=12 considered=60 idle=48/60 avg=2ms"
        );
    }

    #[test]
    fn frame_window_summary_active_zero_composes_avg_zero() {
        // No composes → avg should be 0 (avoid div-by-zero).
        let w = FrameWindow {
            considered: 60,
            composes: 0,
            idle: 60,
            accum_ms: 0,
            gated: false,
        };
        assert_eq!(
            summary_bytes(&w, 60),
            "frames=0 considered=60 idle=60/60 avg=0ms"
        );
    }

    #[test]
    fn frame_window_summary_gated_omits_avg() {
        let w = FrameWindow {
            considered: 60,
            composes: 0,
            idle: 30,
            accum_ms: 0,
            gated: true,
        };
        assert_eq!(summary_bytes(&w, 60), "(gated) considered=60 idle=30/60");
    }

    #[test]
    fn frame_window_avg_zero_composes_returns_zero() {
        let w = FrameWindow {
            considered: 0,
            composes: 0,
            idle: 0,
            accum_ms: 0,
            gated: false,
        };
        assert_eq!(w.avg_compose_ms(), 0);
    }

    #[test]
    fn frame_window_avg_division() {
        let w = FrameWindow {
            considered: 60,
            composes: 30,
            idle: 30,
            accum_ms: 90,
            gated: false,
        };
        assert_eq!(w.avg_compose_ms(), 3);
    }

    #[test]
    fn frame_window_summary_handles_large_counts() {
        // Ensure write_u64 handles realistic mid-range values without
        // truncation. ~one hour at 60Hz = 216000 considered.
        let w = FrameWindow {
            considered: 216_000,
            composes: 1234,
            idle: 214_766,
            accum_ms: 5678,
            gated: false,
        };
        assert_eq!(
            summary_bytes(&w, 60),
            "frames=1234 considered=216000 idle=214766/60 avg=4ms"
        );
    }
}
