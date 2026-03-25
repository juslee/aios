//! VirtIO-GPU 2D driver — MMIO legacy (v1) transport, polled I/O.
//!
//! Probes VirtIO MMIO slots for a GPU device (device_id=16), initializes the
//! controlq virtqueue, and provides 2D resource management (create, attach
//! backing, scanout, transfer, flush) via polled command submission.
//!
//! Per VirtIO spec §5.7 (GPU device), using legacy MMIO v1 transport.
//! Reuses the VirtIO common infrastructure from `virtio_common.rs`.

use shared::gpu::*;
use shared::order_for_pages;
use shared::storage::*;
use spin::Mutex;

use super::virtio_common::*;
use crate::arch::aarch64::mmu::{DIRECT_MAP_BASE, MMIO_BASE};
use crate::dtb::DeviceTree;

/// VirtIO-GPU config space: num_scanouts at config offset 8.
/// Legacy MMIO v1: config space starts at MMIO offset 0x100.
const GPU_CONFIG_NUM_SCANOUTS: usize = VIRTIO_MMIO_CONFIG_SPACE + 8;

/// Offset within the DMA command page where response data starts.
/// The 4K page is split: cmd at 0, response at 2048.
const RESP_OFFSET: usize = 2048;

/// Global VirtIO-GPU device instance.
static VIRTIO_GPU: Mutex<Option<VirtioGpu>> = Mutex::new(None);

