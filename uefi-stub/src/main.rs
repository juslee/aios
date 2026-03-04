#![no_std]
#![no_main]

extern crate alloc;

mod elf;

use alloc::vec;
use core::ptr;
use shared::{BootInfo, BOOTINFO_MAGIC};
use uefi::mem::memory_map::MemoryMap;
use uefi::prelude::*;
use uefi::proto::console::gop::GraphicsOutput;
use uefi::proto::media::file::{File, FileAttribute, FileInfo, FileMode, FileType};
use uefi::proto::media::fs::SimpleFileSystem;

/// PL011 UART base address on QEMU virt — used for post-ExitBootServices output.
const UART_BASE: u64 = 0x0900_0000;
const UART_DR: u64 = UART_BASE;
const UART_FR: u64 = UART_BASE + 0x018;
const UART_FR_TXFF: u32 = 1 << 5;

/// EFI_DTB_TABLE_GUID: b1b621d5-f19c-41a5-830b-d9152c69aae0
const DTB_TABLE_GUID: uefi::Guid = uefi::guid!("b1b621d5-f19c-41a5-830b-d9152c69aae0");

/// ACPI 2.0 Table GUID: 8868e871-e4f1-11d3-bc22-0080c73c8881
const ACPI2_TABLE_GUID: uefi::Guid = uefi::guid!("8868e871-e4f1-11d3-bc22-0080c73c8881");

#[entry]
fn main() -> Status {
    uefi::helpers::init().unwrap();
    log::info!("AIOS UEFI stub");

    // --- Load kernel ELF from ESP ---
    let kernel = load_kernel();
    log::info!(
        "Kernel loaded at {:#x}, size {:#x}, entry {:#x}",
        kernel.phys_base,
        kernel.size,
        kernel.entry
    );

    // --- Allocate BootInfo page ---
    let boot_info_ptr = allocate_boot_info();

    // SAFETY: We just allocated a zeroed page for BootInfo. Pointer is valid and aligned.
    let boot_info = unsafe { &mut *boot_info_ptr };
    boot_info.magic = BOOTINFO_MAGIC;
    boot_info.kernel_phys_base = kernel.phys_base;
    boot_info.kernel_size = kernel.size;

    // --- Acquire GOP framebuffer info ---
    acquire_gop(boot_info);

    // --- Locate DTB from UEFI config tables ---
    acquire_dtb(boot_info);

    // --- Locate ACPI RSDP ---
    acquire_acpi(boot_info);

    // --- Acquire RNG seed (best-effort) ---
    acquire_rng_seed(boot_info);

    let entry_point = kernel.entry;
    let boot_info_addr = boot_info_ptr as u64;

    log::info!("Calling ExitBootServices...");

    // --- Exit Boot Services ---
    // After this call, NO UEFI services are available.
    let memory_map =
        unsafe { uefi::boot::exit_boot_services(Some(uefi::boot::MemoryType::LOADER_DATA)) };

    // Store memory map info in BootInfo.
    // We count entries via the iterator and compute buffer address from the first entry.
    // Use the UEFI-reported descriptor size (may exceed sizeof(MemoryDescriptor)).
    let map_len = memory_map.len();
    let desc_size = memory_map.meta().desc_size;
    // Get raw buffer address: the first descriptor's address is the start of the map buffer.
    let mut map_buf_addr: u64 = 0;
    if let Some(desc) = memory_map.entries().next() {
        map_buf_addr = desc as *const _ as u64;
    }
    boot_info.memory_map_addr = map_buf_addr;
    boot_info.memory_map_count = map_len as u64;
    boot_info.memory_map_entry_size = desc_size as u64;

    // Leak the MemoryMapOwned so its drop doesn't try to use the freed UEFI allocator.
    core::mem::forget(memory_map);

    // --- Post-ExitBootServices: print via raw UART ---
    uart_puts("AIOS UEFI stub: ExitBootServices OK, jumping to kernel at 0x");
    uart_put_hex(entry_point);
    uart_puts("\r\n");

    // --- Jump to kernel ---
    // SAFETY: entry_point is the kernel's _start address (validated by ELF loader).
    // boot_info_addr is a page-aligned physical address of a valid BootInfo struct.
    // After this, the UEFI stub never returns.
    unsafe {
        core::arch::asm!(
            "br {entry}",
            entry = in(reg) entry_point,
            in("x0") boot_info_addr,
            options(noreturn)
        );
    }
}

