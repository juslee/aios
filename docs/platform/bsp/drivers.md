# AIOS Driver Mapping and Device Tree Bindings

Part of: [bsp.md](../bsp.md) — Board Support Package Architecture
**Related:** [platforms.md](./platforms.md) — Per-platform hardware, [model.md](./model.md) — BSP model, [../../kernel/device-model.md](../../kernel/device-model.md) — Device model architecture

---

## §9 Driver Mapping Matrix

Each hardware function maps to exactly one driver file per platform. The kernel selects the correct driver at runtime based on the DTB `compatible` string — never at compile time. All drivers implement the abstract trait defined in `hal.md` §3; the kernel programs against the trait, not the concrete type.

The tables below use the following conventions for driver file paths:

- `arch/aarch64/` — architecture-level drivers that talk directly to ARM IP blocks
- `drivers/` — device-class drivers layered above the arch-level primitives
- `platform/` — per-board `Platform` trait implementations

### §9.1 Interrupt Controllers

The interrupt controller is the first peripheral the kernel programs after entering `kernel_main`. Its driver must be operational before any other IRQ-driven subsystem can start.

| Function | QEMU virt | Pi 4 (BCM2711) | Pi 5 (BCM2712) | Apple Silicon |
|---|---|---|---|---|
| Interrupt controller | GICv3 | GICv2 (GIC-400) | GICv3 | AIC (v1 on M1, v2 on M2+) |
| Driver file | `arch/aarch64/gic.rs` | `arch/aarch64/gicv2.rs` | `arch/aarch64/gic.rs` | `arch/aarch64/aic.rs` |
| DTB compatible | `arm,gic-v3` | `arm,gic-400` | `arm,gic-v3` | `apple,aic` / `apple,aic2` |
| IRQ model | SPI / PPI / LPI | SPI / PPI | SPI / PPI / LPI | HW events |
| IPI mechanism | `ICC_SGI1R_EL1` | `GICD_SGIR` | `ICC_SGI1R_EL1` | `AIC_IPI_SEND` |
| CPU interface | System registers (ICC_*) | MMIO (GICC_*) | System registers (ICC_*) | MMIO |
| Per-CPU redistributors | Yes (GICR) | No | Yes (GICR) | No |
| Affinity routing | Yes (GICD_CTLR.ARE) | No (bitmap targeting) | Yes (GICD_CTLR.ARE) | N/A |

Key differences between GICv2 and GICv3:

- **GICv2** — the CPU interface is memory-mapped (`GICC_IAR`, `GICC_EOIR`). IRQs are targeted to CPUs by a bitmask written to `GICD_ITARGETSR`. There are no redistributors. The Pi 4 uses a GIC-400 which implements GICv2.
- **GICv3** — the CPU interface is accessed via system registers (`ICC_IAR1_EL1`, `ICC_EOIR1_EL1`). Each CPU has a per-core redistributor block spaced 128 KiB apart (QEMU: `0x080A_0000`, stride `0x20000`). IRQ targeting uses affinity routing (`GICD_IROUTER`). QEMU virt and Pi 5 both use GICv3.
- **AIC** — Apple's proprietary interrupt controller found on all Apple Silicon SoCs. AIC v1 (M1 family) and AIC v2 (M2 and later) share the same event-driven model. Events are numbered from a flat space rather than SPI/PPI. FIQ is used for timer interrupts and IPIs rather than IRQ. The `aic.rs` driver translates the AIC event model to AIOS's abstract `InterruptController` trait.

### §9.2 Serial / UART

The UART is the first output device — it must be usable before any other driver is loaded. The `Platform::init_uart()` call (hal.md §3) returns a `&'static dyn Uart` handle that the kernel uses for all early logging.

