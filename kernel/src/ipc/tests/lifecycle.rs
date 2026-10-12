//! Lifecycle self-tests (capability-lifetime ADR §6), run from the
//! ipc-timeout thread (pid 1, pinned to CPU 2) after the wake-token test and
//! before the Timeout test: exit, wait and reap, the exit cascade, a single
//! waiter per child, the wake token on the wait path, and the exit walk over
//! channel endpoints (#219's probe).
//!
//! Every exit keeps to ADR §7's interim exit rule: the ipc-timeout thread,
//! which nothing exits, exits only thread-less processes (pids 12, 13, 15, 16
//! and 18), and pid 14 exits itself from its only thread, which exit step B
//! leaves out. So no victim is Running or Runnable at B, and no caller is
//! killed partway through the exit. Pids 12, 13, 16, 17 and 18 hold no
//! capabilities; pid 14 holds ChannelCreate and pid 15 a delegatable
//! SharedMemoryCreate.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::arch::aarch64::daif;
use crate::arch::aarch64::timer::TICK_COUNT;
use crate::ipc::channel::{ipc_call, ipc_reply};
use crate::ipc::{
    channel_create, channel_create_unchecked, channel_destroy_unchecked, channel_endpoint_states,
    channel_set_peer, shmem, EndpointState,
};
use crate::mm::pgtable::VmFlags;
use crate::sched::{self, wake};
use crate::syscall::IpcError;
use crate::task::process::{
    process_exit, process_install, process_wait, wait_step, KernelResourceLimits, ProcessId,
    PROCESS_WAITERS,
};
use crate::task::{CpuSet, SchedulerClass, Thread, ThreadId, ThreadState};
use shared::tripwire::WakeSource;
use shared::{deadline_after, Capability, Syscall, WaitState};

use super::super::timeout::{get_wakeup_error, TimeoutEntry, TIMEOUT_QUEUE};
use super::TEST_PID;

/// Thread-less case: exited twice, reaped through SVC 25.
const REAP_PID: ProcessId = ProcessId(12);
/// Thread-less case: no parent, so nobody may reap it.
const ORPHAN_PID: ProcessId = ProcessId(13);
/// Self-exit case: one thread, pinned to CPU 2, that exits its own process.
const SELF_EXIT_PID: ProcessId = ProcessId(14);
/// Shm exit case: creates the region pid 1 maps and borrows.
const SHM_PID: ProcessId = ProcessId(15);
/// Window 2 case: one waiter, woken between its arm and its block.
const WINDOW_PID: ProcessId = ProcessId(16);
/// Self-exit case: pid 14's child, whose waiter entry the exit clears.
const GRANDCHILD_PID: ProcessId = ProcessId(17);
/// Exit walk case: owns the endpoints of two probe channels.
const WALK_PID: ProcessId = ProcessId(18);

/// Pid 14's exit code.
const SELF_EXIT_CODE: i32 = 7;
/// Pid 12's first exit code, which its second exit must not replace.
const REAP_CODE: i32 = -6;

/// The CPU pid 14's thread and the ipc-timeout thread are pinned to.
const PINNED_CPU: usize = 2;

/// Yields the test waits, at most, for pid 14's thread. The Normal queue is
/// FIFO, so each yield lets every Normal thread queued ahead on CPU 2 run
/// once before the poll runs again.
const MAX_POLLS: usize = 32;

/// Ticks until the Window 2 case's safety entry fires: long enough that
/// crowding on CPU 2 cannot let it fire while a consumed token is on its
/// way to `block_current`. It costs these ticks only when the token is lost.
const SAFETY_TICKS: u64 = 2000;

/// SharedMemoryCreate's flags for the Shm exit case.
const SHM_FLAGS: VmFlags = VmFlags::READ.union(VmFlags::WRITE);

/// Pid 14's thread, or [`NO_THREAD`] until [`init`] creates it.
static SELF_EXIT_TID: AtomicU32 = AtomicU32::new(NO_THREAD);

/// [`SELF_EXIT_TID`] before the thread exists.
const NO_THREAD: u32 = u32::MAX;

/// Set by the ipc-timeout thread (Release) when the Self-exit case starts.
static SELF_EXIT_START: AtomicBool = AtomicBool::new(false);

