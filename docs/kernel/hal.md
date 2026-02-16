# AIOS Hardware Abstraction Layer (HAL)

## Deep Technical Architecture

**Parent document:** [architecture.md](../project/architecture.md) — Section 2.1 Full Stack Overview (Hardware Abstraction Layer)
**Related:** [boot.md](./boot.md) — Boot sequence and platform detection, [subsystem-framework.md](../platform/subsystem-framework.md) — Userspace device management, [scheduler.md](./scheduler.md) — Timer and GIC integration

-----

## 1. Overview

The HAL is the lowest layer of the AIOS kernel. It sits directly on hardware and exposes a uniform interface that the rest of the kernel programs against. The kernel never touches raw MMIO registers or device-specific data structures outside the HAL — all hardware access flows through trait implementations.

The HAL has one design goal: **adding a new platform is implementing seven methods.** A platform is a specific hardware board — QEMU virt, Raspberry Pi 4 (BCM2711), Raspberry Pi 5 (BCM2712), or any future aarch64 board. Each platform provides different hardware for the same logical functions (interrupts, timer, serial, GPU, network, storage, RNG). The HAL abstracts these differences behind a single `Platform` trait with seven initialization methods. For hardware that only some platforms provide (USB, WiFi, Bluetooth), the HAL uses extension traits that platforms opt into — see Section 12.

### 1.1 HAL Boundary

```
┌─────────────────────────────────────────────────────────────┐
│  Kernel Core                                                │
│  (scheduler, IPC, capability manager, memory manager)       │
│                                                             │
│  Programs against abstract types:                           │
│  InterruptController, Timer, Uart, GpuDevice,               │
│  NetworkDevice, StorageDevice                               │
├─────────────────────────────────────────────────────────────┤
│  HAL BOUNDARY — Platform trait + device traits              │
├─────────────────────────────────────────────────────────────┤
│  Platform Implementations                                   │
│                                                             │
│  QemuPlatform          │ Pi4Platform    │ Pi5Platform       │
│  ├── GICv3             │ ├── GICv2     │ ├── GICv3         │
│  ├── ARM Generic Timer │ ├── ARM Timer │ ├── ARM Timer     │
│  ├── PL011 UART        │ ├── PL011    │ ├── PL011         │
│  ├── VirtIO-GPU        │ ├── VC4/V3D  │ ├── V3D 7.1      │
│  ├── VirtIO-Net        │ ├── Genet    │ ├── Genet         │
│  └── VirtIO-Blk        │ └── SD/eMMC  │ └── SD/eMMC      │
├─────────────────────────────────────────────────────────────┤
│  Raw Hardware (MMIO registers, device memory, DMA)          │
└─────────────────────────────────────────────────────────────┘
```

### 1.2 What the HAL Is Not

The HAL handles **boot-time hardware initialization and kernel-level device access**. It does not cover:

- **Userspace device management** — handled by the Subsystem Framework (subsystem-framework.md). The Subsystem Framework builds on top of HAL-initialized devices.
- **Hot-plug device discovery** — handled by the Device Registry service in userspace. The HAL only initializes hardware present at boot.
- **High-level protocols** — TCP/IP, TLS, HTTP are userspace concerns. The HAL provides raw network device access.

-----

## 2. Platform Detection

Platform detection runs during kernel early boot (boot.md §3.3, Step 4) after the device tree is parsed. The kernel reads the root `compatible` string from the flattened device tree and selects the matching platform implementation:

```rust
/// Selected at boot time by reading the DTB compatible string.
pub fn detect_platform(dt: &DeviceTree) -> Box<dyn Platform> {
    let compat = dt.root_compatible();
    match compat {
        c if c.contains("qemu") => Box::new(QemuPlatform),
        c if c.contains("brcm,bcm2711") => Box::new(RaspberryPi4Platform),
        c if c.contains("brcm,bcm2712") => Box::new(RaspberryPi5Platform),
        _ => panic!("Unknown platform: {}", compat),
    }
}
```

**Compatible strings by platform:**

| Platform | DTB Compatible String | SoC |
|---|---|---|
| QEMU virt | `qemu,virt` | Virtual |
| Raspberry Pi 4 | `brcm,bcm2711` | BCM2711 |
| Raspberry Pi 5 | `brcm,bcm2712` | BCM2712 |

The detected platform is stored in `KernelState.platform` and used for all subsequent hardware initialization.

-----

## 3. Platform Trait

The `Platform` trait is the core abstraction. Every supported platform implements seven `init_*` methods — one for each hardware class the kernel needs during boot — plus an `as_any()` method for extension trait discovery (§12.3):

```rust
pub trait Platform: Send + Sync {
    /// Initialize the interrupt controller.
    ///
    /// QEMU / Pi 5: GICv3 (distributor + redistributor per CPU).
    /// Pi 4: GICv2 GIC-400 (distributor + CPU interface).
    ///
    /// Called during early boot Step 5. The returned controller is stored
    /// in KernelState and used by the scheduler for IRQ routing.
    fn init_interrupts(&self, dt: &DeviceTree) -> Result<InterruptController>;

    /// Initialize the system timer.
    ///
    /// All platforms use the ARM Generic Timer (CNTFRQ_EL0), but the
    /// frequency varies. QEMU: 62.5 MHz. Pi 4: 54 MHz. Pi 5: 54 MHz.
    ///
    /// Called during early boot Step 6. Configures the 1ms scheduler tick.
    fn init_timer(&self, dt: &DeviceTree) -> Result<Timer>;

    /// Initialize the serial console.
    ///
    /// All supported platforms use PL011 UART at different MMIO base
    /// addresses. The device tree provides the base address.
    ///
    /// Called during early boot Step 3. From this point kprintln!() works.
    fn init_uart(&self, dt: &DeviceTree) -> Result<Uart>;

    /// Initialize the GPU / display controller.
    ///
    /// QEMU: VirtIO-GPU (virtqueue-based, wgpu backend).
    /// Pi 4: VideoCore VI (VC4/V3D, Vulkan 1.0).
    /// Pi 5: VideoCore VII (V3D 7.1, Vulkan 1.2).
    ///
    /// Called during Phase 2 (core services) when the Display Subsystem
    /// starts. Returns a device handle the compositor programs against.
    fn init_gpu(&self, dt: &DeviceTree) -> Result<GpuDevice>;

    /// Initialize the primary network interface.
    ///
    /// QEMU: VirtIO-Net (virtqueue-based).
    /// Pi 4/5: Broadcom Genet (BCM54213PE Gigabit Ethernet).
    ///
    /// Called during Phase 2 when the Network Subsystem starts.
    /// Returns a device handle for the smoltcp network stack.
    fn init_network(&self, dt: &DeviceTree) -> Result<NetworkDevice>;

    /// Initialize the primary storage controller.
    ///
    /// QEMU: VirtIO-Blk (virtqueue-based).
    /// Pi 4/5: Arasan SDHCI (SD/eMMC) + XHCI (USB storage).
    ///
    /// Called during Phase 1 when the Block Engine starts.
    /// Returns a device handle for raw block I/O.
    fn init_storage(&self, dt: &DeviceTree) -> Result<StorageDevice>;

    /// Initialize the hardware random number generator.
    ///
    /// QEMU: VirtIO-RNG (virtqueue-based).
    /// Pi 4/5: bcm2835-rng (MMIO register).
    ///
    /// Called during early boot Step 10 (before KASLR). Supplements the
    /// one-shot UEFI rng_seed with a persistent entropy source for runtime
    /// crypto: capability token generation, nonces, key derivation.
    fn init_rng(&self, dt: &DeviceTree) -> Result<RngDevice>;

    /// Allows downcasting to concrete platform type for extension trait checks.
    /// See §12.3 for the runtime discovery pattern.
    fn as_any(&self) -> &dyn Any;
}
```

