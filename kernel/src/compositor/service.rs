//! Compositor service — the kernel-side service process that drives the display.
//!
//! Mirrors the GPU Service pattern (`kernel/src/gpu/service.rs`):
//!   * dedicated kernel process (`ProcessId(10)`, name "compositor")
//!   * dedicated IPC channel registered as service "compositor"
//!   * Interactive scheduler class, full CPU affinity
//!   * thread entry unmasks IRQs then runs the main loop
//!
//! M24 wires the service skeleton. The main loop currently waits for IPC
//! messages and replies with a placeholder — surface dispatch (Step 12) and
//! the composition loop (Step 14) hang off this same loop in subsequent steps.
//!
//! Per docs/platform/compositor.md §2.

use core::sync::atomic::Ordering;

use shared::compositor::{
    CompositorCommand, CompositorEvent, CompositorRequest, DamageTracker, SurfaceContentType,
    SurfaceId, SurfaceLayer, SurfaceTitle,
};
use shared::gpu::{DisplayInfo, GpuBufferHandle, GpuError, VirtioGpuRect, AIOS_BLUE_B8G8R8A8};
use shared::ipc::{ChannelId, SharedMemoryId};
use spin::Mutex;

use crate::arch::aarch64::timer::TICK_COUNT;
use crate::compositor::focus::FOCUS_MANAGER;
use crate::compositor::input_route;
use crate::compositor::render;
use crate::compositor::shell;
use crate::compositor::surface::{self, SurfaceError, SURFACE_TABLE};
use crate::compositor::window::WINDOW_Z_ORDER;
use crate::drivers::virtio_gpu;
use crate::ipc;
use crate::service;
use crate::task::process::ProcessId;
use crate::task::ThreadId;

// ---------------------------------------------------------------------------
// Compositor service state
// ---------------------------------------------------------------------------

/// Compositor service runtime state, held inside the service loop.
///
/// Step 11 adds the composition buffers and display info so the service
/// owns its own front/back framebuffers post-handoff. Surface tracking
/// and the damage tracker arrive in Steps 12 and 13.
struct CompositorState {
    /// IPC channel for this service.
    ///
    /// Owned by `CompositorState` so the service is self-contained;
    /// `compositor_loop` uses the local `ch` binding from
    /// `COMPOSITOR_CHANNEL` for the receive loop. Read by Step 17 input
    /// routing once per-client channels arrive in M26.
    #[allow(dead_code)]
    channel: ChannelId,
    /// Display geometry as reported by the VirtIO-GPU driver.
    display: DisplayInfo,
    /// Front buffer — currently scanned out.
    front_buffer: Option<GpuBufferHandle>,
    /// Back buffer — render target for the next frame.
    back_buffer: Option<GpuBufferHandle>,
    /// Tick at which the most recent frame was composed (for 60fps pacing).
    last_frame_tick: u64,
    /// Total frames composed since boot (for periodic stats logging).
    frame_count: u64,
    /// Sum of per-frame compose-time in milliseconds; resets every 60 frames.
    frame_ms_accum: u64,
    /// Per-frame screen-space damage accumulator.
    damage: DamageTracker,
    /// `true` until the first post-handoff frame is presented (forces a clear).
    needs_initial_clear: bool,
}

impl CompositorState {
    fn new(channel: ChannelId, display: DisplayInfo) -> Self {
        Self {
            channel,
            display,
            front_buffer: None,
            back_buffer: None,
            last_frame_tick: 0,
            frame_count: 0,
            frame_ms_accum: 0,
            damage: DamageTracker::new(),
            needs_initial_clear: true,
        }
    }
}

/// Frame budget in 1 kHz ticks — 16 ticks ≈ 60fps.
const FRAME_BUDGET_TICKS: u64 = 16;
/// Watchdog threshold — log a warning if any frame's compose+present
/// exceeds 100ms.
const FRAME_WATCHDOG_MS: u64 = 100;
/// How often to emit aggregated frame timing stats (in frames).
const STATS_EVERY_FRAMES: u64 = 60;

// ---------------------------------------------------------------------------
// Compositor service thread entry
// ---------------------------------------------------------------------------

/// Compositor service thread entry point.
///
/// Unmasks IRQs (required for IPC timeouts and scheduling preemption),
/// initializes service state, and enters the main loop. Never returns.
pub fn compositor_entry() -> ! {
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit only. Safe at EL1.
    // Required for timer-driven preemption + IPC timeouts on this core.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    compositor_loop();
}

