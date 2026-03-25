---
author: claude
date: 2026-03-25
tags: [gpu, compute, kits, workflow]
status: final
---

# Lesson: Implementation can outpace phase doc checkboxes

## Context

Phase 6 M22 (Compute Kit Tier 1 & Gate) was planned as 5 steps: define types (Step 16), define trait (Step 17), implement kernel wrapper (Step 18), shared crate refactoring (Step 19), and quality gate (Step 20). However, all code for Steps 16-18 was already implemented during M20 (Custom GPU Service) because the GPU Service needed to verify the Kit API worked end-to-end.

## Lesson

When a milestone depends on types/traits that a previous milestone needs for verification, the natural implementation flow will pull those types forward. The phase doc checkboxes for the later milestone become stale — unchecked despite the code existing.

## Impact

M22 became a pure verification, testing, and documentation milestone. No new code files were created. The work was:
- Verify existing implementations match the spec
- Expand test coverage (437 → 442 tests, including cross-Kit dyn-compatibility for all 13 traits)
- Update documentation (CLAUDE.md, phase doc, development plan)
- Fix pre-existing markdown lint warnings

## Recommendation

When writing future phase docs, if a Kit trait is needed by an earlier milestone's acceptance criteria, consider moving the trait definition step into that earlier milestone. Alternatively, accept that gate milestones will be lightweight verification passes — this is fine and even desirable as a final quality checkpoint.
