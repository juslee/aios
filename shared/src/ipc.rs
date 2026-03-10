//! IPC-related shared types and validation helpers.

/// Upper bound of the user virtual address space (exclusive).
///
/// AArch64 convention: addresses below 0x0000_8000_0000_0000 belong to user
/// space (TTBR0), addresses at or above belong to kernel space (TTBR1).
pub const USER_VA_LIMIT: usize = 0x0000_8000_0000_0000;

/// Validate that a (ptr, len) range lies entirely within user VA space.
///
/// Returns false if:
/// - `ptr + len` overflows
/// - `ptr` is in kernel space (>= USER_VA_LIMIT)
/// - `ptr + len` extends into kernel space (> USER_VA_LIMIT)
pub fn validate_user_va(ptr: usize, len: usize) -> bool {
    let end = match ptr.checked_add(len) {
        Some(e) => e,
        None => return false,
    };
    ptr < USER_VA_LIMIT && end <= USER_VA_LIMIT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_va_valid_range() {
        assert!(validate_user_va(0x400000, 4096));
        assert!(validate_user_va(0, 256));
        assert!(validate_user_va(USER_VA_LIMIT - 1, 1));
    }

    #[test]
    fn user_va_zero_len() {
        assert!(validate_user_va(0, 0));
        assert!(validate_user_va(0x1000, 0));
        assert!(validate_user_va(USER_VA_LIMIT - 1, 0));
    }

    #[test]
    fn user_va_at_boundary() {
        assert!(!validate_user_va(USER_VA_LIMIT, 0));
        assert!(!validate_user_va(USER_VA_LIMIT, 1));
    }

    #[test]
    fn user_va_crosses_boundary() {
        assert!(!validate_user_va(USER_VA_LIMIT - 1, 2));
        assert!(!validate_user_va(USER_VA_LIMIT - 100, 200));
    }

    #[test]
    fn user_va_kernel_pointer() {
        assert!(!validate_user_va(0xFFFF_0000_0000_0000, 1));
        assert!(!validate_user_va(0xFFFF_FFFF_FFFF_FFFF, 0));
    }

    #[test]
    fn user_va_overflow() {
        assert!(!validate_user_va(usize::MAX, 1));
        assert!(!validate_user_va(usize::MAX - 10, 100));
        assert!(!validate_user_va(1, usize::MAX));
    }

    #[test]
    fn user_va_large_valid() {
        assert!(validate_user_va(0, USER_VA_LIMIT));
        assert!(validate_user_va(0, USER_VA_LIMIT - 1));
    }
}
