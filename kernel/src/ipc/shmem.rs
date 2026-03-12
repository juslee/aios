//! Shared memory lifecycle — create, map, share, unmap.
//!
//! Provides zero-copy data transfer between processes via shared physical pages.
//! Regions are reference-counted and capability-gated. W^X is enforced at both
//! creation and mapping time.
//!
//! Per ipc.md §4.4–4.6, memory.md §7.
//!
//! Lock ordering: PROCESS_TABLE > SHARED_REGION_TABLE > CHANNEL_TABLE.

use core::sync::atomic::{AtomicU32, Ordering};

use shared::{SharedMemoryId, MAX_SHARED_MAPPINGS, MAX_SHARED_REGIONS};
use spin::Mutex;

use crate::mm::pgtable::VmFlags;
use crate::observability::metrics::METRICS;
use crate::syscall::IpcError;
use crate::task::process::ProcessId;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const PAGE_SIZE: usize = 4096;

/// User heap base — shared memory regions are mapped starting here.
/// Each region gets a unique VA slot to avoid collisions.
const SHM_VA_BASE: usize = crate::mm::uspace::USER_HEAP_BASE;

/// Spacing between shared memory region VA slots (1 MiB).
const SHM_VA_STRIDE: usize = 0x0010_0000; // 1 MiB

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// A per-process mapping of a shared memory region.
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub struct SharedMapping {
    /// Process that holds this mapping.
    pub pid: ProcessId,
    /// Virtual address where the region is mapped.
    pub vaddr: usize,
    /// Permission flags (subset of max_flags).
    pub flags: VmFlags,
}

/// A shared memory region backed by physically contiguous pages.
#[allow(dead_code)]
pub struct SharedMemoryRegion {
    /// Region identifier (index into SHARED_REGION_TABLE).
    pub id: SharedMemoryId,
    /// Base physical address of the backing pages.
    pub base_phys: usize,
    /// Buddy order used for allocation (2^order pages).
    pub order: usize,
    /// Size in bytes (may be < 2^order * PAGE_SIZE if user requested non-power-of-2).
    pub size_bytes: usize,
    /// Number of active mappings (atomic for concurrent read).
    pub ref_count: AtomicU32,
    /// Process that created this region.
    pub creator: ProcessId,
    /// Maximum permission flags (W^X enforced at creation).
    pub max_flags: VmFlags,
    /// Capability token that authorized creation.
    pub creation_cap: Option<shared::CapabilityTokenId>,
    /// Active mappings (one per process that has mapped this region).
    pub mappings: [Option<SharedMapping>; MAX_SHARED_MAPPINGS],
}

// SAFETY: SharedMemoryRegion is only accessed under the SHARED_REGION_TABLE mutex.
// The AtomicU32 ref_count can be read outside the lock for diagnostics only.
unsafe impl Send for SharedMemoryRegion {}

// ---------------------------------------------------------------------------
// Global shared region table
// ---------------------------------------------------------------------------

pub static SHARED_REGION_TABLE: Mutex<[Option<SharedMemoryRegion>; MAX_SHARED_REGIONS]> = {
    const NONE: Option<SharedMemoryRegion> = None;
    Mutex::new([NONE; MAX_SHARED_REGIONS])
};

// ---------------------------------------------------------------------------
// Create
// ---------------------------------------------------------------------------

