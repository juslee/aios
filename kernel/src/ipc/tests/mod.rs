//! IPC test initialization and thread entry points.
//!
//! Creates test processes, grants capabilities, and spawns IPC test threads
//! (server, caller, timeout, priority inheritance, capability enforcement).
//! Called from main.rs after sched::init() but before enter_scheduler().

mod bad_pid;
mod create_abi;
mod dead_thread;
mod kit_errors;
mod lifecycle;
mod select_cap;
mod share;
mod stale_id;
mod syscall_args;
mod timeout_args;
mod wait_loops;
mod wake_helper;
mod wake_token;

use crate::sched;
use crate::syscall::IpcError;
use crate::task::process::ProcessId;
use crate::task::ThreadId;
use shared::{ChannelId, DEFAULT_TIMEOUT_TICKS, MAX_CHANNELS, MAX_MESSAGE_SIZE};
use spin::Mutex;

use super::channel::{ipc_call, ipc_recv, ipc_reply, ipc_send};
use super::{channel_create, channel_create_unchecked, channel_destroy, channel_set_peer};

// ---------------------------------------------------------------------------
// IPC test initialization
// ---------------------------------------------------------------------------

/// Channel ID shared between IPC test threads (set by init).
static TEST_CHANNEL: Mutex<Option<ChannelId>> = Mutex::new(None);

/// Channel ID for priority inheritance test threads.
static PI_TEST_CHANNEL: Mutex<Option<ChannelId>> = Mutex::new(None);

/// Process 1 ("ipc-test") owns the IPC server, caller and timeout threads.
/// The self-tests that grant or revoke capabilities check they run in it.
const TEST_PID: shared::ProcessId = shared::ProcessId(1);

