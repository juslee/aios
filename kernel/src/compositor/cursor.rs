//! Software cursor — sprite + position tracking.
//!
//! The compositor draws a 16×16 software cursor on top of the composited
//! frame as the last operation before scanout. M25 uses a fixed arrow
//! sprite encoded as premultiplied-alpha A8R8G8B8 pixels matching the
//! `blit_alpha_premultiplied` blender's expectations
//! (kernel/src/compositor/render.rs).
//!
//! Per docs/platform/compositor/input.md §7.2.
//
// `CURSOR_POS` is updated by the input router (Step 20) and read by the
// composition loop (Step 14, kept gated behind `COMPOSITOR_PRESENT_ENABLED`).
// `render_cursor` is wired by the same composition path.
#![allow(dead_code)]

use spin::Mutex;

use super::render::blit_alpha_premultiplied;

// ---------------------------------------------------------------------------
// Cursor sprite — 16×16 premultiplied-alpha A8R8G8B8 ("0xAARRGGBB")
// ---------------------------------------------------------------------------

/// Sprite width in pixels.
pub const CURSOR_WIDTH: u32 = 16;
/// Sprite height in pixels.
pub const CURSOR_HEIGHT: u32 = 16;

const T: u32 = 0x0000_0000; // fully transparent
const W: u32 = 0xFFFF_FFFF; // opaque white (premultiplied: 0xFF FF FF FF)
const B: u32 = 0xFF00_0000; // opaque black outline

/// Standard arrow cursor sprite — top-left points to the hot spot at (0, 0).
///
/// White fill with a 1-pixel black outline. The sprite is laid out
/// row-major in scanline order so it can be fed directly to
/// `blit_alpha_premultiplied`.
#[rustfmt::skip]
pub const CURSOR_ARROW: [u32; (CURSOR_WIDTH * CURSOR_HEIGHT) as usize] = [
    B, T, T, T, T, T, T, T, T, T, T, T, T, T, T, T,
    B, B, T, T, T, T, T, T, T, T, T, T, T, T, T, T,
    B, W, B, T, T, T, T, T, T, T, T, T, T, T, T, T,
    B, W, W, B, T, T, T, T, T, T, T, T, T, T, T, T,
    B, W, W, W, B, T, T, T, T, T, T, T, T, T, T, T,
    B, W, W, W, W, B, T, T, T, T, T, T, T, T, T, T,
    B, W, W, W, W, W, B, T, T, T, T, T, T, T, T, T,
    B, W, W, W, W, W, W, B, T, T, T, T, T, T, T, T,
    B, W, W, W, W, W, W, W, B, T, T, T, T, T, T, T,
    B, W, W, W, W, W, B, B, B, T, T, T, T, T, T, T,
    B, W, W, B, W, W, B, T, T, T, T, T, T, T, T, T,
    B, W, B, T, B, W, W, B, T, T, T, T, T, T, T, T,
    B, B, T, T, B, W, W, B, T, T, T, T, T, T, T, T,
    B, T, T, T, T, B, W, W, B, T, T, T, T, T, T, T,
    T, T, T, T, T, B, W, W, B, T, T, T, T, T, T, T,
    T, T, T, T, T, T, B, B, T, T, T, T, T, T, T, T,
];

// ---------------------------------------------------------------------------
// Cursor position
// ---------------------------------------------------------------------------

/// Current cursor position in display coordinates, updated by the input
/// pipeline on each pointer event. Initialized to the centre of an
/// 800×600 default display; the first pointer event re-anchors it.
///
/// Lock ordering: leaf — never held while acquiring SURFACE_TABLE,
/// FOCUS_MANAGER, or any IPC global.
pub static CURSOR_POS: Mutex<(i32, i32)> = Mutex::new((400, 300));

/// Update the cursor position. Called from the input router (Step 20).
pub fn set_position(x: i32, y: i32) {
    let mut p = CURSOR_POS.lock();
    *p = (x, y);
}

/// Get the current cursor position.
pub fn position() -> (i32, i32) {
    *CURSOR_POS.lock()
}

// ---------------------------------------------------------------------------
// Cursor rendering
// ---------------------------------------------------------------------------

/// Composite the cursor sprite on top of `dst` at the current position.
/// Must be invoked AFTER all surface composition and BEFORE the present
/// transfer to the GPU — the cursor is on top of every other surface.
///
/// Returns the screen-space damage rect for the cursor draw, or `None`
/// when the cursor is fully off-screen.
pub fn render_cursor(
    dst: &mut [u32],
    dst_w: u32,
    dst_h: u32,
) -> Option<shared::compositor::DamageRect> {
    let (x, y) = position();
    blit_alpha_premultiplied(
        &CURSOR_ARROW,
        CURSOR_WIDTH,
        CURSOR_HEIGHT,
        dst,
        dst_w,
        dst_h,
        x,
        y,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
//
// Cursor sprite shape tests live in `shared::compositor::tests`
// (Step 23) where they execute under host-side `just test`. The
// kernel `CURSOR_POS` mutex itself is exercised via the integration
// path in M26.
