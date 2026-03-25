//! VirtIO-GPU wire-format types, AIOS-native GPU types, and constants.
//!
//! Wire-format structs are `repr(C)` to match the VirtIO GPU specification.
//! AIOS-native types (`GpuPixelFormat`, `DisplayInfo`, `GpuError`, `GpuBufferHandle`)
//! are kernel-agnostic and live here for host-side unit testing.
//!
//! Per VirtIO GPU spec §5.7 and docs/platform/gpu/drivers.md §3.1–3.5.

// ---------------------------------------------------------------------------
// VirtIO-GPU command type constants
// ---------------------------------------------------------------------------

/// GET_DISPLAY_INFO — query display dimensions.
pub const VIRTIO_GPU_CMD_GET_DISPLAY_INFO: u32 = 0x0100;
/// RESOURCE_CREATE_2D — create a 2D pixel resource on the host.
pub const VIRTIO_GPU_CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
/// RESOURCE_UNREF — destroy a resource.
pub const VIRTIO_GPU_CMD_RESOURCE_UNREF: u32 = 0x0102;
/// SET_SCANOUT — bind a resource to a display scanout.
pub const VIRTIO_GPU_CMD_SET_SCANOUT: u32 = 0x0103;
/// RESOURCE_FLUSH — signal the host to refresh the display.
pub const VIRTIO_GPU_CMD_RESOURCE_FLUSH: u32 = 0x0104;
/// TRANSFER_TO_HOST_2D — copy pixel data from guest to host resource.
pub const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
/// RESOURCE_ATTACH_BACKING — bind guest DMA pages to a resource.
pub const VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
/// RESOURCE_DETACH_BACKING — unbind guest DMA pages from a resource.
pub const VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING: u32 = 0x0107;

// ---------------------------------------------------------------------------
// VirtIO-GPU response type constants
// ---------------------------------------------------------------------------

/// Success, no payload.
pub const VIRTIO_GPU_RESP_OK_NODATA: u32 = 0x1100;
/// Success, payload is `VirtioGpuRespDisplayInfo`.
pub const VIRTIO_GPU_RESP_OK_DISPLAY_INFO: u32 = 0x1101;
/// Unspecified error.
pub const VIRTIO_GPU_RESP_ERR_UNSPEC: u32 = 0x1200;
/// Out of host memory.
pub const VIRTIO_GPU_RESP_ERR_OUT_OF_MEMORY: u32 = 0x1201;
/// Invalid scanout ID.
pub const VIRTIO_GPU_RESP_ERR_INVALID_SCANOUT_ID: u32 = 0x1202;
/// Invalid resource ID.
pub const VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID: u32 = 0x1203;
/// Invalid rendering context.
pub const VIRTIO_GPU_RESP_ERR_INVALID_CONTEXT_ID: u32 = 0x1204;
/// Invalid parameter.
pub const VIRTIO_GPU_RESP_ERR_INVALID_PARAMETER: u32 = 0x1205;

// ---------------------------------------------------------------------------
// VirtIO-GPU flags
// ---------------------------------------------------------------------------

/// When set in `VirtioGpuCtrlHdr.flags`, the `fence_id` field is valid.
pub const VIRTIO_GPU_FLAG_FENCE: u32 = 1;

// ---------------------------------------------------------------------------
// VirtIO-GPU pixel formats (spec §5.7.6.8)
// ---------------------------------------------------------------------------

/// VirtIO-GPU pixel format identifiers (wire-format values).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VirtioGpuFormat {
    B8G8R8A8Unorm = 1,
    R8G8B8A8Unorm = 67,
}

// ---------------------------------------------------------------------------
// VirtIO-GPU wire-format structs (repr(C), match spec exactly)
// ---------------------------------------------------------------------------

/// Control/cursor command header — precedes every VirtIO-GPU command.
///
/// Size: 24 bytes.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct VirtioGpuCtrlHdr {
    /// Command or response type (`VIRTIO_GPU_CMD_*` / `VIRTIO_GPU_RESP_*`).
    pub type_: u32,
    /// Flags — bit 0 = `VIRTIO_GPU_FLAG_FENCE`.
    pub flags: u32,
    /// Fence ID (valid when `VIRTIO_GPU_FLAG_FENCE` is set).
    pub fence_id: u64,
    /// 3D rendering context ID (0 for 2D commands).
    pub ctx_id: u32,
    /// Ring index (0 for single-ring operation).
    pub ring_idx: u8,
    /// Padding — must be zero.
    pub padding: [u8; 3],
}

