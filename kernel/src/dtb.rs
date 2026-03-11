//! Minimal device tree parsing for early boot.
//!
//! Uses fdt_parser's zero-alloc `base` module to parse the DTB and extract
//! all boot-critical values upfront into a plain struct.

use fdt_parser::base::Fdt;

/// Extracted device tree information needed for early boot (Steps 3–6).
pub struct DeviceTree {
    /// Root compatible string (e.g., "linux,dummy-virt" on QEMU).
    pub root_compatible: [u8; 64],
    pub root_compatible_len: usize,
    /// PL011 UART base address from compatible search.
    pub uart_base: Option<u64>,
    /// GICv3 distributor base.
    pub gicd_base: Option<u64>,
    /// GICv3 redistributor base.
    pub gicr_base: Option<u64>,
    /// Timer PPI INTID (typically 30 for non-secure physical timer).
    pub timer_ppi: u32,
    /// Number of CPU cores.
    cpu_count_val: usize,
    /// Per-CPU MPIDR values from DTB cpu@N reg property.
    cpu_mpidrs: [u64; 8],
    /// PSCI method: true = hvc, false = smc.
    pub psci_hvc: bool,
}

impl DeviceTree {
    /// Parse a device tree from a physical address and extract all needed values.
    ///
    /// # Safety
    /// `phys_addr` must point to a valid FDT blob in readable memory.
    pub unsafe fn parse(phys_addr: u64) -> Option<Self> {
        if phys_addr == 0 {
            return None;
        }

        // Read the FDT header to get totalsize, then construct the slice.
        let ptr = phys_addr as *const u8;

        // SAFETY: Caller guarantees phys_addr points to valid FDT memory.
        // FDT magic at offset 0 (big-endian 0xd00dfeed) and totalsize at offset 4.
        let magic = u32::from_be(core::ptr::read_volatile(ptr as *const u32));
        if magic != 0xd00dfeed {
            return None;
        }
        let totalsize = u32::from_be(core::ptr::read_volatile((ptr as *const u32).add(1))) as usize;

        // SAFETY: The FDT blob is totalsize bytes starting at phys_addr.
        let data = core::slice::from_raw_parts(ptr, totalsize);
        let fdt = Fdt::from_bytes(data).ok()?;

        let mut dt = DeviceTree {
            root_compatible: [0u8; 64],
            root_compatible_len: 0,
            uart_base: None,
            gicd_base: None,
            gicr_base: None,
            timer_ppi: 30, // default fallback
            cpu_count_val: 0,
            cpu_mpidrs: [0; 8],
            psci_hvc: true, // default: QEMU uses hvc
        };

        // Extract root compatible string from the root node (level 0).
        for node in fdt.all_nodes().flatten() {
            if node.is_root() {
                if let Ok(mut compats) = node.compatibles() {
                    if let Some(compat) = compats.next() {
                        let len = compat.len().min(63);
                        dt.root_compatible[..len].copy_from_slice(&compat.as_bytes()[..len]);
                        dt.root_compatible_len = len;
                    }
                }
                break;
            }
        }

        // Extract CPU count and MPIDR values from /cpus/cpu@N nodes.
        if let Some(cpus_node) = fdt.find_nodes("/cpus").flatten().next() {
            for child in cpus_node.children().flatten() {
                if child.name().starts_with("cpu@") {
                    let idx = dt.cpu_count_val;
                    if idx < 8 {
                        // Extract MPIDR from the `reg` property of each cpu@N node.
                        if let Ok(mut regs) = child.reg() {
                            if let Some(reg) = regs.next() {
                                dt.cpu_mpidrs[idx] = reg.address;
                            }
                        }
                    }
                    dt.cpu_count_val += 1;
                }
            }
        }

        // Extract UART base: search for arm,pl011 compatible node
        if let Some(node) = fdt.find_compatible(&["arm,pl011"]).flatten().next() {
            if let Ok(mut regs) = node.reg() {
                if let Some(reg) = regs.next() {
                    dt.uart_base = Some(reg.address);
                }
            }
        }

        // Extract GIC bases: search for arm,gic-v3 compatible node
        if let Some(node) = fdt.find_compatible(&["arm,gic-v3"]).flatten().next() {
            if let Ok(mut regs) = node.reg() {
                if let Some(gicd) = regs.next() {
                    dt.gicd_base = Some(gicd.address);
                    if let Some(gicr) = regs.next() {
                        dt.gicr_base = Some(gicr.address);
                    }
                }
            }
        }

        // Extract PSCI method
        if let Some(psci_node) = fdt.find_nodes("/psci").flatten().next() {
            if let Ok(method) = psci_node.find_property("method") {
                if let Ok(s) = method.str() {
                    dt.psci_hvc = s == "hvc";
                }
            }
        }

        // Extract timer PPI from armv8-timer interrupts property.
        // The timer node has 4 interrupt specifiers: secure phys, non-secure phys,
        // virtual, hyp. We want the second one (non-secure physical timer).
        if let Some(node) = fdt.find_compatible(&["arm,armv8-timer"]).flatten().next() {
            if let Ok(irqs) = node.interrupts() {
                // Skip first (secure phys), take second (non-secure phys)
                let mut irq_iter = irqs;
                let _secure = irq_iter.next();
                if let Some(ns_phys) = irq_iter.next() {
                    // GIC interrupt specifier: [type, number, flags]
                    // type=1 means PPI, number is the PPI index.
                    // INTID = PPI number + 16
                    let mut cells = [0u32; 3];
                    for (i, val) in ns_phys.enumerate() {
                        if i < 3 {
                            cells[i] = val;
                        }
                    }
                    if cells[0] == 1 {
                        // PPI type confirmed
                        dt.timer_ppi = cells[1] + 16;
                    }
                }
            }
        }

        Some(dt)
    }

