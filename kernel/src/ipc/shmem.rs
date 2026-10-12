//! Shared memory lifecycle — create, map, share, unmap — and private memory
//! (MemoryMap / MemoryUnmap).
//!
//! Provides zero-copy data transfer between processes via shared physical pages.
//! Regions are reference-counted and capability-gated. W^X is enforced at both
//! creation and mapping time.
//!
//! A region leaves its slot only through `region_detach`: at its last unmap,
//! at process-exit cleanup, or in the cascade when its creator revokes the
//! `SharedMemoryCreate` token it was created under (capability-lifetime ADR
//! §5). Kernel code reads region memory through the direct map only between
//! `region_borrow` and `region_release`; a region detached while borrowed
//! keeps its frames draining, and its slot out of use, until the last
//! release. Frames are always freed with no table lock held.
//!
//! Per ipc.md §4.4–4.7, memory/virtual.md §7.
//!
//! Lock ordering: PROCESS_TABLE > SHARED_REGION_TABLE > CHANNEL_TABLE. The
//! borrow API, the last unmap and exit cleanup take SHARED_REGION_TABLE
//! alone; the cascade nests it under the revoking PROCESS_TABLE hold.
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
    /// Region identifier: its SHARED_REGION_TABLE slot and generation.
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
    /// Token in `creator`'s capability table that authorised the create: the
    /// one `cap::create_with_access` found under its `PROCESS_TABLE` hold,
    /// which is also the parent of the creator's minted SharedMemoryAccess.
    pub creation_cap: shared::CapabilityTokenId,
    /// Active mappings (one per process that has mapped this region).
    pub mappings: [Option<SharedMapping>; MAX_SHARED_MAPPINGS],
    /// Live kernel borrows (`region_borrow` calls not yet matched by a
    /// `region_release`). While it is above 0, a detach leaves the frames
    /// draining instead of freeing them, because a kernel thread may still
    /// read them through the direct map.
    kernel_borrows: u32,
}

// SAFETY: SharedMemoryRegion is only accessed under the SHARED_REGION_TABLE mutex.
// The AtomicU32 ref_count can be read outside the lock for diagnostics only.
unsafe impl Send for SharedMemoryRegion {}

// ---------------------------------------------------------------------------
// Global shared region table
// ---------------------------------------------------------------------------

/// One `SHARED_REGION_TABLE` slot: its region, if any; the frames of a
/// detached region that kernel borrows still hold, if any; and the generation
/// that the slot's region carries, or that its next region will carry.
///
/// A region in the slot always has the id `SharedMemoryId::new(slot,
/// generation)`. `region_detach` is the only way a region leaves its slot: it
/// advances the generation in the same `SHARED_REGION_TABLE` hold, so an id
/// issued before the detach never matches the slot again. A slot freed at
/// `ID_GEN_MAX` is retired (`ID_GEN_RETIRED`) and never allocated again. A
/// draining slot is not allocated until its last `region_release`, so a
/// release never meets a newer region. All fields are private to this
/// module; other code reaches a region only through `region_mut`, which
/// treats a draining slot as empty.
pub struct RegionSlot {
    generation: u32,
    region: Option<SharedMemoryRegion>,
    draining: Option<DrainingFrames>,
}

/// The frames of a detached region that kernel borrows still hold. The last
/// `region_release` frees them and empties the record.
struct DrainingFrames {
    /// Id of the detached region: only its borrows are released here.
    id: SharedMemoryId,
    /// Base physical address of the backing pages.
    base_phys: usize,
    /// Buddy order the pages were allocated with.
    order: usize,
    /// Borrows not yet released; above 0 while the record exists.
    borrows: u32,
}

impl RegionSlot {
    /// A never-used slot: empty, at the first generation.
    const EMPTY: Self = Self {
        generation: shared::ID_GEN_FIRST,
        region: None,
        draining: None,
    };

    /// Whether `shared_memory_create` may put a new region here: empty, not
    /// draining and not retired.
    fn is_free(&self) -> bool {
        self.region.is_none()
            && self.draining.is_none()
            && self.generation != shared::ID_GEN_RETIRED
    }
}

