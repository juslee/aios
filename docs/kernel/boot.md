# AIOS Boot and Init Sequence

## Deep Technical Architecture

**Parent document:** [architecture.md](../project/architecture.md) — Section 6.1 Boot Sequence
**Related:** [ipc.md](./ipc.md) — IPC and syscalls, [spaces.md](../storage/spaces.md) — Space Storage, [airs.md](../intelligence/airs.md) — AI Runtime Service, [development-plan.md](../project/development-plan.md) — Phase plan

-----

## 1. Overview

The parent architecture document describes the boot sequence at a high level: five service manager phases layered on top of firmware handoff and kernel early boot. This document goes deeper — the actual data structures, initialization order, timing constraints, hardware differences, recovery paths, and the mechanisms that make a sub-3-second boot possible.

The boot sequence has one invariant that governs every design decision: **the system is usable at each phase boundary.** If any phase after Phase 2 (core services) fails, the user still gets a functional — if degraded — desktop. AIRS failure doesn't block boot. Network failure doesn't block boot. The only hard dependencies on the critical path are: firmware, kernel, storage, display, and the compositor.

-----

## 2. Firmware Handoff

### 2.1 UEFI Boot on aarch64

AIOS boots via UEFI on aarch64. The firmware is not part of AIOS — it's provided by the platform (QEMU's built-in UEFI, or the Pi's firmware). AIOS controls everything from the moment the kernel receives execution.

**Boot flow:**

```
Platform firmware (not AIOS)
  │
  ├── POST: hardware self-test, DRAM training, PCI enumeration
  ├── UEFI firmware initialization
  ├── Read ESP (EFI System Partition) from boot device
  ├── Load \EFI\BOOT\BOOTAA64.EFI (AIOS UEFI stub)
  │
  ▼
AIOS UEFI stub (runs in UEFI Boot Services, EL1)
  │
  ├── Parse UEFI memory map
  ├── Locate and load kernel ELF from ESP
  ├── Acquire framebuffer via GOP (Graphics Output Protocol)
  ├── Acquire device tree or ACPI tables
  ├── Request RNG seed from UEFI for KASLR
  ├── Allocate contiguous region for kernel
  ├── ExitBootServices() — point of no return
  │
  ▼
Jump to kernel entry point (all UEFI Boot Services gone)
```

### 2.2 What the Kernel Receives

The UEFI stub assembles a `BootInfo` structure and passes it to the kernel entry point in a register (`x0`). This is the kernel's only source of information about the hardware:

```rust
/// Passed from UEFI stub to kernel entry point.
/// Lives in a region marked as BootInfo in the memory map.
#[repr(C)]
pub struct BootInfo {
    /// Magic number for validation: 0x41494F53_424F4F54 ("AIOSBOOT")
    magic: u64,

    /// UEFI memory map: array of MemoryDescriptor entries.
    /// The kernel uses this to know what physical memory exists,
    /// what's reserved, and what's free.
    memory_map: MemoryMap,

    /// Framebuffer for early visual output (before compositor exists).
    /// Acquired from UEFI GOP. May be None on headless systems.
    framebuffer: Option<FramebufferInfo>,

    /// Device tree blob (FDT). On QEMU and Pi, this describes
    /// all hardware: interrupt controllers, timers, UARTs, etc.
    device_tree: Option<DeviceTreeInfo>,

    /// ACPI RSDP (Root System Description Pointer).
    /// QEMU provides both DTB and ACPI. Pi provides DTB only.
    /// Kernel prefers DTB when both are present.
    acpi_rsdp: Option<PhysAddr>,

    /// UEFI Runtime Services function table.
    /// Provides: GetTime, SetTime, ResetSystem, GetVariable.
    /// Available after ExitBootServices (unlike Boot Services).
    runtime_services: Option<PhysAddr>,

    /// Random seed from UEFI RNG protocol. Used for KASLR.
    /// If unavailable, kernel falls back to timer-based entropy.
    rng_seed: [u8; 32],

    /// Physical address where the kernel ELF was loaded.
    kernel_phys_base: PhysAddr,

    /// Size of kernel image in memory (text + rodata + data + bss).
    kernel_size: usize,

    /// Physical address of the initramfs (cpio archive).
    initramfs_base: PhysAddr,
    initramfs_size: usize,

    /// Command line arguments (from UEFI boot variable or config).
    cmdline: CommandLine,
}

#[repr(C)]
pub struct MemoryMap {
    entries: *const MemoryDescriptor,
    entry_count: usize,
    entry_size: usize,          // UEFI descriptor size (may be > sizeof)
}

#[repr(C)]
pub struct MemoryDescriptor {
    memory_type: MemoryType,
    physical_start: PhysAddr,
    virtual_start: VirtAddr,    // unused, UEFI sets to 0
    page_count: u64,            // pages of 4 KiB each
    attributes: u64,            // cacheability, write-protection
}

pub enum MemoryType {
    Conventional,               // free, usable by kernel
    LoaderCode,                 // UEFI stub code, reclaimable
    LoaderData,                 // UEFI stub data, reclaimable
    BootServicesCode,           // reclaimable after ExitBootServices
    BootServicesData,           // reclaimable after ExitBootServices
    RuntimeServicesCode,        // reserved, UEFI Runtime uses this
    RuntimeServicesData,        // reserved, UEFI Runtime uses this
    Reserved,                   // firmware-reserved, do not touch
    AcpiReclaimable,            // ACPI tables, reclaimable after parsing
    AcpiNvs,                    // ACPI non-volatile storage, reserved
    MemoryMappedIO,             // device MMIO, not real RAM
    BootInfo,                   // the BootInfo struct itself
    KernelImage,                // where the kernel ELF was loaded
    Initramfs,                  // the initial ramdisk
}

#[repr(C)]
pub struct FramebufferInfo {
    base: PhysAddr,             // physical address of pixel buffer
    size: usize,                // total buffer size in bytes
    width: u32,                 // pixels
    height: u32,                // pixels
    stride: u32,                // bytes per row (may include padding)
    format: PixelFormat,        // Bgr8, Rgb8, or custom bitmask
}

pub enum PixelFormat {
    Bgr8,                       // most common: blue-green-red, 8 bits each
    Rgb8,
    Bitmask {
        red: PixelBitmask,
        green: PixelBitmask,
        blue: PixelBitmask,
    },
}

#[repr(C)]
pub struct DeviceTreeInfo {
    base: PhysAddr,
    size: usize,
}

#[repr(C)]
pub struct CommandLine {
    ptr: *const u8,
    len: usize,
}
```

### 2.3 EFI System Partition Layout

The ESP is a FAT32 partition at the start of the boot device:

```
/EFI/BOOT/
    BOOTAA64.EFI            — AIOS UEFI stub (fallback boot path)
/EFI/AIOS/
    BOOTAA64.EFI            — AIOS UEFI stub (primary boot path)
    aios.elf                — kernel ELF image
    initramfs.cpio          — initial ramdisk (cpio archive)
    boot.cfg                — boot configuration (command line, options)
    aios.elf.prev           — previous kernel (for rollback)
    initramfs.cpio.prev     — previous initramfs (for rollback)
```

The ESP is small (64-256 MB). It holds only the boot chain. The OS itself lives in the AIOS partition (raw block device managed by the Block Engine). The `.prev` files support A/B rollback: if a new kernel fails to boot three times, the UEFI stub loads `.prev` instead.

### 2.4 QEMU Boot vs Real Hardware