/// Compositor main loop — receives IPC requests, replies, runs the
/// composition step on a 16ms cadence (60fps). Step 10 ships a stub that
/// just acknowledges every message; Step 12 wires real surface dispatch
/// and Step 14 plugs in the frame pacer.
fn compositor_loop() -> ! {
    // Wait for the channel to be installed by `init_compositor`.
    let ch = loop {
        if let Some(ch) = *COMPOSITOR_CHANNEL.lock() {
            break ch;
        }
        crate::sched::thread_yield();
    };

    crate::kinfo!(Compositor, "Compositor: started, channel={}", ch.0);

    let display = virtio_gpu::display_info().unwrap_or_else(DisplayInfo::default);
    let mut state = CompositorState::new(ch, display);

    // Take ownership of the display from the GPU Service.
    let mut display_owned = false;
    if display.width > 0 && display.height > 0 {
        match display_handoff(&mut state) {
            Ok(()) => display_owned = true,
            Err(e) => {
                crate::kerror!(
                    Compositor,
                    "Compositor: display handoff failed ({:?}); display will remain owned by GPU Service",
                    e
                );
            }
        }
    } else {
        crate::kwarn!(
            Compositor,
            "Compositor: no display reported; running headless"
        );
    }

    // M26 Step 24: bring up the desktop shell surfaces (Status Strip
    // first; Taskbar / Workspace land in subsequent steps). Skip when the
    // handoff failed — there's no display to render onto.
    if display_owned {
        if let Err(e) = shell::init_shell_surfaces(display.width, display.height) {
            crate::kwarn!(
                Compositor,
                "Compositor: shell init failed ({:?}); continuing without shell chrome",
                e
            );
        }
    }

    let mut recv_buf = [0u8; ipc::MAX_MESSAGE_SIZE];

    // Short receive timeout (~1 frame budget) so the loop runs the compose
    // step regularly even with no clients. Step 14 ships this as a
    // poll-then-compose loop; Step 17 will switch to a proper
    // ipc_select between the channel and a frame-due notification.
    const RECV_TIMEOUT_TICKS: u64 = FRAME_BUDGET_TICKS;

    loop {
        match ipc::ipc_recv(ch, &mut recv_buf, RECV_TIMEOUT_TICKS) {
            Ok((len, sender_tid)) => {
                process_request(&recv_buf[..len], sender_tid, ch);
            }
            Err(e) => {
                if e == crate::syscall::IpcError::Epipe as i64 {
                    crate::kinfo!(
                        Compositor,
                        "Compositor: channel destroyed (EPIPE), exiting loop"
                    );
                    break;
                }
                // Etimedout is expected (no clients) — fall through.
                if e != crate::syscall::IpcError::Etimedout as i64 {
                    crate::kwarn!(Compositor, "Compositor: recv error {}", e);
                }
            }
        }

        // Step 20: drain typed input events from the kernel input queue
        // and route them through the M25 input pipeline (coalesce → hotkey
        // filter → focus router → IPC delivery).
        input_route::drain_and_route();

        // M26 Step 24: drive the desktop shell tick. The shell decides
        // internally whether each surface needs to redraw based on its
        // own cadence (Status Strip = 1 Hz). Always cheap when nothing
        // changed and the shell mutex is leaf-only.
        shell::tick(TICK_COUNT.load(Ordering::Relaxed));

        // Run a compose-and-present cycle if we're inside the 16ms cadence
        // and at least one surface is damaged (or we still owe an initial
        // clear). Idle periods skip composition entirely → 0 GPU work.
        present_frame_if_due(&mut state);
    }

    // Mark ourselves dead and yield forever (matches GPU Service exit path).
    let cpu = crate::arch::aarch64::exceptions::core_id() as usize;
    let my_tid = { *crate::task::CURRENT_THREAD[cpu].lock() };
    if let Some(tid) = my_tid {
        let mut table = crate::task::THREAD_TABLE.lock();
        if let Some(thread) = &mut table[tid.0 as usize] {
            thread.sched.state = crate::task::ThreadState::Dead;
        }
    }
    loop {
        crate::sched::thread_yield();
    }
}

// ---------------------------------------------------------------------------
// Frame composition
// ---------------------------------------------------------------------------

