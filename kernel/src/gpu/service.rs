//! GPU Service: capability-gated IPC service for GPU buffer management.
//!
//! Wraps the VirtIO-GPU 2D driver in an IPC service following the echo service
//! pattern from Phase 3. Manages buffer allocation, double-buffered display,
//! and capability-gated access to GPU resources.
//!
//! Per docs/platform/gpu/drivers.md §3.3–3.5, docs/platform/gpu/display.md §7.2–7.4.

use shared::gpu::{
    DisplayInfo, FenceTracker, GpuBufferHandle, GpuCommand, GpuError, GpuRequest, GpuResponse,
    VirtioGpuRect, MAX_GPU_BUFFERS,
};

use crate::drivers::virtio_gpu;
use crate::ipc;
use crate::service;
use crate::task::process::ProcessId;

// ---------------------------------------------------------------------------
// GPU Service state
// ---------------------------------------------------------------------------

/// Maximum number of buffers the GPU Service tracks.
const MAX_BUFFERS: usize = MAX_GPU_BUFFERS;

/// GPU Service runtime state, held inside the service loop.
struct GpuServiceState {
    /// Allocated buffer tracking table.
    buffers: [Option<GpuBufferHandle>; MAX_BUFFERS],
    /// Display information from VirtIO-GPU.
    display: DisplayInfo,
    /// IPC channel for this service (used by Phase 7+ compositor for swap notification).
    #[allow(dead_code)]
    channel: shared::ChannelId,
    /// Front buffer (currently displayed).
    front_buffer: Option<GpuBufferHandle>,
    /// Back buffer (rendering target).
    back_buffer: Option<GpuBufferHandle>,
    /// Fence tracker for asynchronous command completion (Phase 7+ IRQ-driven I/O).
    #[allow(dead_code)]
    fence_tracker: FenceTracker,
    /// Whether double buffering is active.
    double_buffering: bool,
}

impl GpuServiceState {
    fn new(display: DisplayInfo, channel: shared::ChannelId) -> Self {
        Self {
            buffers: [None; MAX_BUFFERS],
            display,
            channel,
            front_buffer: None,
            back_buffer: None,
            fence_tracker: FenceTracker::new(),
            double_buffering: false,
        }
    }

