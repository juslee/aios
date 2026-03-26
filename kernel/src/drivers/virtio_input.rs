//! VirtIO-input driver — MMIO legacy (v1) transport, polled I/O.
//!
//! Probes VirtIO MMIO slots for input devices (device_id=18). Supports MULTIPLE
//! devices (keyboard + tablet are separate MMIO slots on QEMU). The eventq is
//! device-to-driver: pre-filled with empty VirtioInputEvent buffers that the
//! device writes into.
//!
//! Per VirtIO spec §5.8 (input device), using legacy MMIO v1 transport.
//! Reuses VirtIO common infrastructure from `virtio_common.rs`.

use shared::input::*;
use shared::order_for_pages;
use shared::storage::*;
use spin::Mutex;

use super::virtio_common::*;
use crate::arch::aarch64::mmu::{DIRECT_MAP_BASE, MMIO_BASE};
use crate::dtb::DeviceTree;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of input devices supported simultaneously.
pub const MAX_INPUT_DEVICES: usize = 4;

/// Size of the VirtIO-input event struct (8 bytes).
const INPUT_EVENT_SIZE: usize = core::mem::size_of::<VirtioInputEvent>();

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors from VirtIO-input driver operations.
#[derive(Debug)]
#[allow(dead_code)] // Some variants used only in error paths or future phases.
pub enum InputError {
    /// Device probe failed (wrong magic, version, or device ID).
    ProbeError,
    /// Device initialization failed.
    InitFailed,
    /// Virtqueue setup failed.
    QueueError,
    /// DMA memory allocation failed.
    OutOfMemory,
}

// ---------------------------------------------------------------------------
// Device state
// ---------------------------------------------------------------------------

/// Per-device VirtIO-input state.
pub struct VirtioInputDevice {
    /// MMIO virtual base address (MMIO_BASE + phys).
    base: usize,
    /// Physical base address (for logging/debugging).
    _phys_base: usize,
    /// Eventq descriptor table virtual address (for future cleanup).
    _desc_virt: usize,
    /// Eventq available ring virtual address.
    avail_virt: usize,
    /// Eventq used ring virtual address.
    used_virt: usize,
    /// DMA page for event buffers — physical address (for future cleanup).
    _event_buf_phys: usize,
    /// DMA page for event buffers — virtual address.
    event_buf_virt: usize,
    /// Last consumed used ring index.
    last_used_idx: u16,
    /// Negotiated eventq size.
    queue_size: u16,
    /// Device index (0, 1, 2, 3).
    pub device_id: InputDeviceId,
    /// Human-readable device name from config space.
    name: [u8; 64],
    /// Length of the device name.
    name_len: u8,
    /// Whether device has absolute axes (tablet).
    pub has_abs: bool,
    /// Maximum X axis value (from ABS_INFO, typically 32767).
    pub abs_max_x: u32,
    /// Maximum Y axis value.
    pub abs_max_y: u32,
}

/// Global array of input devices. Lock ordering: leaf lock (after BLOCK_ENGINE).
static INPUT_DEVICES: Mutex<[Option<VirtioInputDevice>; MAX_INPUT_DEVICES]> =
    Mutex::new([None, None, None, None]);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Probe and initialize all VirtIO-input devices.
///
/// Scans DTB bases first, then brute-force MMIO slots. Initializes each
/// device found with device_id=18. Returns the number of devices initialized.
pub fn init_all(dt: &DeviceTree) -> usize {
    let mut count = 0usize;
    // Track already-initialized physical addresses to avoid double-init when
    // both DTB and brute-force scan discover the same device.
    let mut initialized_phys = [0usize; MAX_INPUT_DEVICES];

    // Strategy 1: DTB-provided VirtIO MMIO bases.
    for i in 0..dt.virtio_mmio_count {
        if count >= MAX_INPUT_DEVICES {
            crate::kwarn!(Input, "max input devices reached, stopping scan");
            return count;
        }
        let phys = dt.virtio_mmio_bases[i] as usize;
        if probe_slot(phys) && try_init_device(phys, &mut count, &mut initialized_phys) {
            continue;
        }
    }

    // Strategy 2: Brute-force scan of MMIO region.
    for slot in 0..VIRTIO_MMIO_SLOT_COUNT {
        if count >= MAX_INPUT_DEVICES {
            crate::kwarn!(Input, "max input devices reached, stopping scan");
            return count;
        }
        let phys = VIRTIO_MMIO_REGION_BASE as usize + slot * VIRTIO_MMIO_REGION_STRIDE as usize;
        if probe_slot(phys) {
            try_init_device(phys, &mut count, &mut initialized_phys);
        }
    }

    if count == 0 {
        crate::kinfo!(Input, "No VirtIO-input devices found");
    }
    count
}

