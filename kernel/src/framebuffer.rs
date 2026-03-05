//! Framebuffer driver for UEFI GOP-provided linear framebuffer.
//!
//! Renders pixels via write_volatile to the identity-mapped framebuffer.
//! Phase 1 uses Non-Cacheable Normal memory — writes are immediately
//! visible to the display without cache flushes.

use shared::BootInfo;

/// Framebuffer handle wrapping the GOP linear framebuffer.
pub struct Framebuffer {
    base: *mut u32,
    width: u32,
    height: u32,
    /// Stride in bytes (not pixels).
    stride: u32,
    /// Pixel format: 0 = Bgr8, 1 = Rgb8.
    format: u32,
}

// SAFETY: Framebuffer is only used from the boot CPU in Phase 1.
// Phase 2+ needs proper synchronization if multiple cores draw.
unsafe impl Send for Framebuffer {}

impl Framebuffer {
    /// Construct from BootInfo. Returns None if no framebuffer is available.
    pub fn from_boot_info(bi: &BootInfo) -> Option<Self> {
        if bi.framebuffer == 0 || bi.fb_width == 0 || bi.fb_height == 0 {
            return None;
        }
        Some(Framebuffer {
            base: bi.framebuffer as *mut u32,
            width: bi.fb_width,
            height: bi.fb_height,
            stride: bi.fb_stride,
            format: bi.fb_pixel_format,
        })
    }

    /// Pack an (r, g, b) triple into a u32 pixel value for this format.
    pub fn pack_pixel(&self, r: u8, g: u8, b: u8) -> u32 {
        match self.format {
            // Bgr8: byte order [B, G, R, A] → little-endian u32 = 0xAARRGGBB
            0 => 0xFF00_0000 | (r as u32) << 16 | (g as u32) << 8 | b as u32,
            // Rgb8: byte order [R, G, B, A] → little-endian u32 = 0xAABBGGRR
            1 => 0xFF00_0000 | (b as u32) << 16 | (g as u32) << 8 | r as u32,
            _ => 0xFF00_0000, // Unknown format → opaque black
        }
    }

    /// Fill a rectangle with a pre-packed u32 pixel color.
    /// Coordinates are clamped to framebuffer bounds.
    pub fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, pixel: u32) {
        // Pre-clamp to framebuffer bounds.
        let x1 = x.min(self.width);
        let y1 = y.min(self.height);
        let x2 = (x + w).min(self.width);
        let y2 = (y + h).min(self.height);

        let stride_u32 = (self.stride / 4) as usize;

        for row in y1..y2 {
            let row_base = row as usize * stride_u32;
            for col in x1..x2 {
                // SAFETY: Framebuffer is identity-mapped, base is valid GOP
                // framebuffer address, and coordinates are clamped to bounds.
                unsafe {
                    core::ptr::write_volatile(self.base.add(row_base + col as usize), pixel);
                }
            }
        }
    }

    /// Render the Phase 1 test pattern: black background with centered #5B8CFF rectangle.
    pub fn render_test_pattern(&mut self) {
        let black = self.pack_pixel(0, 0, 0);
        self.fill_rect(0, 0, self.width, self.height, black);

        // Centered 60% × 60% rectangle in #5B8CFF.
        let blue = self.pack_pixel(0x5B, 0x8C, 0xFF);
        let rect_w = self.width * 60 / 100;
        let rect_h = self.height * 60 / 100;
        let rect_x = (self.width - rect_w) / 2;
        let rect_y = (self.height - rect_h) / 2;
        self.fill_rect(rect_x, rect_y, rect_w, rect_h, blue);
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn stride(&self) -> u32 {
        self.stride
    }

    pub fn format(&self) -> u32 {
        self.format
    }

    pub fn base_addr(&self) -> u64 {
        self.base as u64
    }
}
