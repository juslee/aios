//! GICv3 per-CPU timer PPI setup: register layout and readback rules.
//!
//! Crash-fix step 200 (#200) makes every CPU configure its timer PPI on its
//! own redistributor instead of relying on firmware: wake the redistributor,
//! disable the PPI and wait for `GICR_CTLR.RWP`, put it in Group 1, write its
//! priority, check that it is level-sensitive, enable it, and read the result
//! back. `kernel/src/arch/aarch64/gic.rs` does the MMIO. This module holds the
//! part that is pure logic, so that `just test` and `just miri` check it:
//!
//! - the register offsets and bit positions the sequence uses;
//! - [`PpiConfig`]: a validated (INTID, priority) pair and the masks, byte
//!   offset and bit position derived from it;
//! - [`PpiConfig::check`]: what a [`PpiReadback`] says about each property,
//!   as a [`PpiStatus`] of [`Flag`]s, with the `[smp]` line field values
//!   ([`Flag::field`]) and the failed flags' names ([`PpiStatus::failed_flags`]);
//! - [`waker_step`]: the next action of the redistributor wake sequence;
//! - [`security_disabled`]: `GICD_CTLR.DS` from a `GICD_CTLR` read.
//!
//! # Source
//!
//! Checked against Arm IHI 0069H.b, "Arm Generic Interrupt Controller (GIC)
//! Architecture Specification, v3 and v4" (GICv3.3 and GICv4.2, ID041224),
//! fetched from developer.arm.com on 2026-10-11:
//!
//! - §12.10: each redistributor has two contiguous 64 KiB frames, RD_base then
//!   SGI_base, so SGI_base = RD_base + 0x1_0000.
//! - §12.11.2 `GICR_CTLR` (RD_base + 0x0000): `RWP` is bit 3 and tracks writes
//!   to `GICR_ICENABLER0`.
//! - §12.11.42 `GICR_WAKER` (RD_base + 0x0014): bit 31 and bit 0 are
//!   IMPLEMENTATION DEFINED, bits \[30:3\] RES0, `ChildrenAsleep` bit 2,
//!   `ProcessorSleep` bit 1. "Changing ProcessorSleep from 1 to 0 when
//!   ChildrenAsleep is not 1 results in UNPREDICTABLE behavior", which is why
//!   [`waker_step`] waits in that state instead of clearing the bit. Under
//!   `DS` = 0 the register is RAZ/WI to Non-secure accesses, so it reads as
//!   awake.
//! - §12.11.12 `GICR_IGROUPR0` (SGI_base + 0x0080): a 1 is Group 1 when
//!   `DS` = 1. Under `DS` = 0 the register is RAZ/WI to Non-secure accesses,
//!   so a Non-secure write is ignored and reads back 0; the group is Secure
//!   firmware's to set and the kernel cannot check it ([`Flag::Unchecked`]).
//! - §12.11.14 `GICR_IGRPMODR0` (SGI_base + 0x0D00): RES0 when `DS` = 1 (an
//!   implementation may make it RAZ/WI), RAZ/WI to Non-secure accesses when
//!   `DS` = 0. A Non-secure read can therefore never show a Secure group, so
//!   the sequence does not access it; a PPI left in Group 0 or Secure Group 1
//!   under `DS` = 0 shows instead as a priority (and enable) readback of 0.
//! - §12.11.21 `GICR_IPRIORITYR<n>` (SGI_base + 0x0400 + 4n, byte-accessible,
//!   so INTID i's byte is at SGI_base + 0x0400 + i). Under `DS` = 0 a field of
//!   a Group 0 or Secure Group 1 interrupt is RAZ/WI to Non-secure accesses.
//! - §4.8 and Table 4-9: a GIC with one Security state implements at least 4
//!   priority bits (\[7:4\]); §4.8.7: one with two Security states implements at
//!   least 5 (\[7:3\]), and a Non-secure access to a Non-secure Group 1
//!   interrupt's priority stores `(v >> 1) | 0x80` and reads back the stored
//!   value shifted left by one. Unimplemented low-order bits are RAZ/WI. A
//!   priority whose low four bits are 0 therefore reads back exactly as
//!   written in every view.
//! - §4.8.6 (priority masking): writing 0xFF to `ICC_PMR_EL1` sets the lowest
//!   supported priority, which reads 0xF0 with 4 implemented bits, and only an
//!   interrupt of higher priority (numerically lower) than the mask is
//!   signalled. Hence [`PRIORITY_LIMIT`].
//! - §12.11.8 `GICR_ICFGR1` (SGI_base + 0x0C04): PPI i's `Int_config` is bits
//!   [2(i-16)+1 : 2(i-16)]; 0b00 is level, 0b10 edge, bit 2(i-16) RES0.
//!   Whether software can program it is IMPLEMENTATION DEFINED, and changing
//!   it while the interrupt is enabled is UNPREDICTABLE, so the sequence only
//!   reads it.
//! - §12.11.25 `GICR_ISENABLER0` (SGI_base + 0x0100) and §12.11.5
//!   `GICR_ICENABLER0` (SGI_base + 0x0180): bit i sets or clears INTID i's
//!   enable.
//! - §12.9.4 `GICD_CTLR`: bit 6 is `DS` in the view of a GIC with one Security
//!   state or with `DS` = 1 (RAO/WI with one Security state; once set, only a
//!   reset clears it). In the Non-secure view of a GIC with `DS` = 0, bits
//!   \[30:5\] are RES0. See [`security_disabled`] for what that means for the
//!   read.

