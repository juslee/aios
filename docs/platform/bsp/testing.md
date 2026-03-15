# AIOS BSP Testing Strategy

Part of: [bsp.md](../bsp.md) — Board Support Package Architecture
**Related:** [platforms.md](./platforms.md) — Per-platform hardware, [model.md](./model.md) — BSP model & porting guide

---

## §11 Testing Strategy

BSP testing spans three environments: QEMU-based CI (automated, every push), DTB overlay emulation (semi-automated, platform-specific parsing), and real hardware (manual, gate-keeping). Each layer catches a different failure class. QEMU catches regressions. DTB overlays catch platform detection and driver selection bugs without requiring physical hardware. Real hardware catches MMIO address errors, timing assumptions, and hardware errata that emulators paper over.

### §11.1 QEMU-Based CI

QEMU is the primary testing platform. All CI runs on `virt` machine because it is the only QEMU machine with a faithful GICv3, PL011, ARM Generic Timer, and VirtIO stack — the full set of peripherals the kernel depends on.

**Machine configuration:**

```text
qemu-system-aarch64 \
  -machine virt,gic-version=3 \
  -cpu cortex-a72 \
  -smp 4 \
  -m 2G \
  -nographic \
  -bios /opt/homebrew/share/qemu/edk2-aarch64-code.fd \
  -drive if=none,id=disk0,file=aios.img,format=raw \
  -device virtio-blk-pci,drive=disk0 \
  -drive if=none,id=data0,file=data.img,format=raw \
  -device virtio-blk-device,drive=data0 \
  -device ramfb
```

**CI pipeline** (`.github/workflows/ci.yml`), triggered on every push:

| Job | Command | Pass condition |
|---|---|---|
| Check (fmt + clippy + build) | `just check` | Zero diff from `rustfmt`, zero clippy warnings, zero build warnings |
| Build (release) | `just build-release` | Zero warnings |
| Test (host) | `just test` | All unit tests pass |
| Security (audit + deny) | `just audit` + `just deny` | No known vulnerabilities, license compliance |
| Miri (unsafe UB detection) | `just miri` | No undefined behavior detected |

**Boot test timeout budget:**

| Phase | Timeout | Rationale |
|---|---|---|
| Full boot sequence | 30 seconds | Complete `kernel_main` through all `EarlyBootPhase` steps |
| Per-step validation | 10 seconds | Single milestone acceptance criterion match |
| Storage self-test | 15 seconds | Block engine WAL replay + write + read round-trip |
| SMP bringup | 5 seconds | All 4 secondary cores report in |

**UART string matching** uses exact substring match. The accept script reads from `-nographic` stdout and exits 0 on match, 1 on timeout:

```bash
#!/usr/bin/env bash
# scripts/boot-test.sh — run QEMU and match a UART string
EXPECTED="${1:?usage: boot-test.sh <expected-string>}"
timeout 30 qemu-system-aarch64 [flags] 2>&1 | grep -qF "$EXPECTED"
```

The expected string per phase is defined in each phase doc's "Acceptance:" block. Example from Phase 0: `"AIOS kernel booted"`. Example from Phase 2 M8: `"kernel address space ready"`. The string must appear verbatim on a single output line.

**Metrics tracked per CI run:**

| Metric | Source | Alert threshold |
|---|---|---|
| Boot time (UART first line) | Timestamp delta | > 5 seconds |
| Memory free at boot | UART diagnostic | < 480K pages on 2G |
| IPC round-trip (bench) | Gate 1 bench output | > 10 µs |
| Context switch time | Gate 1 bench output | > 5 µs |

### §11.2 Platform Emulation

QEMU cannot faithfully emulate Pi 4, Pi 5, or Apple Silicon hardware. `qemu-system-aarch64 -machine raspi4b` exists but omits GIC-400, uses a different interrupt model, and does not support VirtIO. Testing real Pi hardware paths requires either physical boards or DTB overlay injection.

**DTB overlay testing approach:**

A Pi 4-like DTB is hand-crafted with GICv2 base addresses (`0xFF84_1000` for GICD, `0xFF84_2000` for GICC — matching platforms.md §5.2) but run against the `virt` machine for the CPU and timer. The kernel's `detect_platform()` function reads compatible strings from the DTB — it will see `"raspberrypi,4-model-b"` and select `Pi4Platform`. The MMIO accesses will fault (addresses do not exist on `virt`), but the test validates that:

