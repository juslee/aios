//! Status Strip — top-edge Panel-layer surface (M26 Step 24).
//!
//! 32-pixel-tall surface locked to `y == 0`, spanning the full display
//! width. Renders the live system status as
//! `AIOS  HH:MM  CPU: N/A  MEM: NN%  CORES: N` using the spleen 8×16 font
//! shared with window decorations. Refreshes once per second; redraws are
//! skipped when none of the cached display values changed since the last
//! tick (damage optimization).
//!
//! The Status Strip is **compositor-internal**: it is owned by
//! `ProcessId(10)` and registered in `SURFACE_TABLE` with the well-known
//! compositor channel. `service::is_self_channel` already suppresses IPC
//! delivery onto our own receive queue, so the shell surface never receives
//! events even though it appears in the surface table.
//!
//! Per docs/experience/experience.md §6.

use core::sync::atomic::{AtomicBool, Ordering};

use shared::compositor::{
    format_hhmm, format_percent_2digits, format_u32_left4, DamageRegion, SurfaceContentType,
    SurfaceId, SurfaceLayer, SurfaceTitle,
};
use shared::ipc::SharedMemoryId;
use spin::Mutex;

use crate::compositor::surface::{mark_damaged, surface_attach_buffer, surface_create};
use crate::compositor::text::draw_text_clipped;
use crate::ipc::shmem::{region_dmap_addr, region_size, shared_memory_create};
use crate::mm::frame::FRAME_ALLOC;
use crate::mm::pgtable::VmFlags;
use crate::task::process::ProcessId;

use super::ShellError;

// ---------------------------------------------------------------------------
// Layout & colors
// ---------------------------------------------------------------------------

/// Fixed Status Strip height in pixels (matches phase doc §M26 Step 24).
pub const STRIP_HEIGHT: u32 = 32;

/// Background color for the Status Strip (B8G8R8A8). A dark slate that
/// reads as "system chrome" against both light and dark client surfaces.
const STRIP_BG: u32 = 0xFF14_1923;
/// Foreground (text) color.
const STRIP_FG: u32 = 0xFFEC_F0F8;

/// Vertical glyph baseline inside the 32-pixel strip — leaves 8 px padding
/// above and 8 below the 16-pixel-tall spleen glyphs.
const TEXT_Y: i32 = 8;
/// Left margin before the first glyph.
const TEXT_X: i32 = 8;

/// Update cadence for the Status Strip in milliseconds (1 Hz).
const TICK_INTERVAL_MS: u64 = 1000;

/// Bytes-per-pixel for the back-buffer (B8G8R8A8 = 4).
const BYTES_PER_PIXEL: usize = 4;

/// Sentinel value meaning "no value cached yet" for cores. Real core counts
/// are always 1..=`smp::MAX_CORES` so `u32::MAX` is unambiguous.
const CORES_UNSET: u32 = u32::MAX;

// ---------------------------------------------------------------------------
// Shell state — kept behind a single Mutex
// ---------------------------------------------------------------------------

/// Cached, per-tick render state for the Status Strip.
///
/// `cached_*` fields hold the most recent snapshot of inputs that affect
/// the rendered glyphs; `tick()` redraws only when at least one differs
/// from the previously rendered value. The first redraw is forced because
/// `last_render_tick == 0` while `now_ms > 0`.
struct StatusStripState {
    /// SurfaceId allocated during `init`; `NONE` while uninitialized.
    surface_id: SurfaceId,
    /// SharedMemoryId for the surface's backing buffer.
    shmem_id: SharedMemoryId,
    /// Cached direct-map virtual address of `shmem_id`'s pages.
    buffer_va: usize,
    /// Capacity in pixels of `buffer_va`'s backing region.
    buffer_pixels: usize,
    /// Strip width (== display width at init time).
    width: u32,
    /// Strip height (== `STRIP_HEIGHT`).
    height: u32,
    /// Wall-clock tick at the most recent successful redraw.
    last_render_tick: u64,
    /// Last rendered HH:MM bytes.
    cached_time: [u8; 5],
    /// Last rendered memory percent.
    cached_mem_pct: u32,
    /// Last rendered core count.
    cached_cores: u32,
}

