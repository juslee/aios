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

- [ ] Step 1: hook contract. Fetch the current Claude Code hooks reference and record the exact input fields and output shapes the subcommands rely on, in "Hook contract" below.
- [ ] Step 2: shared hook plumbing in `tools/src/cmd/hook/mod.rs`, the `aios hook` subcommand group in `main.rs`, and test helpers for running the binary with stdin.
- [ ] Step 3: `aios hook repeat-error` plus tests.
- [ ] Step 4: `aios hook path-guard` plus tests.
- [ ] Step 5: `aios hook route-shadow` plus tests, including one against a local fake HTTP server through real `curl`.
- [ ] Step 6: review loop (correctness, security, conventions, docs) until a clean round.
- [ ] Step 7: gates: `cargo fmt --check -p aios-tools`, `cargo clippy -p aios-tools --all-targets -- -D warnings`, `cargo test -p aios-tools`, `just docs-check`.
- [ ] Step 8: one live `route-shadow` call against Jev (owner's key), checked by hand.

## Hook contract

Filled in by step 1, with the docs URL and the date read. Until then, the assumptions below come from a docs reading on 2026-10-05:

- Every hook gets one JSON object on stdin with at least `session_id`, `transcript_path`, `cwd`, `hook_event_name`. Tool events add `tool_name`, `tool_input`, and an id for the tool call. `PostToolUse` adds the tool's result. `PostToolUseFailure` adds `error`, a string that carries a Bash exit code in its text, for example `Command exited with code 1: ...`. Hooks running inside a subagent may also carry agent fields (for example `agent_id`, `agent_type`).
- `PreToolUse` denies with `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"..."}}` on stdout and exit 0. No output means no decision.
- `PostToolUse` and `PostToolUseFailure` add context for the model with `{"hookSpecificOutput":{"hookEventName":"<event>","additionalContext":"..."}}`.
- Exit 2 is a blocking error that feeds stderr to the model; the subcommands never use it. Any other non-zero exit is a non-blocking error shown to the user.

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
- When a signature's count reaches 2 or more, emit `additionalContext`. The text depends on where the hook runs:
  - Inside a subagent (an agent id is present): "`<command>` has failed <n> times with the same error. Do not run it again unchanged. Stop and report back to your caller with the command, the error and what you changed between attempts, so the lead can get it reviewed."
  - In the main session: "`<command>` has failed <n> times with the same error. Do not run it again unchanged. Before the next attempt, ask the code-reviewer agent to diagnose it: give it the command, the error and what you changed between attempts."
  - The command is quoted cut to 200 bytes.
- Success (`PostToolUse`, `tool_name` `Bash`): drop every signature with the same normalised command, so a fixed command starts clean. No output.
- Any other event or tool: no output.
- Files untouched for 7 days are deleted when the subcommand next writes state.

**`path-guard`**

- Options: `--deny <prefix>` (repeatable, at least one; a repository-relative directory prefix ending in `/`) and optional `--reason <text>` appended to the deny reason.
- Only `Edit`, `Write`, `MultiEdit` (`tool_input.file_path`) and `NotebookEdit` (`tool_input.notebook_path`) are checked; any other tool gets no output.
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

(to be filled during implementation)

## Decisions Made

(to be filled during implementation)

## Lessons Learned

(to be filled during implementation)
