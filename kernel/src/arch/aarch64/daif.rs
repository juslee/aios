//! DAIF interrupt-mask helpers.
//!
//! `with_irqs_masked` saves DAIF, masks IRQs and restores the saved value,
//! so it nests: a caller that already runs with IRQs masked (an IRQ handler,
//! or code inside another masked section) stays masked. It is a closure
//! rather than a guard object because a guard that stores its own saved DAIF
//! breaks when guards are dropped out of order (crash-fix ADR, "Rust guard
//! pitfall").

/// Run `f` on this CPU with IRQs masked, then restore DAIF as it was.
///
/// No IRQ runs on this CPU inside `f`, and the thread cannot be preempted or
/// migrated there, so a per-CPU index read inside `f` stays valid until `f`
/// returns. Keep `f` short: it delays the timer tick on this CPU.
#[inline]
pub fn with_irqs_masked<R>(f: impl FnOnce() -> R) -> R {
    let saved: u64;
    // SAFETY: Reading DAIF and setting DAIF.I are permitted at EL1. Only
    // kernel code calls this, and all kernel code runs at EL1: EL0 code
    // enters the kernel only through the exception vectors, which run at
    // EL1. Masking IRQs cannot break an invariant another context relies on.
    // The asm is not marked `nomem`, so it is a compiler barrier: memory
    // accesses in `f` cannot be moved before the mask. At EL0 with
    // SCTLR_EL1.UMA = 0 (edk2's value, which the kernel keeps) the MRS/MSR
    // would trap to EL1 as a system-register access (EC 0x18), which
    // `lower_el_sync_handler` reports as an unknown EL0 exception before
    // halting the CPU.
    unsafe {
        core::arch::asm!(
            "mrs {saved}, DAIF",
            "msr DAIFSet, #0x2",
            saved = out(reg) saved,
            options(nostack)
        );
    }
    let result = f();
    // SAFETY: `saved` is the DAIF value read on entry at EL1, so writing it
    // back restores exactly the caller's mask state: IRQs stay masked for a
    // caller that entered masked. This function is the only writer of
    // `saved`. Not `nomem`, so accesses in `f` cannot be moved after the
    // restore. Writing a different value could unmask IRQs inside an IRQ
    // handler or a caller's masked section, and the IRQ path could then
    // re-enter it.
    unsafe {
        core::arch::asm!("msr DAIF, {saved}", saved = in(reg) saved, options(nostack));
    }
    result
}
