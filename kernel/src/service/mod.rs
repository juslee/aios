//! Minimal service manager: registry, audit ring, echo service.
//!
//! Provides service registration, lookup, death notification, and
//! an audit ring for security-relevant events.

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::ipc::{self, ChannelId};
use crate::sched;
use crate::task::process::ProcessId;
use crate::task::{CpuSet, SchedulerClass, Thread, ThreadId, ThreadState};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Service registry
// ---------------------------------------------------------------------------

use shared::{ServiceName, ServiceState, MAX_SERVICES};

/// Entry in the service registry.
pub struct ServiceEntry {
    pub name: ServiceName,
    pub pid: ProcessId,
    pub channel: ChannelId,
    pub state: ServiceState,
}

/// Service manager holding the registry.
pub struct ServiceManager {
    services: [Option<ServiceEntry>; MAX_SERVICES],
    count: usize,
}

impl ServiceManager {
    const fn new() -> Self {
        const NONE: Option<ServiceEntry> = None;
        Self {
            services: [NONE; MAX_SERVICES],
            count: 0,
        }
    }
}

/// Global service manager.
pub static SERVICE_MANAGER: Mutex<ServiceManager> = Mutex::new(ServiceManager::new());

/// Register a service in the registry.
pub fn service_register(name: &[u8], pid: ProcessId, channel: ChannelId) -> Result<(), i64> {
    let mut mgr = SERVICE_MANAGER.lock();
    let svc_name = ServiceName::from_bytes(name);

    // Check for duplicate name.
    for entry in mgr.services.iter().flatten() {
        if entry.name == svc_name {
            return Err(crate::syscall::IpcError::Enospc as i64);
        }
    }

    // Find free slot.
    let slot = mgr
        .services
        .iter()
        .position(|s| s.is_none())
        .ok_or(crate::syscall::IpcError::Enospc as i64)?;

    mgr.services[slot] = Some(ServiceEntry {
        name: svc_name,
        pid,
        channel,
        state: ServiceState::Running,
    });
    mgr.count += 1;

    let name_str = core::str::from_utf8(svc_name.as_bytes()).unwrap_or("<binary>");
    crate::kinfo!(
        Ipc,
        "Service '{}' registered (pid={}, ch={})",
        name_str,
        pid.0,
        channel.0
    );
    Ok(())
}

/// Look up a service by name. Returns (pid, channel) if found and running.
pub fn service_lookup(name: &[u8]) -> Option<(ProcessId, ChannelId)> {
    let mgr = SERVICE_MANAGER.lock();
    for entry in mgr.services.iter().flatten() {
        if entry.state == ServiceState::Running && entry.name.matches(name) {
            return Some((entry.pid, entry.channel));
        }
    }
    None
}

