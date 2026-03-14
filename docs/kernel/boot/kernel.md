# AIOS Kernel Early Boot

Part of: [boot.md](../boot.md) — Boot and Init Sequence
**Related:** [firmware.md](./firmware.md) — Firmware handoff and BootInfo, [services.md](./services.md) — Service startup, [performance.md](./performance.md) — Boot timing, [hal.md](../hal.md) — Platform trait, [memory.md](../memory.md) — Memory management

-----

## 3. Kernel Early Boot

Early boot runs entirely in kernel space (EL1). No interrupts, no virtual memory (initially), no heap. Everything is statically allocated or uses the boot stack. Each step must complete before the next begins.

### 3.1 Phase Tracking

The kernel tracks boot progress through an enum with 18 variants (defined in `shared/src/boot.rs`, re-exported in `kernel/src/boot_phase.rs`). Phase transitions are logged to the UART via structured logging macros:

```rust
// Source: shared/src/boot.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u32)]
pub enum EarlyBootPhase {
    EntryPoint = 0,              // Entered kernel entry point
    ExceptionVectors = 1,        // Rust vector table installed at VBAR_EL1
    DeviceTreeParsed = 2,        // Device tree parsed, platform detected
    UartReady = 3,               // PL011 UART fully initialized
    InterruptsReady = 4,         // GICv3 distributor + redistributor configured
    TimerReady = 5,              // ARM Generic Timer configured (1 kHz tick)
    MmuEnabled = 6,              // TTBR0 identity map built and installed
    PageAllocatorReady = 7,      // Buddy allocator initialized from UEFI map
    HeapReady = 8,               // Slab allocator active, alloc works
    LogRingsReady = 9,           // Per-core log ring buffers usable
    RngReady = 10,               // (reserved for future use)
    KaslrApplied = 11,           // (reserved — slide computed but not yet applied)
    CapabilityManagerReady = 12, // (reserved for future use)
    IpcReady = 13,               // (reserved for future use)
    AuditLogReady = 14,          // (reserved for future use)
    ProcessManagerReady = 15,    // SMP cores online, scheduler initialized
    ProvenanceReady = 16,        // (reserved for future use)
    Complete = 17,               // Early boot complete
}

pub const EARLY_BOOT_PHASE_COUNT: usize = 18;
```

The phase is tracked via `AtomicU64` for safe concurrent access:

```rust
// Source: kernel/src/boot_phase.rs

static CURRENT_PHASE: AtomicU64 = AtomicU64::new(0);

pub fn advance_boot_phase(phase: EarlyBootPhase) {
    CURRENT_PHASE.store(phase as u64, Ordering::Relaxed);
    if phase >= EarlyBootPhase::UartReady {
        crate::kinfo!(Boot, "{:?} — {}ms", phase, boot_elapsed_ms());
    }
}
```

Boot timing reads `CNTVCT_EL0` (ARM Generic Timer virtual count) directly — no interrupts or heap required.

### 3.2 Kernel State

There is **no centralized `KernelState` struct**. Each subsystem manages its own global state via `static` variables (atomics for concurrent access, `Mutex<T>` where needed). This avoids a God-object and lets each subsystem be initialized independently:

```text
Subsystem          Global state                              Location
──────────────────────────────────────────────────────────────────────────────
Boot phase         CURRENT_PHASE: AtomicU64                  boot_phase.rs
Boot timing        BOOT_START_TICKS: AtomicU64               boot_phase.rs
UART               UART_BASE_ADDR: AtomicUsize                arch/aarch64/uart.rs
GIC                (init returns InterruptController)         arch/aarch64/gic.rs
Timer              TICK_INTERVAL: AtomicU64                   arch/aarch64/timer.rs
                   TICK_COUNT: AtomicU64
                   NEED_RESCHED: AtomicBool
Memory pools       FRAME_ALLOC: Mutex<Option<FrameAllocator>> mm/frame.rs
Slab allocator     SLAB: Mutex<SlabAllocator>                mm/slab.rs
ASID               ASID_ALLOC: Mutex<AsidAllocator>           mm/uspace.rs
Scheduler          RUN_QUEUES: [Mutex<RunQueue>; MAX_CORES]  sched/mod.rs
                   THREAD_TABLE: Mutex<[Option<Thread>; 64]> task/mod.rs
IPC channels       CHANNEL_TABLE: Mutex<[Option<Channel>]>   ipc/mod.rs
Capabilities       (per-process CapabilityTable)             cap/mod.rs
Processes          PROCESS_TABLE: Mutex<[Option<ProcessControl>]> task/process.rs
Services           SERVICE_MANAGER: Mutex<ServiceManager>    service/mod.rs
```

