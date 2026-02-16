# AIOS Boot and Init Sequence

## Deep Technical Architecture

**Parent document:** [architecture.md](../project/architecture.md) — Section 6.1 Boot Sequence
**Related:** [hal.md](./hal.md) — Platform trait, device abstractions, porting guide, [ipc.md](./ipc.md) — IPC and syscalls, [scheduler.md](./scheduler.md) — Scheduling classes and context multipliers, [memory.md](./memory.md) — Memory management and pool sizing, [spaces.md](../storage/spaces.md) — Space Storage, [airs.md](../intelligence/airs.md) — AI Runtime Service, [compositor.md](../platform/compositor.md) — Display handoff and framebuffer, [security.md](../security/security.md) — Capability system and trust levels, [identity.md](../experience/identity.md) — Identity initialization, [agents.md](../applications/agents.md) — Agent lifecycle and state persistence, [attention.md](../intelligence/attention.md) — Attention Manager initialization, [context-engine.md](../intelligence/context-engine.md) — Context Engine startup, [preferences.md](../intelligence/preferences.md) — Preference Service startup, [development-plan.md](../project/development-plan.md) — Phase plan

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
    acpi_rsdp: Option<PhysicalAddress>,

    /// UEFI Runtime Services function table.
    /// Provides: GetTime, SetTime, ResetSystem, GetVariable.
    /// Available after ExitBootServices (unlike Boot Services).
    runtime_services: Option<PhysicalAddress>,

    /// Random seed from UEFI RNG protocol. Used for KASLR.
    /// If unavailable, kernel falls back to timer-based entropy.
    rng_seed: [u8; 32],

    /// Physical address where the kernel ELF was loaded.
    kernel_phys_base: PhysicalAddress,

    /// Size of kernel image in memory (text + rodata + data + bss).
    kernel_size: usize,

    /// Physical address of the initramfs (cpio archive).
    initramfs_base: PhysicalAddress,
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
    physical_start: PhysicalAddress,
    virtual_start: VirtualAddress, // unused, UEFI sets to 0
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
    base: PhysicalAddress,             // physical address of pixel buffer
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
    base: PhysicalAddress,
    size: usize,
}

#[repr(C)]
pub struct CommandLine {
    ptr: *const u8,
    len: usize,
}
```

### 2.3 Kernel Command Line

The `CommandLine` in `BootInfo` is a UTF-8 string parsed by the kernel during Step 4 (device tree parse). It comes from `boot.cfg` on the ESP or from the UEFI `LoadOptions` variable. Recognized options:

```
Option              Default   Description
────────────────────────────────────────────────────────────
quiet               off       Suppress kernel log output to UART. Boot phase
                              transitions are still logged; service logs are not.
debug               off       Enable verbose kernel logging: page table setup
                              details, capability minting, IPC channel creation.
safe                off       Boot into safe mode (§9.3) — reduced service set,
                              no AIRS, no agents, no network.
console=<device>    uart0     Kernel log output device. Supported: uart0, none.
                              "none" disables UART logging entirely.
earlybreak          off       Halt after kernel early boot completes (before
                              launching Service Manager). Drop to UART debug
                              prompt. Useful for kernel debugging.
maxcpus=<n>         all       Limit the number of secondary CPUs brought online
                              via PSCI. 1 = boot CPU only (single-core mode).
kaslr=<on|off>      on        Enable or disable KASLR. Off is useful for
                              debugging with predictable addresses.
airs.timeout=<ms>   5000      Override the AIRS health timeout. Set higher on
                              slow storage (e.g., SD card on Pi 4).
audit=<on|off>      on        Enable or disable the kernel audit log.
```

Unknown options are ignored and logged at `debug` level if `debug` is on. The command line is stored in `KernelState` and available to the Service Manager via its `ServiceManagerBootInfo`.

### 2.4 EFI System Partition Layout

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

### 2.5 QEMU Boot vs Real Hardware

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

### 2.6 Exception Level Model

AIOS runs at **EL1** (OS kernel privilege). It does not use EL2 (hypervisor) and does not act as a hypervisor. The levels below the kernel:

```
Exception Level     Who occupies it            AIOS's relationship
────────────────────────────────────────────────────────────────────
EL3 (Secure Monitor) ARM Trusted Firmware (ATF)  AIOS calls it via SMC for PSCI
                     Present on Pi 4/5.          (CPU_ON, SYSTEM_RESET, etc.)
                     Not present on QEMU.

EL2 (Hypervisor)    KVM (if QEMU uses -enable-kvm) AIOS is unaware of EL2.
                     Not used on Pi bare-metal.     UEFI drops to EL1 before
                                                    jumping to kernel.

EL1 (OS Kernel)     AIOS kernel                 This is where we run.
                     Full access to page tables,
                     interrupt controller, timers.

EL0 (User)          Service Manager, all services, All userspace processes.
                     agents, compositor.
```

**PSCI conduit selection:** The device tree `/psci` node specifies the conduit:
- `method = "smc"` → Pi 4/5 (ATF at EL3 handles the call)
- `method = "hvc"` → QEMU without KVM (QEMU emulates PSCI at EL2)
- `method = "hvc"` → QEMU with KVM (KVM intercepts HVC and handles PSCI)

The kernel reads this during Step 4 (device tree parse) and stores it for SMP bringup (§3.5). The choice of HVC vs SMC is the *only* place where exception levels affect AIOS — everything else runs at EL1/EL0 uniformly.

**UEFI guarantees:** The UEFI firmware (edk2) always drops to EL1 before calling `ExitBootServices()`. By the time the kernel entry point runs, EL2 is either not present (bare metal Pi) or occupied by KVM/QEMU (transparent to the kernel). The kernel never touches EL2 registers.

The kernel abstracts these differences behind a `Platform` trait initialized during early boot. The full HAL specification — device abstractions, MMIO primitives, VirtIO transport, DMA, and the guide for adding new platforms — is in [hal.md](./hal.md).

```rust
pub trait Platform: Send + Sync {
    fn init_interrupts(&self, dt: &DeviceTree) -> Result<InterruptController>;
    fn init_timer(&self, dt: &DeviceTree) -> Result<Timer>;
    fn init_uart(&self, dt: &DeviceTree) -> Result<Uart>;
    fn init_gpu(&self, dt: &DeviceTree) -> Result<GpuDevice>;
    fn init_network(&self, dt: &DeviceTree) -> Result<NetworkDevice>;
    fn init_storage(&self, dt: &DeviceTree) -> Result<StorageDevice>;
    fn init_rng(&self, dt: &DeviceTree) -> Result<RngDevice>;
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

The seven methods are called at different points during boot — UART, interrupts, timer, and RNG during kernel early boot (before heap), and GPU, network, and storage during service manager phases (after heap). See hal.md §3.2 for the full initialization order.

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
    /// Hardware RNG initialized. Runtime entropy available.
    RngReady,
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
    pub boot_start: u64,           // timer counter value at entry
    pub phase_timestamps: [u64; 17], // one per EarlyBootPhase variant; resize if enum grows
}
```

### 3.3 Step-by-Step Early Boot

Each step below includes what it initializes and why it must happen at that point.

**Step 1: Entry point.** The UEFI stub jumps here. The kernel is running on a temporary stack allocated by the UEFI stub. BSS is zeroed. `x0` holds the physical address of `BootInfo`. The processor is at EL1, MMU is on (UEFI's identity mapping), caches are on.

Immediately at entry, before any other code runs:

```asm
// Enable FP/NEON (CPACR_EL1.FPEN = 0b11)
// Without this, any floating-point or NEON instruction traps to EL1.
// Rust's codegen freely uses NEON registers for memcpy/memset,
// so this must happen before ANY Rust code executes.
mrs  x1, CPACR_EL1
orr  x1, x1, #(3 << 20)    // FPEN = 0b11: no trapping
msr  CPACR_EL1, x1
isb                          // ensure the change is visible

// Save BootInfo pointer
mov  x19, x0                // x19 is callee-saved
```

This enables the Advanced SIMD (NEON) and floating-point unit. On aarch64, NEON is mandatory (not optional like on ARMv7), but access still traps unless `CPACR_EL1.FPEN` is set. UEFI may or may not have enabled it — the kernel sets it unconditionally. NEON is later used by: GGML runtime (Phase 3 AIRS inference), `memcpy`/`memset` optimizations throughout the kernel, and cryptographic operations (AES-NI equivalent instructions via ARMv8 Crypto Extensions).

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

**Step 6: Timer setup.** Read `CNTFRQ_EL0` for the timer frequency (typically 62.5 MHz on QEMU, varies on Pi). Configure `CNTP_CTL_EL0` for the physical timer. Set a **1ms tick** (1000 Hz) for the scheduler — this provides < 1ms worst-case scheduling latency, necessary for the compositor's 16.6ms frame deadline (scheduler.md §10.1). Enable the timer interrupt in the GIC.

**Watchdog timer:** Also at this step, arm a hardware watchdog using the ARM Generic Timer's watchdog function (or the platform's watchdog: SP805 on QEMU, bcm2835-wdt on Pi). The watchdog is set to a **30-second timeout** — long enough for a normal boot (target ~1.8s) plus margin for slow storage. If the kernel hangs during boot and never clears the watchdog, the hardware forces a reset after 30 seconds, incrementing the `consecutive_failures` counter in UEFI variables (see §9.1). After Phase 5 completes (boot success), the watchdog is reconfigured to a **60-second timeout** and becomes the runtime watchdog — the Service Manager pings it every 30 seconds via syscall. During shutdown, it's shortened to 15 seconds (§11.2). Recovery mode disables the watchdog to allow interactive debugging via UART.

**Step 7: MMU enable — page table setup.** This is the most complex step:

```
Before MMU reconfiguration:
  TTBR0_EL1 → UEFI's identity map (phys == virt)
  TTBR1_EL1 → not set (no kernel high-half mapping)

After:
  TTBR1_EL1 → Kernel page table (high-half mapping)
    0xFFFF_0000_0000_0000          → kernel text (RX) + rodata (RO) + data/bss (RW, NX)
    0xFFFF_0000_4000_0000          → kernel heap
    0xFFFF_0000_8000_0000          → kernel stacks (per-core + per-process, with guard pages)
    0xFFFF_0001_0000_0000          → physical memory direct map
    0xFFFF_0002_0000_0000          → MMIO regions (device memory)

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

**Kernel stack lifecycle:** At entry (Step 1), the boot CPU runs on a temporary stack allocated by the UEFI stub — typically 64 KiB, location unknown to the kernel. During Step 7, the kernel allocates a proper 16 KiB kernel stack at a known virtual address (`0xFFFF_0000_8000_0000 + core_id * 0x10000`) with a **guard page** — a 4 KiB page mapped as no-access immediately below the stack. Stack overflow writes to the guard page, triggering a page fault caught by the exception handler (instead of silently corrupting the heap). After the stack switch, the UEFI stub stack is released. Secondary cores (§3.5) receive their own 16 KiB stacks with guard pages at the same virtual base + offset. User processes receive stacks from the Process Manager (Step 15); user stacks are allocated in the process's TTBR0 address space, default 1 MiB, also with guard pages.

**Cache coherency:** ARMv8 provides hardware cache coherency between cores (MOESI protocol via the Cache Coherent Interconnect). The kernel does *not* need explicit D-cache maintenance for inter-core communication — hardware handles it. However, two cache maintenance operations are required during boot:

1. **After KASLR remapping (Step 11):** `IC IALLU` (Invalidate All to Point of Unification, inner shareable) + `ISB` — ensures the instruction cache reflects the new kernel virtual addresses. Without this, stale I-cache entries pointing at old addresses cause unpredictable execution.
2. **After writing exception vectors (Step 2):** `DC CVAU` (Clean by VA to Point of Unification) on the vector table, then `IC IVAU` + `ISB` — ensures instruction cache sees the freshly written handlers. (On most ARMv8 implementations this is handled by hardware, but the architecture does not guarantee it — clean + invalidate is required for correctness.)

MMIO regions (device memory) are mapped as `nGnRnE` (non-Gathering, non-Reordering, non-Early Write Acknowledgement) — strongly-ordered, uncacheable. This ensures device register accesses are not reordered or cached.

**Step 8: Physical page allocator.** Initialize a buddy allocator using the free physical pages from the UEFI memory map. Pages of type `Conventional`, `LoaderCode`, `LoaderData`, `BootServicesCode`, and `BootServicesData` are added to the free pool. Pages occupied by the kernel, initramfs, BootInfo, UEFI Runtime, and MMIO are excluded.

The buddy allocator manages pages in orders 0 through 10 (4 KiB to 4 MiB blocks). Allocation and deallocation are O(log n) in the number of orders.

**Step 9: Kernel heap.** Initialize a slab allocator on top of the buddy allocator. The slab allocator provides `alloc::alloc::GlobalAlloc` — from this point, `Box`, `Vec`, `String`, `HashMap`, and all other heap types work. The slab allocator has size classes: 32, 64, 128, 256, 512, 1024, 2048, 4096 bytes. Larger allocations go directly to the buddy allocator.

**Step 10: Hardware RNG.** Call `platform.init_rng(dt)` to initialize the hardware random number generator. On QEMU this is VirtIO-RNG (virtqueue-based, entropy from host `/dev/urandom`). On Pi 4/5 this is the bcm2835-rng (MMIO TRNG). The returned `RngDevice` is stored in `KernelState.rng` and provides runtime entropy for KASLR, capability token generation, nonces, and key derivation — replacing reliance on the one-shot 32-byte UEFI seed.

**Step 11: KASLR.** Use the hardware RNG to compute a random kernel base offset (aligned to 2 MiB). Remap the kernel at the new virtual address. Update all absolute address references (the kernel is compiled as position-independent). This makes kernel address prediction harder for exploits. Falls back to `BootInfo.rng_seed` if the hardware RNG init failed (should not happen on supported platforms).

**Step 12: Capability manager.** Create the root capability — the single capability from which all others are derived:

```rust
pub struct CapabilityManager {
    root: CapabilityToken,
    token_table: HashMap<TokenId, CapabilityToken>,
    next_id: AtomicU64,
}

impl CapabilityManager {
    fn bootstrap() -> Self {
        let root = CapabilityToken {
            id: TokenId(0),
            capability: Capability::Root,     // can derive any capability
            holder: AgentId::KERNEL,
            granted_by: Identity::Kernel,
            created_at: Timestamp::BOOT,
            expires: Timestamp::MAX, // Trust Level 0: Duration::MAX (security.md §10.3)
            delegatable: true,
            attenuations: vec![],
            revoked: false,
            parent_token: None,
            usage_count: 0,
            last_used: Timestamp::BOOT,
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

**Step 13: IPC subsystem.** Initialize the endpoint table and message buffer pools. No channels exist yet — they'll be created when the Service Manager spawns services. But the infrastructure must be ready.

**Step 14: Audit log.** Initialize a kernel ring buffer (64 KiB, circular) for audit events. During early boot, events are buffered here. Once Space Storage is available (Phase 1 of the Service Manager), the ring buffer is flushed to `system/audit/boot/` and subsequent events are written to space storage in real time.

**Step 15: Process manager.** Initialize the process table and scheduler. The scheduler uses four scheduling classes — RT (EDF for compositor/audio deadlines), Interactive (priority round-robin with input boost), Normal (Weighted Fair Queuing for agents and inference), and Idle (FIFO for background maintenance). See scheduler.md §3.1. The kernel itself runs as process 0.

**Step 16: Provenance chain.** Initialize the append-only Merkle-linked provenance log. The first entry records the kernel boot event, signed by the kernel's built-in key. All subsequent system events (service start, capability grant, agent spawn) are appended to this chain.

**Step 17: Early boot complete.** All kernel subsystems are initialized. The kernel is ready to create userspace processes.

```
[boot] Complete — 180ms
[boot] Memory: 3847 MiB free of 4096 MiB total
[boot] Kernel: 1.4 MiB (text: 680 KiB, data: 720 KiB)
[boot] Launching Service Manager...
```

### 3.4 PL011 UART for Early Debug

The PL011 UART is the first and last resort for debugging. It's initialized before anything else (after exception vectors) and remains available even after the display subsystem takes over. On Pi hardware, it's accessible via GPIO pins 14/15 (or the dedicated UART header on Pi 5).

The UART is configured at 115200 baud, 8N1, no flow control. The kernel's `kprintln!()` macro writes directly to the UART data register. In early boot (before the heap exists), formatting uses a small fixed buffer on the stack.

During normal operation, the UART is used by the recovery shell (see Section 9). In production builds, kernel log output to UART can be disabled via the command line (`quiet` flag in `boot.cfg`).

### 3.5 SMP Boot: Secondary CPU Bringup

The entire early boot sequence (Steps 1–17) runs on the boot CPU (core 0) only. Secondary cores are parked by firmware in a WFI (Wait For Interrupt) loop. They are brought online after the Process Manager and Scheduler are initialized (after Step 15), but before the Service Manager is launched.

**Why not earlier?** Secondary cores need working page tables (Step 7), a heap (Step 9), and a scheduler (Step 15). Bringing them online before these exist would require separate bootstrap stacks and synchronization primitives that add complexity with no benefit — there's nothing for them to do during single-threaded init.

**PSCI (Power State Coordination Interface):** AIOS uses the ARM PSCI interface to bring secondary cores online. PSCI is provided by firmware (EL3 on real hardware, or the hypervisor on QEMU). The kernel discovers the PSCI conduit (HVC or SMC) from the device tree `/psci` node.

```rust
pub fn bring_secondary_cpus_online(dt: &DeviceTree, scheduler: &Scheduler) {
    let psci_method = dt.psci_conduit(); // HVC on QEMU, SMC on Pi
    let cpu_nodes = dt.cpu_nodes();      // one per core

    for (i, cpu) in cpu_nodes.iter().enumerate().skip(1) {
        // Skip core 0 (boot CPU, already running)
        let mpidr = cpu.mpidr();

        // Allocate a per-core kernel stack (16 KiB)
        let stack = alloc_kernel_stack(SECONDARY_STACK_SIZE);

        // Set up a trampoline: the secondary core will jump here
        // and find its stack pointer, page table, and entry function.
        let trampoline = SecondaryTrampoline {
            stack_top: stack.top(),
            page_table: kernel_page_table_phys(),
            entry: secondary_cpu_entry as usize,
            core_id: i,
        };
        SECONDARY_TRAMPOLINES[i].store(trampoline);

        // PSCI CPU_ON: wake the core
        // target_cpu: MPIDR of the core to wake
        // entry_point: physical address of trampoline code
        // context_id: index into SECONDARY_TRAMPOLINES
        psci_cpu_on(psci_method, mpidr, secondary_trampoline_phys(), i);

        kprintln!("[boot] CPU {} online (MPIDR: {:#x})", i, mpidr);
    }

    // Wait for all secondaries to check in
    while ONLINE_CPU_COUNT.load(Ordering::Acquire) < cpu_nodes.len() {
        core::hint::spin_loop();
    }
    kprintln!("[boot] All {} CPUs online", cpu_nodes.len());
}

