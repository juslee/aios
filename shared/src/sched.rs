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

/// Unique process identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessId(pub u32);

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
    /// Suspended by the kernel (memory limit, debugging).
    Suspended,
    /// Thread has exited.
    Dead,
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
}
