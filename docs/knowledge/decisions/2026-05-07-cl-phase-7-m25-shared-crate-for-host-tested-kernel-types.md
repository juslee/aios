---
author: claude
date: 2026-05-07
tags: [testing, kernel, shared-crate, no-std]
status: final
---

# Pure-data kernel types belong in `shared/`, not `kernel/`

## Decision

When a kernel module needs a pure-data type (no MMIO, no kernel
state, no architecture-specific behaviour), declare the type in
`shared/src/` and have the kernel module import it — even when the
only consumer is the kernel.

## Why

`just test` excludes the `kernel` crate (it's a `no_std` `no_main`
binary targeting `aarch64-unknown-none`; it can't link against
`std::test`). Tests in `kernel/src/**/cfg(test) mod tests` compile
under `cargo test` but never execute. They're documentation, not
verification.

Tests in `shared/src/**/cfg(test) mod tests` run on every push —
the workspace's `cargo test --exclude kernel --exclude uefi-stub`
hits them on the host target. So:

- Pure data → `shared/`, write tests, they actually run.
- State + locks + hardware → `kernel/`, integration-test via QEMU.

## How it played out in M25

Phase 7 M25 needed:

- `ZOrder` — fixed-array list of `SurfaceId`s with push/remove/raise.
- `FocusHistory` — bounded MRU container of `SurfaceId`s.
- `RouteTarget` enum + `route_event()` — input-routing decision.
- `WindowDecoration`, `HitZone`, `ResizeEdge` — decoration metrics
  and zone enum.
- `hit_zone()` — geometric hit-test pure function.
- `clamp_window_size()` — minimum-size clamp helper.

I started by writing some of these in `kernel/` and adding
`#[cfg(test)] mod tests` blocks, then moved them all to
`shared/src/compositor.rs` for Step 23. The kernel modules are now
thin wrappers — `WINDOW_Z_ORDER` is a `Mutex<ZOrder>`, `FocusManager`
holds a `FocusHistory`, etc. The 25 new shared-crate tests all run
host-side.

## Counter-example: when to keep types in `kernel/`

- `Surface` (in `kernel/src/compositor/surface.rs`) holds a
  `ChannelId` and `ProcessId`, both of which are kernel concepts.
  Tests for surface lifecycle live in `kernel/` and don't run
  host-side; integration testing in M26 covers them.
- `CompositorState` (service struct) holds `GpuBufferHandle`s
  pointing into DMA pages. Hardware-bound — stays in `kernel/`.

## Heuristic

If the type doesn't touch any of:
- `unsafe` for hardware/pointers
- `spin::Mutex` or atomics for cross-core sync
- Architecture-specific intrinsics
- Kernel-only crates (e.g. `crate::cap`, `crate::ipc`)

…then it belongs in `shared/` and should have host-side tests.

## Gotcha

`shared/` does need to depend on `alloc` for some test helpers
(`alloc::vec![...]`), but the production code itself stays
`no_std + no_alloc`. The `#[cfg(test)]` blocks pull in `alloc`
just for assertion construction.