/// Entry point for secondary CPUs after trampoline sets up stack and MMU.
fn secondary_cpu_entry(core_id: usize) {
    // Install this core's exception vectors
    write_vbar_el1(exception_vectors_phys());

    // Enable this core's GIC redistributor (GICv3) or CPU interface (GICv2)
    enable_gic_for_core(core_id);

    // Enable the timer interrupt for this core (scheduler tick)
    enable_timer_interrupt();

    // Register this core with the scheduler
    scheduler_register_core(core_id);

    // Signal that this core is online
    ONLINE_CPU_COUNT.fetch_add(1, Ordering::Release);

    // Enter the scheduler idle loop — this core will pick up
    // work when the Service Manager starts spawning services.
    scheduler_idle_loop();
}
```

**Per-platform core counts:**

```
Platform            Cores   PSCI Conduit    Notes
──────────────────────────────────────────────────────────
QEMU (default)      4      HVC             Configurable via -smp
Raspberry Pi 4      4      SMC             Cortex-A72
Raspberry Pi 5      4      SMC             Cortex-A76
```

**The `maxcpus=` command line option** limits how many secondary cores are brought online. `maxcpus=1` keeps the system single-core (useful for debugging race conditions). Default is all available cores.

**Timing:** Secondary CPU bringup takes ~5ms total (PSCI call + per-core init). It happens between Step 15 (Process Manager) and Step 17 (Early boot complete). By the time the Service Manager launches, all cores are online and the scheduler can distribute work across them.

### 3.6 SMMU / IOMMU: DMA Protection

Without an IOMMU, any DMA-capable device can read or write arbitrary physical memory — effectively bypassing all kernel page table isolation. On a capability-based OS, this is a critical gap: a compromised USB or network device could read kernel memory, steal capability tokens, or corrupt the provenance chain.

**ARM SMMU (System Memory Management Unit)** provides per-device address translation and access control for DMA transactions, analogous to Intel VT-d:

```
Without SMMU:
  Device → DMA request (physical address) → RAM (any address!)

With SMMU:
  Device → DMA request (IOVA) → SMMU → translate via device page table
                                       → check permissions
                                       → physical address (restricted)
                                       → RAM (only allowed regions)
```

**Per-platform status:**

```
Platform       SMMU Hardware          Status
──────────────────────────────────────────────────
QEMU           VirtIO IOMMU           Optional; enabled with -device virtio-iommu
               (or iommu=smmuv3)      Required for testing DMA isolation.
Pi 4           None                   No SMMU. DMA is unrestricted.
                                      Mitigation: restricted device drivers,
                                      bounce buffering for untrusted devices.
Pi 5           SMMU (in BCM2712)      Available. Configured during boot.
```

**When SMMU is initialized:** After Step 8 (page allocator ready — SMMU page tables need physical pages) but before the Service Manager launches any device-accessing services. Specifically:

1. **Step 8.5 (new, between page allocator and heap):** If the device tree contains an SMMU node (`/smmu` or `/iommu`), initialize the SMMU hardware: program the Stream Table (maps device stream IDs to per-device page tables), configure the Command Queue and Event Queue, and enable translation.
2. **Per-device page tables** are created when device drivers initialize. Each device's DMA is restricted to specific physical regions: the Block Engine can DMA to/from its I/O buffers, but not to kernel text or capability tables.
3. **On Pi 4 (no SMMU):** The kernel uses *bounce buffers* — all DMA goes through a dedicated physical region, and the kernel copies data in/out. This is slower but safe. Drivers for untrusted buses (USB) always use bounce buffers regardless of SMMU presence.

```rust
pub struct SmmuConfig {
    /// Physical base address of SMMU registers (from device tree)
    base: PhysicalAddress,
    /// Stream table: maps StreamId → device context (page table, config)
    stream_table: &'static mut [StreamTableEntry],
    /// Command queue for SMMU configuration commands
    cmd_queue: CommandQueue,
    /// Event queue for SMMU faults (DMA access violations)
    event_queue: EventQueue,
}

pub fn init_smmu(dt: &DeviceTree, page_allocator: &BuddyAllocator) -> Option<SmmuConfig> {
    let smmu_node = dt.find_compatible("arm,smmu-v3")?;
    let base = smmu_node.reg_base();
    // ... configure stream table, queues, enable translation
    Some(config)
}
```

**SMMU faults** (device tries to DMA to an unauthorized address) are logged as audit events and the offending transaction is aborted. The kernel does not crash — the device driver receives an I/O error, and the Service Manager may restart the affected service.

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

**Cross-phase dependencies:** Each phase requires all services from every previous phase to be healthy before it starts. The graph below shows only intra-phase dependencies. Cross-phase dependencies are implicit: every Phase 2 service depends on `space_storage` (Phase 1), every Phase 3 service depends on Phase 2 core services (specifically `space_storage` for persistent state and `compositor` for display access where needed), and so on. Phase 3 (AI) and Phase 4 (User) run in parallel after Phase 2 completes — they depend on Phase 2 but not on each other (see §6.1 timeline).

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
║  PHASE 3 + PHASE 4: CONCURRENT after Phase 2 completes            ║
║  (Phase 3 is non-critical; Phase 4 does not wait for Phase 3)     ║
╠═══════════════════════════╦═══════════════════════════════════════╣
║  PHASE 3: AI SERVICES     ║  PHASE 4: USER SERVICES               ║
║  (non-critical path)      ║  (critical path continues)            ║
╠═══════════════════════════╬═══════════════════════════════════════╣
║                            ║                                       ║
║  airs_core ──→ space_indexer║  identity_service                     ║
║      │                     ║       │                                ║
║      └──→ context_engine   ║       ▼                                ║
║                            ║  preference_service                    ║
║  (5-second timeout: if     ║       │                                ║
║   AIRS not healthy by then,║       ├──→ attention_manager           ║
║   Phase 5 proceeds without ║       │                                ║
║   it; AIRS loads in bg)    ║       └──→ agent_runtime               ║
║                            ║                                       ║
╠═══════════════════════════╩═══════════════════════════════════════╣
║  PHASE 5: EXPERIENCE (starts when Phase 4 completes)              ║
║  (Phase 3 may still be loading — that's OK)                       ║
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

### 4.8 Service Discovery

When the Service Manager spawns a service, it creates IPC channels connecting that service to its dependencies. But a newly started service needs to know *which* channel connects to *which* dependency. This is the service discovery problem.

**Solution: `ServiceBootInfo` channel table.** Each service receives a `ServiceBootInfo` structure (passed via `x0`, just like the kernel passes `BootInfo` to the Service Manager). It contains the service's capability tokens and a table mapping `ServiceId` → `ChannelId`:

```rust
pub struct ServiceBootInfo {
    /// This service's identity
    service_id: ServiceId,

    /// Capability tokens granted to this service
    capabilities: Vec<CapabilityToken>,

    /// Channel table: maps dependency ServiceId to the ChannelId
    /// for communicating with that dependency.
    /// Example: space_storage's table contains:
    ///   { ObjectStore → ChannelId(7), AuditLog → ChannelId(12) }
    channels: HashMap<ServiceId, ChannelId>,

    /// Channel for receiving health checks from Service Manager
    health_channel: ChannelId,

