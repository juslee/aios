//! User memory access for syscall handlers.
//!
//! Handlers read and write user memory only through `copy_from_user` and
//! `copy_to_user`, and only with no lock held. A handler that must reject a
//! bad buffer before it has side effects (IpcCall, IpcRecv, CapabilityList)
//! also calls `validate_user_ptr` up front. A bad range is EINVAL: it is a
//! malformed argument, not a missing capability (EPERM).
//!
//! The range check keeps a copy out of the TTBR1 half. It does not put the
//! whole range in the TTBR0 half: with the T0SZ=20 that `boot.S` keeps from
//! edk2, TTBR0 translates only `[0, 2^44)`, so a validated address from
//! 0x1000_0000_0000 up to `USER_VA_LIMIT` (2^47) is in neither half and takes
//! a translation fault, like an unmapped user page. The check does not prove
//! the pages are mapped either, and what a validated address reaches depends
//! on the TTBR0 in use: the copies are sound only while TTBR0 holds nothing
//! but the calling process's user mappings. Nothing establishes that yet,
//! because nothing switches TTBR0 per thread. CPUs 1-3 keep the boot
//! identity map that `boot.S` loads (`TTBR0_L0`, built by `mmu::init_mmu`:
//! device memory below 0x4000_0000 and RAM from 0x4000_0000 to 0xC000_0000),
//! and CPU 0 keeps the test address space `main.rs` last switches to. On the
//! identity map a validated address below 0xC000_0000 reaches device MMIO or
//! the physical alias of kernel RAM, the kernel image and the frame of the
//! kernel buffer being copied included. The scheduler's per-thread TTBR0
//! switch must establish the precondition before the first EL0 process runs.
//! No EL0 process exists today, and the kernel self-tests that call
//! `syscall_dispatch` pass only ranges the check rejects, or a zero-length
//! one, which the copies never touch.
//!
//! Nothing recovers from a fault yet, and PAN is not configured: an unmapped
//! or inaccessible user page takes an EL1 data abort, which halts the CPU
//! (the current-EL synchronous vector in `arch/aarch64/exceptions.rs` ends in
//! `b .`). Fault recovery, when it exists, belongs in these two functions.

use super::IpcError;

/// Validate that a user buffer `[ptr, ptr + len)` lies in the user VA range
/// and outside page 0 (`shared::validate_user_va`). Null fails at any length.
pub(super) fn validate_user_ptr(ptr: usize, len: usize) -> bool {
    shared::validate_user_va(ptr, len)
}

/// Copy `dst.len()` bytes from the user buffer at `src` into the kernel
/// buffer `dst`. Returns EINVAL, before touching memory, unless the whole
/// source range passes `validate_user_ptr`.
///
/// The copy is a snapshot: callers decode and check `dst`, never the user
/// bytes again, so a concurrent EL0 write cannot change what was checked.
pub(super) fn copy_from_user(dst: &mut [u8], src: usize) -> Result<(), i64> {
    if !validate_user_ptr(src, dst.len()) {
        return Err(IpcError::Einval as i64);
    }
    // SAFETY: [src, src + dst.len()) is non-null and lies wholly below
    // USER_VA_LIMIT, so outside the TTBR1 half, and a byte copy has no
    // alignment requirement. TTBR0 (T0SZ=20, kept from edk2) translates only
    // [0, 2^44); an address from 2^44 up to USER_VA_LIMIT is in neither half
    // and faults like an unmapped page. The copy is sound only while TTBR0
    // holds nothing but the calling process's user mappings: the range then
    // reaches only that process's memory and cannot alias `dst`.
    // validate_user_ptr above keeps the range out of the TTBR1 half; nothing
    // maintains the TTBR0 precondition yet (module doc: CPUs 1-3 run on the
    // boot identity map, CPU 0 on main.rs's test address space). The
    // scheduler's per-thread TTBR0 switch must establish it before the first
    // EL0 process runs.
    // Violated, on the identity map a validated address below 0xC000_0000
    // reads device MMIO or the physical alias of kernel RAM, `dst`'s own
    // frame included, so an EL0 caller could copy out kernel memory; an
    // unmapped page, or an address TTBR0 does not translate, takes an EL1
    // data abort and halts the CPU.
    // Concurrency: the copy also needs no other thread to write the source
    // range while it runs. That holds today because no EL0 thread exists and
    // the kernel self-tests pass only ranges the check rejects or empty
    // ones. Once a process has several EL0 threads, one may write the range
    // from another CPU mid-copy; callers then see a torn snapshot, which is
    // harmless to them because they decode and check only `dst` and never
    // re-read the user bytes, but a non-atomic read racing a write is still
    // undefined behaviour for `copy_nonoverlapping`. The work that adds
    // fault recovery here must replace it with an asm byte copy before the
    // first EL0 process runs.
    unsafe { core::ptr::copy_nonoverlapping(src as *const u8, dst.as_mut_ptr(), dst.len()) };
    Ok(())
}

/// Copy the kernel buffer `src` to the user buffer at `dst`. Returns EINVAL,
/// before touching memory, unless the whole destination range passes
/// `validate_user_ptr`.
pub(super) fn copy_to_user(dst: usize, src: &[u8]) -> Result<(), i64> {
    if !validate_user_ptr(dst, src.len()) {
        return Err(IpcError::Einval as i64);
    }
    // SAFETY: [dst, dst + src.len()) is non-null and lies wholly below
    // USER_VA_LIMIT, so outside the TTBR1 half, and a byte copy has no
    // alignment requirement. TTBR0 (T0SZ=20, kept from edk2) translates only
    // [0, 2^44); an address from 2^44 up to USER_VA_LIMIT is in neither half
    // and faults like an unmapped page. The copy is sound only while TTBR0
    // holds nothing but the calling process's user mappings: the range then
    // reaches only that process's memory and cannot alias `src`.
    // validate_user_ptr above keeps the range out of the TTBR1 half; nothing
    // maintains the TTBR0 precondition yet (module doc: CPUs 1-3 run on the
    // boot identity map, CPU 0 on main.rs's test address space). The
    // scheduler's per-thread TTBR0 switch must establish it before the first
    // EL0 process runs.
    // Violated, on the identity map a validated address below 0xC000_0000
    // writes device MMIO or the physical alias of kernel RAM, `src`'s own
    // frame included, so an EL0 caller could overwrite kernel memory; an
    // unmapped or read-only page, or an address TTBR0 does not translate,
    // takes an EL1 data abort and halts the CPU.
    // Concurrency: the copy also needs no other thread to read or write the
    // destination range while it runs. That holds today because no EL0
    // thread exists and the kernel self-tests pass only ranges the check
    // rejects or empty ones. Once a process has several EL0 threads, one may
    // access the range from another CPU mid-copy; it then sees torn bytes,
    // which is its own race since the kernel never reads the range back, but
    // a non-atomic write racing another access is still undefined behaviour
    // for `copy_nonoverlapping`. The work that adds fault recovery here must
    // replace it with an asm byte copy before the first EL0 process runs.
    unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), dst as *mut u8, src.len()) };
    Ok(())
}