| Feature | QEMU virt | Pi 4 (BCM2711) | Pi 5 (BCM2712) | Apple Silicon |
|---|---|---|---|---|
| Primary UART | PL011 | PL011 (UART0) | PL011 via RP1 | S5L UART |
| Driver file | `arch/aarch64/uart.rs` | `arch/aarch64/uart.rs` | `arch/aarch64/uart.rs` | `arch/aarch64/s5l_uart.rs` |
| DTB compatible | `arm,pl011` | `arm,pl011` | `arm,pl011` | `apple,s5l-uart` |
| Reference clock | 24 MHz | 48 MHz | 48 MHz | 24 MHz |
| Baud 115200 | IBRD=13, FBRD=1 | IBRD=26, FBRD=0 | IBRD=26, FBRD=0 | divisor register |
| FIFO depth | 16 bytes | 16 bytes | 16 bytes | 64 bytes |

PL011 register map (QEMU base `0x0900_0000`; Pi 4 base from DTB):

| Register | Offset | Purpose |
|---|---|---|
| `UARTDR` | `0x000` | Data (read = RX, write = TX) |
| `UARTFR` | `0x018` | Flags: TXFF (bit 5), BUSY (bit 3), RXFE (bit 4) |
| `UARTIBRD` | `0x024` | Integer baud divisor |
| `UARTFBRD` | `0x028` | Fractional baud divisor |
| `UARTLCR_H` | `0x02C` | Line control: word length, FIFO enable |
| `UARTCR` | `0x030` | Control: UARTEN, TXE, RXE |

S5L UART differences from PL011:

- Register layout is entirely different — `UCON` (control), `UTRSTAT` (TX/RX status), `UTXH` (TX data), `URXH` (RX data).
- Baud rate is set via a single 16-bit divisor register rather than IBRD/FBRD pair.
- FIFO status bits are in different positions; transmit-ready is `UTRSTAT[1]` (buffer empty) rather than `UARTFR.TXFF`.
- The `s5l_uart.rs` driver implements the same `Uart` trait as `uart.rs`; callers see no difference.

### §9.3 Storage

| Feature | QEMU virt | Pi 4 (BCM2711) | Pi 5 (BCM2712) | Apple Silicon |
|---|---|---|---|---|
| Primary storage | VirtIO-blk | EMMC2 (SD/eMMC) | SD via RP1 | ANS NVMe |
| Driver file | `drivers/virtio_blk.rs` | `drivers/sdhci.rs` | `drivers/sdhci.rs` | `drivers/ans.rs` |
| DTB compatible | `virtio,mmio` | `brcm,bcm2711-emmc2` | `brcm,bcm2712-sdhci` | `apple,ans3` |
| Transport | VirtIO MMIO (spec §4.2) | SDHCI registers | SDHCI over PCIe→RP1 | Custom NVMe over DART |
| DMA model | Virtqueue descriptors | ADMA2 | ADMA2 | DART-protected |
| Max transfer | Virtqueue length | 512 KiB (ADMA2 limit) | 512 KiB (ADMA2 limit) | 4 MiB |
| Block size | 512 bytes | 512 bytes | 512 bytes | 4 KiB |

VirtIO-blk specifics — see `docs/kernel/device-model/virtio.md` §10 for virtqueue internals. QEMU exposes the device at MMIO base `0x0A00_0000`, magic `0x74726976` ("virt"), device ID 2, polled I/O (no IRQ).

SDHCI specifics — the `sdhci.rs` driver handles both Pi 4 and Pi 5 because both expose a standard SDHCI register interface (though via different physical paths). The driver reads the base address and IRQ from the DTB node; no board-specific `#[cfg]` is needed.

ANS specifics — the Apple NVMe Storage Controller uses a proprietary command protocol layered over PCIe. All DMA is mediated by the DART IOMMU (`drivers/dart.rs`), which must be initialized before any ANS transfer. See §10.2 for the DTB node that exposes the DART.

### §9.4 GPU and Display

