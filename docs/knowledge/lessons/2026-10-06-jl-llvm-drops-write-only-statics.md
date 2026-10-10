---
author: jl + claude
date: 2026-10-06
tags: [kernel, tooling]
status: final
---

# Lesson: LLVM deletes stores to a static that nothing reads

## What happened

In crash-fix step 1b, task K3 (2026-09-28), a function stored to four new kernel statics (`CURRENT_TID`, `LAST_RUN`, `WAKE_PENDING`, `WAKE_AT`), but no code read them yet; the readers were a later task. The kernel ELF at `opt-level = 1` contained no store to any of the four. Only a discarded `ldr xzr` of an atomic counter load remained. Statics in the same function that were also read elsewhere (`SWITCH_GEN`, `IRQ_CTX`, `LAST_CPU`) kept their stores.

## Why it happened

LLVM's GlobalOpt treats an internal global that is never loaded as dead and removes its stores, atomic ones included. This is correct, but it makes a half-landed instrumentation step look as if its stores are missing.

Re-checked on 2026-10-10 with `nightly-2026-10-09`, `aarch64-unknown-none`, `-C opt-level=1 -C relocation-model=static`: a function that does `WRITE_ONLY.store(x, Relaxed)` and `READ_BACK.store(x, Relaxed)` compiles to a single `str` to `READ_BACK`; the store to `WRITE_ONLY` is gone. The dev profile is `opt-level = 1` (workspace `Cargo.toml`), so the kernel ELF behaves the same way.

## What we learned

- A disassembly with no store to a new static does not mean the code is wrong. First check that something reads the static.
- Per-symbol instruction and V-register counts grow when the reader lands in a later task, because the stores reappear.

## How to avoid next time

- When checking a disassembly for an instrumentation store, or comparing per-symbol counts across tasks, confirm that a load of the static exists in the build.
- Do not add a dummy read or `#[used]` to force the stores in. Land the reader, or record in the review that the stores are expected to be absent until it does.
- See `2026-10-06-jl-irq-path-codegen-hazards.md` for how to read the kernel ELF per symbol.
