# AIOS — Claude Code Project Instructions

## Project Identity

```
Name:           AIOS — AI-First Operating System
Target arch:    aarch64 (hard-float ABI)
Kernel target:  aarch64-unknown-none
UEFI target:    aarch64-unknown-uefi  (Phase 1+)
Host target:    native (for unit tests, shared crate)
Build system:   just + cargo
License:        BSD-2-Clause
Toolchain:      Rust nightly (updated to latest at session start, pinned in rust-toolchain.toml)
Workspace:      resolver = "2", edition = "2021"
Linker script:  emitted via build.rs (not .cargo/config.toml)
Relocation:     static (relocation-model=static throughout all phases)
QEMU machine:   virt, cpu=cortex-a72, -smp 4 -m 2G
UART:           PL011 at 0x0900_0000 (QEMU); DTB-sourced Phase 1+
Kernel load:    0x4008_0000 physical (Phase 0), virtual 0xFFFF_0000_0000_0000+ (Phase 1+)
```

---

## Architecture Document Map

| Topic | Document | Key Sections |
|---|---|---|
| System overview & vision | `docs/project/overview.md` | §1 Vision, §2 Architecture |
| Development plan & phases | `docs/project/development-plan.md` | §3 Dependencies, §8 Phase table |
| Full architecture | `docs/project/architecture.md` | All |
| Boot sequence (Phase 0, QEMU `-kernel`) | `docs/kernel/boot.md` | §3.3 Steps 1-2 |
| Boot sequence (Phase 1+, UEFI) | `docs/kernel/boot.md` | §2 full, §3.3 Steps 1-9 |
| Boot lifecycle & phases | `docs/kernel/boot-lifecycle.md` | All |
| BootInfo struct | `docs/kernel/boot.md` | §2.2 |
| HAL & Platform trait | `docs/kernel/hal.md` | §2-3 |
| PL011 UART driver | `docs/kernel/hal.md` | §4.3 |
| GICv3 interrupt controller | `docs/kernel/hal.md` | §4.1 |
| ARM Generic Timer | `docs/kernel/hal.md` | §4.2 |
| Virtual memory & page tables | `docs/kernel/memory.md` | §3-3.2 |
| Physical memory (buddy allocator) | `docs/kernel/memory.md` | §2.2 |
| Slab allocator & heap | `docs/kernel/memory.md` | §4.1 |
| IPC & syscalls | `docs/kernel/ipc.md` | All (Phase 3+) |
| Scheduler | `docs/kernel/scheduler.md` | All (Phase 3+) |
| Space Storage | `docs/storage/spaces.md` | All (Phase 4+) |
| Flow (smart clipboard) | `docs/storage/flow.md` | All (Phase 11+) |
| Compositor | `docs/platform/compositor.md` | All (Phase 5-6+) |
| Networking | `docs/platform/networking.md` | All (Phase 7+) |
| Audio subsystem | `docs/platform/audio.md` | All (Phase 22+) |
| Subsystem framework | `docs/platform/subsystem-framework.md` | All |
| POSIX compatibility | `docs/platform/posix.md` | All (Phase 15+) |
| Power management | `docs/platform/power-management.md` | All (Phase 19+) |
| AI Runtime (AIRS) | `docs/intelligence/airs.md` | All (Phase 8+) |
| Context engine | `docs/intelligence/context-engine.md` | All (Phase 8+) |
| Attention management | `docs/intelligence/attention.md` | All (Phase 11+) |
| Task manager | `docs/intelligence/task-manager.md` | All (Phase 11+) |
| Preferences | `docs/intelligence/preferences.md` | All (Phase 8+) |
| Agents | `docs/applications/agents.md` | All (Phase 10+) |
| Browser | `docs/applications/browser.md` | All (Phase 21+) |
| UI toolkit | `docs/applications/ui-toolkit.md` | All (Phase 20+) |
| Security model | `docs/security/security.md` | All (all phases) |
| Experience layer | `docs/experience/experience.md` | All (Phase 6+) |
| Accessibility | `docs/experience/accessibility.md` | All (Phase 23+) |
| Identity | `docs/experience/identity.md` | All (Phase 3+) |

---

## Session Start Checklist

Before any implementation work, run these steps at the start of every session:

1. **Update system tools**: Run `brew upgrade qemu just` to get the latest QEMU and just versions
2. **Update Rust nightly toolchain**: Check for the latest nightly (`rustc +nightly --version`), update `rust-toolchain.toml` to the latest date, verify the build still passes
3. **Update dependencies**: Run `cargo update` to pull latest compatible versions of all dependencies, commit `Cargo.lock` if changed
4. **Verify build**: Run `just check` (or `cargo build --target aarch64-unknown-none` if justfile doesn't exist yet) to confirm zero warnings after updates

---

## Phase Implementation Workflow

When implementing Phase N:

1. **READ** (in this order):
   - `docs/phases/NN-phase-name.md` — the phase implementation doc
   - All architecture docs listed in the phase doc's "Architecture References" table
   - This file's Code Conventions and Quality Gates sections

2. **BRANCH**: Create `claude/phase-N-MK-name` from latest `main` (one branch per milestone)
   - Example: `claude/phase-0-m2-boots` for Phase 0 Milestone 2

3. **PLAN** before writing any code:
   - Identify which Milestone you are targeting (M1/M2/M3)
   - List files to create or modify
   - Verify no step dependencies are unmet
   - Use TodoWrite for milestone tracking

4. **IMPLEMENT** one step at a time:
   - Each step in the phase doc is atomic — complete it fully before moving on
   - Every step has an "Acceptance:" block — this is your done condition
   - Do not proceed to the next step if acceptance criteria are not met

5. **VERIFY** after each step:
   - Run the acceptance criteria commands (`cargo build`, `just run`, `just check`, etc.)
   - For QEMU output: match exact strings in acceptance criteria
   - For objdump: check section addresses match linker script values

6. **UPDATE CLAUDE.md** after each milestone:
   - Add new files/dirs to Workspace Layout
   - Add new addresses/offsets to Key Technical Facts
   - Add new doc references to Architecture Doc Map
   - Add new patterns to Code Conventions
   - Update Quality Gates if new verification steps were added

7. **COMMIT** after each Milestone completes:
   - Format: `Phase N MK: <Milestone name>`
   - Example: `Phase 0 M1: Compiles — aarch64 ELF with zero warnings`
   - Include CLAUDE.md updates in the same commit

8. **PR** after each milestone completes: push branch, create PR to `main`
   - One PR per milestone — keeps reviews small and focused
   - Merge to `main` before starting the next milestone

**BLOCKED?** Read the referenced architecture doc section. Architecture docs are the source of truth. Never invent register offsets, struct fields, or memory addresses.

---

## Code Conventions

### Rust

- `#![no_std]` everywhere in `kernel/` and `shared/`
- `#![no_main]` in `kernel/` and `uefi-stub/`
- All `unsafe` blocks require a `// SAFETY:` comment (see Unsafe Documentation Standard below)
- No TODO comments in code — complete implementations only
- Naming: `snake_case` for functions/variables, `CamelCase` for types, `SCREAMING_SNAKE` for constants
- Error handling: `Result<T, E>` for fallible operations; panics reserved for unrecoverable invariant violations
- Panic handler: always prints to UART then halts with `wfe` loop (not `loop {}`)
- Prefer the best approach over the simplest — choose the design that is cleanest, most maintainable, and architecturally sound, even if a shortcut exists

### Architecture-Specific (aarch64)

- FPU must be enabled before any Rust code runs (`boot.S` is responsible)
- BSS must be zeroed before `kernel_main` is called (`boot.S` is responsible)
- `VBAR_EL1` must be set before interrupts are unmasked
- All MMIO access via `core::ptr::read_volatile` / `core::ptr::write_volatile`
- Memory-mapped registers: define as `const` physical addresses; map to virtual after Phase 1 MMU
- W^X: no page is both writable and executable
- Stack alignment: 16-byte (ABI requirement)
- Secondary cores: park with `wfe` (not `wfi`) — `sev` wakes all simultaneously

### Assembly

- Files use `.S` extension (uppercase — Rust build system handles preprocessing)
- Entry symbols: `#[no_mangle]` on the Rust side
- Vector table: `.align 7` (128 bytes) per entry in assembly; `ALIGN(2048)` for section in linker script
- All 16 exception vector entries present; stubs `b .` until real handlers added
- Boot order (strict): FPU enable → VBAR install → park secondaries → set SP → zero BSS → branch to `kernel_main`

### Crate & Dependency Rules

- All kernel crates: `no_std`, `no_main`
- All dependencies: must be `no_std` compatible
- License: MIT or Apache-2.0 preferred (BSD-2-Clause compatible). **No GPL in kernel/ or shared/**
- `Cargo.lock`: committed (binary crate, reproducible builds)

---

## File Placement

```
kernel/src/arch/aarch64/       aarch64-specific code (uart.rs, exceptions.rs, boot.S, linker.ld)
kernel/src/arch/aarch64/mod.rs re-exports arch-specific items
kernel/src/                    platform-agnostic kernel logic
shared/src/lib.rs              types crossing kernel/stub boundary (BootInfo, etc.)
uefi-stub/src/                 UEFI stub code (Phase 1+)
docs/phases/                   phase implementation docs (NN-name.md, flat, no subdirs)
```

---

## Quality Gates

Every milestone must pass all applicable gates:

| Gate | Command | Passes when |
|---|---|---|
| Compile | `cargo build --target aarch64-unknown-none` | Zero warnings |
| Check | `just check` (fmt-check + clippy + build) | Zero warnings, zero errors |
| Test | `just test` (host-side unit tests) | All pass |
| QEMU | `just run` | Expected UART string matches phase acceptance criteria |
| CI | Push to GitHub | All CI jobs pass |
| Objdump | `cargo objdump -- -h` | Sections at expected addresses |
| EL | Boot diagnostics | EL = 1, core ID = 0 |

Never mark a milestone complete if any gate fails.

---

## Key Technical Facts

```
QEMU virt RAM base:           0x4000_0000
Kernel load address:          0x4008_0000 (Phase 0); virtual mapping Phase 1+
UART base (QEMU):             0x0900_0000
UART DR offset:               0x000
UART FR offset:               0x018 (TXFF = bit 5, BUSY = bit 3)
UART IBRD:                    0x024
UART FBRD:                    0x028
UART LCR_H:                   0x02C
UART CR:                      0x030
GICv3 GICD base:              0x0800_0000
GICv3 GICR base:              0x080A_0000
ARM Generic Timer frequency:  62.5 MHz (62500000 Hz) on QEMU
1 ms tick count:              freq / 1000 = 62500
PL011 UART clock (Phase 1+):  24 MHz (APB peripheral clock)
PL011 baud 115200:            IBRD=13, FBRD=1
BootInfo magic:               0x41494F53_424F4F54 ("AIOSBOOT" as u64)
PSCI CPU_ON (64-bit):         0xC400_0003
PSCI conduit on QEMU:         hvc; on Pi 4/5: smc
FPU enable sequence:          mrs x1, CPACR_EL1; orr x1, x1, #(3 << 20); msr CPACR_EL1, x1; isb
QEMU boot to EL:              EL1 directly (no EL2 setup needed)
MMU off at entry (Phase 0):   physical = virtual; MMIO works directly
Vector table alignment:       section ALIGN(2048) in linker.ld + .balign 128 per entry in asm
Boot stub vectors section:    .text.vectors (boot.S, early boot safety net)
Rust vectors section:         .text.rvectors (exceptions.rs, installed from kernel_main)
llvm-tools component name:    llvm-tools (not llvm-tools-preview)
QEMU serial flag:             -nographic (implies -serial mon:stdio; no explicit -serial)
QEMU GDB flag:                -gdb tcp::1234 (not -s)
```

---

## Phase Doc Generation Workflow

When generating a phase doc for Phase N:

1. **READ** in order:
   - `docs/project/development-plan.md` §8 — phase name, duration, deliverable
   - Architecture docs for the subsystems this phase implements (cross-reference against Architecture Document Map above)
   - The previous phase doc — for milestone numbering continuity and "Unlocks" field

2. **STRUCTURE** (match Phase 0/1 template exactly):
   - Header: `# Phase N: <Name>`
   - Metadata: Tier, Duration, Deliverable, Status: Planned, Prerequisites, Unlocks
   - `## Objective` — 2-3 paragraphs
   - `## Architecture References` — table: Topic | Document | Relevant Sections
   - `## Milestones` — table: Milestone | Steps | Target | Observable result
   - One `## Milestone N` section per milestone, with `### Step N:` subsections
   - Each Step: What, Tasks (checkboxes), Note (if needed), Key reference, Acceptance criteria
   - `## Decision Points` — table
   - `## Phase Completion Criteria` — checklist

3. **CONVENTIONS**:
   - Never duplicate architecture content — reference it
   - Acceptance criteria must be mechanical (run command → see output)
   - Each phase has exactly 3 milestones
   - Duration must match `development-plan.md`

---

## Milestone Numbering

```
Phase 0:  M1–M3
Phase 1:  M4–M6
Phase 2:  M7–M9
Phase N:  M(3N+1) – M(3N+3)
```

---

## Workspace Layout

Current (post-Phase 0 M3 — Scaffold Complete):

```
aios/
├── CLAUDE.md
├── README.md
├── CONTRIBUTING.md
├── .gitignore
├── Cargo.toml            workspace root (resolver = "2", edition = "2021")
├── Cargo.lock            committed for reproducibility
├── rust-toolchain.toml   pinned nightly + aarch64-unknown-none + components
├── justfile              build recipes (build, run, debug, check, test, clean)
├── LICENSE               BSD-2-Clause
├── .cargo/
│   └── config.toml       relocation-model=static for aarch64-unknown-none
├── .claude/
│   ├── settings.json
│   ├── agents/           team-lead, kernel-dev, doc-writer, code-reviewer, verifier, doc-auditor
│   └── skills/           build-team, implement-phase, generate-phase-doc, verify-phase
├── .github/
│   └── workflows/ci.yml  check + build-release + test
├── kernel/
│   ├── Cargo.toml
│   ├── build.rs          emits linker script path
│   └── src/
│       ├── main.rs       kernel_main, global_asm, println, panic_handler, boot diagnostics
│       └── arch/aarch64/
│           ├── mod.rs    pub mod uart, pub mod exceptions
│           ├── boot.S    FPU enable, VBAR install, park secondaries, BSS zero, branch kernel_main
│           ├── uart.rs   PL011 driver (putc, _print, print!/println! macros)
│           ├── exceptions.rs  Rust exception vector table + CPU register helpers
│           └── linker.ld .text.boot, .text.vectors, .text.rvectors, stack 16 KiB
├── shared/
│   ├── Cargo.toml
│   └── src/lib.rs        BootInfo, PhysAddr, VirtAddr, MemoryType, PixelFormat
└── docs/                 (existing architecture + phase docs)
```

---

## Unsafe Documentation Standard

Every `unsafe` block in `kernel/` requires a preceding comment:

```rust
// SAFETY: <invariant that makes this safe>
// <who maintains the invariant>
// <what happens if violated>
unsafe { ... }
```

Examples:

```rust
// SAFETY: UART base address 0x0900_0000 is valid MMIO on QEMU virt.
// QEMU maps this region unconditionally. Writing to unmapped memory
// on a different machine would cause a synchronous abort.
unsafe { core::ptr::write_volatile(uart_base as *mut u32, byte as u32) };
```

---

## Git Branching Convention

All work happens on `claude/*` branches. Never commit directly to `main`.

- Milestone implementations: `claude/phase-N-MK-name` (e.g., `claude/phase-0-m2-boots`)
- Doc generation: `claude/phase-N-docs` (e.g., `claude/phase-5-docs`)
- Doc updates from code changes: `claude/docs-update-*`
- One PR per milestone — merge to `main` before starting the next milestone

---

## Team & Agent Architecture

Single team lead + specialist agents. Fully autonomous — human reviews async via PRs.

**Agents** (defined in `.claude/agents/`):

| Agent | Role | Spawned by |
|---|---|---|
| `team-lead` | Orchestrates phases, manages tasks, commits, creates PRs | User or `/build-team` |
| `kernel-dev` | Implements Rust/asm code per phase doc steps | team-lead |
| `doc-writer` | Generates phase docs from architecture docs | team-lead |
| `code-reviewer` | Runs quality gates, reviews code conventions | team-lead |
| `verifier` | Boots QEMU, validates acceptance criteria | team-lead |
| `doc-auditor` | Validates docs on every change, loops until clean | Hook (auto) or team-lead |

**Skills** (defined in `.claude/skills/`):

| Skill | Trigger | Purpose |
|---|---|---|
| `/build-team` | Start of autonomous session | Creates team, spawns agents |
| `/implement-phase N` | Phase implementation request | Full phase implementation workflow |
| `/generate-phase-doc N` | Phase doc request | Generates phase doc from arch docs |
| `/verify-phase N` | After implementation | Runs all quality gates |

**Document Lifecycle**: All doc changes go to `claude/*` branches with PRs. Doc-auditor loops (audit → fix → re-audit) until zero issues, max 10 passes.

**Existing skills reused** (not recreated):
- `superpowers:writing-plans`, `superpowers:verification-before-completion`
- `engineering-workflow-skills:pr`, `commit-commands:commit`
- `sc:implement`, `sc:test`, `sc:build`, `sc:analyze`
- `pr-review-toolkit:review-pr`

---

## CLAUDE.md Self-Maintenance

Team-lead updates this file after every milestone:

1. Review what changed (new files, crates, constants, conventions)
2. Update: Workspace Layout, Key Technical Facts, Architecture Doc Map, Code Conventions, Quality Gates
3. Commit as part of the milestone commit (same commit)
