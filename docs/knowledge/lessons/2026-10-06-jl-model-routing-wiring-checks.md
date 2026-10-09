---
author: jl + claude
date: 2026-10-06
tags: [tooling]
status: final
---

# Lesson: Check model-routing wiring headless before relying on it

## What happened

The model-routing ADR (`docs/knowledge/decisions/2026-10-06-jl-model-routing-hooks.md`) requires a real-run check of each Claude Code mechanism before the harness relies on it. T2 of the two-team harness plan ran those checks headless with `claude -p` in a scratch repository, on 2026-10-09 with Claude Code 2.1.292. The results for the routing wiring:

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

A second probe also showed that a `PreToolUse` agent hook does block ("Agent hook condition was met").

## Why it happened

- **A subagent ends with a hand-back tool call.** Its turn ends on a tool result, so Claude Code has no model turn to re-invoke with the block, and drops it. An agent's frontmatter `Stop` hook maps to the same event, so it would fail the same way (frontmatter hooks do not run in `-p`, so T2 could not run it).
- **Agent hooks do not resolve aliases.** The `model` field of a `type: agent` hook goes to the API as written. Agent frontmatter `model: fable` (and the Agent tool's `model`) is a separate path, which T2 did not check; T16's E8 checks it.
- **`-p` never reaches ExitPlanMode.** A headless run cannot show the plan-approval flow, so the plan gate's input can only be checked interactively (OWNER-PROBES OP2).
- **`CLAUDE_CODE_EFFORT_LEVEL` outranks every agent's `effort`.** H6 only reads per-agent values with the variable unset.

## What we learned

- **No Fable gate can be a `SubagentStop` or frontmatter `Stop` hook** on 2.1.292. The kernel-dev step review is lead-run instead: before fast-forwarding a kernel-dev range, the lead spawns code-reviewer (Fable) read-only on the range, and must-fix findings go to a fresh kernel-dev (plan D8, owner 2026-10-09 22:04).
- **Every `type: agent` hook names the full model id `claude-fable-5-1`**, never the alias. Agent frontmatter keeps `model: fable`, and T16 E8 checks that it runs on Fable.
- **A `PreToolUse` agent hook does block,** so the ExitPlanMode plan gate (D9) stays a hook.
- **`SubagentStop` and the `route-outcome` join work for background agents too,** so `route-outcome` needs no change (H4, H5).
- **The project's `effortLevel: "high"` reaches the lead and frontmatter `effort` reaches each agent,** once `CLAUDE_CODE_EFFORT_LEVEL` is gone (H6).

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

  `S` and `S7` are fresh `<scratch>` directories outside every git repository. Then the checks:

  | # | Command | Pass | If it fails |
  | --- | --- | --- | --- |
  | H4 | `jq -c 'select(.hook_event_name=="SubagentStop") \| {agent_type, agent_id, stop_hook_active}' "$L"` | one or more records for `probe-writer` (foreground) and for `probe-kdev` (background) | `route-outcome` loses its `stopped` records for the missing kind |
  | H5 | `jq -r 'select(.kind=="launched") \| [.subagent_type, .status, .agent_id] \| @tsv' "$S/hookstate/route-outcome.jsonl"`; `jq -r 'select(.kind=="stopped") \| [.agent_type, .agent_id] \| @tsv' "$S/hookstate/route-outcome.jsonl"`; `grep -c '"kind":"launched"' "$S/hookstate/route-outcome.jsonl"` | both agents' `agent_id` values identical in `launched` and `stopped`, for `completed` and `async_launched`; the count is 2 (compact JSON, which `brief.sh`'s count relies on) | file an issue against `route-outcome` |
  | H6 | `jq -r 'select(.hook_event_name=="PreToolUse") \| [(.agent_type // "main"), (.effort.level // "absent")] \| @tsv' "$L" \| sort -u` | `main high`, `probe-writer low`, `probe-kdev medium` | `main xhigh`: the owner's user-level `effortLevel` applied, so remove it; `absent` everywhere: record it |
  | H7 | `grep -c 'not_found_error.*model: fable' "$S7/h7.debug"`; `grep -c 'dispatching to firstParty model=claude-fable-5-1' "$S7/h7.debug"`; `grep -c 'Agent hook condition was met' "$S7/h7.debug"` | 1 or more each | the alias starting to work changes nothing (the full id still works); a 404 on the full id means finding the current Fable id and changing every agent hook |
  | H8 | `jq -c 'select(.tool_name=="ExitPlanMode") \| {keys: (.tool_input \| keys), head: ((.tool_input.plan // "") \| .[0:13]), planFilePath: .tool_input.planFilePath}' "$L"` | keys include `plan` and `planFilePath`, `head` is `# Plan: probe`; no record means `-p` still does not reach ExitPlanMode | no record: check the plan gate interactively (OWNER-PROBES OP2) |

  The `SubagentStop` block is not re-checked: no design relies on it. If a later CLI does re-invoke a subagent after such a block, a hook-based step gate becomes possible again, which is a design change for the owner. Teardown: `rm -rf "$S" "$S7"`.