/// Rectangle — used by transfer, flush, scanout commands.
///
/// Size: 16 bytes.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct VirtioGpuRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Single display scanout info within `VirtioGpuRespDisplayInfo`.
///
/// Size: 24 bytes.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct VirtioGpuDisplayOne {
    /// Rectangle describing the scanout dimensions.
    pub r: VirtioGpuRect,
    /// Nonzero if this scanout is enabled.
    pub enabled: u32,
    /// Reserved flags.
    pub flags: u32,
}

/// Maximum number of scanouts in the display info response.
pub const VIRTIO_GPU_MAX_SCANOUTS: usize = 16;

/// Response to `GET_DISPLAY_INFO` — contains info for up to 16 scanouts.
///
/// Size: 24 (header) + 16 × 24 (scanouts) = 408 bytes.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct VirtioGpuRespDisplayInfo {
    pub hdr: VirtioGpuCtrlHdr,
    pub pmodes: [VirtioGpuDisplayOne; VIRTIO_GPU_MAX_SCANOUTS],
}

/// RESOURCE_CREATE_2D command.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct VirtioGpuResourceCreate2d {
    pub hdr: VirtioGpuCtrlHdr,
    /// Driver-assigned resource ID (must be nonzero and unique).
    pub resource_id: u32,
    /// Pixel format (`VirtioGpuFormat` value).
    pub format: u32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// SET_SCANOUT command — binds a resource to a display output.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct VirtioGpuSetScanout {
    pub hdr: VirtioGpuCtrlHdr,
    /// Region of the resource to display.
    pub r: VirtioGpuRect,
    /// Scanout index (0 = primary display).
    pub scanout_id: u32,
    /// Resource ID (0 = disable scanout).
    pub resource_id: u32,
}

/// RESOURCE_FLUSH command — triggers display refresh.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct VirtioGpuResourceFlush {
    pub hdr: VirtioGpuCtrlHdr,
    /// Region to flush to display.
    pub r: VirtioGpuRect,
    /// Resource ID.
    pub resource_id: u32,
    pub padding: u32,
}

/// TRANSFER_TO_HOST_2D command — uploads pixel data to host resource.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct VirtioGpuTransferToHost2d {
    pub hdr: VirtioGpuCtrlHdr,
    /// Region within the resource to update.
    pub r: VirtioGpuRect,
    /// Byte offset into guest backing memory.
    pub offset: u64,
    /// Resource ID.
    pub resource_id: u32,
    pub padding: u32,
}

/// RESOURCE_ATTACH_BACKING command header.
///
/// Followed by `nr_entries` of `VirtioGpuMemEntry` structs.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct VirtioGpuResourceAttachBacking {
    pub hdr: VirtioGpuCtrlHdr,
    /// Resource ID.
    pub resource_id: u32,
    /// Number of `VirtioGpuMemEntry` structs following.
    pub nr_entries: u32,
}

/// Memory entry for `RESOURCE_ATTACH_BACKING` — one contiguous DMA region.
///
/// Size: 16 bytes.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct VirtioGpuMemEntry {
    /// Guest physical address.
    pub addr: u64,
    /// Length in bytes.
    pub length: u32,
    pub padding: u32,
}

/// RESOURCE_UNREF command — destroys a resource.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct VirtioGpuResourceUnref {
    pub hdr: VirtioGpuCtrlHdr,
    /// Resource ID to destroy.
    pub resource_id: u32,
    pub padding: u32,
}

/// RESOURCE_DETACH_BACKING command — unbinds guest memory.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct VirtioGpuResourceDetachBacking {
    pub hdr: VirtioGpuCtrlHdr,
    /// Resource ID.
    pub resource_id: u32,
    pub padding: u32,
}

// ---------------------------------------------------------------------------
// Compile-time size assertions (must match VirtIO spec wire format)
// ---------------------------------------------------------------------------

