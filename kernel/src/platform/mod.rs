//! Platform abstraction layer.
//!
//! Defines the `Platform` trait (hal.md §3) and platform detection from
//! the device tree's root compatible string.

pub mod qemu;

use crate::arch::aarch64::gic::InterruptController;
use crate::arch::aarch64::timer::Timer;
use crate::arch::aarch64::uart::Uart;
use crate::dtb::DeviceTree;

/// Hardware abstraction trait for platform-specific initialization.
///
/// Each platform (QEMU virt, Raspberry Pi 4/5, etc.) implements this trait
/// to initialize its specific hardware. Only `init_uart`, `init_interrupts`,
/// and `init_timer` are needed for Phase 1; others are Phase 2+.
pub trait Platform: Send + Sync {
    fn name(&self) -> &'static str;
    fn init_uart(&self, dt: &DeviceTree) -> Uart;
    fn init_interrupts(&self, dt: &DeviceTree) -> InterruptController;
    fn init_timer(&self, dt: &DeviceTree, ic: &InterruptController) -> Timer;
}

/// Detect the platform from the device tree root compatible string.
///
/// Returns a static reference because there is no heap at detection time.
pub fn detect_platform(dt: &DeviceTree) -> &'static dyn Platform {
    let compat = dt.root_compatible_str();

    if compat.contains("virt") || compat.contains("qemu") {
        static QEMU: qemu::QemuPlatform = qemu::QemuPlatform;
        return &QEMU;
    }

    panic!("Unknown platform: {}", compat);
}