/// Create a new shared memory region.
///
/// Allocates physically contiguous pages from Pool::User, enforces W^X on
/// `flags`, and records the region in the global table.
///
/// Returns the region ID on success.
pub fn shared_memory_create(
    pid: ProcessId,
    size: usize,
    flags: VmFlags,
) -> Result<SharedMemoryId, i64> {
    // W^X enforcement: reject WRITE+EXECUTE.
    if flags.contains(VmFlags::WRITE | VmFlags::EXECUTE) {
        crate::kwarn!(Mm, "shm_create: W^X violation (pid={})", pid.0);
        return Err(IpcError::Eperm as i64);
    }

    // Capability check (requires SharedMemoryCreate).
    let cap_token = crate::cap::check_shared_memory_create(pid)?;

    // Round size up to page granularity.
    let size_pages = size.div_ceil(PAGE_SIZE).max(1);

    // Compute buddy order: smallest 2^order >= size_pages.
    let order = order_for_pages(size_pages);

    // Allocate from Pool::User.
    let base_phys = crate::mm::frame::alloc_user_pages(order).ok_or_else(|| {
        crate::kwarn!(
            Mm,
            "shm_create: OOM allocating {} pages (pid={})",
            1 << order,
            pid.0
        );
        IpcError::Enospc as i64
    })?;

    // Zero the region (defense in depth — prevent information leaks).
    let dmap_va = crate::arch::aarch64::mmu::DIRECT_MAP_BASE + base_phys;
    // SAFETY: base_phys is a freshly allocated region from Pool::User.
    // The direct map covers all RAM. We zero 2^order pages.
    unsafe {
        core::ptr::write_bytes(dmap_va as *mut u8, 0, (1 << order) * PAGE_SIZE);
    }

    // Find a free slot in the table.
    let mut table = SHARED_REGION_TABLE.lock();
    let idx = table.iter().position(|s| s.is_none()).ok_or_else(|| {
        // No free slots — free the pages and return error.
        // SAFETY: base_phys was just allocated with the given order.
        unsafe { crate::mm::frame::free_user_pages(base_phys, order) };
        crate::kwarn!(Mm, "shm_create: region table full (pid={})", pid.0);
        IpcError::Enospc as i64
    })?;

    let id = SharedMemoryId(idx as u32);
    table[idx] = Some(SharedMemoryRegion {
        id,
        base_phys,
        order,
        size_bytes: size_pages * PAGE_SIZE,
        ref_count: AtomicU32::new(0),
        creator: pid,
        max_flags: flags,
        creation_cap: Some(cap_token),
        mappings: [const { None }; MAX_SHARED_MAPPINGS],
    });

    // Release SHARED_REGION_TABLE before granting capability (lock ordering:
    // PROCESS_TABLE must not be acquired while SHARED_REGION_TABLE is held).
    drop(table);

    // Auto-grant SharedMemoryAccess to the creator so they can map their own region.
    if let Err(e) = crate::cap::grant_to_process(
        pid,
        shared::Capability::SharedMemoryAccess(idx as u32),
        true, // delegatable — creator can share with others
    ) {
        crate::kwarn!(
            Mm,
            "shm_create: failed to auto-grant SharedMemoryAccess to pid={} (err={})",
            pid.0,
            e
        );
    }

    #[cfg(feature = "kernel-metrics")]
    METRICS.shm_create.inc();

    crate::kinfo!(
        Mm,
        "shm_create: id={} size={:#x} pages={} order={} phys={:#x} pid={}",
        idx,
        size_pages * PAGE_SIZE,
        1 << order,
        order,
        base_phys,
        pid.0
    );

    Ok(id)
}

// ---------------------------------------------------------------------------
// Map
// ---------------------------------------------------------------------------

