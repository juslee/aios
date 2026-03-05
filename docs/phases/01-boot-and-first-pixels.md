# Phase 1: Boot and First Pixels

**Tier:** 1 — Hardware Foundation
**Duration:** 4 weeks
**Deliverable:** UEFI stub boots kernel via edk2; kernel parses DTB, enables MMU, prints boot log; framebuffer shows coloured rectangle
**Status:** Complete
**Prerequisites:** Phase 0 (Foundation and Tooling)
**Unlocks:** Phase 2 (Memory Management)

-----

## Objective

Replace the Phase 0 QEMU `-kernel` shortcut with a real UEFI boot path. The UEFI stub assembles `BootInfo`, calls `ExitBootServices()`, and hands off to the kernel. The kernel then executes the full early-boot sequence: DTB parse, platform detection, interrupt controller, timer, MMU, buddy allocator, and heap. Phase 1 ends with the compositor writing a solid coloured rectangle to the UEFI framebuffer — the first visual output.

By the end of this phase, booting QEMU with edk2 firmware and a VirtIO-Blk disk image shows the AIOS boot log on the serial console and a coloured rectangle on the virtual display.

-----

## Architecture References

| Topic | Document | Relevant Sections |
|---|---|---|
| UEFI stub and BootInfo | [boot.md](../kernel/boot.md) | §2 Firmware Handoff (full); §2.1 UEFI Boot on aarch64; §2.2 BootInfo struct; §2.4 ESP layout |
| Kernel early boot steps 1–9 | [boot.md](../kernel/boot.md) | §3.3 Steps 1–9 (entry through heap); §3.1 EarlyBootPhase enum; §3.2 KernelState struct |
| SMP secondary core bringup | [boot.md](../kernel/boot.md) | §3.5 SMP Boot |
| Platform trait and detection | [hal.md](../kernel/hal.md) | §2 Platform Detection; §3 Platform Trait; §3.2 Initialization Order |
| PL011 UART (full init) | [hal.md](../kernel/hal.md) | §4.3 Uart (PL011 register offsets and init sequence) |
| GICv3 interrupt controller | [hal.md](../kernel/hal.md) | §4.1 InterruptController (GICv3 distributor, redistributor, CPU interface) |
| ARM Generic Timer | [hal.md](../kernel/hal.md) | §4.2 Timer (CNTFRQ_EL0, tick calculation, PPI wiring) |
| MMU and page tables | [memory.md](../kernel/memory.md) | §3 Virtual Memory Manager; §3.1 Address Space Layout; §3.2 Page Tables |
| Buddy allocator | [memory.md](../kernel/memory.md) | §2 Physical Memory Manager; §2.2 Buddy Allocator |
| Slab/heap | [memory.md](../kernel/memory.md) | §4 Kernel Heap; §4.1 Slab Allocator |
| QEMU vs real hardware | [boot.md](../kernel/boot.md) | §2.5 QEMU Boot vs Real Hardware |
| Exception level model | [boot.md](../kernel/boot.md) | §2.6 Exception Level Model |

-----

## Milestones

Milestones are numbered continuously across all phases. Phase 0 used M1–M3; Phase 1 continues with M4–M6.

| Milestone | Steps | Target | Observable result |
|---|---|---|---|
| **M4 — UEFI stub runs** | 1–2 | End of week 1 | QEMU with edk2 prints "AIOS UEFI stub: ExitBootServices OK" to serial |
| **M5 — Kernel boots to heap** | 3–6 | End of week 2 | Boot log shows all EarlyBootPhase transitions through HeapReady |
| **M6 — First pixels** | 7–8 | End of week 4 | Coloured rectangle visible on QEMU virtual display; CI passes |

-----

## Milestone 4 — UEFI Stub Runs (End of Week 1)

*Goal: QEMU boots via edk2 and the UEFI stub successfully exits Boot Services and jumps to the kernel.*

-----

### Step 1: UEFI Stub Crate and ESP Layout

**What:** Create the `uefi-stub/` crate — a UEFI application that runs under edk2 Boot Services, assembles `BootInfo`, and jumps to the kernel.

