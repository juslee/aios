//! Shared memory lifecycle — create, map, share, unmap — and private memory
//! (MemoryMap / MemoryUnmap).
//!
//! Provides zero-copy data transfer between processes via shared physical pages.
//! Regions are reference-counted and capability-gated. W^X is enforced at both
//! creation and mapping time.
//!
//! Per ipc.md §4.4–4.7, memory/virtual.md §7.
//!
//! Lock ordering: PROCESS_TABLE > SHARED_REGION_TABLE > CHANNEL_TABLE.
//!
//! Errno policy (ipc.md §3.2): EINVAL for a request that no caller could
//! make (an out-of-range id, WRITE | EXECUTE, flags beyond the region's
//! `max_flags`, a region size above 4 MiB, a MemoryMap above 64 pages, a
//! private address that is not the caller's allocation); ENOSPC only for a
//! full table; EPERM when the caller lacks a right the request needs (a
//! capability, a mapping of the region, being its creator); EPIPE when the
//! region is gone. One EPERM here is not a missing right: a
//! SharedMemoryShare target pid whose slot holds no process, which the
//! process accessors report as EPERM today (ipc.md §3.2).

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
///
/// Interim layout: memory/virtual.md §3.1 puts shared memory at
/// `0x1_0000_0000` and the agent heap here. The shared memory and private
/// windows move when processes get user address spaces (ipc.md §4.7).
const SHM_VA_BASE: usize = crate::mm::uspace::USER_HEAP_BASE;

/// Largest SharedMemoryCreate request, in pages: the largest buddy block
/// (`2^MAX_ORDER` pages, 4 MiB). A region is one buddy block, so a larger
/// size could never be allocated or fit its VA slot; it is EINVAL.
const MAX_SHARED_REGION_PAGES: usize = 1 << crate::mm::buddy::MAX_ORDER;