```
                        QEMU                    Raspberry Pi 4/5
─────────────────────────────────────────────────────────────────
Firmware                Built-in UEFI           VideoCore + UEFI
                        (edk2-aarch64)          (via edk2-rpi)
Boot device             VirtIO-Blk disk         SD card or USB
Device discovery        DTB (QEMU-generated)    DTB (Pi firmware)
ACPI                    Available               Not available
Interrupt controller    GICv3 (virtual)         GIC-400 (GICv2)
Timer                   ARM Generic Timer       ARM Generic Timer
UART                    PL011 (MMIO)            PL011 (MMIO)
GPU                     VirtIO-GPU              VideoCore VI/VII
Network                 VirtIO-Net              Genet Ethernet
Storage                 VirtIO-Blk              SD/eMMC + USB
RNG                     VirtIO-RNG              bcm2835-rng
Framebuffer             UEFI GOP (VirtIO-GPU)   UEFI GOP (HDMI)
Acceleration            HVF (macOS), KVM (Linux) Native aarch64
```

**Key difference for boot:** QEMU provides GICv3, Pi 4 provides GICv2 (GIC-400). The kernel's interrupt setup path branches based on the device tree. Pi 5 provides GICv3 natively.

The kernel abstracts these differences behind a `Platform` trait initialized during early boot:

```rust
pub trait Platform {
    fn init_interrupts(&self, dt: &DeviceTree) -> Result<InterruptController>;
    fn init_timer(&self, dt: &DeviceTree) -> Result<Timer>;
    fn init_uart(&self, dt: &DeviceTree) -> Result<Uart>;
    fn platform_name(&self) -> &'static str;
}

pub struct QemuPlatform;
pub struct RaspberryPi4Platform;
pub struct RaspberryPi5Platform;

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

-----

## 3. Kernel Early Boot

Early boot runs entirely in kernel space (EL1). No interrupts, no virtual memory (initially), no heap. Everything is statically allocated or uses the boot stack. Each step must complete before the next begins.

### 3.1 Phase Tracking

The kernel tracks its boot progress through an enum. This is written to the UART at each transition and recorded for crash diagnostics:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EarlyBootPhase {
    /// Entered kernel entry point. Stack pointer set. BSS zeroed.
    EntryPoint,
    /// Exception vectors installed at VBAR_EL1.
    ExceptionVectors,
    /// UART initialized. kprintln!() works from here.
    UartReady,
    /// Device tree parsed. Platform detected.
    DeviceTreeParsed,
    /// GICv3 (or GICv2) interrupt controller initialized.
    InterruptsReady,
    /// ARM Generic Timer configured. Timer interrupts enabled.
    TimerReady,
    /// MMU enabled. TTBR0 (user) and TTBR1 (kernel) page tables active.
    /// Identity mapping removed. Running on virtual addresses.
    MmuEnabled,
    /// Physical page allocator (buddy system) initialized.
    PageAllocatorReady,
    /// Kernel heap (slab allocator) initialized. alloc works.
    HeapReady,
    /// KASLR slide applied (if RNG seed was available).
    KaslrApplied,
    /// Capability manager initialized. Root capability exists.
    CapabilityManagerReady,
    /// IPC subsystem initialized. Channels and endpoints available.
    IpcReady,
    /// Audit log ring buffer initialized. Kernel events are logged.
    AuditLogReady,
    /// Process manager initialized. Can create and schedule processes.
    ProcessManagerReady,
    /// Provenance chain initialized. Can sign and record events.
    ProvenanceReady,
    /// Early boot complete. Ready to launch Service Manager.
    Complete,
}

/// Mutable global, accessed only from the boot CPU during single-threaded init.
static mut BOOT_PHASE: EarlyBootPhase = EarlyBootPhase::EntryPoint;

fn advance_boot_phase(phase: EarlyBootPhase) {
    unsafe { BOOT_PHASE = phase; }
    // If UART is ready, print the transition
    if phase as u32 >= EarlyBootPhase::UartReady as u32 {
        kprintln!("[boot] {:?} — {}ms", phase, boot_elapsed_ms());
    }
}
```

### 3.2 Kernel State

The kernel maintains a global state structure that tracks all initialized subsystems. This is built up during early boot and consulted by all kernel code:

```rust
pub struct KernelState {
    pub boot_info: &'static BootInfo,
    pub platform: &'static dyn Platform,
    pub boot_phase: EarlyBootPhase,

    // Hardware
    pub interrupt_controller: Option<InterruptController>,
    pub timer: Option<Timer>,
    pub uart: Option<Uart>,

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
    pub boot_start: u64,           // timer counter value at entry
    pub phase_timestamps: [u64; 16], // counter value at each phase transition
}
```

### 3.3 Step-by-Step Early Boot

Each step below includes what it initializes and why it must happen at that point.

