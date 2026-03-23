---
name: implement-phase
description: >
  Implements an AIOS phase step by step from the phase doc.
  Use when asked to implement Phase N or work on a specific phase.
argument-hint: "[phase-number]"
---

# Implement AIOS Phase $ARGUMENTS

Follow the Phase Implementation Workflow from CLAUDE.md:

## Phase 1: Research & Planning

1. Read `docs/phases/` and find the doc matching phase $ARGUMENTS (glob for `$ARGUMENTS-*.md` or `0$ARGUMENTS-*.md`)
2. Read all Architecture References listed in the phase doc
3. Read CLAUDE.md Code Conventions and Quality Gates
4. Search the knowledge hive for relevant lessons and decisions:
    - Use Obsidian MCP search_notes with keywords from the phase doc
    - Review any matching docs/knowledge/lessons/ and docs/knowledge/decisions/
    - Factor known pitfalls into implementation approach
5. Create a working plan doc in docs/knowledge/plans/:
    - File: docs/knowledge/plans/phase-$ARGUMENTS-description.md
    - Plan out each milestone and step: approach, key decisions, risks, dependencies
    - Include code structure decisions, data structure choices, algorithm rationale
    - Status: in-progress

## Phase 2: Phase Doc Reconciliation

6. Compare the plan against the current phase doc (`docs/phases/`):
    - If planning reveals changes needed (new steps, reordered steps, updated acceptance criteria, corrected references):
      update the phase doc to match the plan
    - Commit and push phase doc updates before any implementation begins
    - This ensures the phase doc is the accurate source of truth for implementation

## Phase 3: Create Worktree

7. Create an isolated worktree for all implementation work:

```bash
# Ensure we're on main and up to date
git checkout main && git pull origin main

# Create worktree with a new branch
# Branch name: claude/phase-$ARGUMENTS-MK-<short-description> (matches CLAUDE.md convention)
# Worktree path: .claude/worktrees/phase-$ARGUMENTS
git worktree add .claude/worktrees/phase-$ARGUMENTS -b claude/phase-$ARGUMENTS-MK-<short-description> main
```

8. **Switch working directory** to the worktree. ALL subsequent work (implementation, commits, pushes, quality gates) happens inside the worktree:

```bash
cd .claude/worktrees/phase-$ARGUMENTS
```

**IMPORTANT**: From this point forward, every file edit, git command, build command, and test command MUST be executed inside the worktree directory. Do NOT operate in the main repo directory until `/merge-and-cleanup` at the end.

## Phase 4: Implementation

9. Create TodoWrite with one item per step, grouped by milestone
10. For each milestone:
    For each step within the milestone (including the shared crate refactoring step baked into the phase doc):
    a. Implement the step
    b. Run acceptance criteria for the step
    c. If any gate fails: fix before proceeding
    d. Commit and push: `Phase $ARGUMENTS MN: Step X — <step description>`
    After all steps in milestone complete:
    e. Update CLAUDE.md, README.md, phase doc (check off completed tasks)
    f. Commit and push: `Phase $ARGUMENTS MN: update docs`

## Phase 5: Verify & Audit

11. Dead code cleanup: find all `#[allow(dead_code)]` items — remove the item if truly unused, or remove just the attribute if the code is now used. Commit and push.
12. Run `/verify-phase $ARGUMENTS` — build/test/QEMU quality gates must all pass
13. Run `/audit-loop` — recursive triple audit (doc, code review, security/bug review) that loops until 0 issues
14. Update the phase doc Status to "Complete", check off all Phase Completion Criteria, commit and push

## Phase 6: Knowledge Distillation

15. Distill knowledge from the working plan doc:
    - Extract hard-won insights → docs/knowledge/lessons/ (permanent)
    - Extract key decisions → docs/knowledge/decisions/ (permanent)
    - Use YYYY-MM-DD-initials-phase-$ARGUMENTS-description.md naming
    - Delete the working plan doc (docs/knowledge/plans/phase-*.md)
    - Commit and push

## Phase 7: PR, Review & Merge

16. Create PR to main (from the worktree — `gh pr create` works from any checkout)
17. Run `/review-pr-comments`: wait for Copilot/reviewer comments, fix issues, reply, resolve conversations, push fixes
18. Run `/merge-and-cleanup`: squash merge the PR, delete remote/local branch, remove worktree, fast-forward main
    - `/merge-and-cleanup` auto-detects the worktree, removes it, and returns to the main repo
