//! Surface lifecycle management.
//!
//! `Surface` ties together a kernel-side bookkeeping record (id, owner pid,
//! IPC channel, shmem-backed buffer, position, size, state) with the public
//! `SurfaceLayer`/`SurfaceState`/`DamageRegion` types from the compositor
//! protocol (`shared/src/compositor.rs`). The compositor owns the
//! `SURFACE_TABLE` global and serializes all surface lifecycle operations
//! through it.
//!
//! State machine validation lives in `shared::compositor::SurfaceState::can_transition_to`.
//!
//! Per docs/platform/compositor/protocol.md §3.1.

use core::sync::atomic::{AtomicU64, Ordering};

use shared::compositor::{
    DamageRegion, SurfaceContentType, SurfaceId, SurfaceLayer, SurfaceState, SurfaceTitle,
    MAX_SURFACES,
};
use shared::ipc::{ChannelId, SharedMemoryId};
use spin::Mutex;

use crate::task::process::ProcessId;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that surface lifecycle operations can return.
///
/// Mapped to IPC error codes when the compositor replies to a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Wired by Step 14 IPC dispatch and Step 15 self-test.
pub enum SurfaceError {
    /// `SURFACE_TABLE` is full.
    TableFull,
    /// Surface ID does not exist (or has been destroyed).
    NotFound,
    /// Caller is not the owner of the surface.
    NotOwner,
    /// Requested state transition is invalid for the current state.
    InvalidTransition,
    /// Width or height is zero.
    InvalidSize,
}

// ---------------------------------------------------------------------------
// Surface
// ---------------------------------------------------------------------------

/// A compositor surface — the kernel-side record for a window or panel.
#[derive(Clone, Copy)]
#[allow(dead_code)] // Several fields read by render/composition pipeline (Steps 13-14).
pub struct Surface {
    pub id: SurfaceId,
    pub state: SurfaceState,
    pub layer: SurfaceLayer,
    pub title: SurfaceTitle,
    pub content_type: SurfaceContentType,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Shared memory buffer carrying the surface's pixel data, set by `AttachBuffer`.
    pub shmem_id: Option<SharedMemoryId>,
    /// Process that created this surface.
    pub owner_pid: ProcessId,
    /// Channel back to the owner — events (Configure, Input, BufferReleased) are sent here.
    pub channel: ChannelId,
    /// Insertion order within the layer; preserves z-order tie-break.
    pub layer_seq: u64,
    /// True when the surface has produced damage since the last frame.
    pub damaged: bool,
}

// ---------------------------------------------------------------------------
// Surface table — kernel-global, fixed-size, mutex-protected
// ---------------------------------------------------------------------------

/// System-wide table of compositor surfaces.
///
/// Lock ordering: `... > BLOCK_ENGINE > SURFACE_TABLE > {VIRTIO_BLK,
/// VIRTIO_GPU, VIRTIO_INPUT}`. Hold this lock briefly — never call into
/// `virtio_*` drivers while holding it.
pub static SURFACE_TABLE: Mutex<[Option<Surface>; MAX_SURFACES]> = {
    const NONE: Option<Surface> = None;
    Mutex::new([NONE; MAX_SURFACES])
};

/// Monotonic surface-id allocator. Never reused, even across `surface_destroy`.
#[allow(dead_code)] // Read by surface_create() once IPC dispatch lands in Step 14.
static NEXT_SURFACE_ID: AtomicU64 = AtomicU64::new(SurfaceId::FIRST.0);

/// Monotonic per-layer insertion counter for stable z-order tie-break.
#[allow(dead_code)] // Read by surface_create() / surface_set_layer() — Step 14.
static NEXT_LAYER_SEQ: AtomicU64 = AtomicU64::new(1);

// ---------------------------------------------------------------------------
// Surface lifecycle operations
// ---------------------------------------------------------------------------

/// Create a new surface. Returns the assigned `SurfaceId`.
///
/// The surface is created in `SurfaceState::Created`. The caller (the
/// compositor's IPC dispatcher) is responsible for sending the `Configure`
/// event back to the client before any `AttachBuffer` call is processed.
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)] // Wired by Step 14 IPC dispatch and Step 15 self-test.
pub fn surface_create(
    owner_pid: ProcessId,
    channel: ChannelId,
    width: u32,
    height: u32,
    title: SurfaceTitle,
    content_type: SurfaceContentType,
    layer: SurfaceLayer,
) -> Result<SurfaceId, SurfaceError> {
    if width == 0 || height == 0 {
        return Err(SurfaceError::InvalidSize);
    }

    let id = SurfaceId(NEXT_SURFACE_ID.fetch_add(1, Ordering::Relaxed));
    let layer_seq = NEXT_LAYER_SEQ.fetch_add(1, Ordering::Relaxed);

    let mut table = SURFACE_TABLE.lock();
    let slot = table
        .iter_mut()
        .find(|s| s.is_none())
        .ok_or(SurfaceError::TableFull)?;

    *slot = Some(Surface {
        id,
        state: SurfaceState::Created,
        layer,
        title,
        content_type,
        x: 0,
        y: 0,
        width,
        height,
        shmem_id: None,
        owner_pid,
        channel,
        layer_seq,
        damaged: true,
    });

    Ok(id)
}

