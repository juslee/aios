# AIOS — AI-First Operating System

A bare-metal aarch64 operating system built in Rust, designed from the ground up for AI as infrastructure.

[![CI](https://github.com/juslee/aios/actions/workflows/ci.yml/badge.svg)](https://github.com/juslee/aios/actions/workflows/ci.yml)
![License](https://img.shields.io/badge/license-BSD--2--Clause-blue)
![Language](https://img.shields.io/badge/language-Rust-orange)
![Target](https://img.shields.io/badge/target-aarch64-lightgrey)

---

## What is AIOS

AIOS is an operating system designed around a single premise: AI is infrastructure, not interface. Rather than layering machine learning capabilities on top of a legacy kernel, AIOS treats inference, context, and autonomous agents as first-class kernel-level services — as fundamental as a file system or network stack. There is no compatibility shim with POSIX, no decades-old driver model to work around, and no separation between "the OS" and "the AI layer." They are the same thing.

The hardware target is aarch64, starting with QEMU's `virt` machine (`cortex-a72`) for development and progressing to the Raspberry Pi 4 and Pi 5 for real-hardware validation. This pairing gives a fast, deterministic development loop in the emulator while keeping a concrete physical target in scope throughout every phase.

The architecture is a capability-based microkernel. User processes communicate through typed IPC channels; hardware access is gated by unforgeable capability tokens. On top of the microkernel sit Spaces — isolated execution environments analogous to processes, but with explicit capability grants — and AIRS, the AI Runtime System, which manages inference engines, context stores, and agent lifecycles as native OS services. A compositor, storage subsystem, and networking stack complete the platform layer before the experience layer exposes Workspaces, a Conversation Bar, a browser, and a settings UI.

Phase 0 (toolchain and bare-metal boot) and Phase 1 (boot and first pixels) are complete. Phase 2 (memory management) is in progress with M7 (physical memory manager) and M8 (virtual memory, W^X, KASLR slide computation) complete. The kernel boots via edk2 firmware on QEMU virt with 4 SMP cores online, a framebuffer test pattern rendered, and full TTBR1 kernel page tables with W^X enforcement active.

---

## Architecture

```
Experience Layer:  Workspace, Conversation Bar, Browser, Settings, Agents
Services Layer:    AIRS (inference, context, agents), Storage, Compositor, Networking
Subsystem Layer:   Universal hardware abstraction with capability gates
Kernel:            Microkernel (IPC, scheduler, memory, capabilities)
Hardware:          aarch64 — QEMU virt -> Raspberry Pi 4/5
```

See [docs/project/overview.md](docs/project/overview.md) for full architectural detail.

---

## Prerequisites

| Tool | Version | Notes |
|------|---------|-------|
| Rust nightly | pinned via `rust-toolchain.toml` | `rustup show` installs it automatically |
| QEMU | 6.0+ | `qemu-system-aarch64` must be on `$PATH` |
| just | any recent | task runner; `cargo install just` |
| mtools | any | required from Phase 1 onward for disk image creation |
| GDB | any (optional) | `aarch64-none-elf-gdb` or multiarch GDB for debugging |

---

## Getting Started

```sh
git clone https://github.com/juslee/aios.git
cd aios

# Install the pinned Rust toolchain and targets
rustup show

# Build the kernel
just build

# Run under QEMU
just run
```

---

## Project Structure

```
aios/
├── docs/
│   ├── project/          # Vision, architecture, development plan
│   ├── kernel/           # Kernel subsystem specifications
│   └── phases/           # Per-phase implementation guides (00-, 01-, ...)
├── .claude/
│   ├── agents/           # Claude agent definitions
│   └── skills/           # Reusable skill scripts
├── kernel/               # Kernel source (aarch64-unknown-none)
├── shared/               # Shared types (BootInfo, PhysAddr, etc.)
└── uefi-stub/            # UEFI boot stub (aarch64-unknown-uefi)
```

---

## Development Plan

30 phases across 8 tiers, targeting approximately 2.7 years to a production OS.

| Tier | Name | Phases | Focus |
|------|------|--------|-------|
| 1 | Hardware Foundation | 0–3 | Boot, memory management, IPC |
| 2 | Core System Services | 4–7 | Storage, GPU, compositor, networking |
| 3 | AI & Intelligence | 8–11 | AIRS, semantic search, agents |
| 4 | Platform Maturity | 12–15 | SDK, security, performance, POSIX layer |
| 5 | Hardware & Connectivity | 16–19 | NTM, USB, wireless, power management |
| 6 | Rich Experience | 20–23 | UI toolkit, browser, media, accessibility |
| 7 | Production OS | 24–27 | Secure boot, Linux compatibility, launch |

See [docs/project/development-plan.md](docs/project/development-plan.md) for the full phase breakdown.

---

## Build Commands

| Command | Description |
|---------|-------------|
| `just build` | Compile the kernel for `aarch64-unknown-none` |
| `just build-stub` | Compile the UEFI stub for `aarch64-unknown-uefi` |
| `just disk` | Build kernel + stub and create ESP disk image (requires mtools) |
| `just run` | Boot via edk2 firmware with UEFI stub |
| `just run-display` | Boot with QEMU display window (for framebuffer visual verification) |
| `just run-direct` | Boot kernel directly via QEMU `-kernel` (Phase 0 mode) |
| `just debug` | Launch QEMU with GDB stub on `tcp::1234` |
| `just check` | Run format check, clippy, and build (both targets) |
| `just test` | Run unit tests |

---

## License

BSD-2-Clause. See [LICENSE](LICENSE).

No GPL dependencies. All third-party crates must be BSD, MIT, Apache-2.0, or ISC licensed.

---

## Contributing

This project follows conventions documented in [CLAUDE.md](CLAUDE.md) and [CONTRIBUTING.md](CONTRIBUTING.md). Read those files before opening a pull request — they cover branch workflow, commit style, documentation standards, and phase doc structure.
