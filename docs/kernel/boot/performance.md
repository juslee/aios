# AIOS Boot Performance and Early Framebuffer

Part of: [boot.md](../boot.md) — Boot and Init Sequence
**Related:** [kernel.md](./kernel.md) — Kernel early boot, [services.md](./services.md) — Service startup phases, [compositor.md](../../platform/compositor.md) — Display handoff

-----

## 6. Boot Performance Budget

### 6.1 Critical Path Timeline

The critical path is the sequence of steps that cannot be parallelized — each depends on the previous:

```text
Time (ms)     Phase                    What happens
──────────────────────────────────────────────────────────────────
   0          Firmware                 POST, DRAM, UEFI init
 500          Firmware complete        Load AIOS kernel from ESP
──────────────────────────────────────────────────────────────────
 510          Kernel entry             Exception vectors, UART
 530          Device tree parsed       Platform detected
 550          Interrupts + timer       Interrupt controller initialized
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
1350          Audio subsystem up       (non-critical, continues in bg)
1400          POSIX compat ready       BSD userland bridge
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

```text
Component                Target     Notes
─────────────────────────────────────────────────────────────────
Firmware                 ~500ms     Not controllable. DRAM training dominates.
                                    QEMU is faster (~200ms). Pi 4 is ~500ms.
                                    Pi 5 is ~400ms. Apple Silicon is ~300ms
                                    (m1n1 + U-Boot).
Kernel early boot         200ms     Most time in page table setup and
                                    device tree parsing.
Phase 1 (storage)         300ms     WAL replay adds time after dirty shutdown.
                                    First boot adds ~200ms for formatting.
Phase 2 (core services)   500ms     GPU init is the bottleneck.
                                    VirtIO-GPU is fast (~50ms).
                                    Pi VC4/V3D is slower (~200ms).
                                    Apple AGX is ~150ms (custom init).
Phase 4 (user services)   200ms     Identity unlock may block on user input
                                    (passphrase). Biometric is faster.
Phase 5 (experience)      150ms     First compositor frame.
─────────────────────────────────────────────────────────────────
Software critical path: ~1,350ms    Kernel + Phase 1–5, excludes firmware.
Total (with firmware):  ~1,850ms    Well under 3-second target.
```

### 6.3 Parallel vs Sequential Timeline

```text
Time: 0       500      1000      1500      2000      2500      3000
      |--------|---------|---------|---------|---------|---------|
      ████████████████████                                        Firmware
               ██████████                                         Kernel boot
                         ███████████                               Phase 1
                                    ████████████████               Phase 2
                                                   ·················> Phase 3 (AI, bg)
                                                   ███████         Phase 4
                                                          █████    Phase 5
                                                              ↑
                                                      BOOT COMPLETE
                                                        ~1,850ms
                                                     (includes firmware)

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

UEFI Runtime Services provide `GetTime()` which returns a wall-clock time from the platform's RTC (Real-Time Clock). On QEMU, this is the host's wall clock. On Pi 4/5, this is the hardware RTC if a battery-backed module is attached, or epoch (January 1, 2000) if not. On Apple Silicon, m1n1 provides an accurate RTC backed by the always-on SoC RTC block (battery-backed via the PMU). The kernel reads UEFI `GetTime()` once during early boot (after Step 6, timer setup) and stores it as `boot_wall_time` in `KernelState`. This provides a *best-effort* wall-clock time that may be inaccurate but is not zero.

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

```text
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

```text
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
