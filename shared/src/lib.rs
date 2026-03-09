#![no_std]

/// Physical address type alias.
pub type PhysAddr = u64;

/// Virtual address type alias.
pub type VirtAddr = u64;

/// Magic number for BootInfo validation: "AIOSBOOT" as u64.
pub const BOOTINFO_MAGIC: u64 = 0x41494F53_424F4F54;

/// Information passed from UEFI stub to kernel entry point.
///
/// All fields use fixed-layout primitives for a stable C ABI across toolchain
/// updates. Fields that may be absent use `u64` with 0 meaning "not present".
/// Phase 1 populates all available fields and leaves optional ones as 0 when
/// unavailable; Phase 0 sets only `magic` and zeroes the rest.
#[repr(C)]
pub struct BootInfo {
    /// Magic number for validation: 0x41494F53_424F4F54 ("AIOSBOOT")
    pub magic: u64,

    /// UEFI memory map: physical address of the MemoryDescriptor array (0 = absent).
    pub memory_map_addr: u64,
    /// Number of MemoryDescriptor entries in the memory map.
    pub memory_map_count: u64,
    /// Size of each MemoryDescriptor entry in bytes (UEFI descriptor size may exceed sizeof).
    pub memory_map_entry_size: u64,

    /// Framebuffer base address (0 = not available / headless).
    pub framebuffer: u64,

    /// Device tree blob base address (0 = not present).
    pub device_tree: u64,

    /// ACPI RSDP physical address (0 = not present).
    pub acpi_rsdp: u64,

    /// UEFI Runtime Services table address (0 = not available).
    pub runtime_services: u64,

    /// Random seed from UEFI RNG protocol for KASLR.
    pub rng_seed: [u8; 32],

    /// Physical address where the kernel ELF was loaded.
    pub kernel_phys_base: PhysAddr,

    /// Size of kernel image in memory.
    pub kernel_size: u64,

    /// Physical address of the initramfs (0 = not present).
    pub initramfs_base: u64,
    /// Size of the initramfs in bytes (0 = not present).
    pub initramfs_size: u64,

    /// Command line string address (0 = not present).
    pub cmdline_addr: u64,
    /// Command line length in bytes.
    pub cmdline_len: u64,

    /// Framebuffer width in pixels (0 = not available).
    pub fb_width: u32,
    /// Framebuffer height in pixels.
    pub fb_height: u32,
    /// Framebuffer stride in bytes (byte offset from one row to the next).
    pub fb_stride: u32,
    /// Framebuffer pixel format: 0 = Bgr8, 1 = Rgb8 (matches PixelFormat repr).
    pub fb_pixel_format: u32,
    /// Framebuffer total size in bytes (stride * height).
    pub fb_size: u64,
}

/// Classification of physical memory regions.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryType {
    Conventional = 0,
    LoaderCode = 1,
    LoaderData = 2,
    BootServicesCode = 3,
    BootServicesData = 4,
    RuntimeServicesCode = 5,
    RuntimeServicesData = 6,
    Reserved = 7,
    AcpiReclaimable = 8,
    AcpiNvs = 9,
    MemoryMappedIO = 10,
    BootInfo = 11,
    KernelImage = 12,
    Initramfs = 13,
}

/// Pixel format for framebuffer.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Bgr8 = 0,
    Rgb8 = 1,
}

/// UEFI memory descriptor — matches the EFI_MEMORY_DESCRIPTOR layout.
///
/// The UEFI stub stores the raw memory map returned by ExitBootServices().
/// The kernel iterates these via `BootInfo.memory_map_addr` with stride
/// `BootInfo.memory_map_entry_size` (which may exceed `size_of::<MemoryDescriptor>()`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MemoryDescriptor {
    /// UEFI memory type (EFI_MEMORY_TYPE). Values 0–13 are translated to `MemoryType`
    /// via `MemoryDescriptor::memory_type()`.
    pub ty: u32,
    /// Padding to align phys_start to 8 bytes (UEFI ABI requirement).
    pub _pad: u32,
    /// Physical address of the start of the memory region.
    pub phys_start: u64,
    /// Virtual address (set by SetVirtualAddressMap; unused by kernel).
    pub virt_start: u64,
    /// Number of 4 KiB pages in the region.
    pub page_count: u64,
    /// Memory attributes (EFI_MEMORY_ATTRIBUTES).
    pub attribute: u64,
}

// ── Physical Memory Pool Types ──────────────────────────────────────────

/// Physical memory pool classification.
///
/// Each pool is backed by its own buddy allocator instance with a dedicated
/// physical address range. Pool assignment is determined at boot based on
/// total detected RAM (see `PoolConfig::from_total_ram`).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pool {
    /// Kernel heap, page tables, slab caches.
    Kernel = 0,
    /// User-space process pages.
    User = 1,
    /// AI model weights and inference buffers (0 on small-RAM systems).
    Model = 2,
    /// DMA-capable buffers for device I/O (low physical addresses preferred).
    Dma = 3,
}