This design emerged from `no_std` constraints: there is no global heap at early boot, so subsystems cannot be heap-allocated into a central struct. Each subsystem's `init()` function writes to its own statics.

### 3.3 Step-by-Step Early Boot

The boot sequence is split between **boot.S** (assembly, pre-Rust) and **kernel_main** (Rust). Each step must complete before the next begins. The step numbers below match the comments in `kernel/src/main.rs`.

#### Phase A: boot.S (assembly, before kernel_main)

**boot.S Step 1: FPU enable.** The very first instruction enables the Advanced SIMD (NEON) and floating-point unit. Without this, any floating-point or NEON instruction traps to EL1. Rust's codegen freely uses NEON registers for `memcpy`/`memset`, so this must happen before ANY Rust code executes.

```asm
mrs  x1, CPACR_EL1
orr  x1, x1, #(3 << 20)    // FPEN = 0b11: no trapping
msr  CPACR_EL1, x1
isb                          // ensure the change is visible
mov  x19, x0                // save BootInfo pointer (callee-saved)
```

**boot.S Step 2: Stub exception vectors.** Install a minimal exception vector table (`.text.vectors` section) at `VBAR_EL1`. These stub vectors simply loop (`b .`) on any exception — a safety net until the Rust vector table replaces them in kernel_main.

**boot.S Step 3: Park secondary cores.** Read `MPIDR_EL1[7:0]` to get the core ID. If non-zero, branch to a `wfe` parking loop. Only core 0 (the boot CPU) continues.

**boot.S Step 4: Set stack pointer.** Load SP from `__stack_top` (128 KiB stack at end of BSS, defined in linker.ld).

**boot.S Step 5: Zero BSS.** Loop from `__bss_start` to `__bss_end`, writing zero in 8-byte strides.

**boot.S Step 6: Build minimal TTBR1.** Construct a 3-page page table hierarchy (L0 → L1 → L2) in BSS statics. L2 contains 4 × 2MB block descriptors covering `0x40000000–0x40800000` with MAIR Attr3 (WB cacheable, Inner Shareable, AF set). This minimal map is sufficient to branch to the virtual kernel_main address.

**boot.S Step 7: Configure TCR_EL1.** Set T1SZ=16 (48-bit kernel VA), TG1=4KiB granule, IRGN1/ORGN1=WB WA, SH1=Inner Shareable. Preserve existing TTBR0 configuration (bits[15:0]) from edk2.

**boot.S Step 8: Install TTBR1 and branch to virtual kernel_main.** Write L0 physical address to `TTBR1_EL1`. Execute `TLBI VMALLE1` + `DSB NSH` (non-broadcast — broadcast hangs with parked cores under NC memory). Compute virtual kernel_main address by adding `VIRT_PHYS_OFFSET` (`0xFFFE_FFFF_C000_0000`). Convert SP to virtual. Branch to virtual `kernel_main` with `x0 = x19` (BootInfo physical address).

#### Phase B: kernel_main (Rust)

**Step 1: Boot timing + BootInfo validation.** Read `CNTVCT_EL0` for boot timing baseline (`boot_phase::init_boot_timing()`). Validate BootInfo pointer is non-zero and magic equals `0x41494F53_424F4F54`. If invalid, halt with error message on UART. No phase advance yet — UART is not fully initialized.

**Step 2: Rust exception vectors.** Install the full Rust-owned exception vector table (`.text.rvectors` section) at `VBAR_EL1`, replacing the boot.S stubs. The Rust vectors handle: synchronous exceptions (syscalls via SVC, page faults, alignment faults), IRQ dispatch to `irq_handler_el1`, and SError/FIQ stubs. All unexpected exceptions dump registers to UART via direct `putc()` (not `println!()`, to prevent recursive faults). Advance phase: `ExceptionVectors`.

