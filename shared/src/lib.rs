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
