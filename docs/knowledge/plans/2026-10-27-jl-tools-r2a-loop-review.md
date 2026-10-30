---
author: jl + claude
date: 2026-10-27
tags: [tooling]
status: in-progress
---

# Plan: Tools R2a: `aios loop review` (read-only PR review) and `/justin:review`

## Context

R2, the self-improving PR loop, is the next tools PR. Its 2026-10-11 plan (1131 lines, 25 tasks, plus a 665-line R2/R3 contract) was written to be built in parallel with an unmerged R3. R3 merged on 2026-10-20 as #257 (`4cbb812`), so that plan's coordination machinery is obsolete:
- the vendored API freeze;
- Phase A/B and the convergence task;
- the merge-order gate;
- the contract amendments.

On 2026-10-27 the owner chose to cut R2 into three PRs, each with its own short plan:

| PR | Scope |
|---|---|
| **R2a** (this plan) | `aios loop review`, read-only: one review round (gather, three lenses, verify), the findings report, a merge verdict (`ready` / `pending` / `needs-human`, reported only), the Option-A gate, the shim's strict `loop)` arm, prompts and config, `/justin:review`, the ≤ $0.50 probe. **No GitHub writes, no git push, no commits, no fix stage.** |
| R2b | The write side: fix and sync stages, rounds until convergence, push, thread replies and resolves, the needs-human label and comment, pre-existing issue filing, CI and reviewer waits, `aios loop merge`, `/justin:merge`, narrowing `/review-pr-comments`. |
| R2c | The eval suites (loop-stage and skill), `scores.json`, the eval gate on prompt, model and skill changes, first recorded scores (≤ $25, owner-approved cases). |

R2a is useful on its own: an attended lead or the owner gets a reproducible, model-reviewed findings report and a merge verdict for any same-repo PR, before running `/review-pr-comments` or `/merge-and-cleanup`.

**Branch:** `claude/tools-r2-loop`, worktree `W` = `/Users/juslee/dev/aios/.claude/worktrees/tools-r2`, created from `origin/main` 4cbb812 on 2026-10-27. It is team-build's (label `team-build`). R2b and R2c get their own branches after R2a merges.

**Inputs:**
- Spec: `docs/knowledge/discussions/2026-09-22-jl-rust-agent-tools.md` (SPEC).
- Loop design: `docs/knowledge/discussions/2026-09-22-jl-justin-review-merge-loop.md` (LOOP). It exists only on `claude/justin-review-merge-loop`, which Task 0 folds in.
- The superseded plan, for design detail only: `.git/aios-agent/tools-r2r3-work/2026-10-11-jl-tools-r2-loop.md` (OLD). This plan wins wherever they differ.
- Lessons:
  - `docs/knowledge/lessons/2026-10-20-jl-r3-brief-checkpoint-port.md` (jail, hermetic identity, test repos inside the checkout, `current_exe()`);
  - `2026-10-06-jl-model-routing-wiring-checks.md` (headless `claude -p` probing);
  - `2026-09-29-jl-soak-port-awk-and-process-parity.md` (supervised processes).

## Binding decisions (owner; never re-asked)