/// Compose and present a new frame if we're inside the 60fps cadence and
/// damage is pending. No-op when:
///   * the compositor has no display (no front/back buffer)
///   * the previous frame was less than `FRAME_BUDGET_TICKS` ago
///   * no surface is damaged AND no initial clear is pending
///
/// The structural pacing — the `last_frame_tick`, `frame_count`,
/// `frame_ms_accum`, watchdog, and 60-frame stats logging — is already
/// wired here. M24 keeps the actual `present_frame()` body parked behind
/// `COMPOSITOR_PRESENT_ENABLED` because the post-handoff IPC bench path
/// surfaces several pre-existing kernel-side races (data aborts at low
/// virtual addresses; cap-table torn reads; virtio_input modulo-by-zero
/// — patched separately) when the compositor adds frame-pacing pressure.
/// Step 17 (M25) re-enables the present path after wiring real client
/// surfaces with attached buffers.
fn present_frame_if_due(state: &mut CompositorState) {
    if !COMPOSITOR_PRESENT_ENABLED {
        return;
    }

    let now = TICK_COUNT.load(Ordering::Relaxed);
    if now < state.last_frame_tick.saturating_add(FRAME_BUDGET_TICKS) {
        return;
    }
    if state.front_buffer.is_none() || state.back_buffer.is_none() {
        return;
    }

    let any_damage = state.needs_initial_clear || surface_table_has_damage();
    if !any_damage {
        return;
    }

    let frame_start = now;
    if let Err(e) = present_frame(state) {
        crate::kwarn!(Compositor, "Compositor: present_frame failed ({:?})", e);
        return;
    }
    let frame_end = TICK_COUNT.load(Ordering::Relaxed);
    let elapsed_ms = frame_end.saturating_sub(frame_start);

    state.frame_count += 1;
    state.frame_ms_accum += elapsed_ms;
    state.last_frame_tick = frame_end;

    if elapsed_ms > FRAME_WATCHDOG_MS {
        crate::kwarn!(
            Compositor,
            "Compositor: frame took {}ms (>{}ms threshold)",
            elapsed_ms,
            FRAME_WATCHDOG_MS
        );
    }

    if state.frame_count.is_multiple_of(STATS_EVERY_FRAMES) {
        let avg = state.frame_ms_accum / STATS_EVERY_FRAMES;
        crate::kinfo!(
            Compositor,
            "Compositor: frames={} avg compose+present={}ms",
            state.frame_count,
            avg
        );
        state.frame_ms_accum = 0;
    }
}

/// Master switch for the compose-and-present loop. M24 ships with the
/// loop scaffolding wired but presentation gated off (see the doc-comment
/// on `present_frame_if_due` for the full reason). Step 17 (M25) flips
/// this to true once the IPC dispatch resolves shmem-backed surface
/// buffers and the pre-existing torn-read paths are addressed.
const COMPOSITOR_PRESENT_ENABLED: bool = false;

/// Compose any pending surface damage into the back buffer, push it to the
/// host, flush the resource, then swap front/back so the freshly-composed
/// buffer is the new front.
///
/// M24 ships an opaque-clear-only pipeline: the back buffer is filled with
/// `DEFAULT_CLEAR_COLOR` and presented. Client-surface compositing relies
/// on `shmem_id`-to-pixel resolution that arrives with the IPC dispatch
/// in Step 17 (M25). The full compose path through `render::compose_frame`
/// is exercised by the unit tests in `shared::compositor`.
fn present_frame(state: &mut CompositorState) -> Result<(), GpuError> {
    let clear_first = state.needs_initial_clear;
    let mut frame_damage = DamageTracker::new();

    let (back_resource_id, back_width, back_height) = {
        let back = state
            .back_buffer
            .as_mut()
            .ok_or(GpuError::InvalidResource)?;
        let resource_id = back.resource_id;
        let width = back.width;
        let height = back.height;
        let back_pixels = framebuffer_slice(back);
        render::compose_frame(
            back_pixels,
            width,
            height,
            &[],
            &mut frame_damage,
            clear_first,
            render::DEFAULT_CLEAR_COLOR,
            |_surface| None,
        );
        (resource_id, width, height)
    };

    state.damage = frame_damage;
    state.needs_initial_clear = false;

    let rect = VirtioGpuRect {
        x: 0,
        y: 0,
        width: back_width,
        height: back_height,
    };
    virtio_gpu::gpu_transfer_to_host(back_resource_id, &rect, 0)?;
    virtio_gpu::gpu_resource_flush(back_resource_id, &rect)?;

    swap_buffers_after_compose(state)?;
    clear_surface_damage();
    Ok(())
}

