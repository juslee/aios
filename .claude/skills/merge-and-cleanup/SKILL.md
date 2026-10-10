---
name: merge-and-cleanup
description: >
  Preserve a PR worktree's soak results, squash merge the PR,
  delete the remote and local branch, remove the worktree, and update main.
  Run by the user after PR approval; agents hand off instead of merging.
argument-hint: "[PR]"
disable-model-invocation: true
---

# Merge and Cleanup

Preserve a PR worktree's soak results, squash merge the PR, delete the remote and local branch, remove the worktree, and update main.

`gh pr merge --delete-branch` (gh 2.99 and later, cli/cli#14007) runs `git worktree remove` without `--force` on the linked worktree that has the PR branch checked out, then deletes the local branch with `git branch -D`. Git refuses that removal when the worktree has modified or untracked files or is locked, and gh then warns and keeps both the worktree and the branch. Git deletes **ignored** files without asking. Soak logs (`target/soak/`) are gitignored, so Step 3 must copy them out **before** Step 4 merges. On 2026-10-06 PR #211 lost its soak logs because the copy ran after the merge.

Run the skill from the main checkout. Each Bash call starts a new shell and keeps no variables, so every block below recomputes what it needs from `<number>`. Substitute it from Step 1 before running a block; the blocks look up the branch name from the PR on GitHub, so a mistyped branch cannot point them at the wrong worktree. Commands quoted in the prose use `<branch-name>`, `<main-checkout>`, `<worktree>`, `<path>`, `<pr-head-sha>` and `<run>` (an entry of the worktree's `target/soak/`): fill them in from the output of the earlier blocks, and quote each filled-in value for the shell, as the prose commands do: paths can hold spaces, and branch names can hold quotes or `$` (single quotes then, with each `'` written as `'\''`). `<backup-name>` is a name the user picks, and `<title>` and `<file>` come from the squash message the user gives.

## Step 1: Resolve the PR

With a PR number as the argument:

```bash
gh pr view <number> --json number,url,headRefName,headRefOid,state --jq '"\(.number) \(.state) \(.headRefName) \(.headRefOid) \(.url)"'
```

Without an argument, list the open PRs and ask the user which one to merge (the skill runs from the main checkout, which is normally on `main`, so the current branch does not identify the PR):

```bash
gh pr list --state open --json number,headRefName,title --jq '.[] | "\(.number) \(.headRefName) \(.title)"'
```

Record the number and the branch name (`headRefName`). Then, by state:

- `OPEN`: continue with Step 2.
- `MERGED`: an earlier run stopped after the merge, or the PR was merged elsewhere. Report it and skip Steps 2 and 4: run Step 3 if a worktree still has the branch checked out (its copies skip what an earlier run already copied), then Steps 5 to 9.
- `CLOSED`: report it and stop.

## Step 2: Pre-merge check

Verify CI checks have passed before merging:

```bash
gh pr checks <number>
```

If any checks are failing or pending, report the status and wait. Do NOT attempt to merge with failing checks.

Then check whether the PR is gated:

```bash
bash scripts/agent/brief.sh --no-fetch | awk -v n="- #<number> " 'index($0, n) == 1 {p = 1} p && /merge-ready:/ {print; exit}'
```

- `merge-ready: no` naming `gated by #N` or `named in needs-human #N`: report the issue and stop. The owner closes or edits it first. Cross-team merge order exists only as these gate issues (rule 11).
- Other `no` reasons (draft, unresolved threads, changes requested): report them and stop unless the user confirms each by name.

Then check the QEMU lock:

```bash
bash scripts/agent/qemu-lock.sh status
```

If it shows `mode=quiet`, stop: merging moves `main`, and the next hook call starts a background `aios` rebuild during a rate soak. Merge after `QUIET-END`.

## Step 3: Preserve the PR's worktree (before merging)

When the PR is already `MERGED` (Step 1), Step 4 is skipped: read every "go to Step 4" and "before Step 4" in this step as Step 5.

Find the worktree that has the PR's branch checked out. Match by branch, not by the current directory:

```bash
PR=<number>; BRANCH="$(gh pr view "$PR" --json headRefName --jq .headRefName)"; : "${BRANCH:?}"
MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"; : "${MAIN_REPO:?}"
WORKTREE_PATH="$(git -C "$MAIN_REPO" worktree list --porcelain | awk -v b="branch refs/heads/$BRANCH" '/^worktree /{w=substr($0,10)} $0==b{print w}')"
echo "main checkout: $MAIN_REPO"
echo "PR worktree:   ${WORKTREE_PATH:-none}"
git -C "$MAIN_REPO" worktree list
[ "$(printf '%s' "$WORKTREE_PATH" | grep -c '')" -le 1 ] || { echo "more than one worktree has $BRANCH checked out"; exit 1; }
```

If the block reports more than one worktree, show the list and stop: gh removes only the first, and the user decides what happens to the others.

- **No worktree** (`none`): show the worktree list to the user and ask them to confirm that no worktree holds this PR's work (a worktree on a detached HEAD or another branch does not match). Then run check 2 only and go to Step 4.
- **The main checkout itself** (the two paths are equal): there is no linked worktree to remove. gh checks out `main` there, fast-forwards it (`git pull --ff-only`) and deletes the local branch with `git branch -D`. Uncommitted edits either move onto `main` with the checkout or make it fail after the PR is already merged. If a linked worktree has `main` checked out, gh warns, skips the checkout and the local delete, and exits 0. Run checks 1 and 2, then go to Step 4.
- **A linked worktree**: run checks 1 to 3, then copy 4.

If a check ends with `worktree path not found`, the directory was deleted by hand and nothing is left to preserve. Run `git -C "<main-checkout>" worktree prune`, re-run the first block of this step (it then reports `none`), and continue as for no worktree.

1. **Uncommitted work.** In a linked worktree, modified or untracked files are not lost: they make git refuse the removal, and gh then keeps the worktree and the local branch. In the main checkout they ride along onto `main` or block gh's checkout. Either way they may be work the PR is missing:

   ```bash
   PR=<number>; BRANCH="$(gh pr view "$PR" --json headRefName --jq .headRefName)"; : "${BRANCH:?}"
   MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"; : "${MAIN_REPO:?}"
   WORKTREE_PATH="$(git -C "$MAIN_REPO" worktree list --porcelain | awk -v b="branch refs/heads/$BRANCH" '/^worktree /{w=substr($0,10)} $0==b{print w}')"
   [ -d "${WORKTREE_PATH:?}" ] || { echo "worktree path not found: $WORKTREE_PATH"; exit 2; }
   ST="$(git -C "$WORKTREE_PATH" status --porcelain)" || { echo "git status failed"; exit 2; }
   if [ -z "$ST" ]; then echo "no uncommitted work"; else printf '%s\n' "$ST"; exit 1; fi
   ```

   On `no uncommitted work`, continue. On a list, show it and ask whether it belongs in the PR, and stop until the user decides. On any other ending, report it and stop.

2. **The local branch must equal the PR head.** gh deletes the local branch with `git branch -D`, so a local commit that was never pushed is lost. Step 4 refuses to merge until the local branch equals the PR head or does not exist:

   ```bash
   PR=<number>; BRANCH="$(gh pr view "$PR" --json headRefName --jq .headRefName)"; : "${BRANCH:?}"
   MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"; : "${MAIN_REPO:?}"
   LOCAL="$(git -C "$MAIN_REPO" rev-parse --verify --quiet "refs/heads/$BRANCH")"
   HEAD_OID="$(gh pr view "$PR" --json headRefOid --jq .headRefOid)"
   echo "local: ${LOCAL:-none}  PR head: ${HEAD_OID:?}"
   if [ -n "$LOCAL" ] && [ "$LOCAL" != "$HEAD_OID" ]; then
     git -C "$MAIN_REPO" fetch --quiet origin "refs/pull/$PR/head" || { echo "fetch failed"; exit 2; }
     echo "local commits not in the PR:"; git -C "$MAIN_REPO" log --oneline "$HEAD_OID..$LOCAL"
     echo "PR commits not in the local branch:"; git -C "$MAIN_REPO" log --oneline "$LOCAL..$HEAD_OID"
     exit 1
   fi
   ```

   Exit 0 means they match (or there is no local branch). The block fetches the PR head from `refs/pull/<number>/head`, which GitHub keeps after the branch is deleted, so it works in every PR state. On `fetch failed`, report it and stop. Otherwise show both lists and stop until the user picks one of these, then re-run the check:

   - **Local commits only** (the branch is ahead): the user pushes them with `git -C "<worktree>" push -u origin "<branch-name>"` (or `git -C "<main-checkout>" push -u origin "<branch-name>"` when no worktree has the branch checked out). The PR head moves, so CI runs again: go back to Step 2.
   - **PR commits only** (the branch is behind): fast-forward it to the PR head the block just fetched, with `git -C "<worktree>" merge --ff-only <pr-head-sha>`, or `git -C "<main-checkout>" branch -f "<branch-name>" <pr-head-sha>` when no worktree has the branch checked out.
   - **Local commits the user wants to drop** (ahead or diverged): the user keeps them on a backup branch with `git -C "<main-checkout>" branch "<backup-name>" "<branch-name>"`, then resets the branch to the PR head with `git -C "<worktree>" reset --keep <pr-head-sha>` (or `git -C "<main-checkout>" branch -f "<branch-name>" <pr-head-sha>` without a worktree).

   When the PR is already `MERGED`, pushing changes nothing that was merged: local-only commits are not in the merge, so the user keeps them on a backup branch (the third option) before Step 7 deletes the branch.

3. **Ignored files the merge deletes.** List every ignored path except all of `target/` (build output, plus the `target/soak/` that copy 4 handles), `aios.img` (which `just disk` rebuilds), `data.img` (which `just create-data-disk` recreates empty, so any storage state on it is dropped), and files tools regenerate (`.DS_Store`, `__pycache__/`, editor swap and backup files):

   ```bash
   PR=<number>; BRANCH="$(gh pr view "$PR" --json headRefName --jq .headRefName)"; : "${BRANCH:?}"
   MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"; : "${MAIN_REPO:?}"
   WORKTREE_PATH="$(git -C "$MAIN_REPO" worktree list --porcelain | awk -v b="branch refs/heads/$BRANCH" '/^worktree /{w=substr($0,10)} $0==b{print w}')"
   [ -d "${WORKTREE_PATH:?}" ] || { echo "worktree path not found: $WORKTREE_PATH"; exit 2; }
   IGN="$(git -C "$WORKTREE_PATH" status --porcelain --ignored=matching)" || { echo "git status failed"; exit 2; }
   LEFT="$(printf '%s\n' "$IGN" | grep '^!! ' | grep -Ev -e '^!! target/$' \
     -e '^!! (aios|data)\.img$' -e '^!! "?(.*/)?\.DS_Store"?$' -e '^!! "?(.*/)?__pycache__/"?$' \
     -e '\.sw[po]"?$' -e '~"?$')"
   if [ -z "$LEFT" ]; then echo "no other ignored files"; else printf '%s\n' "$LEFT"; exit 1; fi
   ```

   On `no other ignored files`, continue. A list of `!!` paths: show it and stop until the user says what to keep. Any other ending is a failure: report it and stop. Soak runs that `out=` wrote elsewhere under `target/` are skipped too: ask the user whether there are any. Runs written outside `target/` show up in check 1 instead.

4. **Soak results.** Copy each entry of the worktree's `target/soak/` (except Finder's `.DS_Store`) to the main checkout as `target/soak/pr<number>-<run>`, the convention of `target/soak/pr209-fix-198`. The block refuses while any run holds a `.scratch.*` directory, which `aios soak` (`just soak`) removes when a run ends. An existing copy that matches is skipped; one that differs stops the block:

   ```bash
   set -o pipefail
   PR=<number>; BRANCH="$(gh pr view "$PR" --json headRefName --jq .headRefName)"; : "${BRANCH:?}"
   MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"; : "${MAIN_REPO:?}"
   WORKTREE_PATH="$(git -C "$MAIN_REPO" worktree list --porcelain | awk -v b="branch refs/heads/$BRANCH" '/^worktree /{w=substr($0,10)} $0==b{print w}')"
   [ "${WORKTREE_PATH:?}" != "$MAIN_REPO" ] || { echo "PR branch is in the main checkout: nothing to copy"; exit 0; }
   [ -d "$WORKTREE_PATH" ] || { echo "worktree path not found: $WORKTREE_PATH"; exit 1; }
   SRC_DIR="$WORKTREE_PATH/target/soak"; DST_DIR="$MAIN_REPO/target/soak"
   if [ -d "$SRC_DIR" ]; then
     ACTIVE="$(find "$SRC_DIR" -name '.scratch.*' -prune -print)"
     [ -z "$ACTIVE" ] || { printf 'soak running or killed:\n%s\n' "$ACTIVE"; exit 1; }
     BAD="$(find "$SRC_DIR" \( -type d ! -perm -u=rx -o ! -type d ! -perm -u=r \) -print 2>&1)"
     [ -z "$BAD" ] || { printf 'unreadable:\n%s\n' "$BAD"; exit 1; }
     mkdir -p "$DST_DIR" &&
     find "$SRC_DIR" -mindepth 1 -maxdepth 1 ! -name .DS_Store -print0 | while IFS= read -r -d '' src; do
       dst="$DST_DIR/pr$PR-${src##*/}"
       if [ -e "$dst" ]; then
         if [ -z "$(diff -rq "$src" "$dst" 2>&1)" ]; then echo "already copied: $dst"
         else echo "exists and differs: $dst"; exit 1; fi
       else
         cp -Rp "$src" "$dst" && [ -z "$(diff -rq "$src" "$dst" 2>&1)" ] && echo "copied: $dst" \
           || { echo "copy failed: $dst"; exit 1; }
       fi
     done && echo "soak results preserved"
   else
     echo "no target/soak in the worktree"
   fi
   ```

   The block ends with `soak results preserved`, `no target/soak in the worktree` or `PR branch is in the main checkout: nothing to copy`. Any other ending is a failure: report it and stop.

   - `soak running or killed`: a soak still running would lose every log it writes after the copy. Ask the user to let it finish. A `.scratch.*` left by a killed run holds only disk images: the user removes it, then re-run the block.
   - `exists and differs`: an earlier copy no longer matches the run, because a `copy failed` left a partial copy or the run changed after it was copied (for example, a soak re-run into the same `out=`). The block never overwrites. Show `diff -rq "<worktree>/target/soak/<run>" "<main-checkout>/target/soak/pr<number>-<run>"`, remove the old `target/soak/pr<number>-<run>` after the user confirms, and re-run the block.
   - `copy failed`: the partial copy stays in the main checkout, and re-runs report it as `exists and differs` (above).
   - `unreadable`: the listed files or directories (or `find` errors) cannot be read, so the copy would be incomplete. Let the user fix the permissions, then re-run the block. The comparisons count anything `diff -rq` prints, errors included, as a difference, because macOS `diff` exits 0 when it cannot read a directory.

## Step 4: Squash merge

Run from the main checkout, never from inside the worktree that the merge removes. The block re-checks that the local branch matches the PR head, and `--match-head-commit` makes GitHub refuse the merge if the head moves after the check:

```bash
PR=<number>; BRANCH="$(gh pr view "$PR" --json headRefName --jq .headRefName)"; : "${BRANCH:?}"
MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"; : "${MAIN_REPO:?}"
cd "$MAIN_REPO" || exit 1
HEAD_OID="$(gh pr view "$PR" --json headRefOid --jq .headRefOid)"
LOCAL="$(git rev-parse --verify --quiet "refs/heads/$BRANCH")"
if [ -n "$LOCAL" ] && [ "$LOCAL" != "${HEAD_OID:?}" ]; then
  echo "local $BRANCH is $LOCAL but the PR head is $HEAD_OID: not merging"; exit 1
fi
gh pr merge "$PR" --squash --delete-branch --match-head-commit "${HEAD_OID:?}"
```

Without `--subject`/`--body-file`, GitHub builds the squash message from the repository settings (title from the PR or its single commit, body from the concatenated commit messages). If the user gives a squash message, add `--subject "<title>" --body-file "<file>"`.

This merges the PR as a single squash commit, then cleans up locally and deletes the remote branch. For a linked worktree with no modified or untracked files that is not locked, gh removes it and deletes the local branch; otherwise gh warns, leaves both and still exits 0. In the main-checkout case it checks out `main` and deletes the local branch. If a linked worktree has `main` checked out, gh warns, skips the checkout and the local delete, and exits 0.

If gh exits non-zero, check the PR state with `gh pr view <number> --json state --jq .state`:

- `OPEN`: the merge failed (merge conflicts, failing checks, a moved head). Report the error and stop.
- `MERGED`: the merge succeeded and gh's local cleanup failed, for example because uncommitted edits blocked its checkout of `main`. gh then skips deleting the remote branch as well. Report the error and continue with Step 5; Steps 5 and 7 finish the cleanup.

## Step 5: Verify the worktree is gone

```bash
PR=<number>; BRANCH="$(gh pr view "$PR" --json headRefName --jq .headRefName)"; : "${BRANCH:?}"
MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"; : "${MAIN_REPO:?}"
git -C "$MAIN_REPO" worktree prune
WORKTREE_PATH="$(git -C "$MAIN_REPO" worktree list --porcelain | awk -v b="branch refs/heads/$BRANCH" '/^worktree /{w=substr($0,10)} $0==b{print w}')"
if [ -z "$WORKTREE_PATH" ]; then echo "no worktree has $BRANCH checked out"
elif [ "$WORKTREE_PATH" = "$MAIN_REPO" ]; then echo "main checkout is still on $BRANCH"
else echo "worktree still present: $WORKTREE_PATH"; fi
```

- `no worktree has <branch-name> checked out`: gh removed the linked worktree, or, in the main-checkout case, switched the main checkout to `main` without removing anything.
- `main checkout is still on <branch-name>`: gh did not switch it to `main`. Either its checkout failed (Step 4 exited non-zero), a linked worktree has `main` checked out (gh skipped the checkout; the user switches that worktree off `main` first), or the PR was merged without this skill (Step 1, `MERGED`). Run Step 3 check 1, let the user commit or move the edits, then run `git -C "<main-checkout>" checkout main`.
- `worktree still present: <path>`: gh is older than 2.99, the worktree has modified or untracked files or is locked (git refused, and gh printed a warning but exited 0), or the PR was merged without this skill (Step 1, `MERGED`). Run Step 3's checks 1 and 3 on it, and its copy 4 if it has not run. If both checks come back clean, remove it with `git -C "<main-checkout>" worktree remove "<path>"`. Otherwise show the result and let the user decide: commit or move the files, or run `git -C "<main-checkout>" worktree remove --force "<path>"` after the user confirms. A locked worktree (`git worktree list` shows `locked`) needs `git -C "<main-checkout>" worktree unlock "<path>"` first, after the user confirms. Step 7 cannot delete the branch while a worktree has it checked out.

## Step 6: Update main

Fast-forward the main checkout:

```bash
MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"; : "${MAIN_REPO:?}"
[ "$(git -C "$MAIN_REPO" symbolic-ref --short -q HEAD)" = main ] || { echo "main checkout is not on main"; exit 1; }
git -C "$MAIN_REPO" fetch --prune origin && git -C "$MAIN_REPO" merge --ff-only origin/main
```

`--prune` drops the deleted PR branch's `origin/<branch-name>` ref. A fast-forward works even when the main checkout has unrelated local edits, where `git pull` with rebase refuses. If the main checkout is not on `main`, or the merge refuses (local `main` has diverged from `origin/main`, or local edits touch files the update changes), report the message as is and stop.

After the fast-forward, install a changed toolchain:

```bash
MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"; : "${MAIN_REPO:?}"
MERGED="$(gh pr view <number> --json mergeCommit --jq .mergeCommit.oid)"; : "${MERGED:?}"
if git -C "$MAIN_REPO" diff --name-only "$MERGED^" "$MERGED" | grep -qx 'rust-toolchain.toml'; then
  (cd "$MAIN_REPO" && rustup install) && echo "toolchain=$(sed -n 's/^channel = "\(.*\)"/\1/p' "$MAIN_REPO/rust-toolchain.toml") installed"
else
  echo "toolchain unchanged"
fi
```

`rustup install` with no argument installs the active toolchain that `rust-toolchain.toml` names, with its components and targets. It prompts (settings ask rule); the user is present. With `RUSTUP_AUTO_INSTALL=0` (rule 11, Toolchain) this is the only install.

## Step 7: Delete the remaining branches

gh normally deletes the local and the remote branch in Step 4. Check whether either remains:

```bash
PR=<number>; BRANCH="$(gh pr view "$PR" --json headRefName --jq .headRefName)"; : "${BRANCH:?}"
MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"; : "${MAIN_REPO:?}"
echo "local:  $(git -C "$MAIN_REPO" rev-parse --verify --quiet "refs/heads/$BRANCH" || echo deleted)"
REMOTE="$(git -C "$MAIN_REPO" ls-remote --heads origin "refs/heads/$BRANCH")" || { echo "ls-remote failed"; exit 1; }
REMOTE_OID="$(printf '%s\n' "$REMOTE" | awk -F'\t' -v r="refs/heads/$BRANCH" '$2==r{print $1}')"
echo "remote: ${REMOTE_OID:-deleted}"
gh pr view "$PR" --json state,headRefOid --jq '"PR:     \(.state) \(.headRefOid)"'
```

`ls-remote` matches a pattern by its tail, so the block keeps only the exact `refs/heads/<branch-name>` line. A remaining branch may be deleted only when the PR is `MERGED` and the branch's SHA equals the PR head SHA. Each block below enforces both and deletes nothing otherwise. Run the block for the branch that remains, after the user confirms.

Local branch:

```bash
PR=<number>; BRANCH="$(gh pr view "$PR" --json headRefName --jq .headRefName)"; : "${BRANCH:?}"
MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"; : "${MAIN_REPO:?}"
STATE="$(gh pr view "$PR" --json state --jq .state)"; HEAD_OID="$(gh pr view "$PR" --json headRefOid --jq .headRefOid)"
[ "${STATE:?}" = MERGED ] || { echo "PR is $STATE, not MERGED: not deleting"; exit 1; }
LOCAL="$(git -C "$MAIN_REPO" rev-parse --verify --quiet "refs/heads/$BRANCH")" || { echo "no local $BRANCH"; exit 0; }
if [ "$LOCAL" = "${HEAD_OID:?}" ]; then
  git -C "$MAIN_REPO" branch -D "$BRANCH"
else
  git -C "$MAIN_REPO" fetch --quiet origin "refs/pull/$PR/head" || { echo "fetch failed"; exit 2; }
  echo "local $BRANCH is $LOCAL but the merged PR head is $HEAD_OID: not deleting. Commits not in the PR:"
  git -C "$MAIN_REPO" log --oneline "$HEAD_OID..$LOCAL"; exit 1
fi
```

Remote branch:

```bash
PR=<number>; BRANCH="$(gh pr view "$PR" --json headRefName --jq .headRefName)"; : "${BRANCH:?}"
MAIN_REPO="$(git worktree list --porcelain | awk 'NR==1{print substr($0,10); exit}')"; : "${MAIN_REPO:?}"
STATE="$(gh pr view "$PR" --json state --jq .state)"; HEAD_OID="$(gh pr view "$PR" --json headRefOid --jq .headRefOid)"
[ "${STATE:?}" = MERGED ] || { echo "PR is $STATE, not MERGED: not deleting"; exit 1; }
git -C "$MAIN_REPO" push --force-with-lease="refs/heads/$BRANCH:${HEAD_OID:?}" origin --delete "refs/heads/$BRANCH"
```

The lease makes git refuse the delete (`rejected ... (stale info)`) when the remote branch is no longer at the merged PR head, for example after a push since the check above. Then show the remote commits with `git -C "<main-checkout>" fetch origin "<branch-name>"` and `git -C "<main-checkout>" log --oneline "<pr-head-sha>..origin/<branch-name>"`, and leave the branch.

The push guard (`.claude/hooks/git-push-guard.py`) asks before `gh pr merge` (Step 4), `git branch -D` and `git push --delete` (and before the `git branch -f` fixes in Step 3 and `git worktree remove --force` in Step 5); each confirmation is the user's.

`git branch -d` compares ancestry with HEAD or the branch's upstream, not with the squash commit, so it cannot tell whether the branch's work reached main; the SHA check against the merged PR head is what makes `-D` safe. If the SHAs differ, the branch has commits that are not in the merged PR: the blocks show them and leave the branch. Never delete a branch whose PR is not merged.

## Step 8: Tell the other lead

If this session runs with `AIOS_TEAM`, run ListAgents. If the other team's lead is listed under its team name, send it:

```text
From <this team>: MAIN-MOVED <new main sha> #<number>
<output of: git -C "<main-checkout>" diff --name-only <new main sha>^ <new main sha>>
expected conflicts: <owned branches of the receiver that change the same files, if this session knows them; else "check yours">
<"harness changed: restart after your current step" when .claude/settings.json, .claude/hooks/, .claude/agents/ or .claude/rules/ changed>
<the toolchain line from Step 6>
```

In a solo session, report the same lines to the user, who passes them on.

## Step 9: Report

Print a summary of what was done:

- PR number and URL, and the squash commit: `git -C "<main-checkout>" log -1 --oneline "$(gh pr view <number> --json mergeCommit --jq .mergeCommit.oid)"`
- Soak results copied (each `target/soak/pr<number>-<run>`), or that there were none
- Whether the worktree was removed (and its path), or that the PR branch was in the main checkout and no worktree was removed
- Whether the local and remote branches were deleted
- Current HEAD on main (`git -C "<main-checkout>" log -1 --oneline`), which is newer than the squash commit when other PRs merged since
