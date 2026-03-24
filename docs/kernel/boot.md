---
tags: [kernel, boot]
type: architecture
---

# AIOS Boot and Init Sequence

## Deep Technical Architecture

**Parent document:** [architecture.md](../project/architecture.md) — Section 6.1 Boot Sequence
**Related:** [hal.md](./hal.md) — Platform trait, device abstractions, porting guide, [ipc.md](./ipc.md) — IPC and syscalls, [scheduler.md](./scheduler.md) — Scheduling classes and context multipliers, [memory.md](./memory.md) — Memory management and pool sizing, [spaces.md](../storage/spaces.md) — Space Storage, [airs.md](../intelligence/airs.md) — AI Runtime Service, [compositor.md](../platform/compositor.md) — Display handoff and framebuffer, [model.md](../security/model.md) — Capability system and trust levels, [identity.md](../experience/identity.md) — Identity initialization, [agents.md](../applications/agents.md) — Agent lifecycle and state persistence, [attention.md](../intelligence/attention.md) — Attention Manager initialization, [context-engine.md](../intelligence/context-engine.md) — Context Engine startup, [preferences.md](../intelligence/preferences.md) — Preference Service startup, [development-plan.md](../project/development-plan.md) — Phase plan

-----

## 1. Overview

The parent architecture document describes the boot sequence at a high level: five service manager phases layered on top of firmware handoff and kernel early boot. This document goes deeper — the actual data structures, initialization order, timing constraints, hardware differences, recovery paths, and the mechanisms that make a sub-3-second boot possible.

The boot sequence has one invariant that governs every design decision: **the system is usable at each phase boundary.** If any phase after Phase 2 (core services) fails, the user still gets a functional — if degraded — desktop. AIRS failure doesn't block boot. Network failure doesn't block boot. The only hard dependencies on the critical path are: firmware, kernel, storage, display, and the compositor.

### 1.1 Boot Philosophy

AIOS follows the **minimum-kernel, maximum-delegation** principle observed in production microkernels:

- **seL4 model:** The kernel boots minimally and hands the root task a `BootInfo` containing capabilities to all system resources. The root task — not the kernel — sets up drivers, filesystems, and services. AIOS follows this pattern: the kernel initializes hardware, builds page tables, and delegates everything else to the Service Manager via capability handoff.

- **Zircon model:** Fuchsia's Zircon kernel embeds `userboot` (the first userspace process) at compile time. The kernel doesn't decompress or interpret boot images — userboot does. AIOS similarly defers service startup, storage initialization, and AI runtime loading to userspace processes.

- **Two-phase handoff:** The UEFI stub runs in Boot Services (full UEFI API available), assembles a `BootInfo` struct with everything the kernel needs, then calls `ExitBootServices()` — the point of no return. The kernel receives `BootInfo` in `x0` and never calls back into UEFI Boot Services.

The kernel's job during early boot is narrow: enable the FPU, set up exception vectors, parse the device tree, initialize the UART/GIC/timer, build page tables, bring secondary cores online, and initialize the scheduler + IPC subsystem. Everything else — storage, display composition, networking, AI — is userspace work.

-----

## Document Map

| Topic | Document | Sections |
|---|---|---|
| **Firmware handoff** (UEFI, BootInfo, ESP, EL model, QEMU vs HW) | [firmware.md](./boot/firmware.md) | §2.1–§2.6 |
| **Kernel early boot** (boot.S, kernel_main steps, SMP, SMMU) | [kernel.md](./boot/kernel.md) | §3.1–§3.6 |
| **Service Manager & startup phases** | [services.md](./boot/services.md) | §4.1–§4.8, §5 |
| **Boot performance & early framebuffer** | [performance.md](./boot/performance.md) | §6.1–§6.5, §7.1–§7.4 |
| **Panic handler, recovery, initramfs** | [recovery.md](./boot/recovery.md) | §8.1–§8.4, §9.1–§9.6, §10.1–§10.3 |
| **Shutdown, implementation order, principles** | [lifecycle.md](./boot/lifecycle.md) | §11, §12, §23, §24 |
| **Boot test strategy & cross-doc deps** | [testing.md](./boot/testing.md) | §13.1–§13.4, §14 |
| **Suspend, resume, semantic state** | [suspend.md](./boot/suspend.md) | §15.1–§15.5 |
| **Boot intelligence, on-demand, encryption** | [intelligence.md](./boot/intelligence.md) | §16.1–§16.3, §17.1–§17.3, §18.1–§18.3 |
| **Boot accessibility & first boot** | [accessibility.md](./boot/accessibility.md) | §19.1–§19.3, §20.1–§20.3, §21.1–§21.3 |
| **Research kernel innovations** | [research.md](./boot/research.md) | §22.1–§22.19 |

-----

## Cross-Reference Index

