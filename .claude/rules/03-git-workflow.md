# Git Workflow

## Branching Convention

All work happens on `claude/*` branches. Never commit directly to `main`.

- Milestone implementations: `claude/phase-N-MK-name` (e.g., `claude/phase-0-m2-boots`)
- Doc generation: `claude/phase-N-docs` (e.g., `claude/phase-5-docs`)
- Doc updates from code changes: `claude/docs-update-*`
- One PR per milestone — merge to `main` before starting the next milestone

## Worktrees

All implementation and doc generation work uses isolated git worktrees:

```bash
git checkout main && git pull origin main
git worktree add .claude/worktrees/phase-N -b claude/phase-N-description main
cd .claude/worktrees/phase-N
```

All subsequent work (edits, builds, commits, pushes) happens inside the worktree. The main repo stays on `main`.

## Commit Convention

- Commit and push immediately after each step passes verification
- Do not batch multiple steps into a single commit
- Format: `Phase N MK: Step X — <step description>`
- Example: `Phase 2 M8: Step 4 — page table infrastructure`

## PR Workflow

1. Push branch, create PR to `main`
2. Wait 3-7 minutes for Copilot/automated reviewers to post comments
3. Address all comments: fix issues, reply, resolve conversations
4. Squash merge via `/merge-and-cleanup`
