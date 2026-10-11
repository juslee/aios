---
author: jl + claude
date: 2026-10-10
tags: [tooling]
status: final
---

# ADR: Two-team agent harness

## Context

PR #234 (`claude/harness-teams`) replaced the single `team-lead` agent and the `build-team` skill with two attended lead sessions, `team-build` and `team-fix`. Each lead spawns subagents from the roster in `.claude/agents/`. How the harness works is in [rule 11](../../../.claude/rules/11-teams.md) and [agent-loop.md](../../project/agent-loop.md) (Teams). Model routing and its hooks are in [the model-routing ADR](2026-10-06-jl-model-routing-hooks.md). Writer placement is in [the isolated-writers lesson](../lessons/2026-10-06-jl-isolated-writers-fast-forward.md). This ADR records the reasons for three decisions that are not written down anywhere else: how a session is identified, why agent memory and the remember plugin were retired, and why the guard analyses what the QEMU lock wrapper runs, with which `run` calls the settings allow.

## Decisions

### A session's identity is an environment variable

**Decision.** A lead is launched from the main checkout as `AIOS_TEAM=<team> claude -n <team> --model opus`, where `<team>` is `team-build` or `team-fix`. `AIOS_TEAM` keys the lead's handoff file, the QEMU lock's `--team` and the guard's team rules. Every other session is solo and sets `AIOS_SESSION=<name>`, which keys its handoff file `.remember/handoff-solo-<name>.md`. The `-n` display name is only a label. `/justin:team` stops unless `AIOS_TEAM` equals the team and ListAgents lists the session under that name.

**Why.**

- **Display names do not survive a relaunch.** Sessions choose their own names, and a fresh launch resets them. On 2026-10-06 the two leads came back as `aios-83` and `aios-39`. State keyed on a display name would point at the wrong handoff file or lock owner after a restart.
- **Hooks and shells cannot read the name.** On 2026-10-06, `claude -n <name>` did not set `CLAUDE_CODE_SESSION_NAME`. A variable set on the launch line is the only identity that both the guard and the skills' shell commands can read.

**How to apply.** Key new per-team or per-session state on `AIOS_TEAM` or `AIOS_SESSION`. Never key it on the display name. Before relying on a display name in a later Claude Code version, check the installed binary ([the verifying-harness-claims lesson](../lessons/2026-10-06-jl-verifying-harness-claims.md), *Settings schema questions*).

### Agent memory and the remember plugin are retired

**Decision.** No agent definition in `.claude/agents/` sets `memory:`. The project no longer enables the remember plugin (`remember@claude-plugins-official`), and `precompact-save.sh` and its PreCompact hook are deleted. Each session's only repo-side continuity file is its handoff, `.remember/handoff-<key>.md`, which `/justin:pause` writes itself (rule 11). The notes the agents had kept in memory became lessons under `docs/knowledge/lessons/`. `/merge-and-cleanup` no longer copies `.claude/agent-memory/` out of a PR's worktree. One-time cleanup after the merge, listed in PR #234's hand-off: the owner removes the plugin from the user settings; the shared `.remember/remember.md` is split into the two leads' handoff files; its `logs/` move to `<git-common-dir>/aios-agent/remember-retired/logs/`; and every `.claude/agent-memory/` store is deleted.

**Why.**

- **`memory: project` splits an agent's notes across worktrees.** The notes were kept under `.claude/agent-memory/` in the checkout the agent ran in, which the old `.gitignore` ignored. PR #234 dropped that ignore line, so a store left in a checkout shows as untracked after the merge; the one-time cleanup above deletes every store. Writers now run in their own temporary worktrees, and every branch has its own worktree. Each run therefore saw only the notes written in that checkout, and the notes were lost with the worktree unless something copied them out first. `/merge-and-cleanup` carried a copy step for that reason.
- **Memory gives read-only agents write access.** When an agent sets `memory:`, Claude Code lets it edit files under `.claude/agent-memory/`. code-reviewer, doc-auditor, skeptic and verifier are meant to be read-only (`disallowedTools: Write, Edit, NotebookEdit`). A `memory:` key would undo that.
- **The remember plugin duplicated `/justin:pause`.** Once pause writes one handoff per session, the plugin's shared `remember.md` and its PreCompact save did the same job a second way. The owner retired the plugin on 2026-10-05 under the standing no-legacy rule (rule 01, No Legacy).

**How to apply.**

- Do not add `memory:` to an agent definition. A durable finding from an agent goes into `docs/knowledge/` (a lesson or an ADR, rule 08) through a writer's commit, where every worktree and session can read it.
- Keep the read-only agents without write tools. A reviewer that needs to record something puts it in its report, and the lead or a writer commits it.
- Carry session continuity only in the handoff file and the personal auto-memory (agent-loop.md, "Where state lives"). Do not add a plugin or hook that saves session state alongside `/justin:pause`.

### The guard analyses what the QEMU lock wrapper runs, and only a verifier's per-team `run` is allowed

