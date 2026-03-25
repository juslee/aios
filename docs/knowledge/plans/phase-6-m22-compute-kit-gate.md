---
author: claude
date: 2026-03-25
tags: [gpu, compute, kits]
status: in-progress
phase: 6
milestone: M22
---

# Plan: Phase 6 M22 — Compute Kit Tier 1 & Gate

## Approach

Milestones 19–21 built the VirtIO-GPU 2D driver, Custom GPU Service (IPC-based, double-buffered), and font rendering/boot log display. M22 is the final milestone — it defines the Compute Kit Tier 1 `GpuSurface` trait, implements it in the kernel, ensures all quality gates pass, and updates documentation.

**Key finding: the code already exists.** During M20 (GPU Service), `GpuSurface` trait, `ComputeError`, `SurfaceBuffer`, `DamageRect`, and `SemanticHint` were implemented in `shared/src/kits/compute.rs`. `KernelGpuSurface` (zero-sized wrapper) was implemented in `kernel/src/gpu/mod.rs`. The phase doc checkboxes for Steps 16-17 are unchecked despite the code being present.

**What M22 actually needs to do:**
1. **Step 16** — Verify existing types, check off tasks (already done in M20)
2. **Step 17** — Verify `GpuSurface` trait, check off tasks (already done in M20)
3. **Step 18** — Verify `KernelGpuSurface` implementation, potentially add an end-to-end test demonstrating Kit API usage
4. **Step 19** — Review for shared crate completeness, add any missing tests, verify all 13 Kit traits are dyn-compatible
5. **Step 20** — Run all quality gates, update CLAUDE.md/README/developer guide, run audit loop, dead code cleanup

**Shared crate refactoring:** All Compute Kit types are already in `shared/`. No additional moves needed. Review `kernel/src/gpu/` for any types that leaked into kernel-only scope.

**Current test count:** 437 tests (shared crate, host-side). Expect to add 5-10 more tests for comprehensive Compute Kit coverage.

## Progress

- [ ] Step 16: Compute Kit module and error types
  - [ ] 16a: Verify `pub mod compute;` exists in `shared/src/kits/mod.rs` ✓ (already present)
  - [ ] 16b: Verify `pub use kits::compute as compute_kit;` exists in `shared/src/lib.rs` ✓ (already present)
  - [ ] 16c: Verify `ComputeError` has 5 variants with correct derives (Debug, Clone, Copy, PartialEq, Eq) ✓ (already present)
  - [ ] 16d: Verify `SurfaceBuffer`, `DamageRect`, `SemanticHint` structs/enums ✓ (already present)
  - [ ] 16e: Add missing tests if coverage gaps exist (all 5 ComputeError variants, struct field verification)
  - [ ] 16f: Verify: `just check` + `just test`

- [ ] Step 17: GpuSurface trait definition
  - [ ] 17a: Verify `GpuSurface` trait with 4 methods matches spec ✓ (already present)
  - [ ] 17b: Verify dyn-compatibility test exists ✓ (already present at line 156)
  - [ ] 17c: Verify trait signatures match docs/kits/kernel/compute.md §2
  - [ ] 17d: Note any deviations from spec (format type is `GpuPixelFormat` not `PixelFormat` — intentional, spec uses placeholder name)
  - [ ] 17e: Verify: `just check` + `just test`

- [ ] Step 18: Kernel GpuSurface implementation
  - [ ] 18a: Verify `KernelGpuSurface` zero-sized struct in `kernel/src/gpu/mod.rs` ✓ (already present)
  - [ ] 18b: Verify `impl GpuSurface for KernelGpuSurface` with all 4 methods ✓ (already present)
  - [ ] 18c: Verify GPU Service `gpu_service_loop` tests KernelGpuSurface Kit API
  - [ ] 18d: Verify QEMU display shows content rendered via Kit API (`just run-gpu`)
  - [ ] 18e: Verify: `just check` + `just run-gpu`

- [ ] Step 19: Shared crate refactoring
  - [ ] 19a: Review `kernel/src/gpu/` for types that should be in shared — expect none (all are already there)
  - [ ] 19b: Add comprehensive host-side tests for Compute Kit: all error variant equality, SurfaceBuffer field access, DamageRect zero-area, SemanticHint all-variants-distinct
  - [ ] 19c: Verify all 13 Kit traits are dyn-compatible (3 Memory + 4 IPC + 1 Capability + 4 Storage + 1 Compute = 13)
  - [ ] 19d: Verify: `just check` + `just test` (expect test count > 437)

- [ ] Step 20: Quality gate and final documentation
  - [ ] 20a: `just check` — zero warnings, zero errors
  - [ ] 20b: `just test` — all pass, verify count increased
  - [ ] 20c: `just run` — boots normally without VirtIO-GPU (GOP fallback)
  - [ ] 20d: `just run-gpu` — VirtIO-GPU display with boot log text
  - [ ] 20e: Update CLAUDE.md: Workspace Layout (confirm all M22 files listed), Key Technical Facts (Compute Kit: GpuSurface trait 4 methods, ComputeError 5 variants, verify existing facts current)
  - [ ] 20f: Update README.md: Project Structure, Build Commands
  - [ ] 20g: Update docs/project/developer-guide.md: test counts, Compute Kit patterns
  - [ ] 20h: Update phase doc: check all Step 16-20 checkboxes, Status = Complete
  - [ ] 20i: Update Kit docs if trait signatures deviate from spec
  - [ ] 20j: Dead code cleanup: review `#[allow(dead_code)]` in `kernel/src/gpu/service.rs` lines 33,40 (fence_tracker, double_buffering fields)
  - [ ] 20k: Run full audit loop until 0 issues
  - [ ] 20l: Update `docs/project/development-plan.md`: mark Phase 6 complete

## Code Structure Decisions

- **No new code files needed**: All Compute Kit types and kernel implementation already exist from M20. M22 is a verification, testing, and documentation milestone.
- **Format type name**: Spec uses `PixelFormat` but implementation uses `GpuPixelFormat`. This is intentional — `PixelFormat` exists in `shared/src/boot.rs` for the boot framebuffer; `GpuPixelFormat` in `shared/src/gpu.rs` is the GPU-specific type. Note deviation in Kit docs if not already noted.
- **Dead code**: `fence_tracker` and `double_buffering` fields in `GpuServiceState` have `#[allow(dead_code)]`. These are Phase 7+ infrastructure — keep the fields, keep the attribute, document why.
- **Test strategy**: Focus on expanding Compute Kit test coverage with edge cases (zero-size buffers, all error variants, all semantic hints, dyn-compat for all 13 Kit traits in one test).

## Dependencies & Risks

- **Depends on**: M19-M21 complete (they are — all code exists and works)
- **Risk**: The kernel crate doesn't compile for host tests (it's `aarch64-unknown-none`). All Kit tests are in the shared crate. No risk here — shared crate tests already pass.
- **Risk**: `just run-gpu` requires QEMU with `-device virtio-gpu-device` and a display. CI may not have a display. The `just run` (no GPU) path is the CI-safe test.

## Phase Doc Reconciliation

The phase doc has Steps 16-17 unchecked despite the code being implemented in M20. During execution:
- Check off all Step 16-17 tasks that are already done
- Step 18 acceptance says "QEMU display shows content rendered via KernelGpuSurface" — verify this works
- Step 20 specifies "test count > 394" — current count is 437, will increase further

## Issues Encountered

(to be filled during implementation)

## Decisions Made

(to be filled during implementation)

## Lessons Learned

(to be filled during implementation)
