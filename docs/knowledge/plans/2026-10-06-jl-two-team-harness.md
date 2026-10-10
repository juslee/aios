---
author: jl + claude
date: 2026-10-06
tags: [tooling, security]
status: draft
---

# Plan: two-team harness (team-build and team-fix) with the model-routing Part 2 wiring

> **v5 (2026-10-09):** T2 ran on Claude Code 2.1.292 (`D/T2-results-2026-10-09.md`) and the owner made two decisions: writer placement is **isolated + fast-forward** (D3), and the Fable step review is a **lead-run gate before the fast-forward**, not a `SubagentStop` hook (D8); agent hooks name `claude-fable-5-1`. Re-anchored on `origin/main` at e430305 (#232). Changelog: `D/REPLAN-v5.md`. Where a draft has a `.v5` file next to it, use the `.v5` file, else the `.v4` file, else the v3 original; older versions are kept unchanged for the record.

> **v4 (2026-10-09):** reconciled with `origin/main` at 1a5c363 (#222–#230 merged since f12dbc7); see `D/RECONCILE-v4.md`.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. The controller is the attended `team-build` lead session in the main checkout (see Execution); every `.claude/**` edit prompts the owner.

**Goal:** Two attended lead sessions, `team-build` and `team-fix`, split the work by domain and share one host safely, and every agent runs on the model, effort and hooks that the model-routing ADR requires.

**Architecture:** Every agent is a subagent of a lead session (no Claude Code teammates). Writers and the verifier are spawned with `isolation: "worktree"` and work in their own temporary worktree, reset to the branch worktree's tip; the lead checks each range, has Fable review kernel-dev's, fast-forwards the branch worktree to it, removes the temporary worktree and pushes. Readers run read-only in the main checkout. Model and effort live in agent frontmatter; every hook is registered once, in `.claude/settings.json`, so it also runs in `-p`, SDK and untrusted sessions. One host QEMU lock serves both teams, and cross-team order lives in GitHub labels and gate issues, with lead-to-lead messages only mirroring that state.

**Tech stack:** Claude Code 2.1.292 (terminal CLI; 2.1.285 when v3 was written) settings, agents, skills, hooks and workflows; the `aios` host binary (`tools/`, std Rust, `aios hook` from #220, `aios soak` from #230, installed at `target/tools/installed/aios` with a provenance stamp by `just tools` since #203); POSIX `sh` (the `aios` shim, `scripts/agent/qemu-lock.sh`); Python 3 (`.claude/hooks/git-push-guard.py`); bash (`scripts/agent/brief.sh`); `gh`.

**Spec:** the requirements are `docs/knowledge/decisions/2026-10-06-jl-model-routing-hooks.md`, section "Part 2 requirements (owned by the two-team harness PR)" and "Consequences", on `main` since #220 (verbatim extract: `D/part2-requirements-final-1a5c363.extract.md`; #229 changed only its Shim bullet since `D/part2-requirements-final-5838ad2.extract.md`), plus the owner decisions under Decisions. `D` below is the design folder `<git-common-dir>/aios-agent/teams-design-v3/`; every draft this plan applies is there, mirroring repository paths. Evidence: `D/evidence-v3.md` (E1–E23). Headless check procedures: `D/probe/HEADLESS.md.v5`. Checks only the owner can run: `D/OWNER-PROBES.md.v5`. T2's results: `D/T2-results-2026-10-09.md`.

## Global Constraints

Every task's requirements include these.

- Base: `origin/main` at e430305 or later (the v5 drafts are anchored there; the v4 drafts at 1a5c363, and nothing they anchor on changed in #232 except the docs-check allow and the discussion doc's lines 126 and 225, which no note touches). `W` was created at cae3fff (T0) and takes e430305 by merge in T2 Step 5. It contains #220 (5838ad2: `aios hook repeat-error|path-guard|route-shadow|route-outcome`), #221 (f12dbc7: `/merge-and-cleanup` preserves soak results before merging), #203 (aa52bff: the shim runs `target/tools/installed/aios` after a provenance-stamp check, and `guard` fails closed) and Tools R4 (1a5c363: `aios soak`; `scripts/soak-qemu.sh` deleted; `just soak` depends on `tools` and runs the checkout's own installed binary). All are merged; none is duplicated here.
- Branch `claude/harness-teams`, worktree `<main>/.claude/worktrees/harness-teams` (`W`). Push only with `git -C "$W" push -u origin claude/harness-teams`. Never push or merge `main`, never force, never stash, never rebase (rule 03).
- Commit message: `Harness teams: Tn — <task title>` (one commit per task; fix rounds add `(review fixes)`), ending with the attribution line the session's system reminder gives.
- `.claude/**` edits run in a foreground implementer, so the permission prompt reaches the owner. `.claude/settings.json` changes only by the exact transformation `D/.claude/settings.json.apply.jq.v5`, which the owner approves block by block (T8).
- Placement (D3): every implementer, writer and verifier works in its own temporary worktree from `W`'s tip and never in `W`; only the controller (or lead) moves `W`, by `git -C "$W" merge --ff-only`, and pushes from it. Nobody calls EnterWorktree.
- Every `type: agent` hook names the full model id `claude-fable-5-1` (T2 H7); agent frontmatter keeps the alias `fable` (checked in T16, E8).
- No QEMU boot anywhere in this PR: it changes no kernel, stub or shared code. The lock wrapper's tests use a fake QEMU (a symlink to `/bin/sleep`).
- Agent routing, verbatim from the ADR plus owner answer O3: kernel-dev `opus`/`high`; worker `sonnet`/`high`; doc-writer `opus`/`high`; code-reviewer `fable`/`high`; verifier `sonnet`/`medium`; doc-auditor `sonnet`/`medium`; skeptic `sonnet`/`medium`.
- Hook registrations, from the ADR: `repeat-error` synchronous on `PostToolUse` and `PostToolUseFailure`, matcher `Bash`; `route-shadow` `async: true` on `PreToolUse`, matcher `Agent`, printing nothing; `route-outcome` `async: true` on `PostToolUse` and `PostToolUseFailure` (matcher `Agent`) and on `SubagentStop`; `path-guard --deny kernel/ --deny uefi-stub/ --deny shared/` on `PreToolUse`, matcher `Edit|Write|MultiEdit|NotebookEdit`, plus this PR's `--agent-type worker`. No `SubagentStop` agent hook (D8).
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
4. **A Fable review that never lets go.** A kernel-dev range the reviewer keeps rejecting, or a plan the gate keeps rejecting. Expected: at most three review rounds per kernel-dev range and two plan blocks per session, then the lead or owner decides and records it; an unapproved range is never fast-forwarded. Pinned in T11 (team SKILL, Placement step 2) and T16 (E5; E6 through OP2).
5. **A lead killed mid-soak (SIGKILL, closed terminal) leaves the QEMU lock behind.** Expected: `status` says `state=dead`, `clear-stale` removes it only when no QEMU runs, and a live holder is never cleared. Pinned in T6.
6. **A range that is not what it claims.** A writer that forgot its reset (its commits sit on `main`'s head, and the range would drag `main`'s newer commits into `W`), a stray commit, or two writers racing on one `W`. Expected: the range check (first-parent count equals the reported commits, the oldest one's parent is the tip) refuses it before any fast-forward; a refused `merge --ff-only` leaves `W` unchanged; the refused temporary worktree stays until a redo lands. Pinned in T11 (team SKILL, Placement steps 1 and 4) and T16 (E1, E5).
7. **The `git reset --hard *` allow used outside an agent's own temporary worktree.** Expected: guard rule 4 denies it to an agent anywhere else (a branch worktree, the main checkout) and asks on a main thread, so the allow never resets real work unprompted; an agent that commits in the main checkout is denied. Pinned in T7 (rule 4 rows 4–6) and T16 (E9).

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
| 2026-10-09 21:35 | Q1 yes (Fable step gate as a settings `SubagentStop` hook; superseded at 22:04, next rows), Q3 yes (`RUSTUP_AUTO_INSTALL=0`, merging session installs once), Q4 yes (no Claude review on drafts), Q5 treat a stale or dirty binary as missing, Q6 literal-path `git -C <W>` allows only (no wildcards), Q7 team-build ports `soak-matrix.sh` to `aios soak` in its own PR before this one. Q2 resolved by #203 merging first (#229). | D8, D10, D12, Q4 patch notes, settings jq v4, T19 |
| 2026-10-09 22:00 | After T2's H3 failed: D3 becomes **isolated + fast-forward**. Writers (kernel-dev, worker, doc-writer) and the verifier are spawned with `isolation: "worktree"` and never call EnterWorktree. The lead records `W`'s tip and puts it in the prompt; the agent's first command is `git reset --hard <tip>` on its own `worktree-agent-<id>` branch; it works, runs its gates there, and commits. The lead runs `git -C <W> merge --ff-only worktree-agent-<id>` (a refusal merges nothing, and a fresh writer redoes the work from the new tip), `git worktree remove .claude/worktrees/agent-<id>`, `git -C <W> branch -d worktree-agent-<id>` (`-C <W>`: from the main checkout `-d` refuses), and pushes from `W`. Readers stay in the main checkout with `git -C <W>`; the verifier boots from its own temporary worktree through the QEMU lock. Guard rule 3 keeps denying a `name` without `isolation` and a writer type without isolation; the EnterWorktree pinning rule goes. | D3, guard rules 3–4, T7, T10, T11, T16 |
| 2026-10-09 22:04 | After T2's H7: D8 is no longer a `SubagentStop` hook (2.1.292 discards a `SubagentStop` block for subagents). Before fast-forwarding a kernel-dev range, the lead spawns code-reviewer (Fable; `rules` and `bugs`) read-only on `git -C <W> diff <tip>..worktree-agent-<id>` in the main checkout; must-fix findings go to a fresh kernel-dev; at most 3 rounds, then the lead or the owner decides; only an approved range is fast-forwarded. Agent hooks name `claude-fable-5-1` (`model: "fable"` returns 404 there); agent frontmatter `model: fable` stays and T16 checks it runs on Fable. **Q1 is superseded** by this decision: no Fable gate is a hook, in settings or frontmatter. | D8, D9, D18, T8, T10, T11, T15, T16 |
| standing | Never keep legacy. | every task |

### Design decisions

| # | Decision | Why |
| --- | --- | --- |
| D1 | **Teams and identity.** Two attended terminal sessions, launched from the main checkout as `AIOS_TEAM=<team> claude -n <team> --model opus`, with `<team>` `team-build` or `team-fix`. `AIOS_TEAM` keys the handoff file, the lock's `--team` and the guard; the `-n` name must equal it (`/justin:team` preflight). Every other session is solo with `AIOS_SESSION=<name>`. | Session display names are self-chosen and reset on a fresh launch (seen on 2026-10-06: the leads came back as aios-83 and aios-39). `CLAUDE_CODE_SESSION_NAME` is not set by `-n`. An env var is the only identity the guard and the skills can read. |
| D2 | **Subagents only.** No Claude Code teammates. Leads never pass `name` without `isolation` (guard rule 3). The project's `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS` line goes; lead-to-lead messages use cross-session messaging, which has its own gate (E16). | E2–E4; teammates added nothing the design needs. OP1 confirms messaging without the project flag. |
| D3 | **Writer placement: isolated + fast-forward** (owner, 2026-10-09 22:00). Writers (kernel-dev, worker, doc-writer) and the verifier are spawned with `isolation: "worktree"` and declare `isolation: worktree` in their frontmatter (the marker guard rule 3 reads). They never call EnterWorktree, and no agent lists it in `tools:`. The lead records `W`'s tip (`git -C <W> rev-parse HEAD`) and puts it in the prompt. The agent first checks it is in a temporary worktree (`.../.claude/worktrees/agent-<id>` on `worktree-agent-<id>`), then runs `git reset --hard <tip>` (allowed in settings, Q8; confined by guard rule 4), works, runs its gates there with a cold build, and commits. The lead checks the range (`rev-list --first-parent <tip>..worktree-agent-<id>` is exactly the reported commits; the oldest one's first parent is the tip), has Fable review a kernel-dev range (D8), runs `git -C <W> merge --ff-only worktree-agent-<id>` (refused: nothing merges, a fresh writer redoes the work from the new tip, and the refused temporary worktree stays until the redo lands), `git worktree remove` (never `--force`) and `git -C <W> branch -d worktree-agent-<id>`, and pushes from `W`. One writer per branch worktree at a time. The verifier resets to the sha it boots, writes run directories under `<W>/target/soak/`, and is cleaned up the same way. Readers (code-reviewer, doc-auditor, skeptic, Workflow lenses) run in the main checkout with `git -C <W>`. Leads stay in the main checkout. **`CARGO_TARGET_DIR` stays per worktree** (never `W`'s `target/`): the justfile reads `target/` relative to its checkout (`kernel_elf`, `stub_efi`, `--target-dir target/tools`, `target/tools/installed`), so a redirected build would leave `just disk` and `just soak` reading stale or missing artifacts; `W`'s uplifted kernel ELF and stamped `aios` must describe `W`'s head, not an unreviewed (or refused) range; and concurrent builds would serialize on one cargo lock. The price is a cold build per spawn. | T2 on 2.1.292: H3 (EnterWorktree from an isolated agent is refused: "its writes are limited to that folder"), A2a (a non-isolated agent's EnterWorktree is refused, and it then committed on `main`), A2b (`git switch` works but leaves the temporary worktree behind), A3 (this mechanism, foreground and background); H1 (unchanged temporary worktrees are removed by Claude Code, ones with commits are not); H2 (an agent's `cwd` is its own temporary worktree). |
| D4 | **Roster: seven agents.** kernel-dev, worker (absorbs v1's tools-dev), doc-writer, verifier, code-reviewer (absorbs the security lens), doc-auditor, skeptic, with the routing in Global Constraints. `team-lead.md` and the `build-team` skill are deleted. worker writes `tools/`, `scripts/`, `.github/`, the justfile, `.gitattributes`, `.claude/` harness files and `docs/` work; doc-writer writes phase docs, architecture docs and ADR amendments. | ADR roster; E18 (reserved name, TeamCreate gone). The ADR gives doc-writer a model but no scope; design docs need its Opus judgment, every other `docs/` task stays with worker as the ADR says. |
| D5 | **Effort.** Remove `CLAUDE_CODE_EFFORT_LEVEL` from the project env; set `effortLevel: "high"` in project settings; every agent sets `effort` in its frontmatter. Leads launch with `--model opus` and no `--effort`, so effort has one source. | ADR: the variable outranks frontmatter `effort`, and Opus 5.5 ignores a user-settings `effortLevel`. T2 H6 (2.1.292): `main high`, per-agent values applied. |
| D6 | **Logging hooks exactly as the ADR lists them** (Global Constraints). `route-shadow` gets its own `PreToolUse` entry beside the guard's, so the guard stays synchronous. | ADR. |
| D7 | **`path-guard` registered once, in `settings.json`**, with a new `--agent-type worker` flag: it decides only for worker, denies a payload with `agent_id` and no `agent_type`, and passes every other caller before any path or `git` work. No frontmatter copy. | Owner 11:28; frontmatter hooks do not run in `-p`, SDK or untrusted sessions (ADR Known limits); two registrations would be legacy. |
| D8 | **Fable step review: a lead-run gate before the fast-forward** (owner, 2026-10-09 22:04). kernel-dev's report starts with `RESULT: committed <tip>..<head> (<n> commits)`, `BRANCH: worktree-agent-<id>` and `W:` lines. Before fast-forwarding, the lead spawns code-reviewer (frontmatter `model: fable`) read-only in the main checkout with the `rules` and `bugs` lenses on `git -C <W> diff <tip>..worktree-agent-<id>` (for a merge of `origin/main`, on `git -C <W> show --remerge-diff`, the hand resolution; a clean merge needs no review). Must-fix findings go to a **fresh kernel-dev spawn that resets to the reviewed head** (`git reset --hard <head>`) and adds a `(Fable review fixes)` commit on top; the next round reviews `<tip>..<new head>` with the earlier findings and answers. At most three rounds, then the lead decides each open finding or asks the owner, and records the ruling; only an approved range is fast-forwarded. Superseded temporary branches are ancestors of the merged head, so `branch -d` deletes them after the fast-forward. No hook, no patch file, no marker counting; kernel-dev keeps `maxTurns: 300`. | ADR "Fable before done", as amended (T16). T2 H7 on 2.1.292: the `SubagentStop` agent hook ran but its block was discarded ("Stop hook block discarded (turn ended by tool result, no model re-invoke)"), because a subagent ends with a hand-back tool call; a frontmatter `Stop` maps to the same event. Fixing on top of the reviewed head (rather than a new branch from the tip) keeps the reviewed commits, makes round N+1's delta exactly the fix, and needs no `branch -D`: the alternative, redoing from the tip, repeats the whole step and leaves an unmerged branch that only the owner can delete. |
| D9 | **Fable plan gate**: a `PreToolUse` agent hook on `ExitPlanMode` in `settings.json`, `model: claude-fable-5-1` (agent hooks send `model` verbatim; `fable` returned 404, T2 H7), `timeout: 600`. It reviews only plans whose first heading starts `# Plan:`, with code-reviewer's `plan` lens, at most two blocks per session. Every working plan is written in plan mode (`/implement-phase` and `/justin:team` enter it), and worker commits the reviewed `planFilePath` file unchanged. | Owner 11:28; ADR (ExitPlanMode input carries `plan` and `planFilePath`, E21). Requiring plan mode closes the gap where an optional plan mode would skip the gate; committing the reviewed file means the reviewed plan is the committed one. A `PreToolUse` agent hook on `claude-fable-5-1` that returns ok false denies the tool call (T2 probe 3, 2026-10-10, shown for Bash; probe 2's hooks returned ok true, so it showed only that the full id runs). ExitPlanMode is not reached in `-p` (T2 H8), so OWNER-PROBES OP2 is the gate's only real-run check. |
| D10 | **Shim (`.claude/hooks/aios`), `hook` branch, on #203's shim.** Never builds in the foreground; always exits 0; runs a fresh binary through #203's hard-link check. Stale (one background build) and dirty (no build) are treated as missing (Q5 default; the ADR, as amended by #229, leaves this to the owner). Missing, stale, dirty, unusable or failing binary: `path-guard` denies only the agent types its `--agent-type` names (every caller when it names none, and any payload with `agent_id` but no `agent_type`); every other hook prints nothing. | ADR "Shim"; exit 3 (the shim's old failure code) is non-blocking, so `path-guard` would fail open; an unconditional deny would block every edit in CI and on a fresh clone (Review Focus 1). |
| D11 | **One host QEMU lock for both teams**: `scripts/agent/qemu-lock.sh run --team team-build\|team-fix\|solo --mode boot\|quiet`. Exit 75 held, 76 foreign QEMU, 77 deferred under load (above 30 in `boot`, 3.0 in `quiet` after a settle wait). `status` reports `state=live\|starting\|dead`; `clear-stale` removes only a dead holder's lock with no QEMU running, and logs it. Quiet windows follow `QUIET-REQ`, `QUIET-ACK` or `QUIET-LATER`, owner pause of host load, lease, `QUIET-END`. Among agents only the verifier boots (guard rule 1). R4b's flock lease replaces it later. | Domain split: team-fix soaks too. On 2026-10-06 the contention was host load (load1 about 240), which gave false WEDGEs; the load gate turns those into DEFERRED. Restarts left wrapper processes dead; pid liveness detects that at once. v4: a stop TERMs every process but QEMU first and waits up to 15 s, so `aios soak` (which supervises QEMU in its own process group since R4) ends its boot itself instead of recording a QEMU killed under it. |
| D12 | **Toolchain.** `RUSTUP_AUTO_INSTALL=0` in the project env; guard rule 2 denies `rustup` installs, updates and removals to agents and asks on the main thread; after a merge that changes `rust-toolchain.toml`, `/merge-and-cleanup` runs `rustup install` once and says so in `MAIN-MOVED`. Open question Q3. | 2026-10-06: two sessions raced an implicit install into `~/.rustup` after #202, and an agent reinstalled the toolchain under the other team's build. E22. |
| D13 | **Ownership per branch**: GitHub labels `team-build` and `team-fix` on issues and PRs, and "Owned branches" in each handoff file. Overlaps are announced (`HELLO`, `HEADS-UP`); the second branch to merge resolves conflicts. Harness files change only on team-build's branches. Cross-team merge order exists only as `needs-human` gate issues. Message vocabulary: `HELLO`, `HEADS-UP`, `MAIN-MOVED`, `QUIET-REQ`, `QUIET-ACK`, `QUIET-LATER`, `QUIET-END`, `LOCK-STALE`, each mirroring durable state. | Both teams write kernel code (features versus fixes); 2026-10-06's real overlaps (crash-1b porting `lock_order.rs`, #219 conflicting in crash-1b) were per branch, not per path. |
| D14 | **Remember retired; agent memory retired.** Delete the project plugin line, `precompact-save.sh` and its PreCompact hook; `/justin:pause` writes `.remember/handoff-<key>.md` itself. No agent keeps `memory:`; its notes become lessons (T3). | O1, O2, no-legacy; `memory: project` splits notes across worktrees and gives reviewers write tools. |
| D15 | **`/merge-and-cleanup` = #221 plus a delta**: remove its agent-memory copy; add the gate-issue refusal, the quiet-lease refusal, the toolchain install and `MAIN-MOVED` (`D/.claude/skills/merge-and-cleanup/SKILL.md.delta-notes.md`). | #221 merged first, as the task required; no duplicate. |
| D16 | **Sequencing with #203: #203 merged first** (aa52bff, 2026-10-09, as #229). This PR adds the `hook` branch to #203's shim (`D/.claude/hooks/aios.v4`) and its tests to #203's `shim.rs`; no `is_stale` remains. Q2 is resolved by the merge order. | v3's whole-file shim draft would revert #203. |
| D17 | **Audit.** `/audit-loop` runs the committed workflow `.claude/workflows/audit-loop.js`, one round per run; skeptics by severity (three for must-fix, one otherwise); at most one audit per team at a time; no new round during the other team's quiet window. | Replaces the uncommitted v2.js engine (753 agents in one run on 2026-09-29). |
| D18 | **Step reviews: kernel-dev only** (the D8 lead-run review). worker and doc-writer steps get no Fable step review; the audit covers them. The `repeat-error` main-session text ("the project expects the code-reviewer agent to diagnose the failure") is served by a new code-reviewer `diagnose` lens, so the hard-coded Rust text stays. | O4. No Rust rewording needed. |
| D19 | **`/review-pr-comments` stays** the PR loop until R2. The O5 note goes into the R2 design doc on `claude/justin-review-merge-loop`, not into this PR. | O5; no second PR loop. |
| D20 | **The Jev evaluation's home**: agent-loop.md "Model routing" (the 200-dispatch rule, the join, the 0–2 complexity scale, `-p` gaps, unjoined stops, not-launched dispatches, Workflow lenses not logged), and a routing-log count in `/justin:brief`. | The ADR requires the evaluation; v2's issue draft is gone with Part 2 moving into this PR. |
| D21 | **Checks: headless first, owner only for what needs a person.** T2 and T16 run every mechanism check with `claude -p` (settings hooks run there). `D/OWNER-PROBES.md.v5` keeps only what needs two live terminals, the interactive plan-approval UI (the plan gate, since ExitPlanMode is not reached in `-p`), or a person answering prompts. | The ADR requires real-run checks before reliance; most need no person. |

### Open questions for the owner

Q1–Q7 were answered at T0 (Decisions Made); Q1 is superseded by the 22:04 decision. Q8 is new in v5 and is needed before T7 (default applies if unanswered).

| # | Question | Default |
| --- | --- | --- |
| Q8 | (new, v5) Background writers and the verifier start with `git reset --hard <tip>` in their own temporary worktree. No allow rule covers it on `main`, a background agent cannot answer a prompt, and the auto-mode classifier may refuse a hard reset. Add `Bash(git reset --hard *)` to the project allow list, with guard rule 4 denying it to an agent anywhere but its own temporary worktree (`.../.claude/worktrees/agent-<id>` on `worktree-agent-<id>`) and asking for it on every main thread? The alternative is no allow: every writer and verifier spawn runs in the foreground so you answer each reset (two writers in parallel are then impractical). | Yes (`settings.json.apply.jq.v5`; guard rule 4 rows 4–5). |
| Q1 | (superseded 2026-10-09 22:04) Register the Fable step gate in `settings.json` as a `SubagentStop` hook? Answered yes at 21:35; then T2 H7 showed the block is discarded for subagents, and the owner made the step review lead-run (D8). | — |
| Q2 | Resolved: #203 merged first (aa52bff). The `hook` branch goes onto #203's shim (D16). | — |
| Q5 | (new, #229) The ADR's Shim requirement now leaves it to you what `aios hook` does with a stale or dirty binary: run it (a background build on stale), or treat it as missing (`path-guard` denies worker, the other three print nothing), the counterpart of `guard`'s ask. Stale includes a binary that fails its provenance stamp. | Treat as missing (implemented in `aios.v4`). |
| Q6 | (new, #223) #223's owner-approved note rejects `git -C * <subcmd>` allow rules (the first `*` can carry `-c core.pager=<cmd>`). v3's settings added eight. v4 drops them, so leads' and readers' `git -C <W>` calls prompt (or go to the auto-mode classifier), and headless `-p` runs refuse them. Accept per-worktree allows with the path written out (`Bash(git -C <W> diff *)`, safe because `-c` must precede the subcommand): on `--allowedTools` for the T12/T17 `-p` runs, and in `.claude/settings.local.json` per worktree for attended leads (rule 11)? | Yes: drop the wildcard rules (`settings.json.apply.jq.v4`); literal-path allows only. |
| Q7 | (new, #227/#230) `scripts/soak-matrix.sh` runs each arm's `scripts/soak-qemu.sh`, which R4 deleted, so it refuses every arm at or after 1a5c363; its help also relies on rustup installing a missing toolchain, which `RUSTUP_AUTO_INSTALL=0` stops. Who ports it, and does team-fix's next A/B wait for that? | team-build files an issue and ports it in its own PR, not this one; guard rule 1 keeps treating it as a QEMU start. |
| Q3 | Toolchain installs (D12): `RUSTUP_AUTO_INSTALL=0` for every Claude Code session, agents denied every `rustup` install, and the merging session installs once. Approve? | Yes. |
| Q4 | Skip the Claude code review on draft PRs, so it runs once on `ready_for_review` after the audit (`D/.github/workflows/claude-code-review.yml.patch-notes.md`). It is not a required check. Approve? | Yes. |

## Execution

- **Who.** The `team-build` lead session (the owner's attended terminal; launched today as it is: `AIOS_TEAM` has no meaning until this PR merges) is the controller. It runs superpowers:subagent-driven-development over T0–T18; the owner runs T19 with the leads after the merge.
- **Implementers** are `general-purpose` subagents on `model: sonnet` (the ADR routes this work to worker, which does not exist yet in a session started on `main`), spawned with `isolation: "worktree"`, `run_in_background: false` (most tasks touch `.claude/**`, which prompts; the foreground also lets the owner approve the reset, which `main`'s settings do not allow yet), and the prompt: "W is `<main>/.claude/worktrees/harness-teams`, branch `claude/harness-teams`, tip `<tip>`. You work in your own temporary worktree, `A`, never in W. First check that `git rev-parse --show-toplevel` ends in `/.claude/worktrees/agent-<id>` and `git branch --show-current` is `worktree-agent-<id>`; if not, stop and report. Then run `git reset --hard <tip>` and check `git rev-parse HEAD` equals the tip. Never call EnterWorktree, never `cd` or `git -C` into W or another checkout, and never set `CARGO_TARGET_DIR`. Do task Tn of the plan at `A/docs/knowledge/plans/2026-10-06-jl-two-team-harness.md`; the drafts are in `D`. Every acceptance command that names `$W` runs in A: set `W=$(git rev-parse --show-toplevel)` in each command block. Commit on your temporary branch with the message given, and report `RESULT: committed <tip>..<head> (<n> commits)`, `BRANCH: worktree-agent-<id>`, each acceptance and gate command with its output." The controller records `<tip>` with `git -C "$W" rev-parse HEAD` right before each spawn. T0 and T1 ran before D3 changed (2026-10-09), by the controller.
- **Placement (D3) for every implementer range**, by the controller from the main checkout, each command its own Bash call: (1) the range check, `git -C "$W" rev-list --first-parent <tip>..worktree-agent-<id>` lists exactly the reported commits and the oldest one's first parent is `<tip>`; (2) the task reviewer approves the range (per-task gate 6) before anything merges; (3) `git -C "$W" merge --ff-only worktree-agent-<id>` (refused: nothing merges, and a fresh implementer redoes the task from the new tip); (4) `git worktree remove "$MAIN/.claude/worktrees/agent-<id>"` and `git -C "$W" branch -d worktree-agent-<id>`; (5) `git -C "$W" push -u origin claude/harness-teams`. One implementer at a time: tasks run in order on one branch, so a second implementer would only race the first. The controller's `git -C "$W"` calls prompt (no wildcard allow, Q6); the owner may add the literal-path allows from agent-loop.md.v5's Launch bullet to the main checkout's `.claude/settings.local.json` for `W`.
- **Task reviewers** are `general-purpose` subagents on `model: sonnet` (O4 keeps Fable for kernel-dev steps and the audit; this PR has no kernel-dev step), read-only, in the main checkout with `git -C "$W"`, on the range `<tip>..worktree-agent-<id>` before its fast-forward (head files through `git -C "$W" show worktree-agent-<id>:<path>`). For T4, T5 and T7 the reviewer prompt adds: "This is a guard. Attack it: find an input that makes it allow what it should deny, or fail open; do not compare it with the spec line by line" (lesson `2026-10-06-jl-path-guard-spec-correct-bypasses.md`).
- **Bootstrap.** A session loads `.claude/agents/`, settings and workflows from the checkout it starts in. The new agent types and hooks therefore exist only for a session started inside the branch's checkouts. T12 starts a headless `claude -p` session inside its implementer's worktree (before the fast-forward) and T16 inside `W` for that, a one-time exception to rule 03's "start sessions in the main checkout", recorded in the PR description.
- **Drafts.** Each task applies its drafts from `D` exactly: the `.v5` file where one exists, else the `.v4` file, else the v3 original; a deviation is recorded under "Issues Encountered" with its reason. Line anchors in the `.v5` and `.v4` drafts hold on `origin/main` at e430305 (#232 changed only the settings allow and two in-place lines of the discussion doc, `D/REPLAN-v5.md`); re-anchor by text if `main` moved.

### Per-task gate

A task is done when all of these hold, in this order. Gates 1–4 run in the implementer's own worktree `A` at its head (where a command names `$W`, `W` is `A`); gates 5–7 are the controller's.

1. Its acceptance commands print what the task says. The implementer pastes each command and its output into its report.
2. **Docs gate:** `just tools` in `W` (only when the task changed `tools/`), then `(cd "$W" && AIOS_TOOLS_BIN="$W/target/tools/installed/aios" just docs-check)`. Until T17 it exits 1 and lists exactly one new finding: `knowledge-hygiene` `plans-not-empty` for `docs/knowledge/plans/2026-10-06-jl-two-team-harness.md`. From T17 on it prints `No new drift since baseline.` and exits 0. (Without `tools/` changes, `AIOS_TOOLS_BIN="$MAIN/target/tools/installed/aios"`. The shim runs an override without a stamp or freshness test; `just tools` in `W` stamps `source dirty`, which only matters to the shim's own binary, never to an override.)
3. **Tools gate** (tasks that change `tools/`): `cargo fmt --check -p aios-tools`, `cargo clippy -p aios-tools --all-targets -- -D warnings`, `cargo test -p aios-tools`, all exit 0, run in `W`.
4. **Guard gate** (tasks that change `.claude/hooks/`): `/usr/bin/python3 -m unittest discover -s .claude/hooks/tests` passes in `W`; `sh -n`, `dash -n` and `shellcheck -s sh` pass on changed POSIX `sh` files.
5. The range check passes (Placement step 1), and `git -C "$A" status --short` prints nothing after the commit.
6. The task reviewer approves spec compliance and quality on the range, or the controller records a ruling for each open finding. Findings go to a fresh implementer that resets to the reviewed head and commits `(review fixes)` on top; the reviewer then re-checks the whole range.
7. The controller fast-forwards `W`, removes the temporary worktree and branch, and pushes (Placement steps 3–5); `git -C "$W" status --short` prints nothing.

## Progress

- [x] **T0: preconditions and worktree** (controller; no commit). Done 2026-10-09: CLI 2.1.292, `main` cae3fff, stamp `source clean`, 4 hook programs, Q1 and Q3–Q7 answered (Decisions Made); `W` created at cae3fff.

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

- [x] **T1: the working plan, the team labels and the draft PR** (controller). Done 2026-10-09: adb77da (`PLAN.md.v4`), draft PR #234 labelled `team-build`. T2 Step 3 replaces the committed plan with this v5.

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

- [ ] **T2: headless mechanism checks, the merge of `main`, and the two lessons** (controller runs the checks; implementers write)

  **Files:** Modify the plan (replace it with `D/PLAN.md.v5`; "Issues Encountered"). Create `docs/knowledge/lessons/2026-10-06-jl-isolated-writers-fast-forward.md`, `docs/knowledge/lessons/2026-10-06-jl-model-routing-wiring-checks.md`. Merge `origin/main` (e430305 or later).

  - [x] Step 1: run `D/probe/HEADLESS.md.v4`, section "T2" (2026-10-09, CLI 2.1.292, scratch `<scratch>`), plus the alternative runs A2 and A3 and the model-id probe 2 (`<scratch 2>`). Results: `D/T2-results-2026-10-09.md`.
  - [x] Step 2: compare with each "Pass" cell and apply the stated change. Outcome: **H1** pass (clean-up: unchanged temporary worktrees are removed; no probe commits because EnterWorktree failed); **H2** pass (`agent_id`, `agent_type`; `cwd` is the agent's own `.claude/worktrees/agent-<id>`; `$CLAUDE_PROJECT_DIR` stays the repository root); **H3** failed (an isolated agent's EnterWorktree into the branch worktree is refused), so the plan was re-planned: the owner chose isolated + fast-forward (D3), and H3 is **redefined** as that mechanism, which run A3 passed (foreground and background; `git branch -d` needs `-C <W>`); **H4** pass (`SubagentStop` for the foreground and the background agent); **H5** pass (identical `agent_id`, `completed` and `async_launched`); **H6** pass (`main high`, `probe-writer low`, `probe-kdev medium`; T19 Step 3 has nothing to do); **H7** is **redefined** as the agent-hook model-id check, which passed (`fable` 404s, `claude-fable-5-1` runs; a `SubagentStop` block is discarded for subagents, so D8 became the lead-run review); **H8** no record in `-p` (E6 moves to OWNER-PROBES OP2). The v5 procedure for re-runs is `D/probe/HEADLESS.md.v5`, section "T2".
  - [x] Step 3 (implementer, the first live use of D3 in this repository): replace the working plan and record T2. Prompt as in Execution; task: "`cp "$D/PLAN.md.v5" docs/knowledge/plans/2026-10-06-jl-two-team-harness.md` (it holds every entry the committed copy has). Under 'Issues Encountered', paste every command and output from `D/T2-results-2026-10-09.md` and the files it names (`<scratch>/run-a*.json` results, `run-a3.txt`, the debug lines it quotes, probe 2's `d.log` lines), with `claude --version`. Commit `Harness teams: T2 — plan v5 (isolated + fast-forward placement, lead-run Fable review) and T2 results`." Then Placement (Execution). This exercises the reset, the range check, `merge --ff-only`, `git worktree remove` and `git -C "$W" branch -d` on the real repository under `main`'s guard; the controller gives the outputs of its Placement commands to the Step 5 implementer, which adds them under "Issues Encountered" in its commit.
  - [x] Step 4 (implementer): merge `origin/main` into the branch: "`git fetch origin` and `git merge --no-edit origin/main`; no conflicts are expected (#232 changed `.claude/settings.json`, `Cargo.lock` and the discussion doc, none of which this branch has changed yet); run the docs gate; the merge commit is the commit." Then Placement (a merge range: the range check lists the merge commit as the one first-parent commit). (Step 3's Placement outputs go into Step 5's commit.)
  - [x] Step 5 (implementer): write the two lessons (rule 08 frontmatter, tags `tooling`), following `D/docs/knowledge/lessons/agent-memory-migration.patch-notes.md.v5` §2:
    - `isolated-writers-fast-forward`: from kernel-dev/worktree-isolation-push (main checkout's `.claude/agent-memory/kernel-dev/`, corrected) plus H1–H3, A2a, A2b, A3 and Step 3's Placement outputs. The placement mechanism (tip, reset, commit, range check, `merge --ff-only`, `git worktree remove`, `git -C <W> branch -d`, and why `-C <W>`: from the main checkout, `HEAD` is `main`, which lacks the commits, so `-d` refuses), that a temporary worktree with commits is not removed automatically while an unchanged one is, and why `CARGO_TARGET_DIR` stays per worktree;
    - `model-routing-wiring-checks`: H4–H8, with the new facts: a `SubagentStop` agent-hook block is discarded for subagents ("Stop hook block discarded (turn ended by tool result, no model re-invoke)"); agent hooks send `model` verbatim, so `fable` 404s and `claude-fable-5-1` works, while agent frontmatter `model: fable` is a separate path that T16 E8 checks; a `PreToolUse` agent hook that returns ok false denies the tool call (probe 3, 2026-10-10).

    Each lesson keeps the exact commands (from `D/probe/HEADLESS.md.v5`) so agent-loop's "Re-check after each Claude Code update" can re-run them, and states the CLI version checked (2.1.292). Then Placement.
  - [ ] Acceptance: `git -C "$W" log --first-parent --format=%s -4` shows the lessons commit, the merge of `origin/main`, the plan v5 commit and T1's; `git -C "$W" merge-base --is-ancestor e430305 HEAD && echo merged` prints `merged`; `git -C "$W" show --stat HEAD` lists the two lessons; `git -C "$MAIN" branch --list 'worktree-agent-*'` prints nothing from this task; the docs gate shows only `plans-not-empty`.

- [x] **T3: agent-memory lessons**

  **Files:** Create the lessons in `D/docs/knowledge/lessons/agent-memory-migration.patch-notes.md.v5` §2 marked T3 (11, plus `2026-10-06-jl-host-clippy-shared-tests.md` only if its condition holds).

  - [x] Step 1 (controller, before spawning): copy every store, read-only, as §1 says:
    ```bash
    H=$(git -C "$MAIN" rev-parse --path-format=absolute --git-common-dir)/aios-agent/agent-memory-harvest
    for c in "$MAIN" "$MAIN"/.claude/worktrees/*; do
      [ -d "$c/.claude/agent-memory" ] && mkdir -p "$H/$(basename "$c")" && cp -R "$c/.claude/agent-memory/." "$H/$(basename "$c")/"
    done
    find "$H" -name '*.md' ! -name MEMORY.md | wc -l
    ```
    Expected: 41 or more notes (17 main, 20 crash-1b, 4 already harvested from #218 and #221; counted 2026-10-09).
  - [x] Step 2 (implementer): write each lesson from its sources in `$H`, re-checking every claim against `W` at HEAD, with rule 08 naming and frontmatter (`author: jl + claude`, `date: 2026-10-06`, `tags`, `status: final`). Outcome: 11 lessons written. Claims the code or the tools contradicted were corrected or dropped: the `ArmTrngLib` line is not tied to the CPU model (it stays under `-cpu max`); soak output is not only in the main checkout; log messages are no longer cut at 48 bytes (continuation entries, #219); the docs-check override path is `target/tools/installed/aios`, not `target/tools/release/aios`; the sandbox doc no longer omits `IpcSelect`; `scripts/docs/check.py` and `soak-qemu.sh` are gone (`aios soak`, `--classify`); the bare `grep` binary-file claim depends on the grep build. The step-1b-only code (tripwire counters, `shared::lock`, `IrqSpinLock`) is not on `main`, so those lessons keep the method and mark the branch examples as history. The T2 lesson `isolated-writers-fast-forward` lost its one mention of the retired note store so that the acceptance `rg` is empty.
  - [x] Step 3: the host-clippy condition: `(cd "$W" && cargo clippy -p shared --all-targets 2>&1 | tail -n 5)`; write the lesson only if the failures it records still appear. Outcome: the failures still appear (`could not compile shared (lib test) due to 1 previous error; 24 warnings emitted`), so `2026-10-06-jl-host-clippy-shared-tests.md` was written.
  - [x] Acceptance:
    ```bash
    cd "$W" && for f in soak-noise-base-rates reading-soak-output ab-soak-method irq-path-codegen-hazards miri-single-seed \
      llvm-drops-write-only-statics tcg-irqs-at-tb-starts gates-in-worktrees verifying-harness-claims \
      testing-harness-shell-snippets ipc-capability-doc-touchpoints; do
      test -f "docs/knowledge/lessons/2026-10-06-jl-$f.md" || echo "missing $f"; done
    rg -n 'agent-memory|MEMORY\.md' docs/knowledge/lessons/2026-10-06-jl-*.md
    ```
    Expected: no output from either command. The docs gate shows only `plans-not-empty`.

- [x] **T4: `path-guard --agent-type`** (TDD; implementer, then a guard-attacking reviewer)

  **Files:** Modify `tools/src/cmd/hook/path_guard.rs` (the `Args` struct at lines 31–38, `run` at line 66, the module doc at lines 1–3). Test `tools/tests/hook_path_guard.rs`. Draft: `D/tools/src/cmd/hook/path_guard.rs.patch-notes.md.v4`.

  **Interfaces:** Produces the CLI form `aios hook path-guard --agent-type <NAME> [--agent-type <NAME>...] --deny <PREFIX>...`, which T5's shim fallback parses and T8 registers.

  - [x] Step 1: write the failing tests, appended to `tools/tests/hook_path_guard.rs`:
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
  - [x] Step 2: `(cd "$W" && cargo test -p aios-tools --test hook_path_guard)`. Expected: the new tests fail (clap rejects `--agent-type`, so `guard` sees exit 2); the existing tests pass.
  - [x] Step 3: implement. In `Args`:
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
  - [x] Step 4: `(cd "$W" && cargo fmt -p aios-tools && cargo test -p aios-tools --test hook_path_guard)`. Expected: all pass, the old tests unchanged.
  - [x] Acceptance: the tools gate; `(cd "$W" && just tools && target/tools/installed/aios hook path-guard --help | grep -- '--agent-type')` prints the flag's help line; `rg -n 'frontmatter of the' tools/src/cmd/hook/path_guard.rs` prints nothing; the docs gate (branch binary) shows only `plans-not-empty`.

- [x] **T5: the shim's `hook` branch** (TDD; implementer, then a guard-attacking reviewer)

  **Files:** Modify `.claude/hooks/aios` (replace with `D/.claude/hooks/aios.v4`, which is `origin/main`'s #203 shim plus the `hook` branch). Test `tools/tests/shim.rs`. Draft: `D/.claude/hooks/aios.patch-notes.md.v4` (the test table, rows 1–16).

  **Interfaces:** Consumes T4's flag form. Produces: `aios hook ...` always exits 0, never builds in the foreground, and runs only a fresh binary (Q5 default: stale and dirty fall back like missing).

  - [x] Step 1: add `run_stdin_at(&self, shim: &Path, args, envs, stdin: &str) -> Output` to `Sandbox` in `tools/tests/shim.rs`, next to `run_at` (same `isolated(&mut cmd)`, `self.path_env()` and `current_dir`; stdin piped and written as `guard_passes_on_only_the_binarys_own_0_and_2` does), plus `run_stdin(args, envs, stdin)` for the sandbox's own shim, a `PG` constant (`["hook", "path-guard", "--agent-type", "worker", "--deny", "kernel/"]`), a `payload(fields)` helper (a `PreToolUse` `Write` of `/x/kernel/a.rs` plus `fields`) and `is_deny(&Output)`. Use #203's helpers, not v3's: `FAKE_CARGO_DELAY` (not `FAKE_JUST_DELAY`), `built()`, `no_build_started()`, `wait_for_background_build()` (not `just_log()`), and `install(Some(source), fresh)` for any custom binary (a hand-written binary fails the stamp).
  - [x] Step 2: write the failing tests, one per row of the draft's table (rows 1–16; row 12 extends the existing `a_linked_worktree_fails_closed_when_git_cannot_name_the_main_checkout` fixture with `hook route-outcome` and `{}` on stdin).
  - [x] Step 3: `(cd "$W" && cargo test -p aios-tools --test shim)`. Expected: the new tests fail on `main`'s shim (`hook` takes the `aios <other>` path: a foreground build, exit 3, the old binary's output), and the existing tests pass.
  - [x] Step 4: `cp "$D/.claude/hooks/aios.v4" "$W/.claude/hooks/aios"` (mode 755). This is the first `.claude/` edit by a foreground implementer: the owner observes `D/OWNER-PROBES.md.v5` OP3 here (and again in T7 and T8). If the owner answered Q5 "run it", apply the two-line change in the draft notes' Q5 section and flip rows 10, 14 and 15 first.
  - [x] Step 5: `(cd "$W" && cargo fmt -p aios-tools && cargo test -p aios-tools --test shim)`. Expected: all pass. (The v4 draft was smoke-tested on 2026-10-09 in a scratch repository with `D/.claude/hooks/aios.v4.smoke.sh`: rows 1–11 and 13–16 and `guard` on a dirty stamp behaved as the table says.)
  - [x] Acceptance: the tools gate; `sh -n`, `dash -n`, `shellcheck -s sh` on `.claude/hooks/aios` exit 0; `git -C "$W" diff origin/main --stat -- .claude/hooks/aios tools/tests/shim.rs` lists only those two files; `git -C "$W" diff origin/main -- .claude/hooks/aios | grep -c '^-[^-]'` prints `2` or less (#203's lines stay: only the `stop` comment and its case list change).

- [x] **T6: the host QEMU lock wrapper**

  **Files:** Create `scripts/agent/qemu-lock.sh` (from `D/scripts/agent/qemu-lock.sh.v4`, mode 755).

  - [x] Step 1: copy the draft; `chmod 755`.
  - [x] Step 2: the smoke test, in a scratch repository outside `W` (the fake QEMU is a symlink, because a copied system binary is killed by code signing):
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
  - [x] Step 3 (v4): the harness-first stop check: `sh "$D/scripts/agent/qemu-lock.sh.v4.t8-smoke.sh" "$W/scripts/agent/qemu-lock.sh" "$(mktemp -d)/t8"` prints `rc=143 qemu-running-at-harness-term`, `free` and `no-qemu-left` (v3's leaf-first stop printed `qemu-already-gone-at-harness-term`; run 2026-10-09, three times each).
  - [x] Step 4 (review fixes, 2026-10-10): save the block below as `t6-review.sh` and run `sh t6-review.sh "$W/scripts/agent/qemu-lock.sh" "$(mktemp -d)/t6r"`. Expected: (a) `rc=143`, `free`, `no-sleep-left` (an ignore-TERM command is SIGKILLed, the lock released); (b) `rc=78`, `survivors=<pids>` and `state=dead` (a survivor that cannot be killed keeps the lock; `AIOS_QEMU_LOCK_KILL=true` stands in for a failed SIGKILL), `1`, `cleaned-up`; (c) `not cleared: the lock changed hands`, `rc=1`, the new holder's `team=solo` and `pid=` still in the owner file, `0`; (d) one `cleared`, one `not cleared: the lock changed`, `rcs=0 1`, `1`, `free`, `0`; (e) `--team needs a value`, `--label needs a value`, `--max-load needs a value`, each `rc=2`, then `free`.
    ```sh
    #!/bin/sh
    # T6 review-fix smoke. usage: step4.sh <qemu-lock.sh> <scratch-dir>
    set -u
    SRC=$1
    S=$2
    mkdir -p "$S" && cd "$S" || exit 1
    git init -q -b main . && git -c user.email=a@b -c user.name=a commit -q --allow-empty -m s
    cp "$SRC" q.sh
    L=.git/aios-agent/qemu.lock
    export AIOS_QEMU_LOCK_LOADAVG=1 AIOS_QEMU_LOCK_GRACE=2

    echo "== a: a command that ignores TERM is SIGKILLed, the lock is released"
    printf 'trap "" TERM\nsleep 37\n' >ign.sh
    sh q.sh run --team solo --mode boot --label ign --eta-min 1 -- sh ign.sh &
    W=$!
    sleep 1
    kill -TERM "$(sed -n 's/^pid=//p' $L/owner)"
    wait $W
    echo "rc=$?"
    sh q.sh status
    pgrep -f 'sleep 37' || echo no-sleep-left

    echo "== b: a survivor that cannot be killed keeps the lock"
    AIOS_QEMU_LOCK_KILL=true sh q.sh run --team solo --mode boot --label unk --eta-min 1 -- sh ign.sh 2>b.err &
    W=$!
    sleep 1
    kill -TERM "$(sed -n 's/^pid=//p' $L/owner)"
    wait $W
    echo "rc=$?"
    sh q.sh status | grep -E '^(state|survivors)='
    grep -c 'survived SIGKILL; the lock stays held' b.err
    sed -n 's/^survivors=//p' $L/owner >surv
    for p in $(cat surv); do kill -KILL "$p" 2>/dev/null; done
    sleep 1
    pgrep -f 'sleep 37' || echo cleaned-up
    rm -rf $L

    echo "== c: a lock replaced between is_stale and the move survives clear-stale"
    mkdir -p r2 && cd r2 && git init -q -b main . && git -c user.email=a@b -c user.name=a commit -q --allow-empty -m s
    cp ../q.sh q.sh
    sh q.sh run --team solo --mode boot --label holder --eta-min 1 -- sleep 15 &
    H=$!
    sleep 1
    HP=$(sed -n 's/^pid=//p' .git/aios-agent/qemu.lock/owner)
    cd ..
    mkdir -p shim && cat >shim/pgrep <<EOF
    #!/bin/sh
    # first call: swap the stale lock for a live holder's, as a racing run would
    if [ ! -f $S/swapped ]; then
        : >$S/swapped
        rm -rf $S/$L
        mkdir $S/$L
        printf 'team=solo\npid=$HP\n' >$S/$L/owner
        exit 1
    fi
    exec /usr/bin/pgrep "\$@"
    EOF
    chmod 755 shim/pgrep
    mkdir -p $L && printf 'team=team-fix\npid=999999\n' >$L/owner
    PATH="$S/shim:$PATH" sh q.sh clear-stale; echo "rc=$?"
    sleep 0.2
    cat $L/owner
    ls .git/aios-agent | grep -c clearing || true
    kill -TERM "$H" 2>/dev/null; wait $H
    rm -rf $L

    echo "== d: two concurrent clear-stale calls on one stale lock"
    mkdir -p $L && printf 'team=team-fix\npid=999999\n' >$L/owner
    rm -f .git/aios-agent/qemu-lock.log
    sh q.sh clear-stale >d1.out 2>&1 &
    P1=$!
    sh q.sh clear-stale >d2.out 2>&1 &
    P2=$!
    wait $P1; R1=$?
    wait $P2; R2=$?
    cat d1.out d2.out | sort | uniq -c
    echo "rcs=$(echo $R1 $R2 | tr ' ' '\n' | sort | tr '\n' ' ')"
    grep -c cleared_at .git/aios-agent/qemu-lock.log
    sh q.sh status
    ls .git/aios-agent | grep -c clearing || true

    echo "== e: missing option values"
    sh q.sh run --team; echo "rc=$?"
    sh q.sh run --team solo --mode boot --label; echo "rc=$?"
    sh q.sh run --team solo --mode boot --label x --eta-min 1 --max-load; echo "rc=$?"
    sh q.sh status
    ```
  - [x] Acceptance: `shellcheck -s sh scripts/agent/qemu-lock.sh` and `dash -n scripts/agent/qemu-lock.sh` exit 0, Step 3 prints its three lines, Step 4 prints what it lists, and the smoke test prints, in order: `free`; the t1 owner lines with `state=live` and `rc=75`; `deferred: load 40 is above 30 (boot mode)` and `rc=77`; `deferred: load 5 is above 3.0 (quiet mode)` and `rc=77`; `ran-t5`, `rc=0`; the fake QEMU's process line and `rc=76` (no `NOT-RUN` anywhere); `state=dead`, `cleared`, `rc=0`, `free`; `free`, `no-sleep-left`; `--team must be team-build, team-fix or solo`, `rc=2`. (Run on the v3 draft on 2026-10-06 and on the v4 draft on 2026-10-09: exactly this output.) `cat "$S/.git/aios-agent/qemu-lock.log"` shows the cleared owner lines. The docs gate shows only `plans-not-empty`.

- [x] **T7: guard rules 1–4** (implementer in the foreground; guard-attacking reviewer)

  **Files:** Modify `.claude/hooks/git-push-guard.py`. Test `.claude/hooks/tests/test_git_push_guard.py`. Draft: `D/.claude/hooks/git-push-guard.py.patch-notes.md.v5` (every row and its exact deny text).

  - [x] Step 1: write one failing test per table row of the draft (rule 1 rows 1–7 plus the lock directory without an owner file; rule 2 rows 1–3 with `rustup +nightly component add rust-src`, `rustup toolchain install`, `rustup install` and `sh -c 'rustup update'`; rule 3 rows 1–4, row 2 with an agent file whose frontmatter has `isolation: worktree` and row 4 with one that has none, plus an unreadable agent file; rule 4 rows 1–7, plus row 4 through `git -C <branch worktree> reset --hard`, row 4 for a directory `agent-x` on a branch that is not `worktree-agent-x`, and row 6 through `sh -c 'git commit -m x'`; the `--classify` exclusion for `just soak` and `target/tools/installed/aios soak`, and a denied `target/tools/installed/aios soak runs=1`). Each test builds the payload the guard reads (`tool_name`, `tool_input`, `cwd`, optional `agent_id` and `agent_type`) and the environment (`AIOS_TEAM`), and asserts the decision and a substring of the reason. The temporary-worktree fixture is a scratch repository with a linked worktree at `.claude/worktrees/agent-x` on `worktree-agent-x` (T2 H2 passed: an agent's payload `cwd` is that worktree).
  - [x] Step 2: `(cd "$W" && /usr/bin/python3 -m unittest discover -s .claude/hooks/tests)`. Expected: the new tests fail, the existing 55 pass.
  - [x] Step 3: implement the four rules in the existing analyzer (reuse its command resolver for `sh -c`, `eval`, aliases and script files, its git option scanner for `reset --hard`, and its working-tree and branch resolution; any exception still becomes `ask`), route `Agent` payloads to rule 3 and `EnterWorktree` payloads to rule 4, check rule 4 rows 4–6 on Bash and Monitor, and extend the module docstring's table and first line.
  - [x] Step 4: re-run the tests. Expected: all pass.
  - [x] Acceptance: the guard gate; the test count grows by at least 30 (`... -v 2>&1 | grep -c ' ok$'` before and after); `rg -n 'only the verifier starts QEMU|never pattern-kill QEMU|agents never change the toolchain|becomes a teammate|team leads stay in the main checkout|agents never call EnterWorktree|agents reset only their own temporary worktree|exists only for an isolated agent|agents never commit in the main checkout' .claude/hooks/git-push-guard.py` finds each text; `rg -n 'tools:.*EnterWorktree|not pinned' .claude/hooks/git-push-guard.py` prints nothing.

- [ ] **T8: settings** (the owner approves each block; implementer in the foreground)

  **Files:** Modify `.claude/settings.json`. Delete `.claude/hooks/precompact-save.sh`. Drafts: `D/.claude/settings.json.apply.jq.v5`, `D/.claude/settings.json.patch-notes.md.v5`, `D/.claude/hooks/precompact-save.sh.DELETE.md`.

  - [ ] Step 1: show the owner the diff: `S=$(mktemp -d); jq -f "$D/.claude/settings.json.apply.jq.v5" "$W/.claude/settings.json" > "$S/settings.new.json" && diff <(jq -S . "$W/.claude/settings.json") <(jq -S . "$S/settings.new.json")`, block by block (env, effortLevel, hooks, allow, ask, deny, enabledPlugins). `W`'s settings are `main`'s at e430305 or later after T2 Step 4's merge; the jq's output on e430305 is `D/.claude/settings.json.v5-output-e430305.json`. The allow block includes Q8's `Bash(git reset --hard *)` (drop it if the owner answered Q8 no, and record that writers then run in the foreground).
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
    jq -e '([.. | objects | select(.type? == "agent") | .model] | (length == 1 and all(. == "claude-fable-5-1"))) and (.permissions.allow | index("Bash(git reset --hard *)") != null) and ([.permissions.allow[] | select(. == "Bash(AIOS_TOOLS_BIN=target/tools/installed/aios just docs-check)")] | length == 1)' $F
    jq -e '[.. | strings | select(test("FABLE-GATE|fable-gate"))] | length == 0' $F
    ```
    Expected: `true`, `true`; `PostToolUse,PostToolUseFailure,PreToolUse,SessionStart,SubagentStop`; exactly these ten lines:
    ```text
    -=command:"$CLAUDE_PROJECT_DIR"/.claude/hooks/setup-dev-env.sh
    Bash|Monitor|Agent|EnterWorktree=command:/usr/bin/python3 "$CLAUDE_PROJECT_DIR"/.claude/hooks/git-push-guard.py
    Edit|Write|MultiEdit|NotebookEdit=command:path-guard --agent-type worker --deny kernel/ --deny uefi-stub/ --deny shared/
    Agent=command:route-shadow(async)
    ExitPlanMode=agent:(claude-fable-5-1)
    Bash=command:repeat-error
    Agent=command:route-outcome(async)
    Bash=command:repeat-error
    Agent=command:route-outcome(async)
    -=command:route-outcome(async)
    ```
    then `true`, `true`, `true`, `gone`, `true`, `true`, `true`, `true`, `true` (v4's #223 and #203 checks, #223/#229's rules kept, then v5's: the one agent hook on `claude-fable-5-1`, Q8's allow, #232's docs-check allow present once, and no step-gate text). (All pass on the v5 jq output from e430305, `D/.claude/settings.json.v5-output-e430305.json`, 2026-10-09; with Q8 answered no, the second-to-last check fails on its middle clause only.)

- [ ] **T9: rules**

  **Files:** Create `.claude/rules/11-teams.md` (from `D/.claude/rules/11-teams.md.v5`). Modify `.claude/rules/02-quality-gates.md`, `03-git-workflow.md`, `04-phase-workflow.md`, `08-knowledge-hive.md` per `D/.claude/rules/02-03-04-08.patch-notes.md.v5`.

  - [ ] Step 1: add rule 11; apply the four rule patches.
  - [ ] Acceptance:
    ```bash
    cd "$W"
    rg -n 'TodoWrite|Team-lead updates|brew upgrade qemu|rebase open worktrees|until it is rebased' .claude/rules
    rg -n 'Objdump \| .cargo objdump|QEMU \| .just run' .claude/rules/02-quality-gates.md
    rg -c 'isolation: "worktree"|git reset --hard <tip>|merge --ff-only worktree-agent-|branch -d worktree-agent-|qemu-lock.sh|RUSTUP_AUTO_INSTALL|team-build|team-fix' .claude/rules/11-teams.md
    rg -lF 'qemu-lock.sh run' .claude/rules/11-teams.md .claude/rules/02-quality-gates.md
    rg -n 'team-lead' .claude/rules | rg -v 'not `team-\*`, `team-lead`'
    rg -n 'EnterWorktree\(\{path|SubagentStop. agent hook|Fable step gate|fable-gate' .claude/rules
    ```
    Expected: nothing; nothing; a count of 8 or more; both files; nothing; nothing. The docs gate shows only `plans-not-empty`.

- [ ] **T10: agents, and the Agents table**

  **Files:** Replace `.claude/agents/{kernel-dev,doc-writer,verifier,code-reviewer,doc-auditor}.md`; create `worker.md` and `skeptic.md` (all from `D/.claude/agents/`, using the `.v5` files for kernel-dev, worker, doc-writer, verifier and code-reviewer; doc-auditor and skeptic are the v3 drafts); delete `team-lead.md` (`D/.claude/agents/team-lead.md.DELETE.md`). Modify `.claude/CLAUDE.md`: the Agents table and the layout `agents/` line only (`D/.claude/CLAUDE.md.patch-notes.md.v5`, "Lines 165–175" agents line and "Lines 217–224"), because docs-check's harness-tables check compares them with the files.

  - [ ] Step 1: copy the seven files; `git rm .claude/agents/team-lead.md`; apply the two CLAUDE.md edits.
  - [ ] Acceptance:
    ```bash
    cd "$W"
    ls .claude/agents | tr '\n' ' '
    rg -n '^(memory|background|permissionMode|hooks):|MultiEdit|ListAgents|\bSkill\b|^tools:.*EnterWorktree' .claude/agents
    for f in .claude/agents/*.md; do printf '%s %s %s\n' "$(basename "$f" .md)" "$(sed -n 's/^model: //p' "$f")" "$(sed -n 's/^effort: //p' "$f")"; done
    rg -l '^isolation: worktree$' .claude/agents | sort | xargs -n1 basename | tr '\n' ' '
    rg -c 'git reset --hard <(tip|sha)>' .claude/agents/kernel-dev.md .claude/agents/worker.md .claude/agents/doc-writer.md .claude/agents/verifier.md
    rg -n '^## Lens: (plan|rules|bugs|diagnose)$' .claude/agents/code-reviewer.md | wc -l
    rg -n 'team-lead' .claude/agents .claude/CLAUDE.md
    rg -n 'fable-gate|FABLE-GATE|Fable step gate|Fable gates' .claude/agents .claude/CLAUDE.md
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
    then `doc-writer.md kernel-dev.md verifier.md worker.md ` (the writer marker guard rule 3 reads); a count of 1 or more for each of the four files; `4`; nothing; nothing. The docs gate shows only `plans-not-empty` (harness-tables and pointer-doctor clean).

- [ ] **T11: skills, and the Skills table**

  **Files:** Create `.claude/skills/justin/skills/team/SKILL.md` (from `SKILL.md.v5`). Replace `.claude/skills/justin/skills/{pause,brief,start}/SKILL.md` (pause from `SKILL.md.v5`), `.claude/skills/justin/.claude-plugin/plugin.json`, `.claude/skills/audit-loop/SKILL.md` (from `SKILL.md.v5`). Delete `.claude/skills/build-team/`. Modify `.claude/skills/merge-and-cleanup/SKILL.md` (`D/.claude/skills/merge-and-cleanup/SKILL.md.delta-notes.md`) and `implement-phase`, `verify-phase`, `review-pr-comments`, `write-arch-doc`, `generate-phase-doc` (`D/.claude/skills/skills.patch-notes.md.v5`). Modify `.claude/CLAUDE.md`: the Skills table and the layout `skills/` lines (`D/.claude/CLAUDE.md.patch-notes.md.v5`, "Lines 230–241" and the `skills/` lines).

  - [ ] Step 1: apply the files and notes. The team SKILL's Placement section and Restart bullet cover temporary worktrees (H1: Claude Code removes unchanged ones; ones with commits stay until the lead's clean-up), so no conditional edit remains.
  - [ ] Acceptance:
    ```bash
    cd "$W"
    rg -n 'remember:remember|TodoWrite|build-team|team-lead|just run 2>&1|cargo objdump|brew upgrade|agent-memory|agent memory' .claude/skills
    rg -n 'handoff-(build|ship|solo)\.md|handoff-<role>|aios-owner' .claude/skills
    ls .claude/skills/justin/skills | tr '\n' ' '
    jq -r .description .claude/skills/justin/.claude-plugin/plugin.json
    rg -c 'qemu-lock.sh status|rustup install|MAIN-MOVED|gated by' .claude/skills/merge-and-cleanup/SKILL.md
    rg -n 'EnterPlanMode|ExitPlanMode' .claude/skills/implement-phase/SKILL.md | wc -l
    rg -n 'FABLE-GATE|fable-gate|Fable step gate|EnterWorktree\(|enters a worktree' .claude/skills
    rg -c 'merge --ff-only worktree-agent-|branch -d worktree-agent-|rev-list --first-parent' .claude/skills/justin/skills/team/SKILL.md
    ```
    Expected: nothing; nothing; `brief doctor pause start team`; a description naming the five skills; 4 or more; 2 or more; nothing; 3 or more. The docs gate shows only `plans-not-empty`.

- [ ] **T12: the audit workflow**

  **Files:** Create `.claude/workflows/audit-loop.js`. Draft: `D/.claude/workflows/audit-loop.js.patch-notes.md`; source `<git-common-dir>/aios-agent/audit/branch-audit-loop.v2.js`.

  - [ ] Step 1: the implementer loads the `workflow-authoring` skill, then writes the workflow per the notes (one round, typed lenses, skeptics by severity, a pool of six, the ledger args, the return shape).
  - [ ] Step 2 (controller, before the fast-forward): run one docs-mode round headless inside the implementer's worktree `A`, which holds the committed workflow on top of T10's agents (`A` and, after the fast-forward, `W` are the only checkouts where `skeptic` and the new `code-reviewer` exist; Execution, Bootstrap). Set `W` to `A` for this block:
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

  **Files:** Modify `.gitignore` (`D/.gitignore.patch-notes.md`), `docs/knowledge/discussions/2026-09-22-jl-rust-agent-tools.md` (`D/docs/knowledge/discussions/...patch-notes.md.v5`: re-anchored on e430305; no edit touches #232's lines 126 and 225), `.github/workflows/claude-code-review.yml` (`D/.github/workflows/claude-code-review.yml.patch-notes.md`, only if Q4 is yes).

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

  **Files:** Modify `.claude/CLAUDE.md` (the remaining sections of `D/.claude/CLAUDE.md.patch-notes.md.v5`), `docs/project/agent-loop.md`, `docs/project/developer-guide.md`, `docs/project/ai-agent-context.md` (their `.v5` notes in `D/docs/project/`).

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
    rg -n 'Fable step gate|two Fable gates|fable-gate|plus EnterWorktree|then call `EnterWorktree`|You enter it with EnterWorktree|pinned-agents-enter-worktrees' .claude/CLAUDE.md docs/project
    rg -n '^### Placement$' docs/project/agent-loop.md
    ```
    Expected: a line in each file; a line in each file; both headings; 4 or more; nothing; nothing; nothing; one line. The docs gate shows only `plans-not-empty`, with harness-tables, pointer-doctor and repo-paths clean.

- [ ] **T16: headless end-to-end checks, the wiring lesson and the ADR amendment**

  **Files:** Modify `docs/knowledge/lessons/2026-10-06-jl-model-routing-wiring-checks.md` (append E1–E9), `docs/knowledge/lessons/2026-10-06-jl-isolated-writers-fast-forward.md` (append E1 and E5's placement results), `docs/knowledge/decisions/2026-10-06-jl-model-routing-hooks.md` (dated amendment, `D/docs/knowledge/decisions/2026-10-06-jl-model-routing-hooks.md.amendment-notes.md.v5`), the plan ("Issues Encountered").

  - [ ] Step 1 (controller): run `D/probe/HEADLESS.md.v5`, section "T16": setup, runs E and G, the checks E1–E9 (E6 is OWNER-PROBES OP2), teardown.
  - [ ] Step 2: any failing check goes back to the task that owns it (E1, E7: T4/T5/T8; E2: T7; E3, E4: T8; E5: T10/T11; E8: T10, code-reviewer's frontmatter model; E9: T7) as a fix round, then T16 runs again.
  - [ ] Step 3 (owner): run `D/OWNER-PROBES.md.v5` OP2; paste the report block under "Issues Encountered". A failing OP2 is a fix round for T8/T11 like Step 2.
  - [ ] Step 4 (implementer): append the results (E1–E9, OP2) to the two lessons; write the ADR amendment with every `<...>` field filled.
  - [ ] Acceptance: E1–E5 and E7–E9 pass; the OP2 report says `result: pass` (E6); `rg -n '<(date of T16|n|date|version|E8 result)>' docs/knowledge/decisions/2026-10-06-jl-model-routing-hooks.md` prints nothing; `git -C "$W" diff origin/main --stat -- docs/knowledge/decisions/` shows only additions at the end of the ADR (`git -C "$W" diff origin/main -- docs/knowledge/decisions/2026-10-06-jl-model-routing-hooks.md | grep -c '^-[^-]'` prints `0`); `git -C "$MAIN" branch --list 'worktree-agent-*'` lists none of T16's temporary branches. The docs gate shows only `plans-not-empty`.

- [ ] **T17: gates, audit, distillation**

  **Files:** Whatever the audit's confirmed findings touch; delete the plan.

  - [ ] Step 1: all gates in `W`: the tools gate, the guard gate, `shellcheck` on `scripts/agent/qemu-lock.sh`, `scripts/agent/brief.sh` and `.claude/hooks/aios`, `actionlint .github/workflows/*.yml`, `just check` (zero warnings; no kernel change, so this only confirms nothing broke).
  - [ ] Step 2: `/audit-loop` rounds, each run headless inside `W` as in T12 (with the same `--allowedTools` list, `W` the real branch worktree) with `mode` from the audit-loop SKILL's classification (`tools`, since `tools/`, `scripts/`, `.claude/` changed) and the latest gate output; the controller routes each confirmed in-diff finding to an implementer, runs Placement on its range (Execution; it ends with the push), and starts the next round. Stop rule: the SKILL's (a complete round with an empty `in_diff`; or 4 rounds, or 2 incomplete in a row, which leaves the PR a draft behind a `needs-human` gate issue).
  - [ ] Step 3: distil (rule 04, step 11): move anything in "Issues Encountered" and "Decisions Made" that no lesson or the ADR amendment holds yet into a lesson; then `git rm docs/knowledge/plans/2026-10-06-jl-two-team-harness.md`.
  - [ ] Acceptance: every Step 1 command exits 0; the last round's JSON (`<git-common-dir>/aios-agent/audit/claude-harness-teams/<head>.json`) has `"complete": true` and `"in_diff": []`; `(cd "$W" && just docs-check)` prints `No new drift since baseline.`

- [ ] **T18: ready, PR comments, hand-off**

  - [ ] Step 1: `gh pr ready <n>`; run `/review-pr-comments` (fixes go to implementers; reply and resolve after the fix is pushed).
  - [ ] Step 2: hand off: report the PR URL and `gh pr checks <n>`, list T19 for the owner, and ask the owner to run `/merge-and-cleanup <n>`. Never merge.
  - [ ] Acceptance: `gh pr view <n> --json isDraft --jq .isDraft` prints `false`; `gh pr checks <n>` shows every required check passing.

- [ ] **T19: after the merge** (owner, with the leads; local state only)

  - [ ] Step 1: rebuild the binary on `main` and restart nothing yet: `(cd "$MAIN" && just tools && tail -n 1 target/tools/installed/aios.stamp && target/tools/installed/aios hook path-guard --help | grep -c -- --agent-type)` prints `source clean` and `1`.
  - [ ] Step 2 (owner): remove `remember@claude-plugins-official` from `~/.claude-personal/settings.json` (O1).
  - [ ] Step 3: nothing to do. T2's H6 showed `main high`, so the owner's user-level `effortLevel` does not apply and `~/.claude-personal/settings.json` keeps it.
  - [ ] Step 4 (O2): split `.remember/remember.md` by its session sections into `.remember/handoff-team-fix.md` (the aios-de section) and `.remember/handoff-team-build.md` (the aios-e1 and aios-ae sections), rewritten under the five pause headings; move `.remember/logs/` to `<git-common-dir>/aios-agent/remember-retired/logs/`; delete the rest of `.remember/` except the two handoff files.
  - [ ] Step 5: team-build deletes `.claude/agent-memory/` in the main checkout and in its own worktrees (`phase-7-m26`, `justin-loop` and any other team-build worktree; `tools-r4` and `ci-soak-matrix` were removed when #230 and #227 merged, and #230's `/merge-and-cleanup` already merged `tools-r4`'s two notes into the main checkout's store). team-fix moves crash-1b's working notes into crash-1b's plan (migration notes §4), then deletes its worktrees' stores.
  - [ ] Step 6: label the open branches' PRs and issues `team-build` or `team-fix` (team-build: justin-loop, phase-7-m26 #149, and the Q7 soak-matrix port issue; team-fix: crash-1b, cap-lifetime, and the kernel bugs #217, #212, #215, #216, #186–#190, #187; tools-203, tools-205-206, tools-r4, ci-soak-matrix and docs-184 have merged), and file the cross-team gate issues that still apply, each titled `Gate: merge PR #<n> only after ...` with `needs-human`: toolchain PRs never between A/B arms; #149 after the boot-crash fix (#165 already gates it). The v3 gate "crash-1b before R4's deletion of `scripts/soak-qemu.sh`" is moot: R4 merged first (1a5c363), so team-build sends team-fix a `HEADS-UP`: crash-1b's next merge of `main` deletes `scripts/soak-qemu.sh`, its soak commands become `just soak` (or `aios soak`) through the lock wrapper, and `scripts/soak-matrix.sh` cannot run post-R4 arms until Q7's port.
  - [ ] Step 7: restart both leads per agent-loop.md, Teams, Launch (settings, agents and hooks load at session start); this is `D/OWNER-PROBES.md.v5` OP1. team-build records the OP1 report in the wiring lesson through a small docs PR (or an issue if it fails).
  - [ ] Step 8: each restarted lead has a writer merge `main` into each of its own active worktrees, through Placement (rule 11: "merge origin/main" is the one task allowed on a branch whose tip lacks `.claude/rules/11-teams.md`; a kernel-dev merge is reviewed on its remerge diff).
  - [ ] Acceptance: `ls "$MAIN/.remember"` prints only `handoff-team-build.md handoff-team-fix.md`; `find "$MAIN" -path '*/.claude/agent-memory' -prune -print` prints nothing once team-fix has done its part; `for w in $(git -C "$MAIN" worktree list --porcelain | awk '/^worktree /{print $2}' | grep -v '/\.claude/worktrees/agent-'); do test -f "$w/.claude/rules/11-teams.md" || echo "$w"; done` prints nothing; the OP1 report says `result: pass`, and OP3's report (from T5, T7, T8) is in the distilled wiring lesson.

## Dependencies & Risks

- **Depends on:** #220 (merged, 5838ad2), #221 (merged, f12dbc7), #203 (merged, aa52bff), Tools R4 (merged, 1a5c363) and #232 (merged, e430305); the owner's answers to Q1–Q7 (T0) and Q8 (before T7); an attended session for every `.claude/**` edit.
- **Risk: a mechanism differs from the strings.** T2 ran before any task relied on it (2.1.292); H3 failed and the plan was re-planned (D3), H7 changed D8 and the hook model id. A later CLI can change them again: agent-loop's "Re-check after each Claude Code update" re-runs `D/probe/HEADLESS.md.v5`, distilled into the two T2 lessons.
- **Risk: a writer forgets the reset.** Its temporary branch then starts at the main checkout's `HEAD` (`main`), not at `W`'s tip. If `W`'s tip is not an ancestor, `merge --ff-only` refuses. If it is (a branch with no commits of its own, behind `main`), the fast-forward would pull `main`'s newer commits into `W` without a merge. Mitigations: the agent's preflight checks `HEAD` equals the tip after the reset; its report names `<tip>..<head>` and the commit count; the lead's range check (first-parent count equals the reported commits, the oldest one's parent is the tip) refuses the range before any fast-forward (Review Focus 6).
- **Risk: two writers race on one branch.** Both reset to the same tip; the second fast-forward is refused, and its work is redone from the new tip (a cherry-pick of the refused range is allowed). Mitigation: one writer per branch worktree at a time (rule 11; two writers only on two branches); the refusal itself is safe, because nothing merges.
- **Risk: temporary worktrees holding commits after a refused fast-forward, a crash or a usage-limit stop.** Claude Code removes only unchanged temporary worktrees (H1). Leftovers keep commits on local `worktree-agent-*` branches that are never pushed, so a lost main checkout loses them. Mitigations: `/justin:pause` records them under "Pending ranges"; `/justin:team` restart resumes or removes them; a refused range is kept until its redo lands, then deleted with `git -C <W> branch -D`, which the guard puts to the owner; `checkpoint.sh` lists them as "not a claude/* branch", and pause tells the lead not to push them. `/merge-and-cleanup` does not look at them (a temporary branch cannot be tied to a PR by name), so a pending range left when a PR merges stays local until a lead resumes or deletes it.
- **Risk: cold builds.** Every temporary worktree starts with an empty `target/`: kernel, stub and disk image for kernel-dev and the verifier, the release build of `aios-tools` when a recipe depends on `tools` (`just soak`, `just docs-check` with the branch binary). It costs minutes, host load and a few GB of disk per worktree until the lead removes it. `CARGO_TARGET_DIR` is not shared (D3: the justfile's relative `target/` paths, `W`'s artifacts describing `W`'s head, cargo's build-directory lock). If cold builds hurt, a separate PR can add a compiler cache; this one does not.
- **Risk: the verifier's builds and results.** It builds cold in its own worktree, so `just tools` and `just disk` run before the lock and `just soak --no-build` inside it (otherwise cargo would run under the lock and inflate the load gate's reading). Its worktree may be removed when it ends (an unchanged-looking worktree: no commits beyond the creation base), so it writes run directories to `<W>/target/soak/` by absolute path; that relies on Bash child processes writing outside the agent's worktree, which works with no sandbox configured (T2 A2b's `git switch` wrote the common git directory). A future sandbox that confines Bash writes would break it: then the verifier must commit nothing and the lead copies the run directories before removing its worktree, which needs the worktree to survive. No QEMU boots in this PR, so OP1 and the first post-merge gate soak are its first real runs.
- **Risk: the guard rules under the new placement.** Rule 3 row 2 reads a frontmatter key (`isolation: worktree`) instead of `tools:`; an agent file without it is not protected, which T10's acceptance checks for the four writer types. Rule 4 rows 4–6 depend on Claude Code's temporary-worktree names (`agent-<id>`, `worktree-agent-<id>`): a renamed scheme makes rule 4 deny every agent reset (fail closed), which stops all writers until the guard follows; T16's E9 and E1 catch it on the CLI in use. Rule 4 row 6 denies agent commits in the main checkout only; an agent spawned without isolation in some other checkout is caught by rule 3 row 2 for project writers only. The `git reset --hard *` allow (Q8) is safe only while rule 4 runs: a session started with the guard disabled or failing open loses the confinement (the guard fails to `ask`, not open, on errors).
- **Risk: the shim's freshness check on every Bash call, edit and Agent call.** #203's `verdict` (find, `git ls-tree`, `git hash-object` of the 4 MB binary) plus a hard link: about 130 ms per call at load 138 (measured 2026-10-09 on main's shim); about 68 ms idle (discussion doc).
- **Risk: Fable spend.** The plan gate (per plan), the lead's review of each kernel-dev range (at most three rounds, plus a remerge-diff review per kernel-dev merge of `main`) and two audit lenses per round. T16 runs the range review once or more.
- **Risk: textual conflicts** with crash-1b (`.claude/CLAUDE.md`, rule 05) and any harness PR merged after e430305. #203, #205/#206, R4 and #232 have merged; the drafts are anchored on them. The second to merge resolves (D13); re-anchor drafts by text.
- **Risk: a stop kills QEMU under `aios soak`.** v3's wrapper TERMed leaves first, so the harness could record a QEMU killed by the stop as a boot. v4 stops the harness first (T6 Step 3).
- **Risk: a guard bug blocks spawns or boots.** Every rule has unit tests (T7), the guard fails to `ask`, and T16 exercises rules 3 and 4 for real.

## Issues Encountered

(filled during execution; T2 and T16 paste their check outputs here)

### T2 headless checks (2026-10-09, CLI 2.1.292)

Results: T2 headless mechanism checks (CLI 2.1.292, 2026-10-09, scratch `<scratch>`).

| # | Result | Evidence |
|---|---|---|
| H1 | pass (cleanup) / n/a (commits) | temp `agent-*` worktrees removed when unchanged; no probe commits because EnterWorktree failed (H3) |
| H2 | pass | PreToolUse payloads carry agent_id + agent_type; cwd = the agent's own `.claude/worktrees/agent-<id>`; `_project_dir` = repo root (CLAUDE_PROJECT_DIR does not follow the agent) |
| H3 | **FAIL** | isolated subagent's EnterWorktree(path=<branch worktree>) refused: "would not give this agent write access there ... its writes are limited to that folder". D3 as written is not available on 2.1.292 |
| H4 | pass | SubagentStop records for probe-writer (fg, incl. stop_hook_active true) and probe-kdev (bg) |
| H5 | pass | launched/stopped agent_id identical (fg completed, bg async_launched) |
| H6 | pass | main high, probe-writer low, probe-kdev medium |
| H7 | inconclusive | the `model: fable` agent hook ran (debug: "Processing agent hook", Grep + StructuredOutput) but probe-kdev could not write (H3), only one probe-kdev stop logged; re-run after the placement re-plan |
| H8 | no record in -p | ExitPlanMode not reached in -p (planned fallback: OWNER-PROBES OP2) |

Alternatives probed (run A2):
- A2a: non-isolated subagent + EnterWorktree → refused ("only available to sessions whose working directory is inside a worktree"); the agent then committed on `main` in the repo root (unsafe default).
- A2b: isolated subagent runs `git switch <branch>` (branch checked out nowhere else) in its own temp worktree → works, commits land on the branch; the temp worktree and its `worktree-agent-*` branch stay behind (they hold commits) until removed.

#### Run A3 (isolated + fast-forward placement) — owner chose this for D3 (2026-10-09 22:00)
- Works: isolated writers `git reset --hard <branch tip>` on their own `worktree-agent-*` branch, commit; the lead runs `git -C <W> merge --ff-only worktree-agent-<id>`; W then holds the commits (fg and bg both verified).
- Cleanup: `git worktree remove .claude/worktrees/agent-<id>` works once merged; `git branch -d worktree-agent-<id>` must run with `-C <W>` (from the main checkout, HEAD=main does not contain the commit, so -d refuses). A writer's temp worktree with commits is NOT auto-removed.

#### H7 root causes
1. `SubagentStop` agent-hook blocks are discarded for subagents on 2.1.292: debug `Agent hook condition was not met: PROBE-GATE-BLOCK ...` then `[end-turn] Stop hook block discarded (turn ended by tool result, no model re-invoke)`. Subagents end via a hand-back tool call, so a SubagentStop block never reaches them. → D8 cannot be a hook. Owner (2026-10-09 22:04): the lead runs the Fable step review (code-reviewer, rules+bugs lenses) on the writer's temp-branch range BEFORE fast-forwarding; must-fix → fresh kernel-dev spawn; max 3 rounds, then lead/owner decides.
2. Agent hooks pass `model` to the API verbatim: `model: "fable"` → `404 not_found_error "model: fable"`, hook returns no structured output. `claude-fable-5-1` works (probe 2, its own `<scratch 2>`). Every agent hook (D9 plan gate) must use `claude-fable-5-1`. Agent frontmatter / Agent-tool `model: fable` aliases are a separate path: verify in T16.
3. A PreToolUse agent hook on `claude-fable-5-1` that returns ok false denies the tool call (probe 3, 2026-10-10, its own `<scratch 3>`: "Hook denied tool use for Bash", and `ran.txt` was never created; see Raw excerpts). Probe 2 does not show this: its hooks returned ok true. So D9 (ExitPlanMode PreToolUse gate) stays viable; ExitPlanMode itself is untestable in -p (H8) → OP2.

#### Raw excerpts

Decisive lines only. `<scratch>` is the T2 run's scratch directory (runs A, A2, A3), `<scratch 2>` probe 2's, `<scratch 3>` probe 3's.

`claude --version` (`<scratch>/version.txt`):

```text
2.1.292 (Claude Code)
```

Run A (`<scratch>/run-a.json`, `result` field): the foreground probe-writer's EnterWorktree refusal (H3), and step 3's count:

```text
"Entering <scratch>/repo/.claude/worktrees/probe would not give this agent write access there, so the switch was not made. This agent is isolated in the worktree <scratch>/repo/.claude/worktrees/agent-a4e307693b3a5f88c, so its writes are limited to that folder. This path is in a different worktree (<scratch>/repo/.claude/worktrees/probe). ..."
stops=1
```

Run A2 (`<scratch>/run-a2.json`, `result` field). A2a, non-isolated:

```text
"Cannot enter worktree: the current working directory <scratch>/repo is the repository root, not an isolated worktree — switching is only available to sessions whose working directory is inside a worktree of this repository."
[main 431213f] probe-a2a
```

A2b, isolated:

```text
$ git branch --show-current
worktree-agent-ad917417c34f2a84d
$ git switch claude/probe2
Switched to branch 'claude/probe2'
$ git commit -m probe-a2b
[claude/probe2 20b11a2] probe-a2b
 1 file changed, 1 insertion(+)
```

Run A3 (`<scratch>/run-a3.json`, the element with `type == "result"`, its `result` field). The foreground writer's reset, then the lead's step 3 from the repository root:

```text
$ git reset --hard 9c6e3f4d7f5f3b6fa10705cf0b44e31716d98776
HEAD is now at 9c6e3f4 probe
[worktree-agent-a2ddd816fd4503b80 f8e1d35] probe-fg
Updating 9c6e3f4..f8e1d35
Fast-forward
error: the branch 'worktree-agent-a2ddd816fd4503b80' is not fully merged
hint: If you are sure you want to delete it, run 'git branch -D worktree-agent-a2ddd816fd4503b80'
```

The background writer, then the lead's step 6 fast-forward:

```text
HEAD is now at f8e1d35 probe-fg
[worktree-agent-a8f7405866b59f0ba d13127d] probe-bg
Updating f8e1d35..d13127d
Fast-forward
```

`<scratch>/run-a3.debug` (the `SubagentStop` agent-hook block, H7):

```text
1083: [DEBUG] Hooks: Agent hook condition was not met: PROBE-GATE-BLOCK 1: append the line second-pass to probe-bg.txt, commit it with the message probe-second-pass, then stop.
1084: [DEBUG] [end-turn] Stop hook block discarded (turn ended by tool result, no model re-invoke): Agent hook condition was not met: PROBE-GATE-BLOCK 1: ...
```

Probe 2 (`<scratch 2>/d.log`; two `PreToolUse` agent hooks on Bash, `model: "fable"` and `model: "claude-fable-5-1"`, both told to return ok true):

```text
593: [DEBUG] [API:timing] dispatching to firstParty model=fable
596: [DEBUG] [API:timing] dispatching to firstParty model=claude-fable-5-1
601: [ERROR] API error (attempt 1/11): 404 404 {"type":"error","error":{"type":"not_found_error","message":"model: fable"},...}
612: [DEBUG] Hooks: Agent hook did not return structured output
628: [DEBUG] Hooks: Agent hook condition was met
```

Probe 3 (2026-10-10, `<scratch 3>/d.log`; one `PreToolUse` agent hook on Bash, `model: "claude-fable-5-1"`, told to always return ok false; `claude -p --model sonnet --allowedTools Bash`, asked to run `echo PROBE-RAN > ran.txt`):

```text
594: [DEBUG] [API:timing] dispatching to firstParty model=claude-fable-5-1
613: [DEBUG] Hooks: Got structured output: {"ok":false,"reason":"PROBE-PRETOOL-BLOCK do not run this command."}
614: [DEBUG] Hooks: Agent hook condition was not met: PROBE-PRETOOL-BLOCK do not run this command.
618: [DEBUG] Hook denied tool use for Bash
619: [DEBUG] Bash tool permission denied
```

`<scratch 3>/r/ran.txt` was never created.

### T2 Steps 3–4 placement (2026-10-10, CLI 2.1.292)

The first live use of D3 in this repository. The controller's Placement commands, from the main checkout:

```text
$ git -C "$W" rev-list --first-parent --count adb77da..worktree-agent-a77f7be750f795ed5   -> 2
oldest first-parent commit's parent == adb77da -> yes
$ git -C "$W" merge --ff-only worktree-agent-a77f7be750f795ed5   -> fast-forward to 39ddd69
$ git worktree remove .claude/worktrees/agent-a77f7be750f795ed5   -> ok
$ git -C "$W" branch -d worktree-agent-a77f7be750f795ed5   -> Deleted branch worktree-agent-a77f7be750f795ed5 (was 39ddd69).
$ git -C "$W" push origin claude/harness-teams   -> ok
```

The writer's `git reset --hard <tip>` ran without a permission prompt in that session (auto mode).

**Deviation:** Steps 3 and 4 ran in one writer spawn, the merge first and then the plan commit: 62cb33e merges `origin/main` at af59149 (#235, which contains e430305) into adb77da, and 39ddd69 (plan v5 and the T2 results) sits on top. The first-parent order, newest first, is therefore plan v5, then the merge, then T1, the reverse of the T2 acceptance's listing ("the merge of `origin/main`, the plan v5 commit and T1's"). The Step 5 lessons commit goes on top of 39ddd69. Nothing else changes: `merge-base --is-ancestor e430305 HEAD` holds and the range check counted the merge as one first-parent commit.

**Step 5 writer (2026-10-10):** two of its Bash calls were refused by Claude Code's worktree-isolation check, not by a permission rule: a compound command that set a shell variable to a path under `.git/` ("this command names git in a form too complex to verify that it stays inside the worktree"), and a `sed -i` whose file argument was an unquoted variable ("runs sed with a value computed at runtime (the variable P) where an option may stand ... so what it runs cannot be shown not to be git"). Both were redone as plain commands or with the Read and Edit tools. Recorded in lesson `2026-10-06-jl-isolated-writers-fast-forward.md`.

### T4 review fixes (2026-10-10)

The guard-attacking review of 103047e found two ways the `worker` agent could still write under a denied prefix, and one ordering nit. (1) `target_path` read `file_path` and only fell back to `notebook_path`, so `NotebookEdit` with `file_path: docs/a.md` and `notebook_path: kernel/n.ipynb` was allowed. Fix: every path field present (`file_path`, `notebook_path`) is checked and any one under a prefix denies; a present field that is not a non-empty string, or no field at all, is an error (deny). (2) A leading `~` was joined to `cwd` as a relative name, so `~/<repo>/kernel/src/x.rs` was allowed. Fix: a path whose first component starts with `~` (`~`, `~/...`, `~user/...`) is denied unexpanded, because whether Claude Code expands it is not verified. Nit: `run` now filters by tool name, then agent type (`scope`), then reads paths, so a malformed call from a non-worker gets no decision and the same call from the worker or an unidentified subagent is denied. Tests were written first and failed (5 failures), then passed.

### T5 review fixes (2026-10-10)

The guard-attacking review of 6beae0e found two inputs the shim's path-guard fallback allowed that the binary denies. (1) A payload with an `"agent_type"` key whose value is empty, `null` or not a string, and no `agent_id`, printed nothing; the fallback now denies whenever the key is present and no non-empty string is captured. (2) The greedy sed took the last `"agent_type"` on the line, which could be a nested key in `tool_input`; the fallback now counts the keys and denies when there is more than one (ambiguous fails closed, so the first-wins alternative was not taken). The header comment no longer claims the fallback reads fields "as in the binary". Nits: `set -f` around the `$hook_types` loop (a `*` value never globs), the row-13 fake binary echoes its stdin and the test asserts the payload arrived, and a hook-branch test covers a missing and a foreign stamp (`a_missing_or_foreign_stamp_is_stale_to_a_hook`). The new tests (`the_path_guard_fallback_denies_what_it_cannot_read`) failed before the shim change.

### T6 review fixes (2026-10-10)

The review of 2bc7a0c found a must-fix and four nits in `scripts/agent/qemu-lock.sh`. Must-fix: a command that ignores TERM survived the stop and the lock was released while it ran (`stop_tree` and `on_signal` sent TERM only). Fix: the harness-first TERM and the grace wait stay; then the whole tree (the pids seen first plus whatever it started since, QEMU included) gets TERM, a 3 s wait, SIGKILL, a 3 s wait. The lock is released only when nothing of the tree runs; otherwise it stays (the dead wrapper's pid makes `status` say `state=dead`), the survivors go to stderr and to a `survivors=` owner line, and the wrapper exits 78. Nits: (1) `clear-stale` moves the lock to `qemu.lock.clearing.<pid>` (atomic), then re-checks the owner pid, `state=dead` and that no QEMU runs, on the moved directory, before it logs and removes; on a failed check it moves the lock back when no new lock exists, else leaves it and says where. (2) Every option that takes a value, given without one, prints `<option> needs a value` and exits 2. (3) The load average is read and compared under `LC_ALL=C`. (4) Usage text: CMD's stdin is /dev/null (now explicit), and a SIGKILLed wrapper orphans CMD while `clear-stale` checks only for QEMU. A new test hook, `AIOS_QEMU_LOCK_KILL`, replaces the SIGKILL command so a survivor that cannot be killed can be simulated. Steps 2 and 3 produced the same output as before; Step 4 is new. Race case (c) is deterministic: a `pgrep` shim on PATH swaps the stale lock for a live holder's lock on its first call, which is the moment between `is_stale` and the move.

### T7 review fixes (2026-10-10)

The guard-attacking review of 92f8fcd found inputs that `git-push-guard.py` allowed against rules 1–4. Q8 (the `Bash(git reset --hard *)` allow) makes rule 4 the only thing confining an agent's reset, so every item was fixed, test first (91 tests before, 114 after; 21 tests failed before the guard change, and 2 more probes added during the work failed before their fix; the main-thread safety test passed throughout). (1) Rule 4 now models git's environment for an agent's `reset --hard` and its main-checkout commands: a repository-moving `GIT_*` variable (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_COMMON_DIR`, `GIT_INDEX_FILE` and the rest that `git_env_risky` flags) set inline, exported, through `env NAME=` or inherited by the session, `--git-dir`/`--work-tree`, or `-c`/`--config-env` `core.worktree`, `core.bare` or `include.*` denies; before, `git --git-dir=$M/.git commit` and `GIT_DIR=... git reset --hard` from the agent's own worktree were allowed. (2) Rule 1: `cargo run`/`cargo r` whose first program argument after `--` is `soak` (not `--classify`) is a QEMU start whatever names the package (`--bin aios`, `-p aios-tools`, `--manifest-path`, a cwd under `tools/`); no other package has a `soak` argument, so the target is not checked. (3) Rule 1: the wrapper is trusted only when its realpath is `scripts/agent/qemu-lock.sh` of the main checkout, the project or the agent's own temporary worktree (a symlink counts as its target); any other `qemu-lock.sh`, and any script called `run --team ...` (a renamed copy, which the guard cannot read when the same command creates it), gets the wrapper's caller rules and has its body read. A script without a `#!` line run directly is now read as shell, as the shell runs it. (4) Rule 3 reads the frontmatter the way YAML does: BOM, leading blank lines, `--- # comment`, an unquoted comment after a space and `#`, quoted keys and values, anchors and tags, and a top level that is indented as a whole; an `isolation` key that is not plainly `worktree` (an alias, a block, another spelling, two values) asks on a spawn without `isolation: "worktree"`. (5) Rule 2: agents are denied every `pkill`/`killall` and every kill of pids found by pattern (`kill $(pgrep …)`, backticks, `… | xargs kill`, `… | while read p; do kill $p`, a variable assigned from `$(pgrep …)`); a main thread is asked for those and for a pkill pattern that is computed, a regex or names `aios`/`soak`; a literal that is a piece of `qemu-system-aarch64` (three characters or more) is denied to everyone like `qemu`. (6) An agent's reset must be in `agent-<id>` where `<id>` is its `agent_id` (or `agent_id` is `agent-<id>`). (7) From an agent, in the main checkout: `rebase`, `pull`, `reset` in any mode, `update-ref`, `symbolic-ref` writes, `stash` (but `list`/`show`), `commit-tree`, `fast-import`, `branch -f/-D/-M/-C`, `checkout -B` and `switch -C` are denied; the main thread's asks are unchanged. (8) Rule 1 sees through `setsid`, `screen` (and `screen -X stuff`), `tmux` (`new`, `new-window`, `send-keys`, `-c` and the rest) and `env -S`/`--split-string`. (9) Agents are denied `rustup default <name>`, `override` (but `list`), `target add/remove`, `toolchain link/uninstall`, `self update/uninstall`, `set`, `run`, `rustup-init`, `<proxy> +<toolchain>` and `RUSTUP_*` set for a proxy; the main thread keeps its behaviour. False positives fixed: `qemu-system-aarch64 --version/-version/-h/--help` as the first argument, `command -v/-V`, `type`, and `just --show/-n/--dry-run/--evaluate/--list` and the other print-only options start nothing; `just --command CMD` is read as CMD (argv, no shell). A parse error asks when the text mentions `qemu`, `just`, `pkill`, `killall`, `rustup` (and `kill`, `soak`, `cargo`, `tmux`, `screen`, `setsid`), as it already did for git; an unresolvable command on a line that names one of the first five asks too, so the existing test whose script runs `"$QEMU"` now expects `ask` (a neutral copy keeps its original intent). Deviations from the review text: read-only forms stay allowed to agents in the main checkout (`symbolic-ref --short HEAD`, `stash list`, `rustup default` without a name, `rustup override list`), and `switch -C`/`branch -C` were added as equivalents. Not covered (code the guard does not read): a verifier that edits the wrapper in its own temporary worktree before running it, kills inside a program the guard does not parse, and pids passed through a file (`pgrep … > f; kill $(cat f)` in separate commands).

### T4 placement

On 2026-10-10, `git worktree remove` of an implementer's temporary worktree failed with "Directory not empty", because VS Code's rust-analyzer had opened it as a cargo workspace and was writing `target/flycheck0`. Git had already unregistered the worktree, so the controller removed the orphan directory, whose commits were already fast-forwarded. Fix: exclude `.claude/worktrees` from rust-analyzer (`rust-analyzer.files.exclude`) in the owner's editor settings, or tolerate the orphan by removing it after checking `git merge-base --is-ancestor <head> W`.

## Decisions Made

(T0 records the owner's answers to Q1 and Q3–Q7; rulings during execution follow)
- v5 (2026-10-09): the 22:00 and 22:04 owner decisions (Decisions, owner table) are applied in `D/PLAN.md.v5` and its `.v5` drafts (`D/REPLAN-v5.md`). Q1 is superseded by the 22:04 decision. Q8 (the `git reset --hard *` allow) is new: <answer, date>.
- T2 (2026-10-09): H3 FAILED on 2.1.292 (isolated agents cannot EnterWorktree into W). Owner: D3 becomes isolated + fast-forward (writers commit on their own `worktree-agent-*` branch from the branch tip; the lead `merge --ff-only`s W and removes the temp worktree). H7: SubagentStop blocks are discarded for subagents, and `model: fable` 404s in agent hooks. Owner: D8 becomes a lead-run Fable review before the fast-forward (max 3 rounds); agent hooks use `claude-fable-5-1`. Details: `D/T2-results-2026-10-09.md`.

- T0 (2026-10-09): owner answered Q1 yes, Q3 yes, Q4 yes, Q5 treat stale/dirty as missing, Q6 literal-path `git -C <W>` allows only, Q7 team-build ports soak-matrix.sh in its own PR first (issue #233). CLI 2.1.292; main cae3fff; installed aios stamp `source clean`; `aios hook --help` lists 4 programs.
- 2026-10-10 05:48: owner answered Q8 yes: add `Bash(git reset --hard *)` to the project allow list, confined by guard rule 4 (agents only in their own `agent-<id>` worktree; asks on a main thread).

## Lessons Learned

(to be filled during execution; T17 distils them)

### T6 qemu-lock.sh (2026-10-10)

No deviation from the v4 draft. `shellcheck -s sh` and `dash -n` exit 0. Step 3 printed `rc=143 qemu-running-at-harness-term`, `free`, `no-qemu-left`. The Step 2 smoke output matched the Acceptance list line by line; the lock log showed the cleared owner lines (`cleared_at=... by_pid=...`, `team=team-fix`, `pid=999999`). Docs gate: only the knowledge-hygiene plan entry is new.