    /// Channel for sending lifecycle events to Service Manager
    lifecycle_channel: ChannelId,
}
```

**How it works:**

1. Service Manager creates all IPC channels before starting the service.
2. Each channel has two endpoints — one for the new service, one for the existing dependency.
3. The dependency's endpoint is delivered to the already-running service via a `NewPeer` message on its lifecycle channel.
4. The new service's endpoints are all packed into `ServiceBootInfo.channels`.
5. The service looks up a dependency by `ServiceId` and gets back a `ChannelId` — no runtime discovery needed.

**Late discovery:** If a service starts after its dependents (e.g., AIRS starts late due to the 5-second timeout), the Service Manager sends `ServiceAvailable { id, channel }` messages to all services that declared a soft dependency on it. Those services can then establish communication. This is how the Attention Manager picks up AIRS after boot.

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

**Input Subsystem** registers with the framework and starts handling keyboard and mouse/touchpad events. On QEMU, this is VirtIO-Input (paravirtualized, no USB stack needed). On Pi, input requires the USB host controller:

```
Pi 4/5 Input path:
  1. USB host controller init (DesignWare xHCI on Pi 4, RP1 xHCI on Pi 5)
     - Controller is discovered from Device Registry (device tree node)
     - xHCI rings allocated from kernel DMA-safe memory (bounce buffer
       region on Pi 4 where there's no SMMU; SMMU-mapped on Pi 5)
     - Controller reset, port power-on, initial hub enumeration
  2. USB hub enumeration
     - Pi 4: integrated VL805 USB 3.0 hub (4 ports)
     - Pi 5: RP1 southbridge (4 USB ports, 2× USB 3.0 + 2× USB 2.0)
     - Enumerate all connected devices, match USB class codes
  3. USB HID driver
     - Claim keyboard (class 0x03, subclass 0x01, protocol 0x01)
     - Claim mouse/touchpad (class 0x03, subclass 0x01, protocol 0x02)
     - Set up interrupt transfers for input polling
  4. Route events to compositor's input router

Timing: USB enumeration takes 50-200ms (device-dependent).
If USB fails on Pi, keyboard/mouse are unavailable — this is a
Phase 2 critical failure on Pi (but not on QEMU, which uses VirtIO-Input).
```

**Display Subsystem** initializes the GPU driver. On QEMU, this is VirtIO-GPU: the driver negotiates display resolution, allocates scanout buffers, and sets up the rendering pipeline via wgpu. On Pi, this is the VC4/V3D driver (Pi 4) or V3D 7.1 (Pi 5), which provides Vulkan capabilities. The display subsystem takes over from the early framebuffer (see Section 7 for the handoff).

**GPU memory on Pi:** VideoCore VI/VII shares system RAM with the CPU — there is no discrete VRAM. The Pi firmware reserves a contiguous region for the GPU (specified in `config.txt` as `gpu_mem`, default 76 MB on Pi 4, 64 MB on Pi 5). The kernel discovers this reservation via the device tree `/reserved-memory` node during Step 4 and excludes it from the buddy allocator. The Display Subsystem uses this region for scanout buffers, texture memory, and render targets. Importantly, the AIRS model selection thresholds (§5 Phase 3) account for GPU-reserved memory — "available RAM" means total RAM minus kernel minus GPU reservation:

```
Pi 4 (4 GB model):  4096 - ~2 (kernel) - 76 (GPU) = ~4018 MB available
                     → selects 3B Q4_K_M (~2.0 GB)
Pi 4 (8 GB model):  8192 - ~2 (kernel) - 76 (GPU) = ~8114 MB available
                     → selects 8B Q4_K_M (~4.5 GB)
Pi 5 (8 GB):        8192 - ~2 (kernel) - 64 (GPU) = ~8126 MB available
                     → selects 8B Q4_K_M (~4.5 GB)
QEMU (default 4 GB): no GPU reservation (VirtIO-GPU uses host memory)
                     → selects 3B Q4_K_M (~2.0 GB)
```

**Compositor** starts after display. It creates the initial render pipeline, registers with the input subsystem for event routing, and presents the first composited frame. At this point, the splash screen transitions from the early framebuffer to the compositor.

**Network Subsystem** starts in parallel with display/compositor. It initializes the network stack (smoltcp), configures the network interface (VirtIO-Net on QEMU, Genet Ethernet on Pi), and starts DHCP. Basic TCP/IP is available from this point — but the full Network Translation Module (space resolver, shadow engine, etc.) comes later (Phase 16 in the development plan).

**POSIX Compatibility** starts in parallel with other Phase 2 services. It initializes the translation layer: mounts the POSIX filesystem view over spaces (`/spaces/`, `/home/`, `/tmp/`, `/dev/`, `/proc/`), sets up the C library (musl libc) shim, and makes BSD tools available.

**Audio Subsystem** starts in parallel with network and POSIX. It registers with the Subsystem Framework and initializes the audio hardware: VirtIO-Sound on QEMU, or PWM/I2S via the BCM audio peripheral on Pi 4/5 (accessed through the DMA controller, PL330 on Pi 4, RP1 on Pi 5). The audio subsystem is **not critical** — if it fails, boot continues without sound. It provides: PCM output (mixing engine), optional input (microphone), and routing (HDMI audio vs 3.5mm jack vs Bluetooth). The scheduler grants audio threads RT class scheduling (same as compositor) to meet latency deadlines (scheduler.md §3.1).

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

**Attention Manager** starts after Preferences (see [attention.md](../intelligence/attention.md) for the full attention model). It initializes the notification pipeline, loads attention rules from preferences, and begins accepting notifications from other services. AIRS is a soft dependency: if available, the Attention Manager enables AI-powered triage; otherwise, it uses rule-based triage. This is why the Attention Manager is in Phase 4 (not Phase 3) — it must not block on AIRS loading.

**Agent Runtime** starts last in this phase. It initializes the agent sandbox infrastructure, loads the list of approved agents from `system/agents/`, and prepares to spawn agents on request. It does not spawn agents yet — that happens in Phase 5.

**Phase 4 budget: ~200ms.**

### Phase 5: Experience

The final phase makes the system user-facing.

**Workspace** renders the home view. It queries the Agent Runtime for active agent tasks, Space Storage for recent spaces, and the Attention Manager for the notification digest. The first frame of the Workspace is the "boot complete" moment — the user sees a usable desktop.

**Conversation Bar** initializes. If AIRS is available, it's fully functional. If AIRS is still loading, it shows a subtle "AI loading..." indicator and disables natural language features until AIRS reports healthy. Keyword search (via the full-text index) works immediately.

**Autostart Agents** are spawned. Any agents marked as autostart in the user's preferences are launched by the Agent Runtime. These are lightweight agents the user wants always running — a music agent, a backup agent, etc.

**Boot Complete Signal:** The Service Manager records the total boot time in `system/audit/boot/` and logs it to UART:

```
[boot] Phase 5 complete — boot to desktop in 1,847ms
[boot] Services: 18 running, 0 failed, 0 degraded
[boot] AIRS: healthy (model: llama-3.1-8b-q4_k_m, loaded in 3,200ms)
```

**Phase 5 budget: ~300ms (first frame).**

### First Boot Experience

Design principle 7 says "first boot and normal boot are the same code path." The code path is the same — but what the user *sees* is different, because there's no identity, no preferences, and no spaces yet.

**What's different on first boot:**

| Phase | Normal boot | First boot |
|---|---|---|
| Phase 1 | Verify existing spaces | **Format storage, create system spaces** (~200ms extra) |
| Phase 2 | Normal startup | Same (compositor, input ready) |
| Phase 3 | Load existing model | Same (or skip if no model pre-loaded in `system/models/`) |
| Phase 4 Identity | Unlock with stored passphrase/biometric | **Setup flow: create new identity** |
| Phase 4 Preferences | Load from space | Use defaults |
| Phase 5 | Desktop with recent spaces | **Empty desktop → setup flow overlay** |

**The setup flow** is a compositor overlay rendered by the Identity Service in coordination with the Workspace. It is *not* a separate installer binary — it runs inside the normal service pipeline, using the same compositor, input subsystem, and IPC channels as any other UI.

```
First Boot Setup Flow (compositor overlay):

1. Language & Locale Selection
   - Grid of language options, keyboard layout detection
   - Selected via touchscreen, mouse, or keyboard arrows
   - This sets initial preferences (written to user/preferences/ when created)

2. Passphrase Creation
   - "Create a passphrase to protect your data"
   - Min 8 characters, strength meter
   - This passphrase derives the master encryption key for user spaces
   - Optionally: connect hardware security key (USB)
   - On Pi 5: optionally enable fingerprint (if USB reader present)

3. Wi-Fi Configuration (if network not already connected)
   - Scan for networks, select, enter password
   - Skippable — AIOS is fully functional offline
   - If connected: time sync via NTP (see §6.5)

4. AIRS Model Selection (if not pre-loaded)
   - "AIOS includes a local AI assistant. Choose a model:"
   - Options based on available RAM (same thresholds as §5 Phase 3)
   - Download starts in background if network available
   - Skippable — AIOS works without AIRS (conversation bar degraded)

5. Complete
   - Identity created, user space created, preferences written
   - Setup overlay fades out, Workspace renders home view
   - First provenance entry for user identity recorded
```

**Timing:** The setup flow adds user-wait time (passphrase typing, Wi-Fi selection) but no extra code path. Once complete, the system is in the same state as a normal boot — identity unlocked, preferences loaded, Workspace visible. Subsequent boots skip the setup flow entirely because `system/identity/` already exists.

**Headless first boot (no display):** If no framebuffer is available (UEFI GOP absent), the setup flow runs on UART. The Identity Service detects the absence of the compositor and falls back to a text-mode setup. This is primarily for development (QEMU `-nographic` mode).

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
 645          Hardware RNG             RngDevice initialized, entropy available
 650          KASLR                    Kernel address randomized
 660          Core subsystems          Cap mgr, IPC, audit, procmgr
 665          SMP bringup              Secondary CPUs online via PSCI (~5ms)
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

### 6.5 Time and Timestamps

**Before NTP:** From kernel entry until the Network Subsystem obtains an NTP response, all timestamps are *monotonic, relative to boot*. The ARM Generic Timer counter starts at an arbitrary value set by firmware; the kernel normalizes this to `0 = kernel entry`. All audit log entries, provenance records, and boot timing measurements use this monotonic counter.

UEFI Runtime Services provide `GetTime()` which returns a wall-clock time from the platform's RTC (Real-Time Clock). On QEMU, this is the host's wall clock. On Pi 4/5, this is the hardware RTC if a battery-backed module is attached, or epoch (January 1, 2000) if not. The kernel reads UEFI `GetTime()` once during early boot (after Step 6, timer setup) and stores it as `boot_wall_time` in `KernelState`. This provides a *best-effort* wall-clock time that may be inaccurate but is not zero.

**NTP sync:** The Network Subsystem initiates an NTP query as one of its first actions after DHCP completes (Phase 2). When the NTP response arrives:

1. Compute the delta between NTP time and `boot_wall_time + elapsed_monotonic`.
2. Store the delta in `KernelState.ntp_offset`.
3. From this point, `wall_time() = boot_wall_time + elapsed_monotonic + ntp_offset`.
4. Retroactively patch audit log entries? **No.** Audit entries keep their original monotonic timestamps. The NTP offset is recorded as a single audit event: `NtpSync { offset_ms: i64 }`. Log readers apply the offset when displaying wall-clock times.

**If NTP never arrives** (offline system), wall-clock time is derived from the UEFI RTC. If the RTC has no battery (common on Pi 4), times are wrong but monotonically increasing — which is sufficient for audit ordering and provenance chain integrity.

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

## 8. Kernel Panic Handler

When the kernel encounters an unrecoverable error — double fault, assertion failure, out-of-memory with no recourse, or an explicit `panic!()` — the panic handler takes over. Its job: capture maximum diagnostic information and persist it, even if the heap, storage, or display subsystem is broken.

### 8.1 What Gets Captured

```rust
#[repr(C)]
pub struct PanicDump {
    magic: u64,                             // 0x41494F53_50414E43 ("AIOSPAN C")
    boot_phase: EarlyBootPhase,             // how far boot got before panic
    timestamp: u64,                         // timer counter value
    panic_message: [u8; 512],               // truncated panic!() message
    cpu_id: usize,                          // which core panicked

    // Full register state at point of panic
    registers: RegisterDump,

    // Exception context (if panic was triggered by an exception)
    exception: Option<ExceptionInfo>,

    // Stack trace: return addresses from the stack
    backtrace: [u64; 32],                   // up to 32 frames
    backtrace_depth: usize,

    // Last 16 KiB of the kernel log ring buffer
    log_tail: [u8; 16384],
    log_tail_len: usize,
}

pub struct RegisterDump {
    x: [u64; 31],                           // x0–x30
    sp: u64,
    pc: u64,
    pstate: u64,                            // CPSR/SPSR
    esr_el1: u64,                           // Exception Syndrome Register
    far_el1: u64,                           // Fault Address Register
    elr_el1: u64,                           // Exception Link Register
}
```

### 8.2 Persistence Strategy

The panic handler cannot assume the heap, Space Storage, or even the Block Engine are functional. It uses a layered persistence strategy — try the best option, fall back:

```
Persistence priority (try in order):

1. Reserved panic region on block device
   - A 64 KiB region at a fixed LBA (right after the superblock)
   - Written via raw block I/O (direct MMIO to the storage controller)
   - Does NOT go through the Block Engine, Object Store, or WAL
   - Works even if the entire storage stack is corrupt
   - On next boot, the Block Engine reads this region and copies
     the dump to system/crash/ if Space Storage is functional

2. UEFI Runtime Variable
   - If raw block I/O fails (storage hardware dead)
   - Write a truncated dump (< 1 KiB: message, registers, PC, ESR)
     to a UEFI variable via Runtime Services
   - Survives power cycle (stored in SPI flash / NVRAM)
   - Limited size, but captures the most critical info

3. UART only
   - If both of the above fail
   - Dump everything to UART (serial console)
   - Requires a connected terminal to capture output
   - Always attempted regardless of other persistence
```

### 8.3 Panic Display

If the early framebuffer is available (before compositor handoff) or can be reclaimed (after compositor, by reverting to the GOP framebuffer):

```
┌──────────────────────────────────────────────────┐
│                                                    │
│              AIOS KERNEL PANIC                     │
│                                                    │
│  Phase: HeapReady                                  │
│  Message: out of memory: buddy allocator           │
│           exhausted during slab refill             │
│                                                    │
│  PC:  0xffff0000_00042a8c                          │
│  ESR: 0x00000000_96000045 (data abort, current EL) │
│  FAR: 0x00000001_80000000                          │
│                                                    │
│  Backtrace:                                        │
│    #0 0xffff0000_00042a8c slab_alloc+0x1c          │
│    #1 0xffff0000_00038f10 box_new+0x28             │
│    #2 0xffff0000_0001cd44 capability_create+0x54   │
│    ...                                             │
│                                                    │
│  Crash dump saved to block device.                 │
│  System will reboot in 10 seconds.                 │
│  (Press any key for UART debug shell)              │
│                                                    │
└──────────────────────────────────────────────────┘
```

The panic screen is rendered with the same `EarlyFramebuffer` code used for the splash screen — direct pixel writes, no GPU driver, no heap allocation. The font is a compiled-in 8x16 bitmap font, not the TTF fonts from the initramfs.

### 8.4 Multi-Core Panic

If one core panics, it must stop the others:

1. Panicking core sets a global `PANIC_FLAG` (atomic store, `Ordering::Release`).
2. Panicking core sends an SGI (Software Generated Interrupt) to all other cores.
3. Other cores receive the SGI, check `PANIC_FLAG`, and enter a `WFI` loop.
4. Panicking core now has exclusive access to UART, framebuffer, and block device.
5. Dump proceeds single-threaded.

If the panicking core is unable to send SGIs (GIC not initialized yet), the other cores are still in their PSCI WFI loop from firmware and won't interfere.

-----

## 9. Recovery Mode

### 9.1 Failure Detection

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

### 9.2 Recovery Shell

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

### 9.3 Safe Mode

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

### 9.4 Rollback

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

### 9.5 Factory Reset

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

### 9.6 OTA Updates

The A/B rollback mechanism (§9.4) protects against bad updates, but this section describes how updates arrive on the ESP in the first place.

**Update delivery:** A system update is a signed archive containing any combination of: a new kernel ELF, a new initramfs, and new Phase 3-5 service binaries. Updates are fetched by the Network Translation Module (when available) from a configured update endpoint, or applied manually from a USB drive.

```
Update flow:

1. Update agent (Phase 5 background agent) checks for updates
   - Fetches update manifest from configured endpoint (HTTPS)
   - Compares manifest version against current version
   - If newer: downloads update archive to a temporary space

2. Signature verification
   - Archive is signed with AIOS release key (Ed25519)
   - Public key is compiled into the kernel (immutable)
   - If signature fails: discard archive, log audit event, done

3. Stage the update (while system is running)
   - Mount ESP (FAT32) via the Block Engine's ESP access path
   - Rename current kernel → aios.elf.prev
   - Rename current initramfs → initramfs.cpio.prev
   - Write new kernel → aios.elf
   - Write new initramfs → initramfs.cpio
   - Sync ESP

4. Stage service updates
   - New Phase 3-5 service binaries → system/services/ space
   - Old binaries are retained as previous versions (Object Store
     keeps them as content-addressed objects; the old content hashes
     remain valid until garbage collection)

5. Trigger reboot (user-confirmed or automatic at next idle period)
   - Boot counter is NOT reset — the new kernel must reach Phase 5
   - If the new kernel fails 3 times, rollback to .prev (§9.4)

6. Post-update verification
   - New kernel boots, reaches Phase 5, clears boot counter
   - Update agent records successful update in system/audit/
   - .prev files remain on ESP as rollback targets
```

**ESP write access:** Only the update agent and the recovery shell can write to the ESP. The ESP is not mounted during normal operation. Write access requires a `EspWriteAccess` capability that the Service Manager mints only for the update agent.

**Manual updates (USB):** Plug in a USB drive with a signed update archive at `/aios-update/`. The update agent detects it (via the device registry) and follows the same verification and staging flow. This works even without network.

-----

## 10. Initramfs and System Image

### 10.1 What's in the Initramfs

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

### 10.2 Boot Image Format

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

### 10.3 Transition from Initramfs to System Space

Once Space Storage is running (end of Phase 1), services can be loaded from the persistent `system/services/` space instead of the initramfs. This transition matters for Phase 3-5 services:

```
Phase 1-2 services:  loaded from initramfs (in memory, fast)
Phase 3-5 services:  loaded from system/services/ space (persistent storage)
```

The distinction matters because Phase 3-5 services can be updated independently of the kernel. Updating AIRS doesn't require a new initramfs — just update the binary in `system/services/`. The initramfs contains only the minimum needed to bootstrap storage and core services.

On first boot, the Service Manager copies Phase 3-5 service binaries from the initramfs to `system/services/`. On subsequent boots, it loads from the space. If a service binary in the space is corrupt, it falls back to the initramfs copy.

-----

## 11. Shutdown and Reboot

### 11.1 Graceful Shutdown Sequence

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

### 11.2 Forced Shutdown

If graceful shutdown takes longer than 10 seconds, the kernel forces the issue:

```
 0s    Graceful shutdown begins
 5s    Services still running → warning logged
 8s    Remaining services receive SIGKILL
10s    Force: storage flush (WAL commit), then power off
       No data loss thanks to WAL, but state may be incomplete
```

The watchdog timer (ARM Generic Timer) is set to 15 seconds at shutdown start. If the kernel hangs during shutdown, the hardware watchdog forces a reset. On the next boot, the WAL replay recovers any incomplete writes.

### 11.3 Agent State Persistence

Agents that need to survive reboot set `persistent: true` in their manifest (see [agents.md](../applications/agents.md) §2.4 `AgentManifest` and §3 Agent Lifecycle). Their state is stored in their designated space:

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

## 12. Implementation Order

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

## 13. Boot Test Strategy

The boot sequence is the most critical code path in AIOS — if it breaks, nothing works. Every change to boot-related code must be validated by automated tests before merging.

### 13.1 CI Boot Smoke Test

Every PR runs a QEMU boot smoke test:

```
Boot smoke test (runs in CI on every PR):

1. Build kernel + initramfs + UEFI stub
2. Launch QEMU (aarch64, no KVM, 4 GB RAM, VirtIO devices)
3. Capture UART output
4. Assert: "[boot] Complete" appears within 500ms (kernel early boot)
5. Assert: "Phase 1 complete" appears within 1000ms
6. Assert: "Phase 2 complete" appears within 2000ms
7. Assert: "Phase 5 complete — boot to desktop" appears within 5000ms
8. Assert: no "[PANIC]" in UART output
9. Assert: "Services: N running, 0 failed, 0 degraded" (0 failures)
10. Shutdown cleanly, verify "[shutdown] clean shutdown" in UART

Total CI time: ~10 seconds per run (dominated by QEMU startup)
```

### 13.2 Platform Test Matrix

```
Test Level     QEMU (CI)       Pi 4 (manual/nightly)  Pi 5 (manual/nightly)
──────────────────────────────────────────────────────────────────────────────
Normal boot    Every PR        Nightly                 Nightly
First boot     Every PR        Weekly                  Weekly
Recovery mode  Every PR        Monthly                 Monthly
Rollback       Every PR        Monthly                 Monthly
Safe mode      Every PR        Monthly                 Monthly
SMP (4 cores)  Every PR        Nightly                 Nightly
maxcpus=1      Weekly          Monthly                 Monthly
```

Pi testing uses physical hardware connected to a CI runner via serial console (UART) and relay-controlled power for automated reboot. The relay allows hard power-cycle testing — essential for verifying watchdog and WAL recovery paths.

### 13.3 Boot Timing Regression

The CI records Phase 5 completion time from UART output. A **regression threshold** of +10% from the rolling average triggers a warning; +20% blocks the PR. This catches accidental performance regressions (e.g., a new service added to the critical path, or an accidentally-synchronous operation in Phase 2).

```
Tracked metrics (from UART timestamps):
  - Kernel early boot (entry → Complete)
  - Phase 1 duration (storage)
  - Phase 2 duration (core services)
  - Phase 4 duration (user services)
  - Total boot-to-desktop (entry → Phase 5 complete)
  - AIRS health time (Phase 3, non-critical but tracked)
```

### 13.4 Failure Injection Tests

Run weekly in CI (slower, ~60 seconds each):

- **Service crash during boot:** Kill a Phase 2 service mid-startup. Verify: Service Manager restarts it, boot completes, audit log records the failure.
- **AIRS timeout:** Start QEMU with insufficient RAM for any model. Verify: Phase 3 times out, Phase 4-5 proceed, desktop appears without AIRS.
- **Storage corruption:** Corrupt the WAL header before boot. Verify: Block Engine detects corruption, WAL replay recovers, boot completes.
- **Three consecutive failures:** Kill the kernel three times before Phase 5. Verify: Fourth boot enters recovery mode, UART shows recovery shell prompt.
- **Watchdog expiry:** Inject a `sleep(35s)` in Phase 1. Verify: Watchdog fires, system reboots, `consecutive_failures` increments.

-----

## 14. Cross-Document Dependencies

This section tracks concepts that boot.md references which are defined (or need to be defined) in other documents. If you modify any of these, check the corresponding document for consistency.

| Concept used in boot.md | Defined in | What boot.md needs from it |
|---|---|---|
| `Platform` trait, 7 `init_*` methods, `InterruptController`, `Timer`, `Uart`, `GpuDevice`, `NetworkDevice`, `StorageDevice`, `RngDevice` | [hal.md](./hal.md) §2–3 | Device trait signatures must match §2.4 and §3.3 here. Initialization order (UART/interrupts/timer early, GPU/network/storage in service phases) must agree with hal.md §3.2. |
| `Scheduler`, four scheduling classes (RT, Interactive, Normal, Idle), 1ms tick | [scheduler.md](./scheduler.md) §3.1, §10.1 | Timer tick rate (Step 6) and scheduling class names in Step 15 must stay consistent with scheduler.md. |
| `BuddyAllocator`, `SlabAllocator`, slab size classes | [memory.md](./memory.md) | Buddy allocator order range (0–10) and slab size classes (32–4096 bytes) cited in Steps 8–9 must match memory.md. |
| `CapabilityManager`, `CapabilityToken`, root capability, trust levels, `Capability::Root` | [security.md](../security/security.md) §10 | `Timestamp::MAX` for Trust Level 0 tokens (Step 12) and capability delegation model (§4.7) must stay aligned. |
| `IpcSubsystem`, `ChannelId`, health check protocol | [ipc.md](./ipc.md) | Health check message format (§4.4) and Service Manager IPC channels (§4.1 step 5) must match ipc.md's channel semantics. |
| Compositor framebuffer handoff, display subsystem, wgpu pipeline | [compositor.md](../platform/compositor.md) | Handoff sequence (§7.4) and Phase 2 display startup must match compositor.md's initialization. |
| AIRS model selection by RAM, `system/models/` space, GGML runtime, 5-second timeout | [airs.md](../intelligence/airs.md) | Model size thresholds (§5 Phase 3: ≥8 GB → 8B, ≥4 GB → 3B, <4 GB → 1B) and the 5-second health timeout must stay consistent with airs.md's model registry. |
| Identity Service, Ed25519 keypair, `system/identity/` space | [identity.md](../experience/identity.md) | Phase 4 Identity startup and identity unlock flow must match identity.md's key management. |
| Attention Manager, AI triage vs rule-based fallback | [attention.md](../intelligence/attention.md) | The soft AIRS dependency described in Phase 4 must match attention.md's initialization requirements. |
| Context Engine, signal collection, rule-based heuristic fallback | [context-engine.md](../intelligence/context-engine.md) | Phase 3 Context Engine startup and its AIRS dependency must match context-engine.md's fallback behavior. |
| Preference Service, `user/preferences/` space | [preferences.md](../intelligence/preferences.md) | Phase 4 Preference startup and the preference space path must match preferences.md. |
| `AgentManifest.persistent`, agent shutdown protocol, `ShutdownSignal` | [agents.md](../applications/agents.md) §2.4, §3 | The 5-second shutdown grace period (§11.3) and persistent agent relaunching must match agents.md's lifecycle model. |
| Block Engine, Object Store, Space Storage, WAL, LSM-tree, system spaces | [spaces.md](../storage/spaces.md) | Phase 1 startup sequence and system space paths (`system/audit/`, `system/models/`, etc.) must agree with spaces.md's space hierarchy. |
| ARM SMMU (SMMUv3), stream tables, DMA isolation, bounce buffers | [hal.md](./hal.md) | SMMU initialization (§3.6) and per-device DMA page tables must align with hal.md's DMA abstractions. Pi 4 bounce buffer strategy must match hal.md's DMA API. |
| USB host controller (xHCI), USB HID, hub enumeration | [hal.md](./hal.md) | Phase 2 USB input path on Pi must match hal.md's USB abstraction (if defined). xHCI driver is platform-specific (DesignWare on Pi 4, RP1 on Pi 5). |
| Audio subsystem (PCM, mixing, I2S/PWM, HDMI audio) | [compositor.md](../platform/compositor.md) or future `audio.md` | Phase 2 Audio Subsystem startup must match whatever audio document is created. RT scheduling class for audio threads must match scheduler.md. |
| Watchdog timer (SP805, bcm2835-wdt), boot timeout, runtime ping | [hal.md](./hal.md) | Watchdog hardware abstraction and timeout values (30s boot, 60s runtime, 15s shutdown) must be consistent across hal.md and boot.md. |
| GPU memory reservation (`/reserved-memory` node, `gpu_mem`), VideoCore carve-out | [compositor.md](../platform/compositor.md) | GPU memory split on Pi (76 MB Pi 4, 64 MB Pi 5) and its effect on available RAM must match compositor.md's VRAM requirements. |

-----

## 15. Suspend, Resume, and Semantic State

Users rarely cold boot. The daily experience is closing a lid, pressing a power button, or walking away. The system's job is to make returning feel instantaneous and *lossless* — nothing should ever be lost, regardless of how the system went down. AIOS provides four layers of state continuity, from fastest-cheapest to most-resilient:

```
Layer               Resume Time    Survives          State Fidelity
──────────────────────────────────────────────────────────────────────
S3 (Suspend-to-RAM)     < 200ms   Lid close/open    Perfect (RAM powered)
S4 (Hibernate)          ~1.5s     Power loss         Perfect (RAM → disk)
Semantic Resume         ~2.0s     Kernel update      Reconstructed (semantic)
Ambient Continuity     (always on) Crash, panic, fire Continuous (Spaces)
```

### 15.1 Suspend-to-RAM (S3)

The fastest resume path. CPU cores are powered down, DRAM stays in self-refresh, and all device state is saved to kernel memory. On wake, the kernel restores device state and resumes exactly where it left off.

**Suspend sequence:**

```
1. User closes lid / presses power button / idle timeout
     │
     ▼
2. Service Manager receives SuspendRequest
   - Broadcasts SuspendPrepare to all running services (via lifecycle channel)
   - Services have 2 seconds to:
     ├── Flush pending I/O (Space Storage commits dirty buffers)
     ├── Save volatile state (compositor saves scanout buffer hash)
     ├── Park DMA engines (Block Engine quiesces controller)
     └── Reply: SuspendReady
   - Services that don't reply in 2 seconds are force-frozen
     │
     ▼
3. Kernel suspend path
   - Disable all interrupts except wake sources
   - Save per-core state: VBAR_EL1, TTBR0_EL1, SP, callee-saved registers
   - Save GIC state: distributor config, redistributor per-core state
   - Save timer state: CNTV_CTL_EL0, CNTV_CVAL_EL0
   - Call platform.suspend_devices():
     ├── UART: save baud rate config (restore on wake)
     ├── GPU: save mode/scanout state (VirtIO-GPU or VC4/V3D)
     ├── Network: save MAC filters, link state (Genet or VirtIO-Net)
     ├── Storage: controller quiesced (no DMA in flight)
     └── RNG: no state to save
   - Park secondary CPUs via PSCI CPU_SUSPEND
   - Boot CPU enters PSCI SYSTEM_SUSPEND
     │
     ▼
   DRAM in self-refresh. System draws < 50 mW.
```

**Wake sequence:**

```
Wake source fires (lid open, power button, RTC alarm, network wake-on-LAN)
     │
     ▼
1. Firmware restarts boot CPU at the suspend resume entry point
   - NOT the normal boot path — jumps to saved resume address
   - Boot CPU restores: MMU (TTBR1_EL1), stack pointer, exception vectors
     │
     ▼
2. Kernel resume path
   - Restore GIC state (distribupts, redistributor)
   - Restore timer state, re-arm scheduler tick
   - Call platform.resume_devices() (reverse of suspend_devices)
   - Resume secondary CPUs via PSCI CPU_ON (same trampoline as boot)
     │
     ▼
3. Service Manager resumes
   - Broadcasts ResumeNotify to all frozen services
   - Services restore volatile state, re-establish connections
   - Compositor presents the last frame immediately (no re-render needed)
     │
     ▼
4. User sees their desktop — exactly as they left it
   Total resume time: < 200ms (dominated by device re-init)
```

**Wake sources by platform:**

```
Platform    Wake Sources                                Notes
──────────────────────────────────────────────────────────────
QEMU        Keyboard, timer (RTC alarm)                 No lid, no real power mgmt
Pi 4        GPIO (power button), USB (keyboard),        No built-in RTC; external
            Genet (wake-on-LAN), timer (ext RTC)        RTC module needed for timed wake
Pi 5        GPIO (power button), USB, Genet,            Built-in RTC with battery
            RTC (built-in), PCIe wake                   connector — timed wake works
```

**PSCI power states:** ARM PSCI defines power states for suspend. AIOS uses the deepest state that preserves DRAM:

```rust
pub enum SuspendPowerState {
    /// CPU cores off, L2 off, DRAM in self-refresh.
    /// Deepest state that preserves memory. Used for S3.
    DeepSleep,
    /// CPU cores in WFI, L2 retained, DRAM active.
    /// Used for short idle (< 30 seconds). Faster wake (~5ms).
    LightSleep,
}
```

### 15.2 Hibernate (S4)

Hibernate writes the entire system state to persistent storage, then powers off completely. On wake, the state is read back and the system resumes. This survives complete power loss — pull the plug, replace the battery, come back a week later, everything is exactly where you left it.

**Hibernate is S3 with a safety net.** AIOS enters S3 first (fast wake), and starts writing the hibernate image to storage *in the background while the system is suspended*. If power fails during S3 (DRAM loses content), the next boot detects the hibernate image and resumes from it. If S3 wake succeeds normally, the hibernate image is discarded.

```
Suspend Request
  │
  ├── Enter S3 (immediate, < 200ms)
  │     User's screen off, system sleeping
  │
  └── Background: write hibernate image to block device
      (DMA engine runs in sleep, writing DRAM pages to
       a reserved partition: the hibernate image partition)
        │
        ├── Power stays on → S3 wake is used (fast, < 200ms)
        │                     Hibernate image discarded
        │
        └── Power lost → next cold boot detects hibernate image
                          Restore from disk (~1.5s)
```

**Hibernate image format:**

```rust
pub struct HibernateImage {
    magic: u64,                         // 0x41494F53_48494245 ("AIOSHIBE")
    version: u32,                       // format version
    kernel_version: u64,                // must match running kernel
    checksum: [u8; 32],                // SHA-256 of payload
    page_count: u64,                   // number of pages saved
    compressed_size: u64,              // zstd-compressed payload size

    // CPU state for each core
    cpu_states: [CpuSuspendState; MAX_CPUS],

    // Device state snapshots
    device_states: DeviceStateBlock,

    // Compressed memory pages (zstd stream)
    // Only dirty pages are saved — clean pages backed by
    // Space Storage or mmap'd files are not included
    // (they'll be demand-paged from storage on access).
    pages: CompressedPageStream,
}
```

**Key optimization:** Only dirty pages are written. Clean pages (kernel text, mmap'd model weights, read-only space data) are demand-paged from storage on resume. On a system with 4 GB RAM where 1.5 GB is clean file-backed pages, the hibernate image is ~2.5 GB uncompressed, ~1.2 GB compressed. At SD card write speeds (50 MB/s), that's ~24 seconds to write — which is fine because it happens in background during S3.

**Hibernate partition:** A dedicated raw partition on the block device (not a Space — it must be accessible before Space Storage starts). The Block Engine reserves this during first-boot formatting:

```
Block device layout:
  [Superblock] [Panic dump] [Hibernate partition] [WAL] [Main storage]
                              └─ sized to match physical RAM
```

### 15.3 Semantic Resume

This is where AIOS diverges from every other OS.

Traditional hibernate saves raw memory — a perfect snapshot of RAM. But that snapshot is brittle: it's tied to a specific kernel version (data structures must match), specific hardware (device handles are meaningless after a hardware change), and a specific moment (no way to partially resume). If you update the kernel, the hibernate image is invalid. If you move the disk to a different machine, the image is useless.

**Semantic Resume saves meaning, not bits.** Instead of dumping 4 GB of RAM, it captures a compact semantic description of the user's state:

```rust
pub struct SemanticSnapshot {
    /// When this snapshot was taken
    timestamp: Timestamp,

    /// Active workspace layout
    workspace: WorkspaceState,

    /// Open spaces and cursor positions within each
    open_spaces: Vec<OpenSpaceState>,

    /// Active agents and their conversation context
    active_agents: Vec<AgentSessionState>,

    /// Compositor window geometry and z-order
    window_layout: Vec<WindowState>,

    /// Conversation bar state (draft text, history position)
    conversation: ConversationBarState,

    /// Context Engine's last inference (work/play/focus mode)
    context_mode: ContextMode,

    /// Attention Manager's pending notification queue
    pending_notifications: Vec<NotificationState>,

    /// Currently focused element (which window, which field)
    focus: FocusState,

    /// Scroll positions, selection ranges, cursor positions
    /// across all visible content
    view_states: Vec<ViewState>,
}

pub struct OpenSpaceState {
    space_id: SpaceId,
    /// Content hash of the object being viewed/edited
    object_hash: ContentHash,
    /// Cursor/selection within the content
    cursor: CursorState,
    /// Scroll position (normalized 0.0–1.0)
    scroll: f64,
    /// Unsaved edits (stored as a diff against the object)
    pending_edits: Option<EditDiff>,
}

pub struct AgentSessionState {
    agent_id: AgentId,
    /// Conversation history (lightweight: just message IDs referencing Spaces)
    conversation_ref: SpaceObjectRef,
    /// Agent's declared resumable state (agent-specific, opaque to the kernel)
    agent_state: Vec<u8>,
    /// What the agent was doing when suspended
    active_task: Option<TaskDescription>,
}

pub struct WindowState {
    service_id: ServiceId,
    /// Position and size (logical pixels)
    geometry: Rect,
    /// Z-order index
    z_order: u32,
    /// Minimized / maximized / floating
    display_mode: WindowDisplayMode,
    /// Content identity (which space/object/agent this window shows)
    content_ref: ContentReference,
}
```

**When Semantic Resume is used:**

```
Boot starts → check for semantic snapshot in system/session/
  │
  ├── Snapshot exists + kernel version matches → try S4 hibernate first
  │     (hibernate is faster and preserves more state)
  │
  ├── Snapshot exists + kernel version CHANGED → semantic resume
  │     1. Boot proceeds normally through all 5 phases
  │     2. After Phase 5, Service Manager reads semantic snapshot
  │     3. Workspace restores window layout from WindowState entries
  │     4. Spaces are opened and scrolled to saved positions
  │     5. Agents are relaunched and handed their AgentSessionState
  │     6. Conversation bar restores draft text
  │     7. Context Engine adopts saved mode (skips cold inference)
  │     8. Focus is restored to the saved element
  │     User sees their workspace reconstructed in ~500ms after desktop
  │
  └── No snapshot → fresh boot (first boot or after factory reset)
```

**Why this matters:**
- **Kernel updates don't disrupt your session.** Update, reboot, everything is back. No other OS does this.
- **Cross-device continuity.** Copy your Spaces to a new device, boot, and your workspace reconstructs itself. The semantic snapshot travels with your data because it's stored *in* Spaces.
- **Crash recovery.** Even after a kernel panic, the last semantic snapshot (written continuously — see §15.4) restores context.
- **Partial resume.** Semantic Resume can skip stale elements. If an agent was uninstalled since the snapshot, it's silently dropped. If a space was deleted, that window is omitted. The system doesn't crash on stale state — it adapts.

**The semantic snapshot is written to `system/session/` as a Space object.** This means it's versioned, content-addressed, and encrypted (if user spaces are encrypted). The Service Manager writes a new snapshot every 60 seconds during normal operation, and immediately before suspend/shutdown. The overhead is negligible — it's typically < 50 KiB of structured data.

### 15.4 Ambient State Continuity

Semantic Resume captures state every 60 seconds. But what about the 59 seconds between snapshots? If the power cuts 30 seconds after the last snapshot, 30 seconds of work could be lost.

**Ambient State Continuity** is the principle that user-visible state is *continuously* persisted. The system should *never* lose more than a few seconds of user activity, regardless of how it goes down.

This is possible because Spaces already provides content-addressed, versioned storage. The missing piece is making writes *continuous* rather than batched:

```
Traditional OS:
  User types → in-memory buffer → "Save" → disk
  Power loss before save → data lost

AIOS Ambient Continuity:
  User types → in-memory buffer → continuous trickle to Space WAL
  Power loss → WAL replay → at most ~2 seconds of keystrokes lost
```

**Implementation — three tiers:**

**Tier 1: Edit Journal (< 2 second loss window).** Every user input event that modifies content (keystroke, paste, drag, delete) is appended to a per-space *edit journal* in the Block Engine's WAL. The WAL is designed for sequential appends and is fsynced every 2 seconds. On crash, WAL replay reapplies the journal to the last committed object version.

```rust
pub struct EditJournalEntry {
    space_id: SpaceId,
    object_hash: ContentHash,          // base version
    timestamp: Timestamp,
    operation: EditOperation,
}

pub enum EditOperation {
    InsertText { offset: usize, text: String },
    DeleteRange { offset: usize, len: usize },
    ReplaceRange { offset: usize, len: usize, text: String },
    // ... extensible per content type
}
```

**Tier 2: Semantic Snapshot (60-second interval).** The full SemanticSnapshot from §15.3, capturing workspace layout, agent states, and view positions. Written to `system/session/` as a Space object.

**Tier 3: Space Object Commits (application-driven).** Agents and services commit completed units of work to Spaces on their own schedule. A document agent commits after each paragraph. A music agent commits its playlist state after each track change. These are full content-addressed objects with version history.

**On recovery (crash, panic, power loss):**

```
1. Block Engine starts, replays WAL
   → Tier 1 edit journal entries applied to objects
   → At most ~2 seconds of edits lost

2. Space Storage starts, verifies objects
   → Tier 3 committed objects are intact (content-addressed, checksummed)

3. Phase 5 starts, reads semantic snapshot from system/session/
   → Tier 2 workspace layout restored (at most ~60 seconds stale)
   → Window positions may be slightly off; agents may ask
     "Resume from where you left off?" if their state is stale

4. User sees their workspace, with content intact
   → The document they were typing has everything except
     the last ~2 seconds of keystrokes
```

**Cost:** The WAL write overhead for Tier 1 is ~500 bytes per keystroke event, fsynced in batches every 2 seconds. On a 100 WPM typist, that's ~4 KB/s — negligible even on SD cards. The semantic snapshot (Tier 2) is < 50 KiB every 60 seconds. The total overhead of ambient continuity is unmeasurable in normal usage.

### 15.5 Proactive Wake

AIRS observes usage patterns over time: when the user typically wakes the device, how long boot takes, which services and models they use first. Proactive Wake uses this to pre-warm the system *before* the user arrives.

```
Monday–Friday:
  User's alarm is 7:00 AM (calendar event in Spaces)
  User typically opens the laptop at 7:15 AM
  AIRS model load takes ~3 seconds

  → System wakes at 7:12 AM (3 minutes before predicted use)
  → Pre-loads AIRS model into memory (fault in pages from mmap)
  → Warms Space index caches (recent workspaces)
  → Checks for and downloads OTA updates (if idle window)
  → NTP sync (clock may have drifted during sleep)
  → Screen stays off — no power wasted on display
  → When user opens lid at 7:15 → instant response, model warm
```

**How it works:**

```rust
pub struct ProactiveWakeConfig {
    /// Whether proactive wake is enabled (user preference).
    /// Default: on. Can be disabled for power savings.
    enabled: bool,

    /// Minimum confidence before scheduling a proactive wake.
    /// Range: 0.0–1.0. Default: 0.7 (70% confidence).
    confidence_threshold: f32,

    /// How far ahead of predicted use to wake (for pre-warming).
    /// Default: 180 seconds. Adjusted by AIRS based on observed
    /// pre-warm duration (model load time + cache warming time).
    lead_time: Duration,

    /// Maximum time to stay awake if the user doesn't arrive.
    /// Default: 600 seconds (10 minutes). After this, re-suspend.
    max_idle_awake: Duration,

    /// Power source requirement. Default: AcOrBatteryAbove50.
    power_policy: ProactiveWakePowerPolicy,
}

pub enum ProactiveWakePowerPolicy {
    /// Only proactive-wake on AC power
    AcOnly,
    /// AC or battery above threshold
    AcOrBatteryAbove50,
    /// Always (even on low battery)
    Always,
}
```

**Wake scheduling:** The kernel programs the RTC (Pi 5's built-in RTC, or an external RTC module on Pi 4) with a wake alarm. On QEMU, the UEFI RTC is used. The alarm fires, the system resumes from S3, runs the pre-warm tasks with the screen off, then either:
- The user arrives → screen on, instant response
- The timeout expires → re-suspend (cost: a few seconds of power)

**Learning:** AIRS maintains a simple usage model in `system/session/proactive_wake`:

```
Day-of-week × hour-of-day → probability of first interaction
```

A 7×24 grid (168 cells), updated daily with exponential decay. After two weeks of consistent usage, predictions are reliable. No cloud needed — all local.

**Privacy:** Proactive Wake schedules are stored locally in `system/session/` and never leave the device. The usage model is a simple probability grid, not a detailed activity log. The user can inspect and delete it via Preferences.

-----

## 16. Boot Intelligence

### 16.1 Boot Intent Detection

Not every boot should result in a full desktop. AIOS detects *why* it's booting and adapts the service graph accordingly:

```rust
pub enum BootIntent {
    /// Normal boot — user pressed power button or opened lid.
    /// Full service graph: Phases 1-5.
    Interactive,

    /// Resume from suspend — S3 or S4.
    /// Skip Phases 1-5, restore from memory or disk image.
    Resume,

    /// Proactive wake — RTC alarm, no user yet.
    /// Phase 1-2 only. Pre-warm caches. Screen off. Re-suspend after timeout.
    ProactiveWake,

    /// Scheduled task — calendar event, backup schedule, OTA check.
    /// Phase 1-3 only. Run the task, then suspend.
    ScheduledTask { task: ScheduledTaskId },

    /// Recovery — three consecutive boot failures.
    /// Minimal services, UART console.
    Recovery,

    /// Safe mode — user held Shift during boot.
    /// Reduced services, no AIRS, no agents.
    SafeMode,

    /// Update — staged update, need to apply and verify.
    /// Full boot but with update verification on Phase 5 completion.
    Update,

    /// Data transfer — USB device plugged into a powered-off device.
    /// (Pi only: USB-C power + data) Phase 1-2 only, expose storage via USB gadget.
    DataTransfer,
}
```

**How intent is detected:**

```
Boot starts
  │
  ├── UEFI variable: consecutive_failures >= 3?
  │     YES → BootIntent::Recovery
  │
  ├── Hibernate image present with valid kernel version?
  │     YES → BootIntent::Resume (S4)
  │
  ├── S3 resume entry point? (resume from RAM)
  │     YES → BootIntent::Resume (S3)
  │
  ├── RTC alarm triggered? (device tree / UEFI wake source register)
  │     YES → check scheduled_tasks table in system/session/
  │     ├── Proactive wake entry → BootIntent::ProactiveWake
  │     └── Scheduled task entry → BootIntent::ScheduledTask
  │
  ├── Boot command line contains "safe"?
  │     YES → BootIntent::SafeMode
  │
  ├── Staged update detected? (aios.elf is newer than last successful boot)
  │     YES → BootIntent::Update
  │
  └── Default → BootIntent::Interactive
```

**Service graph adaptation:** The Service Manager reads `BootIntent` from `KernelState` and adjusts the phase plan:

```
Intent              Phases Run          Display   AIRS    Network   Services
──────────────────────────────────────────────────────────────────────────────
Interactive         1-5 (full)          On        Yes     Yes       All
Resume              (skipped)           On        Warm    Restore   Restore
ProactiveWake       1-2 (partial)       Off       Warm    Yes       Minimal
ScheduledTask       1-3 (partial)       Off       Maybe   Yes       Task-specific
Recovery            1 + recovery shell  UART      No      No        Minimal
SafeMode            1-2, 4 (partial)    On        No      No        Reduced
Update              1-5 (full)          On        Yes     Yes       All + verify
DataTransfer        1-2 (partial)       Off       No      USB only  Storage + USB
```

### 16.2 Predictive Boot Configuration

AIRS learns usage patterns and adjusts the boot configuration to optimize for expected use. This isn't about changing *which* services start — it's about changing *how* they start:

**Model pre-selection:** If AIRS observes that the user always loads the coding agent on weekday mornings, and that agent benefits from the code-specialized model variant, AIRS can pre-select that model during Phase 3 instead of the general-purpose default. The model switch is seamless — by the time the user opens the coding agent, the right model is already loaded.

**Cache warming:** The Block Engine can prefetch blocks that are likely to be needed. AIRS maintains a per-intent block access trace:

```rust
pub struct BootAccessTrace {
    intent: BootIntent,
    context: BootContext,           // day of week, time of day, peripherals
    blocks_accessed: Vec<BlockId>,  // ordered by first access time
    timestamp: Timestamp,
}
```

On the next boot with a matching context, the Block Engine prefetches these blocks during Phase 1 (while other services are initializing). By the time Phase 5 renders the workspace, the hot data is already in the page cache. This is similar to Linux's `readahead` but context-aware — different prefetch sets for different usage patterns.

**Agent prelaunch:** If the user always launches the same three agents after boot, the Agent Runtime can start them during Phase 5 before the workspace is visible. The agents are ready by the time the user sees the desktop. This is controlled by a frequency threshold — agents launched in 80%+ of recent boots are auto-prelaunch candidates (distinct from the explicit "autostart" flag in agent manifests).

### 16.3 Readahead and Predictive I/O

Beyond AIRS-driven prediction, the kernel itself performs boot readahead — a proven technique made smarter:

**Boot trace recording:** During every boot, the Block Engine records which blocks are read, in what order, and at what time relative to boot start. This trace is saved to `system/session/boot_trace`:

```rust
pub struct BootTrace {
    boot_id: u64,
    intent: BootIntent,
    entries: Vec<BootTraceEntry>,
}

pub struct BootTraceEntry {
    block_id: BlockId,
    time_offset_us: u64,    // microseconds since kernel entry
    service: ServiceId,     // which service requested the read
}
```

**Readahead replay:** On the next boot, the Block Engine starts a readahead thread immediately after init. It reads the previous boot trace and issues prefetch requests for blocks in the recorded order, staying ~500ms ahead of expected demand. The prefetch runs at the lowest I/O priority (below any foreground service reads).

**Adaptive merging:** Over multiple boots, traces converge. The Block Engine merges the last 5 traces, keeping blocks that appear in 60%+ of them and ordering by median access time. Blocks unique to a single boot (one-time operations) are dropped.

**Impact on SD card:** Random 4K reads on a Class 10 SD card: ~2 MB/s. Sequential reads: ~50 MB/s. By converting random boot reads into a sequential prefetch stream, readahead can reduce Phase 1 storage init from 300ms to ~100ms on SD-backed Pi devices.

-----

## 17. On-Demand Services (Socket Activation)

Not every service needs to run from boot. Some services are used infrequently and waste memory and CPU time if started eagerly. AIOS supports *on-demand activation*: a service starts the first time something tries to communicate with it.

### 17.1 Mechanism

The Service Manager creates IPC channels for on-demand services at boot, but does *not* start the service process. When a message arrives on the channel, the Service Manager intercepts it, starts the service, delivers the buffered message, and connects the channel transparently:

```rust
pub struct ServiceDescriptor {
    // ... existing fields ...

    /// Activation mode for this service.
    activation: ActivationMode,
}

pub enum ActivationMode {
    /// Start during the assigned boot phase (current behavior).
    Boot,
    /// Start on first IPC message to this service's channel.
    OnDemand {
        /// Pre-create channels during boot so senders don't need
        /// to know whether the service is running.
        channel_count: usize,
    },
    /// Start on a timer (e.g., daily maintenance tasks).
    Scheduled { interval: Duration },
}
```

### 17.2 Which Services Benefit

```
Service               Default Mode   Why
──────────────────────────────────────────────────────────────
block_engine          Boot           Critical path. Must exist for everything.
space_storage         Boot           Critical path. Storage for all services.
compositor            Boot           Critical path. User needs to see something.
airs_core             Boot           Loads asynchronously already. Model pre-warm.
posix_compat          OnDemand       Only needed when running BSD/Linux binaries.
                                     Many users may never need it.
audio_subsystem       OnDemand       No audio until user plays media or receives
                                     a notification sound. First audio event
                                     triggers start (~100ms latency on first sound).
browser_runtime       OnDemand       Only needed when opening web content.
print_subsystem       OnDemand       Only needed when printing.
bluetooth_subsystem   OnDemand       Only needed when connecting BT devices.
```

### 17.3 Impact

Moving `posix_compat`, `audio_subsystem`, and `bluetooth_subsystem` from Boot to OnDemand saves:
- ~80ms off Phase 2 critical path (three fewer services to health-check)
- ~15 MB RSS on idle system (three fewer processes resident)

The first activation of an on-demand service adds ~50-150ms latency (process create, ELF load, init). For POSIX compat this means the first Unix command takes an extra 100ms. For audio, the first notification sound has ~100ms extra latency. These are acceptable trade-offs for a faster boot and lower idle memory.

-----

## 18. Encrypted Storage Unlock

AIOS encrypts user data at rest. The encryption key is derived from the user's passphrase (or biometric, or hardware key). The boot sequence must handle the unlock ceremony — the point where the user provides their credential so encrypted spaces become readable.

### 18.1 What's Encrypted

```
Space                    Encrypted?   Why
──────────────────────────────────────────────────────────
system/config/           No          Needed before unlock (device settings)
system/devices/          No          Hardware config, no user data
system/audit/            No          Must be writable before unlock
system/models/           No          AI models are not user-sensitive
system/services/         No          Service binaries, no user data
system/session/          Yes         Contains user activity patterns
system/credentials/      Yes         Passwords, tokens, keys
system/identity/         Yes*        Encrypted with hardware-derived key
                                     (separate from user passphrase)
user/                    Yes         All user data
shared/                  Yes         Collaborative data
web-storage/             Yes         Browser data
```

### 18.2 Key Derivation

```
User passphrase
  │
  ▼
Argon2id (memory-hard KDF)
  - 256 MB memory cost (tuned to take ~500ms on Pi 4)
  - 3 iterations
  - 32-byte salt (random, stored in system/identity/)
  │
  ▼
Master Key (256-bit)
  │
  ├──→ HKDF("space-encryption") → Space Encryption Key
  │     Used for per-space ChaCha20-Poly1305
  │
  ├──→ HKDF("identity-unlock") → Identity Unlock Key
  │     Decrypts the Ed25519 private key in system/identity/
  │
  └──→ HKDF("credential-store") → Credential Store Key
       Decrypts system/credentials/
```

### 18.3 Boot-Time Unlock Flow

```
Phase 4: Identity Service starts
  │
  ├── system/identity/ exists?
  │     NO → first boot (§5 Phase 5 first boot setup flow)
  │
  ├── YES → read encrypted identity blob
  │
  ├── Attempt auto-unlock:
  │   ├── Hardware security key present (USB FIDO2)?
  │   │     → HMAC challenge-response → derive key → unlock
  │   │     → no user interaction needed (~200ms)
  │   │
  │   ├── Biometric reader available?
  │   │     → fingerprint scan → derive key → unlock
  │   │     → minimal user interaction (~500ms)
  │   │
  │   └── Neither available → fall through to passphrase
  │
  ├── Passphrase required:
  │   ├── Compositor is running (Phase 2 complete)
  │   │     → render passphrase prompt overlay
  │   │     → user types passphrase
  │   │     → Argon2id derivation (~500ms on Pi 4)
  │   │     → attempt decrypt
  │   │     ├── Success → unlock, continue boot
  │   │     └── Failure → "Incorrect passphrase", retry (max 10 attempts)
  │   │
  │   └── No compositor (headless) → passphrase via UART
  │
  ▼
Identity unlocked → derive Space Encryption Key
  → Space Storage unlocks encrypted spaces
  → Phase 4 continues (Preferences, Attention Manager, etc.)
```

**Timing impact:** On a fast path (hardware key or biometric), unlock adds ~200-500ms. With a passphrase, it adds user-wait time (typing) + 500ms (Argon2id). The Argon2id cost is tunable — faster on powerful hardware, deliberately slow enough on all platforms to resist brute force.

**Lock-on-suspend:** When the system enters S3/S4, the master key is zeroed from memory. On resume, the unlock ceremony runs again. For S3 resume (< 200ms), this means the user must authenticate again — but a hardware key or fingerprint makes this near-instant. The passphrase prompt appears on the compositor's first resume frame.

-----

## 19. Boot Accessibility

AIOS is unusable if a user with a disability cannot complete the first boot experience. Accessibility must work from the *first frame* — before user preferences exist, before AIRS loads, before any setup occurs.

### 19.1 Pre-Setup Accessibility

The first-boot setup flow (§5 Phase 5) includes accessibility as its very first step — *before* language selection:

```
First Boot — Revised Setup Flow:

0. Accessibility Detection (BEFORE any other UI)
   │
   ├── Check connected USB devices for assistive hardware:
   │   ├── Braille display (USB HID, usage page 0x41)
   │   │     → enable Braille output driver
   │   ├── Switch access device (USB HID, specific vendor IDs)
   │   │     → enable switch scanning mode
   │   └── Screen reader request (special key held: F5 at boot)
   │         → enable text-to-speech (built-in eSpeak-NG, no AIRS needed)
   │
   ├── Offer accessibility options on first frame:
   │   "Press F5 for screen reader. Press F6 for high contrast.
   │    Press F7 for large text. Press Enter to continue."
   │   Displayed in large, high-contrast text by default (24pt, white on dark)
   │   Spoken aloud if screen reader is active
   │
   └── Continue to Language Selection → Passphrase → Wi-Fi → AIRS → Complete

1. Language & Locale Selection
   (all subsequent screens respect accessibility choices from step 0)
```

### 19.2 Built-In Accessibility Engine

The compositor includes a minimal accessibility engine that works without AIRS:

```
Accessibility Feature          AIRS Required?   Boot Availability
──────────────────────────────────────────────────────────────────
High contrast mode             No               From first frame
Large text (2× font scaling)   No               From first frame
Screen reader (eSpeak-NG TTS)  No               From first frame (initramfs)
Braille display output         No               From first frame (USB HID)
Switch scanning (single-switch No               From first frame
  or two-switch navigation)
Reduced motion                 No               From first frame
AI-enhanced descriptions       Yes              After Phase 3 (AIRS)
AI-powered voice control       Yes              After Phase 3 (AIRS)
```

**eSpeak-NG** is compiled into the initramfs (~800 KiB, supports 100+ languages). It provides functional (if robotic) text-to-speech from boot without requiring AIRS. When AIRS loads (Phase 3), voice output can be upgraded to neural TTS if the user prefers.

### 19.3 Accessibility Persistence

Once the user selects accessibility options during first boot, they're stored in `system/config/accessibility` (unencrypted — must be readable before identity unlock):

```rust
pub struct BootAccessibilityConfig {
    screen_reader: bool,
    high_contrast: bool,
    large_text: bool,
    reduced_motion: bool,
    braille_display: bool,
    switch_access: bool,
    tts_voice: TtsVoice,           // eSpeak variant
    tts_rate: f32,                 // speech rate multiplier
    preferred_language: String,     // for TTS
}
```

This config is read by the compositor during Phase 2, before the identity unlock prompt. This means the passphrase entry screen is already accessible — the screen reader is active, text is large, contrast is high — before the user has to type their passphrase.

-----

## 20. Hardware Boot Feedback

### 20.1 The Problem

Not every boot has a display. A headless Pi (server, IoT, NAS) has no monitor. Even on a display-equipped system, there's a gap between power-on and the first framebuffer pixel (~500ms firmware time). During this gap, the user has no indication that the system is alive.

### 20.2 LED Status Indicators

The Raspberry Pi has a green Activity LED (active-low GPIO on Pi 4, RP1-controlled on Pi 5). AIOS uses this LED to communicate boot progress via blink patterns:

```
Pattern                 Meaning                          Duration
──────────────────────────────────────────────────────────────────
Solid on                Firmware running                  0-500ms
1 blink/sec             Kernel early boot                 500-700ms
2 blinks/sec            Phase 1 (storage)                 700-1000ms
3 blinks/sec            Phase 2 (core services)           1000-1500ms
Solid on                Phase 5 complete (boot OK)        1500ms+
SOS pattern             Kernel panic (... --- ...)        Until reboot
Fast flash (10 Hz)      Recovery mode                     Until resolved
```

```rust
pub struct LedBootIndicator {
    gpio: GpioPin,      // Pi 4: GPIO 42, Pi 5: via RP1
}

impl LedBootIndicator {
    /// Called by advance_boot_phase() alongside UART logging
    fn indicate_phase(&mut self, phase: EarlyBootPhase) {
        let pattern = match phase {
            EarlyBootPhase::EntryPoint ..= EarlyBootPhase::TimerReady
                => BlinkPattern::Hertz(1),
            EarlyBootPhase::MmuEnabled ..= EarlyBootPhase::Complete
                => BlinkPattern::Hertz(2),
            _ => BlinkPattern::SolidOn,
        };
        self.set_pattern(pattern);
    }

    fn indicate_panic(&mut self) {
        self.set_pattern(BlinkPattern::Sos);
    }
}
```

On QEMU, the LED indicator is a no-op (no physical LED). The UART output serves the same purpose.

### 20.3 Audio Boot Chime

If an audio output device is detected during Phase 2 (HDMI audio, 3.5mm jack, or USB audio), the kernel can play a short boot chime:

```
Boot chime timing:
  Phase 2 complete → play a short tone (200ms, 440 Hz sine wave)
                     Indicates: display + audio + input are working
  Phase 5 complete → play completion chime (two ascending tones)
                     Indicates: system fully booted, desktop visible
  Panic            → play error tone (low descending tone)
                     Indicates: something went very wrong
```

The boot chime is a generated waveform (no audio file needed), written directly to the audio hardware's PCM buffer. It works before the Audio Subsystem service starts — the HAL provides raw audio output for this purpose.

**User preference:** The boot chime can be disabled via `system/config/boot` (`chime: false`). Default: on. It's one of the few settings read from unencrypted system config before identity unlock.

-----

## 21. First Boot as Conversation

The traditional first-boot experience is a wizard: fixed steps, fixed order, multiple screens of settings the user doesn't understand. AIOS replaces this with a conversation — a natural language exchange that adapts to the user.

### 21.1 How It Works

The first-boot setup flow (§5 Phase 5) already describes the fixed steps: language, passphrase, Wi-Fi, AIRS model. The conversational first boot wraps these steps in a natural interaction:

```
[Screen: clean dark background with AIOS logo]
[After Phase 3 AIRS loads (typically ~3 seconds into boot):]

AIOS:  "Hello! I'm setting up your new computer.
        What language do you prefer?"

User:  "English"

AIOS:  "Got it — English. I've also detected a US keyboard layout.
        Does that look right?"

User:  "Yes"

AIOS:  "To protect your data, I'll encrypt everything on this device.
        Please choose a passphrase — something you'll remember but
        others won't guess."

       [Passphrase input field appears]

User:  [types passphrase]

AIOS:  "Strong passphrase. I see a Wi-Fi network nearby — 'HomeNetwork'.
        Want to connect?"

User:  "Yes, the password is ..."

AIOS:  "Connected. One last thing — I can help you find files, draft
        text, and manage your work using a local AI model that runs
        entirely on this device. Nothing leaves your computer.
        Want to set that up? It'll take about a minute to download."

User:  "Sure"

AIOS:  "Downloading now. You're all set — your desktop is ready.
        If you need anything, I'm in the bar at the bottom of the screen."

       [Setup overlay fades out → Workspace]
```

### 21.2 Adaptive Flow

The conversation adapts based on the user's responses and detected context:

- **No network available?** Skip Wi-Fi, don't offer AIRS download: "No Wi-Fi detected. You can connect later from the network settings."
- **User says "I'm blind"** → immediately activates screen reader + Braille if connected, continues setup via speech: "Screen reader activated. I'll speak all options aloud."
- **User says "I don't want AI"** → AIRS download skipped, conversation bar configured for keyword search only: "No problem. You can always enable it later in preferences."
- **User asks "What is this?"** → explains AIOS briefly: "AIOS is an operating system designed around you. Your files are organized by meaning, not folders. Everything is encrypted and runs locally."
- **Young user / simple responses** → simplifies language. **Technical user** → offers advanced options (UART console access, developer mode, custom partitioning).

### 21.3 Fallback: Fixed Wizard

If AIRS fails to load during first boot (model not available, insufficient RAM, Phase 3 timeout), the setup falls back to the fixed step-by-step wizard described in §5. The wizard is functional but non-conversational — it uses standard UI elements (buttons, text fields, dropdowns) instead of natural language. The user experience is merely good instead of great.

The conversational flow and the fixed wizard produce the same result: an identity, a passphrase, optional Wi-Fi, optional AIRS model. The difference is in the experience.

-----

## 22. Research Kernel Innovations

Several ideas from research and niche kernels have proven valuable but never reached mainstream operating systems. AIOS adopts the best of these, adapted to its architecture.

### 22.1 Orthogonal Persistence (from EROS / KeyKOS / Phantom OS)

**The idea:** There is no "boot" — only resume. The entire system state (processes, capabilities, memory) is continuously checkpointed to persistent storage. Power loss is indistinguishable from a pause. The OS resumes from the last checkpoint as if nothing happened.

**History:** KeyKOS (1983) introduced persistent capabilities that survived across reboots. EROS (Extremely Reliable Operating System, 1991) formalized this into *orthogonal persistence* — the programmer never explicitly saves or loads data. Phantom OS (2009, Russian research) extended this to a full persistent object space where processes literally cannot tell that the machine was powered off.

None of these reached mainstream adoption. The reasons: performance overhead of continuous checkpointing, incompatibility with existing software that assumes volatile memory, and the difficulty of handling hardware state (device registers, DMA buffers) across power cycles.

**What AIOS takes from this:**

AIOS cannot adopt full orthogonal persistence (it needs to run legacy POSIX software, and device state is too complex to checkpoint). But it adopts the *user-facing* principle: **the user should never notice that the machine was off.**

- **Ambient State Continuity (§15.4)** is AIOS's version of continuous checkpointing. User-visible state (edits, scroll positions, selections) trickles into the WAL continuously. The checkpoint granularity is ~2 seconds for keystrokes, ~60 seconds for workspace layout. This provides the *illusion* of orthogonal persistence without the overhead of checkpointing the entire address space.

- **Semantic Resume (§15.3)** is AIOS's version of persistent capabilities. Instead of persisting raw memory (which breaks across kernel updates), AIOS persists *meaning*: which spaces are open, which agents are active, what the user was looking at. This is more resilient than EROS's approach because it survives kernel changes, hardware changes, and even cross-device migration.

- **Space Storage** is inherently persistent and content-addressed. Objects are never lost once committed. Version history is preserved. This gives AIOS the storage semantics of a persistent OS without requiring the kernel to manage persistence.

**What's different from EROS/Phantom:** Those systems persisted the entire process state (registers, stack, heap). AIOS persists only the *semantic* state and lets services reconstruct their process state from it. This means services can be updated, patched, or replaced between checkpoints — something impossible in EROS. The trade-off is that reconstruction takes ~500ms (vs. instant resume in EROS), but reconstruction survives changes that EROS cannot.

### 22.2 Single-Address-Space Boot (from Singularity / Unikernels)

**The idea:** During boot, there is only one process: the kernel. All boot-critical code — the Block Engine, Object Store, Space Storage — runs in kernel space with no context switches, no IPC overhead, no page table switches. After core services are initialized, the kernel "splits" them into separate isolated processes.

**History:** Microsoft Research's Singularity OS (2003-2010) used Software Isolated Processes (SIPs) — processes that share a single address space but are isolated by the type system (Sing#, a dialect of C#). Boot was fast because there was no hardware isolation overhead. Unikernels (MirageOS, IncludeOS, Unikraft) take this further: the entire application is compiled into the kernel with no process boundary at all, booting in as little as 5ms.

Mainstream OSes never adopted this because they rely on hardware isolation (page tables, privilege rings) for security. Running services in kernel space means a bug in any service can corrupt the kernel.

**What AIOS takes from this — Phase 0 Boot Acceleration:**

AIOS is written in Rust. Rust's ownership and borrowing system provides compile-time memory safety guarantees that are normally provided by hardware isolation (page tables). During early boot, when there's only one CPU and no untrusted code, this safety guarantee is sufficient.

**The optimization:** Phase 1 services (Block Engine, Object Store, Space Storage) can be compiled as *kernel modules* that run in the kernel's address space during boot. No process creation, no context switches, no IPC — direct function calls:

```rust
/// During early boot, Phase 1 runs as direct function calls
/// in the kernel's address space. No process isolation overhead.
mod boot_phase1 {
    pub fn init_storage(
        platform: &dyn Platform,
        dt: &DeviceTree,
        allocator: &BuddyAllocator,
    ) -> Result<SpaceStorageHandle> {
        // These are direct function calls, not IPC:
        let block_engine = block_engine::init(platform.init_storage(dt)?)?;
        let object_store = object_store::init(&block_engine)?;
        let space_storage = space_storage::init(&object_store)?;
        Ok(space_storage)
    }
}

/// After Phase 1, the kernel spawns these as separate processes
/// with their own address spaces, capabilities, and IPC channels.
/// The transition is seamless — the running state is handed off.
fn transition_to_isolated(
    space_storage: SpaceStorageHandle,
    svcmgr: &ServiceManager,
) {
    // Create process for Block Engine
    let be_proc = svcmgr.spawn_service(ServiceId::BlockEngine);
    // Transfer device handle to the new process via capability
    be_proc.grant_capability(space_storage.block_device_cap);
    // The in-kernel Block Engine code is now unreachable
    // and its memory is reclaimed.

    // Repeat for Object Store and Space Storage...
}
```

**Why this is safe in Rust:** The Block Engine, Object Store, and Space Storage are Rust crates with `#![forbid(unsafe_code)]` (except for the thin MMIO layer, which is audited). Rust's type system prevents them from corrupting the kernel's data structures. A logic bug in the Block Engine during boot might cause incorrect behavior, but it cannot overwrite kernel memory, jump to arbitrary addresses, or escalate privileges — the compiler prevents it.

**Performance impact:** Eliminating process creation and IPC for Phase 1 saves:
- ~3 context switches per service start (create process, switch to it, switch back) → 0
- ~6 IPC round-trips for health checks and dependency signals → 0 (direct function calls)
- Estimated savings: **50-80ms off Phase 1** (from ~300ms to ~220ms)

**When isolation begins:** After Phase 1 completes and storage is healthy, the kernel transitions to normal isolated mode. Phase 2+ services always run as separate processes with hardware isolation — they interact with untrusted input (network, USB, user content) and must be sandboxed. The single-address-space optimization is *only* for Phase 1, which processes only trusted, integrity-checked data (the superblock, WAL, and content-addressed objects).

**Build system support:** The same Rust crates are compiled twice:
1. As `#[no_std]` kernel modules (for Phase 1 boot, linked into the kernel binary)
2. As standalone ELF binaries (for post-boot isolated operation, in the initramfs)

The dual-compilation is managed by the build system with feature flags:

```toml
# block_engine/Cargo.toml
[features]
default = ["standalone"]
standalone = ["std", "ipc-client"]     # normal isolated mode
kernel-module = ["no_std", "direct"]    # Phase 1 boot mode
```

### 22.3 Capability Persistence Across Reboot (from KeyKOS / EROS)

**The idea:** In KeyKOS and EROS, capabilities are persistent — they survive reboots. A process holding a capability to access a file still holds that capability after a power cycle. The capability system is part of the persistent state.

**Mainstream OSes don't do this.** On Linux/macOS/Windows, all permissions are re-established on every boot. File descriptors are gone. POSIX capabilities are reset. Every service re-authenticates, re-opens files, re-establishes connections.

**What AIOS takes from this:**

Agent capabilities are stored in Spaces. When an agent is shut down (§11.3), its capability set is serialized to `system/agents/<agent_id>/capabilities`. On relaunch, the Agent Runtime reads this set and re-mints equivalent capabilities — provided the capability policy still allows them.

```rust
pub struct PersistedCapabilitySet {
    agent_id: AgentId,
    /// Capabilities the agent held at shutdown.
    /// These are capability *descriptions*, not live tokens.
    /// Live tokens are re-minted on relaunch.
    capabilities: Vec<CapabilityDescription>,
    /// The manifest version that granted these capabilities.
    /// If the manifest has changed (updated agent), capabilities
    /// are re-evaluated against the new manifest.
    manifest_version: ContentHash,
}

pub struct CapabilityDescription {
    capability: Capability,
    reason: String,
    granted_at: Timestamp,
    granted_by: Identity,
}
```

**Key difference from EROS:** EROS persists the raw capability tokens. AIOS persists the *descriptions* and re-mints new tokens. This means:
- A revoked capability stays revoked across reboots (the re-mint check catches it)
- A policy change takes effect on the next boot (new manifest → re-evaluation)
- Capability tokens have fresh nonces and timestamps (preventing replay attacks)
- The capability system doesn't need to be part of the checkpoint (it's reconstructed)

