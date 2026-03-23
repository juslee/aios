---
name: generate-phase-doc
description: >
  Generates a phase implementation doc from architecture docs.
  Use when asked to write or generate a phase doc for Phase N.
argument-hint: "[phase-number]"
---

# Generate Phase Doc for Phase $ARGUMENTS

Follow the Phase Doc Generation Workflow from CLAUDE.md:

1. Read `docs/project/development-plan.md` §8 for phase $ARGUMENTS name and deliverable
2. Identify relevant architecture docs (use Architecture Document Map in CLAUDE.md)
3. Read those architecture docs
3b. Search the knowledge hive for relevant decisions that may affect phase planning:
    - Use Obsidian MCP search_notes with subsystem keywords
    - Review docs/knowledge/decisions/ for prior architectural choices
4. Read the previous phase doc for milestone numbering continuity
5. Create an isolated worktree for the doc work:

```bash
git checkout main && git pull origin main
git worktree add .claude/worktrees/phase-$ARGUMENTS-docs -b claude/phase-$ARGUMENTS-docs main
cd .claude/worktrees/phase-$ARGUMENTS-docs
```

All subsequent work (edits, commits, pushes) happens inside the worktree.

6. Generate `docs/phases/` with the correct `NN-name.md` filename
7. Follow the Phase 0/1 template structure exactly (see CLAUDE.md)
8. For each milestone, include a shared crate refactoring step at the end:
    - Review code written in `kernel/` during the milestone
    - Move pure data structures (no hardware deps) to `shared/src/`
    - Write host-side unit tests for moved code
    - Acceptance: `just check` + `just test` pass
9. Milestone numbers: M(3*$ARGUMENTS+1) through M(3*$ARGUMENTS+M), where M is the number of milestones in the phase (variable, 3+ per phase)
10. Commit and push the generated phase doc
11. Run `/audit-loop` — auto-detects docs-only mode, loops until 0 issues
12. Create PR for review
13. Run `/review-pr-comments`: wait for Copilot/reviewer comments, fix issues, reply, resolve conversations, push fixes
14. Run `/merge-and-cleanup`: squash merge the PR, delete remote/local branch, remove worktree, fast-forward main
