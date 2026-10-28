//! Taskbar — bottom-edge Panel-layer surface (M26 Step 25).
//!
//! 40-pixel-tall surface locked to `y == display_height - 40`, spanning
//! the full display width. Renders a `[W]` workspace button on the far
//! left, a horizontal list of non-shell client surface titles in the
//! middle, and an `N windows` count readout on the far right. The focused
//! entry is highlighted with a brighter background.
//!
//! Damage tracking: redraws are skipped when neither the focused surface
//! nor the visible-entry snapshot (id + title prefix) has changed since
//! the last successful render. The taskbar is always-on but cheap when
//! idle.
//!
//! Like the Status Strip, the Taskbar is **compositor-internal**: it is
//! owned by `ProcessId(10)`, registered in `SURFACE_TABLE` with the
//! compositor's well-known channel, and never receives IPC events
//! (`service::is_self_channel` suppresses self-deliveries). Step 27 adds
//! the input-routing path that turns taskbar entries into focus
//! switchers.
//!
//! Per docs/experience/experience.md §2 (Five Surfaces) and the M26
//! working plan (`docs/knowledge/plans/phase-7-m26-desktop-shell.md`).

use core::sync::atomic::{AtomicBool, Ordering};

use shared::compositor::{
    compute_taskbar_layout, taskbar_entry_truncate, taskbar_pointer_action, DamageRegion,
    SurfaceContentType, SurfaceId, SurfaceLayer, SurfaceTitle, TaskbarCell, TaskbarLayout,
    TaskbarPointerAction, MAX_TASKBAR_ENTRIES,
};
use shared::input::{ButtonState, InputEvent, MouseButton};
use shared::ipc::SharedMemoryId;
use spin::Mutex;

use crate::compositor::focus::FOCUS_MANAGER;
use crate::compositor::surface::{
    mark_damaged, surface_attach_buffer, surface_create, surface_set_position, Surface,
    SURFACE_TABLE,
};
use crate::compositor::text::{draw_text_clipped, TITLE_GLYPH_HEIGHT, TITLE_GLYPH_WIDTH};
use crate::compositor::window::fill_rect;
use crate::ipc::shmem::{region_dmap_addr, region_size, shared_memory_create};
use crate::mm::pgtable::VmFlags;
use crate::task::process::ProcessId;

use super::ShellError;

// ---------------------------------------------------------------------------
// Layout & colors
// ---------------------------------------------------------------------------

/// Fixed Taskbar height in pixels (matches the phase doc M26 Step 25 spec).
pub const TASKBAR_HEIGHT: u32 = 40;

/// Bytes-per-pixel for the back-buffer (B8G8R8A8 = 4).
const BYTES_PER_PIXEL: usize = 4;

/// Bar background — same dark tone as the Status Strip's chrome family
/// but a touch lighter so the two strips read as distinct surfaces when
/// stacked on a black wallpaper.
const TASKBAR_BG: u32 = 0xFF0F_141C;
/// Background of an unfocused entry cell.
const ENTRY_BG_UNFOCUSED: u32 = 0xFF1A_1A1A;
/// Background of the focused entry cell — a saturated blue band.
const ENTRY_BG_FOCUSED: u32 = 0xFF30_60A0;
/// Workspace-button background.
const WORKSPACE_BG: u32 = 0xFF2A_2A2A;
/// Foreground (text) color used everywhere on the taskbar.
const TEXT_FG: u32 = 0xFFFF_FFFF;

/// Inner padding between an entry cell's edge and its glyph run.
const ENTRY_PADDING_X: i32 = 8;

/// Maximum number of glyphs that fit inside one entry cell after both
/// padding margins. `(TASKBAR_ENTRY_WIDTH - 2 * ENTRY_PADDING_X) /
/// TITLE_GLYPH_WIDTH = (200 - 16) / 8 = 23`.
const ENTRY_TITLE_MAX_CHARS: usize = 23;

