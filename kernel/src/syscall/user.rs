//! User memory access for syscall handlers.
//!
//! Handlers read and write user memory only through `copy_from_user` and
//! `copy_to_user`, and only with no lock held. A handler that must reject a
//! bad buffer before it has side effects (IpcCall, IpcRecv, CapabilityList)
//! also calls `validate_user_ptr` up front. A bad range is EINVAL: it is a
//! malformed argument, not a missing capability (EPERM).
//!
//! The range check does not prove the pages are mapped. Nothing recovers from
//! a fault yet, and PAN is not configured: an unmapped or inaccessible user
//! page takes an EL1 data abort, which halts the CPU (the current-EL
//! synchronous vector in `arch/aarch64/exceptions.rs` ends in `b .`). Fault
//! recovery, when it exists, belongs in these two functions.

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
    // USER_VA_LIMIT, so it cannot overlap `dst`, a kernel buffer in the TTBR1
    // half, and a byte copy has no alignment requirement.
    // The validate_user_ptr check above establishes this; whether the range is
    // mapped is up to EL0 and is not checked.
    // Without the check an EL0 caller could make the kernel copy out kernel
    // memory; an unmapped user page takes an EL1 data abort and halts the CPU.
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
    // USER_VA_LIMIT, so it cannot overlap `src`, a kernel buffer in the TTBR1
    // half, and a byte copy has no alignment requirement.
    // The validate_user_ptr check above establishes this; whether the range is
    // mapped and writable is up to EL0 and is not checked.
    // Without the check an EL0 caller could make the kernel overwrite kernel
    // memory; an unmapped or read-only user page takes an EL1 data abort and
    // halts the CPU.
    unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), dst as *mut u8, src.len()) };
    Ok(())
}