/// What `region_detach` did with a region's frames, for its caller to finish
/// with no table lock held (`Detached::finish`). Neither `Clone` nor `Copy`:
/// each detach yields one value, and finishing consumes it, so a block is
/// freed once.
enum Detached {
    /// No kernel borrow: the frames go back to Pool::User.
    Free {
        base_phys: usize,
        /// Buddy order, at most `MAX_ORDER` (10).
        order: u32,
    },
    /// Kernel borrows remain: the slot drains, and the last `region_release`
    /// frees the frames.
    Draining { id: SharedMemoryId, borrows: u32 },
}

// 16 bytes, so a FreeList of MAX_SHARED_REGIONS entries is 1 KiB plus its
// count.
const _: () = assert!(core::mem::size_of::<Option<Detached>>() == 16);

/// Detach the region `id` names from `slot`, which a locked
/// `SHARED_REGION_TABLE` holds: the only way a region leaves its slot.
///
/// Drops the region together with its `mappings`, which force-unmaps every
/// mapper at once (no mapping writes a page table yet: `shared_memory_map`
/// only records it), and advances the slot's generation (retiring it at
/// `ID_GEN_MAX`). With no kernel borrow the caller gets the frames to free;
/// otherwise the slot keeps them as `DrainingFrames` until the last
/// `region_release`. Returns `None`, changing nothing, when the slot is
/// empty, draining or holds another generation's region.
///
/// Frees, logs and wakes nothing: the caller acts on the result after it has
/// released every table lock.
fn region_detach(slot: &mut RegionSlot, id: SharedMemoryId) -> Option<Detached> {
    if slot.region.as_ref()?.id != id {
        return None;
    }
    let region = slot.region.take()?;
    slot.generation = shared::next_generation(slot.generation);
    if region.kernel_borrows == 0 {
        return Some(Detached::Free {
            base_phys: region.base_phys,
            // shared_memory_create bounds a region's order by MAX_ORDER (10),
            // so the cast keeps every bit.
            order: region.order as u32,
        });
    }
    slot.draining = Some(DrainingFrames {
        id,
        base_phys: region.base_phys,
        order: region.order,
        borrows: region.kernel_borrows,
    });
    Some(Detached::Draining {
        id,
        borrows: region.kernel_borrows,
    })
}

/// The regions one `SHARED_REGION_TABLE` pass detached: `MAX_SHARED_REGIONS`
/// entries plus a count, 1 KiB. A pass detaches at most one region per slot,
/// so the list never fills. Its owner finishes it (`finish`) after releasing
/// every table lock, so a region free never nests `FRAME_ALLOC` under a table.
pub(crate) struct FreeList {
    entries: [Option<Detached>; MAX_SHARED_REGIONS],
    count: usize,
}

impl FreeList {
    /// An empty list.
    pub(crate) const fn new() -> Self {
        Self {
            entries: [const { None }; MAX_SHARED_REGIONS],
            count: 0,
        }
    }

    /// Record one detach. At most one detach per slot per pass, and the list
    /// has one entry per slot, so `count` stays in bounds.
    fn push(&mut self, detached: Detached) {
        self.entries[self.count] = Some(detached);
        self.count += 1;
    }

    /// With no lock held: finish every detach (`Detached::finish`). Returns
    /// how many regions the pass detached, freed and draining.
    fn finish(self) -> usize {
        for detached in self.entries.into_iter().take(self.count).flatten() {
            detached.finish();
        }
        self.count
    }
}

