# AIOS Boot and Init Sequence

## Deep Technical Architecture

**Parent document:** [architecture.md](../project/architecture.md) — Section 6.1 Boot Sequence
**Related:** [hal.md](./hal.md) — Platform trait, device abstractions, porting guide, [ipc.md](./ipc.md) — IPC and syscalls, [scheduler.md](./scheduler.md) — Scheduling classes and context multipliers, [memory.md](./memory.md) — Memory management and pool sizing, [spaces.md](../storage/spaces.md) — Space Storage, [airs.md](../intelligence/airs.md) — AI Runtime Service, [compositor.md](../platform/compositor.md) — Display handoff and framebuffer, [security.md](../security/security.md) — Capability system and trust levels, [identity.md](../experience/identity.md) — Identity initialization, [agents.md](../applications/agents.md) — Agent lifecycle and state persistence, [attention.md](../intelligence/attention.md) — Attention Manager initialization, [context-engine.md](../intelligence/context-engine.md) — Context Engine startup, [preferences.md](../intelligence/preferences.md) — Preference Service startup, [development-plan.md](../project/development-plan.md) — Phase plan

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
| **Firmware handoff** (UEFI, BootInfo, ESP, EL model, QEMU vs HW) | [boot-firmware.md](./boot-firmware.md) | §2.1–§2.6 |
| **Kernel early boot** (boot.S, kernel_main steps, SMP, SMMU) | [boot-kernel.md](./boot-kernel.md) | §3.1–§3.6 |
| **Service Manager & startup phases** | [boot-services.md](./boot-services.md) | §4.1–§4.8, §5 |
| **Boot performance & early framebuffer** | [boot-performance.md](./boot-performance.md) | §6.1–§6.5, §7.1–§7.4 |
| **Panic handler, recovery, initramfs** | [boot-recovery.md](./boot-recovery.md) | §8.1–§8.4, §9.1–§9.6, §10.1–§10.3 |
| **Shutdown, implementation order, principles** | [boot-lifecycle.md](./boot-lifecycle.md) | §11, §12, §23, §24 |
| **Boot test strategy & cross-doc deps** | [boot-testing.md](./boot-testing.md) | §13.1–§13.4, §14 |
| **Suspend, resume, semantic state** | [boot-suspend.md](./boot-suspend.md) | §15.1–§15.5 |
| **Boot intelligence, on-demand, encryption** | [boot-intelligence.md](./boot-intelligence.md) | §16.1–§16.3, §17.1–§17.3, §18.1–§18.3 |
| **Boot accessibility & first boot** | [boot-accessibility.md](./boot-accessibility.md) | §19.1–§19.3, §20.1–§20.3, §21.1–§21.3 |
| **Research kernel innovations** | [boot-research.md](./boot-research.md) | §22.1–§22.15 |

-----

## Cross-Reference Index

| Section | Sub-file |
|---|---|
| §2.1 UEFI Boot on aarch64 | [boot-firmware.md](./boot-firmware.md) |
| §2.2 What the Kernel Receives (BootInfo) | [boot-firmware.md](./boot-firmware.md) |
| §3.1 Phase Tracking (EarlyBootPhase) | [boot-kernel.md](./boot-kernel.md) |
| §3.3 Step-by-Step Early Boot | [boot-kernel.md](./boot-kernel.md) |
| §3.5 SMP Boot | [boot-kernel.md](./boot-kernel.md) |
| §4.1 Service Manager | [boot-services.md](./boot-services.md) |
| §5 Service Startup Phases | [boot-services.md](./boot-services.md) |
| §6.1 Critical Path Timeline | [boot-performance.md](./boot-performance.md) |
| §7 Early Framebuffer | [boot-performance.md](./boot-performance.md) |
| §8 Kernel Panic Handler | [boot-recovery.md](./boot-recovery.md) |
| §9 Recovery Mode | [boot-recovery.md](./boot-recovery.md) |
| §10 Initramfs | [boot-recovery.md](./boot-recovery.md) |
| §11 Shutdown and Reboot | [boot-lifecycle.md](./boot-lifecycle.md) |
| §12 Implementation Order | [boot-lifecycle.md](./boot-lifecycle.md) |
| §13 Boot Test Strategy | [boot-testing.md](./boot-testing.md) |
| §15 Suspend/Resume | [boot-suspend.md](./boot-suspend.md) |
| §16 Boot Intelligence | [boot-intelligence.md](./boot-intelligence.md) |
| §19 Boot Accessibility | [boot-accessibility.md](./boot-accessibility.md) |
| §22 Research Innovations | [boot-research.md](./boot-research.md) |

-----

## 11. Future Directions

This section captures ideas from OS research and production systems that could improve AIOS's boot sequence in future phases.

### 11.1 Hubris-Style Deterministic Boot

Oxide's Hubris OS builds the entire system image at compile time — task placements, memory regions, and IPC channels are all determined statically. This eliminates runtime allocation during boot and makes the boot sequence fully deterministic and formally verifiable. AIOS could adopt this pattern for the kernel early boot path (Phases A–B), where all allocations are currently from static arrays or the bump allocator. The benefit: formal verification of the boot sequence becomes tractable.

**Reference:** Hubris (Oxide Computer, 2021) — build-time system description, no dynamic task creation.

### 11.2 ZBI-Style Structured Boot Data

Fuchsia's Zircon Boot Image (ZBI) packages the kernel, bootloader, and all boot items into a single structured container with typed headers. Each boot item has a type tag and length, allowing the kernel to iterate items without knowing the container format. AIOS's current `BootInfo` is a flat C struct — adding new fields requires recompilation of both stub and kernel. A ZBI-like approach would allow extensible boot data with forward/backward compatibility.

**Reference:** Zircon Boot Image format (Fuchsia project).

### 11.3 Boot Intelligence (Context-Aware Boot)

The [boot-intelligence.md](./boot-intelligence.md) companion document describes boot intelligence — using past boot traces to predict optimal service startup order, prefetch frequently-accessed data pages, and adapt AIRS model selection based on usage patterns. This is a long-term goal that integrates the Context Engine (Phase 8+) with the boot sequence.

### 11.4 Measured Boot and Attestation

seL4's formally verified boot chain provides a foundation for measured boot: each boot stage hashes the next stage's binary and extends a TPM PCR (or equivalent). AIOS could integrate with ARM TrustZone and the platform TPM (or firmware TPM on QEMU) to provide attestation — proving to a remote verifier that the system booted an unmodified kernel and initramfs. This is critical for enterprise and IoT deployment scenarios.

**Reference:** seL4 Verified Boot (NICTA/Data61), ARM Platform Security Architecture (PSA).