```text
Exception Vector Table (aligned to 2048 bytes):
  Offset    Exception             Handler
  ─────────────────────────────────────────
  0x000     Sync from current EL  sync_current_el_handler
  0x080     IRQ from current EL   irq_current_el_handler  → irq_handler_el1
  0x100     FIQ from current EL   fiq_stub (b .)
  0x180     SError from current   serror_stub (b .)
  0x200     Sync from lower EL    lower_el_sync_handler   (SVC dispatch)
  0x280     IRQ from lower EL     irq_lower_el_handler
  0x300     FIQ from lower EL     fiq_stub (b .)
  0x380     SError from lower EL  serror_stub (b .)
```

**Note on two-phase vector install:** boot.S installs stub vectors (Step 2 assembly) that are a safety net during the assembly-to-Rust transition. kernel_main installs the full Rust vectors (Step 2 Rust) that handle IRQs, SVCs, and fault diagnostics. The stubs are never invoked during normal boot.

**Step 3: DTB parse + platform detection.** If `boot_info.device_tree != 0`, parse the FDT (flattened device tree) using `fdt-parser`. Extract CPU count, PSCI method, interrupt controller and timer base addresses. If no DTB, fall back to QEMU virt defaults. Detect platform via root compatible string (see §2.6 Platform trait). Advance phase: `DeviceTreeParsed`.

**Step 4: Full PL011 UART initialization.** Call `platform.init_uart(&dt)`. This performs a full UART init sequence: set IBRD=13, FBRD=1 (115200 baud at 24 MHz APB clock), configure LCR_H for 8N1, enable TX+RX+UART via CR register. Advance phase: `UartReady`.

**Two-phase UART init:** The UEFI stub writes raw bytes to the UART data register after `ExitBootServices()` (before kernel entry). The kernel's `kinfo!()` macro also writes directly to UART before Step 4. Step 4's `init_uart` reconfigures the UART properly — prior output relied on edk2's initialization, which may not persist post-EBS on all firmware versions.

**Step 5a: TTBR0 identity map.** Build a TTBR0 identity map with 3 × 1GB block descriptors: 0–1GB (device memory, MAIR Attr0), 1–2GB (RAM, MAIR Attr3 WB), 2–3GB (RAM, MAIR Attr3 WB). This must run before GIC/timer init because QEMU edk2 firmware may not preserve device-memory MMIO mappings in TTBR0 post-ExitBootServices. TLBI uses `VMALLE1` + `DSB NSH` (non-broadcast; broadcast hangs with parked cores under NC memory). Advance phase: `MmuEnabled`.

**Step 5b: GICv3 + Timer.** Initialize the GICv3 distributor (GICD at `0x0800_0000`) and redistributor (GICR at `0x080A_0000`). Configure the CPU interface via ICC system registers. Then initialize the ARM Generic Timer: read `CNTFRQ_EL0` (62.5 MHz on QEMU), configure 1 kHz tick (62500 counts/tick), wire timer PPI (INTID 30) through GIC. Advance phases: `InterruptsReady`, `TimerReady`.

**Step 6: Buddy allocator + Slab heap.** Initialize physical memory from the UEFI memory map. Walk the memory descriptors, exclude kernel image, BootInfo, and MMIO regions. Partition into 4 pools: kernel (128 MB), user (1792 MB), model (0), DMA (64 MB). Initialize buddy allocator (orders 0–10, bitmap coalescing, poison fill on free). Advance phase: `PageAllocatorReady`.

Switch the global allocator from the 128 KiB static bump allocator to the slab allocator (5 size classes: 64, 128, 256, 512, 4096 bytes, magazine layer, red zones). Run a `Box<[u8; 1024]>` write/read/drop cycle to verify heap correctness. Advance phases: `HeapReady`, `LogRingsReady`. Flush ring-buffered log entries to UART via `drain_logs()`.

