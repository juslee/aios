/* AIOS Kernel Linker Script — Phase 0 */

ENTRY(_start)

SECTIONS
{
    . = 0x40080000;

    /* Boot code first — _start is the ELF entry point at load address */
    .text.boot : { *(.text.boot) }

    /* Exception vector table: VBAR_EL1 requires 2048-byte alignment */
    .text.vectors ALIGN(2048) : {
        __vectors_start = .;
        *(.text.vectors)
        __vectors_end = .;
    }

    .text : { *(.text .text.*) }
    .rodata : ALIGN(8) { *(.rodata .rodata.*) }
    .data : ALIGN(8) { *(.data .data.*) }

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

    /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
}
