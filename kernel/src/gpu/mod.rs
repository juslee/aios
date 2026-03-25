//! GPU subsystem: service, text rendering, and Compute Kit implementation.
//!
//! The GPU Service wraps the VirtIO-GPU 2D driver in a capability-gated
//! IPC service with double-buffered display management.

pub mod service;
pub mod text;

use shared::compute_kit::{ComputeError, DamageRect, GpuSurface, SemanticHint, SurfaceBuffer};
use shared::gpu::GpuPixelFormat;

/// Kernel-side implementation of the Compute Kit `GpuSurface` trait.
///
/// Zero-sized unit struct that delegates to the GPU Service via IPC.
/// Follows the same pattern as `KernelFrameAllocator` and other Kit wrappers.
pub struct KernelGpuSurface;

impl GpuSurface for KernelGpuSurface {
    fn allocate_buffer(
        &self,
        width: u32,
        height: u32,
        format: GpuPixelFormat,
    ) -> Result<SurfaceBuffer, ComputeError> {
        // Phase 6: direct driver call. Phase 7+ will route via IPC to GPU Service.
        let handle = crate::drivers::virtio_gpu::gpu_allocate_framebuffer(width, height)
            .map_err(|_| ComputeError::AllocationFailed)?;

        Ok(SurfaceBuffer {
            id: handle.resource_id,
            width: handle.width,
            height: handle.height,
            format,
            fb_virt: handle.fb_virt,
            stride: handle.stride,
        })
    }

    fn submit_damage(
        &self,
        buffer: &SurfaceBuffer,
        damage: &[DamageRect],
    ) -> Result<(), ComputeError> {
        // Transfer damaged regions to host and flush.
        for d in damage {
            let rect = shared::gpu::VirtioGpuRect {
                x: d.x,
                y: d.y,
                width: d.width,
                height: d.height,
            };
            crate::drivers::virtio_gpu::gpu_transfer_to_host(buffer.id, &rect, 0)
                .map_err(|_| ComputeError::ServiceError)?;
            crate::drivers::virtio_gpu::gpu_resource_flush(buffer.id, &rect)
                .map_err(|_| ComputeError::ServiceError)?;
        }
        Ok(())
    }

    fn set_semantic_hint(&self, _hint: SemanticHint) -> Result<(), ComputeError> {
        // Informational only in Phase 6. Compositor uses this in Phase 7+.
        Ok(())
    }

    fn request_direct_scanout(&self) -> Result<bool, ComputeError> {
        // Single surface, no compositor — always direct scanout.
        Ok(true)
    }
}
