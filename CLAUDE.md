# AIOS — Claude Code Project Instructions

> **Rules and conventions** are in `.claude/rules/` (auto-loaded). This file contains reference data only.

## Project Identity

```text
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
Kernel load:    0x4008_0000 physical (Phase 0–1, identity map); VMA 0xFFFF_0000_0008_0000 (Phase 2+)
```

---

## Architecture Document Map

Topic-to-document index lives in `docs/project/doc-map.md`. Read it on demand when locating arch sections; do not inline here.

---

## Key Technical Facts

Only invariants that are easy to get wrong, cross-binary, or have explicit hang/UNPREDICTABLE warnings. Per-driver registers, struct sizes, MAX_* consts, build paths, and test counts live in code — grep for them.

```text
# Address layout (cross-binary contract)
QEMU virt RAM base:           0x4000_0000
Kernel LMA:                   0x4008_0000  (physical load; linker.ld AT clause)
Virtual kernel VMA:           0xFFFF_0000_0008_0000
VIRT_PHYS_OFFSET:             0xFFFE_FFFF_C000_0000  (KERNEL_VIRT - KERNEL_PHYS)
DIRECT_MAP_BASE:              0xFFFF_0001_0000_0000  (all RAM, RW+XN, 2MB blocks)
MMIO_BASE:                    0xFFFF_0010_0000_0000  (Attr0 device memory)
TTBR1 T1SZ:                   16  (48-bit kernel VA; set in boot.S before TTBR1_EL1 write)
BootInfo magic:               0x41494F53_424F4F54 ("AIOSBOOT")

# Early-boot fixed MMIO
UART base (PL011, QEMU):      0x0900_0000
PL011 APB clock (Phase 1+):   24 MHz
GICv3 GICD base:              0x0800_0000
GICv3 GICR base:              0x080A_0000
ARM Generic Timer freq:       62.5 MHz on QEMU; 1 ms tick = freq/1000 = 62500
Timer PPI INTID:              30 (EL1 physical timer)

# Boot invariants
QEMU boots to EL1 directly    (no EL2 setup)
MMU off at entry (Phase 0):   physical = virtual; MMIO works directly
FPU enable sequence:          mrs x1, CPACR_EL1; orr x1, x1, #(3 << 20); msr CPACR_EL1, x1; isb
                              MUST run before any Rust code.
Vector table:                 ALIGN(2048) in linker.ld + .balign 128 per entry in asm.
                              .text.vectors (boot.S stub) → .text.rvectors (exceptions.rs, set after kernel_main).
PSCI CPU_ON (64-bit):         0xC400_0003 — hvc on QEMU, smc on Pi 4/5
PSCI entry phys conversion:   smp.rs converts virtual _secondary_entry to physical before CPU_ON.
Boot CPU SP virt conversion:  boot.S adds VIRT_PHYS_OFFSET to SP before branching to virtual kernel_main.
Syscall ABI:                  SVC #0 from EL0; x8 = number, x0-x5 = args, x0 = return.
                              Phase 3 threads run at EL1 → IPC is a direct call, NOT SVC. SVC path wired for future EL0.

# MMU strategy (do not get this wrong)
edk2 state post-EBS:          MMU ON, SCTLR=0x30d0198d, TCR T0SZ=20 (44-bit VA)
edk2 MAIR:                    0xffbb4400 (Attr0=Device, Attr1=NC, Attr2=WT, Attr3=WB)
Phase 1 MMU strategy:         TTBR0-only swap, reuse edk2 MAIR/TCR.
                              Changing MAIR/TCR while MMU on is CONSTRAINED UNPREDICTABLE — do not.
Phase 1 identity map:         3×1GB blocks (device@0, RAM@0x40M, RAM@0x80M) via L0→L1.
TLBI Phase 1 (init_mmu):      tlbi vmalle1 + dsb nsh — non-broadcast.
                              Broadcast (vmalle1is + dsb ish) HANGS with parked cores under NC memory.
TLBI Phase 2+ (kmap/tlb):     tlbi vmalle1is + dsb ish — safe after WB upgrade enables global exclusive monitor.
Boot TTBR1 (boot.S):          3 static BSS pages (L0/L1/L2), 4×2MB blocks covering kernel image.
                              Minimal map sufficient to jump to virtual kernel_main.
Full TTBR1 (kmap.rs):         Built in kernel_main: text=RX, rodata=RO, data=RW, direct map, MMIO.
                              Replaces boot TTBR1 via TLBI VMALLE1IS.
KASLR (M8):                   Slide computed (CNTPCT_EL0 / rng_seed entropy) but NOT applied;
                              init_kernel_address_space ignores it. Non-zero slide is a later milestone.
TTBR0 format:                 bits[63:48] = ASID, bits[47:0] = PGD physical address.
TTBR0 switch barriers:        DSB SY → MSR TTBR0_EL1 → TLBI VMALLE1IS → DSB ISH → ISB.
ASID width:                   16-bit. AsidAllocator tracks generation; full TLBI VMALLE1IS on generation wrap.

# NC memory atomic limitation (causes hangs — read before any inter-core sync)
Exclusive load/store pairs (ldaxr/stlxr) require the global exclusive monitor, which only works
on Inner Shareable + Cacheable memory. spin::Mutex (and any atomic RMW: fetch_add, compare_exchange,
swap) HANGS on Non-Cacheable Normal memory.
  Phase 1: use only load(Acquire) / store(Release) for inter-core sync.
  Phase 2 M8: TTBR0 RAM blocks upgraded to WB (Attr3); spinlocks safe after TTBR1 active.

# Slab allocator
Size classes: 5 (64, 128, 256, 512, 4096B); smaller rounds up to 64. Backed by frame allocator (kernel pool).

# Concurrency / kernel correctness
Lock ordering (full, M25):    PROCESS_TABLE > SHARED_REGION_TABLE > NOTIFICATION_TABLE > CHANNEL_TABLE >
                              SELECT_WAITERS > BLOCK_ENGINE > SURFACE_TABLE >
                              {VIRTIO_BLK, VIRTIO_GPU, VIRTIO_INPUT (leaf)} >
                              {INPUT_QUEUE, PENDING_POINTER, FOCUS_MANAGER, WINDOW_Z_ORDER,
                               DRAG_STATE, CURSOR_POS, TITLE_FONT (leaf, independent)}
Capability enforcement:       channel_create → ChannelCreate;
                              ipc_call/send/recv → ChannelAccess;
                              ipc_reply → NONE (spec §9.1).
Compositor invariant (M25):   FOCUS_MANAGER, WINDOW_Z_ORDER, DRAG_STATE are leaves — every
                              public op snapshots state, drops the lock, then issues IPC
                              follow-ups. Never hold any of these across ipc_send/ipc_call.
```