This gives AIOS the *user experience* of persistent capabilities (agents resume with their permissions intact) without the security risks of blindly restoring old tokens.

### 22.4 Self-Healing Services (from MINIX 3)

**The idea:** MINIX 3's Reincarnation Server monitors every driver and service. If one crashes, it is restarted transparently — the rest of the system never notices. This works because MINIX 3 is a microkernel: drivers run in userspace and communicate via IPC, so a crashed driver can be restarted without rebooting.

**AIOS already has this.** The Service Manager (§4) monitors services via health checks and restarts them according to their `RestartPolicy`. But MINIX 3 adds one important detail that AIOS should adopt: **stateless restart with client-side retry.**

In MINIX 3, IPC clients buffer their last request. When a service crashes and is restarted, clients automatically re-send their buffered request. The service restarts from a clean state, processes the request, and the client never sees an error — just a brief delay.

AIOS adopts this for Phase 2+ services:

```rust
pub struct ResilientChannel {
    channel: ChannelId,
    /// Last sent message, buffered for retry
    last_request: Option<Message>,
    /// Service Manager notification channel for service restarts
    svcmgr_events: ChannelId,
}

impl ResilientChannel {
    pub fn send_and_recv(&mut self, msg: Message) -> Result<Message> {
        self.last_request = Some(msg.clone());
        match self.channel.call(msg) {
            Ok(reply) => Ok(reply),
            Err(ChannelError::PeerDied) => {
                // Service crashed. Wait for Service Manager to restart it.
                let new_channel = self.wait_for_service_restart()?;
                self.channel = new_channel;
                // Re-send the buffered request to the new instance
                self.channel.call(self.last_request.take().unwrap())
            }
        }
    }
}
```

