//! Process control structures and the process lifecycle.
//!
//! A process owns an address space and one or more threads. Each process
//! has kernel-enforced resource limits (ipc.md §3.3) that bound its
//! consumption of kernel objects.
//!
//! A process-table slot moves Empty → Alive (`process_install`) → Exited
//! (`process_exit`, steps A–E) → Empty (the reap in `process_wait`), and is
//! never installed again (capability-lifetime ADR §6).
//!
//! Per scheduler.md §3, ipc.md §3.3.

use super::{ThreadId, ThreadState, MAX_THREADS};
use crate::cap::CapabilityTable;
use crate::mm::uspace::UserAddressSpace;
use crate::observability::tripwire;
use crate::sched::wake;
use crate::syscall::IpcError;
use shared::tripwire::WakeSource;
use shared::{process_wait_action, ProcessState, ProcessWaitAction};
use spin::Mutex;

// Re-export shared types.
pub use shared::{KernelResourceLimits, ProcessId, MAX_PROCESSES};

// ---------------------------------------------------------------------------
// Process control block
// ---------------------------------------------------------------------------

/// Maximum threads per process.
const MAX_THREADS_PER_PROCESS: usize = 16;

/// Process control block — owns an address space and tracks its threads.
///
/// About 18 KiB, almost all of it the 256-token capability table, so a
/// `ProcessControl` never moves: every `PROCESS_TABLE` slot holds one for the
/// whole boot, and install and reap reset its fields in place. A kernel stack
/// is 32 KiB with no guard page, and a debug build can make more than one
/// copy of a moved value.
#[allow(dead_code)]
pub struct ProcessControl {
    /// Process identifier: the slot's index.
    pub pid: ProcessId,
    /// User address space (None for kernel-only processes).
    pub address_space: Option<UserAddressSpace>,
    /// Kernel resource limits for this process.
    pub resource_limits: KernelResourceLimits,
    /// Per-process capability table (model/capabilities.md §3.2).
    pub cap_table: CapabilityTable,
    /// Thread IDs belonging to this process.
    pub thread_ids: [Option<ThreadId>; MAX_THREADS_PER_PROCESS],
    /// Human-readable name (for debugging).
    pub name: [u8; 32],
    /// Lifecycle state. Every write checks `ProcessState::may_become`.
    pub state: ProcessState,
    /// The process that may reap this one with `process_wait`, if any.
    pub parent: Option<ProcessId>,
    /// Set by `process_install` and never cleared: a used slot is never
    /// installed again, because a `ProcessId` carries no generation. Written
    /// only under `PROCESS_TABLE`.
    pub ever_used: bool,
}

impl ProcessControl {
    /// A never-used, Empty slot for `pid`.
    const fn empty(pid: ProcessId) -> Self {
        Self {
            pid,
            address_space: None,
            // The most restrictive limits: an Empty slot creates nothing.
            resource_limits: KernelResourceLimits::web(),
            cap_table: CapabilityTable::new(),
            thread_ids: [None; MAX_THREADS_PER_PROCESS],
            name: [0; 32],
            state: ProcessState::Empty,
            parent: None,
            ever_used: false,
        }
    }

    /// Move to `next`, which `ProcessState::may_become` must allow from the
    /// current state.
    ///
    /// # Panics
    ///
    /// On a change `may_become` forbids: every caller has checked the state
    /// under the same `PROCESS_TABLE` hold, so it is a kernel bug.
    fn set_state(&mut self, next: ProcessState) {
        assert!(
            self.state.may_become(&next),
            "process {}: {:?} -> {:?}",
            self.pid.0,
            self.state,
            next
        );
        self.state = next;
    }
}

// ---------------------------------------------------------------------------
// Global process table
// ---------------------------------------------------------------------------

/// Backing array of `PROCESS_TABLE`, indexed by `ProcessId::index()`.
pub type ProcessTable = [ProcessControl; MAX_PROCESSES];

/// Every slot Empty and never used, slot `i` holding pid `i`. Evaluated at
/// compile time, so no table-sized value is ever built on a stack.
const fn empty_table() -> ProcessTable {
    let mut table = [const { ProcessControl::empty(ProcessId(0)) }; MAX_PROCESSES];
    let mut i = 0;
    while i < MAX_PROCESSES {
        table[i].pid = ProcessId(i as u32);
        i += 1;
    }
    table
}