/// Offset of a redistributor's SGI_base frame from its RD_base (IHI 0069H.b
/// §12.10).
pub const GICR_SGI_BASE_OFFSET: usize = 0x1_0000;

/// `GICR_CTLR`, Redistributor Control Register (RD_base offset).
pub const GICR_CTLR: usize = 0x0000;

/// `GICR_WAKER`, Redistributor Wake Register (RD_base offset).
pub const GICR_WAKER: usize = 0x0014;

/// `GICR_IGROUPR0`, Interrupt Group Register 0 (SGI_base offset).
pub const GICR_IGROUPR0: usize = 0x0080;

/// `GICR_ISENABLER0`, Interrupt Set-Enable Register 0 (SGI_base offset).
pub const GICR_ISENABLER0: usize = 0x0100;

/// `GICR_ICENABLER0`, Interrupt Clear-Enable Register 0 (SGI_base offset).
pub const GICR_ICENABLER0: usize = 0x0180;

/// `GICR_IPRIORITYR0`, the first Interrupt Priority Register (SGI_base
/// offset). INTID i's priority byte is at this offset plus i.
pub const GICR_IPRIORITYR0: usize = 0x0400;

/// `GICR_ICFGR1`, Interrupt Configuration Register 1, PPIs 16-31 (SGI_base
/// offset).
pub const GICR_ICFGR1: usize = 0x0C04;

/// `GICR_CTLR.RWP`: a write to `GICR_ICENABLER0` may not yet be visible.
pub const GICR_CTLR_RWP: u32 = 1 << 3;

/// `GICR_WAKER.ProcessorSleep`.
pub const GICR_WAKER_PROCESSOR_SLEEP: u32 = 1 << 1;

/// `GICR_WAKER.ChildrenAsleep` (read-only).
pub const GICR_WAKER_CHILDREN_ASLEEP: u32 = 1 << 2;

/// `GICD_CTLR.DS` (Disable Security), bit 6.
pub const GICD_CTLR_DS: u32 = 1 << 6;

/// The `GICR_IGROUPR0` value the setup writes: every SGI and PPI in Group 1
/// (Non-secure Group 1 when `DS` = 0). It matches what edk2 leaves on CPU 0.
pub const IGROUPR0_ALL_GROUP1: u32 = 0xFFFF_FFFF;

/// Lowest PPI INTID.
pub const PPI_INTID_MIN: u32 = 16;

/// Highest PPI INTID in the `GICR_*0` / `GICR_ICFGR1` range.
pub const PPI_INTID_MAX: u32 = 31;

/// Exclusive upper bound for a PPI priority byte.
///
/// The kernel writes `ICC_PMR_EL1` = 0xFF. With the minimum 4 implemented
/// priority bits (IHI 0069H.b §4.8, Table 4-9) that mask reads 0xF0, and the
/// GIC signals only interrupts whose priority is numerically below the mask,
/// so a priority of 0xF0 or above may never be signalled.
pub const PRIORITY_LIMIT: u8 = 0xF0;

