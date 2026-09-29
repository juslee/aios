//! Instruction/data cache synchronisation for code the stub writes.
//!
//! The stub copies the kernel's executable segment with the MMU and caches on,
//! so the new instructions can sit dirty in the data cache while the
//! instruction cache still holds whatever those addresses held before.
//! AArch64 instruction fetch is not coherent with data writes unless
//! `CTR_EL0.IDC` and `CTR_EL0.DIC` say so, and real cores (Cortex-A72 on the
//! Pi 4, for one) set neither. QEMU TCG does not model caches, so a missing
//! sync never shows there.
//!
//! [`sync_icache`] uses the Arm ARM's sequence for making written instructions
//! fetchable (`DC CVAU`, `DSB`, `IC`, `DSB`, `ISB`), skipping the clean when
//! `CTR_EL0.IDC` is set and the invalidate when `CTR_EL0.DIC` is set. It is
//! based on Linux's arm64 EFI stub (`efi_cache_sync_image` in
//! `drivers/firmware/efi/libstub/arm64.c`), which issues no `DSB` before its
//! `IC IALLUIS` and never skips it. Linux cleans with `DC CVAU` only in builds
//! without `CONFIG_ARM64_WORKAROUND_CLEAN_CACHE`; its default build enables
//! that Cortex-A53 errata workaround and uses `DC CIVAC`, which reaches the
//! Point of Coherency.
//!
//! This stub cleans to the Point of Unification only, on purpose. That is
//! enough for fetches through the stub's own cacheable mapping, but not for
//! code or data that a core reads with its MMU off (secondary core bring-up);
//! the kernel has to clean those to the Point of Coherency itself.

use core::arch::asm;
use core::ops::Range;
use shared::CacheType;

/// Make `range`, just written through the data side, safe to execute.
///
/// Every byte of `range` must be mapped, readable, at the address the stub
/// wrote it through. The sequence is:
///
/// 1. read `CTR_EL0` for the data cache line size and the IDC/DIC bits;
/// 2. unless IDC: `DC CVAU` on every data cache line of `range`;
/// 3. `DSB ISH`, so the cleans (or, with IDC, the stores) complete;
/// 4. unless DIC: `IC IALLUIS`, which drops stale lines from every
///    instruction cache in the Inner Shareable domain, whatever alias they
///    were fetched through;
/// 5. `DSB ISH; ISB`, so the invalidation completes and this core refetches.
pub fn sync_icache(range: Range<u64>) {
    let ctr_el0 = read_ctr_el0();
    let ctr = CacheType::from_ctr_el0(ctr_el0);
    log::info!(
        "I/D cache sync {:#x}..{:#x}: CTR_EL0={:#x} (D-line {} B, IDC={}, DIC={})",
        range.start,
        range.end,
        ctr_el0,
        ctr.dcache_line_bytes(),
        u8::from(ctr.idc()),
        u8::from(ctr.dic()),
    );

    if !ctr.idc() {
        for addr in ctr.dcache_line_addresses(range) {
            // SAFETY: DC CVAU cleans the data cache line holding `addr` to the
            // PoU. It changes no memory contents and can fault only if `addr`
            // is not mapped readable; dcache_line_addresses yields addresses
            // inside `range` only.
            // The caller maintains this: sync_icache requires `range` to be
            // mapped, and load_elf passes the identity-mapped pages it has just
            // written the segment into.
            // If `addr` were unmapped, the instruction would take a data abort
            // into the firmware's vectors and the stub would hang before
            // ExitBootServices.
            unsafe {
                asm!("dc cvau, {addr}", addr = in(reg) addr, options(nostack, preserves_flags))
            };
        }
    }

    // SAFETY: DSB ISH waits for this core's earlier stores and cache
    // maintenance to complete in the Inner Shareable domain; it accesses no
    // memory and is legal at every exception level.
    // The architecture guarantees this; the block is not `nomem`, so the
    // compiler keeps the segment copy and the cleans before it.
    // Without it, the invalidation below could overtake the cleans and the
    // instruction cache could refill stale lines.
    unsafe { asm!("dsb ish", options(nostack, preserves_flags)) };

    if !ctr.dic() {
        // SAFETY: IC IALLUIS invalidates every instruction cache in the Inner
        // Shareable domain to the PoU. Instruction caches hold only clean
        // copies, so no data is lost, and the instruction is legal at EL1 and
        // EL2, where UEFI runs the stub.
        // The architecture and the UEFI AArch64 execution environment
        // guarantee this; a hypervisor that traps it (HCR_EL2.TPU) performs
        // it on the stub's behalf.
        // If it were not permitted, the stub would take an exception into the
        // firmware's vectors and hang before ExitBootServices.
        unsafe { asm!("ic ialluis", options(nostack, preserves_flags)) };
    }

    // SAFETY: DSB ISH waits for the invalidation to complete in the Inner
    // Shareable domain, and ISB makes this core refetch every later
    // instruction; neither accesses memory, and both are legal at every
    // exception level.
    // The architecture guarantees this; the block is not `nomem`, so the
    // compiler does not move memory accesses across it.
    // Without it, this core could still run instructions fetched before the
    // invalidation when it branches into the kernel.
    unsafe { asm!("dsb ish", "isb", options(nostack, preserves_flags)) };
}

/// Read `CTR_EL0`, the Cache Type Register.
fn read_ctr_el0() -> u64 {
    let ctr_el0: u64;
    // SAFETY: MRS CTR_EL0 reads a read-only identification register. It has no
    // side effects and touches no memory, and it is always readable at EL1
    // and EL2 (SCTLR_EL1.UCT only gates EL0).
    // The architecture guarantees this on every ARMv8-A core; UEFI runs the
    // stub at EL1 or EL2, and a hypervisor that traps the read
    // (HCR_EL2.TID2) returns an emulated value.
    // If it were not permitted, the read would take an exception into the
    // firmware's vectors and the stub would hang before ExitBootServices.
    unsafe {
        asm!(
            "mrs {ctr}, ctr_el0",
            ctr = out(reg) ctr_el0,
            options(nomem, nostack, preserves_flags)
        )
    };
    ctr_el0
}