/// System-wide process table: a fixed `ProcessControl` per pid, reset in
/// place by install and reap.
pub static PROCESS_TABLE: Mutex<ProcessTable> = Mutex::new(empty_table());

/// Range-checked access to `pid`'s slot in a locked `PROCESS_TABLE`, in any
/// state. For lifecycle code only; capability paths use `process_mut` /
/// `process_ref`, which accept only an Alive process.
///
/// Returns `Err(EINVAL)` when `pid` is `>= MAX_PROCESSES`. No process can
/// exist at such a pid, so a raw index would run past the end of the table
/// and panic the kernel.
pub fn process_slot_mut(
    table: &mut ProcessTable,
    pid: ProcessId,
) -> Result<&mut ProcessControl, i64> {
    match pid.index() {
        // `index()` only returns pids below MAX_PROCESSES, the length of the
        // table array, so this indexing cannot panic.
        Some(idx) => Ok(&mut table[idx]),
        None => Err(IpcError::Einval as i64),
    }
}

/// Look up the Alive process `pid` in a locked `PROCESS_TABLE`, mutably.
///
/// Returns `Err(EINVAL)` when `pid` is out of range (see `process_slot_mut`)
/// and `Err(EPERM)` when its slot is not Alive (Empty, or exited): every
/// capability path fails closed for an exited process.
pub fn process_mut(table: &mut ProcessTable, pid: ProcessId) -> Result<&mut ProcessControl, i64> {
    let proc = process_slot_mut(table, pid)?;
    if proc.state == ProcessState::Alive {
        Ok(proc)
    } else {
        Err(IpcError::Eperm as i64)
    }
}

/// Look up the Alive process `pid` in a locked `PROCESS_TABLE`.
///
/// Returns `Err(EINVAL)` when `pid` is `>= MAX_PROCESSES` and `Err(EPERM)`
/// when its slot is not Alive (Empty, or exited).
pub fn process_ref(table: &ProcessTable, pid: ProcessId) -> Result<&ProcessControl, i64> {
    match pid.index() {
        // `index()` only returns pids below MAX_PROCESSES, the length of the
        // table array, so this indexing cannot panic.
        Some(idx) if table[idx].state == ProcessState::Alive => Ok(&table[idx]),
        Some(_) => Err(IpcError::Eperm as i64),
        None => Err(IpcError::Einval as i64),
    }
}

/// Install process `pid`, reset in place: Empty to Alive, with `parent` (the
/// process that may reap it), `name` (cut to 32 bytes), `limits`, an empty
/// capability table, no threads and no address space. Clears the slot's
/// `PROCESS_WAITERS` entry under the same hold (`PROCESS_TABLE →
/// PROCESS_WAITERS`).
///
/// Errors: EINVAL for a pid `>= MAX_PROCESSES`; EEXIST when the slot is not
/// Empty or was ever used (a pid is never reused).
pub fn process_install(
    pid: ProcessId,
    parent: Option<ProcessId>,
    name: &[u8],
    limits: KernelResourceLimits,
) -> Result<(), i64> {
    let mut table = PROCESS_TABLE.lock();
    let proc = process_slot_mut(&mut table, pid)?;
    if proc.state != ProcessState::Empty || proc.ever_used {
        return Err(IpcError::Eexist as i64);
    }
    proc.pid = pid;
    proc.address_space = None;
    proc.resource_limits = limits;
    proc.cap_table.clear();
    proc.thread_ids = [None; MAX_THREADS_PER_PROCESS];
    let len = name.len().min(proc.name.len());
    proc.name = [0; 32];
    proc.name[..len].copy_from_slice(&name[..len]);
    proc.parent = parent;
    // process_slot_mut checked the pid's range.
    if let Some(idx) = pid.index() {
        PROCESS_WAITERS.lock()[idx] = None;
    }
    proc.ever_used = true;
    proc.set_state(ProcessState::Alive);
    Ok(())
}