    /// Track a newly allocated buffer. Returns the buffer slot index.
    fn track_buffer(&mut self, handle: GpuBufferHandle) -> Result<usize, GpuError> {
        for (i, slot) in self.buffers.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(handle);
                return Ok(i);
            }
        }
        Err(GpuError::OutOfMemory)
    }

    /// Find a tracked buffer by resource ID.
    fn find_buffer(&self, resource_id: u32) -> Option<&GpuBufferHandle> {
        self.buffers
            .iter()
            .flatten()
            .find(|h| h.resource_id == resource_id)
    }

    /// Remove a tracked buffer by resource ID. Returns the handle for cleanup.
    fn remove_buffer(&mut self, resource_id: u32) -> Option<GpuBufferHandle> {
        for slot in self.buffers.iter_mut() {
            if let Some(handle) = slot {
                if handle.resource_id == resource_id {
                    return slot.take();
                }
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// GPU Service thread entry
// ---------------------------------------------------------------------------

/// GPU Service thread entry point.
///
/// Unmasks IRQs (required for IPC timeout/scheduling), initializes the GPU
/// Service state, registers the IPC service, and enters the command loop.
pub fn gpu_service_entry() -> ! {
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1. Required
    // for timer interrupts (IPC timeout, scheduling) to fire on this CPU.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    gpu_service_loop();
}

/// GPU Service main loop. Creates IPC channel, registers service, processes commands.
fn gpu_service_loop() -> ! {
    let display = virtio_gpu::display_info().unwrap_or_else(DisplayInfo::default);

    // Wait for the GPU Service channel that was created during boot init.
    // The channel ID is stored in GPU_SERVICE_CHANNEL by the boot code.
    let ch = loop {
        if let Some(ch) = *GPU_SERVICE_CHANNEL.lock() {
            break ch;
        }
        crate::sched::thread_yield();
    };

    crate::kinfo!(Gpu, "GPU Service: started, channel={}", ch.0);

    let mut state = GpuServiceState::new(display, ch);

    // Initialize double buffering if display is valid.
    if display.width > 0 && display.height > 0 {
        init_double_buffering(&mut state);
    }

    let mut recv_buf = [0u8; ipc::MAX_MESSAGE_SIZE];

    loop {
        match ipc::ipc_recv(ch, &mut recv_buf, ipc::DEFAULT_TIMEOUT_TICKS) {
            Ok((len, _sender)) => {
                let resp = dispatch_command(&mut state, &recv_buf[..len]);
                let resp_bytes = gpu_response_as_bytes(&resp);
                let result = ipc::ipc_reply(ch, resp_bytes);
                if result < 0 {
                    crate::kwarn!(Gpu, "GPU Service: reply failed with {}", result);
                }
            }
            Err(e) => {
                if e == crate::syscall::IpcError::Epipe as i64 {
                    crate::kinfo!(Gpu, "GPU Service: channel destroyed (EPIPE), exiting loop");
                    break;
                }
                // Timeout is expected when no clients — continue.
                if e != crate::syscall::IpcError::Etimedout as i64 {
                    crate::kwarn!(Gpu, "GPU Service: recv error {}", e);
                }
            }
        }
    }

    // Mark ourselves dead and yield forever (same pattern as echo server).
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
// Command dispatch
// ---------------------------------------------------------------------------

/// Decode a GpuRequest from raw IPC bytes and dispatch to the appropriate handler.
fn dispatch_command(state: &mut GpuServiceState, data: &[u8]) -> GpuResponse {
    if data.len() < core::mem::size_of::<GpuRequest>() {
        return GpuResponse::error(GpuError::CommandFailed);
    }

    // SAFETY: GpuRequest is repr(C) with only u32 integer fields (no padding UB).
    // Invariant: data.len() >= size_of::<GpuRequest>() checked above.
    // Maintained by: the size guard at the top of this function.
    // Violation: if GpuRequest had non-integer fields (references, restricted enums),
    // copy_nonoverlapping from IPC data could produce invalid representations (UB).
    // Currently safe because all fields are plain integer types.
    let req = unsafe {
        let mut req = GpuRequest::zeroed();
        core::ptr::copy_nonoverlapping(
            data.as_ptr(),
            &mut req as *mut GpuRequest as *mut u8,
            core::mem::size_of::<GpuRequest>(),
        );
        req
    };

    let cmd = match GpuCommand::from_u32(req.command) {
        Some(c) => c,
        None => {
            crate::kwarn!(Gpu, "GPU Service: unknown command {}", req.command);
            return GpuResponse::error(GpuError::CommandFailed);
        }
    };

    match cmd {
        GpuCommand::GetDisplayInfo => handle_get_display_info(state),
        GpuCommand::AllocateBuffer => handle_allocate_buffer(state, &req),
        GpuCommand::ReleaseBuffer => handle_release_buffer(state, &req),
        GpuCommand::Present => handle_present(state, &req),
        GpuCommand::GetBufferInfo => handle_get_buffer_info(state, &req),
        GpuCommand::SwapBuffers => handle_swap_buffers(state),
    }
}

// ---------------------------------------------------------------------------
// Command handlers
// ---------------------------------------------------------------------------

fn handle_get_display_info(state: &GpuServiceState) -> GpuResponse {
    let d = &state.display;
    let mut resp = GpuResponse::zeroed();
    resp.width = d.width;
    resp.height = d.height;
    resp.format = d.format as u32;
    resp.scanout_id = d.scanout_id;
    resp
}

fn handle_allocate_buffer(state: &mut GpuServiceState, req: &GpuRequest) -> GpuResponse {
    let width = if req.width > 0 {
        req.width
    } else {
        state.display.width
    };
    let height = if req.height > 0 {
        req.height
    } else {
        state.display.height
    };

    // Reject zero-sized buffers.
    if width == 0 || height == 0 {
        return GpuResponse::error(GpuError::InvalidResource);
    }

    match virtio_gpu::gpu_allocate_framebuffer(width, height) {
        Ok(handle) => {
            let resource_id = handle.resource_id;
            match state.track_buffer(handle) {
                Ok(_slot) => {
                    crate::kinfo!(
                        Gpu,
                        "GPU Service: allocated buffer (resource={}, {}x{})",
                        resource_id,
                        width,
                        height
                    );
                    let mut resp = GpuResponse::zeroed();
                    resp.resource_id = resource_id;
                    resp.width = width;
                    resp.height = height;
                    resp
                }
                Err(e) => {
                    // Buffer table full — release the just-allocated buffer.
                    let _ = virtio_gpu::gpu_resource_detach_backing(resource_id);
                    let _ = virtio_gpu::gpu_resource_unref(resource_id);
                    // SAFETY: handle.fb_phys and handle.order were returned by
                    // alloc_dma_pages inside gpu_allocate_framebuffer. GpuBufferHandle
                    // is Copy so handle is still valid. Double-free impossible because
                    // track_buffer failed (handle not stored). Violation: buddy bitmap
                    // corruption if phys_addr/order are wrong.
                    unsafe { crate::mm::frame::free_dma_pages(handle.fb_phys, handle.order) };
                    GpuResponse::error(e)
                }
            }
        }
        Err(e) => GpuResponse::error(e),
    }
}

fn handle_release_buffer(state: &mut GpuServiceState, req: &GpuRequest) -> GpuResponse {
    match state.remove_buffer(req.resource_id) {
        Some(handle) => {
            let _ = virtio_gpu::gpu_resource_detach_backing(handle.resource_id);
            let _ = virtio_gpu::gpu_resource_unref(handle.resource_id);
            // SAFETY: handle.fb_phys and handle.order were returned by alloc_dma_pages
            // inside gpu_allocate_framebuffer. remove_buffer ensures this handle is
            // removed from tracking (no double-free).
            // Maintained by: buffer tracking table (each handle stored once).
            // Violation: buddy bitmap corruption if phys_addr/order are wrong.
            unsafe { crate::mm::frame::free_dma_pages(handle.fb_phys, handle.order) };
            crate::kinfo!(
                Gpu,
                "GPU Service: released buffer (resource={})",
                handle.resource_id
            );
            GpuResponse::zeroed()
        }
        None => GpuResponse::error(GpuError::InvalidResource),
    }
}

fn handle_present(state: &GpuServiceState, req: &GpuRequest) -> GpuResponse {
    let handle = match state.find_buffer(req.resource_id) {
        Some(h) => h,
        None => return GpuResponse::error(GpuError::InvalidResource),
    };

    // Use damage rect from request, or full buffer if damage is zero-sized.
    let rect = if req.damage_w > 0 && req.damage_h > 0 {
        // Validate damage rect stays within buffer bounds.
        if req.damage_x.saturating_add(req.damage_w) > handle.width
            || req.damage_y.saturating_add(req.damage_h) > handle.height
        {
            return GpuResponse::error(GpuError::InvalidResource);
        }
        VirtioGpuRect {
            x: req.damage_x,
            y: req.damage_y,
            width: req.damage_w,
            height: req.damage_h,
        }
    } else {
        VirtioGpuRect {
            x: 0,
            y: 0,
            width: handle.width,
            height: handle.height,
        }
    };

    if let Err(e) = virtio_gpu::gpu_transfer_to_host(req.resource_id, &rect, 0) {
        return GpuResponse::error(e);
    }
    if let Err(e) = virtio_gpu::gpu_resource_flush(req.resource_id, &rect) {
        return GpuResponse::error(e);
    }

    GpuResponse::zeroed()
}

fn handle_get_buffer_info(state: &GpuServiceState, req: &GpuRequest) -> GpuResponse {
    match state.find_buffer(req.resource_id) {
        Some(handle) => {
            let mut resp = GpuResponse::zeroed();
            resp.resource_id = handle.resource_id;
            resp.width = handle.width;
            resp.height = handle.height;
            resp.stride = handle.stride;
            resp.format = handle.format as u32;
            resp.fb_virt = handle.fb_virt as u64;
            resp
        }
        None => GpuResponse::error(GpuError::InvalidResource),
    }
}

fn handle_swap_buffers(state: &mut GpuServiceState) -> GpuResponse {
    if !state.double_buffering {
        return GpuResponse::error(GpuError::DeviceNotFound);
    }

    if let Err(e) = swap_buffers(state) {
        return GpuResponse::error(e);
    }

    GpuResponse::zeroed()
}

// ---------------------------------------------------------------------------
// Double buffering
// ---------------------------------------------------------------------------

/// Initialize double buffering: allocate front and back buffers, set scanout.
fn init_double_buffering(state: &mut GpuServiceState) {
    let w = state.display.width;
    let h = state.display.height;

    let front = match virtio_gpu::gpu_allocate_framebuffer(w, h) {
        Ok(handle) => handle,
        Err(e) => {
            crate::kwarn!(Gpu, "GPU Service: failed to allocate front buffer: {:?}", e);
            return;
        }
    };

    let back = match virtio_gpu::gpu_allocate_framebuffer(w, h) {
        Ok(handle) => handle,
        Err(e) => {
            crate::kwarn!(Gpu, "GPU Service: failed to allocate back buffer: {:?}", e);
            // Clean up front buffer.
            let _ = virtio_gpu::gpu_resource_detach_backing(front.resource_id);
            let _ = virtio_gpu::gpu_resource_unref(front.resource_id);
            // SAFETY: front.fb_phys/order were returned by alloc_dma_pages inside
            // gpu_allocate_framebuffer. Maintained by: local scope (not yet stored).
            // Violation: buddy bitmap corruption if phys_addr/order are wrong.
            unsafe { crate::mm::frame::free_dma_pages(front.fb_phys, front.order) };
            return;
        }
    };

    // Bind front buffer to scanout 0.
    let rect = VirtioGpuRect {
        x: 0,
        y: 0,
        width: w,
        height: h,
    };
    if let Err(e) = virtio_gpu::gpu_set_scanout(state.display.scanout_id, front.resource_id, &rect)
    {
        crate::kwarn!(Gpu, "GPU Service: set_scanout failed: {:?}", e);
        // Clean up both buffers on scanout failure.
        let _ = virtio_gpu::gpu_resource_detach_backing(front.resource_id);
        let _ = virtio_gpu::gpu_resource_unref(front.resource_id);
        // SAFETY: front/back fb_phys/order were returned by alloc_dma_pages inside
        // gpu_allocate_framebuffer. Not yet stored in state (no other references).
        // Violation: buddy bitmap corruption if phys_addr/order are wrong.
        unsafe { crate::mm::frame::free_dma_pages(front.fb_phys, front.order) };
        let _ = virtio_gpu::gpu_resource_detach_backing(back.resource_id);
        let _ = virtio_gpu::gpu_resource_unref(back.resource_id);
        unsafe { crate::mm::frame::free_dma_pages(back.fb_phys, back.order) };
        return;
    }

    // Fill front buffer with AIOS blue and present it.
    let pixel_count = (w as usize) * (h as usize);
    if pixel_count > 0 {
        // SAFETY: front.fb_virt points to DMA pages from gpu_allocate_framebuffer.
        // Maintained by: gpu_allocate_framebuffer guarantees fb_virt covers
        // width*height*4 bytes. We write exactly pixel_count u32s.
        // Violation: writing past the allocation corrupts adjacent DMA memory.
        unsafe {
            let fb = front.fb_virt as *mut u32;
            let fb_slice = core::slice::from_raw_parts_mut(fb, pixel_count);
            fb_slice.fill(shared::gpu::AIOS_BLUE_B8G8R8A8);
        }
    }
    // Transfer front buffer content to host and flush display.
    let _ = virtio_gpu::gpu_transfer_to_host(front.resource_id, &rect, 0);
    let _ = virtio_gpu::gpu_resource_flush(front.resource_id, &rect);

    crate::kinfo!(
        Gpu,
        "VirtIO-GPU: double buffering enabled (front={}, back={})",
        front.resource_id,
        back.resource_id
    );

    state.front_buffer = Some(front);
    state.back_buffer = Some(back);
    state.double_buffering = true;
}

/// Swap front and back buffers: rebind scanout to new front, present.
fn swap_buffers(state: &mut GpuServiceState) -> Result<(), GpuError> {
    let front = state.front_buffer.take().ok_or(GpuError::InvalidResource)?;
    let back = state.back_buffer.take().ok_or(GpuError::InvalidResource)?;

    // New front = old back; new back = old front.
    let new_front = back;
    let new_back = front;

    let rect = VirtioGpuRect {
        x: 0,
        y: 0,
        width: new_front.width,
        height: new_front.height,
    };

    // Bind new front to scanout, transfer, and flush.
    // On error, restore buffers to state before returning to prevent DMA leak.
    let result = (|| -> Result<(), GpuError> {
        virtio_gpu::gpu_set_scanout(state.display.scanout_id, new_front.resource_id, &rect)?;
        virtio_gpu::gpu_transfer_to_host(new_front.resource_id, &rect, 0)?;
        virtio_gpu::gpu_resource_flush(new_front.resource_id, &rect)?;
        Ok(())
    })();

    // Always restore buffers to state (even on error) to prevent handle loss.
    if result.is_ok() {
        state.front_buffer = Some(new_front);
        state.back_buffer = Some(new_back);
    } else {
        // Restore original positions (swap back).
        state.front_buffer = Some(new_back);
        state.back_buffer = Some(new_front);
    }

    result
}

// ---------------------------------------------------------------------------
// Serialization helpers
// ---------------------------------------------------------------------------

/// View a GpuResponse as a byte slice for IPC reply.
fn gpu_response_as_bytes(resp: &GpuResponse) -> &[u8] {
    // SAFETY: GpuResponse is repr(C) with only integer fields. All fields are
    // initialized (zeroed() or explicitly set). The slice covers exactly
    // size_of::<GpuResponse>() bytes from resp's address.
    // Maintained by: GpuResponse constructors (zeroed(), error()).
    // Violation: passing a struct with uninitialized padding would expose
    // undefined bytes in the IPC reply. Not possible with current integer-only fields.
    unsafe {
        core::slice::from_raw_parts(
            resp as *const GpuResponse as *const u8,
            core::mem::size_of::<GpuResponse>(),
        )
    }
}

// ---------------------------------------------------------------------------
// GPU Service channel (set during boot init, read by service thread)
// ---------------------------------------------------------------------------

use spin::Mutex;

/// Channel ID for the GPU Service, set during boot init.
pub static GPU_SERVICE_CHANNEL: Mutex<Option<shared::ChannelId>> = Mutex::new(None);

/// Initialize the GPU Service: create process, channel, and thread.
///
/// Called from `kernel_main` after `virtio_gpu::init()` succeeds.
/// The GPU Service thread starts running when the scheduler begins.
pub fn init_gpu_service() {
    use crate::cap;
    use crate::task::process::{KernelResourceLimits, ProcessControl, PROCESS_TABLE};
    use crate::task::{CpuSet, SchedulerClass, Thread, ThreadId};

    // --- Create Process 9: gpu-svc ---
    {
        let mut procs = PROCESS_TABLE.lock();
        let mut name = [0u8; 32];
        name[..7].copy_from_slice(b"gpu-svc");
        procs[9] = Some(ProcessControl {
            pid: ProcessId(9),
            address_space: None,
            resource_limits: KernelResourceLimits::native(),
            cap_table: cap::CapabilityTable::new(),
            thread_ids: [None; 16],
            name,
        });
    }

    // Grant capabilities to the GPU Service process.
    let _ = cap::grant_to_process(ProcessId(9), shared::Capability::ChannelCreate, true);
    let _ = cap::grant_to_process(ProcessId(9), shared::Capability::GpuMmioAccess, false);
    let _ = cap::grant_to_process(ProcessId(9), shared::Capability::GpuBufferCreate, true);
    let _ = cap::grant_to_process(ProcessId(9), shared::Capability::DisplayControl, false);
    let _ = cap::grant_to_process(ProcessId(9), shared::Capability::DebugPrint, false);

    // Create IPC channel for the GPU Service.
    let gpu_server_tid = ThreadId(0x900); // Debug label for the GPU Service thread.
    let ch = ipc::channel_create_unchecked(gpu_server_tid);

    // Grant ChannelAccess for the GPU Service channel.
    let _ = cap::grant_to_process(ProcessId(9), shared::Capability::ChannelAccess(ch), false);
    // Also grant to kernel process (pid=0) so kernel threads can call GPU Service.
    let _ = cap::grant_to_process(ProcessId(0), shared::Capability::ChannelAccess(ch), false);

    // Register in the service manager.
    service::service_register(b"gpu-service", ProcessId(9), ch)
        .expect("Failed to register gpu-service");

    // Store channel for the service thread to pick up.
    *GPU_SERVICE_CHANNEL.lock() = Some(ch);

    // --- Create GPU Service thread ---
    {
        let stack_phys = crate::sched::alloc_kernel_stack();
        let stack_virt_top = crate::sched::phys_to_virt(stack_phys) + crate::sched::STACK_SIZE;

        let mut thread = Thread::new_kernel(
            gpu_server_tid,
            b"gpu-service\0\0\0\0\0",
            gpu_service_entry as *const () as usize,
            stack_phys,
        );
        thread.sched.class = SchedulerClass::Interactive;
        thread.sched.effective_class = SchedulerClass::Interactive;
        thread.sched.affinity = CpuSet::all();
        thread.context.sp = stack_virt_top as u64;
        thread.owner_pid = Some(ProcessId(9));

        let idx = crate::sched::allocate_thread(thread).expect("thread table full for gpu-service");
        crate::sched::enqueue_on_cpu(0, ThreadId(idx as u32), SchedulerClass::Interactive);
    }

    crate::kinfo!(Gpu, "GPU Service initialized (pid=9, ch={})", ch.0);
}