| Feature | QEMU virt | Pi 4 (BCM2711) | Pi 5 (BCM2712) | Apple Silicon |
|---|---|---|---|---|
| GPU | VirtIO-GPU / ramfb | VideoCore VI (VC4/V3D) | VideoCore VII (V3D 7.1) | AGX (G13/G14/G15/G16) |
| Display output | ramfb / VirtIO-GPU | HDMI via VideoCore | HDMI via VideoCore | DCP |
| GPU driver | `drivers/virtio_gpu.rs` | `drivers/v3d.rs` | `drivers/v3d.rs` | `drivers/agx.rs` |
| Display driver | `framebuffer.rs` | `drivers/vc4_hdmi.rs` | `drivers/vc4_hdmi.rs` | `drivers/dcp.rs` |
| DTB compatible (GPU) | `virtio,mmio` | `brcm,bcm2711-v3d` | `brcm,bcm2712-v3d` | `apple,agx` |
| DTB compatible (display) | n/a (GOP from UEFI) | `brcm,bcm2711-hdmi0` | `brcm,bcm2712-hdmi0` | `apple,dcp` |
| Rendering API | wgpu (Vulkan subset) | Vulkan 1.0 | Vulkan 1.2 | Metal-compatible |
| IOMMU | n/a | None (IOMMU bypass) | None | DART |

On QEMU the framebuffer is set up by the UEFI stub via GOP before `ExitBootServices`. The kernel uses the `BootInfo.framebuffer` field directly; no GPU driver is needed for basic display. The VirtIO-GPU driver (Phase 6+) enables accelerated rendering.

On Pi 4/5 the VideoCore GPU is controlled by the firmware mailbox (§10.2). The `vc4_hdmi.rs` driver handles display timing and HDMI output; `v3d.rs` handles the 3D pipeline exposed via a DRM/KMS-style interface.

On Apple Silicon the Display Coprocessor (DCP) is a separate ARM core running Apple firmware that owns the display pipeline. The `dcp.rs` driver communicates with DCP via a shared-memory IPC protocol. The AGX GPU uses the DART IOMMU for all command buffer and texture DMA.

### §9.5 Networking

| Feature | QEMU virt | Pi 4 (BCM2711) | Pi 5 (BCM2712) | Apple Silicon |
|---|---|---|---|---|
| NIC | VirtIO-Net | Genet v5 | Genet via RP1 | PCIe Ethernet |
| Driver file | `drivers/virtio_net.rs` | `drivers/genet.rs` | `drivers/genet.rs` | `drivers/pcie_nic.rs` |
| DTB compatible | `virtio,mmio` | `brcm,bcm2711-genet-v5` | `brcm,bcm2712-genet` | PCIe class `0200` |
| Speed | Configurable (host-limited) | 1 Gbps | 1 Gbps | 1–10 Gbps |
| DMA model | Virtqueue | HW descriptor ring | HW descriptor ring | PCIe DMA |
| RX offload | Host-provided | Checksum | Checksum | Checksum + TSO |

VirtIO-Net uses the same virtqueue transport as VirtIO-blk; see `docs/kernel/device-model/virtio.md` §10. The `genet.rs` driver on Pi 4/5 handles both hardware variants because the register interface is compatible; the DTB `compatible` string distinguishes them for any version-specific workarounds.

### §9.6 Random Number Generator

A hardware RNG is required during boot to seed KASLR (`kernel/src/mm/kaslr.rs`) and to populate `BootInfo.rng_seed` on platforms that provide it.

| Feature | QEMU virt | Pi 4 (BCM2711) | Pi 5 (BCM2712) | Apple Silicon |
|---|---|---|---|---|
| RNG | VirtIO-RNG | BCM2835 RNG | BCM2835 RNG | Apple TRNG |
| Driver file | `drivers/virtio_rng.rs` | `drivers/bcm_rng.rs` | `drivers/bcm_rng.rs` | `drivers/apple_trng.rs` |
| DTB compatible | `virtio,mmio` | `brcm,bcm2835-rng` | `brcm,bcm2835-rng` | `apple,trng` |
| Interface | Virtqueue read | MMIO status + data | MMIO status + data | MMIO data register |
| Entropy quality | Host CSPRNG | Hardware noise | Hardware noise | On-chip TRNG |

All RNG drivers expose the same `Rng::read_bytes(&mut [u8]) -> Result<(), RngError>` interface (hal.md §3). The kernel never reads more than 32 bytes at startup; drivers are not required to provide streaming throughput.

### §9.7 Timer

