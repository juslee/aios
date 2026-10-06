---
author: jl + claude
date: 2026-10-06
tags: [tooling]
status: final
---

# ADR: Model routing hooks for the agent team

## Context

The owner wants the agent team to run like this:

- The Opus session leads.
- Sonnet agents do routine work.
- Fable reviews at fixed points: before kernel work is called done, and before an implementation plan is committed.
- Jev (TypeSafe's classifier model) is tried as a router for subagent dispatches.

Claude Code configuration supplies most of this. Each agent definition takes a `model:` and an `effort:`. Hooks can be set in settings or in an agent's frontmatter, and a `type: agent` hook runs a reviewer on a chosen model. What configuration does not supply is four small deterministic programs:

- one that notices a Bash command failing the same way again and says so to the agent;
- one that keeps the Sonnet worker out of kernel, UEFI stub and shared code;
- one that asks Jev how a dispatch would be routed without letting it decide;
- one that logs what happened to each dispatch, so Jev's answers can be judged.

They are the four `aios hook` subcommands in the host binary (`tools/src/cmd/hook/`), built on branch `claude/agent-model-routing` (part 1). The wiring is part 2: agent definitions, settings entries, the shim and the docs. Part 2 is owned by the two-team harness PR. This ADR records the hook design and carries part 2's requirements forward, replacing the working plan they were first written in.

## Options Considered

### Option A: Jev routes dispatches now

A `PreToolUse` hook on the dispatch tool asks Jev and rewrites the dispatch's `model` (through `updatedInput`) or denies it.

- Pros: a model choice per dispatch from day one, using a seam Claude Code already has.
- Cons: nothing shows that Jev can make this call. A wrong route spends Opus on a rename or sends an MMU change to Sonnet, and nothing would record which. Every dispatch would also wait for a network round-trip.

### Option B: path rules in code, Jev in shadow mode only

Where the deciding evidence is a path, a deterministic rule decides: the worker may not edit `kernel/`, `uefi-stub/` or `shared/`. Jev is asked in the background on every dispatch, and its answers are logged next to what the dispatch did. Jev decides nothing until that log has been evaluated.

- Pros: the boundary that matters most is exact and costs no model call. The log produces the evidence that Option A lacks, and no dispatch waits for Jev.
- Cons: two logging hooks to maintain, and an API key. Routing stays fixed per agent definition until the evaluation, which needs 200 or more logged dispatches.

### Option C: no Jev

- Pros: the least code, no API key and no external call.
- Cons: the project never learns whether a cheap classifier can route, which the owner asked to find out.

## Decision

**Option B.** The reasons:

1. **The 2026-09-22 review-triage evaluation** ([research note](../research/2026-09-22-jl-jev-review-comment-triage-eval.md)). Jev (`jev-1.13.0`) was scored on 784 replied Copilot review threads. Its `needs_change` AUC was 0.72, but its Brier score was no better than always predicting the base rate (0.060 against 0.061), and a same-input Sonnet baseline did no better. The push-backs rested on evidence in other files and phase plans, which neither model saw. "Does this task need a stronger model" has the same shape.
2. **The deciding evidence lives in the files a task touches, not in its prompt.** "Tidy the helper" may touch page tables. Where that evidence is a path, a path rule decides it exactly; that is `path-guard`.
3. **Shadow log first.** `route-shadow` and `route-outcome` write two logs that join per dispatch. Jev routes nothing until the evaluation in the part 2 requirements below has compared its answers with outcomes.

## The four subcommands

Common rules (`tools/src/cmd/hook/mod.rs`):

- A hook reads one JSON object from stdin, capped at 16 MiB. It writes at most one JSON object to stdout, puts diagnostics on stderr, and exits 0 on every payload path.
- A clap usage error (a bad registration, never a bad payload) exits 2 before any hook code runs. On `PreToolUse` that blocks the call.
- Parsing is lenient: every field is optional, unknown fields are ignored, and a field of the wrong type reads as absent. Only a payload that is not a JSON object is an input error.
- State lives in `<git common dir>/aios-agent/hooks/`, which every worktree shares. `AIOS_HOOK_STATE_DIR` overrides it (tests).
- A session or agent id becomes part of a file name only if it is made of `[A-Za-z0-9_-]` and is at most 128 bytes long. Any other id becomes a 16-hex-digit FNV-1a digest. A `<session>-<agent>` stem longer than 200 bytes is hashed as a whole, so a file name stays under 255 bytes.
- No new dependencies. HTTP goes through the system `curl`, and the crate still forbids `unsafe`.

| Subcommand | Event (registered in part 2) | Job | On internal error |
| --- | --- | --- | --- |
| `repeat-error` | `PostToolUseFailure` and `PostToolUse`, matcher `Bash` | Notice the same Bash command failing the same way twice, and say so next to the tool result | Fail open: no output |
| `path-guard` | `PreToolUse`, matcher `Edit\|Write\|MultiEdit\|NotebookEdit`, in the `worker` agent's frontmatter only | Deny edits under the `--deny` prefixes (`kernel/`, `uefi-stub/`, `shared/`) | Fail closed: a deny naming the error |
| `route-shadow` | `PreToolUse`, matcher `Agent`, `async: true` | Ask Jev how the dispatch would be routed and log the answer; never decide | Fail open: a failed Jev call is an error record in the log; no state directory or a failed append goes to stderr |
| `route-outcome` | `PostToolUse` and `PostToolUseFailure`, matcher `Agent`, and `SubagentStop`, all `async: true` | Log what happened to each dispatch | Fail open: an `error` record, else stderr |

**`repeat-error`.** A failure's signature is the normalised command plus the normalised error. The command is trimmed with whitespace runs collapsed. In the error, whitespace runs are collapsed, `0x` hex numbers become `0x#` and digit runs of four or more become `#`, and the result is cut to 512 bytes. When a signature's count is 2 or more, each failure adds an `additionalContext` written as statements of fact. Inside a subagent it says the project expects the agent to stop and report back to its caller. In the main session it says the project expects the code-reviewer agent to diagnose the failure before another attempt. A successful run of the same command drops its signatures, and an interrupted failure is ignored. Each session-and-agent file keeps at most 64 signatures, and files untouched for 7 days are deleted on the next write.

**`path-guard`.**

- *Target.* The target is `tool_input.file_path`, or `notebook_path` when that is absent. A checked tool with neither is denied. A relative target is joined to the input `cwd`.
- *Physical resolution.* The path is resolved the way the OS resolves it. Each symlink's target is spliced in before the next component is applied, so a `..` applies to the real directory. One path may pass through at most 40 links, and a `..` after a component that does not exist is denied.
- *Repository root.* The root comes from the target, not from `cwd`. The hook runs `git rev-parse --show-toplevel` from the target's longest existing ancestor, with `LC_ALL=C`. It first removes seven variables from git's environment: `GIT_DIR`, `GIT_WORK_TREE`, `GIT_COMMON_DIR`, `GIT_INDEX_FILE`, `GIT_OBJECT_DIRECTORY`, `GIT_CEILING_DIRECTORIES` and `GIT_DISCOVERY_ACROSS_FILESYSTEM`. The root git reports must be the nearest ancestor that holds a `.git` entry, which stops `core.worktree` or a rewritten gitfile from moving it.
- *Git metadata.* A path with a `.git` component at any depth is denied.
- *No repository.* A target in no repository gets no decision. That takes git's "not a git repository (or any ...)" text with exit 128, and no `.git` entry anywhere above the target. Every other git failure is denied: a pruned worktree, a bare repository, or git missing.
- *Matching.* Prefixes are matched ASCII case-insensitively, because macOS volumes are case-insensitive by default.
- *Bad registration.* A malformed `--deny` value (empty, absolute, or with an empty, `.` or `..` component) denies every checked call.

**`route-shadow`.** `AIOS_ROUTE_SHADOW=off` makes it do nothing. Otherwise it posts the Jev request below and appends one record to `route-shadow.jsonl`. It never prints anything.

- *Transport.* The request goes through `curl -sS --fail-with-body --max-time 4`. The `Authorization` header reaches curl on stdin as a `-K -` config line, so `TYPESAFE_API_KEY` is in no argv and no log. The key is also scrubbed from every error text before that text is cut, and a key with a control character is refused.
- *Request body.* The body is written to a mode-0600 file in the state directory and removed afterwards.
- *Missing key.* A missing key is logged as an error record, so the log shows gaps in coverage.

**`route-outcome`.** It appends `launched`, `launch_failed` and `stopped` records to `route-outcome.jsonl` and never prints anything. It resolves the state directory only for an event that gets a record, so other events cost no `git` call. If an append fails, the failure is appended to the same file as an `error` record (`kind`, `ts`, `event`, `error`). That record starts with a newline, so it ends a partial line instead of extending it. Readers skip blank and non-JSON lines.

## Hook-contract facts relied on

Sources, read and re-verified on 2026-10-05:

- <https://code.claude.com/docs/en/hooks.md>
- <https://code.claude.com/docs/en/sub-agents.md> (the dispatch tool name and frontmatter hooks)
- <https://code.claude.com/docs/en/tools.md> (the tool table, the Bash failure rule and `NotebookEdit`)

"Not documented" means not in those pages.

- **Input.** One JSON object on stdin, with `session_id`, `transcript_path`, `cwd`, `hook_event_name` and others. Inside a subagent, `agent_id` and `agent_type` are added; the main thread has no `agent_id`. Hooks from settings also run inside subagents. Agent-frontmatter hooks fire when that agent runs as a subagent or as the main session through `--agent`.
- **Matchers.** A matcher made only of letters, digits, `_`, `-`, spaces, `,` and `|` is an exact tool name or a list of exact names. Any other matcher is an unanchored regex, so `Edit.*` also matches `NotebookEdit`.
- **Edit tools.** For `Write` and `Edit`, `tool_input.file_path` is always absolute. The field that carries a `NotebookEdit` path is not documented: `notebook_path` is a guess. `MultiEdit` is not in the tool table.
- **Dispatch tool.** The dispatch tool is `Agent` (renamed from `Task` in 2.1.63). Matching `Task` in a hook is not documented. Its `tool_input` has `prompt`, `description`, `subagent_type` and an optional `model` alias.
- **`PostToolUse` for `Agent`.** `tool_response.status` is `completed` (foreground) or `async_launched` (background, the default). Both carry `agentId` and `resolvedModel`. Only `completed` carries `content` and telemetry: `modelsUsed` (an array of strings), and `totalTokens`, `totalDurationMs` and `totalToolUseCount` (numbers).
- **`PostToolUseFailure`.** It does not fire for calls rejected before execution. For a Bash command that ran and exited, `error` starts with `Exit code N`; when the shell itself could not start it is a bare message with no exit-code line. The docs call everything after the first line display text.
  - For `grep`, `rg`, `egrep`, `fgrep`, `find`, `diff`, `test`, `[`, `git diff` and `git grep`, exit 1 is a valid result. For every other command, exit 1 is a failure. Exits above 1 are not addressed; the hooks assume they are failures.
- **`SubagentStop`.** It adds `agent_id`, `agent_type`, `agent_transcript_path`, `last_assistant_message` and `stop_hook_active`.
  - It has no `stop_reason` field; the hooks reference uses that name only for a `-p` process-exit field.
  - Claude Code caps consecutive stop-hook blocks at 8 (`CLAUDE_CODE_STOP_HOOK_BLOCK_CAP`). The count resets whenever the agent calls a tool.
  - Not documented: that it fires for background subagents.
  - Not documented: that its `agent_id` is the same value as the launching call's `tool_response.agentId`.
- **Output and exit codes.** A `PreToolUse` deny is `hookSpecificOutput.permissionDecision: "deny"` with a reason shown to Claude. Across hooks, deny wins over defer, ask and allow.
  - `additionalContext` is capped at 10,000 characters. It should be factual statements, because imperative text can trip Claude's prompt-injection defences.
  - Exit 0 with a JSON object on stdout is the intended path. Only exit 2 or a JSON deny blocks; any other exit is a non-blocking error.
- **Trust and headless sessions.** Project-level agent-frontmatter hooks run only after the folder has been trusted interactively, and are never used in a `claude -p` or SDK session (sub-agents.md; permissions.md, "What runs before you trust a folder"). Hooks from `settings.json` fire for every agent, and `agent_type` is in their input inside a subagent or when the session was launched with `--agent`; `agent_id` alone is subagent-only.
- **Timeouts and async hooks.** A timed-out `PreToolUse` command hook does not block the call. Async hooks cannot block or decide, and their `timeout` is not enforced. Any `additionalContext` or `systemMessage` in their output is still delivered on the next turn.

## Jev's request

`POST https://api.typesafe.ai/v1/systemone` (`AIOS_JEV_URL` overrides it for tests), with `model` pinned to `jev-1.13.0` because thresholds are tuned per version.

- **State.** `subagent_type`, `description`, and `prompt` cut to 8,000 characters.
- **`complexity` (score).** "How much reasoning does carrying out `prompt` need?" The live reply scores it from 0 to 2, not 1 to 3: `score` is in [0, 2], and `legend` and `probabilities` are keyed "0", "1", "2". Analysis must read the levels as 0, 1 and 2:
  - 0: mechanical (a lookup, a search with a clear target, a rename, formatting, wording);
  - 1: routine (a change or check that follows an existing pattern in one area);
  - 2: hard (debugging with an unclear cause, a change across interacting parts, a choice between designs).
- **`low_level_hazard` (noul).** Does the prompt ask for changes to, or diagnosis of, code that involves concurrency, memory ordering, page tables or the MMU, interrupt or exception handling, lock ordering, or boot sequencing?
- **`work_kind` (choice).** One of `read_only_search`, `implement_change`, `review_or_audit`, `run_and_verify`, `write_docs`, `research_external` or `other`.

## Logs and join keys

Both logs are JSON Lines files in the state directory.

- **`route-shadow.jsonl`**, one record per dispatch:
  - `ts` (Unix seconds);
  - `session_id`, `agent_id` and `tool_use_id`;
  - `subagent_type`, the requested `model`, `description`, the `prompt` as sent, `prompt_chars` and `prompt_truncated`;
  - `jev_model`, `latency_ms` and `answers`. `answers` is the reply's object, parsed and re-serialised with key order kept, so `0.50` is logged as `0.5`;
  - `error`, which is null on success. On an error record `jev_model` and `answers` are null, and `latency_ms` is null when no request was made.
- **`route-outcome.jsonl`**:
  - `launched`: `ts`, `session_id`, `caller_agent_id` (only when a subagent dispatched the call), `tool_use_id`, `subagent_type` and `model`. From `tool_response` it takes `agent_id`, `status` and `resolved_model`. It adds `models_used`, `total_tokens`, `total_duration_ms` and `total_tool_use_count` only when the field is present in its documented type, and `models_used` only when it also holds at most 16 names of at most 128 bytes each, so no telemetry value is unbounded.
  - `launch_failed`: `ts`, `session_id`, `caller_agent_id` (when present), `tool_use_id` and `subagent_type`, plus `is_interrupt` and `error` cut to 512 bytes (no `model`).
  - `stopped`: `ts`, `session_id`, `agent_id`, `agent_type`, `stop_hook_active`, `agent_transcript_path`, and `last_assistant_message` cut to 4,000 characters, with `message_chars` and `message_truncated`.
  - `error`: an internal failure, as described above.

The two joins:

- **Shadow to launch:** `route-shadow.tool_use_id` = `launched.tool_use_id`. It is the same field on both sides of one tool call.
- **Launch to stop:** `launched.agent_id` = `stopped.agent_id`. **To be verified.** The docs never say that `tool_response.agentId` and the `SubagentStop` `agent_id` are one value. Both are stored as given, so the join holds only once a real dispatch shows them equal. Until then a missing join means zero *observed* stops, not zero stops.
- **Stop attempts:** the number of `stopped` records for one `agent_id` is the number of stop attempts. It uses only `SubagentStop`'s own field, so it does not depend on the second join.

## Live calls

Four `route-shadow` calls went to Jev with the owner's key and were checked by hand. Two ran on 2026-10-05 at the Step 5 commit (662 ms and 428 ms) and two on 2026-10-06 at the "lead fixes review round 3" commit (446 ms and 428 ms). Both commits predate the rebase onto `main` 1ffac2a; their pre-rebase hashes were e2a7daf and 38af18e. Every record had `jev_model` `jev-1.13.0`, `error` null and all three answers, and the key appeared in no state file. The answers:

| Dispatch | `complexity` | Other answers |
| --- | --- | --- |
| A rename | 0.03 | |
| A lock and MMU diagnosis | 2.0 | `low_level_hazard` 0.99 |
| A feature that follows an existing pattern | 1.0 | |
| A read-only search | 0.11 | `work_kind` `read_only_search` |

## Known limits

- **Bash writes.** Bash can still write files, for example with `sed -i` or a redirection. `path-guard` stops the agent's edit tools; it is a routing aid, not a sandbox.
- **Nested repositories and submodules.** A nested repository or submodule that already exists under a denied prefix moves the root. Its own `.git` makes it the nearest root, so a path inside it is compared relative to that repository and no longer starts with `kernel/`. The `.git` deny stops an edit tool from planting such a root, but not one that is already there.
- **Trust and headless sessions.** `path-guard` is registered only in the `worker` agent's frontmatter, so it runs only in an interactive session whose folder is trusted. In an untrusted folder, and in every `claude -p` or SDK session, a `worker` dispatch has no path guard and nothing denies an edit under `kernel/`, `uefi-stub/` or `shared/`. The guard is interactive-session-only. Registering it in `settings.json` would cover headless runs, but the subcommand has no agent filter, so it would also deny the lead and kernel-dev; that needs a `path-guard` change first (a check of the input's `agent_type` against `worker`), which is not part of this decision.
- **Timeouts.** A timed-out `PreToolUse` hook fails open; Claude Code decides that, not the hook. A stalled `path-guard` therefore allows the edit.
- **`-p` sessions.** Headless sessions kill async hooks still running at teardown. The `route-shadow` and `route-outcome` records for a session's last dispatch may be missing.
- **Untested scrub rows.** Removing `GIT_INDEX_FILE` or `GIT_DISCOVERY_ACROSS_FILESYSTEM` from `path-guard`'s git environment cannot fail a test. `rev-parse --show-toplevel` never reads the index, and a mount boundary cannot be built portably. Both stay in the list as defence in depth. Each of the other five rows makes a test fail when removed.
- **Lost updates.** Two hook processes writing one state file at the same time can lose an update. The only effect is a `repeat-error` nudge arriving one failure late, which was accepted rather than adding file locking.