/// Number of bytes of each title we cache for damage comparison. The
/// renderer truncates to `ENTRY_TITLE_MAX_CHARS` (23) at draw time, so
/// 24 bytes of cache covers the visible run and an extra char to detect
/// trailing-character changes that happen to land exactly at the cut.
const CACHED_TITLE_BYTES: usize = 24;

// ---------------------------------------------------------------------------
// Damage-tracking snapshot
// ---------------------------------------------------------------------------

/// One entry's contribution to the damage hash. Two snapshots compare
/// equal when the rendered taskbar would look identical.
#[derive(Clone, Copy, PartialEq, Eq)]
struct EntrySnapshot {
    surface_id: SurfaceId,
    /// `len` valid bytes in `title_prefix`, capped at `CACHED_TITLE_BYTES`.
    title_len: u8,
    title_prefix: [u8; CACHED_TITLE_BYTES],
}

impl EntrySnapshot {
    const fn empty() -> Self {
        Self {
            surface_id: SurfaceId::NONE,
            title_len: 0,
            title_prefix: [0; CACHED_TITLE_BYTES],
        }
    }

    fn from_title(id: SurfaceId, title: &SurfaceTitle) -> Self {
        let bytes = title.as_bytes();
        let cut = if bytes.len() > CACHED_TITLE_BYTES {
            CACHED_TITLE_BYTES
        } else {
            bytes.len()
        };
        let mut title_prefix = [0u8; CACHED_TITLE_BYTES];
        title_prefix[..cut].copy_from_slice(&bytes[..cut]);
        Self {
            surface_id: id,
            title_len: cut as u8,
            title_prefix,
        }
    }

    fn title_bytes(&self) -> &[u8] {
        &self.title_prefix[..self.title_len as usize]
    }
}

/// Full per-frame snapshot of inputs that affect the taskbar's pixels.
#[derive(Clone, Copy, PartialEq, Eq)]
struct TaskbarSnapshot {
    focused: SurfaceId,
    total_count: u32,
    entry_count: u8,
    entries: [EntrySnapshot; MAX_TASKBAR_ENTRIES],
}

impl TaskbarSnapshot {
    const fn empty() -> Self {
        Self {
            focused: SurfaceId::NONE,
            total_count: 0,
            entry_count: 0,
            entries: [EntrySnapshot::empty(); MAX_TASKBAR_ENTRIES],
        }
    }
}

// ---------------------------------------------------------------------------
// Taskbar state
// ---------------------------------------------------------------------------

struct TaskbarState {
    surface_id: SurfaceId,
    shmem_id: SharedMemoryId,
    /// Direct-map VA of the surface's backing buffer.
    buffer_va: usize,
    /// Capacity of the buffer in u32 pixels.
    buffer_pixels: usize,
    /// Surface width in pixels (== display width at init).
    width: u32,
    /// Surface height in pixels (== `TASKBAR_HEIGHT`).
    height: u32,
    /// Last rendered snapshot; redraw is skipped when the next snapshot
    /// matches.
    cached_snapshot: TaskbarSnapshot,
    /// `true` until the first render — forces an unconditional first draw.
    needs_first_render: bool,
}

/// Lock ordering: leaf — never co-held with `SURFACE_TABLE`,
/// `SHARED_REGION_TABLE`, `FOCUS_MANAGER`, or any IPC mutex. `tick()`
/// snapshots inputs (releasing those locks) before taking this mutex.
static TASKBAR: Mutex<Option<TaskbarState>> = Mutex::new(None);