/// Pid 14's thread's result, stored (Release) as its last act before
/// `exit_current`: 0 until then, otherwise [`DONE`] with a bit per check
/// that passed.
static SELF_EXIT_DONE: AtomicU32 = AtomicU32::new(0);

/// [`SELF_EXIT_DONE`] bits.
const DONE: u32 = 1 << 0;
/// `wait_step(me, 14, 17)` armed and registered.
const ARMED: u32 = 1 << 1;
/// The probe channel was created.
const CREATED: u32 = 1 << 2;
/// `process_exit(14, 7)` returned `Ok`.
const EXITED: u32 = 1 << 3;
/// `ipc_call` on the probe returned EPERM (pid 14 is no longer Alive).
const CALL_EPERM: u32 = 1 << 4;
/// `ipc_reply` on the probe returned EPIPE (the exit destroyed it).
const REPLY_EPIPE: u32 = 1 << 5;

/// Install pids 14 and 17 and create pid 14's thread: process 14 (parent 1,
/// ChannelCreate), its child process 17 (no threads), and one pid-14 thread
/// pinned to CPU 2 and queued there. Called from `tests::init` before
/// `enter_scheduler`.
pub(super) fn init() {
    let limits = KernelResourceLimits::native();
    process_install(SELF_EXIT_PID, Some(TEST_PID), b"life-self-exit", limits)
        .expect("process 14 install");
    let _ = crate::cap::grant_to_process(SELF_EXIT_PID, Capability::ChannelCreate, false);
    process_install(
        GRANDCHILD_PID,
        Some(SELF_EXIT_PID),
        b"life-grandchild",
        limits,
    )
    .expect("process 17 install");

    let stack_phys = sched::alloc_kernel_stack();
    let stack_virt_top = sched::phys_to_virt(stack_phys) + sched::STACK_SIZE;
    let mut thread = Thread::new_kernel(
        ThreadId(0x204),
        b"life-self-exit\0\0",
        self_exit_entry as *const () as usize,
        stack_phys,
    );
    thread.sched.class = SchedulerClass::Normal;
    thread.sched.effective_class = SchedulerClass::Normal;
    thread.sched.affinity = CpuSet::single(PINNED_CPU);
    thread.context.sp = stack_virt_top as u64;
    thread.owner_pid = Some(SELF_EXIT_PID);

    let idx = sched::allocate_thread(thread).expect("thread table full for lifecycle thread");
    SELF_EXIT_TID.store(idx as u32, Ordering::Release);
    sched::enqueue_on_cpu(PINNED_CPU, ThreadId(idx as u32), SchedulerClass::Normal);
}

/// Run every case in the ipc-timeout thread, whose tid is `me` (pinned to
/// CPU 2, so the tid is its own, as `wake::prepare_block` requires).
pub(super) fn lifecycle_test(me: ThreadId) {
    thread_less_case(me);
    self_exit_case(me);
    shm_exit_case(me);
    window_case(me);
    exit_walk_case(me);
}

/// Install `pid` with `parent` and Native limits, logging a failure.
fn install(pid: ProcessId, parent: Option<ProcessId>, name: &[u8]) -> bool {
    match process_install(pid, parent, name, KernelResourceLimits::native()) {
        Ok(()) => true,
        Err(e) => {
            crate::kwarn!(Ipc, "Lifecycle: install of pid {} failed: {}", pid.0, e);
            false
        }
    }
}

/// Reap `pid`, a child of pid 1, with `process_wait`, but only after its
/// exit returned `Ok` (`exited`): exit step E has then published the code, so
/// the wait returns at once. Without a finished exit the wait would block
/// for good, and the Timeout test after this one would never run. `None`
/// when it did not wait.
fn reap_after_exit(me: ThreadId, pid: ProcessId, exited: bool) -> Option<Result<i32, i64>> {
    exited.then(|| process_wait(me, TEST_PID, pid))
}

/// Log `Lifecycle: <name> as expected` if every check passed, otherwise a
/// warning naming the first check that failed.
fn report(name: &str, checks: &[(bool, &str)]) {
    match checks.iter().find(|(ok, _)| !ok) {
        None => crate::kinfo!(Ipc, "Lifecycle: {} as expected", name),
        Some((_, failed)) => crate::kwarn!(Ipc, "Lifecycle: {} failed: {}", name, failed),
    }
}