**Impact:** A transient crash in the Network Subsystem during boot doesn't fail the boot — the client (e.g., NTP sync) retries transparently after restart. A crash in the Display Subsystem triggers a restart and the compositor re-renders — the user sees a brief flicker instead of a failed boot.

### 22.5 Incremental Boot (from Genode / seL4)

**The idea:** In Genode (and other L4-family systems), the system starts with a tiny trusted computing base (TCB) and incrementally extends itself. Each new component runs in its own protection domain with only the capabilities explicitly granted to it. There is no "big bang" moment where the system suddenly becomes functional — functionality accumulates smoothly.

AIOS's phased boot (§4-5) already follows this pattern, but Genode takes it further: **every component can be started, stopped, and replaced at any time**, not just during boot phases. The system is always in a partial state, and that's fine.

**What AIOS takes from this:**

The Service Manager already restarts failed services. Extending this to **live service replacement** — upgrading a running service without rebooting — is the natural next step:

```
Live service upgrade:
  1. New binary placed in system/services/ (via OTA or manual update)
  2. Service Manager notices the content hash changed
  3. Service Manager spawns new instance alongside the old one
  4. New instance initializes and reports healthy
  5. Service Manager redirects IPC channels from old → new
  6. Old instance receives GracefulStop, saves state, exits
  7. New instance takes over seamlessly
  No reboot. No downtime. No user disruption.
```