/// Memory pressure level based on free page ratio in the user pool.
///
/// Thresholds from memory.md §2.3:
/// - Normal:   >20% free
/// - Low:      11–20% free
/// - Critical: 5–10% free
/// - Oom:      <5% free
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemoryPressure {
    Normal = 0,
    Low = 1,
    Critical = 2,
    Oom = 3,
}

impl MemoryPressure {
    /// Determine pressure level from free and total page counts.
    pub fn from_free_ratio(free: usize, total: usize) -> Self {
        if total == 0 {
            return MemoryPressure::Oom;
        }
        let percent = (free * 100) / total;
        if percent > 20 {
            MemoryPressure::Normal
        } else if percent > 10 {
            MemoryPressure::Low
        } else if percent >= 5 {
            MemoryPressure::Critical
        } else {
            MemoryPressure::Oom
        }
    }
}

const MIB: usize = 1024 * 1024;
const GIB: usize = 1024 * MIB;

/// Per-pool byte budgets computed from total detected RAM.
///
/// See memory.md §2.4 for the tier table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolConfig {
    pub kernel: usize,
    pub model: usize,
    pub user: usize,
    pub dma: usize,
    pub reserved: usize,
}

impl PoolConfig {
    /// Compute pool sizes from total detected RAM.
    ///
    /// Tiers (memory.md §2.4):
    /// - <4 GB:   kernel=128M, model=0,  dma=64M,  reserved=64M,  user=remainder
    /// - <8 GB:   kernel=256M, model=2G, dma=128M, reserved=128M, user=remainder
    /// - <16 GB:  kernel=256M, model=4G, dma=128M, reserved=128M, user=remainder
    /// - ≥16 GB:  kernel=256M, model=8G, dma=128M, reserved=128M, user=remainder
    pub fn from_total_ram(total: usize) -> Self {
        let (kernel, model, dma, reserved) = if total < 4 * GIB {
            (128 * MIB, 0, 64 * MIB, 64 * MIB)
        } else if total < 8 * GIB {
            (256 * MIB, 2 * GIB, 128 * MIB, 128 * MIB)
        } else if total < 16 * GIB {
            (256 * MIB, 4 * GIB, 128 * MIB, 128 * MIB)
        } else {
            (256 * MIB, 8 * GIB, 128 * MIB, 128 * MIB)
        };

        let fixed = kernel + model + dma + reserved;
        let user = total.saturating_sub(fixed);

        PoolConfig {
            kernel,
            model,
            user,
            dma,
            reserved,
        }
    }
}

/// Compute the buddy address for a given address at a given order.
///
/// Classic XOR trick: the buddy of block at `addr` (relative to `base`)
/// with size `1 << (order + PAGE_SHIFT)` is found by flipping bit `order + PAGE_SHIFT`.
///
/// `addr` and `base` are physical addresses; `order` is the buddy order (0 = 4 KiB).
pub const fn buddy_of(addr: usize, base: usize, order: usize) -> usize {
    let page_shift = 12; // 4 KiB pages
    let offset = addr - base;
    let buddy_offset = offset ^ (1 << (order + page_shift));
    base + buddy_offset
}

