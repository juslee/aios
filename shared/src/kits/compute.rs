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
    fn compute_error_all_variants_distinct() {
        let variants = [
            ComputeError::DeviceUnavailable,
            ComputeError::AllocationFailed,
            ComputeError::InvalidParameters,
            ComputeError::ServiceError,
            ComputeError::PermissionDenied,
        ];
        // Every variant equals itself.
        for v in &variants {
            assert_eq!(*v, *v);
        }
        // Every pair of distinct variants is unequal.
        for (i, a) in variants.iter().enumerate() {
            for (j, b) in variants.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "variants {i} and {j} should differ");
                }
            }
        }
    }

    #[test]
    fn compute_error_is_copy() {
        let e = ComputeError::AllocationFailed;
        let e2 = e; // Copy
        assert_eq!(e, e2);
    }

    #[test]
    fn surface_buffer_fields() {
        let buf = SurfaceBuffer {
            id: 42,
            width: 1280,
            height: 800,
            format: GpuPixelFormat::B8G8R8A8,
            fb_virt: 0xDEAD_0000,
            stride: 5120,
        };
        assert_eq!(buf.id, 42);
        assert_eq!(buf.width, 1280);
        assert_eq!(buf.height, 800);
        assert_eq!(buf.format, GpuPixelFormat::B8G8R8A8);
        assert_eq!(buf.fb_virt, 0xDEAD_0000);
        assert_eq!(buf.stride, 5120);
        // Stride is in bytes; for B8G8R8A8 it must be at least width * 4 and
        // should be a multiple of the bytes-per-pixel, but may include padding.
        assert!(buf.stride >= buf.width * 4);
        assert_eq!(buf.stride % 4, 0);
    }

    #[test]
    fn surface_buffer_is_copy() {
        let buf = SurfaceBuffer {
            id: 1,
            width: 640,
            height: 480,
            format: GpuPixelFormat::B8G8R8A8,
            fb_virt: 0,
            stride: 2560,
        };
        let buf2 = buf; // Copy
        assert_eq!(buf.id, buf2.id);
    }

    #[test]
    fn damage_rect_fields() {
        let d = DamageRect {
            x: 10,
            y: 20,
            width: 100,
            height: 50,
        };
        assert_eq!(d.x, 10);
        assert_eq!(d.y, 20);
        assert_eq!(d.width, 100);
        assert_eq!(d.height, 50);
    }

    #[test]
    fn damage_rect_full_surface() {
        // Damage rect covering the entire 1280x800 surface.
        let d = DamageRect {
            x: 0,
            y: 0,
            width: 1280,
            height: 800,
        };
        assert_eq!(d.x, 0);
        assert_eq!(d.width * d.height, 1_024_000);
    }

    #[test]
    fn semantic_hint_all_variants_distinct() {
        let variants = [
            SemanticHint::UiText,
            SemanticHint::VideoPlayback,
            SemanticHint::Rendering3D,
            SemanticHint::ScrollingContent,
            SemanticHint::StaticContent,
        ];
        for v in &variants {
            assert_eq!(*v, *v);
        }
        for (i, a) in variants.iter().enumerate() {
            for (j, b) in variants.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "hints {i} and {j} should differ");
                }
            }
        }
    }

    #[test]
    fn semantic_hint_is_copy() {
        let h = SemanticHint::UiText;
        let h2 = h; // Copy
        assert_eq!(h, h2);
    }

    /// Verify GpuSurface is dyn-compatible (object-safe).
    #[test]
    fn gpu_surface_dyn_compatible() {
        fn _accept(_s: &dyn GpuSurface) {}
    }

    /// Verify all 13 Kit traits are dyn-compatible in one place.
    /// This cross-Kit test lives here because Compute Kit is the last Kit added.
    #[test]
    fn all_kit_traits_dyn_compatible() {
        // Memory Kit (3 traits)
        fn _frame_allocator(_: &dyn crate::kits::memory::FrameAllocator) {}
        fn _address_space(_: &dyn crate::kits::memory::AddressSpace) {}
        fn _pressure_monitor(_: &dyn crate::kits::memory::MemoryPressureMonitor) {}
        // Capability Kit (1 trait)
        fn _capability_enforcer(_: &dyn crate::kits::capability::CapabilityEnforcer) {}
        // IPC Kit (4 traits)
        fn _channel_ops(_: &dyn crate::kits::ipc::ChannelOps) {}
        fn _notification_ops(_: &dyn crate::kits::ipc::NotificationOps) {}
        fn _select_ops(_: &dyn crate::kits::ipc::SelectOps) {}
        fn _shared_memory_ops(_: &dyn crate::kits::ipc::SharedMemoryOps) {}
        // Storage Kit (4 traits)
        fn _block_store(_: &dyn crate::kits::storage::BlockStore) {}
        fn _space_manager(_: &dyn crate::kits::storage::SpaceManager) {}
        fn _object_store(_: &dyn crate::kits::storage::ObjectStore) {}
        fn _version_store(_: &dyn crate::kits::storage::VersionStoreOps) {}
        // Compute Kit (1 trait)
        fn _gpu_surface(_: &dyn GpuSurface) {}
    }
}
