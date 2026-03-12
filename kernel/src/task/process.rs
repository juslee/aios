//! Process control structures.
//!
//! A process owns an address space and one or more threads. Each process
//! has kernel-enforced resource limits (ipc.md §3.3) that bound its
//! consumption of kernel objects.
//!
//! Per scheduler.md §3, ipc.md §3.3.

use core::sync::atomic::{AtomicI32, Ordering};

use super::{ThreadId, ThreadState, MAX_THREADS, THREAD_TABLE};
use crate::cap::CapabilityTable;
use crate::mm::uspace::UserAddressSpace;
use spin::Mutex;

// Re-export shared types.
pub use shared::{KernelResourceLimits, ProcessId};

// ---------------------------------------------------------------------------
// Process control block
// ---------------------------------------------------------------------------

/// Maximum threads per process.
const MAX_THREADS_PER_PROCESS: usize = 16;

/// Maximum processes system-wide.
pub const MAX_PROCESSES: usize = 32;

/// Process control block — owns an address space and tracks its threads.
#[allow(dead_code)]
pub struct ProcessControl {
    /// Process identifier.
    pub pid: ProcessId,
    /// User address space (None for kernel-only processes).
    pub address_space: Option<UserAddressSpace>,
    /// Kernel resource limits for this process.
    pub resource_limits: KernelResourceLimits,
    /// Per-process capability table (security.md §3.1).
    pub cap_table: CapabilityTable,
    /// Thread IDs belonging to this process.
    pub thread_ids: [Option<ThreadId>; MAX_THREADS_PER_PROCESS],
    /// Human-readable name (for debugging).
    pub name: [u8; 32],
}

// ---------------------------------------------------------------------------
// Global process table
// ---------------------------------------------------------------------------

/// System-wide process table. BSS-allocated via `Option<ProcessControl>`.
pub static PROCESS_TABLE: Mutex<[Option<ProcessControl>; MAX_PROCESSES]> = {
    const NONE: Option<ProcessControl> = None;
    Mutex::new([NONE; MAX_PROCESSES])
};

// ---------------------------------------------------------------------------
// Process wait infrastructure
// ---------------------------------------------------------------------------

/// Per-child process waiter: if a thread is blocked in process_wait() for
/// child_pid, PROCESS_WAITERS[child_pid] = Some(waiter_tid).
pub static PROCESS_WAITERS: Mutex<[Option<ThreadId>; MAX_PROCESSES]> = {
    const NONE: Option<ThreadId> = None;
    Mutex::new([NONE; MAX_PROCESSES])
};

/// Per-process exit codes. Set by process_exit, read by process_wait.
static EXIT_CODES: [AtomicI32; MAX_PROCESSES] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: AtomicI32 = AtomicI32::new(i32::MIN);
    [ZERO; MAX_PROCESSES]
};

// ---------------------------------------------------------------------------
// Process lifecycle
// ---------------------------------------------------------------------------

/// Exit a process: mark all threads dead, clean up channels (set peer EPIPE),
/// wake ProcessWait waiters, and notify the service manager.
pub fn process_exit(pid: ProcessId, exit_code: i32) {
    let idx = pid.0 as usize;
    if idx >= MAX_PROCESSES {
        return;
    }

    crate::kinfo!(Ipc, "process_exit: pid={} exit_code={}", pid.0, exit_code);

    // Store exit code for waiters.
    EXIT_CODES[idx].store(exit_code, Ordering::Release);

    // 1. Walk thread table: mark all threads owned by this process as Dead.
    {
        let mut table = THREAD_TABLE.lock();
        for thread in table.iter_mut().flatten() {
            if thread.owner_pid == Some(pid) {
                thread.sched.state = ThreadState::Dead;
            }
        }
    }

    // 2. Walk channel table: destroy channels owned by threads of this process.
    //    Wake any blocked threads with EPIPE.
    //    Lock ordering: THREAD_TABLE before CHANNEL_TABLE to avoid deadlock.
    //    Build a thread→pid lookup first, then scan channels.
    {
        // Snapshot thread ownership under THREAD_TABLE lock (released before CHANNEL_TABLE).
        let thread_pids: [Option<ProcessId>; MAX_THREADS] = {
            let table = THREAD_TABLE.lock();
            let mut pids = [None; MAX_THREADS];
            for (i, slot) in table.iter().enumerate() {
                if let Some(t) = slot {
                    pids[i] = t.owner_pid;
                }
            }
            pids
        };

        let mut channels = crate::ipc::CHANNEL_TABLE.lock();
        for ch in channels.iter_mut().flatten() {
            let owner_a_pid = {
                let idx = ch.owner_a.0 as usize;
                if idx < thread_pids.len() {
                    thread_pids[idx]
                } else {
                    None
                }
            };
            let owner_b_pid = ch.owner_b.and_then(|tid| {
                let idx = tid.0 as usize;
                if idx < thread_pids.len() {
                    thread_pids[idx]
                } else {
                    None
                }
            });

            if owner_a_pid == Some(pid) || owner_b_pid == Some(pid) {
                // Wake any blocked threads on this channel.
                if let Some(receiver_tid) = ch.waiting_receiver.take() {
                    crate::sched::unblock(receiver_tid);
                }
                if let Some(caller_tid) = ch.pending_caller.take() {
                    crate::sched::unblock(caller_tid);
                }
                // Mark channel as destroyed by setting endpoints to Dead.
                ch.state_a = shared::EndpointState::Dead;
                ch.state_b = shared::EndpointState::Dead;
            }
        }
    }

    // 3. Notify service manager.
    crate::service::service_on_death(pid);

    // 4. Wake any thread blocked in process_wait() for this pid.
    {
        let mut waiters = PROCESS_WAITERS.lock();
        if let Some(waiter_tid) = waiters[idx].take() {
            crate::sched::unblock(waiter_tid);
        }
    }
}

/// Block the current thread until a child process exits. Returns the exit code.
pub fn process_wait(parent_tid: ThreadId, child_pid: ProcessId) -> Result<i32, i64> {
    let idx = child_pid.0 as usize;
    if idx >= MAX_PROCESSES {
        return Err(crate::syscall::IpcError::Eperm as i64);
    }

    // Check if process already exited.
    let code = EXIT_CODES[idx].load(Ordering::Acquire);
    if code != i32::MIN {
        return Ok(code);
    }

    // Check if child process exists.
    {
        let procs = PROCESS_TABLE.lock();
        if procs[idx].is_none() {
            return Err(crate::syscall::IpcError::Eperm as i64);
        }
    }

    // Register as waiter.
    {
        let mut waiters = PROCESS_WAITERS.lock();
        waiters[idx] = Some(parent_tid);
    }

    // Block until woken by process_exit.
    crate::sched::block_current(ThreadState::BlockedProcessWait {
        child_pid: child_pid.0,
    });

    // After being unblocked, read the exit code.
    let code = EXIT_CODES[idx].load(Ordering::Acquire);
    if code != i32::MIN {
        Ok(code)
    } else {
        Err(crate::syscall::IpcError::Eperm as i64)
    }
}