| Section | Sub-file |
|---|---|
| §2.1 UEFI Boot on aarch64 | [firmware.md](./boot/firmware.md) |
| §2.2 What the Kernel Receives (BootInfo) | [firmware.md](./boot/firmware.md) |
| §3.1 Phase Tracking (EarlyBootPhase) | [kernel.md](./boot/kernel.md) |
| §3.3 Step-by-Step Early Boot | [kernel.md](./boot/kernel.md) |
| §3.5 SMP Boot | [kernel.md](./boot/kernel.md) |
| §4.1 Service Manager | [services.md](./boot/services.md) |
| §5 Service Startup Phases | [services.md](./boot/services.md) |
| §6.1 Critical Path Timeline | [performance.md](./boot/performance.md) |
| §7 Early Framebuffer | [performance.md](./boot/performance.md) |
| §8 Kernel Panic Handler | [recovery.md](./boot/recovery.md) |
| §9 Recovery Mode | [recovery.md](./boot/recovery.md) |
| §10 Initramfs | [recovery.md](./boot/recovery.md) |
| §11 Shutdown and Reboot | [lifecycle.md](./boot/lifecycle.md) |
| §12 Implementation Order | [lifecycle.md](./boot/lifecycle.md) |
| §13 Boot Test Strategy | [testing.md](./boot/testing.md) |
| §15 Suspend/Resume | [suspend.md](./boot/suspend.md) |
| §16 Boot Intelligence | [intelligence.md](./boot/intelligence.md) |
| §19 Boot Accessibility | [accessibility.md](./boot/accessibility.md) |
| §22 Research Innovations | [research.md](./boot/research.md) |

-----

## Future Directions

This section captures ideas from OS research and production systems that could improve AIOS's boot sequence in future phases.

### Hubris-Style Deterministic Boot

Oxide's Hubris OS builds the entire system image at compile time — task placements, memory regions, and IPC channels are all determined statically. This eliminates runtime allocation during boot and makes the boot sequence fully deterministic and formally verifiable. AIOS could adopt this pattern for the kernel early boot path (Phases A–B), where all allocations are currently from static arrays or the bump allocator. The benefit: formal verification of the boot sequence becomes tractable.

**Reference:** Hubris (Oxide Computer, 2021) — build-time system description, no dynamic task creation.

### ZBI-Style Structured Boot Data

Fuchsia's Zircon Boot Image (ZBI) packages the kernel, bootloader, and all boot items into a single structured container with typed headers. Each boot item has a type tag and length, allowing the kernel to iterate items without knowing the container format. AIOS's current `BootInfo` is a flat C struct — adding new fields requires recompilation of both stub and kernel. A ZBI-like approach would allow extensible boot data with forward/backward compatibility.

**Reference:** Zircon Boot Image format (Fuchsia project).

### Boot Intelligence (Context-Aware Boot)

The [intelligence.md](./boot/intelligence.md) companion document describes boot intelligence — using past boot traces to predict optimal service startup order, prefetch frequently-accessed data pages, and adapt AIRS model selection based on usage patterns. This is a long-term goal that integrates the Context Engine (Phase 12+) with the boot sequence.

### Measured Boot and Attestation

seL4's formally verified boot chain provides a foundation for measured boot: each boot stage hashes the next stage's binary and extends a TPM PCR (or equivalent). AIOS could integrate with ARM TrustZone and the platform TPM (or firmware TPM on QEMU) to provide attestation — proving to a remote verifier that the system booted an unmodified kernel and initramfs. This is critical for enterprise and IoT deployment scenarios.

**Reference:** seL4 Verified Boot (NICTA/Data61), ARM Platform Security Architecture (PSA).

### μEFI Firmware Isolation

The μEFI paper (USENIX ATC 2025) demonstrates microkernel-style isolation for UEFI firmware components, limiting the blast radius of firmware vulnerabilities. AIOS's UEFI stub already follows the minimal-touch principle; μEFI validates treating BootInfo as untrusted input and hashing critical regions before kernel handoff. See [research.md §22.16](./boot/research.md) for full analysis.

### Firecracker-Style Device Minimalism

AWS Firecracker achieves ≤125ms boot by reducing the virtual device model to the absolute minimum: VirtIO MMIO devices only, no PCI enumeration, no legacy emulation. AIOS's use of QEMU `-machine virt` with VirtIO MMIO aligns with this approach. Future device drivers should prefer VirtIO MMIO on QEMU and native MMIO on hardware, avoiding PCI overhead where possible. See [research.md §22.17](./boot/research.md) for full analysis.

### HongMeng IPC Frequency Optimization

Huawei's HongMeng kernel (OSDI 2024) identifies IPC *frequency* — not per-invocation cost — as the dominant microkernel bottleneck. Batch IPC and shared-memory data transfer for boot-time service initialization could reduce AIOS's service startup latency. See [research.md §22.18](./boot/research.md) for full analysis.

### LionsOS Control-Plane / Data-Plane Separation

The seL4 Device Driver Framework (sDDF) in LionsOS separates control-plane policy (capability-based IPC) from data-plane performance (lock-free shared-memory ring buffers). This achieves near-native I/O with full driver isolation — a model AIOS should adopt as drivers move from kernel space to user space in Phase 6+. See [research.md §22.19](./boot/research.md) for full analysis.