---

## Phase Doc Generation Workflow

When generating a phase doc for Phase N:

1. **READ** in order:
   - `docs/project/development-plan.md` §8 — phase name, duration, deliverable
   - Architecture docs for the subsystems this phase implements (use `docs/project/doc-map.md` to locate them)
   - The previous phase doc — for milestone numbering continuity and "Unlocks" field

2. **STRUCTURE** (match established Phase 04/05 conventions):
   - Header: `# Phase N: <Name>`
   - Metadata block, `-----` separators between all sections
   - `## Objective`, `## Architecture References` (relative links), `## Milestones` (numbering context + summary table)
   - `## Milestone N — <Name> (timeframe)` with italic `*Goal:*` line, `### Step N:` subsections
   - Each Step: What, Tasks (checkboxes), Note (if needed), Key reference, Acceptance criteria
   - `-----` between every step and between milestones
   - `## Decision Points`, `## Phase Completion Criteria`

3. **CONVENTIONS**:
   - Never duplicate architecture content — reference it
   - Acceptance criteria must be mechanical (run command → see output)
   - Each phase has 3+ milestones (variable, no upper limit)
   - Step numbering: continuous across milestones within a phase (variable count per milestone), resets to 1 each new Phase
   - Milestone numbering: continuous across all phases (M16, M17, M18, M19...)
   - Duration must match `development-plan.md`
   - Full details in `/generate-phase-doc` skill

---

## Workspace Layout

Cargo workspace, three members. Run `ls kernel/src` for current per-file breakdown.

