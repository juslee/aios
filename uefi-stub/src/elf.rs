//! Minimal ELF64 loader — only parses PT_LOAD segments for kernel loading.

use core::ptr;

const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const EM_AARCH64: u16 = 0xB7;
const PT_LOAD: u32 = 1;

#[repr(C)]
struct Elf64Header {
    e_ident: [u8; 16],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: u64,
    e_phoff: u64,
    e_shoff: u64,
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

#[repr(C)]
struct Elf64Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

/// Result of loading a kernel ELF.
pub struct LoadedKernel {
    /// Entry point address.
    pub entry: u64,
    /// Physical base address of the lowest loaded segment.
    pub phys_base: u64,
    /// Total size in memory (highest address - lowest address).
    pub size: u64,
}

/// Get a reference to the i-th program header from the ELF file buffer.
fn get_phdr(
    file_data: &[u8],
    phoff: u64,
    phentsize: u16,
    i: usize,
) -> Result<&Elf64Phdr, &'static str> {
    let offset = phoff as usize + i * phentsize as usize;
    if offset + core::mem::size_of::<Elf64Phdr>() > file_data.len() {
        return Err("Program header out of bounds");
    }
    // SAFETY: Bounds checked above. Reading from file buffer.
    Ok(unsafe { &*(file_data.as_ptr().add(offset) as *const Elf64Phdr) })
}

/// Load an ELF64 aarch64 kernel from a file buffer into physical memory.
///
/// Uses UEFI boot services to allocate pages at each segment's physical address.
/// Returns the entry point and kernel extent information.
pub fn load_elf(file_data: &[u8]) -> Result<LoadedKernel, &'static str> {
    if file_data.len() < core::mem::size_of::<Elf64Header>() {
        return Err("ELF file too small");
    }

    // SAFETY: file_data is at least sizeof(Elf64Header) bytes and aligned to u8.
    // We read fields individually to avoid alignment issues.
    let hdr = file_data.as_ptr() as *const Elf64Header;
    let (magic, class, data, machine, e_entry, e_phoff, e_phentsize, e_phnum) = unsafe {
        let h = &*hdr;
        (
            &h.e_ident[0..4],
            h.e_ident[4],
            h.e_ident[5],
            h.e_machine,
            h.e_entry,
            h.e_phoff,
            h.e_phentsize,
            h.e_phnum,
        )
    };

    if magic != ELF_MAGIC {
        return Err("Invalid ELF magic");
    }
    if class != ELFCLASS64 {
        return Err("Not ELF64");
    }
    if data != ELFDATA2LSB {
        return Err("Not little-endian");
    }
    if machine != EM_AARCH64 {
        return Err("Not aarch64");
    }

    // Pass 1: Find the total physical memory extent across all PT_LOAD segments.
    let mut lowest_addr: u64 = u64::MAX;
    let mut highest_end: u64 = 0;

    for i in 0..e_phnum as usize {
        let phdr = get_phdr(file_data, e_phoff, e_phentsize, i)?;
        if phdr.p_type != PT_LOAD {
            continue;
        }
        if phdr.p_filesz > phdr.p_memsz {
            return Err("PT_LOAD filesz > memsz");
        }
        let end = phdr.p_paddr.checked_add(phdr.p_memsz).ok_or("overflow")?;
        if phdr.p_paddr < lowest_addr {
            lowest_addr = phdr.p_paddr;
        }
        if end > highest_end {
            highest_end = end;
        }
    }

    if lowest_addr == u64::MAX {
        return Err("No PT_LOAD segments found");
    }

    // Allocate the entire kernel range in one shot (page-aligned base).
    let alloc_base = lowest_addr & !0xFFF;
    let alloc_end = highest_end.div_ceil(0x1000) * 0x1000;
    let total_pages = ((alloc_end - alloc_base) / 0x1000) as usize;

    let status = uefi::boot::allocate_pages(
        uefi::boot::AllocateType::Address(alloc_base),
        uefi::boot::MemoryType::LOADER_DATA,
        total_pages,
    );
    if status.is_err() {
        return Err("Failed to allocate pages for kernel");
    }

    // Zero the entire allocated region so BSS is clean.
    // SAFETY: We just allocated total_pages at alloc_base.
    unsafe { ptr::write_bytes(alloc_base as *mut u8, 0, total_pages * 0x1000) };

    // Pass 2: Copy PT_LOAD segment file data into physical memory.
    for i in 0..e_phnum as usize {
        let phdr = get_phdr(file_data, e_phoff, e_phentsize, i)?;
        if phdr.p_type != PT_LOAD {
            continue;
        }

        let file_end = phdr.p_offset.checked_add(phdr.p_filesz).ok_or("overflow")?;
        if file_end as usize > file_data.len() {
            return Err("PT_LOAD segment data out of bounds");
        }

        // SAFETY: Destination is within our allocated region; source is within file buffer.
        unsafe {
            let src = file_data.as_ptr().add(phdr.p_offset as usize);
            let dst = phdr.p_paddr as *mut u8;
            ptr::copy_nonoverlapping(src, dst, phdr.p_filesz as usize);
        }
    }

    Ok(LoadedKernel {
        entry: e_entry,
        phys_base: lowest_addr,
        size: highest_end - lowest_addr,
    })
}
