---
author: jl + claude
date: 2026-10-06
tags: [tooling]
status: final
---

# Lesson: Check model-routing wiring headless before relying on it

## What happened

The model-routing ADR (`docs/knowledge/decisions/2026-10-06-jl-model-routing-hooks.md`) requires a real-run check of each Claude Code mechanism before the harness relies on it. T2 of the two-team harness PR (#234) ran those checks headless with `claude -p` in a scratch repository, on 2026-10-09 with Claude Code 2.1.292. The results for the routing wiring:

| # | Check | Result |
| --- | --- | --- |
| H4 | `SubagentStop` fires for foreground and background subagents | pass: records for the foreground probe-writer (including one with `stop_hook_active` true) and the background probe-kdev |
| H5 | `route-outcome`'s `launched` to `stopped` join | pass: identical `agent_id` values in `launched` and `stopped`, for the foreground (`completed`) and the background (`async_launched`) dispatch |
| H6 | Per-agent effort and the lead's | pass: `main high`, `probe-writer low`, `probe-kdev medium` |
| H7 | Agent hooks (first: a `SubagentStop` block; redefined: the model id) | the block was discarded; the alias `fable` 404s; `claude-fable-5-1` runs |
| H8 | The ExitPlanMode `PreToolUse` input | no record: ExitPlanMode is not reached in `-p` |

H7 found two things the design had assumed wrongly:

- **A `SubagentStop` agent-hook block is discarded for subagents.** The hook ran (Grep, StructuredOutput), and the debug log shows `Agent hook condition was not met: PROBE-GATE-BLOCK ...` followed by `[end-turn] Stop hook block discarded (turn ended by tool result, no model re-invoke)`.
- **Agent hooks send `model` verbatim.** The hook with `model: "fable"` got `404 not_found_error "model: fable"` and "Agent hook did not return structured output". The hook with `model: "claude-fable-5-1"` ran, and its condition was met.

Probe 2's hooks returned ok true, so it showed only that the full id runs, not that a block holds. A third probe (probe 3, 2026-10-10, same CLI) did show it: a `PreToolUse` agent hook on Bash with `model: "claude-fable-5-1"`, told to always return ok false, denied the tool call. Its debug log has `dispatching to firstParty model=claude-fable-5-1`, `Hooks: Got structured output: {"ok":false,"reason":"PROBE-PRETOOL-BLOCK do not run this command."}`, `Agent hook condition was not met: ...`, `Hook denied tool use for Bash` and `Bash tool permission denied`, and the command's output file `ran.txt` was never created.

## Why it happened

- **A subagent ends with a hand-back tool call.** Its turn ends on a tool result, so Claude Code has no model turn to re-invoke with the block, and drops it. Only `SubagentStop` was probed. An agent's frontmatter `Stop` hook maps to the same event and is expected to fail the same way (not probed; frontmatter hooks do not run in `-p`).
- **Agent hooks do not resolve aliases.** The `model` field of a `type: agent` hook goes to the API as written. Agent frontmatter `model: fable` (and the Agent tool's `model`) is a separate path, which T2 did not check; T16's E8 checks it.
- **`-p` never reaches ExitPlanMode.** A headless run cannot show the plan-approval flow, so the plan gate's input can only be checked interactively (OWNER-PROBES OP2).
- **`CLAUDE_CODE_EFFORT_LEVEL` outranks every agent's `effort`.** H6 only reads per-agent values with the variable unset.

## What we learned

- **No Fable gate can be a `SubagentStop` hook** on 2.1.292, and none is built on a frontmatter `Stop` hook either: it maps to the same event and is expected to fail the same way (not probed; frontmatter hooks do not run in `-p`). The kernel-dev step review is lead-run instead: before fast-forwarding a kernel-dev range, the lead spawns code-reviewer (Fable) read-only on the range, and must-fix findings go to a fresh kernel-dev (owner decision, 2026-10-09 22:04; rule 11, Reviews).
- **Every `type: agent` hook names the full model id `claude-fable-5-1`**, never the alias. Agent frontmatter keeps `model: fable`, and T16 E8 checks that it runs on Fable.
- **A `PreToolUse` agent hook on `claude-fable-5-1` that returns ok false denies the tool call** (probe 3, shown for Bash), so the ExitPlanMode plan gate stays a hook. ExitPlanMode itself is still unchecked: `-p` never reaches it, so OWNER-PROBES OP2 checks the gate.
- **`SubagentStop` and the `route-outcome` join work for background agents too,** so `route-outcome` needs no change (H4, H5).
- **The scratch project's `effortLevel: "high"` (the aios `.claude/settings.json` has it too) reaches the lead and frontmatter `effort` reaches each agent,** once `CLAUDE_CODE_EFFORT_LEVEL` is gone (H6).

## How to avoid next time

- **Run the probe before relying on a hook mechanism,** and read the debug log, not only the transcript: a discarded block and a 404 leave no transcript trace.
- **Use full model ids in agent hooks.** If the full id ever 404s, find the current Fable id in the CLI's model list and change the hooks before relying on them.
- **Re-check after each Claude Code update** (checked on Claude Code 2.1.292). The procedure is `probe/HEADLESS.md.v5`, section "T2", in the design folder `$(git rev-parse --git-common-dir)/aios-agent/teams-design-v3` (local to the owner's clone, not in the repository). Run from the main checkout:

  ```sh
  MAIN="$(cd "$(git rev-parse --path-format=absolute --git-common-dir)/.." && pwd)"
  D="$(git rev-parse --path-format=absolute --git-common-dir)/aios-agent/teams-design-v3"
  claude --version
  S=$(mktemp -d); sh "$D/probe/setup.sh.v5" "$S"
  R=$S/repo; WT=$R/.claude/worktrees/probe; L=$R/.git/probe-hooks.jsonl
  BASE="env -u CLAUDE_CODE_EFFORT_LEVEL -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT AIOS_PROBE_BIN=$MAIN/target/tools/installed/aios AIOS_HOOK_STATE_DIR=$S/hookstate"
  TOOLS="Bash,Read,Write,Edit,Grep,Glob,Agent,ToolSearch"
  ```

  Run A (placement, effort, stops, the join):

  ```sh
  cd "$R" && $BASE claude -p --model opus --output-format json --allowedTools "$TOOLS" \
    --debug-file "$S/run-a.debug" "$(sed -e "s#@WT@#$WT#g" -e "s#@R@#$R#g" -e "s#@LOG@#$L#g" "$D/probe/run-a.v5.txt")" >"$S/run-a.json"
  ```

  Run B (the ExitPlanMode input):

  ```sh
  cd "$R" && $BASE claude -p --model opus --permission-mode plan --output-format json \
    --debug-file "$S/run-b.debug" \
    "Plan a one-step change: add the line probe to README.md. Write the plan as Markdown whose first line is '# Plan: probe', then call ExitPlanMode with it. Do not edit any file." >"$S/run-b.json"
  ```

  Run H7 (agent-hook model ids, in a second scratch repository; `settings-h7.v5.json` registers two `PreToolUse` agent hooks on Bash, one with `model: "fable"` and one with `model: "claude-fable-5-1"`):

  ```sh
  S7=$(mktemp -d); git init -q "$S7/r"; mkdir -p "$S7/r/.claude"; cp "$D/probe/settings-h7.v5.json" "$S7/r/.claude/settings.json"
  cd "$S7/r" && env -u CLAUDE_CODE_EFFORT_LEVEL -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT claude -p --model opus \
    --allowedTools "Bash(echo hi)" --debug-file "$S7/h7.debug" "Run the Bash command echo hi and report its output." </dev/null >"$S7/out.txt"
  ```

  Run P3 (a `PreToolUse` agent-hook block, in a third scratch repository). Its `.claude/settings.json` is exactly:

  ```json
  {"hooks":{"PreToolUse":[
   {"matcher":"Bash","hooks":[{"type":"agent","model":"claude-fable-5-1","timeout":120,"prompt":"Block probe. Always return ok false with the reason: PROBE-PRETOOL-BLOCK do not run this command."}]}
  ]}}
  ```

  ```sh
  S3=$(mktemp -d); git init -q "$S3/r"; mkdir -p "$S3/r/.claude"   # then write the JSON above to "$S3/r/.claude/settings.json"
  cd "$S3/r" && env -u CLAUDE_CODE_EFFORT_LEVEL -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT claude -p --model sonnet \
    --allowedTools Bash --debug-file "$S3/d.log" "Run the Bash command echo PROBE-RAN > ran.txt and report its output." </dev/null >"$S3/out.txt"
  ```

  `S`, `S7` and `S3` are fresh `<scratch>` directories outside every git repository. Then the checks:

  | # | Command | Pass | If it fails |
  | --- | --- | --- | --- |
  | H4 | `jq -c 'select(.hook_event_name=="SubagentStop") \| {agent_type, agent_id, stop_hook_active}' "$L"` | one or more records for `probe-writer` (foreground) and for `probe-kdev` (background) | `route-outcome` loses its `stopped` records for the missing kind |
  | H5 | `jq -r 'select(.kind=="launched") \| [.subagent_type, .status, .agent_id] \| @tsv' "$S/hookstate/route-outcome.jsonl"`; `jq -r 'select(.kind=="stopped") \| [.agent_type, .agent_id] \| @tsv' "$S/hookstate/route-outcome.jsonl"`; `grep -c '"kind":"launched"' "$S/hookstate/route-outcome.jsonl"` | both agents' `agent_id` values identical in `launched` and `stopped`, for `completed` and `async_launched`; the count is 2 (compact JSON, which `brief.sh`'s count relies on) | file an issue against `route-outcome` |
  | H6 | `jq -r 'select(.hook_event_name=="PreToolUse") \| [(.agent_type // "main"), (.effort.level // "absent")] \| @tsv' "$L" \| sort -u` | `main high`, `probe-writer low`, `probe-kdev medium` | `main xhigh`: the owner's user-level `effortLevel` applied, so remove it; `absent` everywhere: record it |
  | H7 | `grep -c 'not_found_error.*model: fable' "$S7/h7.debug"`; `grep -c 'dispatching to firstParty model=claude-fable-5-1' "$S7/h7.debug"`; `grep -c 'Agent hook condition was met' "$S7/h7.debug"` | 1 or more each | the alias starting to work changes nothing (the full id still works); a 404 on the full id means finding the current Fable id and changing every agent hook |
  | P3 | `grep -c 'Hook denied tool use' "$S3/d.log"`; `grep -c 'dispatching to firstParty model=claude-fable-5-1' "$S3/d.log"`; `test -e "$S3/r/ran.txt" && echo ran \|\| echo not-run` | 1 or more each; `not-run` | the plan gate cannot block: it is a design change for the owner before the harness relies on it |
  | H8 | `jq -c 'select(.tool_name=="ExitPlanMode") \| {keys: (.tool_input \| keys), head: ((.tool_input.plan // "") \| .[0:13]), planFilePath: .tool_input.planFilePath}' "$L"` | keys include `plan` and `planFilePath`, `head` is `# Plan: probe`; no record means `-p` still does not reach ExitPlanMode | no record: check the plan gate interactively (OWNER-PROBES OP2) |

  The `SubagentStop` block is not re-checked: no design relies on it. If a later CLI does re-invoke a subagent after such a block, a hook-based step gate becomes possible again, which is a design change for the owner. Teardown: `rm -rf "$S" "$S7" "$S3"`.

## Update 2026-10-10: end-to-end checks (T16)

T16 ran the harness end to end with `claude -p` (CLI 2.1.292, `W` at 0b52a6a): run E (26 turns) exercised a worker, a kernel-dev and a code-reviewer under the project hooks; run G reran the part the first attempt lost. E6 is the owner's interactive OWNER-PROBES OP2. Results:

| # | Check | Result |
| --- | --- | --- |
| E1 | A worker's `kernel/` Write is denied; its `docs/` commit reaches `W` by fast-forward | pass: path-guard denied it 5 times ("which this agent may not edit"); the main thread's `kernel/` write (control) was allowed |
| E2 | Guard rule 3 (spawn shape) | pass: 4 hits (named spawn without isolation, writer without isolation, `model` passed) |
| E3 | `repeat-error` | pass: the state file was written and "has failed 2 times" appeared 3 times in the transcript |
| E4 | `route-shadow`, `route-outcome` | pass: 7 `route-shadow` records, one per Agent call (`kind` null: no TYPESAFE result), `launched`/`stopped` pairs for all four agent types; the 3 denied spawns left shadow records only |
| E5 | The lead-run Fable range review | pass: code-reviewer launched after kernel-dev, round 1 found 0 must/should/nit; no new worktrees or temporary branches; 0 agent hooks processed |
| E6 | The ExitPlanMode Fable plan gate (OP2, interactive) | pass: see below |
| E7 | A stale `aios` binary (main's, without `--agent-type`) | pass: the shim fallback denied the worker's `kernel/` Write ("path-guard could not run", 5 hits); the lead's docs write was allowed |
| E8 | Agent frontmatter `model: fable` | pass: 3 dispatches to `claude-fable-5-1`, 0 `not_found_error` (the same run also dispatched opus 38, sonnet 5, haiku 1 times) |
| E9 | Guard rule 4 messages | pass: "agents never call EnterWorktree" 5, "agents reset only their own temporary worktree" 5, "exists only for an isolated agent" 3 |

E6, the owner's run, debug counts: `dispatching to claude-fable-5-1` 6, `not_found_error` 0, `planFilePath` 2, "Processing agent hook" 2, "Hook denied" 1. The first ExitPlanMode was blocked ("FABLE-PLAN-GATE block 1/2: Lens: plan ...": the acceptance "it looks right" was not a command); the session rewrote the criterion as a command, the second pass approved it, and the approval dialog followed. The sub-check that a non-working plan ("# Notes: OP2") passes the gate unreviewed was not exercised.

Non-obvious findings:

- **`--add-dir` is variadic in `claude -p`, like `--allowedTools`.** It swallows a following prompt as another directory (run G's first attempt failed this way). Put `--` before the prompt.
- **A standalone `sleep` is blocked in `-p`** (`sleep 30` was refused). Wait on a condition instead.
- **Agent frontmatter `model: fable` resolves to `claude-fable-5-1` (E8),** unlike in an agent hook, where `fable` 404s (H7). The two paths are separate; T2's open question is closed.
- **The ExitPlanMode Fable gate works interactively (E6).** `-p` still never reaches it, so only an owner-run session shows it. A block is retried by the session: it fixed the plan and the second pass approved.
- **A lead's own Writes are fenced by no hook, by design.** path-guard covers the worker only; the lead's `kernel/` or `docs/` writes are not denied (E1's control, E7's lead write), which rule 11 states.
- **The shim fallback also fences a worker on a stale binary (E7).** With no `--agent-type` support, "path-guard could not run" denied the worker's `kernel/` Write and nothing else.

### Re-running the T16 checks

agent-loop's "Re-check after each Claude Code update" re-runs T16 as well as T2. The full procedure is `probe/HEADLESS.md.v5`, section "T16", in the same design folder as T2's; the run E prompt is `probe/run-e.v5.txt` there. That prompt is 14 steps: a worker's write and its fast-forward, a main-thread `kernel/` write, guard rule 3 and 4 probes, a repeated failing command, and a kernel-dev step with the lead's Fable review of its range. HEADLESS ran it from the harness PR's branch worktree; once the harness is on `main`, run it from the main checkout:

```sh
MAIN="$(cd "$(git rev-parse --path-format=absolute --git-common-dir)/.." && pwd)"
D="$(git rev-parse --path-format=absolute --git-common-dir)/aios-agent/teams-design-v3"
W=$MAIN   # the checkout whose harness is under test
claude --version
(cd "$W" && just tools)
git -C "$MAIN" worktree add .claude/worktrees/harness-e2e -b claude/harness-e2e-scratch "$(git -C "$W" rev-parse HEAD)"
W2=$MAIN/.claude/worktrees/harness-e2e; S=$(mktemp -d)
git -C "$MAIN" worktree list --porcelain >"$S/worktrees-before.txt"
E2E="env -u CLAUDE_CODE_EFFORT_LEVEL -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT AIOS_TOOLS_BIN=$W/target/tools/installed/aios AIOS_HOOK_STATE_DIR=$S/hooks"
TOOLS_E="Bash,Read,Write,Edit,Grep,Glob,LSP,Agent,EnterWorktree,ToolSearch"
```

Run E (the harness end to end):

```sh
cd "$W" && $E2E claude -p --model opus --output-format json --allowedTools "$TOOLS_E" --add-dir "$W2" --debug-file "$S/run-e.debug" \
  -- "$(sed -e "s#@W2@#$W2#g" "$D/probe/run-e.v5.txt")" >"$S/run-e.json"
```

Run G (the shim fallback) needs `OLD`, an `aios` binary whose `path-guard` has no `--agent-type`. `main`'s binary has the flag once the harness merges, so build `OLD` from a commit before the harness PR, such as `ac6d26e` (its pinned toolchain must already be installed: `RUSTUP_AUTO_INSTALL=0`):

```sh
O=$(mktemp -d)/old; git -C "$MAIN" worktree add --detach "$O" ac6d26e
(cd "$O" && just tools); OLD=$O/target/tools/installed/aios
TIPG=$(git -C "$W2" rev-parse HEAD)
cd "$W" && env -u CLAUDE_CODE_EFFORT_LEVEL -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT AIOS_TOOLS_BIN="$OLD" \
  AIOS_HOOK_STATE_DIR=$S/hooks-g claude -p --model opus --output-format json --allowedTools "$TOOLS_E" --add-dir "$W2" -- \
  "Agent tool: subagent_type worker, isolation worktree, run_in_background false, prompt: 'W is $W2, branch claude/harness-e2e-scratch, tip $TIPG. Do your preflight, then Write kernel/e2e-g.txt containing probe and report the result verbatim, with your BRANCH line. Commit nothing.' Then Write $W2/docs/e2e-g-main.md yourself and report the result. Then run git worktree list --porcelain and report it." >"$S/run-g.json"
```

The `--` before each prompt is needed because `--add-dir` takes several values. Step 14 of `run-e.v5.txt` ends with `sleep 30`, which `-p` refuses; on 2.1.292 E4's records were complete anyway. If a `stopped` record is missing, suspect `-p` teardown killing the async `route-outcome` hook first (the model-routing ADR, Known limits): change the last step to wait until the `stopped` count in `route-outcome.jsonl` reaches the `launched` count, and run E again.

`T` is run E's transcript, `T=$(ls -t "${CLAUDE_CONFIG_DIR:-$HOME/.claude}"/projects/*/"$(jq -r .session_id "$S/run-e.json")".jsonl)`, and `TG` run G's, the same way from `run-g.json`.

| # | Command | Pass | If it fails |
| --- | --- | --- | --- |
| E1 | `test ! -e "$W2/kernel/e2e-probe.txt" && test -e "$W2/docs/e2e-probe.md" && test -e "$W2/kernel/e2e-main.txt" && echo ok`; `grep -c 'which this agent may not edit' "$T"`; `git -C "$W2" log --format=%s -3` | `ok`; 1 or more; `e2e probe` (or `e2e probe (Fable review fixes)`) above `e2e worker` | `path-guard`'s `settings.json` registration, or the team skill's Placement steps (guard rule 4 too, if Claude Code renamed its `agent-<id>` temporary worktrees: rule 4 then denies the worker's reset) |
| E2 | `grep -c 'becomes a teammate\|spawn it with isolation\|do not pass model' "$T"` | 3 or more | guard rule 3 (`git-push-guard.py`) |
| E3 | `ls "$S/hooks/repeat-error/"`; `grep -c 'has failed 2 times' "$T"` | a state file; 1 or more | the `repeat-error` hook |
| E4 | `wc -l <"$S/hooks/route-shadow.jsonl"`; `jq -r 'select(.kind=="launched" or .kind=="stopped") \| [.kind, .agent_id] \| @tsv' "$S/hooks/route-outcome.jsonl"` | one shadow record per Agent call; every `launched` id has an equal `stopped` id; the denied spawns have shadow records only | `route-shadow` or `route-outcome` |
| E5 | `jq -r 'select(.kind=="launched") \| .subagent_type' "$S/hooks/route-outcome.jsonl" \| sort \| uniq -c`; `git -C "$W2" log -1 --format=%s`; `git -C "$MAIN" worktree list --porcelain \| grep -c 'refs/heads/worktree-agent-'`; `git -C "$MAIN" branch --list 'worktree-agent-*'`; `grep -c 'Processing agent hook' "$S/run-e.debug"` | code-reviewer launched after kernel-dev; last commit `e2e probe` (or `e2e probe (Fable review fixes)`); no temporary worktrees or branches beyond `worktrees-before.txt`; `0` agent hooks | the lead's range review and clean-up (team skill, rule 11) |
| E6 | OWNER-PROBES OP2 (interactive; `-p` never reaches ExitPlanMode) | `result: pass` | the ExitPlanMode plan gate |
| E7 | `grep -c 'path-guard could not run' "$TG"`; `test -e "$W2/docs/e2e-g-main.md" && echo ok`; for each temporary worktree in run G's report, `test ! -e <it>/kernel/e2e-g.txt && echo denied` | 1 or more; `ok`; `denied` | the `.claude/hooks/aios` shim fallback |
| E8 | `grep -o 'dispatching to firstParty model=[^ ]*' "$S/run-e.debug" \| sort \| uniq -c`; `grep -c 'not_found_error' "$S/run-e.debug"` | a `model=claude-fable-…` line; `0` | code-reviewer's frontmatter names `claude-fable-5-1` instead of `fable`; record it in the ADR |
| E9 | `grep -c 'agents never call EnterWorktree' "$T"`; `grep -c 'agents reset only their own temporary worktree' "$T"`; `grep -c 'exists only for an isolated agent' "$T"` | 1 or more each | guard rule 4 (`git-push-guard.py`) |

Teardown: for every temporary worktree not in `worktrees-before.txt`, run `git -C "$MAIN" worktree remove --force <path>` and `git -C "$MAIN" branch -D <worktree-agent-…>`; the guard puts both to the owner. Then run `git -C "$MAIN" worktree remove --force "$W2"`, `git -C "$MAIN" branch -D claude/harness-e2e-scratch`, `git -C "$MAIN" worktree remove --force "$O"` and `rm -rf "$S"`. Record the CLI version the checks ran on.

## Open check: two live leads (OP1)

The harness removed `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS` from the project env, because aios uses no Claude Code teammates (rule 11). The two leads still message each other across sessions. That should keep working, because cross-session messaging has its own gate. The evidence comes from reading the 2.1.285 and 2.1.289 binaries, not from a run. They carry a separate gate message, `[uds-messaging] Skipped: cross-session messaging gate off`, and they have held messages and the `crossSessionInbound` setting, none of which depends on agent teams. A headless `-p` run cannot check this, because it needs two live sessions. The first real check is OWNER-PROBES OP1 (`OWNER-PROBES.md.v5` in the design folder named above). It runs when both leads restart from the main checkout after the harness merges (a post-merge owner step in PR #234's hand-off).

With the project flag gone, OP1 must show:

- Each lead's ListAgents lists the other under its team name (`team-build`, `team-fix`).
- Each lead receives the other's `From <team>: HELLO`, and no "held" or `crossSessionInbound` notice appears.
- A lead resumed with `claude --resume <team>` keeps its name without `/rename`, and its peer receives its new `HELLO`.
- A background worker spawned by a lead runs its first command, `git reset --hard <tip>`, with no permission prompt.

If a message never arrives, run the receiving lead once with `--debug-file` and read the lines that contain `uds-messaging` or `Cross-session messaging`:

- **Gate off:** the messaging gate is off for this account or CLI version. This needs a design decision from the owner. Do not put the agent-teams flag back as a stopgap.
- **Message held:** the two leads are probably in different permission modes. Claude Code holds cross-session messages while the modes differ, which is why rule 11 and agent-loop.md (Teams, Launch) require both leads to use the same mode.

Until OP1 passes, a lead that cannot see its peer in ListAgents works in one-lead mode (agent-loop.md, Teams) and never sends to a guessed name. After OP1 runs, team-build replaces this section with its report in a small docs PR. The report gives the CLI version, `result: pass` or `fail`, what each item above showed, and the debug lines for anything that failed. A failure also gets an issue labelled `team-build`.