const _: () = assert!(core::mem::size_of::<VirtioGpuCtrlHdr>() == 24);
const _: () = assert!(core::mem::size_of::<VirtioGpuRect>() == 16);
const _: () = assert!(core::mem::size_of::<VirtioGpuDisplayOne>() == 24);
const _: () = assert!(core::mem::size_of::<VirtioGpuRespDisplayInfo>() == 408);
const _: () = assert!(core::mem::size_of::<VirtioGpuMemEntry>() == 16);
const _: () = assert!(core::mem::size_of::<VirtioGpuResourceCreate2d>() == 40);
const _: () = assert!(core::mem::size_of::<VirtioGpuSetScanout>() == 48);
const _: () = assert!(core::mem::size_of::<VirtioGpuResourceFlush>() == 48);
const _: () = assert!(core::mem::size_of::<VirtioGpuTransferToHost2d>() == 56);
const _: () = assert!(core::mem::size_of::<VirtioGpuResourceAttachBacking>() == 32);
const _: () = assert!(core::mem::size_of::<VirtioGpuResourceUnref>() == 32);
const _: () = assert!(core::mem::size_of::<VirtioGpuResourceDetachBacking>() == 32);

// ---------------------------------------------------------------------------
// AIOS-native GPU types (not VirtIO wire format)
// ---------------------------------------------------------------------------

/// AIOS-native pixel format for GPU framebuffers.
///
/// Distinct from the boot `PixelFormat` in `shared/src/boot.rs` and the
/// VirtIO `VirtioGpuFormat` wire values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum GpuPixelFormat {
    /// Blue-Green-Red-Alpha, 8 bits per channel.
    B8G8R8A8 = 0,
    /// Red-Green-Blue-Alpha, 8 bits per channel.
    R8G8B8A8 = 1,
}

impl GpuPixelFormat {
    /// Bytes per pixel for this format.
    pub const fn bytes_per_pixel(self) -> u32 {
        4
    }

    /// Convert to VirtIO wire format value.
    pub const fn to_virtio(self) -> u32 {
        match self {
            GpuPixelFormat::B8G8R8A8 => VirtioGpuFormat::B8G8R8A8Unorm as u32,
            GpuPixelFormat::R8G8B8A8 => VirtioGpuFormat::R8G8B8A8Unorm as u32,
        }
    }
}

/// Display information for a single scanout.
#[derive(Debug, Clone, Copy)]
pub struct DisplayInfo {
    /// Display width in pixels.
    pub width: u32,
    /// Display height in pixels.
    pub height: u32,
    /// Pixel format.
    pub format: GpuPixelFormat,
    /// Scanout index (typically 0 for primary).
    pub scanout_id: u32,
}

impl DisplayInfo {
    /// Default display info used when no scanout is available.
    pub const fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            format: GpuPixelFormat::B8G8R8A8,
            scanout_id: 0,
        }
    }
}

/// GPU operation error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuError {
    /// No VirtIO-GPU device found during probe.
    DeviceNotFound,
    /// Device initialization failed.
    InitFailed,
    /// GPU command returned an error response.
    CommandFailed,
    /// DMA memory allocation failed.
    OutOfMemory,
    /// Invalid resource ID.
    InvalidResource,
    /// Scanout configuration failed.
    ScanoutFailed,
    /// Command poll timed out.
    Timeout,
    /// Requested resolution exceeds MAX_ORDER allocation limit.
    ResolutionTooLarge,
}

/// Handle to a GPU framebuffer — returned by `allocate_framebuffer()`.
///
/// Contains all information needed to render into and present the buffer.
#[derive(Debug, Clone, Copy)]
pub struct GpuBufferHandle {
    /// VirtIO resource ID.
    pub resource_id: u32,
    /// Framebuffer width in pixels.
    pub width: u32,
    /// Framebuffer height in pixels.
    pub height: u32,
    /// Pixel format.
    pub format: GpuPixelFormat,
    /// Bytes per row (width × bytes_per_pixel, may include padding).
    pub stride: u32,
    /// DMA backing physical address.
    pub fb_phys: usize,
    /// DMA backing virtual address (via direct map).
    pub fb_virt: usize,
    /// Number of physical pages in the backing allocation.
    pub page_count: usize,
    /// Buddy allocator order used for allocation (for deallocation).
    pub order: usize,
}

/// Maximum framebuffer size in bytes that fits in a single contiguous
/// DMA allocation (buddy MAX_ORDER=10 → 4 MiB = 1024 pages).
pub const MAX_FRAMEBUFFER_BYTES: usize = 4 * 1024 * 1024;

