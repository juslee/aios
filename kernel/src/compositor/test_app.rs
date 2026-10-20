//! Test application — first real client of the compositor (M26 Step 28).
//!
//! Runs as a kernel-mode process (`ProcessId(11)`, name `"test-app"`) and
//! exercises the full IPC surface lifecycle end-to-end:
//!
//!   1. Create its own per-client receive channel.
//!   2. `service_lookup("compositor")` → resolve the compositor's
//!      well-known channel.
//!   3. `ipc_call(compositor_ch, CreateSurface)` → receive `Configure`
//!      reply; learn the SurfaceId.
//!   4. Allocate a shared-memory buffer via `shared_memory_create`,
//!      populate it with a colored background + "Hello from AIOS!" text
//!      using the compositor's spleen-font helper.
//!   5. `ipc_send(compositor_ch, AttachBuffer)` — surface goes Active.
//!   6. Loop on `ipc_recv(my_channel)` decoding events: append typed
//!      characters to the displayed text; respond to `CloseRequested`.
//!
//! The test app is the "first real client" that motivated the
//! per-client `client_channel` field on `CompositorRequest`. Until this
//! step, every `Surface.channel` was the compositor's own channel
//! (placeholder), causing the M25 self-channel feedback loop. Step 28
//! flips per-client channels on; the M25 `is_self_channel` predicate
//! becomes a no-op for client surfaces but is still honored for
//! shell-internal surfaces.
//!
//! Per docs/phases/07-window-compositor-and-shell.md M26 Step 28.

use shared::compositor::{
    CompositorEvent, CompositorEventTag, CompositorRequest, DamageRegion, SurfaceContentType,
    SurfaceId, SurfaceLayer,
};
use shared::input::{InputEvent, KeyCode, KeyState};
use shared::ipc::{ChannelId, MAX_MESSAGE_SIZE};
use shared::Capability;

use crate::compositor::text::{draw_text_clipped, TITLE_GLYPH_WIDTH};
use crate::ipc::shmem::{region_dmap_addr, region_size, shared_memory_create};
use crate::mm::pgtable::VmFlags;
use crate::task::process::ProcessId;
use crate::task::{ThreadId, ThreadState};

// ---------------------------------------------------------------------------
// Test app constants
// ---------------------------------------------------------------------------

/// Process id reserved for the test app.
const TEST_APP_PID: ProcessId = ProcessId(11);

/// Debug label for the test app's main thread. `0xC00` is reserved
/// for the test app — bench uses 0xB00–0xB02, GPU Service uses 0x900,
/// compositor uses 0xA10, input thread uses 0xA00.
const TEST_APP_TID: ThreadId = ThreadId(0xC00);

/// Test app surface size — small enough to coexist with the shell on a
/// 1280×800 default QEMU display.
const APP_WIDTH: u32 = 400;
const APP_HEIGHT: u32 = 300;

/// Bytes-per-pixel for the back-buffer (B8G8R8A8 = 4).
const BYTES_PER_PIXEL: usize = 4;

/// Test app background — a soft pastel blue that contrasts the shell's
/// dark slate without being too saturated.
const APP_BG: u32 = 0xFF8898C8;
/// Foreground (text) color.
const APP_FG: u32 = 0xFFFFFFFF;

/// Maximum number of typed characters appended to the welcome text.
const MAX_TYPED_CHARS: usize = 64;

/// IPC call timeout — generous because the compositor may be busy
/// composing a frame or processing input when the test app calls.
const CALL_TIMEOUT_TICKS: u64 = 1000; // 1 second

// ---------------------------------------------------------------------------
// Spawn — boot-time setup
// ---------------------------------------------------------------------------

