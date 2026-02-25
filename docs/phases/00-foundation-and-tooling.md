# Phase 0: Foundation and Tooling

**Tier:** 1 — Hardware Foundation
**Duration:** 2 weeks
**Deliverable:** Project scaffold, CI, `just build && just run`
**Status:** Planned
**Prerequisites:** None (first phase)
**Unlocks:** Phase 1 (Boot and First Pixels)

-----

## Objective

Set up a Rust bare-metal project targeting aarch64 that compiles a minimal kernel, runs it on QEMU, and outputs text to UART. Establish the build system, project structure, and CI pipeline that all subsequent phases build on.

By the end of this phase, `just run` produces a QEMU window (or terminal) showing "AIOS kernel booting..." from a Rust `#![no_std]` binary running in EL1 on emulated aarch64 hardware.

-----

## Architecture References

These existing documents define the technical design. This phase doc focuses on implementation order and acceptance criteria — not duplicating the architecture.

| Topic | Document | Relevant Sections |
|---|---|---|
| Boot sequence | [boot.md](../kernel/boot.md) | §3 Kernel Early Boot (Phase 0 uses a subset; §2 Firmware Handoff is Phase 1) |
| HAL and platform trait | [hal.md](../kernel/hal.md) | §1 Overview (Uart trait reference; §2 Platform Detection is Phase 1) |
| Memory layout | [memory.md](../kernel/memory.md) | §2 Physical Memory Manager, §2.1 Bootstrap |
| Project overview | [overview.md](../project/overview.md) | §9 Hardware Strategy |
| Technology stack | [development-plan.md](../project/development-plan.md) | §7 Technology Stack |

-----

## Milestone Steps

### Step 1: Rust Toolchain and Target Setup

**What:** Install and pin the Rust nightly toolchain with the `aarch64-unknown-none` target.

**Tasks:**
- [ ] Create `rust-toolchain.toml` pinning a specific nightly version
- [ ] Add `aarch64-unknown-none` as the default target
- [ ] Add components: `rust-src`, `llvm-tools`, `clippy`, `rustfmt`
- [ ] Verify `cargo build --target aarch64-unknown-none` produces an empty binary

**Note:** `aarch64-unknown-none` uses the hard-float ABI — the compiler emits NEON/FP instructions freely. This is correct for AIOS (needed for GGML/NEON in later phases) but requires enabling the FPU in boot assembly before any Rust code executes (see Step 3).

**Acceptance:** `rustup show` displays the pinned nightly with aarch64-unknown-none. `cargo build` succeeds (even if the binary does nothing).

-----

### Step 2: Project Scaffold and Cargo Workspace

**What:** Create the workspace layout, project hygiene files, and crate structure that will grow across all 28 phases.

**Tasks:**
- [ ] Create root `Cargo.toml` as a workspace with `resolver = "2"` and `edition = "2021"`
- [ ] Create `kernel/` crate — `#![no_std]`, `#![no_main]`, panic handler
- [ ] Create `shared/` crate — `#![no_std]`, shared types between kernel and userspace (initially a minimal `BootInfo` stub with `#[repr(C)]` and the magic number field)
- [ ] Verify `shared/` compiles with `--target aarch64-unknown-none` (must not accidentally pull `std`)
- [ ] Create `.cargo/config.toml` — set default target, rustflags for bare-metal (`-C link-arg=-T...`, `-C relocation-model=static`)
- [ ] Create `.gitignore` — ignore `target/`, QEMU disk images, editor files
- [ ] Create `LICENSE` — BSD-2-Clause (per overview.md §1)
- [ ] Create `README.md` — project name, one-line description, build instructions (`just build && just run`)
- [ ] Commit `Cargo.lock` — the kernel is a binary target; lock file ensures reproducible CI builds

**Note:** `-C relocation-model=static` is correct for Phase 0 (fixed load address). This will change to position-independent code when KASLR is introduced in Phase 1+.

**Note:** All kernel crate dependencies must be `no_std` compatible with MIT or Apache-2.0 licenses, consistent with the BSD-2-Clause OS license policy.

**Proposed layout:**
```
aios/
├── Cargo.toml              (workspace root, resolver = "2")
├── Cargo.lock              (committed for reproducibility)
├── rust-toolchain.toml
├── justfile
├── LICENSE                  (BSD-2-Clause)
├── README.md
├── .gitignore
├── .cargo/
│   └── config.toml         (target defaults, linker flags)
├── kernel/
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs         (entry point, panic handler)
│   │   └── arch/
│   │       └── aarch64/
│   │           ├── mod.rs
│   │           ├── boot.S   (assembly entry)
│   │           └── linker.ld
│   └── build.rs            (optional: pass linker script path)
├── shared/
│   ├── Cargo.toml
│   └── src/
│       └── lib.rs          (#![no_std], BootInfo stub)
└── docs/                   (existing)
```

**Acceptance:** `cargo build` compiles both crates with zero warnings. `file` on the kernel ELF shows "ELF 64-bit LSB executable, ARM aarch64". `shared/` compiles for both the bare-metal target and the host.

