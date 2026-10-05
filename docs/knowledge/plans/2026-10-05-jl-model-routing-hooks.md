---
author: jl + claude
date: 2026-10-05
tags: [tooling]
status: in-progress
---

# Plan: model routing hooks for the agent team

## Approach

The owner wants the agent team to run as: Opus leads, Sonnet agents do routine work, Fable reviews at fixed points, and Jev (TypeSafe) is tried as a router for subagent dispatches. Claude Code supplies most of this through configuration: a `model:` and `effort:` per agent definition, and hooks, including `type: agent` hooks that run on a chosen model. What it does not supply is three small deterministic programs, which this plan adds to the `aios` host binary as `aios hook <name>`:

| Subcommand | Event (registered in part 2) | Job | On internal error |
|---|---|---|---|
| `repeat-error` | `PostToolUseFailure` and `PostToolUse`, matcher `Bash` | Notice the same Bash command failing the same way twice and tell the agent to stop retrying and get a review | fail open: no output, exit 0 |
| `path-guard` | `PreToolUse`, matcher `Edit\|Write\|MultiEdit\|NotebookEdit`, in the frontmatter of the Sonnet `worker` agent only | Deny edits under configured repository prefixes (`kernel/`, `uefi-stub/`, `shared/`) so kernel code stays with `kernel-dev` on Opus | fail closed: deny with the error as the reason |
| `route-shadow` | `PreToolUse`, matcher for the subagent-dispatch tool | Ask Jev how the dispatch would be routed and log the answer next to what actually happened; never decides anything | fail open: log the error, no output, exit 0 |

Jev runs in shadow mode only. The 2026-09-22 review-triage evaluation found Jev no better than the base rate when the deciding evidence sat in other files, and "does this task need a stronger model" has the same shape. The log lets the owner check that before Jev decides anything.

The work is split in two because other open branches own the files that wire these programs in:

- **Part 1 (this branch, now):** the three subcommands, their tests and this plan. It touches only new files under `tools/src/cmd/hook/` and `tools/tests/`, plus one line each in `tools/src/cmd/mod.rs` and `tools/src/main.rs`.
- **Part 2 (after `claude/harness-team-config` and `claude/tools-203-guard-fail-closed` merge):** wiring. Requirements only, listed under "Part 2 requirements" below.

## Progress