From 2026-10-11 (memory `aios-tools-r2-r3.md`, OLD's Decisions Made):
1. **#204 = Option A.** The changed-paths gate covers every input of the aios binary: `tools/**`, `shared/**`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `rust-toolchain`, `.cargo/**` and `justfile`. A gated PR is never `ready`.
2. **Loop exit 3, full strictness, in the shim.** `aios loop` exits 3 in these cases:
   - a stale binary whose foreground rebuild fails;
   - a dirty build;
   - a binary that is rebuilt but still not fresh;
   - a missing binary whose build fails;
   - `AIOS_TOOLS_BIN` set.

   A PR build runs directly only under `--tree` (R2a's dry-run equivalent) or, later, `eval`.
3. **Stage 0 (decision 6).**
   - Attended only. Nothing merges or auto-merges, and no `justin/review` status is posted.
   - Kernel-area fixes always go to needs-human (R2b).
   - The probe budget is ≤ $0.50, with fable confirmed in `-p`.
   - Retro and `metrics.jsonl` are deferred.
4. **Labels** `agent-stop` and `hard` exist (decision 10).

From 2026-10-27: the three-PR cut above. R2a does not fold in the `claude/justin-review-merge-loop` branch's prose unchanged: Task 0 merges it, then adds an Amendments section.

## Approach

### What R2a builds

`aios loop review <PR>... [--tree DIR] [--budget-usd N]`. For each PR, in order:

1. **Preflight**, reads and local checks only, all before any `claude` call:
   - **Kill switch.** The run is stopped if any of these holds: the `STOP` file exists; the `agent-stop` label is missing or its query errors; an open `agent-stop` issue exists or that query errors. It fails closed.
   - **Self-location.** Without `--tree`, `current_exe()` must canonicalise to `<main>/target/tools/installed/aios`.
   - **Team owner.** If `AIOS_TEAM` is set and the PR carries the other team's label, refuse.
   - **Head.** Refuse a fork PR (`isCrossRepository`).
   - **The QEMU lock.** If `qemu-lock.sh status` shows `mode=quiet`, stop with exit 4.
   - **The per-PR lock.**
2. **Gather**, deterministic. It creates a detached round worktree of the PR head at `ROUND_ROOT/loop-pr<n>`, chosen by the Task 1 probe, and collects:
   - the diff against the merge-base with `origin/main`, and the changed paths (`--no-renames`);
   - failing checks, with log tails;
   - `docs-check --json` new findings in the round worktree, excluding `plans-not-empty`, which becomes a note;
   - unresolved review threads, all pages, split into trusted and untrusted authors;
   - needs-human issues, with `merge_ready::gates`;
   - knowledge-hive notes selected by path tags;
   - prior verify dismissals for this PR from `findings.jsonl`.
3. **Review**: three lens stages in parallel (`correctness`, `security`, `conventions`), each a read-only `claude -p` call with a JSON schema.
4. **Verify**: one `claude -p` call that confirms or refutes each lens finding and scopes it `in-diff` or `pre-existing`.
   - **Deterministic findings** are confirmed without verify: a failing check, new docs drift, and an unresolved thread from a trusted author.
5. **Verdict**:
   - **Review status:** `clean` (no confirmed in-diff finding), `findings`, or `incomplete` (a stage failed twice, or the budget ran out).
   - **Merge verdict:** `ready` iff all of these hold:
     - the review is `clean`;
     - `merge_ready::blockers(..)` is empty;
     - the Option-A gate is empty;
     - GitHub's file list cross-checks against git's;
     - the kill switch is clear.

     Otherwise `pending` (transient blockers) or `needs-human`.

   Both are **reported, never written to GitHub**.
6. **Report**:
   - stdout, one block per PR;
   - `report.json` and `report.md` in the run directory;
   - one `findings.jsonl` record per verified finding;
   - the round worktree is discarded.

Outside the state dir and the round worktree, nothing changes: no gh write, no commit, no push. The read-only property is mechanical:
- every gh call goes through `gh::GhCli`, which runs `gh::is_read` before it spawns (`tools/src/gh.rs` L157);
- there is no `Command::new("gh")` in `cmd/loop/`;
- no stage has a write tool;
- a scenario invariant asserts zero gh writes and unchanged origin refs in every scenario.

### Reuse (on main)

- `tools/src/gh.rs`: `GhCli`, `GhEnv` (the loop's const `LOOP_GH_ENV` sets `GH_PROMPT_DISABLED=1`, `GH_NO_UPDATE_NOTIFIER=1` and `NO_COLOR=1`, and removes `GH_REPO` and `GH_HOST`), `GhOutcome::first_stderr_line`, `is_read`.
- `tools/src/merge_ready.rs`: `PR_FIELDS`, `NEEDS_HUMAN_ISSUES_ARGS`, `alt`, `check_state`, `gates`, `labelled_needs_human`, `blockers`. The loop's `ready` is therefore never looser than brief's `merge-ready: yes`.
- `tools/src/cmd/soak/host.rs` `git()` (L67): the hardened git builder (`LC_ALL=C`, no replace objects or grafts). Every loop git call uses it, because each feeds a decision; `tools/src/git.rs`'s inherit policy is not used.
- `TOOLS_INPUTS`, `TOOLS_EXCLUDE_DIRS` and `TOOLS_JUNK` (host.rs L397-412) move to `tools/src/inputs.rs` (one source for the gate, soak and the shim test).
- `proc::Supervisor` (`tools/src/proc.rs` L35) runs every `claude` stage in its own process group with a timeout. `cmd::soak::signals::Interrupts` (`signals.rs` L53) maps signals to exits 129, 130, 131 and 143.
- `hook::write_atomic` and `hook::append_line` (`tools/src/cmd/hook/mod.rs` L371, L355) handle state files.
- Test harness: `tests/common/fake_gh.rs`, `tests/common/jail.rs` (`FAKE_CLAUDE_TOOLS`, `bin`, `hermetic`, `script_commands`, `SH_WORDS`), and `tests/common/mod.rs` (`isolated`, `unique_dir`, `TestRepo`).
- The shim (`.claude/hooks/aios`): `stop()` L275, `runnable()` L287, `dirty_cause()` L369, `verdict()` L404, `build_fg()` L440, the `AIOS_TOOLS_BIN` branch L491, `case $sub` L513.

### Stage isolation (the read-only stages)

- **Environment.** `env_clear()` plus an allowlist: `PATH`, `HOME`, `USER`, `LANG`, `TERM`, `TMPDIR`, `CLAUDE_CONFIG_DIR`, and any variable the Task 1 probe proves necessary. If `CLAUDE_CONFIG_DIR` is unset, exit 2 ("refusing the default config").
- **Flags** (finalised as D2 by Task 1):
  - `--output-format json`, `--json-schema <stage schema>`, `--model <alias>`, `--max-budget-usd <per-call>`, `--no-session-persistence`;
  - `--permission-mode` = D2's mode (`dontAsk` expected);
  - `--tools Read,Grep,Glob` (no Bash: see D3);
  - `--allowedTools Read,Grep,Glob`;
  - `--disallowedTools` with `Bash`, `Edit`, `Write`, `Agent`, `WebFetch` and `WebSearch` (a speed bump; the toolset is the control);
  - `--strict-mcp-config --mcp-config '{"mcpServers":{}}'`;
  - D1's isolation flags.

  The prompt goes on stdin.
- **Settings.** `--settings <state>/pr-<n>/stage-settings.json`, materialised from the `origin/main` blob of `.claude/settings.json`. It holds:
  - main's guard `PreToolUse` hook, with an absolute path;
  - main's `deny` and `ask` lists;
  - an empty `allow`;
  - `enabledPlugins` with every plugin main names set to `false`;
  - no other hook, no `env`, and mode 0600.

  A blob without the guard entry is exit 2.
- **What is read from where.** Prompts, config and hive notes are read from `origin/main` blobs, so a PR cannot change its own reviewer. `--tree DIR` reads them from DIR instead (R2a's own pre-merge check, and prompt work). Stage settings always come from `origin/main`.

### Threat model (stated up front; findings outside it are residuals, not new mechanism)

- **Adversaries:**
  - PR content, its comments, and its own `.claude/CLAUDE.md` and rules;
  - a confused or prompt-injected stage.
- **What R2a protects:**
  - no writes anywhere (the mechanical read-only property above);
  - spend (per-call and per-PR caps, one round);
  - the account (`CLAUDE_CONFIG_DIR` required);
  - **an honest verdict**: brief's blockers, the gate and the cross-check are deterministic, so a model stage can add findings but never make a gated or blocked PR `ready`.
- **Residuals (accepted for R2a):**
  - A prompt-injected PR can steer the lenses into a false `clean`. At Stage 0 the owner's merge and the ruleset are the boundary.
  - The PR's own CLAUDE.md may load as instruction text, if Task 1 finds that no candidate prevents it.
  - #213 (the stamp follows local HEAD).
  - The shim arm's check-then-exec race.
  - A direct `…/aios loop` call skips the shim. It still meets the self-location check, and the settings ask (D4) prompts on it.
  - `--tree` prompts steer only a run the owner started.

## Progress

**Conventions.**
- **Placement**, rule 11, for every writer task:
  1. The lead records `tip=$(git -C <W> rev-parse HEAD)`.
  2. The writer is spawned with `isolation: "worktree"`, and its first command is `git reset --hard <tip>`.
  3. The lead checks the range.
  4. `git -C <W> merge --ff-only worktree-<name>`.
  5. `git worktree remove` (never `--force`), then `git -C <W> branch -d`.
  6. `git -C <W> push -u origin claude/tools-r2-loop`.
- **Foreground.** Tasks marked **[FG]** edit `.claude/**` or `tools/src/cmd/loop/**`. Both are protected paths: the settings ask on `Edit(**/tools/src/cmd/loop/**)`, and Claude Code protects `.claude/`. Those workers are spawned with `run_in_background: false`.
- **The #264 interim rule.** Before fast-forwarding any range that touches `.claude/`, the lead shows the owner the full `.claude/` diff and waits for an explicit OK.
- **TG**, the writer's gate before each commit: `cargo fmt --check -p aios-tools && cargo clippy -p aios-tools --all-targets -- -D warnings && cargo test -p aios-tools`, then DG.
- **DG**, the docs gate. From Task 0b until the ship pass, `just docs-check --json` exits 1, and its new findings are exactly `knowledge-hygiene plans-not-empty docs/knowledge/plans/2026-10-27-jl-tools-r2a-loop-review.md`. After the ship pass, `just docs-check` prints `No new drift since baseline.`
- **Writer rules.**
  - No `git stash`.
  - No real `gh` or `claude`: tests use fakes under the jail.
  - No `cargo run … loop`; tests use `env!("CARGO_BIN_EXE_aios")`.
  - No verify step starts with `just -`.
  - No token-shaped literals, no `/Users/` paths, and no account emails in committed files.
  - No settings edits.
- **Commit subject:** `Tools R2a Task N: <what>`, plus the session trailers.
- **T(args) = N:** `cargo test -p aios-tools <args> 2>&1 | grep -E '^test result:'` prints exactly one line, and it starts `test result: ok. N passed` (`≥ N` where marked).

- [x] **Task 0: Plan, loop design and draft PR** [worker; docs only]
  - [x] 0a (lead): `W` exists on `claude/tools-r2-loop` at 4cbb812 (done 2026-10-27). Verify: `git -C <W> rev-parse --abbrev-ref HEAD` prints `claude/tools-r2-loop`.
  - [x] 0b (worker): Commit this approved plan unchanged as `docs/knowledge/plans/2026-10-27-jl-tools-r2a-loop-review.md`, with rule 08 frontmatter. Verify: `git show --name-only --format= HEAD` prints exactly that path; DG.
  - [x] 0c (worker): `git merge --no-ff origin/claude/justin-review-merge-loop`.
    - It is a clean merge that adds only `docs/knowledge/discussions/2026-09-22-jl-justin-review-merge-loop.md`. On any conflict, abort and report.
    - Then, in a second commit, add a top section `## Amendments (Tools R2, 2026-10-27)` to that file. LOOP's body stays as written. The section records:
      - Rust in `tools/src/cmd/loop/`, run as `.claude/hooks/aios loop …`;
      - fakes are `#!/bin/sh` scripts on the R3 jail;
      - the R2a/R2b/R2c cut;
      - the Stage-0 scope: no auto-merge, no statuses, no sweep, no retro yet, `/merge-and-cleanup` stays, and `/review-pr-comments` stays (narrowed in R2b);
      - Option A;
      - exit 3 with full strictness;
      - stage settings, minimal and pinned to `origin/main`, superseding LOOP's `--setting-sources user`;
      - prompts and config read from `origin/main` except under `--tree`;
      - verdicts reported only in R2a;
      - trusted authors are the owner and Copilot (`claude[bot]` cannot post: `.github/workflows/claude-code-review.yml` grants `pull-requests: read`);
      - model aliases `sonnet`, `opus` and `fable`;
      - 6 required checks.
    - Verify: `git -C <worker wt> log -1 --format=%P HEAD~1 | wc -w` prints `2`; `grep -c '^## Amendments (Tools R2, 2026-10-27)' docs/knowledge/discussions/2026-09-22-jl-justin-review-merge-loop.md` prints `1`; DG.
  - [x] 0d (lead): Placement, then `gh pr create --draft --label team-build --title "Tools R2a: aios loop review (read-only) and /justin:review"`. Verify: `gh pr view <n> --json isDraft --jq .isDraft` prints `true`.

- [x] **Task 1: Probes and recorded fixtures** [OWNER-attended; lead's main thread; read-only `gh`; `claude` spend ≤ $0.50 total]
  All output goes to `<S>/r2a-probe/` (`<S>` = the session scratchpad). Every `claude` call:
  - runs with `env -u CLAUDE_CODE_EFFORT_LEVEL -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT`;
  - appends `<id>\t<cost>` to `cost.tsv` from the envelope;
  - is preceded by a check of the running total: the lead stops at $0.45 and reports the probes not run.

  The first call measures a fresh call's cost, with `--max-budget-usd 0.15`. Later calls are capped at 1.5× that cost, and acceptance-only calls use the small form (`--tools "" --setting-sources "" --strict-mcp-config --mcp-config '{"mcpServers":{}}'`). Priority, in order:
  - [x] 1a: **Envelope and flags.**
    - `--output-format json --json-schema <tiny schema>` with `--model sonnet`: save `success.json`, and record where `structured_output` lands and the cost key.
    - `--model fable` (small form) returns a structured result: `fable.json`.
    - `--model opus` is accepted.
    - The overrun envelope (`--max-budget-usd 0.0001`): `over_budget.json`.
    - An `env -i` start with the allowlist authenticates; record any extra variable it needs.
    - Whether `--max-turns` is accepted. If not, it is never used.
  - [x] 1b: **Permission mode (D2).** One call in a scratch git repo with `--permission-mode dontAsk --tools Read,Bash --allowedTools Read 'Bash(git diff:*)' --disallowedTools 'Bash(git commit*)' Agent WebFetch WebSearch`. The prompt runs `true`, then `git diff`. Record from `permission_denials` whether `true` was denied without a prompt and `git diff` ran: `denied.json`. If `dontAsk` prompts, retry once with `--permission-mode manual --permission-prompts none`.
  - [x] 1c: **Isolation (D1) and placement (`ROUND_ROOT`).**
    - **The scratch repo** `<S>/r2a-probe/iso` (not an aios worktree) holds:
      - a project `.claude/settings.json` hook that touches `marker-project-hook`;
      - a `.claude/CLAUDE.md` asking the model to echo a nonce.
    - **The stage-shaped call:**
      - D2's mode;
      - a `--settings` file whose `PreToolUse` hook touches `marker-settings-hook` (it stands in for the guard);
      - `--tools Read,Grep,Glob,Bash`, empty strict MCP, and `--output-format stream-json --verbose`;
      - one Bash command that only a user-scope allow of the personal config grants.
    - **Candidates, in order**, stopping at the first that qualifies:
      1. `--restricted --disable-slash-commands`;
      2. `--setting-sources "" --disable-slash-commands`;
      3. `--setting-sources local --disable-slash-commands`.
    - **A candidate qualifies when all of these hold:**
      - `marker-settings-hook` exists and no project-hook marker exists;
      - the init event lists no plugin, MCP server or skill, and its `tools` equals the `--tools` set;
      - the user-allowed command is in `permission_denials`.
    - **Fallbacks.** If none qualifies, use an empty stage cwd plus `--add-dir <round worktree>`. If that also loads project hooks, stop and ask the owner. Record whether the nonce came back; if it did, the CLAUDE.md residual applies.
    - **Placement:**
      - Make a detached worktree of `origin/main` at `<main>/.claude/worktrees/loop-probe` with `git -C <main> worktree add --detach`, run the D1 call there with the prompt "run `git diff --stat HEAD~1` and report it", and feed the guard a `git diff` PreToolUse payload with that cwd.
      - `ROUND_ROOT` = `<main>/.claude/worktrees` if the command ran and the guard gave no decision. Otherwise try `<os temp>/aios-loop-rt/<12 hex of sha256(common dir)>`. If neither works, stop and ask the owner.
      - Remove the probe worktree without `--force`.
  - [x] 1d: **gh shapes (GET only).**
    - `gh pr view 257 --json <PR_FIELDS>,headRefOid,baseRefName,isCrossRepository,files,changedFiles,commits`;
    - `gh pr checks 257 --json name,state,bucket,link`;
    - the `reviewThreads` GraphQL page query on #257;
    - `gh issue list` with `NEEDS_HUMAN_ISSUES_ARGS`;
    - `gh run view 40448856000 --log-failed` (#279's failed Security job, RUSTSEC-2026-0385);
    - `gh api repos/{owner}/{repo}/labels/agent-stop`, and the 404 of a missing label;
    - `gh issue list --label agent-stop --state open --json number`;
    - the Copilot reviewer login, as it appears on threads and in `reviews`.
  - [x] 1e (worker): Sanitise and commit the fixtures:
    - `tools/tests/fixtures/loop/claude/{success,fable,denied,over_budget}.json`;
    - `tools/tests/fixtures/loop/gh/{pr_view,checks,threads_p1,issues,label_agent_stop,agent_stop_empty}.json`, `label_404.{err,rc}` and `run_log_failed.txt`;
    - `README.txt` with the CLI versions and the probe commands.

    In the same commit, record D1, D2, `ROUND_ROOT`, the Copilot login and the cost total under this plan's Decisions Made.
  - [x] 1f: Verify:
    - `awk -F'\t' '{s+=$2} END{exit !(s<=0.50)}' <S>/r2a-probe/cost.tsv`;
    - `jq -e '.subtype=="success"'` on `success.json` and `fable.json`;
    - every listed fixture is non-empty and valid JSON where `.json`;
    - `git -C <W> grep -nE '/Users/|/home/|/private/|gh[pousr]_[A-Za-z0-9]{36}|sk-ant-' -- tools/tests/fixtures/loop` prints nothing;
    - `grep -cE '^- \*\*(D1|D2|ROUND_ROOT) \(final\):\*\* [^<]' <plan>` prints `3`;
    - `git -C <main> worktree list | grep -c loop-probe` prints `0`.

- [x] **Task 2: `tools/src/inputs.rs` and the loop module skeleton** [2a-2b worker; 2c FG]
  - [x] 2a: Move `TOOLS_INPUTS`, `TOOLS_EXCLUDE_DIRS` and `TOOLS_JUNK` from `soak/host.rs` to `pub const`s in `tools/src/inputs.rs`, and add `pub mod inputs;` to `lib.rs`. `host.rs` uses `crate::inputs::…`, and the test `tools_inputs_match_the_recipe_and_the_shim` moves with them.
  - [x] 2b: `pub fn is_tools_input(path: &str) -> bool`.
    - Directory entries match as prefixes on a `/` boundary; file entries match exactly. Both compare case-folded (fail-closed on APFS).
    - A table test covers these rows:
      - true: `tools/x`, `tools`, `shared/src/lib.rs`, `Cargo.lock`, `rust-toolchain`, `.cargo/config.toml`, `justfile`, `TOOLS/x`, `Justfile`;
      - false: `toolsx/a`, `docs/tools/x`, `kernel/Cargo.toml`, `Cargo.toml.bak`.
    - Verify: T(`--lib inputs::`) ≥ 2; `cargo test -p aios-tools --test soak_harness` passes.
  - [x] 2c [FG]: `tools/src/cmd/mod.rs` gets `#[path = "loop/mod.rs"] pub mod pr_loop;` (`loop` is a keyword; the directory keeps the protected name). `tools/src/cmd/loop/mod.rs` gets a module doc that states the read-only property. Verify: TG.

- [x] **Task 3: Config, prompts and schemas** [3a worker; 3b-3c FG]
  - [x] 3a: `scripts/agent/loop-config.json`:
    - **Models** (aliases): lenses `sonnet`, with `opus` for correctness and security when the diff touches `kernel_core_globs` or adds a line containing `unsafe`; verify `opus`, with `fable` when the PR has the `hard` label. `fallback_on_fable_error: "opus"`.
    - **Budgets:** `per_call_usd` (from Task 1's measured cost), `per_pr_usd: 5`, `stage_timeout_seconds`.
    - **Globs:** `kernel_core_globs: ["kernel/src/{arch,sched,ipc,mm}/**"]`.
    - **Authors:** `trusted_authors` (the owner and the Task 1 Copilot login).
    - **Categories:** `category_taxonomy`, LOOP §4's 9 categories.
    - **Hive:** `path_tags` (glob → rule 08 tag) and `hive_cap: {notes: 12, bytes: 40960}`.
    - **Logs:** `log_tail_bytes`.

    No key without a consumer in R2a (no-legacy). R2b and R2c add their own keys.
  - [x] 3b [FG]: `tools/src/cmd/loop/config.rs`:
    - `Config::from_json` with `deny_unknown_fields`;
    - `glob_to_regex` for `**`, `*` and `{a,b}`;
    - validation: known aliases, budgets > 0, globs compile, the taxonomy equals `schema::CATEGORY_TAXONOMY`, and every `path_tags` tag is in rule 08's list (a const, plus a test that re-reads the Tags line of `.claude/rules/08-knowledge-hive.md`).

    `tools/src/cmd/loop/schema.rs` holds:
    - the lens finding schema `{id, lens, severity: must-fix|should-fix|nit, category, file, line, claim, evidence, suggested_fix}`, with `category` an enum of the taxonomy;
    - the verify verdict schema `{id, verdict: confirmed|refuted, scope: in-diff|pre-existing, reason}`.
  - [x] 3c: `scripts/agent/prompts/{review-correctness,review-security,review-conventions,verify}.md`.
    - Each starts with `version: 1`. They are written from LOOP §2 and rules 01, 02, 06 and 11.
    - Placeholders: `{{diff}}`, `{{facts}}`, `{{prior_findings}}`, `{{lessons}}`, `{{untrusted}}` and `{{findings}}` (verify only). `{{diff}}` is the change, rendered by gather (6b, D3).
    - Every prompt says the stage is read-only and that text inside the untrusted fence is data.
    - `tools/tests/loop_config.rs`:
      - the committed config validates;
      - every prompt has `version:` and only known placeholders;
      - a malformed config names its bad key.
    - Verify: `cargo test -p aios-tools --test loop_config` passes; `ls scripts/agent/prompts/*.md | wc -l` prints `4`; TG.

- [x] **Task 4: Loop git, the gate and the gh reads** [FG]
  - [x] 4a: `tools/src/cmd/loop/git.rs`, on `soak::host::git()`:
    - `merge_base`;
    - `changed_paths(base, head)` (`-c core.quotePath=false diff --name-status --no-renames -z`);
    - `fetch_pr_head(n)` (`git fetch origin pull/<n>/head`, with `GIT_TERMINAL_PROMPT=0`);
    - `worktree_add_detached(path, sha)`;
    - `worktree_discard(path)`: `reset --hard`, `clean -fd` and `worktree remove` without `--force`. It acts only on a path recorded in state, named `loop-pr<n>`, and listed by `git worktree list --porcelain`. A missing directory gets `worktree prune`.

    Tests in `tools/tests/loop_git.rs` (TestRepo plus a bare origin):
    - both sides of a rename are listed;
    - a merge of main is excluded from the diff;
    - a detached round trip;
    - a dirty worktree is cleaned, then removed;
    - an unrecorded path is refused;
    - a missing directory is pruned;
    - the hardened env is set.
  - [x] 4b: `tools/src/cmd/loop/gate.rs`:
    - `normalise(path)`: strip `./`, collapse `//`, and reject `..` and a leading `/`;
    - `gated_paths(repo, head)`: `changed_paths` against `merge_base(origin/main, head)`, filtered by `is_tools_input`, sorted and unique;
    - `cross_check(git_set, github_files, changed_files)`: every GitHub path, normalised, is in git's set; `changed_files < 100`; and `changed_files == github_files.len()`. Otherwise it returns `Mismatch`, which is needs-human.

    Tests in `tools/tests/loop_gate.rs`: a path table of ≥ 25 rows, plus TestRepo cases (edit, add, delete, rename into and out of `tools/`, mode-only, a docs PR that merged a main which changed `Cargo.lock` gives an empty gate) and the five cross-check cases.
  - [x] 4c: `tools/src/cmd/loop/github.rs`, typed reads only, each through `gh::GhCli { cwd: <main>, env: LOOP_GH_ENV }`, each with a named consumer:
    - `pr_view` (preflight, gather, verdict);
    - `pr_checks` (log tails);
    - `review_threads` (paged until `hasNextPage=false`);
    - `issues_needs_human` (`NEEDS_HUMAN_ISSUES_ARGS`);
    - `label_exists` (404 gives `false`; any other error is `Err`);
    - `issues_labelled("agent-stop")`;
    - `run_log_failed`.

    `fn json(..)` maps `GhOutcome` (`Missing` gives "gh is not installed"; `Failed` carries `first_stderr_line`; `Refused` is an internal error). Tests in `tools/tests/loop_gh.rs`: every Task 1 gh fixture deserialises, every read argv passes `gh::is_read`, and pagination goes past 100.
  - [x] 4d: Verify:
    - `cargo test -p aios-tools --test loop_git --test loop_gate --test loop_gh` passes;
    - `git -C <worker wt> grep -n 'Command::new("gh")' -- tools/src` lists only `tools/src/gh.rs`;
    - TG.

- [x] **Task 5: State, kill switch, preflight and the stage runner** [FG]
  - [x] 5a: `tools/src/cmd/loop/state.rs`.
    - **Layout:** `<git-common-dir>/aios-agent/loop/` holds:
      - `pr-<n>/lock/pid`;
      - `pr-<n>/stage-settings.json`;
      - `pr-<n>/run-<epoch>/{round-01-<stage>.json, report.json, report.md}`;
      - `findings.jsonl`;
      - `STOP`.
    - **The state dir** honours `AIOS_LOOP_STATE_DIR` only under `cfg!(debug_assertions)`.
    - **Lock:** a live pid refuses; a dead pid is taken over.
    - **`FindingRecord`:** `{pr, head, lens, severity, category, file, line, claim, prompt_version, disposition: confirmed|dismissed-by-verify|pre-existing}`. It is appended once per verified finding, and `prior_for_pr(n)` reads a PR's records back.
    - **`kill_switch()`:** stopped when `STOP` exists, when `label_exists("agent-stop")` is false or errors, or when the open `agent-stop` issue query returns ≥ 1 or errors. It runs before the first stage and again before verify.
    - **`self_location()`:** without `--tree`, exit 2 unless `current_exe().canonicalize()` = `<main>/target/tools/installed/aios`. The debug-only test escape `AIOS_LOOP_ANY_EXE=1` holds only together with `AIOS_LOOP_STATE_DIR=<root>/state`, where the first `gh` and `claude` on PATH canonicalise to `<root>/bin/{gh,claude}`.
    - **`team_owner_check()`:** after `pr_view`, before the lock. If `AIOS_TEAM` is set and the PR carries the other team's label, exit 1 with `pr #<n>: owned by <team>; run it from that team's lead`.
    - **Fork:** a `isCrossRepository` PR gives `needs-human: fork PR` before any fetch.
    - **Quiet:** a `qemu-lock.sh status` with `mode=quiet` stops with exit 4.

    Tests in `tools/tests/loop_state.rs`, one per bullet, including the kill switch failing closed on a 404 and on a gh error.
  - [x] 5b: `tools/src/cmd/loop/{settings,claude}.rs`.
    - `settings::materialise(main, state)`: from the `origin/main` blob, as in Stage isolation.
    - `trait Claude` and `ClaudeCli` on `proc::Supervisor`: `env_clear` plus the allowlist; it maps the envelope to `StageResult::{Ok(Value), Empty, Error, Refusal, UsageLimit, PermissionDenied(cmd), OverBudget, Timeout}`.
    - A lens result with an unknown category is an `Error`.

    Tests in `tools/tests/loop_claude.rs`:
    - every Task 1 claude fixture maps to its variant;
    - an argv golden per stage under `tools/tests/golden/loop/argv/`;
    - the child env is exactly the allowlist (no `AIOS_*`, `CLAUDECODE` or `SSH_AUTH_SOCK`);
    - an unset `CLAUDE_CONFIG_DIR` is exit 2;
    - the settings file equals the expected keys exactly;
    - a Supervisor timeout gives `Timeout`.
  - [x] 5c: Verify: `cargo test -p aios-tools --test loop_state --test loop_claude` passes; TG.

- [x] **Task 6: Gather, review, verify, verdict and the CLI** [FG]
  - [x] 6a: `tools/tests/common/loop_fake.rs` and `tools/tests/loop_fakes.rs`.
    - **`FAKE_CLAUDE`** (`#!/bin/sh`):
      - it finds its root from `$0`;
      - it logs argv, env and stdin under `<root>/claude-calls/<SEQ>/`;
      - it reads the `stage:` line at the top of each prompt and emits `<root>/claude/<stage>.json`;
      - its modes are `error`, `empty`, `sleep`, `usage-limit`, `refusal` and `over-budget`.
    - **Its commands equal `jail::FAKE_CLAUDE_TOOLS`.** If the fake needs fewer than the 8 listed, the same commit shrinks the list to what it runs (no-legacy). `jail.rs` is ordinary main code now.
    - **`run_loop(scenario) -> Outcome`:**
      - real git (a clone plus a bare origin, PR head `claude/demo` on `refs/pull/1/head`);
      - `FakeGh` with the Task 1 fixtures;
      - the jail;
      - `AIOS_LOOP_ANY_EXE=1` and `AIOS_LOOP_STATE_DIR=<root>/state`.
    - **The invariant `assert_read_only(&Outcome)`:** every fake-gh call passes `gh::is_read`, origin refs are unchanged, the clone's branches and HEAD are unchanged, and no `loop-pr*` worktree is left.
  - [x] 6b: `tools/src/cmd/loop/{gather,hive}.rs`.
    - **The change (`{{diff}}`, D3):** `git log --no-decorate --format='%h %s' <merge-base>..<head>`, then `git diff --no-ext-diff --no-textconv --no-renames <merge-base> <head>`, both run by gather in Rust. The stages have no shell, so this is the only git output they see; any other line of a file is read from the round worktree.
    - **`Facts`:** the changed paths, failing checks with log tails, docs-check new findings (with `plans-not-empty` split into `Facts.plan_present`), trusted threads, and untrusted threads inside a nonce fence.
    - **Lessons:** hive notes selected by `path_tags`, `status: final` first and then newest, capped; dropped notes are recorded.
    - **Gates:** `merge_ready::gates`.
    - **Prior findings:** dismissals from `findings.jsonl`.

    docs-check is a child of `current_exe()` (on Linux, `/proc/self/exe`, as in brief), run in the round worktree. Notes come from `origin/main` blobs, or from `--tree`.
  - [x] 6c: `tools/src/cmd/loop/review.rs`.
    - The 3 lenses run under `std::thread::scope`, then verify.
    - Model choice follows the config.
    - A fable error gets one opus retry, recorded as `fallback`. Any other stage failure gets one retry, then the PR is `incomplete`.
    - Deterministic findings are confirmed without verify.
    - Spend is summed against `per_pr_usd`; going over makes the PR `incomplete`, and no more stages run.
  - [x] 6d: `tools/src/cmd/loop/verdict.rs`. `ready` is as in Approach step 5. `classify(blockers)` splits brief's blocker texts:
    - **pending:** `N pending`; `mergeable: UNKNOWN`; `merge state: UNKNOWN`; `BLOCKED` or `UNSTABLE` only together with `N pending`; `merge state: BEHIND`.
    - **needs-human:** every other text, any gated path, any cross-check mismatch, and a review that is not `clean`.

    An unknown blocker text is needs-human (fail closed). Unit tests call `merge_ready::blockers` on crafted values, so the classes cover every text it can produce.
  - [x] 6e: `tools/src/cmd/loop/mod.rs` and `main.rs`.
    - **CLI:** `Command::Loop` (after `Hook`, before `Soak`) with the subcommand `review <PR>... [--tree DIR] [--budget-usd N]`.
    - **Output per PR:**

      ```
      pr #<n> @ <sha7>: clean|findings|incomplete (<k> confirmed, <p> pre-existing; $<cost>)
        - [<severity>] <file>:<line> <category>: <claim>
      merge: ready | pending: <reasons> | needs-human: <reasons>
      ```

      The PR's `report.json` and `report.md` paths are printed last.
    - **Exit codes:**
      - 0: every PR is `clean` and `ready`;
      - 1: any finding, `incomplete`, `pending`, `needs-human` or refusal;
      - 2: a usage or internal error;
      - 4: stopped (kill switch or quiet);
      - 129/130/131/143: a signal. The stage process group is stopped and the run directory is kept.

      The binary never exits 3.
  - [x] 6f: `tools/tests/loop_scenarios.rs`. Every scenario runs `assert_read_only`. The scenarios:
    1. clean PR, ready;
    2. one confirmed in-diff finding;
    3. a finding refuted by verify;
    4. a pre-existing finding (reported, does not block);
    5. a failing check is a deterministic finding, with its log tail in the lens facts;
    6. a trusted unresolved thread is confirmed;
    7. untrusted thread text appears only inside the fence;
    8. new docs drift is confirmed, and `plans-not-empty` is a note only;
    9. a gated `tools/` path makes the verdict needs-human;
    10. a cross-check mismatch is needs-human;
    11. pending checks give `pending`;
    12. a needs-human gate issue is needs-human;
    13. the `hard` label sends verify to fable;
    14. a fable error falls back to opus once;
    15. a lens errors twice and the PR is `incomplete`;
    16. the per-PR budget is exceeded and the PR is `incomplete`;
    17. the STOP file stops with zero claude calls;
    18. the `agent-stop` label is missing (404) and the run stops;
    19. another team's PR is refused with zero claude calls and no state;
    20. a fork PR is needs-human with zero claude calls;
    21. a release exe elsewhere without `--tree` exits 2;
    22. under `--tree`, prompts are read from the tree;
    23. SIGTERM during a lens gives 143 and leaves no live child process;
    24. a prior dismissal reaches the lens prompt;
    25. a kernel-core diff selects opus for correctness and security;
    26. two PRs in one run, in order.

    Goldens go under `tools/tests/golden/loop/`.
  - [x] 6g: Mutation checks, not committed:
    - replace `GhCli` with a raw `Command::new("gh")` → `loop_gh` and `assert_read_only` fail;
    - make `is_tools_input` case-sensitive → `loop_gate` fails;
    - classify `N pending` as needs-human → scenario 11 fails;
    - skip the kill switch → scenario 18 fails.

    The worker reports one failing `test result` line per mutation, and `git status --porcelain -- tools/src` is empty after it restores the code.
  - [x] 6h: Verify: `cargo test -p aios-tools --test loop_scenarios --test loop_fakes` passes; T(`--lib pr_loop::verdict`) ≥ 3; TG.

- [x] **Task 7: The shim's strict `loop)` arm** [FG: `.claude/hooks/aios`]
  - [x] 7a: Tests first, appended to `tools/tests/shim.rs`:
    - `loop_refuses_aios_tools_bin_with_exit_3`;
    - `loop_with_a_failing_stale_rebuild_exits_3`;
    - `loop_with_a_dirty_binary_exits_3_naming_the_cause`;
    - `loop_rebuilt_but_still_not_fresh_exits_3`;
    - `loop_with_a_missing_binary_and_failing_build_exits_3`;
    - `loop_with_a_fresh_binary_runs_it` (the variable `AIOS_SHIM_STALE` stays unset);
    - `loop_stale_rebuild_succeeds_then_runs`;
    - `a_subcommand_named_loopx_is_other`.

    Each exit-3 test also asserts an empty stdout and that the fake binary did not run.
  - [x] 7b: In the shim:
    - Before the `AIOS_TOOLS_BIN` branch: `[ "$sub" = loop ] && [ -n "${AIOS_TOOLS_BIN:-}" ] && stop "AIOS_TOOLS_BIN is not used for loop; …"`.
    - A last `loop)` arm in `case $sub`:
      - missing: `build_fg`, then `runnable`;
      - stale: `build_fg || exit 3` with `aios: rebuilding … failed; aios loop never runs a stale or unverified binary (run: just tools)`;
      - after any build, recompute `verdict` and exit 3 unless it is `fresh`;
      - `dirty-*`: exit 3, naming `dirty_cause`;
      - `fresh`: `exec "$bin" "$@"`.
    - The header gets an `aios loop` paragraph that names both residuals.
    - Verify: `cargo test -p aios-tools --test shim` passes; `sh -n .claude/hooks/aios`; `diff <(git show origin/main:.claude/hooks/aios | shellcheck -s sh -f gcc - | sed -E 's/.*(SC[0-9]+).*/\1/' | sort -u) <(shellcheck -s sh -f gcc .claude/hooks/aios | sed -E 's/.*(SC[0-9]+).*/\1/' | sort -u) | grep -c '^>'` prints `0` (no new shellcheck code); TG.

- [x] **Task 8: `/justin:review`, and docs** [FG for `.claude/**`]
  - [x] 8a: `.claude/skills/justin/skills/review/SKILL.md` (model-invocable).
    - It runs `.claude/hooks/aios loop review <PR…>` from the main checkout with the Bash tool's `run_in_background: true` (a run can take more than 10 minutes), then waits for the completion notification.
    - It reports each PR's block, and the report path.
    - **Exit handling:**
      - 3: show the shim message; never suggest `AIOS_TOOLS_BIN`;
      - 4: report the stop;
      - 129/130/131/143: "interrupted; rerun to review again", with no rerun on its own;
      - 2: show the message and stop.
    - **Stopping a running review:** the `STOP` file, the `agent-stop` label, or stopping the background task. Esc does not stop it.
    - It never claims a merge. It says the verdict is advisory and that the owner merges with `/merge-and-cleanup`.
    - It expects one permission prompt per run (D4: the settings `ask` rules on every `aios loop` spelling).
  - [x] 8b: Skill wiring, in one commit with 8a (`harness-tables`):
    - `.claude/skills/justin/.claude-plugin/plugin.json`: bump the version and list `/justin:review`;
    - `.claude/CLAUDE.md`: a Skills-table row and the layout skills line;
    - `start/SKILL.md`: propose `/justin:review <n>` for an open PR with unresolved threads or no review yet;
    - `team/SKILL.md` loop step 8: `gh pr ready <n>`, then `/justin:review <n>` (a report), then `/review-pr-comments`.
  - [x] 8c: Docs:
    - **`docs/project/agent-loop.md`:**
      - the session-skills table gains `/justin:review`;
      - "Where state lives" gains `aios-agent/loop/`;
      - the Stop paragraph explains the background review and its three stops, and drops "come with the loops" for the parts that now exist;
      - a short "PR loop (R2a)" section: the command, the outputs, the exit codes, and "advisory; R2b adds fixing".
    - **SPEC:**
      - the R2 row splits into R2a/R2b/R2c, with an R2a "as built" note;
      - the §1 Protection text names Option A's full list, still called the guarantee;
      - the §2 Freshness table gains an `aios loop` column;
      - the Open Questions on Option A/B and exit 3 are marked answered (#204, 2026-10-11), and OQ8 (gate widening) stays open for R2b/Stage 5.
    - **Rule 05:** `tools/src/cmd/loop/` and `scripts/agent/{prompts,loop-config.json}`.
    - **The developer guide:** an `aios loop review` subsection near §5.1. Counts wait for the ship pass.
  - [x] 8d: Verify:
    - DG;
    - `git -C <W> grep -c 'run_in_background' -- .claude/skills/justin/skills/review/SKILL.md` ≥ 1;
    - `git -C <W> grep -n '/justin:review' -- .claude/skills/justin/skills/start/SKILL.md .claude/skills/justin/skills/team/SKILL.md` prints ≥ 2 lines;
    - `jq -e '.description | contains("/justin:review")' <W>/.claude/skills/justin/.claude-plugin/plugin.json`.

- [ ] **Task 9: The real review run** [OWNER-run from a terminal; ≤ $5]
  - The lead builds the branch into the scratchpad: `cargo build --release --locked -p aios-tools --manifest-path <W>/Cargo.toml --target-dir <S>/r2a-target`.
  - The owner runs `<S>/r2a-target/release/aios loop review <PR> --tree <W>` from the main checkout, on two PRs:
    - #280, small (lockfile plus `renovate.json`);
    - #149, team-build's Phase 7 M26 PR, for real kernel code (kernel-core lens models).

    Before R2a merges, `origin/main` has no prompts, so `--tree` supplies them.
  - Verify:
    - `<S>/r2a-target/release/aios loop review <PR> --tree <W> > <S>/r2a-run-<PR>.txt; echo "rc=$?"` prints `rc=0` or `rc=1`, and `grep -c '^pr #<PR> @ ' <S>/r2a-run-<PR>.txt` prints `1`;
    - `ls "$(git -C <main> rev-parse --path-format=absolute --git-common-dir)"/aios-agent/loop/pr-<PR>/run-*/round-01-*.json | wc -l` prints `4` (three lenses and verify);
    - `gh api repos/{owner}/{repo}/pulls/<PR>` `updated_at` is unchanged across the run (no writes);
    - `git -C <main> worktree list | grep -c loop-pr` prints `0`.

    The lead pastes both report blocks into the PR body.

- [ ] **Task 10: Simplify, audit, ship pass, ready, hand-off** [lead + simplifier + worker]
  - [ ] 10a: The simplifier over `merge-base..HEAD`, then Placement.
  - [ ] 10b: `/audit-loop <W>` (tools mode), at most 4 rounds, under this plan's threat model. Findings outside it are residuals, not new mechanism.
  - [ ] 10c [FG]: The ship pass, as one worker commit:
    - the `.claude/CLAUDE.md` Workspace Layout (`tools/src/inputs.rs`, `tools/src/cmd/loop/`, `scripts/agent/` prompts and config);
    - the developer-guide counts;
    - distil this plan into `docs/knowledge/lessons/<date>-jl-r2a-loop-review.md` (the probe answers, D1/D2/`ROUND_ROOT`, the read-only invariant), then `git rm` this plan.

    Verify: `just docs-check` prints `No new drift since baseline.`; TG.
  - [ ] 10d: `gh pr ready`, then `/review-pr-comments`, then the hand-off: the PR URL, `gh pr checks`, and "run `/merge-and-cleanup` (no argument; #261)". After the merge, the lead deletes `claude/justin-review-merge-loop` (its content is merged) and starts R2b's plan.

## Code Structure Decisions

- **The module is `cmd/loop/` declared as `pr_loop`.** Files: `mod`, `config`, `schema`, `git`, `gate`, `github`, `state`, `settings`, `claude`, `gather`, `hive`, `review`, `verdict`. The directory stays under the existing Edit/Write ask rule.
- **Read-only by construction.** There is no gh write type in R2a (R2b adds `GhWrite` and `assert_allowed`). Every gh call goes through `GhCli` and `is_read`. Stages have no write tool. The only local writes are the state dir and the loop's own round worktree.
- **Hardened git for the loop**, on `soak::host::git()`. The inherit-policy `tools/src/git.rs` is for brief and checkpoint parity.
- **One round, no resume.** A rerun is a new run directory. `findings.jsonl` carries dismissals forward. R2b adds `state.json` and resume with the multi-round state machine.
- **The verdict is brief's verdict plus the loop's extras**, with a pending vs needs-human split, so R2b's label logic has a tested classifier.
- **No new dependencies.** Time comes from `SystemTime`.
- **No `just loop` recipe.** `Bash(just *)` is allowed unprompted, and R2b's write side must prompt.
- **No settings change in R2a.** It makes no git or GitHub writes, so rule 03's compiled-writer bullet does not apply yet. The guard already asks on the plain shim spelling. R2b adds the loop asks (OLD Task 20) before the first write path exists.

## Dependencies & Risks

- **Depends on:**
  - main 4cbb812: R3's `gh.rs`, `merge_ready.rs` and jail;
  - the `agent-stop` and `hard` labels (exist);
  - the owner for Task 1 (≤ $0.50) and Task 9 (≤ $5).
- **Risks:**
  1. A stage escapes isolation through user plugins, hooks or MCP. Mitigations: Task 1c's probe, the minimal pinned settings, the exact `--tools`, and the empty MCP config.
  2. A false `clean` from an injected PR: accepted at Stage 0. The deterministic verdict still blocks gated or red PRs.
  3. Runaway cost. Mitigations: per-call and per-PR caps, one round, and the probe total.
  4. The wrong account. Mitigation: `CLAUDE_CONFIG_DIR` is required, and the env is an allowlist.
  5. CI reaching a real `gh` or `claude`. Mitigations: the jail (no `/usr/bin` or `/bin`), the `GH_*` and `CLAUDE*` strip, and `assert_all_matched`.
  6. CLI drift. Re-run Task 1 after a CLI update; the fixtures record the version.
  7. Many owner prompts (protected paths). This is intended.
  8. Plan creep in audit rounds. The threat model is fixed above; out-of-model findings become residuals in the PR body, and the round cap is 4.

## Owner Questions

None open for R2a. Deferred to R2b or later:
- OQ8: widening the gate, and the shim race;
- the exact loop settings asks;
- kernel-area fixes (OQ13).

## Issues Encountered

- `--output-format json` prints an event array on some calls and one result object on others; the stage parser accepts both and uses the last `type == "result"` element (claude 2.1.311).
- `--max-budget-usd` is checked after spending: a $0.0001 cap still billed $0.00996, so the per-PR cap can be overshot by one call.
- `--max-turns` is not in the CLI's help, so it is never used.
- `dontAsk` still auto-allows read-only commands (`true`).
- Copilot has not reviewed recent PRs, so trusted threads come mostly from the owner.
- `gh pr checks --json` exits 0 even when a check fails (gh 2.103.3, checked by the lead on #279), so `github::pr_checks` treats a non-zero exit as an error.
- Task 3a's `per_call_usd` must cover a stage-shaped call (about $0.04): set it to `0.15`, and keep `per_pr_usd` 5.
- Task 6: an earlier writer's uncommitted draft (based before D3) was reused for `gather`, `hive`, `review`, `verdict` and the `Interrupts` wiring in `claude.rs`. Dropped from it: prompt `version: 2` and its "fence every thread" wording (the committed prompts stay `version: 1` and D3's `{{diff}}`); `gate.rs`'s non-ASCII fail-closed rule and `display_path`/`gate_of`/`is_gated` (Task 4 scope; the gate is `is_tools_input` as the plan says; a Unicode case-fold gap on APFS remains a residual, a candidate for R2b); its `DocsCheck` trait and `AIOS_LOOP_DOCS_CHECK` hook (the scenarios run the real `aios docs-check` in a tiny repo).
- Task 6: `Facts.render` puts a trusted author's comment text in the facts; only CI log tails and the comments of untrusted authors are inside the nonce fence (as Approach step 2 says). The verify prompt gets the lens findings inside the fence.
- Task 6: review-thread trust is judged per comment, not per thread (an automated security review of 32d62e8 found that anyone can reply to a thread a trusted author opened, and the reply reached the facts outside the fence). A trusted thread's untrusted replies are rendered inside the fence with the thread's id and place; scenario 7b and a `gather` unit test pin it.
- Task 6: a change over 192 KiB (commit list plus diff) is cut, and the cut makes the review `incomplete` (a lens that never saw the whole diff cannot say `clean`).
- Task 6: `origin/main` is as of the last fetch: the loop fetches only `pull/<n>/head` (a write to FETCH_HEAD only), so the merge-base and the prompt blobs are as fresh as the caller's fetch.

## Decisions Made

- 2026-10-27 (owner): three PRs, R2a/R2b/R2c, cut as in Context.
- 2026-10-27 (owner): fold `claude/justin-review-merge-loop` into R2a (Task 0c).
- The 2026-10-11 decisions under Binding decisions.
- **D1 (final):** --restricted --disable-slash-commands, with --settings <stage settings>, --tools Read,Grep,Glob,Bash and --strict-mcp-config --mcp-config '{"mcpServers":{}}'. The project hooks and CLAUDE.md do not load; only Claude Code's three built-in plugins load; no MCP and no skills (Task 1c, 2026-10-27).
- **D2 (final):** --permission-mode dontAsk. Writes outside --allowedTools are denied without a prompt and listed in permission_denials; Claude Code's built-in read-only auto-allow still runs read-only commands such as true (Task 1b).
- **D3 (owner, 2026-10-28, amends D1):** the stage toolset is `Read,Grep,Glob`, without Bash. An automated security review found that the allowed `git diff`, `git log` and `git show` take `--output=<path>`, which writes a file. main's deny list covers `git diff` and `git log` with `--output`, and `git show` is only in `ask` (denied under `dontAsk`), but other write or exec forms of those commands would each need their own rule. Gather renders the commit list and the diff into `{{diff}}` instead. Task 1c's `init_restricted.json` was recorded with Bash in the toolset; Task 9's real run is the first stage call with the D3 toolset.
- **D4 (owner, 2026-10-29):** the settings `ask` on every `aios loop` spelling lands in R2a (the nine rules), because a model-invoked `/justin:review` could otherwise spend up to `per_pr_usd` per PR without a prompt (found in Task 8: `Bash(.claude/hooks/aios *)` is allowed and nothing asked).
- **ROUND_ROOT (final):** `<main>/.claude/worktrees`. A stage call there with real-shaped stage settings ran git diff and Read; the guard gives git diff/log/show there no decision (Task 1c).
- **Copilot login (Task 1d):** copilot-pull-request-reviewer (GraphQL thread authors and gh reviews; REST shows Copilot). trusted_authors = juslee, copilot-pull-request-reviewer. Copilot's last review was on #144.
- **Task 6 (worker, 2026-10-28):**
  - The verdict reads the PR again after the review (`pr_view`, then the kill switch once more): a changed head is `pending`, and the blockers are those of the PR as it is at the end, not at the start of a round that can take minutes. A stop at that point is exit 4 with no verdict.
  - The stdout block adds `  incomplete: <why>` and `  note: <what could not be read>` lines, and `  - ... (pre-existing)` lines after the confirmed ones; `report: <path>` lines (json, md) come last.
  - An internal error of one PR (a gh read fails, the fetch fails) ends the run with exit 2; a refusal (other team, fork, a held lock) goes on to the next PR and exits 1; a stop or a signal ends the run at once.
  - `jail::FAKE_CLAUDE_TOOLS` shrank to `cat env mkdir sleep` and `FAKE_TOOLS` to 8 entries, to what the fakes run (no-legacy); the loop scenarios link `git` and `kill` into their jail themselves.
  - `assert_read_only` also checks each gh call's environment (`LOOP_GH_ENV`) and working directory, and the claude toolset; `loop_gh.rs` gains `no_loop_source_spawns_gh_itself`. Both exist so the 6g mutation that swaps `GhCli` for a raw `Command::new("gh")` fails a test.
  - A 27th scenario (not in the list): a `mode=quiet` QEMU lock stops the run with exit 4.
  - `cargo test --lib pr_loop::verdict` matches the real module path `cmd::pr_loop::verdict::tests::*` by substring.
- **Probe cost (Task 1):** $0.2797 of the $0.50 cap; a full-toolset fresh call costs $0.133, a small-form call $0.005-0.025, a stage-shaped call about $0.03-0.04.

## Lessons Learned

(filled during implementation)
