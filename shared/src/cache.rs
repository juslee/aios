//! AArch64 CPU cache geometry from `CTR_EL0` (Cache Type Register).
//!
//! Pure decoding and address arithmetic, so it can be tested on the host. The
//! register read and the cache maintenance instructions stay with the caller
//! (`uefi-stub/src/cache.rs`).

use core::ops::Range;

/// `CTR_EL0.DminLine`, bits [19:16]: log2 of the number of 4-byte words in the
/// smallest data or unified cache line.
const DMINLINE_SHIFT: u32 = 16;
const DMINLINE_MASK: u64 = 0xF;
/// `CTR_EL0.IDC`, bit 28.
const IDC_BIT: u64 = 1 << 28;
/// `CTR_EL0.DIC`, bit 29.
const DIC_BIT: u64 = 1 << 29;

/// The `CTR_EL0` fields that decide how to make data writes visible to
/// instruction fetch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheType {
    /// Smallest data or unified cache line, in bytes. Always a power of two
    /// from 4 to 128 KiB.
    dcache_line_bytes: u64,
    idc: bool,
    dic: bool,
}

impl CacheType {
    /// Decode a raw `CTR_EL0` value. Fields other than DminLine, IDC and DIC
    /// are ignored.
    pub const fn from_ctr_el0(ctr_el0: u64) -> Self {
        let dminline = (ctr_el0 >> DMINLINE_SHIFT) & DMINLINE_MASK;
        Self {
            dcache_line_bytes: 4 << dminline,
            idc: ctr_el0 & IDC_BIT != 0,
            dic: ctr_el0 & DIC_BIT != 0,
        }
    }

    /// Smallest data or unified cache line in bytes (`4 << DminLine`). Stepping
    /// a by-VA data cache instruction by this size reaches every line of every
    /// data cache.
    pub const fn dcache_line_bytes(self) -> u64 {
        self.dcache_line_bytes
    }

    /// `CTR_EL0.IDC`: when set, cleaning the data cache to the Point of
    /// Unification is not required for instruction-to-data coherence.
    pub const fn idc(self) -> bool {
        self.idc
    }

    /// `CTR_EL0.DIC`: when set, invalidating the instruction cache to the
    /// Point of Unification is not required for data-to-instruction coherence.
    pub const fn dic(self) -> bool {
        self.dic
    }