/// Lock ordering: this Mutex is a leaf — never held while acquiring
/// `SURFACE_TABLE`, `SHARED_REGION_TABLE`, `FRAME_ALLOC`, or any IPC lock.
/// `tick()` snapshots the metric inputs (releasing those locks) BEFORE
/// taking this lock to render.
static STATUS_STRIP: Mutex<Option<StatusStripState>> = Mutex::new(None);

/// `true` once `init` has populated `STATUS_STRIP`. Read by `tick` to
/// avoid acquiring the mutex on the fast no-op path.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Allocate the Status Strip surface and its backing buffer.
///
/// Sequence (lock-ordering compliant — `SHARED_REGION_TABLE > SURFACE_TABLE`):
///   1. Allocate a shmem region sized for `display_width × STRIP_HEIGHT × 4`.
///   2. Snapshot the direct-map VA of the region (for kernel-side writes).
///   3. Fill the buffer with the dark background (initial visual state).
///   4. Create the surface with `Panel` layer, `SystemUI` content type.
///   5. Attach the buffer (transitions Created → Active).
///   6. Cache state inside `STATUS_STRIP` and flip `INITIALIZED` to true.
///   7. Log a single boot line so the verifier can confirm the surface ID.
pub(super) fn init(display_width: u32) -> Result<(), ShellError> {
    if display_width == 0 {
        return Err(ShellError::NoDisplay);
    }

    let pixel_count = (display_width as usize) * (STRIP_HEIGHT as usize);
    let byte_count = pixel_count * BYTES_PER_PIXEL;

    // Step 1: allocate the shmem region. The compositor process holds
    // `SharedMemoryCreate` (granted in `init_compositor`), so this never
    // fails on the capability path.
    let shmem_id = shared_memory_create(
        ProcessId(10),
        byte_count,
        VmFlags::READ.union(VmFlags::WRITE),
    )
    .map_err(|_| ShellError::ShmemCreate)?;

    // Step 2: snapshot the direct-map VA. `region_dmap_addr` and
    // `region_size` each acquire `SHARED_REGION_TABLE` briefly; we drop
    // before any subsequent surface call.
    let buffer_va = region_dmap_addr(shmem_id).ok_or(ShellError::ShmemCreate)?;
    let buffer_bytes = region_size(shmem_id).ok_or(ShellError::ShmemCreate)?;
    let buffer_pixels = buffer_bytes / BYTES_PER_PIXEL;
    if buffer_pixels < pixel_count {
        return Err(ShellError::ShmemCreate);
    }

    // Step 3: fill the buffer. Direct-map pages are part of the global
    // direct map (DIRECT_MAP_BASE-rooted), backed by physically contiguous
    // RAM allocated above; the compositor service thread is the sole
    // writer for the lifetime of the strip.
    // SAFETY: `buffer_va` is the direct-map address of a freshly-allocated
    // shmem region of `byte_count` bytes (verified above). The slice
    // covers exactly `pixel_count` u32s and lives for the lifetime of the
    // shell (regions are never freed once the shell is up).
    // Maintained by: only the shell renderer writes through this VA;
    // shmem region lifetime is permanent for the compositor process.
    // Violation: writing past `pixel_count` would corrupt adjacent shmem
    // pages or the next allocation in the user pool.
    let buffer = unsafe { core::slice::from_raw_parts_mut(buffer_va as *mut u32, pixel_count) };
    buffer.fill(STRIP_BG);

    // Step 4: register the surface. `surface_create` acquires
    // `SURFACE_TABLE` briefly; lock ordering allows this after the shmem
    // path (`SHARED_REGION_TABLE > SURFACE_TABLE`).
    let channel = match *crate::compositor::service::COMPOSITOR_CHANNEL.lock() {
        Some(ch) => ch,
        None => return Err(ShellError::SurfaceCreate),
    };
    let title = SurfaceTitle::from_bytes(b"status-strip");
    let surface_id = surface_create(
        ProcessId(10),
        channel,
        display_width,
        STRIP_HEIGHT,
        title,
        SurfaceContentType::SystemUI,
        SurfaceLayer::Panel,
    )
    .map_err(|_| ShellError::SurfaceCreate)?;

    // Step 5: attach the buffer (transitions Created → Active).
    surface_attach_buffer(
        surface_id,
        shmem_id,
        DamageRegion::FullSurface,
        ProcessId(10),
    )
    .map_err(|_| ShellError::AttachBuffer)?;

    // Step 6: cache the state. The `(0,0)` surface position is the default
    // assigned by `surface_create`, which is exactly what the Panel layer
    // wants for the top-edge strip — no follow-up move is needed.
    let state = StatusStripState {
        surface_id,
        shmem_id,
        buffer_va,
        buffer_pixels,
        width: display_width,
        height: STRIP_HEIGHT,
        last_render_tick: 0,
        cached_time: *b"--:--",
        cached_mem_pct: u32::MAX,
        cached_cores: CORES_UNSET,
    };
    *STATUS_STRIP.lock() = Some(state);
    INITIALIZED.store(true, Ordering::Release);

    // Step 7: log the surface ID for verification. Mutexes are released
    // above so `kinfo!` (which can route through the log ring) is safe.
    crate::kinfo!(
        Compositor,
        "shell: status-strip surface={} created ({}x{})",
        surface_id.0,
        display_width,
        STRIP_HEIGHT
    );

    Ok(())
}