/// Returns true if any surface has its damaged flag set.
fn surface_table_has_damage() -> bool {
    let table = SURFACE_TABLE.lock();
    table.iter().any(|s| s.as_ref().is_some_and(|s| s.damaged))
}

/// Clear damaged flags on all surfaces. Called immediately after a successful
/// compose+present so the next idle tick correctly skips composition.
fn clear_surface_damage() {
    let mut table = SURFACE_TABLE.lock();
    for slot in table.iter_mut() {
        if let Some(surface) = slot.as_mut() {
            surface.damaged = false;
        }
    }
}

/// View a `GpuBufferHandle`'s framebuffer as a mutable `[u32]` slice.
///
/// Returns an empty slice for zero-size buffers. The slice covers exactly
/// `width * height` pixels. Takes `&mut` on the handle so the borrow checker
/// can prevent overlapping mutable views into the same buffer.
fn framebuffer_slice(handle: &mut GpuBufferHandle) -> &mut [u32] {
    let pixel_count = (handle.width as usize) * (handle.height as usize);
    if pixel_count == 0 {
        return &mut [];
    }
    // SAFETY: handle.fb_virt points to DMA pages allocated by
    // gpu_allocate_framebuffer. The allocation covers width*height*4 bytes
    // (verified at allocation time). The compositor service is the sole
    // owner of front/back buffers; the &mut handle ensures no concurrent
    // alias exists at the language level.
    // Maintained by: CompositorState owns the handles for the lifetime of
    // the service thread; release_buffer is only called after the handles
    // are removed from state.
    // Violation: a stale or overlapping fb_virt would let the compositor
    // write past the allocation, corrupting adjacent DMA pages.
    unsafe { core::slice::from_raw_parts_mut(handle.fb_virt as *mut u32, pixel_count) }
}

/// Swap front/back buffers after a compose has finished writing into back.
/// Rebinds scanout to the new front, then exchanges the handles in state.
fn swap_buffers_after_compose(state: &mut CompositorState) -> Result<(), GpuError> {
    let front = state.front_buffer.take().ok_or(GpuError::InvalidResource)?;
    let back = state.back_buffer.take().ok_or(GpuError::InvalidResource)?;

    let new_front = back;
    let new_back = front;
    let rect = VirtioGpuRect {
        x: 0,
        y: 0,
        width: new_front.width,
        height: new_front.height,
    };

    let result =
        virtio_gpu::gpu_set_scanout(state.display.scanout_id, new_front.resource_id, &rect);

    if result.is_ok() {
        state.front_buffer = Some(new_front);
        state.back_buffer = Some(new_back);
    } else {
        // Restore original positions on error so we don't leak handles.
        state.front_buffer = Some(new_back);
        state.back_buffer = Some(new_front);
    }
    result
}

// ---------------------------------------------------------------------------
// Display handoff
// ---------------------------------------------------------------------------