/// Map an existing shared memory region into a process's address space.
///
/// `flags` must be a subset of the region's `max_flags`. Returns the
/// virtual address where the region was mapped.
pub fn shared_memory_map(
    pid: ProcessId,
    region_id: SharedMemoryId,
    flags: VmFlags,
) -> Result<usize, i64> {
    // W^X enforcement.
    if flags.contains(VmFlags::WRITE | VmFlags::EXECUTE) {
        return Err(IpcError::Eperm as i64);
    }

    // Capability check (SharedMemoryAccess).
    crate::cap::check_shared_memory_access(pid, region_id.0)?;

    let mut table = SHARED_REGION_TABLE.lock();
    let region = table[region_id.0 as usize]
        .as_mut()
        .ok_or(IpcError::Epipe as i64)?;

    // Verify flags are a subset of max_flags.
    if !region.max_flags.contains(flags) {
        crate::kwarn!(
            Mm,
            "shm_map: flags not subset of max_flags (pid={}, region={})",
            pid.0,
            region_id.0
        );
        return Err(IpcError::Eperm as i64);
    }

    // Check for duplicate mapping.
    if region
        .mappings
        .iter()
        .any(|m| m.is_some_and(|m| m.pid == pid))
    {
        crate::kwarn!(
            Mm,
            "shm_map: already mapped (pid={}, region={})",
            pid.0,
            region_id.0
        );
        return Err(IpcError::Eexist as i64);
    }

    // Find a free mapping slot.
    let slot_idx = region
        .mappings
        .iter()
        .position(|m| m.is_none())
        .ok_or_else(|| {
            crate::kwarn!(Mm, "shm_map: max mappings reached (region={})", region_id.0);
            IpcError::Enospc as i64
        })?;

    // Compute VA for this mapping: base + region_id * stride.
    let va = SHM_VA_BASE + (region_id.0 as usize) * SHM_VA_STRIDE;
    let base_phys = region.base_phys;
    let size_bytes = region.size_bytes;
    let map_flags = flags | VmFlags::USER;

    region.mappings[slot_idx] = Some(SharedMapping {
        pid,
        vaddr: va,
        flags,
    });
    region.ref_count.fetch_add(1, Ordering::Relaxed);

    drop(table);

    // Phase 3: All processes are kernel-only (no user address space).
    // The mapping is tracked in the region table; kernel threads access
    // shared memory via the direct map (DIRECT_MAP_BASE + phys).
    // Full TTBR0 page table mapping is deferred to Phase 4+ when
    // user-space processes have real address spaces.
    let _ = (base_phys, size_bytes, map_flags);

    #[cfg(feature = "kernel-metrics")]
    METRICS.shm_map.inc();

    crate::kinfo!(
        Mm,
        "shm_map: region={} va={:#x} pages={} pid={}",
        region_id.0,
        va,
        size_bytes / PAGE_SIZE,
        pid.0
    );

    Ok(va)
}

// ---------------------------------------------------------------------------
// Unmap
// ---------------------------------------------------------------------------

/// Unmap a shared memory region from a process.
///
/// Decrements the reference count. If ref_count reaches 0, frees the
/// backing pages.
pub fn shared_memory_unmap(pid: ProcessId, region_id: SharedMemoryId) -> Result<(), i64> {
    let mut table = SHARED_REGION_TABLE.lock();
    let region = table[region_id.0 as usize]
        .as_mut()
        .ok_or(IpcError::Epipe as i64)?;

    // Find and remove the mapping for this pid.
    let mapping = region
        .mappings
        .iter_mut()
        .find(|m| m.is_some_and(|m| m.pid == pid));

    let mapping_info = match mapping {
        Some(slot) => {
            let info = slot.unwrap();
            *slot = None;
            info
        }
        None => {
            crate::kwarn!(
                Mm,
                "shm_unmap: not mapped (pid={}, region={})",
                pid.0,
                region_id.0
            );
            return Err(IpcError::Eperm as i64);
        }
    };

    // Guard against underflow: if ref_count is already 0, something is
    // seriously wrong (double unmap). Log and bail rather than wrapping.
    let current_ref = region.ref_count.load(Ordering::Relaxed);
    if current_ref == 0 {
        crate::kwarn!(
            Mm,
            "shm_unmap: ref_count already 0 (region={}), skipping",
            region_id.0
        );
        return Err(IpcError::Einval as i64);
    }
    let old_ref = region.ref_count.fetch_sub(1, Ordering::Relaxed);
    let base_phys = region.base_phys;
    let order = region.order;
    let size_bytes = region.size_bytes;

    // If last reference, free the region.
    let should_free = old_ref == 1;
    if should_free {
        table[region_id.0 as usize] = None;
    }

    drop(table);

    // Phase 3 (kernel-only threads): page table teardown is deferred.
    // The pages are still accessible via direct map until process exit.
    // Phase 4+ will unmap pages from the process's user page tables here.
    let _ = mapping_info;

    if should_free {
        // SAFETY: base_phys was allocated via alloc_user_pages with the given order.
        // ref_count was 1 (now 0), so no other process has the pages mapped.
        unsafe { crate::mm::frame::free_user_pages(base_phys, order) };
        crate::kinfo!(
            Mm,
            "shm_unmap: region={} freed ({} pages) last_ref by pid={}",
            region_id.0,
            size_bytes / PAGE_SIZE,
            pid.0
        );
    } else {
        crate::kinfo!(
            Mm,
            "shm_unmap: region={} unmapped for pid={} (refs={})",
            region_id.0,
            pid.0,
            old_ref - 1
        );
    }

    #[cfg(feature = "kernel-metrics")]
    METRICS.shm_unmap.inc();

    Ok(())
}

