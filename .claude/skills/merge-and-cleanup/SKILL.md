---
name: merge-and-cleanup
description: >
  Squash merge a PR, preserve its worktree's soak results and agent memory,
  delete the remote and local branch, remove the worktree, and update main.
  Run by the user after PR approval; agents hand off instead of merging.
disable-model-invocation: true
---

# Merge and Cleanup

Squash merge a PR, keep what only its worktree holds, clean up the branch and worktree, and fast-forward main.

`gh pr merge --delete-branch` deletes the local branch **and removes the worktree that has it checked out**, together with every untracked file in it (gh 2.102). Soak logs (`target/soak/`) and agent memory (`.claude/agent-memory/`) are untracked, so Step 3 must copy them out **before** Step 4 merges. On 2026-10-06 PR #211 lost its soak logs because the copy ran after the merge.

## Step 1: Detect PR

If a PR number was passed as an argument, use it. Otherwise, detect from the current branch:

```bash
gh pr view --json number,url,headRefName --jq '{number, url, branch: .headRefName}'
```

If this fails (e.g., no PR exists for the current branch), ask the user for the PR number.

Save the PR number and branch name for later steps. Record the main repository path from any checkout:

```bash
MAIN_REPO="$(cd "$(git rev-parse --path-format=absolute --git-common-dir)/.." && pwd)"
```

## Step 2: Pre-merge check

Verify CI checks have passed before merging:

```bash
gh pr checks <number>
```

If any checks are failing or pending, report the status and wait. Do NOT attempt to merge with failing checks.

## Step 3: Preserve the PR's worktree (before merging)

Find the worktree that has the PR's branch checked out. Match by branch, not by the current directory: this skill usually runs from the main checkout.

```bash
WORKTREE_PATH="$(git -C "$MAIN_REPO" worktree list --porcelain | awk -v b="branch refs/heads/<branch-name>" '/^worktree /{w=substr($0,10)} $0==b{print w}')"
```

If `WORKTREE_PATH` is empty, no worktree holds the branch: skip to Step 4.

Otherwise:

1. **Uncommitted work.** The merge removes the worktree, so anything not committed is lost. Check it first (ignore `target/`):

   ```bash
   git -C "$WORKTREE_PATH" status --porcelain | grep -v '^?? target/'
   ```

   If the output is not empty, show it to the user and stop until they say how to proceed.

2. **Soak results.** Copy every run directory to the main checkout as `pr<number>-<run>`, the convention of `target/soak/pr209-fix-198`:

   ```bash
   if [ -d "$WORKTREE_PATH/target/soak" ]; then
     mkdir -p "$MAIN_REPO/target/soak"
     for run in "$WORKTREE_PATH"/target/soak/*/; do
       [ -d "$run" ] && cp -Rp "$run" "$MAIN_REPO/target/soak/pr<number>-$(basename "$run")"
     done
   fi
   ```

3. **Agent memory.** Archive the worktree's `.claude/agent-memory/`, which is not tracked, so notes agents wrote there survive:

   ```bash
   if [ -d "$WORKTREE_PATH/.claude/agent-memory" ]; then
     DEST="$(git -C "$MAIN_REPO" rev-parse --path-format=absolute --git-common-dir)/aios-agent/agent-memory-harvest/pr<number>-$(basename "$WORKTREE_PATH")"
     mkdir -p "$DEST" && cp -Rp "$WORKTREE_PATH/.claude/agent-memory/." "$DEST/"
   fi
   ```

   Report the archive path in Step 8 so the user can move notes worth keeping into the main checkout's `.claude/agent-memory/`.

Confirm the copies exist (`ls`) before Step 4.

## Step 4: Squash merge

Run this from the main checkout, never from inside the worktree that the merge removes:

```bash
cd "$MAIN_REPO"
gh pr merge <number> --squash --delete-branch
```

If the PR has a curated squash message (for example a "Proposed squash commit message" comment), pass it with `--subject` and `--body-file` instead of letting gh concatenate the branch's commit messages.

This merges the PR with a single squash commit, deletes the remote branch, deletes the local branch and removes its worktree.

If the merge fails (e.g., merge conflicts, failing checks), report the error and stop.

## Step 5: Verify the worktree is gone

```bash
git -C "$MAIN_REPO" worktree prune
git -C "$MAIN_REPO" worktree list --porcelain | grep -F "branch refs/heads/<branch-name>"
```

If the worktree is still listed (older gh versions leave it), remove it with `git -C "$MAIN_REPO" worktree remove "$WORKTREE_PATH"`. Step 3 already showed any uncommitted changes; never add `--force` without asking the user.

## Step 6: Update main

Fast-forward the main checkout:

```bash
git -C "$MAIN_REPO" fetch origin
git -C "$MAIN_REPO" merge --ff-only origin/main
```

A fast-forward merge works even when the main checkout has unrelated local edits, where `git pull` with rebase refuses. If it still refuses, report the conflicting files and stop.

## Step 7: Delete local branch

`gh pr merge --delete-branch` normally deletes it already. If it still exists:

```bash
git -C "$MAIN_REPO" branch -d <branch-name>
```

After a squash merge, `git branch -d` normally fails with "not fully merged", because the squash commit on main has a different SHA from the branch commits. In that case confirm the PR is merged (`gh pr view <number> --json state --jq .state` prints `MERGED`), then ask the user to confirm before running `git branch -D <branch-name>`. Never force-delete a branch whose PR is not merged.

## Step 8: Report

Print a summary of what was done:

- PR number and URL, and the merge commit
- Where soak results and agent memory were archived (or that there were none)
- Whether the worktree was removed (and its path)
- Whether the local branch was deleted
- Current HEAD on main