/// Initialize processes, grant capabilities, create IPC test threads.
///
/// Called from main.rs after sched::init() but before enter_scheduler().
///
/// Creates:
/// - Process 0 ("kernel"): owns idle + scheduler test threads, all caps
/// - Process 1 ("ipc-test"): owns IPC server/caller/timeout, IPC caps
/// - Process 2 ("pi-test"): owns PI server/caller, IPC caps
/// - Process 3 ("cap-test-denied"): NO ChannelCreate cap (for enforcement test)
pub fn init() {
    use crate::cap;
    use crate::task::process::{process_install, KernelResourceLimits};
    use crate::task::{CpuSet, SchedulerClass, Thread, THREAD_TABLE};

    // --- Create processes ---

    // Process 0: kernel (owns idle threads, scheduler test threads).
    process_install(
        ProcessId(0),
        None,
        b"kernel",
        KernelResourceLimits::system(),
    )
    .expect("process 0 install");
    // Grant kernel process all capability types.
    let _ = cap::grant_to_process(ProcessId(0), shared::Capability::ChannelCreate, true);
    let _ = cap::grant_to_process(ProcessId(0), shared::Capability::DebugPrint, true);
    let _ = cap::grant_to_process(ProcessId(0), shared::Capability::SpawnAgent, true);
    let _ = cap::grant_to_process(ProcessId(0), shared::Capability::SharedMemoryCreate, true);

    // Assign existing idle + test threads to kernel process.
    {
        let mut table = THREAD_TABLE.lock();
        for thread in table.iter_mut().flatten() {
            if thread.owner_pid.is_none() {
                thread.owner_pid = Some(ProcessId(0));
            }
        }
    }

    // Process 1: IPC test (server, caller, timeout).
    process_install(
        ProcessId(1),
        None,
        b"ipc-test",
        KernelResourceLimits::native(),
    )
    .expect("process 1 install");
    let _ = cap::grant_to_process(ProcessId(1), shared::Capability::ChannelCreate, true);
    let _ = cap::grant_to_process(ProcessId(1), shared::Capability::DebugPrint, false);

    // Process 2: PI test (priority inheritance server + caller).
    process_install(
        ProcessId(2),
        None,
        b"pi-test",
        KernelResourceLimits::native(),
    )
    .expect("process 2 install");
    let _ = cap::grant_to_process(ProcessId(2), shared::Capability::ChannelCreate, true);
    let _ = cap::grant_to_process(ProcessId(2), shared::Capability::DebugPrint, false);

    // Process 3: cap-test-denied (NO ChannelCreate — used to test enforcement).
    process_install(
        ProcessId(3),
        None,
        b"cap-test-denied",
        KernelResourceLimits::native(),
    )
    .expect("process 3 install");
    // Process 3 gets DebugPrint only — no ChannelCreate, no ChannelAccess.
    let _ = cap::grant_to_process(ProcessId(3), shared::Capability::DebugPrint, false);

    crate::kinfo!(Cap, "Processes 0-3 created with capabilities");

    // --- Create IPC test channel ---
    // (channel_create now checks caps — process 1 holds ChannelCreate)

    // We create the channel on behalf of the caller thread (process 1).
    // For init-time channels, we temporarily bypass cap checks by using
    // channel_create_unchecked (the cap check inside channel_create would
    // fail because the thread doesn't exist yet to look up owner_pid).
    let caller_tid = ThreadId(0x200);
    let server_tid = ThreadId(0x201);

    // Both endpoints belong to process 1 (the caller and server threads).
    let ch = channel_create_unchecked(ProcessId(1));
    channel_set_peer(ch, ProcessId(1)).expect("Failed to set IPC channel peer");
    *TEST_CHANNEL.lock() = Some(ch);

    // Grant ChannelAccess for the test channel to process 1.
    let _ = cap::grant_to_process(ProcessId(1), shared::Capability::ChannelAccess(ch), false);

    // Create server thread (receives requests, sends replies).
    {
        let stack_phys = sched::alloc_kernel_stack();
        let stack_virt_top = sched::phys_to_virt(stack_phys) + sched::STACK_SIZE;

        let mut thread = Thread::new_kernel(
            server_tid,
            b"ipc-server\0\0\0\0\0\0",
            ipc_server_entry as *const () as usize,
            stack_phys,
        );
        thread.sched.class = SchedulerClass::Normal;
        thread.sched.effective_class = SchedulerClass::Normal;
        thread.sched.affinity = CpuSet::all();
        thread.context.sp = stack_virt_top as u64;
        thread.owner_pid = Some(ProcessId(1));

        let idx = sched::allocate_thread(thread).expect("thread table full for IPC server");
        sched::enqueue_on_cpu(0, ThreadId(idx as u32), SchedulerClass::Normal);
    }

    // Create caller thread (sends requests, receives replies).
    {
        let stack_phys = sched::alloc_kernel_stack();
        let stack_virt_top = sched::phys_to_virt(stack_phys) + sched::STACK_SIZE;

        let mut thread = Thread::new_kernel(
            caller_tid,
            b"ipc-caller\0\0\0\0\0\0",
            ipc_caller_entry as *const () as usize,
            stack_phys,
        );
        thread.sched.class = SchedulerClass::Normal;
        thread.sched.effective_class = SchedulerClass::Normal;
        thread.sched.affinity = CpuSet::all();
        thread.context.sp = stack_virt_top as u64;
        thread.owner_pid = Some(ProcessId(1));

        let idx = sched::allocate_thread(thread).expect("thread table full for IPC caller");
        sched::enqueue_on_cpu(1, ThreadId(idx as u32), SchedulerClass::Normal);
    }

    // Create timeout test thread (calls IpcCall with no server → timeout).
    {
        let stack_phys = sched::alloc_kernel_stack();
        let stack_virt_top = sched::phys_to_virt(stack_phys) + sched::STACK_SIZE;

        let mut thread = Thread::new_kernel(
            ThreadId(0x202),
            b"ipc-timeout\0\0\0\0\0",
            ipc_timeout_entry as *const () as usize,
            stack_phys,
        );
        thread.sched.class = SchedulerClass::Normal;
        thread.sched.effective_class = SchedulerClass::Normal;
        // Pinned to CPU 2 from creation. A timeout wake from check_timeouts
        // may run on any ticking CPU, and unblock queues the woken thread on
        // the waking CPU when affinity allows; pinned, it always comes back
        // to CPU 2, whose own tick preempts at slice expiry, instead of
        // starving behind the Interactive bench threads on CPU 0.
        thread.sched.affinity = CpuSet::single(2);
        thread.context.sp = stack_virt_top as u64;
        thread.owner_pid = Some(ProcessId(1));

        let idx = sched::allocate_thread(thread).expect("thread table full for IPC timeout");
        sched::enqueue_on_cpu(2, ThreadId(idx as u32), SchedulerClass::Normal);
    }

    // Dead test helper (process 1, pinned to CPU 2): ends itself when it
    // first runs; the ipc-timeout thread probes it.
    dead_thread::init();

    // Lifecycle test: processes 14 and 17, and pid 14's thread (pinned to
    // CPU 2), which exits its own process when the test starts it.
    lifecycle::init();

    // Wake helper of the loop tests (process 1, pinned to CPU 2): acts on
    // the ipc-timeout thread's waits while they wait.
    wake_helper::init();

    // --- Priority inheritance test threads ---
    {
        let pi_caller_tid = ThreadId(0x300);
        let pi_server_tid = ThreadId(0x301);

        // Both endpoints belong to process 2 (the PI caller and server).
        let pi_ch = channel_create_unchecked(ProcessId(2));
        channel_set_peer(pi_ch, ProcessId(2)).expect("Failed to set PI channel peer");
        *PI_TEST_CHANNEL.lock() = Some(pi_ch);

        // Grant ChannelAccess for PI channel to process 2.
        let _ = cap::grant_to_process(
            ProcessId(2),
            shared::Capability::ChannelAccess(pi_ch),
            false,
        );

        // Normal-class server.
        {
            let stack_phys = sched::alloc_kernel_stack();
            let stack_virt_top = sched::phys_to_virt(stack_phys) + sched::STACK_SIZE;

            let mut thread = Thread::new_kernel(
                pi_server_tid,
                b"pi-server\0\0\0\0\0\0\0",
                pi_server_entry as *const () as usize,
                stack_phys,
            );
            thread.sched.class = SchedulerClass::Normal;
            thread.sched.effective_class = SchedulerClass::Normal;
            thread.sched.affinity = CpuSet::all();
            thread.context.sp = stack_virt_top as u64;
            thread.owner_pid = Some(ProcessId(2));

            let idx = sched::allocate_thread(thread).expect("thread table full for PI server");
            sched::enqueue_on_cpu(3, ThreadId(idx as u32), SchedulerClass::Normal);
        }

        // Interactive-class caller.
        {
            let stack_phys = sched::alloc_kernel_stack();
            let stack_virt_top = sched::phys_to_virt(stack_phys) + sched::STACK_SIZE;

            let mut thread = Thread::new_kernel(
                pi_caller_tid,
                b"pi-caller\0\0\0\0\0\0\0",
                pi_caller_entry as *const () as usize,
                stack_phys,
            );
            thread.sched.class = SchedulerClass::Interactive;
            thread.sched.effective_class = SchedulerClass::Interactive;
            thread.sched.affinity = CpuSet::all();
            thread.context.sp = stack_virt_top as u64;
            thread.owner_pid = Some(ProcessId(2));

            let idx = sched::allocate_thread(thread).expect("thread table full for PI caller");
            sched::enqueue_on_cpu(3, ThreadId(idx as u32), SchedulerClass::Interactive);
        }
    }

    // --- Capability enforcement test thread ---
    // Process 3 has NO ChannelCreate cap. This thread attempts channel_create
    // and expects EPERM.
    {
        let stack_phys = sched::alloc_kernel_stack();
        let stack_virt_top = sched::phys_to_virt(stack_phys) + sched::STACK_SIZE;

        let mut thread = Thread::new_kernel(
            ThreadId(0x400),
            b"cap-denied\0\0\0\0\0\0",
            cap_denied_entry as *const () as usize,
            stack_phys,
        );
        thread.sched.class = SchedulerClass::Normal;
        thread.sched.effective_class = SchedulerClass::Normal;
        thread.sched.affinity = CpuSet::all();
        thread.context.sp = stack_virt_top as u64;
        thread.owner_pid = Some(ProcessId(3));

        let idx = sched::allocate_thread(thread).expect("thread table full for cap-denied");
        sched::enqueue_on_cpu(0, ThreadId(idx as u32), SchedulerClass::Normal);
    }

    crate::kinfo!(
        Ipc,
        "IPC test threads created (server, caller, timeout, PI, cap-denied)"
    );
}

