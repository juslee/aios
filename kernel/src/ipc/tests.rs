//! IPC test initialization and thread entry points.
//!
//! Creates test processes, grants capabilities, and spawns IPC test threads
//! (server, caller, timeout, priority inheritance, capability enforcement).
//! Called from main.rs after sched::init() but before enter_scheduler().

use crate::sched;
use crate::syscall::IpcError;
use crate::task::ThreadId;
use shared::{ChannelId, DEFAULT_TIMEOUT_TICKS, MAX_MESSAGE_SIZE};
use spin::Mutex;

use super::channel::{ipc_call, ipc_recv, ipc_reply};
use super::{channel_create, channel_destroy, channel_set_peer, CHANNEL_TABLE};

// ---------------------------------------------------------------------------
// IPC test initialization
// ---------------------------------------------------------------------------

/// Channel ID shared between IPC test threads (set by init).
static TEST_CHANNEL: Mutex<Option<ChannelId>> = Mutex::new(None);

/// Channel ID for priority inheritance test threads.
static PI_TEST_CHANNEL: Mutex<Option<ChannelId>> = Mutex::new(None);

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
    use crate::task::process::{KernelResourceLimits, ProcessControl, ProcessId, PROCESS_TABLE};
    use crate::task::{CpuSet, SchedulerClass, Thread, THREAD_TABLE};

    // --- Create processes ---

    // Process 0: kernel (owns idle threads, scheduler test threads).
    {
        let mut procs = PROCESS_TABLE.lock();
        let mut name = [0u8; 32];
        name[..6].copy_from_slice(b"kernel");
        procs[0] = Some(ProcessControl {
            pid: ProcessId(0),
            address_space: None,
            resource_limits: KernelResourceLimits::system(),
            cap_table: cap::CapabilityTable::new(),
            thread_ids: [None; 16],
            name,
        });
    }
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
    {
        let mut procs = PROCESS_TABLE.lock();
        let mut name = [0u8; 32];
        name[..8].copy_from_slice(b"ipc-test");
        procs[1] = Some(ProcessControl {
            pid: ProcessId(1),
            address_space: None,
            resource_limits: KernelResourceLimits::native(),
            cap_table: cap::CapabilityTable::new(),
            thread_ids: [None; 16],
            name,
        });
    }
    let _ = cap::grant_to_process(ProcessId(1), shared::Capability::ChannelCreate, true);
    let _ = cap::grant_to_process(ProcessId(1), shared::Capability::DebugPrint, false);

    // Process 2: PI test (priority inheritance server + caller).
    {
        let mut procs = PROCESS_TABLE.lock();
        let mut name = [0u8; 32];
        name[..7].copy_from_slice(b"pi-test");
        procs[2] = Some(ProcessControl {
            pid: ProcessId(2),
            address_space: None,
            resource_limits: KernelResourceLimits::native(),
            cap_table: cap::CapabilityTable::new(),
            thread_ids: [None; 16],
            name,
        });
    }
    let _ = cap::grant_to_process(ProcessId(2), shared::Capability::ChannelCreate, true);
    let _ = cap::grant_to_process(ProcessId(2), shared::Capability::DebugPrint, false);

    // Process 3: cap-test-denied (NO ChannelCreate — used to test enforcement).
    {
        let mut procs = PROCESS_TABLE.lock();
        let mut name = [0u8; 32];
        name[..15].copy_from_slice(b"cap-test-denied");
        procs[3] = Some(ProcessControl {
            pid: ProcessId(3),
            address_space: None,
            resource_limits: KernelResourceLimits::native(),
            cap_table: cap::CapabilityTable::new(),
            thread_ids: [None; 16],
            name,
        });
    }
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

    let ch = channel_create_unchecked(caller_tid);
    channel_set_peer(ch, server_tid).expect("Failed to set IPC channel peer");
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
        thread.sched.affinity = CpuSet::all();
        thread.context.sp = stack_virt_top as u64;
        thread.owner_pid = Some(ProcessId(1));

        let idx = sched::allocate_thread(thread).expect("thread table full for IPC timeout");
        sched::enqueue_on_cpu(2, ThreadId(idx as u32), SchedulerClass::Normal);
    }

    // --- Priority inheritance test threads ---
    {
        let pi_caller_tid = ThreadId(0x300);
        let pi_server_tid = ThreadId(0x301);

        let pi_ch = channel_create_unchecked(pi_caller_tid);
        channel_set_peer(pi_ch, pi_server_tid).expect("Failed to set PI channel peer");
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

/// Create a channel without capability checks (for init-time setup).
/// Used when threads don't exist yet so owner_pid lookup would fail.
pub(crate) fn channel_create_unchecked(owner: ThreadId) -> ChannelId {
    let mut table = CHANNEL_TABLE.lock();
    let idx = table
        .iter()
        .position(|s| s.is_none())
        .expect("channel table full");
    let id = ChannelId(idx as u32);
    table[idx] = Some(super::Channel::new(id, owner));
    crate::kinfo!(
        Ipc,
        "Channel {} created (unchecked) by thread {}",
        idx,
        owner.0
    );
    id
}

// ---------------------------------------------------------------------------
// IPC test thread entry points
// ---------------------------------------------------------------------------

/// IPC server thread: receives requests and sends replies.
fn ipc_server_entry() -> ! {
    // Unmask IRQs — enter_scheduler left them masked when it dispatched us.
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    let ch = loop {
        if let Some(ch) = *TEST_CHANNEL.lock() {
            break ch;
        }
        sched::thread_yield();
    };
    crate::kinfo!(Ipc, "Server: started, channel={}", ch.0);

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
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
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

/// IPC timeout test thread: calls IpcCall on a channel with no receiver.
fn ipc_timeout_entry() -> ! {
    // Unmask IRQs — enter_scheduler left them masked when it dispatched us.
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    // Create a channel with no server — timeout is guaranteed.
    let caller_tid = match super::current_thread_id() {
        Some(t) => t,
        None => loop {
            sched::thread_yield();
        },
    };

    let ch = match channel_create(caller_tid) {
        Ok(c) => c,
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

    // Test channel destroy → EPIPE.
    let ch2 = match channel_create(caller_tid) {
        Ok(c) => c,
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

    loop {
        sched::thread_yield();
    }
}

// ---------------------------------------------------------------------------
// Priority inheritance test threads
// ---------------------------------------------------------------------------

/// PI server: Normal-class server that checks if it was elevated to
/// Interactive during request processing (via priority inheritance).
fn pi_server_entry() -> ! {
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    let ch = loop {
        if let Some(ch) = *PI_TEST_CHANNEL.lock() {
            break ch;
        }
        sched::thread_yield();
    };
    crate::kinfo!(Ipc, "PI-Server: started (Normal class), channel={}", ch.0);

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

    loop {
        sched::thread_yield();
    }
}

/// Capability enforcement test: thread in process 3 (no ChannelCreate cap)
/// attempts to create a channel. Should get EPERM.
fn cap_denied_entry() -> ! {
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
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
        Ok(ch) => {
            crate::kwarn!(
                Cap,
                "Cap: UNEXPECTED: unauthorized ChannelCreate succeeded (ch={})",
                ch.0
            );
        }
        Err(e) if e == crate::syscall::IpcError::Eperm as i64 => {
            crate::kinfo!(Cap, "Cap: unauthorized ChannelCreate -> EPERM (expected)");
        }
        Err(e) => {
            crate::kwarn!(Cap, "Cap: unexpected error {} on ChannelCreate", e);
        }
    }

    loop {
        sched::thread_yield();
    }
}

/// PI caller: Interactive-class caller that exercises priority inheritance.
fn pi_caller_entry() -> ! {
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
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

    loop {
        sched::thread_yield();
    }
}
