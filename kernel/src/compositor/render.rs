//! Software compositor — flat-z-order blitting and damage tracking.
//!
//! M24 ships an opaque `B8G8R8A8` blitter (`blit_opaque`) used by all shell
//! and test surfaces, plus a premultiplied-alpha blitter
//! (`blit_alpha_premultiplied`) ready for window decorations and the
//! software cursor (M25). The composition function iterates surfaces in
//! z-order (layer first, then `layer_seq` insertion order) and blits each
//! visible region into the back buffer.
//!
//! Per docs/platform/compositor/rendering.md §5.1, §5.2.

use shared::compositor::DamageTracker;
use shared::gpu::AIOS_BLUE_B8G8R8A8;

use super::surface::Surface;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compose all visible surfaces into a destination framebuffer.
///
/// `dst` is the destination framebuffer in `B8G8R8A8` format (one `u32` per
/// pixel). The caller is responsible for sorting `surfaces` in render order
/// — typically by `(SurfaceLayer, layer_seq)` ascending so background
/// draws first.
///
/// `resolve_src` resolves a surface to its source pixel slice, or returns
/// `None` if the surface has no buffer attached yet (state < Active or
/// `shmem_id` not yet resolved). The closure is allocation-free.
///
/// Damaged regions are accumulated into `damage` for the caller to feed
/// to the present path. When `clear_first` is true, the entire destination
/// is cleared to `clear_color` before any surface is blitted (used for the
/// first frame after handoff).
#[allow(dead_code)] // Wired by Step 14 (composition loop) and Step 15 (self-test).
#[allow(clippy::too_many_arguments)]
pub fn compose_frame<F>(
    dst: &mut [u32],
    dst_width: u32,
    dst_height: u32,
    surfaces: &[Surface],
    damage: &mut DamageTracker,
    clear_first: bool,
    clear_color: u32,
    mut resolve_src: F,
) where
    F: FnMut(&Surface) -> Option<&[u32]>,
{
    if dst_width == 0 || dst_height == 0 || dst.is_empty() {
        return;
    }

    if clear_first {
        dst.fill(clear_color);
        damage.mark_full();
    }

    for surface in surfaces {
        // M26 Step 26: skip hidden surfaces (e.g., Workspace toggled
        // off via Super). `visible` is independent of `state` —
        // surfaces with attached buffers can still be hidden.
        if !surface.visible {
            continue;
        }
        let src = match resolve_src(surface) {
            Some(s) => s,
            None => continue,
        };
        if surface.width == 0 || surface.height == 0 {
            continue;
        }
        if src.len() < (surface.width as usize) * (surface.height as usize) {
            continue;
        }
        let blitted = blit_opaque(
            src,
            surface.width,
            surface.height,
            dst,
            dst_width,
            dst_height,
            surface.x,
            surface.y,
        );
        if let Some(rect) = blitted {
            damage.union(rect);
        }
    }
}

/// Blit an opaque `B8G8R8A8` source rectangle into the destination at
/// `(dst_x, dst_y)`. Returns the actual blitted rectangle in destination
/// coordinates (clipped to the destination bounds), or `None` if the source
/// is fully off-screen.
#[allow(dead_code)] // Wired by Step 14 (composition loop) and Step 15 (self-test).
#[allow(clippy::too_many_arguments)]
pub fn blit_opaque(
    src: &[u32],
    src_w: u32,
    src_h: u32,
    dst: &mut [u32],
    dst_w: u32,
    dst_h: u32,
    dst_x: i32,
    dst_y: i32,
) -> Option<shared::compositor::DamageRect> {
    let clip = ClipRect::new(dst_x, dst_y, src_w, src_h, dst_w, dst_h)?;

    let dst_stride = dst_w as usize;
    let src_stride = src_w as usize;

    for row in 0..clip.copy_h as usize {
        let src_row_start = (clip.src_y + row) * src_stride + clip.src_x;
        let dst_row_start = (clip.dst_y as usize + row) * dst_stride + clip.dst_x as usize;
        let src_slice = &src[src_row_start..src_row_start + clip.copy_w as usize];
        let dst_slice = &mut dst[dst_row_start..dst_row_start + clip.copy_w as usize];
        dst_slice.copy_from_slice(src_slice);
    }

    Some(shared::compositor::DamageRect {
        x: clip.dst_x as u32,
        y: clip.dst_y as u32,
        width: clip.copy_w,
        height: clip.copy_h,
    })
}