## Part 2 requirements (owned by the two-team harness PR)

Owned by the two-team harness PR (owner decision 2026-10-05, 21:33). It is built after `claude/harness-team-config` merges and after part 1 (`claude/agent-model-routing`) merges. These are requirements only; the design is that PR's job. The roster is settled: the owner chose the Opus-lead, Sonnet-worker, Fable-reviewer map of the model-routing plan (now this ADR) over the Build/Ship draft's, and the draft's `tools-dev` agent folds into `worker`. Points marked "reported" came from the harness session on 2026-10-05 and are to be checked by that PR.

- The lead is the session itself, not an agent: `team-lead` is a reserved name in Claude Code 2.1.289 (the Agent tool rejects it, as it does `main`, `user` and `system`, and `SendMessage` to `team-lead` reaches the session's own lead; reported). The lead runs Opus at high effort through its launch (`claude --model opus --effort high`) or `model`/`effortLevel` settings, and `.claude/agents/team-lead.md` is deleted. Teams form only in a terminal `claude` session, not the VS Code panel (reported).
- Agent models and effort: kernel-dev `opus`/high; `worker` `sonnet`/high for `tools/`, `scripts/`, `docs/` and `.github/` work, with `path-guard --deny kernel/ --deny uefi-stub/ --deny shared/` as a `PreToolUse` hook in its frontmatter (matcher `Edit|Write|MultiEdit|NotebookEdit`; `MultiEdit` survives only as a permission-rule alias, which is harmless in a matcher; reported). That registration holds only in trusted interactive sessions (see Known limits), so part 2 must either state that headless `worker` runs are unguarded and not dispatch `worker` from `-p` or SDK sessions, or first add an `agent_type` filter to `path-guard` and register it in `settings.json` as well; doc-writer `opus`/high; code-reviewer `fable`/high; verifier and doc-auditor `sonnet`/medium.
- Fable before "done": a `Stop` hook in kernel-dev's frontmatter (documented to run as `SubagentStop` for that agent only), `type: agent`, `model: fable`, reviewing the change and blocking with reasons. `SubagentStop` input carries `stop_hook_active`, and Claude Code caps consecutive continuations at 8 (`CLAUDE_CODE_STOP_HOOK_BLOCK_CAP`), but that count resets whenever the subagent calls a tool, so a review-fix-review cycle is not bounded by it. The gate therefore also caps its own blocks per `agent_id`, for example by counting that agent's `stopped` records in `route-outcome.jsonl`, so it cannot loop. Probe both the frontmatter `Stop` to `SubagentStop` mapping and the cap with a real subagent before relying on them.
- Fable before the plan: in `/implement-phase`, a Fable review of the working plan before the plan commit. A `PreToolUse` hook matched on `ExitPlanMode` runs before Claude leaves plan mode and receives the plan content in its input (hooks.md, ExitPlanMode); whether the gate uses it or a skill step is part 2's choice.
- `settings.json`: `repeat-error` on `PostToolUseFailure` and `PostToolUse` for `Bash`; `route-shadow` on `PreToolUse` for the dispatch tool (matcher `Agent`), registered with `async: true`. The hooks reference documents async command hooks as running in the background: they cannot block or decide (decision fields have no effect), `timeout` is not enforced on them, and any `additionalContext` or `systemMessage` in their JSON output is still delivered on the next turn, so `route-shadow` must keep printing nothing. A dispatch never waits for Jev; curl's 4 s cap remains the bound on the hook's own run time. `-p` (headless) sessions kill async hooks still running at teardown, so the record for a session's final dispatch may be missing from the Jev evaluation.
- `route-outcome` (built in part 1): register it with `async: true` on `PostToolUse` and `PostToolUseFailure` (matcher `Agent`) and on `SubagentStop`. That `SubagentStop` fires for background subagents is not documented: verify it with a real background subagent run, since background is the default dispatch. Verify the join key in the same run, before the Jev evaluation relies on it: compare `tool_response.agentId` (the `launched` record's `agent_id`) with the `SubagentStop` `agent_id` (the `stopped` record's) for one real dispatch, foreground and background. If they differ in form (for example an `agent-` prefix on one side) or id space, the `launched` to `stopped` join must go through another key (the transcript path, or the order of events per session) and `route-outcome` needs a change; until then a missing join means zero observed stops per dispatch, not zero stops.
- Shim (`.claude/hooks/aios`): `hook` subcommands never rebuild in the foreground (a stale binary runs while a background build starts, as for `guard`); a missing binary means no output for `repeat-error`, `route-shadow` and `route-outcome`, and a deny for `path-guard`.
- `CLAUDE_CODE_EFFORT_LEVEL` is `"default"` in the project `env` block. The docs list `low`, `medium`, `high`, `xhigh`, `max` and `auto` (`auto` is the documented spelling of "use the model default", which `"default"` presumably meant) and say the variable outranks `effortLevel`. The model configuration page also states that an agent's frontmatter `effort` overrides the session level but not `CLAUDE_CODE_EFFORT_LEVEL`, so with the variable set every agent runs at that one level and the per-agent effort in the roster above never applies. The variable must therefore be removed, not set to `auto` (which would still outrank frontmatter). Only the treatment of an invalid value such as `"default"` is undocumented. Set the main-session effort through `effortLevel` in the project `.claude/settings.json` (or `modelSettings`), not the user settings file, because Opus 5.5 and later ignore a user-settings `effortLevel`; it accepts `low`, `medium`, `high` and `xhigh` only (not `max`), and verify per-agent effort with a real subagent run (the `effort.level` field in a hook input from inside that subagent shows what applied).
- Docs: the CLAUDE.md agent table and workspace layout, and `docs/project/agent-loop.md`.
- Jev evaluation: after 200 or more logged dispatches, compare Jev's answers with the outcomes (gates passed first time, Fable blocked, work redone) before Jev routes anything.

## Consequences

- Part 1 must merge before part 2. A hook registration that names an `aios hook` subcommand missing from `main` exits 2 (a clap usage error). For the synchronous `PreToolUse` registration, `path-guard` in the worker's frontmatter, that blocks every matching edit. For the synchronous `repeat-error` registration on `PostToolUse`/`PostToolUseFailure`, exit 2 cannot block but its stderr (the clap usage error) is shown to Claude after every Bash call. For the `async: true` settings entries it is a silent non-blocking error and the logs stay empty.
- Part 2 builds from the requirements above, not from the deleted working plan. Before it relies on them, it must check these with a real run:
  - the frontmatter `Stop` to `SubagentStop` mapping and the block cap;
  - `SubagentStop` firing for background subagents;
  - the `launched` to `stopped` join;
  - per-agent effort.
  If the join fails, `route-outcome` needs a change before the evaluation.
- Routing stays fixed per agent definition until the Jev evaluation has run on 200 or more logged dispatches. The evaluation reads `complexity` on the 0 to 2 scale and allows for missing records from `-p` sessions and for unjoined stops. Jev routes only if its answers predict the outcomes. What happens to the shadow hooks if they do not is for the evaluation to decide.
- The worker's kernel boundary rests on `path-guard` plus agent discipline, not on a sandbox. Bash writes, existing nested repositories and a stalled hook are outside it. A boundary that must hold against a hostile agent needs OS-level enforcement, which is not in scope.
- The hook contract was read on 2026-10-05. A Claude Code release that renames the dispatch tool or the `SubagentStop` fields silently stops the logs. Lenient parsing keeps that from breaking a session, but it shows only as gaps in the logs.