/// Attach a shared memory buffer to a surface. Marks the surface as damaged
/// according to `damage` and transitions Created/Configured → Active on the
/// first attach.
#[allow(dead_code)] // Wired by Step 14 IPC dispatch and Step 15 self-test.
pub fn surface_attach_buffer(
    id: SurfaceId,
    shmem_id: SharedMemoryId,
    damage: DamageRegion,
    caller_pid: ProcessId,
) -> Result<(), SurfaceError> {
    let mut table = SURFACE_TABLE.lock();
    let surface = find_mut(&mut table, id).ok_or(SurfaceError::NotFound)?;

    if surface.owner_pid.0 != caller_pid.0 {
        return Err(SurfaceError::NotOwner);
    }
    if surface.state.is_terminal() {
        return Err(SurfaceError::InvalidTransition);
    }

    let next_state = match surface.state {
        // First buffer attach takes us through Configured → Active even if the
        // client called AttachBuffer before processing its Configure event.
        SurfaceState::Created | SurfaceState::Configured => SurfaceState::Active,
        // Subsequent attaches keep us in Active (idempotent self-transition).
        SurfaceState::Active | SurfaceState::Suspended => SurfaceState::Active,
        SurfaceState::Destroyed => return Err(SurfaceError::InvalidTransition),
    };
    if !surface.state.can_transition_to(next_state) {
        return Err(SurfaceError::InvalidTransition);
    }

    surface.shmem_id = Some(shmem_id);
    surface.state = next_state;
    if damage.has_damage() {
        surface.damaged = true;
    }

    Ok(())
}

/// Tear down a surface. Releases its slot in `SURFACE_TABLE`. The `SurfaceId`
/// itself is never reused — `NEXT_SURFACE_ID` continues forward.
#[allow(dead_code)] // Wired by Step 14 IPC dispatch and Step 15 self-test.
pub fn surface_destroy(id: SurfaceId, caller_pid: ProcessId) -> Result<(), SurfaceError> {
    let mut table = SURFACE_TABLE.lock();
    let slot = table
        .iter_mut()
        .find(|s| s.as_ref().is_some_and(|surface| surface.id == id))
        .ok_or(SurfaceError::NotFound)?;

    let surface = slot.as_mut().expect("filtered for Some above");
    if surface.owner_pid.0 != caller_pid.0 {
        return Err(SurfaceError::NotOwner);
    }
    if !surface.state.can_transition_to(SurfaceState::Destroyed) {
        return Err(SurfaceError::InvalidTransition);
    }

    *slot = None;
    Ok(())
}

/// Resize a surface. Returns the new (possibly clamped) dimensions so the
/// caller can include them in the follow-up `Configure` event.
#[allow(dead_code)] // Wired by Step 14 IPC dispatch.
pub fn surface_resize(
    id: SurfaceId,
    width: u32,
    height: u32,
    caller_pid: ProcessId,
) -> Result<(u32, u32), SurfaceError> {
    if width == 0 || height == 0 {
        return Err(SurfaceError::InvalidSize);
    }
    let mut table = SURFACE_TABLE.lock();
    let surface = find_mut(&mut table, id).ok_or(SurfaceError::NotFound)?;
    if surface.owner_pid.0 != caller_pid.0 {
        return Err(SurfaceError::NotOwner);
    }
    if surface.state.is_terminal() {
        return Err(SurfaceError::InvalidTransition);
    }
    surface.width = width;
    surface.height = height;
    surface.damaged = true;
    Ok((width, height))
}

/// Move a surface to a different z-order layer. Allocates a fresh insertion
/// sequence so the surface lands at the top of the new layer.
#[allow(dead_code)] // Wired by Step 14 IPC dispatch.
pub fn surface_set_layer(
    id: SurfaceId,
    layer: SurfaceLayer,
    caller_pid: ProcessId,
) -> Result<(), SurfaceError> {
    let mut table = SURFACE_TABLE.lock();
    let surface = find_mut(&mut table, id).ok_or(SurfaceError::NotFound)?;
    if surface.owner_pid.0 != caller_pid.0 {
        return Err(SurfaceError::NotOwner);
    }
    if surface.state.is_terminal() {
        return Err(SurfaceError::InvalidTransition);
    }
    surface.layer = layer;
    surface.layer_seq = NEXT_LAYER_SEQ.fetch_add(1, Ordering::Relaxed);
    surface.damaged = true;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn find_mut(table: &mut [Option<Surface>; MAX_SURFACES], id: SurfaceId) -> Option<&mut Surface> {
    table
        .iter_mut()
        .filter_map(|s| s.as_mut())
        .find(|s| s.id == id)
}

/// Mark a surface as damaged (compositor-internal; bypasses ownership checks).
///
/// Used when the compositor itself detects a reason to recomposite a surface
/// (e.g., a focus change or a layer reshuffle).
#[allow(dead_code)] // Used by Steps 13 and 14.
pub fn mark_damaged(id: SurfaceId) {
    let mut table = SURFACE_TABLE.lock();
    if let Some(surface) = find_mut(&mut table, id) {
        surface.damaged = true;
    }
}