/// Priority bits that may be unimplemented (RAZ/WI) or lost to the
/// Non-secure priority view; a priority must have them clear to read back
/// exactly.
pub const PRIORITY_LOW_BITS: u8 = 0x0F;

/// The timer PPI's priority byte on every CPU.
///
/// edk2 (stable202408, QEMU virt) leaves 0x80 in CPU 0's INTID-30 byte, its
/// default priority for every interrupt (read from the QEMU monitor on
/// 2026-10-11), and 0x80 meets every rule of [`PpiConfig::new`]. Writing it
/// on CPU 0 rewrites the value it already holds there, so CPU 0's GIC state
/// does not change.
pub const TIMER_PPI_PRIORITY: u8 = 0x80;

/// Why a [`PpiConfig`] was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GicError {
    /// The INTID is not a PPI in 16-31.
    IntidNotPpi(u32),
    /// Priority 0 cannot be told apart from a RAZ readback.
    PriorityZero,
    /// The priority is at or above [`PRIORITY_LIMIT`], so `ICC_PMR_EL1` =
    /// 0xFF may mask it.
    PriorityMasked(u8),
    /// The priority has a bit of [`PRIORITY_LOW_BITS`] set, so it may not read
    /// back as written.
    PriorityInexact(u8),
}

/// A validated timer PPI configuration: the INTID and the priority byte that
/// the setup writes and the readback check expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PpiConfig {
    intid: u32,
    priority: u8,
}

impl PpiConfig {
    /// Validate `intid` and `priority`.
    ///
    /// The INTID must be a PPI (16-31). The priority must be non-zero (so a
    /// RAZ readback cannot pass), below [`PRIORITY_LIMIT`] (so the PMR does
    /// not mask it) and have no bit of [`PRIORITY_LOW_BITS`] set (so the
    /// readback is exact in every priority view).
    pub const fn new(intid: u32, priority: u8) -> Result<Self, GicError> {
        if intid < PPI_INTID_MIN || intid > PPI_INTID_MAX {
            return Err(GicError::IntidNotPpi(intid));
        }
        if priority == 0 {
            return Err(GicError::PriorityZero);
        }
        if priority >= PRIORITY_LIMIT {
            return Err(GicError::PriorityMasked(priority));
        }
        if priority & PRIORITY_LOW_BITS != 0 {
            return Err(GicError::PriorityInexact(priority));
        }
        Ok(Self { intid, priority })
    }

    /// The PPI's INTID.
    pub const fn intid(self) -> u32 {
        self.intid
    }

    /// The priority byte written and expected back.
    pub const fn priority(self) -> u8 {
        self.priority
    }

    /// The PPI's bit in `GICR_IGROUPR0`, `GICR_ISENABLER0` and
    /// `GICR_ICENABLER0`.
    pub const fn enable_mask(self) -> u32 {
        1 << self.intid
    }

    /// The SGI_base offset of the PPI's `GICR_IPRIORITYR` byte.
    pub const fn priority_byte_offset(self) -> usize {
        GICR_IPRIORITYR0 + self.intid as usize
    }

    /// The position of the PPI's edge bit (`Int_config[1]`) in `GICR_ICFGR1`;
    /// the PPI is level-sensitive when this bit is 0.
    pub const fn icfgr1_edge_bit(self) -> u32 {
        2 * (self.intid - PPI_INTID_MIN) + 1
    }

    /// Judge a readback taken after the setup.
    ///
    /// `ds` is `GICD_CTLR.DS` as [`security_disabled`] reports it. The group
    /// flag is [`Flag::Unchecked`] when `ds` is false, because `GICR_IGROUPR0`
    /// is then RAZ/WI to the kernel's accesses; every other flag is always
    /// checked.
    pub const fn check(self, readback: &PpiReadback, ds: bool) -> PpiStatus {
        let mask = self.enable_mask();
        PpiStatus {
            wake: Flag::from_pass(readback.wake_ok),
            rwp: Flag::from_pass(readback.rwp_ok),
            group1: if ds {
                Flag::from_pass(readback.igroupr0 & mask != 0)
            } else {
                Flag::Unchecked
            },
            enabled: Flag::from_pass(readback.isenabler0 & mask != 0),
            priority: Flag::from_pass(readback.priority == self.priority),
            level: Flag::from_pass(readback.icfgr1 & (1 << self.icfgr1_edge_bit()) == 0),
        }
    }
}

