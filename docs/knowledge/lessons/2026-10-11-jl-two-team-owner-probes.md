---
author: jl + claude
date: 2026-10-11
tags: [tooling]
status: in-progress
---

# Lesson: Two premises of the two-team harness wait for interactive owner probes

## What happened

The two-team harness (PR #234) rests on two Claude Code behaviours that no headless `claude -p` run can show, because each needs live interactive sessions. The harness's design folder lists them as owner probes: `OWNER-PROBES.md.v5` in `$(git rev-parse --git-common-dir)/aios-agent/teams-design-v3` (local to the owner's clone, not in the repository). OP1 is cross-session messaging between the two leads, and OP3 is a foreground subagent's permission prompt for a `.claude/` path. Neither had been run when PR #234 was distilled. OP2, the ExitPlanMode plan gate, passed and is recorded in [the model-routing wiring lesson](2026-10-06-jl-model-routing-wiring-checks.md) (E6).

## Why it happened

- A `-p` run has one session and no person to answer a prompt, so it cannot show a message between two leads or a prompt reaching the owner.
- OP1 can only run when both leads restart from the main checkout after the harness merges. OP3 was to be watched during PR #234's shim, guard and settings tasks, and no report of it was recorded.

## What we learned

### Cross-session messaging without the agent-teams flag (OP1)

The harness removed `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS` from the project env, because aios uses no Claude Code teammates (rule 11). The two leads still message each other across sessions. That should keep working, because cross-session messaging has its own gate. The evidence comes from reading the 2.1.285 and 2.1.289 binaries, not from a run. They carry a separate gate message, `[uds-messaging] Skipped: cross-session messaging gate off`, and they have held messages and the `crossSessionInbound` setting, none of which depends on agent teams. Running OP1 is a post-merge owner step in PR #234's hand-off.

With the project flag gone, OP1 must show:

- Each lead's ListAgents lists the other under its team name (`team-build`, `team-fix`).
- Each lead receives the other's `From <team>: HELLO`, and no "held" or `crossSessionInbound` notice appears.
- A lead resumed with `claude --resume <team>` keeps its name without `/rename`, and its peer receives its new `HELLO`.
- A background worker spawned by a lead runs its first command, `git reset --hard <tip>`, with no permission prompt.

If a message never arrives, run the receiving lead once with `--debug-file` and read the lines that contain `uds-messaging` or `Cross-session messaging`:

- **Gate off:** the messaging gate is off for this account or CLI version. This needs a design decision from the owner. Do not put the agent-teams flag back as a stopgap.
- **Message held:** the two leads are probably in different permission modes. Claude Code holds cross-session messages while the modes differ, which is why rule 11 and agent-loop.md (Teams, Launch) require both leads to use the same mode.

### Foreground `.claude/` edit prompts (OP3)

OP3 checks the premise of rule 11's "`.claude/**` edits run in the foreground": a foreground subagent's permission prompt for a `.claude/` path reaches the owner in the lead's window and names the file, and the subagent waits for the answer and reports a refusal without trying another route. Until it runs, that premise is unconfirmed. Verifier boots do not rest on it: they run in the background, and the settings allow their `qemu-lock.sh run --team team-build` or `--team team-fix` call (rule 11, QEMU and the host; agent-loop.md, Host QEMU lock). What PR #234 did show is that in auto mode the classifier refuses an agent's Write of `.claude/settings.json`, a `git rm` of hook files and the commit that carries them (#244), so those changes are owner steps (rule 11, Config).

## How to avoid next time

- Until OP1 passes, a lead that cannot see its peer in ListAgents works in one-lead mode (agent-loop.md, Teams) and never sends to a guessed name.
- Record each result as a dated update at the end of this lesson, not by rewriting the sections above: the CLI version, `result: pass` or `fail`, what each item showed, and the debug lines for anything that failed. A failure also gets an issue labelled `team-build`. When both probes have a passing result, set `status: final`.
- Re-run both probes after a Claude Code update that changes messaging or permission prompts, with the headless checks in agent-loop.md, "Re-check after each Claude Code update".
