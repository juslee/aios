# AIOS Boot Test Strategy

Part of: [boot.md](../boot.md) — Boot and Init Sequence
**Related:** [kernel.md](./kernel.md) — Kernel early boot, [lifecycle.md](./lifecycle.md) — Shutdown and implementation order

-----

## 13. Boot Test Strategy

The boot sequence is the most critical code path in AIOS — if it breaks, nothing works. Every change to boot-related code must be validated by automated tests before merging.

### 13.1 CI Boot Smoke Test

The target CI pipeline includes a QEMU boot smoke test (not yet implemented — current CI runs `just check` and `just test` only):

```text
Boot smoke test (target design — to be added to CI):

1. Build kernel + initramfs + UEFI stub
2. Launch QEMU (aarch64, no KVM, 4 GB RAM, VirtIO devices)
3. Capture UART output
4. Assert: boot completion marker appears within 500ms (kernel early boot)
5. Assert: Phase 1 completion within 1000ms
6. Assert: Phase 2 completion within 2000ms
7. Assert: Phase 5 completion within 5000ms
8. Assert: no "[PANIC]" in UART output
9. Assert: all services running with 0 failures
10. Shutdown cleanly, verify clean shutdown marker in UART

Total CI time: ~10 seconds per run (dominated by QEMU startup)
```

The specific UART assertion strings will be defined as each phase is implemented. Current CI validates compilation (`just check`) and host-side unit tests (`just test`).

### 13.2 Platform Test Matrix

```text
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

```text
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
| `Platform` trait, 7 `init_*` methods, `InterruptController`, `Timer`, `Uart`, `GpuDevice`, `NetworkDevice`, `StorageDevice`, `RngDevice` | [hal.md](./hal.md) §3 (Platform Trait), §4 (Device Abstractions) | Device trait signatures must match hal.md §3. Initialization order (UART/interrupts/timer early, GPU/network/storage in service phases) must agree with hal.md §3.2. |
| `Scheduler`, four scheduling classes (RT, Interactive, Normal, Idle), 1ms tick | [scheduler.md](./scheduler.md) §3.1, §10.1 | Timer tick rate (Step 6) and scheduling class names in Step 15 must stay consistent with scheduler.md. |
| `BuddyAllocator`, `SlabAllocator`, slab size classes | [memory.md](./memory.md) | Buddy allocator order range (0–10) and slab size classes (64–4096 bytes) cited in Steps 8–9 must match memory.md. |
| `CapabilityManager`, `CapabilityToken`, root capability, trust levels, `Capability::Root` | [security.md](../security/security.md) §10 | `Timestamp::MAX` for Trust Level 0 tokens (Step 12) and capability delegation model (security.md §3.5) must stay aligned. |
| `IpcSubsystem`, `ChannelId`, health check protocol | [ipc.md](../ipc.md) | Health check protocol (services.md §4.4) and Service Manager IPC channels (ipc.md §4.1) must match ipc.md's channel semantics. |
| Compositor framebuffer handoff, display subsystem, wgpu pipeline | [compositor.md](../platform/compositor.md) | Handoff sequence (§7.4) and Phase 2 display startup must match compositor.md's initialization. |
| AIRS model selection by RAM, `system/models/` space, GGML runtime, 5-second timeout | [airs.md](../intelligence/airs.md) | Model size thresholds (§5 Phase 3: ≥16 GB → 8B Q5_K_M, ≥8 GB → 8B Q4_K_M, ≥4 GB → 3B, ≥2 GB → 1B, <2 GB → no local model) and the 5-second health timeout must stay consistent with airs.md §4.6. |
| Identity Service, Ed25519 keypair, `system/identity/` space | [identity.md](../experience/identity.md) | Phase 4 Identity startup and identity unlock flow must match identity.md's key management. |
| Attention Manager, AI triage vs rule-based fallback | [attention.md](../intelligence/attention.md) | The soft AIRS dependency described in Phase 4 must match attention.md's initialization requirements. |
| Context Engine, signal collection, rule-based heuristic fallback | [context-engine.md](../intelligence/context-engine.md) | Phase 3 Context Engine startup and its AIRS dependency must match context-engine.md's fallback behavior. |
| Preference Service, `user/preferences/` space | [preferences.md](../intelligence/preferences.md) | Phase 4 Preference startup and the preference space path must match preferences.md. |
| `AgentManifest.persistent`, agent shutdown protocol, `ShutdownSignal` | [agents.md](../applications/agents.md) §2.4, §3 | The 5-second shutdown grace period (§11.3) and persistent agent relaunching must match agents.md's lifecycle model. |
| Block Engine, Object Store, Space Storage, WAL, LSM-tree, system spaces | [spaces.md](../storage/spaces.md) | Phase 1 startup sequence and system space paths (`system/audit/`, `system/models/`, etc.) must agree with spaces.md's space hierarchy. |
| ARM SMMU (SMMUv3), stream tables, DMA isolation, bounce buffers | [hal.md](./hal.md) | SMMU initialization (hal.md §15) and per-device DMA page tables must align with hal.md's DMA abstractions. Pi 4 bounce buffer strategy must match hal.md's DMA API. |
| USB host controller (xHCI), USB HID, hub enumeration | [hal.md](./hal.md) | Phase 2 USB input path on Pi must match hal.md's USB abstraction (if defined). xHCI driver is platform-specific (DesignWare on Pi 4, RP1 on Pi 5). |
| Audio subsystem (PCM, mixing, I2S/PWM, HDMI audio) | [audio.md](../platform/audio.md) | Phase 2 Audio Subsystem startup must match audio.md. RT scheduling class for audio threads must match scheduler.md. |
| Watchdog timer (virtual watchdog on QEMU, bcm2835-wdt on Pi), boot timeout, runtime ping | [hal.md](./hal.md) | Watchdog hardware abstraction and timeout values (30s boot, 60s runtime, 15s shutdown) must be consistent across hal.md and boot.md. |
| GPU memory reservation (`/reserved-memory` node, `gpu_mem`), VideoCore carve-out | [compositor.md](../platform/compositor.md) | GPU memory split on Pi (76 MB Pi 4, 64 MB Pi 5) and its effect on available RAM must match compositor.md's VRAM requirements. |

-----
