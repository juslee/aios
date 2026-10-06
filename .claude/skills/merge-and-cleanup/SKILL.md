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

`gh pr merge --delete-branch` (gh 2.99 and later, cli/cli#14007) runs `git worktree remove` without `--force` on the linked worktree that has the PR branch checked out, then deletes the local branch with `git branch -D`. Git refuses that removal when the worktree has modified or untracked files, and gh then warns and keeps both the worktree and the branch. Git deletes **ignored** files without asking. Soak logs (`target/soak/`) and agent memory (`.claude/agent-memory/`) are gitignored, so Step 3 must copy them out **before** Step 4 merges. On 2026-10-06 PR #211 lost its soak logs because the copy ran after the merge.

Run the skill from the main checkout. Each Bash call starts a new shell and keeps no variables, so every block below recomputes what it needs from `<number>` and `<branch-name>`. Substitute both from Step 1 before running a block.

## Step 1: Resolve the PR

With a PR number as the argument:

```bash
gh pr view <number> --json number,url,headRefName,headRefOid,state --jq '"\(.number) \(.state) \(.headRefName) \(.headRefOid) \(.url)"'
```

Without an argument, run the same command without `<number>` to detect the PR from the current branch. If that fails (for example, in the main checkout on `main`), ask the user for the PR number.

Record the number and the branch name (`headRefName`). If the state is not `OPEN`, report it and stop.

## Step 2: Pre-merge check

Verify CI checks have passed before merging:

```bash
gh pr checks <number>
```

If any checks are failing or pending, report the status and wait. Do NOT attempt to merge with failing checks.

## Step 3: Preserve the PR's worktree (before merging)

Find the worktree that has the PR's branch checked out. Match by branch, not by the current directory:

```bash
BRANCH='<branch-name>'
MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"
WORKTREE_PATH="$(git -C "${MAIN_REPO:?}" worktree list --porcelain | awk -v b="branch refs/heads/$BRANCH" '/^worktree /{w=substr($0,10)} $0==b{print w}')"
echo "main checkout: $MAIN_REPO"
echo "PR worktree:   ${WORKTREE_PATH:-none}"
git -C "$MAIN_REPO" worktree list
```

- **No worktree** (`none`): show the worktree list to the user and ask them to confirm that no worktree holds this PR's work (a worktree on a detached HEAD or another branch does not match). Then run check 2 only and go to Step 4.
- **The main checkout itself** (the two paths are equal): there is no linked worktree, and gh deletes nothing. It checks out `main` there and fast-forwards it. Run check 2 only and go to Step 4.
- **A linked worktree**: run checks 1 to 3, then copies 4 and 5.

1. **Uncommitted work.** Modified or untracked files are not lost: they make git refuse the removal, and gh then keeps the worktree and the local branch. They may still be work the PR is missing:

   ```bash
   BRANCH='<branch-name>'
   MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"
   WORKTREE_PATH="$(git -C "${MAIN_REPO:?}" worktree list --porcelain | awk -v b="branch refs/heads/$BRANCH" '/^worktree /{w=substr($0,10)} $0==b{print w}')"
   git -C "${WORKTREE_PATH:?}" status --porcelain
   ```

   If the output is not empty, show it and ask whether it belongs in the PR. Stop until the user decides.

2. **Commits that are not in the PR.** gh deletes the local branch with `git branch -D`, so a local commit that was never pushed is lost:

   ```bash
   PR=<number>; BRANCH='<branch-name>'
   MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"
   LOCAL="$(git -C "${MAIN_REPO:?}" rev-parse --verify --quiet "refs/heads/$BRANCH")"
   HEAD_OID="$(gh pr view "$PR" --json headRefOid --jq .headRefOid)"
   echo "local: ${LOCAL:-none}  PR head: ${HEAD_OID:?}"
   [ -z "$LOCAL" ] || [ "$LOCAL" = "$HEAD_OID" ]
   ```

   A non-zero exit means the local branch and the PR head differ. Show `git -C "$MAIN_REPO" log --oneline "$HEAD_OID..$BRANCH"` (after `git -C "$MAIN_REPO" fetch origin` if the PR head is not local) and stop until the user pushes the commits or confirms they can go.

3. **Ignored files the merge deletes.** List every ignored path except those this step copies (`target/soak/` within `target/`, `.claude/agent-memory/`) and the disk images and `esp/` that `just disk` rebuilds:

   ```bash
   BRANCH='<branch-name>'
   MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"
   WORKTREE_PATH="$(git -C "${MAIN_REPO:?}" worktree list --porcelain | awk -v b="branch refs/heads/$BRANCH" '/^worktree /{w=substr($0,10)} $0==b{print w}')"
   git -C "${WORKTREE_PATH:?}" status --porcelain --ignored=matching | grep '^!!' \
     | grep -v -e '^!! target/$' -e '^!! \.claude/agent-memory/' -e '^!! "\{0,1\}[^/]*\.img"\{0,1\}$' -e '^!! esp/$'
   ```

   No output (exit 1) means nothing else is at risk. Otherwise show the list and stop until the user says what to keep. A `.remember/` here is the remember plugin's fallback handoff; its section belongs in the main checkout's `.remember/remember.md`. Soak runs that `out=` wrote outside `target/soak/` sit under `target/` and are not listed: ask the user whether there are any.

4. **Soak results.** Copy each entry of the worktree's `target/soak/` to the main checkout as `pr<number>-<name>`, the convention of `target/soak/pr209-fix-198`. An existing copy that matches is skipped; one that differs stops the block:

   ```bash
   PR=<number>; BRANCH='<branch-name>'
   MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"
   WORKTREE_PATH="$(git -C "${MAIN_REPO:?}" worktree list --porcelain | awk -v b="branch refs/heads/$BRANCH" '/^worktree /{w=substr($0,10)} $0==b{print w}')"
   SRC_DIR="${WORKTREE_PATH:?}/target/soak"; DST_DIR="$MAIN_REPO/target/soak"
   if [ -d "$SRC_DIR" ]; then
     mkdir -p "$DST_DIR" &&
     find "$SRC_DIR" -mindepth 1 -maxdepth 1 -print0 | while IFS= read -r -d '' src; do
       dst="$DST_DIR/pr$PR-${src##*/}"
       if [ -e "$dst" ]; then
         if diff -rq "$src" "$dst" >/dev/null; then echo "already copied: $dst"
         else echo "exists and differs: $dst"; exit 1; fi
       else
         cp -Rp "$src" "$dst" && diff -rq "$src" "$dst" >/dev/null && echo "copied: $dst" \
           || { echo "copy failed: $dst"; exit 1; }
       fi
     done && echo "soak results preserved"
   else
     echo "no target/soak in the worktree"
   fi
   ```

   The block ends with `soak results preserved` or `no target/soak in the worktree`. Any other ending is a failure: report it and stop.

5. **Agent memory.** Merge the worktree's `.claude/agent-memory/` into the main checkout's. That is where the `memory: project` agents (`.claude/agents/*.md`) read and write notes when a session starts in the main checkout, as rule 03 recommends. A new worktree starts without them, because `git worktree add` copies no ignored files. New files are copied; a file that exists in both and differs is listed, never overwritten:

   ```bash
   BRANCH='<branch-name>'
   MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"
   WORKTREE_PATH="$(git -C "${MAIN_REPO:?}" worktree list --porcelain | awk -v b="branch refs/heads/$BRANCH" '/^worktree /{w=substr($0,10)} $0==b{print w}')"
   SRC_DIR="${WORKTREE_PATH:?}/.claude/agent-memory"; DST_DIR="$MAIN_REPO/.claude/agent-memory"
   if [ -d "$SRC_DIR" ]; then
     (cd "$SRC_DIR" && find . -type f -print0) | while IFS= read -r -d '' rel; do
       rel="${rel#./}"; src="$SRC_DIR/$rel"; dst="$DST_DIR/$rel"
       if [ ! -e "$dst" ]; then
         mkdir -p "$(dirname "$dst")" && cp -p "$src" "$dst" && echo "added: $rel" \
           || { echo "copy failed: $rel"; exit 1; }
       elif ! cmp -s "$src" "$dst"; then
         echo "differs: $rel"
       fi
     done && echo "agent memory merged"
   else
     echo "no agent memory in the worktree"
   fi
   ```

   The block ends with `agent memory merged` or `no agent memory in the worktree`; any other ending is a failure. For each `differs:` file, show `diff` between the main checkout's copy and the worktree's, and merge the worktree's notes into the main checkout's file before Step 4 (keep every note from both, and ask the user where two notes contradict). The merge deletes the worktree's copy. A file already merged by hand still shows as `differs:` on a re-run.

## Step 4: Squash merge

Run from the main checkout, never from inside the worktree that the merge removes. The block re-checks that the local branch matches the PR head, and `--match-head-commit` makes GitHub refuse the merge if the head moves after the check:

```bash
PR=<number>; BRANCH='<branch-name>'
MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"
cd "${MAIN_REPO:?}" || exit 1
HEAD_OID="$(gh pr view "$PR" --json headRefOid --jq .headRefOid)"
LOCAL="$(git rev-parse --verify --quiet "refs/heads/$BRANCH")"
if [ -n "$LOCAL" ] && [ "$LOCAL" != "${HEAD_OID:?}" ]; then
  echo "local $BRANCH is $LOCAL but the PR head is $HEAD_OID: not merging"; exit 1
fi
gh pr merge "$PR" --squash --delete-branch --match-head-commit "${HEAD_OID:?}"
```

Without `--subject`/`--body-file`, GitHub builds the squash message from the repository settings (title from the PR or its single commit, body from the concatenated commit messages). If the user gives a squash message, add `--subject "<title>" --body-file <file>`.

This merges the PR as a single squash commit and deletes the remote branch. When the PR worktree has no modified or untracked files, gh also removes it and deletes the local branch; otherwise gh warns and leaves both, and Steps 5 and 7 handle them.

If the merge fails (merge conflicts, failing checks, a moved head), report the error and stop.

## Step 5: Verify the worktree is gone

```bash
BRANCH='<branch-name>'
MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"
git -C "${MAIN_REPO:?}" worktree prune
if git -C "$MAIN_REPO" worktree list --porcelain | grep -Fx "branch refs/heads/$BRANCH"; then
  echo "worktree still present"
else
  echo "worktree removed"
fi
```

gh leaves the worktree when gh is older than 2.99, when the worktree has modified or untracked files (git refused, and gh printed a warning but still exited 0), or when the merge ran from inside it. A retry with `git worktree remove` without `--force` fails for the same reason. Re-run Step 3's checks 1 and 3 on it, show the result, and let the user decide: commit or move the files, or run `git -C "$MAIN_REPO" worktree remove --force <path>` after the user confirms. Step 3 already copied the soak results and agent memory. Step 7 cannot delete the branch while this worktree has it checked out.

In the main-checkout case (Step 3), the main checkout is on `main` now and nothing is removed.

## Step 6: Update main

Fast-forward the main checkout:

```bash
MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"
[ "$(git -C "${MAIN_REPO:?}" symbolic-ref --short -q HEAD)" = main ] || { echo "main checkout is not on main"; exit 1; }
git -C "$MAIN_REPO" fetch --prune origin && git -C "$MAIN_REPO" merge --ff-only origin/main
```

`--prune` drops the deleted PR branch's `origin/<branch-name>` ref. A fast-forward works even when the main checkout has unrelated local edits, where `git pull` with rebase refuses. If the main checkout is not on `main`, or the merge refuses (local `main` has diverged from `origin/main`, or local edits touch files the update changes), report the message as is and stop.

## Step 7: Delete the local branch

gh normally deletes it in Step 4. Check whether it remains:

```bash
PR=<number>; BRANCH='<branch-name>'
MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"
echo "local: $(git -C "${MAIN_REPO:?}" rev-parse --verify --quiet "refs/heads/$BRANCH" || echo deleted)"
gh pr view "$PR" --json state,headRefOid --jq '"PR: \(.state) \(.headRefOid)"'
```

If it remains, delete it only when the PR state is `MERGED` and the local SHA equals the PR head SHA, after the user confirms:

```bash
BRANCH='<branch-name>'
MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"
git -C "${MAIN_REPO:?}" branch -D "$BRANCH"
```

`git branch -d` refuses after a squash merge (the squash commit on main has a different SHA from the branch commits), so the SHA check is what makes `-D` safe. If the SHAs differ, the local branch has commits that are not in the merged PR: show them and leave the branch. Never force-delete a branch whose PR is not merged.

## Step 8: Report

Print a summary of what was done:

- PR number and URL, and the squash commit (`git log -1 --oneline origin/main`)
- Soak results copied (each `target/soak/pr<number>-<name>`) and agent-memory files added or merged by hand, or that there were none
- Whether the worktree was removed (and its path)
- Whether the local branch was deleted
- Current HEAD on main