This is particularly valuable for AIRS model updates (swap in a new model without restarting the entire AI stack), compositor patches (fix a rendering bug without losing window state), and security patches (apply a fix to the Network Subsystem without dropping connections).

**Constraint:** Live replacement only works for services whose state is serializable to Spaces. Kernel-level components (memory manager, IPC subsystem, scheduler) cannot be live-replaced — they require a reboot. But with Semantic Resume (§15.3), even kernel updates feel almost seamless.

### 22.6 Multikernel Architecture (from Barrelfish)

**The idea:** Barrelfish (ETH Zurich / Microsoft Research, 2009) treats a multicore machine as a distributed system. Each core runs its own OS kernel instance. Cores communicate via explicit message passing, not shared memory. There is no shared kernel state — each core has its own scheduler, its own memory allocator, its own page tables.

**History:** Traditional OSes treat multicores as a shared-memory machine and use locks to synchronize kernel data structures. This worked on 4-8 cores but scales poorly to 64+ cores — lock contention, cache-line bouncing, and NUMA effects dominate. Barrelfish demonstrated that a message-passing architecture eliminates contention entirely: each core makes local decisions and coordinates asynchronously.

The key insight is that modern hardware is already heterogeneous. A phone has CPU cores, GPU cores, a neural processing unit (NPU), a DSP, and various I/O coprocessors. They don't share memory coherently — they communicate via DMA, command queues, and interrupts. A multikernel architecture acknowledges this reality instead of pretending everything is a uniform shared-memory machine.

**What AIOS takes from this — per-core boot and heterogeneous dispatch:**

AIOS doesn't fully adopt the multikernel model (the overhead of cross-core message passing is unnecessary on 4-core Pi/QEMU with coherent caches). But it adopts two key ideas:

1. **Per-core boot independence.** During SMP bringup (§3.5), secondary cores boot independently. Each core initializes its own scheduler run queue, its own per-core allocator slab, and its own interrupt configuration. No global lock is held during secondary boot — the boot CPU and secondary CPUs operate in parallel after the trampoline.

2. **Heterogeneous compute dispatch for AI.** The AIRS inference engine treats the CPU and GPU as separate *compute domains* with explicit data transfer, not shared memory. Model weights are loaded into GPU memory via DMA. Inference requests are submitted via a command queue. Results are read back via a completion queue. This is Barrelfish's message-passing model applied to the CPU↔GPU boundary:

```rust
pub struct ComputeDomain {
    domain_type: ComputeType,  // CPU, GPU, NPU (future)
    /// Command queue for submitting work
    command_queue: RingBuffer<ComputeCommand>,
    /// Completion queue for receiving results
    completion_queue: RingBuffer<ComputeResult>,
    /// Memory region owned by this domain (not shared)
    local_memory: MemoryRegion,
}

pub enum ComputeType {
    /// ARM CPU cores — general compute, scheduling, IPC
    Cpu,
    /// GPU (VirtIO-GPU on QEMU, VC4/V3D on Pi) — inference, rendering
    Gpu,
    /// Neural Processing Unit (future hardware) — dedicated inference
    Npu,
}
```

**Why this matters for AI:** AI workloads are inherently heterogeneous — model loading is I/O-bound, tokenization is CPU-bound, matrix multiplication is GPU-bound. Barrelfish's insight that each processing element should be treated as its own domain with explicit communication maps perfectly to AI inference pipelines. When AIOS eventually supports hardware with dedicated NPUs (Apple Neural Engine, Qualcomm Hexagon), the multikernel communication model is already in place.

### 22.7 Formal Verification (from seL4)

**The idea:** seL4 (NICTA/Data61, 2009) is the world's first formally verified OS kernel. A machine-checked proof (in Isabelle/HOL) guarantees that the C implementation correctly implements the abstract specification. This means: no buffer overflows, no null pointer dereferences, no privilege escalation, no information leaks — these are *mathematically impossible*, not just unlikely.

**History:** Formal verification of a full kernel was considered impossible until seL4 proved otherwise. The proof covers the entire kernel: capability system, IPC, scheduling, memory management, interrupt handling. It took approximately 11 person-years to verify ~10,000 lines of C. Subsequent work extended the proof to the binary level (translation validation), proving that the compiler didn't introduce bugs.

The verification only covers the kernel (~10K LOC). Drivers, services, and applications are not verified. But because seL4 is a microkernel with strong isolation, unverified code cannot violate the kernel's guarantees — a buggy driver can crash itself but cannot corrupt the kernel or other processes.

**What AIOS takes from this — verified kernel invariants:**

Full formal verification of AIOS is not practical (the kernel will be larger than seL4, and verification scales poorly). But AIOS adopts verified *invariants* for security-critical subsystems:

1. **Capability system invariants.** The capability derivation and delegation logic — the part that determines who can access what — is small enough (~2K LOC) to verify. Key properties to prove:
   - *Monotonic attenuation:* a derived capability never has more permissions than its parent
   - *No capability amplification:* holding two capabilities never grants more than their union
   - *Revocation completeness:* revoking a capability revokes all its descendants

2. **IPC channel isolation.** The kernel IPC path is the security boundary between all services. Proving that messages cannot leak across channels, that capability transfer respects the derivation tree, and that no TOCTOU races exist in the message copy path.

3. **Memory isolation.** The page table management code guarantees that no process can map another process's physical pages without holding a valid capability. This is the foundation of all isolation in AIOS.

```rust
/// These invariants are verified via model checking (Kani / MIRI)
/// and exhaustive testing. Full Isabelle/HOL proofs are a future goal.
///
/// Invariant 1: Capability attenuation
/// For all cap_child derived from cap_parent:
///   cap_child.permissions ⊆ cap_parent.permissions
///
/// Invariant 2: Address space isolation
/// For all processes p1, p2 where p1 ≠ p2:
///   mapped_pages(p1) ∩ mapped_pages(p2) = ∅
///   unless shared via explicit shared-memory capability
///
/// Invariant 3: IPC confidentiality
/// For all channels c, messages m sent on c:
///   only the holder of c's receive capability can read m
```

**Rust's role:** Rust provides a significant head start. Memory safety, the absence of data races, and ownership semantics are *already* verified at compile time. seL4's proof had to establish these properties manually for C code. In Rust, the verifier only needs to prove higher-level properties (capability semantics, scheduling fairness) — the memory safety layer is already handled by `rustc`.

**Practical approach:** AIOS uses Kani (Rust model checker) and proptest for automated verification of kernel invariants during CI. Full formal proofs in Lean 4 or Isabelle are a long-term research goal, starting with the capability subsystem.

### 22.8 Intralingual OS Design (from Theseus OS)

**The idea:** Theseus OS (Yale/Rice, 2020) builds the OS using the programming language's type system and module system as the primary isolation and composition mechanism. Instead of hardware-enforced process boundaries, Theseus uses Rust's ownership, lifetimes, and crate boundaries to isolate OS components. Each component is a separately compiled crate that can be loaded, unloaded, and replaced at runtime — like a microkernel, but without the IPC overhead.

**History:** Traditional OSes have two isolation mechanisms: hardware isolation (page tables, privilege rings) for strong boundaries, and nothing at all within the kernel. Theseus introduces a third option: *language-level isolation*. Each kernel component (scheduler, memory manager, device driver) is a Rust crate with explicit dependencies. The type system ensures that one crate cannot access another's internal state. Crate boundaries are *compilation boundaries* — a bug in the network driver cannot corrupt the scheduler because they're in separate crates with no unsafe shared state.

The key innovation is **live evolution**: any crate can be swapped at runtime without rebooting. The old crate is unloaded, its resources are transferred to the new crate, and execution continues. This works because Rust's ownership system makes resource transfers explicit and safe.

**What AIOS takes from this — crate-level kernel modularity:**

AIOS's kernel is already structured as separate Rust crates (allocator, scheduler, IPC, capability system, HAL). Theseus validates that this is the right architecture and suggests going further:

1. **Crate-level fault isolation.** If the network driver panics, only that crate's state is lost. The panic handler (§8) catches the panic, unloads the faulted crate, and the Service Manager restarts the corresponding userspace service. Other kernel crates continue unaffected because they share no mutable state with the faulted crate.

2. **Hot-swappable drivers.** Device drivers are kernel crates that implement the HAL's `Platform` trait. A new driver version can be loaded alongside the old one, tested, and atomically swapped:

```rust
/// Hot-swap a kernel driver crate at runtime.
/// Only possible for drivers that implement the HAL trait
/// and hold no state that cannot be transferred.
pub fn hot_swap_driver(
    old: &dyn Platform,
    new_crate: &LoadedCrate,
) -> Result<()> {
    // 1. Quiesce the old driver (stop DMA, drain queues)
    old.quiesce()?;

    // 2. Extract transferable state (device register base, IRQ number)
    let device_state = old.export_state()?;

    // 3. Initialize new driver with the extracted state
    let new_driver = new_crate.init_with_state(device_state)?;

    // 4. Atomically swap the driver reference
    //    (protected by a brief interrupt-disable window)
    kernel::swap_platform_driver(old, new_driver);

    // 5. Unload old crate, reclaim its memory
    old.unload();
    Ok(())
}
```

3. **Compile-time dependency auditing.** The crate dependency graph is the kernel's architectural blueprint. CI checks enforce: no circular dependencies, no `unsafe` in non-HAL crates, no shared mutable statics, and every inter-crate interface goes through a defined trait.

