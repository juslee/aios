//! QEMU virt platform implementation.

use crate::arch::aarch64::gic::{self, InterruptController};
use crate::arch::aarch64::timer::{self, Timer};
use crate::arch::aarch64::uart::{self, Uart};
use crate::dtb::DeviceTree;
use crate::platform::Platform;

/// QEMU virt machine platform.
pub struct QemuPlatform;

impl Platform for QemuPlatform {
    fn name(&self) -> &'static str {
        "QemuPlatform"
    }

    fn init_uart(&self, dt: &DeviceTree) -> Uart {
        let base = dt.uart_base.unwrap_or(0x0900_0000);
        uart::init_pl011(base as usize)
    }

    fn init_interrupts(&self, dt: &DeviceTree) -> InterruptController {
        let (gicd, gicr) = dt.gic_bases();
        gic::init_gicv3(gicd as usize, gicr as usize)
    }

    fn init_timer(&self, dt: &DeviceTree, ic: &InterruptController) -> Timer {
        timer::init_generic_timer(dt.timer_ppi, ic)
    }
}