/// Blit a premultiplied-alpha `A8R8G8B8` source rectangle onto the
/// destination using the standard `out = src + dst * (1 - src_alpha)`
/// formula. Each channel is processed in 8-bit fixed-point integer math.
///
/// The destination format is `B8G8R8A8` (display native); the source is
/// expected as `A8R8G8B8` packed into a `u32` in little-endian byte order
/// (i.e. `0xAARRGGBB`). M24 uses this only for upcoming window
/// decorations and the cursor sprite (M25).
#[allow(dead_code)] // Wired by Step 18 (M25) — premultiplied path verified by unit test.
#[allow(clippy::too_many_arguments)]
pub fn blit_alpha_premultiplied(
    src: &[u32],
    src_w: u32,
    src_h: u32,
    dst: &mut [u32],
    dst_w: u32,
    dst_h: u32,
    dst_x: i32,
    dst_y: i32,
) -> Option<shared::compositor::DamageRect> {
    let clip = ClipRect::new(dst_x, dst_y, src_w, src_h, dst_w, dst_h)?;

    let dst_stride = dst_w as usize;
    let src_stride = src_w as usize;

    for row in 0..clip.copy_h as usize {
        let src_row = (clip.src_y + row) * src_stride + clip.src_x;
        let dst_row = (clip.dst_y as usize + row) * dst_stride + clip.dst_x as usize;
        for col in 0..clip.copy_w as usize {
            let src_px = src[src_row + col];
            let dst_px = dst[dst_row + col];
            dst[dst_row + col] = blend_premultiplied(src_px, dst_px);
        }
    }

    Some(shared::compositor::DamageRect {
        x: clip.dst_x as u32,
        y: clip.dst_y as u32,
        width: clip.copy_w,
        height: clip.copy_h,
    })
}

/// Z-order key used by callers to sort surfaces before composition.
///
/// Sort ascending: `(SurfaceLayer as u8, layer_seq)`. This keeps the
/// background drawn first, then Normal, then TopLevel, etc., with stable
/// insertion-order tie-break inside each layer.
#[allow(dead_code)] // Used by Step 14 composition loop sorter.
pub fn z_order_key(surface: &Surface) -> (u8, u64) {
    (surface.layer as u8, surface.layer_seq)
}

/// Convenience constant — the AIOS-blue clear color used when `clear_first`
/// is requested by `compose_frame`.
#[allow(dead_code)] // Used by Step 14 main loop default-clear and Step 15 test.
pub const DEFAULT_CLEAR_COLOR: u32 = AIOS_BLUE_B8G8R8A8;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

struct ClipRect {
    src_x: usize,
    src_y: usize,
    dst_x: i32,
    dst_y: i32,
    copy_w: u32,
    copy_h: u32,
}

impl ClipRect {
    fn new(dst_x: i32, dst_y: i32, src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Option<Self> {
        if src_w == 0 || src_h == 0 {
            return None;
        }

        let dst_w_i = dst_w as i64;
        let dst_h_i = dst_h as i64;
        let src_w_i = src_w as i64;
        let src_h_i = src_h as i64;
        let dx = dst_x as i64;
        let dy = dst_y as i64;

        let dst_x_start = dx.max(0);
        let dst_y_start = dy.max(0);
        let dst_x_end = (dx + src_w_i).min(dst_w_i);
        let dst_y_end = (dy + src_h_i).min(dst_h_i);
        if dst_x_end <= dst_x_start || dst_y_end <= dst_y_start {
            return None;
        }

        let src_x = (dst_x_start - dx) as usize;
        let src_y = (dst_y_start - dy) as usize;
        let copy_w = (dst_x_end - dst_x_start) as u32;
        let copy_h = (dst_y_end - dst_y_start) as u32;

        Some(Self {
            src_x,
            src_y,
            dst_x: dst_x_start as i32,
            dst_y: dst_y_start as i32,
            copy_w,
            copy_h,
        })
    }
}

/// Premultiplied alpha blend: `out = src + dst * (1 - src_alpha)`.
///
/// `src` and `dst` are 32-bit pixels in `0xAARRGGBB` order. Alpha is the
/// top byte. We compute 1−α as `255 − α` and use shift-by-8 instead of
/// divide-by-255 (off-by-one rounding, but consistent with most reference
/// software compositors).
#[inline(always)]
fn blend_premultiplied(src: u32, dst: u32) -> u32 {
    let sa = (src >> 24) & 0xFF;
    let inv = 255u32 - sa;

    let sr = (src >> 16) & 0xFF;
    let sg = (src >> 8) & 0xFF;
    let sb = src & 0xFF;

    let dr = (dst >> 16) & 0xFF;
    let dg = (dst >> 8) & 0xFF;
    let db = dst & 0xFF;
    let da = (dst >> 24) & 0xFF;

    let out_a = sa + ((da * inv) >> 8);
    let out_r = sr + ((dr * inv) >> 8);
    let out_g = sg + ((dg * inv) >> 8);
    let out_b = sb + ((db * inv) >> 8);

    ((out_a & 0xFF) << 24) | ((out_r & 0xFF) << 16) | ((out_g & 0xFF) << 8) | (out_b & 0xFF)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
//
// Tests for the rendering primitives live in the shared crate where the
// `DamageRect` and pixel layout types are defined. The kernel-side render
// module is exercised end-to-end by Step 15's multi-surface composition test.

#[cfg(test)]
mod tests {
    // The kernel render module references kernel-internal types (Surface).
    // Pure-pixel-math tests for blit_opaque, blit_alpha_premultiplied, and
    // ClipRect live in tests modules that don't import Surface.
}
