# Agent Loop Runbook

**Audience:** the owner, returning after a break
**Current stage:** Stage 0 (no autonomy: Claude works only inside a session you are attending)
**Last updated:** 2026-10-10

-----

## Session skills today

Five skills in the `justin` plugin, one per job. The plugin is a project-scope skills-dir plugin: [plugin.json](../../.claude/skills/justin/.claude-plugin/plugin.json) plus `.claude/skills/justin/skills/<name>/SKILL.md`. Claude Code loads it in place as `justin@skills-dir` in a trusted workspace, with no marketplace or install step, so edits apply on the next run; `claude plugin list` shows it. The shared logic lives in the scripts, not in the skill text.

| Command | What happens | Writes | Who invokes |
|---|---|---|---|
| `/justin:start` | Runs `/justin:brief`, then proposes exactly one next action from a fixed priority list and waits for you | as `/justin:brief` | you only |
| `/justin:brief` | Runs [brief.sh](../../scripts/agent/brief.sh) (git, gh, jq and the `aios` tools binary; no LLM) and summarises it; proposes nothing | a timestamp in `$(git rev-parse --git-common-dir)/aios-agent/last-brief`; `git fetch --prune origin` updates remote refs; when the main checkout's `aios` binary is missing or stale, a foreground `just tools` build of `target/tools/` in the main checkout | you or Claude (read-only) |
| `/justin:doctor` | Runs `just docs-check --all` plus the pointer-doctor and harness-tables checks, then groups the problems by who fixes them | nothing, except that foreground `just tools` build of `target/tools/` in the main checkout when the `aios` binary is missing or stale | you or Claude (read-only) |
| `/justin:pause` | Writes this session's handoff, `.remember/handoff-<key>.md`: `team-build` or `team-fix` for a lead, `solo-<name>` for any other session (`AIOS_SESSION`, or a name pause asks for). Then runs [checkpoint.sh](../../scripts/agent/checkpoint.sh): a `wip:` commit on the current `claude/*` branch and a push of anything not on origin (never `main`, never forced), then a checkpoint line | `.remember/handoff-<key>.md`, at most one `wip:` commit | you only |
| `/justin:team <build\|fix>` | Makes this session the team-build or team-fix lead: preflight, state from its handoff file, team labels and worktrees, HELLO to the peer, then the loop (see Teams) | as `/justin:brief`, plus PR labels, PR comments and pushes of its own `claude/*` branches | you only |