**Step 6b: KASLR + Full TTBR1.** Compute KASLR slide from `BootInfo.rng_seed` or `CNTPCT_EL0` (2 MiB aligned, 0–128 MB range). The slide is **computed and logged but not applied** — the kernel continues at the fixed base. Build full TTBR1 page tables with fine-grained 4KB W^X pages: `.text`=RX, `.rodata`=RO, `.data`/`.bss`=RW+NX. Add physical memory direct map at `DIRECT_MAP_BASE` (`0xFFFF_0001_0000_0000`, 2MB blocks) and MMIO map at `MMIO_BASE` (`0xFFFF_0010_0000_0000`, device memory). Switch TTBR1 to full tables, replacing boot.S minimal 2MB blocks, via `mm::kmap::init_kernel_address_space()`.

Enable direct-map mode for the buddy allocator (`mm::buddy::enable_direct_map()`). Convert slab allocator internal free-list pointers from physical to virtual addresses (`mm::slab::convert_to_direct_map()`). This must happen while the TTBR0 identity map is still active — before any TTBR0 switch to user address spaces. Flush log rings again via `drain_logs()`.

**Step 7: SMP secondary core bringup.** Via PSCI `CPU_ON` (`0xC400_0003`, HVC conduit on QEMU). Each secondary core executes `_secondary_entry` in boot.S: enable FPU → install Rust exception vectors → load MAIR/TCR/TTBR0/TTBR1 → enable MMU → load per-core SP from `SECONDARY_STACKS` array → branch to `secondary_main`. Secondary stacks are allocated by `smp.rs` during this step. Each secondary core initializes its own GIC redistributor and timer. Advance phase: `ProcessManagerReady`.