// ---------------------------------------------------------------------------
// IPC test thread entry points
// ---------------------------------------------------------------------------

/// IPC server thread: receives requests and sends replies.
fn ipc_server_entry() -> ! {
    // Unmask IRQs — enter_scheduler left them masked when it dispatched us.
    // SAFETY: Clearing DAIF.I unmasks IRQs on this CPU only and touches no
    // memory. This thread holds no lock yet, and VBAR_EL1 on this CPU already
    // points at the Rust vector table, so an IRQ reaches irq_el1_entry.
    // kernel_main (CPU 0) and _secondary_entry in boot.S (CPUs 1-3) install
    // the vectors before any thread runs; the scheduler starts this thread
    // with IRQs masked.
    // Unmasking before the vectors were installed would send the next IRQ to
    // a stale vector and halt the CPU; unmasking while holding a lock that the
    // IRQ path takes could deadlock this CPU.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    let ch = loop {
        if let Some(ch) = *TEST_CHANNEL.lock() {
            break ch;
        }
        sched::thread_yield();
    };
    crate::kinfo!(Ipc, "Server: started, channel={}", ch);

    let mut recv_buf = [0u8; MAX_MESSAGE_SIZE];

    for i in 0..5u32 {
        match ipc_recv(ch, &mut recv_buf, DEFAULT_TIMEOUT_TICKS) {
            Ok((len, sender)) => {
                crate::kinfo!(
                    Ipc,
                    "Server: recv {} bytes from thread {} iter={}",
                    len,
                    sender.0,
                    i
                );

                // Echo back with "REPLY:" prefix.
                let mut reply = [0u8; MAX_MESSAGE_SIZE];
                let prefix = b"REPLY:";
                let reply_len = (prefix.len() + len).min(MAX_MESSAGE_SIZE);
                reply[..prefix.len()].copy_from_slice(prefix);
                let data_len = reply_len - prefix.len();
                reply[prefix.len()..reply_len].copy_from_slice(&recv_buf[..data_len]);

                let result = ipc_reply(ch, &reply[..reply_len]);
                if result < 0 {
                    crate::kwarn!(Ipc, "Server: reply failed with {}", result);
                }
            }
            Err(e) => {
                crate::kwarn!(Ipc, "Server: recv failed with {} iter={}", e, i);
            }
        }
    }

    // Keep yielding forever after test iterations.
    loop {
        sched::thread_yield();
    }
}