/// Take control of the display from the GPU Service.
///
/// Allocates two DMA-backed framebuffers (front + back), pre-fills the
/// front with AIOS blue, binds it to scanout 0 via `gpu_set_scanout`,
/// pushes a transfer + flush so the new content is visible, then sets
/// `crate::compositor::COMPOSITOR_ACTIVE` to true. From this point the
/// GPU Service stops driving the display (its own swap_buffers calls
/// will check `COMPOSITOR_ACTIVE` and bail out — wired in this step).
///
/// If allocation fails partway through, any successfully allocated buffer
/// is freed before returning. The compositor stays in the "headless"
/// state and the GPU Service continues to drive the display.
fn display_handoff(state: &mut CompositorState) -> Result<(), GpuError> {
    let w = state.display.width;
    let h = state.display.height;

    let front = virtio_gpu::gpu_allocate_framebuffer(w, h)?;
    let back = match virtio_gpu::gpu_allocate_framebuffer(w, h) {
        Ok(handle) => handle,
        Err(e) => {
            release_buffer(&front);
            return Err(e);
        }
    };

    // Pre-fill front buffer with AIOS blue so the scanout swap shows
    // a clean color, not whatever zeroed DMA pages happened to contain.
    fill_buffer(&front, AIOS_BLUE_B8G8R8A8);

    let rect = VirtioGpuRect {
        x: 0,
        y: 0,
        width: w,
        height: h,
    };

    // Bind front buffer to scanout 0 — this is where GPU Service ownership
    // ends and compositor ownership begins. The VIRTIO_GPU mutex serializes
    // any concurrent GPU Service activity, so we can't race even though
    // both services are running.
    let bind = (|| -> Result<(), GpuError> {
        virtio_gpu::gpu_set_scanout(state.display.scanout_id, front.resource_id, &rect)?;
        virtio_gpu::gpu_transfer_to_host(front.resource_id, &rect, 0)?;
        virtio_gpu::gpu_resource_flush(front.resource_id, &rect)?;
        Ok(())
    })();

    if let Err(e) = bind {
        release_buffer(&front);
        release_buffer(&back);
        return Err(e);
    }

    state.front_buffer = Some(front);
    state.back_buffer = Some(back);

    // Publish the handoff so the GPU Service knows to stop driving the
    // display. Release ordering ensures the buffer mutations above are
    // visible before any other core observes the flag flip.
    crate::compositor::COMPOSITOR_ACTIVE.store(true, Ordering::Release);

    crate::kinfo!(
        Compositor,
        "Compositor: display handoff complete (scanout {} = front buffer resource={}, {}x{})",
        state.display.scanout_id,
        state
            .front_buffer
            .as_ref()
            .map(|h| h.resource_id)
            .unwrap_or(0),
        w,
        h
    );

    Ok(())
}

/// Fill an entire framebuffer with a single B8G8R8A8 pixel value.
fn fill_buffer(handle: &GpuBufferHandle, color: u32) {
    let pixel_count = (handle.width as usize) * (handle.height as usize);
    if pixel_count == 0 {
        return;
    }
    // SAFETY: handle.fb_virt points into a DMA allocation owned by this
    // handle. `gpu_allocate_framebuffer` guarantees the allocation covers
    // width*height*4 bytes; we write exactly pixel_count u32s.
    // Maintained by: the buffer is held in the compositor service state and
    // not yet reachable by any other thread.
    // Violation: writing past the allocation would corrupt adjacent DMA
    // pages.
    unsafe {
        let fb = handle.fb_virt as *mut u32;
        let slice = core::slice::from_raw_parts_mut(fb, pixel_count);
        slice.fill(color);
    }
}

/// Release a framebuffer obtained from `gpu_allocate_framebuffer`: detach
/// backing pages, unref the VirtIO resource, free the DMA pages.
fn release_buffer(handle: &GpuBufferHandle) {
    let _ = virtio_gpu::gpu_resource_detach_backing(handle.resource_id);
    let _ = virtio_gpu::gpu_resource_unref(handle.resource_id);
    // SAFETY: handle.fb_phys / handle.order were returned by alloc_dma_pages
    // inside gpu_allocate_framebuffer. release_buffer is only called on
    // handles that have not been stored elsewhere (allocation-failure path
    // or shutdown path).
    // Maintained by: callers that hand us the handle never retain a copy.
    // Violation: a double-free would corrupt the buddy bitmap.
    unsafe { crate::mm::frame::free_dma_pages(handle.fb_phys, handle.order) };
}

// ---------------------------------------------------------------------------
// Compositor service channel (set during boot init, read by service thread)
// ---------------------------------------------------------------------------

/// Returns `true` if `ch` is the compositor's own service channel.
///
/// M25 sets every `Surface.channel` to the well-known compositor channel
/// (no per-client channels yet — that arrives in M26). The input router
/// uses this predicate to suppress event delivery onto our own recv
/// queue, which would otherwise round-trip the bytes back into
/// `process_request`, fail the size check, and spam the log.
pub fn is_self_channel(ch: ChannelId) -> bool {
    match *COMPOSITOR_CHANNEL.lock() {
        Some(self_ch) => self_ch == ch,
        None => false,
    }
}

/// Channel ID for the compositor service, set during boot init.
///
/// Lock ordering: this mutex is a leaf — release before acquiring any
/// kernel global. It is touched only at init (write) and by the
/// compositor entry thread (one-time read).
pub static COMPOSITOR_CHANNEL: Mutex<Option<ChannelId>> = Mutex::new(None);

