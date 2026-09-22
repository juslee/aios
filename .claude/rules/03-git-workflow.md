# Git Workflow

## Branching Convention

All work happens on `claude/*` branches. Never commit directly to `main`.

- Milestone implementations: `claude/phase-N-MK-name` (e.g., `claude/phase-0-m2-boots`)
- Doc generation: `claude/phase-N-docs` (e.g., `claude/phase-5-docs`)
- Doc updates from code changes: `claude/docs-update-*`
- One PR per milestone — the user merges it to `main` before the next milestone starts

## Main Is User-Only

Never push to `main` and never merge a pull request, in any form: no `git push` that updates `main` (`main`, `HEAD:main`, `heads/main`, `refs/heads/main`, or a bare `git push` while on `main`), no force push, no `gh pr merge` (including `--auto` and `--admin`), no `gh api` merge or contents/refs writes, and no MCP merge or push tools. The user merges by running `/merge-and-cleanup` (user-invocable only). When a PR is ready, stop and hand off: report the PR URL and the `gh pr checks` status, and ask the user to run `/merge-and-cleanup`.

Always push an explicit branch name (`git push -u origin claude/<branch>`), never a bare `git push` or `git push origin HEAD`.

## Guard Rails

- The `main` ruleset on GitHub requires a PR and the CI checks, and blocks force pushes and deletion, with no bypass. `.claude/hooks/git-push-guard.py` (PreToolUse on Bash and Monitor) is defence in depth: it denies pushes to `main` and asks before git and gh forms that run commands, discard work or publish outside this repository.
- An ask from the guard or a permission rule prompts even in auto mode, and becomes a refusal in `-p` or background runs. Do not rephrase a refused command to get past it; report what was refused and why.
- `/justin:pause` pushes through `scripts/agent/checkpoint.sh`, whose refspec is a shell variable, so the guard asks once per pause. That is intended: pause is user-invoked, so the user is there to confirm. An unattended runner (a later stage) needs the script moved under `.claude/` and trusted by the guard first.
- Settings, hooks and the guard load from the checkout a session starts in. A worktree created before a settings change keeps its old rules until it is rebased onto `main`. Start sessions in the main checkout, or rebase open worktrees after a settings change merges.

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
4. Hand off to the user, who squash merges via `/merge-and-cleanup`
