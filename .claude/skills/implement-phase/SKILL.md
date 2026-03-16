---
name: implement-phase
description: >
  Implements an AIOS phase step by step from the phase doc.
  Use when asked to implement Phase N or work on a specific phase.
argument-hint: "[phase-number]"
---

# Implement AIOS Phase $ARGUMENTS

Follow the Phase Implementation Workflow from CLAUDE.md:

1. Read `docs/phases/` and find the doc matching phase $ARGUMENTS (glob for `$ARGUMENTS-*.md` or `0$ARGUMENTS-*.md`)
2. Read all Architecture References listed in the phase doc
3. Read CLAUDE.md Code Conventions and Quality Gates
3b. Search the knowledge hive for relevant lessons and decisions:
    - Use Obsidian MCP search_notes with keywords from the phase doc
    - Review any matching docs/knowledge/lessons/ and docs/knowledge/decisions/
    - Factor known pitfalls into implementation approach
3c. Create a working plan doc in docs/knowledge/plans/:
    - File: docs/knowledge/plans/phase-$ARGUMENTS-description.md
    - Track approach, decisions, issues encountered as you go
    - Status: in-progress
4. Create worktree via `git worktree add .claude/worktrees/phase-$ARGUMENTS -b claude/phase-$ARGUMENTS-*` from main; work inside the worktree
5. Create TodoWrite with one item per step, grouped by milestone
6. For each milestone (M1, M2, M3):
   a. Implement all steps in order
   b. Run acceptance criteria after each step
   c. If any gate fails: fix before proceeding
   d. After milestone complete: update CLAUDE.md, commit with `Phase $ARGUMENTS MN: <name>`
7. After all milestones: push branch, create PR to main
7b. Distill knowledge from the working plan doc:
    - Extract hard-won insights → docs/knowledge/lessons/ (permanent)
    - Extract key decisions → docs/knowledge/decisions/ (permanent)
    - Use YYYY-MM-DD-initials-phase-$ARGUMENTS-description.md naming
    - Delete the working plan doc (docs/knowledge/plans/phase-*.md)
    - Commit with the milestone
8. Run doc-auditor if any docs were modified
