---
author: jl + claude
date: 2026-10-06
tags: [kernel, sched, tooling]
status: final
---

# Lesson: Code on the IRQ path must not use NEON, and the compiler uses it where you do not expect

## What happened

The kernel's IRQ entry saves only general-purpose registers. The `irq_el1_entry` assembly in `kernel/src/arch/aarch64/exceptions.rs` stores the caller-saved `x0` to `x18`, `x29` and `x30`, and there is no `q` or `v` register save at entry (nor at a thread switch, per the ADR). The crash-fix ADR (`docs/knowledge/decisions/2026-09-22-jl-crash-fix-preemption-and-fp.md`, H5 and Decision 2) plans an eager full FP/SIMD save in a later step; until then, any V-register instruction reachable from `irq_el1_entry` can corrupt a thread's vector state.

Reviewing the shared types added for crash-fix step 1b found three ways V registers or panics reached IRQ-path code that looked scalar:

- `u64::count_ones()` compiles to `fmov d0, x0` / `cnt v0.8b, v0.8b` / `addv b0, v0.8b` / `fmov w0, s0` on `aarch64-unknown-none`, because the target has no FEAT_CSSC.
- A hand-written popcount loop (`x &= x - 1; n += 1`) is no escape: LLVM's loop idiom pass turns it back into the same `cnt`.
- `iter().enumerate()` pulls in a call to `panic_const_add_overflow`, because `Enumerate::next` is `#[rustc_inherit_overflow_checks]`. A clippy `arithmetic_side_effects` deny does not catch it.

## Why it happened

The IRQ-path rule is easy to state and hard to see in source. The usual V-register sources are stack arrays, zero-initialisation or copies of aggregates of 16 bytes or more (`stp q`, `str q`, `memcpy`), and the prebuilt `core` and `compiler_builtins`. Integer intrinsics and iterator adapters are not on that list, and the gate that does look at the binary (the V-register listing in the ADR's step 1b) only runs once all the code is written.

## What we learned

Re-checked on 2026-10-10 with `nightly-2026-10-09`, `-C opt-level=1 -C relocation-model=static`, target `aarch64-unknown-none`:

- Both `x.count_ones()` and the `x &= x - 1` loop produce the `fmov`/`cnt`/`addv` sequence above.
- With `-C overflow-checks=on`, a `for (i, x) in a.iter().enumerate()` loop emits an `R_AARCH64_CALL26` to `panic_const_add_overflow`; the same loop written `for i in 0..n` with `a.get(i)` does not.
- At 368d99a the only `count_ones` in `shared/` or `kernel/` is `CpuSet::count` in `shared/src/sched.rs`; nothing in `sched/`, `arch/` or `task/` calls it. The dev profile is `opt-level = 1` (workspace `Cargo.toml`), so overflow checks stay on.
- A scalar alternative for a bit count is a nibble-table loop; check its disassembly too.

## How to avoid next time

**Pre-check shared code before the kernel links it.** Build a throwaway `no_std` staticlib outside the repository:

1. It depends on `shared` by path (`shared` does `extern crate alloc`, so give the crate a dummy `#[global_allocator]`), uses `[profile.dev] opt-level = 1` with `panic = "abort"`, and exposes each generic instance through a `#[no_mangle] extern "C"` wrapper.
2. Build with `RUSTFLAGS="-C relocation-model=static"` and the pinned nightly (`RUSTUP_TOOLCHAIN=<channel in rust-toolchain.toml>`), for `--target aarch64-unknown-none`.
3. Run the sysroot's `llvm-objdump -d -r -l -C` on the `.a` (macOS `ar x` fails on the GNU archive, so do not unpack it) and scan the target functions for V registers, `fmov`/`cnt`/`addv`, `memcpy`/`memset`, `blr`, and the `R_AARCH64_CALL26` targets.
4. Add `x.count_ones()` as a positive control: it must show `cnt v0.8b`. A scan that cannot find the control is broken.

**For kernel code, the built ELF is enough.** After `cargo build --target aarch64-unknown-none`, run `llvm-objdump -d --no-show-raw-insn -C target/aarch64-unknown-none/debug/kernel` and count, per symbol, V-register operands, `memcpy`/`memset`, `blr` and `panic_const|overflow`. The binary is `$(rustc --print sysroot)/lib/rustlib/aarch64-apple-darwin/bin/llvm-objdump`. The asdf `rust-objdump` and `cargo objdump` shims fail in a worktree ("No version is set for command cargo-objdump"), and the sysroot has no `llvm-addr2line` or `llvm-symbolizer`. To name an address, use `llvm-objdump -d -C --start-address=0x... --stop-address=0x...` with literal hex (zsh arithmetic truncates 16-digit hex: `number truncated after 15 digits`), `llvm-nm -C --defined-only` to find the symbol, or `lldb -b <elf> -o "image lookup -a <addr>"`, which prints the function and the source line.

**Scan traps found in review**, each of which turned a real hazard into a clean-looking zero:

- *Loose symbol match.* `9schedule` matched the mangled `9scheduler10timer_tick` first and reported 0 hazards for the wrong function. Match the full mangled suffix with `index()`, and print the matched header line and the line count beside the counts. An empty block also reads as "0 hazards". `schedule` itself is internalised as `...9scheduler8schedule.llvm.<hash>`, so a `8schedule>:$` anchor finds nothing. (In `llvm-nm -C` the names are `kernel::sched::scheduler::timer_tick`, with a `kernel::` prefix, and `schedule (.llvm.<hash>)`.)
- *Generic suffix.* With `-C`, a monomorphised generic's header ends in its type arguments (`<...::scan::<kernel::..., kernel::...>>:`), so a `name(::h[0-9a-f]+)?>:$` anchor silently matches nothing. Match `::scan::<.*>>:$` for those. Plain `#[inline(never)]` kernel functions anchor fine.
- *Hex addresses read as registers.* A bare `[vqdsh][0-9]+` matches the `d5544` inside `bl 0xffff0000000d5544` and the `d5` in `#0xd5`. Require operand context, for example `\t(.*(, |\t|\[))?[vqdsh][0-9]+(\.[0-9]*[bhsd]|,|\]|$)`, and print the matching lines with `-l` source annotations before believing a non-zero count.
- *Cold panic stubs* attribute to the function's closing line, not to the line that panics.

Existing overflow-panic call sites on the IRQ path are the baseline to compare against, not zero. A later step's parity claim is "no new V site, no new call", symbol by symbol.
