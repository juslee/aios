//! Capability Kit — capability enforcement, token lifecycle, and attenuation.
//!
//! Architecture reference: `docs/kits/kernel/capability.md`

extern crate alloc;

use alloc::vec::Vec;

use crate::cap::{Capability, CapabilityHandle, CapabilityToken, CapabilityTokenId};
use crate::sched::ProcessId;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by Capability Kit operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityError {
    /// The holder does not possess a token granting the requested capability.
    NotGranted { requested: Capability },
    /// The token has been revoked (cascade or explicit).
    Revoked { token_id: CapabilityTokenId },
    /// The token has expired (past its `expires_at_tick`).
    Expired { token_id: CapabilityTokenId },
    /// The per-process capability table is full (256 slots).
    TableFull,
    /// The attenuation request is invalid (not a narrowing of the parent).
    InvalidAttenuation { reason: &'static str },
    /// The handle does not reference a valid token slot.
    InvalidHandle { handle: CapabilityHandle },
    /// The parent token does not allow delegation.
    NotDelegatable { token_id: CapabilityTokenId },
}

// ---------------------------------------------------------------------------
// Error conversions (i64 round-trip for syscall ABI)
// ---------------------------------------------------------------------------

// Negative error codes for syscall returns.
const ERR_NOT_GRANTED: i64 = -4001;
const ERR_REVOKED: i64 = -4002;
const ERR_EXPIRED: i64 = -4003;
const ERR_TABLE_FULL: i64 = -4004;
const ERR_INVALID_ATTENUATION: i64 = -4005;
const ERR_INVALID_HANDLE: i64 = -4006;
const ERR_NOT_DELEGATABLE: i64 = -4007;

impl From<CapabilityError> for i64 {
    fn from(e: CapabilityError) -> i64 {
        match e {
            CapabilityError::NotGranted { .. } => ERR_NOT_GRANTED,
            CapabilityError::Revoked { .. } => ERR_REVOKED,
            CapabilityError::Expired { .. } => ERR_EXPIRED,
            CapabilityError::TableFull => ERR_TABLE_FULL,
            CapabilityError::InvalidAttenuation { .. } => ERR_INVALID_ATTENUATION,
            CapabilityError::InvalidHandle { .. } => ERR_INVALID_HANDLE,
            CapabilityError::NotDelegatable { .. } => ERR_NOT_DELEGATABLE,
        }
    }
}

impl TryFrom<i64> for CapabilityError {
    type Error = i64;

    fn try_from(code: i64) -> Result<Self, i64> {
        match code {
            ERR_NOT_GRANTED => Ok(CapabilityError::NotGranted {
                requested: Capability::DebugPrint,
            }),
            ERR_REVOKED => Ok(CapabilityError::Revoked {
                token_id: CapabilityTokenId(0),
            }),
            ERR_EXPIRED => Ok(CapabilityError::Expired {
                token_id: CapabilityTokenId(0),
            }),
            ERR_TABLE_FULL => Ok(CapabilityError::TableFull),
            ERR_INVALID_ATTENUATION => {
                Ok(CapabilityError::InvalidAttenuation { reason: "unknown" })
            }
            ERR_INVALID_HANDLE => Ok(CapabilityError::InvalidHandle {
                handle: CapabilityHandle(0),
            }),
            ERR_NOT_DELEGATABLE => Ok(CapabilityError::NotDelegatable {
                token_id: CapabilityTokenId(0),
            }),
            other => Err(other),
        }
    }
}

// ---------------------------------------------------------------------------
// Kit trait
// ---------------------------------------------------------------------------

/// Capability enforcement interface.
///
/// Implementors wrap the kernel's process table and capability tables,
/// providing a unified API for checking, granting, revoking, attenuating,
/// and listing capabilities.
pub trait CapabilityEnforcer {
    /// Check whether `holder` possesses a token granting `action`.
    ///
    /// Returns the handle of the authorizing token on success.
    fn check(
        &self,
        holder: ProcessId,
        action: &Capability,
    ) -> Result<CapabilityHandle, CapabilityError>;

    /// Grant a new capability token to `holder`.
    fn grant(
        &mut self,
        holder: ProcessId,
        cap: Capability,
        granted_by: ProcessId,
    ) -> Result<CapabilityHandle, CapabilityError>;

    /// Revoke a capability token held by `holder`.
    fn revoke(
        &mut self,
        holder: ProcessId,
        handle: CapabilityHandle,
    ) -> Result<(), CapabilityError>;

    /// Create a narrowed (attenuated) child token from an existing token.
    fn attenuate(
        &mut self,
        holder: ProcessId,
        handle: CapabilityHandle,
        narrowed: Capability,
    ) -> Result<CapabilityHandle, CapabilityError>;

    /// List all active (non-revoked, non-expired) capability tokens for a process.
    ///
    /// Returns an owned Vec because the underlying data is mutex-guarded and
    /// cannot be borrowed across the lock boundary.
    fn list_active(&self, holder: ProcessId) -> Vec<CapabilityToken>;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    // -- CapabilityError variants --

    #[test]
    fn capability_error_debug_all_variants() {
        let variants: &[CapabilityError] = &[
            CapabilityError::NotGranted {
                requested: Capability::ChannelCreate,
            },
            CapabilityError::Revoked {
                token_id: CapabilityTokenId(1),
            },
            CapabilityError::Expired {
                token_id: CapabilityTokenId(2),
            },
            CapabilityError::TableFull,
            CapabilityError::InvalidAttenuation {
                reason: "test reason",
            },
            CapabilityError::InvalidHandle {
                handle: CapabilityHandle(99),
            },
            CapabilityError::NotDelegatable {
                token_id: CapabilityTokenId(3),
            },
        ];
        for v in variants {
            let s = format!("{:?}", v);
            assert!(!s.is_empty());
        }
        assert_eq!(variants.len(), 7);
    }

    #[test]
    fn capability_error_clone_and_eq() {
        let a = CapabilityError::TableFull;
        let b = a.clone();
        assert_eq!(a, b);
        assert_ne!(
            CapabilityError::TableFull,
            CapabilityError::InvalidAttenuation { reason: "x" }
        );
    }

    // -- i64 round-trip --

    #[test]
    fn capability_error_to_i64_all_variants() {
        assert_eq!(
            i64::from(CapabilityError::NotGranted {
                requested: Capability::SpawnAgent
            }),
            ERR_NOT_GRANTED
        );
        assert_eq!(
            i64::from(CapabilityError::Revoked {
                token_id: CapabilityTokenId(1)
            }),
            ERR_REVOKED
        );
        assert_eq!(
            i64::from(CapabilityError::Expired {
                token_id: CapabilityTokenId(1)
            }),
            ERR_EXPIRED
        );
        assert_eq!(i64::from(CapabilityError::TableFull), ERR_TABLE_FULL);
        assert_eq!(
            i64::from(CapabilityError::InvalidAttenuation { reason: "x" }),
            ERR_INVALID_ATTENUATION
        );
        assert_eq!(
            i64::from(CapabilityError::InvalidHandle {
                handle: CapabilityHandle(0)
            }),
            ERR_INVALID_HANDLE
        );
        assert_eq!(
            i64::from(CapabilityError::NotDelegatable {
                token_id: CapabilityTokenId(0)
            }),
            ERR_NOT_DELEGATABLE
        );
    }

    #[test]
    fn capability_error_i64_round_trip() {
        let codes = [
            ERR_NOT_GRANTED,
            ERR_REVOKED,
            ERR_EXPIRED,
            ERR_TABLE_FULL,
            ERR_INVALID_ATTENUATION,
            ERR_INVALID_HANDLE,
            ERR_NOT_DELEGATABLE,
        ];
        for code in codes {
            let err = CapabilityError::try_from(code).expect("should parse");
            let back: i64 = err.into();
            assert_eq!(back, code, "round-trip failed for code {code}");
        }
    }

    #[test]
    fn capability_error_try_from_unknown_code() {
        assert_eq!(CapabilityError::try_from(0_i64), Err(0));
        assert_eq!(CapabilityError::try_from(-9999_i64), Err(-9999));
    }

    // -- Trait dyn-compatibility --

    fn _assert_capability_enforcer_dyn(_: &dyn CapabilityEnforcer) {}

    #[test]
    fn capability_enforcer_is_dyn_compatible() {
        // Compilation of the assertion function is the real test.
    }
}