1. The correct platform struct is selected.
2. The DTB parser extracts the right peripheral addresses.
3. Driver selection code reaches the right `init_*` dispatch.

The test exits before attempting MMIO. A panic with the message `"Pi4Platform::init_uart called"` counts as a pass for the detection test.

**DTB overlay build:**

```text
scripts/dtb/pi4-overlay.dts   — Pi 4 compatible strings + GIC-400 addresses
scripts/dtb/pi5-overlay.dts   — Pi 5 compatible strings + GICv3 + RP1 stub
scripts/dtb/apple-m1.dts      — Apple M1 compatible string + AIC stub
```

Compile with `dtc -I dts -O dtb -o overlay.dtb overlay.dts`. Pass to QEMU with `-dtb overlay.dtb` to override the generated device tree.

**Platform coverage matrix:**

| Platform | QEMU `virt` CI | DTB overlay detection | Real hardware |
|---|---|---|---|
| QEMU (`virt`) | Full | N/A | N/A |
| Pi 4 (BCM2711) | Partial (VirtIO paths only) | Detection test | Manual |
| Pi 5 (BCM2712 + RP1) | Partial (VirtIO paths only) | Detection test | Manual |
| Apple Silicon (M-series) | Partial (VirtIO paths only) | Detection test | Manual |

**QEMU limitations that DTB overlays cannot cover:**

- RP1 south bridge (Pi 5): custom PCIe-attached I/O die with different UART, USB, and GPIO controllers. No emulation available.
- Apple AIC (Apple Interrupt Controller): fundamentally different architecture from GIC. Not emulatable on `virt`.
- BCM2711 PCIe controller (Pi 4): used for USB 3.0 and NVMe. QEMU `virt` uses VirtIO-blk instead.
- Platform-specific DMA coherency domains: QEMU does not model cache coherency differences between IOMMU configurations.

### §11.3 Real Hardware Testing

Real hardware testing gates each BSP port before it is declared production-ready. The procedure requires physical access and is done manually, though the test harness is scripted.

**Physical setup:**

| Component | Pi 4 / Pi 5 | Apple Silicon |
|---|---|---|
| Serial console | FTDI FT232R or CP2102 USB-to-UART on GPIO pins 14/15 (TXD/RXD) | USB-C debug cable (Mac-to-Mac or USB-C serial adapter) |
| Baud rate | 115200, 8N1 | 115200, 8N1 |
| Terminal | `minicom -b 115200 -D /dev/ttyUSB0` | `screen /dev/cu.usbserial-* 115200` |
| Boot medium | SD card (Pi) or USB drive (Pi 5 optional) | USB drive |
| JTAG | SWD via Raspberry Pi Debug Probe + OpenOCD | Custom Apple Silicon probe (limited availability) |

**SD card and disk imaging:**

```bash
# Flash Pi 4 / Pi 5
just flash-pi4 /dev/diskN   # macOS: /dev/disk2, Linux: /dev/sdb
# or manually:
dd if=aios.img of=/dev/diskN bs=4M conv=fsync status=progress
```

The `just flash-pi4` recipe writes the ESP FAT32 image to the first partition and a raw `data.img` to the second partition. The Pi bootloader chainloads `BOOTAA64.EFI` from the ESP.

**Network boot (TFTP) for faster iteration:**

TFTP boot eliminates SD card write cycles during driver development. Configure the Pi's `config.txt` for network boot, then serve the UEFI stub and kernel ELF from a TFTP server:

```text
# config.txt (Pi 4 / Pi 5)
dtoverlay=disable-bt
enable_uart=1
arm_64bit=1
# TFTP boot: set BOOT_ORDER in EEPROM to 0x21 (network then SD)
```

```bash
# Host TFTP server
sudo python3 -m tftpy.TftpServer -r /path/to/esp -i 0.0.0.0 &
```

Network boot reduces iteration time from ~90 seconds (SD write + insert + boot) to ~15 seconds (rebuild + network transfer + boot).

**Test harness (expect script):**

The test harness reads UART output line by line and matches against a sequence of expected strings. Each string corresponds to one boot phase acceptance criterion. The harness records timestamps and reports which step passed or failed:

```python
#!/usr/bin/env python3
# scripts/hw-test.py — real hardware boot test harness
import serial, time, sys

STEPS = [
    ("UART working",         "AIOS kernel"),
    ("EL level",             "EL = 1"),
    ("Platform detected",    "Pi4Platform"),   # or Pi5Platform, etc.
    ("Timer frequency",      "62500000 Hz"),   # QEMU; real Pi: 54000000 Hz
    ("Core count",           "4 cores"),
    ("Memory detected",      "RAM:"),
    ("Interrupts working",   "timer tick"),
    ("SMP bringup",          "core 3 ready"),
]

port = serial.Serial(sys.argv[1], 115200, timeout=60)
for label, pattern in STEPS:
    deadline = time.time() + 30
    while time.time() < deadline:
        line = port.readline().decode(errors="replace").strip()
        if pattern in line:
            print(f"  PASS  {label}: {line}")
            break
    else:
        print(f"  FAIL  {label}: pattern '{pattern}' not seen within 30s")
        sys.exit(1)
print("All steps passed.")
```

### §11.4 Regression Matrix

The matrix below defines what is automated, what is manual, and what is out of scope per platform. "Automated" means a CI job or scripted test runs on every push. "Manual" means a human runs the test on real hardware before declaring a milestone complete. "N/A" means the test is not applicable to that platform.

| Test Category | QEMU CI | Pi 4 Manual | Pi 5 Manual | Apple Manual |
|---|---|---|---|---|
| Boot to UART | Automated | Manual | Manual | Manual |
| Platform detection | Automated | Manual | Manual | Manual |
| Interrupt delivery (GIC) | Automated | Manual | Manual | Manual |
| Timer tick (1 kHz) | Automated | Manual | Manual | Manual |
| SMP bringup (4 cores) | Automated | Manual | Manual | Manual |
| Storage R/W (VirtIO-blk) | Automated | N/A | N/A | N/A |
| Storage R/W (NVMe/SD) | N/A | Manual | Manual | Manual |
| Framebuffer (solid color) | Visual (QEMU display) | Visual | Visual | Visual |
| Network ping (VirtIO-net) | Automated | N/A | N/A | N/A |
| Network ping (real NIC) | N/A | Manual | Manual | Manual |
| USB enumeration | N/A | Manual | Manual | Manual |
| Full boot sequence | Automated | Manual | Manual | Manual |
| Panic recovery | Automated (forced panic) | Manual | Manual | Manual |
| Thermal throttling | N/A | Manual | Manual | Manual |
| Power management | N/A | Manual | Manual | Manual |

---

## §12 Validation Checklist

### §12.1 Boot Validation

Per-platform boot acceptance criteria. Each row maps to one `EarlyBootPhase` variant and one expected UART output line. The "Expected Output" column shows a substring, not the full line.

| Step | Command / Check | Expected Output |
|---|---|---|
| UART working | Serial console opens, kernel starts | `"AIOS kernel"` |
| EL level | Boot diagnostics | `"EL = 1"` |
| Platform detected | DTB parse + `detect_platform()` | Platform name string (e.g., `"Pi4Platform"`) |
| Timer frequency | UART diagnostics after `init_timer()` | Correct Hz for platform (see table below) |
| Core count | UART diagnostics after DTB parse | Expected core count (`"4 cores"` for Pi 4) |
| Memory detected | UART after `init_memory()` | RAM size string (e.g., `"8192 MiB"`) |
| Interrupts working | Timer tick after `init_interrupts()` + unmask | `"timer tick"` (repeated) |
| SMP bringup | All secondary cores report in | `"core N ready"` for N = 1, 2, 3 |
| Kernel address space | After `init_kernel_address_space()` | `"kernel address space ready"` |
| Storage online | After `BlockEngine::init()` | `"storage online"` |

**Per-platform timer frequencies:**

| Platform | `CNTFRQ_EL0` | Source |
|---|---|---|
| QEMU `virt` | 62,500,000 Hz | QEMU fixed value |
| Pi 4 (BCM2711) | 54,000,000 Hz | SoC crystal oscillator |
| Pi 5 (BCM2712) | 54,000,000 Hz | SoC crystal oscillator |
| Apple M1 | 24,000,000 Hz | Apple AIC clock domain |
| Apple M2 | 24,000,000 Hz | Apple AIC clock domain |