/// VirtIO-GPU device state.
struct VirtioGpu {
    /// MMIO virtual base address (via TTBR1 MMIO mapping).
    base: usize,
    /// Descriptor table virtual address (via direct map).
    desc_virt: usize,
    /// Available ring virtual address.
    avail_virt: usize,
    /// Used ring virtual address.
    used_virt: usize,
    /// Command/response DMA page physical address.
    cmd_phys: usize,
    /// Command/response DMA page virtual address.
    cmd_virt: usize,
    /// Last seen used ring index.
    last_used_idx: u16,
    /// Negotiated virtqueue size.
    queue_size: u16,
    /// Next resource ID to allocate (starts at 1, 0 is reserved).
    next_resource_id: u32,
    /// Display information from GET_DISPLAY_INFO.
    display: DisplayInfo,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Probe for a VirtIO-GPU device and initialize it.
///
/// Returns `true` if a GPU device was found and initialized.
pub fn init(dt: &DeviceTree) -> bool {
    let phys_base = match probe(dt) {
        Some(base) => base,
        None => {
            crate::kinfo!(Gpu, "No VirtIO-GPU device found");
            return false;
        }
    };

    let virt_base = MMIO_BASE + phys_base;

    match init_device(virt_base) {
        Ok(gpu) => {
            crate::kinfo!(
                Gpu,
                "VirtIO-GPU: scanout {}: {}x{}",
                gpu.display.scanout_id,
                gpu.display.width,
                gpu.display.height
            );
            *VIRTIO_GPU.lock() = Some(gpu);
            true
        }
        Err(e) => {
            crate::kerror!(Gpu, "VirtIO-GPU init failed: {:?}", e);
            false
        }
    }
}

/// Get the display resolution, or None if no GPU device.
#[allow(dead_code)] // Used by M20 GPU Service
pub fn display_info() -> Option<DisplayInfo> {
    VIRTIO_GPU.lock().as_ref().map(|g| g.display)
}

/// Allocate a framebuffer and display it (solid color test frame).
///
/// Called from kernel_main after successful init to prove the display pipeline.
pub fn display_test_frame() -> Result<(), GpuError> {
    let mut guard = VIRTIO_GPU.lock();
    let gpu = guard.as_mut().ok_or(GpuError::DeviceNotFound)?;

    let width = gpu.display.width;
    let height = gpu.display.height;

    let handle = gpu.allocate_framebuffer(width, height)?;

    // Fill framebuffer with AIOS blue.
    // SAFETY: fb_virt points to zeroed DMA pages allocated by alloc_dma_pages.
    // The region is mapped RW via direct map. We fill width×height pixels.
    unsafe {
        let fb = handle.fb_virt as *mut u32;
        let pixel_count = (width * height) as usize;
        for i in 0..pixel_count {
            core::ptr::write_volatile(fb.add(i), AIOS_BLUE_B8G8R8A8);
        }
    }

    // Set scanout, transfer to host, flush.
    let rect = VirtioGpuRect {
        x: 0,
        y: 0,
        width,
        height,
    };
    gpu.set_scanout(0, handle.resource_id, &rect)?;
    gpu.present_frame(&handle)?;

    crate::kinfo!(
        Gpu,
        "VirtIO-GPU: first frame displayed ({}x{})",
        width,
        height
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Device probe
// ---------------------------------------------------------------------------

/// Find a VirtIO GPU device. DTB first, then brute-force MMIO scan.
fn probe(dt: &DeviceTree) -> Option<usize> {
    // Strategy 1: DTB-provided VirtIO MMIO bases.
    for i in 0..dt.virtio_mmio_count {
        let phys = dt.virtio_mmio_bases[i] as usize;
        if let Some(p) = probe_slot(phys) {
            crate::kinfo!(Gpu, "VirtIO-GPU: DTB slot {}, phys {:#x}", i, p);
            return Some(p);
        }
    }

    // Strategy 2: Brute-force scan of MMIO region.
    for slot in 0..VIRTIO_MMIO_SLOT_COUNT {
        let phys = VIRTIO_MMIO_REGION_BASE as usize + slot * VIRTIO_MMIO_REGION_STRIDE as usize;
        if let Some(p) = probe_slot(phys) {
            crate::kinfo!(Gpu, "VirtIO-GPU: MMIO scan slot {}, phys {:#x}", slot, p);
            return Some(p);
        }
    }

    None
}

/// Check if a VirtIO MMIO slot contains a GPU device.
/// Returns `Some(phys)` if found, `None` otherwise.
fn probe_slot(phys: usize) -> Option<usize> {
    let virt = MMIO_BASE + phys;
    // SAFETY: MMIO region 0x0-0x40000000 is mapped as device memory in TTBR1.
    // Each slot is 512 bytes apart, within the mapped range.
    unsafe {
        let magic = mmio_read32(virt + VIRTIO_MMIO_MAGIC_VALUE);
        if magic != VIRTIO_MMIO_MAGIC {
            return None;
        }
        let version = mmio_read32(virt + VIRTIO_MMIO_VERSION);
        if version != 1 {
            // Log version mismatch for GPU device IDs so we can diagnose
            // QEMU defaulting to modern transport.
            let device_id = mmio_read32(virt + VIRTIO_MMIO_DEVICE_ID);
            if device_id == VIRTIO_DEVICE_ID_GPU {
                crate::kwarn!(
                    Gpu,
                    "VirtIO-GPU: found at {:#x} but version={}, expected 1",
                    phys,
                    version
                );
            }
            return None;
        }
        let device_id = mmio_read32(virt + VIRTIO_MMIO_DEVICE_ID);
        if device_id == VIRTIO_DEVICE_ID_GPU {
            Some(phys)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Device initialization (VirtIO spec §3.1, legacy MMIO v1)
// ---------------------------------------------------------------------------

fn init_device(base: usize) -> Result<VirtioGpu, GpuError> {
    // SAFETY: All MMIO accesses are to a VirtIO device at a validated address.
    // The base address was confirmed to have correct magic, version, and device_id.
    unsafe {
        // 1. Reset device.
        mmio_write32(base + VIRTIO_MMIO_STATUS, 0);

        // 2. Set ACKNOWLEDGE.
        let mut status = VIRTIO_STATUS_ACKNOWLEDGE;
        mmio_write32(base + VIRTIO_MMIO_STATUS, status);

        // 3. Set DRIVER.
        status |= VIRTIO_STATUS_DRIVER;
        mmio_write32(base + VIRTIO_MMIO_STATUS, status);

        // 4. Read and negotiate features (2D only — no 3D features).
        let device_features = mmio_read32(base + VIRTIO_MMIO_DEVICE_FEATURES) as u64;
        crate::kinfo!(Gpu, "VirtIO-GPU: device features {:#010x}", device_features);

        // Write zero features (no 3D, no EDID).
        mmio_write32(base + VIRTIO_MMIO_DRIVER_FEATURES, 0);

        // 5. Set GUEST_PAGE_SIZE (legacy requirement).
        mmio_write32(base + VIRTIO_MMIO_GUEST_PAGE_SIZE, VIRT_PAGE_SIZE as u32);

        // 6. Set up controlq (queue 0).
        mmio_write32(base + VIRTIO_MMIO_QUEUE_SEL, 0);
        core::arch::asm!("dsb sy");

        let queue_num_max = mmio_read32(base + VIRTIO_MMIO_QUEUE_NUM_MAX);
        if queue_num_max == 0 {
            crate::kerror!(Gpu, "VirtIO-GPU: controlq not available");
            return Err(GpuError::InitFailed);
        }

        let queue_size = (QUEUE_SIZE as u32).min(queue_num_max) as u16;
        mmio_write32(base + VIRTIO_MMIO_QUEUE_NUM, queue_size as u32);
        mmio_write32(base + VIRTIO_MMIO_QUEUE_ALIGN, VIRT_PAGE_SIZE as u32);

        // Allocate contiguous DMA memory for the virtqueue.
        let total_bytes = virtqueue_size(queue_size as usize);
        let total_pages = total_bytes.div_ceil(VIRT_PAGE_SIZE);
        let order = order_for_pages(total_pages);

        let vq_phys = crate::mm::frame::alloc_dma_pages(order).ok_or(GpuError::OutOfMemory)?;
        let vq_virt = DIRECT_MAP_BASE + vq_phys;

        // Zero the entire DMA allocation.
        let alloc_pages = 1usize << order;
        core::ptr::write_bytes(vq_virt as *mut u8, 0, alloc_pages * VIRT_PAGE_SIZE);

        // Compute sub-structure addresses.
        let desc_virt = vq_virt;
        let avail_virt = vq_virt + avail_offset(queue_size as usize);
        let used_virt = vq_virt + used_offset(queue_size as usize);

        // Tell device the queue page frame number.
        let pfn = (vq_phys / VIRT_PAGE_SIZE) as u32;
        mmio_write32(base + VIRTIO_MMIO_QUEUE_PFN, pfn);

        // Allocate a separate DMA page for command/response buffers.
        let cmd_page_phys = crate::mm::frame::alloc_dma_page().ok_or(GpuError::OutOfMemory)?;
        let cmd_page_virt = DIRECT_MAP_BASE + cmd_page_phys;
        core::ptr::write_bytes(cmd_page_virt as *mut u8, 0, VIRT_PAGE_SIZE);

        // 7. Set DRIVER_OK.
        status |= VIRTIO_STATUS_DRIVER_OK;
        mmio_write32(base + VIRTIO_MMIO_STATUS, status);

        // 8. Read config space: num_scanouts.
        let num_scanouts = mmio_read32(base + GPU_CONFIG_NUM_SCANOUTS);
        crate::kinfo!(Gpu, "VirtIO-GPU: initialized, {} scanouts", num_scanouts);

        let mut gpu = VirtioGpu {
            base,
            desc_virt,
            avail_virt,
            used_virt,
            cmd_phys: cmd_page_phys,
            cmd_virt: cmd_page_virt,
            last_used_idx: 0,
            queue_size,
            next_resource_id: 1,
            display: DisplayInfo::default(),
        };

        // Query display info.
        gpu.display = gpu.get_display_info()?;

        Ok(gpu)
    }
}

// ---------------------------------------------------------------------------
// Command submission
// ---------------------------------------------------------------------------

impl VirtioGpu {
    /// Submit a command on the controlq and poll for the response.
    ///
    /// Copies `cmd` to the DMA page at offset 0, sets up a 2-descriptor chain
    /// (device-readable cmd, device-writable response), notifies device, and
    /// polls for completion. Response is copied to `resp`.
    fn submit_command(&mut self, cmd: &[u8], resp: &mut [u8]) -> Result<(), GpuError> {
        let cmd_len = cmd.len();
        let resp_len = resp.len();
        assert!(cmd_len <= RESP_OFFSET);
        assert!(resp_len <= VIRT_PAGE_SIZE - RESP_OFFSET);

        // SAFETY: cmd_virt points to a zeroed DMA page. We copy command data
        // to offset 0 and response area at RESP_OFFSET. Both are within the page.
        // Physical addresses in descriptors are valid for VirtIO device DMA.
        unsafe {
            let cmd_virt = self.cmd_virt;
            let cmd_phys = self.cmd_phys;

            // Copy command to DMA page.
            core::ptr::copy_nonoverlapping(cmd.as_ptr(), cmd_virt as *mut u8, cmd_len);

            // Zero response area.
            core::ptr::write_bytes((cmd_virt + RESP_OFFSET) as *mut u8, 0, resp_len);

            // Build 2-descriptor chain.
            let desc_base = self.desc_virt as *mut VirtqDesc;

            // Descriptor 0: command (device-readable).
            core::ptr::write_volatile(
                desc_base.add(0),
                VirtqDesc {
                    addr: cmd_phys as u64,
                    len: cmd_len as u32,
                    flags: VIRTQ_DESC_F_NEXT,
                    next: 1,
                },
            );

            // Descriptor 1: response (device-writable).
            core::ptr::write_volatile(
                desc_base.add(1),
                VirtqDesc {
                    addr: (cmd_phys + RESP_OFFSET) as u64,
                    len: resp_len as u32,
                    flags: VIRTQ_DESC_F_WRITE,
                    next: 0,
                },
            );

            // Add descriptor chain head to available ring.
            let avail_idx = core::ptr::read_volatile((self.avail_virt + 2) as *const u16);
            let ring_offset = 4 + (avail_idx % self.queue_size) as usize * 2;
            core::ptr::write_volatile(
                (self.avail_virt + ring_offset) as *mut u16,
                0, // chain head = descriptor 0
            );

            core::arch::asm!("dsb sy");
            core::ptr::write_volatile((self.avail_virt + 2) as *mut u16, avail_idx.wrapping_add(1));
            core::arch::asm!("dsb sy");

            // Notify device (queue 0).
            mmio_write32(self.base + VIRTIO_MMIO_QUEUE_NOTIFY, 0);

            // Poll for completion.
            let expected_idx = self.last_used_idx.wrapping_add(1);
            let mut timeout = POLL_TIMEOUT;
            loop {
                core::arch::asm!("dsb sy");
                let used_idx = core::ptr::read_volatile((self.used_virt + 2) as *const u16);
                if used_idx == expected_idx {
                    break;
                }
                timeout -= 1;
                if timeout == 0 {
                    crate::kerror!(Gpu, "VirtIO-GPU: poll timeout");
                    return Err(GpuError::Timeout);
                }
            }
            self.last_used_idx = expected_idx;

            // Copy response from DMA page.
            core::ptr::copy_nonoverlapping(
                (cmd_virt + RESP_OFFSET) as *const u8,
                resp.as_mut_ptr(),
                resp_len,
            );

            Ok(())
        }
    }

    /// Submit a command with extra data (3-descriptor chain).
    ///
    /// Used for RESOURCE_ATTACH_BACKING where the command header is followed
    /// by a separate data block (mem_entry array).
    fn submit_command_with_extra(
        &mut self,
        cmd: &[u8],
        extra: &[u8],
        resp: &mut [u8],
    ) -> Result<(), GpuError> {
        let cmd_len = cmd.len();
        let extra_len = extra.len();
        let resp_len = resp.len();
        assert!(cmd_len + extra_len <= RESP_OFFSET);
        assert!(resp_len <= VIRT_PAGE_SIZE - RESP_OFFSET);

        // SAFETY: Same as submit_command. Extra data is placed at cmd_len offset.
        unsafe {
            let cmd_virt = self.cmd_virt;
            let cmd_phys = self.cmd_phys;

            // Copy command header.
            core::ptr::copy_nonoverlapping(cmd.as_ptr(), cmd_virt as *mut u8, cmd_len);
            // Copy extra data after header.
            core::ptr::copy_nonoverlapping(
                extra.as_ptr(),
                (cmd_virt + cmd_len) as *mut u8,
                extra_len,
            );
            // Zero response area.
            core::ptr::write_bytes((cmd_virt + RESP_OFFSET) as *mut u8, 0, resp_len);

            // Build 3-descriptor chain.
            let desc_base = self.desc_virt as *mut VirtqDesc;

            // Descriptor 0: command header (device-readable).
            core::ptr::write_volatile(
                desc_base.add(0),
                VirtqDesc {
                    addr: cmd_phys as u64,
                    len: cmd_len as u32,
                    flags: VIRTQ_DESC_F_NEXT,
                    next: 1,
                },
            );

            // Descriptor 1: extra data (device-readable).
            core::ptr::write_volatile(
                desc_base.add(1),
                VirtqDesc {
                    addr: (cmd_phys + cmd_len) as u64,
                    len: extra_len as u32,
                    flags: VIRTQ_DESC_F_NEXT,
                    next: 2,
                },
            );

            // Descriptor 2: response (device-writable).
            core::ptr::write_volatile(
                desc_base.add(2),
                VirtqDesc {
                    addr: (cmd_phys + RESP_OFFSET) as u64,
                    len: resp_len as u32,
                    flags: VIRTQ_DESC_F_WRITE,
                    next: 0,
                },
            );

            // Add chain head to available ring.
            let avail_idx = core::ptr::read_volatile((self.avail_virt + 2) as *const u16);
            let ring_offset = 4 + (avail_idx % self.queue_size) as usize * 2;
            core::ptr::write_volatile((self.avail_virt + ring_offset) as *mut u16, 0);

            core::arch::asm!("dsb sy");
            core::ptr::write_volatile((self.avail_virt + 2) as *mut u16, avail_idx.wrapping_add(1));
            core::arch::asm!("dsb sy");

            mmio_write32(self.base + VIRTIO_MMIO_QUEUE_NOTIFY, 0);

            // Poll for completion.
            let expected_idx = self.last_used_idx.wrapping_add(1);
            let mut timeout = POLL_TIMEOUT;
            loop {
                core::arch::asm!("dsb sy");
                let used_idx = core::ptr::read_volatile((self.used_virt + 2) as *const u16);
                if used_idx == expected_idx {
                    break;
                }
                timeout -= 1;
                if timeout == 0 {
                    crate::kerror!(Gpu, "VirtIO-GPU: poll timeout (extra)");
                    return Err(GpuError::Timeout);
                }
            }
            self.last_used_idx = expected_idx;

            core::ptr::copy_nonoverlapping(
                (cmd_virt + RESP_OFFSET) as *const u8,
                resp.as_mut_ptr(),
                resp_len,
            );

            Ok(())
        }
    }

    /// Check response header for success.
    fn check_response(resp: &[u8]) -> Result<(), GpuError> {
        if resp.len() < 4 {
            return Err(GpuError::CommandFailed);
        }
        let type_ = u32::from_le_bytes([resp[0], resp[1], resp[2], resp[3]]);
        if type_ == VIRTIO_GPU_RESP_OK_NODATA || type_ == VIRTIO_GPU_RESP_OK_DISPLAY_INFO {
            Ok(())
        } else {
            crate::kerror!(Gpu, "VirtIO-GPU: error response {:#x}", type_);
            Err(GpuError::CommandFailed)
        }
    }
}

// ---------------------------------------------------------------------------
// GPU 2D commands
// ---------------------------------------------------------------------------

impl VirtioGpu {
    /// Query display info from the device.
    fn get_display_info(&mut self) -> Result<DisplayInfo, GpuError> {
        let cmd = VirtioGpuCtrlHdr {
            type_: VIRTIO_GPU_CMD_GET_DISPLAY_INFO,
            flags: 0,
            fence_id: 0,
            ctx_id: 0,
            ring_idx: 0,
            padding: [0; 3],
        };

        let cmd_bytes = as_bytes(&cmd);
        let mut resp_bytes = [0u8; core::mem::size_of::<VirtioGpuRespDisplayInfo>()];

        self.submit_command(cmd_bytes, &mut resp_bytes)?;
        Self::check_response(&resp_bytes)?;

        // Parse response: find first enabled scanout.
        // SAFETY: resp_bytes is exactly the size of VirtioGpuRespDisplayInfo,
        // which is a repr(C) struct with known layout. The bytes were written
        // by the VirtIO device and we verified the response type.
        let resp = unsafe { &*(resp_bytes.as_ptr() as *const VirtioGpuRespDisplayInfo) };

        for (i, pmode) in resp.pmodes.iter().enumerate() {
            if pmode.enabled != 0 && pmode.r.width > 0 && pmode.r.height > 0 {
                return Ok(DisplayInfo {
                    width: pmode.r.width,
                    height: pmode.r.height,
                    format: GpuPixelFormat::B8G8R8A8,
                    scanout_id: i as u32,
                });
            }
        }

        // No enabled scanout — use fallback.
        crate::kwarn!(
            Gpu,
            "VirtIO-GPU: no enabled scanout, using 1024x768 fallback"
        );
        Ok(DisplayInfo {
            width: 1024,
            height: 768,
            format: GpuPixelFormat::B8G8R8A8,
            scanout_id: 0,
        })
    }

    /// Create a 2D resource on the host GPU.
    fn resource_create_2d(
        &mut self,
        resource_id: u32,
        format: u32,
        width: u32,
        height: u32,
    ) -> Result<(), GpuError> {
        let cmd = VirtioGpuResourceCreate2d {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_RESOURCE_CREATE_2D,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                ring_idx: 0,
                padding: [0; 3],
            },
            resource_id,
            format,
            width,
            height,
        };

        let mut resp = [0u8; core::mem::size_of::<VirtioGpuCtrlHdr>()];
        self.submit_command(as_bytes(&cmd), &mut resp)?;
        Self::check_response(&resp)?;

        crate::kinfo!(
            Gpu,
            "VirtIO-GPU: resource {} created ({}x{} fmt={})",
            resource_id,
            width,
            height,
            format
        );
        Ok(())
    }

    /// Attach guest DMA pages to a resource.
    fn resource_attach_backing(
        &mut self,
        resource_id: u32,
        entries: &[VirtioGpuMemEntry],
    ) -> Result<(), GpuError> {
        let cmd = VirtioGpuResourceAttachBacking {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                ring_idx: 0,
                padding: [0; 3],
            },
            resource_id,
            nr_entries: entries.len() as u32,
        };

        let extra = as_byte_slice(entries);
        let mut resp = [0u8; core::mem::size_of::<VirtioGpuCtrlHdr>()];
        self.submit_command_with_extra(as_bytes(&cmd), extra, &mut resp)?;
        Self::check_response(&resp)?;

        let total_pages: u32 = entries
            .iter()
            .map(|e| e.length / VIRT_PAGE_SIZE as u32)
            .sum();
        crate::kinfo!(
            Gpu,
            "VirtIO-GPU: backing attached (resource={}, {} pages)",
            resource_id,
            total_pages
        );
        Ok(())
    }

    /// Detach guest DMA pages from a resource.
    #[allow(dead_code)]
    fn resource_detach_backing(&mut self, resource_id: u32) -> Result<(), GpuError> {
        let cmd = VirtioGpuResourceDetachBacking {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                ring_idx: 0,
                padding: [0; 3],
            },
            resource_id,
            padding: 0,
        };

        let mut resp = [0u8; core::mem::size_of::<VirtioGpuCtrlHdr>()];
        self.submit_command(as_bytes(&cmd), &mut resp)?;
        Self::check_response(&resp)
    }

    /// Destroy a resource.
    #[allow(dead_code)]
    fn resource_unref(&mut self, resource_id: u32) -> Result<(), GpuError> {
        let cmd = VirtioGpuResourceUnref {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_RESOURCE_UNREF,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                ring_idx: 0,
                padding: [0; 3],
            },
            resource_id,
            padding: 0,
        };

        let mut resp = [0u8; core::mem::size_of::<VirtioGpuCtrlHdr>()];
        self.submit_command(as_bytes(&cmd), &mut resp)?;
        Self::check_response(&resp)
    }

    /// Bind a resource to a display scanout.
    fn set_scanout(
        &mut self,
        scanout_id: u32,
        resource_id: u32,
        rect: &VirtioGpuRect,
    ) -> Result<(), GpuError> {
        let cmd = VirtioGpuSetScanout {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_SET_SCANOUT,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                ring_idx: 0,
                padding: [0; 3],
            },
            r: *rect,
            scanout_id,
            resource_id,
        };

        let mut resp = [0u8; core::mem::size_of::<VirtioGpuCtrlHdr>()];
        self.submit_command(as_bytes(&cmd), &mut resp)?;
        Self::check_response(&resp)
    }

    /// Transfer pixel data from guest backing to host resource.
    fn transfer_to_host_2d(
        &mut self,
        resource_id: u32,
        rect: &VirtioGpuRect,
        offset: u64,
    ) -> Result<(), GpuError> {
        let cmd = VirtioGpuTransferToHost2d {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                ring_idx: 0,
                padding: [0; 3],
            },
            r: *rect,
            offset,
            resource_id,
            padding: 0,
        };

        let mut resp = [0u8; core::mem::size_of::<VirtioGpuCtrlHdr>()];
        self.submit_command(as_bytes(&cmd), &mut resp)?;
        Self::check_response(&resp)
    }

