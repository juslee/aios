//! 4-level page table infrastructure for aarch64.
//!
//! Provides `PageTableEntry`, `PageTable`, `VmFlags`, and address-space mapping
//! helpers with W^X enforcement built into the PTE API.
//!
//! Per memory.md §3.2.

// ── Page table entry ────────────────────────────────────────────────────

/// 64-bit aarch64 page table entry.
#[repr(transparent)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    // ── Bit positions ────────────────────────────────────────────────
    pub const VALID: u64 = 1 << 0;
    pub const TABLE: u64 = 1 << 1;
    pub const ATTR_IDX_SHIFT: u32 = 2;
    pub const ATTR_IDX_MASK: u64 = 0b111 << 2;
    pub const AP_USER: u64 = 1 << 6;
    pub const AP_RO: u64 = 1 << 7;
    pub const SH_INNER: u64 = 0b11 << 8;
    pub const AF: u64 = 1 << 10;
    pub const NG: u64 = 1 << 11;
    pub const PXN: u64 = 1 << 53;
    pub const UXN: u64 = 1 << 54;

    /// Physical address mask for PTE output address (bits [47:12]).
    pub const PHYS_MASK: u64 = 0x0000_FFFF_FFFF_F000;

    /// Empty (invalid) entry.
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Construct from a raw u64 value.
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Create a table descriptor pointing to the next-level page table.
    pub const fn new_table(next_table_phys: usize) -> Self {
        Self((next_table_phys as u64 & Self::PHYS_MASK) | Self::TABLE | Self::VALID)
    }

    /// Create a 4KB page descriptor (L3 leaf).
    ///
    /// `mair_idx` selects the memory attribute (0=device, 1=NC, 3=WB).
    /// `flags` determines permissions. W^X is enforced: panics if both
    /// WRITE and EXECUTE are set.
    pub fn new_page(frame_phys: usize, mair_idx: u64, flags: VmFlags) -> Self {
        assert!(
            !flags.contains(VmFlags::WRITE | VmFlags::EXECUTE),
            "W^X violation: page cannot be both writable and executable"
        );

        let mut pte = (frame_phys as u64 & Self::PHYS_MASK)
            | Self::VALID
            | Self::TABLE // bit1 = 1 for L3 page descriptor (required by ARM spec)
            | Self::AF
            | Self::SH_INNER
            | ((mair_idx & 0x7) << Self::ATTR_IDX_SHIFT);

        // Permission bits
        if !flags.contains(VmFlags::WRITE) {
            pte |= Self::AP_RO;
        }
        if !flags.contains(VmFlags::EXECUTE) {
            pte |= Self::PXN | Self::UXN;
        } else if !flags.contains(VmFlags::USER) {
            // Kernel-only executable: EL1 can execute (PXN=0), EL0 cannot (UXN=1).
            // Without this, kernel .text pages would be executable at EL0 — the
            // aarch64 equivalent of missing SMEP.
            pte |= Self::UXN;
        }
        if flags.contains(VmFlags::USER) {
            pte |= Self::AP_USER;
            pte |= Self::NG; // user pages are non-global (ASID-tagged)
        }

        Self(pte)
    }

    /// Create a 2MB block descriptor (L2 leaf).
    ///
    /// `block_phys` must be 2MB-aligned. Same W^X enforcement as `new_page`.
    pub fn new_block_2m(block_phys: usize, mair_idx: u64, flags: VmFlags) -> Self {
        assert!(block_phys & 0x1F_FFFF == 0, "2MB block must be 2MB-aligned");
        assert!(
            !flags.contains(VmFlags::WRITE | VmFlags::EXECUTE),
            "W^X violation: block cannot be both writable and executable"
        );

        // Block descriptor: bit[1] = 0 (not table)
        let mut pte = (block_phys as u64 & 0x0000_FFFF_FFE0_0000)
            | Self::VALID
            | Self::AF
            | Self::SH_INNER
            | ((mair_idx & 0x7) << Self::ATTR_IDX_SHIFT);

        if !flags.contains(VmFlags::WRITE) {
            pte |= Self::AP_RO;
        }
        if !flags.contains(VmFlags::EXECUTE) {
            pte |= Self::PXN | Self::UXN;
        } else if !flags.contains(VmFlags::USER) {
            pte |= Self::UXN; // kernel-only executable (see new_page)
        }
        if flags.contains(VmFlags::USER) {
            pte |= Self::AP_USER;
            pte |= Self::NG;
        }

        Self(pte)
    }

    // ── Queries ──────────────────────────────────────────────────────

    pub const fn is_valid(&self) -> bool {
        self.0 & Self::VALID != 0
    }

    pub const fn is_table(&self) -> bool {
        self.0 & (Self::VALID | Self::TABLE) == (Self::VALID | Self::TABLE)
    }

    pub const fn is_block(&self) -> bool {
        self.0 & (Self::VALID | Self::TABLE) == Self::VALID
    }

    pub const fn is_writable(&self) -> bool {
        self.0 & Self::AP_RO == 0
    }

    /// True if this page is executable at EL1 (kernel).
    /// PXN (Privileged Execute-Never) controls EL1 execution.
    pub const fn is_executable(&self) -> bool {
        self.0 & Self::PXN == 0
    }

    pub const fn is_user(&self) -> bool {
        self.0 & Self::AP_USER != 0
    }

    /// Extract the output physical address from this entry.
    pub const fn phys_addr(&self) -> usize {
        (self.0 & Self::PHYS_MASK) as usize
    }

    /// Extract the next-level table physical address (for table descriptors).
    pub const fn table_addr(&self) -> usize {
        self.phys_addr()
    }

    /// Raw u64 value.
    pub const fn raw(&self) -> u64 {
        self.0
    }

    // ── W^X-enforcing mutators ───────────────────────────────────────

    /// Make this page writable (clears executable bits).
    pub fn set_writable(&mut self) {
        self.0 &= !Self::AP_RO;
        self.0 |= Self::PXN | Self::UXN;
    }

    /// Make this page kernel-executable (sets read-only, clears PXN, sets UXN).
    /// EL1 can execute (PXN=0), EL0 cannot (UXN=1).
    pub fn set_executable(&mut self) {
        self.0 |= Self::AP_RO;
        self.0 &= !Self::PXN;
        self.0 |= Self::UXN; // block EL0 execution
    }
}

