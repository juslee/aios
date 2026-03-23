# Phase Implementation Workflow

When implementing Phase N:

1. **READ**: Phase doc → architecture docs → `.claude/rules/*` conventions (and CLAUDE.md reference data)
2. **WORKTREE**: Create isolated worktree at `.claude/worktrees/phase-N`
3. **PLAN**: Identify milestone, list files to create/modify, verify dependencies, use TodoWrite
4. **IMPLEMENT**: One step at a time. Each step is atomic — complete it fully before moving on. Every step has an "Acceptance:" block — this is your done condition.
5. **VERIFY**: Run acceptance criteria commands after each step
6. **COMMIT + PUSH**: After each step passes verification (not batched)
7. **UPDATE DOCS** after each milestone:
   - CLAUDE.md: Workspace Layout, Key Technical Facts, Architecture Doc Map
   - README.md: Project Structure, Build Commands, status text
   - Phase doc: Check off completed tasks, update Status field
   - Developer guide: file sizes, test counts, new patterns
   - Architecture docs: corrections or deviations from spec
8. **AUDIT**: Run `/audit-loop` — recursive triple audit until 0 issues
9. **PR**: Push branch, create PR, run `/review-pr-comments`, then `/merge-and-cleanup`

**BLOCKED?** Read the referenced architecture doc section. Architecture docs are the source of truth. Never invent register offsets, struct fields, or memory addresses.

## Session Start Checklist

Before any implementation work:

1. `brew upgrade qemu just` — update system tools
2. Update Rust nightly in `rust-toolchain.toml`, verify build
3. `cargo update` — pull latest dependencies, commit `Cargo.lock` if changed
4. `just check` — confirm zero warnings
