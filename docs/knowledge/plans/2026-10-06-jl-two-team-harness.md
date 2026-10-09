---
author: jl + claude
date: 2026-10-06
tags: [tooling, security]
status: draft
---

# Plan: two-team harness (team-build and team-fix) with the model-routing Part 2 wiring

> **v4 (2026-10-09):** reconciled with `origin/main` at 1a5c363 (#222–#230 merged since f12dbc7); see `D/RECONCILE-v4.md`. Where a draft has a `.v4` file next to it, use the `.v4` file; the v3 originals are kept unchanged for the record.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. The controller is the attended `team-build` lead session in the main checkout (see Execution); every `.claude/**` edit prompts the owner.

**Goal:** Two attended lead sessions, `team-build` and `team-fix`, split the work by domain and share one host safely, and every agent runs on the model, effort and hooks that the model-routing ADR requires.

**Architecture:** Every agent is a subagent of a lead session (no Claude Code teammates). Writers and the verifier are pinned to their branch worktree with `isolation: "worktree"` plus `EnterWorktree({path})`; readers run read-only in the main checkout. Model and effort live in agent frontmatter; every hook is registered once, in `.claude/settings.json`, so it also runs in `-p`, SDK and untrusted sessions. One host QEMU lock serves both teams, and cross-team order lives in GitHub labels and gate issues, with lead-to-lead messages only mirroring that state.

**Tech stack:** Claude Code 2.1.285 (terminal CLI) settings, agents, skills, hooks and workflows; the `aios` host binary (`tools/`, std Rust, `aios hook` from #220, `aios soak` from #230, installed at `target/tools/installed/aios` with a provenance stamp by `just tools` since #203); POSIX `sh` (the `aios` shim, `scripts/agent/qemu-lock.sh`); Python 3 (`.claude/hooks/git-push-guard.py`); bash (`scripts/agent/brief.sh`); `gh`.

**Spec:** the requirements are `docs/knowledge/decisions/2026-10-06-jl-model-routing-hooks.md`, section "Part 2 requirements (owned by the two-team harness PR)" and "Consequences", on `main` since #220 (verbatim extract: `D/part2-requirements-final-1a5c363.extract.md`; #229 changed only its Shim bullet since `D/part2-requirements-final-5838ad2.extract.md`), plus the owner decisions under Decisions. `D` below is the design folder `<git-common-dir>/aios-agent/teams-design-v3/`; every draft this plan applies is there, mirroring repository paths. Evidence: `D/evidence-v3.md` (E1–E23). Headless check procedures: `D/probe/HEADLESS.md.v4`. Checks only the owner can run: `D/OWNER-PROBES.md.v4`.

## Global Constraints

Every task's requirements include these.

- Base: `origin/main` at 1a5c363 or later (the v4 drafts are anchored there). It contains #220 (5838ad2: `aios hook repeat-error|path-guard|route-shadow|route-outcome`), #221 (f12dbc7: `/merge-and-cleanup` preserves soak results before merging), #203 (aa52bff: the shim runs `target/tools/installed/aios` after a provenance-stamp check, and `guard` fails closed) and Tools R4 (1a5c363: `aios soak`; `scripts/soak-qemu.sh` deleted; `just soak` depends on `tools` and runs the checkout's own installed binary). All are merged; none is duplicated here.
- Branch `claude/harness-teams`, worktree `<main>/.claude/worktrees/harness-teams` (`W`). Push only with `git -C "$W" push -u origin claude/harness-teams`. Never push or merge `main`, never force, never stash, never rebase (rule 03).
- Commit message: `Harness teams: Tn — <task title>` (one commit per task; fix rounds add `(review fixes)`), ending with the attribution line the session's system reminder gives.
- `.claude/**` edits run in a foreground implementer, so the permission prompt reaches the owner. `.claude/settings.json` changes only by the exact transformation `D/.claude/settings.json.apply.jq.v4`, which the owner approves block by block (T8).
- No QEMU boot anywhere in this PR: it changes no kernel, stub or shared code. The lock wrapper's tests use a fake QEMU (a symlink to `/bin/sleep`).
- Agent routing, verbatim from the ADR plus owner answer O3: kernel-dev `opus`/`high`; worker `sonnet`/`high`; doc-writer `opus`/`high`; code-reviewer `fable`/`high`; verifier `sonnet`/`medium`; doc-auditor `sonnet`/`medium`; skeptic `sonnet`/`medium`.
- Hook registrations, from the ADR: `repeat-error` synchronous on `PostToolUse` and `PostToolUseFailure`, matcher `Bash`; `route-shadow` `async: true` on `PreToolUse`, matcher `Agent`, printing nothing; `route-outcome` `async: true` on `PostToolUse` and `PostToolUseFailure` (matcher `Agent`) and on `SubagentStop`; `path-guard --deny kernel/ --deny uefi-stub/ --deny shared/` on `PreToolUse`, matcher `Edit|Write|MultiEdit|NotebookEdit`, plus this PR's `--agent-type worker`.
- `CLAUDE_CODE_EFFORT_LEVEL` is absent from the project env; `effortLevel` is `"high"` in project settings (values `low`, `medium`, `high`, `xhigh` only).
- Never use the reserved names `team-lead`, `main`, `user` or `system` for an agent, team or session.
- `tools/` stays a std crate with `unsafe` forbidden and only the approved dependencies (clap, anyhow, serde, serde_json, regex, and signal-hook since R4, rule 01). The shim and `qemu-lock.sh` stay POSIX `sh` (dash and macOS `/bin/sh`).
- An agent's `tools:` list names only tools in docs-check's `KNOWN_TOOLS`; never `ListAgents` or `MultiEdit`.
- Final ADRs get a dated amendment; their body is never rewritten. Architecture docs are not touched.
- Owner rule: never keep legacy. Nothing retired keeps a stub, a disabled flag, a dead reference or a compatibility note.
- Agents and this plan never edit `~/.claude*`. User-settings changes are owner steps in T19.
- Do not touch the lock-order text in `.claude/CLAUDE.md` (`scripts/docs/baseline.json` accepts `lock-order|.claude/CLAUDE.md|unknown:VIRTIO_INPUT`).

## Review Focus

Failure modes the requirements imply but no single feature test exercises. Each has a pinning test in the task named.

1. **CI and fresh clones run the project hooks with no `aios` binary.** `claude-code-action` (`.github/workflows/claude*.yml`) loads `.claude/settings.json` on a runner with no `target/tools`. Expected: no hook denies an edit, nothing is printed, the session is not slowed by a build loop. Pinned in T5 (shim rows 1, 5) and T16 (E7's control case).
2. **`main`'s binary is older than the hooks that call it.** Right after this PR merges, `target/tools/installed/aios` fails its provenance stamp (the merge changed `tools/`), so it is stale; built anyway, it would have no `--agent-type`. Expected: worker edits under `kernel/` are denied, nobody else's edits are, `repeat-error` stays silent, and the background build the hook starts fixes it. Pinned in T5 (rows 7, 8) and T16 (E7).
3. **A subagent payload without `agent_type`, or with a wrong-typed or empty one.** Expected: `path-guard` and guard rule 1 fail closed for it. Pinned in T4 (rows 4, 10, 13) and T7 (rule 1 row 3).
4. **A Fable gate that never lets go.** A blocked kernel-dev that keeps disagreeing, or a plan the gate keeps rejecting. Expected: at most three step blocks per agent and two plan blocks per session, then the lead or owner decides; `maxTurns: 300` ends kernel-dev regardless. Pinned in T2 (H7) and T16 (E5, E6).
5. **A lead killed mid-soak (SIGKILL, closed terminal) leaves the QEMU lock behind.** Expected: `status` says `state=dead`, `clear-stale` removes it only when no QEMU runs, and a live holder is never cleared. Pinned in T6.

## Decisions

### Owner decisions (binding)

| When | Decision | Where it lands |
| --- | --- | --- |
| 2026-10-05 21:30 | This PR follows `claude/harness-team-config` (merged as #218, c6f5511). Model routing follows the aios-de plan, now the ADR. Handoffs are per-role files written by `/justin:pause`; the remember plugin and `precompact-save.sh` are retired. | D4, D14 |
| 2026-10-05 21:33 | The two-team PR owns the agent and settings wiring of model-routing Part 2; the model-routing branch keeps the hook programs and merges first (merged as #220, 5838ad2). | D4–D10 |
| 2026-10-06 10:25 | `/merge-and-cleanup` finds the PR's worktree by branch and copies `target/soak/` before `gh pr merge --delete-branch`. | Done by #221 (D15) |
| 2026-10-06 10:48 | Domain split, not a lifecycle split: `team-build` (harness, the two-team PR including the Part 2 wiring, tooling, new features) and `team-fix` (crash-fix step 1b #164 and nearby bugs such as #217, then cap-lifetime). Handoff files `.remember/handoff-team-build.md` and `.remember/handoff-team-fix.md`. Avoid the reserved names `team-lead`, `main`, `user`, `system`. QEMU ownership is re-derived (team-fix soaks too). | D1, D11, D13 |
| 2026-10-06 11:28 (relayed) | Also register `path-guard` in `settings.json` with an `agent_type` filter (preferred over "headless worker runs are unguarded"). The Fable plan gate is an `ExitPlanMode` hook, not an `/implement-phase` step. `effortLevel` goes in project settings. | D5, D7, D9 |
| 2026-10-06 11:44 | O1: drop the project `remember@claude-plugins-official` line, and the owner removes the plugin from `~/.claude-personal/settings.json` when this PR merges. O2: migrate `.remember/remember.md` into the per-team handoff files, archive `logs/` under `<git-common-dir>/aios-agent/remember-retired/`, delete the rest. O3: skeptic `sonnet`/`medium`. O4: Fable step reviews for kernel-dev only. O5: R2's acceptance criteria delete `/review-pr-comments` and point the PR loop at `/justin:review`. | D4, D14, D18, D19, T19 |
| 2026-10-09 21:35 | Q1 yes (Fable step gate as a settings `SubagentStop` hook), Q3 yes (`RUSTUP_AUTO_INSTALL=0`, merging session installs once), Q4 yes (no Claude review on drafts), Q5 treat a stale or dirty binary as missing, Q6 literal-path `git -C <W>` allows only (no wildcards), Q7 team-build ports `soak-matrix.sh` to `aios soak` in its own PR before this one. Q2 resolved by #203 merging first (#229). | D8, D10, D12, Q4 patch notes, settings jq v4, T19 |
| standing | Never keep legacy. | every task |

### Design decisions

| # | Decision | Why |
| --- | --- | --- |
| D1 | **Teams and identity.** Two attended terminal sessions, launched from the main checkout as `AIOS_TEAM=<team> claude -n <team> --model opus`, with `<team>` `team-build` or `team-fix`. `AIOS_TEAM` keys the handoff file, the lock's `--team` and the guard; the `-n` name must equal it (`/justin:team` preflight). Every other session is solo with `AIOS_SESSION=<name>`. | Session display names are self-chosen and reset on a fresh launch (seen on 2026-10-06: the leads came back as aios-83 and aios-39). `CLAUDE_CODE_SESSION_NAME` is not set by `-n`. An env var is the only identity the guard and the skills can read. |
| D2 | **Subagents only.** No Claude Code teammates. Leads never pass `name` without `isolation` (guard rule 3). The project's `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS` line goes; lead-to-lead messages use cross-session messaging, which has its own gate (E16). | E2–E4; teammates added nothing the design needs. OP1 confirms messaging without the project flag. |
| D3 | **Writer placement.** Writers (kernel-dev, worker, doc-writer) and the verifier are spawned with `isolation: "worktree"` and first call `EnterWorktree({path})`. Readers (code-reviewer, doc-auditor, skeptic, Workflow lenses) run in the main checkout with `git -C <W>`. Leads stay in the main checkout. | E1, E5–E7. T2 (H1–H3) checks it with a real run before anything depends on it. |
| D4 | **Roster: seven agents.** kernel-dev, worker (absorbs v1's tools-dev), doc-writer, verifier, code-reviewer (absorbs the security lens), doc-auditor, skeptic, with the routing in Global Constraints. `team-lead.md` and the `build-team` skill are deleted. worker writes `tools/`, `scripts/`, `.github/`, the justfile, `.gitattributes`, `.claude/` harness files and `docs/` work; doc-writer writes phase docs, architecture docs and ADR amendments. | ADR roster; E18 (reserved name, TeamCreate gone). The ADR gives doc-writer a model but no scope; design docs need its Opus judgment, every other `docs/` task stays with worker as the ADR says. |
| D5 | **Effort.** Remove `CLAUDE_CODE_EFFORT_LEVEL` from the project env; set `effortLevel: "high"` in project settings; every agent sets `effort` in its frontmatter. Leads launch with `--model opus` and no `--effort`, so effort has one source. | ADR: the variable outranks frontmatter `effort`, and Opus 5.5 ignores a user-settings `effortLevel`. T2 H6 checks what applies. |
| D6 | **Logging hooks exactly as the ADR lists them** (Global Constraints). `route-shadow` gets its own `PreToolUse` entry beside the guard's, so the guard stays synchronous. | ADR. |
| D7 | **`path-guard` registered once, in `settings.json`**, with a new `--agent-type worker` flag: it decides only for worker, denies a payload with `agent_id` and no `agent_type`, and passes every other caller before any path or `git` work. No frontmatter copy. | Owner 11:28; frontmatter hooks do not run in `-p`, SDK or untrusted sessions (ADR Known limits); two registrations would be legacy. |
| D8 | **Fable step gate**: a `SubagentStop` agent hook in `settings.json`, matcher `kernel-dev`, `model: fable`, `timeout: 600`. kernel-dev writes its range as `<W>/target/fable-gate/<head>.patch` and starts its report with `RESULT:` and `W:` lines; the gate applies code-reviewer's `rules` and `bugs` lenses to the patch. It caps itself at three blocks per agent by counting its own marker (`FABLE-GATE block`) in `agent_transcript_path`; kernel-dev's `maxTurns: 300` is the hard backstop. Open question Q1 confirms settings over frontmatter. | ADR "Fable before done"; E19, E20. Agent hooks read files but cannot run git, hence the patch file. Counting `stopped` records in `route-outcome.jsonl`, as the ADR suggests, fails twice: the record is written asynchronously, and a missing binary leaves the count at zero, which would never cap. |
| D9 | **Fable plan gate**: a `PreToolUse` agent hook on `ExitPlanMode` in `settings.json`, `model: fable`, `timeout: 600`. It reviews only plans whose first heading starts `# Plan:`, with code-reviewer's `plan` lens, at most two blocks per session. Every working plan is written in plan mode (`/implement-phase` and `/justin:team` enter it), and worker commits the reviewed `planFilePath` file unchanged. | Owner 11:28; ADR (ExitPlanMode input carries `plan` and `planFilePath`, E21). Requiring plan mode closes the gap where an optional plan mode would skip the gate; committing the reviewed file means the reviewed plan is the committed one. |
| D10 | **Shim (`.claude/hooks/aios`), `hook` branch, on #203's shim.** Never builds in the foreground; always exits 0; runs a fresh binary through #203's hard-link check. Stale (one background build) and dirty (no build) are treated as missing (Q5 default; the ADR, as amended by #229, leaves this to the owner). Missing, stale, dirty, unusable or failing binary: `path-guard` denies only the agent types its `--agent-type` names (every caller when it names none, and any payload with `agent_id` but no `agent_type`); every other hook prints nothing. | ADR "Shim"; exit 3 (the shim's old failure code) is non-blocking, so `path-guard` would fail open; an unconditional deny would block every edit in CI and on a fresh clone (Review Focus 1). |
| D11 | **One host QEMU lock for both teams**: `scripts/agent/qemu-lock.sh run --team team-build\|team-fix\|solo --mode boot\|quiet`. Exit 75 held, 76 foreign QEMU, 77 deferred under load (above 30 in `boot`, 3.0 in `quiet` after a settle wait). `status` reports `state=live\|starting\|dead`; `clear-stale` removes only a dead holder's lock with no QEMU running, and logs it. Quiet windows follow `QUIET-REQ`, `QUIET-ACK` or `QUIET-LATER`, owner pause of host load, lease, `QUIET-END`. Among agents only the verifier boots (guard rule 1). R4b's flock lease replaces it later. | Domain split: team-fix soaks too. On 2026-10-06 the contention was host load (load1 about 240), which gave false WEDGEs; the load gate turns those into DEFERRED. Restarts left wrapper processes dead; pid liveness detects that at once. v4: a stop TERMs every process but QEMU first and waits up to 15 s, so `aios soak` (which supervises QEMU in its own process group since R4) ends its boot itself instead of recording a QEMU killed under it. |
| D12 | **Toolchain.** `RUSTUP_AUTO_INSTALL=0` in the project env; guard rule 2 denies `rustup` installs, updates and removals to agents and asks on the main thread; after a merge that changes `rust-toolchain.toml`, `/merge-and-cleanup` runs `rustup install` once and says so in `MAIN-MOVED`. Open question Q3. | 2026-10-06: two sessions raced an implicit install into `~/.rustup` after #202, and an agent reinstalled the toolchain under the other team's build. E22. |
| D13 | **Ownership per branch**: GitHub labels `team-build` and `team-fix` on issues and PRs, and "Owned branches" in each handoff file. Overlaps are announced (`HELLO`, `HEADS-UP`); the second branch to merge resolves conflicts. Harness files change only on team-build's branches. Cross-team merge order exists only as `needs-human` gate issues. Message vocabulary: `HELLO`, `HEADS-UP`, `MAIN-MOVED`, `QUIET-REQ`, `QUIET-ACK`, `QUIET-LATER`, `QUIET-END`, `LOCK-STALE`, each mirroring durable state. | Both teams write kernel code (features versus fixes); 2026-10-06's real overlaps (crash-1b porting `lock_order.rs`, #219 conflicting in crash-1b) were per branch, not per path. |
| D14 | **Remember retired; agent memory retired.** Delete the project plugin line, `precompact-save.sh` and its PreCompact hook; `/justin:pause` writes `.remember/handoff-<key>.md` itself. No agent keeps `memory:`; its notes become lessons (T3). | O1, O2, no-legacy; `memory: project` splits notes across worktrees and gives reviewers write tools. |
| D15 | **`/merge-and-cleanup` = #221 plus a delta**: remove its agent-memory copy; add the gate-issue refusal, the quiet-lease refusal, the toolchain install and `MAIN-MOVED` (`D/.claude/skills/merge-and-cleanup/SKILL.md.delta-notes.md`). | #221 merged first, as the task required; no duplicate. |
| D16 | **Sequencing with #203: #203 merged first** (aa52bff, 2026-10-09, as #229). This PR adds the `hook` branch to #203's shim (`D/.claude/hooks/aios.v4`) and its tests to #203's `shim.rs`; no `is_stale` remains. Q2 is resolved by the merge order. | v3's whole-file shim draft would revert #203. |
| D17 | **Audit.** `/audit-loop` runs the committed workflow `.claude/workflows/audit-loop.js`, one round per run; skeptics by severity (three for must-fix, one otherwise); at most one audit per team at a time; no new round during the other team's quiet window. | Replaces the uncommitted v2.js engine (753 agents in one run on 2026-09-29). |
| D18 | **Step reviews: kernel-dev only** (the D8 gate). worker and doc-writer steps get no Fable step review; the audit covers them. The `repeat-error` main-session text ("the project expects the code-reviewer agent to diagnose the failure") is served by a new code-reviewer `diagnose` lens, so the hard-coded Rust text stays. | O4. No Rust rewording needed. |
| D19 | **`/review-pr-comments` stays** the PR loop until R2. The O5 note goes into the R2 design doc on `claude/justin-review-merge-loop`, not into this PR. | O5; no second PR loop. |
| D20 | **The Jev evaluation's home**: agent-loop.md "Model routing" (the 200-dispatch rule, the join, the 0–2 complexity scale, `-p` gaps, unjoined stops, not-launched dispatches, Workflow lenses not logged), and a routing-log count in `/justin:brief`. | The ADR requires the evaluation; v2's issue draft is gone with Part 2 moving into this PR. |
| D21 | **Checks: headless first, owner only for what needs a person.** T2 and T16 run every mechanism check with `claude -p` (settings hooks run there). `D/OWNER-PROBES.md` keeps only what needs two live terminals, the interactive plan-approval UI, or a person answering prompts. | The ADR requires real-run checks before reliance; most need no person. |

### Open questions for the owner (defaults apply if unanswered at T0)

| # | Question | Default |
| --- | --- | --- |
| Q1 | The ADR text puts the Fable step gate in kernel-dev's frontmatter as a `Stop` hook. This plan registers it in `settings.json` as a `SubagentStop` hook matched on `kernel-dev` (D8), for the reason you gave for `path-guard`: frontmatter hooks do not run in `-p`, SDK or untrusted sessions. Approve the settings registration? | Yes (settings only; the ADR amendment records it). |
| Q2 | Resolved: #203 merged first (aa52bff). The `hook` branch goes onto #203's shim (D16). | — |
| Q5 | (new, #229) The ADR's Shim requirement now leaves it to you what `aios hook` does with a stale or dirty binary: run it (a background build on stale), or treat it as missing (`path-guard` denies worker, the other three print nothing), the counterpart of `guard`'s ask. Stale includes a binary that fails its provenance stamp. | Treat as missing (implemented in `aios.v4`). |
| Q6 | (new, #223) #223's owner-approved note rejects `git -C * <subcmd>` allow rules (the first `*` can carry `-c core.pager=<cmd>`). v3's settings added eight. v4 drops them, so leads' and readers' `git -C <W>` calls prompt (or go to the auto-mode classifier), and headless `-p` runs refuse them. Accept per-worktree allows with the path written out (`Bash(git -C <W> diff *)`, safe because `-c` must precede the subcommand): on `--allowedTools` for the T12/T17 `-p` runs, and in `.claude/settings.local.json` per worktree for attended leads (rule 11)? | Yes: drop the wildcard rules (`settings.json.apply.jq.v4`); literal-path allows only. |
| Q7 | (new, #227/#230) `scripts/soak-matrix.sh` runs each arm's `scripts/soak-qemu.sh`, which R4 deleted, so it refuses every arm at or after 1a5c363; its help also relies on rustup installing a missing toolchain, which `RUSTUP_AUTO_INSTALL=0` stops. Who ports it, and does team-fix's next A/B wait for that? | team-build files an issue and ports it in its own PR, not this one; guard rule 1 keeps treating it as a QEMU start. |
| Q3 | Toolchain installs (D12): `RUSTUP_AUTO_INSTALL=0` for every Claude Code session, agents denied every `rustup` install, and the merging session installs once. Approve? | Yes. |
| Q4 | Skip the Claude code review on draft PRs, so it runs once on `ready_for_review` after the audit (`D/.github/workflows/claude-code-review.yml.patch-notes.md`). It is not a required check. Approve? | Yes. |

## Execution

- **Who.** The `team-build` lead session (the owner's attended terminal; launched today as it is: `AIOS_TEAM` has no meaning until this PR merges) is the controller. It runs superpowers:subagent-driven-development over T0–T18; the owner runs T19 with the leads after the merge.
- **Implementers** are `general-purpose` subagents on `model: sonnet` (the ADR routes this work to worker, which does not exist yet in a session started on `main`), spawned with `isolation: "worktree"`, `run_in_background: false` (most tasks touch `.claude/**`, which prompts), and the prompt: "W is `<main>/.claude/worktrees/harness-teams`, branch `claude/harness-teams`. First call EnterWorktree with path W (load it with ToolSearch `select:EnterWorktree`); then check `git rev-parse --show-toplevel` equals W. Never `cd` or `git -C` elsewhere. Do task Tn of the plan at `W/docs/knowledge/plans/2026-10-06-jl-two-team-harness.md`; the drafts are in `D`." T1 and T2 run before that mechanism is proven, so the controller does them itself (T1 is a copy and a commit; T2 changes no repository file until its lessons, which an implementer writes after H1–H3 pass).
- **Task reviewers** are `general-purpose` subagents on `model: sonnet` (O4 keeps Fable for kernel-dev steps and the audit), read-only, in the main checkout with `git -C "$W"`. For T4, T5 and T7 the reviewer prompt adds: "This is a guard. Attack it: find an input that makes it allow what it should deny, or fail open; do not compare it with the spec line by line" (lesson `2026-10-06-jl-path-guard-spec-correct-bypasses.md`).
- **Bootstrap.** A session loads `.claude/agents/`, settings and workflows from the checkout it starts in. The new agent types and hooks therefore exist only for a session started inside `W`. T12 and T16 start headless `claude -p` sessions inside `W` for that, a one-time exception to rule 03's "start sessions in the main checkout", recorded in the PR description.
- **Drafts.** Each task applies its drafts from `D` exactly, the `.v4` file where one exists; a deviation is recorded under "Issues Encountered" with its reason. Line anchors in the `.v4` drafts are `origin/main` at 1a5c363 (the v3 drafts without a `.v4` were checked there unchanged, `D/RECONCILE-v4.md`); re-anchor by text if `main` moved.

### Per-task gate

A task is done when all of these hold, in this order:

1. Its acceptance commands print what the task says. The implementer pastes each command and its output into its report.
2. **Docs gate:** `just tools` in `W` (only when the task changed `tools/`), then `(cd "$W" && AIOS_TOOLS_BIN="$W/target/tools/installed/aios" just docs-check)`. Until T17 it exits 1 and lists exactly one new finding: `knowledge-hygiene` `plans-not-empty` for `docs/knowledge/plans/2026-10-06-jl-two-team-harness.md`. From T17 on it prints `No new drift since baseline.` and exits 0. (Without `tools/` changes, `AIOS_TOOLS_BIN="$MAIN/target/tools/installed/aios"`. The shim runs an override without a stamp or freshness test; `just tools` in `W` stamps `source dirty`, which only matters to the shim's own binary, never to an override.)
3. **Tools gate** (tasks that change `tools/`): `cargo fmt --check -p aios-tools`, `cargo clippy -p aios-tools --all-targets -- -D warnings`, `cargo test -p aios-tools`, all exit 0, run in `W`.
4. **Guard gate** (tasks that change `.claude/hooks/`): `/usr/bin/python3 -m unittest discover -s .claude/hooks/tests` passes in `W`; `sh -n`, `dash -n` and `shellcheck -s sh` pass on changed POSIX `sh` files.
5. `git -C "$W" status --short` prints nothing after the commit; the commit is pushed (`git -C "$W" push -u origin claude/harness-teams`).
6. The task reviewer approves spec compliance and quality, or the controller records a ruling for each open finding.

## Progress

- [ ] **T0: preconditions and worktree** (controller; no commit)

  **Files:** none in the repository.

  - [ ] Step 1: main is current and clean.
    ```bash
    MAIN="$(cd "$(git rev-parse --path-format=absolute --git-common-dir)/.." && pwd)"   # the main checkout
    D=$MAIN/.git/aios-agent/teams-design-v3     # the drafts; W is set in Step 4
    git -C "$MAIN" fetch -q origin
    git -C "$MAIN" merge-base --is-ancestor 1a5c363 origin/main && echo base-ok
    git -C "$MAIN" status --short
    git -C "$MAIN" branch --show-current
    git -C "$MAIN" merge --ff-only origin/main
    ```
    Expected: `base-ok`; an empty status; `main`; a fast-forward or "Already up to date." Shell variables do not persist between Bash tool calls: every later command block re-sets `MAIN`, `D` and `W` first (lesson `2026-10-06-jl-testing-harness-shell-snippets.md`, T3).
  - [ ] Step 2: the CLI and the hook binary.
    ```bash
    claude --version
    (cd "$MAIN" && just tools) && tail -n 1 "$MAIN/target/tools/installed/aios.stamp"
    "$MAIN/target/tools/installed/aios" hook --help | grep -cE '^  (repeat-error|path-guard|route-shadow|route-outcome) '
    ```
    Expected: a version line (record it; 2.1.285 on 2026-10-06), `source clean`, and `4` (checked on 1a5c363, 2026-10-09).
  - [ ] Step 3: the owner answers Q1 and Q3–Q7 (or "defaults"; Q2 is resolved). Record the answers under "Decisions Made".
  - [ ] Step 4: create the worktree.
    ```bash
    git -C "$MAIN" worktree add .claude/worktrees/harness-teams -b claude/harness-teams origin/main
    W=$MAIN/.claude/worktrees/harness-teams
    git -C "$W" branch --show-current; git -C "$W" rev-parse HEAD; git -C "$MAIN" rev-parse origin/main
    ```
    Expected: `claude/harness-teams`, and two equal shas.

- [ ] **T1: the working plan, the team labels and the draft PR** (controller)

  **Files:** Create `docs/knowledge/plans/2026-10-06-jl-two-team-harness.md` (this file).

  - [ ] Step 1: `cp "$D/PLAN.md.v4" "$W/docs/knowledge/plans/2026-10-06-jl-two-team-harness.md"`, then `git -C "$W" add docs/knowledge/plans/2026-10-06-jl-two-team-harness.md` and `git -C "$W" commit -m "Harness teams: T1 — working plan"`.
  - [ ] Step 2: push, then create the two team labels (the owner approves; they are used from here on):
    ```bash
    git -C "$W" push -u origin claude/harness-teams
    gh label create team-build --color 1d76db --description 'Owned by the team-build lead (harness, tools, CI, features)'
    gh label create team-fix --color d93f0b --description 'Owned by the team-fix lead (crash fix, kernel bugs, cap lifetime)'
    ```
  - [ ] Step 3: open the draft PR with the label; record its number as `<n>`.
    ```bash
    gh pr create --draft --base main --head claude/harness-teams --label team-build \
      --title "Harness: two-team leads (team-build, team-fix) and model-routing Part 2 wiring" \
      --body "Implements docs/knowledge/plans/2026-10-06-jl-two-team-harness.md. Requirements: docs/knowledge/decisions/2026-10-06-jl-model-routing-hooks.md, Part 2. Draft until /audit-loop converges (T17)."
    ```
    (End the body with the PR attribution line from the system reminder.)
  - [ ] Acceptance: the docs gate shows only `plans-not-empty`; `gh pr view <n> --json isDraft,labels --jq '[.isDraft, ([.labels[].name] | join(","))]'` prints `[true,"team-build"]`.

- [ ] **T2: headless mechanism checks** (controller runs; an implementer writes the two lessons)

  **Files:** Modify the plan ("Issues Encountered"). Create `docs/knowledge/lessons/2026-10-06-jl-pinned-agents-enter-worktrees.md`, `docs/knowledge/lessons/2026-10-06-jl-model-routing-wiring-checks.md`.

  - [ ] Step 1: run `D/probe/HEADLESS.md.v4`, section "T2", exactly: setup, run A, run B, then the commands of checks H1–H8. Use the CLI recorded in T0.
  - [ ] Step 2: for each check, compare with its "Pass" cell. Apply the stated change for any failure before the task it affects: H2 (guard rule 4 row 2, T7), H3 (stop and re-plan D3), H4 (OP4), H5 (file an issue against `route-outcome`; the Jev evaluation waits on it), H6 (`main xhigh`: T19 owner step 3), H7 (model id, or caps fall back to `maxTurns`: amend D8, D9 and the gate prompts in `D/.claude/settings.json.apply.jq`), H8 (E6 moves to OP2).
  - [ ] Step 3: paste every command and output, with `claude --version`, under "Issues Encountered".
  - [ ] Step 4: an implementer writes the two lessons (rule 08 frontmatter, tags `tooling`): pinned-agents from kernel-dev/worktree-isolation-push (main checkout's `.claude/agent-memory/kernel-dev/`) plus H1–H3; model-routing-wiring-checks from H4–H8. Each lesson keeps the exact commands so agent-loop's "Re-check after each Claude Code update" can re-run them, and states the CLI version checked.
  - [ ] Acceptance: H1, H3 and H4 (foreground) pass; each other check is pass or has its recorded change applied; `git -C "$W" show --stat HEAD` lists the plan and the two lessons; the docs gate shows only `plans-not-empty`.

- [ ] **T3: agent-memory lessons**

  **Files:** Create the lessons in `D/docs/knowledge/lessons/agent-memory-migration.patch-notes.md.v4` §2 marked T3 (11, plus `2026-10-06-jl-host-clippy-shared-tests.md` only if its condition holds).

  - [ ] Step 1 (controller, before spawning): copy every store, read-only, as §1 says:
    ```bash
    H=$(git -C "$MAIN" rev-parse --path-format=absolute --git-common-dir)/aios-agent/agent-memory-harvest
    for c in "$MAIN" "$MAIN"/.claude/worktrees/*; do
      [ -d "$c/.claude/agent-memory" ] && mkdir -p "$H/$(basename "$c")" && cp -R "$c/.claude/agent-memory/." "$H/$(basename "$c")/"
    done
    find "$H" -name '*.md' ! -name MEMORY.md | wc -l
    ```
    Expected: 41 or more notes (17 main, 20 crash-1b, 4 already harvested from #218 and #221; counted 2026-10-09).
  - [ ] Step 2 (implementer): write each lesson from its sources in `$H`, re-checking every claim against `W` at HEAD, with rule 08 naming and frontmatter (`author: jl + claude`, `date: 2026-10-06`, `tags`, `status: final`).
  - [ ] Step 3: the host-clippy condition: `(cd "$W" && cargo clippy -p shared --all-targets 2>&1 | tail -n 5)`; write the lesson only if the failures it records still appear.
  - [ ] Acceptance:
    ```bash
    cd "$W" && for f in soak-noise-base-rates reading-soak-output ab-soak-method irq-path-codegen-hazards miri-single-seed \
      llvm-drops-write-only-statics tcg-irqs-at-tb-starts gates-in-worktrees verifying-harness-claims \
      testing-harness-shell-snippets ipc-capability-doc-touchpoints; do
      test -f "docs/knowledge/lessons/2026-10-06-jl-$f.md" || echo "missing $f"; done
    rg -n 'agent-memory|MEMORY\.md' docs/knowledge/lessons/2026-10-06-jl-*.md
    ```
    Expected: no output from either command. The docs gate shows only `plans-not-empty`.

- [ ] **T4: `path-guard --agent-type`** (TDD; implementer, then a guard-attacking reviewer)

  **Files:** Modify `tools/src/cmd/hook/path_guard.rs` (the `Args` struct at lines 31–38, `run` at line 66, the module doc at lines 1–3). Test `tools/tests/hook_path_guard.rs`. Draft: `D/tools/src/cmd/hook/path_guard.rs.patch-notes.md.v4`.

  **Interfaces:** Produces the CLI form `aios hook path-guard --agent-type <NAME> [--agent-type <NAME>...] --deny <PREFIX>...`, which T5's shim fallback parses and T8 registers.

  - [ ] Step 1: write the failing tests, appended to `tools/tests/hook_path_guard.rs`:
    ```rust
    /// `edit` plus the subagent fields a payload from inside an agent carries.
    fn edit_by(cwd: &Path, path: &str, agent_type: Option<Value>, agent_id: Option<&str>) -> Value {
        let mut v = edit(cwd, path);
        if let Some(t) = agent_type {
            v["agent_type"] = t;
        }
        if let Some(id) = agent_id {
            v["agent_id"] = json!(id);
        }
        v
    }

    const WORKER: &[&str] = &["--agent-type", "worker"];

    #[test]
    fn the_named_agent_type_is_guarded() {
        let dir = repo("at-worker");
        let p = dir.join("kernel/src/lib.rs");
        let run = guard(WORKER, &edit_by(&dir, p.to_str().unwrap(), Some(json!("worker")), Some("a1")), &dir);
        assert!(deny_reason(&run).contains("kernel/src/lib.rs"));
    }

    #[test]
    fn another_agent_type_gets_no_decision() {
        let dir = repo("at-kernel-dev");
        let p = dir.join("kernel/src/lib.rs");
        assert_no_decision(&guard(WORKER, &edit_by(&dir, p.to_str().unwrap(), Some(json!("kernel-dev")), Some("a1")), &dir));
    }

    #[test]
    fn the_main_thread_gets_no_decision() {
        let dir = repo("at-main");
        let p = dir.join("kernel/src/lib.rs");
        assert_no_decision(&guard(WORKER, &edit_by(&dir, p.to_str().unwrap(), None, None), &dir));
    }

    #[test]
    fn a_subagent_without_an_agent_type_is_denied_anywhere() {
        let dir = repo("at-unidentified");
        let p = dir.join("docs/a.md");
        let reason = deny_reason(&guard(WORKER, &edit_by(&dir, p.to_str().unwrap(), None, Some("a1")), &dir));
        assert!(reason.contains("agent_type"), "{reason}");
    }

    #[test]
    fn agent_types_are_exact_names() {
        let dir = repo("at-case");
        let p = dir.join("kernel/src/lib.rs");
        assert_no_decision(&guard(WORKER, &edit_by(&dir, p.to_str().unwrap(), Some(json!("Worker")), Some("a1")), &dir));
    }

    #[test]
    fn the_named_agent_type_may_edit_outside_the_prefixes() {
        let dir = repo("at-docs");
        let p = dir.join("docs/a.md");
        assert_no_decision(&guard(WORKER, &edit_by(&dir, p.to_str().unwrap(), Some(json!("worker")), Some("a1")), &dir));
    }

    #[test]
    fn the_filter_runs_before_path_resolution() {
        let dir = repo("at-before-resolve");
        let mut payload = edit_by(&dir, "kernel/src/lib.rs", Some(json!("kernel-dev")), Some("a1"));
        payload["cwd"] = json!(dir.join("no-such-dir"));
        assert_no_decision(&guard(WORKER, &payload, &dir));
    }

    #[test]
    fn an_empty_agent_type_flag_denies_every_checked_call() {
        let dir = repo("at-empty-flag");
        let p = dir.join("docs/a.md");
        let reason = deny_reason(&guard(&["--agent-type", " "], &edit_by(&dir, p.to_str().unwrap(), Some(json!("kernel-dev")), Some("a1")), &dir));
        assert!(reason.contains("--agent-type"), "{reason}");
    }

    #[test]
    fn every_named_agent_type_is_guarded() {
        let dir = repo("at-two");
        let p = dir.join("kernel/src/lib.rs");
        let flags = &["--agent-type", "worker", "--agent-type", "doc-writer"];
        deny_reason(&guard(flags, &edit_by(&dir, p.to_str().unwrap(), Some(json!("doc-writer")), Some("a1")), &dir));
    }

    #[test]
    fn a_wrong_typed_agent_type_counts_as_absent() {
        let dir = repo("at-number");
        let p = dir.join("docs/a.md");
        deny_reason(&guard(WORKER, &edit_by(&dir, p.to_str().unwrap(), Some(json!(7)), Some("a1")), &dir));
    }

    #[test]
    fn a_session_launched_as_the_agent_is_guarded() {
        let dir = repo("at-dash-agent");
        let p = dir.join("kernel/src/lib.rs");
        deny_reason(&guard(WORKER, &edit_by(&dir, p.to_str().unwrap(), Some(json!("worker")), None), &dir));
    }

    #[test]
    fn unchecked_tools_are_filtered_before_the_agent() {
        let dir = repo("at-read");
        let mut payload = edit_by(&dir, dir.join("kernel/src/lib.rs").to_str().unwrap(), Some(json!("worker")), Some("a1"));
        payload["tool_name"] = json!("Read");
        assert_no_decision(&guard(WORKER, &payload, &dir));
    }

    #[test]
    fn an_empty_agent_type_with_an_agent_id_is_denied() {
        let dir = repo("at-empty-type");
        let p = dir.join("docs/a.md");
        deny_reason(&guard(WORKER, &edit_by(&dir, p.to_str().unwrap(), Some(json!("")), Some("a1")), &dir));
    }
    ```
  - [ ] Step 2: `(cd "$W" && cargo test -p aios-tools --test hook_path_guard)`. Expected: the new tests fail (clap rejects `--agent-type`, so `guard` sees exit 2); the existing tests pass.
  - [ ] Step 3: implement. In `Args`:
    ```rust
    /// Agent type this guard decides for (repeatable). Without it, every caller is checked.
    #[arg(long = "agent-type", value_name = "NAME")]
    pub agent_type: Vec<String>,
    ```
    In `run`, right after the `target_path` early return and the prefix validation:
    ```rust
    match scope(&args.agent_type, input)? {
        Scope::Check => {}
        Scope::Skip => return Ok(None),
        Scope::Unidentified => {
            return Ok(Some(pre_tool_use_deny(&format!(
                "this call comes from a subagent (agent_id is set) whose agent_type is missing, so \
                 path-guard cannot tell whether it applies (it applies to: {}); denied",
                args.agent_type.join(", ")
            ))))
        }
    }
    ```
    and below `run`:
    ```rust
    /// Who this call is for, under `--agent-type`.
    enum Scope {
        /// Check the target as before.
        Check,
        /// Not one of the named agent types: no decision.
        Skip,
        /// A subagent that names no type: deny (fail closed).
        Unidentified,
    }

    /// Without `--agent-type` every caller is checked. With it, only the named types
    /// are; an empty or wrong-typed `agent_type` counts as absent, and an absent one
    /// with an `agent_id` cannot be identified. An empty flag value is an error, so a
    /// registration mistake denies every checked call instead of guarding nobody.
    fn scope(types: &[String], input: &HookInput) -> Result<Scope> {
        if types.is_empty() {
            return Ok(Scope::Check);
        }
        if let Some(bad) = types.iter().find(|t| t.trim().is_empty()) {
            bail!("--agent-type {bad:?} is empty");
        }
        let agent_type = input.agent_type.as_deref().filter(|t| !t.is_empty());
        let agent_id = input.agent_id.as_deref().filter(|id| !id.is_empty());
        Ok(match (agent_type, agent_id) {
            (Some(t), _) if types.iter().any(|name| name == t) => Scope::Check,
            (Some(_), _) | (None, None) => Scope::Skip,
            (None, Some(_)) => Scope::Unidentified,
        })
    }
    ```
    Update the module doc's first lines to: "Registered on `PreToolUse` for the edit tools in `.claude/settings.json`, with `--agent-type worker`. Fails closed: an error becomes a deny (see `OnError`)." and add one paragraph on the filter and its fail-closed case.
  - [ ] Step 4: `(cd "$W" && cargo fmt -p aios-tools && cargo test -p aios-tools --test hook_path_guard)`. Expected: all pass, the old tests unchanged.
  - [ ] Acceptance: the tools gate; `(cd "$W" && just tools && target/tools/installed/aios hook path-guard --help | grep -- '--agent-type')` prints the flag's help line; `rg -n 'frontmatter of the' tools/src/cmd/hook/path_guard.rs` prints nothing; the docs gate (branch binary) shows only `plans-not-empty`.

- [ ] **T5: the shim's `hook` branch** (TDD; implementer, then a guard-attacking reviewer)

  **Files:** Modify `.claude/hooks/aios` (replace with `D/.claude/hooks/aios.v4`, which is `origin/main`'s #203 shim plus the `hook` branch). Test `tools/tests/shim.rs`. Draft: `D/.claude/hooks/aios.patch-notes.md.v4` (the test table, rows 1–16).

  **Interfaces:** Consumes T4's flag form. Produces: `aios hook ...` always exits 0, never builds in the foreground, and runs only a fresh binary (Q5 default: stale and dirty fall back like missing).

  - [ ] Step 1: add `run_stdin_at(&self, shim: &Path, args, envs, stdin: &str) -> Output` to `Sandbox` in `tools/tests/shim.rs`, next to `run_at` (same `isolated(&mut cmd)`, `self.path_env()` and `current_dir`; stdin piped and written as `guard_passes_on_only_the_binarys_own_0_and_2` does), plus `run_stdin(args, envs, stdin)` for the sandbox's own shim, a `PG` constant (`["hook", "path-guard", "--agent-type", "worker", "--deny", "kernel/"]`), a `payload(fields)` helper (a `PreToolUse` `Write` of `/x/kernel/a.rs` plus `fields`) and `is_deny(&Output)`. Use #203's helpers, not v3's: `FAKE_CARGO_DELAY` (not `FAKE_JUST_DELAY`), `built()`, `no_build_started()`, `wait_for_background_build()` (not `just_log()`), and `install(Some(source), fresh)` for any custom binary (a hand-written binary fails the stamp).
  - [ ] Step 2: write the failing tests, one per row of the draft's table (rows 1–16; row 12 extends the existing `a_linked_worktree_fails_closed_when_git_cannot_name_the_main_checkout` fixture with `hook route-outcome` and `{}` on stdin).
  - [ ] Step 3: `(cd "$W" && cargo test -p aios-tools --test shim)`. Expected: the new tests fail on `main`'s shim (`hook` takes the `aios <other>` path: a foreground build, exit 3, the old binary's output), and the existing tests pass.
  - [ ] Step 4: `cp "$D/.claude/hooks/aios.v4" "$W/.claude/hooks/aios"` (mode 755). This is the first `.claude/` edit by a foreground implementer: the owner observes `D/OWNER-PROBES.md.v4` OP3 here (and again in T7 and T8). If the owner answered Q5 "run it", apply the two-line change in the draft notes' Q5 section and flip rows 10, 14 and 15 first.
  - [ ] Step 5: `(cd "$W" && cargo fmt -p aios-tools && cargo test -p aios-tools --test shim)`. Expected: all pass. (The v4 draft was smoke-tested on 2026-10-09 in a scratch repository with `D/.claude/hooks/aios.v4.smoke.sh`: rows 1–11 and 13–16 and `guard` on a dirty stamp behaved as the table says.)
  - [ ] Acceptance: the tools gate; `sh -n`, `dash -n`, `shellcheck -s sh` on `.claude/hooks/aios` exit 0; `git -C "$W" diff origin/main --stat -- .claude/hooks/aios tools/tests/shim.rs` lists only those two files; `git -C "$W" diff origin/main -- .claude/hooks/aios | grep -c '^-[^-]'` prints `2` or less (#203's lines stay: only the `stop` comment and its case list change).

- [ ] **T6: the host QEMU lock wrapper**

  **Files:** Create `scripts/agent/qemu-lock.sh` (from `D/scripts/agent/qemu-lock.sh.v4`, mode 755).

  - [ ] Step 1: copy the draft; `chmod 755`.
  - [ ] Step 2: the smoke test, in a scratch repository outside `W` (the fake QEMU is a symlink, because a copied system binary is killed by code signing):
    ```sh
    S=$(mktemp -d) && cd "$S" && git init -q -b main . && git -c user.email=a@b -c user.name=a commit -q --allow-empty -m s
    cp "$W/scripts/agent/qemu-lock.sh" q.sh && mkdir fake && ln -s /bin/sleep fake/qemu-system-aarch64
    sh q.sh status
    AIOS_QEMU_LOCK_LOADAVG=1 sh q.sh run --team team-fix --mode boot --label t1 --eta-min 1 -- sleep 3 & sleep 1
    sh q.sh run --team team-build --mode boot --label t2 --eta-min 1 -- true; echo rc=$?
    wait
    AIOS_QEMU_LOCK_LOADAVG=40 dash q.sh run --team team-build --mode boot --label t3 --eta-min 1 -- echo NOT-RUN; echo rc=$?
    AIOS_QEMU_LOCK_LOADAVG=5 dash q.sh run --team team-fix --mode quiet --settle-min 0 --label t4 --eta-min 1 -- echo NOT-RUN; echo rc=$?
    AIOS_QEMU_LOCK_LOADAVG=1 dash q.sh run --team solo --mode boot --label t5 --eta-min 1 -- echo ran-t5; echo rc=$?
    fake/qemu-system-aarch64 20 & F=$!
    AIOS_QEMU_LOCK_LOADAVG=1 dash q.sh run --team team-fix --mode quiet --label t6 --eta-min 1 -- echo NOT-RUN; echo rc=$?
    kill $F
    mkdir .git/aios-agent/qemu.lock && printf 'team=team-fix\npid=999999\n' >.git/aios-agent/qemu.lock/owner
    sh q.sh status; sh q.sh clear-stale; echo rc=$?; sh q.sh status
    AIOS_QEMU_LOCK_LOADAVG=1 dash q.sh run --team team-build --mode boot --label t7 --eta-min 1 -- sh -c 'sleep 31' & sleep 1
    kill -TERM "$(sed -n 's/^pid=//p' .git/aios-agent/qemu.lock/owner)"; sleep 1; sh q.sh status; pgrep -f 'sleep 31' || echo no-sleep-left
    sh q.sh run --team ship --mode boot --label x --eta-min 1 -- true; echo rc=$?
    ```
  - [ ] Step 3 (v4): the harness-first stop check: `sh "$D/scripts/agent/qemu-lock.sh.v4.t8-smoke.sh" "$W/scripts/agent/qemu-lock.sh" "$(mktemp -d)/t8"` prints `rc=143 qemu-running-at-harness-term`, `free` and `no-qemu-left` (v3's leaf-first stop printed `qemu-already-gone-at-harness-term`; run 2026-10-09, three times each).
  - [ ] Acceptance: `shellcheck -s sh scripts/agent/qemu-lock.sh` and `dash -n scripts/agent/qemu-lock.sh` exit 0, Step 3 prints its three lines, and the smoke test prints, in order: `free`; the t1 owner lines with `state=live` and `rc=75`; `deferred: load 40 is above 30 (boot mode)` and `rc=77`; `deferred: load 5 is above 3.0 (quiet mode)` and `rc=77`; `ran-t5`, `rc=0`; the fake QEMU's process line and `rc=76` (no `NOT-RUN` anywhere); `state=dead`, `cleared`, `rc=0`, `free`; `free`, `no-sleep-left`; `--team must be team-build, team-fix or solo`, `rc=2`. (Run on the v3 draft on 2026-10-06 and on the v4 draft on 2026-10-09: exactly this output.) `cat "$S/.git/aios-agent/qemu-lock.log"` shows the cleared owner lines. The docs gate shows only `plans-not-empty`.

- [ ] **T7: guard rules 1–4** (implementer in the foreground; guard-attacking reviewer)

  **Files:** Modify `.claude/hooks/git-push-guard.py`. Test `.claude/hooks/tests/test_git_push_guard.py`. Draft: `D/.claude/hooks/git-push-guard.py.patch-notes.md.v4` (every row and its exact deny text).

  - [ ] Step 1: write one failing test per table row of the draft (rule 1 rows 1–7 plus the lock directory without an owner file; rule 2 rows 1–3 with `rustup +nightly component add rust-src`, `rustup toolchain install`, `rustup install` and `sh -c 'rustup update'`; rule 3 rows 1–4 plus an unreadable agent file; rule 4 rows 1–3; the `--classify` exclusion for `just soak` and `target/tools/installed/aios soak`, and a denied `target/tools/installed/aios soak runs=1`). Each test builds the payload the guard reads (`tool_name`, `tool_input`, `cwd`, optional `agent_id` and `agent_type`) and the environment (`AIOS_TEAM`), and asserts the decision and a substring of the reason. If T2's H2 failed, leave rule 4 row 2 and its test out and record it.
  - [ ] Step 2: `(cd "$W" && /usr/bin/python3 -m unittest discover -s .claude/hooks/tests)`. Expected: the new tests fail, the existing 55 pass.
  - [ ] Step 3: implement the four rules in the existing analyzer (reuse its command resolver for `sh -c`, `eval`, aliases and script files; any exception still becomes `ask`), route `Agent` and `EnterWorktree` payloads to rules 3 and 4, and extend the module docstring's table and first line.
  - [ ] Step 4: re-run the tests. Expected: all pass.
  - [ ] Acceptance: the guard gate; the test count grows by at least 23 (`... -v 2>&1 | grep -c ' ok$'` before and after); `rg -n 'only the verifier starts QEMU|never pattern-kill QEMU|agents never change the toolchain|becomes a teammate|team leads stay in the main checkout' .claude/hooks/git-push-guard.py` finds each text.

- [ ] **T8: settings** (the owner approves each block; implementer in the foreground)

  **Files:** Modify `.claude/settings.json`. Delete `.claude/hooks/precompact-save.sh`. Drafts: `D/.claude/settings.json.apply.jq.v4`, `D/.claude/settings.json.patch-notes.md.v4`, `D/.claude/hooks/precompact-save.sh.DELETE.md`.

  - [ ] Step 1: show the owner the diff: `S=$(mktemp -d); jq -f "$D/.claude/settings.json.apply.jq.v4" "$W/.claude/settings.json" > "$S/settings.new.json" && diff <(jq -S . "$W/.claude/settings.json") <(jq -S . "$S/settings.new.json")`, block by block (env, effortLevel, hooks, allow, ask, deny, enabledPlugins).
  - [ ] Step 2: after approval, write it: the implementer copies `$S/settings.new.json` over `W/.claude/settings.json` with the Write tool (the prompt is the owner's approval of that exact content), and `git rm .claude/hooks/precompact-save.sh`.
  - [ ] Acceptance, run in `W` against `.claude/settings.json`:
    ```bash
    F=.claude/settings.json
    jq -e '.env | (has("CLAUDE_CODE_EFFORT_LEVEL") or has("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS")) | not' $F
    jq -e '.env.RUSTUP_AUTO_INSTALL == "0" and .effortLevel == "high"' $F
    jq -r '.hooks | keys | join(",")' $F
    jq -r '[.hooks[][] | (.matcher // "-") + "=" + ([.hooks[] | .type + ":" + ((.command // "") | sub(".*aios hook ";"")) + (if .async then "(async)" else "" end) + (if .model then "(" + .model + ")" else "" end)] | join(";"))] | .[]' $F
    jq -e '.enabledPlugins | has("remember@claude-plugins-official") | not' $F
    jq -e '[.permissions.allow[] | select(test("qemu-system-aarch64 \\*|git stash|force-with-lease|git rebase|precompact|^Bash\\(rustup \\*\\)"))] | length == 0' $F
    jq -e '(.permissions.deny | index("Bash(pkill *qemu*)") != null) and (.permissions.ask | index("Write(**/tools/src/cmd/hook/**)") != null) and (.permissions.ask | index("Bash(rustup install*)") != null)' $F
    test ! -e .claude/hooks/precompact-save.sh && echo gone
    jq -e '[.permissions.allow[] | select(startswith("Bash(git -C "))] | length == 0' $F
    jq -e '([.permissions.allow[] | select(test("release/aios|soak-qemu"))] | length == 0) and (.permissions.allow | index("Bash(AIOS_TOOLS_BIN=target/tools/installed/aios just docs-check)") != null)' $F
    jq -e '(.permissions.deny | index("Edit(**/target/tools/**)") != null) and (.permissions.deny | index("Bash(git log *--output*)") != null) and (.permissions.ask | index("Bash(just -*)") != null) and (.permissions.allow | index("Bash(rust-analyzer --version)") != null)' $F
    ```
    Expected: `true`, `true`; `PostToolUse,PostToolUseFailure,PreToolUse,SessionStart,SubagentStop`; exactly these eleven lines:
    ```text
    -=command:"$CLAUDE_PROJECT_DIR"/.claude/hooks/setup-dev-env.sh
    Bash|Monitor|Agent|EnterWorktree=command:/usr/bin/python3 "$CLAUDE_PROJECT_DIR"/.claude/hooks/git-push-guard.py
    Edit|Write|MultiEdit|NotebookEdit=command:path-guard --agent-type worker --deny kernel/ --deny uefi-stub/ --deny shared/
    Agent=command:route-shadow(async)
    ExitPlanMode=agent:(fable)
    Bash=command:repeat-error
    Agent=command:route-outcome(async)
    Bash=command:repeat-error
    Agent=command:route-outcome(async)
    -=command:route-outcome(async)
    kernel-dev=agent:(fable)
    ```
    then `true`, `true`, `true`, `gone`, `true`, `true`, `true` (the last three: v4's #223 and #203 checks, and #223/#229's rules kept). (All pass on the v4 jq output from 1a5c363, `D/.claude/settings.json.v4-output-1a5c363.json`, 2026-10-09.) If T2's H7 changed the gate model to a full id, the `(fable)` lines show that id.

- [ ] **T9: rules**

  **Files:** Create `.claude/rules/11-teams.md` (from `D/.claude/rules/11-teams.md`). Modify `.claude/rules/02-quality-gates.md`, `03-git-workflow.md`, `04-phase-workflow.md`, `08-knowledge-hive.md` per `D/.claude/rules/02-03-04-08.patch-notes.md`.

  - [ ] Step 1: add rule 11; apply the four rule patches.
  - [ ] Acceptance:
    ```bash
    cd "$W"
    rg -n 'TodoWrite|Team-lead updates|brew upgrade qemu|rebase open worktrees|until it is rebased' .claude/rules
    rg -n 'Objdump \| .cargo objdump|QEMU \| .just run' .claude/rules/02-quality-gates.md
    rg -c 'isolation: "worktree"|EnterWorktree\(\{path|qemu-lock.sh|RUSTUP_AUTO_INSTALL|team-build|team-fix' .claude/rules/11-teams.md
    rg -lF 'qemu-lock.sh run' .claude/rules/11-teams.md .claude/rules/02-quality-gates.md
    rg -n 'team-lead' .claude/rules | rg -v 'not `team-\*`, `team-lead`'
    ```
    Expected: nothing; nothing; a count of 6 or more; both files; nothing. The docs gate shows only `plans-not-empty`.

- [ ] **T10: agents, and the Agents table**

  **Files:** Replace `.claude/agents/{kernel-dev,doc-writer,verifier,code-reviewer,doc-auditor}.md`; create `worker.md` and `skeptic.md` (all from `D/.claude/agents/`, using `verifier.md.v4`, `worker.md.v4` and `doc-writer.md.v4` for those three); delete `team-lead.md` (`D/.claude/agents/team-lead.md.DELETE.md`). Modify `.claude/CLAUDE.md`: the Agents table and the layout `agents/` line only (`D/.claude/CLAUDE.md.patch-notes.md.v4`, "Lines 165–175" agents line and "Lines 217–224"), because docs-check's harness-tables check compares them with the files.

  - [ ] Step 1: copy the seven files; `git rm .claude/agents/team-lead.md`; apply the two CLAUDE.md edits.
  - [ ] Acceptance:
    ```bash
    cd "$W"
    ls .claude/agents | tr '\n' ' '
    rg -n '^(memory|isolation|background|permissionMode|hooks):|MultiEdit|ListAgents|\bSkill\b' .claude/agents
    for f in .claude/agents/*.md; do printf '%s %s %s\n' "$(basename "$f" .md)" "$(sed -n 's/^model: //p' "$f")" "$(sed -n 's/^effort: //p' "$f")"; done
    rg -c 'EnterWorktree' .claude/agents/kernel-dev.md .claude/agents/worker.md .claude/agents/doc-writer.md .claude/agents/verifier.md
    rg -n '^## Lens: (plan|rules|bugs|diagnose)$' .claude/agents/code-reviewer.md | wc -l
    rg -n 'team-lead' .claude/agents .claude/CLAUDE.md
    ```
    Expected: `code-reviewer.md doc-auditor.md doc-writer.md kernel-dev.md skeptic.md verifier.md worker.md`; nothing; exactly
    ```text
    code-reviewer fable high
    doc-auditor sonnet medium
    doc-writer opus high
    kernel-dev opus high
    skeptic sonnet medium
    verifier sonnet medium
    worker sonnet high
    ```
    a count of 1 or more for each of the four files; `4`; nothing. The docs gate shows only `plans-not-empty` (harness-tables and pointer-doctor clean).

- [ ] **T11: skills, and the Skills table**

  **Files:** Create `.claude/skills/justin/skills/team/SKILL.md`. Replace `.claude/skills/justin/skills/{pause,brief,start}/SKILL.md`, `.claude/skills/justin/.claude-plugin/plugin.json`, `.claude/skills/audit-loop/SKILL.md`. Delete `.claude/skills/build-team/`. Modify `.claude/skills/merge-and-cleanup/SKILL.md` (`D/.claude/skills/merge-and-cleanup/SKILL.md.delta-notes.md`) and `implement-phase`, `verify-phase`, `review-pr-comments`, `write-arch-doc`, `generate-phase-doc` (`D/.claude/skills/skills.patch-notes.md`). Modify `.claude/CLAUDE.md`: the Skills table and the layout `skills/` lines (`D/.claude/CLAUDE.md.patch-notes.md.v4`, "Lines 230–241" and the `skills/` lines).

  - [ ] Step 1: apply the files and notes. If T2's H1 left temporary worktrees behind, add to the team SKILL's "Restart" bullet: "remove clean `.claude/worktrees/agent-*` worktrees (`git worktree remove`, never `--force`)".
  - [ ] Acceptance:
    ```bash
    cd "$W"
    rg -n 'remember:remember|TodoWrite|build-team|team-lead|just run 2>&1|cargo objdump|brew upgrade|agent-memory|agent memory' .claude/skills
    rg -n 'handoff-(build|ship|solo)\.md|handoff-<role>|aios-owner' .claude/skills
    ls .claude/skills/justin/skills | tr '\n' ' '
    jq -r .description .claude/skills/justin/.claude-plugin/plugin.json
    rg -c 'qemu-lock.sh status|rustup install|MAIN-MOVED|gated by' .claude/skills/merge-and-cleanup/SKILL.md
    rg -n 'EnterPlanMode|ExitPlanMode' .claude/skills/implement-phase/SKILL.md | wc -l
    ```
    Expected: nothing; nothing; `brief doctor pause start team`; a description naming the five skills; 4 or more; 2 or more. The docs gate shows only `plans-not-empty`.

- [ ] **T12: the audit workflow**

  **Files:** Create `.claude/workflows/audit-loop.js`. Draft: `D/.claude/workflows/audit-loop.js.patch-notes.md`; source `<git-common-dir>/aios-agent/audit/branch-audit-loop.v2.js`.

  - [ ] Step 1: the implementer loads the `workflow-authoring` skill, then writes the workflow per the notes (one round, typed lenses, skeptics by severity, a pool of six, the ledger args, the return shape).
  - [ ] Step 2: run one docs-mode round headless inside `W` (the only place `skeptic` and the new `code-reviewer` exist; Execution, Bootstrap):
    ```bash
    S=$(mktemp -d); BASE=$(git -C "$W" merge-base origin/main HEAD); HEAD_SHA=$(git -C "$W" rev-parse HEAD)
    # v4 (Q6): no `git -C *` allow rules exist (#223), and `-p` refuses prompts, so the readers' git calls are
    # allowed for this run only, with W written out: nothing can sit between `-C <W>` and the subcommand.
    cd "$W" && env -u CLAUDE_CODE_EFFORT_LEVEL -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT claude -p --model opus --output-format json \
      --allowedTools "Bash(git -C $W status *)" "Bash(git -C $W log *)" "Bash(git -C $W diff *)" "Bash(git -C $W show *)" \
        "Bash(git -C $W rev-parse *)" "Bash(git -C $W merge-base *)" "Bash(git -C $W grep *)" -- \
      "Run the workflow named audit-loop with the Workflow tool, with args {\"worktree\":\"$W\",\"branch\":\"claude/harness-teams\",\"base\":\"$BASE\",\"head\":\"$HEAD_SHA\",\"mode\":\"docs\",\"context\":\"T12 acceptance run\",\"gates\":\"(docs-only acceptance run)\",\"fixed\":[],\"refuted\":[]}. Print its return value as JSON and nothing else." >"$S/t12.json"
    jq -r .result "$S/t12.json" | jq -r 'keys | join(",")'
    jq -r .result "$S/t12.json" | jq -c .lens_failures
    ```
  - [ ] Acceptance: `rg -n 'model:|effort:' .claude/workflows/audit-loop.js` prints nothing; `rg -c 'agentType' .claude/workflows/audit-loop.js` is 3 or more; the run prints `complete,in_diff,lens_failures,pre_existing,refuted,uncertain` and `[]` (a lens failing with an unknown agent type means the run did not start inside `W`). The docs gate shows only `plans-not-empty`.

- [ ] **T13: brief.sh**

  **Files:** Modify `scripts/agent/brief.sh` per `D/scripts/agent/brief.sh.patch-notes.md.v4`.

  - [ ] Step 1: apply the notes.
  - [ ] Acceptance:
    ```bash
    cd "$W"
    shellcheck scripts/agent/brief.sh && bash -n scripts/agent/brief.sh && echo lint-ok
    bash scripts/agent/brief.sh --no-fetch | rg -n '^## (Host QEMU lock|Routing log|Handoffs)$' | wc -l
    rg -n 'now\.md|remember\.md' scripts/agent/brief.sh
    T=$(mktemp -d); mkdir "$T/r"; printf 'x\n' >"$T/r/handoff-solo-a.md"; printf 'y\n' >"$T/r/handoff-team-fix.md"
    sed "s#^HANDOFF_DIR=.*#HANDOFF_DIR=$T/r#" scripts/agent/brief.sh >"$T/brief.sh"
    bash "$T/brief.sh" --no-fetch | sed -n '/^## Handoffs$/,/^## [^H]/p' | grep -E '^(solo-a|team-fix) \('
    ```
    Expected: `lint-ok`; `3`; nothing from the first `rg`; two lines, one starting `solo-a (` and one `team-fix (`. The docs gate shows only `plans-not-empty`.

- [ ] **T14: remember-retirement references and the review workflow**

  **Files:** Modify `.gitignore` (`D/.gitignore.patch-notes.md`), `docs/knowledge/discussions/2026-09-22-jl-rust-agent-tools.md` (`D/docs/knowledge/discussions/...patch-notes.md.v4`), `.github/workflows/claude-code-review.yml` (`D/.github/workflows/claude-code-review.yml.patch-notes.md`, only if Q4 is yes).

  - [ ] Step 1: apply the three notes.
  - [ ] Acceptance:
    ```bash
    cd "$W"
    rg -n --hidden -g '!.git' -g '!target' -g '!docs/phases' -g '!docs/knowledge/plans' 'remember:remember|remember@claude|precompact-save|\.remember/(now|remember)\.md' .
    rg -n 'agent-memory' .gitignore
    rg -n 'settings.local.json' .gitignore
    actionlint .github/workflows/claude-code-review.yml && echo actionlint-ok
    ```
    Expected: nothing (CLAUDE.md, agent-loop and developer-guide hits are fixed in T15, so run this check again in T15); nothing; one line; `actionlint-ok`. The docs gate shows only `plans-not-empty`.

- [ ] **T15: docs**

  **Files:** Modify `.claude/CLAUDE.md` (the remaining sections of `D/.claude/CLAUDE.md.patch-notes.md.v4`), `docs/project/agent-loop.md`, `docs/project/developer-guide.md`, `docs/project/ai-agent-context.md` (their notes in `D/docs/project/`: the `.v4` notes for agent-loop and developer-guide).

  - [ ] Step 1: apply the four notes.
  - [ ] Acceptance:
    ```bash
    cd "$W"
    rg -n 'src/cmd/hook/' .claude/CLAUDE.md docs/project/developer-guide.md
    rg -n '11-teams' .claude/CLAUDE.md docs/project/developer-guide.md
    rg -n '^## (Teams|Model routing)$' docs/project/agent-loop.md
    rg -n 'TYPESAFE_API_KEY|AIOS_ROUTE_SHADOW|route-outcome.jsonl|200 or more' docs/project/agent-loop.md | wc -l
    rg -n 'Push immediately|Report completion to team-lead|Single team lead|CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS|Doc-auditor loops' .claude/CLAUDE.md docs/project
    rg -n --hidden -g '!.git' -g '!target' -g '!docs/phases' -g '!docs/knowledge/plans' 'remember:remember|remember@claude|precompact-save|\.remember/(now|remember)\.md' .
    ```
    Expected: a line in each file; a line in each file; both headings; 4 or more; nothing; nothing. The docs gate shows only `plans-not-empty`, with harness-tables, pointer-doctor and repo-paths clean.

- [ ] **T16: headless end-to-end checks, the wiring lesson and the ADR amendment**

  **Files:** Modify `docs/knowledge/lessons/2026-10-06-jl-model-routing-wiring-checks.md` (append E1–E7), `docs/knowledge/decisions/2026-10-06-jl-model-routing-hooks.md` (dated amendment, `D/docs/knowledge/decisions/2026-10-06-jl-model-routing-hooks.md.amendment-notes.md.v4`), the plan ("Issues Encountered").

  - [ ] Step 1 (controller): run `D/probe/HEADLESS.md.v4`, section "T16": setup, runs E, F, G, the checks E1–E7, teardown.
  - [ ] Step 2: any failing check goes back to the task that owns it (E1, E7: T4/T5/T8; E2: T7; E3, E4: T8; E5: T8/T10; E6: T8/T10/T11) as a fix round, then T16 runs again.
  - [ ] Step 3 (owner): run `D/OWNER-PROBES.md.v4` OP2, and OP4 if T2's H4 found no background `SubagentStop`; paste the report blocks under "Issues Encountered". A failing OP2 is a fix round for T8/T11 like Step 2.
  - [ ] Step 4 (implementer): append the results (E1–E7, OP2, OP4) to the wiring lesson; write the ADR amendment with every `<...>` field filled.
  - [ ] Acceptance: E1–E7 pass (E6 may be recorded as moved to OP2 when T2's H8 found no ExitPlanMode in `-p`); the OP2 report says `result: pass`; `rg -n '<(date of T16|n|date|version|result|date of the Q5 answer|run it|stale or dirty)' docs/knowledge/decisions/2026-10-06-jl-model-routing-hooks.md` prints nothing; `git -C "$W" diff origin/main --stat -- docs/knowledge/decisions/` shows only additions at the end of the ADR (`git -C "$W" diff origin/main -- docs/knowledge/decisions/2026-10-06-jl-model-routing-hooks.md | grep -c '^-[^-]'` prints `0`). The docs gate shows only `plans-not-empty`.

- [ ] **T17: gates, audit, distillation**

  **Files:** Whatever the audit's confirmed findings touch; delete the plan.

  - [ ] Step 1: all gates in `W`: the tools gate, the guard gate, `shellcheck` on `scripts/agent/qemu-lock.sh`, `scripts/agent/brief.sh` and `.claude/hooks/aios`, `actionlint .github/workflows/*.yml`, `just check` (zero warnings; no kernel change, so this only confirms nothing broke).
  - [ ] Step 2: `/audit-loop` rounds, each run headless inside `W` as in T12 (with the same `--allowedTools` list) with `mode` from the audit-loop SKILL's classification (`tools`, since `tools/`, `scripts/`, `.claude/` changed) and the latest gate output; the controller routes each confirmed in-diff finding to an implementer, pushes, and starts the next round. Stop rule: the SKILL's (a complete round with an empty `in_diff`; or 4 rounds, or 2 incomplete in a row, which leaves the PR a draft behind a `needs-human` gate issue).
  - [ ] Step 3: distil (rule 04, step 11): move anything in "Issues Encountered" and "Decisions Made" that no lesson or the ADR amendment holds yet into a lesson; then `git rm docs/knowledge/plans/2026-10-06-jl-two-team-harness.md`.
  - [ ] Acceptance: every Step 1 command exits 0; the last round's JSON (`<git-common-dir>/aios-agent/audit/claude-harness-teams/<head>.json`) has `"complete": true` and `"in_diff": []`; `(cd "$W" && just docs-check)` prints `No new drift since baseline.`

- [ ] **T18: ready, PR comments, hand-off**

  - [ ] Step 1: `gh pr ready <n>`; run `/review-pr-comments` (fixes go to implementers; reply and resolve after the fix is pushed).
  - [ ] Step 2: hand off: report the PR URL and `gh pr checks <n>`, list T19 for the owner, and ask the owner to run `/merge-and-cleanup <n>`. Never merge.
  - [ ] Acceptance: `gh pr view <n> --json isDraft --jq .isDraft` prints `false`; `gh pr checks <n>` shows every required check passing.

- [ ] **T19: after the merge** (owner, with the leads; local state only)

  - [ ] Step 1: rebuild the binary on `main` and restart nothing yet: `(cd "$MAIN" && just tools && tail -n 1 target/tools/installed/aios.stamp && target/tools/installed/aios hook path-guard --help | grep -c -- --agent-type)` prints `source clean` and `1`.
  - [ ] Step 2 (owner): remove `remember@claude-plugins-official` from `~/.claude-personal/settings.json` (O1).
  - [ ] Step 3 (owner, only if T2's H6 showed `main xhigh`): remove `effortLevel` from `~/.claude-personal/settings.json`.
  - [ ] Step 4 (O2): split `.remember/remember.md` by its session sections into `.remember/handoff-team-fix.md` (the aios-de section) and `.remember/handoff-team-build.md` (the aios-e1 and aios-ae sections), rewritten under the five pause headings; move `.remember/logs/` to `<git-common-dir>/aios-agent/remember-retired/logs/`; delete the rest of `.remember/` except the two handoff files.
  - [ ] Step 5: team-build deletes `.claude/agent-memory/` in the main checkout and in its own worktrees (`phase-7-m26`, `justin-loop` and any other team-build worktree; `tools-r4` and `ci-soak-matrix` were removed when #230 and #227 merged, and #230's `/merge-and-cleanup` already merged `tools-r4`'s two notes into the main checkout's store). team-fix moves crash-1b's working notes into crash-1b's plan (migration notes §4), then deletes its worktrees' stores.
  - [ ] Step 6: label the open branches' PRs and issues `team-build` or `team-fix` (team-build: justin-loop, phase-7-m26 #149, and the Q7 soak-matrix port issue; team-fix: crash-1b, cap-lifetime, and the kernel bugs #217, #212, #215, #216, #186–#190, #187; tools-203, tools-205-206, tools-r4, ci-soak-matrix and docs-184 have merged), and file the cross-team gate issues that still apply, each titled `Gate: merge PR #<n> only after ...` with `needs-human`: toolchain PRs never between A/B arms; #149 after the boot-crash fix (#165 already gates it). The v3 gate "crash-1b before R4's deletion of `scripts/soak-qemu.sh`" is moot: R4 merged first (1a5c363), so team-build sends team-fix a `HEADS-UP`: crash-1b's next merge of `main` deletes `scripts/soak-qemu.sh`, its soak commands become `just soak` (or `aios soak`) through the lock wrapper, and `scripts/soak-matrix.sh` cannot run post-R4 arms until Q7's port.
  - [ ] Step 7: each lead merges `main` into its own active worktrees (rule 11 needs `.claude/rules/11-teams.md` there before any writer enters).
  - [ ] Step 8: restart both leads per agent-loop.md, Teams, Launch (settings, agents and hooks load at session start); this is `D/OWNER-PROBES.md` OP1. team-build records the OP1 report in the wiring lesson through a small docs PR (or an issue if it fails).
  - [ ] Acceptance: `ls "$MAIN/.remember"` prints only `handoff-team-build.md handoff-team-fix.md`; `find "$MAIN" -path '*/.claude/agent-memory' -prune -print` prints nothing once team-fix has done its part; `for w in $(git -C "$MAIN" worktree list --porcelain | awk '/^worktree /{print $2}'); do test -f "$w/.claude/rules/11-teams.md" || echo "$w"; done` prints nothing; the OP1 report says `result: pass`, and OP3's report (from T5, T7, T8) is in the distilled wiring lesson.

## Dependencies & Risks

- **Depends on:** #220 (merged, 5838ad2), #221 (merged, f12dbc7), #203 (merged, aa52bff) and Tools R4 (merged, 1a5c363); the owner's answers to Q1 and Q3–Q7 at T0 (defaults otherwise); an attended session for every `.claude/**` edit.
- **Risk: a mechanism differs from the strings.** T2 runs before any task relies on it, and each check names what changes on failure. H3 failing stops the plan.
- **Risk: the shim's freshness check on every Bash call, edit and Agent call.** #203's `verdict` (find, `git ls-tree`, `git hash-object` of the 4 MB binary) plus a hard link: about 130 ms per call at load 138 (measured 2026-10-09 on main's shim); about 68 ms idle (discussion doc).
- **Risk: Fable spend.** The plan gate (per plan), the kernel-dev step gate (per kernel-dev stop, at most three blocks) and two audit lenses per round. T16 runs each once.
- **Risk: textual conflicts** with crash-1b (`.claude/CLAUDE.md`, rule 05) and any harness PR merged after 1a5c363. #203, #205/#206 and R4 have merged; the `.v4` drafts are re-anchored on them. The second to merge resolves (D13); re-anchor drafts by text.
- **Risk: a stop kills QEMU under `aios soak`.** v3's wrapper TERMed leaves first, so the harness could record a QEMU killed by the stop as a boot. v4 stops the harness first (T6 Step 3).
- **Risk: a guard bug blocks spawns or boots.** Every rule has unit tests (T7), the guard fails to `ask`, and T16 exercises rule 3 for real.

## Issues Encountered

(filled during execution; T2 and T16 paste their check outputs here)

## Decisions Made

(T0 records the owner's answers to Q1 and Q3–Q7; rulings during execution follow)

- T0 (2026-10-09): owner answered Q1 yes, Q3 yes, Q4 yes, Q5 treat stale/dirty as missing, Q6 literal-path `git -C <W>` allows only, Q7 team-build ports soak-matrix.sh in its own PR first (issue #233). CLI 2.1.292; main cae3fff; installed aios stamp `source clean`; `aios hook --help` lists 4 programs.

## Lessons Learned

(to be filled during execution; T17 distils them)
