---
author: jl
date: 2026-03-23
tags: [kernel, memory, security, kits]
status: final
---

# ADR: Kit Trait Return Types — Owned vs Borrowed

## Context

Phase 5 M16 introduced Kit traits (`FrameAllocator`, `CapabilityEnforcer`, etc.) in `shared/src/kits/`. The phase doc specified `list_active` returning `&[Option<CapabilityToken>]`, but the kernel implementation wraps mutex-guarded global statics (`PROCESS_TABLE`, `FRAME_ALLOC`).

## Decision

Use **owned return types** (`Vec<CapabilityToken>`) instead of borrowed slices for Kit trait methods that need to return data from mutex-guarded state. Returning `&[...]` from a locked mutex is unsound — the borrow would outlive the lock guard.

## Alternatives Considered

1. **`&[Option<CapabilityToken>]`** (phase doc spec) — unsound with mutex-guarded state
2. **Callback/visitor pattern** (`fn list_active(&self, f: impl FnMut(&CapabilityToken))`) — avoids allocation but complicates the trait interface
3. **Fixed-size output buffer** (`fn list_active(&self, out: &mut [Option<CapabilityToken>]) -> usize`) — no allocation, but awkward API

## Rationale

`Vec` is the simplest correct solution. The `shared` crate already uses `extern crate alloc`, so `Vec` is available. The allocation cost is negligible compared to the mutex lock contention. The trait remains dyn-compatible (object-safe) with `Vec` returns.

## Consequences

- All Kit traits that return collections from mutex-guarded state use `Vec<T>`
- `shared` crate's `alloc` dependency is load-bearing for Kit traits
- Future phases should follow this pattern for any Kit trait wrapping global statics