    /// One address per data cache line that overlaps `range`, for issuing a
    /// by-VA data cache instruction (`DC CVAU`, `DC CVAC`, ...) on each.
    ///
    /// The first address is `range.start` itself: a by-VA instruction acts on
    /// the whole line that holds its address and needs no alignment. Each later
    /// address is the start of the next line. Every address lies inside
    /// `range`, so all of them are mapped whenever the range is. An empty
    /// range yields nothing.
    pub fn dcache_line_addresses(self, range: Range<u64>) -> impl Iterator<Item = u64> {
        let line_mask = self.dcache_line_bytes - 1;
        let end = range.end;
        let first = (range.start < end).then_some(range.start);
        core::iter::successors(first, move |&addr| {
            let next = (addr & !line_mask).checked_add(line_mask + 1)?;
            (next < end).then_some(next)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    /// `CTR_EL0` of QEMU's `cortex-a72` model (and the Cortex-A72 TRM reset
    /// value): 64-byte lines, PIPT I-cache, IDC = DIC = 0.
    const CORTEX_A72_CTR: u64 = 0x8444_C004;

    fn addrs(ct: CacheType, range: Range<u64>) -> Vec<u64> {
        ct.dcache_line_addresses(range).collect()
    }

    #[test]
    fn decodes_cortex_a72() {
        let ct = CacheType::from_ctr_el0(CORTEX_A72_CTR);
        assert_eq!(ct.dcache_line_bytes(), 64);
        assert!(!ct.idc());
        assert!(!ct.dic());
    }

    // The next two tests use literal register values, not the module's field
    // constants, so a wrong bit position in the decoder fails them.

    #[test]
    fn decodes_idc_and_dic_independently() {
        // 0x8444_C004 with bit 28 (IDC) set.
        let idc = CacheType::from_ctr_el0(0x9444_C004);
        assert!(idc.idc());
        assert!(!idc.dic());

        // 0x8444_C004 with bit 29 (DIC) set.
        let dic = CacheType::from_ctr_el0(0xA444_C004);
        assert!(!dic.idc());
        assert!(dic.dic());

        let both = CacheType::from_ctr_el0(0xB444_C004);
        assert!(both.idc() && both.dic());
        assert_eq!(both.dcache_line_bytes(), 64);
    }

    #[test]
    fn dminline_is_read_from_bits_19_16() {
        // CWG = 5, ERG = 6, DminLine = 3, L1Ip = PIPT, IminLine = 2: every
        // size field differs, so decoding any other one gives another size.
        let ct = CacheType::from_ctr_el0(0x8563_C002);
        assert_eq!(ct.dcache_line_bytes(), 32);
        assert!(!ct.idc());
        assert!(!ct.dic());
    }

    #[test]
    fn dminline_covers_full_field() {
        for field in 0..=DMINLINE_MASK {
            let ct = CacheType::from_ctr_el0(field << DMINLINE_SHIFT);
            assert_eq!(ct.dcache_line_bytes(), 4 << field);
            assert!(ct.dcache_line_bytes().is_power_of_two());
        }
        assert_eq!(CacheType::from_ctr_el0(0).dcache_line_bytes(), 4);
        assert_eq!(
            CacheType::from_ctr_el0(DMINLINE_MASK << DMINLINE_SHIFT).dcache_line_bytes(),
            128 * 1024
        );
    }

    #[test]
    fn other_fields_do_not_leak_into_decode() {
        // Every bit set except DminLine, IDC and DIC.
        let others = !((DMINLINE_MASK << DMINLINE_SHIFT) | IDC_BIT | DIC_BIT);
        let ct = CacheType::from_ctr_el0(others | (4 << DMINLINE_SHIFT));
        assert_eq!(ct.dcache_line_bytes(), 64);
        assert!(!ct.idc());
        assert!(!ct.dic());
    }

    #[test]
    fn aligned_range_yields_each_line_once() {
        let ct = CacheType::from_ctr_el0(CORTEX_A72_CTR);
        assert_eq!(
            addrs(ct, 0x4008_0000..0x4008_0100),
            [0x4008_0000, 0x4008_0040, 0x4008_0080, 0x4008_00C0]
        );
    }

    #[test]
    fn page_yields_one_address_per_line() {
        let ct = CacheType::from_ctr_el0(CORTEX_A72_CTR);
        let got = addrs(ct, 0x4008_0000..0x4008_1000);
        assert_eq!(got.len(), 4096 / 64);
        assert!(got.windows(2).all(|w| w[1] - w[0] == 64));
    }

    #[test]
    fn unaligned_start_begins_at_start_then_aligns() {
        let ct = CacheType::from_ctr_el0(CORTEX_A72_CTR);
        assert_eq!(addrs(ct, 0x1030..0x10C1), [0x1030, 0x1040, 0x1080, 0x10C0]);
    }

    #[test]
    fn unaligned_end_includes_partial_last_line() {
        let ct = CacheType::from_ctr_el0(CORTEX_A72_CTR);
        assert_eq!(addrs(ct, 0x1000..0x1041), [0x1000, 0x1040]);
        assert_eq!(addrs(ct, 0x1000..0x1040), [0x1000]);
    }

    #[test]
    fn every_address_is_inside_the_range() {
        let ct = CacheType::from_ctr_el0(CORTEX_A72_CTR);
        for (start, end) in [(0x1001, 0x1002), (0x103F, 0x1041), (0x2000, 0x2FFF)] {
            let got = addrs(ct, start..end);
            assert!(!got.is_empty());
            assert!(got.iter().all(|&a| (start..end).contains(&a)));
        }
    }

    #[test]
    fn single_byte_range_yields_one_address() {
        let ct = CacheType::from_ctr_el0(CORTEX_A72_CTR);
        assert_eq!(addrs(ct, 0x1FFF..0x2000), [0x1FFF]);
    }

    #[test]
    fn empty_or_inverted_range_yields_nothing() {
        let ct = CacheType::from_ctr_el0(CORTEX_A72_CTR);
        assert!(addrs(ct, 0x1000..0x1000).is_empty());
        assert!(addrs(ct, 0x1041..0x1041).is_empty());
        let inverted = Range {
            start: 0x2000,
            end: 0x1000,
        };
        assert!(addrs(ct, inverted).is_empty());
    }

    #[test]
    fn range_at_top_of_address_space_terminates() {
        let ct = CacheType::from_ctr_el0(CORTEX_A72_CTR);
        let start = u64::MAX - 0x7F;
        assert_eq!(addrs(ct, start..u64::MAX), [start, u64::MAX - 0x3F]);
    }

    #[test]
    fn smallest_line_size_steps_by_four() {
        let ct = CacheType::from_ctr_el0(0);
        assert_eq!(addrs(ct, 0x1002..0x100A), [0x1002, 0x1004, 0x1008]);
    }
}
