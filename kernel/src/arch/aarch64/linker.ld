/* AIOS Kernel Linker Script — Phase 0 stub */
/* Full linker script with sections added in Step 3 */

ENTRY(_start)

SECTIONS
{
    . = 0x40080000;

    .text : { *(.text .text.*) }
    .rodata : { *(.rodata .rodata.*) }
    .data : { *(.data .data.*) }

    . = ALIGN(8);
    __bss_start = .;
    .bss : { *(.bss .bss.*) *(COMMON) }
    __bss_end = .;

    /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
}