/// AIOS blue color: #5B8CFF in B8G8R8A8 format (little-endian u32).
pub const AIOS_BLUE_B8G8R8A8: u32 = 0xFF5B_8CFF;

/// Maximum GPU buffers tracked by the GPU Service.
pub const MAX_GPU_BUFFERS: usize = 8;

// ---------------------------------------------------------------------------
// Boot log text rendering colors (B8G8R8A8 format)
// ---------------------------------------------------------------------------

/// Dark blue-grey background for boot log display (#1A1A2E).
pub const BOOT_LOG_BG: u32 = 0xFF1A_1A2E;

/// Light grey foreground for boot log text (#E0E0E0).
pub const BOOT_LOG_FG: u32 = 0xFFE0_E0E0;

/// AIOS blue for boot log header text (#5B8CFF).
pub const BOOT_LOG_HEADER: u32 = AIOS_BLUE_B8G8R8A8;

// ---------------------------------------------------------------------------
// Fence tracker (Phase 6 M20 — double buffering)
// ---------------------------------------------------------------------------

/// Tracks VirtIO-GPU fence completion for asynchronous command synchronization.
///
/// Fences are monotonically increasing IDs assigned to commands. The host signals
/// completion by returning a response with the same fence_id. The driver marks
/// fences as completed and waiters check completion status.
///
/// Per docs/platform/gpu/drivers.md §3.4.
#[derive(Debug, Clone)]
pub struct FenceTracker {
    /// Next fence ID to assign (monotonically increasing).
    pub next_id: u64,
    /// All fence IDs <= this value are complete.
    pub last_completed: u64,
}

impl Default for FenceTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl FenceTracker {
    /// Create a new fence tracker.
    pub const fn new() -> Self {
        Self {
            next_id: 1,
            last_completed: 0,
        }
    }

    /// Allocate a new fence ID.
    pub fn allocate(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Mark a fence (and all prior fences) as completed.
    pub fn complete(&mut self, fence_id: u64) {
        if fence_id > self.last_completed {
            self.last_completed = fence_id;
        }
    }

    /// Check whether a specific fence has completed.
    pub fn is_complete(&self, fence_id: u64) -> bool {
        fence_id <= self.last_completed
    }
}

// ---------------------------------------------------------------------------
// GPU Service IPC protocol types (Phase 6 M20)
// ---------------------------------------------------------------------------

/// GPU Service command identifiers sent via IPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum GpuCommand {
    /// Query display dimensions and format.
    GetDisplayInfo = 1,
    /// Allocate a GPU framebuffer.
    AllocateBuffer = 2,
    /// Release a previously allocated buffer.
    ReleaseBuffer = 3,
    /// Present (transfer + flush) a buffer's damage region.
    Present = 4,
    /// Query buffer info (virtual address, dimensions, stride).
    GetBufferInfo = 5,
    /// Swap front and back buffers (double buffering).
    SwapBuffers = 6,
}

impl GpuCommand {
    /// Convert from a raw u32 discriminant.
    pub fn from_u32(val: u32) -> Option<Self> {
        match val {
            1 => Some(Self::GetDisplayInfo),
            2 => Some(Self::AllocateBuffer),
            3 => Some(Self::ReleaseBuffer),
            4 => Some(Self::Present),
            5 => Some(Self::GetBufferInfo),
            6 => Some(Self::SwapBuffers),
            _ => None,
        }
    }
}

/// GPU Service request message — sent from client to GPU Service via IPC.
///
/// Flat `repr(C)` struct for zero-copy serialization into `RawMessage.data[256]`.
/// All fields are always present; unused fields are 0.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct GpuRequest {
    /// Command discriminant (matches `GpuCommand` values).
    pub command: u32,
    /// Resource ID (for ReleaseBuffer, Present, GetBufferInfo).
    pub resource_id: u32,
    /// Buffer width in pixels (for AllocateBuffer).
    pub width: u32,
    /// Buffer height in pixels (for AllocateBuffer).
    pub height: u32,
    /// Pixel format as u32 (for AllocateBuffer; 0 = B8G8R8A8).
    pub format: u32,
    /// Damage region X offset (for Present).
    pub damage_x: u32,
    /// Damage region Y offset (for Present).
    pub damage_y: u32,
    /// Damage region width (for Present).
    pub damage_w: u32,
    /// Damage region height (for Present).
    pub damage_h: u32,
}