/// Thread-less case. Pid 12 (parent 1) exits twice, both `Ok`, and SVC 25
/// through `syscall_dispatch` reaps it at once with x0 = 0 and x1 = -6
/// sign-extended: the first code stands. A second wait gets EPERM (the slot
/// is Empty) and a reinstall EEXIST (a pid is never reused). Pid 13 has no
/// parent, so after its exit a wait gets EPERM. Pid 0, the kernel's, cannot
/// exit (EPERM).
///
/// Lines: `Lifecycle: reap as expected`, `Lifecycle: wait ABI as expected`.
fn thread_less_case(me: ThreadId) {
    if !install(REAP_PID, Some(TEST_PID), b"life-reap") {
        return;
    }
    let exited = process_exit(REAP_PID, REAP_CODE).is_ok();
    let exited_again = process_exit(REAP_PID, 5).is_ok();
    // SVC 25 only after a finished exit, which it then reaps at once.
    let (x0, x1) = if exited {
        super::create_abi::svc(Syscall::ProcessWait, &[u64::from(REAP_PID.0)])
    } else {
        (i64::MIN, 0)
    };
    let rewait = reap_after_exit(me, REAP_PID, exited);
    let reinstall = process_install(
        REAP_PID,
        Some(TEST_PID),
        b"life-reap",
        KernelResourceLimits::native(),
    );
    let orphan = install(ORPHAN_PID, None, b"life-orphan")
        && process_exit(ORPHAN_PID, 0).is_ok()
        && process_wait(me, TEST_PID, ORPHAN_PID) == Err(IpcError::Eperm as i64);
    let kernel = process_exit(ProcessId(0), 0);

    let eperm = IpcError::Eperm as i64;
    report(
        "reap",
        &[
            (exited, "exit"),
            (exited_again, "second exit"),
            (rewait == Some(Err(eperm)), "second wait"),
            (reinstall == Err(IpcError::Eexist as i64), "reinstall"),
            (orphan, "orphan wait"),
            (kernel == Err(eperm), "pid 0 exit"),
        ],
    );
    report(
        "wait ABI",
        &[(x0 == 0, "x0"), (x1 == i64::from(REAP_CODE) as u64, "x1")],
    );
}

/// Pid 14's thread, pinned to CPU 2. It yields until the Self-exit case
/// starts; `thread_yield` returns with IRQs unmasked, so the thread runs the
/// rest with IRQs on, as the other test threads do. Then, logging nothing:
///
/// 1. `wait_step(me, 14, 17)` arms and registers it as pid 17's waiter;
///    `cancel_block` ends that wait at once.
/// 2. It creates a probe channel (`channel_create` records pid 14 as its
///    creator), exits pid 14 with code 7, and checks that `ipc_call` on the
///    probe returns EPERM (pid 14 is not Alive) and `ipc_reply` EPIPE (exit
///    step C destroyed the channel; a live one would give EPROTO).
/// 3. It arms and wakes itself, so a token is pending, publishes its result
///    in [`SELF_EXIT_DONE`] and ends with `exit_current`, which must block
///    as Dead whatever the token.
fn self_exit_entry() -> ! {
    let me = ThreadId(SELF_EXIT_TID.load(Ordering::Acquire));
    loop {
        sched::thread_yield();
        if SELF_EXIT_START.load(Ordering::Acquire) {
            break;
        }
    }

    let mut result = DONE;
    if wait_step(me, SELF_EXIT_PID, GRANDCHILD_PID) == Ok(None) {
        result |= ARMED;
    }
    wake::cancel_block(me);

    let probe = channel_create(me).ok().map(|(ch, _access)| ch);
    if probe.is_some() {
        result |= CREATED;
    }
    if process_exit(SELF_EXIT_PID, SELF_EXIT_CODE).is_ok() {
        result |= EXITED;
    }
    if let Some(ch) = probe {
        let mut buf = [0u8; 8];
        if ipc_call(ch, b"probe", &mut buf, 100) == IpcError::Eperm as i64 {
            result |= CALL_EPERM;
        }
        if ipc_reply(ch, b"probe") == IpcError::Epipe as i64 {
            result |= REPLY_EPIPE;
        }
    }

    // Arm before the done flag, so the ipc-timeout thread cannot read Idle
    // before this thread's exit_current has run.
    wake::prepare_block(me);
    sched::unblock(me, WakeSource::SelfTest);
    SELF_EXIT_DONE.store(result, Ordering::Release);
    sched::exit_current()
}

