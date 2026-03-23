//! Memory Kit — physical frame allocation, virtual address space management,
//! and memory pressure monitoring.
//!
//! Architecture reference: `docs/kits/kernel/memory.md`

use crate::{MemoryPressure, PhysAddr, Pool, VirtAddr};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by Memory Kit operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryError {
    /// No free frames available in the requested pool.
    OutOfMemory,
    /// Attempted to create a mapping that is both writable and executable.
    WxViolation,
    /// The specified region does not exist or is invalid.
    InvalidRegion,
    /// The caller lacks the required capability for this operation.
    CapabilityDenied,
    /// The virtual address range is already mapped.
    AlreadyMapped,
    /// The virtual address range is not currently mapped.
    NotMapped,
    /// The operation would exceed the memory budget for this pool or process.
    BudgetExceeded,
    /// The maximum number of mapped regions has been reached.
    TooManyRegions,
}

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// A physical page frame tagged with its owning pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysFrame {
    /// Physical address of the frame (4 KiB aligned).
    pub addr: PhysAddr,
    /// Pool from which this frame was allocated.
    pub pool: Pool,
}

/// Page permission flags with compile-time W^X enforcement.
///
/// Construct via [`PagePermissions::new`] which rejects any combination
/// where both `write` and `execute` are true.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PagePermissions {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
    pub user: bool,
}

impl PagePermissions {
    /// Create a new permission set, enforcing the W^X invariant.
    ///
    /// Returns `Err(MemoryError::WxViolation)` if both `write` and `execute`
    /// are true.
    pub fn new(read: bool, write: bool, execute: bool, user: bool) -> Result<Self, MemoryError> {
        if write && execute {
            return Err(MemoryError::WxViolation);
        }
        Ok(Self {
            read,
            write,
            execute,
            user,
        })
    }
}

/// Describes a virtual memory mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    /// Virtual address of the mapping start.
    pub vaddr: VirtAddr,
    /// Size in bytes (must be page-aligned).
    pub size: usize,
    /// Permission flags for this mapping.
    pub perms: PagePermissions,
    /// Pool that backs the physical frames.
    pub pool: Pool,
}

/// Statistics for a single memory pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolStats {
    /// Number of free 4 KiB frames in this pool.
    pub free_frames: usize,
    /// Total number of 4 KiB frames in this pool.
    pub total_frames: usize,
}

// ---------------------------------------------------------------------------
// Kit traits
// ---------------------------------------------------------------------------

/// Physical frame allocator interface.
///
/// Implementors manage a pool-partitioned buddy allocator and expose
/// allocation, deallocation, pressure queries, and per-pool statistics.
pub trait FrameAllocator {
    /// Allocate a single 4 KiB frame from the specified pool.
    fn alloc_frame(&self, pool: Pool) -> Result<PhysFrame, MemoryError>;

    /// Return a previously allocated frame to its pool.
    fn free_frame(&self, frame: PhysFrame) -> Result<(), MemoryError>;

    /// Query the current memory pressure level for a pool.
    fn pool_pressure(&self, pool: Pool) -> MemoryPressure;

    /// Return allocation statistics for a pool.
    fn pool_stats(&self, pool: Pool) -> PoolStats;
}

/// Virtual address space management interface.
///
/// Implementors manage page tables, mappings, and permission changes for
/// a single address space (kernel or per-process).
pub trait AddressSpace {
    /// Create a new virtual mapping backed by physical frames from `pool`.
    fn map(
        &mut self,
        vaddr: VirtAddr,
        size: usize,
        perms: PagePermissions,
        pool: Pool,
    ) -> Result<(), MemoryError>;

    /// Remove a virtual mapping starting at `vaddr`.
    fn unmap(&mut self, vaddr: VirtAddr, size: usize) -> Result<(), MemoryError>;

    /// Change the permissions on an existing mapping.
    fn protect(
        &mut self,
        vaddr: VirtAddr,
        size: usize,
        perms: PagePermissions,
    ) -> Result<(), MemoryError>;

    /// Query the mapping at a virtual address, if any.
    fn query(&self, vaddr: VirtAddr) -> Option<Mapping>;
}