    /// Construct a DeviceTree with known QEMU virt defaults.
    /// Used when no DTB is available (e.g., UEFI+ACPI boot without DTB config table).
    pub fn qemu_defaults() -> Self {
        let mut dt = DeviceTree {
            root_compatible: [0u8; 64],
            root_compatible_len: 0,
            uart_base: Some(0x0900_0000),
            gicd_base: Some(0x0800_0000),
            gicr_base: Some(0x080A_0000),
            timer_ppi: 30, // non-secure physical timer PPI INTID
            cpu_count_val: 4,
            cpu_mpidrs: [0, 1, 2, 3, 0, 0, 0, 0],
            psci_hvc: true,
        };
        let compat = b"linux,dummy-virt";
        dt.root_compatible[..compat.len()].copy_from_slice(compat);
        dt.root_compatible_len = compat.len();
        dt
    }

    /// Root compatible string as &str.
    pub fn root_compatible_str(&self) -> &str {
        core::str::from_utf8(&self.root_compatible[..self.root_compatible_len]).unwrap_or("unknown")
    }

    /// GIC bases as a tuple, with QEMU defaults as fallback.
    pub fn gic_bases(&self) -> (u64, u64) {
        (
            self.gicd_base.unwrap_or(0x0800_0000),
            self.gicr_base.unwrap_or(0x080A_0000),
        )
    }

    /// Number of CPU cores found in the DTB.
    pub fn cpu_count(&self) -> usize {
        self.cpu_count_val
    }

    /// MPIDR value for a given CPU index (for PSCI CPU_ON target_cpu parameter).
    pub fn cpu_mpidr(&self, index: usize) -> u64 {
        self.cpu_mpidrs[index]
    }

    /// PSCI method as a string.
    pub fn psci_method(&self) -> Option<&'static str> {
        if self.psci_hvc {
            Some("hvc")
        } else {
            Some("smc")
        }
    }
}
