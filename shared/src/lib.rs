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
