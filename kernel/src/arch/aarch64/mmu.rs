//! MMU and page table management for aarch64.
//!
//! Builds kernel page tables (TTBR1_EL1), enables the MMU, and provides
//! the identity map needed during the transition. Per memory.md §3.

// Kernel virtual address space layout (memory.md §3.1)
#[allow(dead_code)]
pub const KERNEL_BASE: usize = 0xFFFF_0000_0000_0000;
#[allow(dead_code)]
pub const DIRECT_MAP_BASE: usize = 0xFFFF_0001_0000_0000;
#[allow(dead_code)]
pub const MMIO_BASE: usize = 0xFFFF_0002_0000_0000;
#[allow(dead_code)]
pub const PAGE_SIZE: usize = 4096;