### 3.1 Platform Implementations

Each platform struct is zero-sized. All state lives in the returned device handles, not in the platform struct itself:

```rust
pub struct QemuPlatform;
pub struct RaspberryPi4Platform;
pub struct RaspberryPi5Platform;
```

### 3.2 Initialization Order

The seven methods are not called all at once. They're called at specific points during boot as their dependencies become available:

```
Early Boot (kernel space):
  Step 3:  init_uart()        — first sign of life (no heap)
  Step 5:  init_interrupts()  — enables IRQ routing (no heap)
  Step 6:  init_timer()       — enables preemptive scheduling (no heap)
  ──── Step 9: heap initialized ────
  Step 10: init_rng()         — entropy for KASLR and runtime crypto

Service Manager Phases (userspace, heap available):
  Phase 1: init_storage()     — Block Engine needs raw block access
  Phase 2: init_gpu()         — Display Subsystem needs GPU handle
  Phase 2: init_network()     — Network Subsystem needs NIC handle
```

UART, interrupts, and timer run before the heap exists (Steps 3/5/6) and must use only stack and static allocation. RNG runs just after heap init (Step 10) so VirtIO-RNG can allocate its virtqueue; the bcm2835-rng is pure MMIO but uniformity keeps the code simple. Storage, GPU, and network run in userspace service manager phases and can allocate freely.

-----

## 4. Device Abstractions

Each `init_*` method returns a device handle that abstracts the underlying hardware. The kernel and userspace services program against these abstractions.

### 4.1 InterruptController

```rust
/// Abstraction over GICv2 and GICv3.
pub struct InterruptController {
    variant: GicVariant,
    distributor_base: *mut u8,
    max_irqs: u32,
}

enum GicVariant {
    V2 {
        cpu_interface_base: *mut u8,
    },
    V3 {
        redistributor_base: *mut u8,
        redistributor_stride: usize,
    },
}

impl InterruptController {
    /// Enable a specific interrupt (SPI, PPI, or SGI).
    pub fn enable_irq(&self, irq: u32);

    /// Disable a specific interrupt.
    pub fn disable_irq(&self, irq: u32);

    /// Acknowledge an interrupt (read IAR). Returns the interrupt ID.
    /// Called by the IRQ handler at the start of interrupt servicing.
    pub fn acknowledge(&self) -> u32;

    /// Signal end-of-interrupt (write EOIR).
    /// Called by the IRQ handler after servicing completes.
    pub fn end_of_interrupt(&self, irq: u32);

    /// Set interrupt priority (0 = highest, 255 = lowest).
    pub fn set_priority(&self, irq: u32, priority: u8);

    /// Route an interrupt to a specific CPU (GICv3: affinity routing).
    pub fn set_target(&self, irq: u32, cpu: u32);

    /// Send a software-generated interrupt (SGI) to another CPU.
    /// Used for inter-processor interrupts (IPI) during SMP bringup.
    pub fn send_ipi(&self, target_cpu: u32, sgi_id: u32);

    /// Return the GIC version for platform-specific paths.
    pub fn version(&self) -> GicVersion;
}

pub enum GicVersion {
    V2,
    V3,
}
```

**GICv2 vs GICv3 differences handled internally:**

| Operation | GICv2 (Pi 4) | GICv3 (QEMU, Pi 5) |
|---|---|---|
| Acknowledge IRQ | Read GICC_IAR | Read ICC_IAR1_EL1 (system register) |
| End of interrupt | Write GICC_EOIR | Write ICC_EOIR1_EL1 |
| CPU target | GICD_ITARGETSR (8-bit bitmap) | GICD_IROUTER (affinity value) |
| Per-CPU config | GICC_* registers | GICR_* per redistributor |
| Max SPIs | 1020 | 1020 (LPIs extend to millions) |

### 4.2 Timer

```rust
/// ARM Generic Timer abstraction.
///
/// All platforms use the ARM architectural timer. Differences are
/// limited to frequency (read from CNTFRQ_EL0) and the GIC IRQ
/// number for timer interrupts (read from device tree).
pub struct Timer {
    frequency_hz: u64,
    tick_interval: u64,     // counter ticks per scheduler tick (1ms)
    timer_irq: u32,         // GIC IRQ number for the physical timer
}

impl Timer {
    /// Read the current counter value (CNTVCT_EL0).
    pub fn now(&self) -> u64;

    /// Convert counter ticks to nanoseconds.
    pub fn ticks_to_ns(&self, ticks: u64) -> u64;

    /// Convert nanoseconds to counter ticks.
    pub fn ns_to_ticks(&self, ns: u64) -> u64;

    /// Set the next timer interrupt to fire after `ticks` counter ticks.
    /// Writes CNTP_CVAL_EL0 = CNTVCT_EL0 + ticks.
    pub fn set_next_deadline(&self, ticks: u64);

    /// Enable the physical timer (CNTP_CTL_EL0.ENABLE = 1).
    pub fn enable(&self);

    /// Disable the physical timer.
    pub fn disable(&self);

    /// Return the timer frequency in Hz.
    pub fn frequency(&self) -> u64;

    /// Return the GIC IRQ number for this timer.
    pub fn irq(&self) -> u32;
}
```

**Timer frequencies by platform:**

| Platform | CNTFRQ_EL0 | Ticks per 1ms |
|---|---|---|
| QEMU virt | 62,500,000 Hz | 62,500 |
| Raspberry Pi 4 | 54,000,000 Hz | 54,000 |
| Raspberry Pi 5 | 54,000,000 Hz | 54,000 |

### 4.3 Uart

```rust
/// PL011 UART abstraction.
///
/// All supported platforms use the ARM PL011 UART. Only the MMIO
/// base address differs (provided by the device tree).
pub struct Uart {
    base: *mut u8,
}

impl Uart {
    /// Write a single byte. Blocks if the transmit FIFO is full.
    pub fn write_byte(&self, byte: u8);

    /// Read a single byte. Returns None if the receive FIFO is empty.
    pub fn read_byte(&self) -> Option<u8>;

    /// Write a string (convenience wrapper over write_byte).
    pub fn write_str(&self, s: &str);

    /// Check if data is available to read.
    pub fn has_data(&self) -> bool;

    /// Flush the transmit FIFO (wait until all bytes are sent).
    pub fn flush(&self);
}
```

The UART is initialized to 115200 baud, 8N1, no flow control on all platforms. Configuration is hardcoded — there's no need for runtime baud rate changes.

**PL011 registers used (offset from base):**

| Register | Offset | Purpose |
|---|---|---|
| UARTDR | 0x000 | Data register (read/write) |
| UARTFR | 0x018 | Flag register (TXFF, RXFE bits) |
| UARTIBRD | 0x024 | Integer baud rate divisor |
| UARTFBRD | 0x028 | Fractional baud rate divisor |
| UARTLCR_H | 0x02C | Line control (8N1 config) |
| UARTCR | 0x030 | Control register (enable TX/RX) |
| UARTIMSC | 0x038 | Interrupt mask |