/// Allocate `PROCESS_TABLE[11]`, grant capabilities, create the test
/// app's main thread, and enqueue it on CPU 0.
///
/// Called from `kernel_main` after `compositor::init_compositor`
/// returns. The thread starts running once `sched::start()` lets
/// secondaries online and timer ticks begin.
///
/// Lock ordering: PROCESS_TABLE → grant_to_process (briefly takes
/// PROCESS_TABLE again) → channel_create (CHANNEL_TABLE) — same shape
/// as `gpu::service::init_gpu_service` and `compositor::service::init_compositor`.
pub fn spawn_test_app() {
    use crate::cap;
    use crate::task::process::{KernelResourceLimits, ProcessControl, PROCESS_TABLE};
    use crate::task::{CpuSet, SchedulerClass, Thread};

    // --- Allocate Process 11: test-app ---
    {
        let mut procs = PROCESS_TABLE.lock();
        let mut name = [0u8; 32];
        name[..8].copy_from_slice(b"test-app");
        procs[TEST_APP_PID.0 as usize] = Some(ProcessControl {
            pid: TEST_APP_PID,
            address_space: None,
            resource_limits: KernelResourceLimits::native(),
            cap_table: cap::CapabilityTable::new(),
            thread_ids: [None; 16],
            name,
        });
    }

    // Grant the minimum capability set — no GPU MMIO, no DisplayControl,
    // no CompositorCreateSurface (the compositor accepts CreateSurface
    // from any caller; per-client channel auth is via ChannelAccess).
    let _ = cap::grant_to_process(TEST_APP_PID, Capability::ChannelCreate, true);
    let _ = cap::grant_to_process(TEST_APP_PID, Capability::SharedMemoryCreate, false);
    let _ = cap::grant_to_process(TEST_APP_PID, Capability::DebugPrint, false);

    // --- Create the test app's main thread ---
    let stack_phys = crate::sched::alloc_kernel_stack();
    let stack_virt_top = crate::sched::phys_to_virt(stack_phys) + crate::sched::STACK_SIZE;

    let mut thread = Thread::new_kernel(
        TEST_APP_TID,
        b"test-app\0\0\0\0\0\0\0\0",
        test_app_entry as *const () as usize,
        stack_phys,
    );
    thread.sched.class = SchedulerClass::Normal;
    thread.sched.effective_class = SchedulerClass::Normal;
    thread.sched.affinity = CpuSet::all();
    thread.context.sp = stack_virt_top as u64;
    thread.owner_pid = Some(TEST_APP_PID);

    let idx = crate::sched::allocate_thread(thread).expect("thread table full for test-app");
    crate::sched::enqueue_on_cpu(0, ThreadId(idx as u32), SchedulerClass::Normal);

    crate::kinfo!(
        Compositor,
        "test-app: spawned (pid={}, tid={:#x})",
        TEST_APP_PID.0,
        TEST_APP_TID.0
    );
}

// ---------------------------------------------------------------------------
// Thread entry — runs the full client lifecycle
// ---------------------------------------------------------------------------

/// Test app thread entry point. Never returns; on shutdown (CloseRequested
/// or unrecoverable error) marks itself Dead and yields forever.
pub fn test_app_entry() -> ! {
    // Unmask IRQs so timer-driven preemption + IPC timeouts can fire.
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit only. Safe at EL1.
    // Required so this thread can be preempted by scheduler ticks while
    // blocked in ipc_recv.
    // Maintained by: every kernel-mode thread that wants to receive
    // wake-from-timeout signals must unmask IRQs at entry.
    // Violation: an IRQ-masked thread would never see its receive
    // timeout fire → permanent deadlock if the compositor crashes.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    // Try to bring the app online. If any step fails (compositor not
    // up, OOM, etc.), log the failure and exit cleanly rather than
    // panicking the kernel.
    if let Err(reason) = run_test_app() {
        crate::kwarn!(Compositor, "test-app: shutdown ({})", reason);
    }
    exit_thread();
}

/// Mark the current thread Dead and yield forever. Mirrors the shutdown
/// pattern used by GPU Service and Compositor Service.
fn exit_thread() -> ! {
    let cpu = crate::arch::aarch64::exceptions::core_id() as usize;
    let my_tid = { *crate::task::CURRENT_THREAD[cpu].lock() };
    if let Some(tid) = my_tid {
        let mut table = crate::task::THREAD_TABLE.lock();
        if let Some(thread) = &mut table[tid.0 as usize] {
            thread.sched.state = ThreadState::Dead;
        }
    }
    loop {
        crate::sched::thread_yield();
    }
}