/// What the setup observed on one CPU's redistributor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PpiReadback {
    /// The bounded `GICR_WAKER` sequence ended with the redistributor awake.
    pub wake_ok: bool,
    /// The bounded `GICR_CTLR.RWP` poll after the `GICR_ICENABLER0` write
    /// ended with RWP clear.
    pub rwp_ok: bool,
    /// `GICR_IGROUPR0` read after the group write.
    pub igroupr0: u32,
    /// `GICR_ISENABLER0` read after the enable write.
    pub isenabler0: u32,
    /// The PPI's `GICR_IPRIORITYR` byte read after the priority write.
    pub priority: u8,
    /// `GICR_ICFGR1` as read.
    pub icfgr1: u32,
}

/// The result of one property check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    /// The readback shows the property.
    Pass,
    /// The readback contradicts the property.
    Fail,
    /// The kernel cannot read the property (the group under `DS` = 0). Not a
    /// failure.
    Unchecked,
}

impl Flag {
    const fn from_pass(pass: bool) -> Self {
        if pass {
            Flag::Pass
        } else {
            Flag::Fail
        }
    }

    /// True for [`Flag::Fail`].
    pub const fn failed(self) -> bool {
        matches!(self, Flag::Fail)
    }

    /// The flag as an `[smp]` line field value: `1` for [`Flag::Pass`], `0`
    /// for [`Flag::Fail`], `-` for [`Flag::Unchecked`].
    pub const fn field(self) -> char {
        match self {
            Flag::Pass => '1',
            Flag::Fail => '0',
            Flag::Unchecked => '-',
        }
    }
}

/// One CPU's timer PPI setup result, one [`Flag`] per property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PpiStatus {
    /// The redistributor woke within the bound.
    pub wake: Flag,
    /// `GICR_CTLR.RWP` cleared within the bound after the disable.
    pub rwp: Flag,
    /// The PPI is in Group 1 (Unchecked under `DS` = 0).
    pub group1: Flag,
    /// The PPI is enabled.
    pub enabled: Flag,
    /// The priority byte reads back as written.
    pub priority: Flag,
    /// The PPI is level-sensitive.
    pub level: Flag,
}

impl PpiStatus {
    /// True when no flag is [`Flag::Fail`]; [`Flag::Unchecked`] does not fail.
    pub const fn ok(&self) -> bool {
        !(self.wake.failed()
            || self.rwp.failed()
            || self.group1.failed()
            || self.enabled.failed()
            || self.priority.failed()
            || self.level.failed())
    }

    /// Each flag with its name, in the order the setup establishes them.
    pub const fn flags(&self) -> [(&'static str, Flag); 6] {
        [
            ("wake", self.wake),
            ("rwp", self.rwp),
            ("group1", self.group1),
            ("enabled", self.enabled),
            ("priority", self.priority),
            ("level", self.level),
        ]
    }

    /// The names of the failed flags, for the setup-failure log line.
    pub const fn failed_flags(&self) -> FailedFlags {
        FailedFlags(*self)
    }
}

/// Displays the names of a [`PpiStatus`]'s [`Flag::Fail`] flags, separated by
/// spaces, in [`PpiStatus::flags`] order; [`Flag::Unchecked`] flags are not
/// failures and are not named. Empty when [`PpiStatus::ok`] is true.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailedFlags(PpiStatus);

impl core::fmt::Display for FailedFlags {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut first = true;
        for (name, flag) in self.0.flags() {
            if flag.failed() {
                if !first {
                    f.write_str(" ")?;
                }
                f.write_str(name)?;
                first = false;
            }
        }
        Ok(())
    }
}

/// The next action of the redistributor wake sequence, from a `GICR_WAKER`
/// read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakerStep {
    /// `ProcessorSleep` = 0 and `ChildrenAsleep` = 0: awake.
    Done,
    /// Read again: a wake is in progress (`ProcessorSleep` = 0,
    /// `ChildrenAsleep` = 1), or the redistributor is still entering sleep
    /// (`ProcessorSleep` = 1, `ChildrenAsleep` = 0), where clearing
    /// `ProcessorSleep` is UNPREDICTABLE (IHI 0069H.b §12.11.42).
    Wait,
    /// Write this value (`ProcessorSleep` cleared, every other bit as read,
    /// IMPLEMENTATION DEFINED bits 0 and 31 included), then read again.
    Write(u32),
}