impl GpuRequest {
    /// Create a zeroed request.
    pub const fn zeroed() -> Self {
        Self {
            command: 0,
            resource_id: 0,
            width: 0,
            height: 0,
            format: 0,
            damage_x: 0,
            damage_y: 0,
            damage_w: 0,
            damage_h: 0,
        }
    }
}

/// GPU Service response message — sent from GPU Service to client via IPC.
///
/// Flat `repr(C)` struct for zero-copy serialization into `RawMessage.data[256]`.
/// All fields are always present; unused fields are 0.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct GpuResponse {
    /// Status: 0 = success, negative = GpuError (see `GpuError::to_status`).
    pub status: i32,
    /// Resource ID (for AllocateBuffer response).
    pub resource_id: u32,
    /// Display/buffer width in pixels.
    pub width: u32,
    /// Display/buffer height in pixels.
    pub height: u32,
    /// Bytes per row.
    pub stride: u32,
    /// Pixel format as u32.
    pub format: u32,
    /// Framebuffer virtual address (for GetBufferInfo).
    pub fb_virt: u64,
    /// Scanout index (for GetDisplayInfo).
    pub scanout_id: u32,
    /// Padding for alignment.
    pub _pad: u32,
}

impl GpuResponse {
    /// Create a zeroed (success) response.
    pub const fn zeroed() -> Self {
        Self {
            status: 0,
            resource_id: 0,
            width: 0,
            height: 0,
            stride: 0,
            format: 0,
            fb_virt: 0,
            scanout_id: 0,
            _pad: 0,
        }
    }

    /// Create an error response from a `GpuError`.
    pub fn error(err: GpuError) -> Self {
        let mut resp = Self::zeroed();
        resp.status = err.to_status();
        resp
    }
}

impl GpuError {
    /// Convert to a negative i32 status code for IPC response.
    pub fn to_status(self) -> i32 {
        match self {
            GpuError::DeviceNotFound => -1,
            GpuError::InitFailed => -2,
            GpuError::CommandFailed => -3,
            GpuError::OutOfMemory => -4,
            GpuError::InvalidResource => -5,
            GpuError::ScanoutFailed => -6,
            GpuError::Timeout => -7,
            GpuError::ResolutionTooLarge => -8,
        }
    }

