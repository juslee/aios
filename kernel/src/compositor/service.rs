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

use shared::ipc::ChannelId;
use spin::Mutex;

use crate::ipc;
use crate::service;
use crate::task::process::ProcessId;

// ---------------------------------------------------------------------------
// Compositor service state
// ---------------------------------------------------------------------------

/// Compositor service runtime state, held inside the service loop.
///
/// M24 keeps the state minimal — surfaces, render targets, and damage
/// tracking arrive in Steps 11-14.
struct CompositorState {
    /// IPC channel for this service.
    #[allow(dead_code)] // Filled out in Step 14 when the loop dispatches commands.
    channel: ChannelId,
}

impl CompositorState {
    fn new(channel: ChannelId) -> Self {
        Self { channel }
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

    let _state = CompositorState::new(ch);
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
