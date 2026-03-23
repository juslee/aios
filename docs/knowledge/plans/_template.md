---
author: claude
date: YYYY-MM-DD
tags: []
status: in-progress
phase: N
milestone: MK
---

# Plan: Phase N MK — Description

## Approach

High-level strategy for this milestone. Summarize what prior milestones built and what this milestone adds on top. Call out any architectural patterns or design choices that shape the implementation.

**Key gaps found during exploration:** Identify missing APIs, missing types, or structural gaps in existing code that must be addressed before the main work can proceed.

**Shared crate refactoring (end of milestone):** List pure data structures from this milestone (and prior milestones if applicable) currently in `kernel/` that should move to `shared/` for host-side unit testing. Distinguish sharable (pure logic, no hardware deps) from non-sharable (MMIO, interrupts, hardware state).

## Progress

Detailed per-step breakdown with sub-tasks. Each step maps to a step in the phase doc. Sub-tasks are the implementation plan — granular enough to code against.

- [ ] Step N: step title from phase doc
  - [ ] Na: sub-task with specific file/module/function to create or modify
  - [ ] Nb: sub-task
  - [ ] Nc: sub-task
  - [ ] Nd: Verify: `just check` + `just test` + `just run`
- [ ] Step N+1: step title from phase doc
  - [ ] (N+1)a: sub-task
  - [ ] (N+1)b: sub-task
  - [ ] (N+1)c: Verify: `just check` + `just test` + `just run`
- [ ] ...repeat for each feature step in the phase doc...
- [ ] Step N+M-2: step title from phase doc
  - [ ] (N+M-2)a: sub-task
  - [ ] (N+M-2)b: sub-task
  - [ ] (N+M-2)c: Verify: `just check` + `just test` + `just run`
- [ ] Step N+M-1: End-to-end validation and quality gates
  - [ ] (N+M-1)a: Add end-to-end test to self-tests
  - [ ] (N+M-1)b: Update CLAUDE.md, phase doc, developer-guide.md
  - [ ] (N+M-1)c: Run full audit loop
  - [ ] (N+M-1)d: Verify all gates
- [ ] Step N+M: Shared crate refactoring
  - [ ] (N+M)a: Move type/module to shared + tests
  - [ ] (N+M)b: Move type/module to shared + tests
  - [ ] (N+M)c: Verify: `just check` + `just test` + `just run`

## Code Structure Decisions

Key decisions about where code lives, how modules are organized, what data structures to use, and algorithm choices. Document the *why* behind each choice.

- **decision**: rationale
- **decision**: rationale

## Dependencies & Risks

- **Depends on**: what must exist before this work can start
- **Risk**: what could go wrong and how to mitigate

## Issues Encountered

Track problems discovered during implementation and how they were resolved.

(to be filled during implementation)

## Decisions Made

Key choices made during implementation. Graduate significant ones to `decisions/` at end.

(to be filled during implementation)

## Lessons Learned

Hard-won insights discovered during implementation. Graduate to `lessons/` at end.

(to be filled during implementation)