-----

### Step 3: Linker Script and Assembly Entry Point

**What:** Write the linker script that places kernel sections at the correct addresses for QEMU virt machine, and the assembly stub that initializes the CPU state and jumps to Rust.

**Linker script tasks:**
- [ ] Create `kernel/src/arch/aarch64/linker.ld`
- [ ] Set `.text` origin at `0x4008_0000` (512 KiB above QEMU virt RAM base at `0x4000_0000` — leaves room for the DTB that QEMU places at RAM start)
- [ ] Define sections: `.text`, `.rodata`, `.data`, `.bss`, stack region
- [ ] Export `__bss_start` and `__bss_end` symbols (used by boot.S for BSS zeroing)
- [ ] Place stub exception vector table in `.text.vectors` with `.align 11` (2048-byte alignment required by `VBAR_EL1`)
- [ ] Wire linker script into build via `.cargo/config.toml` or `build.rs`

**Assembly entry point tasks (`boot.S`):**

The boot sequence must follow this exact order:

- [ ] **1. EL2→EL1 drop (if needed):** Read `CurrentEL`. If at EL2, configure `HCR_EL2` (set RW bit for AArch64 EL1), set `SPSR_EL2` to EL1h with DAIF masked, set `ELR_EL2` to the continuation label, `eret` to EL1
- [ ] **2. Install stub exception vectors:** Write address of the vector table to `VBAR_EL1`. Stub entries branch-to-self (`b .`) so any early fault halts deterministically instead of jumping to garbage memory
- [ ] **3. Enable FP/NEON:** Set `CPACR_EL1.FPEN = 0b11` (`mrs x0, CPACR_EL1; orr x0, x0, #(3 << 20); msr CPACR_EL1, x0; isb`). This must happen before any Rust code — the compiler emits NEON instructions for `memcpy`/`memset` even during BSS zeroing
- [ ] **4. Park secondary cores:** Read `MPIDR_EL1`, extract core ID (Aff0 field). If not core 0, enter `wfe` loop. (`wfe` is used instead of `wfi` because there is no interrupt controller configured yet — `wfi` would never return)
- [ ] **5. Set stack pointer:** Load stack top address from linker-script-defined symbol
- [ ] **6. Zero BSS:** Loop from `__bss_start` to `__bss_end` using `str xzr` (safe now that FP/NEON is enabled)
- [ ] **7. Branch to `kernel_main`**

**Note:** QEMU `-kernel` loads the ELF at whatever virtual address the ELF headers specify — `0x4008_0000` is the linker script origin, not a QEMU-assigned address. QEMU also generates a DTB and passes its physical address in register `x0` (following the Linux arm64 boot protocol), though Phase 0 does not parse it.

**Acceptance:** `cargo objdump -- -d` shows the entry point symbol at `0x4008_0000`. `cargo objdump -- -h` shows `.text` at `0x4008_0000`, `.bss` after `.data`, no overlapping sections. Vector table section is 2048-byte aligned.

**Key reference:** [boot.md](../kernel/boot.md) §3.3 Steps 1-2 document the full early boot initialization order (FPU enable, VBAR install). Phase 0 implements a simplified version of this; the full UEFI handoff and `BootInfo` assembly is Phase 1 work.

-----

### Step 4: UART Output (PL011)

**What:** Implement minimal PL011 UART driver so the kernel can print text. This is the first HAL device.

**Tasks:**
- [ ] Create `kernel/src/arch/aarch64/uart.rs` — PL011 at QEMU virt UART0 base (`0x0900_0000`)
- [ ] Implement `putc(byte: u8)` — spin on TXFF flag in UARTFR (offset `0x018`), write to UARTDR (offset `0x000`)
- [ ] Implement `print!` / `println!` macros wrapping UART output
- [ ] Call `println!("AIOS kernel booting...")` from `kernel_main`

**Acceptance:** Running in QEMU shows "AIOS kernel booting..." on serial output.

**Note:** If UART output does not appear, the failure may be in Step 3 (wrong load address, missing FPU enable, BSS not zeroed) rather than the UART driver itself. Debug by checking that the entry point address matches expectations (`-d in_asm` QEMU flag) before investigating UART registers.

**Key reference:** [hal.md](../kernel/hal.md) §1 defines the Uart trait. Phase 0 implements a hardcoded QEMU version; the trait abstraction comes in Phase 1.

-----

### Step 5: Justfile (Build System)

**What:** Create the `justfile` with recipes that will be used throughout all phases.