/// Get the number of initialized input devices.
#[allow(dead_code)] // Used by compositor (M25+).
pub fn device_count() -> usize {
    let guard = INPUT_DEVICES.lock();
    guard.iter().filter(|d| d.is_some()).count()
}

/// Poll a single device for one event. Returns the event if available.
///
/// Checks the used ring for completed event buffers, extracts the event,
/// and recycles the buffer back to the available ring.
#[allow(dead_code)] // Used by compositor (M25+).
pub fn poll_device(device_idx: usize) -> Option<VirtioInputEvent> {
    let mut guard = INPUT_DEVICES.lock();
    let dev = guard[device_idx].as_mut()?;

    // SAFETY: used_virt points to the used ring in DMA memory, mapped via
    // DIRECT_MAP_BASE. Reading the u16 idx field is valid for the lifetime
    // of the DMA allocation. Misreading would return stale/no events.
    let used_idx = unsafe { core::ptr::read_volatile((dev.used_virt + 2) as *const u16) };

    if dev.last_used_idx == used_idx {
        return None; // No new events.
    }

    let ring_idx = (dev.last_used_idx % dev.queue_size) as usize;
    let elem_addr = dev.used_virt + 4 + ring_idx * 8;

    // SAFETY: elem_addr points to a VirtqUsedElem (8 bytes: id:u32 + len:u32)
    // in the used ring DMA memory. Valid while the virtqueue allocation exists.
    let desc_id = unsafe { core::ptr::read_volatile(elem_addr as *const u32) };

    // Bounds check: reject invalid descriptor IDs from device to prevent
    // out-of-bounds DMA reads. A conformant device never exceeds queue_size.
    if desc_id >= dev.queue_size as u32 {
        crate::kerror!(
            Input,
            "invalid desc_id {} from device (max {})",
            desc_id,
            dev.queue_size
        );
        dev.last_used_idx = dev.last_used_idx.wrapping_add(1);
        return None;
    }

    // Read the event from the DMA event buffer.
    let buf_addr = dev.event_buf_virt + (desc_id as usize) * INPUT_EVENT_SIZE;

    // SAFETY: buf_addr points to an 8-byte VirtioInputEvent in the DMA event
    // buffer page. desc_id was bounds-checked above (< queue_size). The buffer
    // is 8-byte aligned (desc_id * 8 within a page-aligned DMA allocation).
    let event = unsafe { core::ptr::read_volatile(buf_addr as *const VirtioInputEvent) };

    dev.last_used_idx = dev.last_used_idx.wrapping_add(1);

    // Recycle the descriptor back to the available ring.
    recycle_buffer(dev, desc_id as u16);

    Some(event)
}

/// Poll all devices, collecting raw events into the provided buffer.
///
/// Returns the number of events collected. Caller must provide a buffer
/// large enough for the expected event burst.
pub fn poll_all(buf: &mut [(InputDeviceId, VirtioInputEvent)]) -> usize {
    let mut guard = INPUT_DEVICES.lock();
    let mut total = 0;

    for idx in 0..MAX_INPUT_DEVICES {
        if total >= buf.len() {
            break;
        }
        if let Some(dev) = guard[idx].as_mut() {
            let device_id = dev.device_id;
            loop {
                if total >= buf.len() {
                    break;
                }

                // SAFETY: Same as poll_device — reads used ring in DMA memory.
                let used_idx =
                    unsafe { core::ptr::read_volatile((dev.used_virt + 2) as *const u16) };

                if dev.last_used_idx == used_idx {
                    break;
                }

                let ring_idx = (dev.last_used_idx % dev.queue_size) as usize;
                let elem_addr = dev.used_virt + 4 + ring_idx * 8;

                // SAFETY: Same as poll_device — reads VirtqUsedElem from DMA.
                let desc_id = unsafe { core::ptr::read_volatile(elem_addr as *const u32) };

                // Bounds check (same as poll_device).
                if desc_id >= dev.queue_size as u32 {
                    dev.last_used_idx = dev.last_used_idx.wrapping_add(1);
                    continue;
                }

                let buf_addr = dev.event_buf_virt + (desc_id as usize) * INPUT_EVENT_SIZE;

                // SAFETY: Same as poll_device — reads VirtioInputEvent from DMA.
                // desc_id bounds-checked above.
                let event =
                    unsafe { core::ptr::read_volatile(buf_addr as *const VirtioInputEvent) };

                dev.last_used_idx = dev.last_used_idx.wrapping_add(1);
                recycle_buffer(dev, desc_id as u16);

                buf[total] = (device_id, event);
                total += 1;
            }
        }
    }

    total
}

