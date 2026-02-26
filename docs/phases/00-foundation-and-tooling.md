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
| Boot sequence | [boot.md](../kernel/boot.md) | §3.3 Steps 1-2 (FPU enable, VBAR install; §2 Firmware Handoff is Phase 1 — QEMU `-kernel` handles the EL transition) |
| HAL and platform trait | [hal.md](../kernel/hal.md) | §3 Platform Trait (`init_uart`, Uart trait; §2 Platform Detection is Phase 1) |
| Memory layout | [memory.md](../kernel/memory.md) | §2 Physical Memory Manager, §2.1 Bootstrap (background only — explains the QEMU RAM base at `0x4000_0000` relevant to the load address decision; PMM implementation is Phase 2) |
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
- [ ] Create `kernel/` crate — `#![no_std]`, `#![no_main]`, stub panic handler (`loop {}` — upgraded to UART output in Step 8 once UART exists)
- [ ] Create `shared/` crate — `#![no_std]`, with the full `BootInfo` struct skeleton from boot.md §2.2 (all 12 fields, using `Option<T>` for optional fields). Phase 0 only populates `magic`; Phase 1 populates the rest. Starting with the full struct avoids an ABI-breaking change at the Phase 0/1 boundary.
- [ ] Verify `shared/` compiles with `--target aarch64-unknown-none` (must not accidentally pull `std`)
- [ ] Create `.cargo/config.toml` — set default target; use `build.rs` (not hardcoded `-T` in config.toml) to pass the linker script path, as config.toml paths are relative to the workspace root and break when building from subdirectories
- [ ] Create `-C relocation-model=static` rustflag in `.cargo/config.toml` (correct for Phase 0 fixed load address; will change to PIC when KASLR is introduced in Phase 1+)
- [ ] Create `.gitignore` — ignore `target/`, QEMU disk images, editor files
- [ ] Create `LICENSE` — BSD-2-Clause (per overview.md §1)
- [ ] Create `README.md` — project name, one-line description, prerequisites (Rust nightly, QEMU 6.0+), build instructions (`just build && just run`)
- [ ] Commit `Cargo.lock` — the kernel is a binary target; lock file ensures reproducible CI builds

**Note:** All kernel crate dependencies must be `no_std` compatible with MIT or Apache-2.0 licenses, consistent with the BSD-2-Clause OS license policy.

**Proposed layout:**
```
aios/
├── Cargo.toml              (workspace root, resolver = "2")
├── Cargo.lock              (committed for reproducibility)
├── rust-toolchain.toml
├── justfile
├── LICENSE                 (BSD-2-Clause)
├── README.md
├── .gitignore
├── .cargo/
│   └── config.toml         (target defaults, relocation-model=static)
├── kernel/
│   ├── Cargo.toml
│   ├── build.rs            (emits cargo:rustc-link-arg=-T<linker script path>)
│   └── src/
│       ├── main.rs         (kernel_main, panic handler stub)
│       └── arch/
│           └── aarch64/
│               ├── mod.rs
│               ├── boot.S  (assembly entry point)
│               ├── uart.rs (PL011 driver — added in Step 4)
│               ├── exceptions.rs (vector table — added in Step 8)
│               └── linker.ld
├── shared/
│   ├── Cargo.toml
│   └── src/
│       └── lib.rs          (#![no_std], full BootInfo skeleton)
└── docs/                   (existing)
```

**Acceptance:** `cargo build` compiles both crates with zero warnings. `file` on the kernel ELF shows "ELF 64-bit LSB executable, ARM aarch64". `shared/` compiles for both `aarch64-unknown-none` and the host target.

-----

### Step 3: Linker Script and Assembly Entry Point

**What:** Write the linker script that places kernel sections at the correct addresses for QEMU virt machine, and the assembly stub that initializes the CPU state and jumps to Rust.

**EL note:** QEMU `-kernel` on the virt machine boots directly to EL1. EL2→EL1 drop is not performed here — QEMU handles the transition, just as UEFI firmware does on real hardware (see boot.md §2.6: "the kernel never touches EL2 registers"). There is no EL2 setup in Phase 0's boot assembly.

**Linker script tasks:**
- [ ] Create `kernel/src/arch/aarch64/linker.ld`
- [ ] Set `.text` origin at `0x4008_0000` (512 KiB above QEMU virt RAM base at `0x4000_0000` — leaves room for the DTB QEMU places at RAM start; see Decision Points for rationale)
- [ ] Define sections in order: `.text`, `.rodata`, `.data`, `.bss`, stack region
- [ ] Export `__bss_start` and `__bss_end` symbols (used by boot.S for BSS zeroing)
- [ ] Place stub exception vector table in `.text.vectors` with `.align 11` (2048-byte alignment required by `VBAR_EL1`)
- [ ] Emit the linker script path via `build.rs` with `println!("cargo:rustc-link-arg=-T{}", ...)`

**Assembly entry point tasks (`boot.S`):**