/// IPC caller thread: sends requests and receives replies.
fn ipc_caller_entry() -> ! {
    // Unmask IRQs — enter_scheduler left them masked when it dispatched us.
    // SAFETY: Clearing DAIF.I unmasks IRQs on this CPU only and touches no
    // memory. This thread holds no lock yet, and VBAR_EL1 on this CPU already
    // points at the Rust vector table, so an IRQ reaches irq_el1_entry.
    // kernel_main (CPU 0) and _secondary_entry in boot.S (CPUs 1-3) install
    // the vectors before any thread runs; the scheduler starts this thread
    // with IRQs masked.
    // Unmasking before the vectors were installed would send the next IRQ to
    // a stale vector and halt the CPU; unmasking while holding a lock that the
    // IRQ path takes could deadlock this CPU.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    let ch = loop {
        if let Some(ch) = *TEST_CHANNEL.lock() {
            break ch;
        }
        sched::thread_yield();
    };
    for i in 0..5u32 {
        let msg = b"PING";
        let mut reply_buf = [0u8; MAX_MESSAGE_SIZE];

        let start = crate::arch::aarch64::timer::read_counter();
        let result = ipc_call(ch, msg, &mut reply_buf, DEFAULT_TIMEOUT_TICKS);
        let end = crate::arch::aarch64::timer::read_counter();

        if result >= 0 {
            let elapsed_ticks = end.wrapping_sub(start);
            // Convert to nanoseconds: ticks * 1_000_000_000 / 62_500_000 = ticks * 16
            let elapsed_ns = elapsed_ticks * 16;

            let reply_len = result as usize;
            let reply_str =
                core::str::from_utf8(&reply_buf[..reply_len]).unwrap_or("<invalid utf8>");
            crate::kinfo!(
                Ipc,
                "Caller: got '{}' in {} ns ({}us) iter={}",
                reply_str,
                elapsed_ns,
                elapsed_ns / 1000,
                i
            );

            #[cfg(feature = "kernel-metrics")]
            crate::observability::metrics::METRICS
                .ipc_roundtrip_ns
                .observe(elapsed_ns);
        } else {
            crate::kwarn!(Ipc, "Caller: ipc_call failed with {} iter={}", result, i);
        }

        sched::thread_yield();
    }

    loop {
        sched::thread_yield();
    }
}