    /// Convert from a negative i32 status code. Returns None for 0 (success)
    /// or unknown codes.
    pub fn from_status(status: i32) -> Option<Self> {
        match status {
            -1 => Some(GpuError::DeviceNotFound),
            -2 => Some(GpuError::InitFailed),
            -3 => Some(GpuError::CommandFailed),
            -4 => Some(GpuError::OutOfMemory),
            -5 => Some(GpuError::InvalidResource),
            -6 => Some(GpuError::ScanoutFailed),
            -7 => Some(GpuError::Timeout),
            -8 => Some(GpuError::ResolutionTooLarge),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Host-side tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;
    use core::mem::size_of;

    #[test]
    fn wire_format_struct_sizes() {
        assert_eq!(size_of::<VirtioGpuCtrlHdr>(), 24);
        assert_eq!(size_of::<VirtioGpuRect>(), 16);
        assert_eq!(size_of::<VirtioGpuDisplayOne>(), 24);
        assert_eq!(size_of::<VirtioGpuRespDisplayInfo>(), 408);
        assert_eq!(size_of::<VirtioGpuMemEntry>(), 16);
        assert_eq!(size_of::<VirtioGpuResourceCreate2d>(), 40);
        assert_eq!(size_of::<VirtioGpuSetScanout>(), 48);
        assert_eq!(size_of::<VirtioGpuResourceFlush>(), 48);
        assert_eq!(size_of::<VirtioGpuTransferToHost2d>(), 56);
        assert_eq!(size_of::<VirtioGpuResourceAttachBacking>(), 32);
        assert_eq!(size_of::<VirtioGpuResourceUnref>(), 32);
        assert_eq!(size_of::<VirtioGpuResourceDetachBacking>(), 32);
    }

    #[test]
    fn command_constants() {
        assert_eq!(VIRTIO_GPU_CMD_GET_DISPLAY_INFO, 0x0100);
        assert_eq!(VIRTIO_GPU_CMD_RESOURCE_CREATE_2D, 0x0101);
        assert_eq!(VIRTIO_GPU_CMD_RESOURCE_UNREF, 0x0102);
        assert_eq!(VIRTIO_GPU_CMD_SET_SCANOUT, 0x0103);
        assert_eq!(VIRTIO_GPU_CMD_RESOURCE_FLUSH, 0x0104);
        assert_eq!(VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D, 0x0105);
        assert_eq!(VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING, 0x0106);
        assert_eq!(VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING, 0x0107);
    }

    #[test]
    fn response_constants() {
        assert_eq!(VIRTIO_GPU_RESP_OK_NODATA, 0x1100);
        assert_eq!(VIRTIO_GPU_RESP_OK_DISPLAY_INFO, 0x1101);
        assert_eq!(VIRTIO_GPU_RESP_ERR_UNSPEC, 0x1200);
        assert_eq!(VIRTIO_GPU_RESP_ERR_OUT_OF_MEMORY, 0x1201);
        assert_eq!(VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID, 0x1203);
    }

    #[test]
    fn gpu_error_derives() {
        let e1 = GpuError::DeviceNotFound;
        let e2 = GpuError::DeviceNotFound;
        let e3 = GpuError::OutOfMemory;
        assert_eq!(e1, e2);
        assert_ne!(e1, e3);
        // Debug derive
        let _ = alloc::format!("{:?}", e1);
    }

    #[test]
    fn display_info_default() {
        let info = DisplayInfo::default();
        assert_eq!(info.width, 0);
        assert_eq!(info.height, 0);
        assert_eq!(info.format, GpuPixelFormat::B8G8R8A8);
        assert_eq!(info.scanout_id, 0);
    }

    #[test]
    fn gpu_pixel_format_to_virtio() {
        assert_eq!(GpuPixelFormat::B8G8R8A8.to_virtio(), 1);
        assert_eq!(GpuPixelFormat::R8G8B8A8.to_virtio(), 67);
    }

    #[test]
    fn gpu_pixel_format_bpp() {
        assert_eq!(GpuPixelFormat::B8G8R8A8.bytes_per_pixel(), 4);
        assert_eq!(GpuPixelFormat::R8G8B8A8.bytes_per_pixel(), 4);
    }

    #[test]
    fn gpu_buffer_handle_fields() {
        let handle = GpuBufferHandle {
            resource_id: 1,
            width: 1024,
            height: 768,
            format: GpuPixelFormat::B8G8R8A8,
            stride: 4096,
            fb_phys: 0x4100_0000,
            fb_virt: 0xFFFF_0001_4100_0000,
            page_count: 768,
            order: 10,
        };
        assert_eq!(handle.resource_id, 1);
        assert_eq!(handle.width, 1024);
        assert_eq!(handle.height, 768);
        assert_eq!(handle.stride, 4096);
        assert_eq!(handle.order, 10);
    }

    #[test]
    fn max_framebuffer_bytes() {
        assert_eq!(MAX_FRAMEBUFFER_BYTES, 4 * 1024 * 1024);
        // 1024×768×4 = 3,145,728 bytes — fits
        assert!(1024 * 768 * 4 <= MAX_FRAMEBUFFER_BYTES);
        // 1920×1080×4 = 8,294,400 bytes — does NOT fit
        assert!(1920 * 1080 * 4 > MAX_FRAMEBUFFER_BYTES);
    }

    #[test]
    fn virtio_gpu_format_values() {
        assert_eq!(VirtioGpuFormat::B8G8R8A8Unorm as u32, 1);
        assert_eq!(VirtioGpuFormat::R8G8B8A8Unorm as u32, 67);
    }

    #[test]
    fn fence_flag_value() {
        assert_eq!(VIRTIO_GPU_FLAG_FENCE, 1);
    }

    // --- GPU Service IPC protocol tests (Phase 6 M20) ---

    #[test]
    fn gpu_request_size_fits_message() {
        assert!(
            size_of::<GpuRequest>() <= 256,
            "GpuRequest must fit in MAX_MESSAGE_SIZE"
        );
    }

    #[test]
    fn gpu_response_size_fits_message() {
        assert!(
            size_of::<GpuResponse>() <= 256,
            "GpuResponse must fit in MAX_MESSAGE_SIZE"
        );
    }

    #[test]
    fn gpu_request_zeroed() {
        let req = GpuRequest::zeroed();
        assert_eq!(req.command, 0);
        assert_eq!(req.resource_id, 0);
        assert_eq!(req.width, 0);
    }

    #[test]
    fn gpu_response_zeroed() {
        let resp = GpuResponse::zeroed();
        assert_eq!(resp.status, 0);
        assert_eq!(resp.resource_id, 0);
        assert_eq!(resp.fb_virt, 0);
    }

    #[test]
    fn gpu_response_error() {
        let resp = GpuResponse::error(GpuError::OutOfMemory);
        assert_eq!(resp.status, -4);
    }

    #[test]
    fn gpu_command_discriminants() {
        assert_eq!(GpuCommand::GetDisplayInfo as u32, 1);
        assert_eq!(GpuCommand::AllocateBuffer as u32, 2);
        assert_eq!(GpuCommand::ReleaseBuffer as u32, 3);
        assert_eq!(GpuCommand::Present as u32, 4);
        assert_eq!(GpuCommand::GetBufferInfo as u32, 5);
        assert_eq!(GpuCommand::SwapBuffers as u32, 6);
    }

    #[test]
    fn gpu_command_from_u32() {
        assert_eq!(GpuCommand::from_u32(1), Some(GpuCommand::GetDisplayInfo));
        assert_eq!(GpuCommand::from_u32(6), Some(GpuCommand::SwapBuffers));
        assert_eq!(GpuCommand::from_u32(0), None);
        assert_eq!(GpuCommand::from_u32(7), None);
        assert_eq!(GpuCommand::from_u32(u32::MAX), None);
    }

    #[test]
    fn gpu_error_status_round_trip() {
        let errors = [
            GpuError::DeviceNotFound,
            GpuError::InitFailed,
            GpuError::CommandFailed,
            GpuError::OutOfMemory,
            GpuError::InvalidResource,
            GpuError::ScanoutFailed,
            GpuError::Timeout,
            GpuError::ResolutionTooLarge,
        ];
        for err in errors {
            let status = err.to_status();
            assert!(status < 0, "error status must be negative");
            let recovered = GpuError::from_status(status);
            assert_eq!(recovered, Some(err), "round-trip failed for {:?}", err);
        }
    }

    #[test]
    fn gpu_error_from_status_success() {
        assert_eq!(GpuError::from_status(0), None);
    }

    #[test]
    fn gpu_error_from_status_unknown() {
        assert_eq!(GpuError::from_status(-99), None);
        assert_eq!(GpuError::from_status(1), None);
    }

    #[test]
    fn max_gpu_buffers() {
        assert_eq!(MAX_GPU_BUFFERS, 8);
    }

    // --- FenceTracker tests ---

    #[test]
    fn fence_tracker_new() {
        let ft = FenceTracker::new();
        assert_eq!(ft.next_id, 1);
        assert_eq!(ft.last_completed, 0);
    }

    #[test]
    fn fence_tracker_allocate_increments() {
        let mut ft = FenceTracker::new();
        assert_eq!(ft.allocate(), 1);
        assert_eq!(ft.allocate(), 2);
        assert_eq!(ft.allocate(), 3);
        assert_eq!(ft.next_id, 4);
    }

    #[test]
    fn fence_tracker_complete_updates() {
        let mut ft = FenceTracker::new();
        ft.complete(5);
        assert_eq!(ft.last_completed, 5);
        // Completing an older fence doesn't reduce last_completed.
        ft.complete(3);
        assert_eq!(ft.last_completed, 5);
        // Completing a newer fence advances.
        ft.complete(10);
        assert_eq!(ft.last_completed, 10);
    }

    #[test]
    fn fence_tracker_is_complete() {
        let mut ft = FenceTracker::new();
        let f1 = ft.allocate();
        let f2 = ft.allocate();
        let f3 = ft.allocate();

        assert!(!ft.is_complete(f1));
        assert!(!ft.is_complete(f2));
        assert!(!ft.is_complete(f3));

        ft.complete(f2);
        assert!(ft.is_complete(f1)); // f1 <= f2
        assert!(ft.is_complete(f2));
        assert!(!ft.is_complete(f3)); // f3 > f2
    }

    #[test]
    fn fence_tracker_zero_always_complete() {
        let ft = FenceTracker::new();
        // Fence 0 is always complete (0 <= 0).
        assert!(ft.is_complete(0));
    }
}