/// Decide the next wake-sequence action from a `GICR_WAKER` value.
///
/// The caller loops, with a bound: read `GICR_WAKER`, act on the step, and
/// stop at [`WakerStep::Done`]. Unlike a wake guarded by "only if
/// `ChildrenAsleep`", this finishes a wake that is already in progress, and
/// it never clears `ProcessorSleep` before `ChildrenAsleep` reads 1. A
/// register that is RAZ/WI to the caller (Non-secure, `DS` = 0) reads 0 and
/// is [`WakerStep::Done`] at once.
pub const fn waker_step(waker: u32) -> WakerStep {
    let sleep = waker & GICR_WAKER_PROCESSOR_SLEEP != 0;
    let asleep = waker & GICR_WAKER_CHILDREN_ASLEEP != 0;
    match (sleep, asleep) {
        (false, false) => WakerStep::Done,
        (true, true) => WakerStep::Write(waker & !GICR_WAKER_PROCESSOR_SLEEP),
        (false, true) | (true, false) => WakerStep::Wait,
    }
}

/// `GICD_CTLR.DS` from a Non-secure `GICD_CTLR` read.
///
/// With `DS` = 1 the read uses the single Security state view, where bit 6 is
/// `DS` and reads 1 (IHI 0069H.b §12.9.4). With `DS` = 0 the Non-secure view
/// has bit 6 RES0, which in practice reads 0; Linux's
/// `gic_dist_security_disabled()` relies on the same read. The spec's RES0
/// definition does not promise a 0 there, and if a `DS` = 0 GIC ever read 1,
/// the group flag would be checked against a RAZ `GICR_IGROUPR0` and fail:
/// a visible false failure, never a false pass.
pub const fn security_disabled(gicd_ctlr: u32) -> bool {
    gicd_ctlr & GICD_CTLR_DS != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIMER_PPI: u32 = 30;

    fn timer_config() -> PpiConfig {
        PpiConfig::new(TIMER_PPI, TIMER_PPI_PRIORITY).expect("timer PPI config is valid")
    }

    /// The setup's readback when every write took effect.
    fn as_written(cfg: PpiConfig) -> PpiReadback {
        PpiReadback {
            wake_ok: true,
            rwp_ok: true,
            igroupr0: IGROUPR0_ALL_GROUP1,
            isenabler0: cfg.enable_mask(),
            priority: cfg.priority(),
            icfgr1: 0,
        }
    }

    fn all_pass() -> PpiStatus {
        PpiStatus {
            wake: Flag::Pass,
            rwp: Flag::Pass,
            group1: Flag::Pass,
            enabled: Flag::Pass,
            priority: Flag::Pass,
            level: Flag::Pass,
        }
    }

    #[test]
    fn bit_positions_for_ppi_16_30_31() {
        let cases = [
            (16, 1 << 16, 0x410, 1),
            (30, 1 << 30, 0x41E, 29),
            (31, 1 << 31, 0x41F, 31),
        ];
        for (intid, mask, offset, edge_bit) in cases {
            let cfg = PpiConfig::new(intid, TIMER_PPI_PRIORITY).unwrap();
            assert_eq!(cfg.intid(), intid);
            assert_eq!(cfg.enable_mask(), mask, "intid {intid}");
            assert_eq!(cfg.priority_byte_offset(), offset, "intid {intid}");
            assert_eq!(cfg.icfgr1_edge_bit(), edge_bit, "intid {intid}");
        }
    }

    #[test]
    fn timer_byte_lies_in_ipriorityr7_bits_23_16() {
        // The monitor reads the INTID 30 byte as bits [23:16] of the
        // IPRIORITYR7 word at SGI_base + 0x41C.
        let off = timer_config().priority_byte_offset();
        assert_eq!(off & !3, GICR_IPRIORITYR0 + 4 * 7);
        assert_eq!((off & 3) * 8, 16);
    }

    #[test]
    fn non_ppi_intids_rejected() {
        for intid in [0, 15, 32, 1020, u32::MAX] {
            assert_eq!(
                PpiConfig::new(intid, TIMER_PPI_PRIORITY),
                Err(GicError::IntidNotPpi(intid))
            );
        }
    }

    #[test]
    fn priority_rules() {
        assert_eq!(PpiConfig::new(TIMER_PPI, 0), Err(GicError::PriorityZero));
        assert_eq!(
            PpiConfig::new(TIMER_PPI, 0xFF),
            Err(GicError::PriorityMasked(0xFF))
        );
        assert_eq!(
            PpiConfig::new(TIMER_PPI, 0xF0),
            Err(GicError::PriorityMasked(0xF0))
        );
        assert_eq!(
            PpiConfig::new(TIMER_PPI, 0xA8),
            Err(GicError::PriorityInexact(0xA8))
        );
        assert_eq!(
            PpiConfig::new(TIMER_PPI, 0x81),
            Err(GicError::PriorityInexact(0x81))
        );
        for ok in [0x10, 0x80, 0xA0, 0xE0] {
            assert_eq!(
                PpiConfig::new(TIMER_PPI, ok).map(PpiConfig::priority),
                Ok(ok)
            );
        }
    }

    #[test]
    fn accepted_priorities_are_exactly_0x10_to_0xe0_in_steps_of_16() {
        let accepted: usize = (0..=u8::MAX)
            .filter(|&p| PpiConfig::new(TIMER_PPI, p).is_ok())
            .inspect(|&p| assert!(p % 0x10 == 0 && (0x10..=0xE0).contains(&p), "{p:#x}"))
            .count();
        assert_eq!(accepted, 14);
    }

    #[test]
    fn timer_priority_reads_back_exactly_in_every_view() {
        let p = TIMER_PPI_PRIORITY;
        // One Security state, 4 implemented bits [7:4].
        assert_eq!(p & 0xF0, p);
        // Two Security states, Non-secure view, 5 implemented bits [7:3]:
        // stored (p >> 1) | 0x80, read back as the stored value << 1.
        let stored = ((p >> 1) | 0x80) & 0xF8;
        assert_eq!(stored << 1, p);
    }

    #[test]
    fn edk2_cpu0_state_passes_with_ds1() {
        // CPU 0 on main, QEMU monitor 2026-10-11: IGROUPR0 0xffffffff,
        // ISENABLER0 0x40000000, IPRIORITYR7 0x80808080, ICFGR1 0.
        let readback = PpiReadback {
            wake_ok: true,
            rwp_ok: true,
            igroupr0: 0xFFFF_FFFF,
            isenabler0: 0x4000_0000,
            priority: 0x80,
            icfgr1: 0,
        };
        let status = timer_config().check(&readback, true);
        assert_eq!(status, all_pass());
        assert!(status.ok());
    }

    #[test]
    fn qemu_reset_state_on_cpus_1_to_3_fails_group_with_ds1() {
        // CPUs 1-3 on main, QEMU monitor 2026-10-11: IGROUPR0 0,
        // ISENABLER0 0x40000000, priority byte 0 (QEMU reset), ICFGR1 0.
        let readback = PpiReadback {
            wake_ok: true,
            rwp_ok: true,
            igroupr0: 0,
            isenabler0: 0x4000_0000,
            priority: 0,
            icfgr1: 0,
        };
        let status = timer_config().check(&readback, true);
        assert_eq!(status.group1, Flag::Fail);
        assert_eq!(status.priority, Flag::Fail);
        assert_eq!(status.enabled, Flag::Pass);
        assert_eq!(status.level, Flag::Pass);
        assert!(!status.ok());
    }

    #[test]
    fn skipped_group_write_fails_group_only_with_ds1() {
        // Every write but IGROUPR0's lands (a skipped group write).
        let cfg = timer_config();
        let readback = PpiReadback {
            igroupr0: 0,
            ..as_written(cfg)
        };
        let status = cfg.check(&readback, true);
        assert_eq!(
            status,
            PpiStatus {
                group1: Flag::Fail,
                ..all_pass()
            }
        );
        assert!(!status.ok());
    }

    #[test]
    fn raz_wi_group_is_unchecked_and_ok_with_ds0() {
        // DS = 0: IGROUPR0 is RAZ/WI to Non-secure accesses, the rest as
        // written (firmware put the PPI in Non-secure Group 1).
        let cfg = timer_config();
        let readback = PpiReadback {
            igroupr0: 0,
            ..as_written(cfg)
        };
        let status = cfg.check(&readback, false);
        assert_eq!(
            status,
            PpiStatus {
                group1: Flag::Unchecked,
                ..all_pass()
            }
        );
        assert!(status.ok());
    }

    #[test]
    fn group_is_unchecked_with_ds0_even_when_it_reads_set() {
        let cfg = timer_config();
        let status = cfg.check(&as_written(cfg), false);
        assert_eq!(status.group1, Flag::Unchecked);
        assert!(status.ok());
    }

    #[test]
    fn ignored_priority_write_fails_with_ds0() {
        // DS = 0 and Secure firmware left the PPI in a Secure group: its
        // priority field is RAZ/WI to Non-secure accesses.
        let cfg = timer_config();
        let readback = PpiReadback {
            igroupr0: 0,
            priority: 0,
            ..as_written(cfg)
        };
        let status = cfg.check(&readback, false);
        assert_eq!(status.priority, Flag::Fail);
        assert_eq!(status.group1, Flag::Unchecked);
        assert!(!status.ok());
    }

    #[test]
    fn secure_ppi_fails_priority_and_enable_with_ds0() {
        // The whole Secure-group state: ISENABLER0 and the priority byte are
        // RAZ/WI too, and ICFGR1's bit reads 0 (so level passes).
        let cfg = timer_config();
        let readback = PpiReadback {
            wake_ok: true,
            rwp_ok: true,
            igroupr0: 0,
            isenabler0: 0,
            priority: 0,
            icfgr1: 0,
        };
        let status = cfg.check(&readback, false);
        assert_eq!(
            status,
            PpiStatus {
                group1: Flag::Unchecked,
                enabled: Flag::Fail,
                priority: Flag::Fail,
                ..all_pass()
            }
        );
        assert!(!status.ok());
    }

    #[test]
    fn edge_triggered_fails_level() {
        let cfg = timer_config();
        let readback = PpiReadback {
            icfgr1: 1 << 29,
            ..as_written(cfg)
        };
        let status = cfg.check(&readback, true);
        assert_eq!(
            status,
            PpiStatus {
                level: Flag::Fail,
                ..all_pass()
            }
        );
        assert!(!status.ok());
    }

    #[test]
    fn other_ppis_edge_bits_do_not_affect_level() {
        let cfg = timer_config();
        let readback = PpiReadback {
            icfgr1: !(1 << 29),
            ..as_written(cfg)
        };
        assert_eq!(cfg.check(&readback, true).level, Flag::Pass);
    }

    #[test]
    fn other_ppis_bits_do_not_satisfy_group_or_enable() {
        let cfg = timer_config();
        let readback = PpiReadback {
            igroupr0: !cfg.enable_mask(),
            isenabler0: !cfg.enable_mask(),
            ..as_written(cfg)
        };
        let status = cfg.check(&readback, true);
        assert_eq!(status.group1, Flag::Fail);
        assert_eq!(status.enabled, Flag::Fail);
    }

    #[test]
    fn poll_timeouts_fail_wake_and_rwp() {
        let cfg = timer_config();
        let wake = cfg.check(
            &PpiReadback {
                wake_ok: false,
                ..as_written(cfg)
            },
            true,
        );
        assert_eq!(
            wake,
            PpiStatus {
                wake: Flag::Fail,
                ..all_pass()
            }
        );
        assert!(!wake.ok());
        let rwp = cfg.check(
            &PpiReadback {
                rwp_ok: false,
                ..as_written(cfg)
            },
            false,
        );
        assert_eq!(
            rwp,
            PpiStatus {
                rwp: Flag::Fail,
                group1: Flag::Unchecked,
                ..all_pass()
            }
        );
        assert!(!rwp.ok());
    }

    #[test]
    fn waker_step_for_the_four_sleep_states() {
        // (ProcessorSleep, ChildrenAsleep)
        assert_eq!(waker_step(0b000), WakerStep::Done); // (0, 0) awake
        assert_eq!(waker_step(0b100), WakerStep::Wait); // (0, 1) waking
        assert_eq!(waker_step(0b010), WakerStep::Wait); // (1, 0) entering sleep
        assert_eq!(waker_step(0b110), WakerStep::Write(0b100)); // (1, 1) asleep
    }

    #[test]
    fn waker_write_keeps_implementation_defined_bits() {
        assert_eq!(waker_step(0x8000_0007), WakerStep::Write(0x8000_0005));
        assert_eq!(waker_step(0x8000_0006), WakerStep::Write(0x8000_0004));
        assert_eq!(waker_step(0x0000_0007), WakerStep::Write(0x0000_0005));
    }

    #[test]
    fn waker_implementation_defined_bits_do_not_change_the_step() {
        assert_eq!(waker_step(0x8000_0001), WakerStep::Done);
        assert_eq!(waker_step(0x8000_0005), WakerStep::Wait);
        assert_eq!(waker_step(0x8000_0003), WakerStep::Wait);
    }

    #[test]
    fn waker_raz_reads_done() {
        // DS = 0: GICR_WAKER is RAZ/WI to Non-secure accesses.
        assert_eq!(waker_step(0), WakerStep::Done);
    }

    #[test]
    fn waker_sequence_from_sleep_reaches_done() {
        // A redistributor that follows §12.11.42: ProcessorSleep written,
        // ChildrenAsleep follows one read later.
        let mut sleep = true;
        let mut asleep = true;
        let mut writes = 0;
        for _ in 0..8 {
            let waker = (u32::from(sleep) << 1) | (u32::from(asleep) << 2);
            match waker_step(waker) {
                WakerStep::Done => break,
                WakerStep::Wait => asleep = sleep,
                WakerStep::Write(v) => {
                    writes += 1;
                    sleep = v & GICR_WAKER_PROCESSOR_SLEEP != 0;
                }
            }
        }
        assert!(!sleep && !asleep);
        assert_eq!(writes, 1);
    }

    #[test]
    fn ds_bit_from_gicd_ctlr() {
        // GICD_CTLR = 0x52 on QEMU virt without secure=on.
        assert!(security_disabled(0x0000_0052));
        assert!(!security_disabled(0x0000_0012));
        assert!(!security_disabled(0));
    }

    #[test]
    fn ppi_status_ok_ignores_unchecked() {
        let status = PpiStatus {
            group1: Flag::Unchecked,
            ..all_pass()
        };
        assert!(status.ok());
        assert!(!Flag::Unchecked.failed());
        assert!(Flag::Fail.failed());
        assert!(!Flag::Pass.failed());
    }

    #[test]
    fn flag_fields_for_the_smp_line() {
        assert_eq!(Flag::Pass.field(), '1');
        assert_eq!(Flag::Fail.field(), '0');
        assert_eq!(Flag::Unchecked.field(), '-');
    }

    #[test]
    fn failed_flags_empty_when_ok() {
        assert_eq!(alloc::format!("{}", all_pass().failed_flags()), "");
        let unchecked = PpiStatus {
            group1: Flag::Unchecked,
            ..all_pass()
        };
        assert_eq!(alloc::format!("{}", unchecked.failed_flags()), "");
    }

    #[test]
    fn failed_flags_names_the_skipped_group_write() {
        // The fault-injection case: the IGROUPR0 write skipped on a CPU 1-3
        // redistributor, which reads QEMU's reset value 0.
        let cfg = timer_config();
        let status = cfg.check(
            &PpiReadback {
                igroupr0: 0,
                ..as_written(cfg)
            },
            true,
        );
        assert_eq!(status.group1.field(), '0');
        assert_eq!(alloc::format!("{}", status.failed_flags()), "group1");
    }

    #[test]
    fn failed_flags_names_every_failure_in_check_order() {
        let all_fail = PpiStatus {
            wake: Flag::Fail,
            rwp: Flag::Fail,
            group1: Flag::Fail,
            enabled: Flag::Fail,
            priority: Flag::Fail,
            level: Flag::Fail,
        };
        assert_eq!(
            alloc::format!("{}", all_fail.failed_flags()),
            "wake rwp group1 enabled priority level"
        );
        let two = PpiStatus {
            rwp: Flag::Fail,
            group1: Flag::Unchecked,
            level: Flag::Fail,
            ..all_pass()
        };
        assert_eq!(alloc::format!("{}", two.failed_flags()), "rwp level");
    }
}