/// IPC timeout test thread, pinned to CPU 2. It checks EPIPE after channel
/// destroy, EINVAL for out-of-range channel ids, EINVAL for out-of-range pids
/// on the SharedMemoryShare path (`shm_bad_pid_test`), the IpcSelect
/// capability check (`select_cap_test`), EINVAL for hostile syscall
/// arguments, MemoryUnmap ownership and the shared memory EINVAL/EPERM split
/// (`syscall_args_test`, whose bad-buffer checks use `select_cap_test`'s
/// `owned_a`), the IPC Kit's error variants through `KernelIpc`
/// (`kit_errors_test`, on all three of `select_cap_test`'s channels), EPIPE
/// for stale channel and region ids whose slot was reused, and the region
/// cascade of a SharedMemoryCreate revoke with a kernel borrow held
/// (`stale_id_test`), the x0/x1 return of ChannelCreate and
/// SharedMemoryCreate with their minted tokens (`create_abi_test`), and
/// SharedMemoryShare as delegation and the Kit destroy's creator check
/// (`share_test`, which grants into process 2), and that a thread ended by
/// `exit_current` stays Dead (`dead_thread_test`, on a helper pinned to
/// CPU 2), and that a wake between a thread's arm and its block is kept as a
/// token that the block consumes (`wake_token_test`, on this thread's own
/// tid, which the pin to CPU 2 makes safe to read), and the process
/// lifecycle: exit, wait and reap, the exit cascade, one waiter per child,
/// the token on the wait path, and the exit walk over channel endpoints
/// (`lifecycle_test`, on pids 12-18), and that `notification_wait` and
/// `ipc_select` return on their own condition, failure or deadline whatever
/// wakes them (`notify_loop_test`, `select_loop_test`, with the wake helper
/// pinned to CPU 2; their timed-out cases take about 2 s each, so the lines
/// after them print after the bench). Last, it calls IpcCall on a channel
/// with no receiver (expects ETIMEDOUT): the call holds the channel's minted
/// ChannelAccess, so it blocks until its 100-tick timeout, and a late timeout
/// wake can hide only that line.
fn ipc_timeout_entry() -> ! {
    // Unmask IRQs — enter_scheduler left them masked when it dispatched us.
    // SAFETY: Clearing DAIF.I unmasks IRQs on this CPU only and touches no
    // memory. This thread holds no lock yet, and VBAR_EL1 on this CPU already
    // points at the Rust vector table, so an IRQ reaches irq_el1_entry.
    // kernel_main (CPU 0) and _secondary_entry in boot.S (CPUs 1-3) install
    // the vectors before any thread runs; the scheduler starts this thread
    // with IRQs masked.
    // Unmasking before the vectors were installed would send the next IRQ to
    // a stale vector and halt the CPU; unmasking while holding a lock that the
    // IRQ path takes could deadlock this CPU.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    let caller_tid = match super::current_thread_id() {
        Some(t) => t,
        None => loop {
            sched::thread_yield();
        },
    };

    // Test channel destroy → EPIPE. The minted ChannelAccess(ch2) stays live
    // after the destroy, so the recv passes the capability check and fails
    // at the lookup.
    let ch2 = match channel_create(caller_tid) {
        Ok((c, _access)) => c,
        Err(_) => loop {
            sched::thread_yield();
        },
    };
    // Destroy channel, then try to recv — should get EPIPE.
    let _ = channel_destroy(ch2);
    let mut buf = [0u8; 64];
    let result = ipc_recv(ch2, &mut buf, 0);
    match result {
        Err(e) if e == IpcError::Epipe as i64 => {
            crate::kinfo!(Ipc, "Destroy test: EPIPE as expected");
        }
        _ => {
            crate::kwarn!(Ipc, "Destroy test: unexpected result {:?}", result);
        }
    }

    // Out-of-range channel id → EINVAL, not an index-out-of-bounds panic.
    // recv and send are rejected by check_channel_access. reply has no
    // capability check, so it exercises the CHANNEL_TABLE lookup itself.
    // A log message keeps at most 96 bytes (two ring entries); longer text
    // is cut and marked with `~`, so keep these lines short.
    let einval = IpcError::Einval as i64;
    let recv_result = ipc_recv(ChannelId::from_raw(MAX_CHANNELS as u32), &mut buf, 0);
    let send_result = ipc_send(ChannelId::from_raw(MAX_CHANNELS as u32), b"BAD_ID");
    let reply_result = ipc_reply(ChannelId::from_raw(u32::MAX), b"BAD_ID");
    if recv_result == Err(einval) && send_result == einval && reply_result == einval {
        crate::kinfo!(Ipc, "Bad-id test: EINVAL as expected");
    } else {
        crate::kwarn!(
            Ipc,
            "Bad-id test: recv={:?} send={} reply={}",
            recv_result,
            send_result,
            reply_result
        );
    }

    bad_pid::shm_bad_pid_test(caller_tid);
    let select_channels = select_cap::select_cap_test(caller_tid);
    syscall_args::syscall_args_test(caller_tid, select_channels.map(|c| c.owned_a));
    kit_errors::kit_errors_test(caller_tid, select_channels);
    stale_id::stale_id_test(caller_tid);
    create_abi::create_abi_test(caller_tid);
    share::share_test(caller_tid);
    dead_thread::dead_thread_test();
    wake_token::wake_token_test(caller_tid);
    lifecycle::lifecycle_test(caller_tid);
    wait_loops::notify_loop_test(caller_tid);
    wait_loops::select_loop_test(caller_tid);

    // Timeout test, last. The channel has no server, so the call, authorised
    // by the minted ChannelAccess, can only time out.
    let ch = match channel_create(caller_tid) {
        Ok((c, _access)) => c,
        Err(e) => {
            crate::kwarn!(Ipc, "Timeout test: channel_create failed: {}", e);
            loop {
                sched::thread_yield();
            }
        }
    };

    let msg = b"TIMEOUT_TEST";
    let mut reply_buf = [0u8; MAX_MESSAGE_SIZE];

    // Use a short timeout (100 ticks = 100ms).
    crate::kinfo!(Ipc, "Timeout test: calling with 100ms timeout...");
    let result = ipc_call(ch, msg, &mut reply_buf, 100);

    if result == IpcError::Etimedout as i64 {
        crate::kinfo!(Ipc, "Timeout test: ETIMEDOUT as expected");
    } else {
        crate::kwarn!(Ipc, "Timeout test: unexpected result {}", result);
    }

    loop {
        sched::thread_yield();
    }
}

