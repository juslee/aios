//! ASID (Address Space Identifier) allocator.
//!
//! Each process gets a unique 16-bit ASID. TLB entries are tagged with the ASID
//! so context switches don't require full TLB flushes. When the ASID space wraps,
//! a generation bump + full TLB flush invalidates all stale entries.
//!
//! Per memory.md §3.4.

/// ASID value with generation tracking.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Asid {
    pub value: u16,
    pub generation: u64,
}

impl Asid {
    /// ASID 0 is reserved for the kernel address space.
    pub const KERNEL: Self = Self {
        value: 0,
        generation: 0,
    };
}

/// Sequential ASID allocator with generation-based invalidation.
pub struct AsidAllocator {
    generation: u64,
    next: u16,
    max: u16,
}

impl AsidAllocator {
    /// Create a new ASID allocator.
    ///
    /// ASID 0 is reserved for the kernel; allocation starts at 1.
    pub const fn new() -> Self {
        Self {
            generation: 0,
            next: 1,
            max: u16::MAX,
        }
    }

    /// Allocate a new ASID.
    ///
    /// Returns the ASID and a flag indicating whether a full TLB flush is needed
    /// (true on generation wraparound).
    pub fn alloc(&mut self) -> (Asid, bool) {
        let value = self.next;
        let mut needs_flush = false;

        self.next = self.next.wrapping_add(1);
        if self.next == 0 {
            // Wrapped around — skip 0 (kernel reserved), bump generation
            self.next = 1;
            self.generation = self.generation.wrapping_add(1);
            needs_flush = true;
        }

        (
            Asid {
                value,
                generation: self.generation,
            },
            needs_flush,
        )
    }

    /// Check whether an ASID is still valid (same generation).
    pub fn is_valid(&self, asid: &Asid) -> bool {
        asid.generation == self.generation
    }

    /// Current generation.
    #[allow(dead_code)]
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_alloc_starts_at_1() {
        let mut alloc = AsidAllocator::new();
        let (asid, flush) = alloc.alloc();
        assert_eq!(asid.value, 1);
        assert_eq!(asid.generation, 0);
        assert!(!flush);
    }

    #[test]
    fn sequential_allocation() {
        let mut alloc = AsidAllocator::new();
        for i in 1..=10 {
            let (asid, flush) = alloc.alloc();
            assert_eq!(asid.value, i);
            assert!(!flush);
        }
    }

    #[test]
    fn generation_wraps() {
        let mut alloc = AsidAllocator::new();
        // Set next to max to trigger wrap on next alloc after max
        alloc.next = u16::MAX;

        let (asid, flush) = alloc.alloc();
        assert_eq!(asid.value, u16::MAX);
        assert_eq!(asid.generation, 0);
        assert!(!flush); // this alloc is fine, wrap happens on next

        // This should trigger the wrap
        let (asid2, flush2) = alloc.alloc();
        assert_eq!(asid2.value, 1); // skips 0
        assert_eq!(asid2.generation, 1);
        assert!(flush2);
    }

    #[test]
    fn is_valid_checks_generation() {
        let mut alloc = AsidAllocator::new();
        let (asid_gen0, _) = alloc.alloc();
        assert!(alloc.is_valid(&asid_gen0));

        // Force generation bump
        alloc.next = u16::MAX;
        let _ = alloc.alloc(); // u16::MAX, gen 0
        let _ = alloc.alloc(); // wraps to 1, gen 1

        assert!(!alloc.is_valid(&asid_gen0)); // gen 0 is stale
    }

    #[test]
    fn kernel_asid_is_zero() {
        assert_eq!(Asid::KERNEL.value, 0);
    }
}
