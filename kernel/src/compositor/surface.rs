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
#[allow(dead_code)] // `state`, `content_type`, `damaged`, `shmem_id` are read
                    // only once the gated COMPOSITOR_PRESENT_ENABLED render
                    // path turns on (M26+); the IPC dispatch (Step 20) reads
                    // the rest.
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

/// Process id used by every compositor-internal shell surface (Status
/// Strip, Taskbar, Workspace). The compositor service registers itself
/// as `ProcessId(10)` in `service::init_compositor`; the shell allocates
/// its surfaces from inside that service so they all share this owner.
const COMPOSITOR_PROCESS_ID: ProcessId = ProcessId(10);

impl Surface {
    /// Returns true when this surface is a compositor-internal "shell"
    /// surface (Status Strip, Taskbar, future Workspace).
    ///
    /// Shell predicate is `(owner_pid == compositor) && (layer == Panel)`.
    /// `Panel` is reserved for system chrome per `SurfaceLayer`'s docs;
    /// only the compositor itself is allowed to publish on that layer in
    /// Phase 7. The compound check is defense-in-depth against a future
    /// kernel-internal surface that uses ProcessId(10) but a different
    /// layer (none planned, but the predicate stays robust).
    ///
    /// Used by the Taskbar to filter shell surfaces out of its window
    /// list (Step 25), and by Step 27's input router to refuse keyboard
    /// focus on shell surfaces.
    #[allow(dead_code)] // First consumer is Step 25 taskbar; Step 27 adds
                        // the input-routing call site.
    pub fn is_shell(&self) -> bool {
        self.owner_pid.0 == COMPOSITOR_PROCESS_ID.0 && matches!(self.layer, SurfaceLayer::Panel)
    }
}

// ---------------------------------------------------------------------------
// Surface table — kernel-global, fixed-size, mutex-protected
// ---------------------------------------------------------------------------

/// System-wide table of compositor surfaces.
///
/// Lock ordering (M25): `... > BLOCK_ENGINE > WINDOW_Z_ORDER >
/// DRAG_STATE > SURFACE_TABLE > {VIRTIO_BLK, VIRTIO_GPU, VIRTIO_INPUT}`.
/// The leaf-independent compositor mutexes (FOCUS_MANAGER, CURSOR_POS,
/// TITLE_FONT) sit alongside INPUT_QUEUE/PENDING_POINTER and are never
/// co-held with SURFACE_TABLE. Hold this lock briefly — never call into
/// `virtio_*` drivers or issue IPC while holding it.
pub static SURFACE_TABLE: Mutex<[Option<Surface>; MAX_SURFACES]> = {
    const NONE: Option<Surface> = None;
    Mutex::new([NONE; MAX_SURFACES])
};

/// Monotonic surface-id allocator. Never reused, even across `surface_destroy`.
static NEXT_SURFACE_ID: AtomicU64 = AtomicU64::new(SurfaceId::FIRST.0);

/// Monotonic per-layer insertion counter for stable z-order tie-break.
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

/// Reposition a surface in screen coordinates.
///
/// Used by the compositor's window-move drag handler and by shell
/// surfaces (Taskbar, Workspace) that need to lock to specific edges of
/// the display. Marks the surface damaged so the next composition
/// frame picks up the new position.
#[allow(dead_code)] // First consumer is M26 Step 25 (taskbar bottom-edge
                    // anchoring); the M25 drag handler still mutates
                    // SURFACE_TABLE inline and will migrate to this
                    // helper when the lock-ordering audit revisits it.
pub fn surface_set_position(
    id: SurfaceId,
    x: i32,
    y: i32,
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
    surface.x = x;
    surface.y = y;
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
/// Used by the gated `COMPOSITOR_PRESENT_ENABLED` render loop (M26+) and
/// future call sites that need to force a recomposite without owning the
/// surface (e.g., focus indicator change).
#[allow(dead_code)]
pub fn mark_damaged(id: SurfaceId) {
    let mut table = SURFACE_TABLE.lock();
    if let Some(surface) = find_mut(&mut table, id) {
        surface.damaged = true;
    }
}