/// Drives the entire test app lifecycle: setup → event loop. Returns
/// `Err(reason)` for clean shutdowns (CloseRequested or fatal IPC error).
fn run_test_app() -> Result<(), &'static str> {
    // --- 1. Create the per-client receive channel ---
    let my_channel =
        crate::ipc::channel_create(TEST_APP_TID).map_err(|_| "channel_create failed")?;
    // The compositor process needs ChannelAccess(my_channel) so it can
    // ipc_send events back to us. Grant it now (we hold the create cap
    // so this transitive grant is authorized).
    let _ =
        crate::cap::grant_to_process(ProcessId(10), Capability::ChannelAccess(my_channel), false);

    // --- 2. Look up the compositor service ---
    let (_, compositor_channel) =
        crate::service::service_lookup(b"compositor").ok_or("compositor service not registered")?;
    // Grant ourselves access to the compositor channel so ipc_call
    // passes its capability check.
    let _ = crate::cap::grant_to_process(
        TEST_APP_PID,
        Capability::ChannelAccess(compositor_channel),
        false,
    );

    crate::kinfo!(
        Compositor,
        "test-app: my_ch={} compositor_ch={}",
        my_channel.0,
        compositor_channel.0
    );

    // --- 3. Send CreateSurface, receive Configure ---
    let create_req = CompositorRequest::create_surface(
        APP_WIDTH,
        APP_HEIGHT,
        b"test-app",
        SurfaceLayer::Normal,
        SurfaceContentType::Generic,
        my_channel.0 as u64,
    );
    let surface_id = call_create_surface(compositor_channel, &create_req)?;
    crate::kinfo!(
        Compositor,
        "test-app: surface created id={} (400x300)",
        surface_id.0
    );

    // --- 4. Allocate a shmem buffer and seed initial content ---
    let pixel_count = (APP_WIDTH as usize) * (APP_HEIGHT as usize);
    let byte_count = pixel_count * BYTES_PER_PIXEL;
    let shmem_id = shared_memory_create(
        TEST_APP_PID,
        byte_count,
        VmFlags::READ.union(VmFlags::WRITE),
    )
    .map_err(|_| "shmem_create failed")?;
    let buffer_va = region_dmap_addr(shmem_id).ok_or("region_dmap_addr returned None")?;
    let buffer_bytes = region_size(shmem_id).ok_or("region_size returned None")?;
    if buffer_bytes < byte_count {
        return Err("shmem region too small");
    }

    // Persistent state: the typed-character buffer (appended on each
    // keypress).
    let mut typed = TypedBuffer::new();
    render_buffer(buffer_va, &typed);

    // --- 5. Attach buffer (transitions Created → Active) ---
    let attach_req =
        CompositorRequest::attach_buffer(surface_id, shmem_id, DamageRegion::FullSurface);
    send_attach_buffer(compositor_channel, &attach_req)?;
    crate::kinfo!(Compositor, "test-app: buffer attached, surface Active");

    // --- 6. Event loop ---
    let mut recv_buf = [0u8; MAX_MESSAGE_SIZE];
    loop {
        let (len, _sender) =
            match crate::ipc::ipc_recv(my_channel, &mut recv_buf, CALL_TIMEOUT_TICKS) {
                Ok(r) => r,
                Err(e) if e == crate::syscall::IpcError::Etimedout as i64 => continue,
                Err(e) if e == crate::syscall::IpcError::Epipe as i64 => {
                    return Err("event channel closed");
                }
                Err(_) => continue,
            };
        if len < core::mem::size_of::<CompositorEvent>() {
            continue;
        }
        // SAFETY: recv_buf is at least sizeof(CompositorEvent) bytes
        // (verified above). CompositorEvent is repr(C) Copy with
        // fully-named padding (M25 lesson) so reading it through
        // read_unaligned is sound.
        // Maintained by: the recv buffer outlives the read; the size
        // check guarantees we don't read past it.
        // Violation: a shorter buffer would read uninitialized memory.
        let event: CompositorEvent =
            unsafe { core::ptr::read_unaligned(recv_buf.as_ptr() as *const CompositorEvent) };
        match CompositorEventTag::from_u32(event.tag) {
            Some(CompositorEventTag::Input) => {
                let dec = match event.input.decode() {
                    Some(e) => e,
                    None => continue,
                };
                if let InputEvent::Keyboard {
                    key,
                    state,
                    modifiers: _,
                } = dec
                {
                    if matches!(state, KeyState::Pressed) {
                        if let Some(byte) = keycode_to_ascii(key) {
                            if typed.push(byte) {
                                render_buffer(buffer_va, &typed);
                                let attach = CompositorRequest::attach_buffer(
                                    surface_id,
                                    shmem_id,
                                    DamageRegion::FullSurface,
                                );
                                send_attach_buffer(compositor_channel, &attach)?;
                            }
                        }
                    }
                }
            }
            Some(CompositorEventTag::CloseRequested) => {
                let destroy = CompositorRequest::destroy_surface(surface_id);
                send_destroy_surface(compositor_channel, &destroy);
                return Err("close requested");
            }
            Some(CompositorEventTag::Configure)
            | Some(CompositorEventTag::FocusChanged)
            | Some(CompositorEventTag::BufferReleased)
            | Some(CompositorEventTag::FramePresented)
            | None => {
                // Ignore other events in M26 — Configure is delivered
                // as the ipc_call reply, not a queued event;
                // FocusChanged/BufferReleased/FramePresented are
                // diagnostics-only for the test app.
            }
        }
    }
}

