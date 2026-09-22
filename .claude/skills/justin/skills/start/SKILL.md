---
name: start
description: >
  AIOS session start: runs /justin:brief, then proposes exactly one next action
  from a fixed priority list and waits for the user. User-invoked only.
disable-model-invocation: true
---

# /justin:start

Start a session: brief, then one proposed next action. Runbook for the human side (stages, state locations, merge policy): `docs/project/agent-loop.md`.

1. **Brief.** Invoke the `justin:brief` skill with the Skill tool and follow it. If the Skill tool cannot load it, read `.claude/skills/justin/skills/brief/SKILL.md` and follow it directly.

2. **One next action.** Propose exactly one next action with a one-line reason, using the first rule that applies to the brief:
   1. Uncommitted or unpushed work on a `claude/*` branch: continue it, or checkpoint it with `/justin:pause`.
   2. Main CI is red: fix main before anything else.
   3. An open PR from a `claude/*` branch has failing checks or unresolved review threads (the brief counts both per PR): tend that PR.
   4. A PR shows `merge-ready: yes`: ask the human to review and merge it. Do not merge. `merge-ready` is computed by the script; never propose merging a draft, a PR gated by a `needs-human` issue, or a PR the script marks `no` for any other reason.
   5. A `needs-human` issue blocks the next step (a gate on a PR you would otherwise propose, or a decision the next phase-doc step depends on): ask for that decision and link the issue.
   6. New docs drift since the baseline: if this branch introduced it, fix it here. If it came from `main` (it shows as new on every branch, including a fresh one from `origin/main`), propose a dedicated docs PR that fixes or baselines it; do not fold it into unrelated work.
   7. Otherwise: start the next phase-doc step (`/implement-phase N`, attended). If the brief names an open PR for that milestone, the milestone is already in flight: propose continuing that PR instead, never a second implementation.

3. **Ask.** End by asking whether to proceed with that action, and add one line: `/justin:doctor` reports docs and harness drift; `/justin:pause` checkpoints before a break or `/clear`. Wait for the user.

## Rules

- Never merge a PR, push to `main`, force-push, delete branches, or edit `.claude/settings*.json`, `.claude/rules/`, or `.github/workflows/` from this skill. The human merges.
- Keep the reply short: the user reads it at the start of a session.
- Stage 0 has four session skills: `/justin:start`, `/justin:brief`, `/justin:doctor`, `/justin:pause`. Work, loop, retro and setup belong to later rollout stages (the staged rollout table in the runbook). If the user asks for one of them, say it is not available at Stage 0, name the four skills, and stop.
