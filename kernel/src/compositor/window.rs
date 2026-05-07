//! Window manager — floating layout, z-order, and compositor-rendered decorations.
//!
//! M25 introduces the first piece of the compositor that the user can see and
//! interact with: title bars, focus borders, and a stable z-order list. The
//! compositor draws decorations on top of each surface's client buffer during
//! composition; clients never see the decoration pixels.
//!
//! Per docs/platform/compositor/rendering.md §6.1 (Layout Modes — floating only
//! for Layer 1).
//!
//! ## Lock ordering
//!
//! `WINDOW_Z_ORDER` and `DRAG_STATE` are leaf mutexes — never held while
//! acquiring `SURFACE_TABLE`, the IPC table, or any VirtIO leaf. The
//! z-order list is read by the compositor render path and mutated when a
//! surface is created, focused, or destroyed; both happen in the compositor
//! service thread, so contention is minimal.
//!
//! See [shared::compositor] for the pure data types ([`HitZone`],
//! [`ResizeEdge`], [`WindowDecoration`]) and the geometric `hit_zone()`
//! helper.
//
// Items in this module are introduced incrementally across M25 Steps 17–22.
// Each helper is referenced by a later step; the module-level `dead_code`
// allow keeps the build clean while the wiring lands in the same milestone.
#![allow(dead_code)]

use shared::compositor::{
    HitZone, ResizeEdge, SurfaceId, SurfaceLayer, WindowDecoration, MAX_SURFACES,
    MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH,
};
use spin::Mutex;

use super::surface::{Surface, SURFACE_TABLE};

// ---------------------------------------------------------------------------
// Z-order tracking
// ---------------------------------------------------------------------------

/// Most-recently-focused-last z-order list within a single layer.
///
/// Sorting at composition time is `(layer as u8, position-in-this-list)`.
/// The list stores `SurfaceId::NONE` in slots that have not yet been used,
/// and never reorders — `raise_to_top` removes the entry then pushes it
/// onto the end.
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

    /// Append a newly-created surface to the top of its layer.
    ///
    /// Returns `true` on success. Returns `false` only if the table is
    /// somehow full despite `MAX_SURFACES` headroom — that condition should
    /// already have been rejected at surface-create time.
    pub fn push(&mut self, id: SurfaceId) -> bool {
        if self.len >= MAX_SURFACES || id.is_none() {
            return false;
        }
        self.entries[self.len] = id;
        self.len += 1;
        true
    }

    /// Remove a surface from the z-order list (called on destroy).
    pub fn remove(&mut self, id: SurfaceId) {
        if let Some(pos) = self.entries[..self.len].iter().position(|&s| s == id) {
            for i in pos..self.len - 1 {
                self.entries[i] = self.entries[i + 1];
            }
            self.len -= 1;
            self.entries[self.len] = SurfaceId::NONE;
        }
    }

    /// Move a surface to the top of its layer (most-recently-focused last).
    pub fn raise_to_top(&mut self, id: SurfaceId) {
        if self.entries[..self.len].contains(&id) {
            self.remove(id);
            self.push(id);
        }
    }

    /// Iterate over surface ids in order (bottom of stack first → top last).
    pub fn iter(&self) -> impl Iterator<Item = SurfaceId> + '_ {
        self.entries[..self.len].iter().copied()
    }

    /// Iterate from top of stack down — used by hit-testing.
    pub fn iter_top_down(&self) -> impl Iterator<Item = SurfaceId> + '_ {
        self.entries[..self.len].iter().rev().copied()
    }

    pub fn len(&self) -> usize {
        self.len
    }
}

impl Default for ZOrder {
    fn default() -> Self {
        Self::new()
    }
}

/// Global z-order list. Lock ordering: leaf — never held across IPC or
/// VirtIO calls; specifically never held while `SURFACE_TABLE` is locked.
pub static WINDOW_Z_ORDER: Mutex<ZOrder> = Mutex::new(ZOrder::new());

// ---------------------------------------------------------------------------
// Default placement
// ---------------------------------------------------------------------------

/// Cascading offset between successive new windows so they do not stack
/// exactly atop one another.
const CASCADE_STEP: i32 = 24;

