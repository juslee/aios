/* AIOS Kernel Linker Script — Phase 2 M8 (Virtual Linking) */
/*
 * VMA: kernel code/data lives at virtual addresses starting near KERNEL_VIRT.
 * LMA: physical load address remains at KERNEL_PHYS (set by AT clause).
 *
 * The VMA is offset by the intra-2MB position of KERNEL_PHYS, which keeps
 * the VMA-LMA delta 2MB-aligned so that boot.S can map the kernel with
 * 2MB block descriptors:
 *   L2[0..3] → PA 0x40000000-0x40800000 (8 MiB), covering the kernel image.
 * The ASSERT at the end of SECTIONS enforces the fit.
 *
 * PC-relative ADRP+ADD instructions resolve correctly in both physical
 * execution (before TTBR1) and virtual execution (after TTBR1) because
 * the VMA-LMA offset is constant across all sections.
 */

ENTRY(_start)

KERNEL_VIRT = 0xFFFF000000000000;
KERNEL_PHYS = 0x40080000;

/* Symbols for boot.S and kernel code */
PROVIDE(__kernel_virt_base = KERNEL_VIRT);
PROVIDE(__kernel_phys_base = KERNEL_PHYS);
PROVIDE(__virt_phys_offset = KERNEL_VIRT + (KERNEL_PHYS & 0x1FFFFF) - KERNEL_PHYS);

SECTIONS
{
    /* VMA = KERNEL_VIRT + intra-2MB offset of KERNEL_PHYS */
    . = KERNEL_VIRT + (KERNEL_PHYS & 0x1FFFFF);

    /* ── Executable region ── */
    __text_start = .;

    /* Boot code first — _start is the ELF entry point */
    .text.boot : AT(KERNEL_PHYS) { *(.text.boot) }

    /* Boot stub exception vectors (safety net for early boot) */
    .text.vectors ALIGN(2048) : {
        __vectors_start = .;
        *(.text.vectors)
        __vectors_end = .;
    }

    /* Rust-owned exception vector table (installed from kernel_main) */
    .text.rvectors ALIGN(2048) : { *(.text.rvectors) }

    .text : { *(.text .text.*) }

    . = ALIGN(4096);
    __text_end = .;

    /* ── Read-only data region ── */
    __rodata_start = .;
    .rodata : ALIGN(4096) { *(.rodata .rodata.*) }
    . = ALIGN(4096);
    __rodata_end = .;

    /* ── Read-write data region (data + bss + stack) ── */
    __data_start = .;
    .data : ALIGN(4096) { *(.data .data.*) }

    . = ALIGN(8);
    __bss_start = .;
    .bss : { *(.bss .bss.*) *(COMMON) }
    . = ALIGN(8);
    __bss_end = .;

    /* Boot stack: 128 KiB, 16-byte aligned (ABI requirement).
       ipc::init() alone uses ~62 KiB (ProcessControl temporaries +
       Thread construction) and nested calls (channel_create ~4.5 KiB)
       push total past 64 KiB. 128 KiB gives ample headroom.

       The stack is an output section, not a bare location-counter
       advance, so it falls inside the RW PT_LOAD segment's memsz. The
       UEFI stub allocates pages per PT_LOAD segment and derives
       BootInfo.kernel_size from them; a stack outside every segment
       would sit on pages the firmware still reports free (#198).
       NOLOAD makes it SHT_NOBITS: no file bytes. The stub zero-fills
       the segment; boot.S does not zero it (it lies past __bss_end). */
    .stack (NOLOAD) : ALIGN(16) {
        __stack_bottom = .;
        . += 131072;
        __stack_top = .;
    }

    . = ALIGN(4096);
    __data_end = .;

    __kernel_end = .;

    /* Until kmap.rs installs the full TTBR1, boot.S's minimal TTBR1 maps
       only VA [KERNEL_VIRT, KERNEL_VIRT + 8 MiB) (4 x 2 MiB L2 blocks at
       PA 0x40000000). kernel_main runs on the boot stack through that
       map, so the whole image, stack included, must fit inside it. */
    ASSERT(__kernel_end <= KERNEL_VIRT + 0x800000,
           "kernel image (incl. boot stack) exceeds boot.S's 8 MiB TTBR1 map")

    /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
}