/// Returns the SurfaceId of the Status Strip surface, or `None` before init.
///
/// Used by `super::route_pointer` to identify whether a pointer event
/// targets the status-strip surface (so the dispatcher can drop the
/// click — Status Strip is non-interactive in M26 per phase doc).
pub fn surface_id() -> Option<SurfaceId> {
    if !INITIALIZED.load(Ordering::Acquire) {
        return None;
    }
    STATUS_STRIP.lock().as_ref().map(|s| s.surface_id)
}

// ---------------------------------------------------------------------------
// Tick — invoked once per compositor loop iteration
// ---------------------------------------------------------------------------

/// Re-render the Status Strip if any displayed value changed AND at least
/// `TICK_INTERVAL_MS` has elapsed since the last redraw.
///
/// All metric snapshotting (memory %, core count, current time) runs
/// before the strip's mutex is taken, so we never co-hold this lock with
/// `FRAME_ALLOC` or the SMP counters.
pub(super) fn tick(now_ms: u64) {
    if !INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    // Snapshot inputs first — release every other mutex before touching
    // the Status Strip's own mutex.
    let time_bytes = format_hhmm(now_ms);
    let mem_pct = sample_memory_percent_used();
    let cores = sample_core_count();

    let mut guard = STATUS_STRIP.lock();
    let strip = match guard.as_mut() {
        Some(s) => s,
        None => return,
    };

    let due_for_redraw = now_ms.saturating_sub(strip.last_render_tick) >= TICK_INTERVAL_MS
        || strip.last_render_tick == 0;
    if !due_for_redraw {
        return;
    }

    let values_changed = strip.cached_time != time_bytes
        || strip.cached_mem_pct != mem_pct
        || strip.cached_cores != cores;
    if !values_changed && strip.last_render_tick != 0 {
        // Nothing changed — keep the previous frame; bump the tick so the
        // next redraw check is still cadence-limited.
        strip.last_render_tick = now_ms;
        return;
    }

    // Update cached values BEFORE rendering so a partial render leaves the
    // cache consistent with what landed in the buffer.
    strip.cached_time = time_bytes;
    strip.cached_mem_pct = mem_pct;
    strip.cached_cores = cores;
    strip.last_render_tick = now_ms;

    // SAFETY: `buffer_va` was captured at init from `region_dmap_addr`;
    // shmem regions are never freed for compositor-internal surfaces, so
    // the address remains valid for the lifetime of the kernel. The
    // slice length is bounded by `buffer_pixels` (sized at init >= the
    // visible surface area).
    // Maintained by: shell surface lifecycle — `init` allocates once, no
    // teardown path exists in M26.
    // Violation: writing past `buffer_pixels` would corrupt user-pool
    // pages adjacent to the shmem region.
    let buffer = unsafe {
        core::slice::from_raw_parts_mut(strip.buffer_va as *mut u32, strip.buffer_pixels)
    };
    let surface_id = strip.surface_id;
    let shmem_id = strip.shmem_id;
    let width = strip.width;
    let height = strip.height;

    render_strip(buffer, width, height, &time_bytes, mem_pct, cores);

    // Drop the strip mutex before re-attaching the buffer (which takes
    // SURFACE_TABLE) and marking damage (also SURFACE_TABLE).
    drop(guard);

    // Re-attach the same buffer with full-surface damage so the (future)
    // present pipeline picks up the fresh content. Idempotent because the
    // surface is already Active.
    let _ = surface_attach_buffer(
        surface_id,
        shmem_id,
        DamageRegion::FullSurface,
        ProcessId(10),
    );
    mark_damaged(surface_id);
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

/// Render the strip's contents into `buffer`.
///
/// `buffer.len()` must be at least `width * height` u32s — guaranteed by
/// `init` because the shmem region was sized accordingly.
fn render_strip(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    time: &[u8; 5],
    mem_pct: u32,
    cores: u32,
) {
    if buffer.len() < (width as usize) * (height as usize) {
        return;
    }
    // Background.
    for px in buffer.iter_mut().take((width as usize) * (height as usize)) {
        *px = STRIP_BG;
    }

    let mut x = TEXT_X;
    let max_x = width as i32;

    // "AIOS  "
    x = draw_run(buffer, width, height, x, max_x, b"AIOS  ");
    // Time "HH:MM  "
    x = draw_run(buffer, width, height, x, max_x, time);
    x = draw_run(buffer, width, height, x, max_x, b"  ");
    // CPU: N/A is rendered literally — no scheduler util metric exists yet.
    x = draw_run(buffer, width, height, x, max_x, b"CPU: N/A  ");
    // MEM: NN%
    x = draw_run(buffer, width, height, x, max_x, b"MEM: ");
    let mem_digits = format_percent_2digits(mem_pct);
    x = draw_run(buffer, width, height, x, max_x, &mem_digits);
    x = draw_run(buffer, width, height, x, max_x, b"%  ");
    // CORES: N — `format_u32_left4` left-aligns into a 4-byte fixed-width
    // slot so the field width stays stable as the count changes (1, 2, 4).
    x = draw_run(buffer, width, height, x, max_x, b"CORES: ");
    let core_digits = format_u32_left4(cores);
    let _ = draw_run(buffer, width, height, x, max_x, &core_digits);
}

/// Draw one ASCII-byte run starting at `start_x`, returning the next x
/// cursor. Wrapper around `draw_text_clipped` that increments the cursor
/// using the spleen 8×16 cell width.
fn draw_run(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    start_x: i32,
    max_x: i32,
    text: &[u8],
) -> i32 {
    draw_text_clipped(
        buffer, width, height, start_x, TEXT_Y, max_x, text, STRIP_FG, STRIP_BG,
    );
    start_x + (text.len() as i32) * crate::compositor::text::TITLE_GLYPH_WIDTH
}

// ---------------------------------------------------------------------------
// Metric sampling
// ---------------------------------------------------------------------------

/// Sample the system-wide memory utilization percent (used / total).
///
/// Aggregates `total_pages` and `total_free_pages` across every initialized
/// pool (kernel, user, model, dma). Returns `0` if the frame allocator
/// isn't installed yet (caller renders `00%`). The mutex on `FRAME_ALLOC`
/// is dropped before this function returns so the strip mutex can be
/// taken without lock-ordering risk.
fn sample_memory_percent_used() -> u32 {
    let guard = FRAME_ALLOC.lock();
    let fa = match guard.as_ref() {
        Some(fa) => fa,
        None => return 0,
    };
    let total = fa.total_pages();
    let free = fa.total_free_pages();
    drop(guard);
    if total == 0 {
        return 0;
    }
    let used = total.saturating_sub(free);
    let pct = (used as u64 * 100) / (total as u64);
    pct.min(99) as u32
}

/// Sample the live core count (`smp::online_cpus()`).
fn sample_core_count() -> u32 {
    crate::smp::online_cpus() as u32
}
