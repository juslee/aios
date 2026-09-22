# Agent Loop Runbook

**Audience:** the owner, returning after a break
**Current stage:** Stage 0 (no autonomy: Claude works only inside a session you are attending)
**Last updated:** 2026-09-22

-----

## What `/start` does today

`/start` is a user-invoked skill ([SKILL.md](../../.claude/skills/start/SKILL.md)). Claude never triggers it on its own.

| Command | What happens | Writes |
|---|---|---|
| `/start` or `/start brief` | Runs [brief.sh](../../scripts/agent/brief.sh) (git, gh, jq, python3; no LLM), then summarises it and proposes one next action | a timestamp in `$(git rev-parse --git-common-dir)/aios-agent/last-brief`; `git fetch --prune origin` updates remote refs |
| `/start doctor` | Runs `just docs-check --all` plus the pointer-doctor and harness-tables checks, then groups the problems by who fixes them | nothing |
| `/start pause` | Saves the `.remember` handoff, then runs [checkpoint.sh](../../scripts/agent/checkpoint.sh): a `wip:` commit on the current `claude/*` branch and a push of anything not on origin (never `main`, never forced), then a checkpoint line | `.remember/`, at most one `wip:` commit |

The brief covers: branch and worktree state (dirty, unpushed), open PRs with check status and a `merge-ready` verdict, main CI, the boot soak, the `.remember` handoff, knowledge notes changed since the last session, open `needs-human` issues, the next unchecked phase-doc step, and a one-line docs-check result.

- **merge-ready** is `yes` only for a non-draft PR with at least one check and every check passed, GitHub mergeable with merge state `CLEAN`, no changes requested, no unresolved review threads, and no open `needs-human` issue naming it. When the gates or the review threads cannot be read, it is `no`. `/start` proposes merging only such PRs.
- **claude-review** passing means the review ran, not that it found nothing: the job has `pull-requests: read`, so it cannot post comments. Read its log.
- **Next phase-doc step** also names an open PR whose title carries that milestone (`Phase N MK:`), so work already in flight is not started twice.
- **Soak** shows two lines. The main soak is the newest finished run of a commit on `origin/main` with no uncommitted changes. The newest other soak (a branch commit, a dirty tree, or a run still in progress without `summary.md`) is labelled as not main's state.

**Pause safeguards.** checkpoint.sh commits nothing while a merge, cherry-pick, revert, rebase or bisect is in progress. It stops before committing, and restores the index, when a path new to the repo looks like a secret or an added line looks like a private key or token; that scan also covers local commits not yet on origin. It keeps `wip:` commits local while the branch has an open PR that is ready for review, because each push to such a PR runs CI and the Claude review on unfinished work; mark the PR draft if you want wip pushes.

`work`, `loop`, `retro` and `setup` modes belong to later stages and do not exist yet.

-----

## Pause and resume

**Context limit** (the session is getting long or slow):

1. `/start pause`
2. `/clear`
3. `/start`

**Usage limit.** Policy (decided 2026-09-22): work resumes automatically after the usage limit resets, never on paid overage, so extra usage stays off or capped in the account settings. Stage 0 has no unattended runner, so today a usage limit simply ends the session: run `/start pause` if the session still responds, then `/start` after the reset. The automatic resume arrives with the local runner in a later stage.

**Stepping away** for any other reason: `/start pause`.

-----

## Where state lives

| State | Location | Who can see it |
|---|---|---|
| Decisions waiting for you | GitHub issues labelled `needs-human` | anyone, including GitHub mobile |
| Work queue (Stage 1+) | GitHub issues labelled `agent-ready` / `agent-working`; one issue = one milestone = one PR | anyone |
| Work in flight | PRs from `claude/*` branches; worktrees under `.claude/worktrees/<name>` | PRs: anyone; worktrees: this Mac |
| Working plans | `docs/knowledge/plans/` on the PR branch; distilled into lessons/decisions and deleted before the PR is ready | git |
| Lessons and decisions | `docs/knowledge/lessons/`, `docs/knowledge/decisions/` | git |
| Session handoff | `.remember/remember.md` and `.remember/now.md` in the main checkout (gitignored in every checkout; pause never commits `.remember/`) | this Mac |
| Personal auto-memory | `~/.claude/projects/<repo>/memory/MEMORY.md` | this Mac |
| Accepted docs drift | [scripts/docs/baseline.json](../../scripts/docs/baseline.json) | git |
| Boot soak results | `target/soak/<run>/summary.tsv` (one row per boot, written as it runs) and `summary.md` (with the commit, written when the run finishes) in whichever worktree ran the soak harness | this Mac |

