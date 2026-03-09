//! TLB invalidation primitives for aarch64.
//!
//! All operations use Inner Shareable (IS) variants for SMP correctness
//! and include the required DSB ISH + ISB barriers.
//!
//! Per memory.md §3.4.

use super::asid::Asid;

/// Invalidate a single TLB entry for the given ASID and virtual address.
///
/// Uses `TLBI VAE1IS` — invalidates by VA, EL1, Inner Shareable.
/// The operand encodes ASID in bits [63:48] and VA >> 12 in bits [43:0].
#[allow(dead_code)]
pub fn tlb_invalidate_page(asid: Asid, va: usize) {
    let operand = ((asid.value as u64) << 48) | ((va as u64 >> 12) & 0x0FFF_FFFF_FFFF);
    // SAFETY: TLBI VAE1IS is a TLB maintenance instruction safe at EL1.
    // DSB ISH ensures completion is visible to all shareable domain PEs.
    // ISB ensures subsequent instructions use the new translations.
    unsafe {
        core::arch::asm!(
            "tlbi vae1is, {0}",
            "dsb ish",
            "isb",
            in(reg) operand,
            options(nostack, nomem),
        );
    }
}

/// Invalidate all TLB entries for the given ASID.
///
/// Uses `TLBI ASIDE1IS` — invalidates by ASID, EL1, Inner Shareable.
#[allow(dead_code)]
pub fn tlb_invalidate_asid(asid: Asid) {
    let operand = (asid.value as u64) << 48;
    // SAFETY: TLBI ASIDE1IS is a TLB maintenance instruction safe at EL1.
    unsafe {
        core::arch::asm!(
            "tlbi aside1is, {0}",
            "dsb ish",
            "isb",
            in(reg) operand,
            options(nostack, nomem),
        );
    }
}

/// Invalidate all TLB entries at EL1 (all ASIDs).
///
/// Uses `TLBI VMALLE1IS` — invalidates all, EL1, Inner Shareable.
/// Used on ASID generation wraparound and TTBR1 switch.
pub fn tlbi_all() {
    // SAFETY: TLBI VMALLE1IS is a TLB maintenance instruction safe at EL1.
    unsafe {
        core::arch::asm!("tlbi vmalle1is", "dsb ish", "isb", options(nostack, nomem),);
    }
}