/// Load the kernel ELF from the ESP at \EFI\AIOS\aios.elf.
fn load_kernel() -> elf::LoadedKernel {
    let sfs_handle = uefi::boot::get_handle_for_protocol::<SimpleFileSystem>()
        .expect("No SimpleFileSystem protocol");

    let mut sfs = uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(sfs_handle)
        .expect("Failed to open SimpleFileSystem");

    let mut root = sfs.open_volume().expect("Failed to open root volume");

    let kernel_path = cstr16!("\\EFI\\AIOS\\aios.elf");
    let file_handle = root
        .open(kernel_path, FileMode::Read, FileAttribute::empty())
        .expect("Failed to open kernel ELF");

    let mut regular_file = match file_handle.into_type().expect("Failed to get file type") {
        FileType::Regular(f) => f,
        _ => panic!("Kernel path is not a regular file"),
    };

    // Get file size. First attempt with a small buffer; retry with the required size
    // if the filename is long enough to overflow 256 bytes.
    let mut info_buf = vec![0u8; 256];
    let info = match regular_file.get_info::<FileInfo>(&mut info_buf) {
        Ok(info) => info,
        Err(_) => {
            // Retry with a larger buffer — FileInfo includes a variable-length UTF-16 name.
            info_buf = vec![0u8; 1024];
            regular_file
                .get_info::<FileInfo>(&mut info_buf)
                .expect("Failed to get file info")
        }
    };
    let file_size = info.file_size() as usize;

    // Read entire file, handling potential short reads.
    let mut file_data = vec![0u8; file_size];
    let mut read_total = 0usize;
    while read_total < file_size {
        let n = regular_file
            .read(&mut file_data[read_total..])
            .expect("Failed to read kernel ELF");
        if n == 0 {
            break;
        }
        read_total += n;
    }
    assert_eq!(
        read_total, file_size,
        "Kernel ELF truncated: expected {} bytes, read {}",
        file_size, read_total
    );

    elf::load_elf(&file_data).expect("Failed to parse/load kernel ELF")
}

/// Allocate a page-aligned, zeroed page for BootInfo.
fn allocate_boot_info() -> *mut BootInfo {
    let addr = uefi::boot::allocate_pages(
        uefi::boot::AllocateType::AnyPages,
        uefi::boot::MemoryType::LOADER_DATA,
        1, // one 4 KiB page
    )
    .expect("Failed to allocate BootInfo page");

    let ptr = addr.as_ptr() as *mut BootInfo;
    // SAFETY: allocate_pages returns a valid, writable page.
    unsafe { ptr::write_bytes(ptr as *mut u8, 0, 4096) };
    ptr
}

/// Fill BootInfo framebuffer field from UEFI GOP.
fn acquire_gop(boot_info: &mut BootInfo) {
    let gop_handle = match uefi::boot::get_handle_for_protocol::<GraphicsOutput>() {
        Ok(h) => h,
        Err(_) => {
            log::warn!("No GOP available (headless system)");
            return;
        }
    };

    let mut gop = match uefi::boot::open_protocol_exclusive::<GraphicsOutput>(gop_handle) {
        Ok(g) => g,
        Err(_) => {
            log::warn!("Failed to open GOP protocol");
            return;
        }
    };

    let mode_info = gop.current_mode_info();
    let fb_base = gop.frame_buffer().as_mut_ptr() as u64;

    boot_info.framebuffer = fb_base;

    log::info!(
        "GOP: {}x{} format={:?} at {:#x}",
        mode_info.resolution().0,
        mode_info.resolution().1,
        mode_info.pixel_format(),
        fb_base,
    );
}

/// Fill BootInfo device_tree field from UEFI config tables.
fn acquire_dtb(boot_info: &mut BootInfo) {
    let dtb_addr = uefi::system::with_config_table(|tables| {
        for entry in tables {
            if entry.guid == DTB_TABLE_GUID {
                return Some(entry.address as u64);
            }
        }
        None
    });

    if let Some(addr) = dtb_addr {
        boot_info.device_tree = addr;
        log::info!("DTB at {:#x}", addr);
    } else {
        log::warn!("No DTB found in UEFI config tables");
    }
}

/// Fill BootInfo ACPI RSDP field from UEFI config tables.
fn acquire_acpi(boot_info: &mut BootInfo) {
    let rsdp = uefi::system::with_config_table(|tables| {
        for entry in tables {
            if entry.guid == ACPI2_TABLE_GUID {
                return Some(entry.address as u64);
            }
        }
        None
    });

    if let Some(addr) = rsdp {
        boot_info.acpi_rsdp = addr;
        log::info!("ACPI RSDP at {:#x}", addr);
    }
}

/// Fill BootInfo rng_seed (best-effort; zero-fill if RNG protocol unavailable).
fn acquire_rng_seed(boot_info: &mut BootInfo) {
    use uefi::proto::rng::Rng;

    let rng_handle = match uefi::boot::get_handle_for_protocol::<Rng>() {
        Ok(h) => h,
        Err(_) => {
            log::warn!("No EFI_RNG_PROTOCOL (rng_seed will be zero)");
            return;
        }
    };

    let mut rng = match uefi::boot::open_protocol_exclusive::<Rng>(rng_handle) {
        Ok(r) => r,
        Err(_) => return,
    };

    if rng.get_rng(None, &mut boot_info.rng_seed).is_err() {
        log::warn!("RNG get_rng failed (rng_seed will be zero)");
    }
}

// --- Raw PL011 UART output (post-ExitBootServices) ---

fn uart_putc(byte: u8) {
    // SAFETY: PL011 at 0x0900_0000 is valid MMIO on QEMU virt.
    // After ExitBootServices, this is the only way to produce serial output.
    unsafe {
        while (ptr::read_volatile(UART_FR as *const u32) & UART_FR_TXFF) != 0 {}
        ptr::write_volatile(UART_DR as *mut u32, byte as u32);
    }
}

fn uart_puts(s: &str) {
    for byte in s.bytes() {
        if byte == b'\n' {
            uart_putc(b'\r');
        }
        uart_putc(byte);
    }
}

fn uart_put_hex(val: u64) {
    for i in (0..16).rev() {
        let nibble = ((val >> (i * 4)) & 0xF) as u8;
        let c = if nibble < 10 {
            b'0' + nibble
        } else {
            b'a' + nibble - 10
        };
        uart_putc(c);
    }
}
