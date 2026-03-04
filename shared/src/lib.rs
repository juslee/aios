#![no_std]

use core::num::NonZeroU64;

/// Physical address type alias.
pub type PhysAddr = u64;

/// Virtual address type alias.
pub type VirtAddr = u64;

/// Magic number for BootInfo validation: "AIOSBOOT" as u64.
pub const BOOTINFO_MAGIC: u64 = 0x41494F53_424F4F54;

/// Information passed from UEFI stub to kernel entry point.
///
/// Phase 0 contract:
/// - `magic` is always populated and is the only field guaranteed to contain
///   meaningful data.
/// - All pointer-containing fields use `Option<NonZeroU64>` stubs and will be
///   `None` in Phase 0.
/// - Scalar, non-optional fields (`rng_seed`, `kernel_phys_base`,
///   `kernel_size`) are present in the layout but must be treated as
///   unspecified (typically zero-initialized) and must not be relied on
///   until Phase 1.
///
/// All address/pointer fields use `Option<NonZeroU64>` stubs, which have
/// the same layout as a nullable `u64` (0 = None), keeping the struct
/// FFI-safe, `Send`/`Sync`, and compilable on the host target. Scalar
/// count and size fields use plain `u64` (0 is a valid value). Phase 1
/// replaces these stubs with real pointer types behind
/// `#[cfg(target_arch = "aarch64")]`.
#[repr(C)]
pub struct BootInfo {
    /// Magic number for validation: 0x41494F53_424F4F54 ("AIOSBOOT")
    pub magic: u64,

    /// UEFI memory map pointer (stub: address as NonZeroU64).
    pub memory_map_addr: Option<NonZeroU64>,
    /// Number of memory map entries.
    pub memory_map_count: u64,
    /// Size of each memory map entry.
    pub memory_map_entry_size: u64,

    /// Framebuffer base address (stub: address as NonZeroU64).
    pub framebuffer: Option<NonZeroU64>,

    /// Device tree blob base address (stub: address as NonZeroU64).
    pub device_tree: Option<NonZeroU64>,

    /// ACPI RSDP physical address.
    pub acpi_rsdp: Option<NonZeroU64>,

    /// UEFI Runtime Services table address.
    pub runtime_services: Option<NonZeroU64>,

    /// Random seed from UEFI RNG protocol for KASLR.
    pub rng_seed: [u8; 32],

    /// Physical address where the kernel ELF was loaded.
    pub kernel_phys_base: PhysAddr,

    /// Size of kernel image in memory.
    pub kernel_size: u64,

    /// Physical address of the initramfs.
    pub initramfs_base: Option<NonZeroU64>,
    /// Size of the initramfs.
    pub initramfs_size: u64,

    /// Command line string address (stub: address as NonZeroU64).
    pub cmdline_addr: Option<NonZeroU64>,
    /// Command line length.
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
