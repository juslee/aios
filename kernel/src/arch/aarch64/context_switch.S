// Context switch assembly for aarch64.
//
// save_context(ctx: *mut ThreadContext)   — saves callee-saved regs + SP + LR
// restore_context(ctx: *const ThreadContext) — restores and branches to saved LR
//
// Used for kernel-to-kernel context switching (M11 scheduler).
// For user↔kernel transitions, the TrapFrame save/restore in the
// vector table stubs handles the full register set.
//
// ThreadContext layout (repr(C), 296 bytes):
//   gp_regs[0..31]  = offsets 0x000..0x0F8  (x0-x30)
//   sp              = offset  0x0F8
//   pc              = offset  0x100
//   pstate          = offset  0x108
//   ttbr0           = offset  0x110
//   timer_cval      = offset  0x118
//   timer_ctl       = offset  0x120

.section .text, "ax"

// save_context: save callee-saved registers into ThreadContext at x0.
// Saves x19-x30 (callee-saved), SP, and LR (return address → pc field).
.global save_context
save_context:
    // Save callee-saved registers x19-x30 into their gp_regs slots.
    stp x19, x20, [x0, #(19*8)]
    stp x21, x22, [x0, #(21*8)]
    stp x23, x24, [x0, #(23*8)]
    stp x25, x26, [x0, #(25*8)]
    stp x27, x28, [x0, #(27*8)]
    stp x29, x30, [x0, #(29*8)]
    // Save SP → sp field (offset 0x0F8 = 31*8).
    mov x1, sp
    str x1, [x0, #(31*8)]
    // Save LR (x30) → pc field (offset 0x100 = 32*8).
    // When restore_context loads this, it branches back to the call site.
    str x30, [x0, #(32*8)]
    ret

// restore_context: restore callee-saved registers from ThreadContext at x0.
// Restores x19-x30, SP, then branches to the saved pc (LR).
.global restore_context
restore_context:
    // Restore callee-saved registers x19-x30.
    ldp x19, x20, [x0, #(19*8)]
    ldp x21, x22, [x0, #(21*8)]
    ldp x23, x24, [x0, #(23*8)]
    ldp x25, x26, [x0, #(25*8)]
    ldp x27, x28, [x0, #(27*8)]
    ldp x29, x30, [x0, #(29*8)]
    // Restore SP from sp field.
    ldr x1, [x0, #(31*8)]
    mov sp, x1
    // Load saved pc into x1 and branch to it.
    ldr x1, [x0, #(32*8)]
    br x1
