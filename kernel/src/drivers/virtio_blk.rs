//! VirtIO-blk driver — MMIO legacy (v1) transport, polled I/O.
//!
//! Probes VirtIO MMIO slots (DTB first, brute-force fallback) for a block
//! device (device_id=2), initializes the virtqueue, and provides synchronous
//! sector-level read/write via polled completion.
//!
//! Per VirtIO spec §3.1 (device init), §4.2.2 (MMIO transport), §5.2 (block device).
//! Uses legacy (v1) MMIO register layout (QUEUE_PFN, GUEST_PAGE_SIZE).

use shared::storage::*;
use spin::Mutex;

use crate::arch::aarch64::mmu::{DIRECT_MAP_BASE, MMIO_BASE};
use crate::dtb::DeviceTree;

/// Virtqueue size (number of descriptors). Must be ≤ QUEUE_NUM_MAX from device.
const QUEUE_SIZE: u16 = 128;

/// Polling timeout iterations for virtqueue completion.
const POLL_TIMEOUT: u32 = 10_000_000;

/// Page size used for legacy VirtIO MMIO queue alignment.
const VIRT_PAGE_SIZE: usize = 4096;

/// Global VirtIO-blk device instance.
static VIRTIO_BLK: Mutex<Option<VirtioBlk>> = Mutex::new(None);

/// VirtIO block device state.
struct VirtioBlk {
    /// MMIO virtual base address (via TTBR1 MMIO mapping).
    base: usize,
    /// Device capacity in 512-byte sectors.
    capacity_sectors: u64,
    /// Descriptor table virtual address (via direct map).
    desc_virt: usize,
    /// Available ring virtual address.
    avail_virt: usize,
    /// Used ring virtual address.
    used_virt: usize,
    /// Request buffer physical address (header + data + status).
    req_phys: usize,
    /// Request buffer virtual address.
    req_virt: usize,
    /// Last seen used ring index.
    last_used_idx: u16,
}

// ---------------------------------------------------------------------------
// MMIO helpers (same pattern as uart.rs)
// ---------------------------------------------------------------------------

/// Read a 32-bit MMIO register.
///
/// # Safety
/// `addr` must be a valid MMIO register address mapped as device memory
/// (e.g., via the TTBR1 MMIO map at `MMIO_BASE + phys`).
#[inline(always)]
unsafe fn mmio_read32(addr: usize) -> u32 {
    core::ptr::read_volatile(addr as *const u32)
}

