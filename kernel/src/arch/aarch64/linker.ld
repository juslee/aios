/* AIOS Kernel Linker Script — Phase 1 */

ENTRY(_start)

SECTIONS
{
    . = 0x40080000;

    /* ── Executable region ── */
    __text_start = .;

    /* Boot code first — _start is the ELF entry point at load address */
    .text.boot : { *(.text.boot) }

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
