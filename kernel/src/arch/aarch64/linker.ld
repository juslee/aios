/* AIOS Kernel Linker Script — Phase 2 M8 (Virtual Linking) */
/*
 * VMA: kernel code/data lives at virtual addresses starting near KERNEL_VIRT.
 * LMA: physical load address remains at KERNEL_PHYS (set by AT clause).
 *
 * The VMA is offset by the intra-2MB position of KERNEL_PHYS so that
 * boot.S can use a single 2MB block descriptor to map the kernel:
 *   L2[0] → PA 0x40000000 (2MB-aligned), covering the kernel image.
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

    /* Boot stack: 16 KiB, 16-byte aligned (ABI requirement) */
    . = ALIGN(16);
    __stack_bottom = .;
    . += 16384;
    __stack_top = .;

    . = ALIGN(4096);
    __data_end = .;

    __kernel_end = .;

    /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
}