**Step 8: Framebuffer.** If `boot_info.framebuffer != 0`, construct a `Framebuffer` from BootInfo fields (800×600 Bgr8 on QEMU, stride=3200B). Render a test pattern (#5B8CFF blue). This is the early framebuffer — the compositor takes over in Phase 5+.

**Step 9: Per-agent address spaces.** Switch UART base to the TTBR1 MMIO mapping (`MMIO_BASE + UART_PHYS`) before TTBR0 is repurposed from identity map to user space (`uart::update_base()`). Test TTBR0 switching: create two user address spaces with independent ASIDs, map test pages at `USER_DATA_BASE`, switch between them to verify TTBR0 programming and that no fault occurs.

**Step 10: Scheduler init.** Call `sched::init()`: create idle threads (one per CPU, class=Idle), create test threads. Secondary cores are NOT released yet — they park in `enter_scheduler()` waiting on `SCHED_READY`. Flush log rings via `drain_logs()`.

**Step 11: IPC init.** Call `ipc::init()`: allocate test threads and processes, create IPC channels, test call/reply, direct switch, shared memory, notifications, and select. Must run while secondaries are still parked — `spin::Mutex` has no fairness, so secondaries could starve the boot CPU's access to `THREAD_TABLE`. Flush log rings via `drain_logs()`.

**Step 12: Service manager init.** Call `service::init()`: initialize the service registry, echo service, process lifecycle, and audit ring. Flush log rings via `drain_logs()`.

**Step 13: Storage init.** Probe for a VirtIO-blk device by scanning MMIO range `0x0A00_0000–0x0A00_3E00`. If found, call `storage::init()` to initialize the Block Engine: read or write the superblock, replay the WAL, and initialize the MemTable. Flush log rings via `drain_logs()`.

**Step 14: Benchmarks.** Call `bench::init()` to run Gate 1 benchmarks: IPC round-trip latency, context switch overhead, direct switch overhead, capability operation overhead, and shared memory throughput. Results are logged to UART. Flush log rings via `drain_logs()`.

**Step 15: Release secondaries + Enter scheduler.** Call `sched::start()` to set `SCHED_READY`, waking parked secondary cores. Flush log rings. Unmask IRQ (`msr DAIFClr, #0x2`) — timer interrupts now fire every 1ms. IRQ unmask happens after all boot initialization is complete: GIC, timer, vector table, and all handlers must be initialized first. Call `sched::enter_scheduler()` — picks the first ready thread and never returns.

```text
[boot] AIOS kernel booting...
[boot] BootInfo at 0x4bf3f000, magic OK
[boot] EL: 1
[boot] Core ID: 0
[boot] ExceptionVectors — 0ms
[boot] DeviceTreeParsed — QEMU virt
[boot] UartReady — 1ms
[boot] MmuEnabled — 1ms
[boot] InterruptsReady — 1ms
[boot] TimerReady — 1ms
[boot] PageAllocatorReady — 5ms
[boot] HeapReady — 5ms
[boot] LogRingsReady — 5ms
[boot] ProcessManagerReady — 8ms
[boot] Boot sequence complete, entering scheduler
```

### 3.4 PL011 UART for Early Debug

The PL011 UART is the first and last resort for debugging. It's initialized before anything else (after exception vectors) and remains available even after the display subsystem takes over. On Pi hardware, it's accessible via GPIO pins 14/15 (or the dedicated UART header on Pi 5).

**Two-phase UART init** (see also Step 4 in §3.3):

1. **Pre-init (boot.S → Step 3):** The UART works because edk2 left PL011 configured. The `kinfo!()` macro writes directly to the UART data register at the hardcoded base `0x0900_0000`. No baud rate programming, no lock.
2. **Full init (Step 4):** `init_pl011()` disables the UART, programs IBRD=13/FBRD=1 (115200 baud at 24 MHz APB clock), sets LCR_H for 8N1 with FIFOs, re-enables TX+RX+UART via CR. Updates the global `UART_BASE_ADDR: AtomicUsize`.

The kernel's structured logging macros (`kinfo!`, `kwarn!`, `kerror!` from `observability/mod.rs`) format into a 48-byte `LogEntry` message field and write to UART character-by-character via `UartWriter` (implements `core::fmt::Write`). No heap allocation — all formatting happens on the stack. Output is unprotected by locks in early boot (see `uart.rs` header comment: `spin::Mutex` hangs on NC memory); brief interleaving during SMP bringup is accepted.

After Step 6b (full TTBR1), the UART base is switched from the TTBR0 identity-mapped physical address to the TTBR1 MMIO virtual address (`MMIO_BASE + UART_PHYS`) via `uart::update_base()`. This must happen before TTBR0 is repurposed for user address spaces.

During normal operation, the UART is used by the recovery shell (see Section 9). In production builds, kernel log output to UART can be disabled via the command line (`quiet` flag in `boot.cfg`).

### 3.5 SMP Boot: Secondary CPU Bringup

The entire early boot sequence runs on core 0 (the boot CPU). Secondary cores are parked by boot.S Step 3 in a `wfe` loop. They are brought online in Step 7 of kernel_main, after the full TTBR1 and buddy allocator are ready.

**Why not earlier?** Secondary cores need: (1) per-core stacks allocated from the buddy allocator, (2) the full TTBR1 for MMIO access. The boot CPU builds all of this first.

**PSCI (Power State Coordination Interface):** The kernel discovers the PSCI conduit (HVC on QEMU, SMC on Pi) from the device tree `/psci` node. `smp.rs` converts the virtual `_secondary_entry` symbol to a physical address before calling PSCI CPU_ON (secondary cores start with MMU off).

**Actual bringup sequence** (source: `kernel/src/smp.rs`):

```text
Boot CPU (bring_secondaries_online):
  1. Allocate 16 KiB stack per secondary core (buddy order 2)
  2. Write stack top pointers to SECONDARY_STACKS[i]
  3. DSB SY barrier (ensure stack writes visible)
  4. Convert _secondary_entry virtual → physical (virt_to_phys)
  5. For each secondary: PSCI CPU_ON(mpidr, entry_phys, core_id)
  6. Wait for PRINT_TURN to reach cpu_count (100ms timeout)

Secondary core (_secondary_entry in boot.S → secondary_main in smp.rs):
  1. FPU enable (boot.S)
  2. Install Rust exception vectors at VBAR_EL1 (boot.S)
  3. Load MAIR/TCR/TTBR0/TTBR1, enable MMU (boot.S — safe: MMU was off)
  4. Load per-core SP from SECONDARY_STACKS[core_id] (boot.S)
  5. Branch to secondary_main(core_id) (Rust)
  6. Init GIC redistributor + CPU interface for this core
  7. Install full kmap TTBR1 (replaces boot.S minimal TTBR1)
  8. Wait for PRINT_TURN == core_id, print, store core_id + 1
  9. Init per-core timer (programs CNTP_TVAL_EL0 + CNTP_CTL_EL0)
  10. Enter scheduler (parks in wfe until SCHED_READY)
```

**Historical note: NC memory atomics.** The turn-based printing protocol uses only `load(Acquire)` / `store(Release)` — plain loads/stores with ordering, not exclusive pairs. This pattern was essential when the identity map used Non-Cacheable memory (MAIR Attr1=0x44). After TTBR0 RAM blocks are upgraded to Write-Back cacheable (MAIR Attr3=0xFF), `spin::Mutex` and exclusive pairs become safe. The conservative protocol remains as defense-in-depth — it works regardless of memory attributes.

**Critical: IRQ deferral.** Secondary cores do NOT unmask IRQs in `secondary_main`. Timer tick handlers use `compare_exchange` (exclusive monitor), which can starve the boot CPU's spinlock acquisitions during init. IRQs are unmasked later in `enter_scheduler()` after `SCHED_READY` is set.

**Per-platform core counts:**

```text
Platform            Cores   PSCI Conduit    Notes
──────────────────────────────────────────────────────────────────
QEMU (default)      4      HVC             Configurable via -smp
Raspberry Pi 4      4      SMC             Cortex-A72
Raspberry Pi 5      4      SMC             Cortex-A76
Apple M1            8      HVC             4× Firestorm + 4× Icestorm (big.LITTLE)
Apple M1 Pro/Max   10/10   HVC             8× Firestorm + 2× Icestorm (varies)
Apple M2 Pro       12      HVC             8× Avalanche + 4× Blizzard
```

**The `maxcpus=` command line option** limits how many secondary cores are brought online. `maxcpus=1` keeps the system single-core (useful for debugging race conditions). Default is all available cores.

**Timing:** SMP bringup takes ~3ms total on QEMU (PSCI call + per-core init + turn-based printing). By the time the scheduler is initialized, all cores are online and parked in `enter_scheduler()`.

### 3.6 SMMU / IOMMU: DMA Protection

Without an IOMMU, any DMA-capable device can read or write arbitrary physical memory — effectively bypassing all kernel page table isolation. On a capability-based OS, this is a critical gap: a compromised USB or network device could read kernel memory, steal capability tokens, or corrupt the provenance chain.

**ARM SMMU (System Memory Management Unit)** provides per-device address translation and access control for DMA transactions, analogous to Intel VT-d:

```text
Without SMMU:
  Device → DMA request (physical address) → RAM (any address!)

With SMMU:
  Device → DMA request (IOVA) → SMMU → translate via device page table
                                       → check permissions
                                       → physical address (restricted)
                                       → RAM (only allowed regions)
```

**Per-platform status:**

```text
Platform       SMMU Hardware          Status
──────────────────────────────────────────────────
QEMU           VirtIO IOMMU           Optional; enabled with -device virtio-iommu
               (or iommu=smmuv3)      Required for testing DMA isolation.
Pi 4           None                   No SMMU. DMA is unrestricted.
                                      Mitigation: restricted device drivers,
                                      bounce buffering for untrusted devices.
Pi 5           SMMU (in BCM2712)      Available. Configured during boot.
Apple Silicon  DART (Apple IOMMU)     Available. Per-device DARTs configured
                                      during boot. Different register interface
                                      from ARM SMMU — requires dedicated driver.
```

**When SMMU is initialized:** After the page allocator is ready (SMMU page tables need physical pages) but before the Service Manager launches any device-accessing services. Specifically:

1. **SMMU init (between page allocator and heap):** If the device tree contains an SMMU node (`/smmu` or `/iommu`), initialize the SMMU hardware: program the Stream Table (maps device stream IDs to per-device page tables), configure the Command Queue and Event Queue, and enable translation.
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