// ---------------------------------------------------------------------------
// Process wait infrastructure
// ---------------------------------------------------------------------------

/// Per-child process waiter: the thread registered by `process_wait` for
/// the child, by the child's slot. A leaf `spin::Mutex`, taken under
/// `PROCESS_TABLE` (install, `wait_step`, exit step E) and never held across
/// `unblock`. CPU 0's heartbeat scan try-locks it alone from IRQ context
/// (`ipc::scan`), the second exception to the IRQ-class rule beside
/// CHANNEL_TABLE.
pub static PROCESS_WAITERS: Mutex<[Option<ThreadId>; MAX_PROCESSES]> = {
    const NONE: Option<ThreadId> = None;
    Mutex::new([NONE; MAX_PROCESSES])
};

// ---------------------------------------------------------------------------
// Process lifecycle
// ---------------------------------------------------------------------------

/// Whether thread `tid` is in the victim mask `victims` (bit t = thread slot
/// t).
fn is_victim(victims: u64, tid: ThreadId) -> bool {
    1u64.checked_shl(tid.0)
        .is_some_and(|bit| victims & bit != 0)
}

/// Exit process `pid` with `exit_code` (capability-lifetime ADR §6), in
/// five steps, holding one lock at a time except the pairs named:
///
/// - **A** (`PROCESS_TABLE`, `SHARED_REGION_TABLE` nested): EINVAL for a pid
///   out of range; EPERM for pid 0, the kernel's (its idle threads are never
///   killed), or an Empty slot; `Ok` at once if the process already exited.
///   Otherwise mark it `Exited { code: None }`, revoke every token
///   (`revoke_all`) and detach every region it created (the region
///   cascade). The frames are freed after both locks are released.
/// - **B** (`sched::kill_process_threads`, IRQs masked): mark every thread of
///   `pid` Dead and reset its wait word, except the calling thread, and
///   snapshot the victims: every thread of `pid`, the caller included.
/// - **C** (channels and thread-keyed state, for every victim): under
///   `CHANNEL_TABLE`, clear each `waiting_receiver` that names a victim, mark
///   Dead both endpoints of every channel with an endpoint owned by `pid` and
///   collect the threads blocked on them, and collect every channel `pid`
///   created (`creator == Some(pid)`). After the release, wake the collected
///   threads with EPIPE and destroy the collected channels. Then, per
///   victim, each lock alone: its timeout, reply slot, notification deadline
///   and notification waiter records, a wait's or a select's
///   (`ipc::forget_thread`). A `pending_caller` that names a victim stays
///   until the server replies.
/// - **D**: remove `pid`'s mappings of other creators' regions, then tell the
///   service manager.
/// - **E** (`PROCESS_TABLE → PROCESS_WAITERS`): publish `Exited { code:
///   Some(exit_code) }` last, read the thread registered for `pid` and leave
///   its entry set (only its own `process_wait` reaps), and clear every entry
///   that names a victim. Release, then wake the registered thread.
///
/// The calling thread is never marked Dead, even when it belongs to `pid`,
/// so a thread that exits its own process keeps running until it ends
/// itself with `sched::exit_current()`.
///
/// Until crash-fix step 8's exit safe point, callers keep to the interim
/// exit rule (ADR §7): no victim other than the caller is Running or
/// Runnable at B.
pub fn process_exit(pid: ProcessId, exit_code: i32) -> Result<(), i64> {
    // A: Exited { None }, every token revoked, the created regions detached.
    let mut regions = crate::ipc::shmem::FreeList::new();
    {
        let mut table = PROCESS_TABLE.lock();
        let proc = process_slot_mut(&mut table, pid)?;
        if pid == ProcessId(0) {
            return Err(IpcError::Eperm as i64);
        }
        match proc.state {
            ProcessState::Empty => return Err(IpcError::Eperm as i64),
            ProcessState::Exited { .. } => return Ok(()),
            ProcessState::Alive => {}
        }
        proc.set_state(ProcessState::Exited { code: None });
        proc.cap_table.revoke_all();
        // Every token is revoked, so the cascade detaches every region pid
        // created.
        crate::ipc::shmem::revoke_regions_for_cap(pid, &proc.cap_table, &mut regions);
    }
    crate::ipc::shmem::finish_region_cascade(pid, regions);

    crate::kinfo!(Ipc, "process_exit: pid={} exit_code={}", pid.0, exit_code);

    // B: every thread of pid but the caller marked Dead, with IRQs masked.
    let victims = crate::sched::kill_process_threads(pid);

    // C: channels, then each victim's thread-keyed records.
    exit_channels(pid, victims);
    for slot in 0..MAX_THREADS {
        let tid = ThreadId(slot as u32);
        if is_victim(victims, tid) {
            crate::ipc::forget_thread(tid);
        }
    }

    // D: mappings of other creators' regions, then the service manager.
    crate::ipc::shmem::process_cleanup_shared_memory(pid);
    crate::service::service_on_death(pid);

    // E: publish the code last; wake the registered waiter after the release.
    let waiter = {
        let mut table = PROCESS_TABLE.lock();
        let proc = process_slot_mut(&mut table, pid)?;
        proc.set_state(ProcessState::Exited {
            code: Some(exit_code),
        });
        let mut waiters = PROCESS_WAITERS.lock();
        // A slot is in range: process_slot_mut checked it.
        let waiter = pid.index().and_then(|idx| waiters[idx]);
        for entry in waiters.iter_mut() {
            if entry.is_some_and(|tid| is_victim(victims, tid)) {
                *entry = None;
            }
        }
        waiter
    };
    if let Some(tid) = waiter {
        crate::sched::unblock(tid, WakeSource::PWait);
    }
    Ok(())
}

