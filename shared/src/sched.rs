//! Scheduler and task types shared between kernel and host tests.
//!
//! Includes thread/process IDs, scheduling classes, thread states,
//! CPU affinity bitmask, resource limits, and time slice policy.
//! Per scheduler.md §3, ipc.md §3.3.

// ---------------------------------------------------------------------------
// Thread / Process identity types
// ---------------------------------------------------------------------------

/// Unique thread identifier (generation + index for ABA safety).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadId(pub u32);

/// Maximum processes system-wide (length of the kernel's process table).
pub const MAX_PROCESSES: usize = 32;

/// Unique process identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessId(pub u32);

impl ProcessId {
    /// Slot index of this process in the `MAX_PROCESSES`-entry process table.
    ///
    /// Returns `None` when the pid is `>= MAX_PROCESSES`. No process can exist
    /// at such a pid. Always index the table through this method, never
    /// through `self.0 as usize`: pids can come from untrusted callers, and a
    /// raw index past the end of the table panics the kernel.
    #[inline]
    pub const fn index(self) -> Option<usize> {
        let idx = self.0 as usize;
        if idx < MAX_PROCESSES {
            Some(idx)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Scheduler class (scheduler.md §3.1)
// ---------------------------------------------------------------------------

/// Scheduling class determines which run queue a thread enters.
/// Higher numeric value = higher scheduling priority.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SchedulerClass {
    Idle = 0,
    Normal = 1,
    Interactive = 2,
    RealTime = 3,
}

/// Time slice constants per scheduler class (nanoseconds).
pub const RT_SLICE_NS: u64 = 4_000_000; // 4ms
pub const INTERACTIVE_SLICE_NS: u64 = 10_000_000; // 10ms
pub const NORMAL_SLICE_NS: u64 = 50_000_000; // 50ms
pub const IDLE_SLICE_NS: u64 = 50_000_000; // 50ms

/// Map a scheduler class to its default time slice in nanoseconds.
pub fn default_slice(class: SchedulerClass) -> u64 {
    match class {
        SchedulerClass::RealTime => RT_SLICE_NS,
        SchedulerClass::Interactive => INTERACTIVE_SLICE_NS,
        SchedulerClass::Normal => NORMAL_SLICE_NS,
        SchedulerClass::Idle => IDLE_SLICE_NS,
    }
}

// ---------------------------------------------------------------------------
// Thread state (scheduler.md §3.3)
// ---------------------------------------------------------------------------

/// Thread execution states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    /// On a run queue, ready to execute.
    Runnable,
    /// Currently executing on a CPU.
    Running,
    /// Blocked waiting for IPC message.
    BlockedIpc { channel: u64 },
    /// Blocked waiting for timer expiry.
    BlockedTimer { wake_at: u64 },
    /// Blocked waiting for I/O completion.
    BlockedIo,
    /// Blocked waiting for a notification signal.
    BlockedNotification { notification: u32 },
    /// Blocked in IpcSelect (multi-wait on channels + notifications).
    BlockedSelect,
    /// Blocked waiting for a child process to exit.
    BlockedProcessWait { child_pid: u32 },
    /// Suspended by the kernel (memory limit, debugging).
    Suspended,
    /// Thread has exited. Terminal: see [`ThreadState::may_become`].
    Dead,
}

impl ThreadState {
    /// Whether a thread in this state may be moved to `next`.
    ///
    /// Dead is terminal: a Dead thread may only stay Dead, and every other
    /// change is allowed (capability-lifetime ADR §7). Every kernel write of
    /// a thread's state that could overwrite Dead is gated on this: the
    /// Running writes of `thread_yield` and the scheduler's pick, the
    /// Runnable write of `unblock`, `block_current`'s write, and the direct
    /// and reply switches.
    ///
    /// Takes references so that no 16-byte state is copied: the scheduler
    /// calls this on the IRQ return path, which saves no vector registers.
    #[inline]
    pub const fn may_become(&self, next: &ThreadState) -> bool {
        !matches!(self, ThreadState::Dead) || matches!(next, ThreadState::Dead)
    }
}

// ---------------------------------------------------------------------------
// Wake token (capability-lifetime ADR §8)
// ---------------------------------------------------------------------------

/// A thread's wait word: where the thread is in a wait, as the wake token
/// sees it.
///
/// The kernel keeps one word per thread slot (`sched::wake`), outside
/// THREAD_TABLE, so a thread arms without taking a lock. The word lets a
/// waker that finds its target not yet blocked leave a token instead of
/// losing the wake: the target's `block_current` then consumes the token and
/// returns at once (Linux's `set_current_state` / `try_to_wake_up` split).
///
/// Invariant: a word that is not `Idle` belongs to a Running or Runnable
/// thread, between the `prepare_block` that armed it and the
/// `block_current` or `cancel_block` that ends the wait. Until crash-fix
/// step 8's exit safe point, a thread marked Dead by a process exit while it
/// still runs can arm too; its next `block_current` resets the word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum WaitState {
    /// Not in a wait.
    Idle = 0,
    /// The thread recorded "about to block" and may already be published as
    /// a waiter, but has not blocked.
    Armed = 1,
    /// A waker found the thread Armed and not blocked, and left the token.
    Woken = 2,
}

impl WaitState {
    /// Every wait state.
    pub const ALL: [WaitState; 3] = [WaitState::Idle, WaitState::Armed, WaitState::Woken];

    /// The value the kernel stores for this state.
    #[inline]
    pub const fn raw(self) -> u8 {
        self as u8
    }

    /// The state a stored value names, `None` for a value no state has. The
    /// kernel stores only [`WaitState::raw`] values.
    #[inline]
    pub const fn from_raw(raw: u8) -> Option<WaitState> {
        match raw {
            0 => Some(WaitState::Idle),
            1 => Some(WaitState::Armed),
            2 => Some(WaitState::Woken),
            _ => None,
        }
    }
}

/// What `unblock` does with its target, decided under THREAD_TABLE from the
/// target's state and wait word ([`wake_action`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeAction {
    /// The target is Dead: nothing, no token and no enqueue (Dead is
    /// terminal).
    Dead,
    /// The target is Running or Runnable and Armed: compare-and-swap its word
    /// from Armed to Woken. No state change and no enqueue: its
    /// `block_current` consumes the token instead of blocking.
    LeaveToken,
    /// The target is Running or Runnable and a token is already pending:
    /// nothing.
    TokenPending,
    /// The target is Running or Runnable and Idle, outside any armed window:
    /// nothing. The wake is stale; a waiter re-checks its condition after it
    /// arms, so nothing is lost.
    Skip,
    /// The target is blocked (any Blocked state) or Suspended: make it
    /// Runnable and queue it. Its word is Idle there (the invariant on
    /// [`WaitState`]).
    Enqueue,
}

/// `unblock`'s decision for a target in `state` whose wait word is `word`.
///
/// Takes the state by reference so that no 16-byte state is copied:
/// `unblock` runs on the IRQ path (`check_timeouts`), which saves no vector
/// registers.
pub const fn wake_action(state: &ThreadState, word: WaitState) -> WakeAction {
    match state {
        ThreadState::Dead => WakeAction::Dead,
        ThreadState::Running | ThreadState::Runnable => match word {
            WaitState::Armed => WakeAction::LeaveToken,
            WaitState::Woken => WakeAction::TokenPending,
            WaitState::Idle => WakeAction::Skip,
        },
        ThreadState::BlockedIpc { .. }
        | ThreadState::BlockedTimer { .. }
        | ThreadState::BlockedIo
        | ThreadState::BlockedNotification { .. }
        | ThreadState::BlockedSelect
        | ThreadState::BlockedProcessWait { .. }
        | ThreadState::Suspended => WakeAction::Enqueue,
    }
}

/// What a block site (`block_current`, `try_direct_switch`) does when the
/// current thread asks to move to `next`, decided under THREAD_TABLE from
/// its state and the wait word it swapped to Idle ([`block_action`]).
///
/// Every action leaves the word Idle: the block site reads it with a swap to
/// Idle, which is the consume of a token and the reset of a stale arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockAction {
    /// The thread is already Dead (a process exit marked it): write no
    /// state, consume nothing, and switch away for good.
    AlreadyDead,
    /// `next` is Dead (`exit_current`): write Dead and switch away. A
    /// pending token never makes an exit return.
    Exit,
    /// A token was pending (the swap returned Woken): write nothing and do
    /// not switch. The caller returns at once and re-checks its condition.
    ConsumeToken,
    /// Armed or Idle: write `next` and switch away.
    Sleep,
}

/// The block site's decision for a thread in `state`, whose wait word was
/// `word`, asking to move to `next`.
///
/// `AlreadyDead` is decided first: exit step B resets a victim's word, so a
/// victim that armed before B still reaches its block site with an Idle
/// word, and must neither block as `next` nor trip the arm check that
/// applies to every other block (capability-lifetime ADR §9, "Guard").
pub const fn block_action(state: &ThreadState, word: WaitState, next: &ThreadState) -> BlockAction {
    if matches!(state, ThreadState::Dead) {
        BlockAction::AlreadyDead
    } else if matches!(next, ThreadState::Dead) {
        BlockAction::Exit
    } else if matches!(word, WaitState::Woken) {
        BlockAction::ConsumeToken
    } else {
        BlockAction::Sleep
    }
}

// ---------------------------------------------------------------------------
// Process lifecycle (capability-lifetime ADR §6)
// ---------------------------------------------------------------------------

/// A process-table slot's lifecycle state.
///
/// The four legal changes ([`ProcessState::may_become`]): install (Empty to
/// Alive), exit step A (Alive to `Exited { code: None }`), exit step E, which
/// publishes the code last (`Exited { None }` to `Exited { Some }`), and the
/// reap (`Exited { Some }` to Empty). A slot that was ever Alive is never
/// installed again: a `ProcessId` carries no generation, so a reused pid
/// would hand a stale holder the new process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    /// No process; never installed, or reaped.
    Empty,
    /// Installed and not exited: the only state whose capabilities count.
    Alive,
    /// Exited. `code` is `None` from exit step A until step E publishes it,
    /// so a waiter never reaps a process whose exit is still running.
    Exited { code: Option<i32> },
}

impl ProcessState {
    /// Whether a slot in this state may be moved to `next`: only the four
    /// changes the type's doc lists. Every writer of a slot's state checks
    /// it.
    pub const fn may_become(&self, next: &ProcessState) -> bool {
        matches!(
            (self, next),
            (ProcessState::Empty, ProcessState::Alive)
                | (ProcessState::Alive, ProcessState::Exited { code: None })
                | (
                    ProcessState::Exited { code: None },
                    ProcessState::Exited { code: Some(_) }
                )
                | (ProcessState::Exited { code: Some(_) }, ProcessState::Empty)
        )
    }
}

/// What one pass of `process_wait` (the kernel's `wait_step`) does, decided
/// under `PROCESS_TABLE → PROCESS_WAITERS` by [`process_wait_action`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessWaitAction {
    /// EPERM: the caller's own process is not Alive. If the child's waiter
    /// entry names the calling thread (`clear_entry`), the pass clears it,
    /// so an exited waiter leaves no entry behind.
    CallerNotAlive { clear_entry: bool },
    /// EPERM: the child slot is Empty, or the child is not the caller's.
    NotChild,
    /// EEXIST: another thread is registered as the child's waiter, before or
    /// after the child exits. Decided before any arm.
    OtherWaiter,
    /// The child has exited and its code is published: clear the entry,
    /// reap the slot in place and return `code`.
    Reap { code: i32 },
    /// The child is Alive or still exiting: arm the calling thread's wait,
    /// register it as the child's waiter and block.
    Wait,
}

/// `wait_step`'s decision for thread `me` of process `caller` (Alive or not,
/// `caller_alive`), waiting on a child in state `child` whose parent is
/// `child_parent`, with `waiter` the thread registered for that child.
///
/// In order (capability-lifetime ADR §6): a caller that is not Alive gets
/// EPERM; an Empty child or one that is not the caller's gets EPERM; another
/// registered thread gets EEXIST; an exited child with its code published is
/// reaped; anything else waits.
pub fn process_wait_action(
    caller_alive: bool,
    caller: ProcessId,
    child: &ProcessState,
    child_parent: Option<ProcessId>,
    waiter: Option<ThreadId>,
    me: ThreadId,
) -> ProcessWaitAction {
    let mine = waiter == Some(me);
    if !caller_alive {
        return ProcessWaitAction::CallerNotAlive { clear_entry: mine };
    }
    if *child == ProcessState::Empty || child_parent != Some(caller) {
        return ProcessWaitAction::NotChild;
    }
    if waiter.is_some() && !mine {
        return ProcessWaitAction::OtherWaiter;
    }
    match child {
        ProcessState::Exited { code: Some(code) } => ProcessWaitAction::Reap { code: *code },
        _ => ProcessWaitAction::Wait,
    }
}