/// Set after `init` populates `TASKBAR`. Lets `tick` skip the mutex on
/// the no-op fast path before the shell is up.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Allocate the Taskbar surface and seed its backing buffer.
///
/// Called by `super::init_shell_surfaces` after the Status Strip is up.
/// Mirrors `status_strip::init`: shmem create → cache direct-map VA →
/// fill background → surface_create → attach buffer → cache state →
/// log a single boot line.
pub(super) fn init(display_width: u32, display_height: u32) -> Result<(), ShellError> {
    if display_width == 0 || display_height == 0 {
        return Err(ShellError::NoDisplay);
    }

    let pixel_count = (display_width as usize) * (TASKBAR_HEIGHT as usize);
    let byte_count = pixel_count * BYTES_PER_PIXEL;

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
    // covers exactly `pixel_count` u32s and lives for the lifetime of the
    // shell (regions are never freed once the shell is up).
    // Maintained by: only the taskbar renderer writes through this VA;
    // shmem region lifetime is permanent for the compositor process.
    // Violation: writing past `pixel_count` would corrupt adjacent shmem
    // pages or the next allocation in the user pool.
    let buffer = unsafe { core::slice::from_raw_parts_mut(buffer_va as *mut u32, pixel_count) };
    buffer.fill(TASKBAR_BG);

    let channel = match *crate::compositor::service::COMPOSITOR_CHANNEL.lock() {
        Some(ch) => ch,
        None => return Err(ShellError::SurfaceCreate),
    };
    let title = SurfaceTitle::from_bytes(b"taskbar");
    let surface_id = surface_create(
        ProcessId(10),
        channel,
        display_width,
        TASKBAR_HEIGHT,
        title,
        SurfaceContentType::SystemUI,
        SurfaceLayer::Panel,
    )
    .map_err(|_| ShellError::SurfaceCreate)?;

    surface_attach_buffer(
        surface_id,
        shmem_id,
        DamageRegion::FullSurface,
        ProcessId(10),
    )
    .map_err(|_| ShellError::AttachBuffer)?;

    // Position the surface against the bottom edge.
    let y = (display_height as i32) - (TASKBAR_HEIGHT as i32);
    surface_set_position(surface_id, 0, y, ProcessId(10)).map_err(|_| ShellError::SurfaceCreate)?;

    let state = TaskbarState {
        surface_id,
        shmem_id,
        buffer_va,
        buffer_pixels,
        width: display_width,
        height: TASKBAR_HEIGHT,
        cached_snapshot: TaskbarSnapshot::empty(),
        needs_first_render: true,
    };
    *TASKBAR.lock() = Some(state);
    INITIALIZED.store(true, Ordering::Release);

    crate::kinfo!(
        Compositor,
        "shell: taskbar surface={} created ({}x{})",
        surface_id.0,
        display_width,
        TASKBAR_HEIGHT
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Tick — invoked once per compositor loop iteration
// ---------------------------------------------------------------------------

/// Rebuild the taskbar's snapshot of visible surfaces and redraw if it
/// differs from the last rendered snapshot. The Taskbar is **damage-driven
/// only**: there is no time-based cadence — calling `tick` more often is
/// harmless because the snapshot comparison short-circuits.
///
/// Lock sequence:
///   1. `FOCUS_MANAGER` (leaf) — read keyboard focus, drop.
///   2. `SURFACE_TABLE` — walk once to build the snapshot, drop.
///   3. `TASKBAR` (leaf) — compare-cache, render in-place if dirty, drop.
///   4. `SURFACE_TABLE` again (via `surface_attach_buffer` + `mark_damaged`).
///
/// No two of these locks are ever co-held; lock-ordering chain
/// `SURFACE_TABLE > leaves` is honored.
pub(super) fn tick(_now_ms: u64) {
    if !INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    // 1. Snapshot focus before touching SURFACE_TABLE — FOCUS_MANAGER is
    //    a leaf so we drop it before climbing the lock chain.
    let focused = FOCUS_MANAGER
        .lock()
        .keyboard_focus()
        .unwrap_or(SurfaceId::NONE);

    // 2. Walk SURFACE_TABLE once to build the entry snapshot. We hold
    //    SURFACE_TABLE briefly here — no IPC, no kinfo!, no other lock.
    let snapshot = {
        let table = SURFACE_TABLE.lock();
        build_snapshot(&table[..], focused)
    };

    // 3. Compare against the cached snapshot under the leaf TASKBAR
    //    mutex. Skip the redraw if nothing changed and we've already
    //    drawn the first frame.
    let mut guard = TASKBAR.lock();
    let tb = match guard.as_mut() {
        Some(s) => s,
        None => return,
    };

    // Step 29: route the cache-vs-snapshot comparison through the
    // shared `should_redraw_shell` helper. When this returns false
    // we skip rendering AND skip re-attaching, leaving the surface's
    // `damaged` flag unset so `compose_frame` skips this surface
    // entirely on the next frame (idle-frame fast path).
    let snapshot_changed = tb.cached_snapshot != snapshot;
    if !shared::compositor::should_redraw_shell(tb.needs_first_render, snapshot_changed) {
        return;
    }

    tb.cached_snapshot = snapshot;
    tb.needs_first_render = false;

    let layout = compute_taskbar_layout(tb.width, snapshot.entry_count as usize);

    // SAFETY: `buffer_va` was captured at init from `region_dmap_addr`;
    // shmem regions are never freed for compositor-internal surfaces, so
    // the address remains valid for the lifetime of the kernel. Slice
    // length is bounded by `buffer_pixels` (sized at init).
    // Maintained by: shell surface lifecycle — `init` allocates once, no
    // teardown path exists in M26.
    // Violation: writing past `buffer_pixels` would corrupt user-pool
    // pages adjacent to the shmem region.
    let buffer =
        unsafe { core::slice::from_raw_parts_mut(tb.buffer_va as *mut u32, tb.buffer_pixels) };
    let surface_id = tb.surface_id;
    let shmem_id = tb.shmem_id;
    let width = tb.width;
    let height = tb.height;
    let snapshot_copy = tb.cached_snapshot;

    render_frame(buffer, width, height, &layout, &snapshot_copy);

    // Drop the TASKBAR mutex before re-acquiring SURFACE_TABLE for damage
    // marking — keeps the lock chain (SURFACE_TABLE > leaves) honest.
    drop(guard);

    let _ = surface_attach_buffer(
        surface_id,
        shmem_id,
        DamageRegion::FullSurface,
        ProcessId(10),
    );
    mark_damaged(surface_id);
}

/// Build a `TaskbarSnapshot` from the current `SURFACE_TABLE` contents.
///
/// Filters out shell surfaces (`Surface::is_shell`) and non-visible
/// states (anything but `Active`). Caps the number of cached entries at
/// `MAX_TASKBAR_ENTRIES`; surplus surfaces still count toward
/// `total_count` (rendered as the right-hand "N windows" readout).
fn build_snapshot(table: &[Option<Surface>], focused: SurfaceId) -> TaskbarSnapshot {
    let mut snapshot = TaskbarSnapshot {
        focused,
        total_count: 0,
        entry_count: 0,
        entries: [EntrySnapshot::empty(); MAX_TASKBAR_ENTRIES],
    };

    for slot in table.iter() {
        let surface = match slot.as_ref() {
            Some(s) => s,
            None => continue,
        };
        if surface.is_shell() {
            continue;
        }
        if !surface.state.is_visible() {
            continue;
        }
        snapshot.total_count += 1;
        let idx = snapshot.entry_count as usize;
        if idx < MAX_TASKBAR_ENTRIES {
            snapshot.entries[idx] = EntrySnapshot::from_title(surface.id, &surface.title);
            snapshot.entry_count += 1;
        }
    }

    snapshot
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

/// Fully repaint the taskbar buffer from a snapshot.
///
/// `buffer.len()` must be at least `width * height`. Entries beyond
/// `layout.visible_entries` are not drawn; the surplus contributes only
/// to the `N windows` count text.
fn render_frame(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    layout: &TaskbarLayout,
    snap: &TaskbarSnapshot,
) {
    if buffer.len() < (width as usize) * (height as usize) {
        return;
    }

    // Background.
    fill_rect(buffer, width, height, 0, 0, width, height, TASKBAR_BG);

    // Workspace button.
    render_workspace_button(buffer, width, height, &layout.workspace_button);

    // Entry list (only the layout-visible prefix).
    let entry_limit = layout
        .visible_entries
        .min(snap.entry_count as usize)
        .min(MAX_TASKBAR_ENTRIES);
    for i in 0..entry_limit {
        let cell = layout.entries[i];
        let entry = &snap.entries[i];
        let focused = entry.surface_id == snap.focused && !snap.focused.is_none();
        render_entry(buffer, width, height, cell, entry, focused);
    }

    // Surface count readout.
    render_count(buffer, width, height, &layout.count_cell, snap.total_count);
}

fn render_workspace_button(buffer: &mut [u32], dst_w: u32, dst_h: u32, cell: &TaskbarCell) {
    fill_rect(
        buffer,
        dst_w,
        dst_h,
        cell.x,
        0,
        cell.width,
        TASKBAR_HEIGHT,
        WORKSPACE_BG,
    );
    // Center [W] (3 glyphs) horizontally, vertically inside the strip.
    let label = b"[W]";
    let label_px_w = label.len() as i32 * TITLE_GLYPH_WIDTH;
    let text_x = cell.x + ((cell.width as i32 - label_px_w) / 2);
    let text_y = (TASKBAR_HEIGHT as i32 - TITLE_GLYPH_HEIGHT) / 2;
    let max_x = cell.x + cell.width as i32;
    draw_text_clipped(
        buffer,
        dst_w,
        dst_h,
        text_x,
        text_y,
        max_x,
        label,
        TEXT_FG,
        WORKSPACE_BG,
    );
}

fn render_entry(
    buffer: &mut [u32],
    dst_w: u32,
    dst_h: u32,
    cell: TaskbarCell,
    entry: &EntrySnapshot,
    focused: bool,
) {
    let bg = if focused {
        ENTRY_BG_FOCUSED
    } else {
        ENTRY_BG_UNFOCUSED
    };
    fill_rect(
        buffer,
        dst_w,
        dst_h,
        cell.x,
        0,
        cell.width,
        TASKBAR_HEIGHT,
        bg,
    );

    let trimmed = taskbar_entry_truncate(entry.title_bytes(), ENTRY_TITLE_MAX_CHARS);
    if trimmed.is_empty() {
        return;
    }
    let text_x = cell.x + ENTRY_PADDING_X;
    let text_y = (TASKBAR_HEIGHT as i32 - TITLE_GLYPH_HEIGHT) / 2;
    let max_x = cell.x + cell.width as i32 - ENTRY_PADDING_X;
    draw_text_clipped(
        buffer, dst_w, dst_h, text_x, text_y, max_x, trimmed, TEXT_FG, bg,
    );
}

fn render_count(buffer: &mut [u32], dst_w: u32, dst_h: u32, cell: &TaskbarCell, count: u32) {
    fill_rect(
        buffer,
        dst_w,
        dst_h,
        cell.x,
        0,
        cell.width,
        TASKBAR_HEIGHT,
        TASKBAR_BG,
    );
    // "N windows" — N saturates at 9 to keep the layout single-digit; the
    // total_count field still reflects the true number for diagnostics.
    let display_n = if count > 9 { 9 } else { count };
    let mut text = [0u8; 9];
    text[0] = b'0' + display_n as u8;
    text[1..9].copy_from_slice(b" windows");
    let text_w = text.len() as i32 * TITLE_GLYPH_WIDTH;
    let text_x = cell.x + cell.width as i32 - text_w - ENTRY_PADDING_X;
    let text_y = (TASKBAR_HEIGHT as i32 - TITLE_GLYPH_HEIGHT) / 2;
    let max_x = cell.x + cell.width as i32 - ENTRY_PADDING_X;
    draw_text_clipped(
        buffer, dst_w, dst_h, text_x, text_y, max_x, &text, TEXT_FG, TASKBAR_BG,
    );
}

// ---------------------------------------------------------------------------
// Input routing (M26 Step 27)
// ---------------------------------------------------------------------------

/// Returns the SurfaceId of the Taskbar surface, or `None` before init.
///
/// Used by `super::route_pointer` to identify whether a pointer event
/// targets the taskbar surface.
pub fn surface_id() -> Option<SurfaceId> {
    if !INITIALIZED.load(Ordering::Acquire) {
        return None;
    }
    TASKBAR.lock().as_ref().map(|tb| tb.surface_id)
}

/// Handle a pointer event that resolved to the Taskbar surface.
///
/// Only acts on **left-button press** transitions: a click on the
/// workspace cell toggles the Workspace; a click on an entry cell
/// focuses that surface and raises it to the top of `WINDOW_Z_ORDER`.
/// Releases, motion, and right/middle clicks are silently consumed
/// because the Taskbar is non-pass-through (Panel layer).
///
/// Lock sequence: `TASKBAR` (leaf) — snapshot layout + entries, drop —
/// then call into `workspace::toggle_visibility` (which manages its own
/// locks), or `set_keyboard_focus_safe` + `WINDOW_Z_ORDER` (kernel
/// chain). No lock held across the toggle / focus call.
pub fn handle_pointer(event: &InputEvent) {
    let (px, py) = match event {
        InputEvent::Pointer {
            x,
            y,
            button: Some(MouseButton::Left),
            state: Some(ButtonState::Pressed),
        } => (*x as i32, *y as i32),
        _ => return,
    };

    // Snapshot layout + entries under the leaf TASKBAR mutex.
    let (layout, entry_ids) = {
        let guard = TASKBAR.lock();
        let tb = match guard.as_ref() {
            Some(s) => s,
            None => return,
        };
        let layout = compute_taskbar_layout(tb.width, tb.cached_snapshot.entry_count as usize);
        let mut ids = [SurfaceId::NONE; MAX_TASKBAR_ENTRIES];
        let count = tb.cached_snapshot.entry_count as usize;
        for (slot, snap) in ids
            .iter_mut()
            .zip(tb.cached_snapshot.entries.iter())
            .take(count)
        {
            *slot = snap.surface_id;
        }
        (layout, ids)
    };

    // Translate screen coords to surface-local coords. The taskbar's
    // x is pinned to 0 (full display width), so local_x == screen x.
    // local_y subtracts the surface's pinned screen-y (looked up under
    // SURFACE_TABLE; never co-held with our leaf mutex above).
    let surface_y = match lookup_surface_y() {
        Some(y) => y,
        None => return,
    };
    let local_x = px;
    let local_y = py - surface_y;

    let action = taskbar_pointer_action(&layout, &entry_ids, local_x, local_y);
    match action {
        Some(TaskbarPointerAction::WorkspaceToggle) => {
            crate::compositor::shell::workspace::toggle_visibility();
        }
        Some(TaskbarPointerAction::FocusEntry(id)) => {
            crate::kinfo!(Compositor, "taskbar: focus -> surface={}", id.0);
            super::super::input_route::set_keyboard_focus_safe(Some(id));
            // Raise the focused surface to the top of its layer.
            let mut z = super::super::window::WINDOW_Z_ORDER.lock();
            z.raise_to_top(id);
        }
        None => {}
    }
}

/// Look up the Taskbar surface's screen `y` coordinate from
/// `SURFACE_TABLE`. Returns `None` before init or if the surface has
/// been destroyed (which never happens in M26 — there is no shell
/// teardown path).
fn lookup_surface_y() -> Option<i32> {
    let id = surface_id()?;
    let table = SURFACE_TABLE.lock();
    table
        .iter()
        .filter_map(|s| s.as_ref())
        .find(|s| s.id == id)
        .map(|s| s.y)
}
