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

/// Get the i-th program header from the ELF file buffer.
fn get_phdr(
    file_data: &[u8],
    phoff: u64,
    phentsize: u16,
    i: usize,
) -> Result<Elf64Phdr, &'static str> {
    if (phentsize as usize) < core::mem::size_of::<Elf64Phdr>() {
        return Err("Program header entry size too small");
    }
    let offset = phoff as usize + i * phentsize as usize;
    if offset + core::mem::size_of::<Elf64Phdr>() > file_data.len() {
        return Err("Program header out of bounds");
    }
    // SAFETY: Bounds checked above. `read_unaligned` handles the potentially unaligned
    // file buffer (Vec<u8> alignment is 1, but Elf64Phdr has multi-byte fields).
    Ok(unsafe { ptr::read_unaligned(file_data.as_ptr().add(offset) as *const Elf64Phdr) })
}

/// Load an ELF64 aarch64 kernel from a file buffer into physical memory.
///
/// Uses UEFI boot services to allocate pages at each segment's physical address.
/// Returns the entry point and kernel extent information.
pub fn load_elf(file_data: &[u8]) -> Result<LoadedKernel, &'static str> {
    if file_data.len() < core::mem::size_of::<Elf64Header>() {
        return Err("ELF file too small");
    }

    // SAFETY: file_data is at least sizeof(Elf64Header) bytes. `read_unaligned` copies
    // the header into a properly aligned local, avoiding UB from unaligned references.
    let hdr = unsafe { ptr::read_unaligned(file_data.as_ptr() as *const Elf64Header) };

    let magic = &hdr.e_ident[0..4];
    let class = hdr.e_ident[4];
    let data = hdr.e_ident[5];
    let machine = hdr.e_machine;
    let e_entry = hdr.e_entry;
    let e_phoff = hdr.e_phoff;
    let e_phentsize = hdr.e_phentsize;
    let e_phnum = hdr.e_phnum;

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
    // Also track lowest_vaddr for virtual-to-physical entry point conversion
    // (needed after Phase 2 M8 virtual linking: e_entry is a virtual address).
    let mut lowest_paddr: u64 = u64::MAX;
    let mut lowest_vaddr: u64 = u64::MAX;
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
        if phdr.p_paddr < lowest_paddr {
            lowest_paddr = phdr.p_paddr;
            lowest_vaddr = phdr.p_vaddr;
        }
        if end > highest_end {
            highest_end = end;
        }
    }

    if lowest_paddr == u64::MAX {
        return Err("No PT_LOAD segments found");
    }

    // Allocate the entire kernel range in one shot (page-aligned base).
    let alloc_base = lowest_paddr & !0xFFF;
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

    // Convert virtual entry point to physical address.
    // With virtual linking (Phase 2 M8), e_entry is a virtual address.
    // Physical entry = e_entry - lowest_vaddr + lowest_paddr.
    let phys_entry = e_entry
        .wrapping_sub(lowest_vaddr)
        .wrapping_add(lowest_paddr);

    Ok(LoadedKernel {
        entry: phys_entry,
        phys_base: lowest_paddr,
        size: highest_end - lowest_paddr,
    })
}