/// Maximum cascade distance before wrapping back to the centered position.
const CASCADE_WRAP: i32 = 8 * CASCADE_STEP;

/// Compute a centered position with a cascading offset based on `sequence`.
///
/// `sequence` is a monotonic counter (typically the surface's `layer_seq`)
/// so successive windows do not overlap exactly.
pub fn default_position(
    width: u32,
    height: u32,
    display_w: u32,
    display_h: u32,
    sequence: u64,
) -> (i32, i32) {
    let w = width as i32;
    let h = height as i32;
    let dw = display_w as i32;
    let dh = display_h as i32;
    let cx = ((dw - w) / 2).max(0);
    let cy = ((dh - h) / 2).max(0);
    let offset = ((sequence as i32) * CASCADE_STEP) % CASCADE_WRAP;
    (cx + offset, cy + offset)
}

// ---------------------------------------------------------------------------
// Drag state machine (used by Step 21 move/resize handlers)
// ---------------------------------------------------------------------------

/// Compositor drag mode — at most one window can be dragged at a time, so
/// this is a single global atomic state rather than a per-surface field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragState {
    /// No drag in progress.
    Idle,
    /// User is moving a window by its title bar.
    Moving {
        surface: SurfaceId,
        /// Pointer position when the drag started (display coords).
        start_pointer: (i32, i32),
        /// Surface origin when the drag started.
        start_origin: (i32, i32),
    },
    /// User is resizing a window by an edge or corner.
    Resizing {
        surface: SurfaceId,
        edge: ResizeEdge,
        /// Pointer position when the drag started (display coords).
        start_pointer: (i32, i32),
        /// Surface origin when the drag started.
        start_origin: (i32, i32),
        /// Surface dimensions when the drag started.
        start_dims: (u32, u32),
    },
}

impl DragState {
    pub const fn is_idle(self) -> bool {
        matches!(self, Self::Idle)
    }
}

/// Global drag state. Leaf mutex — never held while sending IPC events or
/// holding `SURFACE_TABLE`/`FOCUS_MANAGER`.
pub static DRAG_STATE: Mutex<DragState> = Mutex::new(DragState::Idle);

// ---------------------------------------------------------------------------
// Decoration rendering
// ---------------------------------------------------------------------------

/// Color for the title bar background of a focused window (B8G8R8A8).
pub const TITLE_BAR_FOCUSED_BG: u32 = 0xFF1E_2A48;
/// Color for the title bar background of an unfocused window.
pub const TITLE_BAR_UNFOCUSED_BG: u32 = 0xFF55_5A66;
/// Title text color.
pub const TITLE_BAR_FG: u32 = 0xFFEC_F0F8;
/// Border color for focused windows.
pub const BORDER_FOCUSED: u32 = 0xFF5B_8CFF;
/// Border color for unfocused windows.
pub const BORDER_UNFOCUSED: u32 = 0xFF7A_8190;
/// Close-button cross color.
pub const CLOSE_BUTTON_FG: u32 = 0xFFFF_FFFF;

/// Fill a rectangle in the destination framebuffer with a solid color.
/// Coordinates are clipped to the framebuffer bounds.
#[allow(clippy::too_many_arguments)]
pub fn fill_rect(
    dst: &mut [u32],
    dst_w: u32,
    dst_h: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    color: u32,
) {
    if width == 0 || height == 0 {
        return;
    }
    let dst_w_i = dst_w as i32;
    let dst_h_i = dst_h as i32;
    let x0 = x.max(0).min(dst_w_i);
    let y0 = y.max(0).min(dst_h_i);
    let x1 = (x.saturating_add(width as i32)).max(0).min(dst_w_i);
    let y1 = (y.saturating_add(height as i32)).max(0).min(dst_h_i);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let stride = dst_w as usize;
    for row in y0 as usize..y1 as usize {
        let start = row * stride + x0 as usize;
        let end = row * stride + x1 as usize;
        dst[start..end].fill(color);
    }
}

