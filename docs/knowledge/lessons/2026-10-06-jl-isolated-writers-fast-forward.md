---
author: jl + claude
date: 2026-10-06
tags: [tooling]
status: final
---

# Lesson: Isolated writers commit in their own worktree; the lead fast-forwards

## What happened

The two-team harness plan (`docs/knowledge/plans/2026-10-06-jl-two-team-harness.md`, decision D3) first placed writers like this: spawn the writer with `isolation: "worktree"`, and have it call `EnterWorktree({path: <W>})` to pin itself to the branch worktree `W`. Before any task relied on that, T2 checked it headless with `claude -p` in a scratch repository, on 2026-10-09 with Claude Code 2.1.292.

- **H3 failed.** An isolated subagent's `EnterWorktree` into the branch worktree was refused: "would not give this agent write access there ... its writes are limited to that folder". The isolated agent can write only its own temporary worktree, `.claude/worktrees/agent-<id>` on branch `worktree-agent-<id>`.
- **Run A2a.** A non-isolated subagent's `EnterWorktree` was refused too: "only available to sessions whose working directory is inside a worktree". The probe agent then went on and committed on `main` in the repository root. That is the unsafe default when an agent runs without isolation.
- **Run A2b.** An isolated subagent ran `git switch <branch>` in its own temporary worktree (the branch was checked out nowhere else). That worked and the commits landed on the branch. The temporary worktree and its `worktree-agent-*` branch stayed behind, because they held commits.
- **Run A3** (isolated + fast-forward) worked for both a foreground and a background agent. The owner made it D3 (2026-10-09 22:00).
- **H1.** Claude Code removes an agent's temporary worktree by itself only when the agent left it unchanged. A temporary worktree whose agent committed is not removed automatically.
- **H2.** An isolated agent's hook payload carries `agent_id` and `agent_type`, and its `cwd` is its own `.claude/worktrees/agent-<id>`. `$CLAUDE_PROJECT_DIR` stays the repository root; it does not follow the agent.

The first live use in this repository was T2 Steps 3 and 4 on `claude/harness-teams` (one writer spawn, 2026-10-10). The controller's placement commands, run from the main checkout, printed:

```text
$ git -C "$W" rev-list --first-parent --count adb77da..worktree-agent-a77f7be750f795ed5   -> 2
oldest first-parent commit's parent == adb77da -> yes
$ git -C "$W" merge --ff-only worktree-agent-a77f7be750f795ed5   -> fast-forward to 39ddd69
$ git worktree remove .claude/worktrees/agent-a77f7be750f795ed5   -> ok
$ git -C "$W" branch -d worktree-agent-a77f7be750f795ed5   -> Deleted branch worktree-agent-a77f7be750f795ed5 (was 39ddd69).
$ git -C "$W" push origin claude/harness-teams   -> ok
```

The writer's `git reset --hard <tip>` ran without a permission prompt in that session (auto mode).