/// Get abs info for a device (for coordinate conversion).
pub fn get_abs_info(device_idx: usize) -> Option<(u32, u32)> {
    let guard = INPUT_DEVICES.lock();
    guard[device_idx]
        .as_ref()
        .filter(|d| d.has_abs)
        .map(|d| (d.abs_max_x, d.abs_max_y))
}

// ---------------------------------------------------------------------------
// Probe
// ---------------------------------------------------------------------------

/// Check if a VirtIO MMIO slot contains an input device (device_id=18).
fn probe_slot(phys: usize) -> bool {
    let virt = MMIO_BASE + phys;
    // SAFETY: MMIO region 0x0-0x40000000 is mapped as device memory in TTBR1.
    // Maintained by init_kernel_address_space() in kmap.rs (MMIO_BASE mapping).
    // Reading an unoccupied slot returns 0 (not the VirtIO magic); reading an
    // unmapped address would cause a synchronous data abort.
    unsafe {
        let magic = mmio_read32(virt + VIRTIO_MMIO_MAGIC_VALUE);
        if magic != VIRTIO_MMIO_MAGIC {
            return false;
        }
        let version = mmio_read32(virt + VIRTIO_MMIO_VERSION);
        if version != 1 {
            return false;
        }
        let device_id = mmio_read32(virt + VIRTIO_MMIO_DEVICE_ID);
        device_id == VIRTIO_DEVICE_ID_INPUT
    }
}

