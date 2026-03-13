//! aarch64-specific kernel code.
//!
//! Re-exports hardware abstraction modules for the ARM AArch64 architecture:
//! UART, exception handling, GICv3 interrupt controller, ARM Generic Timer,
//! MMU/page table management, PSCI power control, and trap/fault handling.
//!
//! Per hal.md §2-4.

pub mod exceptions;
pub mod gic;
pub mod mmu;
pub mod psci;
pub mod timer;
pub mod trap;
pub mod uart;
