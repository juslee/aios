# AIOS Hardware Abstraction Layer (HAL)

## Deep Technical Architecture

**Parent document:** [architecture.md](../project/architecture.md) — Section 2.1 Hardware Abstraction
**Related:** [boot.md](./boot.md) — Boot sequence and platform detection, [subsystem-framework.md](../platform/subsystem-framework.md) — Userspace device management, [scheduler.md](./scheduler.md) — Timer and GIC integration

-----

## 1. Overview

The HAL is the lowest layer of the AIOS kernel. It sits directly on hardware and exposes a uniform interface that the rest of the kernel programs against. The kernel never touches raw MMIO registers or device-specific data structures outside the HAL — all hardware access flows through trait implementations.

The HAL has one design goal: **adding a new platform is implementing six traits.** A platform is a specific hardware board — QEMU virt, Raspberry Pi 4 (BCM2711), Raspberry Pi 5 (BCM2712), or any future aarch64 board. Each platform provides different hardware for the same logical functions (interrupts, timer, serial, GPU, network, storage). The HAL abstracts these differences behind a single `Platform` trait with six initialization methods.

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

The `Platform` trait is the core abstraction. Every supported platform implements exactly six methods — one for each hardware class the kernel needs during boot:

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

The six methods are not called all at once. They're called at specific points during boot as their dependencies become available:

```
Early Boot (kernel space, no heap):
  Step 3:  init_uart()        — first sign of life
  Step 5:  init_interrupts()  — enables IRQ routing
  Step 6:  init_timer()       — enables preemptive scheduling

Service Manager Phases (userspace, heap available):
  Phase 1: init_storage()     — Block Engine needs raw block access
  Phase 2: init_gpu()         — Display Subsystem needs GPU handle
  Phase 2: init_network()     — Network Subsystem needs NIC handle
```

The early boot methods (UART, interrupts, timer) run before the heap exists and must use only static or stack allocation. The later methods (storage, GPU, network) run after the heap is available and can allocate freely.

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

To add support for a new aarch64 board, implement the six `Platform` trait methods:

### 7.1 Steps

1. **Add a platform struct** in `kernel/hal/platforms/`:

```rust
pub struct NewBoardPlatform;
```

2. **Add the DTB compatible string** to `detect_platform()`:

```rust
c if c.contains("vendor,board-soc") => Box::new(NewBoardPlatform),
```

3. **Implement the six trait methods.** Each method reads the device tree to find the relevant hardware node and its MMIO base address, then initializes the device:

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

The interrupt controller and timer are the simplest to port — only register addresses and minor protocol differences. GPU, network, and storage require full device drivers for each new hardware type.

-----

## 8. Kernel Integration

### 8.1 KernelState

The HAL-initialized devices are stored in the global `KernelState` structure:

```rust
pub struct KernelState {
    pub boot_info: &'static BootInfo,
    pub platform: &'static dyn Platform,
    pub boot_phase: EarlyBootPhase,

    // HAL devices (initialized during boot)
    pub interrupt_controller: Option<InterruptController>,
    pub timer: Option<Timer>,
    pub uart: Option<Uart>,
    pub gpu: Option<GpuDevice>,
    pub network: Option<NetworkDevice>,
    pub storage: Option<StorageDevice>,

    // Memory management
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
    pub phase_timestamps: [u64; 16],
}
```

The `Option` wrappers reflect the incremental initialization during boot — UART is `Some` after Step 3, interrupts after Step 5, timer after Step 6, and so on. Accessing a device before its initialization step would panic.

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
──── HAL Devices ───────────────────────────────────────────────────────────────
Interrupt controller    GICv3 (virtual)     GIC-400 (GICv2)     GICv3
Timer frequency         62.5 MHz            54 MHz              54 MHz
UART                    PL011               PL011               PL011
GPU                     VirtIO-GPU          VideoCore VI        VideoCore VII
Network                 VirtIO-Net          Genet (1 Gbps)      Genet (1 Gbps)
Storage                 VirtIO-Blk          Arasan SDHCI        Arasan SDHCI
──── Additional ────────────────────────────────────────────────────────────────
RNG                     VirtIO-RNG          bcm2835-rng         bcm2835-rng
USB                     XHCI (virtual)      XHCI (VL805)        XHCI (RP1)
WiFi                    None                None (external)      None (external)
Bluetooth               None                None (external)      None (external)
DTB compatible          qemu,virt           brcm,bcm2711        brcm,bcm2712
```

-----

## 11. Design Principles

1. **Six methods, one trait.** The Platform trait is intentionally narrow. Six init methods cover everything the kernel needs. Userspace devices (USB peripherals, Bluetooth, WiFi) are handled by the Subsystem Framework, not the HAL.
2. **Device tree as truth.** The HAL never hardcodes MMIO addresses. All addresses come from the device tree. This means the same binary can run on different revisions of the same board.
3. **No runtime polymorphism in hot paths.** The `GicVariant` enum uses match statements, not trait objects, in the IRQ handler. The compiler inlines the correct path. Interrupt latency is the same as a hand-written driver.
4. **Early boot is allocation-free.** UART, interrupt controller, and timer initialization use only stack and static memory. The heap doesn't exist yet when these run.
5. **Later devices can allocate.** GPU, network, and storage init happens after the heap is available (Phase 1/2). These drivers can use `Vec`, `Box`, and other heap types.
6. **Platform structs are zero-sized.** All state lives in the returned device handles. The platform struct is just a namespace for the six init methods.
