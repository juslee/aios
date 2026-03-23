---
name: merge-and-cleanup
description: >
  Squash merge a PR, delete the remote and local branch, remove the worktree
  (if working in one), and update main. Use after PR approval.
---

# Merge and Cleanup

Squash merge the current PR, clean up the branch and worktree, and fast-forward main.

## Step 1: Detect PR

If a PR number was passed as an argument, use it. Otherwise, detect from the current branch:

```bash
gh pr view --json number,url,headRefName --jq '{number, url, branch: .headRefName}'
```

If this fails (e.g., no PR exists for the current branch), ask the user for the PR number.

Save the PR number and branch name for later steps.

## Step 2: Squash merge

```bash
gh pr merge <number> --squash --delete-branch
```

This merges the PR with a single squash commit and deletes the **remote** branch. The local branch may or may not be deleted depending on whether you're in a worktree.

If the merge fails (e.g., merge conflicts, failing checks), report the error and stop.

## Step 3: Detect worktree

Check whether the current working directory is inside a git worktree:

```bash
git rev-parse --git-dir
```

- If the result is a **file** (e.g., `/path/to/repo/.git/worktrees/foo`), you are in a worktree.
- If the result is a **directory** (e.g., `.git`), you are in the main repository.

Alternatively, check `git worktree list` and compare the current directory against worktree paths.

## Step 4: If in a worktree — remove it

Record the worktree path and the main repository path:

```bash
WORKTREE_PATH="$(git rev-parse --show-toplevel)"
MAIN_REPO="$(cd "$(git rev-parse --git-common-dir)/.." && pwd)"
```

Change to the main repository and remove the worktree:

```bash
cd "$MAIN_REPO"
git worktree remove "$WORKTREE_PATH"
```

If `git worktree remove` fails because of uncommitted changes, use `--force` only if the PR was already merged (the changes are safe in main).

## Step 5: Switch to main and pull

You must switch off the branch before deleting it. If you're still on the feature branch (non-worktree case), this also moves you to main:

```bash
git checkout main
git pull origin main
```

## Step 6: Delete local branch

The local branch may still exist after the worktree is removed. Delete it gracefully:

```bash
git branch -d <branch-name>
```

If the branch doesn't exist (already cleaned up), ignore the error and continue.

## Step 7: Report

Print a summary of what was done:

- PR number and URL
- Whether a worktree was removed (and its path)
- Whether a local branch was deleted
- Current HEAD on main after pull