/// Initialize the compositor service: create process, channel, and thread.
///
/// Called from `kernel_main` after `input::init()` succeeds and a display
/// is available. The compositor thread starts running when the scheduler
/// begins.
///
/// Lock ordering (M25 chain — extends the M24 invariant with the
/// window-manager mutexes):
/// `PROCESS_TABLE > SHARED_REGION_TABLE > NOTIFICATION_TABLE >
/// CHANNEL_TABLE > SELECT_WAITERS > BLOCK_ENGINE > WINDOW_Z_ORDER >
/// DRAG_STATE > SURFACE_TABLE > {VIRTIO_BLK, VIRTIO_GPU, VIRTIO_INPUT}`.
/// `FOCUS_MANAGER`, `CURSOR_POS`, and `TITLE_FONT` are leaf-independent
/// (alongside `INPUT_QUEUE` / `PENDING_POINTER`) — never co-held with
/// `SURFACE_TABLE` or any of the chain rungs above. Each declaration
/// site documents its position with a `// Lock ordering:` comment.
pub fn init_compositor() {
    use crate::cap;
    use crate::task::process::{KernelResourceLimits, ProcessControl, PROCESS_TABLE};
    use crate::task::{CpuSet, SchedulerClass, Thread, ThreadId};

    // --- Create Process 10: compositor ---
    {
        let mut procs = PROCESS_TABLE.lock();
        let mut name = [0u8; 32];
        name[..10].copy_from_slice(b"compositor");
        procs[10] = Some(ProcessControl {
            pid: ProcessId(10),
            address_space: None,
            resource_limits: KernelResourceLimits::native(),
            cap_table: cap::CapabilityTable::new(),
            thread_ids: [None; 16],
            name,
        });
    }

    // Grant capabilities to the compositor process.
    let _ = cap::grant_to_process(ProcessId(10), shared::Capability::ChannelCreate, true);
    let _ = cap::grant_to_process(ProcessId(10), shared::Capability::GpuMmioAccess, false);
    let _ = cap::grant_to_process(ProcessId(10), shared::Capability::GpuBufferCreate, true);
    let _ = cap::grant_to_process(ProcessId(10), shared::Capability::DisplayControl, false);
    let _ = cap::grant_to_process(
        ProcessId(10),
        shared::Capability::CompositorCreateSurface,
        true,
    );
    let _ = cap::grant_to_process(
        ProcessId(10),
        shared::Capability::CompositorInputAccess,
        false,
    );
    let _ = cap::grant_to_process(ProcessId(10), shared::Capability::DebugPrint, false);
    // M26 Step 24: shell surfaces (Status Strip first) allocate their own
    // backing buffers via `shared_memory_create`. Granting the create cap
    // here keeps the shell as a normal capability-checked client of the
    // shmem subsystem rather than a privileged shortcut.
    let _ = cap::grant_to_process(ProcessId(10), shared::Capability::SharedMemoryCreate, false);

    // Create the compositor's IPC channel.
    let compositor_tid = ThreadId(0xA10); // Debug label for the compositor thread.
    let ch = ipc::channel_create_unchecked(compositor_tid);

    // Grant ChannelAccess to the compositor process and the kernel process so
    // kernel-side test apps can call us during M24 bring-up.
    let _ = cap::grant_to_process(ProcessId(10), shared::Capability::ChannelAccess(ch), false);
    let _ = cap::grant_to_process(ProcessId(0), shared::Capability::ChannelAccess(ch), false);

    // Register in the service manager so clients can locate the compositor.
    service::service_register(b"compositor", ProcessId(10), ch)
        .expect("Failed to register compositor service");

    // Store the channel for the entry thread to pick up.
    *COMPOSITOR_CHANNEL.lock() = Some(ch);

    // --- Create the compositor service thread ---
    {
        let stack_phys = crate::sched::alloc_kernel_stack();
        let stack_virt_top = crate::sched::phys_to_virt(stack_phys) + crate::sched::STACK_SIZE;

        let mut thread = Thread::new_kernel(
            compositor_tid,
            b"compositor\0\0\0\0\0\0",
            compositor_entry as *const () as usize,
            stack_phys,
        );
        thread.sched.class = SchedulerClass::Interactive;
        thread.sched.effective_class = SchedulerClass::Interactive;
        thread.sched.affinity = CpuSet::all();
        thread.context.sp = stack_virt_top as u64;
        thread.owner_pid = Some(ProcessId(10));

        let idx = crate::sched::allocate_thread(thread).expect("thread table full for compositor");
        crate::sched::enqueue_on_cpu(0, ThreadId(idx as u32), SchedulerClass::Interactive);
    }

    crate::kinfo!(
        Compositor,
        "Compositor service initialized (pid=10, ch={})",
        ch.0
    );
}