/// Spacing between shared memory region VA slots: the largest region
/// (`MAX_SHARED_REGION_PAGES`, 4 MiB). Every region fits its slot and the
/// shared memory window,
/// `[SHM_VA_BASE, SHM_VA_BASE + MAX_SHARED_REGIONS * SHM_VA_STRIDE)`, holds
/// them all without reaching the private window above it.
const SHM_VA_STRIDE: usize = MAX_SHARED_REGION_PAGES * PAGE_SIZE;

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
/// Returns the region ID on success. Errors: EINVAL for WRITE | EXECUTE or
/// a size above `MAX_SHARED_REGION_PAGES` pages (4 MiB); EPERM without
/// `SharedMemoryCreate`; ENOMEM when Pool::User has no block of the needed
/// order; ENOSPC when the region table is full.
pub fn shared_memory_create(
    pid: ProcessId,
    size: usize,
    flags: VmFlags,
) -> Result<SharedMemoryId, i64> {
    // W^X enforcement: reject WRITE+EXECUTE. No caller may ask for it, so
    // it is a malformed request (EINVAL), not a permission denial.
    if flags.contains(VmFlags::WRITE | VmFlags::EXECUTE) {
        crate::kwarn!(Mm, "shm_create: W^X violation (pid={})", pid.0);
        return Err(IpcError::Einval as i64);
    }

    // Round size up to page granularity. `size` comes straight from x0, so
    // bound it before the capability check and the allocation: a region
    // above MAX_SHARED_REGION_PAGES can never be created (EINVAL, like
    // W^X), and the bound keeps `order` at or below MAX_ORDER.
    let size_pages = size.div_ceil(PAGE_SIZE).max(1);
    if size_pages > MAX_SHARED_REGION_PAGES {
        return Err(IpcError::Einval as i64);
    }

    // Capability check (requires SharedMemoryCreate).
    let cap_token = crate::cap::check_shared_memory_create(pid)?;

    // Compute buddy order: smallest 2^order >= size_pages.
    let order = order_for_pages(size_pages);

    // Allocate from Pool::User.
    let base_phys = crate::mm::frame::alloc_user_pages(order).ok_or_else(|| {
        crate::kwarn!(
            Mm,
            "shm_create: OOM allocating {} pages (pid={})",
            1usize << order,
            pid.0
        );
        IpcError::Enomem as i64
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
        "shm_create: id={} size={:#x} order={} phys={:#x} pid={}",
        idx,
        size_pages * PAGE_SIZE,
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
///
/// Errors: EINVAL for an out-of-range id, WRITE | EXECUTE, or flags beyond
/// the region's `max_flags`; EPERM without `SharedMemoryAccess(id)`; EPIPE if
/// the region does not exist; EEXIST if `pid` already maps it; ENOSPC if the
/// region has `MAX_SHARED_MAPPINGS` mappings.
pub fn shared_memory_map(
    pid: ProcessId,
    region_id: SharedMemoryId,
    flags: VmFlags,
) -> Result<usize, i64> {
    if region_id.0 as usize >= MAX_SHARED_REGIONS {
        return Err(IpcError::Einval as i64);
    }

    // W^X enforcement (EINVAL, as in shared_memory_create).
    if flags.contains(VmFlags::WRITE | VmFlags::EXECUTE) {
        return Err(IpcError::Einval as i64);
    }

    // Capability check (SharedMemoryAccess).
    crate::cap::check_shared_memory_access(pid, region_id.0)?;

    let mut table = SHARED_REGION_TABLE.lock();
    let region = table[region_id.0 as usize]
        .as_mut()
        .ok_or(IpcError::Epipe as i64)?;

    // Verify flags are a subset of max_flags. The limit belongs to the
    // region, not to the caller, so asking for more is EINVAL for everyone.
    if !region.max_flags.contains(flags) {
        crate::kwarn!(
            Mm,
            "shm_map: flags not subset of max_flags (pid={}, region={})",
            pid.0,
            region_id.0
        );
        return Err(IpcError::Einval as i64);
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
///
/// Errors: EINVAL for an out-of-range id (or a ref_count already 0); EPIPE
/// if the region does not exist; EPERM if `pid` has no mapping of it, a
/// right the caller lacks, like a missing capability. No capability is
/// checked: holding the mapping is the authority to remove it.
pub fn shared_memory_unmap(pid: ProcessId, region_id: SharedMemoryId) -> Result<(), i64> {
    if region_id.0 as usize >= MAX_SHARED_REGIONS {
        return Err(IpcError::Einval as i64);
    }

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
///
/// Returns `Err(EINVAL)` for a region id `>= MAX_SHARED_REGIONS` or a
/// `target_pid >= MAX_PROCESSES`, before taking any lock; EPIPE if the
/// region does not exist; EPERM if `pid` is not the region's creator (a
/// right the caller lacks) or the target process does not exist. The
/// second EPERM is not a missing right: it is the empty-slot code of the
/// process accessors (`process_mut`) today (ipc.md §3.2). SharedMemoryShare
/// has no Kit wrapper, so a plain `IpcKitError::from_code` decode of either
/// EPERM reads as `CapabilityDenied`, which fits neither.
pub fn shared_memory_share(
    pid: ProcessId,
    region_id: SharedMemoryId,
    target_pid: ProcessId,
) -> Result<(), i64> {
    if region_id.0 as usize >= MAX_SHARED_REGIONS {
        return Err(IpcError::Einval as i64);
    }
    // target_pid comes straight from the SharedMemoryShare syscall. Reject an
    // out-of-range pid here, before SHARED_REGION_TABLE or PROCESS_TABLE is
    // locked; grant_to_process would also return EINVAL for it.
    if target_pid.index().is_none() {
        return Err(IpcError::Einval as i64);
    }

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
                // Guard against underflow (defense-in-depth).
                if region.ref_count.load(Ordering::Relaxed) == 0 {
                    continue;
                }
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
// Private memory (MemoryMap / MemoryUnmap)
// ---------------------------------------------------------------------------

/// Largest MemoryMap request, in pages (256 KiB). A larger size is EINVAL.
const MAX_PRIVATE_PAGES: usize = 64;

/// Maximum live MemoryMap allocations system-wide.
const MAX_PRIVATE_ALLOCATIONS: usize = 64;

/// Base of the private-memory VA window, directly after the shared memory
/// window (`SHM_VA_BASE` + 64 regions × 4 MiB). Slot `i` of
/// `PRIVATE_ALLOC_TABLE` owns `[PRIVATE_VA_BASE + i * PRIVATE_VA_STRIDE, +stride)`.
const PRIVATE_VA_BASE: usize = SHM_VA_BASE + MAX_SHARED_REGIONS * SHM_VA_STRIDE;

/// VA spacing between private allocation slots: room for the largest request.
const PRIVATE_VA_STRIDE: usize = MAX_PRIVATE_PAGES * PAGE_SIZE;

// Both windows end below the user stack.
const _: () = assert!(
    PRIVATE_VA_BASE + MAX_PRIVATE_ALLOCATIONS * PRIVATE_VA_STRIDE
        <= crate::mm::uspace::USER_STACK_BASE
);

/// One MemoryMap allocation: a physically contiguous buddy block from
/// Pool::User, owned by one process.
#[derive(Clone, Copy)]
struct PrivateAllocation {
    /// Process that called MemoryMap; only it may unmap the block.
    owner: ProcessId,
    /// Physical base of the `2^order`-page block.
    base_phys: usize,
    /// Buddy order the block was allocated with.
    order: usize,
    /// Pages the caller asked for (`<= 2^order`); MemoryUnmap must match it.
    pages: usize,
}

/// Every live MemoryMap allocation, indexed by VA slot. MemoryUnmap frees
/// only an entry that matches exactly and belongs to the caller, so it can
/// never return a page it did not hand out (a kernel page, a region's frames,
/// another process's block) to the buddy allocator.
///
/// Leaf lock: nothing else is taken while it is held. Blocks are allocated
/// before it is taken and freed after it is released.
static PRIVATE_ALLOC_TABLE: Mutex<[Option<PrivateAllocation>; MAX_PRIVATE_ALLOCATIONS]> =
    Mutex::new([None; MAX_PRIVATE_ALLOCATIONS]);

/// MemoryMap: allocate private memory for process `pid`.
///
/// Rounds `size` up to whole pages (at least one) and allocates them as one
/// physically contiguous buddy block from Pool::User, zeroes the block, and
/// records it for `pid` in a free slot of `PRIVATE_ALLOC_TABLE`. Returns the
/// slot's address in the private VA window, a user address that stands for
/// the whole allocation: it is the key MemoryUnmap takes, and because the
/// block is contiguous, `[va, va + pages * PAGE_SIZE)` maps onto it with one
/// range.
///
/// This follows `shared_memory_map`, which returns its region's window VA:
/// no process has a user address space yet (every process has
/// `address_space: None`), so nothing is mapped into TTBR0 here either. EL1
/// code would reach the block through the direct map (`DIRECT_MAP_BASE` +
/// `base_phys`); no EL1 code uses the memory MemoryMap allocates (the #188
/// boot self-test only maps and unmaps it). ipc.md §4.7.
///
/// Errors: EINVAL for W^X (WRITE | EXECUTE) and for a size above
/// `MAX_PRIVATE_PAGES` pages, which no caller can allocate; ENOSPC when
/// `PRIVATE_ALLOC_TABLE` is full; ENOMEM when Pool::User has no free block of
/// the needed order.
pub fn memory_map(pid: ProcessId, size: usize, flags: VmFlags) -> Result<usize, i64> {
    // W^X enforcement (EINVAL, as in shared_memory_create).
    if flags.contains(VmFlags::WRITE | VmFlags::EXECUTE) {
        return Err(IpcError::Einval as i64);
    }

    // The fixed per-allocation limit, like SharedMemoryCreate's 4 MiB bound:
    // EINVAL, so that ENOSPC keeps its one meaning, a full table.
    let pages = size.div_ceil(PAGE_SIZE).max(1);
    if pages > MAX_PRIVATE_PAGES {
        return Err(IpcError::Einval as i64);
    }
    let order = order_for_pages(pages);

    let base_phys = crate::mm::frame::alloc_user_pages(order).ok_or(IpcError::Enomem as i64)?;
    let dmap_va = crate::arch::aarch64::mmu::DIRECT_MAP_BASE + base_phys;

    // SAFETY: base_phys is a block of 2^order pages that alloc_user_pages has
    // just handed out, so nothing else references it, and the direct map
    // covers all RAM read-write, so [dmap_va, dmap_va + 2^order pages) is valid.
    // The buddy allocator hands each block out once; kmap.rs maintains the
    // direct map.
    // A write past the block would corrupt a neighbouring allocation; an
    // unmapped VA would take an EL1 data abort and halt the CPU.
    unsafe { core::ptr::write_bytes(dmap_va as *mut u8, 0, (1 << order) * PAGE_SIZE) };

    let slot_idx = {
        let mut table = PRIVATE_ALLOC_TABLE.lock();
        let idx = table.iter().position(|slot| slot.is_none());
        if let Some(i) = idx {
            table[i] = Some(PrivateAllocation {
                owner: pid,
                base_phys,
                order,
                pages,
            });
        }
        idx
    };
    let Some(slot_idx) = slot_idx else {
        // SAFETY: base_phys was allocated above with this order and was never
        // recorded or returned, so this is its only free.
        // This function owns the block until it is recorded.
        // A second free of the block would corrupt the buddy free lists.
        unsafe { crate::mm::frame::free_user_pages(base_phys, order) };
        crate::kwarn!(Mm, "memory_map: table full pid={}", pid.0);
        return Err(IpcError::Enospc as i64);
    };

    crate::kinfo!(Mm, "memory_map: pid={} pages={}", pid.0, pages);
    Ok(PRIVATE_VA_BASE + slot_idx * PRIVATE_VA_STRIDE)
}

/// Slot index of `va` in the private VA window, if `va` is exactly a slot's
/// base address.
fn private_slot(va: usize) -> Option<usize> {
    let offset = va.checked_sub(PRIVATE_VA_BASE)?;
    let idx = offset / PRIVATE_VA_STRIDE;
    (offset % PRIVATE_VA_STRIDE == 0 && idx < MAX_PRIVATE_ALLOCATIONS).then_some(idx)
}

/// MemoryUnmap: release memory that process `pid` mapped.
///
/// An address in the shared memory window unmaps that region through
/// `shared_memory_unmap`; `size` is not used there.
///
/// Any other address must be exactly an address MemoryMap returned to `pid`,
/// with a `size` that rounds up to the same page count; that allocation is
/// removed from `PRIVATE_ALLOC_TABLE` and its block freed. Anything else
/// returns EINVAL and frees nothing: another process's allocation, an address
/// inside or past an allocation, a different size, a direct-map or other
/// kernel address. Another process's allocation is EINVAL, not EPERM: a
/// private address names an allocation only for the process MemoryMap
/// returned it to, so for any other caller it is like any address that is not
/// one of its allocations. (It is not hidden: slots are system-wide, so
/// MemoryMap's own result shows which lower slots are in use.)
pub fn memory_unmap(pid: ProcessId, va: usize, size: usize) -> Result<(), i64> {
    // Check if this VA belongs to a shared region.
    if (SHM_VA_BASE..SHM_VA_BASE + MAX_SHARED_REGIONS * SHM_VA_STRIDE).contains(&va) {
        let region_idx = (va - SHM_VA_BASE) / SHM_VA_STRIDE;
        if region_idx < MAX_SHARED_REGIONS {
            return shared_memory_unmap(pid, SharedMemoryId(region_idx as u32));
        }
    }

    // Private unmap: the caller's exact allocation in that slot, or nothing.
    let pages = size.div_ceil(PAGE_SIZE).max(1);
    let alloc = private_slot(va).and_then(|idx| {
        let mut table = PRIVATE_ALLOC_TABLE.lock();
        let slot = &mut table[idx];
        if slot.is_some_and(|a| a.owner == pid && a.pages == pages) {
            slot.take()
        } else {
            None
        }
    });
    let Some(alloc) = alloc else {
        return Err(IpcError::Einval as i64);
    };

    // SAFETY: alloc was recorded by memory_map for a block that
    // alloc_user_pages returned at base_phys with this order, and it has just
    // been taken out of PRIVATE_ALLOC_TABLE under its lock, so this is the
    // block's only free.
    // memory_map and memory_unmap are the only code that inserts or removes
    // table entries.
    // Freeing a block twice, or one the allocator never handed out, would
    // corrupt the buddy free lists and hand out pages still in use.
    unsafe { crate::mm::frame::free_user_pages(alloc.base_phys, alloc.order) };

    crate::kinfo!(Mm, "memory_unmap: pid={} pages={}", pid.0, alloc.pages);

    Ok(())
}

// ---------------------------------------------------------------------------
// Read region via direct map (for kernel threads)
// ---------------------------------------------------------------------------

/// Get the direct-map virtual address of a shared region's backing memory.
///
/// For Phase 3 kernel threads, this is how processes access shared memory
/// (no user address space to map into).
pub fn region_dmap_addr(region_id: SharedMemoryId) -> Option<usize> {
    if region_id.0 as usize >= MAX_SHARED_REGIONS {
        return None;
    }
    let table = SHARED_REGION_TABLE.lock();
    table[region_id.0 as usize]
        .as_ref()
        .map(|r| crate::arch::aarch64::mmu::DIRECT_MAP_BASE + r.base_phys)
}

/// Get the size in bytes of a shared region.
#[allow(dead_code)]
pub fn region_size(region_id: SharedMemoryId) -> Option<usize> {
    if region_id.0 as usize >= MAX_SHARED_REGIONS {
        return None;
    }
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