**Decision (owner, 2026-10-10).** Guard rule 1 analyses the command after `qemu-lock.sh run`'s `--` as if it were typed on its own at the call's cwd, so every rule applies to it: push and merge, rules 1–4, kills, toolchain, placement. Only the QEMU start it wraps, which is the wrapper's job, gets no rule 1 decision of its own. Where the guard cannot tell what the wrapper will run, it asks. That covers an option word or subcommand from an expansion that may split, and a wrapped command it cannot parse. Apart from that rule 1 exemption, no wrapped command gets a weaker decision than it gets unwrapped. The owner's commit a189f7a (2026-10-11) narrowed the project allow from `Bash(bash scripts/agent/qemu-lock.sh *)` to the wrapper's `status` and `clear-stale`, so every `run` call went to the permission layer: a prompt, or the classifier when the session runs in auto mode. The decision of 2026-10-11 below then allowed `run` again in two per-team forms only.

**Why.** PR #234's security review found a bypass. Because the guard trusted the genuine wrapper without analysing its command, `qemu-lock.sh run ... -- git push origin HEAD:main`, `gh pr merge`, pattern kills, toolchain changes and placement changes passed both the guard and the `Bash(bash scripts/agent/qemu-lock.sh *)` allow. The wrapper's job is the lock, not the command, because the caller picks the command. The guard is defence in depth, not a boundary, so the narrowed allow gave every boot a second check outside the guard (the verifier's two per-team spellings lost it with the 2026-10-11 decision below). That check is a person only outside auto mode: in auto mode the classifier may approve a `run` with nobody asked. An ask from the guard or a permission rule prompts even in auto mode (rule 03). [The wrapped-command lesson](../lessons/2026-10-11-jl-guard-wrapped-commands.md) covers how the analysis is built and the attack rounds it took.

**How to apply.**

- Do not widen the allow beyond the two per-team `run` forms (never back to `qemu-lock.sh *` or to `run *`). Each allow for `run` is an owner decision, made knowing that the guard is then the only check on the wrapped command.
- An ask rule such as `Bash(bash scripts/agent/qemu-lock.sh run *)` would put a person in front of every boot in auto mode too, but it outranks the per-team allows, so every background verifier boot would be refused again. Adding it means going back to foreground boots (option a below). Either way it is an owner settings step (rule 11, Config; #244), not one an agent makes.
- A refused `run` is reported, never retried or reworded (rule 03, `verifier.md`). A refusal can come from the owner, from the classifier, or from a prompt nobody can answer because the agent or session runs in the background or with `-p`. That covers a verifier's call spelled any other way than the per-team allow (`--team solo`, another path, `sh`, a `cd`, `env` or `nohup` prefix), a guard ask on its wrapped command, and a solo main thread's `run --team solo`.
- A new wrapper that runs a caller's command gets the same treatment: the guard analyses its command, and the settings allow only its subcommands that run nothing, plus any narrow form the owner decides on.

**Decision (owner, 2026-10-11): verifier boots use a narrow per-team allow** (#244, comment 6106607425). The project settings allow `Bash(bash scripts/agent/qemu-lock.sh run --team team-build *)` and `Bash(bash scripts/agent/qemu-lock.sh run --team team-fix *)` beside `status` and `clear-stale`; the owner's commit in PR #234 adds them. Verifiers stay background subagents and spell the call exactly `bash scripts/agent/qemu-lock.sh run --team <team-build|team-fix> ...` from their worktree root. In auto mode an allow rule approves without the classifier, so the guard is the check on the wrapped command: rule 1 analyses the command after `--` under every rule, and its ask or deny still wins over the allow. The last attack round on that analysis approved it (18948 evaluations, 0 lost reasons); the gaps outside its model are on #241. A solo main thread's `run --team solo` and any other spelling still prompt.

**Why option b (per-team allow) over option a (no allow, foreground verifier boots).** A background verifier cannot answer a prompt, so with no allow every boot it tried was refused. Spawning boots in the foreground instead would block a lead for a whole soak: a six-boot gate soak or a quiet A/B window holds the lead's turn the whole time, and the lead could do no placement, review or messaging meanwhile. The narrow allow keeps the boot in the background and names only the two team identities; it gives up the second check outside the guard for those two spellings, leaving the guard's wrapped-command analysis, attacked until a round lost no reason, as their only check.

## Consequences

- The options these decisions rejected stay rejected: keying state on the display name, keeping `memory: project` on the agents, trusting the QEMU lock wrapper by its path, and foreground verifier boots.
- An agent's finding reaches other worktrees and sessions only through a commit to `docs/knowledge/`.
- Only the two per-team allows approve a `qemu-lock.sh run` call without a prompt, so a verifier boots in the background and the lead keeps working while it runs. Every other `run` call prompts the owner, or in auto mode the classifier decides it, and is refused in a background or `-p` run.
- The retirement leaves one-time cleanup for the owner after the merge (Decision above).
- Two owner probes of the harness's premises still have to run after the merge (OP1, OP3). They are in [the owner-probes lesson](../lessons/2026-10-11-jl-two-team-owner-probes.md).
