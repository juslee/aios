//! Compute Kit — GPU surface abstraction for compositor and UI toolkit.
//!
//! Architecture reference: `docs/kits/kernel/compute.md` §2

use crate::gpu::GpuPixelFormat;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors from Compute Kit operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputeError {
    /// GPU device not available or not initialized.
    DeviceUnavailable,
    /// Buffer allocation failed (out of GPU memory or resources).
    AllocationFailed,
    /// Invalid buffer dimensions or format.
    InvalidParameters,
    /// IPC communication with GPU Service failed.
    ServiceError,
    /// Capability check failed — caller lacks required permission.
    PermissionDenied,
}

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// An allocated GPU surface buffer.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceBuffer {
    /// Opaque buffer handle (maps to GpuBufferHandle internally).
    pub id: u32,
    /// Buffer width in pixels.
    pub width: u32,
    /// Buffer height in pixels.
    pub height: u32,
    /// Pixel format.
    pub format: GpuPixelFormat,
    /// Virtual address of the framebuffer memory (0 if not mapped).
    pub fb_virt: usize,
    /// Stride in bytes.
    pub stride: u32,
}

/// A rectangular damage region within a surface buffer.
#[derive(Debug, Clone, Copy)]
pub struct DamageRect {
    /// X offset in pixels.
    pub x: u32,
    /// Y offset in pixels.
    pub y: u32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// Semantic hints that inform compositor optimization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticHint {
    /// UI text — subpixel rendering, high priority for clarity.
    UiText,
    /// Video playback — direct scanout candidate, lower composition priority.
    VideoPlayback,
    /// 3D rendering — vsync-aligned, no subpixel.
    Rendering3D,
    /// Scrolling content — predictive composition.
    ScrollingContent,
    /// Static content — cache aggressively.
    StaticContent,
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Display surface trait for compositor and UI toolkit integration.
///
/// Allocates GPU-backed buffers, submits damage regions, and requests
/// direct scanout (bypassing composition when only one surface is visible).
pub trait GpuSurface {
    /// Allocate a new surface buffer with the given dimensions and format.
    fn allocate_buffer(
        &self,
        width: u32,
        height: u32,
        format: GpuPixelFormat,
    ) -> Result<SurfaceBuffer, ComputeError>;

    /// Submit damage regions to the compositor. Only damaged pixels are
    /// re-composited, reducing GPU work.
    fn submit_damage(
        &self,
        buffer: &SurfaceBuffer,
        damage: &[DamageRect],
    ) -> Result<(), ComputeError>;

    /// Set a semantic hint for compositor optimization.
    fn set_semantic_hint(&self, hint: SemanticHint) -> Result<(), ComputeError>;

    /// Request direct scanout (bypass composition). Succeeds only when
    /// this is the only visible surface on the display.
    fn request_direct_scanout(&self) -> Result<bool, ComputeError>;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_error_variants() {
        let e = ComputeError::DeviceUnavailable;
        assert_eq!(e, ComputeError::DeviceUnavailable);
        assert_ne!(e, ComputeError::AllocationFailed);
    }

    #[test]
    fn surface_buffer_default() {
        let buf = SurfaceBuffer {
            id: 1,
            width: 1280,
            height: 800,
            format: GpuPixelFormat::B8G8R8A8,
            fb_virt: 0,
            stride: 5120,
        };
        assert_eq!(buf.width, 1280);
        assert_eq!(buf.height, 800);
    }

    #[test]
    fn damage_rect() {
        let d = DamageRect {
            x: 0,
            y: 0,
            width: 100,
            height: 50,
        };
        assert_eq!(d.width, 100);
    }

    #[test]
    fn semantic_hint_variants() {
        assert_ne!(SemanticHint::UiText, SemanticHint::VideoPlayback);
        assert_eq!(SemanticHint::StaticContent, SemanticHint::StaticContent);
    }

    /// Verify GpuSurface is dyn-compatible (object-safe).
    #[test]
    fn gpu_surface_dyn_compatible() {
        fn _accept(_s: &dyn GpuSurface) {}
    }
}