/// System-wide memory pressure monitor.
///
/// Provides a single method to query the worst-case pressure across all
/// memory pools.
pub trait MemoryPressureMonitor {
    /// Return the highest (worst) pressure level across all pools.
    fn current_level(&self) -> MemoryPressure;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    // -- MemoryError --

    #[test]
    fn memory_error_debug_all_variants() {
        let variants: &[MemoryError] = &[
            MemoryError::OutOfMemory,
            MemoryError::WxViolation,
            MemoryError::InvalidRegion,
            MemoryError::CapabilityDenied,
            MemoryError::AlreadyMapped,
            MemoryError::NotMapped,
            MemoryError::BudgetExceeded,
            MemoryError::TooManyRegions,
        ];
        for v in variants {
            let s = format!("{:?}", v);
            assert!(!s.is_empty());
        }
        assert_eq!(variants.len(), 8);
    }

    #[test]
    fn memory_error_clone_and_eq() {
        let a = MemoryError::OutOfMemory;
        let b = a.clone();
        assert_eq!(a, b);
        assert_ne!(MemoryError::OutOfMemory, MemoryError::WxViolation);
    }

    // -- PagePermissions W^X --

    #[test]
    fn page_permissions_valid_cases() {
        // Read-only
        assert!(PagePermissions::new(true, false, false, false).is_ok());
        // Read-write
        assert!(PagePermissions::new(true, true, false, false).is_ok());
        // Read-execute
        assert!(PagePermissions::new(true, false, true, false).is_ok());
        // Read + user
        assert!(PagePermissions::new(true, false, false, true).is_ok());
        // Write-only (unusual but not W^X violating)
        assert!(PagePermissions::new(false, true, false, false).is_ok());
    }

    #[test]
    fn page_permissions_wx_rejected() {
        let result = PagePermissions::new(true, true, true, false);
        assert_eq!(result, Err(MemoryError::WxViolation));

        // Even without read
        let result = PagePermissions::new(false, true, true, false);
        assert_eq!(result, Err(MemoryError::WxViolation));
    }

    #[test]
    fn page_permissions_fields_accessible() {
        let p = PagePermissions::new(true, true, false, true).unwrap();
        assert!(p.read);
        assert!(p.write);
        assert!(!p.execute);
        assert!(p.user);
    }

    // -- PhysFrame --

    #[test]
    fn phys_frame_construction() {
        let f = PhysFrame {
            addr: 0x4000_0000,
            pool: Pool::Kernel,
        };
        assert_eq!(f.addr, 0x4000_0000);
        assert_eq!(f.pool, Pool::Kernel);
    }

    #[test]
    fn phys_frame_copy_semantics() {
        let a = PhysFrame {
            addr: 0x1000,
            pool: Pool::Dma,
        };
        let b = a;
        assert_eq!(a, b);
    }

    // -- Mapping --

    #[test]
    fn mapping_construction() {
        let perms = PagePermissions::new(true, true, false, true).unwrap();
        let m = Mapping {
            vaddr: 0x40_0000,
            size: 4096,
            perms,
            pool: Pool::User,
        };
        assert_eq!(m.vaddr, 0x40_0000);
        assert_eq!(m.size, 4096);
        assert_eq!(m.pool, Pool::User);
    }

    // -- PoolStats --

    #[test]
    fn pool_stats_construction() {
        let s = PoolStats {
            free_frames: 100,
            total_frames: 200,
        };
        assert_eq!(s.free_frames, 100);
        assert_eq!(s.total_frames, 200);
    }

    // -- Trait dyn-compatibility --

    // These functions assert that the traits are object-safe (dyn-compatible).
    // If they compile, the traits can be used as trait objects.
    fn _assert_frame_allocator_dyn(_: &dyn FrameAllocator) {}
    fn _assert_address_space_dyn(_: &dyn AddressSpace) {}
    fn _assert_pressure_monitor_dyn(_: &dyn MemoryPressureMonitor) {}

    #[test]
    fn traits_are_dyn_compatible() {
        // Compilation of the above functions is the real test.
        // This test just ensures the module compiles with those assertions.
    }
}