// ── VmFlags ─────────────────────────────────────────────────────────────

/// Virtual memory permission flags.
///
/// W^X invariant: `WRITE | EXECUTE` is illegal and will panic at map time.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct VmFlags(u32);

impl VmFlags {
    pub const READ: Self = Self(0b0000_0001);
    pub const WRITE: Self = Self(0b0000_0010);
    pub const EXECUTE: Self = Self(0b0000_0100);
    pub const USER: Self = Self(0b0000_1000);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn bits(&self) -> u32 {
        self.0
    }

    pub const fn contains(&self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl core::ops::BitOr for VmFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

// ── PageTable ───────────────────────────────────────────────────────────

/// 512-entry page table, 4KB-aligned.
#[repr(C, align(4096))]
pub struct PageTable {
    pub entries: [PageTableEntry; 512],
}

impl PageTable {
    /// Zero-initialize a page table.
    pub const fn new() -> Self {
        Self {
            entries: [PageTableEntry::empty(); 512],
        }
    }
}

// ── VA index extraction ─────────────────────────────────────────────────

/// Extract L0 (PGD) index from a 48-bit virtual address.
pub const fn l0_index(va: usize) -> usize {
    (va >> 39) & 0x1FF
}

/// Extract L1 (PUD) index from a 48-bit virtual address.
pub const fn l1_index(va: usize) -> usize {
    (va >> 30) & 0x1FF
}

/// Extract L2 (PMD) index from a 48-bit virtual address.
pub const fn l2_index(va: usize) -> usize {
    (va >> 21) & 0x1FF
}

/// Extract L3 (PTE) index from a 48-bit virtual address.
pub const fn l3_index(va: usize) -> usize {
    (va >> 12) & 0x1FF
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_entry_is_invalid() {
        let pte = PageTableEntry::empty();
        assert!(!pte.is_valid());
        assert!(!pte.is_table());
        assert!(!pte.is_block());
    }

    #[test]
    fn table_descriptor() {
        let pte = PageTableEntry::new_table(0x4000_1000);
        assert!(pte.is_valid());
        assert!(pte.is_table());
        assert!(!pte.is_block());
        assert_eq!(pte.table_addr(), 0x4000_1000);
    }

    #[test]
    fn page_descriptor_rx() {
        let flags = VmFlags::READ | VmFlags::EXECUTE;
        let pte = PageTableEntry::new_page(0x4008_0000, 3, flags);
        assert!(pte.is_valid());
        assert!(pte.is_table()); // L3 pages have bit1 set
        assert!(pte.is_executable());
        assert!(!pte.is_writable());
        assert_eq!(pte.phys_addr(), 0x4008_0000);
    }

    #[test]
    fn page_descriptor_rw() {
        let flags = VmFlags::READ | VmFlags::WRITE;
        let pte = PageTableEntry::new_page(0x4010_0000, 3, flags);
        assert!(pte.is_valid());
        assert!(pte.is_writable());
        assert!(!pte.is_executable());
    }

    #[test]
    #[should_panic(expected = "W^X violation")]
    fn wx_violation_panics() {
        let flags = VmFlags::READ | VmFlags::WRITE | VmFlags::EXECUTE;
        let _ = PageTableEntry::new_page(0x4000_0000, 3, flags);
    }

    #[test]
    fn set_writable_clears_exec() {
        let flags = VmFlags::READ | VmFlags::EXECUTE;
        let mut pte = PageTableEntry::new_page(0x4000_0000, 3, flags);
        assert!(pte.is_executable());
        assert!(!pte.is_writable());

        pte.set_writable();
        assert!(pte.is_writable());
        assert!(!pte.is_executable());
    }

    #[test]
    fn set_executable_sets_readonly() {
        let flags = VmFlags::READ | VmFlags::WRITE;
        let mut pte = PageTableEntry::new_page(0x4000_0000, 3, flags);
        assert!(pte.is_writable());
        assert!(!pte.is_executable());

        pte.set_executable();
        assert!(!pte.is_writable());
        assert!(pte.is_executable());
    }

    #[test]
    fn user_flag_sets_ap_user_and_ng() {
        let flags = VmFlags::READ | VmFlags::USER;
        let pte = PageTableEntry::new_page(0x4000_0000, 3, flags);
        assert!(pte.is_user());
        assert!(pte.raw() & PageTableEntry::NG != 0);
    }

    #[test]
    fn kernel_page_is_global() {
        let flags = VmFlags::READ;
        let pte = PageTableEntry::new_page(0x4000_0000, 3, flags);
        assert!(!pte.is_user());
        assert!(pte.raw() & PageTableEntry::NG == 0);
    }

    #[test]
    fn mair_index_encoded() {
        let flags = VmFlags::READ;
        // MAIR index 3 (WB cacheable)
        let pte = PageTableEntry::new_page(0x4000_0000, 3, flags);
        let attr_idx = (pte.raw() >> 2) & 0x7;
        assert_eq!(attr_idx, 3);

        // MAIR index 0 (device)
        let pte = PageTableEntry::new_page(0x4000_0000, 0, flags);
        let attr_idx = (pte.raw() >> 2) & 0x7;
        assert_eq!(attr_idx, 0);
    }

    #[test]
    fn block_2m_descriptor() {
        let flags = VmFlags::READ | VmFlags::WRITE;
        let pte = PageTableEntry::new_block_2m(0x4020_0000, 3, flags);
        assert!(pte.is_valid());
        assert!(pte.is_block()); // bit1 = 0 for block
        assert!(pte.is_writable());
        assert!(!pte.is_executable());
    }

    #[test]
    #[should_panic(expected = "2MB block must be 2MB-aligned")]
    fn block_2m_alignment_check() {
        let flags = VmFlags::READ;
        let _ = PageTableEntry::new_block_2m(0x4010_0000, 3, flags); // not 2MB aligned
    }

    #[test]
    #[should_panic(expected = "W^X violation")]
    fn block_2m_wx_violation() {
        let flags = VmFlags::READ | VmFlags::WRITE | VmFlags::EXECUTE;
        let _ = PageTableEntry::new_block_2m(0x4020_0000, 3, flags);
    }

    // ── VA index extraction tests ────────────────────────────────────

    #[test]
    fn va_index_extraction() {
        // KERNEL_BASE = 0xFFFF_0000_0000_0000
        // L0 index uses bits[47:39] — for upper-half addresses near the start,
        // these bits are all 0 (the 0xFFFF prefix is in bits[63:48]).
        let va = 0xFFFF_0000_0000_0000_usize;
        assert_eq!(l0_index(va), 0);
        assert_eq!(l1_index(va), 0);
        assert_eq!(l2_index(va), 0);
        assert_eq!(l3_index(va), 0);
    }

    #[test]
    fn va_index_with_offset() {
        // KERNEL_BASE + 0x80000 (512KB offset)
        let va = 0xFFFF_0000_0008_0000_usize;
        assert_eq!(l0_index(va), 0);
        assert_eq!(l1_index(va), 0);
        assert_eq!(l2_index(va), 0); // 512KB < 2MB, same PMD entry
        assert_eq!(l3_index(va), 0x80); // 0x80000 >> 12 = 0x80
    }

    #[test]
    fn va_index_direct_map() {
        // DIRECT_MAP_BASE = 0xFFFF_0001_0000_0000
        // bits[47:0] = 0x0001_0000_0000
        let va = 0xFFFF_0001_0000_0000_usize;
        assert_eq!(l0_index(va), 0);
        assert_eq!(l1_index(va), 4); // (0x1_0000_0000 >> 30) & 0x1FF = 4
    }

    #[test]
    fn va_index_round_trip() {
        // Arbitrary address: verify indices can reconstruct VA (minus offset)
        let va: usize = 0xFFFF_0000_1234_5000;
        let reconstructed = (l0_index(va) << 39)
            | (l1_index(va) << 30)
            | (l2_index(va) << 21)
            | (l3_index(va) << 12);
        // Mask to 48-bit address space (lower 48 bits)
        assert_eq!(
            reconstructed & 0x0000_FFFF_FFFF_F000,
            va & 0x0000_FFFF_FFFF_F000
        );
    }
}