// ---------------------------------------------------------------------------
// Priority inheritance test threads
// ---------------------------------------------------------------------------

/// PI server: Normal-class server that checks if it was elevated to
/// Interactive during request processing (via priority inheritance). After
/// its three round trips it answers the two calls of the timeout-argument
/// test (`timeout_args::server_side`).
fn pi_server_entry() -> ! {
    // SAFETY: Clearing DAIF.I unmasks IRQs on this CPU only and touches no
    // memory. This thread holds no lock yet, and VBAR_EL1 on this CPU already
    // points at the Rust vector table, so an IRQ reaches irq_el1_entry.
    // kernel_main (CPU 0) and _secondary_entry in boot.S (CPUs 1-3) install
    // the vectors before any thread runs; the scheduler starts this thread
    // with IRQs masked.
    // Unmasking before the vectors were installed would send the next IRQ to
    // a stale vector and halt the CPU; unmasking while holding a lock that the
    // IRQ path takes could deadlock this CPU.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    let ch = loop {
        if let Some(ch) = *PI_TEST_CHANNEL.lock() {
            break ch;
        }
        sched::thread_yield();
    };
    crate::kinfo!(Ipc, "PI-Server: started (Normal class), channel={}", ch);

    let mut recv_buf = [0u8; MAX_MESSAGE_SIZE];

    for i in 0..3u32 {
        match ipc_recv(ch, &mut recv_buf, DEFAULT_TIMEOUT_TICKS) {
            Ok((len, sender)) => {
                // Check our effective class — should be elevated to Interactive
                // if priority inheritance is working.
                let my_tid = super::current_thread_id().unwrap_or(ThreadId(0));
                let effective_class = {
                    let table = crate::task::THREAD_TABLE.lock();
                    table[my_tid.0 as usize]
                        .as_ref()
                        .map(|t| t.sched.effective_class)
                };

                if let Some(eff) = effective_class {
                    let class_name = match eff {
                        crate::task::SchedulerClass::RealTime => "RT",
                        crate::task::SchedulerClass::Interactive => "Interactive",
                        crate::task::SchedulerClass::Normal => "Normal",
                        crate::task::SchedulerClass::Idle => "Idle",
                    };
                    crate::kinfo!(
                        Ipc,
                        "PI-Server: recv {} bytes from {}, effective_class={} iter={}",
                        len,
                        sender.0,
                        class_name,
                        i
                    );
                }

                // Reply with class info.
                let mut reply = [0u8; MAX_MESSAGE_SIZE];
                let prefix = b"PI-OK:";
                let reply_len = (prefix.len() + len).min(MAX_MESSAGE_SIZE);
                reply[..prefix.len()].copy_from_slice(prefix);
                let data_len = reply_len - prefix.len();
                reply[prefix.len()..reply_len].copy_from_slice(&recv_buf[..data_len]);

                let result = ipc_reply(ch, &reply[..reply_len]);
                if result < 0 {
                    crate::kwarn!(Ipc, "PI-Server: reply failed with {}", result);
                }

                // After reply, check our class is restored to Normal.
                let restored_class = {
                    let table = crate::task::THREAD_TABLE.lock();
                    table[my_tid.0 as usize]
                        .as_ref()
                        .map(|t| t.sched.effective_class)
                };
                if let Some(eff) = restored_class {
                    let class_name = match eff {
                        crate::task::SchedulerClass::RealTime => "RT",
                        crate::task::SchedulerClass::Interactive => "Interactive",
                        crate::task::SchedulerClass::Normal => "Normal",
                        crate::task::SchedulerClass::Idle => "Idle",
                    };
                    crate::kinfo!(
                        Ipc,
                        "PI-Server: after reply, effective_class={} iter={}",
                        class_name,
                        i
                    );
                }
            }
            Err(e) => {
                crate::kwarn!(Ipc, "PI-Server: recv failed with {} iter={}", e, i);
            }
        }
    }

    timeout_args::server_side(ch);

    loop {
        sched::thread_yield();
    }
}