/// Self-exit case. Starts pid 14's thread and yields until its done flag is
/// set (at most [`MAX_POLLS`] yields), then checks that pid 17's waiter
/// entry, which pid 14's thread registered, is gone (exit step E clears
/// every entry naming a victim, the caller included), reaps pid 14 with
/// `Ok(7)` without blocking, yields until the thread's word reads Idle (its
/// `exit_current` consumed the pending token's word, at most
/// [`MAX_POLLS`] yields), and probes it directly: Dead and in no run queue.
/// The direct probe matters because the scheduler's pick drops a Dead
/// thread before running it, so a Dead row that queued it would never reach
/// `exit_current`.
///
/// Lines: `Lifecycle: exit cascade as expected`, `Lifecycle: waiter cleared
/// as expected`, or a failure line naming the first check that failed.
fn self_exit_case(me: ThreadId) {
    let raw = SELF_EXIT_TID.load(Ordering::Acquire);
    if raw == NO_THREAD {
        crate::kwarn!(Ipc, "Lifecycle: no self-exit thread");
        return;
    }
    let child = ThreadId(raw);
    SELF_EXIT_START.store(true, Ordering::Release);

    let mut done = SELF_EXIT_DONE.load(Ordering::Acquire);
    let mut yields = 0;
    while done == 0 && yields < MAX_POLLS {
        sched::thread_yield();
        yields += 1;
        done = SELF_EXIT_DONE.load(Ordering::Acquire);
    }
    if done == 0 {
        crate::kwarn!(Ipc, "Lifecycle: self-exit thread not done");
        return;
    }

    // A slot index below MAX_PROCESSES.
    let grandchild_entry = GRANDCHILD_PID
        .index()
        .and_then(|idx| PROCESS_WAITERS.lock()[idx]);
    let reaped = reap_after_exit(me, SELF_EXIT_PID, done & EXITED != 0);

    let mut yields = 0;
    while wake::wait_state(child) != WaitState::Idle && yields < MAX_POLLS {
        sched::thread_yield();
        yields += 1;
    }
    let idle = wake::wait_state(child) == WaitState::Idle;
    let probe = sched::thread_probe(child, true);

    let has = |bit: u32| done & bit != 0;
    report(
        "exit cascade",
        &[
            (has(CREATED), "probe create"),
            (has(EXITED), "exit"),
            (has(CALL_EPERM), "call not EPERM"),
            (has(REPLY_EPIPE), "reply not EPIPE"),
            (reaped == Some(Ok(SELF_EXIT_CODE)), "reap"),
            (idle, "word not Idle"),
            (probe.state == Some(ThreadState::Dead), "not Dead"),
            (!probe.queued, "still queued"),
        ],
    );
    report(
        "waiter cleared",
        &[
            (has(ARMED), "register"),
            (grandchild_entry.is_none(), "entry left"),
        ],
    );
}

