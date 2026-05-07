//! Workspace — Normal-layer home view (M26 Step 26).
//!
//! A toggle-visible centered surface showing the AIOS title, the list of
//! system spaces (`storage::space_list`), and live uptime. Acts as the
//! Layer 1 "home screen" — whenever no application has the user's
//! attention, Super reveals a calm, fixed layout. No context inference
//! and no adaptive behavior in M26.
//!
//! Per docs/experience/experience.md §3.1–3.2 (Workspace) and the M26
//! working plan in `docs/knowledge/plans/phase-7-m26-desktop-shell.md`.
//!
//! Like the other shell surfaces, the Workspace is **compositor-internal**
//! — owned by `ProcessId(10)`, registered in `SURFACE_TABLE` with the
//! compositor's well-known channel, never receives IPC events
//! (`service::is_self_channel` suppresses self-deliveries). Visibility
//! defaults to `false`; Super (release-edge of bare-Super) toggles it
//! via `apply_show_workspace` in `compositor::hotkey`.
//!
//! Layer choice: `SurfaceLayer::Normal` (NOT `Panel`). Per the phase
//! doc, the Workspace must sit "behind other Normal-layer windows but
//! above Background", so it stays a peer to client windows in z-order
//! rather than floating above them.

use core::sync::atomic::{AtomicBool, Ordering};

use shared::compositor::{
    format_hhmmss, DamageRegion, SurfaceContentType, SurfaceId, SurfaceLayer, SurfaceTitle,
};
use shared::ipc::SharedMemoryId;
use spin::Mutex;

use crate::compositor::surface::{
    mark_damaged, surface_attach_buffer, surface_create, surface_set_position, surface_set_visible,
};
use crate::compositor::text::{draw_text_clipped, TITLE_GLYPH_HEIGHT, TITLE_GLYPH_WIDTH};
use crate::compositor::window::fill_rect;
use crate::ipc::shmem::{region_dmap_addr, region_size, shared_memory_create};
use crate::mm::pgtable::VmFlags;
use crate::task::process::ProcessId;

use super::ShellError;

// ---------------------------------------------------------------------------
// Layout, sizing, colors
// ---------------------------------------------------------------------------

/// Status Strip occupies the top 32 px; the workspace cannot overlap it.
const STATUS_STRIP_HEIGHT: u32 = 32;
/// Taskbar occupies the bottom 40 px; the workspace cannot overlap it.
const TASKBAR_HEIGHT: u32 = 40;

/// Maximum width of the workspace surface in pixels. Clamped down to
/// `display_width` if the display is narrower.
const WORKSPACE_MAX_WIDTH: u32 = 800;
/// Maximum height. Clamped to whatever sits between the Status Strip
/// and the Taskbar.
const WORKSPACE_MAX_HEIGHT: u32 = 600;

/// Bytes-per-pixel for the back-buffer (B8G8R8A8 = 4).
const BYTES_PER_PIXEL: usize = 4;

/// Workspace background — a deeper midnight tone than the chrome strips
/// to read as "the home screen", not yet another panel.
const WORKSPACE_BG: u32 = 0xFF06_0A14;
/// Title and body text color.
const TEXT_FG: u32 = 0xFFEC_F0F8;
/// Subtle accent used for section headers and the uptime line.
const ACCENT_FG: u32 = 0xFF8A_B4FF;

/// Inner padding from the surface edge to where text begins.
const PADDING_X: i32 = 24;
/// Vertical padding above the centered title.
const TITLE_TOP_Y: i32 = 32;
/// Spaces list begins this many pixels below the title baseline.
const SPACES_HEADER_OFFSET: i32 = 64;
/// Per-line spacing for the spaces list.
const SPACES_LINE_HEIGHT: i32 = 20;
/// Maximum number of space rows we render — Layer 1 systems ship three
/// (system, user/home, ephemeral) but the cap leaves headroom.
const MAX_SPACES_RENDERED: usize = 8;
/// Maximum bytes from each space name we cache in the damage signature
/// (the renderer truncates to fit one line, so 32 is plenty).
const SPACE_NAME_CACHE_BYTES: usize = 32;