/// Render the decoration border around a surface's outer rectangle.
///
/// `(x, y, width, height)` is the **decorated** outer rectangle (i.e.
/// content surface plus decoration). Returns the four border rectangles
/// drawn so the caller can union them into the damage tracker.
#[allow(clippy::too_many_arguments)]
pub fn render_focus_indicator(
    dst: &mut [u32],
    dst_w: u32,
    dst_h: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    focused: bool,
    deco: &WindowDecoration,
) {
    let color = if focused {
        BORDER_FOCUSED
    } else {
        BORDER_UNFOCUSED
    };
    let border = deco.border_width;
    if border == 0 {
        return;
    }
    // Top
    fill_rect(dst, dst_w, dst_h, x, y, width, border, color);
    // Bottom
    fill_rect(
        dst,
        dst_w,
        dst_h,
        x,
        y + height as i32 - border as i32,
        width,
        border,
        color,
    );
    // Left
    fill_rect(dst, dst_w, dst_h, x, y, border, height, color);
    // Right
    fill_rect(
        dst,
        dst_w,
        dst_h,
        x + width as i32 - border as i32,
        y,
        border,
        height,
        color,
    );
}

/// Draw a small "X" close-button glyph centered in the close-button cell.
///
/// Uses two diagonal lines drawn as one-pixel dots — keeps the renderer
/// dependency-free of the spleen-font for the glyph (the glyph is too small
/// for the 16x32 font anyway).
#[allow(clippy::too_many_arguments)]
fn draw_close_glyph(
    dst: &mut [u32],
    dst_w: u32,
    dst_h: u32,
    cell_x: i32,
    cell_y: i32,
    cell_w: u32,
    cell_h: u32,
    color: u32,
) {
    let pad = (cell_w.min(cell_h) / 4).max(2) as i32;
    let inner_w = cell_w as i32 - 2 * pad;
    let inner_h = cell_h as i32 - 2 * pad;
    if inner_w <= 0 || inner_h <= 0 {
        return;
    }
    let span = inner_w.min(inner_h);
    let stride = dst_w as usize;
    for i in 0..span {
        let x_main = cell_x + pad + i;
        let y_main = cell_y + pad + i;
        let x_anti = cell_x + pad + (span - 1 - i);
        let y_anti = cell_y + pad + i;
        if (0..dst_w as i32).contains(&x_main) && (0..dst_h as i32).contains(&y_main) {
            dst[y_main as usize * stride + x_main as usize] = color;
        }
        if (0..dst_w as i32).contains(&x_anti) && (0..dst_h as i32).contains(&y_anti) {
            dst[y_anti as usize * stride + x_anti as usize] = color;
        }
    }
}

/// Render the title bar (background + title text + close button) on top of
/// the surface's content area in the destination framebuffer.
///
/// `(x, y)` is the decorated outer origin. `content_width`/`content_height`
/// are the content dimensions of the surface (decorations wrap them).
#[allow(clippy::too_many_arguments)]
pub fn render_title_bar(
    dst: &mut [u32],
    dst_w: u32,
    dst_h: u32,
    x: i32,
    y: i32,
    content_width: u32,
    focused: bool,
    title: &[u8],
    deco: &WindowDecoration,
) {
    let outer_w = content_width + 2 * deco.border_width;
    let bar_x = x;
    let bar_y = y + deco.border_width as i32;
    let bar_w = outer_w;
    let bar_h = deco.title_bar_height;

    let bg = if focused {
        TITLE_BAR_FOCUSED_BG
    } else {
        TITLE_BAR_UNFOCUSED_BG
    };

    fill_rect(dst, dst_w, dst_h, bar_x, bar_y, bar_w, bar_h, bg);

    // Title text — capped to fit before the close button cell.
    let close_w = deco.close_button_width as i32;
    let text_max_x = bar_x + bar_w as i32 - deco.border_width as i32 - close_w - 4;
    let text_x = bar_x + 8;
    let text_y = bar_y + (bar_h as i32 / 2) - 6;
    if text_max_x > text_x {
        super::text::draw_text_clipped(
            dst,
            dst_w,
            dst_h,
            text_x,
            text_y,
            text_max_x,
            title,
            TITLE_BAR_FG,
            bg,
        );
    }

    // Close button cell.
    let cell_x = bar_x + bar_w as i32 - deco.border_width as i32 - close_w;
    let cell_y = bar_y;
    draw_close_glyph(
        dst,
        dst_w,
        dst_h,
        cell_x,
        cell_y,
        deco.close_button_width,
        deco.title_bar_height,
        CLOSE_BUTTON_FG,
    );
}