impl Detached {
    /// With no lock held: free the frames of a region that had no kernel
    /// borrow, or log a region left draining.
    fn finish(self) {
        match self {
            Detached::Free { base_phys, order } => {
                // SAFETY: region_detach took this region out of its slot
                // under SHARED_REGION_TABLE with no kernel borrow, so no
                // table entry, mapping or borrow refers to its block, and
                // this value, which finish consumes, is the block's only
                // owner (each detach produces one, and Detached is not Copy).
                // shared_memory_create allocated the block with
                // alloc_user_pages at this order, and region_detach is the
                // only way a region leaves its slot.
                // A second free would corrupt the buddy free lists and hand
                // out pages still in use.
                unsafe { crate::mm::frame::free_user_pages(base_phys, order as usize) };
            }
            Detached::Draining { id, borrows } => {
                crate::kinfo!(Mm, "shm: region {} draining ({} borrows)", id, borrows);
            }
        }
    }
}

/// Backing array of `SHARED_REGION_TABLE`, indexed by
/// `SharedMemoryId::index()`.
pub type RegionTable = [RegionSlot; MAX_SHARED_REGIONS];

pub static SHARED_REGION_TABLE: Mutex<RegionTable> =
    Mutex::new([RegionSlot::EMPTY; MAX_SHARED_REGIONS]);

/// Look up the live region `id` names in a locked `SHARED_REGION_TABLE`.
///
/// Returns `Err(EINVAL)` when `id`'s slot is `>= MAX_SHARED_REGIONS`
/// (malformed), and `Err(EPIPE)` when the slot is empty or holds a region
/// with another id (the full `u32`, generation included): the region was
/// freed or never created, and a stale id never reaches the region now in
/// its slot. Every lookup of a region by id goes through this function.
pub(crate) fn region_mut(
    table: &mut RegionTable,
    id: SharedMemoryId,
) -> Result<&mut SharedMemoryRegion, i64> {
    // `index()` only returns ids below MAX_SHARED_REGIONS, the length of the
    // table array, so this indexing cannot panic.
    let idx = id.index().ok_or(IpcError::Einval as i64)?;
    match &mut table[idx].region {
        Some(region) if region.id == id => Ok(region),
        _ => Err(IpcError::Epipe as i64),
    }
}

// ---------------------------------------------------------------------------
// Create
// ---------------------------------------------------------------------------