// ---------------------------------------------------------------------------
// IPC dispatch (Step 20)
// ---------------------------------------------------------------------------

/// Decode a `CompositorRequest` from a raw IPC payload.
///
/// Returns `None` if the payload is too short to hold a request — that
/// typically indicates a protocol error or a misaddressed message; the
/// caller responds with an error reply.
fn decode_request(bytes: &[u8]) -> Option<CompositorRequest> {
    if bytes.len() < core::mem::size_of::<CompositorRequest>() {
        return None;
    }
    // SAFETY: CompositorRequest is repr(C) Copy with no padding-trap
    // fields. We copy the bytes into a freshly-zeroed instance via
    // ptr::read_unaligned to avoid relying on the source alignment.
    // The recv buffer is at most MAX_MESSAGE_SIZE; the requested type
    // size is bounded by the same const (compile-time asserted).
    // Maintained by: the size check above ensures the read is in-bounds.
    // Violation: a shorter slice would read past the buffer → UB.
    let req = unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const CompositorRequest) };
    Some(req)
}

/// Process one `CompositorRequest` and reply with a `CompositorEvent`.
///
/// `sender_tid` identifies the calling thread; we resolve its owning
/// process to record as the surface owner. `service_channel` is the
/// compositor's well-known IPC channel — used as a placeholder
/// `Surface.channel` until per-client channels arrive in M26.
fn process_request(payload: &[u8], sender_tid: ThreadId, service_channel: ChannelId) {
    let req = match decode_request(payload) {
        Some(r) => r,
        None => {
            crate::kwarn!(
                Compositor,
                "Compositor: short IPC payload ({} bytes); replying empty",
                payload.len()
            );
            let _ = ipc::ipc_reply(service_channel, &[]);
            return;
        }
    };

    let owner_pid = match crate::cap::process_of_thread(sender_tid) {
        Some(pid) => pid,
        None => {
            crate::kwarn!(
                Compositor,
                "Compositor: sender tid={} has no owning pid",
                sender_tid.0
            );
            let _ = ipc::ipc_reply(service_channel, &[]);
            return;
        }
    };

    let event = match CompositorCommand::from_u32(req.command) {
        Some(CompositorCommand::CreateSurface) => {
            handle_create_surface(&req, owner_pid, service_channel)
        }
        Some(CompositorCommand::AttachBuffer) => handle_attach_buffer(&req, owner_pid),
        Some(CompositorCommand::DestroySurface) => handle_destroy_surface(&req, owner_pid),
        Some(CompositorCommand::Resize) => handle_resize(&req, owner_pid),
        Some(CompositorCommand::SetLayer) => handle_set_layer(&req, owner_pid),
        None => {
            crate::kwarn!(
                Compositor,
                "Compositor: unknown command {} from pid={}",
                req.command,
                owner_pid.0
            );
            CompositorEvent::zeroed()
        }
    };

    let bytes: &[u8] = unsafe {
        // SAFETY: CompositorEvent is repr(C) Copy. We borrow its bytes
        // for the duration of the synchronous ipc_reply call.
        // Maintained by: `event` is on this stack frame; the borrow is
        // consumed before we return.
        // Violation: a longer-lived borrow would dangle.
        core::slice::from_raw_parts(
            (&event as *const CompositorEvent) as *const u8,
            core::mem::size_of::<CompositorEvent>(),
        )
    };
    let result = ipc::ipc_reply(service_channel, bytes);
    if result < 0 {
        crate::kwarn!(Compositor, "Compositor: reply failed with {}", result);
    }
}