// ---------------------------------------------------------------------------
// Damage tracking snapshot
// ---------------------------------------------------------------------------

/// Per-space row contribution to the damage signature.
#[derive(Clone, Copy, PartialEq, Eq)]
struct SpaceSnapshot {
    name_len: u8,
    name: [u8; SPACE_NAME_CACHE_BYTES],
}

impl SpaceSnapshot {
    const fn empty() -> Self {
        Self {
            name_len: 0,
            name: [0; SPACE_NAME_CACHE_BYTES],
        }
    }
}

/// Snapshot of every input that could affect the rendered workspace
/// pixels. Two snapshots compare equal iff the rendered frame is
/// identical, so equality short-circuits redraws.
#[derive(Clone, Copy, PartialEq, Eq)]
struct WorkspaceSnapshot {
    visible: bool,
    /// Uptime rounded to whole seconds. The renderer formats to HH:MM:SS,
    /// so any sub-second tick that doesn't change this never triggers a
    /// redraw.
    uptime_seconds: u64,
    /// Number of space rows actually populated.
    space_count: u8,
    /// Per-row name cache.
    spaces: [SpaceSnapshot; MAX_SPACES_RENDERED],
    /// `true` when `space_list()` returned an error or empty list.
    spaces_unavailable: bool,
}

