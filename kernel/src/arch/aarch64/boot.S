// AIOS aarch64 boot assembly — Phase 2 M8 (Virtual Linking)
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
// 6. Build minimal TTBR1 page tables for virtual kernel execution
// 7. Configure TCR for TTBR1, install TTBR1_EL1
// 8. Branch to kernel_main at virtual address

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
// _start — entry point, placed in .text.boot at physical 0x40080000
//
// With virtual linking (Phase 2 M8), all symbols have virtual VMAs but
// the code is loaded at physical LMA. PC-relative ADRP+ADD resolves to
// physical addresses while PC is physical. After TTBR1 setup, we branch
// to the virtual kernel_main address.
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
    // ADRP+ADD resolves to physical address (PC is physical).
    adrp    x1, __stack_top
    add     x1, x1, :lo12:__stack_top
    mov     sp, x1

    // Step 5: Zero BSS
    // __bss_start and __bss_end are 8-byte aligned in linker.ld.
    // ADRP+ADD gives physical addresses since PC is physical.
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

    // ─────────────────────────────────────────────────────────────────────
    // Step 6: Build minimal TTBR1 page tables for virtual kernel execution
    //
    // Strategy: 3 static page table pages (L0, L1, L2) in BSS.
    // L2 uses 2MB block descriptors covering the kernel image at
    // PA 0x40000000-0x40800000 (4 blocks = 8 MiB).
    //
    // VA layout: KERNEL_VIRT + intra-2MB offset = 0xFFFF_0000_0008_0000
    // maps to PA 0x40080000 (kernel load address).
    //
    // L0[0] → L1, L1[0] → L2, L2[0..3] → 2MB blocks
    //
    // This minimal mapping is replaced by full TTBR1 (W^X, direct map,
    // KASLR) in kernel_main after pool initialization.
    // ─────────────────────────────────────────────────────────────────────

    // Build L2 block descriptors: 4 × 2MB blocks covering kernel image
    adrp    x1, ttbr1_boot_l2
    add     x1, x1, :lo12:ttbr1_boot_l2

    // Block descriptor attributes (no PA):
    //   VALID=1, AttrIdx=3(WB), SH=ISH(0b11), AF=1, UXN=1
    //   bits: 0x0040_0000_0000_070D
    //   - bit  0:     VALID
    //   - bits 4:2:   AttrIdx = 3 (MAIR WB cacheable)
    //   - bits 9:8:   SH = Inner Shareable
    //   - bit 10:     AF (Access Flag)
    //   - bit 54:     UXN (Unprivileged Execute-Never)
    //   - PXN clear:  kernel can execute (needed for .text)
    movz    x4, #0x070D                     // lower attr bits
    movk    x4, #0x0040, lsl #48            // UXN (bit 54)

    // L2[0]: PA 0x40000000 — contains kernel at offset 0x80000
    movz    x5, #0x4000, lsl #16
    orr     x5, x5, x4
    str     x5, [x1, #0]

    // L2[1]: PA 0x40200000
    movz    x5, #0x4020, lsl #16
    orr     x5, x5, x4
    str     x5, [x1, #8]

    // L2[2]: PA 0x40400000
    movz    x5, #0x4040, lsl #16
    orr     x5, x5, x4
    str     x5, [x1, #16]

    // L2[3]: PA 0x40600000
    movz    x5, #0x4060, lsl #16
    orr     x5, x5, x4
    str     x5, [x1, #24]

    // Build L1[0] → L2 table descriptor (L2_phys | TABLE | VALID)
    orr     x2, x1, #3
    adrp    x3, ttbr1_boot_l1
    add     x3, x3, :lo12:ttbr1_boot_l1
    str     x2, [x3]                        // L1[0] → L2

    // Build L0[0] → L1 table descriptor (L1_phys | TABLE | VALID)
    orr     x2, x3, #3
    adrp    x6, ttbr1_boot_l0
    add     x6, x6, :lo12:ttbr1_boot_l0
    str     x2, [x6]                        // L0[0] → L1

    // ─────────────────────────────────────────────────────────────────────
    // Step 7: Configure TCR_EL1 for TTBR1 and install TTBR1_EL1
    //
    // TCR bits[31:16] control TTBR1 translation:
    //   T1SZ  [21:16] = 16  → 48-bit kernel VA (2^48 address space)
    //   A1    [22]    = 0   → ASID from TTBR0
    //   EPD1  [23]    = 0   → enable TTBR1 walks (edk2 may set 1)
    //   IRGN1 [25:24] = 01  → Write-Back, Write-Allocate
    //   ORGN1 [27:26] = 01  → Write-Back, Write-Allocate
    //   SH1   [29:28] = 11  → Inner Shareable
    //   TG1   [31:30] = 10  → 4 KiB granule
    //
    //   Combined: 0xB510 in bits[31:16]
    //
    // Preserves bits[15:0] (TTBR0 config from edk2: T0SZ=20, etc.)
    //
    // Safe to modify TCR while MMU is on: TTBR1 translations are not
    // yet active (EPD1 was 1 or T1SZ was unconfigured). After we write
    // TCR → ISB → TTBR1 → ISB, TTBR1 translations become live.
    // ─────────────────────────────────────────────────────────────────────
    mrs     x1, TCR_EL1
    movz    x2, #0xFFFF, lsl #16
    bic     x1, x1, x2                      // Clear bits[31:16]
    movz    x2, #0xB510, lsl #16            // TTBR1 config
    orr     x1, x1, x2
    msr     TCR_EL1, x1
    isb

    // Ensure page table writes are visible to the table walker
    dsb     sy

    // Write TTBR1_EL1 with L0 table physical address
    msr     TTBR1_EL1, x6                   // x6 = ttbr1_boot_l0 phys addr
    isb

    // Invalidate TLB and synchronize
    tlbi    vmalle1
    dsb     nsh
    isb

    // ─────────────────────────────────────────────────────────────────────
    // Step 8: Branch to kernel_main at virtual address
    //
    // ADRP+ADD resolves to the physical address of kernel_main (PC is
    // physical, VMA-LMA offset is constant). We add the virt-phys offset
    // to convert to the virtual address.
    //
    // Virt-phys offset = KERNEL_VIRT + (KERNEL_PHYS & 0x1FFFFF) - KERNEL_PHYS
    //                  = 0xFFFF_0000_0008_0000 - 0x4008_0000
    //                  = 0xFFFE_FFFF_C000_0000
    //
    // After `br x2`, instruction fetches go through TTBR1 (upper-half VA).
    // Stack pointer remains physical (lower-half), accessed via TTBR0.
    // ─────────────────────────────────────────────────────────────────────
    adrp    x2, kernel_main
    add     x2, x2, :lo12:kernel_main       // x2 = physical addr of kernel_main

    // Load virt-phys offset: 0xFFFE_FFFF_C000_0000
    movz    x3, #0xFFFE, lsl #48
    movk    x3, #0xFFFF, lsl #32
    movk    x3, #0xC000, lsl #16
    add     x2, x2, x3                      // x2 = virtual addr of kernel_main

    mov     x0, x19                          // x0 = BootInfo pointer (physical)
    br      x2                               // Branch to virtual kernel_main

    // Unreachable — kernel_main is -> !, guard against return
.Lpark:
    wfe
    b       .Lpark

// ---------------------------------------------------------------------------
// _secondary_entry — entry point for secondary cores woken by PSCI CPU_ON
//
// On entry (per SMCCC): x0 = context_id (core index, set by boot CPU)
// MMU is OFF. We must enable it with the same MAIR/TCR/TTBR0/TTBR1/SCTLR
// as the boot CPU before jumping to Rust code.
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
    // PSCI starts cores with MMU OFF, so writing MAIR/TCR/TTBR0/TTBR1 is safe
    // (no CONSTRAINED UNPREDICTABLE when MMU is off).

    // Load MAIR_EL1 from Rust static BOOT_MAIR
    adrp    x1, BOOT_MAIR
    add     x1, x1, :lo12:BOOT_MAIR
    ldr     x2, [x1]
    msr     MAIR_EL1, x2

    // Load TCR_EL1 from Rust static BOOT_TCR
    // (includes TTBR1 config: T1SZ=16, TG1=4KB, SH1=ISH, etc.)
    adrp    x1, BOOT_TCR
    add     x1, x1, :lo12:BOOT_TCR
    ldr     x2, [x1]
    msr     TCR_EL1, x2

    // Load TTBR0_EL1 — address of L0 page table (identity map).
    // TTBR0_L0 is a RawPageTable struct; the entries start at offset 0.
    adrp    x1, TTBR0_L0
    add     x1, x1, :lo12:TTBR0_L0
    msr     TTBR0_EL1, x1

    // Load TTBR1_EL1 — same boot page tables as boot CPU.
    // ttbr1_boot_l0 is the minimal TTBR1 L0 table built by boot CPU in step 6.
    adrp    x1, ttbr1_boot_l0
    add     x1, x1, :lo12:ttbr1_boot_l0
    msr     TTBR1_EL1, x1

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
    // ADRP+ADD resolves to physical addresses (PC is physical after MMU
    // enable with identity map). Stack is physical, accessed via TTBR0.
    adrp    x1, SECONDARY_STACKS
    add     x1, x1, :lo12:SECONDARY_STACKS
    ldr     x2, [x1, x19, lsl #3]  // x2 = SECONDARY_STACKS[core_id]
    mov     sp, x2

    // 5. Branch to secondary_main at virtual address.
    // Convert physical address to virtual using the same offset as boot CPU.
    adrp    x2, secondary_main
    add     x2, x2, :lo12:secondary_main

    // Add virt-phys offset: 0xFFFE_FFFF_C000_0000
    movz    x3, #0xFFFE, lsl #48
    movk    x3, #0xFFFF, lsl #32
    movk    x3, #0xC000, lsl #16
    add     x2, x2, x3

    mov     x0, x19
    br      x2                      // Branch to virtual secondary_main

    // Unreachable — secondary_main is -> !
    b       .Lpark

// ---------------------------------------------------------------------------
// TTBR1 boot page tables — 3 pages in BSS (zeroed by step 5)
//
// These provide a minimal kernel mapping for virtual execution.
// L0[0] → L1 → L2 with 2MB block descriptors.
// Replaced by full TTBR1 (W^X, KASLR, direct map) from kernel_main.
// ---------------------------------------------------------------------------
.section .bss
.balign 4096
.global ttbr1_boot_l0
ttbr1_boot_l0:
    .space 4096

.balign 4096
.global ttbr1_boot_l1
ttbr1_boot_l1:
    .space 4096

.balign 4096
.global ttbr1_boot_l2
ttbr1_boot_l2:
    .space 4096