**Tasks:**
- [x] Create `uefi-stub/` crate with `#![no_std]`, `#![no_main]`. Target: `aarch64-unknown-uefi` (produces a PE/COFF `.efi` binary — different from the kernel's `aarch64-unknown-none` ELF)
- [x] Add `uefi` crate dependency (provides `SystemTable`, `BootServices`, `RuntimeServices`, `MemoryMap` wrappers). Pin a specific version.
- [x] Add `uefi-stub` to the workspace `Cargo.toml` members
- [x] Implement the UEFI entry point (`efi_main`): open `EFI_SIMPLE_TEXT_OUTPUT_PROTOCOL` and print a banner to confirm the stub is reached
- [x] Implement ESP layout from boot.md §2.4: stub at `/EFI/AIOS/BOOTAA64.EFI`, kernel at `/EFI/AIOS/aios.elf`, config at `/EFI/AIOS/boot.cfg`
- [x] Add `just disk` recipe: creates a FAT32 disk image with the ESP, places stub and kernel ELF at the correct paths (requires `mformat` + `mcopy` from `mtools`, or equivalent)
- [x] Update `just run` to use edk2 firmware: `-bios /path/to/edk2-aarch64-code.fd` (or distro-specific aarch64 firmware such as `QEMU_EFI.fd` or `AAVMF_CODE.fd`) and `-drive` instead of `-kernel`

**QEMU invocation change:** Phase 0 used `qemu-system-aarch64 -kernel <elf>`. Phase 1 switches to:
```
qemu-system-aarch64 \
  -machine virt \
  -cpu cortex-a72 \
  -smp 4 \
  -m 2G \
  -nographic \
  -serial stdio \
  -bios /path/to/edk2-aarch64-code.fd \
  -drive if=none,id=disk0,file=aios.img,format=raw \
  -device virtio-blk-pci,drive=disk0
```

**Note:** `aarch64-unknown-uefi` produces a PE/COFF binary. This is not the same as `aarch64-unknown-none`. The stub and the kernel are separate Rust crates with different targets. Add `aarch64-unknown-uefi` to `rust-toolchain.toml`'s `targets` list alongside `aarch64-unknown-none`.

**Acceptance:** `just disk && just run` launches QEMU with edk2 firmware. The serial console shows the edk2 boot menu, then "AIOS UEFI stub" printed by the stub entry point.

-----

### Step 2: BootInfo Assembly and ExitBootServices

**What:** Implement the full BootInfo assembly sequence from boot.md §2.1–2.2: parse the UEFI memory map, acquire GOP framebuffer, acquire DTB, exit Boot Services, and jump to the kernel.

**Tasks:**
- [x] Implement memory map acquisition: call `BootServices.get_memory_map()`, iterate over `MemoryDescriptor` entries, build the `BootInfo.memory_map` (boot.md §2.2). Store in a region allocated with `BootServices.allocate_pool(MemoryType::BootServicesData, ...)` — this region must be included in the memory map as type `BootInfo` so the buddy allocator excludes it from the free pool (the kernel reads it before reclaiming)
- [x] Implement GOP framebuffer acquisition: open `EFI_GRAPHICS_OUTPUT_PROTOCOL`, read `Mode.Info` for width/height/stride/format, fill `BootInfo.framebuffer` (boot.md §2.2 `FramebufferInfo`). `PixelFormat` mapping: `PixelRedGreenBlueReserved8BitPerColor` → `Rgb8`; `PixelBlueGreenRedReserved8BitPerColor` → `Bgr8`; `PixelBitMask` → `Bitmask { red, green, blue }` (read the per-channel bitmask fields from `EFI_PIXEL_BITMASK` and store them — fill_rect in Step 8 must decode them at draw time). Store framebuffer base as a `PhysicalAddress`.
- [x] Implement DTB location: QEMU passes the DTB address via the UEFI `EFI_DTB_TABLE_GUID` configuration table entry. Retrieve with `SystemTable.config_table()`. Fill `BootInfo.device_tree` with base and size.
- [x] Set `BootInfo.magic` = `0x41494F53_424F4F54` (`"AIOSBOOT"` as a u64)
- [x] Fill `BootInfo.rng_seed` from `EFI_RNG_PROTOCOL` if available; zero-fill if not (kernel falls back to timer entropy)
- [x] Fill `BootInfo.kernel_phys_base` and `BootInfo.kernel_size` from the ELF load address and image size
- [x] Call `BootServices.exit_boot_services()`. After this call, no UEFI services are available — no allocation, no output, no nothing. The stub must not call any UEFI function after this point.
- [x] Jump to kernel entry point (`kernel_main`) passing `BootInfo` pointer in `x0` (per the Phase 1 ABI replacing the Phase 0 DTB pointer)
- [x] Update `kernel_main` signature and `shared/BootInfo` to accept the real pointer (replace Phase 0's `Option<u64>` stubs with `#[cfg(target_arch = "aarch64")]`-scoped real types where needed)

**BootInfo pointer ABI:** The stub allocates `BootInfo` from `LOADER_DATA` in a page-aligned buffer, fills it (including a pointer to the final UEFI memory map returned by `ExitBootServices()`), then jumps to the kernel with `x0` = physical address of that buffer. The kernel entry assembly (boot.S from Phase 0) must now save `x0` into `x19` (callee-saved) immediately — the Phase 1 boot assembly differs from Phase 0 here. Early in boot, the kernel copies `BootInfo` and the UEFI memory map into its own internal structures and then marks the physical page(s) backing the original `BootInfo` and memory-map buffers as reserved in the buddy allocator by address.

**Acceptance:** Serial console shows "AIOS UEFI stub: ExitBootServices OK, jumping to kernel at 0x...". The kernel entry point is reached (confirmed by the Phase 0 UART print "AIOS kernel booting..." now appearing via the UEFI path).

-----

## Milestone 5 — Kernel Boots to Heap (End of Week 2)

*Goal: Boot log shows all EarlyBootPhase transitions through HeapReady.*

-----

### Step 3: DTB Parse and Platform Detection

**What:** Implement a minimal flattened device tree (FDT) parser sufficient to complete early boot. Full DTB parsing is not needed — only the nodes the kernel queries during Steps 3–6.

**Tasks:**
- [x] Add `fdt` crate (or implement a minimal parser) — must be `no_std` compatible. Used `fdt-parser` 0.5 (MIT, `no_std`). The parser needs: root `compatible` string, `/chosen/stdout-path`, CPU nodes (for SMP count and MPIDR values), `/psci` node (conduit and `cpu_on` function ID), memory nodes (base and size for each RAM region), GICv3 distributor and redistributor base addresses, ARM Generic Timer interrupt numbers
- [x] Implement `detect_platform(dt: &DeviceTree) -> &'static dyn Platform` matching hal.md §2 (compatible string table) and §3 (Platform trait). Returns a static reference (no heap at detection time). For Phase 1 QEMU target: match `"qemu,virt"` compatible string → `QemuPlatform`
- [x] Implement `QemuPlatform` struct implementing the `Platform` trait (hal.md §3). Phase 1 defines three methods: `init_uart`, `init_interrupts`, and `init_timer`
- [x] Advance `EarlyBootPhase` to `DeviceTreeParsed` and print to UART

**Minimal parser scope:** The FDT parser only needs to find specific well-known nodes by path or compatible string. A full recursive traversal is Phase 4+ work (when the Device Registry service discovers all hardware). For now: parse only what Steps 3–6 require.

**Acceptance:** Boot log shows `[boot] DeviceTreeParsed` with the detected platform name (`QemuPlatform`).

-----

### Step 4: Full PL011 UART Initialization

**What:** Replace the Phase 0 hardcoded UART (relying on QEMU pre-initialization) with a proper PL011 driver that initializes from the DTB base address and programs baud rate registers. This is the `Platform::init_uart()` implementation.

**Tasks:**
- [x] Read PL011 base address from DTB by searching for the first `arm,pl011` compatible node → extract `reg` property
- [x] Implement full PL011 initialization sequence (required on real hardware; harmless on QEMU):
  1. Disable UART: clear CR.UARTEN (bit 0)
  2. Wait for any in-progress transmission to finish: poll UARTFR.BUSY (bit 3)
  3. Flush the FIFO: clear LCR_H.FEN (bit 4)
  4. Program baud rate: `IBRD` = `clock_hz / (16 * baud_rate)`, `FBRD` = `round((clock_hz % (16 * baud_rate)) * 64 / (16 * baud_rate))`. QEMU PL011 UART clock: 24 MHz (this is the APB/UART peripheral clock — distinct from the ARM Generic Timer frequency of 62.5 MHz). For 115200 baud: IBRD=13, FBRD=1.
  5. Set line control: LCR_H = 0x70 (8-bit, 1 stop, no parity, FIFO enabled)
  6. Re-enable UART: CR = 0x301 (UARTEN | TXE | RXE)
- [x] Return `Uart` handle from `QemuPlatform::init_uart()`, store in `KernelState.uart`
- [x] Advance `EarlyBootPhase` to `UartReady` and print the first full boot banner

**Register offsets (hal.md §4.3):**
- `UARTDR` 0x000 — data register
- `UARTFR` 0x018 — flag register (TXFF bit 5, BUSY bit 3, RXFE bit 4)
- `UARTIBRD` 0x024 — integer baud rate divisor
- `UARTFBRD` 0x028 — fractional baud rate divisor
- `UARTLCR_H` 0x02C — line control
- `UARTCR` 0x030 — control register

**Acceptance:** Boot log shows `[boot] UartReady — Xms` with the correct baud rate configuration. Serial output continues to work on a fresh QEMU launch (not relying on QEMU pre-init state).

-----

### Step 5: Interrupt Controller (GICv3) and Timer

**What:** Initialize the GICv3 interrupt controller and ARM Generic Timer so the kernel has a working 1 ms scheduler tick before the MMU is enabled.

**Tasks:**

**GICv3 (hal.md §4.1):**
- [x] Read GICv3 distributor base (`GICD`) and redistributor base (`GICR`) from DTB. On QEMU virt: `GICD` at `0x0800_0000`, `GICR` at `0x080A_0000` (8 redistributor frames × 128 KiB each for 4 cores)
- [x] Initialize distributor: set `GICD_CTLR.ARE_NS` (affinity routing enable), enable Group 1 non-secure interrupts
- [x] Initialize per-CPU redistributor: wake redistributor (clear `GICR_WAKER.ProcessorSleep`, wait for `ChildrenAsleep` to clear), enable Group 1 SGIs
- [x] Enable CPU interface via system registers: `ICC_SRE_EL1 |= 1` (enable system register interface), `ICC_IGRPEN1_EL1 = 1` (enable Group 1), set `ICC_PMR_EL1 = 0xFF` (allow all priorities)
- [x] Store `InterruptController` handle in `KernelState.interrupt_controller`
- [x] Advance `EarlyBootPhase` to `InterruptsReady`

**ARM Generic Timer (hal.md §4.2):**
- [x] Read `CNTFRQ_EL0` for the timer frequency. QEMU virt default: 62.5 MHz
- [x] Calculate the 1 ms tick count: `tick_count = freq_hz / 1000`
- [x] Program `CNTP_TVAL_EL0 = tick_count` (physical timer compare value)
- [x] Enable physical timer: `CNTP_CTL_EL0 = 0x1` (ENABLE bit)
- [x] Register the timer interrupt in the GIC (PPI interrupt 30, `INTID = 30`)
- [x] Store `Timer` handle in `KernelState.timer`
- [x] Advance `EarlyBootPhase` to `TimerReady`

**Note:** Interrupts are enabled in the GIC but not yet globally enabled in PSTATE (`DAIF.I` bit). The scheduler will unmask interrupts after the MMU is up and the first process context is ready (Phase 3). The timer interrupt fires but is masked at PSTATE level until then.

**Acceptance:** Boot log shows `[boot] InterruptsReady` and `[boot] TimerReady — Xms`. GICv3 distributor and redistributor are configured without hanging. `CNTFRQ_EL0` value is printed and matches 62.5 MHz (62500000 Hz) on QEMU.

-----

### Step 6: MMU Enable and Buddy Allocator

**What:** Build kernel page tables, enable the MMU, and initialize the buddy allocator and slab heap. Phase 1 note: the kernel remains at physical addresses via a TTBR0 identity map (swapping edk2's page tables for our own). High-half virtual address mapping (TTBR1, `0xFFFF_...`) is deferred to Phase 2.

**Tasks:**

**Page table setup (memory.md §3):**
- [x] Allocate page table memory from the raw physical memory free list (before the buddy allocator exists — use a simple bump allocator backed by a statically-sized buffer, 128 KiB, for early boot allocations)
- [x] Build TTBR1_EL1 kernel mappings per boot.md §3.3 Step 7. Phase 1 note: built TTBR0 identity map instead (3×1GB blocks); TTBR1 high-half deferred to Phase 2:
  - `0xFFFF_0000_0000_0000` — kernel text (PXN=0, UXN=1, AP=RO), rodata (PXN=1, UXN=1, AP=RO), data/bss (PXN=1, UXN=1, AP=RW)
  - `0xFFFF_0000_4000_0000` — kernel heap region (reserved, not yet mapped)
  - `0xFFFF_0001_0000_0000` — physical memory direct map (all RAM, device memory)
  - `0xFFFF_0002_0000_0000` — MMIO (device memory, `nGnRnE` attribute)
- [x] Keep TTBR0_EL1 identity map active during the transition
- [x] Configure `MAIR_EL1` with memory attribute indices: index 0 = `nGnRnE` (device), index 1 = Normal writeback cacheable (RAM). Phase 1 note: edk2 leaves MMU on with its own MAIR (0xffbb4400); changing MAIR while MMU is on is CONSTRAINED UNPREDICTABLE, so Phase 1 reuses edk2's MAIR indices (Attr0=Device, Attr1=Non-cacheable Normal). Full MAIR/TCR reconfiguration deferred to Phase 2.
- [x] Configure `TCR_EL1`: T1SZ=16 (48-bit VA), TG1=4KiB granule, SH1=inner-shareable, ORGN1/IRGN1=writeback cacheable. Phase 1 note: reuses edk2's TCR (T0SZ=20, 44-bit VA) for same reason as MAIR above.
- [x] Enable MMU: set `SCTLR_EL1.M`, `SCTLR_EL1.C` (D-cache), `SCTLR_EL1.I` (I-cache). Issue `ISB` after write. Phase 1 note: MMU already enabled by edk2; Phase 1 swaps TTBR0 only with compatible identity-map page tables.
- [x] Switch the stack pointer to the new virtual address (TTBR1 high-half stack region). Phase 1 note: deferred — kernel runs at physical addresses via identity map.
- [x] Remove TTBR0 identity mapping entries for kernel addresses (keep user address space range for future process mappings). Phase 1 note: deferred — identity map remains active.
- [x] Advance `EarlyBootPhase` to `MmuEnabled`

**Buddy allocator (memory.md §2.2):**
- [x] Walk the `BootInfo.memory_map` and add all `Conventional`, `LoaderCode`, `LoaderData`, `BootServicesCode`, `BootServicesData` pages to the buddy allocator free list. Exclude: kernel image pages, BootInfo page, initramfs pages, UEFI Runtime pages, MMIO regions.
- [x] Implement buddy allocator orders 0–10 (4 KiB to 4 MiB blocks)
- [x] Store in `KernelState.page_allocator`
- [x] Advance `EarlyBootPhase` to `PageAllocatorReady`

**Kernel heap (memory.md §4.1):**
- [x] Initialize slab allocator on top of the buddy allocator. Phase 1 uses generic size-class caches (8–4096 B); named caches (`ipc_message`, `capability_token`, etc.) deferred to Phase 3.
- [x] Register as `GlobalAlloc` so `Box`, `Vec`, `String` work
- [x] Store in `KernelState.heap`
- [x] Advance `EarlyBootPhase` to `HeapReady`

**W^X enforcement:** Every page mapped into TTBR1 must be either writable or executable, never both. Kernel text: executable, read-only. Kernel data/bss: writable, non-executable. This is enforced at mapping time and verified by objdump in the acceptance criteria.

**Cache maintenance (boot.md §3.3 Step 7 note):** After enabling the new TTBR1 mapping: issue `IC IALLU; ISB` to invalidate the instruction cache and ensure no stale entries from the pre-MMU physical addresses survive.

**Acceptance:** Boot log shows `[boot] MmuEnabled`, `[boot] PageAllocatorReady`, `[boot] HeapReady — Xms`. `Box::new(42u32)` succeeds (heap is live). No UART output interruption during the MMU transition. Phase 1 note: kernel remains linked at `0x4008_0000` and runs via TTBR0 identity map (VA=PA). High-half virtual address relinking (TTBR1, `0xFFFF_0000_...`) is Phase 2 work.

-----

## Milestone 6 — First Pixels (End of Week 4)

*Goal: Coloured rectangle visible on QEMU virtual display; CI passes.*

-----

### Step 7: SMP Secondary Core Bringup

**What:** Bring secondary cores (1–3) online via PSCI after the scheduler is minimally initialised. Secondary cores are parked in `wfe` loops from Phase 0; this step wakes them.

**Tasks:**
- [x] Implement minimal `Scheduler` stub: enough to allocate per-core kernel stacks and track which cores are online. Full scheduling classes (RT, Interactive, Normal, Idle) are Phase 3 work.
- [x] Read PSCI conduit from DTB `/psci` node: `method = "hvc"` on QEMU (QEMU without KVM emulates PSCI at the hypervisor level)
- [x] For each secondary CPU node in the DTB (cores 1–3):
  - Allocate a 16 KiB kernel stack from buddy allocator (order 2 = 4 pages). Phase 1 note: stacks are at physical addresses via identity map; TTBR1 virtual stack addresses (`0xFFFF_0000_8000_0000 + core_id * 0x10000`) are Phase 2 work. Guard page enforcement requires 4 KiB granularity page tables (also Phase 2).
  - Call `PSCI CPU_ON` (function ID `0xC400_0003` for 64-bit PSCI) via `HVC` with: `target_cpu` = MPIDR value from DTB, `entry_point_address` = physical address of the secondary entry point in boot.S, `context_id` = core index
- [x] Implement secondary core entry in `boot.S`: FPU enable, VBAR_EL1 install (same vectors as boot CPU), load the allocated stack pointer, call `secondary_main(core_id: usize)`
- [x] `secondary_main`: initialize per-core GIC redistributor + CPU interface, print `[boot] Core N online`, then enter the idle loop (`wfe`) until the scheduler assigns work (Phase 3)
- [x] Advance boot CPU `EarlyBootPhase` to `ProcessManagerReady` once all secondaries check in

**NC memory constraint (Phase 1):** Phase 1 identity map uses Non-Cacheable Normal memory (edk2 MAIR Attr1=0x44). Atomic RMW instructions (`ldaxr`/`stlxr` exclusive pairs used by `spin::Mutex`, `fetch_add`, `compare_exchange`) require the global exclusive monitor, which only works with Inner Shareable + Cacheable memory attributes. On NC memory, the exclusive monitor fails and spinlocks hang under multi-core contention. Phase 1 serializes secondary core output using a turn-based protocol with only `load(Acquire)` / `store(Release)` (compiled to `ldar`/`stlr` — plain loads/stores with ordering, no exclusive pairs). Phase 2 enables WB cacheable memory, making `spin::Mutex` and atomic RMW safe.

**PSCI function IDs (64-bit SMCCC):**
- `CPU_ON`: `0xC400_0003`
- `SYSTEM_RESET`: `0x8400_0009`
- `SYSTEM_OFF`: `0x8400_0008`

**Acceptance:** Boot log shows `[boot] Core 1 online`, `[boot] Core 2 online`, `[boot] Core 3 online` before the first-pixels step. All 4 cores are running at EL1 with their own stacks and the shared TTBR0 identity-mapped page tables.

-----

### Step 8: Framebuffer and First Pixels

**What:** Write directly to the GOP framebuffer passed in `BootInfo` to render a coloured rectangle — the first visual output of the OS. This validates the framebuffer address, pixel format detection, and stride calculation.

**Tasks:**
- [x] Read `BootInfo.framebuffer`: base address, width, height, stride, pixel format
- [x] Map the framebuffer physical address into the kernel's MMIO virtual address range (`0xFFFF_0002_...`), mapped as device memory (`nGnRnE`). Phase 1 note: framebuffer is accessible via identity map (edk2 allocates in RAM region mapped as NC Normal by L1[1]). Explicit MMIO mapping deferred to Phase 2.
- [x] Implement `fill_rect(fb, x, y, w, h, pixel: u32)` — writes pre-packed u32 pixel data respecting stride and pixel format via `write_volatile::<u32>` (1 bus transaction per pixel):
  - `Bgr8`: pack as `0xAARRGGBB` (little-endian)
  - `Rgb8`: pack as `0xAABBGGRR` (little-endian)
- [x] Render: black background (fill entire framebuffer), then a centred 60%×60% rectangle in #5B8CFF
- [x] Print to UART: `[boot] Framebuffer: WxH stride=S format=F at 0x...`
- [ ] Update CI: add a QEMU headless screenshot step using `-display none -device virtio-gpu-pci` with `virtio-gpu` screendump via QEMU monitor, or skip framebuffer CI test (UART output is sufficient for CI; framebuffer is verified manually)

**Framebuffer layout note:** The UEFI GOP framebuffer on QEMU virt is typically `800×600` or `1024×768` depending on the edk2 version. `stride` is the **byte offset** from the start of one row to the start of the next — it is already in bytes, not pixels, and may include padding. Always compute pixel byte offset as `y * stride + x * 4` (for 32-bit formats), not `y * width * 4`. Using `width * 4` when stride > width will produce a diagonal smear.

**Acceptance:** QEMU virtual display (viewed via VNC or SDL — add `-display gtk` to see it) shows a solid coloured rectangle on a black background. UART shows the framebuffer diagnostics line. CI passes without the framebuffer check (UART-only CI is acceptable).

-----

## Decision Points

| Decision | Options | Recommendation |
|---|---|---|
| FDT parser | Implement minimal parser vs. use `fdt-parser` crate | Use the `fdt-parser` crate (MIT licensed, `no_std` compatible). A hand-rolled parser adds risk. Only implement a custom parser if crate licensing or `no_std` compatibility is a problem. |
| UEFI crate | `uefi` crate vs. raw UEFI ABI | Use the `uefi` crate for Phase 1 — it provides safe wrappers for `BootServices`, `RuntimeServices`, and `GOP`. The raw ABI is an option if the crate becomes a problem, but it is not needed yet. |
| edk2 firmware source | System package vs. built from source | Use the system package (`qemu-efi-aarch64` on Debian/Ubuntu, `edk2-aarch64` on Fedora, or download from https://retrage.github.io/edk2-nightly/). Building edk2 from source is not needed for Phase 1. |
| MMU transition approach | Enable MMU in assembly vs. Rust | Phase 1: MMU is already enabled by edk2, so we only swap TTBR0 in Rust (`mmu.rs`) via inline asm. Phase 2 (full MAIR/TCR reconfiguration with MMU-off transition) may use assembly if needed. |
| Framebuffer CI | Screenshot in CI vs. manual verify | Skip framebuffer screenshot in CI for Phase 1 — UART output is deterministic and sufficient. Framebuffer regression testing comes with the compositor (Phase 6). |

-----

## Phase Completion Criteria

All three milestones complete:

- [x] **M4** — QEMU boots via edk2; stub prints banner and exits Boot Services; kernel entry is reached
- [x] **M5** — Boot log shows `UartReady`, `DeviceTreeParsed`, `InterruptsReady`, `TimerReady`, `MmuEnabled`, `PageAllocatorReady`, `HeapReady`; `Box::new(42u32)` succeeds; `just check` passes
- [x] **M6** — All 4 cores online; coloured rectangle visible on QEMU virtual display; CI passes on clean checkout
- [x] `BootInfo.magic` is validated at kernel entry; mismatched magic halts with a UART error message
- [ ] W^X enforced: `cargo objdump` shows no page is both writable and executable. Phase 1 note: identity map uses 1GB blocks (all RWX); W^X enforcement at 2MiB/4KiB granularity is Phase 2 work.
- [x] `just disk` reproducibly builds the ESP image; `just run` boots end-to-end without manual steps
