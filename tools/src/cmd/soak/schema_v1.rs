//! The frozen tripwire catalogue of schema `v=1` (crash-fix step 1b to
//! capability-lifetime step 10), copied from `shared/src/tripwire.rs` and
//! `shared/src/lock.rs` at `9498a9a`, the last commit that printed `v=1`.
//!
//! Capability-lifetime step 11 shrinks `WakeSource` from 16 to 12 and
//! `LockClass` from 9 to 8 and bumps `SCHEMA_VERSION` to 2, which shifts the
//! indices of the positional per-source and per-lock keys. The soak logs of
//! earlier branches and of `main` still hold `v=1` lines, so this module keeps
//! the old catalogue for [`super::catalogue::Schema::V1`]: nothing here reads
//! `shared`'s current sets. It changes only if a `v=1` line is found to have
//! been read wrongly, never with the kernel.
//!
//! A `v=1` line from before steps 8 to 10 is shorter: its per-source keys are
//! 15 wide (no `selftest`) and it lacks `ubtok`, `ubpend` and `bw`. A trailing
//! index or a key a line leaves out reads 0, so the catalogue is the step-10
//! one for all of them.

use shared::tripwire::Width;

use super::catalogue::KeyDef;

/// Every key of a `v=1` line, in print order, with its width and whether it
/// is a gauge. Only the variants of [`Width`] are used: the slot counts of
/// the sets are [`WAKE_SOURCES`], [`LOCK_CLASSES`] and the others below.
pub const KEYS: [KeyDef; 64] = [
    // Per CPU.
    key("tick", Width::Cpu),
    key("irqsw", Width::Cpu),
    key("irqsw0", Width::Cpu),
    key("nest", Width::Cpu),
    key("elrmm", Width::Cpu),
    key("spsrmm", Width::Cpu),
    key("n4", Width::Cpu),
    key("insched", Width::Cpu),
    key("rsthold", Width::Cpu),
    key("tpidrbad", Width::Cpu),
    // Global scalars.
    key("xdir", Width::One),
    key("xrep", Width::One),
    key("xnever", Width::One),
    key("n1", Width::One),
    key("pcnull", Width::One),
    key("pcphys", Width::One),
    key("pcother", Width::One),
    key("spbad", Width::One),
    key("latereply", Width::One),
    key("misrep", Width::One),
    key("ctbusy", Width::One),
    key("badtid", Width::One),
    key("n2", Width::N2),
    // Unblock outcomes per wake source.
    key("ubrun", Width::Source),
    key("ubrbl", Width::Source),
    key("ubdead", Width::Source),
    key("ubnone", Width::Source),
    key("ubmove", Width::Source),
    // Lock events per lock class.
    key("lkph", Width::Lock),
    key("lkpho", Width::Lock),
    key("lkphrun", Width::Lock),
    key("lkoxp", Width::Lock),
    key("lkself", Width::Lock),
    key("lktry", Width::Lock),
    key("lktph", Width::Lock),
    key("lkstk", Width::Lock),
    // The heartbeat scans.
    key("scana", Width::One),
    key("skipa", Width::One),
    key("skipaself", Width::One),
    key("scanb", Width::One),
    key("skipb", Width::One),
    key("scanstall", Width::One),
    key("hbdefer", Width::One),
    key("orphan", Width::One),
    key("nowaker", Width::One),
    key("wakefl", Width::One),
    key("starved", Width::Class),
    key("dupcur", Width::One),
    key("dupq", Width::One),
    key("qbad", Width::One),
    gauge("orphan_now", Width::One),
    gauge("nowaker_now", Width::One),
    gauge("wakefl_now", Width::One),
    gauge("scanhold1", Width::One),
    gauge("scanhold2", Width::One),
    key("lb", Width::One),
    key("enqfull", Width::One),
    key("badchan", Width::Badchan),
    key("twc", Width::One),
    key("twn", Width::One),
    gauge("twmax", Width::One),
    // Appended by capability-lifetime steps 9 and 10, still `v=1`.
    key("ubtok", Width::Source),
    key("ubpend", Width::Source),
    key("bw", Width::One),
];

const fn key(name: &'static str, width: Width) -> KeyDef {
    KeyDef {
        name,
        width,
        gauge: false,
    }
}

const fn gauge(name: &'static str, width: Width) -> KeyDef {
    KeyDef {
        name,
        width,
        gauge: true,
    }
}

/// The 16 wake sources, in index order of the per-source keys. Step 8
/// appended `selftest` (index 15); older lines are 15 wide.
pub const WAKE_SOURCES: [&str; 16] = [
    "call",
    "reply",
    "send",
    "selcall",
    "selsend",
    "selsig",
    "sig",
    "ndestroy",
    "nto",
    "nsto",
    "pwait",
    "to",
    "chdestroy",
    "pexit",
    "cancel",
    "selftest",
];

/// The 9 IRQ-shared lock statics, in index order of the per-lock keys.
pub const LOCK_CLASSES: [&str; 9] = [
    "THREAD_TABLE",
    "CURRENT_THREAD",
    "RUN_QUEUES",
    "WAKEUP_ERRORS",
    "TIMEOUT_QUEUE",
    "NOTIFY_DEADLINES",
    "NOTIFICATION_TABLE",
    "SELECT_WAITERS",
    "BOOT_LOG",
];

/// The scheduler classes, in the index order of `starved`.
pub const SCHEDULER_CLASSES: [&str; 4] = ["Idle", "Normal", "Interactive", "RealTime"];

/// The N2 kinds, in the index order of `n2`.
pub const N2_KINDS: [&str; 4] = ["rpre", "rblk", "vpre", "vblk"];

/// The badchan sites, in the index order of `badchan`.
pub const BADCHAN_SITES: [&str; 3] = ["cap", "select", "slot"];