impl MemoryDescriptor {
    /// Convert the raw UEFI memory type to our MemoryType enum.
    ///
    /// UEFI memory types: 0=Reserved, 1=LoaderCode, 2=LoaderData,
    /// 3=BootServicesCode, 4=BootServicesData, 5=RuntimeServicesCode,
    /// 6=RuntimeServicesData, 7=Conventional, 8=Unusable,
    /// 9=ACPIReclaim, 10=ACPINvs, 11=MMIO, 12=MMIOPortSpace, 13=PalCode.
    pub fn memory_type(&self) -> MemoryType {
        match self.ty {
            1 => MemoryType::LoaderCode,
            2 => MemoryType::LoaderData,
            3 => MemoryType::BootServicesCode,
            4 => MemoryType::BootServicesData,
            5 => MemoryType::RuntimeServicesCode,
            6 => MemoryType::RuntimeServicesData,
            7 => MemoryType::Conventional,
            9 => MemoryType::AcpiReclaimable,
            10 => MemoryType::AcpiNvs,
            11 | 12 => MemoryType::MemoryMappedIO,
            _ => MemoryType::Reserved,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── PoolConfig sizing tests ─────────────────────────────────────────

    #[test]
    fn pool_config_2g() {
        let cfg = PoolConfig::from_total_ram(2 * GIB);
        assert_eq!(cfg.kernel, 128 * MIB);
        assert_eq!(cfg.model, 0);
        assert_eq!(cfg.dma, 64 * MIB);
        assert_eq!(cfg.reserved, 64 * MIB);
        assert_eq!(cfg.user, 2 * GIB - 128 * MIB - 64 * MIB - 64 * MIB);
        // Verify all pools sum to total
        assert_eq!(
            cfg.kernel + cfg.model + cfg.user + cfg.dma + cfg.reserved,
            2 * GIB
        );
    }

    #[test]
    fn pool_config_4g() {
        let cfg = PoolConfig::from_total_ram(4 * GIB);
        assert_eq!(cfg.kernel, 256 * MIB);
        assert_eq!(cfg.model, 2 * GIB);
        assert_eq!(cfg.dma, 128 * MIB);
        assert_eq!(cfg.reserved, 128 * MIB);
        assert_eq!(
            cfg.kernel + cfg.model + cfg.user + cfg.dma + cfg.reserved,
            4 * GIB
        );
    }

    #[test]
    fn pool_config_8g() {
        let cfg = PoolConfig::from_total_ram(8 * GIB);
        assert_eq!(cfg.kernel, 256 * MIB);
        assert_eq!(cfg.model, 4 * GIB);
        assert_eq!(cfg.dma, 128 * MIB);
        assert_eq!(cfg.reserved, 128 * MIB);
        assert_eq!(
            cfg.kernel + cfg.model + cfg.user + cfg.dma + cfg.reserved,
            8 * GIB
        );
    }

    #[test]
    fn pool_config_16g() {
        let cfg = PoolConfig::from_total_ram(16 * GIB);
        assert_eq!(cfg.kernel, 256 * MIB);
        assert_eq!(cfg.model, 8 * GIB);
        assert_eq!(cfg.dma, 128 * MIB);
        assert_eq!(cfg.reserved, 128 * MIB);
        assert_eq!(
            cfg.kernel + cfg.model + cfg.user + cfg.dma + cfg.reserved,
            16 * GIB
        );
    }

    #[test]
    fn pool_config_32g() {
        let cfg = PoolConfig::from_total_ram(32 * GIB);
        assert_eq!(cfg.kernel, 256 * MIB);
        assert_eq!(cfg.model, 8 * GIB);
        // user gets the remainder
        assert_eq!(
            cfg.kernel + cfg.model + cfg.user + cfg.dma + cfg.reserved,
            32 * GIB
        );
    }

    // ── buddy_of XOR tests ──────────────────────────────────────────────

    #[test]
    fn buddy_of_order_0() {
        let base = 0x4000_0000;
        // Page 0's buddy is page 1, and vice versa
        assert_eq!(buddy_of(base, base, 0), base + 0x1000);
        assert_eq!(buddy_of(base + 0x1000, base, 0), base);
    }

    #[test]
    fn buddy_of_order_1() {
        let base = 0x4000_0000;
        // Order 1 = 8 KiB blocks. Block at +0 has buddy at +0x2000
        assert_eq!(buddy_of(base, base, 1), base + 0x2000);
        assert_eq!(buddy_of(base + 0x2000, base, 1), base);
    }

    #[test]
    fn buddy_of_higher_orders() {
        let base = 0x4000_0000;
        // Order 10 = 4 MiB blocks
        let block = base + 4 * MIB;
        let buddy = buddy_of(block, base, 10);
        assert_eq!(buddy, base); // block at +4M, buddy at +0
        assert_eq!(buddy_of(buddy, base, 10), block); // symmetric
    }

    #[test]
    fn buddy_of_is_symmetric() {
        let base = 0x4000_0000;
        for order in 0..=10 {
            let addr = base + (3 << (order + 12)); // 3rd block at this order
            let buddy = buddy_of(addr, base, order);
            assert_eq!(buddy_of(buddy, base, order), addr);
        }
    }

    // ── MemoryPressure tests ────────────────────────────────────────────

    #[test]
    fn pressure_normal() {
        assert_eq!(
            MemoryPressure::from_free_ratio(25, 100),
            MemoryPressure::Normal
        );
        assert_eq!(
            MemoryPressure::from_free_ratio(21, 100),
            MemoryPressure::Normal
        );
    }

    #[test]
    fn pressure_low() {
        assert_eq!(
            MemoryPressure::from_free_ratio(20, 100),
            MemoryPressure::Low
        );
        assert_eq!(
            MemoryPressure::from_free_ratio(11, 100),
            MemoryPressure::Low
        );
    }

    #[test]
    fn pressure_critical() {
        assert_eq!(
            MemoryPressure::from_free_ratio(10, 100),
            MemoryPressure::Critical
        );
        assert_eq!(
            MemoryPressure::from_free_ratio(5, 100),
            MemoryPressure::Critical
        );
    }

    #[test]
    fn pressure_oom() {
        assert_eq!(MemoryPressure::from_free_ratio(4, 100), MemoryPressure::Oom);
        assert_eq!(MemoryPressure::from_free_ratio(0, 100), MemoryPressure::Oom);
        // Edge: zero total → OOM
        assert_eq!(MemoryPressure::from_free_ratio(0, 0), MemoryPressure::Oom);
    }

    #[test]
    fn pressure_ordering() {
        assert!(MemoryPressure::Normal < MemoryPressure::Low);
        assert!(MemoryPressure::Low < MemoryPressure::Critical);
        assert!(MemoryPressure::Critical < MemoryPressure::Oom);
    }
}