The kernel must read `CNTFRQ_EL0` at runtime and derive the 1 ms tick count (`CNTFRQ_EL0 / 1000`). Any hardcoded timer constant (other than the QEMU default for CI) is a porting defect.

### §12.2 Driver Validation

Per-device-class validation criteria. Tests are run against the real device on real hardware. QEMU CI covers the VirtIO equivalents.

| Device Class | Test | Pass Criteria |
|---|---|---|
| UART | Write 256 bytes, read back via loopback | All bytes match, no framing errors |
| Storage (SD/NVMe) | Write 4 KiB block at sector 0, read back | Data matches, CRC-32C verified |
| Storage (durability) | Write 4 KiB, power cycle, read back | Data persists across power cycle |
| Storage (sequential) | Write 64 MiB sequentially | No write errors; throughput logged |
| GPU framebuffer | Solid-color fill (0x5B8CFF) | Correct color visible on display |
| Network (Ethernet) | ARP + ICMP ping to gateway | Round-trip response within 100 ms |
| RNG | Read 1 KiB from hardware RNG | Chi-squared test passes (p > 0.01) |
| USB (enumeration) | Plug in USB keyboard | Device descriptor readable via UART |
| USB (HID input) | Press key on USB keyboard | Keycode appears in input log |
| Bluetooth (HCI) | `hci_reset` command | HCI event `0x0e` received |
| WiFi (scan) | Trigger scan | At least one BSS in scan results |
| Camera (UVC) | Open `/dev/video0` | Frame descriptor negotiation succeeds |
| Thermal sensor | Read temperature | Value in range 20°C–85°C at idle |
| Power button | Press power button | `PowerButtonEvent` dispatched |

### §12.3 Stress Testing

Stress tests run after a BSP port passes §12.1 and §12.2. They expose timing bugs, race conditions, and thermal limits that single-shot tests miss.

| Test | Duration | Pass Criteria |
|---|---|---|
| Multi-core stress | 10 minutes | No panics, no hangs, no scheduler lockups |
| Memory pressure | Until OOM | Graceful degradation (OOM handler fires), no heap corruption, no silent data loss |
| IRQ storm | 1 minute at maximum deliverable rate | No lost interrupts, no IRQ handler lockup, tick count monotonic |
| Thermal stress | 30 minutes sustained full-core load | CPU throttling activates below thermal trip point, no emergency shutdown |
| Storage endurance | 1,000 random R/W cycles (4 KiB each) | All reads verify against written data (CRC-32C), no corruption |
| SMP hammering | 10 minutes, all cores executing IPC calls | No deadlock, no priority inversion exceeding 50 ms, no livelock |
| Panic recovery | Inject kernel panic, reboot | Panic handler prints reason to UART, halts cleanly (no silent hang) |

**Stress test tooling:**

These tests have no automated harness yet. They are run manually using a custom kernel thread launched from `kernel_main` behind a `#[cfg(feature = "stress-tests")]` feature gate. The thread logs results to the observability ring; the test operator reads them via serial console.

```text
# Enable stress tests
cargo build --target aarch64-unknown-none --features stress-tests
```

The feature is never enabled in CI. It is enabled by the engineer performing real hardware validation.

### §12.4 Porting Sign-Off

A BSP port is declared complete when all of the following are true:

| Criterion | Verified by |
|---|---|
| All §12.1 boot steps pass | Developer + UART log |
| All applicable §12.2 driver tests pass | Developer + UART log |
| All §12.3 stress tests pass | Developer + UART log |
| `just check` passes with zero warnings | CI |
| `just test` passes with zero failures | CI |
| Architecture doc for the platform updated in [platforms.md](./platforms.md) | Doc auditor |
| `detect_platform()` returns correct type for all board revisions | Developer test |
| No hardcoded timer constants or MMIO addresses outside DTB paths | Code review |
| `CLAUDE.md` Key Technical Facts updated with platform constants | Developer |

---

*Testing strategy governs the validation discipline for all BSP ports. When a new platform is added, its entry in §11.4 and §12.2 must be filled in before the PR is merged.*