**Tasks:**
- [ ] `just build` — compile kernel in debug mode
- [ ] `just build-release` — compile kernel in release mode
- [ ] `just run` — build + launch QEMU (see Step 6 for invocation)
- [ ] `just debug` — build + launch QEMU with GDB stub (`-s -S`)
- [ ] `just test` — run `cargo test` on host (for unit tests that don't need QEMU)
- [ ] `just clippy` — run clippy with `--deny warnings`
- [ ] `just fmt` — run `cargo fmt` (reformats in place)
- [ ] `just fmt-check` — run `cargo fmt --check` (CI mode — exits non-zero if formatting differs)
- [ ] `just check` — fmt-check + clippy + build (CI shortcut)
- [ ] `just clean` — cargo clean

**Acceptance:** `just build`, `just clippy`, `just fmt-check`, and `just test` all pass with zero warnings. `just check` passes on a clean checkout without QEMU installed (it does not run `just run`).

-----

### Step 6: QEMU Runner Configuration

**What:** Configure QEMU aarch64 with the virt machine so `just run` launches the kernel.

**Tasks:**
- [ ] QEMU invocation: `qemu-system-aarch64 -machine virt -cpu cortex-a72 -m 2G -nographic -kernel <kernel-elf>`
- [ ] Use `-kernel` with the ELF directly (no `objcopy` to flat binary needed — QEMU reads ELF entry point from headers, and ELF preserves symbols for GDB debugging)
- [ ] Wire into `just run` and `just debug` recipes from Step 5
- [ ] Verify UART output appears in terminal

**Acceptance:** `just run` builds the kernel and launches QEMU. "AIOS kernel booting..." appears in terminal. `Ctrl+A, X` exits QEMU cleanly. `just debug` starts QEMU paused and accepting GDB connections on port 1234.

-----

### Step 7: CI Pipeline (GitHub Actions)

**What:** Set up CI that validates every push and PR.

**Tasks:**
- [ ] Create `.github/workflows/ci.yml`
- [ ] Jobs: `check` (fmt-check + clippy), `build` (debug + release), `test` (host unit tests)
- [ ] Cache Rust toolchain and cargo registry (use `Swatinem/rust-cache` or similar — the nightly toolchain with `rust-src` and `llvm-tools` is ~2 GB)
- [ ] Install QEMU in CI for future integration tests (optional for Phase 0 — can defer to Phase 1; note Phase 1 UEFI boot needs QEMU 7.0+ and the `edk2-aarch64` firmware package)
- [ ] Add CI badge to `README.md`

**Acceptance:** Push to GitHub triggers CI. All jobs pass on a clean checkout.

-----

### Step 8: Panic Handler and Boot Diagnostics

**What:** Set up panic-to-UART output and basic boot diagnostics.

**Tasks:**
- [ ] Implement `#[panic_handler]` that prints panic info (message, file, line) to UART and halts (`loop { wfe }`)
- [ ] Add `kernel/src/arch/aarch64/exceptions.rs` — define the full exception vector table (16 entries at 128-byte spacing, all currently branch-to-self stubs; the real handlers come in Phase 1). This replaces the minimal stub installed in boot.S with a Rust-defined table.
- [ ] Reinstall `VBAR_EL1` pointing to the Rust-defined vector table (the boot.S stub was a temporary safety net)
- [ ] Print boot diagnostics: current EL (should be 1), core ID from `MPIDR_EL1`

**Acceptance:** A `panic!("test")` in `kernel_main` prints the panic message and file:line to UART, then halts. `VBAR_EL1` contains the address of the Rust exception vector table (print the value to UART and confirm it matches the vector table symbol address from `cargo objdump`).

-----

## Decision Points

| Decision | Options | Recommendation |
|---|---|---|
| Boot method for Phase 0 | UEFI stub vs. ELF loaded by `-kernel` | ELF via `-kernel`. UEFI stub is Phase 1 work. QEMU loads the ELF at the address in its headers and passes a DTB pointer in x0. |
| Kernel load address | `0x4008_0000` (512 KiB offset) vs. `0x4010_0000` (1 MiB offset) | `0x4008_0000`. The 512 KiB offset leaves room for QEMU's DTB at RAM base (`0x4000_0000`). The Phase 0 kernel is tiny. If DTB overlap becomes an issue, move to `0x4010_0000`. |
| UART approach | Hardcoded MMIO vs. HAL trait | Hardcoded for Phase 0. The HAL `Platform` trait and `detect_platform()` are Phase 1 work. Keep it simple now, refactor later. |
| Test strategy | Host-only unit tests vs. QEMU integration tests | Host-only for Phase 0. QEMU integration tests (boot → check UART output) in Phase 1. |

-----

## Phase Completion Criteria

- [ ] `just build` compiles a `#![no_std]` aarch64 kernel with zero warnings
- [ ] `just run` boots the kernel in QEMU and prints "AIOS kernel booting..." to UART
- [ ] `just clippy` and `just fmt-check` pass cleanly
- [ ] Panic handler prints location and message to UART
- [ ] Exception vector table is installed (VBAR_EL1 value printed and confirmed)
- [ ] CI pipeline passes on clean checkout
- [ ] Project has LICENSE (BSD-2-Clause), README, .gitignore
- [ ] Adding a new crate to workspace `[members]` in root `Cargo.toml` and running `cargo build --workspace` succeeds without restructuring
