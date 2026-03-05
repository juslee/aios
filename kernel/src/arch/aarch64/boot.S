// AIOS aarch64 boot assembly — Phase 1
//
// Entry: x0 = BootInfo physical address (set by UEFI stub)
//
// Boot order (strict):
// 0. Save BootInfo pointer to callee-saved x19
// 1. Enable FP/NEON (must be first — hard-float ABI)
// 2. Install stub exception vectors to VBAR_EL1
// 3. Park secondary cores (wfe loop)
// 4. Set stack pointer
// 5. Zero BSS
// 6. Branch to kernel_main with x0 = BootInfo pointer

// ---------------------------------------------------------------------------
// Exception vector table (stub) — 16 entries, 128 bytes each
// All entries branch-to-self. Safety net for faults between _start and
// kernel_main. kernel_main installs the Rust-owned table from exceptions.rs.
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
    // Step 0: Save BootInfo pointer (x0) to callee-saved x19 immediately.
    // x0 = BootInfo physical address from UEFI stub. Must be preserved
    // across the entire boot sequence so kernel_main receives it.
    mov     x19, x0

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

    // Step 6: Branch to kernel_main with BootInfo pointer in x0.
    mov     x0, x19
    bl      kernel_main

    // Unreachable — kernel_main is -> !, guard against return
.Lpark:
    wfe
    b       .Lpark

// ---------------------------------------------------------------------------
// _secondary_entry — entry point for secondary cores woken by PSCI CPU_ON
//
// On entry (per SMCCC): x0 = context_id (core index, set by boot CPU)
// MMU is OFF. We must enable it with the same MAIR/TCR/TTBR0/SCTLR as
// the boot CPU before jumping to Rust code.
// ---------------------------------------------------------------------------
.global _secondary_entry
_secondary_entry:
    // Save core_id (from PSCI context_id in x0) to callee-saved x19.
    mov     x19, x0

    // 1. Enable FP/NEON — same as boot CPU. Hard-float ABI requires this
    //    before any Rust code or NEON instruction.
    mrs     x1, CPACR_EL1
    orr     x1, x1, #(3 << 20)
    msr     CPACR_EL1, x1
    isb

    // 2. Install Rust exception vector table (same as boot CPU).
    adrp    x1, __vector_table_el1
    add     x1, x1, :lo12:__vector_table_el1
    msr     VBAR_EL1, x1
    isb

    // 3. Enable MMU with boot CPU's configuration.
    // PSCI starts cores with MMU OFF, so writing MAIR/TCR/TTBR0 is safe
    // (no CONSTRAINED UNPREDICTABLE when MMU is off).

    // Load MAIR_EL1 from Rust static BOOT_MAIR
    adrp    x1, BOOT_MAIR
    add     x1, x1, :lo12:BOOT_MAIR
    ldr     x2, [x1]
    msr     MAIR_EL1, x2

    // Load TCR_EL1 from Rust static BOOT_TCR
    adrp    x1, BOOT_TCR
    add     x1, x1, :lo12:BOOT_TCR
    ldr     x2, [x1]
    msr     TCR_EL1, x2

    // Load TTBR0_EL1 — address of L0 page table (identity map).
    // TTBR0_L0 is a RawPageTable struct; the entries start at offset 0.
    adrp    x1, TTBR0_L0
    add     x1, x1, :lo12:TTBR0_L0
    msr     TTBR0_EL1, x1

    // Ensure system register writes complete before enabling MMU.
    isb
    // Ensure boot CPU's page table memory writes are visible to our table walker.
    dsb     sy

    // Load and write SCTLR_EL1 (contains M bit to enable MMU).
    adrp    x1, BOOT_SCTLR
    add     x1, x1, :lo12:BOOT_SCTLR
    ldr     x2, [x1]
    msr     SCTLR_EL1, x2
    // MMU is now ON — ensure it takes effect before next instruction fetch.
    isb

    // 4. Load per-core stack pointer from SECONDARY_STACKS[core_id].
    // SECONDARY_STACKS is an array of u64 (stack top addresses).
    adrp    x1, SECONDARY_STACKS
    add     x1, x1, :lo12:SECONDARY_STACKS
    ldr     x2, [x1, x19, lsl #3]  // x2 = SECONDARY_STACKS[core_id]
    mov     sp, x2

    // 5. Branch to secondary_main(core_id) in Rust.
    mov     x0, x19
    bl      secondary_main

    // Unreachable — secondary_main is -> !
    b       .Lpark