/// Capability enforcement test: thread in process 3 (no ChannelCreate cap)
/// attempts to create a channel. Should get EPERM.
fn cap_denied_entry() -> ! {
    // SAFETY: Clearing DAIF.I unmasks IRQs on this CPU only and touches no
    // memory. This thread holds no lock yet, and VBAR_EL1 on this CPU already
    // points at the Rust vector table, so an IRQ reaches irq_el1_entry.
    // kernel_main (CPU 0) and _secondary_entry in boot.S (CPUs 1-3) install
    // the vectors before any thread runs; the scheduler starts this thread
    // with IRQs masked.
    // Unmasking before the vectors were installed would send the next IRQ to
    // a stale vector and halt the CPU; unmasking while holding a lock that the
    // IRQ path takes could deadlock this CPU.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    // Small delay to let other threads initialize.
    for _ in 0..5 {
        sched::thread_yield();
    }

    let my_tid = match super::current_thread_id() {
        Some(t) => t,
        None => loop {
            sched::thread_yield();
        },
    };

    // Attempt channel_create — should fail with EPERM because process 3
    // does not hold ChannelCreate capability.
    match channel_create(my_tid) {
        Ok((ch, _access)) => {
            crate::kwarn!(
                Cap,
                "UNEXPECTED: unauthorized ChannelCreate succeeded (ch={})",
                ch
            );
        }
        Err(e) if e == crate::syscall::IpcError::Eperm as i64 => {
            crate::kinfo!(Cap, "unauthorized ChannelCreate -> EPERM (expected)");
        }
        Err(e) => {
            crate::kwarn!(Cap, "unexpected error {} on ChannelCreate", e);
        }
    }

    loop {
        sched::thread_yield();
    }
}