GitHub labels: `needs-human` (waiting for an owner decision; agents do not claim it or change the files it names), `agent-ready` (owner-approved and claimable; only the owner applies it), `agent-working` (claimed by an agent session), `agent` (opened by an agent, so agent work can be counted).

**Holding a PR.** Open a `needs-human` issue whose title names the PR, for example `Gate: merge PR #149 ... only after ...`. The brief shows that PR as `merge-ready: no (gated by #N)` until the issue is closed. A PR named only in the body of a `needs-human` issue is also held back (`named in needs-human #N`).

-----

## Merge policy

You merge. Claude pushes `claude/*` branches, opens PRs, and stops. Merge after review with `/merge-and-cleanup <PR>` in a session you are attending, or on GitHub.

Later (not enabled): GitHub auto-merge behind required status checks on a `main` ruleset, starting with milestones after the boot-crash fix.

-----

## Docs policy

| Doc class | Who changes it | When |
|---|---|---|
| Status docs: README status, phase-doc checkboxes and Status, development-plan §8.1, CLAUDE.md fact tables, doc-map | agent | in the same PR as the change |
| Architecture docs | owner approval only | separate, owner-approved PR |
| CLAUDE.md policy prose, `.claude/rules/`, skills, agents | retro or harness PR | the human merges |

`just docs-check` compares [check.py](../../scripts/docs/check.py) findings with the baseline and reports only new drift (exit 1). A finding is new when its key is not in the baseline or it now occurs on more lines than the baselined `count`. CI runs it on every PR that targets `main` and on every push to `main` (the Docs workflow). It is report-only: drift never fails the check; new drift goes to the job summary and a warning annotation, and the brief reports drift in its own section. The check fails only when check.py itself errors (exit 2), and a failing Docs check then counts against `merge-ready` like any other failing check. Accept drift you do not fix with `just docs-check --update-baseline` in the same PR, and say why in the PR body.

- **New means not in the baseline, not introduced by this branch.** Drift that reaches `main` without a baseline update (for example a merged milestone whose status docs were not updated) shows as new on every later branch. Fix or baseline it in a dedicated docs PR; do not fold it into unrelated work.
- **Confirmed false positives** get a `reason` field on their baseline entry. They are marked `~` in `--all` output, the reason survives `--update-baseline`, and backlog PRs must not "fix" them.

-----

## Stop

Press Esc to interrupt the current turn; close the terminal to end the session. Stage 0 runs nothing in the background (no `/loop`, no scheduled routines, no launchd job), so those two stop everything. A stop file and a label-based kill switch come with the loops.

-----

## Staged rollout

| Stage | What runs | Exit criteria | Status |
|---|---|---|---|
| 0 | `/start` brief, doctor and pause; docs-check (report-only CI); guard rails on permissions and `main`; `needs-human` issues for open decisions | `/start` gives an accurate brief | **current** |
| 1 | `/start work`: attended single item, one issue = one milestone = one PR, docs and low-risk tiers | about 5 attended items merged with no guard-rail violations | planned |
| 2 | Local tend-only loop on this Mac (own PRs: CI fixes, review replies); nightly soak of `main` | 2 weeks with no out-of-policy actions and stable spend | planned |
| 3 | The loop may claim `agent-ready` docs and low-risk items (WIP = 1); kernel-core work stays attended. Requires the boot-crash fix and a required soak check | rework and escalation rates stable over 2 retros | planned |
| 4 | Unattended local runner (launchd): one capped headless run per item, automatic resume after a usage reset; scheduled retro PR against rules and skills | stable spend and merge quality over a month | planned |
| 5 | GitHub auto-merge behind required checks | owner decision | planned |

-----

## First 10 minutes back

1. Open the repo in Claude Code and run `/start`. Read the brief.
2. Answer the open `needs-human` issues: comment your decision, then close or relabel the issue.
3. Review and merge the PRs the brief marks `merge-ready: yes` once you are happy with them (`/merge-and-cleanup <PR>`); gated PRs wait for their `needs-human` issue.
4. Check main CI and the main soak line in the brief; a red `main` comes first. The "newest other soak" line is an experiment, not main's state.
5. Accept the proposed next action or name a different one.
6. Before you leave: `/start pause`.