### 4.4 GpuDevice

```rust
/// GPU device abstraction.
///
/// Provides the interface the Display Subsystem and Compositor
/// program against. Hides VirtIO-GPU vs VideoCore differences.
pub struct GpuDevice {
    variant: GpuVariant,
    capabilities: GpuCapabilities,
}

enum GpuVariant {
    VirtioGpu {
        virtqueues: VirtioQueues,
        scanout_id: u32,
    },
    VideoCore {
        v3d_base: *mut u8,
        hvs_base: *mut u8,
        version: VideoCoreVersion,
    },
}

pub enum VideoCoreVersion {
    /// Raspberry Pi 4: VC4/V3D 4.2, Vulkan 1.0
    V4,
    /// Raspberry Pi 5: V3D 7.1, Vulkan 1.2
    V7,
}

pub struct GpuCapabilities {
    pub max_texture_size: u32,
    pub max_framebuffers: u32,
    pub vulkan_version: Option<(u32, u32)>,  // (major, minor)
    pub supports_compute: bool,
    pub video_memory_bytes: usize,
}

impl GpuDevice {
    /// Allocate a framebuffer of the given dimensions.
    pub fn allocate_framebuffer(
        &self,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<Framebuffer>;

    /// Present a framebuffer to the display (page flip / scanout).
    pub fn present(&self, fb: &Framebuffer) -> Result<()>;

    /// Return the current display resolution.
    pub fn display_resolution(&self) -> (u32, u32);

    /// Set the display resolution (if supported).
    pub fn set_resolution(&self, width: u32, height: u32) -> Result<()>;

    /// Return the GPU capabilities for this platform.
    pub fn capabilities(&self) -> &GpuCapabilities;

    /// Create a render context for the wgpu backend.
    /// The compositor uses this to obtain a wgpu::Device.
    pub fn create_render_context(&self) -> Result<RenderContext>;
}
```

**GPU differences by platform:**

| Feature | QEMU (VirtIO-GPU) | Pi 4 (VC4) | Pi 5 (V3D 7.1) |
|---|---|---|---|
| API | VirtIO virtqueues | V3D MMIO | V3D MMIO |
| Vulkan | Via host GPU | 1.0 (conformant) | 1.2 (conformant) |
| Compute shaders | Host-dependent | No | Yes |
| Video memory | Shared (host) | 256 MB dedicated | 512 MB dedicated |
| Max resolution | Host-dependent | 4K@60 (single) | 4K@60 (dual) |

### 4.5 NetworkDevice

```rust
/// Network device abstraction.
///
/// Provides raw frame send/receive for the smoltcp network stack.
/// Hides VirtIO-Net vs Broadcom Genet differences.
pub struct NetworkDevice {
    variant: NetworkVariant,
    mac_address: [u8; 6],
    mtu: u16,
}

enum NetworkVariant {
    VirtioNet {
        rx_queue: VirtioQueue,
        tx_queue: VirtioQueue,
    },
    BroadcomGenet {
        base: *mut u8,
        dma_base: *mut u8,
        phy_addr: u8,
    },
}

impl NetworkDevice {
    /// Send a raw Ethernet frame.
    pub fn transmit(&self, frame: &[u8]) -> Result<()>;

    /// Receive a raw Ethernet frame into the provided buffer.
    /// Returns the number of bytes written, or None if no frame is available.
    pub fn receive(&self, buffer: &mut [u8]) -> Result<Option<usize>>;

    /// Return the MAC address.
    pub fn mac_address(&self) -> [u8; 6];

    /// Return the maximum transmission unit.
    pub fn mtu(&self) -> u16;

    /// Check if the link is up.
    pub fn link_up(&self) -> bool;

    /// Return the negotiated link speed in Mbps (10, 100, 1000).
    pub fn link_speed(&self) -> u32;
}
```

**Network differences by platform:**

| Feature | QEMU (VirtIO-Net) | Pi 4/5 (Genet) |
|---|---|---|
| Interface | Virtqueue (2 queues) | MMIO + DMA rings |
| Speed | Host-dependent | 1 Gbps |
| MAC address | QEMU-assigned | OTP fuses |
| Checksum offload | Via virtio features | Hardware |
| MTU | 1500 (configurable) | 1500 |

### 4.6 StorageDevice

```rust
/// Block storage device abstraction.
///
/// Provides raw block read/write for the Block Engine.
/// Hides VirtIO-Blk vs SD/eMMC differences.
pub struct StorageDevice {
    variant: StorageVariant,
    block_size: u32,
    total_blocks: u64,
}

enum StorageVariant {
    VirtioBlk {
        virtqueue: VirtioQueue,
    },
    SdMmc {
        sdhci_base: *mut u8,
        card_type: SdCardType,
    },
}

pub enum SdCardType {
    SdHc,    // SD High Capacity
    SdXc,    // SD Extended Capacity
    Emmc,    // Embedded MMC
}

impl StorageDevice {
    /// Read `count` blocks starting at `lba` into `buffer`.
    /// Buffer must be at least count * block_size bytes.
    pub fn read_blocks(
        &self,
        lba: u64,
        count: u32,
        buffer: &mut [u8],
    ) -> Result<()>;

    /// Write `count` blocks starting at `lba` from `buffer`.
    pub fn write_blocks(
        &self,
        lba: u64,
        count: u32,
        buffer: &[u8],
    ) -> Result<()>;

    /// Return the block size in bytes (typically 512).
    pub fn block_size(&self) -> u32;

    /// Return the total number of blocks.
    pub fn total_blocks(&self) -> u64;

    /// Return the total capacity in bytes.
    pub fn capacity_bytes(&self) -> u64 {
        self.total_blocks * self.block_size as u64
    }

    /// Flush any cached writes to stable storage.
    pub fn flush(&self) -> Result<()>;
}
```

**Storage differences by platform:**

| Feature | QEMU (VirtIO-Blk) | Pi 4/5 (SD/eMMC) |
|---|---|---|
| Interface | Virtqueue (1 queue) | SDHCI (Arasan) |
| Block size | 512 bytes | 512 bytes |
| Max capacity | Host file size | Card dependent |
| Flush semantics | Host fsync | CMD12/CMD23 |
| DMA | VirtIO scatter-gather | ADMA2 |
| Typical speed | Host disk speed | ~90 MB/s (UHS-I) |

### 4.7 RngDevice

```rust
/// Hardware random number generator abstraction.
///
/// Provides cryptographically secure random bytes for KASLR,
/// capability token generation, nonce creation, and key derivation.
/// Supplements the one-shot UEFI rng_seed from BootInfo.
pub struct RngDevice {
    variant: RngVariant,
}

enum RngVariant {
    VirtioRng {
        virtqueue: VirtioQueue,
    },
    Bcm2835 {
        base: *mut u8,
    },
}

impl RngDevice {
    /// Fill `buffer` with cryptographically secure random bytes.
    /// Blocks until the hardware RNG has enough entropy.
    pub fn fill_bytes(&self, buffer: &mut [u8]) -> Result<()>;

    /// Read a single random u64. Convenience wrapper.
    pub fn next_u64(&self) -> Result<u64>;

    /// Check if the RNG has entropy available (non-blocking).
    pub fn entropy_available(&self) -> bool;
}
```