/// Shm exit case. Pid 15 (parent 1, delegatable SharedMemoryCreate) creates
/// region R; pid 1's thread shares R from pid 15 to pid 1, maps it for pid
/// 1, borrows it, and exits pid 15. Exit step A's region cascade destroys R
/// (`shm_cascade: pid=15 destroyed 1 regions`), its frames draining for the
/// borrow (`shm: region S.G draining (1 borrows)`). Then, although pid 1's
/// delegated token is still live, a map of R and an unmap of R return EPIPE
/// and the borrow is no longer live. The release frees the frames (`shm:
/// region S.G drained`), pid 15 is reaped with `Ok(0)`, and pid 1's stale
/// token is revoked through `revoke_in_process`.
///
/// Line: `Lifecycle: shm exit as expected`.
fn shm_exit_case(me: ThreadId) {
    if !install(SHM_PID, Some(TEST_PID), b"life-shm") {
        return;
    }
    let granted =
        crate::cap::grant_to_process(SHM_PID, Capability::SharedMemoryCreate, true).is_ok();
    let region = match shmem::shared_memory_create(SHM_PID, 4096, SHM_FLAGS) {
        Ok((region, _access)) => region,
        Err(e) => {
            crate::kwarn!(Ipc, "Lifecycle: shm exit create failed: {}", e);
            let _ = reap_after_exit(me, SHM_PID, process_exit(SHM_PID, 0).is_ok());
            return;
        }
    };
    let shared = shmem::shared_memory_share(SHM_PID, region, TEST_PID).is_ok();
    let mapped = shmem::shared_memory_map(TEST_PID, region, SHM_FLAGS).is_ok();
    let borrow = shmem::region_borrow(TEST_PID, region).ok();
    let borrowed = borrow.is_some();

    let exited = process_exit(SHM_PID, 0).is_ok();

    let epipe = Err(IpcError::Epipe as i64);
    let map_epipe = shmem::shared_memory_map(TEST_PID, region, SHM_FLAGS).map(|_| ()) == epipe;
    let unmap_epipe = shmem::shared_memory_unmap(TEST_PID, region) == epipe;
    let gone = borrow.as_ref().is_some_and(|b| !b.is_live());
    if let Some(b) = borrow {
        shmem::region_release(b);
    }
    let reaped = reap_after_exit(me, SHM_PID, exited);
    super::stale_id::revoke_all(TEST_PID, Capability::SharedMemoryAccess(region));

    report(
        "shm exit",
        &[
            (granted, "grant"),
            (shared, "share"),
            (mapped, "map"),
            (borrowed, "borrow"),
            (exited, "exit"),
            (map_epipe, "map after exit"),
            (unmap_epipe, "unmap after exit"),
            (gone, "borrow still live"),
            (reaped == Some(Ok(0)), "reap"),
        ],
    );
}

/// Window 2 case (#189 b, d). Pid 16 (parent 1, no threads). This thread's
/// `wait_step` arms and registers it; a `wait_step` by another pid-1 thread
/// (the Dead test's helper, named only here) gets EEXIST before any arm.
/// This thread then exits pid 16 with `i32::MIN`: exit step E reads the
/// registered waiter, leaves its entry set (the other thread still gets
/// EEXIST), and wakes this thread while it is Running and Armed, which
/// leaves the token. With a 2000-tick safety entry published, the block
/// consumes the token and returns at once, and the next `wait_step` reaps
/// `i32::MIN`, an ordinary code.
///
/// The verdict is outcome-only, as in the wake-token test: the token was
/// consumed if the safety entry was still queued when the cleanup removed
/// it and the word is Idle. Every TIMEOUT_QUEUE and WAKEUP_ERRORS hold runs
/// with IRQs masked; the two are never held together.
///
/// Lines: `Lifecycle: one waiter as expected`, `Lifecycle: early wake as
/// expected`.
fn window_case(me: ThreadId) {
    if !install(WINDOW_PID, Some(TEST_PID), b"life-window") {
        return;
    }
    let eexist = Err(IpcError::Eexist as i64);
    let other = super::dead_thread::helper_tid();
    let armed = wait_step(me, TEST_PID, WINDOW_PID) == Ok(None);
    let refused_before = other.is_some_and(|o| wait_step(o, TEST_PID, WINDOW_PID) == eexist);
    let exited = process_exit(WINDOW_PID, i32::MIN).is_ok();
    let refused_after = other.is_some_and(|o| wait_step(o, TEST_PID, WINDOW_PID) == eexist);

    // Block only on an armed wait (rule 2: an armed wait ends in
    // block_current or cancel_block).
    let still_queued = armed && {
        daif::with_irqs_masked(|| {
            let now = TICK_COUNT.load(Ordering::Relaxed);
            let deadline = deadline_after(now, SAFETY_TICKS).unwrap_or(u64::MAX);
            TIMEOUT_QUEUE.lock()[me.0 as usize] = Some(TimeoutEntry {
                tid: me,
                wake_at_tick: deadline,
                error_code: IpcError::Etimedout as i64,
            });
        });
        sched::block_current(ThreadState::BlockedProcessWait {
            child_pid: WINDOW_PID.0,
        });
        // check_timeouts wakes with the code only after it drops
        // TIMEOUT_QUEUE, so after a fired entry the code can land after this
        // take, as a late ETIMEDOUT the thread's next ipc_call exit discards.
        let queued =
            daif::with_irqs_masked(|| TIMEOUT_QUEUE.lock()[me.0 as usize].take().is_some());
        if !queued {
            daif::with_irqs_masked(|| get_wakeup_error(me));
        }
        queued
    };
    let word = wake::wait_state(me);
    let reaped = wait_step(me, TEST_PID, WINDOW_PID);
    if reaped == Ok(None) {
        // Pid 16 never finished exiting, so this pass armed again: end that
        // wait (rule 2) rather than block on an exit that is not coming.
        wake::cancel_block(me);
    }

    report(
        "one waiter",
        &[
            (armed, "register"),
            (other.is_some(), "no other thread"),
            (refused_before, "second waiter before exit"),
            (exited, "exit"),
            (refused_after, "second waiter after exit"),
        ],
    );
    report(
        "early wake",
        &[
            (still_queued, "token lost"),
            (word == WaitState::Idle, "word not Idle"),
            (reaped == Ok(Some(i32::MIN)), "reap"),
        ],
    );
}