fn handle_create_surface(
    req: &CompositorRequest,
    owner_pid: ProcessId,
    service_channel: ChannelId,
) -> CompositorEvent {
    let layer = match req.layer {
        0 => SurfaceLayer::Background,
        1 => SurfaceLayer::Normal,
        2 => SurfaceLayer::TopLevel,
        3 => SurfaceLayer::Overlay,
        4 => SurfaceLayer::Panel,
        _ => SurfaceLayer::Normal,
    };
    let content_type =
        SurfaceContentType::from_u8(req.content_type).unwrap_or(SurfaceContentType::Generic);
    let title_bytes = &req.title[..(req.title_len as usize).min(req.title.len())];
    let title = SurfaceTitle::from_bytes(title_bytes);

    match surface::surface_create(
        owner_pid,
        service_channel,
        req.width,
        req.height,
        title,
        content_type,
        layer,
    ) {
        Ok(id) => {
            // Register with the z-order list and the focus manager so the
            // compositor knows where this surface stacks. Acquired in
            // separate scopes to keep each lock hold tight.
            {
                let mut z = WINDOW_Z_ORDER.lock();
                z.push(id);
            }
            // First created surface receives keyboard focus. Notify side
            // effect runs after dropping locks so we don't IPC-recurse.
            let change = {
                let mut fm = FOCUS_MANAGER.lock();
                if fm.keyboard_focus().is_none() {
                    fm.set_keyboard_focus(Some(id))
                } else {
                    super::focus::FocusChange {
                        lost: None,
                        gained: None,
                    }
                }
            };
            input_route::notify_focus_change(change);
            CompositorEvent::configure(id, req.width, req.height, 100)
        }
        Err(e) => {
            crate::kwarn!(
                Compositor,
                "Compositor: surface_create failed ({:?}) for pid={}",
                e,
                owner_pid.0
            );
            CompositorEvent::zeroed()
        }
    }
}

fn handle_attach_buffer(req: &CompositorRequest, owner_pid: ProcessId) -> CompositorEvent {
    let id = SurfaceId(req.surface_id);
    let damage = req.decode_damage();
    let shmem = SharedMemoryId(req.shmem_id);
    match surface::surface_attach_buffer(id, shmem, damage, owner_pid) {
        Ok(()) => CompositorEvent::buffer_released(id, shmem),
        Err(e) => {
            log_surface_error("AttachBuffer", id, owner_pid, e);
            CompositorEvent::zeroed()
        }
    }
}

fn handle_destroy_surface(req: &CompositorRequest, owner_pid: ProcessId) -> CompositorEvent {
    let id = SurfaceId(req.surface_id);
    match surface::surface_destroy(id, owner_pid) {
        Ok(()) => {
            // Remove from z-order and focus state. Notify if the destroy
            // cleared the focused surface.
            {
                let mut z = WINDOW_Z_ORDER.lock();
                z.remove(id);
            }
            let change = {
                let mut fm = FOCUS_MANAGER.lock();
                fm.surface_destroyed(id)
            };
            input_route::notify_focus_change(change);
            CompositorEvent::close_requested(id)
        }
        Err(e) => {
            log_surface_error("DestroySurface", id, owner_pid, e);
            CompositorEvent::zeroed()
        }
    }
}

fn handle_resize(req: &CompositorRequest, owner_pid: ProcessId) -> CompositorEvent {
    let id = SurfaceId(req.surface_id);
    let (w, h) = crate::compositor::window::clamp_window_size(req.width, req.height);
    match surface::surface_resize(id, w, h, owner_pid) {
        Ok((width, height)) => CompositorEvent::configure(id, width, height, 100),
        Err(e) => {
            log_surface_error("Resize", id, owner_pid, e);
            CompositorEvent::zeroed()
        }
    }
}

fn handle_set_layer(req: &CompositorRequest, owner_pid: ProcessId) -> CompositorEvent {
    let id = SurfaceId(req.surface_id);
    let layer = match req.layer {
        0 => SurfaceLayer::Background,
        1 => SurfaceLayer::Normal,
        2 => SurfaceLayer::TopLevel,
        3 => SurfaceLayer::Overlay,
        4 => SurfaceLayer::Panel,
        _ => SurfaceLayer::Normal,
    };
    match surface::surface_set_layer(id, layer, owner_pid) {
        Ok(()) => CompositorEvent::configure(id, 0, 0, 100),
        Err(e) => {
            log_surface_error("SetLayer", id, owner_pid, e);
            CompositorEvent::zeroed()
        }
    }
}

fn log_surface_error(op: &str, id: SurfaceId, owner_pid: ProcessId, err: SurfaceError) {
    crate::kwarn!(
        Compositor,
        "Compositor: {} surface={} pid={} failed: {:?}",
        op,
        id.0,
        owner_pid.0,
        err
    );
}