**RNG differences by platform:**

| Feature | QEMU (VirtIO-RNG) | Pi 4/5 (bcm2835-rng) |
|---|---|---|
| Interface | Virtqueue (1 queue) | MMIO (4 registers) |
| Entropy source | Host `/dev/urandom` | Hardware TRNG |
| Throughput | Host-dependent | ~1 MB/s |
| Blocking | Via virtqueue completion | Poll RNG_STATUS register |

**bcm2835-rng registers (offset from base):**

| Register | Offset | Purpose |
|---|---|---|
| RNG_CTRL | 0x00 | Control register (enable bit) |
| RNG_STATUS | 0x04 | Status (bits 24:0 = words available) |
| RNG_DATA | 0x08 | Random data output (32 bits) |

-----

## 5. MMIO Access

All HAL device drivers access hardware through memory-mapped I/O. The HAL provides safe MMIO primitives that enforce volatile semantics and correct memory ordering:

```rust
/// Read a 32-bit register at `base + offset`.
/// Uses volatile read + compiler fence.
#[inline(always)]
pub unsafe fn mmio_read32(base: *const u8, offset: usize) -> u32 {
    let addr = base.add(offset) as *const u32;
    core::ptr::read_volatile(addr)
}

/// Write a 32-bit register at `base + offset`.
/// Uses volatile write + compiler fence.
#[inline(always)]
pub unsafe fn mmio_write32(base: *mut u8, offset: usize, value: u32) {
    let addr = base.add(offset) as *mut u32;
    core::ptr::write_volatile(addr, value);
}

/// Read-modify-write: set specific bits in a register.
#[inline(always)]
pub unsafe fn mmio_set_bits32(base: *mut u8, offset: usize, bits: u32) {
    let val = mmio_read32(base, offset);
    mmio_write32(base, offset, val | bits);
}

/// Read-modify-write: clear specific bits in a register.
#[inline(always)]
pub unsafe fn mmio_clear_bits32(base: *mut u8, offset: usize, bits: u32) {
    let val = mmio_read32(base, offset);
    mmio_write32(base, offset, val & !bits);
}
```

MMIO regions are mapped with device memory attributes (nGnRnE — non-Gathering, non-Reordering, non-Early-write-acknowledgement) in the kernel page tables. This prevents the CPU from reordering or caching device register accesses. The mapping is set up in boot.md §3.3 Step 7 at virtual address `0xFFFF_0002_0000_0000`.

-----

## 6. VirtIO Transport

QEMU devices use the VirtIO specification. The HAL includes a shared VirtIO transport layer used by VirtIO-GPU, VirtIO-Net, and VirtIO-Blk:

```rust
/// A VirtIO virtqueue (shared ring buffer between driver and device).
pub struct VirtioQueue {
    descriptors: *mut VirtqDesc,
    avail: *mut VirtqAvail,
    used: *mut VirtqUsed,
    queue_size: u16,
    free_head: u16,
    last_used_idx: u16,
}

#[repr(C)]
struct VirtqDesc {
    addr: u64,       // physical address of buffer
    len: u32,        // buffer length
    flags: u16,      // NEXT, WRITE, INDIRECT
    next: u16,       // index of next descriptor in chain
}

impl VirtioQueue {
    /// Add a buffer chain to the available ring.
    pub fn submit(&mut self, buffers: &[VirtioBuffer]) -> Result<u16>;

    /// Check for completed buffers in the used ring.
    pub fn poll_used(&mut self) -> Option<(u16, u32)>;

    /// Notify the device that new buffers are available.
    pub fn notify(&self, transport: &VirtioTransport);
}

/// VirtIO transport (MMIO-based for QEMU virt machine).
pub struct VirtioTransport {
    base: *mut u8,
}

impl VirtioTransport {
    /// Probe for a VirtIO device at the given MMIO address.
    /// Reads the magic value, version, and device ID.
    pub fn probe(base: *mut u8) -> Option<VirtioDeviceInfo>;

    /// Negotiate features with the device.
    pub fn negotiate_features(&self, driver_features: u64) -> u64;

    /// Set up a virtqueue.
    pub fn setup_queue(&self, queue_index: u16, queue: &VirtioQueue);

    /// Mark the driver as ready (DRIVER_OK status bit).
    pub fn activate(&self);
}
```

VirtIO devices are discovered from the device tree. Each VirtIO MMIO device has a node like:

```
virtio_mmio@a000000 {
    compatible = "virtio,mmio";
    reg = <0x0 0xa000000 0x0 0x200>;
    interrupts = <GIC_SPI 16 IRQ_TYPE_EDGE_RISING>;
};
```

The HAL enumerates all `virtio,mmio` nodes, probes each one, and matches the device ID to the appropriate driver (GPU = 16, Net = 1, Blk = 2).

-----

## 7. Adding a New Platform

To add support for a new aarch64 board, implement the seven `Platform` trait methods:

### 7.1 Steps

1. **Add a platform struct** in `kernel/hal/platforms/`:

```rust
pub struct NewBoardPlatform;
```

2. **Add the DTB compatible string** to `detect_platform()`:

```rust
c if c.contains("vendor,board-soc") => Box::new(NewBoardPlatform),
```

3. **Implement the seven trait methods.** Each method reads the device tree to find the relevant hardware node and its MMIO base address, then initializes the device:

```rust
impl Platform for NewBoardPlatform {
    fn init_interrupts(&self, dt: &DeviceTree) -> Result<InterruptController> {
        // Read the interrupt-controller node from dt
        // Determine GICv2 vs GICv3 from compatible string
        // Initialize distributor + per-CPU interface
    }

    fn init_timer(&self, dt: &DeviceTree) -> Result<Timer> {
        // Read CNTFRQ_EL0 for frequency
        // Read timer IRQ number from dt
        // Configure 1ms tick
    }

    fn init_uart(&self, dt: &DeviceTree) -> Result<Uart> {
        // Read UART node from dt (or /chosen/stdout-path)
        // PL011 is common; other UART types need new driver code
    }

    fn init_gpu(&self, dt: &DeviceTree) -> Result<GpuDevice> {
        // Platform-specific GPU driver
        // Must implement allocate_framebuffer + present + create_render_context
    }

    fn init_network(&self, dt: &DeviceTree) -> Result<NetworkDevice> {
        // Platform-specific NIC driver
        // Must implement transmit + receive
    }

    fn init_storage(&self, dt: &DeviceTree) -> Result<StorageDevice> {
        // Platform-specific storage driver
        // Must implement read_blocks + write_blocks + flush
    }

    fn init_rng(&self, dt: &DeviceTree) -> Result<RngDevice> {
        // Platform-specific hardware RNG
        // Must implement fill_bytes
    }
}
```

4. **Test on QEMU** (if the board can be emulated) or on real hardware via UART serial console.

### 7.2 What Stays the Same Across Platforms

The following kernel components are platform-independent and do not change when adding a new board:

- Page table format (4-level, 4 KiB granule, 48-bit VA)
- Exception vector table layout
- Syscall interface (SVC #0)
- Capability system
- IPC message format
- Scheduler algorithm
- Memory allocators (buddy + slab)
- All userspace services

### 7.3 What Changes Per Platform

| Component | What varies |
|---|---|
| Interrupt controller | GICv2 vs GICv3 (register layout, acknowledge/EOI path) |
| Timer | Frequency only (CNTFRQ_EL0 value) |
| UART | Base address only (all platforms use PL011 currently) |
| GPU | Entire driver (VirtIO vs VC4 vs V3D vs other) |
| Network | Entire driver (VirtIO vs Genet vs other) |
| Storage | Entire driver (VirtIO vs SDHCI vs NVMe vs other) |
| RNG | Driver + register layout (VirtIO vs bcm2835 vs other) |

The interrupt controller, timer, and RNG are the simplest to port — only register addresses and minor protocol differences. GPU, network, and storage require full device drivers for each new hardware type.

-----

## 8. Kernel Integration

### 8.1 KernelState

The HAL-initialized devices are stored in the global `KernelState` structure (canonical definition in boot.md §3.2; reproduced here for HAL context):

```rust
pub struct KernelState {
    pub boot_info: &'static BootInfo,
    pub platform: &'static dyn Platform,
    pub boot_phase: EarlyBootPhase,

    // HAL devices (see hal.md for type definitions)
    pub interrupt_controller: Option<InterruptController>,
    pub timer: Option<Timer>,
    pub uart: Option<Uart>,
    pub rng: Option<RngDevice>,
    pub gpu: Option<GpuDevice>,
    pub network: Option<NetworkDevice>,
    pub storage: Option<StorageDevice>,

    // Memory
    pub page_allocator: Option<BuddyAllocator>,
    pub kernel_page_table: Option<PageTable>,
    pub heap: Option<SlabAllocator>,
    pub kaslr_offset: usize,

    // Core subsystems
    pub capability_manager: Option<CapabilityManager>,
    pub ipc: Option<IpcSubsystem>,
    pub audit_log: Option<AuditRingBuffer>,
    pub process_manager: Option<ProcessManager>,
    pub provenance: Option<ProvenanceChain>,
    pub scheduler: Option<Scheduler>,

    // Boot timing
    pub boot_start: u64,
    pub phase_timestamps: [u64; 17], // indexed by EarlyBootPhase as usize (0-based); resize if enum grows
}
```

The `Option` wrappers reflect the incremental initialization during boot — UART is `Some` after Step 3, interrupts after Step 5, timer after Step 6, RNG after Step 10, and so on. Accessing a device before its initialization step would panic.

### 8.2 IRQ Flow

The full interrupt path from hardware to handler:

```
Hardware IRQ fires
  │
  ▼
ARM exception vector (IRQ from current EL or lower EL)
  │
  ▼
irq_handler():
  1. controller.acknowledge()     → read IAR (GICv2) or ICC_IAR1_EL1 (GICv3)
  2. match irq_number:
       TIMER_IRQ → scheduler.timer_tick()
       UART_IRQ  → uart.handle_rx()
       VIRTIO_*  → virtio_irq_handler(device_id)
       other     → spurious, log and ignore
  3. controller.end_of_interrupt() → write EOIR
```

### 8.3 Timer-Scheduler Integration

The timer drives the scheduler's preemption mechanism:

```
Timer fires every 1ms
  │
  ▼
timer_tick():
  1. timer.set_next_deadline(tick_interval)   // re-arm for next 1ms
  2. scheduler.tick()                          // update time accounting
     - Decrement current thread's remaining quantum
     - Check EDF deadlines (RT class)
     - If preemption needed: set PREEMPT_PENDING flag
  3. On return from IRQ handler:
     - If PREEMPT_PENDING: context switch to highest-priority thread
```

-----

## 9. DMA

Devices that perform DMA (VirtIO, Genet, SDHCI) need physical addresses for their buffer descriptors. The HAL provides DMA buffer allocation:

```rust
/// Allocate a physically contiguous, cache-coherent DMA buffer.
pub fn dma_alloc(size: usize, alignment: usize) -> Result<DmaBuffer> {
    let phys = page_allocator.alloc_contiguous(size, alignment)?;
    let virt = kernel_map_dma(phys, size)?;
    Ok(DmaBuffer { virt, phys, size })
}

pub struct DmaBuffer {
    pub virt: *mut u8,     // kernel virtual address (for CPU access)
    pub phys: u64,         // physical address (for device DMA)
    pub size: usize,
}

impl Drop for DmaBuffer {
    fn drop(&mut self) {
        kernel_unmap_dma(self.virt, self.size);
        page_allocator.free_contiguous(self.phys, self.size);
    }
}
```

DMA buffers are mapped with non-cacheable attributes to ensure coherency between CPU writes and device reads (and vice versa). On aarch64, this is achieved with the `Normal Non-Cacheable` memory type in the page table entry.

-----

## 10. Platform Comparison Reference

Complete hardware matrix for all supported platforms:

```
                        QEMU virt           Raspberry Pi 4      Raspberry Pi 5
─────────────────────────────────────────────────────────────────────────────────
SoC                     Virtual             BCM2711             BCM2712
CPU                     Cortex-A72 (emu)    Cortex-A72 (4x)    Cortex-A76 (4x)
RAM                     Configurable        1/2/4/8 GB          4/8 GB
──── HAL Devices (Platform trait, 7 methods) ───────────────────────────────────
Interrupt controller    GICv3 (virtual)     GIC-400 (GICv2)     GICv3
Timer frequency         62.5 MHz            54 MHz              54 MHz
UART                    PL011               PL011               PL011
GPU                     VirtIO-GPU          VideoCore VI        VideoCore VII
Network                 VirtIO-Net          Genet (1 Gbps)      Genet (1 Gbps)
Storage                 VirtIO-Blk          Arasan SDHCI        Arasan SDHCI
RNG                     VirtIO-RNG          bcm2835-rng         bcm2835-rng
──── Extension traits (see §12.6 for full matrix) ──────────────────────────────
USB                     XHCI (virtual)      XHCI (VL805)        XHCI (RP1)
Audio                   VirtIO-Sound        HDMI + I2S           HDMI + I2S
Camera                  None                CSI-2 (1 port)       CSI-2 (2 ports)
PCIe                    Virtual root        Gen 2 x1             Gen 3 x4
GPIO                    None                58 pins              28 pins (RP1)
DTB compatible          qemu,virt           brcm,bcm2711        brcm,bcm2712
```

-----

## 11. Design Principles

1. **Seven methods, one trait.** The Platform trait covers exactly the hardware every AIOS platform must provide: interrupts, timer, UART, GPU, network, storage, and RNG. If a board can't provide all seven, it can't run AIOS. Optional hardware (USB, WiFi, Bluetooth) uses extension traits (Section 12).
2. **Device tree as truth.** The HAL never hardcodes MMIO addresses. All addresses come from the device tree. This means the same binary can run on different revisions of the same board.
3. **No runtime polymorphism in hot paths.** The `GicVariant` enum uses match statements, not trait objects, in the IRQ handler. The compiler inlines the correct path. Interrupt latency is the same as a hand-written driver.
4. **Early boot is allocation-free.** UART, interrupt controller, timer, and RNG initialization use only stack and static memory. The heap doesn't exist yet when these run.
5. **Later devices can allocate.** GPU, network, and storage init happens after the heap is available (Phase 1/2). These drivers can use `Vec`, `Box`, and other heap types.
6. **Platform structs are zero-sized.** All state lives in the returned device handles. The platform struct is just a namespace for the seven init methods.
7. **Extension traits for optional hardware.** The core trait is stable. New optional hardware classes are added as extension traits — existing platforms don't break (Section 12).

-----

## 12. Extension Traits

The core `Platform` trait has seven methods — the mandatory hardware every AIOS platform must provide. But some platforms have additional hardware that others don't. Extension traits handle this without bloating the core trait or breaking existing implementations.

### 12.1 Why Not Add More Methods to Platform?

Adding a method to `Platform` breaks every existing implementation. If we added `init_usb()` to the core trait, every platform would need to implement it — even platforms without USB. The choices would be:

- Return an error (but then the method isn't really "mandatory")
- Provide a default implementation that returns an error (hides the fact that the platform doesn't support it)
- Break the compile for all existing platforms

None of these are good. Extension traits solve this cleanly.

### 12.2 The Pattern

Extension traits extend `Platform` with optional capabilities. The kernel checks at runtime whether the current platform supports each extension:

```rust
/// Optional: platforms with a USB host controller implement this.
pub trait PlatformUsb: Platform {
    fn init_usb(&self, dt: &DeviceTree) -> Result<UsbController>;
}

/// Optional: platforms with WiFi hardware implement this.
pub trait PlatformWifi: Platform {
    fn init_wifi(&self, dt: &DeviceTree) -> Result<WifiDevice>;
}

/// Optional: platforms with Bluetooth hardware implement this.
pub trait PlatformBluetooth: Platform {
    fn init_bluetooth(&self, dt: &DeviceTree) -> Result<BluetoothController>;
}
```

Platforms opt in by implementing the extension trait:

```rust
// QEMU has virtual XHCI, so it implements PlatformUsb
impl PlatformUsb for QemuPlatform {
    fn init_usb(&self, dt: &DeviceTree) -> Result<UsbController> {
        // Initialize virtual XHCI controller
    }
}

// Pi 4 has VL805 XHCI
impl PlatformUsb for RaspberryPi4Platform {
    fn init_usb(&self, dt: &DeviceTree) -> Result<UsbController> {
        // Initialize VL805 XHCI via PCIe
    }
}

// A hypothetical headless board with no USB wouldn't implement PlatformUsb at all.
```

### 12.3 Runtime Discovery

The kernel uses `Any`-based downcasting to check if the platform supports an extension:

```rust
use core::any::Any;

/// Check if the platform supports an extension trait and initialize if so.
fn try_init_usb(platform: &dyn Platform, dt: &DeviceTree) -> Option<UsbController> {
    // The platform object is stored as Box<dyn Platform>.
    // We downcast to the concrete type, then check if it implements PlatformUsb.
    let any = platform.as_any();

    // Try each known platform type
    if let Some(qemu) = any.downcast_ref::<QemuPlatform>() {
        return Some(qemu.init_usb(dt).ok()?);
    }
    if let Some(pi4) = any.downcast_ref::<RaspberryPi4Platform>() {
        return Some(pi4.init_usb(dt).ok()?);
    }
    if let Some(pi5) = any.downcast_ref::<RaspberryPi5Platform>() {
        return Some(pi5.init_usb(dt).ok()?);
    }

    None // Platform doesn't support USB
}
```

To support this, the core `Platform` trait includes an `as_any` method:

```rust
pub trait Platform: Send + Sync {
    // ... the 7 core methods ...

    /// Allows downcasting to concrete platform type for extension trait checks.
    fn as_any(&self) -> &dyn Any;
}

// Every platform implements as_any trivially:
impl Platform for QemuPlatform {
    fn as_any(&self) -> &dyn Any { self }
    // ... 7 init methods ...
}
```

### 12.4 Core vs Extension Decision Rule

A hardware class belongs in the **core trait** if:
- Every realistic AIOS platform has it (interrupts, timer, UART, GPU, network, storage, RNG)
- The kernel cannot boot or function without it
- Absence means "this board cannot run AIOS"

A hardware class belongs in an **extension trait** if:
- Some platforms have it and others don't (USB, WiFi, Bluetooth, camera)
- The kernel can boot and function without it
- Absence means "this feature is unavailable," not "the OS is broken"

### 12.5 Extension Trait Catalog

Each extension trait follows the same pattern as §12.2. This catalog covers all optional hardware classes AIOS may support, organized by implementation priority.

#### Tier 1 — Planned (all current platforms have the hardware)

**`PlatformUsb`** — USB host controller.

```rust
pub trait PlatformUsb: Platform {
    fn init_usb(&self, dt: &DeviceTree) -> Result<UsbController>;
}
```

| Platform | Hardware | Notes |
|---|---|---|
| QEMU | XHCI (virtual) | Emulated xHCI controller |
| Pi 4 | VL805 XHCI | Via PCIe bridge on BCM2711 |
| Pi 5 | RP1 XHCI | Integrated in RP1 south bridge |

USB is a meta-subsystem — plugging in a device can surface new hardware for any subsystem (a webcam → Camera, a headset → Audio, a flash drive → Storage). The USB subsystem discovers devices, matches class drivers, and routes them to the appropriate subsystem via the Subsystem Framework (see subsystem-framework.md §USB).

**`PlatformAudio`** — Audio input/output.

```rust
pub trait PlatformAudio: Platform {
    fn init_audio(&self, dt: &DeviceTree) -> Result<AudioDevice>;
}

pub struct AudioDevice {
    variant: AudioVariant,
    sample_rate: u32,
    channels: u8,
}

enum AudioVariant {
    HdmiAudio { base: *mut u8 },
    I2s { base: *mut u8 },
    VirtioSound { virtqueue: VirtioQueue },
}

impl AudioDevice {
    /// Write PCM samples to the output buffer.
    pub fn write_samples(&self, buffer: &[i16]) -> Result<usize>;

    /// Read PCM samples from the input buffer (microphone).
    pub fn read_samples(&self, buffer: &mut [i16]) -> Result<usize>;

    /// Set the sample rate (e.g. 44100, 48000).
    pub fn set_sample_rate(&self, rate: u32) -> Result<()>;

    /// Set the number of channels (1 = mono, 2 = stereo).
    pub fn set_channels(&self, channels: u8) -> Result<()>;
}
```

| Platform | Hardware | Notes |
|---|---|---|
| QEMU | VirtIO-Sound | Virtual audio device |
| Pi 4 | HDMI audio + PWM + I2S | HDMI audio via VC4; I2S for external DACs |
| Pi 5 | HDMI audio + I2S | HDMI audio via VC7; I2S for external DACs |

Audio is critical for the assistant experience — speech-to-text, text-to-speech, alert sounds, and voice interaction all depend on it. The Audio subsystem provides mixing (multiple agents playing audio simultaneously) and routes through the Subsystem Framework. The scheduler reserves RT-class deadlines for audio (5ms period, 0.5ms WCET — see scheduler.md §6.3).

**`PlatformCamera`** — Camera / image sensor.

```rust
pub trait PlatformCamera: Platform {
    fn init_camera(&self, dt: &DeviceTree) -> Result<CameraDevice>;
}

pub struct CameraDevice {
    variant: CameraVariant,
    max_width: u32,
    max_height: u32,
}

enum CameraVariant {
    Csi { base: *mut u8 },
    UsbUvc { usb_device: UsbDeviceHandle },
}

impl CameraDevice {
    /// Start capturing frames at the given resolution and format.
    pub fn start_capture(
        &self,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<()>;

    /// Stop capturing.
    pub fn stop_capture(&self) -> Result<()>;

    /// Dequeue the next captured frame. Returns None if no frame is ready.
    pub fn next_frame(&self, buffer: &mut [u8]) -> Result<Option<FrameInfo>>;
}
```

| Platform | Hardware | Notes |
|---|---|---|
| QEMU | None | No camera emulation by default |
| Pi 4 | CSI-2 (Unicam) | Supports Pi Camera Module v2/v3 |
| Pi 5 | CSI-2 (Unicam, 2 ports) | Dual camera support |

Camera data flows through the Flow framework (see flow.md) for streaming to vision agents. The browser's `getUserMedia()` API maps to `CameraCapability` (prompted — see browser.md §6).

#### Tier 2 — Future (some current platforms, or expected on future boards)

**`PlatformPcie`** — PCIe host controller.

```rust
pub trait PlatformPcie: Platform {
    fn init_pcie(&self, dt: &DeviceTree) -> Result<PcieController>;
}

impl PcieController {
    /// Enumerate devices on the PCIe bus.
    pub fn enumerate(&self) -> Result<Vec<PcieDevice>>;

    /// Read from a device's configuration space.
    pub fn config_read32(&self, bdf: BusDeviceFunction, offset: u16) -> u32;

    /// Write to a device's configuration space.
    pub fn config_write32(&self, bdf: BusDeviceFunction, offset: u16, value: u32);

    /// Map a device's BAR into kernel virtual address space.
    pub fn map_bar(&self, bdf: BusDeviceFunction, bar: u8) -> Result<MappedBar>;
}
```

| Platform | Hardware | Notes |
|---|---|---|
| QEMU | Virtual PCIe root complex | Configurable |
| Pi 4 | BCM2711 PCIe Gen 2 x1 | Used by VL805 USB controller |
| Pi 5 | BCM2712 PCIe Gen 3 x4 | Exposed via RP1; external slot via FPC |

PCIe is the foundation for NVMe storage, external GPUs, and high-speed networking on future boards. Pi 5's exposed PCIe slot makes this increasingly important. The `PlatformUsb` trait on Pi 4 currently initializes the VL805 through PCIe internally; with `PlatformPcie`, this becomes a proper enumerated bus.

**`PlatformNvme`** — NVMe storage (via PCIe).

```rust
pub trait PlatformNvme: Platform {
    fn init_nvme(&self, dt: &DeviceTree) -> Result<NvmeDevice>;
}

impl NvmeDevice {
    /// Submit a read command to an I/O submission queue.
    pub fn read_blocks(
        &self,
        namespace: u32,
        lba: u64,
        count: u32,
        buffer: &mut [u8],
    ) -> Result<()>;

    /// Submit a write command.
    pub fn write_blocks(
        &self,
        namespace: u32,
        lba: u64,
        count: u32,
        buffer: &[u8],
    ) -> Result<()>;

    /// Flush volatile write cache.
    pub fn flush(&self) -> Result<()>;

    /// Return the namespace capacity in bytes.
    pub fn capacity_bytes(&self, namespace: u32) -> u64;
}
```

| Platform | Hardware | Notes |
|---|---|---|
| QEMU | VirtIO-Blk or emulated NVMe | Via `-drive if=none -device nvme` |
| Pi 4 | None | PCIe used by USB controller |
| Pi 5 | Via PCIe Gen 3 x4 | With M.2 HAT or adapter |

NVMe transforms model loading performance: 4.5 GB Q4_K_M in ~2 seconds vs ~45 seconds from SD card (see airs.md §5). The Block Engine can tier storage across NVMe and SD — hot data on NVMe, cold data on SD (see spaces.md §13).

**`PlatformWatchdog`** — Hardware watchdog timer.

```rust
pub trait PlatformWatchdog: Platform {
    fn init_watchdog(&self, dt: &DeviceTree) -> Result<WatchdogTimer>;
}

impl WatchdogTimer {
    /// Start the watchdog with the given timeout.
    pub fn start(&self, timeout: Duration) -> Result<()>;

    /// Pet/kick the watchdog to prevent reset.
    pub fn pet(&self) -> Result<()>;

    /// Stop the watchdog (if hardware supports it).
    pub fn stop(&self) -> Result<()>;

    /// Return the remaining time before reset.
    pub fn time_remaining(&self) -> Duration;
}
```

| Platform | Hardware | Notes |
|---|---|---|
| QEMU | Virtual watchdog | `-device i6300esb` or similar |
| Pi 4 | BCM2835 watchdog | Shared PM/watchdog block |
| Pi 5 | BCM2835 watchdog | Same IP block |

The kernel sets a 15-second watchdog at shutdown start (see [boot-lifecycle.md](./boot-lifecycle.md) §11). A watchdog provides last-resort recovery from kernel hangs in unattended deployments.

**`PlatformGpio`** — General-purpose I/O pins.

```rust
pub trait PlatformGpio: Platform {
    fn init_gpio(&self, dt: &DeviceTree) -> Result<GpioController>;
}

impl GpioController {
    /// Configure a pin as input or output.
    pub fn set_mode(&self, pin: u32, mode: GpioMode) -> Result<()>;

    /// Set an output pin high or low.
    pub fn write(&self, pin: u32, level: bool) -> Result<()>;

    /// Read the current level of a pin.
    pub fn read(&self, pin: u32) -> Result<bool>;

    /// Register an interrupt handler for a pin edge/level.
    pub fn set_interrupt(
        &self,
        pin: u32,
        trigger: GpioTrigger,
        handler: fn(u32),
    ) -> Result<()>;
}

pub enum GpioMode { Input, Output, AltFunc(u8) }
pub enum GpioTrigger { RisingEdge, FallingEdge, BothEdges, HighLevel, LowLevel }
```

| Platform | Hardware | Notes |
|---|---|---|
| QEMU | None | No GPIO emulation |
| Pi 4 | BCM2711 GPIO (58 pins) | Alt functions for I2C, SPI, UART, PWM |
| Pi 5 | RP1 GPIO (28 pins) | Via RP1 south bridge |

GPIO is the gateway to physical computing — sensors, LEDs, buttons, relays. It also multiplexes as I2C, SPI, and PWM (below). On AIOS, GPIO access requires a capability token scoped to specific pins.

#### Tier 3 — Speculative (future platforms or niche use cases)

**`PlatformI2c`** — I2C bus controller.

```rust
pub trait PlatformI2c: Platform {
    fn init_i2c(&self, dt: &DeviceTree, bus: u8) -> Result<I2cBus>;
}

impl I2cBus {
    /// Write bytes to an I2C device at the given address.
    pub fn write(&self, addr: u8, data: &[u8]) -> Result<()>;

    /// Read bytes from an I2C device.
    pub fn read(&self, addr: u8, buffer: &mut [u8]) -> Result<()>;

    /// Write then read (combined transaction).
    pub fn write_read(
        &self,
        addr: u8,
        write_data: &[u8],
        read_buffer: &mut [u8],
    ) -> Result<()>;
}
```

| Platform | Hardware | Notes |
|---|---|---|
| QEMU | None | No I2C emulation |
| Pi 4 | BCM2711 BSC (6 buses) | Sensors, HATs, displays |
| Pi 5 | RP1 I2C (6 buses) | Via RP1 south bridge |

**`PlatformSpi`** — SPI bus controller.

```rust
pub trait PlatformSpi: Platform {
    fn init_spi(&self, dt: &DeviceTree, bus: u8) -> Result<SpiBus>;
}

impl SpiBus {
    /// Transfer: simultaneous write and read.
    pub fn transfer(
        &self,
        write_data: &[u8],
        read_buffer: &mut [u8],
    ) -> Result<()>;

    /// Set the clock speed.
    pub fn set_clock_hz(&self, hz: u32) -> Result<()>;

    /// Set SPI mode (CPOL/CPHA).
    pub fn set_mode(&self, mode: SpiMode) -> Result<()>;
}
```

| Platform | Hardware | Notes |
|---|---|---|
| QEMU | None | No SPI emulation |
| Pi 4 | BCM2711 SPI (multiple) | External flash, ADCs, displays |
| Pi 5 | RP1 SPI (multiple) | Via RP1 south bridge |

**`PlatformPwm`** — PWM output channels.

```rust
pub trait PlatformPwm: Platform {
    fn init_pwm(&self, dt: &DeviceTree) -> Result<PwmController>;
}

impl PwmController {
    /// Set the PWM period and duty cycle for a channel.
    pub fn configure(
        &self,
        channel: u8,
        period_ns: u64,
        duty_ns: u64,
    ) -> Result<()>;

    /// Enable a PWM channel.
    pub fn enable(&self, channel: u8) -> Result<()>;

    /// Disable a PWM channel.
    pub fn disable(&self, channel: u8) -> Result<()>;
}
```

| Platform | Hardware | Notes |
|---|---|---|
| QEMU | None | No PWM emulation |
| Pi 4 | BCM2711 PWM (2 channels) | Audio out, LED brightness, servos |
| Pi 5 | RP1 PWM (4 channels) | Via RP1 south bridge |

**`PlatformCryptoAccel`** — Hardware cryptographic accelerator.

```rust
pub trait PlatformCryptoAccel: Platform {
    fn init_crypto_accel(&self, dt: &DeviceTree) -> Result<CryptoAccelerator>;
}

impl CryptoAccelerator {
    /// AES-256-GCM encrypt in hardware.
    pub fn aes_gcm_encrypt(
        &self,
        key: &[u8; 32],
        nonce: &[u8; 12],
        plaintext: &[u8],
        aad: &[u8],
        ciphertext: &mut [u8],
        tag: &mut [u8; 16],
    ) -> Result<()>;

    /// AES-256-GCM decrypt in hardware.
    pub fn aes_gcm_decrypt(
        &self,
        key: &[u8; 32],
        nonce: &[u8; 12],
        ciphertext: &[u8],
        aad: &[u8],
        plaintext: &mut [u8],
        tag: &[u8; 16],
    ) -> Result<bool>; // false if tag mismatch

    /// SHA-256 hash in hardware.
    pub fn sha256(&self, data: &[u8], hash: &mut [u8; 32]) -> Result<()>;

    /// Query which algorithms the hardware accelerates.
    pub fn capabilities(&self) -> CryptoCapabilities;
}
```

| Platform | Hardware | Notes |
|---|---|---|
| QEMU | None (or VirtIO-Crypto) | Optional with `-device virtio-crypto` |
| Pi 4 | ARMv8 CE (AESCE, SHA) | CPU instruction extensions, not a separate device |
| Pi 5 | ARMv8 CE (AESCE, SHA) | Cortex-A76 crypto extensions |

Note: ARMv8 Cryptography Extensions (CE) are CPU instructions, not a separate MMIO device. They don't need a HAL extension trait — the Cryptographic Core (security.md §18) uses them directly via inline assembly. This extension trait is for future platforms with dedicated crypto co-processors (separate DMA-capable engines like CryptoCell or CAAM).

**`PlatformNpu`** — Neural Processing Unit / ML accelerator.

```rust
pub trait PlatformNpu: Platform {
    fn init_npu(&self, dt: &DeviceTree) -> Result<NpuDevice>;
}

impl NpuDevice {
    /// Load a compiled model graph onto the NPU.
    pub fn load_graph(&self, graph: &[u8]) -> Result<GraphHandle>;

    /// Submit an inference job. Returns a handle to poll for completion.
    pub fn submit_inference(
        &self,
        graph: GraphHandle,
        inputs: &[TensorBuffer],
    ) -> Result<InferenceHandle>;

    /// Poll for inference completion. Returns output tensors when done.
    pub fn poll_inference(
        &self,
        handle: InferenceHandle,
    ) -> Result<Option<Vec<TensorBuffer>>>;

    /// Query NPU compute capacity (TOPS).
    pub fn compute_tops(&self) -> f32;
}
```

| Platform | Hardware | Notes |
|---|---|---|
| QEMU | None | No NPU emulation |
| Pi 4 | None | GPU compute only |
| Pi 5 | None | GPU compute only |
| Future | 10–40 TOPS NPU | Expected on next-gen SoCs |

No current AIOS platform has a dedicated NPU, but the industry trend is clear — future aarch64 SoCs will include ML accelerators (see architecture.md §Future). AIRS currently runs inference on CPU (see airs.md), but an NPU extension trait would allow hardware-accelerated inference with the same AIRS API.

### 12.6 Platform Comparison (Extension Traits)

```
                        QEMU virt           Raspberry Pi 4      Raspberry Pi 5
──── Tier 1 ────────────────────────────────────────────────────────────────────
USB                     XHCI (virtual)      XHCI (VL805)        XHCI (RP1)
Audio                   VirtIO-Sound        HDMI + I2S           HDMI + I2S
Camera                  None                CSI-2 (1 port)       CSI-2 (2 ports)
──── Tier 2 ────────────────────────────────────────────────────────────────────
PCIe                    Virtual root        Gen 2 x1             Gen 3 x4
NVMe                    Emulated            None                 Via PCIe
Watchdog                Virtual             BCM2835              BCM2835
GPIO                    None                58 pins              28 pins (RP1)
──── Tier 3 ────────────────────────────────────────────────────────────────────
I2C                     None                6 buses              6 buses (RP1)
SPI                     None                Multiple             Multiple (RP1)
PWM                     None                2 channels           4 channels (RP1)
Crypto accelerator      None                ARMv8 CE (CPU)       ARMv8 CE (CPU)
NPU                     None                None                 None
```

### 12.7 WiFi and Bluetooth

```rust
pub trait PlatformWifi: Platform {
    fn init_wifi(&self, dt: &DeviceTree) -> Result<WifiDevice>;
}

pub trait PlatformBluetooth: Platform {
    fn init_bluetooth(&self, dt: &DeviceTree) -> Result<BluetoothController>;
}
```

| Extension Trait | Current Platforms | Notes |
|---|---|---|
| `PlatformWifi` | None | External dongles via USB on all current platforms |
| `PlatformBluetooth` | None | External dongles via USB on all current platforms |

WiFi and Bluetooth are currently external USB devices on all supported platforms, so they're discovered through the USB subsystem → Subsystem Framework path rather than through a platform extension trait. The extension traits exist for future platforms with built-in WiFi/BT hardware (e.g., boards with on-SoC wireless like the ESP32 or future Broadcom SoCs with integrated WLAN).
