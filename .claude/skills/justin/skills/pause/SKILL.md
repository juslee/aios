---
name: pause
description: >
  AIOS checkpoint before a break or /clear: saves the .remember handoff, then
  scripts/agent/checkpoint.sh commits work in progress on the current claude/*
  branch, pushes it, and says when it is safe to /clear. User-invoked only.
disable-model-invocation: true
---

# /justin:pause

Checkpoint the session so the user can `/clear` (context limit) or stop (usage limit) without losing work. Runbook for the human side: `docs/project/agent-loop.md`.

1. **Handoff.** Invoke the `remember:remember` skill with the Skill tool to write `.remember/remember.md`. Note whether it succeeded; if the skill is unavailable or fails, continue.

2. **Checkpoint.** Run the checkpoint script from the current checkout, passing the handoff result (`saved` or `not-saved`):

   ```bash
   bash scripts/agent/checkpoint.sh --handoff saved
   ```

   If this checkout predates the script, run the main checkout's copy (it still acts on the current checkout):

   ```bash
   bash "$(git worktree list --porcelain | awk '/^worktree /{print substr($0, 10); exit}')/scripts/agent/checkpoint.sh" --handoff saved
   ```

   The script does all git work; do not run `git add`, `git commit` or `git push` yourself. It:
   - commits only on a `claude/*` branch, never on `main` or a detached HEAD, and not while a merge, cherry-pick, revert, rebase or bisect is in progress;
   - stages everything not gitignored except `.remember/`, then stops before committing if a path new to the repo looks like a secret (`.env`, `*.pem`, `*.key`, `id_rsa*`, `*credential*`, `*secret*`, ...) or an added line looks like a private key or access token, in the staged changes or in local commits not yet on origin; on a stop it restores the index exactly;
   - commits `wip: checkpoint <branch>` when anything is staged;
   - pushes the branch under its own name (`refs/heads/<branch>`, so a worktree that tracks `origin/main` cannot push to `main`) whenever it has commits not on origin, also when the tree was already clean; never forced, never `--no-verify`, no pull or retry after a rejection;
   - keeps wip commits local while the branch has an open PR that is ready for review, because each push to it starts CI and the Claude review on unfinished work (mark the PR draft to allow wip pushes).

3. **Report.** Print the script's output verbatim. Its first line is `checkpoint: <branch> @ <sha> | wip: ... | pushed: ... | handoff: ...`; its last line is either `Safe to /clear — run /justin:start after` or says what remains only in this checkout.
   - Exit status 0 or 1: add nothing. Status 1 means uncommitted files or commits not on origin remain; the `wip:` and `pushed:` fields say why. Do not work around it.
   - Exit status 3: it stopped on a possible secret and listed the paths. Add one question asking the user how to proceed; do not delete, gitignore or commit those files yourself.

   On resume, `/justin:start` shows the wip commit in the brief. Continue on top of it; squash merge removes it from `main`'s history, so never amend or force-push it.