// ---------------------------------------------------------------------------
// IPC helpers
// ---------------------------------------------------------------------------

fn call_create_surface(
    compositor_ch: ChannelId,
    req: &CompositorRequest,
) -> Result<SurfaceId, &'static str> {
    let req_bytes = request_bytes(req);
    let mut reply = [0u8; MAX_MESSAGE_SIZE];
    let n = crate::ipc::ipc_call(compositor_ch, req_bytes, &mut reply, CALL_TIMEOUT_TICKS);
    if n < 0 {
        return Err("ipc_call(CreateSurface) failed");
    }
    if (n as usize) < core::mem::size_of::<CompositorEvent>() {
        return Err("CreateSurface reply too short");
    }
    // SAFETY: reply buffer holds sizeof(CompositorEvent) bytes
    // (verified). CompositorEvent is repr(C) Copy with explicit
    // padding so read_unaligned is sound.
    // Maintained by: CompositorEvent layout invariants (M25 lesson).
    // Violation: layout drift would corrupt the read.
    let event: CompositorEvent =
        unsafe { core::ptr::read_unaligned(reply.as_ptr() as *const CompositorEvent) };
    match CompositorEventTag::from_u32(event.tag) {
        Some(CompositorEventTag::Configure) => Ok(SurfaceId(event.surface_id)),
        _ => Err("expected Configure reply"),
    }
}

fn send_attach_buffer(
    compositor_ch: ChannelId,
    req: &CompositorRequest,
) -> Result<(), &'static str> {
    let req_bytes = request_bytes(req);
    let mut reply = [0u8; MAX_MESSAGE_SIZE];
    let n = crate::ipc::ipc_call(compositor_ch, req_bytes, &mut reply, CALL_TIMEOUT_TICKS);
    if n < 0 {
        return Err("ipc_call(AttachBuffer) failed");
    }
    Ok(())
}

fn send_destroy_surface(compositor_ch: ChannelId, req: &CompositorRequest) {
    let req_bytes = request_bytes(req);
    let mut reply = [0u8; MAX_MESSAGE_SIZE];
    let _ = crate::ipc::ipc_call(compositor_ch, req_bytes, &mut reply, CALL_TIMEOUT_TICKS);
}

/// Borrow a `CompositorRequest` as `&[u8]` for IPC transport.
///
/// Safe because `CompositorRequest` is `repr(C) Copy` with every byte
/// explicitly named (no implicit padding — M25 lesson).
fn request_bytes(req: &CompositorRequest) -> &[u8] {
    // SAFETY: CompositorRequest is repr(C) Copy with all bytes named
    // via explicit `_pad_*` fields (M25 padding-UB lesson). The slice
    // borrow lives for the duration of `request_bytes` callers' use,
    // which is bounded by the synchronous ipc_call below.
    // Maintained by: CompositorRequest's struct definition keeps every
    // byte named.
    // Violation: a future field added without explicit padding would
    // expose uninitialized memory through this slice.
    unsafe {
        core::slice::from_raw_parts(
            (req as *const CompositorRequest) as *const u8,
            core::mem::size_of::<CompositorRequest>(),
        )
    }
}

// ---------------------------------------------------------------------------
// Rendering — paint the test app's window content
// ---------------------------------------------------------------------------