/// PI caller: Interactive-class caller that exercises priority inheritance.
/// After its three round trips it makes the two calls of the
/// timeout-argument test (`timeout_args::call_side`).
fn pi_caller_entry() -> ! {
    // SAFETY: Clearing DAIF.I unmasks IRQs on this CPU only and touches no
    // memory. This thread holds no lock yet, and VBAR_EL1 on this CPU already
    // points at the Rust vector table, so an IRQ reaches irq_el1_entry.
    // kernel_main (CPU 0) and _secondary_entry in boot.S (CPUs 1-3) install
    // the vectors before any thread runs; the scheduler starts this thread
    // with IRQs masked.
    // Unmasking before the vectors were installed would send the next IRQ to
    // a stale vector and halt the CPU; unmasking while holding a lock that the
    // IRQ path takes could deadlock this CPU.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    let ch = loop {
        if let Some(ch) = *PI_TEST_CHANNEL.lock() {
            break ch;
        }
        sched::thread_yield();
    };

    // Small delay to let server start first and enter ipc_recv().
    for _ in 0..3 {
        sched::thread_yield();
    }

    for i in 0..3u32 {
        let msg = b"PI-PING";
        let mut reply_buf = [0u8; MAX_MESSAGE_SIZE];

        let start = crate::arch::aarch64::timer::read_counter();
        let result = ipc_call(ch, msg, &mut reply_buf, DEFAULT_TIMEOUT_TICKS);
        let end = crate::arch::aarch64::timer::read_counter();

        if result >= 0 {
            let elapsed_ticks = end.wrapping_sub(start);
            let elapsed_ns = elapsed_ticks * 16;
            let reply_len = result as usize;
            let reply_str =
                core::str::from_utf8(&reply_buf[..reply_len]).unwrap_or("<invalid utf8>");
            crate::kinfo!(
                Ipc,
                "PI-Caller(Interactive): got '{}' in {}us iter={}",
                reply_str,
                elapsed_ns / 1000,
                i
            );
        } else {
            crate::kwarn!(Ipc, "PI-Caller: ipc_call failed with {} iter={}", result, i);
        }

        sched::thread_yield();
    }

    timeout_args::call_side(ch);

    loop {
        sched::thread_yield();
    }
}