**What's different from Theseus:** Theseus uses language isolation *instead of* hardware isolation — all code runs in a single address space. AIOS keeps hardware isolation for userspace services (they handle untrusted input and must be sandboxed) but uses Theseus-style crate isolation *within the kernel*. This is the best of both worlds: zero-overhead isolation inside the kernel, hardware-enforced isolation at the kernel-userspace boundary.

### 22.9 Per-Process Namespaces (from Plan 9)

**The idea:** In Plan 9 (Bell Labs, 1992), every process has its own private namespace — its own view of the filesystem tree. Resources are presented as files, and each process can mount, bind, and arrange its namespace independently. There is no single global filesystem; instead, each process constructs its view of the world from composable building blocks.

**History:** Unix has a single global namespace (the filesystem tree). Every process sees the same `/etc/passwd`, the same `/dev/`, the same `/tmp/`. Plan 9 replaced this with *per-process namespaces*: process A might see network resources mounted at `/net/`, while process B sees a completely different network stack — or none at all. This was the intellectual ancestor of Linux mount namespaces, Docker containers, and FreeBSD jails.

The power of Plan 9's design is *composability*. A network filesystem, a local disk, an in-memory filesystem, and a synthetic filesystem (like `/proc`) are all interchangeable. A process can rearrange its namespace without any kernel changes — it's just a user-level operation.

**What AIOS takes from this — per-agent namespaces:**

AIOS agents run in sandboxed processes with capabilities controlling their access. Plan 9's namespace model maps naturally to AIOS's agent isolation:

1. **Each agent sees only its own spaces.** An agent's namespace contains its own spaces (`/spaces/<agent_id>/`), system services it has capabilities for, and nothing else. It cannot even *see* other agents' spaces — they don't exist in its namespace. This is stronger than file permissions: the names themselves are invisible.

2. **Composable service mounting.** When an agent acquires a capability for a new service, that service is *mounted into its namespace*. Losing the capability unmounts it. The namespace is the live reflection of the agent's capability set:

```rust
pub struct AgentNamespace {
    agent_id: AgentId,
    /// Mount table: maps path prefixes to capabilities
    mounts: BTreeMap<PathBuf, CapabilityId>,
}

impl AgentNamespace {
    /// Mount a service into this agent's namespace.
    /// Requires the agent to hold a valid capability for the service.
    pub fn mount(&mut self, path: &Path, cap: CapabilityId) -> Result<()> {
        // Verify the capability is valid and not revoked
        let cap_info = kernel::validate_capability(cap)?;
        self.mounts.insert(path.to_owned(), cap);
        Ok(())
    }

    /// Resolve a path in this agent's namespace.
    /// Returns None if no mount covers this path (the resource
    /// is invisible to this agent).
    pub fn resolve(&self, path: &Path) -> Option<(CapabilityId, &Path)> {
        for (prefix, cap) in self.mounts.iter().rev() {
            if path.starts_with(prefix) {
                let suffix = path.strip_prefix(prefix).unwrap();
                return Some((*cap, suffix));
            }
        }
        None  // Path doesn't exist in this namespace
    }
}
```

3. **Namespace inheritance and restriction.** When an agent spawns a sub-agent, the sub-agent receives a *subset* of the parent's namespace — never more. This is Plan 9's namespace fork, adapted to AIOS's capability model.

**Why this matters for AI:** AI agents need clear, composable boundaries. An agent helping with email should see the user's email space but not their financial documents. Plan 9's namespace model makes this natural: the agent's world is literally limited to what's mounted in its namespace. No ambient authority, no confused deputy, no accidental access.

### 22.10 Asynchronous Everything (from Midori)

**The idea:** Midori (Microsoft Research, 2008-2014, evolved from Singularity) made every operation asynchronous. There are no blocking system calls. Every I/O operation, every IPC message, every resource acquisition returns a promise (future). The scheduler interleaves work across thousands of lightweight tasks without ever blocking a thread on I/O.

**History:** Traditional OSes have blocking syscalls: `read()` blocks until data arrives, `send()` blocks until the buffer is available, `wait()` blocks until the child exits. This means the OS needs one kernel thread per concurrent operation, and thread context switches dominate latency. Midori eliminated this: the entire system — kernel, services, applications — ran on async/await with cooperative scheduling. A single CPU core could handle thousands of concurrent operations because no thread ever blocked.

Midori was cancelled before shipping, but its ideas influenced C#'s async/await, Rust's `Future` trait, and modern JavaScript runtimes.

**What AIOS takes from this — async kernel I/O and boot pipeline:**

Rust's `async/await` gives AIOS native support for Midori-style async. AIOS adopts this at two levels:

1. **Async boot pipeline.** Boot phases (§4-5) launch services as async tasks. Within a phase, all independent services start concurrently. The Service Manager is an async executor:

```rust
/// Service Manager boot: launch all Phase 2 services concurrently
async fn boot_phase2(svcmgr: &ServiceManager) -> Result<()> {
    let display = svcmgr.start(ServiceId::Display);
    let input = svcmgr.start(ServiceId::Input);
    let network = svcmgr.start(ServiceId::Network);
    let audio = svcmgr.start(ServiceId::Audio);

    // All four start concurrently. We only wait for display + input
    // (critical path). Network and audio continue in background.
    let (display_result, input_result) = join!(display, input);
    display_result?;
    input_result?;

    // Phase 2 critical path complete. Move to Phase 3.
    // Network and audio will complete asynchronously.
    Ok(())
}
```

2. **Non-blocking kernel syscalls.** All AIOS syscalls are fundamentally non-blocking. A `read()` on an IPC channel returns immediately with `Poll::Pending` if no message is available. The process yields to the scheduler, which runs other tasks. When the message arrives, the scheduler wakes the waiting task:

```rust
/// Kernel syscall: non-blocking channel receive
pub fn sys_channel_recv(channel: ChannelId) -> SyscallResult {
    match kernel::channel_try_recv(channel) {
        Some(msg) => SyscallResult::Ready(msg),
        None => {
            // No message yet. Register this task for wakeup
            // when a message arrives, then yield to scheduler.
            kernel::register_wakeup(channel, current_task());
            SyscallResult::Pending
        }
    }
}
```

**Impact on boot:** The async model means boot is maximally parallel without explicit thread management. Phase 2 launches display, input, network, and audio as four concurrent async tasks on (potentially) two CPU cores. The scheduler interleaves them based on I/O readiness. There is no "wait for display to finish before starting network" — they naturally interleave around I/O waits.

**Impact on AI inference:** AIRS inference is I/O-heavy (loading model weights from storage, transferring tensors to GPU). Async I/O means the CPU can tokenize the next request while the previous request's weights are still loading from storage. The inference pipeline is naturally pipelined without explicit threading.

### 22.11 Live Kernel Patching (from kpatch / ksplice / kGraft)

**The idea:** Apply security patches and bug fixes to the running kernel without rebooting. The patching system replaces individual functions at runtime by redirecting their call sites to new implementations.

**History:** kpatch (Red Hat, 2014), ksplice (MIT/Oracle, 2009), and kGraft (SUSE, 2014) enable live patching on Linux. The mechanism: a patch is compiled into a kernel module, loaded into memory, and each patched function's entry point is overwritten with a jump to the new implementation. The old function code remains in memory (for rollback). kpatch uses ftrace trampolines; ksplice uses stop_machine() to ensure a consistent state.

The main limitation: live patches can only change function bodies, not data structures. If a bug fix requires changing a struct layout, live patching won't work — a reboot is needed.

**What AIOS takes from this — function-level kernel patching:**

AIOS's Rust kernel can adopt a simplified version of live patching. Because Rust functions are monomorphized and have stable ABIs when `#[repr(C)]` is used, function replacement is straightforward:

```rust
/// Live patch registry: maps function addresses to replacement addresses.
/// Patched functions redirect via a trampoline at their entry point.
pub struct LivePatchRegistry {
    patches: BTreeMap<FunctionAddr, PatchEntry>,
}

pub struct PatchEntry {
    original_addr: FunctionAddr,
    replacement_addr: FunctionAddr,
    /// SHA-256 of the original function bytes (for validation)
    original_hash: [u8; 32],
    /// Capability required to install this patch (Root only)
    required_capability: Capability,
    /// Rollback: original first 16 bytes (overwritten by trampoline)
    saved_prologue: [u8; 16],
}

impl LivePatchRegistry {
    pub fn apply(&mut self, patch: PatchEntry) -> Result<()> {
        // 1. Verify the original function matches expected hash
        //    (ensures we're patching the right thing)
        verify_function_hash(patch.original_addr, &patch.original_hash)?;

        // 2. Disable interrupts on all cores (brief ~10μs window)
        let _guard = kernel::disable_all_interrupts();

        // 3. Overwrite function prologue with branch to replacement
        //    ARM64: B <offset> (unconditional branch, 4 bytes)
        unsafe {
            write_branch_instruction(
                patch.original_addr,
                patch.replacement_addr,
            );
        }

        // 4. Flush instruction caches on all cores
        kernel::flush_icache_all();

        // 5. Register for rollback
        self.patches.insert(patch.original_addr, patch);
        Ok(())
    }
}
```

**Use cases for AIOS:**
- **Security patches:** Fix a vulnerability in the IPC message validation path without rebooting. Critical for an always-on device.
- **Performance tuning:** Replace the scheduler's load balancing function with an improved version observed from runtime profiling.
- **AIRS model loading path:** Patch the model weight decompression function with a faster implementation without interrupting running inference.

**Constraint:** Live patches in AIOS are limited to `#[repr(C)]` kernel functions that don't change their signature or data structure layouts. The Semantic Resume path (§15.3) handles the cases where deeper changes require a reboot.

### 22.12 Deterministic Record-Replay (from rr / PANDA / Mozilla)

**The idea:** Record the entire execution of a program (all inputs, all scheduling decisions, all non-deterministic events) so it can be replayed exactly, instruction by instruction. A bug that took hours to reproduce can be replayed instantly, with full reverse debugging.

**History:** rr (Mozilla, 2014) records Linux program execution by intercepting syscalls, signals, and non-deterministic instructions (RDTSC, CPUID). The recording is compact — only non-deterministic inputs are saved, not the full instruction stream. Replay uses hardware performance counters (perf_event) to count retired instructions, ensuring the replay follows the exact same execution path. PANDA (MIT Lincoln Lab, 2013) extends this to full-system record-replay, capturing every instruction executed by a virtual machine.

Record-replay has been transformative for debugging. Mozilla used rr to find and fix hundreds of concurrency bugs in Firefox. The ability to "go back in time" and inspect any state at any point in a recorded execution makes previously-impossible bugs trivial to diagnose.

**What AIOS takes from this — boot trace recording:**

Boot is the hardest thing to debug in an OS. It happens once, quickly, with limited diagnostic tools (no filesystem, no network, no debugger). A race condition during boot may appear once every 100 boots and vanish under debug instrumentation. Record-replay solves this:

1. **Boot trace recording.** Every boot records a compact trace of non-deterministic events: timer interrupts, device responses, MMIO reads, scheduling decisions. The trace is stored in the panic dump partition (§8.2) — available before Space Storage starts.

```rust
pub struct BootTrace {
    /// Monotonic counter of recorded events
    sequence: u64,
    events: Vec<BootTraceEvent>,
}

pub enum BootTraceEvent {
    /// Timer interrupt on core N at instruction count C
    TimerInterrupt { core: u8, instruction_count: u64 },
    /// MMIO read returned value V from address A
    MmioRead { address: PhysicalAddress, value: u64 },
    /// Scheduler chose task T on core N
    SchedulerDecision { core: u8, task_id: TaskId },
    /// RNG produced bytes B
    RngOutput { bytes: [u8; 32] },
    /// IPC message M delivered to channel C
    IpcDelivery { channel: ChannelId, message_hash: u64 },
}
```

2. **Boot replay in QEMU.** The recorded trace can be replayed in QEMU, reproducing the exact boot sequence. Combined with GDB, this allows stepping through a boot failure that happened on real hardware, instruction by instruction.

3. **AI behavior replay.** When AIRS produces an unexpected result during boot (wrong context mode, incorrect preference inference), the boot trace includes the model inputs and outputs. The AI team can replay the exact inference that produced the bad result and diagnose whether it was a model issue, a data issue, or a timing issue.

**Overhead:** Boot trace recording adds ~2% overhead (dominated by MMIO interception). The trace for a typical 3-second boot is ~500 KB. This is small enough to record every boot and keep the last 10 traces in the panic dump partition.

### 22.13 Learned OS Components (from ML-for-Systems Research)

**The idea:** Replace hand-tuned OS heuristics with machine learning models that adapt to workload patterns. Instead of fixed algorithms for scheduling, caching, memory management, and prefetching, use models that learn from observed behavior.

**History:** Google's "The Case for Learned Index Structures" (Kraska et al., 2018) showed that a simple neural network could replace a B-tree index with lower latency and smaller memory footprint. This sparked a wave of "ML for systems" research: learned scheduling (Decima, MIT, 2019), learned memory allocators, learned admission control for caches (LRB, Carnegie Mellon, 2020), and learned I/O schedulers. The key insight: traditional OS heuristics are fixed policies designed for *average* workloads, but real workloads are *specific* and *predictable*.

**What AIOS takes from this — AIRS-powered OS tuning:**

AIOS has a unique advantage: it already has an AI runtime (AIRS) in the critical path. Using AIRS to optimize OS behavior is natural:

1. **Learned readahead.** The Block Engine's readahead prefetcher (§16.3) currently uses fixed heuristics (sequential detection, stride detection). AIRS replaces these with a tiny model (~100K parameters, runs on CPU) that predicts the next N blocks based on the access pattern history:

```rust
pub struct LearnedReadahead {
    /// Lightweight model (quantized, CPU-only)
    model: TinyModel,
    /// Recent access history (ring buffer of last 1024 block addresses)
    history: RingBuffer<BlockAddress>,
    /// Prediction accuracy tracker (for self-assessment)
    hit_rate: ExponentialMovingAverage,
}

impl LearnedReadahead {
    pub fn predict_next(&self) -> Vec<BlockAddress> {
        let features = self.history.as_feature_vector();
        let predictions = self.model.infer(&features);
        // Only prefetch if the model is confident (> 70% hit rate)
        if self.hit_rate.value() > 0.7 {
            predictions
        } else {
            // Fall back to simple sequential readahead
            self.sequential_fallback()
        }
    }
}
```

2. **Learned scheduler boost.** The scheduler (§scheduler.md) assigns context multipliers based on task class (UI, background, AI inference). AIRS can refine these multipliers based on observed behavior: if the user consistently interacts with a particular agent during morning hours, that agent's tasks get a preemptive boost before the user opens it.

3. **Learned memory pressure response.** The memory manager's eviction policy currently uses a fixed LRU-with-working-set heuristic. AIRS can learn which pages are likely to be re-accessed and prioritize eviction of pages with low predicted reuse probability.

**Safety:** All learned components have a hard fallback to traditional heuristics. If the model's accuracy drops below a threshold, or if AIRS is unavailable (early boot, recovery mode), the system uses fixed algorithms. The learned component is an *optimization*, never a correctness requirement:

```rust
pub trait AdaptivePolicy {
    /// Learned policy (may be unavailable or inaccurate)
    fn learned_decision(&self) -> Option<Decision>;
    /// Fixed fallback (always available, always correct)
    fn fallback_decision(&self) -> Decision;

    fn decide(&self) -> Decision {
        self.learned_decision().unwrap_or_else(|| self.fallback_decision())
    }
}
```

### 22.14 Zero-Copy IPC via Memory Transfer (from L4 / Fuchsia VMOs / seL4)

**The idea:** Instead of copying message data between address spaces, transfer *ownership* of memory pages. The sender unmaps the page from its address space and the receiver maps it into theirs. No data is copied — only page table entries change. For large messages (model weights, image buffers, tensor data), this reduces IPC cost from O(n) to O(1).