/// Mark a service as dead when its owning process exits.
pub fn service_on_death(pid: ProcessId) {
    let mut mgr = SERVICE_MANAGER.lock();
    for entry in mgr.services.iter_mut().flatten() {
        if entry.pid == pid && entry.state == ServiceState::Running {
            entry.state = ServiceState::Dead;
            let name_str = core::str::from_utf8(entry.name.as_bytes()).unwrap_or("<binary>");
            crate::kinfo!(Ipc, "Service '{}' marked dead (pid={})", name_str, pid.0);

            // Drop the manager lock before calling audit_log to avoid potential
            // lock ordering issues.
            let mut event = [0u8; 48];
            let msg = b"service_death";
            let len = msg.len().min(48);
            event[..len].copy_from_slice(&msg[..len]);
            drop(mgr);
            audit_log(pid, &event[..len]);
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// Audit ring
// ---------------------------------------------------------------------------

/// Maximum audit ring entries.
const AUDIT_RING_SIZE: usize = 256;

/// Audit log entry.
#[allow(dead_code)]
pub struct AuditEntry {
    pub timestamp: u64,
    pub pid: u32,
    pub event: [u8; 48],
    pub event_len: usize,
}

impl AuditEntry {
    const fn empty() -> Self {
        Self {
            timestamp: 0,
            pid: 0,
            event: [0u8; 48],
            event_len: 0,
        }
    }
}

/// Global audit ring buffer.
static AUDIT_RING: Mutex<[AuditEntry; AUDIT_RING_SIZE]> = {
    #[allow(clippy::declare_interior_mutable_const)]
    const ENTRY: AuditEntry = AuditEntry::empty();
    Mutex::new([ENTRY; AUDIT_RING_SIZE])
};

/// Current write position in the audit ring.
static AUDIT_HEAD: AtomicUsize = AtomicUsize::new(0);

/// Append an event to the audit ring.
pub fn audit_log(pid: ProcessId, event: &[u8]) {
    let idx = AUDIT_HEAD.fetch_add(1, Ordering::Relaxed) % AUDIT_RING_SIZE;
    let tick = crate::arch::aarch64::timer::TICK_COUNT.load(Ordering::Relaxed);

    let mut entry_event = [0u8; 48];
    let len = event.len().min(48);
    entry_event[..len].copy_from_slice(&event[..len]);

    let mut ring = AUDIT_RING.lock();
    ring[idx] = AuditEntry {
        timestamp: tick,
        pid: pid.0,
        event: entry_event,
        event_len: len,
    };
}

// ---------------------------------------------------------------------------
// Echo service
// ---------------------------------------------------------------------------

/// Channel used by the echo service (set during init).
static ECHO_CHANNEL: Mutex<Option<ChannelId>> = Mutex::new(None);

/// Initialize the service manager: create and register the echo service.
///
/// Creates Process 7 ("echo-svc") with an echo channel, registers it in the
/// service manager, and spawns echo server + client test threads.
pub fn init() {
    use crate::cap;
    use crate::task::process::{KernelResourceLimits, ProcessControl, PROCESS_TABLE};

    // --- Create Process 7: echo service ---
    {
        let mut procs = PROCESS_TABLE.lock();
        let mut name = [0u8; 32];
        name[..8].copy_from_slice(b"echo-svc");
        procs[7] = Some(ProcessControl {
            pid: ProcessId(7),
            address_space: None,
            resource_limits: KernelResourceLimits::native(),
            cap_table: cap::CapabilityTable::new(),
            thread_ids: [None; 16],
            name,
        });
    }
    let _ = cap::grant_to_process(ProcessId(7), shared::Capability::ChannelCreate, true);
    let _ = cap::grant_to_process(ProcessId(7), shared::Capability::DebugPrint, false);

    // Create echo channel (unchecked — init-time setup).
    let echo_server_tid = ThreadId(0x700);
    let echo_client_tid = ThreadId(0x701);

    let ch = ipc::channel_create_unchecked(echo_server_tid);
    ipc::channel_set_peer(ch, echo_client_tid).expect("Failed to set echo channel peer");
    *ECHO_CHANNEL.lock() = Some(ch);

    // Grant ChannelAccess for the echo channel to process 7.
    let _ = cap::grant_to_process(ProcessId(7), shared::Capability::ChannelAccess(ch), false);

    // Register the echo service.
    service_register(b"echo", ProcessId(7), ch).expect("Failed to register echo service");

    // --- Create echo server thread ---
    {
        let stack_phys = sched::alloc_kernel_stack();
        let stack_virt_top = sched::phys_to_virt(stack_phys) + sched::STACK_SIZE;

        let mut thread = Thread::new_kernel(
            echo_server_tid,
            b"echo-server\0\0\0\0\0",
            echo_server_entry as *const () as usize,
            stack_phys,
        );
        thread.sched.class = SchedulerClass::Normal;
        thread.sched.effective_class = SchedulerClass::Normal;
        thread.sched.affinity = CpuSet::all();
        thread.context.sp = stack_virt_top as u64;
        thread.owner_pid = Some(ProcessId(7));

        let idx = sched::allocate_thread(thread).expect("thread table full for echo server");
        sched::enqueue_on_cpu(0, ThreadId(idx as u32), SchedulerClass::Normal);
    }

    // --- Create echo client test thread ---
    {
        let stack_phys = sched::alloc_kernel_stack();
        let stack_virt_top = sched::phys_to_virt(stack_phys) + sched::STACK_SIZE;

        let mut thread = Thread::new_kernel(
            echo_client_tid,
            b"echo-client\0\0\0\0\0",
            echo_client_entry as *const () as usize,
            stack_phys,
        );
        thread.sched.class = SchedulerClass::Normal;
        thread.sched.effective_class = SchedulerClass::Normal;
        thread.sched.affinity = CpuSet::all();
        thread.context.sp = stack_virt_top as u64;
        thread.owner_pid = Some(ProcessId(7));

        let idx = sched::allocate_thread(thread).expect("thread table full for echo client");
        sched::enqueue_on_cpu(1, ThreadId(idx as u32), SchedulerClass::Normal);
    }

    crate::kinfo!(Ipc, "Service manager initialized (echo service registered)");
}

/// Echo server thread: loops receiving messages and replying with "ECHO:" prefix.
fn echo_server_entry() -> ! {
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    let ch = loop {
        if let Some(ch) = *ECHO_CHANNEL.lock() {
            break ch;
        }
        sched::thread_yield();
    };
    crate::kinfo!(Ipc, "Echo server: started, channel={}", ch.0);

    let mut recv_buf = [0u8; ipc::MAX_MESSAGE_SIZE];

    loop {
        match ipc::ipc_recv(ch, &mut recv_buf, ipc::DEFAULT_TIMEOUT_TICKS) {
            Ok((len, sender)) => {
                // Build "ECHO:" + received message.
                let mut reply = [0u8; ipc::MAX_MESSAGE_SIZE];
                let prefix = b"ECHO:";
                let reply_len = (prefix.len() + len).min(ipc::MAX_MESSAGE_SIZE);
                reply[..prefix.len()].copy_from_slice(prefix);
                let data_len = reply_len - prefix.len();
                reply[prefix.len()..reply_len].copy_from_slice(&recv_buf[..data_len]);

                let result = ipc::ipc_reply(ch, &reply[..reply_len]);
                if result < 0 {
                    crate::kwarn!(
                        Ipc,
                        "Echo server: reply failed with {} (sender={})",
                        result,
                        sender.0
                    );
                }
            }
            Err(e) => {
                // EPIPE means channel destroyed — exit the loop.
                if e == crate::syscall::IpcError::Epipe as i64 {
                    crate::kinfo!(Ipc, "Echo server: channel destroyed (EPIPE), exiting");
                    break;
                }
                // Timeout is expected if no clients — continue.
                if e != crate::syscall::IpcError::Etimedout as i64 {
                    crate::kwarn!(Ipc, "Echo server: recv error {}", e);
                }
            }
        }
    }

    // After channel destruction, mark ourselves dead and yield forever.
    let cpu = crate::arch::aarch64::exceptions::core_id() as usize;
    let my_tid = { *crate::task::CURRENT_THREAD[cpu].lock() };
    if let Some(tid) = my_tid {
        let mut table = crate::task::THREAD_TABLE.lock();
        if let Some(thread) = &mut table[tid.0 as usize] {
            thread.sched.state = ThreadState::Dead;
        }
    }
    loop {
        sched::thread_yield();
    }
}

/// Echo client test thread: sends "hello" to echo service, expects "ECHO:hello" back.
/// Then demonstrates service death by calling process_exit.
fn echo_client_entry() -> ! {
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    crate::kinfo!(Ipc, "Echo client: started");

    // Spin-wait for service registration (avoids yield scheduling issues).
    let (svc_pid, svc_ch) = loop {
        if let Some(result) = service_lookup(b"echo") {
            break result;
        }
        core::hint::spin_loop();
    };
    crate::kinfo!(
        Ipc,
        "Echo client: found service (pid={}, ch={})",
        svc_pid.0,
        svc_ch.0
    );

    // Send "hello" and expect "ECHO:hello" back.
    let msg = b"hello";
    let mut reply_buf = [0u8; ipc::MAX_MESSAGE_SIZE];

    let result = ipc::ipc_call(svc_ch, msg, &mut reply_buf, ipc::DEFAULT_TIMEOUT_TICKS);
    if result >= 0 {
        let reply_len = result as usize;
        let reply_str = core::str::from_utf8(&reply_buf[..reply_len]).unwrap_or("<invalid utf8>");
        crate::kinfo!(Ipc, "Echo client: got '{}'", reply_str);

        if reply_str == "ECHO:hello" {
            crate::kinfo!(Ipc, "Echo client: echo service OK");
        } else {
            crate::kwarn!(Ipc, "Echo client: unexpected reply '{}'", reply_str);
        }
    } else {
        crate::kwarn!(Ipc, "Echo client: ipc_call failed with {}", result);
    }

    // Demonstrate service death: exit process 7.
    crate::kinfo!(Ipc, "Echo client: triggering process_exit for pid=7");
    crate::task::process::process_exit(ProcessId(7), 0);

    // After process exit, try the channel — it should fail or return stale data.
    let mut buf = [0u8; 64];
    let result2 = ipc::ipc_call(svc_ch, b"dead", &mut buf, 100);
    if result2 < 0 {
        crate::kinfo!(
            Ipc,
            "Echo client: error {} after service death (EPIPE expected)",
            result2
        );
    } else {
        crate::kinfo!(
            Ipc,
            "Echo client: stale reply ({} bytes) after death, channel draining",
            result2
        );
    }

    // Verify service lookup returns None after death.
    if service_lookup(b"echo").is_none() {
        crate::kinfo!(
            Ipc,
            "Echo client: service lookup returns None after death (expected)"
        );
    } else {
        crate::kwarn!(Ipc, "Echo client: service still found after death");
    }

    loop {
        sched::thread_yield();
    }
}
