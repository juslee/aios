//! Boot-time types crossing the UEFI stub / kernel boundary.
//!
//! Defines `BootInfo`, `MemoryDescriptor`, `MemoryType`, `PixelFormat`,
//! and `EarlyBootPhase`.

use crate::PhysAddr;

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

// ---------------------------------------------------------------------------
// Early boot phases (boot-kernel.md §3.1)
// ---------------------------------------------------------------------------

/// Boot phases from entry point through full initialization.
///
/// Each phase represents a milestone in the boot sequence. The kernel
/// tracks the current phase via an atomic global; structured logging
/// uses this to decide between direct UART output and ring buffer.
///
/// Total: 19 variants (EntryPoint=0 through Complete=18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u32)]
pub enum EarlyBootPhase {
    EntryPoint = 0,
    ExceptionVectors = 1,
    DeviceTreeParsed = 2,
    UartReady = 3,
    InterruptsReady = 4,
    TimerReady = 5,
    MmuEnabled = 6,
    PageAllocatorReady = 7,
    HeapReady = 8,
    LogRingsReady = 9,
    RngReady = 10,
    KaslrApplied = 11,
    CapabilityManagerReady = 12,
    IpcReady = 13,
    AuditLogReady = 14,
    ProcessManagerReady = 15,
    ProvenanceReady = 16,
    GpuReady = 17,
    Complete = 18,
}

/// Total number of boot phase variants.
pub const EARLY_BOOT_PHASE_COUNT: usize = 19;

#[cfg(test)]
mod tests {
    use super::*;

    // --- MemoryDescriptor / MemoryType tests ---

    fn make_descriptor(ty: u32) -> MemoryDescriptor {
        MemoryDescriptor {
            ty,
            _pad: 0,
            phys_start: 0x4000_0000,
            virt_start: 0,
            page_count: 256,
            attribute: 0,
        }
    }

    #[test]
    fn memory_type_loader_code() {
        assert_eq!(make_descriptor(1).memory_type(), MemoryType::LoaderCode);
    }

    #[test]
    fn memory_type_loader_data() {
        assert_eq!(make_descriptor(2).memory_type(), MemoryType::LoaderData);
    }

    #[test]
    fn memory_type_boot_services() {
        assert_eq!(
            make_descriptor(3).memory_type(),
            MemoryType::BootServicesCode
        );
        assert_eq!(
            make_descriptor(4).memory_type(),
            MemoryType::BootServicesData
        );
    }

    #[test]
    fn memory_type_runtime_services() {
        assert_eq!(
            make_descriptor(5).memory_type(),
            MemoryType::RuntimeServicesCode
        );
        assert_eq!(
            make_descriptor(6).memory_type(),
            MemoryType::RuntimeServicesData
        );
    }

    #[test]
    fn memory_type_conventional() {
        assert_eq!(make_descriptor(7).memory_type(), MemoryType::Conventional);
    }

    #[test]
    fn memory_type_acpi() {
        assert_eq!(
            make_descriptor(9).memory_type(),
            MemoryType::AcpiReclaimable
        );
        assert_eq!(make_descriptor(10).memory_type(), MemoryType::AcpiNvs);
    }

    #[test]
    fn memory_type_mmio() {
        // Both UEFI MMIO (11) and MMIOPortSpace (12) map to MemoryMappedIO.
        assert_eq!(
            make_descriptor(11).memory_type(),
            MemoryType::MemoryMappedIO
        );
        assert_eq!(
            make_descriptor(12).memory_type(),
            MemoryType::MemoryMappedIO
        );
    }

    #[test]
    fn memory_type_reserved_default() {
        // UEFI type 0 (Reserved) maps to Reserved.
        assert_eq!(make_descriptor(0).memory_type(), MemoryType::Reserved);
        // UEFI type 8 (Unusable) maps to Reserved.
        assert_eq!(make_descriptor(8).memory_type(), MemoryType::Reserved);
        // UEFI type 13 (PalCode) maps to Reserved.
        assert_eq!(make_descriptor(13).memory_type(), MemoryType::Reserved);
        // Unknown high values map to Reserved.
        assert_eq!(make_descriptor(99).memory_type(), MemoryType::Reserved);
        assert_eq!(
            make_descriptor(u32::MAX).memory_type(),
            MemoryType::Reserved
        );
    }

    #[test]
    fn memory_type_repr_values() {
        assert_eq!(MemoryType::Conventional as u32, 0);
        assert_eq!(MemoryType::LoaderCode as u32, 1);
        assert_eq!(MemoryType::LoaderData as u32, 2);
        assert_eq!(MemoryType::Reserved as u32, 7);
    }