The boot sequence must follow this exact order. The ordering is strict — FPU must be enabled before the compiler can generate any code that executes:

- [ ] **1. Enable FP/NEON (must be first):** `mrs x0, CPACR_EL1; orr x0, x0, #(3 << 20); msr CPACR_EL1, x0; isb`. The hard-float ABI means the compiler emits NEON instructions for `memcpy`/`memset` — including during BSS zeroing. Any NEON instruction before this traps. This must run before anything else.
- [ ] **2. Install stub exception vectors:** Write address of the vector table to `VBAR_EL1`. Stub entries branch-to-self (`b .`) so any early fault halts deterministically instead of jumping to garbage memory. This is a temporary safety net — replaced with the Rust-defined table in Step 8.
- [ ] **3. Park secondary cores:** Read `MPIDR_EL1`, extract core ID (Aff0 field). If not core 0, enter `wfe` loop. `wfe` (Wait For Event) is used instead of `wfi` because it allows the boot CPU to wake parked secondaries later via `sev` (Send Event) during SMP bringup in Phase 1+. `wfi` would require a configured interrupt source to return — not available yet.
- [ ] **4. Set stack pointer:** Load stack top address from linker-script-defined symbol.
- [ ] **5. Zero BSS:** Loop from `__bss_start` to `__bss_end` using `str xzr` (safe now that FPU is enabled in step 1).
- [ ] **6. Branch to `kernel_main`:** The Rust entry point must be declared `#[no_mangle] pub extern "C" fn kernel_main() -> !` — without `#[no_mangle]`, the linker cannot find the symbol and will error.

**QEMU boot protocol note:** QEMU `-kernel` loads the ELF at the virtual address in its ELF headers (`0x4008_0000` per the linker script). QEMU also generates a DTB and passes its physical address in register `x0` following the Linux arm64 boot protocol. Phase 0 does not parse the DTB — `x0` is ignored at entry.

**Acceptance:** `cargo objdump -- -d` shows `kernel_main` near `0x4008_0000`. `cargo objdump -- -h` shows `.text` at `0x4008_0000`, `.bss` after `.data`, no overlapping sections. Vector table section is 2048-byte aligned.

**Key reference:** [boot.md](../kernel/boot.md) §3.3 Steps 1-2 document the FPU enable and VBAR install sequence. Phase 0 implements a subset of this; the full UEFI handoff and `BootInfo` assembly is Phase 1 work.

-----

### Step 4: UART Output (PL011)

**What:** Implement minimal PL011 UART driver so the kernel can print text. This is the first HAL device.

**Tasks:**
- [ ] Create `kernel/src/arch/aarch64/uart.rs` — PL011 at QEMU virt UART0 base (`0x0900_0000`)
- [ ] Implement `putc(byte: u8)` — spin on TXFF flag in UARTFR (offset `0x018`), write to UARTDR (offset `0x000`)
- [ ] Implement `print!` / `println!` macros wrapping UART output
- [ ] Call `println!("AIOS kernel booting...")` from `kernel_main`

**QEMU init note:** On QEMU virt, the PL011 is pre-initialized by QEMU — writing to UARTDR after checking TXFF is sufficient without configuring baud rate registers (IBRD, FBRD, LCR_H, CR). Do not write the full PL011 init sequence for Phase 0; it is unnecessary and QEMU's pre-init values will be overwritten. On real hardware (Phase 5+), full PL011 initialization is required.

**Debug note:** If UART output does not appear, the failure is more likely in Step 3 (wrong load address, missing FPU enable, BSS not zeroed, `kernel_main` not found by linker) than in the UART driver itself. Verify the entry point address with `cargo objdump -- -d` and confirm the binary loads at `0x4008_0000` before investigating UART registers. Use `-d in_asm` in the QEMU invocation to trace executed instructions.

**Key reference:** [hal.md](../kernel/hal.md) §3 defines the `Platform` trait and `init_uart` method. Phase 0 implements a hardcoded QEMU-only UART; the trait abstraction comes in Phase 1.

**Acceptance:** Running in QEMU shows "AIOS kernel booting..." on serial output.

-----

### Step 5: Justfile (Build System)

**What:** Create the `justfile` with recipes that will be used throughout all phases.

