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
Kernel load:    0x4008_0000 physical (Phase 0–1, identity map); VMA 0xFFFF_0000_0008_0000 (Phase 2+)
```

---

## Architecture Document Map

| Topic | Document | Key Sections |
|---|---|---|
| System overview & vision | `docs/project/overview.md` | §1 Vision, §2 Architecture |
| Development plan & phases | `docs/project/development-plan.md` | §3 Dependencies, §5 Gates (incl. Gate 1 retro), §8 Phase table, §8.1 Actual progress |
| Full architecture | `docs/project/architecture.md` | All |
| Language ecosystem (hub) | `docs/project/language-ecosystem.md` | §1 Overview, Document Map, Impl Order |
| Language runtimes | `docs/project/language-ecosystem/runtimes.md` | §2 Rust, §3 Python, §4 TypeScript (QuickJS-ng), §5 WASM (wasmtime + WAMR) |
| Language integration & build plan | `docs/project/language-ecosystem/integration.md` | §6 Dependency chain, §7 Build plan, §8 Key decisions, RuntimeAdapter trait |
| Language operations & security | `docs/project/language-ecosystem/operations.md` | §9 Interop (WIT/Component Model), §10 Observability, §11 Supply chain, §12 Resource isolation |
| Language AI optimization | `docs/project/language-ecosystem/ai.md` | §13 AIRS Runtime Advisor/scheduling/allocation/GC/anomaly, §14 Future directions |
| Boot sequence (hub) | `docs/kernel/boot.md` | §1 Overview, Document Map, Future Directions |
| Firmware handoff (BootInfo, ESP, EL model) | `docs/kernel/boot/firmware.md` | §2.1–§2.6 |
| Kernel early boot (boot.S, kernel_main) | `docs/kernel/boot/kernel.md` | §3.1–§3.6 |
| Service Manager boot phases | `docs/kernel/boot/services.md` | §4–§5 |
| Boot performance & framebuffer | `docs/kernel/boot/performance.md` | §6–§7 |
| Panic handler, recovery, initramfs | `docs/kernel/boot/recovery.md` | §8–§10 |
| Shutdown, implementation order, principles | `docs/kernel/boot/lifecycle.md` | §11, §12, §23, §24 |
| Boot test strategy | `docs/kernel/boot/testing.md` | §13–§14 |
| Suspend/resume, semantic state | `docs/kernel/boot/suspend.md` | §15 |
| Boot intelligence, on-demand services | `docs/kernel/boot/intelligence.md` | §16–§18 |
| Boot accessibility, first boot | `docs/kernel/boot/accessibility.md` | §19–§21 |
| Research kernel innovations | `docs/kernel/boot/research.md` | §22.1–§22.19 |
| HAL & Platform trait | `docs/kernel/hal.md` | §2-3 |
| PL011 UART driver | `docs/kernel/hal.md` | §4.3 |
| GICv3 interrupt controller | `docs/kernel/hal.md` | §4.1 |
| ARM Generic Timer | `docs/kernel/hal.md` | §4.2 |
| Memory management (hub) | `docs/kernel/memory.md` | §1 Overview, §14 Impl order, doc map |
| Physical memory (buddy allocator) | `docs/kernel/memory/physical.md` | §2.2 BuddyAllocator, §2.3 FrameAllocator, §2.4 PagePools |
| Slab allocator & heap | `docs/kernel/memory/physical.md` | §4.1 SlabAllocator, §4.2 Kernel Heap |
| Virtual memory & page tables | `docs/kernel/memory/virtual.md` | §3.2 PageTableEntry, §3.3 KASLR, §3.4 TLB/ASID |
| Per-agent address spaces | `docs/kernel/memory/virtual.md` | §5 Per-Agent Memory, §7 Shared Memory |
| AI model memory | `docs/kernel/memory/ai.md` | §6 Model regions, PagedAttention, KV caches |
| Memory pressure & reclamation | `docs/kernel/memory/reclamation.md` | §8 Pressure/OOM, §10 Swap/zram, §12 Scaling |
| Memory hardening | `docs/kernel/memory/hardening.md` | §9 W^X/PAC/BTI/MTE, §11 Perf, §13 Future |
| IPC & syscalls | `docs/kernel/ipc.md` | All (Phase 3+) |
| Scheduler | `docs/kernel/scheduler.md` | All (Phase 3+) |
| Deadlock prevention | `docs/kernel/deadlock-prevention.md` | All (Phase 3+) |
| Kernel observability | `docs/kernel/observability.md` | All (Phase 3+) |
| Space Storage (hub) | `docs/storage/spaces.md` | §1 Core Insight, §2 Architecture, §11 Design Principles, §12 Impl Order, Document Map |
| Storage data structures | `docs/storage/spaces/data-structures.md` | §3.0–§3.4 Primitive types, Spaces, Objects, CompactObject, Relations |
| Block Engine | `docs/storage/spaces/block-engine.md` | §4.1–§4.10 On-disk layout, LSM-tree, WAL, compression, encryption, WAF |
| Version Store | `docs/storage/spaces/versioning.md` | §5.1–§5.5 Merkle DAG, snapshots, retention, branching |
| Storage encryption | `docs/storage/spaces/encryption.md` | §6.1–§6.3 Key management, nonces, encryption zones |
| Query Engine | `docs/storage/spaces/query-engine.md` | §7.1–§7.6 Query dispatch, full-text, embeddings, learned indexes |
| Space Sync | `docs/storage/spaces/sync.md` | §8.1–§8.4 Merkle exchange, conflict resolution, sync security |
| POSIX compatibility (storage) | `docs/storage/spaces/posix.md` | §9.1–§9.6 Path mapping, translation layer, fd lifecycle |
| Storage budget & pressure | `docs/storage/spaces/budget.md` | §10.1–§10.9 Device profiles, quotas, pressure, AI-driven storage |
| Flow (hub) | `docs/storage/flow.md` | §1 Overview, §2 Architecture, §13 Impl order, §14 Principles, Document Map |
| Flow data model | `docs/storage/flow/data-model.md` | §3.0–§3.4 External types, FlowEntry, transfer lifecycle, TypedContent |
| Flow transforms | `docs/storage/flow/transforms.md` | §4.1–§4.3 Transform engine, pipeline, registry, conversion graph |
| Flow history & sync | `docs/storage/flow/history.md` | §5.1–§5.3 History storage/UI/retention, §9.1–§9.2 Multi-device sync |
| Flow integration | `docs/storage/flow/integration.md` | §6 Compositor, §7 Subsystem channels, §8 Cross-agent, §10 POSIX bridge |
| Flow security | `docs/storage/flow/security.md` | §11.1–§11.3 Capability enforcement, content screening, rate limiting |
| Flow SDK | `docs/storage/flow/sdk.md` | §12.1–§12.3 Rust/Python/TypeScript APIs, PWA web API |
| Flow extensions | `docs/storage/flow/extensions.md` | §15.1–§15.8 Near-term, §16.1–§16.11 Future directions |
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
| Inspector (security dashboard) | `docs/applications/inspector.md` | All (Phase 13+) |
| UI toolkit | `docs/applications/ui-toolkit.md` | All (Phase 20+) |
| Security model (hub) | `docs/security/model.md` | §1 Threat model, §12 Impl order, Document Map |
| Security defense layers | `docs/security/model/layers.md` | §2 Eight security layers deep dive |
| Capability system internals | `docs/security/model/capabilities.md` | §3.1–§3.6 Token lifecycle, kernel table, attenuation, delegation, temporal caps |
| Composable capability profiles | `docs/security/model/capabilities.md` | §3.7 (Phase 28) |
| Crypto, ARM HW security, testing | `docs/security/model/hardening.md` | §4 Crypto, §5 ARM HW, §8 Testing |
| Security operations & zero trust | `docs/security/model/operations.md` | §6 Events, §7 Audit, §9 AIRS, §10 Zero trust, §11 Comparisons, §13 Future |
| AIRS capability intelligence | `docs/intelligence/airs.md` | §5.9 (Phase 29) |
| Fuzzing & input hardening (hub) | `docs/security/fuzzing.md` | §1 Overview, §2 Attack surface, Document Map |
| Fuzzing hardening strategies | `docs/security/fuzzing/strategies.md` | §3.1–3.7 Language, syscall, memory, IPC, driver, manifest, concurrency |
| Fuzzing adoption roadmap | `docs/security/fuzzing/adoption-roadmap.md` | §4.1–4.7 Phased adoption (host-side through formal verification) |
| Fuzzing tooling & catalog | `docs/security/fuzzing/tooling.md` | §5.1–5.4 Tiered tooling, §6 Fuzz target catalog |
| Fuzzing AI-native strategies | `docs/security/fuzzing/ai-native.md` | §7.1–7.3 Dev-time AI, kernel-internal AI, AIRS-dependent |
| Static analysis & formal verification | `docs/security/static-analysis.md` | All (all phases) |
| Experience layer | `docs/experience/experience.md` | All (Phase 6+) |
| Accessibility | `docs/experience/accessibility.md` | All (Phase 23+) |
| Identity | `docs/experience/identity.md` | All (Phase 3+) |
| Developer guide | `docs/project/developer-guide.md` | All (all phases) |
| AI agent context | `docs/project/ai-agent-context.md` | All (all phases) |

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

6. **COMMIT + PUSH** after each step completes:
   - Format: `Phase N MK: Step X — <step description>`
   - Example: `Phase 2 M8: Step 4 — page table infrastructure`
   - Commit and push immediately after each step passes verification
   - Do not batch multiple steps into a single commit

7. **UPDATE ALL DOCS** after each milestone:
   - **CLAUDE.md**: Workspace Layout, Key Technical Facts, Architecture Doc Map, Code Conventions, Quality Gates
   - **README.md**: Project Structure, Build Commands, status text — anything that changed
   - **Phase doc** (`docs/phases/NN-*.md`): Check off completed task boxes (`[ ]` → `[x]`), update Status field (e.g. "In Progress (M4 complete)")
   - **Phase Completion Criteria**: Check off the completed milestone checkbox
   - **Developer guide** (`docs/project/developer-guide.md`): Update file size examples (§3.1), test counts (§5.2, §5.4), and any new patterns or lessons learned from the milestone
   - **Architecture docs** (`docs/kernel/*.md`, `docs/project/*.md`, etc.): Update any referenced architecture docs if the implementation revealed corrections, new facts, or deviations from the spec

8. **AUDIT** after all steps complete, before PR — run recursively until all reach 0 issues:
   - **Doc audit**: Cross-reference errors, technical accuracy, naming consistency in all modified docs
   - **Code review**: Convention compliance, unsafe documentation, W^X, naming, dead code
   - **Security/bug review**: Logic errors, address confusion (virt vs phys), PTE bit correctness, race conditions
   - Fix all genuine issues found, commit, and re-run all three audits
   - Repeat until a full round returns 0 issues across all three categories

9. **PR** after audits pass clean: push branch, create PR to `main`
   - One PR per milestone — keeps reviews small and focused
   - After PR creation: wait 3–7 minutes for Copilot/automated reviewers to post comments
   - Check Copilot/reviewer comments, fix issues, reply and resolve conversations
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
- Phase 1 NC memory: `spin::Mutex` and atomic RMW (`fetch_add`, `compare_exchange`) use exclusive load/store pairs that require Inner Shareable + Cacheable memory. They **hang** on Non-Cacheable Normal memory (Phase 1 identity map). Use only `load(Acquire)` / `store(Release)` for inter-core synchronization until Phase 2 enables WB cacheable attributes.

### Assembly

- Files use `.S` extension (uppercase — Rust build system handles preprocessing)
- Entry symbols: `#[no_mangle]` on the Rust side
- Vector table: `.align 7` (128 bytes) per entry in assembly; `ALIGN(2048)` for section in linker script
- All 16 exception vector entries present; stubs `b .` until real handlers added
- Boot order (strict): FPU enable → VBAR install → park secondaries → set SP → zero BSS → build minimal TTBR1 → configure TCR T1SZ → install TTBR1 → convert SP to virtual → branch to virtual `kernel_main`
- Boot CPU SP: converted from physical to virtual in boot.S (add VIRT_PHYS_OFFSET) before branching to kernel_main. Secondary core SPs remain physical (accessed via TTBR0 identity map).
- Exception handler: uses direct `putc()` output, not `println!()`, to prevent recursive faults when TTBR0 is switched away from identity map

### Crate & Dependency Rules

- All kernel crates: `no_std`, `no_main`
- All dependencies: must be `no_std` compatible
- License: MIT or Apache-2.0 preferred (BSD-2-Clause compatible). **No GPL in kernel/ or shared/**
- `Cargo.lock`: committed (binary crate, reproducible builds)

---

## File Placement

```
kernel/src/arch/aarch64/       aarch64-specific code (uart, exceptions, gic, timer, mmu, psci, trap, boot.S, context_switch.S, linker.ld)
kernel/src/arch/aarch64/mod.rs re-exports arch-specific items (uart, exceptions, gic, timer, mmu, psci, trap)
kernel/src/platform/           Platform trait + per-board implementations (qemu.rs)
kernel/src/mm/                 Memory management (bump, buddy, slab, pools, frame, init, pgtable, kmap, kaslr, asid, tlb, GlobalAlloc)
kernel/src/observability/      Structured logging, metrics, trace points
kernel/src/sched/              Scheduler: per-CPU run queues (4-class FIFO), schedule(), block/unblock, idle threads, load balancer
kernel/src/ipc/                IPC channels, call/reply, direct switch, timeouts, shared memory, notifications, select
kernel/src/cap/                Capability system: per-process tables, enforcement API, cascade revocation
kernel/src/task/               Thread/process data structures for scheduler and IPC
kernel/src/service/            Service manager: registry, echo service, process lifecycle, audit ring
kernel/src/syscall/            Syscall dispatch and handlers (IPC 0-9, Notify 10-12, Stats 13, Cap 14-17, Mem 18-22, Proc 23-25, Time 26-28, Audit 29, Debug 30)
kernel/src/drivers/            Device drivers (virtio_blk)
kernel/src/storage/            Block Engine, WAL, LSM-tree MemTable (Phase 4+)
kernel/src/                    platform-agnostic kernel logic (boot_phase.rs, dtb.rs, smp.rs, framebuffer.rs, bench.rs)
shared/src/                    types crossing kernel/stub boundary (boot, cap, collections, ipc, kaslr, memory, observability, sched, storage, syscall)
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
edk2 MMU state post-EBS:      MMU ON, SCTLR=0x30d0198d, TCR T0SZ=20 (44-bit VA)
edk2 MAIR:                    0xffbb4400 (Attr0=Device, Attr1=NC, Attr2=WT, Attr3=WB)
Phase 1 MMU strategy:         TTBR0-only swap; reuse edk2 MAIR/TCR (changing while MMU on = UNPREDICTABLE)
Phase 1 identity map:         3×1GB blocks (device@0, RAM@0x40M, RAM@0x80M) via L0→L1
TLBI Phase 1 (init_mmu):      tlbi vmalle1 + dsb nsh (non-broadcast; broadcast hangs with parked cores under NC memory)
TLBI Phase 2+ (kmap/tlb):     tlbi vmalle1is + dsb ish (broadcast; safe after WB upgrade enables global exclusive monitor)
Buddy allocator:              Orders 0-10 (4KiB-4MiB), bitmap coalescing, poison fill on free
Page pools (QEMU 2G):         kernel=128MB, user=1792MB, model=0, dma=64MB, reserved=64MB
Free pages (QEMU 2G):         ~508K / ~522K (bitmap + exclusions consume ~14K)
Slab allocator:               5 size classes (64, 128, 256, 512, 4096B), backed by frame allocator (kernel pool)
Vector table alignment:       section ALIGN(2048) in linker.ld + .balign 128 per entry in asm
Boot stub vectors section:    .text.vectors (boot.S, early boot safety net)
Rust vectors section:         .text.rvectors (exceptions.rs, installed from kernel_main)
llvm-tools component name:    llvm-tools (not llvm-tools-preview)
QEMU serial flag:             -nographic (implies -serial mon:stdio; no explicit -serial)
QEMU GDB flag:                -gdb tcp::1234 (not -s)
edk2 firmware path (macOS):   /opt/homebrew/share/qemu/edk2-aarch64-code.fd
ESP disk image:               aios.img (64 MiB FAT32, created by `just disk`)
UEFI stub ESP path:           /EFI/BOOT/BOOTAA64.EFI and /EFI/AIOS/BOOTAA64.EFI
Kernel ELF ESP path:          /EFI/AIOS/aios.elf
ACPI RSDP GUID:               8868e871-e4f1-11d3-bc22-0080c73c8881
DTB Table GUID:               b1b621d5-f19c-41a5-830b-d9152c69aae0
uefi crate version:           0.36 (features: alloc, global_allocator, panic_handler, logger)
SMP secondary entry:          _secondary_entry in boot.S (FPU → VBAR → TTBR1 install → MMU enable → stack → secondary_main)
Secondary MMU enable:         MAIR/TCR/TTBR0/TTBR1 write (safe: MMU off) → ISB → DSB SY → SCTLR write → ISB
GICv3 redistributor spacing:  128 KiB (0x20000) per core
NC memory atomic limitation:  Exclusive load/store pairs (ldaxr/stlxr) require global exclusive monitor
                              → needs Inner Shareable + Cacheable. spin::Mutex HANGS on NC memory.
                              Use only load(Acquire)/store(Release) for inter-core sync in Phase 1.
                              Phase 2 M8 upgrades TTBR0 RAM blocks to WB (Attr3) — spinlocks safe after TTBR1 active.
GOP framebuffer on QEMU:      800x600 Bgr8, stride=3200B, at ~0xBC7A0000 (NC Normal via L1[1])
Virtual kernel VMA:           0xFFFF_0000_0008_0000 (first section VMA = KERNEL_BASE + 0x80000; linker.ld Phase 2 M8+)
Kernel LMA:                   0x4008_0000 (unchanged physical load address; AT clause in linker.ld)
VIRT_PHYS_OFFSET:             0xFFFE_FFFF_C000_0000 (= KERNEL_VIRT - KERNEL_PHYS; add to phys to get virt)
DIRECT_MAP_BASE:              0xFFFF_0001_0000_0000 (all RAM mapped RW+XN, 2MB blocks)
MMIO_BASE:                    0xFFFF_0010_0000_0000 (UART/GIC/etc. mapped with Attr0 device memory)
Boot TTBR1 (boot.S):          3 static pages in BSS (L0/L1/L2); 4×2MB block descriptors covering kernel image
                              → minimal map sufficient to jump to virtual kernel_main
Full TTBR1 (kmap.rs):         Built in kernel_main after pool init; text=RX (38 pages), rodata=RO (42 pages),
                              data=RW (13 pages); direct map + MMIO; replaces boot TTBR1 via TLBI VMALLE1IS
TTBR1 T1SZ:                   16 (48-bit kernel VA); set in boot.S before TTBR1_EL1 write
KASLR slide (M8):             Computed (entropy from CNTPCT_EL0 or BootInfo.rng_seed); logged but NOT applied
                              to TTBR1 (init_kernel_address_space ignores slide; non-zero slide in later milestone)
ASID width:                   16-bit; AsidAllocator tracks generation; full TLBI VMALLE1IS on generation wrap
Slab cache sizes (M9):        5 classes: 64, 128, 256, 512, 4096 bytes; smaller rounds up to 64
Slab magazine size:            32 objects per MagazineRound; current/prev swap for two-chance fast path
Slab red zones:                8 bytes before/after each object (except 4096-byte cache); pattern 0xFEFE_FEFE_FEFE_FEFE
User VA layout (memory/virtual.md §3.1): TEXT=0x400000, DATA=0x1000000, HEAP=0x10000000, STACK_TOP=0x7FFF_FFFF_F000
TTBR0 format:                  bits[63:48]=ASID, bits[47:0]=PGD physical address
TTBR0 switch barriers:         DSB SY → MSR TTBR0_EL1 → TLBI VMALLE1IS → DSB ISH → ISB
Boot CPU SP:                   Converted from physical to virtual in boot.S (add VIRT_PHYS_OFFSET before br to kernel_main)
Secondary TTBR1 install:      _secondary_entry reuses boot CPU's L0/L1/L2 tables; TTBR1_EL1 set before MMU enable
PSCI entry phys conversion:   smp.rs converts virtual _secondary_entry symbol to physical before PSCI CPU_ON call
ramfb device:                 -device ramfb in QEMU; provides GOP without a full GPU driver
Timer tick frequency:         1 kHz (CNTFRQ_EL0 / 1000 counts per tick)
TICK_COUNT:                   Global AtomicU64 incremented every 1ms by timer_tick_handler
NEED_RESCHED:                 Global AtomicBool set by timer tick, checked by scheduler (M11)
Syscall ABI:                  SVC #0 from EL0; x8=syscall number, x0-x5=args, x0=return
Syscall count:                31 (IpcCall=0 through DebugPrint=30)
TrapFrame size:               272 bytes (31 GP regs + SP_EL0 + ELR_EL1 + SPSR_EL1)
ThreadContext size:           296 bytes (31 GP regs + SP + PC + PSTATE + TTBR0 + timer_cval + timer_ctl)
FpContext size:               528 bytes (32x128-bit vregs + FPCR + FPSR, 16-byte aligned)
LogEntry size:                64 bytes (one per cache line, 48-byte message field)
LogRing size:                 256 entries per core (16 KiB)
TraceRecord size:             32 bytes
TraceRing size:               4096 entries per core (128 KiB)
Timer PPI INTID:              30 (EL1 physical timer on QEMU)
MAX_THREADS:                  64 system-wide
MAX_PROCESSES:                32 system-wide
EarlyBootPhase count:         18 variants (EntryPoint=0 through Complete=17)
Scheduler classes:            RT (4ms), Interactive (10ms), Normal (50ms), Idle (50ms) — FIFO per class
Per-CPU run queues:           RUN_QUEUES: [Mutex<RunQueue>; MAX_CORES], lock order = ascending CPU ID
Idle threads:                 One per CPU (class=Idle), created in sched::init(), ensures pick_next() never returns None
IN_SCHEDULER guard:           Per-CPU AtomicBool prevents re-entrant schedule() from timer tick
IPC channel table:            CHANNEL_TABLE: Mutex<[Option<Channel>; 128]>, each channel has 16-slot MessageRing
MAX_MESSAGE_SIZE:             256 bytes (inline payload in RawMessage)
RING_CAPACITY:                16 messages per channel ring buffer
MAX_CHANNELS:                 128 system-wide
DEFAULT_TIMEOUT_TICKS:        5000 (5 seconds at 1 kHz)
IPC direct switch:            Bypasses scheduler when receiver already waiting; saves/restores via save_context/restore_context
Priority inheritance:         Transitive, bounded to MAX_INHERITANCE_DEPTH=8; stored in SchedEntity inherited_* fields
Capability table:             [Option<CapabilityToken>; 256] per process, O(1) handle lookup
Capability enforcement:       channel_create→ChannelCreate, ipc_call/send/recv→ChannelAccess, ipc_reply→NONE (spec §9.1)
Cascade revocation:           revoke token → mark children revoked → walk CHANNEL_TABLE → destroy channels with matching creation_cap
Lock ordering (full M13):     PROCESS_TABLE > SHARED_REGION_TABLE > NOTIFICATION_TABLE > CHANNEL_TABLE > SELECT_WAITERS > BLOCK_ENGINE > VIRTIO_BLK
Kernel IPC invocation:        Phase 3 threads are EL1; IPC via direct function call, NOT SVC. SVC path wired in parallel for future EL0.
Shared memory:                MAX_SHARED_REGIONS=64, MAX_SHARED_MAPPINGS=8 per region, W^X enforced on flags
Notifications:                MAX_NOTIFICATIONS=64, MAX_WAITERS_PER_NOTIFICATION=8, atomic OR into word + mask wake
IpcSelect:                    Multi-wait on channels + notifications, MAX_SELECT_ENTRIES=8, blocking with timeout
Service manager:              MAX_SERVICES=16, echo service for testing, service_register/lookup/on_death
Process lifecycle:            process_create_kernel, process_exit (cleanup: channels, shmem, notifications, caps), process_wait
Audit ring:                   256-entry ring buffer, timestamp + pid + event[48]
Load balancer:                try_load_balance every 4 ticks, migrate Normal threads from overloaded to underloaded CPU
Bench (Gate 1):               IPC round-trip, context switch, direct switch, capability overhead, shared memory throughput
RawMessage size:              272 bytes (ThreadId(4B) + padding(4B) + data(256B) + len(8B)), compile-time asserted
Shared crate unit tests:      309 tests (boot, cap, collections, ipc, kaslr, memory, observability, sched, storage, syscall)
VirtIO MMIO scan range:       0x0A00_0000–0x0A00_3E00, 512-byte stride (QEMU virt)
VirtIO MMIO magic:            0x74726976 ("virt")
VirtIO-blk device ID:         2
VirtIO-blk transport:         MMIO legacy (spec §4.2), polled I/O (no IRQ), single virtqueue
Data disk image:              data.img (256 MiB raw), created by `just create-data-disk`
QEMU data disk flag:          -drive file=data.img,if=none,format=raw,id=disk0 -device virtio-blk-device,drive=disk0
Superblock magic:             0x41494F53_50414345 ("AIOSPACE")
Superblock location:          sectors 0–7 (4 KiB)
WAL location:                 sectors 8–131079 (64 MiB)
WAL entry size:               64 bytes (repr(C)), 8 entries per 512-byte sector
Data region start:            sector 131080
MemTable capacity:            65536 entries, sorted Vec with binary search, dedup via refcount
ContentHash algorithm:        SHA-256 (sha2 crate, no_std)
Block integrity:              CRC-32C on data, verified on read
On-disk data format:          [crc32c:u32 | data_len:u32 | data | padding to sector boundary]
Encrypted on-disk format:     [nonce(12B) | encrypted{crc32c|data_len|data|pad} | tag(16B)]
ENCRYPTION_OVERHEAD:          28 bytes (12 nonce + 16 tag)
AES-256-GCM nonce format:     [random_prefix(4B) | counter(8B)], counter persisted in superblock
Nonce crash recovery:         nonce_counter advanced +1000 on init to prevent reuse after unclean shutdown
Device key derivation:        SHA-256(passphrase + "aios-device-key-salt") → 32-byte AES key (Phase 4 placeholder)
CompactObject size:           512 bytes (repr(C)), ObjectId + SpaceId + name[64] + hashes + timestamps + text_content[128]
Version size:                 256 bytes (repr(C)), hash + parent + content_hash + object_id + timestamp + author[32] + message[64]
ObjectIndex:                  Sorted Vec with binary search on ObjectId, max 16384 entries
version_head storage:         Stores SHA-256(serialized_version_bytes) from write_block(), NOT compute_version_hash()
MAX_SPACES:                   16 system-wide
System spaces:                system/ (Core), user/home/ (Personal), ephemeral/ (Ephemeral) — created at boot
Space-storage service:        Registered via service_register(b"space-storage", pid=0, ch=3)
Slab direct-map fix:          convert_to_direct_map() patches physical→virtual addresses after TTBR1 enabled
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

Current (post-Phase 4 M14 — Object Store, Version Store & Encryption):

```
aios/
├── CLAUDE.md
├── README.md
├── CONTRIBUTING.md
├── .gitignore
├── Cargo.toml            workspace root (resolver = "2", members: kernel, shared, uefi-stub)
├── Cargo.lock            committed for reproducibility
├── rust-toolchain.toml   pinned nightly + aarch64-unknown-none + aarch64-unknown-uefi
├── justfile              build, build-stub, disk, run (edk2), run-display, run-direct, check, test, clean
├── LICENSE               BSD-2-Clause
├── .cargo/
│   └── config.toml       relocation-model=static for aarch64-unknown-none
├── .claude/
│   ├── settings.json
│   ├── agents/           team-lead, kernel-dev, doc-writer, code-reviewer, verifier, doc-auditor
│   └── skills/           build-team, generate-phase-doc, implement-phase, review-pr-comments, verify-phase, write-arch-doc
├── .github/
│   └── workflows/ci.yml  check + build-release + test
├── kernel/
│   ├── Cargo.toml        deps: shared, fdt-parser, spin, sha2, aes-gcm; features: kernel-metrics (default), kernel-tracing, storage-tests (default)
│   ├── build.rs          emits linker script path
│   └── src/
│       ├── main.rs       kernel_main: full boot sequence, extern crate alloc, klog! structured logging, timer tick + IRQ unmask
│       ├── boot_phase.rs EarlyBootPhase enum (18 phases incl. LogRingsReady), advance_boot_phase(), boot timing
│       ├── dtb.rs        DeviceTree wrapper (fdt-parser), DTB parse + QEMU defaults + MPIDR extraction
│       ├── smp.rs        SMP bringup: PSCI CPU_ON, per-core stacks, Scheduler stub, secondary_main, per-core timer init + IRQ unmask
│       ├── framebuffer.rs GOP framebuffer driver: fill_rect, render_test_pattern (#5B8CFF)
│       ├── observability/
│       │   ├── mod.rs    LogLevel, Subsystem, LogEntry (64B), LogRing (256/core), klog!/kinfo!/kwarn!/kerror! macros, drain_logs()
│       │   ├── metrics.rs Counter (per-core sharded), Gauge, Histogram<N>, KernelMetrics registry; feature-gated kernel-metrics
│       │   └── trace.rs  TraceEvent enum, TraceRecord (32B), TraceRing (4096/core), trace_point! macro; feature-gated kernel-tracing
│       ├── sched/
│       │   ├── mod.rs       RunQueue, globals, thread allocation helpers, re-exports
│       │   ├── scheduler.rs schedule(), enter_scheduler(), timer_tick(), block/unblock, check_preemption
│       │   └── init.rs      Scheduler init, idle/test thread entries, load balancer
│       ├── ipc/
│       │   ├── mod.rs    Channel, CHANNEL_TABLE, MessageRing, channel_create/destroy, re-exports
│       │   ├── channel.rs ipc_call, ipc_recv, ipc_reply, ipc_send, ipc_cancel
│       │   ├── timeout.rs Timeout queue, sleep helpers, wakeup error delivery
│       │   ├── tests.rs   IPC test initialization, thread entries, test-only helpers
│       │   ├── direct.rs  IPC direct switch (bypass scheduler), priority inheritance, reply switch
│       │   ├── notify.rs  Notification objects: create/signal/wait, atomic OR + mask wake, timeout support
│       │   ├── select.rs  IPC select: multi-wait on channels + notifications, blocking with timeout
│       │   └── shmem.rs   Shared memory: create/map/share/unmap, W^X enforcement, process cleanup
│       ├── service/
│       │   └── mod.rs    Service manager: registry, echo service, process lifecycle, audit ring
│       ├── cap/
│       │   └── mod.rs    CapabilityToken, CapabilityTable (256/process), check/grant/revoke/attenuate/list, cascade revocation
│       ├── task/
│       │   ├── mod.rs    Thread, ThreadId, ThreadContext (296B), FpContext (528B), SchedEntity, ThreadState, SchedulerClass, CpuSet, THREAD_TABLE
│       │   └── process.rs ProcessControl, ProcessId, KernelResourceLimits (trust-level defaults), PROCESS_TABLE
│       ├── bench.rs      Gate 1 benchmarks: IPC round-trip, context switch, direct switch, cap overhead, shmem throughput
│       ├── drivers/
│       │   ├── mod.rs    Driver module re-exports
│       │   └── virtio_blk.rs VirtIO-blk MMIO transport driver: probe, init, read_sector/write_sector, polled I/O
│       ├── storage/
│       │   ├── mod.rs    Storage subsystem re-exports, BlockEngine init, self-tests (block, object, version, encryption, space)
│       │   ├── block_engine.rs BlockEngine: superblock, format/init, write_block/read_block, CRC-32C, SHA-256, encryption integration, ObjectIndex, SpaceTable
│       │   ├── wal.rs    Write-ahead log: 64-byte WalEntry (repr(C)), circular buffer, append/replay/trim
│       │   ├── lsm.rs    MemTable: sorted Vec with binary search, capacity 65536, insert/get/remove with refcount
│       │   ├── object_store.rs ObjectIndex (sorted Vec + binary search on ObjectId), object_create/read/delete, generate_object_id
│       │   ├── version_store.rs Version Store: Merkle DAG, version_create/list/rollback, object_update
│       │   ├── crypto.rs  DeviceKeyManager: AES-256-GCM encrypt/decrypt, nonce counter, crash recovery
│       │   └── space.rs   SpaceTable, space_create/list/get/delete, init_system_spaces, register_service
│       ├── syscall/
│       │   └── mod.rs    Syscall enum (31 syscalls), IpcError, syscall_dispatch(): IPC(0-9), Notify(10-12), Stats(13), Cap(14-17), Mem(18-22), Proc(23-25), Time(26-28), Audit(29), Debug(30)
│       ├── platform/
│       │   ├── mod.rs    Platform trait, detect_platform()
│       │   └── qemu.rs   QemuPlatform: init_uart, init_interrupts, init_timer
│       ├── mm/
│       │   ├── mod.rs    Switchable GlobalAlloc (bump → slab), enable_slab_allocator()
│       │   ├── bump.rs   128 KiB static bump allocator for early boot
│       │   ├── buddy.rs  Buddy allocator: bitmap coalescing, poison fill, orders 0-10
│       │   ├── slab.rs   Slab allocator (5 size classes: 64-4096B), magazine layer, red zones
│       │   ├── pools.rs  PagePools: 4 buddy instances (kernel/user/model/dma)
│       │   ├── frame.rs  FrameAllocator: pool-aware alloc/free, pressure, global static
│       │   ├── init.rs   init_memory(): UEFI map walk, pool config, bootstrap
│       │   ├── pgtable.rs 4-level page tables (PGD/PUD/PMD/PTE), PageTableEntry bit fields, AddressSpace, W^X API
│       │   ├── kmap.rs   init_kernel_address_space(): full TTBR1 build (text=RX, rodata=RO, data=RW, direct map, MMIO)
│       │   ├── kaslr.rs  KaslrConfig, compute_slide(): 2MB-aligned slide 0..128MB, CNTPCT_EL0/rng_seed entropy
│       │   ├── asid.rs   AsidAllocator: 16-bit ASID alloc with generation tracking, full TLB flush on wrap
│       │   ├── tlb.rs    TLB invalidation wrappers: tlb_invalidate_page (TLBI VAE1IS), tlb_invalidate_asid (TLBI ASIDE1IS), tlbi_all (TLBI VMALLE1IS)
│       │   ├── heap.rs   Typed kernel heap API: kalloc<T>/kfree<T>, kalloc_layout/kfree_layout
│       │   └── uspace.rs Per-agent user address spaces: UserAddressSpace, create/map/switch via TTBR1 direct map
│       └── arch/aarch64/
│           ├── mod.rs    pub mod uart, exceptions, gic, timer, mmu, psci, trap
│           ├── boot.S    _start + _secondary_entry (FPU, VBAR, minimal TTBR1 build, TCR T1SZ=16, MMU enable, stack, branch to virtual kernel_main)
│           ├── uart.rs   PL011 driver with full init (IBRD/FBRD/LCR_H/CR)
│           ├── exceptions.rs  Rust exception vector table, IRQ/SVC entry stubs (TrapFrame save/restore + eret), CPU register helpers
│           ├── gic.rs    GICv3 driver: distributor, redistributor, CPU interface + init_gicv3_secondary + irq_handler_el1
│           ├── psci.rs   PSCI CPU_ON via HVC/SMC (SMCCC ABI); entry point converted virt→phys in smp.rs
│           ├── timer.rs  ARM Generic Timer: frequency, tick, PPI wiring, timer_tick_handler, TICK_COUNT, NEED_RESCHED
│           ├── trap.rs   TrapFrame (272B), lower_el_sync_handler: SVC dispatch, data/instruction abort logging
│           ├── context_switch.S save_context/restore_context: callee-saved regs (x19-x30), SP, LR for kernel-to-kernel switch
│           ├── mmu.rs    TTBR0 identity map (3×1GB blocks, upgraded to WB Attr3 post-M8), edk2-compatible, MMU state export
│           └── linker.ld VMA=0xFFFF_0000_0008_0000 / LMA=0x4008_0000 (AT clause); __kernel_virt_base, __kernel_phys_base, __virt_phys_offset symbols
├── uefi-stub/
│   ├── Cargo.toml        deps: shared, uefi 0.36, log
│   └── src/
│       ├── main.rs       UEFI entry, BootInfo assembly (incl. framebuffer), ExitBootServices, kernel jump
│       └── elf.rs        Minimal ELF64 loader (PT_LOAD segments); converts virtual e_entry to physical for virtually-linked kernel
├── shared/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs        PhysAddr, VirtAddr, BOOTINFO_MAGIC, re-exports from submodules
│       ├── boot.rs       BootInfo, EarlyBootPhase, MemoryDescriptor, MemoryType, PixelFormat
│       ├── cap.rs        Capability enum, CapabilityHandle, CapabilityTokenId, MAX_CAPS_PER_PROCESS
│       ├── collections.rs FixedQueue<T,N>, RingBuffer<T,N> with unit tests
│       ├── ipc.rs        ChannelId, SharedMemoryId, NotificationId, RawMessage, ServiceName, SelectKind, IPC/shmem/notify constants
│       ├── kaslr.rs      KaslrConfig, compute_slide_from_entropy()
│       ├── memory.rs     Pool, PoolConfig, MemoryPressure, buddy_of(), BenchStats, ticks_to_ns()
│       ├── observability.rs LogLevel, Subsystem enums for shared use
│       ├── sched.rs      SchedulerClass, ThreadState, SchedConfig shared types
│       ├── storage.rs    ContentHash, BlockId, ObjectId, SpaceId, Timestamp, ContentType, SecurityZone, StorageError, StorageTier, BlockLocation, CompactObject(512B), Version(256B), Space(128B), SpaceQuota, ProvenanceEntry, ProvenanceAction, EncryptionState, ObjectIndexEntry, compute_version_hash, VirtIO constants
│       └── syscall.rs    Syscall enum (31 variants), IpcError, SyscallResult
└── docs/                 (architecture, phase, and research docs)
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
| `/review-pr-comments` | After PR creation | Wait for reviewer comments, fix, reply, resolve |
| `/write-arch-doc <topic-or-path>` | Architecture doc request | Interactive create/update architecture docs with research |

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