    /// Flush a resource region to the display.
    fn resource_flush(&mut self, resource_id: u32, rect: &VirtioGpuRect) -> Result<(), GpuError> {
        let cmd = VirtioGpuResourceFlush {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_RESOURCE_FLUSH,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                ring_idx: 0,
                padding: [0; 3],
            },
            r: *rect,
            resource_id,
            padding: 0,
        };

        let mut resp = [0u8; core::mem::size_of::<VirtioGpuCtrlHdr>()];
        self.submit_command(as_bytes(&cmd), &mut resp)?;
        Self::check_response(&resp)
    }

    /// Transfer to host and flush for the full framebuffer.
    fn present_frame(&mut self, handle: &GpuBufferHandle) -> Result<(), GpuError> {
        let rect = VirtioGpuRect {
            x: 0,
            y: 0,
            width: handle.width,
            height: handle.height,
        };
        self.transfer_to_host_2d(handle.resource_id, &rect, 0)?;
        self.resource_flush(handle.resource_id, &rect)
    }

    /// Allocate a DMA-backed framebuffer and create a VirtIO-GPU resource.
    fn allocate_framebuffer(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<GpuBufferHandle, GpuError> {
        let bpp = GpuPixelFormat::B8G8R8A8.bytes_per_pixel();
        let total_bytes = (width as usize) * (height as usize) * (bpp as usize);

        if total_bytes > MAX_FRAMEBUFFER_BYTES {
            crate::kwarn!(
                Gpu,
                "VirtIO-GPU: framebuffer {}x{} = {} bytes > {} max, clamping",
                width,
                height,
                total_bytes,
                MAX_FRAMEBUFFER_BYTES
            );
            return Err(GpuError::ResolutionTooLarge);
        }

        let page_count = total_bytes.div_ceil(VIRT_PAGE_SIZE);
        let order = order_for_pages(page_count);

        let fb_phys = crate::mm::frame::alloc_dma_pages(order).ok_or(GpuError::OutOfMemory)?;
        let fb_virt = DIRECT_MAP_BASE + fb_phys;

        // Zero the framebuffer pages.
        // SAFETY: fb_virt points to freshly allocated DMA pages, mapped via direct map.
        unsafe {
            let alloc_pages = 1usize << order;
            core::ptr::write_bytes(fb_virt as *mut u8, 0, alloc_pages * VIRT_PAGE_SIZE);
        }

        let resource_id = self.next_resource_id;
        self.next_resource_id += 1;

        // Create 2D resource.
        self.resource_create_2d(
            resource_id,
            GpuPixelFormat::B8G8R8A8.to_virtio(),
            width,
            height,
        )?;

        // Attach single contiguous DMA region as backing.
        let mem_entry = VirtioGpuMemEntry {
            addr: fb_phys as u64,
            length: total_bytes as u32,
            padding: 0,
        };
        self.resource_attach_backing(resource_id, &[mem_entry])?;

        let stride = width * bpp;

        Ok(GpuBufferHandle {
            resource_id,
            width,
            height,
            format: GpuPixelFormat::B8G8R8A8,
            stride,
            fb_phys,
            fb_virt,
            page_count,
            order,
        })
    }
}

// ---------------------------------------------------------------------------
// Byte-casting helpers
// ---------------------------------------------------------------------------

/// View a `repr(C)` struct as a byte slice.
fn as_bytes<T: Sized>(val: &T) -> &[u8] {
    // SAFETY: repr(C) structs have a defined layout. The resulting slice
    // covers exactly size_of::<T>() bytes from the struct's address.
    unsafe { core::slice::from_raw_parts(val as *const T as *const u8, core::mem::size_of::<T>()) }
}

/// View a slice of `repr(C)` structs as a byte slice.
fn as_byte_slice<T: Sized>(slice: &[T]) -> &[u8] {
    // SAFETY: repr(C) slice elements are contiguous in memory.
    unsafe {
        core::slice::from_raw_parts(slice.as_ptr() as *const u8, core::mem::size_of_val(slice))
    }
}