/// Exit walk case (#219's probe). Pid 18 (parent 1, no threads). Check that
/// `process_exit` marks Dead each channel with the exiting pid on endpoint
/// A or B, and leaves the rest Active, with or without a peer. Four kernel
/// channels (`channel_create_unchecked`, no creator, so exit step C collects
/// none of them): pid 18 on endpoint A with no peer; pid 0 on A with pid 18
/// as the peer on B; pid 0 on both; pid 0 on A with no peer. After
/// `process_exit(18, 0)` the first two have both endpoints Dead and the
/// other two both Active. Each probe channel is then destroyed unchecked,
/// and pid 18 is reaped with `Ok(0)`.
///
/// Line: `Echo client: process_exit matched endpoints A and B, spared other
/// pid (expected)`, unchanged from the echo client, where the probe ran
/// before. A mismatch logs the probe's own warnings.
fn exit_walk_case(me: ThreadId) {
    if !install(WALK_PID, Some(TEST_PID), b"life-walk") {
        return;
    }
    let dead = Some((EndpointState::Dead, EndpointState::Dead));
    let active = Some((EndpointState::Active, EndpointState::Active));

    // Pid on endpoint A only, no peer: the shape of the GPU, compositor and
    // space-storage channels.
    let a_ch = channel_create_unchecked(WALK_PID);
    // Pid on endpoint B only.
    let b_ch = channel_create_unchecked(ProcessId(0));
    // Pid on neither endpoint, both set: must stay Active.
    let other_ch = channel_create_unchecked(ProcessId(0));
    // Another pid on endpoint A and no peer, the shape of most live channels:
    // must stay Active, so a missing peer never counts as a match.
    let lone_ch = channel_create_unchecked(ProcessId(0));
    let mut ok = channel_set_peer(b_ch, WALK_PID).is_ok()
        && channel_set_peer(other_ch, ProcessId(0)).is_ok();
    if !ok {
        crate::kwarn!(Ipc, "Echo client: exit probe channel_set_peer failed");
    }

    let exited = match process_exit(WALK_PID, 0) {
        Ok(()) => true,
        Err(e) => {
            crate::kwarn!(Ipc, "Lifecycle: exit-walk exit failed: {}", e);
            false
        }
    };
    ok &= exited;

    let probes = [
        ("A-owned", a_ch, dead),
        ("B-owned", b_ch, dead),
        ("other", other_ch, active),
        ("no-peer", lone_ch, active),
    ];
    for (name, probe, expected) in probes {
        let states = channel_endpoint_states(probe);
        if states != expected {
            ok = false;
            crate::kwarn!(
                Ipc,
                "Echo client: exit walk wrong for {} probe: {:?}",
                name,
                states
            );
        }
        if let Err(e) = channel_destroy_unchecked(probe) {
            crate::kwarn!(
                Ipc,
                "Echo client: probe channel {} destroy failed: {}",
                probe,
                e
            );
        }
    }
    match reap_after_exit(me, WALK_PID, exited) {
        Some(Ok(0)) => {}
        other => {
            ok = false;
            crate::kwarn!(Ipc, "Lifecycle: exit-walk reap: {:?}", other);
        }
    }
    if ok {
        crate::kinfo!(
            Ipc,
            "Echo client: process_exit matched endpoints A and B, spared other pid (expected)"
        );
    }
}