impl WorkspaceSnapshot {
    const fn empty() -> Self {
        Self {
            visible: false,
            uptime_seconds: u64::MAX,
            space_count: 0,
            spaces: [SpaceSnapshot::empty(); MAX_SPACES_RENDERED],
            spaces_unavailable: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Workspace state
// ---------------------------------------------------------------------------

/// All persistent state the workspace renderer needs.
struct WorkspaceState {
    surface_id: SurfaceId,
    /// Backing shared-memory region (kept for eventual `compose_frame`
    /// resolution; written via the cached direct-map VA).
    shmem_id: SharedMemoryId,
    /// Direct-map VA of `shmem_id`'s pages.
    buffer_va: usize,
    /// Capacity of the buffer in u32 pixels.
    buffer_pixels: usize,
    /// Surface width.
    width: u32,
    /// Surface height.
    height: u32,
    /// Last rendered snapshot.
    cached_snapshot: WorkspaceSnapshot,
    /// `true` until the first render — forces an unconditional first draw
    /// even when the snapshot matches the empty sentinel.
    needs_first_render: bool,
    /// User-controlled visibility flag. `false` at boot per the phase doc.
    visible: bool,
}

/// Lock ordering: leaf — never co-held with `SURFACE_TABLE`,
/// `SHARED_REGION_TABLE`, `FOCUS_MANAGER`, or any IPC mutex. `tick()`
/// snapshots inputs (releasing those locks) before taking this mutex.
static WORKSPACE: Mutex<Option<WorkspaceState>> = Mutex::new(None);

/// Set after `init` populates `WORKSPACE`. Lets `tick` and
/// `toggle_visibility` skip the mutex on the no-op fast path.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Allocate the workspace surface and seed its backing buffer.
///
/// Sequence (lock-ordering compliant — `SHARED_REGION_TABLE >
/// SURFACE_TABLE`):
///   1. Compute the centered geometry.
///   2. Allocate a shmem region, snapshot its direct-map VA, fill bg.
///   3. `surface_create` (Normal layer, `SystemUI` content type).
///   4. `surface_attach_buffer` (Created → Active).
///   5. `surface_set_position` (centered between Status Strip and Taskbar).
///   6. `surface_set_visible(false)` — workspace starts hidden.
///   7. Cache state, flip `INITIALIZED`, log surface id.
pub(super) fn init(display_width: u32, display_height: u32) -> Result<(), ShellError> {
    if display_width == 0 || display_height == 0 {
        return Err(ShellError::NoDisplay);
    }

    // Bound the surface to the chrome-free region. If the display is too
    // small to host any content between the Status Strip and Taskbar, we
    // refuse to bring up the workspace rather than producing a degenerate
    // surface.
    let chrome_height = STATUS_STRIP_HEIGHT.saturating_add(TASKBAR_HEIGHT);
    if display_height <= chrome_height {
        return Err(ShellError::NoDisplay);
    }
    let usable_height = display_height - chrome_height;
    let width = display_width.min(WORKSPACE_MAX_WIDTH);
    let height = usable_height.min(WORKSPACE_MAX_HEIGHT);
    if width == 0 || height == 0 {
        return Err(ShellError::NoDisplay);
    }
    let pixel_count = (width as usize) * (height as usize);
    let byte_count = pixel_count * BYTES_PER_PIXEL;

    // Step 2: shmem.
    let shmem_id = shared_memory_create(
        ProcessId(10),
        byte_count,
        VmFlags::READ.union(VmFlags::WRITE),
    )
    .map_err(|_| ShellError::ShmemCreate)?;

    let buffer_va = region_dmap_addr(shmem_id).ok_or(ShellError::ShmemCreate)?;
    let buffer_bytes = region_size(shmem_id).ok_or(ShellError::ShmemCreate)?;
    let buffer_pixels = buffer_bytes / BYTES_PER_PIXEL;
    if buffer_pixels < pixel_count {
        return Err(ShellError::ShmemCreate);
    }

    // SAFETY: `buffer_va` is the direct-map address of a freshly allocated
    // shmem region of `byte_count` bytes (verified above). The slice
    // covers exactly `pixel_count` u32s and lives for the lifetime of
    // the shell (regions are never freed once the shell is up).
    // Maintained by: only the workspace renderer writes through this
    // VA; shmem region lifetime is permanent for the compositor process.
    // Violation: writing past `pixel_count` would corrupt adjacent shmem
    // pages or the next allocation in the user pool.
    let buffer = unsafe { core::slice::from_raw_parts_mut(buffer_va as *mut u32, pixel_count) };
    buffer.fill(WORKSPACE_BG);

    // Step 3: surface.
    let channel = match *crate::compositor::service::COMPOSITOR_CHANNEL.lock() {
        Some(ch) => ch,
        None => return Err(ShellError::SurfaceCreate),
    };
    let title = SurfaceTitle::from_bytes(b"workspace");
    let surface_id = surface_create(
        ProcessId(10),
        channel,
        width,
        height,
        title,
        SurfaceContentType::SystemUI,
        SurfaceLayer::Normal,
    )
    .map_err(|_| ShellError::SurfaceCreate)?;

    // Step 4: attach buffer.
    surface_attach_buffer(
        surface_id,
        shmem_id,
        DamageRegion::FullSurface,
        ProcessId(10),
    )
    .map_err(|_| ShellError::AttachBuffer)?;

    // Step 5: center the workspace between the chrome strips.
    let x = ((display_width as i32) - (width as i32)) / 2;
    let y = STATUS_STRIP_HEIGHT as i32 + ((usable_height as i32) - (height as i32)) / 2;
    surface_set_position(surface_id, x, y, ProcessId(10)).map_err(|_| ShellError::SurfaceCreate)?;

    // Step 6: workspace starts hidden — Super reveals it.
    surface_set_visible(surface_id, false, ProcessId(10)).map_err(|_| ShellError::SurfaceCreate)?;

    // Step 7: cache state.
    let state = WorkspaceState {
        surface_id,
        shmem_id,
        buffer_va,
        buffer_pixels,
        width,
        height,
        cached_snapshot: WorkspaceSnapshot::empty(),
        needs_first_render: true,
        visible: false,
    };
    *WORKSPACE.lock() = Some(state);
    INITIALIZED.store(true, Ordering::Release);

    crate::kinfo!(
        Compositor,
        "shell: workspace surface={} created ({}x{})",
        surface_id.0,
        width,
        height
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Visibility toggle
// ---------------------------------------------------------------------------

/// Flip the workspace's visibility.
///
/// Wired to bare-Super tap-release via
/// `compositor::hotkey::apply_show_workspace`. Updates both the local
/// `WorkspaceState.visible` flag (used by the next `tick` to decide
/// whether to redraw the uptime line) and the
/// `Surface::visible` flag in `SURFACE_TABLE` (used by `compose_frame`
/// to skip the surface when hidden).
pub fn toggle_visibility() {
    if !INITIALIZED.load(Ordering::Acquire) {
        return;
    }
    let (surface_id, new_visible) = {
        let mut guard = WORKSPACE.lock();
        let ws = match guard.as_mut() {
            Some(s) => s,
            None => return,
        };
        ws.visible = !ws.visible;
        (ws.surface_id, ws.visible)
    };

    // Drop the WORKSPACE leaf lock before climbing back into
    // SURFACE_TABLE via surface_set_visible / mark_damaged.
    let _ = surface_set_visible(surface_id, new_visible, ProcessId(10));
    mark_damaged(surface_id);

    crate::kinfo!(
        Compositor,
        "shell: workspace visibility -> {}",
        if new_visible { "visible" } else { "hidden" }
    );
}

// ---------------------------------------------------------------------------
// Tick — invoked once per compositor loop iteration
// ---------------------------------------------------------------------------

/// Re-render the workspace if its inputs changed since the last
/// successful render. Called from `super::tick` once per loop iteration.
///
/// Behavior:
///   * When `visible == false`, only redraws on the first tick (to seed
///     the buffer) — the surface is excluded from composition anyway, so
///     further updates would burn CPU for nothing.
///   * When `visible == true`, redraws when the second-rounded uptime
///     changes, the spaces list changes, or the visibility flag itself
///     just flipped on.
///
/// Lock sequence: WORKSPACE (leaf, snapshot read) → drop → fetch
/// space list (acquires storage's BLOCK_ENGINE) → WORKSPACE (leaf)
/// for the cache compare and render → drop → SURFACE_TABLE via
/// `surface_attach_buffer` + `mark_damaged`. No two co-held.
pub(super) fn tick(now_ms: u64) {
    if !INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    // Quick pre-check — if hidden and we already drew the (hidden)
    // first frame, skip. Don't even fetch space_list.
    let (currently_visible, needs_first_render) = {
        let guard = WORKSPACE.lock();
        match guard.as_ref() {
            Some(ws) => (ws.visible, ws.needs_first_render),
            None => return,
        }
    };
    if !currently_visible && !needs_first_render {
        return;
    }

    // Build the new snapshot. `space_list()` returns Vec<Space> but only
    // when storage is initialized — in early bring-up or on error the
    // snapshot records `spaces_unavailable = true` so the renderer
    // falls back to "(no spaces)".
    let snapshot = build_snapshot(currently_visible, now_ms);

    // Compare and render under the leaf WORKSPACE lock.
    let mut guard = WORKSPACE.lock();
    let ws = match guard.as_mut() {
        Some(s) => s,
        None => return,
    };

    if !ws.needs_first_render && ws.cached_snapshot == snapshot {
        return;
    }

    ws.cached_snapshot = snapshot;
    ws.needs_first_render = false;

    // SAFETY: `buffer_va` was captured at init from `region_dmap_addr`;
    // shmem regions are never freed for compositor-internal surfaces, so
    // the address remains valid for the lifetime of the kernel. Slice
    // length is bounded by `buffer_pixels` (sized at init).
    // Maintained by: shell surface lifecycle — `init` allocates once,
    // no teardown path exists in M26.
    // Violation: writing past `buffer_pixels` would corrupt user-pool
    // pages adjacent to the shmem region.
    let buffer =
        unsafe { core::slice::from_raw_parts_mut(ws.buffer_va as *mut u32, ws.buffer_pixels) };
    let surface_id = ws.surface_id;
    let shmem_id = ws.shmem_id;
    let width = ws.width;
    let height = ws.height;
    let snapshot_copy = ws.cached_snapshot;

    render_frame(buffer, width, height, &snapshot_copy);

    // Drop the WORKSPACE leaf before re-attaching / marking damage.
    drop(guard);

    let _ = surface_attach_buffer(
        surface_id,
        shmem_id,
        DamageRegion::FullSurface,
        ProcessId(10),
    );
    mark_damaged(surface_id);
}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

fn build_snapshot(visible: bool, now_ms: u64) -> WorkspaceSnapshot {
    let mut snap = WorkspaceSnapshot {
        visible,
        uptime_seconds: now_ms / 1000,
        space_count: 0,
        spaces: [SpaceSnapshot::empty(); MAX_SPACES_RENDERED],
        spaces_unavailable: false,
    };

    match crate::storage::space::space_list() {
        Ok(list) if !list.is_empty() => {
            for space in list.iter().take(MAX_SPACES_RENDERED) {
                let name = space.name_bytes();
                let cut = if name.len() > SPACE_NAME_CACHE_BYTES {
                    SPACE_NAME_CACHE_BYTES
                } else {
                    name.len()
                };
                let mut entry = SpaceSnapshot::empty();
                entry.name_len = cut as u8;
                entry.name[..cut].copy_from_slice(&name[..cut]);
                let idx = snap.space_count as usize;
                snap.spaces[idx] = entry;
                snap.space_count += 1;
            }
        }
        _ => {
            snap.spaces_unavailable = true;
        }
    }

    snap
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

/// Fully repaint the workspace buffer from a snapshot. The buffer must
/// be at least `width * height` u32s — guaranteed by `init`.
fn render_frame(buffer: &mut [u32], width: u32, height: u32, snap: &WorkspaceSnapshot) {
    if buffer.len() < (width as usize) * (height as usize) {
        return;
    }

    // Background.
    fill_rect(buffer, width, height, 0, 0, width, height, WORKSPACE_BG);

    // Centered "AIOS" title.
    let title = b"AIOS";
    let title_w = (title.len() as i32) * TITLE_GLYPH_WIDTH;
    let title_x = ((width as i32) - title_w) / 2;
    draw_text_clipped(
        buffer,
        width,
        height,
        title_x,
        TITLE_TOP_Y,
        width as i32,
        title,
        TEXT_FG,
        WORKSPACE_BG,
    );

    // "Spaces:" header followed by either the entries or "(no spaces)".
    let mut row_y = TITLE_TOP_Y + SPACES_HEADER_OFFSET;
    let max_x = (width as i32) - PADDING_X;
    draw_text_clipped(
        buffer,
        width,
        height,
        PADDING_X,
        row_y,
        max_x,
        b"Spaces:",
        ACCENT_FG,
        WORKSPACE_BG,
    );
    row_y += SPACES_LINE_HEIGHT;

    if snap.spaces_unavailable || snap.space_count == 0 {
        draw_text_clipped(
            buffer,
            width,
            height,
            PADDING_X + TITLE_GLYPH_WIDTH * 2,
            row_y,
            max_x,
            b"(no spaces)",
            TEXT_FG,
            WORKSPACE_BG,
        );
    } else {
        for i in 0..snap.space_count as usize {
            let entry = &snap.spaces[i];
            let name = &entry.name[..entry.name_len as usize];
            // Bullet marker so an empty name still has a visible row.
            draw_text_clipped(
                buffer,
                width,
                height,
                PADDING_X + TITLE_GLYPH_WIDTH * 2,
                row_y,
                max_x,
                b"- ",
                ACCENT_FG,
                WORKSPACE_BG,
            );
            draw_text_clipped(
                buffer,
                width,
                height,
                PADDING_X + TITLE_GLYPH_WIDTH * 4,
                row_y,
                max_x,
                name,
                TEXT_FG,
                WORKSPACE_BG,
            );
            row_y += SPACES_LINE_HEIGHT;
            // Stop drawing if we'd run past the uptime band.
            if row_y >= (height as i32) - SPACES_LINE_HEIGHT * 2 - TITLE_GLYPH_HEIGHT {
                break;
            }
        }
    }

    // Uptime line, anchored near the bottom.
    let uptime_ms = snap.uptime_seconds.saturating_mul(1000);
    let uptime_digits = format_hhmmss(uptime_ms);
    let mut uptime_text = [0u8; b"Uptime: ".len() + 8];
    uptime_text[..b"Uptime: ".len()].copy_from_slice(b"Uptime: ");
    uptime_text[b"Uptime: ".len()..].copy_from_slice(&uptime_digits);
    let uptime_y = (height as i32) - SPACES_LINE_HEIGHT - TITLE_GLYPH_HEIGHT;
    draw_text_clipped(
        buffer,
        width,
        height,
        PADDING_X,
        uptime_y,
        max_x,
        &uptime_text,
        ACCENT_FG,
        WORKSPACE_BG,
    );
}