// ---------------------------------------------------------------------------
// CPU affinity (scheduler.md §3.3)
// ---------------------------------------------------------------------------

/// CPU affinity bitmask — supports up to 64 cores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuSet {
    pub bits: u64,
}

impl CpuSet {
    /// All CPUs allowed.
    pub const fn all() -> Self {
        Self { bits: !0 }
    }

    /// Single CPU allowed. Panics if `cpu >= 64`.
    pub const fn single(cpu: usize) -> Self {
        assert!(cpu < 64, "CpuSet: cpu index out of range");
        Self { bits: 1 << cpu }
    }

    /// Create from raw bitmask.
    pub const fn from_mask(mask: u64) -> Self {
        Self { bits: mask }
    }

    /// Check if a specific CPU is in the set. Returns false if `cpu >= 64`.
    pub const fn contains(&self, cpu: usize) -> bool {
        if cpu >= 64 {
            return false;
        }
        self.bits & (1 << cpu) != 0
    }

    /// Returns the number of CPUs in the set.
    pub const fn count(&self) -> u32 {
        self.bits.count_ones()
    }
}

// ---------------------------------------------------------------------------
// Kernel resource limits (ipc.md §3.3)
// ---------------------------------------------------------------------------

/// Hard limits on kernel object creation per process.
///
/// Set at `ProcessCreate` and cannot be increased. A child process
/// cannot exceed its parent's limits (monotonic restriction).
#[derive(Debug, Clone, Copy)]
pub struct KernelResourceLimits {
    pub max_channels: u32,
    pub max_shared_regions: u32,
    pub max_pending_messages: u32,
    pub max_notification_subscriptions: u32,
    pub max_child_processes: u32,
}

impl KernelResourceLimits {
    /// Level 1 (System) trust level defaults.
    pub const fn system() -> Self {
        Self {
            max_channels: 256,
            max_shared_regions: 128,
            max_pending_messages: 1024,
            max_notification_subscriptions: 64,
            max_child_processes: 32,
        }
    }

    /// Level 2 (Native) trust level defaults.
    pub const fn native() -> Self {
        Self {
            max_channels: 128,
            max_shared_regions: 64,
            max_pending_messages: 512,
            max_notification_subscriptions: 32,
            max_child_processes: 16,
        }
    }

    /// Level 3 (Third-party) trust level defaults.
    pub const fn third_party() -> Self {
        Self {
            max_channels: 64,
            max_shared_regions: 32,
            max_pending_messages: 256,
            max_notification_subscriptions: 16,
            max_child_processes: 8,
        }
    }

    /// Level 4 (Web) trust level defaults.
    pub const fn web() -> Self {
        Self {
            max_channels: 16,
            max_shared_regions: 8,
            max_pending_messages: 64,
            max_notification_subscriptions: 4,
            max_child_processes: 0,
        }
    }

