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