/// Paint background + welcome text + appended typed characters into the
/// shmem buffer at `buffer_va`.
fn render_buffer(buffer_va: usize, typed: &TypedBuffer) {
    let pixel_count = (APP_WIDTH as usize) * (APP_HEIGHT as usize);
    // SAFETY: `buffer_va` is the kernel direct-map address of the test
    // app's shmem region (allocated above). The slice covers exactly
    // `pixel_count` u32s. The test app is the sole writer for the
    // lifetime of the surface.
    // Maintained by: shmem region lifetime is permanent for the test
    // app's process lifetime; `render_buffer` is only called by the
    // test app's own thread.
    // Violation: writing past `pixel_count` would corrupt adjacent
    // shmem pages.
    let buffer = unsafe { core::slice::from_raw_parts_mut(buffer_va as *mut u32, pixel_count) };
    buffer.fill(APP_BG);

    // Welcome text, centered horizontally.
    let welcome = b"Hello from AIOS!";
    let text_w = welcome.len() as i32 * TITLE_GLYPH_WIDTH;
    let welcome_x = ((APP_WIDTH as i32) - text_w) / 2;
    let welcome_y = 80;
    draw_text_clipped(
        buffer,
        APP_WIDTH,
        APP_HEIGHT,
        welcome_x,
        welcome_y,
        APP_WIDTH as i32,
        welcome,
        APP_FG,
        APP_BG,
    );

    // Typed-text indicator.
    let typed_label = b"You typed:";
    draw_text_clipped(
        buffer,
        APP_WIDTH,
        APP_HEIGHT,
        16,
        140,
        APP_WIDTH as i32,
        typed_label,
        APP_FG,
        APP_BG,
    );
    if typed.len > 0 {
        draw_text_clipped(
            buffer,
            APP_WIDTH,
            APP_HEIGHT,
            16,
            164,
            APP_WIDTH as i32,
            typed.as_slice(),
            APP_FG,
            APP_BG,
        );
    }
}

// ---------------------------------------------------------------------------
// Typed-character buffer — appended on each Pressed Keyboard event
// ---------------------------------------------------------------------------

struct TypedBuffer {
    bytes: [u8; MAX_TYPED_CHARS],
    len: usize,
}

impl TypedBuffer {
    const fn new() -> Self {
        Self {
            bytes: [0; MAX_TYPED_CHARS],
            len: 0,
        }
    }

    fn push(&mut self, byte: u8) -> bool {
        if self.len >= MAX_TYPED_CHARS {
            return false;
        }
        self.bytes[self.len] = byte;
        self.len += 1;
        true
    }

    fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// Map a `KeyCode` to a printable ASCII byte. Returns `None` for
/// non-printable keys (modifiers, function keys, navigation, etc.).
/// M26 deliberately ignores Shift state — the welcome line is
/// lowercase-only.
fn keycode_to_ascii(key: KeyCode) -> Option<u8> {
    Some(match key {
        KeyCode::A => b'a',
        KeyCode::B => b'b',
        KeyCode::C => b'c',
        KeyCode::D => b'd',
        KeyCode::E => b'e',
        KeyCode::F => b'f',
        KeyCode::G => b'g',
        KeyCode::H => b'h',
        KeyCode::I => b'i',
        KeyCode::J => b'j',
        KeyCode::K => b'k',
        KeyCode::L => b'l',
        KeyCode::M => b'm',
        KeyCode::N => b'n',
        KeyCode::O => b'o',
        KeyCode::P => b'p',
        KeyCode::Q => b'q',
        KeyCode::R => b'r',
        KeyCode::S => b's',
        KeyCode::T => b't',
        KeyCode::U => b'u',
        KeyCode::V => b'v',
        KeyCode::W => b'w',
        KeyCode::X => b'x',
        KeyCode::Y => b'y',
        KeyCode::Z => b'z',
        KeyCode::Num0 => b'0',
        KeyCode::Num1 => b'1',
        KeyCode::Num2 => b'2',
        KeyCode::Num3 => b'3',
        KeyCode::Num4 => b'4',
        KeyCode::Num5 => b'5',
        KeyCode::Num6 => b'6',
        KeyCode::Num7 => b'7',
        KeyCode::Num8 => b'8',
        KeyCode::Num9 => b'9',
        KeyCode::Space => b' ',
        KeyCode::Period => b'.',
        KeyCode::Comma => b',',
        _ => return None,
    })
}