The ARM Generic Timer is the universal timekeeping source across all platforms. It is an architectural feature of ARMv8, not a peripheral, so there is no separate driver per board. The DTB `timer` node provides IRQ numbers for the four timer types (EL3/EL2/EL1 physical/virtual).

| Feature | QEMU virt | Pi 4 (BCM2711) | Pi 5 (BCM2712) | Apple Silicon |
|---|---|---|---|---|
| Timer source | ARM Generic Timer | ARM Generic Timer | ARM Generic Timer | ARM Generic Timer |
| Driver file | `arch/aarch64/timer.rs` | `arch/aarch64/timer.rs` | `arch/aarch64/timer.rs` | `arch/aarch64/timer.rs` |
| DTB compatible | `arm,armv8-timer` | `arm,armv8-timer` | `arm,armv8-timer` | `arm,armv8-timer` |
| `CNTFRQ_EL0` | 62.5 MHz | 54 MHz | 54 MHz | 24 MHz |
| EL1 physical PPI | 30 | 30 | 30 | 30 |
| Tick period | 1 ms (62500 counts) | 1 ms (54000 counts) | 1 ms (54000 counts) | 1 ms (24000 counts) |

The timer driver reads `CNTFRQ_EL0` at boot to compute `counts_per_ms` dynamically — no hardcoded frequency constant. See `arch/aarch64/timer.rs` for `init_timer()` and `timer_tick_handler()`.

---

## §10 Device Tree Bindings

The kernel parses the DTB using the `fdt-parser` crate (`kernel/src/dtb.rs`). This section specifies which nodes are mandatory, which are platform-specific, and what happens when a node is missing.

### §10.1 Mandatory Nodes

Every platform DTB must provide these nodes for the kernel to boot. Missing a fatal node causes `kernel_panic!` during `dtb.rs` initialization.

```text
/                               Root node
├── compatible                  "platform-vendor,board-name" (e.g. "brcm,bcm2711")
├── #address-cells              Must be 2 (64-bit addresses)
├── #size-cells                 Must be 2 (64-bit sizes)
│
├── chosen/                     Boot parameters
│   ├── bootargs                Kernel command line (optional, empty string if absent)
│   ├── stdout-path             Path to serial console node (e.g. "/soc/serial@9000000")
│   └── rng-seed                64 bytes of boot-time entropy (optional; KASLR falls back
│                               to CNTPCT_EL0 if absent)
│
├── memory@<base>/              Physical RAM descriptor (one or more regions)
│   ├── device_type             "memory"
│   └── reg                     <base-addr> <size> pairs
│
├── cpus/                       CPU topology
│   ├── #address-cells          1 (MPIDR affinity field)
│   ├── cpu@0/
│   │   ├── compatible          "arm,cortex-a72" (or appropriate core)
│   │   ├── device_type         "cpu"
│   │   ├── reg                 MPIDR value (e.g. 0x0 for core 0)
│   │   └── enable-method       "psci"
│   └── cpu@N/ ...
│
├── psci/                       Power State Coordination Interface
│   ├── compatible              "arm,psci-1.0" or "arm,psci-0.2"
│   ├── method                  "hvc" (QEMU/VMs) or "smc" (Pi 4/5 TF-A, m1n1)
│   └── cpu_on                  Function ID (optional; default 0xC4000003 for PSCI 1.0)
│
├── interrupt-controller@<base>/  GICv2, GICv3, or AIC
│   ├── compatible              (see §9.1)
│   ├── #interrupt-cells        3 (GICv2/v3) or 1 (AIC)
│   ├── interrupt-controller    (property, no value)
│   └── reg                     GICD base, GICC/GICR base(s)
│
└── timer/                      ARM Generic Timer (architectural, not MMIO)
    ├── compatible              "arm,armv8-timer"
    └── interrupts              EL3-phys, EL2-phys, EL1-phys, EL1-virt PPI entries
```

Mapping of mandatory nodes to `DeviceTree` struct fields in `kernel/src/dtb.rs`:

| DTB node / property | `DeviceTree` field | Notes |
|---|---|---|
| `/memory@*/reg` | `memory_regions: &[MemoryRegion]` | All usable RAM ranges |
| `/cpus/cpu@N/reg` | `cpu_mpidrs: &[u64]` | One entry per CPU node |
| `/psci/method` | `psci_method: PsciMethod` | `Hvc` or `Smc` |
| `/interrupt-controller@*/compatible` | `intc_compatible: &str` | Selects GICv2/v3/AIC driver |
| `/interrupt-controller@*/reg` | `intc_base: u64` (GICD/AIC) | Primary MMIO base |
| `/interrupt-controller@*/reg` (GICv3 only) | `gicr_base: u64` | Redistributor base |
| `/timer/interrupts[2]` | `timer_irq: u32` | EL1 physical PPI (usually 30) |
| `/chosen/stdout-path` | resolved to `uart_base: u64` | UART MMIO base for early log |
| `/chosen/rng-seed` | `rng_seed: Option<[u8; 64]>` | Boot entropy for KASLR |

### §10.2 Platform-Specific Nodes

These nodes are present on some platforms and absent on others. The kernel queries them after mandatory node parsing is complete.

**Raspberry Pi 4 / BCM2711:**

```text
/soc/
├── mailbox@7e00b840/           VideoCore firmware mailbox
│   ├── compatible              "brcm,bcm2835-mbox"
│   └── reg                     0x7e00b840 size 0x40
│
├── thermal@7d5d2000/           BCM2711 thermal sensor
│   ├── compatible              "brcm,bcm2711-thermal"
│   └── reg                     0x7d5d2000 size 0x10
│
├── emmc2@7e340000/             SD/eMMC controller (SDHCI)
│   ├── compatible              "brcm,bcm2711-emmc2"
│   └── reg                     0x7e340000 size 0x100
│
├── ethernet@7d580000/          Genet v5 NIC
│   ├── compatible              "brcm,bcm2711-genet-v5"
│   └── reg                     0x7d580000 size 0x10000
│
└── rng@7e104000/               BCM2835-compatible RNG
    ├── compatible              "brcm,bcm2835-rng"
    └── reg                     0x7e104000 size 0x10
```

The VideoCore mailbox is required to configure HDMI output and GPU memory split. Without it, display remains at the GOP framebuffer resolution provided by the UEFI stub.

**Raspberry Pi 5 / BCM2712:**

Pi 5 adds RP1 — a custom Raspberry Pi I/O chip connected via PCIe. Most peripherals (UART, SD card, USB, Ethernet) are behind RP1 rather than directly on the SoC MMIO bus. DTB nodes for Pi 5 peripherals carry RP1 PCIe region addresses rather than the direct MMIO addresses seen on Pi 4.

```text
/axi/
├── pcie@120000/                PCIe RC for RP1
│   ├── compatible              "brcm,bcm2712-pcie"
│   └── (RP1 child nodes enumerated via PCIe config space)
│
├── interrupt-controller@107d517000/  GICv3 (BCM2712)
│   ├── compatible              "arm,gic-v3"
│   └── reg                     GICD + GICR bases
│
└── thermal@107d415c00/         BCM2712 thermal sensor
    ├── compatible              "brcm,bcm2712-thermal"
    └── reg                     ...
```

**Apple Silicon (M1 / T8103):**

Apple Silicon DTBs are produced by m1n1. The top-level `arm-io` node contains all SoC peripherals. Each device that performs DMA has a sibling `dart@` node that describes its IOMMU. Addresses below are for M1 (T8103); other variants differ (see platforms.md §7.2).

