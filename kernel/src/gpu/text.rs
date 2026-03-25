//! Bitmap font text rendering for the GPU framebuffer.
//!
//! Uses spleen-font (16x32 PSF2 bitmap font) to render characters directly
//! to the VirtIO-GPU framebuffer. All rendering operates on raw `*mut u32`
//! pointers with pixel-stride addressing for decoupling from GPU Service types.

use spleen_font::{PSF2Font, FONT_16X32};

/// Glyph width in pixels (spleen 16x32).
pub const GLYPH_WIDTH: u32 = 16;

/// Glyph height in pixels (spleen 16x32).
pub const GLYPH_HEIGHT: u32 = 32;

/// Framebuffer parameters bundled for rendering functions.
pub struct FbInfo {
    /// Base pointer to u32 pixel data (B8G8R8A8 format).
    pub fb: *mut u32,
    /// Stride in pixels (byte stride / 4 for B8G8R8A8).
    pub stride_px: u32,
    /// Framebuffer width in pixels.
    pub width: u32,
    /// Framebuffer height in pixels.
    pub height: u32,
}

/// Render a single character glyph to the framebuffer.
///
/// Handles boundary clipping. Falls back to `'?'` for unknown characters.
fn blit_glyph(font: &mut PSF2Font, fb: &FbInfo, ch: char, x: i32, y: i32, fg: u32, bg: u32) {
    // Convert char to UTF-8 bytes for glyph lookup.
    let mut utf8_buf = [0u8; 4];
    let utf8_bytes = ch.encode_utf8(&mut utf8_buf);

    // Look up glyph; fall back to '?' for unknown characters.
    let glyph = match font.glyph_for_utf8(utf8_bytes.as_bytes()) {
        Some(g) => g,
        None => match font.glyph_for_utf8(b"?") {
            Some(g) => g,
            None => return,
        },
    };

    for (row, glyph_row) in glyph.enumerate() {
        let py = y + row as i32;
        if py < 0 {
            continue;
        }
        if py >= fb.height as i32 {
            break;
        }

        for (col, pixel_on) in glyph_row.enumerate() {
            let px = x + col as i32;
            if px < 0 {
                continue;
            }
            if px >= fb.width as i32 {
                break;
            }

            let color = if pixel_on { fg } else { bg };
            let offset = py as usize * fb.stride_px as usize + px as usize;

            // SAFETY: Bounds checked above — px in [0, fb.width) and py in [0, fb.height).
            // Caller guarantees fb.fb has at least stride_px * height elements.
            // Writing to wrong offset would corrupt adjacent framebuffer pixels.
            unsafe {
                fb.fb.add(offset).write(color);
            }
        }
    }
}

/// Render a text string to the framebuffer.
///
/// Handles newlines (`\n`) and automatic line wrapping at framebuffer edge.
/// Stops rendering when text would exceed the bottom of the framebuffer.
fn draw_text(
    font: &mut PSF2Font,
    fb: &FbInfo,
    text: &str,
    start_x: i32,
    start_y: i32,
    fg: u32,
    bg: u32,
) {
    let mut cx = start_x;
    let mut cy = start_y;

    for ch in text.chars() {
        if cy + GLYPH_HEIGHT as i32 > fb.height as i32 {
            break;
        }

        if ch == '\n' {
            cx = start_x;
            cy += GLYPH_HEIGHT as i32;
            continue;
        }

        if cx + GLYPH_WIDTH as i32 > fb.width as i32 {
            cx = start_x;
            cy += GLYPH_HEIGHT as i32;
            if cy + GLYPH_HEIGHT as i32 > fb.height as i32 {
                break;
            }
        }

        blit_glyph(font, fb, ch, cx, cy, fg, bg);
        cx += GLYPH_WIDTH as i32;
    }
}

/// Fill a rectangular region of the framebuffer with a solid color.
fn fill_rect(fb: &FbInfo, x: u32, y: u32, w: u32, h: u32, color: u32) {
    let x_end = (x + w).min(fb.width);
    let y_end = (y + h).min(fb.height);

    for row in y.min(fb.height)..y_end {
        let row_offset = row as usize * fb.stride_px as usize;
        for col in x.min(fb.width)..x_end {
            // SAFETY: row < fb.height and col < fb.width, within framebuffer allocation.
            // Caller ensures fb.fb is valid for stride_px * height elements.
            unsafe {
                fb.fb.add(row_offset + col as usize).write(color);
            }
        }
    }
}

/// Render the kernel boot log to the GPU framebuffer.
///
/// Fills the framebuffer with a dark background, draws a header, then
/// renders recent boot log entries captured by the observability subsystem.
/// The caller must ensure `FbInfo.fb` points to a valid framebuffer of at
/// least `stride_px * height` u32 elements.
pub fn draw_boot_log(fb: &FbInfo) {
    // Create font — if this fails, keep the existing AIOS blue screen.
    let mut font = match PSF2Font::new(FONT_16X32) {
        Ok(f) => f,
        Err(_) => {
            crate::kwarn!(Gpu, "spleen-font: failed to load FONT_16X32");
            return;
        }
    };

    // Fill entire framebuffer with dark background.
    fill_rect(fb, 0, 0, fb.width, fb.height, shared::gpu::BOOT_LOG_BG);

    // Draw header.
    draw_text(
        &mut font,
        fb,
        "AIOS Boot Log",
        GLYPH_WIDTH as i32,
        0,
        shared::gpu::BOOT_LOG_HEADER,
        shared::gpu::BOOT_LOG_BG,
    );

    // Calculate visible log area (below header).
    let log_y_start = (GLYPH_HEIGHT * 2) as i32;
    let max_lines = ((fb.height as i32 - log_y_start) / GLYPH_HEIGHT as i32).max(0) as usize;
    let max_lines = max_lines.min(crate::observability::MAX_LOG_LINES);

    // Retrieve boot log entries.
    let mut lines =
        [[0u8; crate::observability::MAX_LINE_LEN]; crate::observability::MAX_LOG_LINES];
    let mut lens = [0u8; crate::observability::MAX_LOG_LINES];
    let count = crate::observability::take_boot_log(&mut lines, &mut lens);

    crate::kinfo!(
        Gpu,
        "Boot log: {} entries captured, max_lines={}",
        count,
        max_lines
    );

    // Render entries from the beginning of boot (most interesting).
    // Interactive scrolling is a Phase 7+ compositor feature.
    let display_count = count.min(max_lines);

    for i in 0..display_count {
        let len = lens[i] as usize;
        if len == 0 {
            continue;
        }
        let line_str = core::str::from_utf8(&lines[i][..len]).unwrap_or("<invalid>");
        let line_y = log_y_start + (i as i32 * GLYPH_HEIGHT as i32);
        draw_text(
            &mut font,
            fb,
            line_str,
            GLYPH_WIDTH as i32,
            line_y,
            shared::gpu::BOOT_LOG_FG,
            shared::gpu::BOOT_LOG_BG,
        );
    }
}