**Step 1: Entry point.** The UEFI stub jumps here. The kernel is running on a temporary stack allocated by the UEFI stub. BSS is zeroed. `x0` holds the physical address of `BootInfo`. The processor is at EL1, MMU is on (UEFI's identity mapping), caches are on.

**Step 2: Exception vectors.** Write `VBAR_EL1` to point to the kernel's exception vector table. This must happen first because any unexpected exception before this point would jump to whatever garbage is at the current `VBAR_EL1`. The vectors handle: synchronous exceptions (syscalls, page faults, alignment faults), IRQ, FIQ, and SError. All vectors initially point to a panic handler that dumps registers to UART.

```
Exception Vector Table (aligned to 2048 bytes):
  Offset    Exception             Handler
  ─────────────────────────────────────────
  0x000     Sync from current EL  sync_current_el_handler
  0x080     IRQ from current EL   irq_current_el_handler
  0x100     FIQ from current EL   fiq_current_el_handler
  0x180     SError from current   serror_current_el_handler
  0x200     Sync from lower EL    sync_lower_el_handler  (syscalls)
  0x280     IRQ from lower EL     irq_lower_el_handler
  0x300     FIQ from lower EL     fiq_lower_el_handler
  0x380     SError from lower EL  serror_lower_el_handler
```

**Step 3: UART initialization.** Parse the device tree (minimal parse — just find the `/chosen/stdout-path` node) to locate the PL011 UART base address. Initialize the UART with 115200 baud, 8N1. From this point, `kprintln!()` works. This is the first sign of life visible to a developer watching the serial console:

```
[boot] AIOS kernel v0.1.0 (aarch64)
[boot] BootInfo at 0x4000_0000, magic OK
[boot] UartReady — 2ms
```

**Step 4: Device tree parse.** Full parse of the FDT (flattened device tree). Extract: CPU count, memory regions (cross-checked against UEFI memory map), interrupt controller type and base address, timer frequency, all device nodes. Platform detection happens here.

**Step 5: Interrupt controller initialization.** GICv3 on QEMU and Pi 5. GICv2 on Pi 4. Configure the distributor and redistributor (GICv3) or distributor and CPU interface (GICv2). Enable the maintenance timer interrupt (for preemptive scheduling). All other device interrupts are disabled until the relevant driver is loaded.

**Step 6: Timer setup.** Read `CNTFRQ_EL0` for the timer frequency (typically 62.5 MHz on QEMU, varies on Pi). Configure `CNTP_CTL_EL0` for the physical timer. Set a 10ms tick for the scheduler. Enable the timer interrupt in the GIC.

**Step 7: MMU enable — page table setup.** This is the most complex step:

```
Before MMU reconfiguration:
  TTBR0_EL1 → UEFI's identity map (phys == virt)
  TTBR1_EL1 → not set (no kernel high-half mapping)

After:
  TTBR1_EL1 → Kernel page table (high-half mapping)
    0xFFFF_0000_0000_0000 + offset → kernel text (RX)
    0xFFFF_0000_0000_0000 + offset → kernel rodata (RO)
    0xFFFF_0000_0000_0000 + offset → kernel data/bss (RW, NX)
    0xFFFF_0000_0000_0000 + offset → boot stack (RW, NX)
    0xFFFF_0000_0000_0000 + offset → MMIO regions (device memory)
    0xFFFF_0000_0000_0000 + offset → BootInfo + memory map

  TTBR0_EL1 → Temporary identity map (will be replaced per-process)
    Keep identity map active briefly so the instruction that
    enables the new TTBR1 can still execute (it's at a physical
    address). After switching, the identity map entries are removed.

Page table format: 4-level, 4 KiB granule, 48-bit VA
  L0 (PGD) → L1 (PUD) → L2 (PMD) → L3 (PTE)

W^X enforcement:
  Every page is either Writable or Executable, never both.
  Kernel text:   PXN=0, UXN=1, AP=RO   (executable, not writable)
  Kernel rodata: PXN=1, UXN=1, AP=RO   (not executable, not writable)
  Kernel data:   PXN=1, UXN=1, AP=RW   (not executable, writable)
```

**Step 8: Physical page allocator.** Initialize a buddy allocator using the free physical pages from the UEFI memory map. Pages of type `Conventional`, `LoaderCode`, `LoaderData`, `BootServicesCode`, and `BootServicesData` are added to the free pool. Pages occupied by the kernel, initramfs, BootInfo, UEFI Runtime, and MMIO are excluded.

The buddy allocator manages pages in orders 0 through 10 (4 KiB to 4 MiB blocks). Allocation and deallocation are O(log n) in the number of orders.

**Step 9: Kernel heap.** Initialize a slab allocator on top of the buddy allocator. The slab allocator provides `alloc::alloc::GlobalAlloc` — from this point, `Box`, `Vec`, `String`, `HashMap`, and all other heap types work. The slab allocator has size classes: 32, 64, 128, 256, 512, 1024, 2048, 4096 bytes. Larger allocations go directly to the buddy allocator.

**Step 10: KASLR.** If the `BootInfo.rng_seed` is non-zero, compute a random kernel base offset (aligned to 2 MiB). Remap the kernel at the new virtual address. Update all absolute address references (the kernel is compiled as position-independent). This makes kernel address prediction harder for exploits.

If no RNG seed is available (firmware doesn't support the UEFI RNG protocol), KASLR is skipped and a warning is logged. This happens on older QEMU versions; real Pi hardware provides an RNG.

**Step 11: Capability manager.** Create the root capability — the single capability from which all others are derived:

```rust
pub struct CapabilityManager {
    root: CapabilityToken,
    token_table: HashMap<CapabilityTokenId, CapabilityToken>,
    next_id: AtomicU64,
}

impl CapabilityManager {
    fn bootstrap() -> Self {
        let root = CapabilityToken {
            id: CapabilityTokenId(0),
            capability: Capability::Root,     // can derive any capability
            holder: ProcessId::KERNEL,
            granted_by: ProcessId::KERNEL,
            expires: None,
            delegatable: true,
            attenuations: vec![],
        };
        Self {
            root,
            token_table: HashMap::new(),
            next_id: AtomicU64::new(1),
        }
    }
}
```

The root capability is held by the kernel. The Service Manager receives a derived capability that can create any service-level capability but cannot modify the kernel itself.

**Step 12: IPC subsystem.** Initialize the endpoint table and message buffer pools. No channels exist yet — they'll be created when the Service Manager spawns services. But the infrastructure must be ready.

**Step 13: Audit log.** Initialize a kernel ring buffer (64 KiB, circular) for audit events. During early boot, events are buffered here. Once Space Storage is available (Phase 1 of the Service Manager), the ring buffer is flushed to `system/audit/boot/` and subsequent events are written to space storage in real time.

**Step 14: Process manager.** Initialize the process table and scheduler. The scheduler uses priority + deadline scheduling: interactive processes get priority, background work gets deadline guarantees. The kernel itself runs as process 0.

**Step 15: Provenance chain.** Initialize the append-only Merkle-linked provenance log. The first entry records the kernel boot event, signed by the kernel's built-in key. All subsequent system events (service start, capability grant, agent spawn) are appended to this chain.

**Step 16: Early boot complete.** All kernel subsystems are initialized. The kernel is ready to create userspace processes.

```
[boot] Complete — 180ms
[boot] Memory: 3847 MiB free of 4096 MiB total
[boot] Kernel: 1.4 MiB (text: 680 KiB, data: 720 KiB)
[boot] Launching Service Manager...
```

### 3.4 PL011 UART for Early Debug

The PL011 UART is the first and last resort for debugging. It's initialized before anything else (after exception vectors) and remains available even after the display subsystem takes over. On Pi hardware, it's accessible via GPIO pins 14/15 (or the dedicated UART header on Pi 5).

The UART is configured at 115200 baud, 8N1, no flow control. The kernel's `kprintln!()` macro writes directly to the UART data register. In early boot (before the heap exists), formatting uses a small fixed buffer on the stack.

During normal operation, the UART is used by the recovery shell (see Section 8). In production builds, kernel log output to UART can be disabled via the command line (`quiet` flag in `boot.cfg`).

-----

## 4. Service Manager

The Service Manager is the first userspace process. It's the PID 1 of AIOS — responsible for starting, monitoring, and restarting every system service. If the Service Manager dies, the kernel panics (there's nothing left to manage the system).

### 4.1 How It's Spawned

The kernel creates the Service Manager directly, without going through the normal `ProcessCreate` syscall path (since there's no process to call the syscall yet):

```
1. Kernel reads Service Manager ELF from initramfs
   (the initramfs is in memory, loaded by the UEFI stub)
2. Kernel creates a new address space (TTBR0)
3. Kernel loads ELF segments into the new address space
4. Kernel mints a ServiceManagerCapability from the root capability:
   - Can create processes
   - Can create IPC channels
   - Can mint service-level capabilities (but not kernel-level)
   - Can read/write system spaces (once storage exists)
   - Cannot modify kernel state directly
5. Kernel creates IPC channels:
   - svcmgr_to_kernel: for process creation requests
   - kernel_to_svcmgr: for kernel notifications (process exit, etc.)
6. Kernel sets up initial register state:
   - x0 = pointer to ServiceManagerBootInfo (capability tokens, channel IDs)
   - sp = top of allocated user stack
   - pc = ELF entry point
7. Kernel adds the process to the scheduler
8. Scheduler picks up the Service Manager and runs it
```

### 4.2 Service Descriptors

Every service is described by a `ServiceDescriptor` that the Service Manager reads from the initramfs at startup. The descriptors are compiled into the initramfs as a serialized array:

```rust
pub struct ServiceDescriptor {
    /// Unique identifier for this service.
    id: ServiceId,

    /// Human-readable name.
    name: &'static str,

    /// ELF binary content hash (in initramfs or system space).
    binary: ContentHash,

    /// Which boot phase this service belongs to.
    phase: BootPhase,

    /// Services that must be running before this one starts.
    dependencies: &'static [ServiceId],

    /// Capabilities this service needs.
    capabilities: &'static [CapabilityRequest],

    /// How to handle failures.
    restart_policy: RestartPolicy,

    /// Maximum time to wait for the service to report healthy.
    health_timeout: Duration,

    /// Whether this service is required for boot to proceed.
    /// If true, boot halts if this service fails to start.
    /// If false, boot continues and the service is retried in background.
    critical: bool,

    /// Priority for scheduling within its phase.
    /// Higher priority services start first when dependencies allow.
    priority: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootPhase {
    Phase1Storage,
    Phase2Core,
    Phase3Ai,
    Phase4User,
    Phase5Experience,
}

pub struct CapabilityRequest {
    capability: Capability,
    reason: &'static str,
}

pub enum RestartPolicy {
    /// Restart immediately on failure, with exponential backoff.
    Always {
        max_restarts: u32,          // within the window
        window: Duration,           // observation window
        backoff_base: Duration,     // initial delay
        backoff_max: Duration,      // maximum delay
    },
    /// Restart once, then mark as degraded.
    Once,
    /// Never restart. If it dies, it's gone.
    Never,
}
```

### 4.3 Service State Machine

Each service tracks its state through a state machine:

```rust
#[derive(Debug, Clone)]
pub enum ServiceState {
    /// Waiting for dependencies to be satisfied.
    Pending,
    /// Dependencies met, waiting for a slot in the phase.
    Ready,
    /// Process created, waiting for health check.
    Starting {
        pid: ProcessId,
        started_at: Timestamp,
    },
    /// Service reported healthy and is operational.
    Running {
        pid: ProcessId,
        started_at: Timestamp,
        channels: Vec<ChannelId>,
    },
    /// Service exited or crashed. May be restarted.
    Failed {
        exit_code: Option<i32>,
        restart_count: u32,
        last_failure: Timestamp,
        reason: FailureReason,
    },
    /// Service deliberately stopped (shutdown sequence).
    Stopped,
    /// Service failed too many times. Not retrying.
    Degraded {
        restart_count: u32,
        last_failure: Timestamp,
    },
}

pub enum FailureReason {
    ProcessExited(i32),
    ProcessCrashed(Signal),
    HealthCheckTimeout,
    DependencyFailed(ServiceId),
    ResourceExhausted,
}
```

### 4.4 Health Checking

Services report health via a dedicated IPC channel. The protocol is simple:

```
Service Manager sends: HealthCheck { deadline: Timestamp }
Service replies:       HealthStatus::Healthy
                    or HealthStatus::Degraded { reason: String }
                    or (no reply within deadline → timeout → restart)
```

Health checks run every 10 seconds for critical services, every 30 seconds for non-critical. The first health check after startup has a longer timeout (`health_timeout` from the descriptor) to allow for initialization.

### 4.5 Service Dependency Graph

Services form a directed acyclic graph (DAG) where edges represent "must start after" relationships. The Service Manager uses topological sort within each phase to determine startup order, then starts independent services in parallel.

```
╔═══════════════════════════════════════════════════════════════════╗
║  PHASE 1: STORAGE                                                  ║
╠═══════════════════════════════════════════════════════════════════╣
║                                                                    ║
║  block_engine ──→ object_store ──→ space_storage                  ║
║                                                                    ║
╠═══════════════════════════════════════════════════════════════════╣
║  PHASE 2: CORE SERVICES                                           ║
╠═══════════════════════════════════════════════════════════════════╣
║                                                                    ║
║  device_registry ──→ subsystem_framework                          ║
║       │                    │                                       ║
║       ▼                    ├──→ input_subsystem                    ║
║  posix_compat              │                                       ║
║                            ├──→ display_subsystem ──→ compositor   ║
║                            │                                       ║
║                            └──→ network_subsystem                  ║
║                                                                    ║
╠═══════════════════════════════════════════════════════════════════╣
║  PHASE 3: AI SERVICES (parallel with Phase 4 on non-critical path)║
╠═══════════════════════════════════════════════════════════════════╣
║                                                                    ║
║  airs_core ──→ space_indexer                                      ║
║      │                                                             ║
║      └──→ context_engine                                          ║
║                                                                    ║
╠═══════════════════════════════════════════════════════════════════╣
║  PHASE 4: USER SERVICES                                           ║
╠═══════════════════════════════════════════════════════════════════╣
║                                                                    ║
║  identity_service ──→ preference_service                          ║
║                             │                                      ║
║                             ├──→ attention_manager                 ║
║                             │                                      ║
║                             └──→ agent_runtime                     ║
║                                                                    ║
╠═══════════════════════════════════════════════════════════════════╣
║  PHASE 5: EXPERIENCE                                               ║
╠═══════════════════════════════════════════════════════════════════╣
║                                                                    ║
║  workspace ──→ conversation_bar                                   ║
║                     │                                              ║
║                     └──→ autostart_agents                          ║
║                                                                    ║
╚═══════════════════════════════════════════════════════════════════╝
```

### 4.6 Parallel Startup Within Phases

Within each phase, the Service Manager starts services in dependency order but launches independent services in parallel. For example, in Phase 2:

```
t=0ms:   Start device_registry (no dependencies within phase)
t=0ms:   Start posix_compat (depends only on Phase 1 services)
t=50ms:  device_registry reports healthy
         Start subsystem_framework (depends on device_registry)
t=80ms:  subsystem_framework reports healthy
         Start input_subsystem, display_subsystem, network_subsystem
         (all three in parallel — they depend on subsystem_framework
          but not on each other)
t=120ms: input_subsystem healthy
t=150ms: display_subsystem healthy
         Start compositor (depends on display_subsystem)
t=160ms: network_subsystem healthy
t=200ms: compositor healthy
         Phase 2 complete
```

### 4.7 Root Capability Delegation

The Service Manager holds a `ServiceManagerCapability` derived from the kernel's root capability. When it starts a service, it mints the minimum set of capabilities that service needs:

```
Service Manager holds: ServiceManagerCapability
  │
  ├─→ block_engine:     RawStorageAccess, AuditWrite
  ├─→ object_store:     BlockEngineChannel, AuditWrite
  ├─→ space_storage:    ObjectStoreChannel, AuditWrite, SpaceManagement
  ├─→ device_registry:  SpaceWrite("system/devices"), AuditWrite
  ├─→ compositor:       DisplayAccess, InputAccess, SharedMemoryCreate
  ├─→ network:          RawNetworkAccess, SpaceRead("system/credentials")
  ├─→ airs:             InferenceAccess, SpaceReadWrite("system/models"),
  │                     SpaceReadWrite("system/index")
  ├─→ identity:         SpaceReadWrite("system/identity"), CryptoAccess
  ├─→ agent_runtime:    ProcessCreate, CapabilityMint (attenuated)
  └─→ ...
```

Each service receives exactly the capabilities it needs. No service holds the `ServiceManagerCapability` itself. Capability escalation is impossible — a compromised service cannot mint capabilities beyond its own set.

-----

## 5. Service Startup Phases (Detail)

### Phase 1: Storage

Storage is the first service phase because almost everything else depends on persistent state. Before storage, the system has only the initramfs (read-only, in memory).

**Block Engine** starts first. It takes ownership of the raw block device (VirtIO-Blk on QEMU, SD/eMMC on Pi). On first boot, it formats the device: writes the superblock, initializes the WAL region, creates the empty LSM-tree index (empty MemTable, no SSTables). On subsequent boots, it reads the superblock, replays the WAL to recover from any incomplete writes, and verifies the SSTable manifest.

```
Block Engine startup:
  1. Open raw block device (via kernel device handle)
  2. Read superblock at LBA 0
     First boot:  magic absent → format device
     Normal boot: magic present → verify checksum
  3. Replay WAL (skip if clean shutdown flag is set)
     - Scan WAL from tail to head
     - Apply committed entries not yet in main storage
     - Discard uncommitted entries
  4. Verify SSTable manifest (which SSTables are live)
  5. Report healthy to Service Manager
  Target: ~100ms (dominated by device I/O)
```

**Object Store** starts after Block Engine. It provides content-addressed object storage on top of raw blocks. On first boot, it creates the initial reference count table and content index. On normal boot, it verifies the index root and is ready to serve.

**Space Storage** starts after Object Store. It creates the system spaces on first boot:

```
First boot:
  system/             — Core zone
  system/devices/     — Device registry
  system/audit/       — Audit logs
  system/audit/boot/  — Boot audit log (flushed from kernel ring buffer)
  system/config/      — System configuration
  system/models/      — AI model storage
  system/index/       — Search indexes
  system/crash/       — Kernel panic logs
  system/agents/      — Installed agent manifests
  system/credentials/ — Credential store
```

On normal boot, Space Storage verifies these spaces exist and are consistent, then reports healthy. At this point, the kernel's audit ring buffer is flushed to `system/audit/boot/`. From now on, all audit events are written to space storage in real time.

**Phase 1 budget: ~300ms.**

### Phase 2: Core Services

These services make the system interactive. After Phase 2, there's a screen with content and the user can type.

**Device Registry** initializes first. It reads the device tree (or ACPI tables) and populates `system/devices/` with entries for all discovered hardware. On QEMU, this means VirtIO devices. On Pi, this means BCM peripherals.

**Subsystem Framework** initializes next. It registers the framework's core traits and the capability gate for hardware access. All subsystems (input, display, network, audio, etc.) register through this framework.

**Input Subsystem** registers with the framework and starts handling keyboard and mouse/touchpad events. On QEMU, this is VirtIO-Input. On Pi, this is USB HID via the USB host controller. Input events flow through the subsystem to the compositor's input router.

**Display Subsystem** initializes the GPU driver. On QEMU, this is VirtIO-GPU: the driver negotiates display resolution, allocates scanout buffers, and sets up the rendering pipeline via wgpu. On Pi, this is the VC4/V3D driver (Pi 4) or V3D 7.1 (Pi 5), which provides Vulkan capabilities. The display subsystem takes over from the early framebuffer (see Section 7 for the handoff).

**Compositor** starts after display. It creates the initial render pipeline, registers with the input subsystem for event routing, and presents the first composited frame. At this point, the splash screen transitions from the early framebuffer to the compositor.

**Network Subsystem** starts in parallel with display/compositor. It initializes the network stack (smoltcp), configures the network interface (VirtIO-Net on QEMU, Genet Ethernet on Pi), and starts DHCP. Basic TCP/IP is available from this point — but the full Network Translation Module (space resolver, shadow engine, etc.) comes later (Phase 16 in the development plan).

**POSIX Compatibility** starts in parallel with other Phase 2 services. It initializes the translation layer: mounts the POSIX filesystem view over spaces (`/spaces/`, `/home/`, `/tmp/`, `/dev/`, `/proc/`), sets up the C library (musl libc) shim, and makes BSD tools available.

**Phase 2 budget: ~500ms.**

### Phase 3: AI Services

AI services are **not on the critical boot path**. Phase 3 runs in parallel with Phase 4 after Phase 2 completes. If AIRS takes too long, the desktop appears without it.

**AIRS Core** starts first. It scans `system/models/` for available models, loads the default model's weights into memory (memory-mapped from space storage — this is fast because `mmap` avoids copying), and allocates the initial KV cache. The dominant cost is reading model weights from disk: a 4.5 GB Q4_K_M model takes ~2 seconds to memory-map from NVMe, longer from SD card.

```
AIRS startup:
  1. Read model registry from system/models/ space
  2. Select default model based on available RAM:
     >= 8 GB RAM: load 8B Q4_K_M  (~4.5 GB)
     >= 4 GB RAM: load 3B Q4_K_M  (~2.0 GB)
      < 4 GB RAM: load 1B Q4_K_M  (~0.7 GB)
  3. Memory-map model weights (mmap, lazy page-in)
  4. Initialize GGML runtime + NEON SIMD
  5. Warm up: run a short inference to fault in hot pages
  6. Report healthy to Service Manager
```

**The 5-second timeout:** The Service Manager gives AIRS 5 seconds to report healthy. If model loading is slow (large model on slow storage), the Service Manager proceeds to Phase 4 and Phase 5 without AIRS. AIRS continues loading in the background. Once it reports healthy, it's integrated seamlessly — the Context Engine picks it up, the Space Indexer starts, and the conversation bar becomes functional. The user sees the desktop immediately; AI features arrive moments later.

**Space Indexer** starts after AIRS is healthy. On first boot, it has nothing to index. On subsequent boots, it scans for objects modified since the last index update and queues them for embedding generation. This runs entirely in the background at `InferencePriority::Background`.

**Context Engine** starts after AIRS is healthy. It begins collecting signals (active spaces, running agents, input patterns, time of day) and makes its first context inference. If AIRS isn't available, the Context Engine immediately falls back to rule-based heuristics.

**Phase 3 budget: not on critical path. Runs in parallel with Phases 4-5. Target: AIRS healthy within 5 seconds.**

### Phase 4: User Services

User services personalize the system. They depend on storage (Phase 1) and core services (Phase 2), but not necessarily on AIRS (Phase 3).

**Identity Service** starts first. It reads the identity store from `system/identity/` (encrypted). If this is first boot, it generates a new Ed25519 keypair and prompts for a user passphrase (displayed via the compositor, which is already running). On normal boot, it uses the stored passphrase hash (or biometric, or hardware key) to unlock the identity. Once identity is established, per-space encryption keys can be derived.

**Preference Service** starts after Identity. It reads user preferences from the `user/preferences/` space. Display settings, notification thresholds, context overrides, keyboard layout, locale — all loaded and applied. If the preference space doesn't exist (first boot), defaults are used.

**Attention Manager** starts after Preferences. It initializes the notification pipeline, loads attention rules from preferences, and begins accepting notifications from other services. If AIRS is available, it enables AI triage. Otherwise, it uses rule-based triage.

**Agent Runtime** starts last in this phase. It initializes the agent sandbox infrastructure, loads the list of approved agents from `system/agents/`, and prepares to spawn agents on request. It does not spawn agents yet — that happens in Phase 5.

**Phase 4 budget: ~200ms.**

### Phase 5: Experience

The final phase makes the system user-facing.

**Workspace** renders the home view. It queries the Task Manager for active tasks, Space Storage for recent spaces, and the Attention Manager for the notification digest. The first frame of the Workspace is the "boot complete" moment — the user sees a usable desktop.

**Conversation Bar** initializes. If AIRS is available, it's fully functional. If AIRS is still loading, it shows a subtle "AI loading..." indicator and disables natural language features until AIRS reports healthy. Keyword search (via the full-text index) works immediately.

**Autostart Agents** are spawned. Any agents marked as autostart in the user's preferences are launched by the Agent Runtime. These are lightweight agents the user wants always running — a music agent, a backup agent, etc.

**Boot Complete Signal:** The Service Manager records the total boot time in `system/audit/boot/` and logs it to UART:

```
[boot] Phase 5 complete — boot to desktop in 1,847ms
[boot] Services: 18 running, 0 failed, 0 degraded
[boot] AIRS: healthy (model: llama-3.1-8b-q4_k_m, loaded in 3,200ms)
```

**Phase 5 budget: ~300ms (first frame).**

-----

## 6. Boot Performance Budget

### 6.1 Critical Path Timeline

The critical path is the sequence of steps that cannot be parallelized — each depends on the previous:

```
Time (ms)     Phase                    What happens
──────────────────────────────────────────────────────────────────
   0          Firmware                 POST, DRAM, UEFI init
 500          Firmware complete        Load AIOS kernel from ESP
──────────────────────────────────────────────────────────────────
 510          Kernel entry             Exception vectors, UART
 530          Device tree parsed       Platform detected
 550          Interrupts + timer       GICv3 initialized
 600          MMU enabled              Page tables, W^X
 640          Page allocator + heap    Memory management ready
 660          Core subsystems          Cap mgr, IPC, audit, procmgr
 700          Kernel boot complete     Launch Service Manager
──────────────────────────────────────────────────────────────────
 710          Phase 1: Storage         Block Engine start
 810          Block Engine healthy     Object Store start
 880          Object Store healthy     Space Storage start
1000          Phase 1 complete         System spaces ready
──────────────────────────────────────────────────────────────────
1010          Phase 2: Core            Device registry, subsystem
1050          Subsystem framework up   Input, display, network start
1150          Display ready            Compositor start
1200          Network ready            (not on critical path)
1300          Compositor healthy       Early framebuffer → compositor
1500          Phase 2 complete
──────────────────────────────────────────────────────────────────
1510          Phase 3: AI              AIRS starts (PARALLEL)
1510          Phase 4: User            Identity, prefs, attention
1600          Identity authenticated
1650          Preferences loaded
1700          Phase 4 complete
──────────────────────────────────────────────────────────────────
1710          Phase 5: Experience      Workspace renders
1850          First frame presented    BOOT COMPLETE
──────────────────────────────────────────────────────────────────
...           Phase 3 (background)     AIRS model loading continues
4500          AIRS healthy             AI features available
```

### 6.2 Budget Breakdown

```
Component                Target     Notes
─────────────────────────────────────────────────────────────────
Firmware                 ~500ms     Not controllable. DRAM training dominates.
                                    QEMU is faster (~200ms). Pi 4 is ~500ms.
Kernel early boot         200ms     Most time in page table setup and
                                    device tree parsing.
Phase 1 (storage)         300ms     WAL replay adds time after dirty shutdown.
                                    First boot adds ~200ms for formatting.
Phase 2 (core services)   500ms     GPU init is the bottleneck.
                                    VirtIO-GPU is fast (~50ms).
                                    Pi VC4/V3D is slower (~200ms).
Phase 4 (user services)   200ms     Identity unlock may block on user input
                                    (passphrase). Biometric is faster.
Phase 5 (experience)      150ms     First compositor frame.
─────────────────────────────────────────────────────────────────
Critical path total:    ~1,850ms    Well under 3-second target.
With firmware:          ~2,350ms    Comfortable margin.
```

### 6.3 Parallel vs Sequential Timeline

```
Time: 0       500      1000      1500      2000      2500      3000
      |--------|---------|---------|---------|---------|---------|
      ████████████████████                                        Firmware
               ██████████                                         Kernel boot
                         ███████████                               Phase 1
                                    ████████████████               Phase 2
                                    ·································> Phase 3 (AI)
                                                   ███████         Phase 4
                                                          █████    Phase 5
                                                              ↑
                                                      BOOT COMPLETE
                                                        ~1,850ms
                                                     (+ ~500ms firmware)

Legend: █ = on critical path    · = parallel (not blocking boot)
```

### 6.4 Optimization Techniques

Several techniques keep boot under budget:

**Lazy model loading.** AIRS memory-maps model weights instead of reading them into RAM. Pages fault in on first access. The model starts generating tokens before all weights are in memory. This turns a 2-second read into a ~100ms mmap + progressive fault-in during first inference.

**Parallel service startup.** Within each phase, independent services start simultaneously. Phase 2 starts input, display, and network in parallel.

**Initramfs in memory.** The UEFI stub loads the initramfs into contiguous physical memory. The kernel reads it directly — no disk I/O during early service startup.

**Deferred indexing.** The Space Indexer doesn't run during boot. It starts background work after the desktop is visible.

**Warm page cache.** On repeated boots, frequently accessed blocks (superblock, SSTable manifest, model metadata) are likely in the storage device's internal cache, making reads faster.

-----

## 7. Early Framebuffer and Splash

### 7.1 The Problem

The compositor is a complex userspace service that starts in Phase 2. It depends on the display subsystem, which depends on the subsystem framework, which depends on storage. That's at least 1,000ms into boot before the compositor can display anything.

A 1,000ms black screen is unacceptable. The user needs visual feedback that the system is alive.

### 7.2 The Solution: Kernel Framebuffer

The UEFI stub acquires a framebuffer via the Graphics Output Protocol (GOP) before calling `ExitBootServices()`. This framebuffer is a simple linear pixel buffer at a physical address — no GPU driver required, no complex protocol. The kernel can write pixels to it directly.

```rust
pub struct EarlyFramebuffer {
    info: FramebufferInfo,          // from BootInfo
    buffer: &'static mut [u32],    // mapped into kernel virtual address space
}

impl EarlyFramebuffer {
    /// Called once during kernel early boot (after MMU is enabled)
    fn init(boot_info: &BootInfo) -> Option<Self> {
        let fb = boot_info.framebuffer.as_ref()?;
        let buffer = unsafe {
            // Map framebuffer physical address into kernel address space
            // as device memory (uncacheable, write-combining)
            kernel_map_device(fb.base, fb.size)
        };
        Some(Self { info: fb.clone(), buffer })
    }

    /// Draw a pixel at (x, y) with the given color
    fn put_pixel(&mut self, x: u32, y: u32, color: u32) {
        let offset = (y * self.info.stride / 4 + x) as usize;
        if offset < self.buffer.len() {
            self.buffer[offset] = color;
        }
    }

    /// Fill rectangle
    fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, color: u32) {
        for dy in 0..h {
            for dx in 0..w {
                self.put_pixel(x + dx, y + dy, color);
            }
        }
    }

    /// Draw the AIOS splash screen
    fn draw_splash(&mut self) {
        // Dark background
        self.fill_rect(0, 0, self.info.width, self.info.height, 0x001A1A2E);

        // Simple centered logo (compiled-in bitmap, ~2 KiB)
        let logo_x = (self.info.width - LOGO_WIDTH) / 2;
        let logo_y = (self.info.height - LOGO_HEIGHT) / 2 - 40;
        self.blit_bitmap(logo_x, logo_y, &AIOS_LOGO);

        // Progress bar area (drawn empty, updated by advance_progress)
        let bar_x = (self.info.width - PROGRESS_WIDTH) / 2;
        let bar_y = logo_y + LOGO_HEIGHT + 30;
        self.fill_rect(bar_x, bar_y, PROGRESS_WIDTH, PROGRESS_HEIGHT, 0x00333355);
    }

    /// Called at each boot phase to advance the progress bar
    fn advance_progress(&mut self, phase: EarlyBootPhase) {
        let fraction = phase as u32 as f32 / EarlyBootPhase::Complete as u32 as f32;
        let bar_x = (self.info.width - PROGRESS_WIDTH) / 2;
        let bar_y = /* ... */;
        let filled = (PROGRESS_WIDTH as f32 * fraction) as u32;
        self.fill_rect(bar_x, bar_y, filled, PROGRESS_HEIGHT, 0x006C63FF);
    }
}
```

### 7.3 Visual Feedback Timeline

```
Time      Visual
──────────────────────────────────────────
  0ms     Screen off / firmware POST
500ms     AIOS splash appears (kernel draws to GOP framebuffer)
520ms     Progress bar: 10% (UART, device tree)
600ms     Progress bar: 30% (MMU, heap)
700ms     Progress bar: 50% (kernel subsystems)
1000ms    Progress bar: 70% (storage ready)
1300ms    FRAMEBUFFER HANDOFF: compositor takes over
          Smooth transition — compositor's first frame replaces splash
1850ms    Workspace visible. Boot complete.
```

### 7.4 Framebuffer Handoff to Compositor

When the compositor starts, it takes ownership of the display hardware. The handoff must be smooth — no black frame, no flicker:

```
1. Compositor initializes GPU driver (wgpu, VirtIO-GPU or VC4/V3D)
2. Compositor reads the current framebuffer content
   (the splash screen with progress bar)
3. Compositor renders its first frame:
   - Start with the splash screen as the background
   - Cross-fade to the Workspace over ~200ms
4. Compositor signals the kernel: "display handoff complete"
5. Kernel unmaps the early framebuffer
6. From this point, only the compositor writes to the display
```

On headless systems (no framebuffer from UEFI GOP), the early framebuffer is skipped entirely. Boot progress is visible only on UART.

-----

## 8. Recovery Mode

### 8.1 Failure Detection

The Service Manager tracks boot attempts using a counter stored in UEFI Runtime Variables (persistent across reboots):

```rust
pub struct BootAttemptTracker {
    /// Incremented at kernel entry, cleared when Phase 5 completes.
    /// If this reaches 3 without being cleared, recovery mode triggers.
    consecutive_failures: u32,

    /// Set to true when Phase 5 completes and user sees the desktop.
    boot_success: bool,
}
```

**Decision tree:**

```
Boot starts
  │
  ├── consecutive_failures < 3?
  │     YES → normal boot
  │     NO  → recovery mode
  │
  ▼ (normal boot)
Phase 5 completes?
  │
  ├── YES → clear consecutive_failures, boot_success = true
  │
  └── NO (crash or hang during boot)
        │
        ├── Reboot (watchdog or manual)
        │
        └── consecutive_failures incremented
            Loop back to top
```

### 8.2 Recovery Shell

Recovery mode boots with minimal services: kernel + storage + UART console. No compositor, no networking, no AIRS.

```
[AIOS RECOVERY MODE]
Boot failed 3 consecutive times. Starting recovery shell.
Last failure: Phase 2 — compositor failed to start (HealthCheckTimeout)

Available commands:
  status          — show service states from last boot attempt
  logs            — show kernel and service logs
  safe-boot       — boot without AI services, without agents
  rollback        — revert to previous kernel and initramfs
  fsck            — verify and repair storage integrity
  factory-reset   — wipe user spaces, preserve system (DESTRUCTIVE)
  reboot          — attempt normal boot again
  shell           — drop to BSD sh (if POSIX compat is available)

recovery>
```

The recovery shell is a minimal Rust binary compiled into the initramfs. It communicates with the kernel and storage via direct IPC. It does not require the compositor, networking, or any AI services.

### 8.3 Safe Mode

Safe mode boots with reduced services. It's triggered by the `safe-boot` command in the recovery shell, or by a keyboard shortcut held during boot (e.g., holding Shift):

```
Safe mode service list:
  Phase 1: Storage (full)               — spaces must work
  Phase 2: Core (reduced)               — compositor + input only
            No network, no POSIX compat
  Phase 3: SKIPPED                      — no AIRS, no indexer
  Phase 4: Identity only                — no prefs, no attention, no agents
  Phase 5: Workspace (minimal)          — basic desktop, no conversation bar
```

Safe mode is useful for diagnosing issues caused by agents, broken preferences, or AIRS configuration problems. The user gets a functional desktop and can use the Inspector to diagnose what went wrong.

### 8.4 Rollback

If the current kernel or initramfs is broken, the UEFI stub can load the previous versions:

```
ESP layout:
  aios.elf              — current kernel
  aios.elf.prev         — previous kernel
  initramfs.cpio        — current initramfs
  initramfs.cpio.prev   — previous initramfs
```

The `rollback` command in recovery mode:
1. Renames current → `.bad`
2. Renames `.prev` → current
3. Reboots

This restores the last known-good kernel and service binaries.

### 8.5 Factory Reset

Nuclear option. Wipes user spaces but preserves the system:

```
Factory reset:
  1. Wipe user/ space (all personal data)
  2. Wipe shared/ space (all collaborative data)
  3. Wipe web-storage/ space (all browser data)
  4. Wipe system/agents/ (remove all installed agents)
  5. Wipe system/credentials/ (remove all stored credentials)
  6. Preserve system/config/ (keep hardware configuration)
  7. Preserve system/models/ (keep downloaded models)
  8. Preserve kernel + initramfs
  9. Reset boot counter
  10. Reboot → first-boot experience
```

Factory reset requires confirmation (type "FACTORY RESET" on the UART console). It's irreversible. Models are preserved because they're large downloads that aren't user-sensitive.

-----

## 9. Initramfs and System Image

### 9.1 What's in the Initramfs

The initramfs is a cpio archive loaded into memory by the UEFI stub. It contains everything needed to reach the end of Phase 2 (core services running), at which point the system can access the persistent storage partition:

```
initramfs.cpio contents:
  /svcmgr                — Service Manager binary
  /services/
    block_engine          — Block Engine service
    object_store          — Object Store service
    space_storage         — Space Storage service
    device_registry       — Device Registry service
    subsystem_framework   — Subsystem Framework service
    input_subsystem       — Input subsystem service
    display_subsystem     — Display subsystem service
    compositor            — Compositor service
    network_subsystem     — Network subsystem service
    posix_compat          — POSIX compatibility service
  /service_descriptors    — serialized Vec<ServiceDescriptor>
  /logo.bin               — splash screen bitmap (compiled-in fallback too)
  /fonts/
    mono.ttf              — monospace font for terminal
    sans.ttf              — UI font
  /bin/
    sh                    — FreeBSD /bin/sh (for recovery shell)
    ls, cat, grep, ...    — minimal BSD tools (for recovery)
  /lib/
    libc.so               — musl libc shared library
```

Total initramfs size target: **< 32 MB**. The initramfs is compressed (zstd) to ~10 MB on the ESP.

### 9.2 Boot Image Format

The kernel and initramfs are bundled as a single boot image by the build system:

```
AIOS Boot Image:
  ┌────────────────────────────┐
  │  UEFI Stub (PE/COFF)       │  ~64 KiB
  ├────────────────────────────┤
  │  Kernel ELF                │  ~1.5 MB
  ├────────────────────────────┤
  │  Initramfs (zstd-compressed│  ~10 MB
  │  cpio archive)             │
  ├────────────────────────────┤
  │  Boot manifest (JSON)      │  ~1 KiB
  │  - kernel hash             │
  │  - initramfs hash          │
  │  - build timestamp         │
  │  - version string          │
  └────────────────────────────┘
```

The UEFI stub extracts the kernel and initramfs into separate physical memory regions, populates `BootInfo`, and jumps to the kernel. The boot manifest provides integrity verification — the stub checks hashes before jumping.

### 9.3 Transition from Initramfs to System Space

Once Space Storage is running (end of Phase 1), services can be loaded from the persistent `system/services/` space instead of the initramfs. This transition matters for Phase 3-5 services:

```
Phase 1-2 services:  loaded from initramfs (in memory, fast)
Phase 3-5 services:  loaded from system/services/ space (persistent storage)
```

The distinction matters because Phase 3-5 services can be updated independently of the kernel. Updating AIRS doesn't require a new initramfs — just update the binary in `system/services/`. The initramfs contains only the minimum needed to bootstrap storage and core services.

On first boot, the Service Manager copies Phase 3-5 service binaries from the initramfs to `system/services/`. On subsequent boots, it loads from the space. If a service binary in the space is corrupt, it falls back to the initramfs copy.

-----

## 10. Shutdown and Reboot

### 10.1 Graceful Shutdown Sequence

Shutdown is the reverse of boot, with extra care for data integrity:

```
1. User requests shutdown (or system initiates reboot)
     │
     ▼
2. Phase 5 teardown
   - Autostart agents receive shutdown signal (5-second grace period)
   - Agents persist state to their spaces
   - Conversation bar saves conversation to space
   - Workspace saves layout state
     │
     ▼
3. Phase 4 teardown
   - Agent Runtime terminates remaining agents
   - Attention Manager flushes pending notifications to space
   - Preference Service writes any dirty preferences
   - Identity Service locks identity (zero keys from memory)
     │
     ▼
4. Phase 3 teardown
   - Context Engine saves last known context state
   - Space Indexer checkpoints index state
   - AIRS unloads models (just drop the mmap — instant)
     │
     ▼
5. Phase 2 teardown
   - Compositor presents "Shutting down..." screen
   - Network subsystem closes all connections
   - Input subsystem quiesced
   - Display subsystem releases GPU
   - POSIX compat flushes file descriptors
     │
     ▼
6. Phase 1 teardown
   - Space Storage flushes all pending writes
   - Object Store flushes reference count updates
   - Block Engine flushes WAL to stable storage
   - Block Engine writes clean-shutdown flag to superblock
     │
     ▼
7. Kernel shutdown
   - All service processes terminated
   - Audit log: final entry "clean shutdown"
   - Disable interrupts (DAIF mask)
   - Flush caches (DC CIVAC, IC IALLU)
   - Call UEFI Runtime Services: ResetSystem(Shutdown)
   - (or ResetSystem(Reboot) for reboot)
```

### 10.2 Forced Shutdown

If graceful shutdown takes longer than 10 seconds, the kernel forces the issue:

```
 0s    Graceful shutdown begins
 5s    Services still running → warning logged
 8s    Remaining services receive SIGKILL
10s    Force: storage flush (WAL commit), then power off
       No data loss thanks to WAL, but state may be incomplete
```

The watchdog timer (ARM Generic Timer) is set to 15 seconds at shutdown start. If the kernel hangs during shutdown, the hardware watchdog forces a reset. On the next boot, the WAL replay recovers any incomplete writes.

### 10.3 Agent State Persistence

Agents that need to survive reboot use the `Persistence::Persistent` mode. Their state is stored in their designated space:

```
Agent receives: ShutdownSignal { deadline: Timestamp }
Agent has 5 seconds to:
  1. Save conversation context to space
  2. Save task progress to space
  3. Close open sessions
  4. Reply: ShutdownAck
If no ack within 5 seconds: agent is killed
Agent state in spaces survives the reboot
On next boot: agent is relaunched and reads state from space
```

-----

## 11. Implementation Order

The boot sequence maps to the earliest development phases:

```
Phase 0: Foundation & Tooling (Weeks 1-4)
  - Cross-compilation toolchain (Rust → aarch64)
  - QEMU runner scripts
  - UEFI stub skeleton
  - Build system for kernel + initramfs

Phase 1: Boot & First Pixels (Weeks 5-8)
  - UEFI stub: memory map, framebuffer, device tree, RNG
  - Kernel entry: exception vectors, UART, device tree parse
  - GICv3 + timer
  - MMU enable: page tables, W^X
  - Early framebuffer: splash screen
  - Kernel writes "Hello from AIOS" to screen and UART

Phase 2: Memory Management (Weeks 9-12)
  - Buddy allocator (physical pages)
  - Slab allocator (kernel heap)
  - KASLR
  - Per-process address spaces (TTBR0 switching)
  - Shared memory regions

Phase 3: IPC & Capability System (Weeks 13-16)
  - Syscall handler (SVC trap)
  - IPC channels (send/recv/call)
  - Capability manager (create, transfer, revoke)
  - Audit log (ring buffer)
  - Process manager + scheduler
  - Service Manager (PID 1)
  - Provenance chain (first entry)

Phase 4: Block Storage & Object Store (Weeks 17-20)
  - Block Engine (superblock, WAL, LSM-tree)
  - Object Store (content-addressing, dedup)
  - Space Storage (system spaces, Space API)
  - Kernel audit log flush to space storage
  - Phase 1 boot sequence operational

Phase 5-6: GPU, Display, Compositor (Weeks 21-28)
  - VirtIO-GPU driver
  - Framebuffer handoff
  - Compositor
  - Phase 2 boot sequence operational

Phase 7: Input, Terminal, Networking (Weeks 29-34)
  - VirtIO-Input, keyboard/mouse
  - Network (smoltcp, VirtIO-Net)
  - Terminal emulator
  - Phase 2 fully operational

Phase 8: AIRS Core (Weeks 35-39)
  - GGML integration, model loading
  - Phase 3 boot sequence operational

Phase 14: Performance & Optimization (Weeks 55-58)
  - Boot time profiling and optimization
  - Achieve < 3 second boot target
  - Recovery mode implementation
  - Safe mode
  - Rollback mechanism

Phase 24: Secure Boot (Weeks 113-116)
  - Verified boot chain
  - A/B partition scheme
  - Boot counter in UEFI variables
  - Automatic rollback on failure
```

The boot sequence is built incrementally. After Phase 1, the kernel boots and shows pixels. After Phase 3, it launches the Service Manager. After Phase 4, storage works. After Phase 6, there's a desktop. Each phase is a demonstrable milestone — the boot sequence is never "all or nothing."

-----

## 12. Design Principles

1. **Usable at each phase boundary.** Every service is optional except storage and display. AIRS failure doesn't break boot. Network failure doesn't break boot.
2. **Fast on the critical path.** Only the minimum services needed for a desktop are on the critical path. Everything else runs in parallel or deferred.
3. **Visual feedback from the start.** The user sees a splash screen within 500ms of power-on. Never stare at a black screen.
4. **Recovery is always possible.** Three consecutive failures trigger recovery mode. Rollback to previous kernel is always available. Factory reset is the last resort.
5. **Services are independent.** Each service has its own process, its own capabilities, its own restart policy. One service crashing doesn't take down the system.
6. **Boot is audited.** Every phase transition, every service start, every failure is logged. The audit trail is queryable after boot via `system/audit/boot/`.
7. **First boot and normal boot are the same code path.** The only difference is whether system spaces exist yet (first boot creates them). No separate "installer" or "setup wizard."
