//! PSCI (Power State Coordination Interface) wrappers.
//!
//! Provides CPU_ON for SMP secondary core bringup via HVC or SMC,
//! following the SMCCC (SMC Calling Convention) ABI.

/// PSCI function IDs (64-bit SMCCC).
#[allow(dead_code)]
pub const CPU_ON_64: u64 = 0xC400_0003;
#[allow(dead_code)]
pub const SYSTEM_RESET: u64 = 0x8400_0009;
#[allow(dead_code)]
pub const SYSTEM_OFF: u64 = 0x8400_0008;

/// Call PSCI CPU_ON via HVC (QEMU, KVM, Apple Silicon).
///
/// Arguments follow SMCCC: x0=function_id, x1=target_cpu (MPIDR),
/// x2=entry_point, x3=context_id. Returns PSCI status in x0.
#[allow(dead_code)]
pub fn cpu_on_hvc(target_cpu: u64, entry_point: u64, context_id: u64) -> i64 {
    let ret: u64;
    // SAFETY: HVC #0 is the SMCCC conduit for hypervisor calls. The function ID
    // CPU_ON_64 is a well-defined PSCI operation. clobber_abi("C") accounts for
    // SMCCC clobbering x4-x17. We must NOT use options(nomem) because the woken
    // core reads memory (stack, page tables) written by this core — the compiler
    // must not reorder those writes past this call.
    unsafe {
        core::arch::asm!(
            "hvc #0",
            inout("x0") CPU_ON_64 => ret,
            in("x1") target_cpu,
            in("x2") entry_point,
            in("x3") context_id,
            clobber_abi("C"),
        );
    }
    ret as i64
}

/// Call PSCI CPU_ON via SMC (real hardware with EL3 firmware).
#[allow(dead_code)]
pub fn cpu_on_smc(target_cpu: u64, entry_point: u64, context_id: u64) -> i64 {
    let ret: u64;
    // SAFETY: Same rationale as cpu_on_hvc but using SMC conduit.
    unsafe {
        core::arch::asm!(
            "smc #0",
            inout("x0") CPU_ON_64 => ret,
            in("x1") target_cpu,
            in("x2") entry_point,
            in("x3") context_id,
            clobber_abi("C"),
        );
    }
    ret as i64
}