// ---------------------------------------------------------------------------
// Share (grant access to another process via capability)
// ---------------------------------------------------------------------------

/// Share a shared memory region with another process by granting a
/// SharedMemoryAccess capability.
///
/// The caller must own the region (be the creator). The recipient receives
/// a capability that lets them call `shared_memory_map`.
pub fn shared_memory_share(
    pid: ProcessId,
    region_id: SharedMemoryId,
    target_pid: ProcessId,
) -> Result<(), i64> {
    // Verify the caller owns the region.
    {
        let table = SHARED_REGION_TABLE.lock();
        let region = table[region_id.0 as usize]
            .as_ref()
            .ok_or(IpcError::Epipe as i64)?;

        if region.creator != pid {
            crate::kwarn!(
                Mm,
                "shm_share: not creator (pid={}, region={}, creator={})",
                pid.0,
                region_id.0,
                region.creator.0
            );
            return Err(IpcError::Eperm as i64);
        }
    }
    // SHARED_REGION_TABLE lock released before acquiring PROCESS_TABLE (lock ordering).

    // Grant SharedMemoryAccess capability to the target process.
    crate::cap::grant_to_process(
        target_pid,
        shared::Capability::SharedMemoryAccess(region_id.0),
        false,
    )?;

    crate::kinfo!(
        Mm,
        "shm_share: region={} shared from pid={} to pid={}",
        region_id.0,
        pid.0,
        target_pid.0
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Process cleanup
// ---------------------------------------------------------------------------

/// Clean up all shared memory mappings for a process (called on process exit).
#[allow(dead_code)]
pub fn process_cleanup_shared_memory(pid: ProcessId) {
    let mut table = SHARED_REGION_TABLE.lock();

    // Collect regions to potentially free (to avoid double-borrow issues).
    let mut to_free: [(usize, usize); MAX_SHARED_REGIONS] = [(0, 0); MAX_SHARED_REGIONS];
    let mut free_count = 0;

    for slot in table.iter_mut() {
        if let Some(region) = slot {
            // Remove any mapping for this pid.
            let had_mapping = region.mappings.iter_mut().any(|m| {
                if m.is_some_and(|m| m.pid == pid) {
                    *m = None;
                    true
                } else {
                    false
                }
            });

            if had_mapping {
                let old_ref = region.ref_count.fetch_sub(1, Ordering::Relaxed);
                if old_ref == 1 {
                    // Last reference — collect for freeing.
                    to_free[free_count] = (region.base_phys, region.order);
                    free_count += 1;
                    *slot = None;
                }
            }
        }
    }

    drop(table);

    // Free collected pages outside the lock.
    for &(phys, order) in &to_free[..free_count] {
        // SAFETY: phys was allocated via alloc_user_pages with the given order.
        unsafe { crate::mm::frame::free_user_pages(phys, order) };
    }

    if free_count > 0 {
        crate::kinfo!(
            Mm,
            "shm_cleanup: pid={} freed {} regions",
            pid.0,
            free_count
        );
    }
}

// ---------------------------------------------------------------------------
// Memory map (private allocation for user heap)
// ---------------------------------------------------------------------------

/// MemoryMap: allocate private pages for a process (user heap growth).
///
/// Allocates from Pool::User, maps into the caller's address space at the
/// next available VA in the USER_HEAP_BASE region.
pub fn memory_map(pid: ProcessId, size: usize, flags: VmFlags) -> Result<usize, i64> {
    // W^X enforcement.
    if flags.contains(VmFlags::WRITE | VmFlags::EXECUTE) {
        return Err(IpcError::Eperm as i64);
    }

    let size_pages = size.div_ceil(PAGE_SIZE).max(1);

    // For Phase 3 kernel threads, we allocate pages and return the direct-map VA.
    // Full user-space VA management comes in Phase 4.
    let mut allocated: [usize; 64] = [0; 64];
    if size_pages > 64 {
        return Err(IpcError::Enospc as i64);
    }

    for i in 0..size_pages {
        match crate::mm::frame::alloc_user_page() {
            Some(pa) => {
                // Zero the page.
                let dmap_va = crate::arch::aarch64::mmu::DIRECT_MAP_BASE + pa;
                // SAFETY: pa is a freshly allocated page, direct map covers all RAM.
                unsafe {
                    core::ptr::write_bytes(dmap_va as *mut u8, 0, PAGE_SIZE);
                }
                allocated[i] = pa;
            }
            None => {
                // OOM — free what we allocated so far.
                for &pa in &allocated[..i] {
                    // SAFETY: pa was allocated by alloc_user_page above.
                    unsafe { crate::mm::frame::free_user_page(pa) };
                }
                return Err(IpcError::Enospc as i64);
            }
        }
    }

    // For kernel threads: return base PA accessible via direct map.
    let va = crate::arch::aarch64::mmu::DIRECT_MAP_BASE + allocated[0];

    crate::kinfo!(
        Mm,
        "memory_map: {} pages at va={:#x} pid={}",
        size_pages,
        va,
        pid.0
    );

    Ok(va)
}

/// MemoryUnmap: free private pages.
///
/// For Phase 3: accepts a direct-map VA, converts to physical, frees.
pub fn memory_unmap(pid: ProcessId, va: usize, size: usize) -> Result<(), i64> {
    let size_pages = size.div_ceil(PAGE_SIZE).max(1);

    // Check if this VA belongs to a shared region.
    if (SHM_VA_BASE..SHM_VA_BASE + MAX_SHARED_REGIONS * SHM_VA_STRIDE).contains(&va) {
        let region_idx = (va - SHM_VA_BASE) / SHM_VA_STRIDE;
        if region_idx < MAX_SHARED_REGIONS {
            return shared_memory_unmap(pid, SharedMemoryId(region_idx as u32));
        }
    }

    // Private unmap — convert direct-map VA to physical.
    let dmap_base = crate::arch::aarch64::mmu::DIRECT_MAP_BASE;
    if va < dmap_base {
        return Err(IpcError::Eperm as i64);
    }
    let base_pa = va - dmap_base;

    for i in 0..size_pages {
        let pa = base_pa + i * PAGE_SIZE;
        // SAFETY: pa was allocated by memory_map via alloc_user_page.
        unsafe { crate::mm::frame::free_user_page(pa) };
    }

    crate::kinfo!(
        Mm,
        "memory_unmap: {} pages at va={:#x} pid={}",
        size_pages,
        va,
        pid.0
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Read region via direct map (for kernel threads)
// ---------------------------------------------------------------------------

/// Get the direct-map virtual address of a shared region's backing memory.
///
/// For Phase 3 kernel threads, this is how processes access shared memory
/// (no user address space to map into).
#[allow(dead_code)]
pub fn region_dmap_addr(region_id: SharedMemoryId) -> Option<usize> {
    let table = SHARED_REGION_TABLE.lock();
    table[region_id.0 as usize]
        .as_ref()
        .map(|r| crate::arch::aarch64::mmu::DIRECT_MAP_BASE + r.base_phys)
}

/// Get the size in bytes of a shared region.
#[allow(dead_code)]
pub fn region_size(region_id: SharedMemoryId) -> Option<usize> {
    let table = SHARED_REGION_TABLE.lock();
    table[region_id.0 as usize].as_ref().map(|r| r.size_bytes)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute the smallest buddy order such that `2^order >= pages`.
/// Delegates to the shared crate's portable implementation.
fn order_for_pages(pages: usize) -> usize {
    shared::order_for_pages(pages)
}