/// Try to initialize a device at the given physical address.
/// On success, stores it in INPUT_DEVICES and increments count.
/// Skips devices already in `initialized_phys` to avoid double-init.
fn try_init_device(
    phys: usize,
    count: &mut usize,
    initialized_phys: &mut [usize; MAX_INPUT_DEVICES],
) -> bool {
    // Dedup: skip if we already initialized a device at this physical address.
    if initialized_phys[..*count].contains(&phys) {
        return false;
    }
    let virt = MMIO_BASE + phys;
    match init_device(virt, phys, InputDeviceId(*count as u8)) {
        Ok(dev) => {
            let name_str = core::str::from_utf8(&dev.name[..dev.name_len as usize])
                .unwrap_or("<invalid utf8>");
            if dev.has_abs {
                crate::kinfo!(
                    Input,
                    "VirtIO-input: \"{}\" at {:#x} abs: min=0 max={}",
                    name_str,
                    phys,
                    dev.abs_max_x
                );
            } else {
                crate::kinfo!(Input, "VirtIO-input: \"{}\" at {:#x}", name_str, phys);
            }
            let mut guard = INPUT_DEVICES.lock();
            guard[*count] = Some(dev);
            initialized_phys[*count] = phys;
            *count += 1;
            true
        }
        Err(e) => {
            crate::kerror!(Input, "VirtIO-input init at {:#x} failed: {:?}", phys, e);
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Device initialization (VirtIO spec §3.1, legacy MMIO v1)
// ---------------------------------------------------------------------------

fn init_device(
    base: usize,
    phys: usize,
    id: InputDeviceId,
) -> Result<VirtioInputDevice, InputError> {
    // SAFETY: All MMIO accesses target a validated VirtIO device address.
    // base = MMIO_BASE + phys, confirmed to have correct magic/version/device_id.
    // Writing to an invalid MMIO address would cause a synchronous data abort.
    unsafe {
        // 1. Reset device.
        mmio_write32(base + VIRTIO_MMIO_STATUS, 0);

        // 2. ACKNOWLEDGE.
        let mut status = VIRTIO_STATUS_ACKNOWLEDGE;
        mmio_write32(base + VIRTIO_MMIO_STATUS, status);

        // 3. DRIVER.
        status |= VIRTIO_STATUS_DRIVER;
        mmio_write32(base + VIRTIO_MMIO_STATUS, status);

        // 4. Read and negotiate features (no features needed).
        let _device_features = mmio_read32(base + VIRTIO_MMIO_DEVICE_FEATURES);
        mmio_write32(base + VIRTIO_MMIO_DRIVER_FEATURES, 0);

        // 5. GUEST_PAGE_SIZE (legacy requirement).
        mmio_write32(base + VIRTIO_MMIO_GUEST_PAGE_SIZE, VIRT_PAGE_SIZE as u32);

        // 6. Set up eventq (queue 0).
        mmio_write32(base + VIRTIO_MMIO_QUEUE_SEL, 0);
        core::arch::asm!("dsb sy");

        let queue_num_max = mmio_read32(base + VIRTIO_MMIO_QUEUE_NUM_MAX);
        if queue_num_max == 0 {
            crate::kerror!(Input, "eventq not available");
            return Err(InputError::QueueError);
        }

        let queue_size = (QUEUE_SIZE as u32).min(queue_num_max) as u16;
        mmio_write32(base + VIRTIO_MMIO_QUEUE_NUM, queue_size as u32);
        mmio_write32(base + VIRTIO_MMIO_QUEUE_ALIGN, VIRT_PAGE_SIZE as u32);

        // Allocate DMA for the eventq virtqueue.
        let total_bytes = virtqueue_size(queue_size as usize);
        let total_pages = total_bytes.div_ceil(VIRT_PAGE_SIZE);
        let order = order_for_pages(total_pages);

        let vq_phys = crate::mm::frame::alloc_dma_pages(order).ok_or(InputError::OutOfMemory)?;
        let vq_virt = DIRECT_MAP_BASE + vq_phys;

        // SAFETY: vq_virt is a valid DMA allocation mapped via TTBR1 direct map.
        // Zeroing the allocation initializes descriptor/avail/used ring to safe defaults.
        core::ptr::write_bytes(vq_virt as *mut u8, 0, (1usize << order) * VIRT_PAGE_SIZE);

        let desc_virt = vq_virt;
        let avail_virt = vq_virt + avail_offset(queue_size as usize);
        let used_virt = vq_virt + used_offset(queue_size as usize);

        // Tell device the queue page frame number.
        let pfn = (vq_phys / VIRT_PAGE_SIZE) as u32;
        mmio_write32(base + VIRTIO_MMIO_QUEUE_PFN, pfn);

        // Allocate a separate DMA page for event buffers.
        let event_buf_phys = crate::mm::frame::alloc_dma_page().ok_or(InputError::OutOfMemory)?;
        let event_buf_virt = DIRECT_MAP_BASE + event_buf_phys;

        // SAFETY: event_buf_virt is a valid DMA page. Zeroing initializes event buffers.
        core::ptr::write_bytes(event_buf_virt as *mut u8, 0, VIRT_PAGE_SIZE);

        // Pre-fill the eventq: each descriptor points to an 8-byte event buffer slot.
        // The device writes events into these buffers (device-to-driver pattern).
        prefill_eventq(desc_virt, avail_virt, event_buf_phys, queue_size);

        // Notify device that buffers are available.
        core::arch::asm!("dsb sy");
        mmio_write32(base + VIRTIO_MMIO_QUEUE_NOTIFY, 0);
        core::arch::asm!("dsb sy");

        // 7. Set up statusq (queue 1) — allocate but don't use (Phase 8 LED control).
        mmio_write32(base + VIRTIO_MMIO_QUEUE_SEL, 1);
        core::arch::asm!("dsb sy");
        let statusq_max = mmio_read32(base + VIRTIO_MMIO_QUEUE_NUM_MAX);
        if statusq_max > 0 {
            let sq_size = (QUEUE_SIZE as u32).min(statusq_max) as u16;
            mmio_write32(base + VIRTIO_MMIO_QUEUE_NUM, sq_size as u32);
            mmio_write32(base + VIRTIO_MMIO_QUEUE_ALIGN, VIRT_PAGE_SIZE as u32);

            let sq_bytes = virtqueue_size(sq_size as usize);
            let sq_pages = sq_bytes.div_ceil(VIRT_PAGE_SIZE);
            let sq_order = order_for_pages(sq_pages);

            if let Some(sq_phys) = crate::mm::frame::alloc_dma_pages(sq_order) {
                let sq_virt = DIRECT_MAP_BASE + sq_phys;
                // SAFETY: sq_virt is a valid DMA allocation via TTBR1 direct map.
                // Maintained by alloc_dma_pages() returning page-aligned Pool::Dma memory.
                // Writing beyond the allocation would corrupt adjacent DMA state.
                core::ptr::write_bytes(
                    sq_virt as *mut u8,
                    0,
                    (1usize << sq_order) * VIRT_PAGE_SIZE,
                );
                mmio_write32(
                    base + VIRTIO_MMIO_QUEUE_PFN,
                    (sq_phys / VIRT_PAGE_SIZE) as u32,
                );
            }
            // If allocation fails, statusq is just not available — non-fatal.
        }

        // 8. DRIVER_OK.
        status |= VIRTIO_STATUS_DRIVER_OK;
        mmio_write32(base + VIRTIO_MMIO_STATUS, status);

        // 9. Read device config: name and absolute axis info.
        let mut name = [0u8; 64];
        let name_len = read_config_name(base, &mut name);

        let mut has_abs = false;
        let mut abs_max_x = 0u32;
        let mut abs_max_y = 0u32;

        // Check if device supports absolute axes (CFG_EV_BITS for EV_ABS).
        let ev_abs_size = read_config_size(base, VIRTIO_INPUT_CFG_EV_BITS, EV_ABS as u8);
        if ev_abs_size > 0 {
            has_abs = true;
            let abs_x_info = read_abs_info(base, ABS_X as u8);
            let abs_y_info = read_abs_info(base, ABS_Y as u8);
            abs_max_x = abs_x_info.max;
            abs_max_y = abs_y_info.max;
        }

        Ok(VirtioInputDevice {
            base,
            _phys_base: phys,
            _desc_virt: desc_virt,
            avail_virt,
            used_virt,
            _event_buf_phys: event_buf_phys,
            event_buf_virt,
            last_used_idx: 0,
            queue_size,
            device_id: id,
            name,
            name_len,
            has_abs,
            abs_max_x,
            abs_max_y,
        })
    }
}

// ---------------------------------------------------------------------------
// Eventq pre-fill
// ---------------------------------------------------------------------------

/// Pre-fill the eventq available ring with empty event buffers.
///
/// Each descriptor points to an 8-byte slot in the event buffer DMA page.
/// The device writes VirtioInputEvent structs into these buffers when
/// input events occur.
///
/// # Safety
/// All addresses must point to valid DMA memory within the virtqueue and
/// event buffer allocations.
unsafe fn prefill_eventq(
    desc_virt: usize,
    avail_virt: usize,
    event_buf_phys: usize,
    queue_size: u16,
) {
    let desc_base = desc_virt as *mut VirtqDesc;

    for i in 0..queue_size as usize {
        // SAFETY: desc_base + i is within the descriptor table allocation.
        // event_buf_phys + i*8 is within the event buffer DMA page (max 128*8=1024 < 4096).
        // VIRTQ_DESC_F_WRITE marks the buffer as device-writable.
        core::ptr::write_volatile(
            desc_base.add(i),
            VirtqDesc {
                addr: (event_buf_phys + i * INPUT_EVENT_SIZE) as u64,
                len: INPUT_EVENT_SIZE as u32,
                flags: VIRTQ_DESC_F_WRITE,
                next: 0, // No chaining — standalone descriptors.
            },
        );

        // Add descriptor index to available ring.
        // SAFETY: avail_virt + 4 + i*2 is within the available ring allocation.
        core::ptr::write_volatile((avail_virt + 4 + i * 2) as *mut u16, i as u16);
    }

    // Set available ring index to queue_size (all buffers now available).
    // SAFETY: avail_virt + 2 is the avail ring idx field.
    core::arch::asm!("dsb sy");
    core::ptr::write_volatile((avail_virt + 2) as *mut u16, queue_size);
}

// ---------------------------------------------------------------------------
// Buffer recycling
// ---------------------------------------------------------------------------

/// Recycle a consumed event buffer back to the available ring.
///
/// After reading an event from the used ring, the buffer's descriptor must be
/// re-added to the available ring so the device can reuse it.
fn recycle_buffer(dev: &mut VirtioInputDevice, desc_id: u16) {
    // SAFETY: avail_virt points to the eventq available ring in DMA memory.
    // Reading/writing the avail ring idx and ring entries is safe for the
    // lifetime of the DMA allocation. desc_id is a valid descriptor index
    // returned by the device in the used ring.
    unsafe {
        let avail_idx = core::ptr::read_volatile((dev.avail_virt + 2) as *const u16);
        let ring_slot = (avail_idx % dev.queue_size) as usize;

        core::ptr::write_volatile((dev.avail_virt + 4 + ring_slot * 2) as *mut u16, desc_id);

        core::arch::asm!("dsb sy");
        core::ptr::write_volatile((dev.avail_virt + 2) as *mut u16, avail_idx.wrapping_add(1));

        core::arch::asm!("dsb sy");
        mmio_write32(dev.base + VIRTIO_MMIO_QUEUE_NOTIFY, 0);
        core::arch::asm!("dsb sy");
    }
}

// ---------------------------------------------------------------------------
// Config space access (VirtIO spec §5.8.2)
// ---------------------------------------------------------------------------

// VirtIO-input config space layout (at MMIO offset 0x100):
//   +0x00: select (u8)  — config query type
//   +0x01: subsel (u8)  — config query subtype
//   +0x02: size (u8)    — valid bytes in data union
//   +0x03..+0x07: reserved
//   +0x08: data union (string[128] / bitmap[128] / abs_info)

/// Read the config `size` field for a given select/subsel combination.
fn read_config_size(base: usize, select: u8, subsel: u8) -> u8 {
    let config = base + VIRTIO_MMIO_CONFIG_SPACE;
    // SAFETY: config space is mapped as device MMIO at MMIO_BASE + phys.
    // Writing select+subsel as a packed u32 (little-endian: select in byte 0,
    // subsel in byte 1). The device updates size in byte 2 synchronously.
    unsafe {
        let packed = (subsel as u32) << 8 | (select as u32);
        mmio_write32(config, packed);
        core::arch::asm!("dsb sy");
        // Read back: size is in byte 2 of the u32 at config offset 0.
        let val = mmio_read32(config);
        ((val >> 16) & 0xFF) as u8
    }
}

/// Read the device name from config space.
fn read_config_name(base: usize, name: &mut [u8; 64]) -> u8 {
    let size = read_config_size(base, VIRTIO_INPUT_CFG_ID_NAME, 0);
    if size == 0 {
        return 0;
    }

    let config_data = base + VIRTIO_MMIO_CONFIG_SPACE + 0x08;
    let copy_len = (size as usize).min(64);

    // SAFETY: config_data points to the data union in the VirtIO config space.
    // Reading u32 words and extracting bytes. The device guarantees `size` valid
    // bytes. Reading beyond `size` returns garbage but doesn't cause a fault.
    unsafe {
        for i in (0..copy_len).step_by(4) {
            let word = mmio_read32(config_data + i);
            let bytes = word.to_le_bytes();
            for j in 0..4 {
                if i + j < copy_len {
                    name[i + j] = bytes[j];
                }
            }
        }
    }

    copy_len as u8
}

/// Read absolute axis info from config space.
fn read_abs_info(base: usize, axis: u8) -> VirtioInputAbsInfo {
    let size = read_config_size(base, VIRTIO_INPUT_CFG_ABS_INFO, axis);
    if size < 20 {
        return VirtioInputAbsInfo {
            min: 0,
            max: 0,
            fuzz: 0,
            flat: 0,
            res: 0,
        };
    }

    let config_data = base + VIRTIO_MMIO_CONFIG_SPACE + 0x08;

    // SAFETY: config_data points to the abs_info struct in config space.
    // Reading 5 u32 values (20 bytes). The device confirmed size >= 20.
    unsafe {
        VirtioInputAbsInfo {
            min: mmio_read32(config_data),
            max: mmio_read32(config_data + 4),
            fuzz: mmio_read32(config_data + 8),
            flat: mmio_read32(config_data + 12),
            res: mmio_read32(config_data + 16),
        }
    }
}