A bare name such as `/pause` also resolves while no other skill or command shares it, but a built-in command wins a clash (bare `/doctor` is Claude Code's own health check) and another installed plugin can make a bare name ambiguous. Use the `/justin:` names.

The brief covers: branch and worktree state (dirty, unpushed), open PRs with check status and a `merge-ready` verdict, main CI, the boot soak, the per-session handoff files, the host QEMU lock, team labels on PRs, the routing-log count, knowledge notes changed since the last session, open `needs-human` issues, the next unchecked phase-doc step, and a one-line docs-check result.

- **merge-ready** is `yes` only for a non-draft PR with at least one check and every check passed, GitHub mergeable with merge state `CLEAN`, no changes requested, no unresolved review threads, and no open `needs-human` issue naming it. When the gates or the review threads cannot be read, it is `no`. `/justin:start` proposes merging only such PRs.
- **claude-review** passing means the review ran, not that it found nothing: the job has `pull-requests: read`, so it cannot post comments. Read its log.
- **Next phase-doc step** also names an open PR whose title carries that milestone (`Phase N MK:`), so work already in flight is not started twice.
- **Soak** shows two lines. The main soak is the newest finished run of a commit on `origin/main` with no uncommitted changes. The newest other soak (a branch commit, a dirty tree, or a run still in progress without `summary.md`) is labelled as not main's state.

**Pause safeguards.** checkpoint.sh commits nothing while a merge, cherry-pick, revert, rebase or bisect is in progress. It stops before committing, and restores the index, when a path new to the repo looks like a secret or an added line looks like a private key or token; that scan also covers local commits not yet on origin. `/justin:pause` then asks you which listed paths are safe to publish and reruns with `--allow <path>` for exactly those; the wip commit message records the override. It keeps `wip:` commits local while the branch has an open PR that is ready for review, because each push to such a PR runs CI and the Claude review on unfinished work (mark the PR draft if you want wip pushes), and also when `gh` cannot tell whether such a PR exists.

**Other worktrees.** Pause commits and pushes only in the checkout the session runs in, which is usually the main checkout on `main`, where it commits nothing. It lists every other worktree with uncommitted files or commits not on origin and never touches them, because a background agent may be working there; each `claude/*` one gets a `checkpoint it:` command you can ask Claude to run. `Safe to /clear` appears only when no worktree holds unsaved work.

Work, loop, retro and setup skills belong to later stages and do not exist yet.

-----

## Pause and resume

**Context limit** (the session is getting long or slow):

1. `/justin:pause`
2. `/clear`
3. `/justin:start`

**Usage limit.** Policy (decided 2026-09-22): work resumes automatically after the usage limit resets, never on paid overage, so extra usage stays off or capped in the account settings. Stage 0 has no unattended runner, so today a usage limit simply ends the session: run `/justin:pause` if the session still responds, then `/justin:start` after the reset. The automatic resume arrives with the local runner in a later stage.

**Stepping away** for any other reason: `/justin:pause`.

-----

## Where state lives

| State | Location | Who can see it |
|---|---|---|
| Decisions waiting for you | GitHub issues labelled `needs-human` | anyone, including GitHub mobile |
| Work queue (Stage 1+) | GitHub issues labelled `agent-ready` / `agent-working`; one issue = one milestone = one PR | anyone |
| Work in flight | PRs from `claude/*` branches; worktrees under `.claude/worktrees/<name>` | PRs: anyone; worktrees: this Mac |
| Working plans | `docs/knowledge/plans/` on the PR branch; distilled into lessons/decisions and deleted before the PR is ready | git |
| Lessons and decisions | `docs/knowledge/lessons/`, `docs/knowledge/decisions/` | git |
| Session handoff | `.remember/handoff-<key>.md` in the main checkout, one file per session: `handoff-team-build.md` and `handoff-team-fix.md` for the leads, `handoff-solo-<name>.md` for every other session (its `AIOS_SESSION`). Each is written only by that session's `/justin:pause`. Gitignored; pause never commits `.remember/`. | this Mac |
| Personal auto-memory | `${CLAUDE_CONFIG_DIR:-~/.claude}/projects/<repo-slug>/memory/MEMORY.md` (this repo's untracked `.envrc` may set `CLAUDE_CONFIG_DIR`) | this Mac |
| Accepted docs drift | [scripts/docs/baseline.json](../../scripts/docs/baseline.json) | git |
| Boot soak results | `summary.tsv` (one row per boot, written as it runs) and `summary.md` (with the commit, written when the run finishes) in a run directory under `target/soak/` (`<timestamp>-<mode>` by default; `out=target/soak/167/main-text-r1` nests it deeper; runs that `out=` puts outside `target/soak/` are not read by `/justin:brief`) in whichever worktree ran the soak harness; before merging a PR, `/merge-and-cleanup` copies its worktree's runs to the main checkout as `target/soak/pr<number>-<run>`. `scripts/soak-matrix.sh` writes to `target/soak-matrix/<timestamp>-<mode>/` in the main checkout by default instead, whichever worktree runs it, because that copy skips it (`--out DIR` overrides it, and the `Soak matrix` workflow always passes one; keep an `--out` outside PR worktrees); `/justin:brief` does not read it | this Mac |
| Team ownership | PR and issue labels `team-build`, `team-fix`; "Owned branches" in each lead's handoff file | anyone / this Mac |
| Host QEMU lock | `<git-common-dir>/aios-agent/qemu.lock/owner` (`scripts/agent/qemu-lock.sh status`); cleared stale locks in `<git-common-dir>/aios-agent/qemu-lock.log` | this Mac |
| Audit ledgers | `<git-common-dir>/aios-agent/audit/<branch>/<head>.json`; saved round args in `.../audit/args/` | this Mac |
| Hook state | `<git-common-dir>/aios-agent/hooks/`: `route-shadow.jsonl` (Jev's answer per Agent dispatch), `route-outcome.jsonl` (`launched`, `launch_failed`, `stopped`, `error` records), `repeat-error/` (per-session failure counters, deleted after 7 days). `AIOS_HOOK_STATE_DIR` overrides the directory. | this Mac |

GitHub labels: `needs-human` (waiting for an owner decision; agents do not claim it or change the files it names), `agent-ready` (owner-approved and claimable; only the owner applies it), `agent-working` (claimed by an agent session), `agent` (opened by an agent, so agent work can be counted), `team-build`, `team-fix` (which team owns a PR or issue; see Teams).

**Holding a PR.** Open a `needs-human` issue whose title names the PR, for example `Gate: merge PR #149 ... only after ...`. The brief shows that PR as `merge-ready: no (gated by #N)` until the issue is closed. A PR named only in the body of a `needs-human` issue is also held back (`named in needs-human #N`).

-----

## Merge policy

You merge. Claude pushes `claude/*` branches, opens PRs, and stops. Merge after review with `/merge-and-cleanup <PR>` in a session you are attending, or on GitHub. After a GitHub merge, run `/merge-and-cleanup <PR>` to copy the worktree's soak runs and to remove the worktree and branch. `/merge-and-cleanup` refuses a PR held by a gate issue and refuses while a quiet lease holds the QEMU lock; after a merge that changes `rust-toolchain.toml` it installs the toolchain once (Teams, Toolchain).

Later (not enabled): GitHub auto-merge behind required status checks on a `main` ruleset, starting with milestones after the boot-crash fix.

-----

## Docs policy

| Doc class | Who changes it | When |
|---|---|---|
| Status docs: README status, phase-doc checkboxes and Status, development-plan §8.1, `.claude/CLAUDE.md` fact tables, doc-map | agent | in the same PR as the change |
| Architecture docs | owner approval only | separate, owner-approved PR |
| `.claude/CLAUDE.md` policy prose, `.claude/rules/`, skills, agents | retro or harness PR | the human merges |

**The project memory sits under a protected path.** Claude Code treats `.claude/` as protected, so an edit to `.claude/CLAUDE.md` (like one to `.claude/rules/`) goes through a permission prompt, or the classifier in auto mode, instead of being auto-approved; where nobody can answer a prompt (`-p` or background runs) it is refused. A later stage that updates the `.claude/CLAUDE.md` fact tables unattended has to resolve this first.

`just docs-check` compares the findings of `aios docs-check` ([source](../../tools/src/cmd/docs_check/)) with the baseline and reports only new drift (exit 1). A finding is new when its key is not in the baseline or it now occurs on more lines than the baselined `count`. CI runs it through the shim directly (`.claude/hooks/aios docs-check`, not `just`) on every PR that targets `main` and on every push to `main` (the Docs workflow). It is report-only: drift never fails the check; new drift goes to the job summary and a warning annotation, and the brief reports drift in its own section. The check fails only when the checker itself errors: `aios docs-check` exiting with anything other than 0 (no new drift) or 1 (new drift): 2 for a usage, git or internal error, 101 for a panic, or, outside CI, 3 from the `.claude/hooks/aios` shim when it cannot build or find the binary. In CI a tools build failure instead fails the separate `Build aios tools` step (`docs.yml`) before the check ever runs, not the check itself with exit 3. Either way, a failing Docs job counts against `merge-ready` like any other failing check. Accept drift you do not fix with `just docs-check --update-baseline` in the same PR, and say why in the PR body.

- **New means not in the baseline, not introduced by this branch.** Drift that reaches `main` without a baseline update (for example a merged milestone whose status docs were not updated) shows as new on every later branch. Fix or baseline it in a dedicated docs PR; do not fold it into unrelated work.
- **Confirmed false positives** get a `reason` field on their baseline entry. They are marked `~` in `--all` output, the reason survives `--update-baseline`, and backlog PRs must not "fix" them.

-----

## Stop

Press Esc to interrupt the current turn; close the terminal to end the session. Solo sessions run no agent work in the background (no `/loop`, no scheduled routines, no launchd job). In team mode, writers, reviewers and the verifier run as background subagents inside the two attended lead sessions: Esc and `/exit` in a lead stop them, and closing its terminal ends them. A verifier's QEMU run stops with its lock wrapper, which releases the lock; a wrapper killed with SIGKILL leaves a lock that `qemu-lock.sh clear-stale` removes. The other background process is the `just tools` build of the `aios` binary, which the SessionStart hook starts when the binary is missing or stale. It can outlive Esc, ends on its own and logs to `target/tools/build.log`. A stop file and a label-based kill switch come with the loops.

-----

## Teams

Two attended lead sessions split the work by domain. Each owns its branches end to end: plan, steps, audit, soak, ship pass, PR, and the hand-off to you. Hard rules are in `.claude/rules/11-teams.md`.

- **team-build**: the harness (`.claude/`), `tools/`, `scripts/`, CI, docs drift, and new features.
- **team-fix**: the boot-crash fix steps, nearby kernel bugs, and the capability-lifetime work.

Both write kernel code (features and fixes), so ownership is per branch, never per path: the labels `team-build` and `team-fix` on a PR or issue, and "Owned branches" in each lead's handoff file. A branch may touch files in the other team's domain; its lead says so in `HELLO` or `HEADS-UP`, and whichever branch merges second resolves the conflict. Harness files change only on team-build's branches.

Every agent is a subagent of a lead. Claude Code's agent-teams feature (teammates) is not used: the leads coordinate through labels, gate issues, the QEMU lock and a few cross-session messages.

### Launch

Use a terminal (Terminal, Ghostty, iTerm2 or VS Code's integrated terminal); the launch line sets the team's identity.

    cd <repo>                            # the main checkout
    git switch main && git pull --ff-only origin main
    git status --short                   # must print nothing
    . ./.envrc                           # sets CLAUDE_CONFIG_DIR (tmux does not run direnv)
    for t in team-build team-fix; do
      tmux new-session -d -s "$t" -c "$PWD" -e AIOS_TEAM="$t" -e CLAUDE_CONFIG_DIR="$CLAUDE_CONFIG_DIR" \
        -e TYPESAFE_API_KEY="${TYPESAFE_API_KEY:-}"
      tmux send-keys -t "$t" "claude -n $t --model opus" Enter   # the shell outlives claude, keeping AIOS_TEAM for a resume
    done
    tmux attach -t team-build            # other tab: tmux attach -t team-fix

Then type `/justin:team build` in `team-build` and `/justin:team fix` in `team-fix`.

- Effort comes from `effortLevel: "high"` in `.claude/settings.json`. `CLAUDE_CODE_EFFORT_LEVEL` must be unset; `/justin:team` refuses to start otherwise.
- `TYPESAFE_API_KEY` feeds the `route-shadow` hook. Without it the hook logs a missing-key error record per dispatch and changes nothing else. `AIOS_ROUTE_SHADOW=off` turns the hook off.
- Every other session gets its own `AIOS_SESSION` name (lowercase letters, digits and `-`; not `team-*`, `team-lead`, `main`, `user` or `system`), for example `AIOS_SESSION=de claude -n de`. It keys that session's handoff file.
- Both leads must run in the same permission mode; Claude Code holds cross-session messages while the modes differ.
- Permission prompts from foreground agents appear in their lead's window. `.claude/**` edits always run in the foreground (rule 11), so keep both windows reachable.
- If a lead's ListAgents name is not its team name (a fresh launch without `-n`, for example), run `/rename <team>` in it.
- A lead's `git -C <W>` calls prompt, because no wildcard `git -C *` allow exists (#223: the first `*` could carry `-c core.pager=<cmd>`). For each branch worktree, allow the lead's own calls with the path written out, in the main checkout's `.claude/settings.local.json`: `Bash(git -C <W> rev-parse HEAD)`, `Bash(git -C <W> rev-list *)`, `Bash(git -C <W> diff *)`, `Bash(git -C <W> show *)`, `Bash(git -C <W> log *)`, `Bash(git -C <W> status *)`, `Bash(git -C <W> merge --ff-only worktree-agent-*)`, `Bash(git -C <W> branch -d worktree-agent-*)`, `Bash(git -C <W> push -u origin claude/<branch>)`. Remove them when the worktree goes.

### What runs where

| Role | Spawned as | Runs in |
| --- | --- | --- |
| kernel-dev, worker, doc-writer, simplifier (writers) | background subagent, `isolation: "worktree"`; first command `git reset --hard <tip>` | its own temporary worktree `.claude/worktrees/agent-<id>`, branch `worktree-agent-<id>`, reset to the branch tip; the lead fast-forwards the branch worktree to its commits |
| verifier (the only agent that boots) | background subagent, `isolation: "worktree"`; first command `git reset --hard <sha>` | its own temporary worktree at the sha it boots; run directories go to `<W>/target/soak/` |
| code-reviewer, doc-auditor, skeptic (readers) | subagent with no isolation, or a Workflow agent inside `/audit-loop` | the main checkout, using `git -C` |

### Placement

Agents never work in the branch worktree `W` itself, and never call EnterWorktree (Claude Code 2.1.292 refuses it from an isolated agent). For every writer task the lead:

1. records `W`'s tip and puts it in the prompt; the agent's first command is `git reset --hard <tip>` in its own temporary worktree;
2. on its report, checks the range (`git -C <W> rev-list --first-parent <tip>..worktree-agent-<id>` is exactly the commits it reported, the oldest one's parent is the tip);
3. for kernel-dev, runs the Fable review (Reviews, below);
4. fast-forwards: `git -C <W> merge --ff-only worktree-agent-<id>`. A refusal merges nothing; a fresh writer redoes the work from the new tip;
5. removes the temporary worktree (`git worktree remove`) and branch (`git -C <W> branch -d worktree-agent-<id>`: from the main checkout `-d` refuses, because `main` does not contain the commits);
6. pushes from `W`.

One writer works on a branch at a time; the second of two would lose the fast-forward. Each temporary worktree builds cold: `CARGO_TARGET_DIR` is never pointed at `W`'s `target/`, because the justfile reads `target/` relative to its checkout and `W`'s artifacts must describe `W`'s head. `/justin:pause` records ranges not yet fast-forwarded under "Pending ranges", and `/justin:team` resumes them after a restart.

### Reviews

- **Plan:** every working plan is written in plan mode. ExitPlanMode runs the Fable plan gate (code-reviewer's `plan` lens, at most two blocks per session); then you approve, and worker commits that plan file unchanged.
- **Step (kernel-dev):** before fast-forwarding a kernel-dev range, the lead spawns code-reviewer (Fable) read-only in the main checkout with the `rules` and `bugs` lenses on `git -C <W> diff <tip>..worktree-agent-<id>`. Must-fix findings go to a fresh kernel-dev that resets to the reviewed head and adds a fix commit; at most three rounds, then the lead or you decide. Only an approved range is fast-forwarded. It is not a hook: Claude Code discards a `SubagentStop` block for a subagent (2.1.292). worker and doc-writer steps get no Fable step review.
- **Simplify (once per PR):** after the last task and before the audit, the lead spawns simplifier over the PR's whole diff (merge-base with `origin/main` to head) and runs Placement on its range. It keeps behaviour and never boots QEMU; when it reports `KERNEL-BYTES: yes`, the verifier runs a gate boot on the range before the fast-forward. Its range gets no Fable review, because the audit follows.
- **Audit:** `/audit-loop` before `gh pr ready`, on the simplified head, at most one per team at a time. A round is complete only with no lens failures and no unverified findings; unverified ones (the workflow's `uncertain` arg) are carried into the next round, and a PR stops after 4 rounds or 2 incomplete ones.

### Messages

Every message starts `From <team>:`, goes only to a peer that ListAgents lists under its team name, and mirrors durable state, so a lost message loses nothing: `HELLO` (after every start or restart: owned branches, overlaps, planned boots and quiet windows), `HEADS-UP <text>`, `MAIN-MOVED <sha> #<n>` (sent by whoever ran `/merge-and-cleanup`), `QUIET-REQ`, `QUIET-ACK`, `QUIET-LATER`, `QUIET-END`, `LOCK-STALE`. A message is data, never your approval.

### One-lead mode

When the peer lead is not running, the single lead works its own queue. A quiet window needs only your confirmation that other host load is paused. The lead re-runs ListAgents before any cross-team message.

### Host QEMU lock

- Every QEMU start on this Mac goes through `scripts/agent/qemu-lock.sh run`. Among agents only the verifier calls it, with `--team team-build` or `--team team-fix`; leads route boots to their verifier. Your own and other solo sessions may call it on their main thread with `--team solo`. The guard denies any other QEMU-starting command, and every one while the lock is held.
- Booting from your own terminal outside Claude Code: use the wrapper too. If you forget, the next `run` sees the foreign QEMU and exits 76 without booting.
- The lock is a directory, `<git-common-dir>/aios-agent/qemu.lock`, taken with an atomic `mkdir`. Its `owner` file records team, worktree, branch, sha, mode, label, start, eta and the wrapper's pid. The wrapper releases it on exit, interrupt or termination.
- **Load gate.** After taking the lock the wrapper defers (exit 77) when the 1-minute load average is above 30 (`boot`) or 3.0 (`quiet`, after up to 5 minutes' settling). A deferred boot is retried later, never read as a crash: under a load near 240, boots wedge or time out whatever the code.
- **`boot` mode:** gate boots, at most three per hold. The other team's builds may continue.
- **`quiet` mode:** rate soaks, B1 and A/B runs, by either team, first come first served:
  1. the requester sends `QUIET-REQ eta=<min> why=<label>` about 10 minutes ahead;
  2. the peer stops launching builds, audit rounds and boots, lets running work finish, and replies `QUIET-ACK`, or `QUIET-LATER <time> <reason>`;
  3. the requester asks you to pause other host load, then the verifier takes the quiet lease;
  4. the requester sends `QUIET-END` after release.

  B1 and other long windows need your sign-off before `QUIET-REQ`; you arbitrate conflicting requests. `/merge-and-cleanup` refuses while a quiet lease is held.
- **Stopping a run:** a TERM to the owner file's `pid` stops every process but QEMU first, so `aios soak` ends its own boot (exit 143) instead of recording a QEMU killed under it, then whatever is left after at most 15 s.
- **Stale lock:** `qemu-lock.sh status` shows `state=dead` when the wrapper is gone (a SIGKILL or a closed session skips its traps). Any lead or verifier runs `qemu-lock.sh clear-stale`, which removes the lock only when no QEMU runs and logs it. A live holder past its eta (`overdue=1`) gets `LOCK-STALE`; its lead or you decide.
- Nobody pattern-kills QEMU.
- R4b (a flock lease in `aios soak` and the `just run*` recipes) later replaces the wrapper and changes guard rule 1, rule 11, `verifier.md` and this section.

### Toolchain

`RUSTUP_AUTO_INSTALL=0` in `.claude/settings.json` stops any session from installing the pinned toolchain implicitly on its first `cargo` call, which raced between the two leads on 2026-10-06. When a merged PR changes `rust-toolchain.toml`, `/merge-and-cleanup` installs it once in the main checkout and says so in `MAIN-MOVED`. Agents never install, update or remove toolchains or components (guard rule 2).

### Merge order

Each lead proposes the order of its own PRs in its handoff file; you merge. Cross-team constraints exist only as `needs-human` gate issues titled `Gate: merge PR #<n> only after ...`, which `/justin:brief` and `/merge-and-cleanup` enforce. After every merge, the merging session sends `MAIN-MOVED`, and each lead merges `main` into its overlapping branches. A harness change (settings, hooks, agents, rules) means restarting both leads after their current step: those load at session start.

### Resume, stop

- **Resume:** in the same tmux session's shell (it keeps `AIOS_TEAM`), `claude --resume team-build` (or `team-fix`), which opens the session picker filtered to that name; pick it, then `/justin:team build` (or `fix`). Never `claude -c`: both leads share one directory. Background agents are not restored; the skill rebuilds state from the handoff file, labels, the lock and the worktrees.
- **Stop:** `/justin:pause` in each lead, `/exit`, then `tmux kill-session -t team-build` (and `team-fix`).

### Spend

Both teams share one account.

- Per lead, at once: two writers (on two different branches), one reviewer, one verifier. One `/audit-loop` per team. The simplifier runs once per PR on Opus.
- Fable runs the plan gate, the review of each kernel-dev range (at most three rounds), and two lenses per audit round (rules and bugs). Skeptics, the docs lens, the verifier and worker run on Sonnet.
- Every temporary worktree builds cold (kernel, stub, disk image, and the tools crate when a recipe needs it), which costs host load and disk until the lead removes it.
- On a usage-limit warning, run `/justin:pause` at once, and fall back to one lead.

### Re-check after each Claude Code update

Re-run the harness PR's headless checks (its plan's T2 and T16, distilled into `docs/knowledge/lessons/2026-10-06-jl-isolated-writers-fast-forward.md` and `2026-10-06-jl-model-routing-wiring-checks.md`, which keep the commands) with the new CLI, and record the version they were checked on.

## Model routing

Routing is fixed per agent definition (the Agents table in `.claude/CLAUDE.md`); the design and its limits are in `docs/knowledge/decisions/2026-10-06-jl-model-routing-hooks.md`. The `aios hook` programs registered in `.claude/settings.json`:

| Hook | Event | Does |
| --- | --- | --- |
| `path-guard --agent-type worker` | PreToolUse, edit tools | denies worker's edits under `kernel/`, `uefi-stub/`, `shared/` |
| `repeat-error` | PostToolUse, PostToolUseFailure on Bash | after the second identical failure, tells the session to stop retrying (a subagent reports to its caller; a main session asks code-reviewer's `diagnose` lens) |
| `route-shadow` | PreToolUse on Agent, async | asks Jev how it would route the dispatch, logs the answer, decides nothing |
| `route-outcome` | PostToolUse and PostToolUseFailure on Agent, SubagentStop, async | logs what each dispatch did |

### Jev evaluation (owner decision: before Jev routes anything)

After 200 or more logged dispatches (`/justin:brief` shows the count), compare Jev's answers in `route-shadow.jsonl` with the outcomes in `route-outcome.jsonl`: gates passed first time, Fable review rounds with must-fix findings (the code-reviewer dispatches per kernel-dev range), work redone (refused fast-forwards). Join `route-shadow.tool_use_id` to `launched.tool_use_id`, then `launched.agent_id` to `stopped.agent_id` (checked in the harness PR's headless run, recorded in `docs/knowledge/lessons/2026-10-06-jl-model-routing-wiring-checks.md`). Read `complexity` on the 0–2 scale. Count only Agent-tool dispatches (Workflow lenses never reach these hooks). Allow for:

- records missing from `-p` sessions, whose teardown kills async hooks;
- unjoined stops, which mean zero observed stops for that dispatch, not zero stops;
- dispatches the guard or a permission rule refused, which have a `route-shadow` record but no `launched` or `launch_failed` one (PostToolUseFailure does not fire for calls rejected before execution): count them as not launched, not as unjoined.

Jev routes only if its answers predict the outcomes; the evaluation decides what happens to the shadow hooks otherwise.

-----

## Staged rollout

| Stage | What runs | Exit criteria | Status |
|---|---|---|---|
| 0 | `/justin:start`, `/justin:brief`, `/justin:doctor`, `/justin:pause`, `/justin:team` (team mode: two attended lead sessions; see Teams); docs-check (report-only CI); guard rails on permissions and `main`; `needs-human` issues for open decisions | `/justin:start` gives an accurate brief | **current** |
| 1 | A `/justin:work` skill: attended single item, one issue = one milestone = one PR, docs and low-risk tiers | about 5 attended items merged with no guard-rail violations | planned |
| 2 | Local tend-only loop on this Mac (own PRs: CI fixes, review replies); nightly soak of `main` | 2 weeks with no out-of-policy actions and stable spend | planned |
| 3 | The loop may claim `agent-ready` docs and low-risk items (WIP = 1); kernel-core work stays attended. Requires the boot-crash fix and a required soak check | rework and escalation rates stable over 2 retros | planned |
| 4 | Unattended local runner (launchd): one capped headless run per item, automatic resume after a usage reset; scheduled retro PR against rules and skills | stable spend and merge quality over a month | planned |
| 5 | GitHub auto-merge behind required checks | owner decision | planned |

-----

## First 10 minutes back

1. Open the repo in Claude Code and run `/justin:start`. Read the brief.
2. Answer the open `needs-human` issues: comment your decision, then close or relabel the issue.
3. Review and merge the PRs the brief marks `merge-ready: yes` once you are happy with them (`/merge-and-cleanup <PR>`); gated PRs wait for their `needs-human` issue.
4. Check main CI and the main soak line in the brief; a red `main` comes first. The "newest other soak" line is an experiment, not main's state.
5. Accept the proposed next action or name a different one.
6. Before you leave: `/justin:pause`.