/// Exit step C's channel part for process `pid`, whose threads are the
/// victim mask `victims`.
///
/// In one `CHANNEL_TABLE` hold: clear every `waiting_receiver` that names a
/// victim, on every channel, so a later receiver does not get EAGAIN; mark
/// Dead both endpoints of every channel with an endpoint owned by `pid`
/// (endpoint owners are pids, recorded at creation and by
/// `channel_set_peer`, so kernel channels made for label tids match too)
/// and collect the threads blocked on those; and collect the full id of
/// every channel `pid` created. Release CHANNEL_TABLE before calling
/// wake_with_error (which acquires WAKEUP_ERRORS + scheduler locks), then
/// wake the collected threads with EPIPE and destroy the collected channels,
/// each destroy waking its own blocked threads. A kernel channel (`creator:
/// None`) is never collected.
fn exit_channels(pid: ProcessId, victims: u64) {
    let mut epipe_wakeups: [Option<ThreadId>; 16] = [None; 16];
    let mut epipe_count = 0usize;
    let mut doomed = [None; crate::ipc::MAX_CHANNELS];
    let mut doomed_count = 0usize;
    {
        let mut channels = crate::ipc::CHANNEL_TABLE.lock();
        for ch in channels.iter_mut().filter_map(|slot| slot.channel.as_mut()) {
            if ch
                .waiting_receiver
                .is_some_and(|tid| is_victim(victims, tid))
            {
                ch.waiting_receiver = None;
            }
            if ch.creator == Some(pid) {
                // At most one id per slot, so `doomed` (MAX_CHANNELS long)
                // never fills.
                doomed[doomed_count] = Some(ch.id);
                doomed_count += 1;
            }
            if ch.owner_a == pid || ch.owner_b == Some(pid) {
                // Mark channel as destroyed by setting endpoints to Dead.
                ch.state_a = shared::EndpointState::Dead;
                ch.state_b = shared::EndpointState::Dead;
                // Collect threads to wake outside the lock.
                // A collected thread's wake is in flight until
                // wake_with_error reaches unblock.
                for tid in [ch.waiting_receiver.take(), ch.pending_caller.take()]
                    .into_iter()
                    .flatten()
                {
                    if epipe_count < epipe_wakeups.len() {
                        epipe_wakeups[epipe_count] = Some(tid);
                        epipe_count += 1;
                        tripwire::mark_wake_pending(tid, WakeSource::PExit);
                    }
                }
            }
        }
    } // CHANNEL_TABLE released here.

    // Deliver EPIPE wakeups outside the channel lock.
    for tid in epipe_wakeups.iter().take(epipe_count).flatten() {
        crate::ipc::wake_with_error(*tid, IpcError::Epipe as i64, WakeSource::PExit);
    }
    crate::cap::destroy_channels(&doomed[..doomed_count]);
}