    #[test]
    fn memory_type_equality() {
        assert_eq!(MemoryType::Conventional, MemoryType::Conventional);
        assert_ne!(MemoryType::Conventional, MemoryType::Reserved);
    }

    // --- PixelFormat tests ---

    #[test]
    fn pixel_format_repr() {
        assert_eq!(PixelFormat::Bgr8 as u32, 0);
        assert_eq!(PixelFormat::Rgb8 as u32, 1);
    }

    #[test]
    fn pixel_format_equality() {
        assert_eq!(PixelFormat::Bgr8, PixelFormat::Bgr8);
        assert_ne!(PixelFormat::Bgr8, PixelFormat::Rgb8);
    }

    // --- MemoryDescriptor layout tests ---

    #[test]
    fn memory_descriptor_size() {
        // UEFI spec: EFI_MEMORY_DESCRIPTOR is at least 40 bytes.
        assert_eq!(core::mem::size_of::<MemoryDescriptor>(), 40);
    }

    #[test]
    fn memory_descriptor_fields() {
        let desc = make_descriptor(7);
        assert_eq!(desc.ty, 7);
        assert_eq!(desc.phys_start, 0x4000_0000);
        assert_eq!(desc.page_count, 256);
    }

    // --- EarlyBootPhase tests ---

    #[test]
    fn early_boot_phase_count() {
        assert_eq!(EARLY_BOOT_PHASE_COUNT, 19);
        assert_eq!(EarlyBootPhase::GpuReady as u32, 17);
        assert_eq!(EarlyBootPhase::Complete as u32, 18);
    }

    #[test]
    fn early_boot_phase_starts_at_zero() {
        assert_eq!(EarlyBootPhase::EntryPoint as u32, 0);
    }

    #[test]
    fn early_boot_phase_ordering() {
        assert!(EarlyBootPhase::EntryPoint < EarlyBootPhase::UartReady);
        assert!(EarlyBootPhase::UartReady < EarlyBootPhase::MmuEnabled);
        assert!(EarlyBootPhase::MmuEnabled < EarlyBootPhase::HeapReady);
        assert!(EarlyBootPhase::HeapReady < EarlyBootPhase::LogRingsReady);
        assert!(EarlyBootPhase::LogRingsReady < EarlyBootPhase::IpcReady);
        assert!(EarlyBootPhase::IpcReady < EarlyBootPhase::GpuReady);
        assert!(EarlyBootPhase::GpuReady < EarlyBootPhase::Complete);
    }

    #[test]
    fn early_boot_phase_equality() {
        assert_eq!(EarlyBootPhase::EntryPoint, EarlyBootPhase::EntryPoint);
        assert_ne!(EarlyBootPhase::EntryPoint, EarlyBootPhase::Complete);
    }

    #[test]
    fn early_boot_phase_copy() {
        let p = EarlyBootPhase::MmuEnabled;
        let p2 = p;
        assert_eq!(p, p2);
    }

    #[test]
    fn early_boot_phase_contiguous_values() {
        // All 19 variants have sequential repr values 0..=18.
        let phases = [
            EarlyBootPhase::EntryPoint,
            EarlyBootPhase::ExceptionVectors,
            EarlyBootPhase::DeviceTreeParsed,
            EarlyBootPhase::UartReady,
            EarlyBootPhase::InterruptsReady,
            EarlyBootPhase::TimerReady,
            EarlyBootPhase::MmuEnabled,
            EarlyBootPhase::PageAllocatorReady,
            EarlyBootPhase::HeapReady,
            EarlyBootPhase::LogRingsReady,
            EarlyBootPhase::RngReady,
            EarlyBootPhase::KaslrApplied,
            EarlyBootPhase::CapabilityManagerReady,
            EarlyBootPhase::IpcReady,
            EarlyBootPhase::AuditLogReady,
            EarlyBootPhase::ProcessManagerReady,
            EarlyBootPhase::ProvenanceReady,
            EarlyBootPhase::GpuReady,
            EarlyBootPhase::Complete,
        ];
        for (i, phase) in phases.iter().enumerate() {
            assert_eq!(*phase as u32, i as u32, "phase {:?} has wrong value", phase);
        }
        assert_eq!(phases.len(), EARLY_BOOT_PHASE_COUNT);
    }

    #[test]
    fn early_boot_phase_log_rings_before_ipc() {
        // LogRingsReady must come before IpcReady (logging must work before IPC).
        assert!(EarlyBootPhase::LogRingsReady < EarlyBootPhase::IpcReady);
    }

    #[test]
    fn early_boot_phase_mmu_before_heap() {
        // MMU must be enabled before heap allocator is ready.
        assert!(EarlyBootPhase::MmuEnabled < EarlyBootPhase::HeapReady);
    }
}