/// Create a new shared memory region.
///
/// Allocates physically contiguous pages from Pool::User, enforces W^X on
/// `flags`, and records the region in the global table.
///
/// Returns the region ID and the handle of the creator's
/// `SharedMemoryAccess(id)` token, minted as a child of the authorising
/// `SharedMemoryCreate` token in the same `PROCESS_TABLE` hold that publishes
/// the region (`cap::create_with_access`); the region records that token as
/// its `creation_cap`. Errors: EINVAL for WRITE | EXECUTE or a size above
/// `MAX_SHARED_REGION_PAGES` pages (4 MiB); EPERM without
/// `SharedMemoryCreate`; ENOMEM when Pool::User has no block of the needed
/// order; ENOSPC when the region table or the creator's capability table is
/// full.
pub fn shared_memory_create(
    pid: ProcessId,
    size: usize,
    flags: VmFlags,
) -> Result<(SharedMemoryId, shared::CapabilityHandle), i64> {
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

    // Cheap pre-check (requires SharedMemoryCreate) before pages are
    // allocated. Its token id is discarded: the region records the token
    // create_with_access finds under its hold, so a revoke in between cannot
    // leave a region that outlives its token.
    crate::cap::check_shared_memory_create(pid)?;

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

    // Publish the region and mint the creator's access in one PROCESS_TABLE
    // hold, with SHARED_REGION_TABLE nested (PROCESS_TABLE >
    // SHARED_REGION_TABLE). The publish frees nothing: on any error the
    // pages are freed once, here, after both locks are released.
    let size_bytes = size_pages * PAGE_SIZE;
    let published =
        crate::cap::create_with_access(pid, shared::Capability::SharedMemoryCreate, |auth| {
            let mut table = SHARED_REGION_TABLE.lock();
            // The lowest slot that is empty and not retired.
            let idx = table
                .iter()
                .position(RegionSlot::is_free)
                .ok_or(IpcError::Enospc as i64)?;
            let slot = &mut table[idx];
            // A free slot is not retired, so its generation is one `new` accepts.
            let id = SharedMemoryId::new(idx, slot.generation);
            slot.region = Some(SharedMemoryRegion {
                id,
                base_phys,
                order,
                size_bytes,
                ref_count: AtomicU32::new(0),
                creator: pid,
                max_flags: flags,
                creation_cap: auth,
                mappings: [const { None }; MAX_SHARED_MAPPINGS],
                kernel_borrows: 0,
            });
            Ok((id, shared::Capability::SharedMemoryAccess(id)))
        });
    let (id, handle) = match published {
        Ok(created) => created,
        Err(e) => {
            // SAFETY: base_phys was allocated above with this order and was
            // never published (create_with_access returned an error before or
            // in the publish, which stores it only on success), so this is its
            // only free.
            // This function owns the block until a publish stores it.
            // A second free would corrupt the buddy free lists and hand out
            // pages still in use.
            unsafe { crate::mm::frame::free_user_pages(base_phys, order) };
            if e == IpcError::Enospc as i64 {
                crate::kwarn!(Mm, "shm_create: region or cap table full (pid={})", pid.0);
            }
            return Err(e);
        }
    };

    #[cfg(feature = "kernel-metrics")]
    METRICS.shm_create.inc();

    crate::kinfo!(
        Mm,
        "shm_create: id={} size={:#x} order={} phys={:#x} pid={}",
        id,
        size_bytes,
        order,
        base_phys,
        pid.0
    );

    Ok((id, handle))
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
/// the region does not exist or `id` is stale (another generation); EEXIST
/// if `pid` already maps it; ENOSPC if the region has `MAX_SHARED_MAPPINGS`
/// mappings.
pub fn shared_memory_map(
    pid: ProcessId,
    region_id: SharedMemoryId,
    flags: VmFlags,
) -> Result<usize, i64> {
    let Some(idx) = region_id.index() else {
        return Err(IpcError::Einval as i64);
    };

    // W^X enforcement (EINVAL, as in shared_memory_create).
    if flags.contains(VmFlags::WRITE | VmFlags::EXECUTE) {
        return Err(IpcError::Einval as i64);
    }

    // Capability check (SharedMemoryAccess).
    crate::cap::check_shared_memory_access(pid, region_id)?;

    let mut table = SHARED_REGION_TABLE.lock();
    let region = region_mut(&mut table, region_id)?;

    // Verify flags are a subset of max_flags. The limit belongs to the
    // region, not to the caller, so asking for more is EINVAL for everyone.
    if !region.max_flags.contains(flags) {
        crate::kwarn!(
            Mm,
            "shm_map: flags not subset of max_flags (pid={}, region={})",
            pid.0,
            region_id
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
            region_id
        );
        return Err(IpcError::Eexist as i64);
    }

    // Find a free mapping slot.
    let slot_idx = region
        .mappings
        .iter()
        .position(|m| m.is_none())
        .ok_or_else(|| {
            crate::kwarn!(Mm, "shm_map: max mappings reached (region={})", region_id);
            IpcError::Enospc as i64
        })?;

    // Compute VA for this mapping from the region's slot (not the raw id,
    // which carries the generation): base + slot * stride.
    let va = SHM_VA_BASE + idx * SHM_VA_STRIDE;
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
        region_id,
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
/// Decrements the reference count. If ref_count reaches 0, the region is
/// detached (`region_detach`): its frames are freed after the lock is
/// released, or, while kernel borrows hold them, at the last
/// `region_release`.
///
/// Errors: EINVAL for an out-of-range id (or a ref_count already 0); EPIPE
/// if the region does not exist, is draining or `region_id` is stale; EPERM if `pid` has
/// no mapping of it, a right the caller lacks, like a missing capability. No
/// capability is checked: holding the mapping is the authority to remove it.
pub fn shared_memory_unmap(pid: ProcessId, region_id: SharedMemoryId) -> Result<(), i64> {
    let Some(idx) = region_id.index() else {
        return Err(IpcError::Einval as i64);
    };

    let mut table = SHARED_REGION_TABLE.lock();
    let region = region_mut(&mut table, region_id)?;

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
                region_id
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
            region_id
        );
        return Err(IpcError::Einval as i64);
    }
    let old_ref = region.ref_count.fetch_sub(1, Ordering::Relaxed);
    let size_bytes = region.size_bytes;

    // If last reference, detach the region, advancing its slot's generation;
    // its frames are freed (or left draining) after the lock is released.
    let detached = if old_ref == 1 {
        region_detach(&mut table[idx], region_id)
    } else {
        None
    };

    drop(table);

    // Phase 3 (kernel-only threads): page table teardown is deferred.
    // The pages are still accessible via direct map until process exit.
    // Phase 4+ will unmap pages from the process's user page tables here.
    let _ = mapping_info;

    if let Some(detached) = detached {
        let freed = matches!(detached, Detached::Free { .. });
        // Frees the frames, or logs the region as draining.
        detached.finish();
        if freed {
            crate::kinfo!(
                Mm,
                "shm_unmap: region={} freed ({} pages) last_ref by pid={}",
                region_id,
                size_bytes / PAGE_SIZE,
                pid.0
            );
        }
    } else {
        crate::kinfo!(
            Mm,
            "shm_unmap: region={} unmapped for pid={} (refs={})",
            region_id,
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
/// Returns `Err(EINVAL)` for a region id whose slot is `>= MAX_SHARED_REGIONS`
/// or a `target_pid >= MAX_PROCESSES`, before taking any lock; EPIPE if the
/// region does not exist or `region_id` is stale (another generation), so
/// the grant always names the full id of the region the check found; EPERM
/// if `pid` is not the region's creator (a
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
    if region_id.index().is_none() {
        return Err(IpcError::Einval as i64);
    }
    // target_pid comes straight from the SharedMemoryShare syscall. Reject an
    // out-of-range pid here, before SHARED_REGION_TABLE or PROCESS_TABLE is
    // locked; grant_to_process would also return EINVAL for it.
    if target_pid.index().is_none() {
        return Err(IpcError::Einval as i64);
    }

    // Verify the region is the one `region_id` names (region.id, generation
    // included) and that the caller owns it.
    {
        let mut table = SHARED_REGION_TABLE.lock();
        let region = region_mut(&mut table, region_id)?;

        if region.creator != pid {
            crate::kwarn!(
                Mm,
                "shm_share: not creator (pid={}, region={}, creator={})",
                pid.0,
                region_id,
                region.creator.0
            );
            return Err(IpcError::Eperm as i64);
        }
    }
    // SHARED_REGION_TABLE lock released before acquiring PROCESS_TABLE (lock ordering).

    // Grant SharedMemoryAccess for the full id (slot and generation) to the
    // target process.
    crate::cap::grant_to_process(
        target_pid,
        shared::Capability::SharedMemoryAccess(region_id),
        false,
    )?;

    crate::kinfo!(
        Mm,
        "shm_share: region={} shared from pid={} to pid={}",
        region_id,
        pid.0,
        target_pid.0
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Process cleanup
// ---------------------------------------------------------------------------

/// Clean up all shared memory mappings for a process (called on process
/// exit). A region whose last mapping goes is detached (`region_detach`); its
/// frames are freed after `SHARED_REGION_TABLE` is released, or, while
/// kernel borrows hold them, at the last `region_release`.
pub fn process_cleanup_shared_memory(pid: ProcessId) {
    let mut table = SHARED_REGION_TABLE.lock();

    // The regions detached here, finished after the lock is released.
    let mut detached = FreeList::new();

    for slot in table.iter_mut() {
        let Some(region) = &mut slot.region else {
            continue;
        };
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
                // Last reference: detach the region, advancing its slot's
                // generation.
                let id = region.id;
                if let Some(d) = region_detach(slot, id) {
                    detached.push(d);
                }
            }
        }
    }

    drop(table);

    // Free the detached regions' frames (or log them draining) outside the
    // lock.
    let free_count = detached.finish();

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
/// An address in the shared memory window unmaps the region in that slot, by
/// the id the slot stores (EPIPE for an empty slot), through
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
            // The VA names a slot, not an id: unmap the region now in that
            // slot, by the id it stores. An empty slot is EPIPE, as for a
            // destroyed region.
            let stored = SHARED_REGION_TABLE.lock()[region_idx]
                .region
                .as_ref()
                .map(|r| r.id);
            let Some(region_id) = stored else {
                return Err(IpcError::Epipe as i64);
            };
            return shared_memory_unmap(pid, region_id);
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
// Region cascade (revocation of a SharedMemoryCreate token)
// ---------------------------------------------------------------------------

/// The region half of a revoke's cascade, run inside the revoking
/// `PROCESS_TABLE` hold (capability-lifetime ADR §5): detach every region
/// that `pid` created and whose `creation_cap` is revoked in `caps`, `pid`'s
/// capability table, recording each detach in `out`.
///
/// It reads the table, not one token id, so a revoke of an ancestor reaches
/// regions created under any of its descendants, and a `creation_cap` that no
/// slot holds counts as revoked (`is_revoked` fails closed). Other creators'
/// regions and draining slots are left alone. Takes `SHARED_REGION_TABLE`
/// nested under the caller's `PROCESS_TABLE` (an allowed edge) and releases
/// it before returning; frees and logs nothing. The caller finishes `out`
/// with `finish_region_cascade` once every lock is released.
pub(crate) fn revoke_regions_for_cap(
    pid: ProcessId,
    caps: &shared::CapabilityTable,
    out: &mut FreeList,
) {
    let mut table = SHARED_REGION_TABLE.lock();
    for slot in table.iter_mut() {
        let doomed = slot
            .region
            .as_ref()
            .filter(|r| r.creator == pid && caps.is_revoked(r.creation_cap))
            .map(|r| r.id);
        if let Some(id) = doomed {
            if let Some(d) = region_detach(slot, id) {
                out.push(d);
            }
        }
    }
}

/// Finish a region cascade of process `pid` with no lock held: free the
/// frames of every detached region that had no kernel borrow, log each one
/// left draining, and log `shm_cascade: pid=N destroyed K regions` when the
/// cascade detached any.
pub(crate) fn finish_region_cascade(pid: ProcessId, detached: FreeList) {
    let destroyed = detached.finish();
    if destroyed > 0 {
        crate::kinfo!(
            Mm,
            "shm_cascade: pid={} destroyed {} regions",
            pid.0,
            destroyed
        );
    }
}

// ---------------------------------------------------------------------------
// Kernel borrows (EL1 access through the direct map)
// ---------------------------------------------------------------------------

/// A kernel thread's borrow of a region's frames, from `region_borrow` to
/// `region_release`. EL1 reaches region memory through the direct map, which
/// no force-unmap removes, so kernel code dereferences a region's address
/// only while it holds a borrow: a region destroyed meanwhile keeps its
/// frames draining until the last release.
///
/// Neither `Clone` nor `Copy`, and released explicitly, not on `Drop`: an
/// implicit release inside a `SHARED_REGION_TABLE` hold would self-deadlock.
/// A borrow never released leaks the region's frames and keeps its slot out
/// of use for the boot; that is a leak, not a use-after-free.
#[must_use]
pub struct RegionBorrow {
    id: SharedMemoryId,
    addr: usize,
    len: usize,
}

impl RegionBorrow {
    /// Direct-map virtual address of the region's first byte.
    pub fn addr(&self) -> usize {
        self.addr
    }

    /// Size of the region in bytes; `[addr, addr + len)` stays valid until
    /// the borrow is released.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the borrowed region is still live: its slot holds the region
    /// `id` names. False once it is destroyed, while its frames drain.
    pub fn is_live(&self) -> bool {
        let mut table = SHARED_REGION_TABLE.lock();
        region_mut(&mut table, self.id).is_ok()
    }
}

/// Borrow region `id` for kernel access through the direct map, on behalf of
/// process `pid`, which must map it.
///
/// Errors: EINVAL for a malformed slot; EPIPE if the region does not exist,
/// is draining or `id` is stale; EPERM if `pid` has no mapping of it; ENOSPC
/// if the borrow count would overflow.
///
/// Takes `SHARED_REGION_TABLE` alone. Never called from IRQ context or under
/// a lock ranked below `SHARED_REGION_TABLE`.
pub fn region_borrow(pid: ProcessId, id: SharedMemoryId) -> Result<RegionBorrow, i64> {
    let mut table = SHARED_REGION_TABLE.lock();
    let region = region_mut(&mut table, id)?;
    if !region
        .mappings
        .iter()
        .any(|m| m.is_some_and(|m| m.pid == pid))
    {
        return Err(IpcError::Eperm as i64);
    }
    region.kernel_borrows = region
        .kernel_borrows
        .checked_add(1)
        .ok_or(IpcError::Enospc as i64)?;
    Ok(RegionBorrow {
        id,
        addr: crate::arch::aarch64::mmu::DIRECT_MAP_BASE + region.base_phys,
        len: region.size_bytes,
    })
}

/// Release a borrow from `region_borrow`. If its region was destroyed while
/// borrowed and this is the last borrow of its draining frames, the frames
/// are freed after `SHARED_REGION_TABLE` is released and `shm: region S.G
/// drained` is logged.
///
/// Takes `SHARED_REGION_TABLE` alone, never under `PROCESS_TABLE`.
///
/// # Panics
///
/// If the borrow's slot holds neither its live region with a borrow
/// outstanding nor its draining frames: a borrow released twice. Only
/// `region_borrow` makes a `RegionBorrow` and this function consumes it, so
/// that is a kernel bug. The panic comes after the lock is released.
pub fn region_release(b: RegionBorrow) {
    /// What the release found, acted on after the lock.
    enum Outcome {
        /// The live region's count went down.
        Released,
        /// The last borrow of draining frames: free them.
        Drained { base_phys: usize, order: usize },
        /// No borrow of `b.id` to release.
        Mismatch,
    }

    // A borrow's id came from region_mut, so its slot is in range.
    let outcome = b.id.index().map_or(Outcome::Mismatch, |idx| {
        let mut table = SHARED_REGION_TABLE.lock();
        let slot = &mut table[idx];
        if let Some(region) = slot
            .region
            .as_mut()
            .filter(|r| r.id == b.id && r.kernel_borrows > 0)
        {
            region.kernel_borrows -= 1;
            return Outcome::Released;
        }
        let Some(d) = slot.draining.as_mut().filter(|d| d.id == b.id) else {
            return Outcome::Mismatch;
        };
        // A draining record exists only while its count is above 0.
        d.borrows -= 1;
        if d.borrows > 0 {
            return Outcome::Released;
        }
        let drained = Outcome::Drained {
            base_phys: d.base_phys,
            order: d.order,
        };
        slot.draining = None;
        drained
    });

    match outcome {
        Outcome::Released => {}
        Outcome::Drained { base_phys, order } => {
            // SAFETY: the draining record held the last reference to this
            // block: region_detach moved the region's frames there when it
            // left its slot, and the record has just been taken out of the
            // slot under SHARED_REGION_TABLE with its last borrow released,
            // so no table entry or borrow refers to the block.
            // shared_memory_create allocated it with alloc_user_pages at this
            // order; region_detach and this function are the only code that
            // writes `draining`.
            // A second free would corrupt the buddy free lists and hand out
            // pages still in use.
            unsafe { crate::mm::frame::free_user_pages(base_phys, order) };
            crate::kinfo!(Mm, "shm: region {} drained", b.id);
        }
        Outcome::Mismatch => panic!("region_release: no borrow of region {}", b.id),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute the smallest buddy order such that `2^order >= pages`.
/// Delegates to the shared crate's portable implementation.
fn order_for_pages(pages: usize) -> usize {
    shared::order_for_pages(pages)
}
