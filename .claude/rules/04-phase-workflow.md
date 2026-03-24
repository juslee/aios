# Phase Implementation Workflow

When implementing Phase N:

1. **SESSION PREP**: Run session start checklist (below) — update tools, toolchain, dependencies
2. **WORKTREE**: Create isolated worktree at `.claude/worktrees/phase-N` — ALL subsequent work happens inside the worktree
3. **READ**: Phase doc → architecture docs → `.claude/rules/*` conventions → knowledge hive (lessons, decisions)
4. **PLAN**: Write working plan to `docs/knowledge/plans/`, commit as first commit on branch. Identify milestone, list files to create/modify, verify dependencies, use TodoWrite
5. **RECONCILE**: Compare plan against phase doc — update phase doc if needed, commit before implementation
6. **IMPLEMENT**: One step at a time. Each step is atomic — complete it fully before moving on. Every step has an "Acceptance:" block — this is your done condition. Update working plan with issues/decisions/lessons as you go.
7. **VERIFY**: Run acceptance criteria commands after each step
8. **COMMIT + PUSH**: After each step passes verification (not batched)
9. **UPDATE DOCS** after each milestone:
   - CLAUDE.md: Workspace Layout, Key Technical Facts, Architecture Doc Map
   - README.md: Project Structure, Build Commands, status text
   - Phase doc: Check off completed tasks, update Status field
   - Developer guide: file sizes, test counts, new patterns
   - Architecture docs: corrections or deviations from spec
   - Dead code cleanup: remove `#[allow(dead_code)]` if unused or if code is now used
   - Run `/audit-loop` — recursive triple audit until 0 issues. Fix all issues, commit.
10. **FINAL GATE**: Run `/verify-phase` + `/audit-loop` one final time before PR — must be 0 issues
11. **DISTILL**: Read working plan, extract lessons/decisions to knowledge hive, delete plan
12. **PR**: Push branch, create PR, run `/review-pr-comments`, then `/merge-and-cleanup`

**PLAN MODE**: If plan mode is active, the `/implement-phase` skill automatically restricts to research + planning only (no builds, commits, or worktree creation). The plan is written to the system plan file. After user approval, the execution path picks up from Phase 1 with the approved plan.

**BLOCKED?** Read the referenced architecture doc section. Architecture docs are the source of truth. Never invent register offsets, struct fields, or memory addresses.

## Session Start Checklist

Before any implementation work:

1. `brew upgrade qemu just` — update system tools
2. Update Rust nightly in `rust-toolchain.toml`, verify build
3. `cargo update` — pull latest dependencies, commit `Cargo.lock` if changed
4. `just check` — confirm zero warnings
