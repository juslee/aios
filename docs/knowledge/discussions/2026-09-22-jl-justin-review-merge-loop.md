---
author: jl + claude
date: 2026-09-22
tags: [tooling, agent-loop, security, ci]
status: active
---

# Discussion: /justin:review and /justin:merge — the self-improving PR loop

## Context

Stage 0 of the agent loop is merged. It added:

- **Guard rails (#162).** Narrowed permissions, a push guard, and GitHub ruleset `23812283`, which requires a PR and 5 CI checks with no bypass.
- **Session skills (#166).** The `justin` plugin with `/justin:start`, `/justin:brief`, `/justin:doctor` and `/justin:pause`, plus `just docs-check`.
- **The QEMU soak harness (#168).**

PRs are still tended and merged by hand with `/review-pr-comments` and `/merge-and-cleanup`.

This design replaces both skills with a loop. The loop reviews a PR, fixes what the gates report, re-reviews until nothing is left, and lets GitHub merge it automatically. It also learns from its findings so that problems get caught earlier over time.

The owner's decisions, all made on 2026-09-22:

| Topic | Decision |
| --- | --- |
| Merge mechanism | GitHub auto-merge |
| Review gate | Required commit status `justin/review` in the main ruleset |
| Fixing | The loop fixes anything a gate flags, automatically |
| Where it runs | The owner's Mac: on demand first (`/justin:merge [PR…]`), then always-on through a launchd runner (stage 2, separate spec) |
| Convergence | Zero confirmed findings of any severity; at most 5 rounds, then needs-human |
| Auto-merge scope | Everything, including `.claude/**`, `.github/**` and hooks |
| Implementation | Approach C: a deterministic Python orchestrator that calls `claude -p` per stage |
| Models | Sonnet 5 by default, Opus 5 for the verify gate and for kernel-core review lenses, Fable 5.1 as the escalation tier |
| Branch policy | "Require branches to be up to date" on |
| Main red | Freeze, fix, and auto-revert if the fix fails |

## Design

### 1. Components

**`scripts/agent/pr_loop.py`** is a python3 standard-library orchestrator.

- It must run on `/usr/bin/python3` 3.9: no `tomllib` and no `match` statements.
- **CLI:**
  - `pr_loop.py review <PR…|--all>`
  - `pr_loop.py merge <PR…|--all>`
  - `pr_loop.py sweep`
  - options `--max-rounds 5`, `--dry-run`, `--budget-usd N`, `--jobs 1`
- **Per-PR state machine:** `sync → gather → review → verify → (converged | fix → push → wait-ci) → next round`. It ends in `converged` or `needs-human`.
- **Where facts and judgement come from:** facts come from `gh … --json` and `git`. Judgement and edits come only from `claude -p` stages.
- **State** lives in `$(git rev-parse --git-common-dir)/aios-agent/`, which is shared by every worktree, never committed, and survives `just clean`:
  - `pr-<n>/state.json`
  - `pr-<n>/round-NN-*.json`
  - `findings.jsonl`
  - `resume-at`
  - `STOP`
- **Resume:** it resumes from `state.json` after an interruption.
- **Where it runs from:** always from the main checkout on `main`, never from a PR branch. A PR that changes `pr_loop.py` is therefore reviewed by the already-merged loop before its code ever runs.

**`scripts/agent/prompts/`** holds one versioned prompt template per stage:

- `review-correctness.md`
- `review-security.md`
- `review-conventions.md`
- `verify.md`
- `tiebreak.md`
- `fix.md`
- `sync.md` (conflict resolution)

Each prompt carries a `version:` line, and every finding records the version that produced it.

**`scripts/agent/loop-config.json`** holds the model per stage, the escalation rules, the per-call and per-PR budgets, the kernel-core path globs, and the list of trusted comment authors.

**Skills in the `justin` plugin:**

- **`/justin:review [PR…]`** runs `pr_loop.py review`. It sets `justin/review` and never arms auto-merge.
- **`/justin:merge [PR…]`** runs `pr_loop.py merge`. It arms auto-merge, runs the loop, and sweeps worktrees of merged PRs.
- **`/justin:retro`** runs `scripts/agent/retro.py` (section 4).
- **Invocation:** all three can be invoked by the model, so `/justin:start` routes to them and the launchd runner (stage 2) can call them. The routing shape itself is decided in the skills-rename PR.
- **What they replace:** `review-pr-comments` and `merge-and-cleanup` are deleted in the same PR. The references to them in `CLAUDE.md`, rules 03/04, and the `implement-phase`, `generate-phase-doc` and `write-arch-doc` hand-off steps are updated, and `just docs-check` must report 0 new findings.

**Settings for each `claude -p` stage:**

- The stage runs in the PR's worktree, with main's `.claude/settings.json` passed via `--settings` and `--setting-sources user`.
- That includes the plugin disables from #170.
- As a result, a PR that edits `.claude/settings.json` cannot loosen the permissions of the agent working on it.

### 2. The loop

**Sync, before each round:**

- If `mergeStateStatus` is `BEHIND` or `DIRTY`, merge `origin/main` into the branch. Never rebase, so no force-push is needed.
- Conflicts go to a `sync` stage.
- If the PR's own diff (`git patch-id` against the new merge-base) is unchanged, the last `justin/review` verdict carries over to the new head without a review round. CI still re-runs.

**Gather (deterministic):**

- the diff against the merge-base, and the changed files;
- failing CI jobs, with the tail of each log;
- `just docs-check --json`;
- unresolved review threads;
- needs-human gate issues that reference the PR;
- relevant knowledge-hive notes (section 4, "Knowledge hive").

Comments are handled by author:

- **Trusted authors** (the owner, `claude[bot]`, Copilot) become findings.
- **All other authors'** text is passed as quoted data under an "untrusted" header and never acted on.

**Review:** three parallel `claude -p` read-only runs, one per lens.

- **Tools:** `Read`, `Grep`, `Glob`, and `Bash(git diff:*)`/`Bash(git log:*)`. No edit tools.
- **Inputs:** the gathered facts plus the prior rounds' findings and their dispositions, so dismissed items are not re-raised.
- **Output:** JSON findings `{id, lens, severity, category, file, line, claim, evidence, suggested_fix}`.

**Verify:** one `claude -p` run that tries to refute each finding against the code. A finding survives only if it is:

- concrete;
- reproducible from the code; and
- in scope: it concerns lines the PR adds or changes, or behaviour those lines directly affect.

Out-of-scope pre-existing issues are filed as GitHub issues (deduplicated) and do not block. Deterministic failures always count as confirmed findings: red CI, new docs-check drift, and trusted unresolved threads.

**Converged when:**

- zero confirmed findings;
- all required CI checks green;
- docs-check reports 0 new findings;
- no unresolved trusted threads; and
- a clean merge state.

The loop then posts `justin/review=success` on the head SHA.

**Fix:** one `claude -p` run in the worktree.

- **Allowed:** edits and builds.
- **Not allowed:** `git push` and GitHub writes.
- **Requirements:**
  - fix every confirmed finding;
  - pass `just check` and `just test`, plus touched-area gates (docs-check, shellcheck, the guard unit tests);
  - make one commit, `Review round N: <summary>`, with the Co-Authored-By trailer.
- **Output:** a reply for each addressed comment. It may decline a finding only with a stated reason.
- **Disputes:** the next verify re-judges a declined finding. If it is still confirmed, it goes to the Fable tie-break. If Fable upholds it, the PR goes to needs-human.

**Push and wait:** `pr_loop.py` pushes with `git push origin HEAD:<pr-head-branch>`, posts the replies, resolves the addressed threads, and waits on `gh pr checks --watch` with a timeout.

**Budget:** each call runs with `--max-turns` and `--max-budget-usd`, and each PR has a total cap (default $15). Exceeding the cap sends the PR to needs-human.

**After 5 rounds without converging:** the PR gets the `needs-human` label and `justin/review=failure`, and a comment lists the remaining findings.

**Models** (from `loop-config.json`):

| Stage | Default | Escalation |
| --- | --- | --- |
| Review, 3 lenses | Sonnet 5; Opus 5 for the correctness and security lenses when the diff touches kernel core (`kernel/src/{arch,sched,ipc,mm}/**` or any `unsafe`) | — |
| Verify | Opus 5 | Fable 5.1 for PRs labelled `hard` and in the final round |
| Fix | Sonnet 5; Opus 5 from round 3 | Fable 5.1 in round 5 |
| Tie-break | Fable 5.1 | — |

If a Fable call refuses or errors, the stage retries once on Opus 5 and records the fallback.

**Concurrency:** one PR at a time by default (`--jobs 1`). The three review lenses within a round run in parallel.

### 3. Merge mechanics

**Server-side changes.** These are applied only in rollout step 3, after a review-only trial, and each with the owner's confirmation at that time:

- Add required status `justin/review` to ruleset `23812283`, with no `integration_id`.
- Set `strict_required_status_checks_policy: true`.
- Repo settings: `allow_auto_merge=true` and `delete_branch_on_merge=true`.

**Status lifecycle.** The status is posted with `gh api repos/juslee/aios/statuses/<sha>`:

- `pending` when a run starts;
- `success` on convergence;
- `failure` after 5 rounds or on needs-human.

A status is per-SHA, so any new push, by anyone, leaves the new head unverified until the loop runs again. The patch-id carry-over from section 2 is the only exception. The status is a process gate, not a security boundary: any account with write access can post it. The security boundaries are the ruleset and the CI checks.

**Arming auto-merge.** `/justin:merge` runs `gh pr merge <n> --auto --squash` at the start. This is safe because GitHub cannot merge before the 5 CI checks and `justin/review` pass. Draft PRs are skipped unless named explicitly; a named draft is marked ready on convergence.

**All GitHub writes are subprocess calls in `pr_loop.py`, never model actions:**

- push only to the PR's own head branch, never `main`, never `--force`;
- merge only with `--auto --squash`, never immediate, never `--admin`;
- post statuses and replies, and resolve threads.

The model-driven stages cannot push or merge. One settings allow rule is needed: `Bash(python3 scripts/agent/pr_loop.py *)`. The main session adds it with the owner's approval, because agents cannot write `.claude/settings.json`. A direct `gh pr merge` from the model keeps its ask rule.

**Sweep.** It runs at the start of `/justin:merge` and of `/justin:start`. For each worktree whose PR `gh pr view` reports as `MERGED`:

1. Copy `target/soak/` and `target/agent/` to `<main>/target/archive/<branch>/`.
2. Run `git worktree remove`, without `--force`. A dirty worktree is listed for the owner and left in place.
3. Run `git branch -D` on the merged branch.

Step 1 exists because `gh pr merge --delete-branch` removes worktrees together with their ignored files. That is how the soak baseline logs were lost on 2026-09-22.

### 4. Self-improvement

**Findings log.** Every finding is appended to `aios-agent/findings.jsonl`. Each record holds:

- PR, round, lens, severity, category, file, stage that caught it, prompt version;
- disposition: `fixed`, `dismissed-by-verify`, `disputed`, `tiebreak-upheld` or `escaped`.

A category comes from a small fixed taxonomy in `loop-config.json`: `unsafe-doc`, `lock-order`, `concurrency`, `error-handling`, `shell-quoting`, `doc-drift`, `test-gap`, `convention` and `other`.

Per-PR metrics are recorded too: rounds to converge, cost, verify dismissal rate, and the needs-human reason.

An **escape** is a problem found after a PR merged:

- main CI red;
- a soak regression against the baseline; or
- a later PR fixing something an earlier auto-merged PR introduced.

It is recorded against the original PR.

**`/justin:retro`.** It runs after every 5 merged PRs, or on demand. It proposes at most 3 changes, in this order of preference:

1. **A deterministic check**, for any category seen in 3 or more findings across 2 or more PRs, and for every escape. Candidates: a docs-check rule, a test, a clippy lint, a shellcheck rule, or a push-guard case.
2. **An authoring rule**, in `.claude/rules/*` or the `implement` prompt.
3. **A reviewer checklist line**, in `scripts/agent/prompts/review-*.md`.

For categories with a high verify dismissal rate, it adds "do not flag" guidance to the reviewer prompts.

**Retro output:** one PR, which goes through the same loop and auto-merges, plus one note in `docs/knowledge/lessons/`. The note carries:

- subsystem tags from the rule 08 tag list (e.g. `ipc`, `sched`, `memory`);
- `[[wiki-links]]` to the related decision records, to earlier lessons, and to the discussion that spawned it;
- a link to the PR.

With these, Obsidian's graph shows how findings, lessons and fixes relate over time.

**Knowledge hive: the loop reads what it writes.** Before each round, `pr_loop.py`:

1. Maps the changed paths to subsystem tags, using a path-glob → tag table in `loop-config.json` (e.g. `kernel/src/ipc/**` → `ipc`).
2. Selects the notes in `docs/knowledge/lessons/` and `docs/knowledge/decisions/` whose frontmatter `tags` intersect those tags. It prefers `status: final`, puts the newest first, and applies a size cap from `loop-config.json` (default 12 notes / 40 KB).
3. Adds the selected notes to the review and fix prompts under a "Known lessons for this area" header.

A lesson learned on one PR therefore becomes review context for the next PR that touches the same subsystem. That closes the self-improvement cycle. When a note has to be dropped because of the cap, the round's state file says so.

Headless stages read the vault as plain files from the main checkout. They do not use the obsidian MCP server, which runs `npx … mcpvault@latest` and would:

- fetch from npm on every `claude -p` call;
- execute an unpinned package; and
- need MCP approval in headless mode.

The deterministic tag match is faster and reproducible. The MCP server remains the interactive way to browse and search the merged vault. It serves the main checkout's `docs/`, so work on a branch edits files in the worktree instead.

**Guard against self-weakening:** a retro PR that removes or loosens a check must cite evidence in its body. The verify stage treats "weakens a safety check without evidence" as a finding.

**Measurement.** Each retro compares, before and after its previous changes:

- round-1 findings per PR (the target is a falling trend);
- rounds to converge;
- the false-positive rate;
- escapes (the target is 0).

`/justin:brief` prints these on one line.

**Evals: self-improvement must be measured, not assumed.** Two suites gate every change to the loop's own judgement.

**1. Loop-stage evals.** `pr_loop.py eval` runs the review and verify stages against a fixture corpus in `scripts/agent/evals/`. It uses the same prompts and the same `loop-config.json` models as a real run. There are two kinds of case:

- **Seeded-bug cases:** a real diff with one known defect, labelled with the expected file and category. The seed corpus comes from real findings of the 2026-09-22 review rounds, taken at their pre-fix commits:
  - the unquoted scratch-path `rm` in `soak-qemu.sh`;
  - the git option-abbreviation bypass in the settings proposal;
  - the pause secret scan with no override;
  - others from the same rounds.
- **Clean cases:** merged PR diffs that converged and never escaped. They expect zero confirmed findings.

It reports:

- **recall:** the share of seeded bugs confirmed after verify;
- **false-positive rate:** confirmed findings on clean cases;
- **cost per case.**

**2. Skill evals.** `claude plugin eval justin`, with cases in `.claude/skills/justin/evals/`, for example: "given this brief state, `/justin:start` proposes the right next action". It uses the built-in no-plugin baseline arm and `--max-cost-usd`.

**The gate.** A PR that changes any of the following must include fresh scores in `scripts/agent/evals/scores.json`:

- `scripts/agent/prompts/**`;
- the model or escalation settings in `loop-config.json`;
- `.claude/skills/justin/**`.

A drop in recall, or a rise in the false-positive rate, beyond one case against main's recorded scores is a confirmed finding in the review loop. So a retro change to a prompt merges only if it measurably does not make the loop worse.

**The corpus grows itself.** Every escape becomes a new seeded-bug case: the real bug, taken before its fix. Every category that verify often dismisses adds a clean case. Retro PRs add eval cases together with the checks they add.

**Cost.** The suites run only when those files change, plus once a week, under `--max-cost-usd`.

### 5. Safety, failure handling, rollout

**Main red.** This means CI failing on `main`, or a post-merge soak clearly below its baseline. The loop:

1. Freezes: it arms nothing new, and runs `gh pr merge --disable-auto` on armed PRs.
2. Records the last-green `main` SHA.
3. Opens or updates a pinned "main is red" issue.
4. Opens a `claude/fix-main-<sha>` PR through the loop.
5. If that PR fails 5 rounds, opens a revert PR of the offending squash commit, which auto-merges through the loop.
6. Unfreezes when `main` is green again.

**Failure table:**

| Event | Behaviour |
| --- | --- |
| Stage error, timeout or empty output | Never counted as success. Retry once, then leave the PR `pending` |
| Usage limit | Stop, save state, write `resume-at`. `/justin:start` reports it; the stage-2 runner resumes |
| Permission denial in a stage | needs-human, with the denied command. Never retried with broader permissions |
| Budget cap, or a conflict the `sync` stage cannot resolve | needs-human |
| CI failure that looks like infrastructure | `gh run rerun --failed` once before counting it as a finding |

**Untrusted code and text:**

- **PRs from forks:** never built, fixed or merged by the loop, because building runs `build.rs` and proc macros. They go to needs-human.
- **Non-trusted comment authors:** treated as data only.
- **Renovate PRs:** they are built locally, so the hygiene PR adds Renovate `minimumReleaseAge` to delay newly published crate versions.

**Kill switch:** an open issue labelled `agent-stop` (which can be added from a phone), or the file `aios-agent/STOP`. Either one makes `pr_loop.py` arm nothing, push nothing, and exit.

**Rollout:**

1. **Build.** This spec, then a plan in `docs/knowledge/plans/`, then one PR on `claude/justin-review-merge-loop`:
   - `pr_loop.py`, `retro.py`, the prompts, the config and the three skills;
   - unit tests for the state machine with fake `gh`, `git` and `claude` executables;
   - `--dry-run`;
   - the eval suites with a seed corpus of about 10 seeded-bug and 5 clean cases, and the first recorded `scores.json`;
   - a rewrite of rule 03 "Main Is User-Only". Agents still never push to `main` and never run `gh pr merge` themselves; the only merge path becomes GitHub auto-merge armed by `pr_loop.py` behind the required checks.
   It merges with the existing `/merge-and-cleanup`, then deletes that skill and `review-pr-comments`.
2. **Review-only trial.** `/justin:review` on 2–3 real PRs, such as #170 and the skills-rename PR. Compare its findings with a manual read.
3. **Switch GitHub.** Add the required `justin/review` status, the strict up-to-date policy, and the auto-merge repo settings. In this order only: requiring the status before the loop can post it would block every PR.
4. **Stage 2.** The launchd runner, in a separate spec: a schedule, `resume-at`, the budget, and `claude -p "/justin:merge --all"`.
5. **First `/justin:retro`** after 5 merged PRs.

**Owner prerequisite:** extra usage switched off or capped in claude.ai, through `/usage-credits` or <https://claude.ai/settings/usage>. This is what makes unattended runs incur no overage.

## Open Questions

- Does `claude -p` accept `--json-schema` for structured stage output on 2.1.278, or should stages print a fenced JSON block for `pr_loop.py` to parse? To be decided during the plan by a one-call check.
- The exact GitHub status call and ruleset update payload. Verify with a read-only `gh api` dry-run before rollout step 3.
- Where `pr_loop.py` should live. Rule 03 requires unattended pushers to live under `.claude/` and be trusted by the push guard. Moving the orchestrator to `.claude/agent/` would also protect its code from edits by the loop's own fix stage, because agent writes to `.claude/**` go to the owner, while leaving the prompts in `scripts/agent/prompts/` free for retro edits. To be decided in the plan.
- Whether `/justin:start` should run `/justin:merge --all` itself or only propose it. This is decided in the skills-rename PR, which also moves the other 7 skills into the plugin.

## References

- `docs/project/agent-loop.md`: the Stage 0 runbook, to be updated by the implementation PR.
- `.claude/rules/03-git-workflow.md`: the Guard Rails section.
- `.claude/hooks/git-push-guard.py`: the push guard.
- `scripts/soak-qemu.sh` and `just soak`: the soak harness.
- Issues #163, #164, #165, #167: needs-human gates.
- Ruleset `23812283` on `juslee/aios`.

## Outcome

_Fill in when graduated or archived:_

- Graduated to: `docs/project/agent-loop.md` (loop runbook section) when the implementation PR merges.
