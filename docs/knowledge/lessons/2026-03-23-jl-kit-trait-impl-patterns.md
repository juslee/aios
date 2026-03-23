---
author: jl
date: 2026-03-23
tags: [kernel, memory, security, kits]
status: final
---

# Lesson: Kit Trait Implementation Patterns

## Zero-Sized Unit Struct Wrappers

When implementing Kit traits on kernel subsystems backed by global statics (`FRAME_ALLOC`, `PROCESS_TABLE`), use zero-sized unit structs (e.g., `KernelFrameAllocator`, `KernelCapabilitySystem`) as the trait implementor. This avoids:
- Storing duplicate state
- Lifetime issues with global statics
- Name collisions (kernel already has `struct FrameAllocator` — the Kit trait is also named `FrameAllocator`)

Import Kit traits with aliases when names collide: `use shared::kits::memory as memory_kit;`.

## Lock Ordering Across Kit Trait Methods

The `CapabilityEnforcer::revoke()` method must drop the `PROCESS_TABLE` lock before touching `CHANNEL_TABLE` for cascade revocation. This respects the established lock ordering: `PROCESS_TABLE > CHANNEL_TABLE`. Kit trait methods that touch multiple global tables must be aware of lock ordering constraints.

## Distinguishing Revoked vs Missing in Capability Lookups

`CapabilityTable::get(handle)` filters out revoked tokens (returns `None` for revoked). To distinguish "handle doesn't exist" from "handle exists but is revoked", use `tokens()` to access the raw slot array and inspect the `revoked` field directly. This matters for returning the correct `CapabilityError` variant (`InvalidHandle` vs `Revoked`).

## MemoryPressureMonitor: Worst Across All Pools

`current_level()` must compute the worst (highest) pressure across all initialized pools, not just delegate to the user pool's pressure. Skip uninitialized pools (total == 0) to avoid false OOM reports for the Model pool on QEMU.
