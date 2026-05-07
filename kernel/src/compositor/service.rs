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

use shared::gpu::{DisplayInfo, GpuBufferHandle, GpuError, VirtioGpuRect, AIOS_BLUE_B8G8R8A8};
use shared::ipc::ChannelId;
use spin::Mutex;

use crate::drivers::virtio_gpu;
use crate::ipc;
use crate::service;
use crate::task::process::ProcessId;

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
    #[allow(dead_code)] // Used by Step 14 when the loop dispatches commands.
    channel: ChannelId,
    /// Display geometry as reported by the VirtIO-GPU driver.
    display: DisplayInfo,
    /// Front buffer — currently scanned out.
    front_buffer: Option<GpuBufferHandle>,
    /// Back buffer — render target for the next frame.
    #[allow(dead_code)] // Used in Step 14 when the swap loop runs.
    back_buffer: Option<GpuBufferHandle>,
}

impl CompositorState {
    fn new(channel: ChannelId, display: DisplayInfo) -> Self {
        Self {
            channel,
            display,
            front_buffer: None,
            back_buffer: None,
        }
    }
}

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
    if display.width > 0 && display.height > 0 {
        if let Err(e) = display_handoff(&mut state) {
            crate::kerror!(
                Compositor,
                "Compositor: display handoff failed ({:?}); display will remain owned by GPU Service",
                e
            );
        }
    } else {
        crate::kwarn!(
            Compositor,
            "Compositor: no display reported; running headless"
        );
    }

    let mut recv_buf = [0u8; ipc::MAX_MESSAGE_SIZE];

    loop {
        match ipc::ipc_recv(ch, &mut recv_buf, ipc::DEFAULT_TIMEOUT_TICKS) {
            Ok((_len, _sender)) => {
                // Step 12 will decode the CompositorRequest here. For now
                // reply with a zero-length ack so the sender can unblock.
                let result = ipc::ipc_reply(ch, &[]);
                if result < 0 {
                    crate::kwarn!(Compositor, "Compositor: reply failed with {}", result);
                }
            }
            Err(e) => {
                if e == crate::syscall::IpcError::Epipe as i64 {
                    crate::kinfo!(
                        Compositor,
                        "Compositor: channel destroyed (EPIPE), exiting loop"
                    );
                    break;
                }
                // Timeout is expected when no clients — fall through to
                // run the composition step (added in Step 14) and continue.
                if e != crate::syscall::IpcError::Etimedout as i64 {
                    crate::kwarn!(Compositor, "Compositor: recv error {}", e);
                }
            }
        }
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
/// Lock ordering invariant introduced by M24: the compositor's surface
/// table lives BELOW `BLOCK_ENGINE` and ABOVE the VirtIO leaf mutexes,
/// extending the chain to
/// `PROCESS_TABLE > SHARED_REGION_TABLE > NOTIFICATION_TABLE >
/// CHANNEL_TABLE > SELECT_WAITERS > BLOCK_ENGINE > SURFACE_TABLE >
/// {VIRTIO_BLK, VIRTIO_GPU, VIRTIO_INPUT}`. Each declaration site
/// documents its position with a `// Lock ordering:` comment.
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