```text
aios/
├── Cargo.toml            workspace root (resolver = "2"; members: kernel, shared, uefi-stub)
├── rust-toolchain.toml   pinned nightly (aarch64-unknown-none + aarch64-unknown-uefi)
├── justfile              build / build-stub / disk / run* / check / test / clean
├── .claude/
│   ├── agents/           team-lead, kernel-dev, doc-writer, code-reviewer, verifier, doc-auditor
│   ├── rules/            01-code-conventions … 09-tool-priority (auto-loaded)
│   └── skills/           build-team, generate-phase-doc, implement-phase, review-pr-comments,
│                         verify-phase, write-arch-doc, audit-loop, merge-and-cleanup
├── kernel/src/           bare-metal aarch64 kernel (no_std, no_main)
│   ├── arch/aarch64/     boot.S, exceptions, gic, timer, mmu, psci, trap, uart, linker.ld
│   ├── platform/         Platform trait + per-board (qemu)
│   ├── mm/               buddy / slab / pools / page tables / kmap / kaslr / asid / tlb / heap / uspace
│   ├── sched/            scheduler, run queues, load balancer
│   ├── ipc/              channels, direct switch, shmem, notify, select, timeouts
│   ├── cap/              capability tokens & table
│   ├── task/             Thread, Process, ResourceLimits
│   ├── service/          service manager + audit ring
│   ├── syscall/          syscall dispatch (SVC #0)
│   ├── drivers/          virtio_common + virtio_blk / virtio_gpu / virtio_input
│   ├── input/            event translation, polling thread, INPUT_QUEUE
│   ├── gpu/              GPU Service, boot log text rendering
│   ├── compositor/       Compositor service, surface lifecycle, decorations,
│   │                     hit-test/cursor, focus, input routing + IPC dispatch,
│   │                     window move/resize, system hotkeys (M25 adds
│   │                     window/cursor/focus/input_route/hotkey/text)
│   ├── storage/          BlockEngine, WAL, MemTable, object/version stores, crypto, posix bridge, budget
│   ├── observability/    structured log, metrics, trace (feature-gated)
│   └── (top-level)       main.rs, boot_phase, dtb, smp, framebuffer, bench
├── shared/src/           types crossing kernel/stub boundary (no_std)
│   ├── (top-level)       boot, cap, ipc, sched, memory, storage, gpu, input, compositor, syscall,
│   │                     kaslr, observability, collections, lib
│   └── kits/             Kit traits: memory, capability, ipc, storage, compute
├── uefi-stub/src/        UEFI stub: BootInfo assembly, ELF loader, ExitBootServices, kernel jump
└── docs/                 architecture, phase, knowledge docs
```

---

## Team & Agent Architecture

Single team lead + specialist agents. Fully autonomous — human reviews async via PRs.

**Agents** (defined in `.claude/agents/`):

| Agent | Role | Spawned by |
| --- | --- | --- |
| `team-lead` | Orchestrates phases, manages tasks, commits, creates PRs | User or `/build-team` |
| `kernel-dev` | Implements Rust/asm code per phase doc steps | team-lead |
| `doc-writer` | Generates phase docs from architecture docs | team-lead |
| `code-reviewer` | Runs quality gates, reviews code conventions | team-lead |
| `verifier` | Boots QEMU, validates acceptance criteria | team-lead |
| `doc-auditor` | Validates docs on every change, loops until clean | Hook (auto) or team-lead |

**Skills** (defined in `.claude/skills/`):

| Skill | Trigger | Purpose |
| --- | --- | --- |
| `/build-team` | Start of autonomous session | Creates team, spawns agents |
| `/implement-phase N` | Phase implementation request | Full phase implementation workflow |
| `/generate-phase-doc N` | Phase doc request | Generates phase doc from arch docs |
| `/verify-phase N` | After implementation | Runs all quality gates |
| `/review-pr-comments` | After PR creation | Wait for reviewer comments, fix, reply, resolve |
| `/write-arch-doc <topic-or-path>` | Architecture doc request | Interactive create/update architecture docs with research |
| `/merge-and-cleanup [PR]` | After PR approval | Squash merge, delete branch, remove worktree, update main |

**Document Lifecycle**: All doc changes go to `claude/*` branches with PRs. Doc-auditor loops (audit → fix → re-audit) until zero issues, max 10 passes.

**Existing skills reused** (not recreated):

- `superpowers:writing-plans`, `superpowers:verification-before-completion`
- `engineering-workflow-skills:pr`, `commit-commands:commit`
- `sc:implement`, `sc:test`, `sc:build`, `sc:analyze`
- `pr-review-toolkit:review-pr`