    /// Check if all fields in `child` are <= the corresponding fields in `self`.
    pub const fn allows_child(&self, child: &Self) -> bool {
        child.max_channels <= self.max_channels
            && child.max_shared_regions <= self.max_shared_regions
            && child.max_pending_messages <= self.max_pending_messages
            && child.max_notification_subscriptions <= self.max_notification_subscriptions
            && child.max_child_processes <= self.max_child_processes
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── SchedulerClass ordering ─────────────────────────────────────────

    #[test]
    fn sched_class_ordering() {
        assert!(SchedulerClass::Idle < SchedulerClass::Normal);
        assert!(SchedulerClass::Normal < SchedulerClass::Interactive);
        assert!(SchedulerClass::Interactive < SchedulerClass::RealTime);
    }

    #[test]
    fn sched_class_values() {
        assert_eq!(SchedulerClass::Idle as u8, 0);
        assert_eq!(SchedulerClass::Normal as u8, 1);
        assert_eq!(SchedulerClass::Interactive as u8, 2);
        assert_eq!(SchedulerClass::RealTime as u8, 3);
    }

    // ── default_slice ───────────────────────────────────────────────────

    #[test]
    fn slice_values() {
        assert_eq!(default_slice(SchedulerClass::RealTime), 4_000_000);
        assert_eq!(default_slice(SchedulerClass::Interactive), 10_000_000);
        assert_eq!(default_slice(SchedulerClass::Normal), 50_000_000);
        assert_eq!(default_slice(SchedulerClass::Idle), 50_000_000);
    }

    #[test]
    fn slice_ordering() {
        // RT gets the shortest slice (preempts less often → higher priority)
        assert!(
            default_slice(SchedulerClass::RealTime) < default_slice(SchedulerClass::Interactive)
        );
        assert!(default_slice(SchedulerClass::Interactive) < default_slice(SchedulerClass::Normal));
    }

    #[test]
    fn slice_nonzero() {
        // All slices must be positive
        assert!(default_slice(SchedulerClass::Idle) > 0);
        assert!(default_slice(SchedulerClass::Normal) > 0);
        assert!(default_slice(SchedulerClass::Interactive) > 0);
        assert!(default_slice(SchedulerClass::RealTime) > 0);
    }

    // ── ThreadState ─────────────────────────────────────────────────────

    #[test]
    fn thread_state_variants() {
        let s = ThreadState::BlockedIpc { channel: 42 };
        if let ThreadState::BlockedIpc { channel } = s {
            assert_eq!(channel, 42);
        } else {
            panic!("wrong variant");
        }

        let s = ThreadState::BlockedTimer { wake_at: 1000 };
        if let ThreadState::BlockedTimer { wake_at } = s {
            assert_eq!(wake_at, 1000);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn thread_state_equality() {
        assert_eq!(ThreadState::Runnable, ThreadState::Runnable);
        assert_ne!(ThreadState::Running, ThreadState::Runnable);
        assert_ne!(
            ThreadState::BlockedIpc { channel: 1 },
            ThreadState::BlockedIpc { channel: 2 }
        );
    }

    /// One value of every `ThreadState` variant, payloads included.
    const ALL_STATES: [ThreadState; 10] = [
        ThreadState::Runnable,
        ThreadState::Running,
        ThreadState::BlockedIpc { channel: 3 },
        ThreadState::BlockedTimer { wake_at: 9 },
        ThreadState::BlockedIo,
        ThreadState::BlockedNotification { notification: 1 },
        ThreadState::BlockedSelect,
        ThreadState::BlockedProcessWait { child_pid: 2 },
        ThreadState::Suspended,
        ThreadState::Dead,
    ];

    #[test]
    fn may_become_every_pair() {
        // Every variant appears once, so a new variant must be added here.
        for (i, state) in ALL_STATES.iter().enumerate() {
            let matched = match state {
                ThreadState::Runnable => 0,
                ThreadState::Running => 1,
                ThreadState::BlockedIpc { .. } => 2,
                ThreadState::BlockedTimer { .. } => 3,
                ThreadState::BlockedIo => 4,
                ThreadState::BlockedNotification { .. } => 5,
                ThreadState::BlockedSelect => 6,
                ThreadState::BlockedProcessWait { .. } => 7,
                ThreadState::Suspended => 8,
                ThreadState::Dead => 9,
            };
            assert_eq!(matched, i);
        }
        for from in &ALL_STATES {
            for to in &ALL_STATES {
                let want = *from != ThreadState::Dead || *to == ThreadState::Dead;
                assert_eq!(from.may_become(to), want, "{from:?} -> {to:?}");
            }
        }
    }

    #[test]
    fn dead_is_terminal() {
        for to in &ALL_STATES {
            let allowed = ThreadState::Dead.may_become(to);
            assert_eq!(allowed, *to == ThreadState::Dead, "Dead -> {to:?}");
            assert!(to.may_become(&ThreadState::Dead), "{to:?} -> Dead");
        }
        // Payloads do not matter: a blocked state may move to another
        // payload of the same variant.
        assert!(ThreadState::BlockedIpc { channel: 1 }
            .may_become(&ThreadState::BlockedIpc { channel: u64::MAX }));
    }

    // ── Wake token: the decision tables ─────────────────────────────────

    #[test]
    fn wait_state_raw_round_trips() {
        for (i, word) in WaitState::ALL.iter().enumerate() {
            assert_eq!(usize::from(word.raw()), i);
            assert_eq!(WaitState::from_raw(word.raw()), Some(*word));
        }
        assert_eq!(WaitState::from_raw(3), None);
        assert_eq!(WaitState::from_raw(u8::MAX), None);
    }

    /// The state change a wake action implies, if any.
    fn wake_next(action: WakeAction) -> Option<ThreadState> {
        match action {
            WakeAction::Enqueue => Some(ThreadState::Runnable),
            WakeAction::Dead
            | WakeAction::LeaveToken
            | WakeAction::TokenPending
            | WakeAction::Skip => None,
        }
    }

    /// The state change a block action implies for a block to `next`, if
    /// any.
    fn block_next(action: BlockAction, next: ThreadState) -> Option<ThreadState> {
        match action {
            BlockAction::AlreadyDead | BlockAction::ConsumeToken => None,
            BlockAction::Exit => Some(ThreadState::Dead),
            BlockAction::Sleep => Some(next),
        }
    }

    #[test]
    fn wake_action_every_state_and_word() {
        for state in &ALL_STATES {
            for word in WaitState::ALL {
                let want = match (state, word) {
                    (ThreadState::Dead, _) => WakeAction::Dead,
                    (ThreadState::Running | ThreadState::Runnable, WaitState::Armed) => {
                        WakeAction::LeaveToken
                    }
                    (ThreadState::Running | ThreadState::Runnable, WaitState::Woken) => {
                        WakeAction::TokenPending
                    }
                    (ThreadState::Running | ThreadState::Runnable, WaitState::Idle) => {
                        WakeAction::Skip
                    }
                    _ => WakeAction::Enqueue,
                };
                let got = wake_action(state, word);
                assert_eq!(got, want, "{state:?} {word:?}");
                if let Some(next) = wake_next(got) {
                    assert!(state.may_become(&next), "{state:?} -> {next:?}");
                }
            }
        }
    }

    #[test]
    fn block_action_every_state_word_and_next() {
        for state in &ALL_STATES {
            for word in WaitState::ALL {
                for next in &ALL_STATES {
                    let want = if *state == ThreadState::Dead {
                        BlockAction::AlreadyDead
                    } else if *next == ThreadState::Dead {
                        BlockAction::Exit
                    } else if word == WaitState::Woken {
                        BlockAction::ConsumeToken
                    } else {
                        BlockAction::Sleep
                    };
                    let got = block_action(state, word, next);
                    assert_eq!(got, want, "{state:?} {word:?} -> {next:?}");
                    if let Some(to) = block_next(got, *next) {
                        assert!(state.may_become(&to), "{state:?} -> {to:?}");
                    }
                }
            }
        }
        // A token never makes an exit return, and a Dead thread never
        // consumes one.
        assert_eq!(
            block_action(&ThreadState::Running, WaitState::Woken, &ThreadState::Dead),
            BlockAction::Exit
        );
        assert_eq!(
            block_action(
                &ThreadState::Dead,
                WaitState::Woken,
                &ThreadState::BlockedSelect
            ),
            BlockAction::AlreadyDead
        );
    }

    // ── Wake token: the protocol model ──────────────────────────────────

    /// An exhaustive model of one wait against its wakers, explored over
    /// every interleaving of their atomic steps, with the decisions taken
    /// from [`wake_action`] and [`block_action`].
    ///
    /// - The waiter checks its condition, and if it is not set arms (when
    ///   the model arms) and registers its waiter record, all under the
    ///   publish lock; it may publish a timeout entry before or after that;
    ///   then it calls `block_current`. It may be preempted once (Running to
    ///   Runnable and queued) and resumed by the scheduler's pick.
    /// - The condition waker publishes the condition and takes the waiter
    ///   record under the publish lock, then calls `unblock` if it found one.
    /// - The timeout takes the waiter's entry (the timeout is spent), then
    ///   calls `unblock`.
    /// - Exit step B marks the waiter Dead and resets its word, both under
    ///   THREAD_TABLE.
    mod token_model {
        extern crate std;

        use super::super::{
            block_action, wake_action, BlockAction, ThreadState, WaitState, WakeAction,
        };
        use std::collections::HashSet;
        use std::vec::Vec;

        /// The waiter's scheduler state.
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        enum Th {
            Running,
            Runnable,
            Blocked,
            Dead,
        }

        impl Th {
            fn state(self) -> ThreadState {
                match self {
                    Th::Running => ThreadState::Running,
                    Th::Runnable => ThreadState::Runnable,
                    Th::Blocked => ThreadState::BlockedIpc { channel: 0 },
                    Th::Dead => ThreadState::Dead,
                }
            }
        }

        /// Where the waiter publishes its timeout entry.
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub(super) enum Timeout {
            None,
            /// After the arm (the protocol's rule 1).
            AfterArm,
            /// Before the arm (the negative control).
            BeforeArm,
        }

        #[derive(Clone, Copy, Debug)]
        pub(super) struct Config {
            /// The waiter arms (`false`: today's code, no token).
            pub(super) arm: bool,
            pub(super) cond_waker: bool,
            pub(super) timeout: Timeout,
            pub(super) exit_b: bool,
        }

        // Waiter program counters.
        const W_ENTRY_BEFORE: u8 = 0;
        const W_CHECK: u8 = 1;
        const W_ENTRY_AFTER: u8 = 2;
        const W_BLOCK: u8 = 3;
        const W_DONE: u8 = 4;

        /// One state of the model, with the history flags the checks read.
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub(super) struct World {
            th: Th,
            /// The waiter is executing on its CPU.
            on_cpu: bool,
            queued: bool,
            word: WaitState,
            cond: bool,
            record: bool,
            entry: bool,
            /// The timeout fired: it took the entry.
            spent: bool,
            waiter: u8,
            waker: u8,
            timer: u8,
            exit: u8,
            preempted: bool,
            /// The waiter's `block_current` has run.
            pub(super) blocked_ran: bool,
            /// A `block_current` consumed a token.
            pub(super) consumed: bool,
            /// A block to a live state found an Idle word: the step-14 arm
            /// assertion would fire.
            pub(super) assert_hit: bool,
            /// A block of a Dead thread found an Idle word, which the arm
            /// assertion skips because `AlreadyDead` is decided first.
            pub(super) idle_dead_block: bool,
            /// `prepare_block` found a word that was not Idle.
            pub(super) prepare_bad: bool,
            /// The waiter armed after exit B marked it Dead.
            pub(super) armed_while_dead: bool,
            /// Exit B reset an Armed word.
            pub(super) reset_armed: bool,
        }

        impl World {
            fn start(cfg: Config) -> World {
                World {
                    th: Th::Running,
                    on_cpu: true,
                    queued: false,
                    word: WaitState::Idle,
                    cond: false,
                    record: false,
                    entry: false,
                    spent: false,
                    waiter: if cfg.timeout == Timeout::BeforeArm {
                        W_ENTRY_BEFORE
                    } else {
                        W_CHECK
                    },
                    waker: if cfg.cond_waker { 0 } else { 2 },
                    timer: if cfg.timeout == Timeout::None { 2 } else { 0 },
                    exit: if cfg.exit_b { 0 } else { 1 },
                    preempted: false,
                    blocked_ran: false,
                    consumed: false,
                    assert_hit: false,
                    idle_dead_block: false,
                    prepare_bad: false,
                    armed_while_dead: false,
                    reset_armed: false,
                }
            }

            /// The waiter is blocked with its condition set and nothing
            /// queued it.
            pub(super) fn lost_cond(&self) -> bool {
                self.th == Th::Blocked && self.cond && !self.queued
            }

            /// The waiter is blocked with its timeout spent and nothing
            /// queued it.
            pub(super) fn lost_timeout(&self) -> bool {
                self.th == Th::Blocked && self.spent && !self.queued
            }

            pub(super) fn word(&self) -> WaitState {
                self.word
            }

            /// `unblock` of the waiter, one THREAD_TABLE critical section.
            fn unblock(&mut self) {
                let state = self.th.state();
                match wake_action(&state, self.word) {
                    WakeAction::LeaveToken => self.word = WaitState::Woken,
                    WakeAction::Enqueue => {
                        assert!(state.may_become(&ThreadState::Runnable));
                        self.th = Th::Runnable;
                        self.queued = true;
                    }
                    WakeAction::Dead | WakeAction::Skip | WakeAction::TokenPending => {}
                }
            }

            /// `prepare_block`: swap the word to Armed.
            fn prepare(&mut self) {
                if self.word != WaitState::Idle {
                    self.prepare_bad = true;
                }
                if self.th == Th::Dead {
                    self.armed_while_dead = true;
                }
                self.word = WaitState::Armed;
            }

            /// `block_current(BlockedIpc)`, one THREAD_TABLE critical section:
            /// swap the word to Idle and apply the block decision.
            fn block(&mut self) {
                let old = self.word;
                self.word = WaitState::Idle;
                self.blocked_ran = true;
                let next = ThreadState::BlockedIpc { channel: 0 };
                let action = block_action(&self.th.state(), old, &next);
                if old == WaitState::Idle {
                    if action == BlockAction::AlreadyDead {
                        self.idle_dead_block = true;
                    } else {
                        self.assert_hit = true;
                    }
                }
                match action {
                    BlockAction::AlreadyDead => self.on_cpu = false,
                    BlockAction::ConsumeToken => self.consumed = true,
                    BlockAction::Sleep => {
                        self.th = Th::Blocked;
                        self.on_cpu = false;
                    }
                    BlockAction::Exit => panic!("the waiter never blocks as Dead"),
                }
                self.waiter = W_DONE;
            }

            /// The waiter's next step, if it can run one.
            fn step_waiter(mut self, cfg: Config) -> Option<World> {
                if !self.on_cpu || self.waiter == W_DONE {
                    return None;
                }
                match self.waiter {
                    W_ENTRY_BEFORE => {
                        self.entry = true;
                        self.waiter = W_CHECK;
                    }
                    W_CHECK => {
                        if self.cond {
                            self.waiter = W_DONE;
                        } else {
                            if cfg.arm {
                                self.prepare();
                            }
                            self.record = true;
                            self.waiter = if cfg.timeout == Timeout::AfterArm {
                                W_ENTRY_AFTER
                            } else {
                                W_BLOCK
                            };
                        }
                    }
                    W_ENTRY_AFTER => {
                        self.entry = true;
                        self.waiter = W_BLOCK;
                    }
                    _ => self.block(),
                }
                Some(self)
            }

            /// The condition waker's next step.
            fn step_waker(mut self) -> Option<World> {
                match self.waker {
                    0 => {
                        self.cond = true;
                        self.waker = if self.record { 1 } else { 2 };
                        self.record = false;
                    }
                    1 => {
                        self.unblock();
                        self.waker = 2;
                    }
                    _ => return None,
                }
                Some(self)
            }

            /// The timeout's next step: it fires only once the entry exists.
            fn step_timer(mut self) -> Option<World> {
                match self.timer {
                    0 if self.entry => {
                        self.entry = false;
                        self.spent = true;
                        self.timer = 1;
                    }
                    1 => {
                        self.unblock();
                        self.timer = 2;
                    }
                    _ => return None,
                }
                Some(self)
            }

            /// Exit step B: mark the waiter Dead and reset its word.
            fn step_exit(mut self) -> Option<World> {
                if self.exit != 0 {
                    return None;
                }
                if self.word == WaitState::Armed {
                    self.reset_armed = true;
                }
                self.th = Th::Dead;
                self.word = WaitState::Idle;
                self.exit = 1;
                Some(self)
            }

            /// One tick preempts the running waiter (at most once): Running
            /// to Runnable, queued.
            fn step_preempt(mut self) -> Option<World> {
                if !self.on_cpu || self.th != Th::Running || self.preempted {
                    return None;
                }
                if self.waiter == W_DONE {
                    return None;
                }
                self.th = Th::Runnable;
                self.queued = true;
                self.on_cpu = false;
                self.preempted = true;
                Some(self)
            }

            /// The scheduler's pick resumes a queued Runnable waiter. A
            /// queued Dead thread is dropped by the pick, so it never runs.
            fn step_resume(mut self) -> Option<World> {
                if self.on_cpu || !self.queued || self.th != Th::Runnable {
                    return None;
                }
                self.th = Th::Running;
                self.queued = false;
                self.on_cpu = true;
                Some(self)
            }
        }

        /// Every final state (no step can run) reachable from the start.
        pub(super) fn explore(cfg: Config) -> Vec<World> {
            let mut seen = HashSet::new();
            let mut finals = Vec::new();
            let mut stack = Vec::from([World::start(cfg)]);
            while let Some(w) = stack.pop() {
                if !seen.insert(w) {
                    continue;
                }
                let next = [
                    w.step_waiter(cfg),
                    w.step_waker(),
                    w.step_timer(),
                    w.step_exit(),
                    w.step_preempt(),
                    w.step_resume(),
                ];
                let mut any = false;
                for n in next.into_iter().flatten() {
                    any = true;
                    stack.push(n);
                }
                if !any {
                    finals.push(w);
                }
            }
            finals
        }
    }

    use token_model::{explore, Config, Timeout};

    /// Checks every final state of an armed model shares: no arm found a
    /// word that was not Idle, the arm assertion never fires, and the word
    /// is Idle once `block_current` has run.
    fn check_armed_finals(finals: &[token_model::World]) {
        assert!(!finals.is_empty());
        for w in finals {
            assert!(!w.prepare_bad, "{w:?}");
            assert!(!w.assert_hit, "{w:?}");
            if w.blocked_ran {
                assert_eq!(w.word(), WaitState::Idle, "{w:?}");
            }
        }
    }

    #[test]
    fn token_model_never_loses_the_condition() {
        let finals = explore(Config {
            arm: true,
            cond_waker: true,
            timeout: Timeout::None,
            exit_b: false,
        });
        check_armed_finals(&finals);
        for w in &finals {
            assert!(!w.lost_cond(), "lost wakeup: {w:?}");
        }
        // The token path is exercised: some interleaving woke the waiter
        // between its arm and its block, and the block consumed the token.
        assert!(finals.iter().any(|w| w.consumed));
    }

    #[test]
    fn token_model_without_the_arm_loses_the_condition() {
        // Negative control: today's unblock, with no arm, skips a waiter that
        // has registered but not blocked, and the waiter then blocks for good.
        let finals = explore(Config {
            arm: false,
            cond_waker: true,
            timeout: Timeout::None,
            exit_b: false,
        });
        assert!(finals.iter().any(|w| w.lost_cond()));
    }

    #[test]
    fn token_model_never_loses_the_timeout() {
        for cond_waker in [false, true] {
            let finals = explore(Config {
                arm: true,
                cond_waker,
                timeout: Timeout::AfterArm,
                exit_b: false,
            });
            check_armed_finals(&finals);
            for w in &finals {
                assert!(!w.lost_timeout(), "lost timeout: {w:?}");
                assert!(!(cond_waker && w.lost_cond()), "lost wakeup: {w:?}");
            }
            assert!(finals.iter().any(|w| w.consumed));
        }
    }

    #[test]
    fn token_model_entry_before_the_arm_loses_the_timeout() {
        // Negative control: the timeout entry published before the arm can
        // fire into an Idle word, and the waiter then blocks with its timeout
        // spent.
        let finals = explore(Config {
            arm: true,
            cond_waker: false,
            timeout: Timeout::BeforeArm,
            exit_b: false,
        });
        assert!(finals.iter().any(|w| w.lost_timeout()));
    }

    #[test]
    fn token_model_exit_victims_end_idle() {
        let finals = explore(Config {
            arm: true,
            cond_waker: false,
            timeout: Timeout::None,
            exit_b: true,
        });
        check_armed_finals(&finals);
        // Both victims are reached: one that arms after exit B marked it
        // Dead, and one whose Armed word exit B reset, whose block then finds
        // an Idle word and is decided AlreadyDead before the arm check.
        assert!(finals.iter().any(|w| w.armed_while_dead && w.blocked_ran));
        assert!(finals
            .iter()
            .any(|w| w.reset_armed && w.blocked_ran && w.idle_dead_block));
    }

    // ── Wake token: the loop model (capability-lifetime steps 11–13) ────

    /// An exhaustive model of a converted waiting site against its wakers,
    /// explored over every interleaving of their atomic steps, with the
    /// decisions taken from [`wake_action`] and [`block_action`], as the
    /// kernel's `notify.rs`, `select.rs`, `timeout.rs` and `channel.rs` take
    /// them.
    ///
    /// - [`Site::Notify`] is `notification_wait`: a fast path (no arm), then
    ///   passes that each, in one NOTIFICATION_TABLE critical section, take
    ///   a fired record's bits, or consume bits in the word, or arm and
    ///   insert the waiter's record; then publish the deadline after the
    ///   arm (Expired: remove the record, its fired bits or ETIMEDOUT), and
    ///   block. The signaller, under the same lock, sets the bit and moves
    ///   it into an unfired record's `fired`, then calls `unblock`.
    /// - [`Site::Select`] is a two-source `ipc_select` (a notification entry
    ///   and a channel entry): a fast scan (no arm), then passes that arm,
    ///   then check and register each source in one critical section per
    ///   source, then publish the deadline and block. Every exit removes the
    ///   registrations, clears the deadline and cancels the arm. The
    ///   signaller claims (removes) the select record and leaves the bit;
    ///   the sender queues the message and takes the receiver slot. Each
    ///   calls `unblock` if it found a registration.
    /// - [`Site::Sleep`] is `sleep_ticks`: passes that arm, then publish the
    ///   TIMEOUT_QUEUE entry (`arm_timeout`; Expired ends the sleep), and
    ///   block as BlockedTimer. Its only record is the entry, and its exit
    ///   removes the entry and cancels the arm.
    /// - [`Site::Recv`] is `ipc_recv`: a first-pass pop with no arm, then
    ///   passes that, in one CHANNEL_TABLE critical section, pop a queued
    ///   message or arm and take the receiver slot; then publish the
    ///   TIMEOUT_QUEUE entry (`arm_timeout`; Expired gives ETIMEDOUT), and
    ///   block as BlockedIpc. Every exit after the first pass's pop clears
    ///   the receiver slot and the entry and cancels the arm. The sender
    ///   queues the message and takes the receiver slot under the lock,
    ///   then removes the receiver's entry (`clear_timeout`) and calls
    ///   `unblock`.
    /// - The timeout fires once the clock has passed the deadline and the
    ///   entry is published (`check_notification_timeouts`, or
    ///   `check_timeouts` for a sleep or a receive), then calls `unblock`. A
    ///   stale `unblock` (the wake helper's) can land anywhere, for a sleep
    ///   or a receive after an optional removal of the entry (`stale_clear`:
    ///   a stale reply or send's `clear_timeout`, or a late
    ///   `wake_with_error`), and one tick can preempt the waiter.
    mod loop_model {
        extern crate std;

        use super::super::{
            block_action, wake_action, BlockAction, ThreadState, WaitState, WakeAction,
        };
        use std::collections::HashSet;
        use std::vec::Vec;

        /// The waiter's scheduler state.
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        enum Th {
            Running,
            Runnable,
            Blocked,
        }

        impl Th {
            fn state(self, site: Site) -> ThreadState {
                match self {
                    Th::Running => ThreadState::Running,
                    Th::Runnable => ThreadState::Runnable,
                    Th::Blocked => site.blocked(),
                }
            }
        }

        /// The waiting site.
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub(super) enum Site {
            Notify,
            Select,
            Sleep,
            Recv,
        }

        impl Site {
            /// The state the site blocks in.
            fn blocked(self) -> ThreadState {
                match self {
                    Site::Notify => ThreadState::BlockedNotification { notification: 0 },
                    Site::Select => ThreadState::BlockedSelect,
                    Site::Sleep => ThreadState::BlockedTimer { wake_at: 0 },
                    Site::Recv => ThreadState::BlockedIpc { channel: 0 },
                }
            }
        }

        /// The event waker: none, a notification signal, or a channel send
        /// (Select and Recv).
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub(super) enum Event {
            None,
            Signal,
            Send,
        }

        #[derive(Clone, Copy, Debug)]
        pub(super) struct Config {
            pub(super) site: Site,
            pub(super) event: Event,
            /// The wait has a deadline, which the clock may pass.
            pub(super) deadline: bool,
            /// A stale `unblock` may land at any point.
            pub(super) stale: bool,
            /// The stale waker first removes the deadline entry (a stale
            /// reply's or send's `clear_timeout`), at any point before its
            /// `unblock`.
            pub(super) stale_clear: bool,
            /// Each pass re-publishes a missing deadline entry (negative
            /// control: `false`, only the first pass publishes it).
            pub(super) reinsert: bool,
            /// The waiter arms (negative control: `false`).
            pub(super) arm: bool,
            /// Recv only, negative control: each pass takes the receiver
            /// slot and publishes the timeout entry before it arms (ADR §8
            /// rule 1 broken).
            pub(super) late_arm: bool,
            /// Notify only, negative control: the old signal path, which
            /// removes the record and consumes the bits under the lock but
            /// writes the result into a per-thread slot after the release.
            pub(super) old_signal: bool,
            /// Select only, negative control: each source is checked in one
            /// critical section and registered in a later one.
            pub(super) split: bool,
        }

        /// The wait's result.
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub(super) enum Res {
            /// The notification's bits (Notify), or the notification entry
            /// ready (Select).
            Bits,
            /// The channel entry ready (Select).
            Msg,
            Timedout,
        }

        // Waiter program counters.
        const FAST: u8 = 0;
        const PASS: u8 = 1;
        const PASS_C: u8 = 2;
        const REG_N: u8 = 3;
        const REG_C: u8 = 4;
        const DEADLINE: u8 = 5;
        const EXPIRE: u8 = 6;
        const BLOCK: u8 = 7;
        const EXIT_UNREG: u8 = 8;
        const EXIT_CLEAR: u8 = 9;
        const EXIT_CANCEL: u8 = 10;
        const DONE: u8 = 11;
        const LATE_ARM: u8 = 12;

        /// The waiter's record on the notification.
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        enum Rec {
            None,
            /// Registered and not fired (a select record is never fired).
            Armed,
            /// A wait record the signaller moved the bits into.
            Fired,
        }

        /// One state of the model, with the history flags the checks read.
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub(super) struct World {
            th: Th,
            on_cpu: bool,
            queued: bool,
            word: WaitState,
            pc: u8,
            /// This pass armed (its exit cancels).
            armed: bool,
            /// The notification's bit is set in the word.
            bit: bool,
            rec: Rec,
            /// The old signal path's per-thread result slot.
            slot: bool,
            /// Select and Recv: a message is queued on the channel.
            msg: bool,
            /// Select and Recv: the channel's `waiting_receiver` is the
            /// waiter.
            recv: bool,
            /// The deadline entry is published.
            entry: bool,
            /// A pass has published the entry at least once.
            inserted: bool,
            /// The clock has passed the deadline.
            past: bool,
            /// Event waker: 0 = not run, 1 = owes an `unblock`, 2 = done; 3 =
            /// the old path owes its result write; 4 = a sender to a Recv
            /// owes the receiver's `clear_timeout`.
            ev: u8,
            /// Timeout: 0 = idle, 1 = owes an `unblock`.
            timer: u8,
            stale_done: bool,
            /// The stale waker's removal of the entry has run.
            stale_cleared: bool,
            preempted: bool,
            /// The signaller consumed the bit for the waiter (into a record
            /// or the old result slot).
            consumed: bool,
            pub(super) res: Option<Res>,
            /// The clock had passed the deadline when the wait returned.
            past_at_return: bool,
            pub(super) consumed_token: bool,
            /// A block to a live state found an Idle word in an armed model.
            assert_hit: bool,
            /// `prepare_block` found a word that was not Idle.
            prepare_bad: bool,
        }

        impl World {
            fn start(cfg: Config) -> World {
                World {
                    th: Th::Running,
                    on_cpu: true,
                    queued: false,
                    word: WaitState::Idle,
                    pc: FAST,
                    armed: false,
                    bit: false,
                    rec: Rec::None,
                    slot: false,
                    msg: false,
                    recv: false,
                    entry: false,
                    inserted: false,
                    past: false,
                    ev: if cfg.event == Event::None { 2 } else { 0 },
                    timer: 0,
                    stale_done: !cfg.stale,
                    stale_cleared: false,
                    preempted: false,
                    consumed: false,
                    res: None,
                    past_at_return: false,
                    consumed_token: false,
                    assert_hit: false,
                    prepare_bad: false,
                }
            }

            /// The waiter is blocked for good with its condition set, or
            /// with its deadline passed: a lost wake.
            pub(super) fn lost_wake(&self) -> bool {
                let cond = self.bit || self.msg || self.rec == Rec::Fired || self.slot;
                self.th == Th::Blocked && !self.queued && (cond || self.past)
            }

            /// The signaller consumed bits for the waiter, and the waiter
            /// returned without them.
            pub(super) fn lost_bits(&self) -> bool {
                self.consumed && self.res.is_some() && self.res != Some(Res::Bits)
            }

            /// ETIMEDOUT before the deadline passed.
            pub(super) fn early_timeout(&self) -> bool {
                self.res == Some(Res::Timedout) && !self.past_at_return
            }

            /// Token hygiene: no arm found a word that was not Idle, no block
            /// of an armed model found an Idle word, and the word is Idle
            /// once the wait has returned.
            pub(super) fn token_ok(&self) -> bool {
                !self.prepare_bad
                    && !self.assert_hit
                    && (self.res.is_none() || self.word == WaitState::Idle)
            }

            /// No registration or deadline left once the wait has returned.
            pub(super) fn clean(&self) -> bool {
                self.res.is_none() || (!self.recv && !self.entry && self.rec != Rec::Armed)
            }

            fn unblock(&mut self, site: Site) {
                let state = self.th.state(site);
                match wake_action(&state, self.word) {
                    WakeAction::LeaveToken => self.word = WaitState::Woken,
                    WakeAction::Enqueue => {
                        self.th = Th::Runnable;
                        self.queued = true;
                    }
                    WakeAction::Dead | WakeAction::Skip | WakeAction::TokenPending => {}
                }
            }

            fn prepare(&mut self, cfg: Config) {
                if !cfg.arm {
                    return;
                }
                if self.word != WaitState::Idle {
                    self.prepare_bad = true;
                }
                self.word = WaitState::Armed;
                self.armed = true;
            }

            fn ret(&mut self, res: Res, cfg: Config) {
                self.res = Some(res);
                self.past_at_return = self.past;
                self.pc = if matches!(cfg.site, Site::Select | Site::Recv) {
                    EXIT_UNREG
                } else {
                    EXIT_CLEAR
                };
            }

            /// The notify pass, one NOTIFICATION_TABLE critical section.
            fn notify_pass(&mut self, cfg: Config) {
                let fired = if cfg.old_signal {
                    self.slot
                } else {
                    self.rec == Rec::Fired
                };
                if fired {
                    self.slot = false;
                    self.rec = Rec::None;
                    self.ret(Res::Bits, cfg);
                } else if self.bit {
                    self.bit = false;
                    self.rec = Rec::None;
                    self.ret(Res::Bits, cfg);
                } else {
                    self.prepare(cfg);
                    if self.rec == Rec::None {
                        self.rec = Rec::Armed;
                    }
                    self.pc = DEADLINE;
                }
            }

            fn step_waiter(mut self, cfg: Config) -> Option<World> {
                if !self.on_cpu || self.pc == DONE {
                    return None;
                }
                match self.pc {
                    FAST if cfg.site == Site::Sleep => self.pc = PASS,
                    FAST if cfg.site == Site::Recv => {
                        // The first pass's pop, before its arm: it returns
                        // with nothing published and runs no exit.
                        if self.msg {
                            self.msg = false;
                            self.res = Some(Res::Msg);
                            self.pc = DONE;
                        } else {
                            self.pc = PASS;
                        }
                    }
                    FAST => {
                        if self.bit {
                            self.bit = false;
                            self.res = Some(Res::Bits);
                            self.pc = DONE;
                        } else if cfg.site == Site::Select && self.msg {
                            self.res = Some(Res::Msg);
                            self.pc = DONE;
                        } else {
                            self.pc = PASS;
                        }
                    }
                    PASS if cfg.site == Site::Notify => self.notify_pass(cfg),
                    PASS if cfg.site == Site::Sleep => {
                        self.prepare(cfg);
                        self.pc = DEADLINE;
                    }
                    PASS if cfg.site == Site::Recv => {
                        // One CHANNEL_TABLE section: pop, or arm and take
                        // the receiver slot.
                        if self.msg {
                            self.msg = false;
                            self.ret(Res::Msg, cfg);
                        } else {
                            if !cfg.late_arm {
                                self.prepare(cfg);
                            }
                            self.recv = true;
                            self.pc = DEADLINE;
                        }
                    }
                    PASS => {
                        // Select: arm, then the notification entry.
                        self.prepare(cfg);
                        if self.bit {
                            self.bit = false;
                            self.ret(Res::Bits, cfg);
                        } else {
                            if !cfg.split {
                                self.rec = Rec::Armed;
                            }
                            self.pc = PASS_C;
                        }
                    }
                    PASS_C => {
                        if self.msg {
                            self.ret(Res::Msg, cfg);
                        } else if cfg.split {
                            self.pc = REG_N;
                        } else {
                            self.recv = true;
                            self.pc = DEADLINE;
                        }
                    }
                    REG_N => {
                        self.rec = Rec::Armed;
                        self.pc = REG_C;
                    }
                    REG_C => {
                        self.recv = true;
                        self.pc = DEADLINE;
                    }
                    DEADLINE => {
                        if cfg.deadline && self.past {
                            if cfg.site == Site::Notify {
                                self.pc = EXPIRE;
                            } else {
                                self.ret(Res::Timedout, cfg);
                            }
                        } else {
                            if cfg.reinsert || !self.inserted {
                                self.entry = cfg.deadline;
                                self.inserted = true;
                            }
                            self.pc = if cfg.late_arm { LATE_ARM } else { BLOCK };
                        }
                    }
                    LATE_ARM => {
                        self.prepare(cfg);
                        self.pc = BLOCK;
                    }
                    EXPIRE => {
                        // Notify: remove the record under the lock; its
                        // fired bits win over the deadline.
                        let fired = if cfg.old_signal {
                            self.slot
                        } else {
                            self.rec == Rec::Fired
                        };
                        self.rec = Rec::None;
                        self.slot = false;
                        let res = if fired { Res::Bits } else { Res::Timedout };
                        self.ret(res, cfg);
                    }
                    BLOCK => {
                        let old = self.word;
                        self.word = WaitState::Idle;
                        let next = cfg.site.blocked();
                        let action = block_action(&self.th.state(cfg.site), old, &next);
                        if cfg.arm && old == WaitState::Idle {
                            self.assert_hit = true;
                        }
                        self.armed = false;
                        self.pc = PASS;
                        match action {
                            BlockAction::ConsumeToken => self.consumed_token = true,
                            BlockAction::Sleep => {
                                self.th = Th::Blocked;
                                self.on_cpu = false;
                            }
                            BlockAction::AlreadyDead | BlockAction::Exit => {
                                panic!("the waiter is never Dead")
                            }
                        }
                    }
                    EXIT_UNREG => {
                        self.rec = Rec::None;
                        self.recv = false;
                        self.pc = EXIT_CLEAR;
                    }
                    EXIT_CLEAR => {
                        self.entry = false;
                        self.pc = EXIT_CANCEL;
                    }
                    _ => {
                        if self.armed {
                            self.word = WaitState::Idle;
                            self.armed = false;
                        }
                        self.pc = DONE;
                    }
                }
                Some(self)
            }

            /// The event waker's next step.
            fn step_event(mut self, cfg: Config) -> Option<World> {
                match self.ev {
                    0 => {
                        let found = match (cfg.event, cfg.site) {
                            (Event::Send, _) => {
                                self.msg = true;
                                core::mem::take(&mut self.recv)
                            }
                            (_, Site::Select) => {
                                // The claim: remove the record, keep the bit.
                                self.bit = true;
                                core::mem::replace(&mut self.rec, Rec::None) == Rec::Armed
                            }
                            (_, Site::Notify) if self.rec == Rec::Armed => {
                                self.consumed = true;
                                if cfg.old_signal {
                                    self.rec = Rec::None;
                                    self.ev = 3;
                                    return Some(self);
                                }
                                self.rec = Rec::Fired;
                                true
                            }
                            (_, Site::Notify) => {
                                self.bit = true;
                                false
                            }
                            (_, Site::Sleep) => panic!("a sleep has no event waker"),
                            (_, Site::Recv) => panic!("a receive's only event is a send"),
                        };
                        self.ev = match (found, cfg.site) {
                            (true, Site::Recv) => 4,
                            (true, _) => 1,
                            (false, _) => 2,
                        };
                    }
                    3 => {
                        // The old path's result write, after the release.
                        self.slot = true;
                        self.ev = 1;
                    }
                    4 => {
                        // The sender's clear of the receiver's entry, after
                        // the release.
                        self.entry = false;
                        self.ev = 1;
                    }
                    1 => {
                        self.unblock(cfg.site);
                        self.ev = 2;
                    }
                    _ => return None,
                }
                Some(self)
            }

            fn step_clock(mut self, cfg: Config) -> Option<World> {
                if !cfg.deadline || self.past {
                    return None;
                }
                self.past = true;
                Some(self)
            }

            fn step_timer(mut self, cfg: Config) -> Option<World> {
                match self.timer {
                    0 if self.entry && self.past => {
                        self.entry = false;
                        self.timer = 1;
                    }
                    1 => {
                        self.unblock(cfg.site);
                        self.timer = 0;
                    }
                    _ => return None,
                }
                Some(self)
            }

            fn step_stale(mut self, cfg: Config) -> Option<World> {
                if self.stale_done {
                    return None;
                }
                if cfg.stale_clear && !self.stale_cleared {
                    self.entry = false;
                    self.stale_cleared = true;
                } else {
                    self.unblock(cfg.site);
                    self.stale_done = true;
                }
                Some(self)
            }

            fn step_preempt(mut self) -> Option<World> {
                if !self.on_cpu || self.th != Th::Running || self.preempted || self.pc == DONE {
                    return None;
                }
                self.th = Th::Runnable;
                self.queued = true;
                self.on_cpu = false;
                self.preempted = true;
                Some(self)
            }

            fn step_resume(mut self) -> Option<World> {
                if self.on_cpu || !self.queued || self.th != Th::Runnable {
                    return None;
                }
                self.th = Th::Running;
                self.queued = false;
                self.on_cpu = true;
                Some(self)
            }
        }

        /// Every final state (no step can run) reachable from the start.
        pub(super) fn explore(cfg: Config) -> Vec<World> {
            let mut seen = HashSet::new();
            let mut finals = Vec::new();
            let mut stack = Vec::from([World::start(cfg)]);
            while let Some(w) = stack.pop() {
                if !seen.insert(w) {
                    continue;
                }
                let next = [
                    w.step_waiter(cfg),
                    w.step_event(cfg),
                    w.step_clock(cfg),
                    w.step_timer(cfg),
                    w.step_stale(cfg),
                    w.step_preempt(),
                    w.step_resume(),
                ];
                let mut any = false;
                for n in next.into_iter().flatten() {
                    any = true;
                    stack.push(n);
                }
                if !any {
                    finals.push(w);
                }
            }
            finals
        }
    }

    use loop_model::{Event, Res, Site};

    /// The converted site with the real wakers: every final state passes
    /// every check, and the model reaches the token path.
    fn check_loop_site(site: Site, event: Event, deadline: bool) {
        let finals = loop_model::explore(loop_model::Config {
            site,
            event,
            deadline,
            stale: true,
            stale_clear: matches!(site, Site::Sleep | Site::Recv),
            reinsert: true,
            arm: true,
            late_arm: false,
            old_signal: false,
            split: false,
        });
        assert!(!finals.is_empty());
        for w in &finals {
            assert!(!w.lost_wake(), "lost wake: {w:?}");
            assert!(!w.lost_bits(), "lost bits: {w:?}");
            assert!(!w.early_timeout(), "early ETIMEDOUT: {w:?}");
            assert!(w.token_ok(), "token: {w:?}");
            assert!(w.clean(), "left behind: {w:?}");
        }
        assert!(
            finals.iter().any(|w| w.consumed_token),
            "{site:?} {event:?}"
        );
        if event != Event::None {
            assert!(finals
                .iter()
                .any(|w| w.res.is_some_and(|r| r != Res::Timedout)));
        }
        if deadline {
            assert!(finals.iter().any(|w| w.res == Some(Res::Timedout)));
        }
    }

    #[test]
    fn loop_model_notification_wait_never_loses_a_wake() {
        let deadlines: &[bool] = if cfg!(miri) { &[true] } else { &[false, true] };
        for &deadline in deadlines {
            check_loop_site(Site::Notify, Event::Signal, deadline);
        }
        check_loop_site(Site::Notify, Event::None, true);
    }

    #[test]
    fn loop_model_select_never_loses_a_wake() {
        let deadlines: &[bool] = if cfg!(miri) { &[true] } else { &[false, true] };
        for &deadline in deadlines {
            for event in [Event::Signal, Event::Send] {
                check_loop_site(Site::Select, event, deadline);
            }
        }
        check_loop_site(Site::Select, Event::None, true);
    }

    #[test]
    fn loop_model_sleep_never_loses_a_wake() {
        // A stale reply or send that removes the entry, then wakes the
        // sleeper: the next pass re-inserts it.
        check_loop_site(Site::Sleep, Event::None, true);
    }

    #[test]
    fn loop_model_sleep_without_the_reinsert_hangs() {
        // Negative control: only the first pass publishes the entry, so a
        // stale clear and wake leave the sleeper blocked with no entry and
        // no wake.
        let finals = loop_model::explore(loop_model::Config {
            site: Site::Sleep,
            event: Event::None,
            deadline: true,
            stale: true,
            stale_clear: true,
            reinsert: false,
            arm: true,
            late_arm: false,
            old_signal: false,
            split: false,
        });
        assert!(finals.iter().any(|w| w.lost_wake()));
    }

    #[test]
    fn loop_model_recv_never_loses_a_wake() {
        // The sender, the timeout, and a stale sender that removes the
        // entry and then wakes the receiver: the next pass re-inserts it.
        let deadlines: &[bool] = if cfg!(miri) { &[true] } else { &[false, true] };
        for &deadline in deadlines {
            check_loop_site(Site::Recv, Event::Send, deadline);
        }
        check_loop_site(Site::Recv, Event::None, true);
    }

    #[test]
    fn loop_model_recv_entry_before_the_arm_loses_the_wake() {
        // Negative control (today's order): the receiver slot and the
        // timeout entry are published before the arm, so a timeout that
        // fires before the arm finds the receiver Running with an Idle word
        // and skips it, and the receiver then blocks with nothing to wake it.
        let finals = loop_model::explore(loop_model::Config {
            site: Site::Recv,
            event: Event::None,
            deadline: true,
            stale: false,
            stale_clear: false,
            reinsert: true,
            arm: true,
            late_arm: true,
            old_signal: false,
            split: false,
        });
        assert!(finals.iter().any(|w| w.lost_wake()));
    }

    #[test]
    fn loop_model_recv_without_the_reinsert_hangs() {
        // Negative control: only the first pass publishes the entry, so a
        // stale sender's clear and wake leave the receiver blocked with no
        // entry and no wake.
        let finals = loop_model::explore(loop_model::Config {
            site: Site::Recv,
            event: Event::None,
            deadline: true,
            stale: true,
            stale_clear: true,
            reinsert: false,
            arm: true,
            late_arm: false,
            old_signal: false,
            split: false,
        });
        assert!(finals.iter().any(|w| w.lost_wake()));
    }

    #[test]
    fn loop_model_old_signal_path_loses_bits() {
        // Negative control: the result written after the table lock is
        // released, while the timed-out waiter removes its own record.
        let finals = loop_model::explore(loop_model::Config {
            site: Site::Notify,
            event: Event::Signal,
            deadline: true,
            stale: false,
            stale_clear: false,
            reinsert: true,
            arm: true,
            late_arm: false,
            old_signal: true,
            split: false,
        });
        assert!(finals.iter().any(|w| w.lost_bits()));
    }

    #[test]
    fn loop_model_deadline_without_the_arm_loses_the_wake() {
        // Negative control: the deadline published after the waiter record,
        // with no arm, fires into a waiter that has not blocked yet.
        let finals = loop_model::explore(loop_model::Config {
            site: Site::Notify,
            event: Event::None,
            deadline: true,
            stale: false,
            stale_clear: false,
            reinsert: true,
            arm: false,
            late_arm: false,
            old_signal: false,
            split: false,
        });
        assert!(finals.iter().any(|w| w.lost_wake()));
    }

    #[test]
    fn loop_model_split_scan_and_registration_loses_the_event() {
        // Negative control: an event between a source's check and its
        // registration finds no registration to wake.
        for event in [Event::Signal, Event::Send] {
            let finals = loop_model::explore(loop_model::Config {
                site: Site::Select,
                event,
                deadline: false,
                stale: false,
                stale_clear: false,
                reinsert: true,
                arm: true,
                late_arm: false,
                old_signal: false,
                split: true,
            });
            assert!(finals.iter().any(|w| w.lost_wake()), "{event:?}");
        }
    }

    // ── Wake token: the call model (capability-lifetime step 14) ────────

    /// An exhaustive model of `ipc_call` against its wakers, explored over
    /// every interleaving of their atomic steps, with the decisions taken
    /// from [`wake_action`] and [`block_action`], as the kernel's
    /// `channel.rs` takes them.
    ///
    /// - The caller's first pass writes its reply slot (under REPLY_SLOTS),
    ///   then, in one CHANNEL_TABLE critical section, arms and publishes
    ///   itself as `pending_caller`; then it publishes its timeout entry
    ///   (`arm_timeout`) and blocks. A deadline already passed there
    ///   (Expired) cancels the arm and goes to the next pass without
    ///   blocking. The first pass's direct switch to a waiting receiver
    ///   blocks the caller only when `block_action` says Sleep, as
    ///   `block_current` does, so the model's block covers it.
    /// - Every later pass arms, reads the slot (a written reply ends the
    ///   call), then reads the claim under CHANNEL_TABLE (claimed and
    ///   cancelled ends it with ECANCELED), then publishes the timeout entry
    ///   again if it is missing (Expired ends it with ETIMEDOUT), and
    ///   blocks.
    /// - The exit clears `pending_caller` if it is still the caller, takes
    ///   the slot (a written reply wins), removes the entry and cancels the
    ///   arm.
    /// - The claimer is the real replier (claims `pending_caller` with the
    ///   call's `seq`, writes the reply and `done` if the slot is there
    ///   with that `seq`, then removes the entry, then `unblock`; with
    ///   nothing delivered it skips the removal) or a canceller (claims,
    ///   sets `cancelled` if the slot is there with that `seq` and not
    ///   done, then `unblock`). It claims only once the request is queued,
    ///   which is the step that publishes `pending_caller`.
    /// - With two calls, the caller starts its second call at the slot
    ///   post once the first has returned, while the claimer, the timer
    ///   and the stale replier keep their places: a claimer delayed after
    ///   its claim on the first call can reach the second call's slot.
    /// - The timeout fires once the clock has passed the deadline and the
    ///   entry is published, then calls `unblock`. A stale replier removes
    ///   the entry, then calls `unblock`, at any point; one tick can preempt
    ///   the caller.
    mod call_model {
        extern crate std;

        use super::super::{
            block_action, wake_action, BlockAction, ThreadState, WaitState, WakeAction,
        };
        use std::collections::HashSet;
        use std::vec::Vec;

        /// The state the caller blocks in.
        const BLOCKED: ThreadState = ThreadState::BlockedIpc { channel: 0 };

        /// The caller's scheduler state.
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        enum Th {
            Running,
            Runnable,
            Blocked,
        }

        impl Th {
            fn state(self) -> ThreadState {
                match self {
                    Th::Running => ThreadState::Running,
                    Th::Runnable => ThreadState::Runnable,
                    Th::Blocked => BLOCKED,
                }
            }
        }

        /// The thread that claims the call by taking `pending_caller`.
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub(super) enum Claimer {
            None,
            Reply,
            Cancel,
        }

        #[derive(Clone, Copy, Debug)]
        pub(super) struct Config {
            pub(super) claimer: Claimer,
            /// The call has a deadline, which the clock may pass.
            pub(super) deadline: bool,
            /// A stale replier removes the entry and wakes the caller.
            pub(super) stale: bool,
            /// Each later pass re-publishes a missing entry (negative
            /// control: `false`, only the first pass publishes it).
            pub(super) reinsert: bool,
            /// The reply slot is written before `pending_caller` (negative
            /// control: `false`, after it, today's order before the loop).
            pub(super) slot_first: bool,
            /// The arm is inside the critical section that publishes
            /// `pending_caller` (negative control: `false`, after it).
            pub(super) arm_first: bool,
            /// The first pass's Expired path cancels its arm (negative
            /// control: `false`).
            pub(super) cancel_expired: bool,
            /// The caller makes 1 or 2 calls; the second starts at its slot
            /// post once the first has returned, while the claimer keeps its
            /// own place.
            pub(super) calls: u8,
            /// The claimer writes the slot only if its `seq` is the claimed
            /// call's, and a replier that delivered nothing skips its
            /// timeout clear (negative control: `false`, the code before
            /// the `seq`, which writes whatever slot the caller has).
            pub(super) seq_check: bool,
        }

        /// The call's result.
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub(super) enum Res {
            Replied,
            Canceled,
            Timedout,
        }

        // Caller program counters.
        const P1_SLOT: u8 = 0;
        const P1_PUB: u8 = 1;
        const P1_LATE: u8 = 2;
        const P1_TIMEOUT: u8 = 3;
        const P1_CANCEL: u8 = 4;
        const BLOCK: u8 = 5;
        const PASS: u8 = 6;
        const PASS_SLOT: u8 = 7;
        const PASS_CLAIM: u8 = 8;
        const PASS_TIMEOUT: u8 = 9;
        const EXIT_CHAN: u8 = 10;
        const EXIT_SLOT: u8 = 11;
        const EXIT_CLEAR: u8 = 12;
        const EXIT_CANCEL: u8 = 13;
        const DONE: u8 = 14;

        // Claimer program counters.
        const CL_CLAIM: u8 = 0;
        const CL_SLOT: u8 = 1;
        const CL_CLEAR: u8 = 2;
        const CL_WAKE: u8 = 3;
        const CL_DONE: u8 = 4;

        /// One state of the model, with the history flags the checks read.
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub(super) struct World {
            th: Th,
            on_cpu: bool,
            queued: bool,
            word: WaitState,
            pc: u8,
            /// The reply slot is present.
            slot: bool,
            /// The slot's `done` and `cancelled` flags.
            done: bool,
            cancelled: bool,
            /// This pass's read of `cancelled`.
            seen_cancelled: bool,
            /// `pending_caller` is the caller.
            pending: bool,
            /// The timeout entry is published.
            entry: bool,
            /// A pass has published the entry at least once.
            inserted: bool,
            /// The clock has passed the deadline.
            past: bool,
            claimer: u8,
            /// The replier wrote the reply (`done`).
            pub(super) written: bool,
            /// The canceller set `cancelled`.
            cancel_set: bool,
            /// Timeout: 0 = idle, 1 = owes an `unblock`.
            timer: u8,
            stale_done: bool,
            /// The stale replier's removal of the entry has run.
            stale_cleared: bool,
            preempted: bool,
            /// The first pass's `arm_timeout` found the deadline passed.
            pub(super) expired_first: bool,
            pub(super) res: Option<Res>,
            /// The clock had passed the deadline when the call returned.
            past_at_return: bool,
            pub(super) consumed_token: bool,
            /// A block to a live state found an Idle word.
            assert_hit: bool,
            /// `prepare_block` found a word that was not Idle.
            pub(super) prepare_bad: bool,
            /// The current call's sequence number (1 or 2).
            seq: u8,
            /// The slot's `seq`, set by its post.
            slot_seq: u8,
            /// The `seq` the claimer took with `pending_caller`.
            claim_seq: u8,
            /// The `seq` of the call whose claim wrote the reply.
            written_seq: u8,
            /// The `seq` of the call whose claim set `cancelled`.
            cancel_seq: u8,
            /// The first call's result, once the second has started.
            pub(super) first_res: Option<Res>,
            /// The first call's result was wrong (`wrong_result` at its
            /// return).
            first_wrong: bool,
            /// The claimer wrote a slot whose `seq` was not the one it
            /// claimed.
            pub(super) stale_write: bool,
            /// The claimer found a slot with another `seq` and wrote
            /// nothing.
            pub(super) late_dropped: bool,
        }

        impl World {
            fn start(cfg: Config) -> World {
                World {
                    th: Th::Running,
                    on_cpu: true,
                    queued: false,
                    word: WaitState::Idle,
                    pc: P1_SLOT,
                    slot: false,
                    done: false,
                    cancelled: false,
                    seen_cancelled: false,
                    pending: false,
                    entry: false,
                    inserted: false,
                    past: false,
                    claimer: if cfg.claimer == Claimer::None {
                        CL_DONE
                    } else {
                        CL_CLAIM
                    },
                    written: false,
                    cancel_set: false,
                    timer: 0,
                    stale_done: !cfg.stale,
                    stale_cleared: false,
                    preempted: false,
                    expired_first: false,
                    res: None,
                    past_at_return: false,
                    consumed_token: false,
                    assert_hit: false,
                    prepare_bad: false,
                    seq: 1,
                    slot_seq: 0,
                    claim_seq: 0,
                    written_seq: 0,
                    cancel_seq: 0,
                    first_res: None,
                    first_wrong: false,
                    stale_write: false,
                    late_dropped: false,
                }
            }

            /// The call never returned: the caller is blocked for good.
            pub(super) fn hung(&self) -> bool {
                self.res.is_none()
            }

            /// The result of the current call, or of the first call of
            /// two, does not match the state that decides it: a reply
            /// written for this call lost to another result, a reply or a
            /// cancel returned that never happened to this call (one
            /// claimed from the other call counts as never happened), or
            /// ETIMEDOUT before the deadline.
            pub(super) fn wrong_result(&self) -> bool {
                let written = self.written && self.written_seq == self.seq;
                let cancelled = self.cancel_set && self.cancel_seq == self.seq;
                self.first_wrong
                    || match self.res {
                        None => false,
                        Some(Res::Replied) => !written,
                        Some(Res::Canceled) => written || !cancelled,
                        Some(Res::Timedout) => written || !self.past_at_return,
                    }
            }

            /// Token hygiene: no arm found a word that was not Idle, no
            /// block found an Idle word, and the word is Idle once the call
            /// has returned.
            pub(super) fn token_ok(&self) -> bool {
                !self.prepare_bad
                    && !self.assert_hit
                    && (self.res.is_none() || self.word == WaitState::Idle)
            }

            /// Nothing left once the call has returned: no `pending_caller`,
            /// reply slot or timeout entry.
            pub(super) fn clean(&self) -> bool {
                self.res.is_none() || (!self.pending && !self.slot && !self.entry)
            }

            fn unblock(&mut self) {
                match wake_action(&self.th.state(), self.word) {
                    WakeAction::LeaveToken => self.word = WaitState::Woken,
                    WakeAction::Enqueue => {
                        self.th = Th::Runnable;
                        self.queued = true;
                    }
                    WakeAction::Dead | WakeAction::Skip | WakeAction::TokenPending => {}
                }
            }

            fn prepare(&mut self) {
                if self.word != WaitState::Idle {
                    self.prepare_bad = true;
                }
                self.word = WaitState::Armed;
            }

            fn post_slot(&mut self) {
                self.slot = true;
                self.done = false;
                self.cancelled = false;
                self.slot_seq = self.seq;
            }

            /// The first call of two has returned: record its result and
            /// start the second at its slot post, with fresh per-call
            /// state. The claimer, the timer and the stale replier keep
            /// their places, so a write or wake owed to the first call can
            /// land in the second.
            fn next_call(&mut self) {
                self.first_wrong = self.wrong_result();
                self.first_res = self.res;
                self.res = None;
                self.past = false;
                self.expired_first = false;
                self.inserted = false;
                self.preempted = false;
                self.seen_cancelled = false;
                self.seq = 2;
                self.pc = P1_SLOT;
            }

            fn ret(&mut self, res: Res) {
                self.res = Some(res);
                self.past_at_return = self.past;
                self.pc = EXIT_CHAN;
            }

            /// `arm_timeout`: whether the deadline has passed (Expired);
            /// otherwise publish the entry if it is missing.
            fn arm_timeout(&mut self, cfg: Config) -> bool {
                if cfg.deadline && self.past {
                    return true;
                }
                if cfg.deadline && (cfg.reinsert || !self.inserted) {
                    self.entry = true;
                    self.inserted = true;
                }
                false
            }

            fn step_caller(mut self, cfg: Config) -> Option<World> {
                if !self.on_cpu || self.pc == DONE {
                    return None;
                }
                match self.pc {
                    P1_SLOT => {
                        if cfg.slot_first {
                            self.post_slot();
                        }
                        self.pc = P1_PUB;
                    }
                    P1_PUB => {
                        // One CHANNEL_TABLE section: queue the request, arm,
                        // publish pending_caller.
                        if cfg.arm_first {
                            self.prepare();
                        }
                        self.pending = true;
                        self.pc = if cfg.slot_first && cfg.arm_first {
                            P1_TIMEOUT
                        } else {
                            P1_LATE
                        };
                    }
                    P1_LATE => {
                        if !cfg.slot_first {
                            self.post_slot();
                        }
                        if !cfg.arm_first {
                            self.prepare();
                        }
                        self.pc = P1_TIMEOUT;
                    }
                    P1_TIMEOUT => {
                        if self.arm_timeout(cfg) {
                            self.expired_first = true;
                            self.pc = if cfg.cancel_expired { P1_CANCEL } else { PASS };
                        } else {
                            self.pc = BLOCK;
                        }
                    }
                    P1_CANCEL => {
                        self.word = WaitState::Idle;
                        self.pc = PASS;
                    }
                    BLOCK => {
                        let old = self.word;
                        self.word = WaitState::Idle;
                        let action = block_action(&self.th.state(), old, &BLOCKED);
                        self.pc = PASS;
                        match action {
                            BlockAction::ConsumeToken => self.consumed_token = true,
                            BlockAction::Sleep => {
                                if old == WaitState::Idle {
                                    self.assert_hit = true;
                                }
                                self.th = Th::Blocked;
                                self.on_cpu = false;
                            }
                            BlockAction::AlreadyDead | BlockAction::Exit => {
                                panic!("the caller is never Dead")
                            }
                        }
                    }
                    PASS => {
                        self.prepare();
                        self.pc = PASS_SLOT;
                    }
                    PASS_SLOT => {
                        // REPLY_SLOTS: a written reply is taken and ends the
                        // call; otherwise read `cancelled`.
                        if self.slot && self.done {
                            self.slot = false;
                            self.ret(Res::Replied);
                        } else {
                            self.seen_cancelled = self.slot && self.cancelled;
                            self.pc = PASS_CLAIM;
                        }
                    }
                    PASS_CLAIM => {
                        // CHANNEL_TABLE: claimed and cancelled ends the call;
                        // claimed alone is a reply in flight.
                        if !self.pending && self.seen_cancelled {
                            self.ret(Res::Canceled);
                        } else {
                            self.pc = PASS_TIMEOUT;
                        }
                    }
                    PASS_TIMEOUT => {
                        if self.arm_timeout(cfg) {
                            self.ret(Res::Timedout);
                        } else {
                            self.pc = BLOCK;
                        }
                    }
                    EXIT_CHAN => {
                        self.pending = false;
                        self.pc = EXIT_SLOT;
                    }
                    EXIT_SLOT => {
                        // A written reply wins.
                        if self.slot && self.done {
                            self.res = Some(Res::Replied);
                        }
                        self.slot = false;
                        self.pc = EXIT_CLEAR;
                    }
                    EXIT_CLEAR => {
                        self.entry = false;
                        self.pc = EXIT_CANCEL;
                    }
                    _ => {
                        self.word = WaitState::Idle;
                        if self.seq < cfg.calls {
                            self.next_call();
                        } else {
                            self.pc = DONE;
                        }
                    }
                }
                Some(self)
            }

            fn step_claimer(mut self, cfg: Config) -> Option<World> {
                match self.claimer {
                    // The claim needs the queued request, published with
                    // pending_caller.
                    CL_CLAIM if self.pending => {
                        self.pending = false;
                        self.claim_seq = self.seq;
                        self.claimer = CL_SLOT;
                    }
                    CL_SLOT => {
                        let mut delivered = false;
                        if self.slot && !self.done {
                            if !cfg.seq_check || self.slot_seq == self.claim_seq {
                                if self.slot_seq != self.claim_seq {
                                    self.stale_write = true;
                                }
                                delivered = true;
                                if cfg.claimer == Claimer::Reply {
                                    self.done = true;
                                    self.written = true;
                                    self.written_seq = self.claim_seq;
                                } else {
                                    self.cancelled = true;
                                    self.cancel_set = true;
                                    self.cancel_seq = self.claim_seq;
                                }
                            } else {
                                self.late_dropped = true;
                            }
                        }
                        // A replier that delivered nothing skips the clear
                        // (the entry belongs to the caller's next wait); the
                        // code before the seq always cleared.
                        self.claimer =
                            if cfg.claimer == Claimer::Reply && (delivered || !cfg.seq_check) {
                                CL_CLEAR
                            } else {
                                CL_WAKE
                            };
                    }
                    CL_CLEAR => {
                        self.entry = false;
                        self.claimer = CL_WAKE;
                    }
                    CL_WAKE => {
                        self.unblock();
                        self.claimer = CL_DONE;
                    }
                    _ => return None,
                }
                Some(self)
            }

            fn step_clock(mut self, cfg: Config) -> Option<World> {
                if !cfg.deadline || self.past {
                    return None;
                }
                self.past = true;
                Some(self)
            }

            fn step_timer(mut self) -> Option<World> {
                match self.timer {
                    0 if self.entry && self.past => {
                        self.entry = false;
                        self.timer = 1;
                    }
                    1 => {
                        self.unblock();
                        self.timer = 0;
                    }
                    _ => return None,
                }
                Some(self)
            }

            fn step_stale(mut self) -> Option<World> {
                if self.stale_done {
                    return None;
                }
                if !self.stale_cleared {
                    self.entry = false;
                    self.stale_cleared = true;
                } else {
                    self.unblock();
                    self.stale_done = true;
                }
                Some(self)
            }

            fn step_preempt(mut self) -> Option<World> {
                if !self.on_cpu || self.th != Th::Running || self.preempted || self.pc == DONE {
                    return None;
                }
                self.th = Th::Runnable;
                self.queued = true;
                self.on_cpu = false;
                self.preempted = true;
                Some(self)
            }

            fn step_resume(mut self) -> Option<World> {
                if self.on_cpu || !self.queued || self.th != Th::Runnable {
                    return None;
                }
                self.th = Th::Running;
                self.queued = false;
                self.on_cpu = true;
                Some(self)
            }
        }

        /// The final states of one exploration.
        pub(super) type Finals = Vec<World>;

        /// Every final state (no step can run) reachable from the start.
        pub(super) fn explore(cfg: Config) -> Finals {
            let mut seen = HashSet::new();
            let mut finals = Vec::new();
            let mut stack = Vec::from([World::start(cfg)]);
            while let Some(w) = stack.pop() {
                if !seen.insert(w) {
                    continue;
                }
                let next = [
                    w.step_caller(cfg),
                    w.step_claimer(cfg),
                    w.step_clock(cfg),
                    w.step_timer(),
                    w.step_stale(),
                    w.step_preempt(),
                    w.step_resume(),
                ];
                let mut any = false;
                for n in next.into_iter().flatten() {
                    any = true;
                    stack.push(n);
                }
                if !any {
                    finals.push(w);
                }
            }
            finals
        }
    }

    use call_model::{Claimer, Res as CallRes};

    /// The converted call site's configuration with every check on, for
    /// `claimer` and `deadline`.
    fn call_config(claimer: Claimer, deadline: bool) -> call_model::Config {
        call_model::Config {
            claimer,
            deadline,
            stale: true,
            reinsert: true,
            slot_first: true,
            arm_first: true,
            cancel_expired: true,
            calls: 1,
            seq_check: true,
        }
    }

    /// The converted call site with the real wakers: every final state
    /// passes every check, and the model reaches the token path.
    fn check_call_site(claimer: Claimer, deadline: bool) -> call_model::Finals {
        let finals = call_model::explore(call_config(claimer, deadline));
        assert!(!finals.is_empty());
        for w in &finals {
            assert!(!w.hung(), "hung: {w:?}");
            assert!(!w.wrong_result(), "wrong result: {w:?}");
            assert!(w.token_ok(), "token: {w:?}");
            assert!(w.clean(), "left behind: {w:?}");
        }
        assert!(finals.iter().any(|w| w.consumed_token), "{claimer:?}");
        let want = match claimer {
            Claimer::Reply => Some(CallRes::Replied),
            Claimer::Cancel => Some(CallRes::Canceled),
            Claimer::None => None,
        };
        if want.is_some() {
            assert!(finals.iter().any(|w| w.res == want), "{claimer:?}");
        }
        if deadline {
            assert!(finals.iter().any(|w| w.res == Some(CallRes::Timedout)));
            // The first pass's Expired path, which cancels its arm and goes
            // on without blocking, is reached.
            assert!(finals.iter().any(|w| w.expired_first));
        }
        finals
    }

    #[test]
    fn call_model_reply_never_lost() {
        let deadlines: &[bool] = if cfg!(miri) { &[true] } else { &[false, true] };
        for &deadline in deadlines {
            let finals = check_call_site(Claimer::Reply, deadline);
            if deadline {
                // The first pass's Expired path with a concurrent reply
                // (the reply wins) and without one.
                assert!(finals
                    .iter()
                    .any(|w| w.expired_first && w.res == Some(CallRes::Replied)));
                assert!(finals
                    .iter()
                    .any(|w| w.expired_first && w.res == Some(CallRes::Timedout)));
            }
        }
    }

    #[test]
    fn call_model_cancel_never_lost() {
        let deadlines: &[bool] = if cfg!(miri) { &[true] } else { &[false, true] };
        for &deadline in deadlines {
            check_call_site(Claimer::Cancel, deadline);
        }
    }

    #[test]
    fn call_model_timeout_with_a_stale_replier() {
        // No server: a stale replier removes the entry and wakes the call,
        // and only the next pass's re-insert ends it on time.
        check_call_site(Claimer::None, true);
    }

    #[test]
    fn call_model_slot_after_pending_caller_loses_the_reply() {
        // Negative control (the order before the loop): the replier claims
        // the call before the slot exists, writes nothing, and the caller
        // waits for good with its reply in flight.
        let finals = call_model::explore(call_model::Config {
            stale: false,
            slot_first: false,
            ..call_config(Claimer::Reply, false)
        });
        assert!(finals.iter().any(|w| w.hung()));
    }

    #[test]
    fn call_model_arm_after_pending_caller_loses_the_wake() {
        // Negative control: the reply lands before the arm, finds the caller
        // Running with an Idle word and skips it, and the caller then blocks
        // with nothing to wake it.
        let finals = call_model::explore(call_model::Config {
            stale: false,
            arm_first: false,
            ..call_config(Claimer::Reply, false)
        });
        assert!(finals.iter().any(|w| w.hung()));
    }

    #[test]
    fn call_model_without_the_reinsert_hangs() {
        // Negative control: only the first pass publishes the entry, so the
        // stale replier's clear and wake leave the caller blocked with no
        // entry and no wake.
        let finals = call_model::explore(call_model::Config {
            reinsert: false,
            ..call_config(Claimer::None, true)
        });
        assert!(finals.iter().any(|w| w.hung()));
    }

    #[test]
    fn call_model_expired_without_cancel_block_finds_armed() {
        // Negative control: the first pass's Expired path goes on without
        // cancelling its arm, so the next pass's prepare_block finds Armed.
        let finals = call_model::explore(call_model::Config {
            stale: false,
            cancel_expired: false,
            ..call_config(Claimer::None, true)
        });
        assert!(finals.iter().any(|w| w.prepare_bad));
    }

    /// Two calls with a deadline and `claimer` claiming once, with or
    /// without the `seq` check.
    fn two_calls(claimer: Claimer, seq_check: bool) -> call_model::Finals {
        let finals = call_model::explore(call_model::Config {
            calls: 2,
            seq_check,
            ..call_config(claimer, true)
        });
        assert!(!finals.is_empty());
        finals
    }

    /// With the `seq` check, a claim on the first call never writes the
    /// second call's slot: every final state passes every check, and the
    /// window (the first call timed out after the claim, and the claimer
    /// then found the second call's slot) is reached.
    fn check_two_calls(claimer: Claimer) {
        let finals = two_calls(claimer, true);
        for w in &finals {
            assert!(!w.hung(), "hung: {w:?}");
            assert!(!w.wrong_result(), "wrong result: {w:?}");
            assert!(!w.stale_write, "stale write: {w:?}");
            assert!(w.token_ok(), "token: {w:?}");
            assert!(w.clean(), "left behind: {w:?}");
        }
        assert!(
            finals
                .iter()
                .any(|w| w.first_res == Some(CallRes::Timedout) && w.late_dropped),
            "{claimer:?}"
        );
    }

    #[test]
    fn call_model_late_reply_never_reaches_the_next_call() {
        check_two_calls(Claimer::Reply);
    }

    #[test]
    fn call_model_without_seq_the_late_reply_hits_the_next_call() {
        // Negative control (the code before the seq): the replier claims
        // the first call, the call times out, the second call posts its
        // slot, and the late reply lands there and ends the second call.
        let finals = two_calls(Claimer::Reply, false);
        assert!(finals.iter().any(|w| w.stale_write));
        assert!(finals
            .iter()
            .any(|w| w.wrong_result() && w.res == Some(CallRes::Replied)));
    }

    #[test]
    fn call_model_late_cancel_never_reaches_the_next_call() {
        check_two_calls(Claimer::Cancel);
    }

    #[test]
    fn call_model_without_seq_the_late_cancel_hits_the_next_call() {
        // Negative control: the late cancel marks the second call's slot
        // cancelled.
        let finals = two_calls(Claimer::Cancel, false);
        assert!(finals.iter().any(|w| w.stale_write));
    }

    // ── Process lifecycle ───────────────────────────────────────────────

    /// One value of every `ProcessState`, with two published codes.
    const PROCESS_STATES: [ProcessState; 5] = [
        ProcessState::Empty,
        ProcessState::Alive,
        ProcessState::Exited { code: None },
        ProcessState::Exited { code: Some(0) },
        ProcessState::Exited { code: Some(-6) },
    ];

    /// The lifecycle step a state belongs to: 0 Empty, 1 Alive, 2 exiting
    /// (code not yet published), 3 exited with its code.
    fn lifecycle_step(state: &ProcessState) -> usize {
        match state {
            ProcessState::Empty => 0,
            ProcessState::Alive => 1,
            ProcessState::Exited { code: None } => 2,
            ProcessState::Exited { code: Some(_) } => 3,
        }
    }

    #[test]
    fn process_state_may_become_every_pair() {
        for from in &PROCESS_STATES {
            for to in &PROCESS_STATES {
                // The legal changes are exactly one step along the cycle
                // Empty → Alive → Exited(None) → Exited(Some) → Empty.
                let want = (lifecycle_step(from) + 1) % 4 == lifecycle_step(to);
                assert_eq!(from.may_become(to), want, "{from:?} -> {to:?}");
            }
        }
        // No self-change: a writer never rewrites the state it reads, and
        // the published code never changes.
        assert!(!ProcessState::Exited { code: Some(1) }
            .may_become(&ProcessState::Exited { code: Some(2) }));
        assert!(!ProcessState::Alive.may_become(&ProcessState::Exited { code: Some(0) }));
        assert!(!ProcessState::Empty.may_become(&ProcessState::Exited { code: None }));
    }

    #[test]
    fn process_wait_action_every_combination() {
        let caller = ProcessId(1);
        let me = ThreadId(4);
        let other = ThreadId(5);
        let mut seen = [0usize; 5];
        for caller_alive in [true, false] {
            for child in &PROCESS_STATES {
                for parent in [None, Some(caller), Some(ProcessId(2))] {
                    for waiter in [None, Some(me), Some(other)] {
                        let action =
                            process_wait_action(caller_alive, caller, child, parent, waiter, me);
                        let ours = caller_alive && parent == Some(caller);
                        match action {
                            ProcessWaitAction::CallerNotAlive { clear_entry } => {
                                seen[0] += 1;
                                assert!(!caller_alive);
                                assert_eq!(clear_entry, waiter == Some(me));
                            }
                            ProcessWaitAction::NotChild => {
                                seen[1] += 1;
                                assert!(caller_alive);
                                assert!(*child == ProcessState::Empty || parent != Some(caller));
                            }
                            ProcessWaitAction::OtherWaiter => {
                                seen[2] += 1;
                                assert!(ours && *child != ProcessState::Empty);
                                assert_eq!(waiter, Some(other));
                            }
                            ProcessWaitAction::Reap { code } => {
                                seen[3] += 1;
                                assert!(ours && waiter != Some(other));
                                assert_eq!(*child, ProcessState::Exited { code: Some(code) });
                            }
                            ProcessWaitAction::Wait => {
                                seen[4] += 1;
                                assert!(ours && waiter != Some(other));
                                assert!(matches!(
                                    child,
                                    ProcessState::Alive | ProcessState::Exited { code: None }
                                ));
                            }
                        }
                    }
                }
            }
        }
        // 2 × 5 × 3 × 3 combinations: 45 with the caller gone; with it
        // alive, 33 not its child (an Empty child or a foreign parent), and
        // of the 12 left (4 states × 3 waiters) another waiter in 4, a reap
        // in 2 × 2 and a wait in 2 × 2.
        assert_eq!(seen, [45, 33, 4, 4, 4]);
    }

    /// A process-table slot as `wait_step` and `process_exit` see it, for
    /// the lifecycle scenarios: its state, parent and registered waiter.
    struct WaitSlot {
        state: ProcessState,
        parent: Option<ProcessId>,
        waiter: Option<ThreadId>,
    }

    impl WaitSlot {
        /// One `wait_step` pass by thread `me` of the Alive process
        /// `caller`, with its effect on the slot: register on Wait, clear
        /// and reap on Reap, clear an exited caller's own entry.
        fn wait(&mut self, caller: ProcessId, me: ThreadId) -> ProcessWaitAction {
            self.wait_as(true, caller, me)
        }

        fn wait_as(&mut self, alive: bool, caller: ProcessId, me: ThreadId) -> ProcessWaitAction {
            let action =
                process_wait_action(alive, caller, &self.state, self.parent, self.waiter, me);
            match action {
                ProcessWaitAction::Wait => self.waiter = Some(me),
                ProcessWaitAction::Reap { .. } => {
                    self.waiter = None;
                    self.set(ProcessState::Empty);
                    self.parent = None;
                }
                ProcessWaitAction::CallerNotAlive { clear_entry: true } => self.waiter = None,
                _ => {}
            }
            action
        }

        /// A state change, checked against `may_become`.
        fn set(&mut self, next: ProcessState) {
            assert!(self.state.may_become(&next), "{:?} -> {next:?}", self.state);
            self.state = next;
        }
    }

    #[test]
    fn process_wait_one_waiter_then_reap() {
        let parent = ProcessId(1);
        let (t1, t2) = (ThreadId(4), ThreadId(5));
        let mut child = WaitSlot {
            state: ProcessState::Alive,
            parent: Some(parent),
            waiter: None,
        };
        // T1 registers; T2 is refused before and during the exit.
        assert_eq!(child.wait(parent, t1), ProcessWaitAction::Wait);
        assert_eq!(child.wait(parent, t2), ProcessWaitAction::OtherWaiter);
        child.set(ProcessState::Exited { code: None }); // exit step A
        assert_eq!(child.wait(parent, t2), ProcessWaitAction::OtherWaiter);
        // A pass of T1 between A and E waits again: no reap before the code.
        assert_eq!(child.wait(parent, t1), ProcessWaitAction::Wait);
        // Exit step E publishes the code and leaves T1's entry set, so T2 is
        // still refused and only T1 reaps.
        child.set(ProcessState::Exited { code: Some(5) });
        assert_eq!(child.waiter, Some(t1));
        assert_eq!(child.wait(parent, t2), ProcessWaitAction::OtherWaiter);
        assert_eq!(child.wait(parent, t1), ProcessWaitAction::Reap { code: 5 });
        // Reaped: Empty, so every later wait is EPERM.
        assert_eq!(child.wait(parent, t1), ProcessWaitAction::NotChild);
        assert_eq!(child.wait(parent, t2), ProcessWaitAction::NotChild);
    }

    #[test]
    fn process_wait_exited_waiter_clears_its_entry() {
        let parent = ProcessId(1);
        let (t1, t2) = (ThreadId(4), ThreadId(5));
        let mut child = WaitSlot {
            state: ProcessState::Alive,
            parent: Some(parent),
            waiter: None,
        };
        assert_eq!(child.wait(parent, t1), ProcessWaitAction::Wait);
        // The waiter's own process exits; its next pass clears its entry.
        assert_eq!(
            child.wait_as(false, parent, t1),
            ProcessWaitAction::CallerNotAlive { clear_entry: true }
        );
        assert_eq!(child.waiter, None);
        // Another exited thread leaves a live entry alone.
        assert_eq!(child.wait(parent, t2), ProcessWaitAction::Wait);
        assert_eq!(
            child.wait_as(false, parent, t1),
            ProcessWaitAction::CallerNotAlive { clear_entry: false }
        );
        assert_eq!(child.waiter, Some(t2));
    }

    #[test]
    fn process_wait_already_exited_reaps_at_once() {
        let parent = ProcessId(1);
        let mut child = WaitSlot {
            state: ProcessState::Exited {
                code: Some(i32::MIN),
            },
            parent: Some(parent),
            waiter: None,
        };
        // i32::MIN is an ordinary code: no sentinel.
        assert_eq!(
            child.wait(parent, ThreadId(4)),
            ProcessWaitAction::Reap { code: i32::MIN }
        );
        // A child with no parent, or another's, is never the caller's.
        let mut orphan = WaitSlot {
            state: ProcessState::Exited { code: Some(0) },
            parent: None,
            waiter: None,
        };
        assert_eq!(
            orphan.wait(parent, ThreadId(4)),
            ProcessWaitAction::NotChild
        );
    }

    // ── CpuSet ──────────────────────────────────────────────────────────

    #[test]
    fn cpuset_all() {
        let all = CpuSet::all();
        for i in 0..64 {
            assert!(all.contains(i), "CPU {} should be in all()", i);
        }
        assert_eq!(all.count(), 64);
    }

    #[test]
    fn cpuset_single() {
        for cpu in 0..64 {
            let set = CpuSet::single(cpu);
            assert!(set.contains(cpu));
            assert_eq!(set.count(), 1);
            // Other CPUs should not be present
            if cpu > 0 {
                assert!(!set.contains(cpu - 1));
            }
            if cpu < 63 {
                assert!(!set.contains(cpu + 1));
            }
        }
    }

    #[test]
    fn cpuset_from_mask() {
        let set = CpuSet::from_mask(0b1010);
        assert!(!set.contains(0));
        assert!(set.contains(1));
        assert!(!set.contains(2));
        assert!(set.contains(3));
        assert_eq!(set.count(), 2);
    }

    #[test]
    fn cpuset_contains_out_of_range() {
        let all = CpuSet::all();
        assert!(!all.contains(64));
        assert!(!all.contains(128));
        assert!(!all.contains(usize::MAX));
    }

    #[test]
    fn cpuset_empty() {
        let empty = CpuSet::from_mask(0);
        assert_eq!(empty.count(), 0);
        for i in 0..64 {
            assert!(!empty.contains(i));
        }
    }

    // ── KernelResourceLimits ────────────────────────────────────────────

    #[test]
    fn limits_trust_hierarchy() {
        let sys = KernelResourceLimits::system();
        let nat = KernelResourceLimits::native();
        let tp = KernelResourceLimits::third_party();
        let web = KernelResourceLimits::web();

        // Each level is more restrictive
        assert!(sys.allows_child(&nat));
        assert!(nat.allows_child(&tp));
        assert!(tp.allows_child(&web));

        // Reverse should NOT hold (except if equal)
        assert!(!web.allows_child(&tp));
        assert!(!tp.allows_child(&nat));
        assert!(!nat.allows_child(&sys));
    }

    #[test]
    fn limits_self_allows() {
        // Each level allows itself as child
        let sys = KernelResourceLimits::system();
        let nat = KernelResourceLimits::native();
        let tp = KernelResourceLimits::third_party();
        let web = KernelResourceLimits::web();

        assert!(sys.allows_child(&sys));
        assert!(nat.allows_child(&nat));
        assert!(tp.allows_child(&tp));
        assert!(web.allows_child(&web));
    }

    #[test]
    fn limits_web_no_children() {
        let web = KernelResourceLimits::web();
        assert_eq!(web.max_child_processes, 0);
    }

    #[test]
    fn limits_monotonic_fields() {
        let levels = [
            KernelResourceLimits::system(),
            KernelResourceLimits::native(),
            KernelResourceLimits::third_party(),
            KernelResourceLimits::web(),
        ];

        // Each field should decrease or stay equal as trust decreases
        for i in 1..levels.len() {
            assert!(levels[i].max_channels <= levels[i - 1].max_channels);
            assert!(levels[i].max_shared_regions <= levels[i - 1].max_shared_regions);
            assert!(levels[i].max_pending_messages <= levels[i - 1].max_pending_messages);
            assert!(
                levels[i].max_notification_subscriptions
                    <= levels[i - 1].max_notification_subscriptions
            );
            assert!(levels[i].max_child_processes <= levels[i - 1].max_child_processes);
        }
    }

    // ── ThreadId / ProcessId ────────────────────────────────────────────

    #[test]
    fn id_equality() {
        assert_eq!(ThreadId(0), ThreadId(0));
        assert_ne!(ThreadId(0), ThreadId(1));
        assert_eq!(ProcessId(42), ProcessId(42));
        assert_ne!(ProcessId(1), ProcessId(2));
    }

    #[test]
    fn id_copy() {
        let t = ThreadId(5);
        let t2 = t; // Copy
        assert_eq!(t, t2);
    }

    // ── ProcessId::index ────────────────────────────────────────────────

    #[test]
    fn process_id_index_zero() {
        assert_eq!(ProcessId(0).index(), Some(0));
    }

    #[test]
    fn process_id_index_last_valid() {
        assert_eq!(
            ProcessId((MAX_PROCESSES - 1) as u32).index(),
            Some(MAX_PROCESSES - 1)
        );
    }

    #[test]
    fn process_id_index_max_processes_is_out_of_range() {
        assert_eq!(ProcessId(MAX_PROCESSES as u32).index(), None);
    }

    #[test]
    fn process_id_index_u32_max_is_out_of_range() {
        assert_eq!(ProcessId(u32::MAX).index(), None);
    }

    #[test]
    fn process_id_index_in_range_ids_fit_the_table() {
        // Every pid that index() accepts must be a valid position in a
        // MAX_PROCESSES-entry table, i.e. safe to index with.
        let table = [(); MAX_PROCESSES];
        for raw in 0..=(MAX_PROCESSES as u32) {
            match ProcessId(raw).index() {
                Some(idx) => {
                    assert_eq!(idx, raw as usize);
                    assert!(table.get(idx).is_some());
                }
                None => assert_eq!(raw as usize, MAX_PROCESSES),
            }
        }
    }
}