**Tasks:**
- [ ] `just build` — compile kernel in debug mode
- [ ] `just build-release` — compile kernel in release mode
- [ ] `just run` — build + launch QEMU (see Step 6 for full invocation)
- [ ] `just debug` — build + launch QEMU with GDB stub (`-s -S`)
- [ ] `just test` — run `cargo test` on host (for unit tests that don't need QEMU)
- [ ] `just clippy` — run clippy with `--deny warnings`
- [ ] `just fmt` — run `cargo fmt` (reformats in place, for local use)
- [ ] `just fmt-check` — run `cargo fmt --check` (CI mode — exits non-zero if formatting differs)
- [ ] `just check` — fmt-check + clippy + build (CI shortcut; does not require QEMU)
- [ ] `just clean` — cargo clean

**Acceptance:** `just build`, `just clippy`, `just fmt-check`, and `just test` all pass with zero warnings. `just check` passes on a clean checkout without QEMU installed.

-----

### Step 6: QEMU Runner Configuration

**What:** Configure QEMU aarch64 with the virt machine so `just run` launches the kernel.

**Prerequisites:** QEMU 6.0+ (`qemu-system-aarch64`). Phase 1 UEFI boot will additionally require QEMU 7.0+ and the `edk2-aarch64` firmware package — install both now to avoid retrofitting CI later.

**Tasks:**
- [ ] QEMU invocation: `qemu-system-aarch64 -machine virt -cpu cortex-a72 -smp 4 -m 2G -nographic -kernel <kernel-elf>`
  - `-cpu cortex-a72` matches the Raspberry Pi 4 target hardware
  - `-smp 4` emulates 4 cores so the secondary core parking code from Step 3 is exercised
  - On macOS with Apple Silicon, `-accel hvf` can be added for host-accelerated execution (optional for Phase 0)
- [ ] Use `-kernel` with the ELF directly — no `objcopy` to flat binary needed. QEMU reads the ELF entry point from headers, and ELF preserves symbols for GDB debugging.
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
- [ ] Install QEMU 7.0+ and `edk2-aarch64` firmware in CI (install now; QEMU integration tests are optional for Phase 0 but Phase 1 needs both immediately)
- [ ] Add CI badge to `README.md`

**Acceptance:** Push to GitHub triggers CI. All jobs pass on a clean checkout.

-----

### Step 8: Panic Handler and Boot Diagnostics

**What:** Upgrade the stub panic handler to print to UART, define the Rust exception vector table, and print boot diagnostics.

**Tasks:**
- [ ] Upgrade `#[panic_handler]` from the Step 2 `loop {}` stub to print panic info (message, file, line) to UART, then halt (`loop { unsafe { core::arch::asm!("wfe") } }`)
- [ ] Add `kernel/src/arch/aarch64/exceptions.rs` — define the full exception vector table (16 entries at 128-byte spacing, `.align 7` per entry, all entries branch-to-self for now; real handlers come in Phase 1)
- [ ] Reinstall `VBAR_EL1` from `kernel_main` pointing to the Rust-defined vector table. The boot.S stub from Step 3 is a temporary safety net for the window between entry and this point — any fault in that window causes a deterministic halt (branch-to-self), which is intentional. The boot.S stub is removed when Phase 1 installs real exception handlers.
- [ ] Print boot diagnostics to UART: current EL (confirm it is 1), core ID from `MPIDR_EL1`, `VBAR_EL1` value (confirm it matches the vector table symbol address from `cargo objdump`)

**Acceptance:** A `panic!("test")` in `kernel_main` prints the panic message and file:line to UART, then halts. UART shows current EL = 1 and core ID = 0. `VBAR_EL1` printed value matches the vector table symbol from objdump.

-----

## Decision Points

| Decision | Options | Recommendation |
|---|---|---|
| Boot method for Phase 0 | UEFI stub vs. ELF loaded by `-kernel` | ELF via `-kernel`. UEFI stub is Phase 1 work. QEMU loads the ELF at the address in its headers, drops to EL1, and passes a DTB pointer in x0. |
| Kernel load address | `0x4008_0000` (512 KiB offset) vs. `0x4010_0000` (1 MiB offset) | `0x4008_0000`. The 512 KiB offset leaves room for QEMU's DTB at RAM base (`0x4000_0000`). The Phase 0 kernel is tiny. Move to `0x4010_0000` if DTB overlap is observed. |
| Linker script wiring | `.cargo/config.toml` hardcoded `-T` vs. `build.rs` | `build.rs`. The config.toml path is relative to the workspace root and silently breaks when building from subdirectories. `build.rs` with `cargo:rustc-link-arg` is path-safe and idiomatic. |
| UART approach | Hardcoded MMIO vs. HAL trait | Hardcoded for Phase 0. The HAL `Platform` trait and `detect_platform()` are Phase 1 work. Keep it simple now, refactor later. |
| Test strategy | Host-only unit tests vs. QEMU integration tests | Host-only for Phase 0. QEMU integration tests (boot → check UART output) in Phase 1. |

-----

## Phase Completion Criteria

- [ ] `just build` compiles a `#![no_std]` aarch64 kernel with zero warnings
- [ ] `just run` boots the kernel in QEMU and prints "AIOS kernel booting..." to UART
- [ ] `just clippy` and `just fmt-check` pass cleanly
- [ ] Panic handler prints location and message to UART
- [ ] Boot diagnostics confirm EL = 1, core ID = 0, VBAR_EL1 matches vector table symbol
- [ ] CI pipeline passes on clean checkout
- [ ] Project has LICENSE (BSD-2-Clause), README (with QEMU 6.0+ prerequisite), .gitignore
- [ ] Adding a new crate to workspace `[members]` and running `cargo build --workspace` succeeds without restructuring
