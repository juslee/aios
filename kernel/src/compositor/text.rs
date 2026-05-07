//! Text rendering for compositor decorations.
//!
//! Wraps the spleen 8×16 PSF2 font for use against a `&mut [u32]` framebuffer
//! slice. Distinct from `kernel/src/gpu/text.rs`, which renders against a raw
//! `*mut u32` GPU framebuffer; the compositor composes into its own
//! double-buffered DMA buffer that is naturally addressable as a slice.
//!
//! The 8×16 size fits the 24-pixel title bar with 4 pixels of vertical
//! padding above and below.
//
// Wired up by Step 17 (`render_title_bar`) once `compose_frame` lands in
// the M25 main loop. Step 17 itself only declares the helpers.
#![allow(dead_code)]

use spin::Mutex;
use spleen_font::{PSF2Font, FONT_8X16};

/// Glyph cell width for the title-bar font (8×16 spleen).
pub const TITLE_GLYPH_WIDTH: i32 = 8;
/// Glyph cell height for the title-bar font.
pub const TITLE_GLYPH_HEIGHT: i32 = 16;

/// Cached spleen 8×16 font instance. The `PSF2Font` constructor allocates
/// a small lookup table so we keep one instance per kernel rather than
/// reconstructing it on every redraw.
///
/// Lock ordering: leaf — never held while acquiring `SURFACE_TABLE` or
/// `WINDOW_Z_ORDER`. Acquired only by the compositor render path.
static TITLE_FONT: Mutex<Option<PSF2Font<'static>>> = Mutex::new(None);

/// Lazily construct (or return the cached) title-bar font.
fn with_font<R>(f: impl FnOnce(&mut PSF2Font<'static>) -> R) -> Option<R> {
    let mut slot = TITLE_FONT.lock();
    if slot.is_none() {
        match PSF2Font::new(FONT_8X16) {
            Ok(font) => *slot = Some(font),
            Err(_) => return None,
        }
    }
    slot.as_mut().map(f)
}

/// Render a single ASCII character into a `&mut [u32]` framebuffer slice.
///
/// Clips against the framebuffer bounds and against `max_x` (the right-edge
/// cap supplied by the caller — typically the start of the close-button cell).
#[allow(clippy::too_many_arguments)]
fn blit_glyph(
    dst: &mut [u32],
    dst_w: u32,
    dst_h: u32,
    glyph_x: i32,
    glyph_y: i32,
    max_x: i32,
    ch: char,
    fg: u32,
    bg: u32,
) {
    let mut utf8 = [0u8; 4];
    let bytes = ch.encode_utf8(&mut utf8);

    with_font(|font| {
        let glyph = match font.glyph_for_utf8(bytes.as_bytes()) {
            Some(g) => g,
            None => match font.glyph_for_utf8(b"?") {
                Some(g) => g,
                None => return,
            },
        };
        let stride = dst_w as usize;
        for (row, glyph_row) in glyph.enumerate() {
            let py = glyph_y + row as i32;
            if py < 0 {
                continue;
            }
            if py >= dst_h as i32 {
                break;
            }
            for (col, pixel_on) in glyph_row.enumerate() {
                let px = glyph_x + col as i32;
                if px < 0 {
                    continue;
                }
                if px >= max_x || px >= dst_w as i32 {
                    break;
                }
                let color = if pixel_on { fg } else { bg };
                dst[py as usize * stride + px as usize] = color;
            }
        }
    });
}

/// Draw an ASCII byte string into `dst`, clipped against `max_x` and the
/// framebuffer bounds. Stops at the first byte past `max_x` or end-of-input.
///
/// Non-ASCII bytes (top bit set) are rendered via the `?` fallback. UTF-8
/// support is unnecessary for window titles in M25 — apps render their own
/// content and only the title bar uses this path.
#[allow(clippy::too_many_arguments)]
pub fn draw_text_clipped(
    dst: &mut [u32],
    dst_w: u32,
    dst_h: u32,
    start_x: i32,
    start_y: i32,
    max_x: i32,
    text: &[u8],
    fg: u32,
    bg: u32,
) {
    let mut x = start_x;
    for &byte in text {
        if x + TITLE_GLYPH_WIDTH > max_x {
            break;
        }
        let ch = if byte.is_ascii() { byte as char } else { '?' };
        blit_glyph(dst, dst_w, dst_h, x, start_y, max_x, ch, fg, bg);
        x += TITLE_GLYPH_WIDTH;
    }
}