**History:** L4 (Jochen Liedtke, 1993) pioneered fast IPC with small messages passed in registers. For large data, L4 introduced *grant* and *map* operations: a sender could grant a page to a receiver (removing it from the sender's space) or map it (sharing read-only). Fuchsia's Zircon kernel formalized this as Virtual Memory Objects (VMOs) — kernel objects that represent contiguous regions of memory and can be transferred between processes via handles. seL4 uses a similar mechanism via its capability-based memory model.

**What AIOS takes from this — zero-copy tensor and space transfer:**

AIOS's IPC (§ipc.md) supports small messages in registers (≤64 bytes) and larger messages via shared-memory channels. For AI-specific workloads, zero-copy page transfer is critical:

1. **Model weight loading.** When AIRS loads a model from Space Storage, the model weights (often hundreds of MB) are read into pages. With zero-copy, these pages are *transferred* from the Block Engine to the Object Store to AIRS — the data never moves, only the page mappings change:

```rust
/// Zero-copy page transfer between processes.
/// The sender loses access; the receiver gains access.
/// Only page table entries are modified — no data copy.
pub fn transfer_pages(
    from: ProcessId,
    to: ProcessId,
    pages: &[PhysicalPage],
) -> Result<VirtualAddress> {
    // 1. Unmap pages from sender's address space
    for page in pages {
        kernel::unmap_page(from, page)?;
    }
    // 2. Map pages into receiver's address space
    let base = kernel::find_free_region(to, pages.len())?;
    for (i, page) in pages.iter().enumerate() {
        kernel::map_page(to, base + i * PAGE_SIZE, page, PageFlags::READ)?;
    }
    // 3. Flush TLB entries for both processes
    kernel::flush_tlb_range(from, pages);
    kernel::flush_tlb_range(to, pages);
    Ok(base)
}
```

2. **Compositor buffer handoff.** When an application renders a frame, the framebuffer pages are transferred to the compositor — no copy of the pixel data. The compositor composes multiple application buffers into the scanout buffer, then transfers the scanout buffer to the GPU — again, no copy.

3. **Space object transfer.** When a user opens a space (document, image, conversation), the space data is read from storage into pages. These pages are transferred to the application — zero copy. When the application saves, modified pages are transferred back to storage — zero copy.

**Performance impact:** For a 1 GB model weight load, zero-copy saves ~5ms (memcpy at 200 GB/s) and eliminates the need for 2x the physical memory (no source + destination copies). For a 4K framebuffer (33 MB at 60fps), zero-copy saves ~0.16ms per frame — the difference between hitting and missing 60fps on Pi hardware.

### 22.15 Component-Based OS with Manifest-Driven Composition (from Fuchsia)

**The idea:** Fuchsia (Google, 2016-present) structures the entire OS as a tree of *components*. Each component has a manifest declaring its capabilities, dependencies, and exposed services. The component framework resolves dependencies, creates sandboxes, and routes capabilities — all driven by declarative manifests, not imperative code.

**History:** Traditional OSes have a flat process model: every process can (in principle) access any system resource. Sandboxing is bolted on after the fact (seccomp, AppArmor, macOS sandbox profiles). Fuchsia inverts this: every component starts with *zero* capabilities and must declare what it needs. The component framework grants only what the manifest requests and the policy allows. Components discover each other through capability routing, not global names.

This is powerful for composition: a component can be dropped into any system that satisfies its declared dependencies. There's no "install process" beyond placing the component and its manifest.

**What AIOS takes from this — service manifests and capability routing:**

AIOS's Service Manager (§4) already uses service descriptors that declare dependencies. Fuchsia validates this approach and suggests deeper adoption:

1. **Declarative service manifests.** Every AIOS service has a manifest that declares its complete interface: what capabilities it needs, what services it exposes, what resources it consumes, and what its failure mode is:

```rust
pub struct ServiceManifest {
    id: ServiceId,
    binary: ContentHash,

    /// Capabilities this service requires to function
    required_capabilities: Vec<CapabilityRequest>,

    /// Services this service exposes to others
    exposed_services: Vec<ServiceInterface>,

    /// Resource limits (memory, CPU, file descriptors)
    resource_limits: ResourceLimits,

    /// Boot phase this service belongs to (determines start order)
    boot_phase: BootPhase,

    /// How to handle service failure
    restart_policy: RestartPolicy,

    /// Health check configuration
    health_check: HealthCheckConfig,

    /// Dependencies: services that must be healthy before this one starts
    dependencies: Vec<ServiceId>,

    /// Capability routing: how this service's capabilities
    /// are derived from the system's root capabilities
    capability_route: Vec<CapabilityRoute>,
}

pub struct CapabilityRoute {
    /// What the service requests (e.g., "storage:read")
    request: CapabilityRequest,
    /// Where it comes from (e.g., from parent, from framework, from child)
    source: RouteSource,
    /// Attenuation applied during routing
    attenuation: Option<CapabilityAttenuation>,
}
```

2. **Static capability routing verification.** Before boot, the service dependency graph can be statically verified: every capability request has a source, no circular dependencies exist, no service requests capabilities beyond what the system provides. This catches configuration errors *before boot*, not during.

3. **Component isolation profiles.** Each service's manifest generates a precise sandbox: only the declared capabilities are available, only the declared resources are allocated, only the declared IPC channels are created. A service that doesn't declare network access literally has no network syscalls available — they don't exist in its namespace.

**Why this matters for AIOS:** The component model extends naturally to agents. Agent manifests (already described in agents.md) are a special case of service manifests. The unified manifest system means the same tooling, the same static verification, and the same capability routing work for both system services and user-installed agents.

-----

## 23. Documentation Gaps

Concepts referenced in boot.md that do not yet have full documentation elsewhere. These are placeholders for future doc work.

1. **Agent state persistence across reboot** — §10.3 describes the shutdown/relaunch protocol (agents save to spaces, then reload on next boot), but [agents.md](../applications/agents.md) does not yet document the relaunch-on-boot mechanism or how the Agent Runtime discovers which persistent agents to restart. Agents.md §3 (Agent Lifecycle) should add a "Reboot Recovery" subsection covering: how `system/agents/` tracks which agents were running, how agent spaces are re-mounted, and the order of agent relaunch relative to Phase 5 autostart.

2. **Attention Manager initialization requirements** — Phase 4 mentions the Attention Manager loads rules from preferences and optionally connects to AIRS for AI triage. [attention.md](../intelligence/attention.md) documents the attention model thoroughly but does not have a section on boot-time initialization: what the minimal startup state is, what happens before AIRS is available, and how the notification pipeline connects to the compositor. Attention.md should add an "Initialization" section.

3. **AIRS default model selection** — Phase 3 specifies RAM-based model selection thresholds (≥8 GB → 8B Q4_K_M, ≥4 GB → 3B Q4_K_M, <4 GB → 1B Q4_K_M). [airs.md](../intelligence/airs.md) should document these exact thresholds in its model registry section and specify what happens when no model files are present in `system/models/` (first boot with no pre-loaded models).

4. **Context Engine boot behavior** — Phase 3 states the Context Engine falls back to "rule-based heuristics" if AIRS is unavailable. [context-engine.md](../intelligence/context-engine.md) documents `ContextMode` and signal fusion but does not specify what "rule-based heuristics" means concretely at boot time or which signals are available before user activity begins.

5. **Task Manager service** — [overview.md](../project/overview.md) lists a "Task Manager" as a core service ("intent → subtasks, orchestrate") but it does not appear in this document's service dependency graph (§4.5) or in any boot phase. It needs its own document (potentially `docs/intelligence/task-manager.md` or `docs/applications/task-manager.md`) covering: what it manages, how it decomposes intents into subtasks, which boot phase it belongs to, and its dependencies. Until then, Phase 5 Workspace queries the Agent Runtime for active agent tasks instead.

6. **Recovery mode and safe mode operational procedures** — §9 describes recovery mode commands and safe mode service lists, but there is no standalone troubleshooting or operations guide documenting: how to connect a UART console on each platform, how to diagnose common boot failures, or how to restore from backup after a factory reset. This could be a future `docs/operations/recovery.md`.

7. **USB host controller driver** — Phase 2 describes USB enumeration for Pi input, but the xHCI driver itself is not documented anywhere. hal.md should add a USB section covering: xHCI ring buffer setup, USB descriptor parsing, hub enumeration, and HID class driver. This is Pi-specific (QEMU uses VirtIO-Input) and affects input latency on real hardware.

8. **Audio subsystem architecture** — Phase 2 mentions Audio Subsystem startup but there is no `audio.md` document. Needed: PCM mixing engine, I2S/PWM driver for Pi, VirtIO-Sound for QEMU, HDMI audio routing, RT scheduling requirements, and how audio interacts with the compositor for A/V sync.

9. **Measured boot and attestation** — Implementation Order lists "Phase 24: Secure Boot" but boot.md does not describe *what* gets measured or *where* measurements are stored. On Pi there is no discrete TPM — measurements would need to use a software TPM (fTPM in ARM TrustZone) or the UEFI variable store. A future `docs/security/secure-boot.md` should cover: firmware measurement, kernel hash verification, initramfs integrity, and remote attestation for enterprise deployment.

10. **SMMU driver internals** — §3.6 describes when and why SMMU is initialized, but the SMMUv3 driver itself (stream table format, command/event queues, fault handling) needs documentation in hal.md. Pi 5's BCM2712 SMMU has platform-specific quirks that should be documented.

11. **Suspend/resume device state** — §15.1 describes the suspend/resume flow, but the per-device state save/restore for each HAL device (GIC, timer, UART, GPU, network, storage, RNG) needs detailed documentation in hal.md. Each device has different quiesce and restore requirements.

12. **Hibernate image management** — §15.2 describes the hibernate image format and partition, but the background S3→S4 writeback mechanism (DMA engine writing DRAM during suspend) needs hardware-specific documentation per platform. Not all platforms support DMA during low-power states.

13. **Proactive Wake scheduling** — §15.5 describes the concept, but the AIRS usage prediction model, RTC alarm programming per platform, and power policy interaction need a dedicated document, possibly `docs/intelligence/proactive-wake.md`.

14. **Boot-time encryption unlock UX** — §18.3 describes the unlock flow, but the Argon2id tuning parameters per platform, hardware key (FIDO2) integration, and biometric reader support need documentation in `docs/security/encryption.md`.

15. **Accessibility engine** — §19.2 lists built-in accessibility features, but the eSpeak-NG integration, Braille display protocol, and switch scanning input model need their own document, possibly `docs/experience/accessibility.md`.

16. **Single-address-space boot validation** — §22.2 describes running Phase 1 services as kernel modules. The dual-compilation build system, the state handoff mechanism from kernel-module mode to isolated-process mode, and the safety audit requirements for `#![forbid(unsafe_code)]` enforcement need documentation in the build system guide.

17. **Heterogeneous compute dispatch** — §22.6 introduces the `ComputeDomain` abstraction for CPU/GPU/NPU dispatch, but the concrete GPU command queue protocol for VirtIO-GPU (QEMU) and V3D (Pi) is not documented. hal.md should add a "Compute Dispatch" section covering: command ring buffer format, completion interrupt handling, memory transfer (DMA vs. cache-coherent) per platform, and how the inference engine selects CPU vs. GPU for a given model size. Additionally, the NPU `ComputeType` variant is forward-looking — when NPU hardware is targeted, a dedicated `docs/platform/npu.md` will be needed.

18. **Kernel invariant verification tooling** — §22.7 proposes formal verification of capability, IPC, and memory isolation invariants using Kani and proptest. The CI pipeline configuration for running these verifiers, the invariant specification format, and the Lean 4 proof roadmap need documentation. A future `docs/project/verification.md` should cover: which invariants are verified today (model checking), which are planned for full proofs, how verification failures block releases, and the expected person-effort for each verified subsystem.

19. **Crate-level kernel fault isolation** — §22.8 describes Theseus-style crate isolation and hot-swappable drivers. The mechanism for detecting a crate panic (unwinding across crate boundaries in `#![no_std]`), the state extraction protocol for hot-swap (`export_state()`/`init_with_state()`), and the crate dependency enforcement in CI need documentation. hal.md should add a "Driver Hot-Swap" section specifying which drivers support hot-swap (storage: no, network: yes, display: partial) and the quiesce requirements for each.

20. **Per-agent namespace implementation** — §22.9 describes Plan 9-style per-agent namespaces. The namespace creation during agent spawn, the mount table storage (kernel memory vs. space-backed), namespace inheritance rules, and the interaction with the POSIX compatibility layer (§posix.md) need documentation. When an agent makes a POSIX `open()` call, the path is resolved through its namespace — posix.md should document this resolution order and what happens when a path resolves to "not mounted" (ENOENT vs. EACCES).

21. **Async kernel executor** — §22.10 describes async boot and non-blocking syscalls. The kernel's async executor (waker registration, task queue, cooperative yielding) is not documented. scheduler.md should add an "Async Executor" section covering: how kernel async tasks relate to scheduler tasks, the waker storage mechanism, priority inheritance for async waiters, and the cost of a context switch vs. an async yield. The relationship between the Service Manager's async boot loop and the scheduler's run queue needs clarification.

22. **Live kernel patch safety and rollback** — §22.11 describes function-level live patching. The safety constraints (which functions can be patched, how to verify ABI compatibility, how to handle in-flight calls to the patched function), the rollback mechanism (restoring the saved prologue), and the patch distribution format (how patches are delivered via OTA and verified) need documentation. security.md should cover the capability requirements for installing live patches (root-only, with audit logging).

23. **Boot trace storage and replay tooling** — §22.12 describes boot trace recording for deterministic replay. The trace storage partition layout (shared with panic dump), the QEMU replay harness, and the trace analysis tooling need documentation. The interaction between boot trace recording and boot performance (2% overhead claim needs benchmarking methodology). A future `docs/project/debugging.md` should cover the end-to-end workflow: recording a trace on hardware, transferring it to a development machine, and replaying in QEMU with GDB attached.

24. **Learned component training and deployment** — §22.13 describes AIRS-powered OS tuning (learned readahead, scheduler boost, memory pressure response). The training pipeline (when and how models are trained on observed behavior), the model update mechanism (how a new readahead model replaces the old one), the accuracy monitoring system, and the fallback trigger thresholds need documentation. The privacy implications of learning from user behavior patterns should be addressed in security.md — all training data stays on-device and is never transmitted.

25. **Zero-copy page transfer TLB coherence** — §22.14 describes zero-copy IPC via page transfer. The TLB flush strategy (per-page IPI vs. full flush vs. ASID-based invalidation), the interaction with the SMMU (§3.6) when DMA is in flight to transferred pages, and the performance characteristics per platform (QEMU's TLB flush is a no-op; Pi's requires explicit maintenance) need documentation in memory.md. Edge cases: what happens when a page being transferred is also memory-mapped by a third process (answer: transfer fails, falls back to copy).

26. **Service manifest schema and static verification** — §22.15 describes Fuchsia-style service manifests. The manifest file format (TOML? embedded in binary? stored in Space?), the static verification tool (run at build time or boot time?), and the capability routing resolution algorithm need documentation. A future `docs/project/service-manifests.md` should specify the full manifest schema, provide examples for each system service, and document the verification errors and how to resolve them.

27. **Power management policy engine** — Boot.md describes suspend/resume (§15), proactive wake (§15.5), and on-demand services (§17), but there is no unified power management policy document. The interactions between: idle timeout → light sleep, lid close → S3, low battery → hibernate, thermal throttle → frequency scaling, and AIRS predictions for proactive wake need a coherent policy engine. A future `docs/platform/power-management.md` should specify the state machine, the sensor inputs (battery SoC, thermal zone temperatures, lid switch, AC adapter), and the policy rules.

28. **Thermal management during boot and inference** — AI inference generates significant heat. On Pi hardware, sustained inference can thermal-throttle the CPU within 30 seconds. Boot.md does not specify thermal monitoring during boot or how AIRS model selection (§Phase 3) accounts for thermal headroom. hal.md should document the thermal zone sensors per platform, the throttling thresholds, and how the scheduler reduces inference priority when approaching thermal limits.

29. **OTA update atomicity and rollback** — §9.6 describes OTA updates at a high level, but the atomic update mechanism (A/B partitions? overlay filesystem? content-addressed deduplication?), the update verification chain (signature verification, hash checking), the rollback trigger (failed health check after update), and the interaction with live kernel patching (§22.11) need comprehensive documentation. A future `docs/platform/ota-updates.md` should cover the end-to-end update flow from download to verification to installation to rollback.

30. **Cross-device migration and Semantic Resume portability** — §15.3 claims Semantic Resume survives "cross-device migration" but the mechanism is not specified. How does a SemanticSnapshot transfer between devices? What happens when the target device has different hardware (different GPU, different screen resolution, different model availability)? How is the snapshot authenticated and encrypted during transfer? This needs its own section in boot.md or a dedicated `docs/experience/device-migration.md`.

31. **Watchdog timer integration** — hal.md documents watchdog init, but boot.md does not specify watchdog behavior during each boot phase: which phase starts the watchdog, what the timeout is per phase (Phase 1 storage init may take longer on first boot with filesystem creation), and what happens when the watchdog fires during boot (recovery mode? restart current phase? full reboot?). The watchdog-to-recovery-mode escalation path needs documentation.

32. **Multi-user boot behavior** — AIOS identity.md describes user identity, but boot.md assumes a single-user device. If multiple identities exist, boot.md should specify: which user's Semantic Resume state is restored (last active user? auto-detected via biometrics?), how per-user service instances are managed (separate AIRS contexts per user?), and how the login/identity-selection screen interacts with the boot splash timeline (§7).

-----

## 24. Design Principles

1. **Usable at each phase boundary.** Every service is optional except storage and display. AIRS failure doesn't break boot. Network failure doesn't break boot.
2. **Fast on the critical path.** Only the minimum services needed for a desktop are on the critical path. Everything else runs in parallel or deferred.
3. **Visual feedback from the start.** The user sees a splash screen within 500ms of power-on. Never stare at a black screen.
4. **Recovery is always possible.** Three consecutive failures trigger recovery mode. Rollback to previous kernel is always available. Factory reset is the last resort.
5. **Services are independent.** Each service has its own process, its own capabilities, its own restart policy. One service crashing doesn't take down the system.
6. **Boot is audited.** Every phase transition, every service start, every failure is logged. The audit trail is queryable after boot via `system/audit/boot/`.
7. **First boot and normal boot are the same code path.** The only difference is whether system spaces exist yet (first boot creates them). No separate "installer" or "setup wizard."
8. **State is never lost.** Ambient continuity ensures the user never loses more than ~2 seconds of work, regardless of how the system goes down — crash, panic, power loss.
9. **Boot adapts to context.** The system detects *why* it's booting and adapts the service graph. A proactive wake doesn't light the screen. A scheduled task doesn't load the compositor. Intent drives behavior.
10. **The system learns.** Boot readahead, model pre-selection, and proactive wake improve over time as AIRS observes usage patterns. The 100th boot is faster and smarter than the first.
11. **Accessibility from the first frame.** A user with a disability must be able to complete first boot independently. No accessibility feature requires AIRS, network, or user preferences to function.
12. **Research ideas, pragmatically applied.** Orthogonal persistence, single-address-space boot, capability persistence, self-healing services, and live service replacement are adopted from research kernels — but adapted to work with real hardware, existing software, and Rust's safety guarantees.
13. **Heterogeneity is the norm.** CPU, GPU, and future NPU are treated as distinct compute domains with explicit communication, not a uniform shared-memory machine. The OS acknowledges hardware diversity instead of hiding it.
14. **Verify what matters most.** Security-critical invariants (capability attenuation, memory isolation, IPC confidentiality) are formally verified or model-checked. The rest relies on Rust's compile-time guarantees and thorough testing.
15. **The language is the first line of defense.** Rust's type system, ownership model, and crate boundaries provide isolation guarantees that traditional OSes achieve only through hardware mechanisms. AIOS uses both — language safety inside the kernel, hardware isolation at the kernel-userspace boundary.
16. **Every namespace is private.** Agents and services see only what their capabilities allow. There is no global namespace, no ambient authority, no way to discover resources that haven't been explicitly granted. Invisible means inaccessible.
17. **Async by default.** No kernel syscall blocks. Every I/O operation returns immediately or yields to the scheduler. Boot is maximally parallel because services are async tasks, not sequential thread launches.
18. **The kernel is patchable.** Security fixes and performance improvements can be applied to the running kernel without rebooting. When deeper changes require a reboot, Semantic Resume makes it feel seamless.
19. **Every boot is recorded.** Boot trace recording captures non-deterministic events for offline replay and debugging. A boot failure that happens once in 100 boots can be diagnosed from its trace.
20. **Learned components always have fallbacks.** ML-powered optimizations (readahead, scheduling, memory eviction) improve performance when accurate but degrade gracefully to traditional heuristics when the model is unavailable or inaccurate. Intelligence is an optimization, never a correctness requirement.
21. **Zero-copy is the fast path.** Large data (model weights, framebuffers, space objects) moves between processes via page transfer, not memory copy. The kernel's job is to remap page tables, not to copy bytes.
22. **Composition over configuration.** Services and agents are composed from declarative manifests, not configured by imperative scripts. Static verification catches errors before boot. The manifest is the single source of truth for a component's interface.