// ---------------------------------------------------------------------------
// Decorated outer rectangle helpers
// ---------------------------------------------------------------------------

/// The decorated outer rectangle for `surface` given the default decoration.
///
/// Surfaces in the `Background`, `Overlay`, and `Panel` layers are rendered
/// without decorations (Background = wallpaper, Overlay = popovers,
/// Panel = system chrome). Only `Normal` and `TopLevel` are decorated.
pub fn outer_rect(surface: &Surface, deco: &WindowDecoration) -> (i32, i32, u32, u32) {
    if !is_decorated(surface) {
        return (surface.x, surface.y, surface.width, surface.height);
    }
    let outer_w = surface.width + 2 * deco.border_width;
    let outer_h = surface.height + 2 * deco.border_width + deco.title_bar_height;
    (surface.x, surface.y, outer_w, outer_h)
}

/// Returns true when `surface` should receive title-bar + border decorations.
pub const fn is_decorated(surface: &Surface) -> bool {
    matches!(surface.layer, SurfaceLayer::Normal | SurfaceLayer::TopLevel)
}

/// Clamp a candidate `(width, height)` resize to the minimum allowed
/// content dimensions. Returns the adjusted dimensions.
pub fn clamp_window_size(width: u32, height: u32) -> (u32, u32) {
    (width.max(MIN_WINDOW_WIDTH), height.max(MIN_WINDOW_HEIGHT))
}

// ---------------------------------------------------------------------------
// Walk z-order to find the surface whose decorated rectangle contains a point
// ---------------------------------------------------------------------------

/// Walk the global z-order from top to bottom and return the topmost surface
/// (and the zone within it) that contains `(px, py)`. Skips destroyed
/// surfaces, surfaces in `SurfaceState::Created` (no buffer yet), and
/// surfaces whose outer rectangle does not contain the point.
pub fn hit_test_topmost(px: i32, py: i32, deco: &WindowDecoration) -> Option<(SurfaceId, HitZone)> {
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
        if let Some(zone) = shared::compositor::hit_zone(px, py, x, y, w, h, deco) {
            return Some((surface.id, zone));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use shared::compositor::{SurfaceId, MAX_SURFACES};

    #[test]
    fn z_order_push_and_iter() {
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
    fn z_order_raise_to_top_moves_entry() {
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
    fn z_order_remove() {
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
    fn z_order_raise_unknown_is_noop() {
        let mut z = ZOrder::new();
        z.push(SurfaceId(1));
        z.raise_to_top(SurfaceId(99));
        let collected: alloc::vec::Vec<_> = z.iter().collect();
        assert_eq!(collected, alloc::vec![SurfaceId(1)]);
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
    fn default_position_centers_then_cascades() {
        // 200x100 window in a 1280x800 display.
        let (x0, y0) = default_position(200, 100, 1280, 800, 0);
        assert_eq!((x0, y0), (540, 350));
        // Sequence 1 cascades by CASCADE_STEP.
        let (x1, y1) = default_position(200, 100, 1280, 800, 1);
        assert_eq!((x1 - x0, y1 - y0), (CASCADE_STEP, CASCADE_STEP));
    }

    #[test]
    fn clamp_window_size_enforces_minimum() {
        assert_eq!(
            clamp_window_size(50, 50),
            (MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT)
        );
        assert_eq!(clamp_window_size(800, 600), (800, 600));
    }

    #[test]
    fn fill_rect_clips_at_bounds() {
        let mut dst = [0u32; 16];
        // 4x4 framebuffer; fill from (-1, -1) size (3, 3) → only (0,0)..(2,2) drawn.
        fill_rect(&mut dst, 4, 4, -1, -1, 3, 3, 0xAB);
        // Pixels (0,0), (1,0), (0,1), (1,1) should be set.
        assert_eq!(dst[0], 0xAB);
        assert_eq!(dst[1], 0xAB);
        assert_eq!(dst[4], 0xAB);
        assert_eq!(dst[5], 0xAB);
        // Pixel (2,2) untouched.
        assert_eq!(dst[10], 0);
    }
}