/// One pass of `process_wait` for thread `parent_tid` of process
/// `caller_pid`, waiting on `child`, under `PROCESS_TABLE → PROCESS_WAITERS`
/// (capability-lifetime ADR §6). The decision is
/// `shared::process_wait_action`:
///
/// - EINVAL for a child pid out of range;
/// - EPERM if the caller's process is not Alive, also clearing the child's
///   entry if it names `parent_tid`;
/// - EPERM if the child is Empty or not the caller's child;
/// - EEXIST if another thread is registered, before any arm;
/// - `Ok(Some(code))` once the child's code is published: the entry is
///   cleared and the slot reaped in place (Empty, capability table cleared,
///   no address space, no parent; `ever_used` stays set, so the slot is
///   never installed again);
/// - otherwise `Ok(None)`: `parent_tid`'s wait is armed
///   (`wake::prepare_block`) and registered in this hold, so exit step E's
///   wake either finds it Blocked or leaves the token. The caller must then
///   call `block_current` or `wake::cancel_block`.
///
/// `parent_tid` must be the calling thread's own tid (the own-tid rule of
/// `wake::prepare_block`). Takes no THREAD_TABLE hold.
pub(crate) fn wait_step(
    parent_tid: ThreadId,
    caller_pid: ProcessId,
    child: ProcessId,
) -> Result<Option<i32>, i64> {
    let Some(child_idx) = child.index() else {
        return Err(IpcError::Einval as i64);
    };
    let mut table = PROCESS_TABLE.lock();
    let caller_alive = caller_pid
        .index()
        .is_some_and(|idx| table[idx].state == ProcessState::Alive);
    let proc = &mut table[child_idx];
    let mut waiters = PROCESS_WAITERS.lock();
    let entry = &mut waiters[child_idx];
    match process_wait_action(
        caller_alive,
        caller_pid,
        &proc.state,
        proc.parent,
        *entry,
        parent_tid,
    ) {
        ProcessWaitAction::CallerNotAlive { clear_entry } => {
            if clear_entry {
                *entry = None;
            }
            Err(IpcError::Eperm as i64)
        }
        ProcessWaitAction::NotChild => Err(IpcError::Eperm as i64),
        ProcessWaitAction::OtherWaiter => Err(IpcError::Eexist as i64),
        ProcessWaitAction::Reap { code } => {
            *entry = None;
            proc.set_state(ProcessState::Empty);
            proc.cap_table.clear();
            proc.address_space = None;
            proc.parent = None;
            Ok(Some(code))
        }
        ProcessWaitAction::Wait => {
            wake::prepare_block(parent_tid);
            *entry = Some(parent_tid);
            Ok(None)
        }
    }
}

/// Wait, as thread `parent_tid` of process `caller_pid`, until the child
/// `child` has exited, then reap it and return its exit code.
///
/// Loops on [`wait_step`]: each pass either returns (the code, or an error)
/// or arms and registers, then blocks as `BlockedProcessWait`. A wake that
/// lands between the pass and the block leaves a token, so the block returns
/// at once and the next pass reaps; a stale wake costs one pass. There is no
/// timeout.
///
/// Errors: EINVAL for a child pid `>= MAX_PROCESSES`; EPERM when the caller's
/// process is not Alive, or the child is Empty or not the caller's child;
/// EEXIST when another thread already waits on it.
///
/// Reaping frees nothing today. Before `UserAddressSpace` gets a `Drop` or
/// an ASID release, a reap must know that no victim is still on a CPU
/// (ADR §6, "Reapable is not off-CPU").
pub fn process_wait(
    parent_tid: ThreadId,
    caller_pid: ProcessId,
    child: ProcessId,
) -> Result<i32, i64> {
    loop {
        if let Some(code) = wait_step(parent_tid, caller_pid, child)? {
            return Ok(code);
        }
        crate::sched::block_current(ThreadState::BlockedProcessWait { child_pid: child.0 });
    }
}
