// AIOS aarch64 boot assembly — Phase 0
//
// Boot order (strict):
// 1. Enable FP/NEON (must be first — hard-float ABI)
// 2. Install stub exception vectors to VBAR_EL1
// 3. Park secondary cores (wfe loop)
// 4. Set stack pointer
// 5. Zero BSS
// 6. Branch to kernel_main

// ---------------------------------------------------------------------------
// Exception vector table (stub) — 16 entries, 128 bytes each
// All entries branch-to-self; replaced by Rust table in M3 Step 8
// ---------------------------------------------------------------------------
.section .text.vectors, "ax"
.balign 2048                    // 2048-byte alignment for VBAR_EL1

.global __exception_vectors
__exception_vectors:

// Current EL with SP_EL0
.balign 128
    b .                         // Synchronous
.balign 128
    b .                         // IRQ
.balign 128
    b .                         // FIQ
.balign 128
    b .                         // SError

// Current EL with SP_ELx
.balign 128
    b .                         // Synchronous
.balign 128
    b .                         // IRQ
.balign 128
    b .                         // FIQ
.balign 128
    b .                         // SError

// Lower EL using AArch64
.balign 128
    b .                         // Synchronous
.balign 128
    b .                         // IRQ
.balign 128
    b .                         // FIQ
.balign 128
    b .                         // SError

// Lower EL using AArch32
.balign 128
    b .                         // Synchronous
.balign 128
    b .                         // IRQ
.balign 128
    b .                         // FIQ
.balign 128
    b .                         // SError

// ---------------------------------------------------------------------------
// _start — entry point, placed in .text.boot at 0x40080000
// ---------------------------------------------------------------------------
.section .text.boot, "ax"
.global _start
_start:
    // Step 1: Enable FP/NEON (CPACR_EL1.FPEN = 0b11)
    // Must be first — hard-float ABI means compiler emits NEON freely.
    // Any NEON instruction before this enable traps.
    mrs     x1, CPACR_EL1
    orr     x1, x1, #(3 << 20)
    msr     CPACR_EL1, x1
    isb

    // Step 2: Install stub exception vectors to VBAR_EL1
    // Any fault before this jumps to garbage; after this, faults
    // deterministically halt (branch-to-self).
    adrp    x1, __exception_vectors
    add     x1, x1, :lo12:__exception_vectors
    msr     VBAR_EL1, x1
    isb

    // Step 3: Park secondary cores
    // Read MPIDR_EL1, extract Aff0 (bits [7:0]) for core ID.
    // Only core 0 continues; others enter wfe loop.
    mrs     x1, MPIDR_EL1
    and     x1, x1, #0xFF
    cbnz    x1, .Lpark

    // Step 4: Set stack pointer
    // Stack grows downward; __stack_top defined in linker.ld, 16-byte aligned.
    adrp    x1, __stack_top
    add     x1, x1, :lo12:__stack_top
    mov     sp, x1

    // Step 5: Zero BSS
    // __bss_start and __bss_end are 8-byte aligned in linker.ld.
    adrp    x1, __bss_start
    add     x1, x1, :lo12:__bss_start
    adrp    x2, __bss_end
    add     x2, x2, :lo12:__bss_end
.Lbss_loop:
    cmp     x1, x2
    b.ge    .Lbss_done
    str     xzr, [x1], #8
    b       .Lbss_loop
.Lbss_done:

    // Step 6: Branch to kernel_main (Rust entry point, never returns)
    bl      kernel_main

    // Unreachable — kernel_main is -> !, guard against return
.Lpark:
    wfe
    b       .Lpark
