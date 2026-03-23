# Code Conventions

## Rust

- `#![no_std]` everywhere in `kernel/` and `shared/`
- `#![no_main]` in `kernel/` and `uefi-stub/`
- All `unsafe` blocks require a `// SAFETY:` comment explaining: invariant, who maintains it, what happens if violated
- No TODO comments in code — complete implementations only
- Naming: `snake_case` for functions/variables, `CamelCase` for types, `SCREAMING_SNAKE` for constants
- Error handling: `Result<T, E>` for fallible operations; panics reserved for unrecoverable invariant violations
- Panic handler: always prints to UART then halts with `wfe` loop (not `loop {}`)
- Prefer the best approach over the simplest — choose the design that is cleanest, most maintainable, and architecturally sound, even if a shortcut exists

## Architecture-Specific (aarch64)

- FPU must be enabled before any Rust code runs (`boot.S` is responsible)
- BSS must be zeroed before `kernel_main` is called (`boot.S` is responsible)
- `VBAR_EL1` must be set before interrupts are unmasked
- All MMIO access via `core::ptr::read_volatile` / `core::ptr::write_volatile`
- Memory-mapped registers: define as `const` physical addresses; map to virtual after Phase 1 MMU
- W^X: no page is both writable and executable
- Stack alignment: 16-byte (ABI requirement)
- Secondary cores: park with `wfe` (not `wfi`) — `sev` wakes all simultaneously
- NC memory limitation: `spin::Mutex` and atomic RMW (`fetch_add`, `compare_exchange`) HANG on Non-Cacheable Normal memory. Use only `load(Acquire)` / `store(Release)` for inter-core synchronization until Phase 2 enables WB cacheable attributes.

## Assembly

- Files use `.S` extension (uppercase — Rust build system handles preprocessing)
- Entry symbols: `#[no_mangle]` on the Rust side
- Vector table: `.align 7` (128 bytes) per entry in assembly; `ALIGN(2048)` for section in linker script
- All 16 exception vector entries present; stubs `b .` until real handlers added
- Boot order (strict): FPU enable → VBAR install → park secondaries → set SP → zero BSS → build minimal TTBR1 → configure TCR T1SZ → install TTBR1 → convert SP to virtual → branch to virtual `kernel_main`
- Exception handler: uses direct `putc()` output, not `println!()`, to prevent recursive faults

## Crate & Dependency Rules

- All kernel crates: `no_std`, `no_main`
- All dependencies: must be `no_std` compatible
- License: MIT or Apache-2.0 preferred (BSD-2-Clause compatible). **No GPL in kernel/ or shared/**
- `Cargo.lock`: committed (binary crate, reproducible builds)