A kernel-dev note on worktree isolation (2026-09-22, issue #171) had described the same confinement from the other side. It said git commands that `cd` into another worktree are refused, and told kernel-dev to push its own `HEAD` with `git push -u origin HEAD:claude/<branch>`. Part of it still holds; the workaround does not (see below).

## Why it happened

- **Isolation confines the agent, and EnterWorktree does not lift it.** On 2.1.292 an isolated agent's writes are limited to its own temporary worktree. `EnterWorktree` is refused for any other path, and is refused to a non-isolated subagent because its working directory is not inside a worktree. D3 as first written was not available.
- **The confinement covers Bash commands that Claude Code cannot show stay inside the worktree.** Plain git commands in the agent's own worktree run, including `git reset --hard <tip>` (A3) and `git switch` (A2b). This lesson's writer (2026-10-10, 2.1.292) had two commands refused by the isolation check, not by a permission rule:
  - a compound command that set a shell variable to a path under `.git/` and read files through it: "this command names git in a form too complex to verify that it stays inside the worktree. ... Split it into plain, separate commands";
  - a `sed -i` whose file argument was an unquoted shell variable: "runs sed with a value computed at runtime (the variable P) where an option may stand ... so what it runs cannot be shown not to be git".

  That is what the memory note saw as "compound shell commands are refused".
- **`git branch -d` checks against the current `HEAD`.** It deletes a branch only if that branch is merged into `HEAD`. In the main checkout `HEAD` is `main`, which does not contain the writer's commits, so `-d` refuses ("not fully merged"). In `W`, after the fast-forward, `HEAD` contains them, so `git -C <W> branch -d` deletes the branch.
- **Clean-up follows "unchanged".** Claude Code removes a temporary worktree only when the agent changed nothing. Any commit keeps it, so the lead must remove it.

## What we learned

The placement that works on 2.1.292 (D3, "isolated + fast-forward"):

1. The lead records `W`'s tip (`git -C <W> rev-parse HEAD`) right before the spawn and puts it in the prompt.
2. The writer is spawned with `isolation: "worktree"`. It never calls EnterWorktree, never `cd`s or `git -C`s into `W`, and never pushes.
3. Its first commands check that it is in a temporary worktree: `git rev-parse --show-toplevel` ends in `/.claude/worktrees/agent-<id>` and `git branch --show-current` is `worktree-agent-<id>`. Then it runs `git reset --hard <tip>` and checks `git rev-parse HEAD` equals the tip.
4. It works, runs its gates in its own worktree, and commits on `worktree-agent-<id>`.
5. The lead checks the range from the main checkout. `git -C <W> rev-list --first-parent <tip>..worktree-agent-<id>` must list exactly the reported commits, and the oldest one's first parent must be `<tip>`. A merge of `origin/main` is one first-parent commit.
6. The lead runs `git -C <W> merge --ff-only worktree-agent-<id>`. If it is refused, nothing merges: a fresh writer redoes the work from the new tip.
7. The lead runs `git worktree remove .claude/worktrees/agent-<id>` (never `--force`), then `git -C <W> branch -d worktree-agent-<id>`, then pushes from `W` with an explicit refspec.

Two more facts:

- **The memory note's push workaround is retired.** Agents commit and leads push, so no agent pushes `HEAD:claude/<branch>`. Its other advice stands in a new form: keep each git command plain and aimed at the agent's own worktree.
- **`CARGO_TARGET_DIR` stays per worktree.** Never point a writer at `W`'s `target/`:
  - the justfile reads `target/` relative to its checkout (`kernel_elf`, `stub_efi`, `--target-dir target/tools`, `target/tools/installed`), so a redirected build leaves `just disk` and `just soak` reading stale or missing artifacts;
  - `W`'s kernel ELF and stamped `aios` must describe `W`'s head, not an unreviewed or refused range;
  - concurrent builds would serialize on one cargo lock.

  The price is a cold build per spawn.

## How to avoid next time

- **Spawn writers only with `isolation: "worktree"`.** A non-isolated writer whose EnterWorktree is refused may carry on in the main checkout and commit on `main` (A2a).
- **Give the writer the tip, and check the range before merging.** `merge --ff-only` is the only way a writer's commits reach `W`; a refusal merges nothing.
- **Run `branch -d` with `-C <W>`, after the fast-forward.** From the main checkout it refuses.
- **Remove every committed temporary worktree.** Claude Code leaves it behind. A refused range keeps its worktree until the redo lands.
- **Keep isolated agents' commands plain.** One git command per call, aimed at the agent's own worktree; spell paths out, or double-quote a variable where an option could stand (or put `--` before it); use the Read and Edit tools for file edits.
- **Re-check after each Claude Code update** (checked on Claude Code 2.1.292). The procedure is `probe/HEADLESS.md.v5`, section "T2", in the design folder `$(git rev-parse --git-common-dir)/aios-agent/teams-design-v3` (local to the owner's clone, not in the repository). Run from the main checkout:

  ```sh
  MAIN="$(cd "$(git rev-parse --path-format=absolute --git-common-dir)/.." && pwd)"
  D="$(git rev-parse --path-format=absolute --git-common-dir)/aios-agent/teams-design-v3"
  claude --version
  S=$(mktemp -d); sh "$D/probe/setup.sh.v5" "$S"
  R=$S/repo; WT=$R/.claude/worktrees/probe; L=$R/.git/probe-hooks.jsonl
  BASE="env -u CLAUDE_CODE_EFFORT_LEVEL -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT AIOS_PROBE_BIN=$MAIN/target/tools/installed/aios AIOS_HOOK_STATE_DIR=$S/hookstate"
  TOOLS="Bash,Read,Write,Edit,Grep,Glob,Agent,ToolSearch"
  cd "$R" && $BASE claude -p --model opus --output-format json --allowedTools "$TOOLS" \
    --debug-file "$S/run-a.debug" "$(sed -e "s#@WT@#$WT#g" -e "s#@R@#$R#g" -e "s#@LOG@#$L#g" "$D/probe/run-a.v5.txt")" >"$S/run-a.json"
  ```

  `D` is made absolute because the run `cd`s into the scratch repository. `S` is a fresh `<scratch>` directory outside every git repository. Then the checks:

  | # | Command | Pass |
  | --- | --- | --- |
  | H1 | `git -C "$R" worktree list`; `git -C "$R" branch --list 'worktree-*' 'agent-*'` | only the `repo` and `probe` worktrees remain; no temporary branches. A committed temporary worktree is gone only because run A's lead removed it. |
  | H2 | `jq -c 'select(.agent_type=="probe-writer" and .hook_event_name=="PreToolUse") \| {tool_name, agent_id, agent_type, cwd, _project_dir}' "$L"` | every record has `agent_id` and `agent_type`; `cwd` is the agent's own `.../.claude/worktrees/agent-<id>`; `_project_dir` is the repository root. |
  | H3 | run A's output: each agent's `git branch --show-current` and reset output; `git -C "$WT" log --format=%s main..`; `cat "$WT/probe-bg.txt"`; run A's step 3 and 6 outputs | each agent ran on `worktree-agent-<id>` and reset to the tip; each `rev-list --first-parent` printed one commit; both `merge --ff-only` fast-forwarded; `claude/probe` holds `probe-bg` and `probe-fg`; `probe-bg.txt` is `bg`; `git branch -d` from the repository root refused ("not fully merged") and with `-C "$WT"` deleted the branch. |

  If an agent cannot reset or commit in its own worktree, or the fast-forward is refused, D3 is not available on that CLI: stop relying on it and re-plan before the next writer spawn. Teardown: `rm -rf "$S"`.