- [x] Step 1: hook contract. Fetch the current Claude Code hooks reference and record the exact input fields and output shapes the subcommands rely on, in "Hook contract" below.
- [x] Step 2: shared hook plumbing in `tools/src/cmd/hook/mod.rs`, the `aios hook` subcommand group in `main.rs`, and test helpers for running the binary with stdin.
- [ ] Step 3: `aios hook repeat-error` plus tests.
- [ ] Step 4: `aios hook path-guard` plus tests.
- [ ] Step 5: `aios hook route-shadow` plus tests, including one against a local fake HTTP server through real `curl`.
- [ ] Step 6: review loop (correctness, security, conventions, docs) until a clean round.
- [ ] Step 7: gates: `cargo fmt --check -p aios-tools`, `cargo clippy -p aios-tools --all-targets -- -D warnings`, `cargo test -p aios-tools`, `just docs-check`.
- [ ] Step 8: one live `route-shadow` call against Jev (owner's key), checked by hand.

## Hook contract

Source: https://code.claude.com/docs/en/hooks.md, https://code.claude.com/docs/en/sub-agents.md (dispatch tool name, frontmatter hooks) and https://code.claude.com/docs/en/tools.md (tool table, Bash failure rule, NotebookEdit), read and re-verified 2026-10-05. Anything marked "not documented" is not in those pages.

**Input (every event).** One JSON object on stdin. Common fields: `session_id` (string), `prompt_id` (string, UUID, absent until the first user input), `transcript_path` (string), `cwd` (string), `scratchpad_dir` (string, may be absent), `permission_mode` (string, not on every event), `effort` (object with `level`, one of `low`, `medium`, `high`, `xhigh`, `max`; present on tool-context events only when the current model supports the effort parameter), `hook_event_name` (string). The docs example lists them as `session_id`, `prompt_id`, `transcript_path`, `cwd`, `scratchpad_dir`, `permission_mode`, `hook_event_name`.

**Subagent fields.** "When running with `--agent` or inside a subagent, two additional fields are included": `agent_id` (string, "Present only when the hook fires inside a subagent call"; absent in the main thread) and `agent_type` (string, the agent name; present when the session uses `--agent` or the hook fires inside a subagent). Hooks from settings, managed policy and plugins also run inside subagents, so tool events (`PreToolUse`, `PostToolUse`, `PostToolUseFailure`, ...) carry both fields there. Agent-frontmatter hooks fire when the agent runs as a subagent or as the main session via `--agent`.

**PreToolUse.** Adds `tool_name` (string), `tool_input` (object), `tool_use_id` (string, for example `toolu_01ABC123...`). Matches any tool name except `EndConversation`. A matcher is an unanchored JavaScript regex, so `Edit` alone also matches `NotebookEdit`; anchor with `^...$` for a whole-string match.
- Bash `tool_input`: `command`, `description`, `timeout`, `run_in_background`.
- `Write`: `file_path`, `content`. `Edit`: `file_path`, `old_string`, `new_string`, `replace_all`. For `Write`, `Edit` and `Read`, "`tool_input.file_path` is always absolute" (`~` and relative paths are expanded before hooks run).
- `NotebookEdit`: in the tools reference's tool table; it edits one cell at a time by `cell_id`, with modes `replace`, `insert`, `delete` and a `cell_type` for inserts. The name of the field that carries the notebook path is not documented. `MultiEdit`: not in the tools reference's tool table and not mentioned in the hooks or sub-agents pages. `path-guard` therefore reads `file_path` and, failing that, `notebook_path` (a guess, not documented), and fails closed when a checked tool has neither.
- Dispatch tool: `tool_name` is `Agent` ("In version 2.1.63, the Task tool was renamed to Agent. Existing `Task(...)` references in settings and agent definitions still work as aliases"). The hooks page lists `Agent` among the PreToolUse tool names; matching `Task` is not documented for hooks. Its documented `tool_input` table: `prompt` (string), `description` (string), `subagent_type` (string, for example `Explore`), `model` (optional string alias, for example `sonnet`). The prose also mentions `run_in_background` (omitted means background) and, with agent teams, `name`; neither is in the table.

**PostToolUse.** Fires "after a tool has already executed successfully". Adds `tool_name`, `tool_input`, `tool_response` (the result, schema depends on the tool), `tool_use_id`, optional `duration_ms`. Bash `tool_response` is an object with `stdout`, `stderr`, `interrupted`, `isImage` (documented as the shape `updatedToolOutput` must match; `bashEditDiff` may also appear). For `Agent`, `tool_response.status` is `completed` for a foreground subagent or `async_launched` for a background one, and background is the default. A `completed` response carries `agentId`, `content`, `resolvedModel` and run telemetry (`modelsUsed`, `totalTokens`, `totalDurationMs`, `totalToolUseCount`, `usage`). An `async_launched` response carries `agentId`, `description`, `prompt`, `outputFile`, `resolvedModel` and no `content` or usage fields.

**PostToolUseFailure.** Fires when a tool that started executing fails. Adds `tool_name`, `tool_input`, `tool_use_id`, `error` (string), optional `is_interrupt` (boolean), optional `duration_ms`. It does not fire for calls rejected before execution (unknown tool, schema validation failure, permission denial). For Bash and PowerShell, "a command that ran and exited produces a first line `Exit code N`, then any output the command produced as one block with stdout and stderr interleaved", for example `"Exit code 1\nError: Cannot find module 'express'"`. Which Bash exits count as failures is in the tools reference: "A command that exits 1 counts as a valid result for the Bash tool only when Claude Code recognizes exit code 1 as a benign outcome for that command: `grep`, `rg`, `egrep`, `fgrep`, `find`, `diff`, `test`, and `[`, plus `git diff` and `git grep`. Every other command that exits 1 counts as a failure". So a `grep` with no match is a PostToolUse (success), and `cargo test` exiting 1 is a PostToolUseFailure. Exit codes above 1 are not addressed in any sentence; the plan assumes they are failures. A payload may carry a bare message with no exit-code line when the shell could not start, and long strings are middle-truncated around a `... [N characters truncated] ...` marker, with lines such as `Command timed out after 2m 0s` inserted. The docs say to key on `tool_name`, `is_interrupt` and the `Exit code N` first line and to treat the rest as display text, not a stable format.

**Output.**
- PreToolUse deny: `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"..."}}`; the reason is shown to Claude. Other decisions: `allow`, `ask`, `defer`; optional `updatedInput`, `additionalContext`. Precedence across hooks: deny > defer > ask > allow.
- PostToolUse and PostToolUseFailure context: `{"hookSpecificOutput":{"hookEventName":"<event>","additionalContext":"..."}}`. The string is added "next to the tool result" inside a system reminder. Each string is capped at 10,000 characters (over the cap it becomes a file path plus a 2,000-character preview). The docs ask for the text to be "factual statements rather than imperative system instructions", because "text framed as out-of-band system commands can trigger Claude's prompt-injection defenses, which causes Claude to surface the text to you instead of treating it as context".
- Exit 0: success, the intended code for JSON output. Stdout that starts with `{` and ends with `}` (ignoring whitespace) is parsed as JSON; "Your hook's stdout must contain only the JSON object". Such output that fails to parse, or parses but fails schema validation, is a non-blocking error and the action proceeds. Output that starts with `{` but does not end with `}`, or starts with anything else, is plain text. Plain text or empty stdout on PreToolUse, PostToolUse and PostToolUseFailure adds nothing. Stderr on exit 0 goes to the debug log only; Claude never sees it.
- Exit 2: blocking error. PreToolUse: blocks the call, stderr (or the JSON reason) goes to Claude. PostToolUse and PostToolUseFailure: cannot block, stderr is shown to Claude. Exit 2 beats a JSON `allow`.
- Any other exit: non-blocking. With a valid JSON object on stdout the exit code is ignored and the JSON decides, with no error reported. Without it, the transcript shows `<hook name> hook error` with `Failed with non-blocking status code:` and the first line of stderr. "Exit code 1 ... is a non-blocking error and proceeds", so only exit 2 or a JSON deny blocks.
- Timeout: a timed-out `command` hook on PreToolUse does not block the call; it proceeds through the normal permission flow. A stalled `path-guard` therefore fails open; this is outside the hook's control.

Parse leniently: unknown fields are ignored and every field the code does not strictly need is optional, so a Claude Code release that adds fields changes nothing.

## Code Structure Decisions

**Common (`tools/src/cmd/hook/mod.rs`)**

- `aios hook <name> [options]` reads stdin to the end, capped at 16 MiB. More than that is an input error, handled by the subcommand's error policy in the table above.
- At most one JSON object goes to stdout. Diagnostics go to stderr. Exit code is 0 on every path, including errors, so a broken hook never blocks or nags the session; `path-guard` expresses its failure as a deny.
- State lives under the git common directory, so all worktrees share it: `<git common dir>/aios-agent/hooks/`. Resolve it with `git rev-parse --path-format=absolute --git-common-dir`, run from the input's `cwd` (falling back to the process cwd) through `proc::capture`. `AIOS_HOOK_STATE_DIR` overrides it, for tests.
- A session id becomes part of a file name only after sanitising: keep `[A-Za-z0-9_-]`, and if anything else was present or the result is empty or longer than 128 bytes, use a fixed-width hex digest of the original instead. A crafted id can never name a path outside the state directory.
- State files are written to a temporary file in the same directory and renamed over the old one. Two hook processes racing on one file can lose an update; the only effect is a nudge arriving one failure late, which is accepted rather than adding file locking.
- No new dependencies: the crate's approved set (clap, anyhow, serde, serde_json, regex) covers this. HTTP goes through the system `curl`. `#![forbid(unsafe_code)]` stays.

**`repeat-error`**

- Failure (`PostToolUseFailure`, `tool_name` `Bash`): build a signature from the normalised command and the normalised error.
  - Normalised command: trimmed, whitespace runs collapsed to one space.
  - Normalised error: whitespace runs collapsed, `0x` hex numbers replaced by `0x#`, digit runs of four or more replaced by `#` (timestamps, PIDs, durations, addresses), cut to 512 bytes on a character boundary.
- State is one file per session and agent: `repeat-error/<session>[-<agent>].json`, holding at most 64 signatures with counts (oldest evicted).
- When a signature's count reaches 2 or more, emit `additionalContext`. The final wording is settled in step 3 and must read as statements of fact, not system commands (see the hook contract); the intent, by where the hook runs:
  - Inside a subagent (an agent id is present): "`<command>` has failed <n> times with the same error. Do not run it again unchanged. Stop and report back to your caller with the command, the error and what you changed between attempts, so the lead can get it reviewed."
  - In the main session: "`<command>` has failed <n> times with the same error. Do not run it again unchanged. Before the next attempt, ask the code-reviewer agent to diagnose it: give it the command, the error and what you changed between attempts."
  - The command is quoted cut to 200 bytes.
- Success (`PostToolUse`, `tool_name` `Bash`): drop every signature with the same normalised command, so a fixed command starts clean. No output.
- Any other event or tool: no output.
- Files untouched for 7 days are deleted when the subcommand next writes state.

**`path-guard`**

- Options: `--deny <prefix>` (repeatable, at least one; a repository-relative directory prefix ending in `/`) and optional `--reason <text>` appended to the deny reason.
- Only `Edit`, `Write`, `MultiEdit` and `NotebookEdit` are checked; any other tool gets no output. The path is `tool_input.file_path`, or `tool_input.notebook_path` when `file_path` is absent (the docs name only `file_path`, for `Write` and `Edit`, and give it as absolute).
- Resolving the target:
  1. Join a relative path to the input `cwd`.
  2. Normalise `.` and `..` lexically.
  3. Canonicalise the longest existing ancestor (symlinks resolved) and append the remaining components.
- The repository root is `git rev-parse --show-toplevel` run from the input `cwd`, canonicalised the same way. A target outside the root gets no decision: this hook guards the repository, not the filesystem.
- Comparison is ASCII case-insensitive, because macOS volumes are case-insensitive by default (`Kernel/src/x.rs` is `kernel/src/x.rs`).
- Deny reason: "`<relative path>` is under `<prefix>`, which this agent may not edit: kernel, UEFI stub and shared code goes to kernel-dev. Hand this change back to your caller." followed by the `--reason` text if given.
- Fail closed: unparseable input, a missing path field on a checked tool, or a git failure produces a deny whose reason names the error.
- Known limit: Bash can still write files (`sed -i`, redirection). The guard stops the agent's normal edit tools; it is a routing aid, not a sandbox.

**`route-shadow`**

- Exits immediately with no output when `AIOS_ROUTE_SHADOW=off`.
- Reads `tool_input.subagent_type`, `description`, `prompt` and `model` (all optional).
- Jev request: `POST https://api.typesafe.ai/v1/systemone`, overridable with `AIOS_JEV_URL` for tests, with the model pinned to `jev-1.13.0`, since thresholds are tuned per version.
- The request `state` holds `subagent_type`, `description`, and `prompt` cut to 8,000 characters; the log records whether it was cut. Three questions:
  - `complexity` (score), "How much reasoning does carrying out `prompt` need?", with levels:
    1. "Mechanical: a lookup, a search with a clear target, a rename, formatting, or wording."
    2. "Routine: a change or check that follows an existing pattern in one area."
    3. "Hard: debugging with an unclear cause, a change across interacting parts, or a choice between designs."
  - `low_level_hazard` (noul): "Does `prompt` ask for changes to, or diagnosis of, code that involves concurrency, memory ordering, page tables or the MMU, interrupt or exception handling, lock ordering, or boot sequencing?"
  - `work_kind` (choice): "What kind of work does `prompt` ask for?" Options:
    - `read_only_search`: find or read code or docs without changing them
    - `implement_change`: write or modify code, tests or configuration
    - `review_or_audit`: judge existing work for defects or compliance
    - `run_and_verify`: build, test, boot or measure and report the result
    - `write_docs`: write or edit prose documentation
    - `research_external`: gather information from outside the repository
    - `other`: none of these
- Transport: `curl -sS --max-time 4 -X POST <url> -H 'Content-Type: application/json' -K - --data-binary @<body file>`.
  - The `Authorization: Bearer` header goes to curl on stdin as a `-K -` config line, so `TYPESAFE_API_KEY` never appears in argv, the log or stderr.
  - The body file is created mode 0600 in the state directory and removed afterwards.
  - Behind a small transport trait so unit tests can fake it.
- A missing `TYPESAFE_API_KEY` is logged as an error record, not skipped silently, so the log shows coverage gaps.
- Log: one JSON line appended to `<state dir>/route-shadow.jsonl` per call, with these fields:
  - when: `ts` (Unix seconds)
  - identity: `session_id`, the agent id if present, the tool call id
  - the dispatch: `subagent_type`, the requested `model`, `description`, `prompt` (the possibly cut text), `prompt_chars`, `prompt_truncated`
  - Jev: `jev_model` (from the response), `latency_ms`, `answers` (the response's `answers` object verbatim, as raw JSON)
  - `error` (null on success)

  The tool call id is what later joins a record to the dispatch's outcome.
- Never prints a decision. Total wall time stays under 5 s, because part 2 registers it with a 6 s timeout.

## Part 2 requirements

For the wiring PR, after `claude/harness-team-config` and `claude/tools-203-guard-fail-closed` merge. Requirements only; the design is that PR's job.

- Agent models: team-lead inherits (Opus, high effort); kernel-dev `opus`; a new `worker` agent on `sonnet` for `tools/`, `scripts/`, `docs/` and `.github/` work, with `path-guard` in its frontmatter; verifier and doc-auditor `sonnet`; doc-writer `opus`; code-reviewer `fable`.
- Fable before "done": a `Stop` hook in kernel-dev's frontmatter (it runs as `SubagentStop` for that agent only), `type: agent`, `model: fable`, reviewing the change and blocking with reasons, with a cap so it cannot loop.
- Fable before the plan: in `/implement-phase`, a Fable review of the working plan before the plan commit. Leaving plan mode cannot be hooked.
- `settings.json`: `repeat-error` on `PostToolUseFailure` and `PostToolUse` for `Bash`; `route-shadow` on `PreToolUse` for the dispatch tool, timeout 6 s.
- Shim (`.claude/hooks/aios`): `hook` subcommands never rebuild in the foreground (a stale binary runs while a background build starts, as for `guard`); a missing binary means no output for `repeat-error` and `route-shadow`, and a deny for `path-guard`.
- `CLAUDE_CODE_EFFORT_LEVEL` is `"default"` in the project `env` block; the docs list only `low`, `medium`, `high`, `xhigh`, and the variable outranks `effortLevel`. Fix it with the main-session effort setting.
- Docs: the CLAUDE.md agent table and workspace layout, and `docs/project/agent-loop.md`.
- Jev evaluation: after 200 or more logged dispatches, compare Jev's answers with the outcomes (gates passed first time, Fable blocked, work redone) before Jev routes anything.

## Dependencies & Risks

- **Depends on:** nothing for part 1. Part 2 waits for the two branches named above.
- **Risk:** Claude Code hook input fields change. Lenient parsing and the step 1 contract record limit the damage.
- **Risk:** a hook slows every tool call. `repeat-error` does no network work; `route-shadow` runs only on dispatches and is capped at 4 s by curl.

## Issues Encountered

- Step 1 (hook contract): the plan assumed a Bash `error` text like `Command exited with code 1: ...`. The live docs say the first line is `Exit code N`, then output, and call the rest display text. `repeat-error` never parses the exit code (it normalises the whole `error` string into the signature), so no decision changes; tests use `Exit code N` fixtures.
- Step 1 (verification): the first contract draft inferred "any non-zero Bash exit is a PostToolUseFailure" from the absence of a sentence. The tools reference has the rule: exit 1 is a valid result for `grep`, `rg`, `egrep`, `fgrep`, `find`, `diff`, `test`, `[`, `git diff` and `git grep`, and a failure for every other command. For `repeat-error` this means a no-match `grep` arrives as PostToolUse and clears that command's signatures, which is the wanted behaviour, so no decision changes. The draft also listed `content` in the Agent `tool_response` for every status; it is present only on `completed`, and the default `async_launched` response has `description`, `prompt`, `outputFile` instead. Nothing in part 1 reads `tool_response`.
- Step 1 (verification): the docs ask that `additionalContext` be phrased as factual statements, since imperative "system command" phrasing can trip Claude's prompt-injection defences and get the text surfaced to the user instead of acted on. The `repeat-error` nudge drafted in the design is imperative; step 3 writes it as statements (what failed, how often, what the project expects next) with the same content.
- Step 1: the dispatch tool is `Agent` in hook input. Part 2 must register `route-shadow` with matcher `Agent`. `tool_input.model` is an optional alias such as `sonnet`.
- Step 1: the tool-call id field is `tool_use_id`. The `route-shadow` log field keeps the name "tool call id" in the plan text and is stored as `tool_use_id`.
- Step 1: the docs name `file_path` for `Write` and `Edit` only. `NotebookEdit` field names are not documented and `MultiEdit` is not a documented tool, so `path-guard` reads `file_path` or, failing that, `notebook_path`, and denies when a checked tool has neither (already the fail-closed rule). Decision text adjusted below.
- Step 1: `path-guard` fails open on a hook timeout (documented, not changeable); the plan's "fail closed" covers errors the process reports, not a stall. Stderr diagnostics at exit 0 reach only the debug log.

## Decisions Made

- Step 2: the subcommand entry functions take the parsed `HookInput` and a `Ctx` (state-dir override and process cwd), and return `Result<Option<String>>`; `hook::run` owns reading stdin, parsing, the error policy and printing. Why: one place enforces the stdout, stderr and exit-0 discipline, and the three subcommands cannot diverge from it.
- Step 2: an input error (unreadable, over the cap, not a JSON object) is handled by `hook::run` before the subcommand runs, so `route-shadow` reports it on stderr only. The plan says it logs errors to its jsonl; that needs the state dir, which needs the payload's `cwd`, which an unparseable payload does not give. Errors after a successful parse are still logged by the subcommand in step 5.
- Step 2: a clap usage error (for example `path-guard` without `--deny`) exits 2, not 0. It is a registration mistake rather than a payload, and for `path-guard` exit 2 blocks the call, which is fail closed. Every payload path exits 0.
- Step 2: `HookInput` reads a field of the wrong type as absent (not as an input error); a payload that is valid JSON but not an object is an input error. Why: "parse leniently" in the contract, while an array or string cannot be a hook payload.
- Step 2: `state_dir` takes the override from `AIOS_HOOK_STATE_DIR` as the state directory itself (not a parent of `aios-agent/hooks`), and an empty value counts as unset. The per-hook subdirectories (`repeat-error/`) are created by the subcommands through `write_atomic`, which creates missing parent directories.
- Step 2: the missing-session case: `state_key` hashes an absent or empty `session_id` like any unsafe id, so state still lands in one stable file. `<session>-<agent>` can in theory collide with a plain session id that contains a hyphen; ids are UUIDs and a collision only merges two failure counters.
- Step 2: `additional_context` cuts its text to 10,000 bytes on a character boundary (the documented cap is characters, so this is the safe side).

## Lessons Learned

(to be filled during implementation)