```text
/arm-io/
├── aic@23b100000/              Apple Interrupt Controller v1
│   ├── compatible              "apple,aic"
│   ├── #interrupt-cells        1
│   └── reg                     0x23b100000 size 0xc000
│
├── uart0@235100000/            S5L UART (primary console)
│   ├── compatible              "apple,s5l-uart"
│   └── reg                     0x235100000 size 0x1000
│
├── ans@27bcc0000/              ANS NVMe storage controller
│   ├── compatible              "apple,ans3"
│   └── reg                     0x27bcc0000 size 0x100000
│
├── dart-ans@27bcc4000/         DART IOMMU for ANS
│   ├── compatible              "apple,t8103-dart"
│   └── reg                     0x27bcc4000 size 0x4000
│
├── agx@20e100000/              Apple GPU
│   ├── compatible              "apple,agx-g13g"  (M1; varies by die)
│   └── reg                     0x20e100000 size 0x1000000
│
├── dart-agx@20e104000/         DART IOMMU for AGX
│   ├── compatible              "apple,t8103-dart"
│   └── reg                     0x20e104000 size 0x4000
│
├── dcp@289020000/              Display Coprocessor
│   ├── compatible              "apple,dcp"
│   └── reg                     0x289020000 size 0x4000
│
└── smc@23d200000/              System Management Controller
    ├── compatible              "apple,smc"
    └── reg                     0x23d200000 size 0x4000
```

The SMC provides battery status, thermal sensors, keyboard backlight control, and the hardware power button. It communicates via a mailbox-style register protocol. The `smc.rs` driver is needed for thermal management (thermal.md §8) and power management (power-management.md).

### §10.3 DTB Validation

The kernel performs a sequence of consistency checks immediately after parsing the DTB, before any driver initialization. Failed checks either halt boot (fatal) or log a warning and use a fallback value.

| Node / property | Criticality | Failure behavior |
|---|---|---|
| Root `compatible` matches known platform | Fatal | `kernel_panic!("unknown platform: {}")` |
| At least one `/memory@*/reg` region | Fatal | `kernel_panic!("no memory regions in DTB")` |
| `/cpus/cpu@0` exists with valid `reg` | Fatal | `kernel_panic!("no boot CPU in DTB")` |
| `/interrupt-controller` exists | Fatal | `kernel_panic!("no interrupt controller in DTB")` |
| `/interrupt-controller` compatible recognized | Fatal | `kernel_panic!("unknown interrupt controller: {}")` |
| `/timer` node exists | Warning | Falls back to PPI 30 (EL1 physical timer); logged at `kwarn!` |
| `/psci` node exists | Warning | Falls back to HVC method; logged at `kwarn!` |
| `/chosen/stdout-path` resolves to valid UART node | Warning | Early UART continues from `BootInfo.uart_base`; logged at `kwarn!` |
| `/chosen/rng-seed` present and ≥ 32 bytes | Optional | KASLR uses `CNTPCT_EL0` entropy; no warning |
| CPU `enable-method` is `psci` for non-boot CPUs | Warning | SMP bringup skips CPUs without PSCI; logged at `kwarn!` |
| GICv3 GICR covers all CPU MPIDR values | Warning | Missing redistributors cause secondary core init failure at SMP bring-up |

The validation logic lives in `kernel/src/dtb.rs` function `DeviceTree::validate()`. It runs synchronously in `kernel_main` after `parse()` and before `detect_platform()` (hal.md §3). Any fatal error fires before memory initialization, so error output goes directly to the UART via `putc()` rather than through the log ring.

Platform detection uses the root `compatible` property to pick the `Platform` implementation:

```rust
pub fn detect_platform(dt: &DeviceTree) -> &'static dyn Platform {
    let compat = dt.root_compatible_str();
    if compat.contains("virt") || compat.contains("qemu") {
        static QEMU: QemuPlatform = QemuPlatform;
        return &QEMU;
    }
    if compat.contains("brcm,bcm2712") {
        static PI5: Pi5Platform = Pi5Platform;
        return &PI5;
    }
    if compat.contains("brcm,bcm2711") {
        static PI4: Pi4Platform = Pi4Platform;
        return &PI4;
    }
    if compat.contains("apple,t8103") {
        static M1: AppleSiliconPlatform =
            AppleSiliconPlatform { soc: AppleSoc::T8103 };
        return &M1;
    }
    // ... additional Apple SoC variants ...
    panic!("Unknown platform: {}", compat);
}
```

This function is the single point of platform selection. Adding a new board means adding one `contains` arm here and one new `Platform` implementation file under `kernel/src/platform/`.

---

*Part of the BSP Architecture document set. See [bsp.md](../bsp.md) Document Map for the complete list of related documents.*