/// Write a 32-bit MMIO register.
///
/// # Safety
/// `addr` must be a valid MMIO register address mapped as device memory.
#[inline(always)]
unsafe fn mmio_write32(addr: usize, val: u32) {
    core::ptr::write_volatile(addr as *mut u32, val);
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Probe for a VirtIO-blk device and initialize it.
///
/// Returns `true` if a block device was found and initialized.
/// Stores the device in the global `VIRTIO_BLK` mutex.
pub fn init(dt: &DeviceTree) -> bool {
    let phys_base = match probe(dt) {
        Some(base) => base,
        None => {
            crate::kwarn!(Storage, "No VirtIO-blk device found");
            return false;
        }
    };

    let virt_base = MMIO_BASE + phys_base;

    match init_device(virt_base) {
        Ok(blk) => {
            let cap = blk.capacity_sectors;
            let cap_mb = (cap * SECTOR_SIZE as u64) / (1024 * 1024);
            crate::kinfo!(
                Storage,
                "VirtIO-blk: capacity={} sectors ({} MiB)",
                cap,
                cap_mb
            );
            *VIRTIO_BLK.lock() = Some(blk);
            true
        }
        Err(e) => {
            crate::kerror!(Storage, "VirtIO-blk init failed: {:?}", e);
            false
        }
    }
}

/// Read a single 512-byte sector from disk.
pub fn read_sector(sector: u64, buf: &mut [u8; 512]) -> Result<(), StorageError> {
    let mut guard = VIRTIO_BLK.lock();
    let blk = guard.as_mut().ok_or(StorageError::DeviceNotFound)?;
    if sector >= blk.capacity_sectors {
        return Err(StorageError::IoError);
    }
    submit_request(blk, VIRTIO_BLK_T_IN, sector, buf)
}

/// Write a single 512-byte sector to disk.
pub fn write_sector(sector: u64, buf: &[u8; 512]) -> Result<(), StorageError> {
    let mut guard = VIRTIO_BLK.lock();
    let blk = guard.as_mut().ok_or(StorageError::DeviceNotFound)?;
    if sector >= blk.capacity_sectors {
        return Err(StorageError::IoError);
    }
    // Copy into mutable buffer for submit_request (DMA buffer is always mutable).
    let mut data = *buf;
    submit_request(blk, VIRTIO_BLK_T_OUT, sector, &mut data)
}

/// Device capacity in sectors, or 0 if no device.
#[allow(dead_code)]
pub fn capacity_sectors() -> u64 {
    VIRTIO_BLK.lock().as_ref().map_or(0, |b| b.capacity_sectors)
}

// ---------------------------------------------------------------------------
// Device probe
// ---------------------------------------------------------------------------

/// Find a VirtIO block device. DTB first, then brute-force MMIO scan.
fn probe(dt: &DeviceTree) -> Option<usize> {
    // Strategy 1: DTB-provided VirtIO MMIO bases.
    for i in 0..dt.virtio_mmio_count {
        let phys = dt.virtio_mmio_bases[i] as usize;
        if probe_slot(phys) {
            crate::kinfo!(Storage, "VirtIO-blk: DTB slot {}, phys {:#x}", i, phys);
            return Some(phys);
        }
    }

    // Strategy 2: Brute-force scan of MMIO region.
    for slot in 0..VIRTIO_MMIO_SLOT_COUNT {
        let phys = VIRTIO_MMIO_REGION_BASE as usize + slot * VIRTIO_MMIO_REGION_STRIDE as usize;
        if probe_slot(phys) {
            crate::kinfo!(
                Storage,
                "VirtIO-blk: MMIO scan slot {}, phys {:#x}",
                slot,
                phys
            );
            return Some(phys);
        }
    }

    None
}

/// Check if a VirtIO MMIO slot contains a block device.
fn probe_slot(phys: usize) -> bool {
    let virt = MMIO_BASE + phys;
    // SAFETY: MMIO region 0x0-0x40000000 is mapped as device memory in TTBR1.
    unsafe {
        let magic = mmio_read32(virt + VIRTIO_MMIO_MAGIC_VALUE);
        if magic != VIRTIO_MMIO_MAGIC {
            return false;
        }
        let device_id = mmio_read32(virt + VIRTIO_MMIO_DEVICE_ID);
        device_id == VIRTIO_DEVICE_ID_BLK
    }
}

// ---------------------------------------------------------------------------
// Legacy virtqueue layout helpers
// ---------------------------------------------------------------------------

/// Calculate the byte offset of the available ring from the start of the
/// virtqueue allocation (immediately after the descriptor table).
const fn avail_offset(queue_size: usize) -> usize {
    // Descriptor table: queue_size × 16 bytes.
    queue_size * 16
}

/// Calculate the byte offset of the used ring from the start of the
/// virtqueue allocation (page-aligned after the available ring).
const fn used_offset(queue_size: usize) -> usize {
    // Available ring: 4 bytes header + queue_size × 2 bytes + 2 bytes used_event.
    let avail_end = avail_offset(queue_size) + 4 + queue_size * 2 + 2;
    // Align up to page boundary.
    (avail_end + VIRT_PAGE_SIZE - 1) & !(VIRT_PAGE_SIZE - 1)
}

/// Total size of the virtqueue allocation in bytes.
const fn virtqueue_size(queue_size: usize) -> usize {
    // Used ring: 4 bytes header + queue_size × 8 bytes + 2 bytes avail_event.
    let used_end = used_offset(queue_size) + 4 + queue_size * 8 + 2;
    // Align up to page boundary.
    (used_end + VIRT_PAGE_SIZE - 1) & !(VIRT_PAGE_SIZE - 1)
}

// ---------------------------------------------------------------------------
// Device initialization (VirtIO spec §3.1, legacy MMIO v1)
// ---------------------------------------------------------------------------

fn init_device(base: usize) -> Result<VirtioBlk, StorageError> {
    // SAFETY: All MMIO accesses are to a VirtIO device at a validated address.
    // The base address was confirmed to have correct magic and device_id.
    unsafe {
        // 1. Reset device.
        mmio_write32(base + VIRTIO_MMIO_STATUS, 0);

        // 2. Set ACKNOWLEDGE.
        let mut status = VIRTIO_STATUS_ACKNOWLEDGE;
        mmio_write32(base + VIRTIO_MMIO_STATUS, status);

        // 3. Set DRIVER.
        status |= VIRTIO_STATUS_DRIVER;
        mmio_write32(base + VIRTIO_MMIO_STATUS, status);

        // 4. Read and negotiate features (legacy: no SEL registers, only low 32 bits).
        // Legacy MMIO v1: DEVICE_FEATURES at 0x010 returns bits 0-31 directly.
        // DEVICE_FEATURES_SEL does not exist in v1, but writing it is harmless.
        let device_features = mmio_read32(base + VIRTIO_MMIO_DEVICE_FEATURES) as u64;
        crate::kinfo!(
            Storage,
            "VirtIO-blk: device features {:#010x}",
            device_features
        );

        // Acknowledge features we understand.
        let mut driver_features: u32 = 0;
        if device_features & VIRTIO_BLK_F_SIZE_MAX != 0 {
            driver_features |= VIRTIO_BLK_F_SIZE_MAX as u32;
        }
        if device_features & VIRTIO_BLK_F_SEG_MAX != 0 {
            driver_features |= VIRTIO_BLK_F_SEG_MAX as u32;
        }
        if device_features & VIRTIO_BLK_F_BLK_SIZE != 0 {
            driver_features |= VIRTIO_BLK_F_BLK_SIZE as u32;
        }

        // Write driver features (legacy: DRIVER_FEATURES at 0x020).
        mmio_write32(base + VIRTIO_MMIO_DRIVER_FEATURES, driver_features);

        // Legacy v1: no FEATURES_OK step. Proceed directly to queue setup.

        // 5. Set GUEST_PAGE_SIZE (legacy requirement).
        mmio_write32(base + VIRTIO_MMIO_GUEST_PAGE_SIZE, VIRT_PAGE_SIZE as u32);

        // 6. Set up virtqueue 0.
        mmio_write32(base + VIRTIO_MMIO_QUEUE_SEL, 0);
        core::arch::asm!("dsb sy");

        let queue_num_max = mmio_read32(base + VIRTIO_MMIO_QUEUE_NUM_MAX);
        if queue_num_max == 0 {
            crate::kerror!(Storage, "VirtIO-blk: queue not available");
            return Err(StorageError::VirtioError);
        }

        let queue_size = (QUEUE_SIZE as u32).min(queue_num_max) as u16;
        mmio_write32(base + VIRTIO_MMIO_QUEUE_NUM, queue_size as u32);

        // Set queue alignment (legacy).
        mmio_write32(base + VIRTIO_MMIO_QUEUE_ALIGN, VIRT_PAGE_SIZE as u32);

        // Allocate contiguous DMA memory for the virtqueue.
        // Legacy layout: descriptors | avail ring | (page-align) | used ring
        let total_bytes = virtqueue_size(queue_size as usize);
        let total_pages = total_bytes.div_ceil(VIRT_PAGE_SIZE);
        // Compute order for buddy allocation (ceil log2 of pages).
        let order = order_for_pages(total_pages);

        let vq_phys = crate::mm::frame::alloc_dma_pages(order).ok_or(StorageError::IoError)?;
        let vq_virt = DIRECT_MAP_BASE + vq_phys;

        // Zero the entire virtqueue allocation.
        core::ptr::write_bytes(vq_virt as *mut u8, 0, total_pages * VIRT_PAGE_SIZE);

        // Compute sub-structure addresses.
        let desc_virt = vq_virt;
        let avail_virt = vq_virt + avail_offset(queue_size as usize);
        let used_virt = vq_virt + used_offset(queue_size as usize);

        // Tell device the queue page frame number (physical address / page_size).
        let pfn = (vq_phys / VIRT_PAGE_SIZE) as u32;
        mmio_write32(base + VIRTIO_MMIO_QUEUE_PFN, pfn);

        // Allocate a separate DMA page for request buffers (header + data + status).
        let req_page_phys = crate::mm::frame::alloc_dma_page().ok_or(StorageError::IoError)?;
        let req_page_virt = DIRECT_MAP_BASE + req_page_phys;
        core::ptr::write_bytes(req_page_virt as *mut u8, 0, VIRT_PAGE_SIZE);

        // 7. Set DRIVER_OK.
        status |= VIRTIO_STATUS_DRIVER_OK;
        mmio_write32(base + VIRTIO_MMIO_STATUS, status);

        // 8. Read capacity from config space (u64 LE at offset 0x100).
        let cap_lo = mmio_read32(base + VIRTIO_MMIO_CONFIG_SPACE) as u64;
        let cap_hi = mmio_read32(base + VIRTIO_MMIO_CONFIG_SPACE + 4) as u64;
        let capacity_sectors = cap_lo | (cap_hi << 32);

        Ok(VirtioBlk {
            base,
            capacity_sectors,
            desc_virt,
            avail_virt,
            used_virt,
            req_phys: req_page_phys,
            req_virt: req_page_virt,
            last_used_idx: 0,
        })
    }
}

/// Compute the buddy allocator order for the given number of pages.
/// Returns the smallest `order` such that `2^order >= pages`.
fn order_for_pages(pages: usize) -> usize {
    if pages <= 1 {
        return 0;
    }
    let mut order = 0;
    let mut size = 1;
    while size < pages {
        order += 1;
        size <<= 1;
    }
    order
}

// ---------------------------------------------------------------------------
// I/O submission
// ---------------------------------------------------------------------------

/// Submit a read or write request via the virtqueue and poll for completion.
///
/// Uses a 3-descriptor chain:
///   desc[0]: request header (device-readable)
///   desc[1]: data buffer (device-readable for write, device-writable for read)
///   desc[2]: status byte (device-writable)
fn submit_request(
    blk: &mut VirtioBlk,
    req_type: u32,
    sector: u64,
    data: &mut [u8; 512],
) -> Result<(), StorageError> {
    // SAFETY: All pointer arithmetic targets DMA pages allocated by alloc_dma_page().
    // The pages are mapped via DIRECT_MAP_BASE in TTBR1 (WB, QEMU DMA-coherent).
    // Physical addresses in descriptors are valid for VirtIO device DMA.
    unsafe {
        let req_virt = blk.req_virt;
        let req_phys = blk.req_phys;

        // Write request header at req_virt[0..16].
        let header = VirtioBlkReqHeader {
            req_type,
            reserved: 0,
            sector,
        };
        core::ptr::write_volatile(req_virt as *mut VirtioBlkReqHeader, header);

        // Data buffer at req_virt[16..528].
        let data_phys = req_phys + 16;
        let data_virt = req_virt + 16;
        if req_type == VIRTIO_BLK_T_OUT {
            // Write: copy data to DMA buffer.
            core::ptr::copy_nonoverlapping(data.as_ptr(), data_virt as *mut u8, 512);
        }

        // Status byte at req_virt[528].
        let status_phys = req_phys + 528;
        let status_virt = req_virt + 528;
        core::ptr::write_volatile(status_virt as *mut u8, 0xFF); // sentinel

        // Build 3-descriptor chain.
        let desc_base = blk.desc_virt as *mut VirtqDesc;

        // Descriptor 0: request header (device-readable).
        core::ptr::write_volatile(
            desc_base.add(0),
            VirtqDesc {
                addr: req_phys as u64,
                len: 16,
                flags: VIRTQ_DESC_F_NEXT,
                next: 1,
            },
        );

        // Descriptor 1: data buffer.
        let data_flags = if req_type == VIRTIO_BLK_T_IN {
            VIRTQ_DESC_F_NEXT | VIRTQ_DESC_F_WRITE // device writes to buffer
        } else {
            VIRTQ_DESC_F_NEXT // device reads from buffer
        };
        core::ptr::write_volatile(
            desc_base.add(1),
            VirtqDesc {
                addr: data_phys as u64,
                len: 512,
                flags: data_flags,
                next: 2,
            },
        );

        // Descriptor 2: status byte (device-writable).
        core::ptr::write_volatile(
            desc_base.add(2),
            VirtqDesc {
                addr: status_phys as u64,
                len: 1,
                flags: VIRTQ_DESC_F_WRITE,
                next: 0,
            },
        );

        // Add descriptor chain head (index 0) to available ring.
        // Available ring layout: [flags: u16, idx: u16, ring[N]: u16, used_event: u16]
        let avail_virt = blk.avail_virt;
        let avail_idx = core::ptr::read_volatile((avail_virt + 2) as *const u16);
        let ring_offset = 4 + (avail_idx % QUEUE_SIZE) as usize * 2;
        core::ptr::write_volatile(
            (avail_virt + ring_offset) as *mut u16,
            0, // descriptor chain head = index 0
        );

        // Memory barrier before updating avail_idx.
        core::arch::asm!("dsb sy");

        // Increment available ring index.
        core::ptr::write_volatile((avail_virt + 2) as *mut u16, avail_idx.wrapping_add(1));

        // Memory barrier before notifying device.
        core::arch::asm!("dsb sy");

        // Notify the device (write queue index to doorbell).
        mmio_write32(blk.base + VIRTIO_MMIO_QUEUE_NOTIFY, 0);

        // Poll for completion: wait until used ring idx advances.
        let used_virt = blk.used_virt;
        let expected_idx = blk.last_used_idx.wrapping_add(1);

        let mut timeout = POLL_TIMEOUT;
        loop {
            core::arch::asm!("dsb sy");
            let used_idx = core::ptr::read_volatile((used_virt + 2) as *const u16);
            if used_idx == expected_idx {
                break;
            }
            timeout -= 1;
            if timeout == 0 {
                crate::kerror!(Storage, "VirtIO-blk: poll timeout (sector {})", sector);
                return Err(StorageError::VirtioError);
            }
        }
        blk.last_used_idx = expected_idx;

        // Check status byte.
        let dev_status = core::ptr::read_volatile(status_virt as *const u8);
        if dev_status != 0 {
            crate::kerror!(Storage, "VirtIO-blk: request failed, status={}", dev_status);
            return Err(StorageError::IoError);
        }

        // For reads, copy data from DMA buffer to caller's buffer.
        if req_type == VIRTIO_BLK_T_IN {
            core::ptr::copy_nonoverlapping(data_virt as *const u8, data.as_mut_ptr(), 512);
        }

        Ok(())
    }
}
