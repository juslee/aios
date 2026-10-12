---
name: pause
description: >
  AIOS checkpoint before a break or /clear: writes this session's handoff to
  .remember/handoff-<key>.md in the main checkout, then
  `just checkpoint` (the aios binary's checkpoint subcommand) commits work in
  progress on the current claude/* branch, pushes it, and says when it is safe
  to /clear. User-invoked only.
disable-model-invocation: true
---

# /justin:pause

Checkpoint the session so the user can `/clear` (context limit) or stop (usage limit) without losing work. Runbook for the human side: `docs/project/agent-loop.md`.

1. **Key.** Read it with `printf '%s %s\n' "${AIOS_TEAM:-}" "${AIOS_SESSION:-}"`.
   - A lead: the key is `$AIOS_TEAM` (`team-build` or `team-fix`).
   - Any other session: the key is `solo-$AIOS_SESSION`.
   - `AIOS_SESSION` unset: ask the user for a short name for this session, and use `solo-<name>`. Say that launching with `AIOS_SESSION=<name>` saves the question next time.
   - The name must match `[a-z0-9][a-z0-9-]{0,31}`, must not start with `team-`, and must not be `main`, `user` or `system`. Otherwise ask again.

   Only this session writes its file, so neither the two leads nor concurrent solo sessions overwrite each other.

2. **Writers first (team mode).** If background writers are running, wait for them to report, or stop them with TaskStop. Each works in its own temporary worktree, `.claude/worktrees/agent-<id>` on branch `worktree-agent-<id>` (rule 11, Placement). For a writer that reported a committed range, run `/justin:team`'s Placement now if there is time (review, fast-forward, clean up, push). Otherwise leave it: a stopped writer's edits and an unmerged range stay in its temporary worktree, step 3 records them, and step 4 lists them. Temporary branches are never pushed.

3. **Handoff.** Write `<main checkout>/.remember/handoff-<key>.md` with the Write tool. The main checkout is the parent of `git rev-parse --path-format=absolute --git-common-dir`. Replace the whole file. It is gitignored and never committed. Write it in the first person, with these sections:
   - **Owned branches:** per branch: worktree path, last sha, PR and label, next step, what it waits on, and the files outside the team's domain it changes.
   - **Pending ranges:** per temporary worktree not yet fast-forwarded: its path and `worktree-agent-<id>` branch, the branch worktree and tip it started from, the task, and where it stands (uncommitted, committed and unreviewed, in review round N with the open findings, or refused).
   - **Decisions pending:** `needs-human` issues and questions asked in this session but not answered.
   - **Merge order:** this team's PRs in the order it proposes, and the gate issues they wait on.
   - **QEMU:** the output of `bash scripts/agent/qemu-lock.sh status`, and planned quiet windows.
   - **Next:** the first thing to do on resume.

   Note whether the write succeeded (`saved` or `not-saved`).

4. **Checkpoint.** Run the checkpoint from the current checkout, passing the handoff result:

   ```bash
   just checkpoint --handoff saved
   ```

   If this checkout predates the `checkpoint` recipe, run the main checkout's justfile, which runs main's shim. `<main checkout>` is the path on the first `worktree` line of `git worktree list --porcelain`. It still acts on the current checkout:

   ```bash
   just --justfile "<main checkout>/justfile" --working-directory . checkpoint --handoff saved
   ```

   The binary does all git work; do not run `git add`, `git commit` or `git push` yourself. It commits and pushes only in the current checkout. It:
   - commits only on a `claude/*` branch, never on `main` or a detached HEAD, and not while a merge, cherry-pick, revert, rebase or bisect is in progress;
   - stages everything not gitignored except `.remember/`. It then stops before committing if a path new to the repo looks like a secret (`.env`, `*.pem`, `*.key`, `id_rsa*`, `*credential*`, `*secret*`, ...), or an added line looks like a private key or access token, in the staged changes or in local commits not yet on origin. On a stop it restores the index exactly. The scan fails closed: if one of its git calls fails, it commits and pushes nothing, restores the index, never prints `Safe to /clear`, and exits 1;
   - commits `wip: checkpoint <branch>` when anything is staged;
   - pushes the branch under its own name (`refs/heads/<branch>`, so a worktree that tracks `origin/main` cannot push to `main`) whenever it has commits not on origin, also when the tree was already clean. Never forced, never `--no-verify`, no pull or retry after a rejection;
   - keeps wip commits local while the branch has an open PR that is ready for review, because each push to it starts CI and the Claude review on unfinished work (mark the PR draft to allow wip pushes). It also keeps them local when `gh` cannot tell whether such a PR exists;
   - lists every other worktree (the main checkout included) that has uncommitted files or commits not on origin, and never touches them: a background agent may be working there;
   - prints `Safe to /clear` only when no worktree holds work that is not on origin.

   A lead runs in the main checkout, on `main`, so it commits nothing there and lists the team's worktrees. For each `claude/*` worktree that holds this team's work, run the `checkpoint it:` command printed under it, verbatim. It has the form `(cd '<worktree>' && just --justfile '<main checkout>/justfile' --working-directory . checkpoint)`, so it works in any worktree, including one branched before the `checkpoint` recipe existed. The listing marks a temporary `agent-*` worktree as "not a claude/* branch: commit or push it by hand": do neither. It is recorded under Pending ranges and resumed by `/justin:team` (Restart); temporary branches are never pushed.

5. **Report.** Print the command's output verbatim. Its first line is `checkpoint: <branch> @ <sha> | wip: ... | pushed: ... | handoff: ...`. Its last line is either `Safe to /clear — run /justin:start after` or `/clear keeps all of it on disk; ...`, after the lines that say what is not saved and where.
   - Exit status 0: add nothing.
   - Exit status 1: uncommitted files or commits not on origin remain, in this checkout (the `wip:` and `pushed:` fields say why) or in the other worktrees it lists. Do not work around it, and do not checkpoint another team's worktree. For this team's `claude/*` worktrees, run the `checkpoint it:` command printed under each.
   - Exit status 3 has two meanings. Tell them apart by where the text is:
     - Empty stdout and an `aios: ` line on stderr: the shim refused to run a stale binary (`aios checkpoint never commits or pushes with a stale binary`). Report the stderr line and tell the user to run `just tools`, then pause again. Never pass `--allow`: nothing was scanned.
     - A `Stopped before committing` block on stdout: the secret stop, described next.
   - The secret stop (exit status 3, `Stopped before committing` on stdout): it stopped on a possible secret and listed each path. Do not delete, gitignore or commit those files yourself. Ask the user one question: which of the listed paths are safe to publish (the repository is public)? For the paths the user confirms by name, rerun the same command with `--allow <path>` once per path, copied exactly as listed. Never pass `--allow` for a path the user did not confirm, and never on a first run. If the user confirms none, stop there.

   On resume, `/justin:start` (solo) or `/justin:team <team>` shows the wip commit in the brief. Continue on top of it. Squash merge removes it from `main`'s history, so never amend or force-push it.
