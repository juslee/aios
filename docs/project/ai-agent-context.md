# AI Agent Context Guide

**Purpose**: Context-loading checklist for AI coding agents before writing AIOS kernel code.

**When to load**: At agent spawn, before any implementation work. This guide prescribes the minimum reading and behavioral rules for producing correct kernel code.

**Not for humans**: For the human-readable developer guide, see [developer-guide.md](./developer-guide.md).

-----

## 1. Required Reading Order

Before writing code for any phase step, read these documents in order:

### Mandatory (every task)

1. **Phase doc** (`docs/phases/NN-phase-name.md`) -- Read the specific step you are implementing. Note the acceptance criteria -- this is your done condition.
2. **CLAUDE.md** -- Read these sections:
   - Code Conventions (Rust, Assembly, Architecture-Specific)
   - Unsafe Documentation Standard (three-line SAFETY format)
   - Key Technical Facts (addresses, offsets, constants)
   - File Placement (where to put new files)
3. **Developer guide S3** ([developer-guide.md S3](./developer-guide.md#3-aios-kernel-patterns)) -- The four unsafe patterns (MMIO, page tables, SPSC rings, system registers) and three error handling patterns.
4. **Developer guide S5** ([developer-guide.md S5](./developer-guide.md#5-common-pitfalls)) -- All seven pitfalls. These represent real bugs discovered during Phases 1-3.

### Task-specific

5. **Architecture docs** -- Read the documents listed in the phase doc's "Architecture References" table. These are the source of truth for register offsets, struct fields, and memory addresses.
6. **Existing code** -- Before creating new files, read the existing code in the same directory. Match its patterns, naming, and style exactly.

### What to skip

- Developer guide S2 (Rust Competency Model) -- written for humans, not agents
- Developer guide S6-7 (Build/Debug workflow) -- you execute commands, not read about them
- Developer guide S9 (Glossary) -- reference only if you encounter unfamiliar terms

-----

## 2. Pattern Quick-Reference

When implementing kernel code, use these established patterns:

| Task | Pattern | Reference File |
|---|---|---|
| Read/write hardware register | `mmio_read32()`/`mmio_write32()` volatile helpers | `arch/aarch64/uart.rs:102-110` |
| Read ARM system register | `asm!("mrs {}, REG", out(reg) val)` with SAFETY | `arch/aarch64/timer.rs:19-25` |
| Write ARM system register | `asm!("msr REG, {}",  in(reg) val)` + ISB if needed | `arch/aarch64/timer.rs:51-58` |
| New static shared across cores | `AtomicT` with appropriate ordering | `smp.rs:34` (PRINT_TURN) |
| Write-once boot-time static | `UnsafeCell` + `unsafe impl Sync` | `arch/aarch64/mmu.rs:32-39` |
| Per-core data structure | Array indexed by `current_core_id()` | `observability/mod.rs:101` (LOG_RINGS) |
| New module in kernel | `pub mod name;` in parent + file with `//!` doc comment | `arch/aarch64/mod.rs` |
| Shared type (kernel + stub) | Define in `shared/src/`, import in kernel with `pub use shared::` | `observability/mod.rs:16` |
| Error from syscall handler | Return `Err(IpcError::Variant as i64)` | `ipc/mod.rs` (channel_create) |
| Unrecoverable error | `kerror!(Subsys, "msg"); halt()` OR `panic!("msg")` | `main.rs:53-64` |
| Feature-gated code | `#[cfg(feature = "feature-name")] { ... }` | `observability/trace.rs:283-290` |
| Structured logging | `kinfo!(Subsys, "fmt {}", arg)` | `main.rs:37` |
| TLB invalidation (boot) | `tlbi vmalle1` + `dsb nsh` (non-broadcast) | `arch/aarch64/mmu.rs` |
| TLB invalidation (runtime) | `tlbi vmalle1is` + `dsb ish` (broadcast) | `mm/kmap.rs` |
| Compile-time size check | `const _: () = assert!(size_of::<T>() == N);` | `arch/aarch64/trap.rs` |

-----

## 3. Anti-Patterns

Things that agents commonly get wrong. Violating any of these will fail code review:

### Never invent hardware constants

```
WRONG: Guessing a register offset
const GICD_CTLR: usize = 0x000;  // "I think this is right"

RIGHT: Read from architecture doc
// GICv3 GICD_CTLR offset (hal.md S4.1)
const GICD_CTLR: usize = 0x000;
```

If you don't know a register offset, address, or constant -- read the architecture doc or `CLAUDE.md` Key Technical Facts. Never guess.

### Never use spin::Mutex in boot-time code

Phase 1-2 boot code runs on Non-Cacheable memory. `spin::Mutex` hangs. Use `AtomicBool` with `load(Acquire)`/`store(Release)` only. See developer-guide.md S5.1.

### Never leave TODO comments

AIOS convention: no TODO comments in code. If a feature is incomplete, either:
- Complete it fully in this step, OR
- Mark the function/module `#[allow(dead_code)]` if it will be wired in a later phase step

### Never create files in wrong directories

Follow CLAUDE.md File Placement rules exactly:
- aarch64-specific code -> `kernel/src/arch/aarch64/`
- Memory management -> `kernel/src/mm/`
- Platform abstraction -> `kernel/src/platform/`
- Shared types -> `shared/src/`
- Phase docs -> `docs/phases/` (flat, no subdirs)

### Never skip ISB after MSR writes

Always add `isb` after writing to: `VBAR_EL1`, `SCTLR_EL1`, `TCR_EL1`, `TTBR0_EL1`, `TTBR1_EL1`, `CPACR_EL1`. See developer-guide.md S5.4.

### Never omit the SAFETY comment

Every `unsafe` block requires the three-line format:

```rust
// SAFETY: <what invariant makes this safe>
// <who maintains the invariant>
// <what happens if violated>
unsafe { ... }
```

### Never map a page as RWX

W^X policy: pages are writable OR executable, never both. See developer-guide.md S5.5.

-----

## 4. Verification Checklist

Before marking any step complete, verify ALL of these:

### Build verification

- [ ] `cargo build --target aarch64-unknown-none` -- zero warnings
- [ ] `just check` passes (fmt + clippy + build)
- [ ] No new clippy warnings introduced

### Code quality

- [ ] Every `unsafe` block has a `// SAFETY:` comment (three-line format)
- [ ] Every new file has a `//!` module-level doc comment
- [ ] No TODO comments in code
- [ ] Naming follows conventions: `snake_case` functions, `CamelCase` types, `SCREAMING_SNAKE` constants
- [ ] New constants reference their source: `// (hal.md S4.3)` or similar

### Architecture compliance

- [ ] No W^X violations (no page mapped as RW+X)
- [ ] All MMIO access uses volatile read/write
- [ ] ISB after all MSR writes to instruction-affecting registers
- [ ] Correct TLB invalidation strategy (local-only during boot, broadcast after all cores online)
- [ ] Addresses and offsets match CLAUDE.md Key Technical Facts

### Phase acceptance

- [ ] Step's acceptance criteria met (run the exact command from the phase doc)
- [ ] If QEMU output expected, it matches the documented strings

-----

## 5. Commit Protocol

After completing a step:

1. Stage only the files you created or modified
2. Commit with format: `Phase N MK: Step X -- <description>`
3. Push immediately -- do not batch steps
4. Report completion to team-lead with summary of files created/modified
