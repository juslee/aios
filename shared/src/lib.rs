#![no_std]

/// Physical address type alias.
pub type PhysAddr = u64;

/// Virtual address type alias.
pub type VirtAddr = u64;

/// Magic number for BootInfo validation: "AIOSBOOT" as u64.
pub const BOOTINFO_MAGIC: u64 = 0x41494F53_424F4F54;

/// Information passed from UEFI stub to kernel entry point.
///
/// In Phase 0, only `magic` is populated. All pointer-containing fields
/// use `Option<u64>` stubs to keep the struct `Send`/`Sync` and compilable
/// on the host target. Phase 1 replaces stubs with real pointer types
/// behind `#[cfg(target_arch = "aarch64")]`.
#[repr(C)]
pub struct BootInfo {
    /// Magic number for validation: 0x41494F53_424F4F54 ("AIOSBOOT")
    pub magic: u64,

    /// UEFI memory map pointer (stub: address as u64).
    pub memory_map_addr: Option<u64>,
    /// Number of memory map entries.
    pub memory_map_count: Option<u64>,
    /// Size of each memory map entry.
    pub memory_map_entry_size: Option<u64>,

    /// Framebuffer base address (stub: address as u64).
    pub framebuffer: Option<u64>,

    /// Device tree blob base address (stub: address as u64).
    pub device_tree: Option<u64>,

    /// ACPI RSDP physical address.
    pub acpi_rsdp: Option<u64>,

    /// UEFI Runtime Services table address.
    pub runtime_services: Option<u64>,

    /// Random seed from UEFI RNG protocol for KASLR.
    pub rng_seed: [u8; 32],

    /// Physical address where the kernel ELF was loaded.
    pub kernel_phys_base: PhysAddr,

    /// Size of kernel image in memory.
    pub kernel_size: u64,

    /// Physical address of the initramfs.
    pub initramfs_base: Option<u64>,
    /// Size of the initramfs.
    pub initramfs_size: Option<u64>,

    /// Command line string address (stub: address as u64).
    pub cmdline_addr: Option<u64>,
    /// Command line length.
    pub cmdline_len: Option<u64>,
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
    BootInfoRegion = 11,
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
