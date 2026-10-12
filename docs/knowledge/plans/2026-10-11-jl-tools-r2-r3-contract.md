---
author: jl + claude
date: 2026-10-11
tags: [tooling]
status: final
---

# Tools R2/R3 contract: shared modules, harness, shim, files and merge order

Both plans cite this file and do not restate it:
- R2: `2026-10-11-jl-tools-r2-loop.md` (branch `claude/tools-r2-loop`, worktree `.claude/worktrees/tools-r2`);
- R3: `2026-10-11-jl-tools-r3-scripts.md` (branch `claude/tools-r3-scripts`, worktree `.claude/worktrees/tools-r3`).

Each plan's coordination section becomes a link to this file plus a short summary. Where this file and a plan disagree, this file wins. The plan is then corrected in its Task 0.

A change to anything here is a contract amendment. Both branches belong to team-build, so the same lead makes the amendment: it edits this file and both plans in one worker commit, and records it in its handoff file. No `HEADS-UP` message is sent, because there is no other lead to send it to.

Revised 2026-10-11 after the critic round. §13 lists the points that were rejected and why. §14 lists the merged amendments A1-A12 that both plans cite.

## 0. Basis

- **Owner decisions of 2026-10-11** (final; implemented here, never re-asked). They are numbered as the owner gave them:
  1. Option A.
  2. Loop exit 3, with full strictness.
  3. R3 owns `gh.rs`, `git.rs`, `merge_ready.rs` and the fake-gh harness, and merges first.
  4. Pause asks once per pause, through owner settings asks, with no guard deny.
  5. checkpoint exits 3 only when a stale rebuild fails.
  6. R2's Stage-0 scope.
  7. R3 keeps the history-oracle differentials.
  8. R3 hardening is H1 only.
  9. `AIOS_SHIM_STALE`, set and cleared.
  10. The `agent-stop` and `hard` labels exist.
- **Two names for two trees.** Neither is ever written by a lead (rule 11).
  - **`W234`** = `.claude/worktrees/harness-teams` @ b47c756 (#234 head; merge base with main ac6d26e). Every `W234 L<n>` anchor first cited this tree; since R3's Task 0a it cites `origin/main` 2d4a9f7, #234's squash merge (A9). The R2 plan calls it `WT`.
  - **`<BW>`** = the branch's own worktree: `.claude/worktrees/tools-r2` for R2, `.claude/worktrees/tools-r3` for R3. Every verify command in §1, §9 and §10 runs against `<BW>`, never against `W234`, which holds neither branch's changes. The plans' `<W>` means `<BW>`.
- **Repo state checked for this contract:**
  - `main` @ 030b3b7.
  - Both branches start from `origin/main` after #234 merges. #234 merged main before it landed, so the merged shim has `shared` in `inputs=` (L338). R3's Task 0a re-read every `W234 L<n>` on 2d4a9f7, and this file cites those lines; R2's Task 0 re-reads its own.
- **Verified on 2026-10-11 for this contract:**
  - The guard probe in §9: the W234 guard was piped PreToolUse payloads, and the guard writes no files.
  - `gh label list` includes `agent-stop`, `hard` and `needs-human`.
  - `gh pr list --label needs-human --state open` returns `[]`, so the D10 blocker (§3) changes no brief output today.
  - brief.sh and checkpoint.sh both `cd "$ROOT"` (the toplevel; W234 brief L52-56, checkpoint L75-79) before every gh call.
  - brief.sh sets no `GH_*`. checkpoint.sh sets `GH_PROMPT_DISABLED=1` only on `gh pr list --head` (L219).
  - brief's needs-human `refs` scans **every** `#N` in an issue's title and body (W234 brief L204-210): a title `#N` gives `gated by`, a body-only `#N` gives `named in needs-human`. merge-and-cleanup (W234 SKILL.md L55) stops on both. The existing gate #165 names one PR only.
  - checkpoint.sh calls `paste` (L196, L223, L250).
  - W234's qemu-lock.sh calls `ps` (L112, L260, L309, L356), `pgrep` (L87, L249), `sleep` (L274, L367) and `mv` (L174, L180, L344). Its `kill` calls are sh builtins. No line of brief.sh, checkpoint.sh or qemu-lock.sh calls `flock`, `lockf`, `uniq`, `xargs` or `readlink` (qemu-lock.sh L71 names `flock` in a comment only). The three scripts send stderr to `/dev/null` 39, 13 and 19 times.
  - The CI job "Tools (host)" runs on ubuntu-26.04 with `fetch-depth: 0`, has no python step, and sets no `GH_TOKEN`.
  - docs-check `harness-tables` (main `tools/src/cmd/docs_check/checks/harness.rs`) compares the CLAUDE.md Skills table and layout skills list with every tracked `.claude/skills/**/SKILL.md`, plugin sub-skills included, in every `just docs-check`. #234 added its `team` skill and the Skills-table row in one commit (77067cb, "T11 — skills and the Skills table").

## 1. Merge order and sequencing (decision 3)

**Order: R3 merges first, then R2.**
- R3's PR amends the spec Order:
  - the §4 line (L194) becomes `R1 → R4 → R3 → R2 → R5 → R5b`;
  - the Context row (L41) is reworded the same way.
- **Mechanical gate.** Once both PRs exist, the lead files a `needs-human` issue that names **only R2** with a `#`:
  - title: `Gate: merge PR #<R2> only after the R3 scripts PR (claude/tools-r3-scripts) merges`;
  - body: names R3 by its branch or its `https://github.com/<owner>/<repo>/pull/<n>` URL, never as `#<n>` (brief's `refs` would gate or name R3 too, and the owner closes the issue only after R3 merges, so R3 could never merge).
  - Verify, right after filing, from the main checkout: `bash scripts/agent/brief.sh --no-fetch` (still on main until R3 merges) shows R2's line as `merge-ready: no (gated by #<gate>…)`, and R3's `merge-ready:` line names neither `gated by #<gate>` nor `named in needs-human #<gate>`.
  - brief then shows R2 as gated and `/merge-and-cleanup` refuses it. The owner closes the issue when R3 merges.

**R3** builds its tasks in order. One change to that order: its first code commits are the shared API (§2-§5). This is the **API-freeze commit**, recorded as `R3_API_FREEZE=<sha>` in both plans' Decisions Made.
- The freeze holds **complete, tested implementations** of §2-§5, not stubs (rule 01): at `R3_API_FREEZE`, `cargo test -p aios-tools` passes, including R3's unit tests for `gh`, `git` and `merge_ready` and the `agent_support.rs` self-tests (§5).
- After the freeze, R3 may change function bodies and add items, but no public signature listed here changes without an amendment.

**R2** works in two phases.
- **Phase A** (in parallel with R3): R2 writes only files that R3 never touches:
  - `tools/src/cmd/loop/**`, `tools/src/inputs.rs` and the `soak/host.rs` move;
  - `tools/tests/loop_*.rs` and `tools/tests/common/loop_fake.rs`;
  - `scripts/agent/{prompts,evals,loop-config.json}`;
  - the LOOP doc copy;
  - the one-line hunks in `main.rs`, `cmd/mod.rs`, `lib.rs` and `tests/common/mod.rs`.

  Phase A creates **no** `.claude/skills/**/SKILL.md`: a tracked skill without its CLAUDE.md rows fails `harness-tables` in every later per-task gate. The review and merge skills are written in Phase B Task 19, together with their CLAUDE.md rows (§8).

  So that R2 compiles before R3 lands, one Phase-A worker commit vendors R3's frozen files byte for byte: `git checkout <R3_API_FREEZE> -- tools/src/gh.rs tools/src/git.rs tools/src/merge_ready.rs tools/tests/common/fake_gh.rs tools/tests/common/jail.rs`, plus their `pub mod` lines. Its subject is `Tools R2 Task 4: vendor R3's frozen shared modules (replaced by main's at convergence)`. These copies never reach main.
  - **Re-vendoring.** R2 may pick up R3's later body fixes before convergence with the same command from a later R3 commit, subject `Tools R2 Task 4: re-vendor R3's shared modules at <sha> (replaced by main's at convergence)`, recorded as `R3_API_FREEZE_<N>=<sha>` in R2's Decisions Made. Signatures still change only by amendment.
- **Convergence** (R2 Task 23, now the first task of Phase B). It runs as soon as R3 is on `origin/main`.
  - A worker in its temporary worktree, reset to R2's tip, runs `git merge origin/main`. It is a merge, never a rebase (rule 11).
  - For every path R3 owns (§8), it takes `origin/main`'s version verbatim (`git checkout origin/main -- <path>`).
  - It resolves the one-line hunks as unions in alphabetical order (§8), fixes R2's call sites, and commits the merge.
  - The lead range-checks the result and fast-forwards (rule 11 Placement; the oldest commit's first parent is the tip).
  - Verify:
    - `git -C <BW> diff origin/main -- tools/src/gh.rs tools/src/git.rs tools/src/merge_ready.rs tools/tests/common/fake_gh.rs tools/tests/common/jail.rs` is empty, or shows only additive R2 lines that this contract lists (none today);
    - the per-task gate (TG) passes.
- **Phase B** (after convergence, in this order; it follows `/justin:team`'s simplify → audit → ship pass → ready):
  1. Task 3: the shim and `shim.rs` (§6);
  2. the spec edits (§8);
  3. Task 19: the review and merge skills with their CLAUDE.md rows, `plugin.json`, and the `/review-pr-comments` narrowing (§8);
  4. Task 21: docs and rules;
  5. Task 20: the owner's settings change (§11);
  6. Task 24a: the simplifier;
  7. Task 18: scores (§7);
  8. Task 22: the dry run;
  9. Task 24d: `/audit-loop` until a clean round. An audit fix that touches an eval input is re-scored (owner-attended) before the ship pass (§7);
  10. Task 24b with 24c: the ship pass (inventory sections, distil, `git rm` the plan), the **last** commit before `gh pr ready`;
  11. `gh pr ready`, then 24e-24g.

  This makes every file both branches edit a **sequenced edit, not a merge conflict**. The only conflicts left are the trivial one-line unions above.

**If R3 slips:** R2 stops at the end of Phase A and waits. R2 never merges first: the order is an owner decision.

## 2. `tools/src/gh.rs` (R3-owned; read-only runner)

**Choice:** R3's outcome-enum and runner-trait shape, renamed and extended. R2's `trait Gh { read -> Result; json }` with a forced env is rejected:
- a `Result` loses the Missing vs Failed distinction behind brief's `gh is not installed` line and its first-stderr-line notices (D2 keeps the former);
- a forced `GH_PROMPT_DISABLED` would put a value into every call's fake-gh env log, which the oracle (brief.sh sets nothing) does not, and so break R3's differential.

The name `gh::Gh` is retired, so neither plan uses it.

```rust
//! tools/src/gh.rs: run one `gh` READ. Writes never go through this module.

/// What one `gh` read returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhOutcome {
    /// Spawn failed with io::ErrorKind::NotFound: no `gh` on PATH.
    Missing,
    /// `gh` exited non-zero (`status: Some(code)`), was killed by a signal, or
    /// failed to spawn for any reason other than NotFound (`status: None`, stderr
    /// holds "gh: <io error>").
    Failed { status: Option<i32>, stderr: Vec<u8> },
    /// `is_read` rejected the argv; nothing was spawned.
    Refused { reason: &'static str },
    /// Exit 0: stdout exactly as gh wrote it (stderr dropped).
    Ok(Vec<u8>),
}
impl GhOutcome {
    /// `head -n 1` of stderr for Failed (without its newline); empty otherwise.
    pub fn first_stderr_line(&self) -> &[u8];
}

/// Changes to the inherited environment, applied in order: removals, then sets.
/// Every caller's values are literals, so each caller's env is a `const`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GhEnv { pub set: &'static [(&'static str, &'static str)], pub remove: &'static [&'static str] }
impl GhEnv {
    /// Empty: the scripts' behaviour.
    pub const INHERIT: GhEnv = GhEnv { set: &[], remove: &[] };
    /// The one place a GhEnv reaches a Command: `env_remove` each of `remove`, then `env` each of `set`.
    pub fn apply<'c>(&self, cmd: &'c mut std::process::Command) -> &'c mut std::process::Command;
}

pub trait GhRunner { fn run(&self, args: &[&str]) -> GhOutcome; }

/// The real runner: `gh <args>` in `cwd` with `env`, stdin null, stdout and stderr captured.
pub struct GhCli { pub cwd: std::path::PathBuf, pub env: GhEnv }
impl GhRunner for GhCli { /* run_program(OsStr::new("gh"), args, &self.cwd, &self.env) */ }

/// The seam: GhCli::run is exactly this with program = "gh". It calls
/// `is_read(args)` first and returns `Refused` without spawning when that fails.
/// Its Command gets its environment only through `env.apply`.
pub fn run_program(program: &std::ffi::OsStr, args: &[&str], cwd: &std::path::Path, env: &GhEnv) -> GhOutcome;

/// Ok(()) when `args` is a read; Err(reason) otherwise. An allowlist (fail closed):
/// - `api <endpoint> …`, where <endpoint> is args[1] and does not start with `-`. It is refused when it has:
///     `-X`/`--method` (any spelling) with a value other than GET; `--input`;
///     any field flag (`-f`, `-F`, `--field`, `--raw-field`, attached or `=` forms)
///     unless <endpoint> == "graphql"; for graphql, any field value starting with `@`,
///     or a `query=` value containing the word `mutation` (ASCII case-insensitive).
///     (A REST `gh api` with a field is a POST.)
/// - `pr list|view|checks|diff|status`, `issue list|view|status`, `run list|view`,
///   `label list`, `repo view`, `search issues|prs`.
/// - Everything else is refused (`pr merge|edit|comment|ready|close|create`, `issue create|edit|close|comment`,
///   `label create|edit|delete`, `run rerun|cancel|delete`, `repo edit`, `auth`, `secret`, `workflow run`, …).
pub fn is_read(args: &[&str]) -> Result<(), &'static str>;
```

**Environment (decided once).**
- `gh.rs` forces nothing. The caller passes a `GhEnv`:
  - brief uses `GhEnv::INHERIT`;
  - checkpoint uses a `const` `GhEnv { set: &[("GH_PROMPT_DISABLED", "1")], remove: &[] }` in `cmd/checkpoint/`, and only on its `pr list --head` call, as the oracle does;
  - R2 uses `const LOOP_GH_ENV: GhEnv` in `cmd::pr_loop::github`: set `GH_PROMPT_DISABLED=1`, `GH_NO_UPDATE_NOTIFIER=1`, `NO_COLOR=1`; remove `GH_REPO`, `GH_HOST`.
- Every gh `Command` in the crate (`run_program`, and R2's watch and write Commands in `cmd/loop/github.rs`) gets its env through `GhEnv::apply`. No other code calls `env`/`env_remove` on a gh Command.
- Why R2 needs nothing more:
  - stdin is null and stdout/stderr are pipes, so gh already disables prompts, colour, the pager, the spinner and update notices. R2's sets are defence in depth, not behaviour.
  - `LC_ALL=C` is dropped for gh: gh (Go) and its `--jq` (gojq) do not read the locale.
  - Removing `GH_REPO`/`GH_HOST` makes the loop's `{owner}/{repo}` and `pr` commands resolve from the repository it runs in, never from a stray shell variable.

**cwd.** gh resolves `{owner}`/`{repo}` placeholders, `pr list --head` and the default repo from the git remotes of its working directory.
- `GhCli.cwd` is:
  - brief and checkpoint: the toplevel of the current checkout (`git rev-parse --show-toplevel`), as the scripts `cd` there;
  - R2: the main checkout (`dirname(git rev-parse --git-common-dir)`). Never a round worktree: those share the config, but their content is the PR's.
- The fake gh logs `pwd -P` per call (§5), so R3's differential pins the cwd to the oracle's.

**Read vs write split.**
- R3 has only reads.
- R2's writes (`GhWrite`, `assert_allowed`, bodies through 0600 tmpfiles) and its supervised long read `gh pr checks <n> --watch` (run under `proc::Supervisor`, so not through `run_program`) live in **`tools/src/cmd/loop/github.rs`**, inside the protected glob (A12).
  - The watch covers **every** check, required or not, and never passes `--required`: the merge verdict (§3) counts every check in `statusCheckRollup`, so a watch over required checks alone would leave a slower non-required check (claude-review) pending. It is bounded by `ci_wait_minutes` (R2 D21; R2's `checks_reads_never_pass_required` asserts that no typed read or watch argv contains `--required`).
  - That module builds its own `Command::new("gh")`, applies `LOOP_GH_ENV.apply(&mut cmd)` and the cwd rule above.
  - It calls `gh::is_read` for the watch and `assert_allowed` for every write.
- R2's JSON helper (`fn json(r: &dyn GhRunner, args) -> anyhow::Result<Value>`) and its typed reads (`pr_view`, `viewer_login`, `pr_checks` over every check, `review_threads` paging, `issues_labelled`, `label_exists`, `run_log_failed`, and the needs-human dedup's `issue list --search`) are also in `cmd/loop/github.rs`, on top of `GhRunner`. There is no `rate_remaining`: nothing consumes it (A12).
- R2's unit-test seam is a fake `impl GhRunner`.
- Crate-wide verify (both final gates): `git grep -n 'Command::new("gh")' -- tools/src` lists only `tools/src/gh.rs` and `tools/src/cmd/loop/github.rs`.

**R3 unit tests:**
- `run_program_missing_is_missing`;
- `nonzero_exit_is_failed_with_stderr`;
- `refused_argv_spawns_nothing` (the program is a `#!/bin/sh` that touches a marker, and the marker stays absent);
- `env_set_and_remove_applied`. It uses non-`GH_` names (`AIOS_GHENV_TEST_SET`, `AIOS_GHENV_TEST_REMOVE`), so `GH_PROMPT_DISABLED` never appears in `gh.rs`;
- `is_read_table`. Each brief and checkpoint argv passes. These fail: `pr merge 1`, `api -X POST x`, `api repos/x/y -f a=b`, `api graphql -f query=mutation{…}`, `api graphql -F query=@q`, `issue create`, `label create`.

**R2 unit tests:**
- `loop_reads_pass_gh_is_read`, a table of every typed-read argv. R2 does not edit R3's table.
- `gh_write_argv_never_merges`: builds the argv of every `GhWrite` variant (an exhaustive `match` inside the test, so a new variant does not compile until it is listed) and asserts that no argv contains the words `merge`, `--auto` or `--admin`, or the path parts `statuses`, `rulesets` or `git/refs`.

**R2 Task 23c verify, rewritten:** the old `fn json(&self` and `GH_PROMPT_DISABLED`-only-in-gh.rs checks are replaced by:
- the `Command::new("gh")` grep above;
- `git -C <BW> grep -n 'GH_PROMPT_DISABLED' -- tools/src` lists only files under `tools/src/cmd/checkpoint/` and `tools/src/cmd/loop/github.rs`.

**R2 Task 15e verify, rewritten:** the grep `git grep -nE '"pr", *"merge"|--auto' -- tools/src` is dropped. It can never print nothing: R3's `is_read_table` holds `pr merge 1`, `is_read`'s doc comment lists `pr merge`, and R2's own deny table names `--auto`. It is replaced by `gh_write_argv_never_merges` (above). S27 and the argv-log invariant (no `pr merge`, `--auto` or `--admin` in any scenario's log) stay as the behavioural check.

## 3. `tools/src/merge_ready.rs` (R3-owned): one verdict for brief, `/justin:start`, `/merge-and-cleanup` and the loop

**API.** Pure functions on `serde_json::Value`, with no I/O. They mirror brief.sh's jq (W234 L201-241).

```rust
/// brief's `gh pr list --json` field list, byte for byte (W234 L198). R2's `pr view --json` is this
/// plus `,` and its own extra fields, so `blockers` sees the same shape from both.
pub const PR_FIELDS: &str = "number,title,headRefName,isDraft,mergeable,mergeStateStatus,reviewDecision,statusCheckRollup,author,labels";
/// brief's needs-human issue query argv (W234 L166), shared by R2's merge verdict.
pub const NEEDS_HUMAN_ISSUES_ARGS: [&str; 10] =
    ["issue", "list", "--state", "open", "--label", "needs-human", "--limit", "1000", "--json", "number,title,body,updatedAt"];

pub fn clean(v: &Value) -> String;                          // jq tostring + [[:cntrl:]] → "?"
pub fn refs(text: &str) -> Vec<u64>;                         // scan("#([0-9]+)")|tonumber|unique (D4: exact u64)
pub fn alt<'a>(v: &'a Value, keys: &[&str]) -> &'a Value;    // jq `//`: falls through on null and false only
pub fn check_name(c: &Value) -> String;                      // .name // .context // "?"
pub enum CheckState { Pass, Fail, Pending }
pub fn check_state(c: &Value) -> CheckState;                 // brief's PASS and FAIL sets; "" is Pending
pub fn gates(pr: u64, issues: &[Value]) -> (Vec<u64>, Vec<u64>); // (title gates, body-only names)
pub fn labelled_needs_human(pr: &Value) -> bool;             // D10: .labels[].name == "needs-human"
pub fn blockers(pr: &Value, issues: Option<&[Value]>, open_threads: Option<u64>) -> Vec<String>;
pub fn pr_lines(pr: &Value, issues: Option<&[Value]>, open_threads: Option<u64>) -> [String; 3];
```

**Module doc.** It states what the module is and where it came from, in the R4 provenance form (main `tools/src/cmd/soak/classify.rs` L1-2):
- "The merge-ready verdict: a port of the jq program in the former `scripts/agent/brief.sh` (blob at `<BRIEF_ORACLE>`, L201-241). brief and R2's loop call this; nothing reimplements it."
- Later docs and comments cite `merge_ready::refs` and the other functions, not the script. A provenance sentence of the form `former scripts/agent/<x>.sh (blob at <sha>…)` is the one allowed mention (R3 Task 10f, below).

**R3 Task 10f, amended (decision 7 and the R4 precedent).** The switch-over grep becomes:
- `git -C <BW> grep -n -e 'brief\.sh' -e 'checkpoint\.sh' -- . ':!docs/knowledge/plans' ':!docs/knowledge/lessons' ':!docs/knowledge/research' ':!docs/knowledge/decisions' ':!docs/knowledge/discussions/2026-09-22-jl-rust-agent-tools.md' ':!tools/tests/common/agent.rs' | grep -vE 'former `?scripts/agent/(brief|checkpoint)\.sh`? \(blob at `?[0-9a-f]{7,40}'` prints nothing.
  - `tools/tests/common/agent.rs` is the history-oracle loader. It must name both paths to read them with `git cat-file blob <sha>:<path>`, so it is excluded, not obfuscated.
- `git -C <BW> grep -n -e 'brief\.sh' -e 'checkpoint\.sh' -- .claude` prints nothing: skills, settings, rules, hooks and guard tests never name the scripts, not even as provenance.
- **Port items the grep would hit on main today**, each handled by R3:
  - `tools/tests/soak_interleave.rs` L25, L739, L749: the test runs `bash ../scripts/agent/brief.sh --no-fetch` from disk, which breaks when the script is deleted. It switches to `env!("CARGO_BIN_EXE_aios") brief --no-fetch` with the same fake gh and PATH, and its module doc (L25) names `aios brief`. The oracle's Soak rendering stays pinned by the brief scenario `soak_interleaved`.
  - `tools/src/cmd/docs_check/mod.rs` L187 and `checks/milestones.rs` L173: already in R3's plan (Task 10, Comments).
  - `tools/tests/shim.rs` L623: §6.

**The verdict's inputs** (all four are required for a `yes`):
1. **Checks rollup:** `statusCheckRollup`, every check (CheckRun and StatusContext). At least one check must exist, and all must Pass. Required and non-required count the same, so a red Docs or claude-review check blocks.
2. **Review threads:** `open_threads`, the count of unresolved threads by every author, trusted or not. `None` means unknown, which blocks.
   - brief passes the GraphQL count (first 100 threads, newest 50 PRs).
   - R2 passes its own fully paged count.
3. **needs-human issues naming the PR:**
   - a `#<n>` in the title gates it (`gated by #N`);
   - a `#<n>` only in the body names it (`named in needs-human #N`);
   - an unreadable issue list (`issues = None`) gives `needs-human gates unknown`.
4. **The PR's own `needs-human` label** (new, D10): the blocker text `labelled needs-human`.
   - It goes in position 4: after `named in needs-human …`, before `needs-human gates unknown`. The other 11 blockers keep their texts and order.
   - It reads the `labels` field that brief already requests, so no gh argv changes.

Plus GitHub's own state: not a draft, `mergeable == MERGEABLE`, merge state `CLEAN` (or `DRAFT`), and no `CHANGES_REQUESTED`.

**The critics' finding, resolved.**
- R2's loop records a needs-human outcome as the `needs-human` **label** plus one deduplicated **comment** on the PR, as LOOP L149 says, with no commit status (decision 6). It opens **no** gate issue: that would be a second mechanism for the same fact (no-legacy).
- brief, and so `/justin:start` and `/merge-and-cleanup`, now see that label through blocker 4.
- R2's "stricter gate-issue check" and its `gate_refs` name are dropped. brief.sh already blocks on a body mention, so R2's check was not stricter.

**Who implements what:**

| Part | Plan |
| --- | --- |
| `merge_ready.rs` in full, including `labelled_needs_human`, `PR_FIELDS` and `NEEDS_HUMAN_ISSUES_ARGS`; brief renders through `pr_lines` | R3 (Task 5; brief Task 7 uses the consts) |
| **D10** (the brief output changes for a labelled PR). R3's divergence list gains D10 = the `labelled needs-human` blocker. Pinned by brief scenario `pr_labelled_needs_human` (an aios-only golden, listed in `DIVERGENT_SCENARIOS` with reason "D10"; excluded from the oracle differential), the merge_ready unit row `labelled_needs_human_blocks`, and `skill_contract.rs` running merge-and-cleanup's awk on that scenario. Every other brief fixture carries no `needs-human` PR label; a harness assertion keeps them differential | R3 |
| `/merge-and-cleanup` SKILL.md L55: `labelled needs-human` is a hard stop like `gated by`. Report the loop's needs-human comment and stop; the owner removes the label after deciding (`gh pr edit <n> --remove-label needs-human`). Also L52 and L56 (`just brief --no-fetch`, "`aios brief`'s `--limit 30`") | R3 |
| `/justin:start` L19: "computed by `aios brief`"; never propose merging a PR "gated by or named in a `needs-human` issue, or labelled `needs-human`" | R3 (L19 only) |
| agent-loop.md L25 (the merge-ready sentence gains the label) and L72 (Holding a PR: the label also holds it); rule 11 L55 ("refuses a PR the brief shows as gated **or labelled needs-human**") | R3 |
| The loop's merge verdict: **ready** iff converged ∧ `merge_ready::blockers(pr_view, Some(issues), Some(open_threads)).is_empty()` ∧ the Option-A gate is empty ∧ the GitHub/git file-list cross-check passes ∧ the kill switch is clear. `pr_view` is fetched with `PR_FIELDS` + R2's extras; issues with `NEEDS_HUMAN_ISSUES_ARGS`. So `ready` is never looser than brief's `merge-ready: yes` | R2 (Task 15b) |
| The loop's needs-human write (label + deduplicated comment); new scenarios `merge_red_non_required_check_is_needs_human`, `merge_untrusted_open_thread_is_needs_human` and `merge_on_a_labelled_pr_stays_needs_human`; S25 uses `merge_ready::gates` | R2 (Tasks 4d, 14, 15) |
| agent-loop.md: one sentence relating the two verdicts ("`/justin:merge`'s ready = brief's merge-ready + convergence + the Option-A gate") | R2 (Phase B) |

The loop's per-PR metrics line in brief is deferred with the retro (decision 6), so R3 adds nothing for it.

## 4. `tools/src/git.rs` (R3-owned; parity environment)

```rust
//! tools/src/git.rs: git as brief.sh and checkpoint.sh ran it: the inherited
//! environment, unchanged.
pub fn command(cwd: &Path) -> std::process::Command;           // Command::new("git").current_dir(cwd).stdin(null); NO env changes
pub fn out(cwd: &Path, args: &[&str]) -> Option<Vec<u8>>;      // $(git …): stdout without trailing newlines; None on spawn error or non-zero; stderr dropped
pub fn text(cwd: &Path, args: &[&str]) -> Option<String>;      // out() when it is UTF-8, else None
pub fn ok(cwd: &Path, args: &[&str]) -> bool;
pub fn output(cwd: &Path, args: &[&str]) -> std::io::Result<std::process::Output>;
pub fn worktrees(cwd: &Path) -> Vec<PathBuf>;                  // `worktree list --porcelain` "worktree " lines, bytes → OsStr
pub fn branch(cwd: &Path) -> Option<String>;                   // symbolic-ref --short -q HEAD
```

`out` returns bytes, unlike R3's draft `Option<String>`, because paths can be invalid UTF-8 (D8). Per-call extras are added by the caller with `command(cwd).env(…)`, for example checkpoint's `GIT_TERMINAL_PROMPT=0` on its push.

**Environment policy (decided once): inherit.**
- No `LC_ALL`, no `GIT_NO_REPLACE_OBJECTS`, no `GIT_GRAFT_FILE`, no `GIT_TERMINAL_PROMPT` by default.
- Parity constraints for the port:
  - The scripts set none of these, so setting them would be an unlisted divergence.
  - The twins run with `LC_ALL=C TZ=UTC`, so locale-dependent git text (`%cr`, push-rejection reasons) is identical on both sides. The policy is therefore pinned by a unit test, not by the differential: `git_command_sets_no_env` (`Command::get_envs().count() == 0`).
- The replace-refs gap for checkpoint's secret scan (a replace object could hide content from `log -p`) is recorded in the H2/H3 follow-up issue (decision 8), not fixed in R3.

**R2 does not call `git.rs`.** Every loop git call feeds a security decision: the Option-A gate's changed paths, the push ancestry, the range check, and the dirty/ahead preconditions on the PR's branch worktree. So each call stays on the hardened `crate::cmd::soak::host::git()` (`LC_ALL=C`, `GIT_NO_REPLACE_OBJECTS=1`, `GIT_GRAFT_FILE=/dev/null/no-grafts`), in `tools/src/cmd/loop/git.rs`, with `GIT_TERMINAL_PROMPT=0` added for fetch and push.

**R3's follow-up issue (Task 12d) is reworded:**
- Move `docs_check`'s inherited-env git calls (`proc::capture("git", …)`) onto `git.rs`.
- Give `git.rs` a `hardened(cwd)` constructor, then move `soak::host::git()` and `cmd/loop/git.rs` onto it. This happens after both PRs merge, and never weakens their environment.
- `hook::path_guard`'s git (`LC_ALL=C`) stays as it is.

**Spec L78:** R3 adds `merge_ready` to the shared-module list and notes `git`'s inherit policy. R2 adds `inputs`.

## 5. The shared fake-gh harness and PATH jail (R3-owned)

**Files.**
- `tools/tests/common/fake_gh.rs` and `tools/tests/common/jail.rs`, declared as `pub mod fake_gh;` and `pub mod jail;` in `common/mod.rs`.
- R3's `common/agent.rs` (oracle loader, twin-repo scenarios) and R2's `common/loop_fake.rs` (FAKE_CLAUDE, loop scenarios) both build on them.
- Neither plan defines another fake gh. R2's `common/fake_gh.rs` with argv-joined keys, and R3's in-`agent.rs` fake with fixed semantic keys, are both replaced by this one.

**Layout of one fake root** (runtime, under `CARGO_TARGET_TMPDIR`):

```
<root>/bin/gh                    the #!/bin/sh fake; finds <root> as "$(dirname "$0")/.." (pwd -P), never from env
<root>/gh/<NN>-<label>/match     one extended regex (grep -E -f), matched against the rendered argv line
<root>/gh/<NN>-<label>/out[.K]   stdout   (default empty)
<root>/gh/<NN>-<label>/err[.K]   stderr   (default empty)
<root>/gh/<NN>-<label>/rc[.K]    exit code (default 0)
<root>/gh-state/<label>.n        how many calls this fixture has served
<root>/gh-calls/<SEQ>/           one directory per call (SEQ zero-padded, made with mkdir)
    argv     the words, NUL-separated
    cwd      `pwd -P`
    env      K=V lines for GH_PROMPT_DISABLED GH_NO_UPDATE_NOTIFIER NO_COLOR GH_REPO GH_HOST LC_ALL (K=<unset> when unset)
    fixture  the matched <label>, empty for no match
    body     a copy of the first file named by --body-file F, --input F, or a field value @F (-F/--field k=@F)
```

**Keying.**
- The **rendered argv line** is every word followed by one space, with each `\n` replaced by a space. In sh this is `printf '%s ' "$@" | tr '\n' ' '`; `fake_gh::render` in Rust is identical.
- Fixtures are tried in `<NN>` order, and the first whose regex matches serves the call. The Rust builder assigns `NN` in insertion order, so callers insert specific fixtures (`pr list .*--head `) before general ones (`^pr list `).
- **Ordinals:** the K-th call to a fixture serves `out.K`/`err.K`/`rc.K`; with none for K, the highest K present; then the unsuffixed file. This covers R2's thread pages and changing CI state.
- Committed fixture data stays each caller's own (R3: `tools/tests/fixtures/brief/<scenario>/gh/<label>.{out,err,rc}`; R2: `tools/tests/fixtures/loop/gh/…`). Each caller's const table maps `label → regex`, and the harness materialises it.

**Unknown call:** the fake logs the call with an empty `fixture`, prints `fake gh: no fixture for: <rendered line>` to stderr, and **exits 99**. 99 is reserved for harness bugs; gh failure modes are fixtures with `rc != 0`. A missing `gh` is simply no `<root>/bin/gh`.

**Rust API:**
- `FakeGh::install(root) -> FakeGh`;
- `.fixture(label, regex) -> Fixture` with `.out/.err/.rc/.nth(k, out, err, rc)`;
- `.calls() -> Vec<GhCall { argv: Vec<Vec<u8>>, cwd: PathBuf, env: BTreeMap<String, Option<String>>, fixture: String, body: Option<Vec<u8>> }>`;
- `.assert_all_matched()`;
- `.assert_calls_match(allow: &[&str])`: every rendered line matches one regex;
- `.assert_none_match(deny: &[&str])`.

R3's `assert_not_vacuous` and R2's `assert_no_forbidden_writes` both call `assert_all_matched`, and each asserts a non-empty `calls()` where gh is expected. Calls are sequential in both binaries. A `mkdir` race on `<SEQ>` makes the fake exit 98.

**PATH jail.**
- `jail::bin(dir, tools: &[&str]) -> PathBuf` symlinks each named tool, plus `sh` and every entry of `jail::FAKE_TOOLS`, resolved once from the host PATH, into `<dir>/bin`, and panics if a tool is missing. It refuses the names `gh` and `claude`.
- `jail::hermetic(cmd, fake_bin, jail_bin)` applies `common::isolated()`. Then it:
  - removes every `GH_*`, `GITHUB_*`, `AIOS_*` and `CLAUDE*` variable (the scenario re-sets what it needs);
  - sets `PATH=<fake_bin>:<jail_bin>` and nothing else, with no `/usr/bin` and no `/bin`;
  - sets `TZ=UTC` and `LC_ALL=C`.
- **Tool lists** (all `pub const` in `jail.rs`; each entry is an external command some line runs, never a builtin):
  - `FAKE_TOOLS`: every external command that the fake gh's script (`fake_gh.rs`) and R2's `FAKE_CLAUDE` script (`loop_fake.rs`) run. Today's expectation: `cat`, `cp`, `dirname`, `env`, `git`, `grep`, `mkdir`, `sleep`, `sort`, `tr`. The self-test below fixes the exact set from the two sources. R3 owns the constant; R2 edits it in its Phase-B convergence only if `FAKE_CLAUDE` needs a tool not yet listed (an amendment).
  - `QEMU_LOCK_TOOLS`: the external commands of W234's qemu-lock.sh: `git`, `dirname`, `basename`, `cat`, `sed`, `find`, `ps`, `pgrep`, `date`, `sleep`, `mv`, plus whatever the self-test finds on the other paths (`mkdir`, `rm`, `tr`, `awk`, `sysctl` or `cut`, …).
  - `ORACLE_TOOLS` (R3): R3's Task 1a list ∪ `paste` (checkpoint L196, L223, L250) ∪ `QEMU_LOCK_TOOLS`. No `flock`, `lockf`, `uniq`, `xargs` or `readlink`: no script line calls them (§0; no-legacy).
  - R2 passes `QEMU_LOCK_TOOLS` (its quiet-window check runs `qemu-lock.sh status`, R2 plan Task 13a) plus `kill` (`proc::Supervisor` spawns the external `kill`, main `tools/src/proc.rs` L69, L317). `git`, `sh` and `FAKE_TOOLS` come from `jail::bin` itself. `just`, `cargo` and `shellcheck` are fakes in `<fake_bin>`.
- **Missing-command tripwires.** A stderr check alone cannot see a tool the scripts call with `2>/dev/null` (a missing `ps` reads as `state=dead`, and both twins agree), so there are two checks:
  - `jail::assert_no_command_not_found(stderr: &[u8])` fails on `command not found`, `: not found` (dash) or `No such file or directory` naming a bare command. R3's `assert_not_vacuous` calls it on every oracle and aios stderr. R2's harness calls it on the fakes' stderr (the fake gh and `FAKE_CLAUDE` each append their own stderr to `<root>/fake-stderr`) and on every stage's captured stderr.
  - `jail_covers_every_script_command` (R3, `agent_support.rs`): for each oracle blob (brief.sh and checkpoint.sh at their oracle shas, qemu-lock.sh at `BRIEF_ORACLE`) and each fake's source, it extracts the first word of every simple command with one regex (line start, and after `|`, `;`, `&&`, `||`, `$(`, `(`, `then`, `do`, `else`, `!`; comments, assignments and quoted text skipped). It removes a fixed `NOT_COMMANDS` list (sh keywords and builtins, and the functions each script defines). It then asserts that the rest **equals** that script's tool list (no missing tool, no extra one), and that `command -v <tool>` succeeds for each under `hermetic`. A false positive goes into `NOT_COMMANDS` with a line cite.
- Self-tests (R3, `tools/tests/agent_support.rs`):
  - `jail_has_no_gh_or_claude`: `command -v gh` and `command -v claude` fail under `hermetic` with no fakes;
  - `jail_covers_every_script_command` (above);
  - `fake_gh_unknown_call_exits_99`;
  - `fake_gh_ordinals_advance`;
  - `fake_gh_logs_cwd_env_and_body`;
  - `fakes_are_posix_sh` (`sh -n`).
- **The CI image hazard.** GitHub's ubuntu-26.04 image ships `/usr/bin/gh`, and the job sets no `GH_TOKEN`. A test that reached it would fail authentication and pass vacuously through a "GitHub unavailable" path. Four things prevent this: the jail (no `/usr/bin` on PATH), the `GH_*`/`GITHUB_*` strip, `assert_all_matched`, and the non-empty-calls assertion. R2's 6b and Risk 11 wording ("PATH = fakes + `/usr/bin:/bin`") is replaced by this.
- **FAKE_CLAUDE (R2)** finds its root from `$0` the same way, never from `$AIOS_FAKE_ROOT`, so R2's stage env allowlist (`env_clear` + PATH/HOME/USER/LANG/TERM/TMPDIR/CLAUDE_CONFIG_DIR) still reaches it. Its PATH is the jail.
  - That allowlist also drops `isolated()`'s `GIT_CONFIG_GLOBAL=/dev/null` and `GIT_CONFIG_NOSYSTEM=1` inside a stage. So R2's scenario builder writes `user.name`, `user.email` and `commit.gpgsign=false` into the working clone's `.git/config` (round worktrees share it). Both `FAKE_CLAUDE`'s `commit` mode and the binary's own `Review round N:` commit then work without a global identity.
  - `stage_that_commits_itself_is_rejected` first asserts that `commit` mode really created a commit (the round worktree's HEAD moved past the stage's start sha), so it cannot pass vacuously.

## 6. The shim (`.claude/hooks/aios`) and `tools/tests/shim.rs`

Line numbers are W234 @ b47c756. Each plan's Task 0 re-reads them on post-#234 main, where the inputs line gains `shared`. Anchors are given by content too.

**Order of edits:** R3 edits the shim first, on R3's branch. R2 edits it only in Phase B, on top of the merged file (§1). So no hunk is ever merged against another. Both edits run in the foreground: protected file, owner prompts, never `sed`/`cp`.

**R3's lines (Task 8, foreground):**

| Where (W234) | Change |
| --- | --- |
| After `sub=${1:-}` (L136) | `unset AIOS_SHIM_STALE`, with a one-line comment. Every path (guard, hook, `--prebuild`, `AIOS_TOOLS_BIN`, fresh, dirty, rebuilt) then starts with it cleared, including a value inherited from a parent stale run (decision 9) |
| The `<other>` tail, `elif [ "$(verdict "$bin")" = stale ]` (L580-582) | It becomes: `if ! build_fg; then` → if `$sub` = `checkpoint`: `echo "aios: rebuilding $bin failed; aios checkpoint never commits or pushes with a stale binary (run: just tools)" >&2; exit 3`; otherwise the **unchanged** warning `aios: rebuilding $bin failed; running the stale binary (run: just tools)`, then `AIOS_SHIM_STALE=1; export AIOS_SHIM_STALE`. The missing branch (L575-579) and `exec "$bin" "$@"` (L583) are unchanged |
| Header, the `aios <other>` paragraph (L87-91) | Adds: a failed stale rebuild exports `AIOS_SHIM_STALE=1` to the stale binary it runs (brief prints its stale note from it), and `aios checkpoint` exits 3 instead of running it. checkpoint still runs a dirty build and an `AIOS_TOOLS_BIN` override (decision 5). Every other path clears `AIOS_SHIM_STALE` |

- brief and checkpoint need no `case` arm: both reach the `<other>` tail.
- checkpoint does **not** join the loop's strict arm, and R3 does not build any exit-3 helper.
- The R3 plan's Task 8b and its "R2's helper first" sequencing are void.
- R2's OQ12 is answered by decision 5.

**R3's `shim.rs` changes** (appended at end of file, plus one edit):
- `a_stale_binary_whose_rebuild_fails_runs_it_with_aios_shim_stale` (the fake binary prints `${AIOS_SHIM_STALE-unset}`);
- `a_fresh_binary_runs_with_aios_shim_stale_cleared` (with `AIOS_SHIM_STALE=1` inherited);
- `checkpoint_on_a_stale_binary_whose_rebuild_fails_exits_3` (code 3, empty stdout, binary not run, stderr names `just tools`);
- `checkpoint_runs_a_dirty_binary` and `checkpoint_honours_aios_tools_bin` (controls for decision 5);
- `brief_on_a_stale_binary_whose_rebuild_fails_warns_and_runs_it` (control);
- the edit: the doc comment at L623 names `aios brief` instead of `scripts/agent/brief.sh`.

The existing `a_stale_binary_whose_rebuild_fails_warns_and_runs_it` (W234 L626) stays.

**`/justin:pause` and two meanings of exit 3** (R3, Task 10a/10g). A shim exit 3 has empty stdout and an `aios: ` stderr line. checkpoint's secret stop prints its stop block on stdout. The skill tells them apart that way, and `skill_contract.rs` asserts it against both outputs.

**R2's lines (Task 3, Phase B, foreground, on top of R3's):**

| Where (W234) | Change |
| --- | --- |
| Immediately before `if [ -n "${AIOS_TOOLS_BIN:-}" ]; then` (L481) | `[ "$sub" = loop ] && [ -n "${AIOS_TOOLS_BIN:-}" ] && stop "…" "AIOS_TOOLS_BIN is not used for loop; aios loop runs only the main checkout's verified build (unset it)"` (`stop` exits 3 for `loop`) |
| `case $sub in` (L503): a new last arm `loop)` after the `hook)` arm, before `esac` (L573) | Strict (decision 2): missing → `build_fg`, then require runnable; stale → `build_fg`, else exit 3 naming `just tools`; after any build, recompute `verdict` and exit 3 unless `fresh`; `dirty-*` → exit 3 naming `dirty_cause`, with no build; `fresh` → `exec "$bin" "$@"`. The arm reads only `$1` |
| Header | One sentence in the `AIOS_TOOLS_BIN` paragraph (L25-29): `loop` refuses it. A new `aios loop ...` paragraph between the `aios hook` paragraph (ends L86) and R3's rewritten `aios <other>` paragraph |

**Two residuals of the `loop)` arm, accepted and recorded** (R2's threat model and the header paragraph name both):
1. **Check-then-exec race.** The arm checks `verdict "$bin"` and then execs the path. A `just tools` rename in between swaps in a binary that was never checked, and that binary could be a dirty build. The guard and hook arms avoid this with a per-call hard link (W234 L511-523, L558-565), but `loop` cannot: R2's `self_location()` requires `current_exe()` to be `<main>/target/tools/installed/aios`. Accepted at Stage 0: runs are attended, the window is one `exec`, `just tools` is started by the same owner, and rule 11 keeps the main checkout's inputs clean, so a dirty build there needs an owner edit first. Revisit before Stage 5 with OQ8.
2. **Direct-exe spelling.** `<main>/target/tools/installed/aios loop review|merge …` skips the shim (R2 plan L90). It is not allowlisted, so it prompts, and a `-p` or background run is refused. §11 adds explicit asks so that a later broader allow cannot silently cover it.

R2's `shim.rs` tests (Task 3a's list) are appended after R3's. `the_shim_is_posix_sh` and `shellcheck -s sh` (no new findings against `origin/main`'s shim) are both plans' verifies.

**justfile recipes (R3, Task 9).** `brief *args:` and `checkpoint *args:` call `{{ quote(justfile_directory() / ".claude" / "hooks" / "aios") }}`, the soak recipe's idiom (justfile L153), not the relative `.claude/hooks/aios` that `docs-check` uses. So the pause fallback `just --justfile <main>/justfile --working-directory . checkpoint` from a pre-R3 worktree runs **main's** shim, with its checkpoint exit 3 and stale handling. R2 adds no recipe (R2 D10).

## 7. The eval hash vs R3's skill edits (CI never red)

- `committed_scores_are_fresh` (R2, hashing `scripts/agent/prompts/**`, the config's model keys and `.claude/skills/justin/**`) is `#[ignore]`, so it never runs in CI's `cargo test -p aios-tools`. Freshness is enforced instead by the loop's deterministic eval-gate finding (R2 Task 11). It is run explicitly as `cargo test -p aios-tools --test loop_eval -- --ignored committed_scores_are_fresh`:
  - in R2 Task 18c, whose verify is corrected to this form (the plan's form without `-- --ignored` filters out the ignored test and passes vacuously);
  - after any re-score in 24d;
  - in R2's final gate (24g).
- A non-ignored `committed_scores_parse` (schema only, no hash) keeps the file honest in CI. It lands **in the same commit** as `scripts/agent/evals/scores.json` (Task 18b), never earlier, so no earlier push to R2's draft PR has a red test.
- **Ordering:** R3 merges first and no `scores.json` exists on main then, so R3 never re-scores. R2 records scores (Task 18) once, in Phase B:
  - after convergence;
  - after Tasks 19 and 21, which edit the justin skills and evals;
  - after the 24a simplifier;
  - before the 24d audit and the ship pass (§1 Phase B).

  An audit-round fix that touches an eval input forces one owner-attended re-score before the ship pass, so the lead batches prompt and skill fixes. The ship pass edits no eval input. The first run stays under $25, and the owner approves the case list before scores are recorded (decision 6).
- R3's PR is never loop-reviewed, because R2 is not merged then. R3's plan uses `/review-pr-comments` (Task 12e). After R2 merges, a later PR that edits a justin skill gets an eval-gate finding (needs-human), never a red CI.

## 8. Every other shared file: owner and how the second branch merges

The rule is §1: R3 edits on its own branch. R2 edits a shared file only in Phase B, on the merged tree. The exceptions are the one-line union hunks, which R2 resolves at convergence.

"Ship pass" means rule 11's inventory pass, and it is literally the last commit before `gh pr ready`: on R2 it comes after Tasks 18 and 22 and the 24d audit (§1 Phase B), so nothing follows it. R2 runs its own ship pass on the merged tree and never preserves R3's counts by hand. One inventory edit happens earlier: `harness-tables` fails any commit whose tracked skills and CLAUDE.md Skills table or layout skills list disagree, so the commit that adds or narrows a skill edits those rows itself (#234's precedent 77067cb). The ship pass then re-checks them.

| File | R3 (first) | R2 (second) | How R2 merges |
| --- | --- | --- | --- |
| `tools/src/gh.rs`, `git.rs`, `merge_ready.rs`, `tests/common/fake_gh.rs`, `tests/common/jail.rs` | creates; owner | vendored copy in Phase A; consumes | takes main's version verbatim (§1) |
| `tools/src/lib.rs` | `pub mod gh; pub mod git; pub mod merge_ready;` (alphabetical between `cmd` and `paths`) + crate-doc sentence | `pub mod inputs;` (between `git` and `merge_ready`) + crate-doc sentence | union; crate doc rewritten once |
| `tools/src/main.rs` | `Brief`, `Checkpoint` variants (before `DocsCheck`), match arms, `fn brief`/`fn checkpoint` (raw `args_os`, as `soak`) | `Loop` (between `Hook` and `Soak`), arm, fn | union; variants alphabetical: Brief, Checkpoint, DocsCheck, Hook, Loop, Soak |
| `tools/src/cmd/mod.rs` | `pub mod brief; pub mod checkpoint;` | `#[path = "loop/mod.rs"] pub mod pr_loop;` (before `soak`) | separate hunks |
| `tools/tests/common/mod.rs` | `pub mod agent; pub mod fake_gh; pub mod jail;` | `pub mod loop_fake;` | union, alphabetical |
| `tools/tests/soak_interleave.rs` | L25, L739-749: runs `aios brief` (§3) | none | — |
| `tools/Cargo.toml` | description (L7) → `(brief, checkpoint, docs-check, hook, soak; agent-loop commands in later PRs)` | → `(brief, checkpoint, docs-check, hook, loop, soak; the guard in R5)` | R2 rewrites the line in Phase B |
| `Cargo.toml`, `Cargo.lock` | unchanged | unchanged | both final gates: `git -C <BW> diff --exit-code origin/main -- Cargo.toml Cargo.lock` |
| `justfile` | two recipes after `docs-check-all` (main L347-348) (§6) | none | — |
| `.claude/skills/justin/skills/brief/SKILL.md`, `pause/SKILL.md` | owner (switch-over) | none | — |
| `.claude/skills/justin/skills/start/SKILL.md` | L19 only (§3) | L18 (propose `/justin:review <n>`), L30 (seven skills), Phase B | on top |
| `.claude/skills/justin/skills/team/SKILL.md`, `.claude-plugin/plugin.json`, `skills/{review,merge}/`, `evals/` | not touched (`plugin.json` stays 0.2.0) | owner (0.3.0). `skills/{review,merge}/SKILL.md` are created in Phase B Task 19 only, in the commit that adds their CLAUDE.md rows; they are hashed at Task 18 | — |
| `.claude/skills/merge-and-cleanup/SKILL.md` | L52, L55, L56 (§3) | none | — |
| `.claude/skills/review-pr-comments/` | untouched (R3 still uses it in 12e) | **kept and narrowed**, Task 19f, Phase B (below) | — |
| `.claude/rules/03-git-workflow.md` | L22 becomes the **one** general Guard Rails bullet (below) | extends that same bullet with the loop specifics, and edits PR Workflow step 4 (L48); Phase B | on top; no second bullet |
| `.claude/rules/11-teams.md` | L55: "gated **or labelled needs-human**" | one Git-authority bullet: `aios loop`'s own detached round worktrees `loop-pr<n>-r<NN>` and its `<sha>:refs/heads/<claude/*>` pushes; agents never use those worktrees. Phase B | on top |
| `.claude/rules/04-phase-workflow.md`, `05-file-placement.md` | none | rule 04 L28 (review skill), rule 05 placement for `scripts/agent/{prompts,evals,loop-config.json}` and `tools/src/cmd/loop/` | — |
| `docs/project/agent-loop.md` | L11 ("shared logic lives in the `aios` binary"), L16, L18, L25, L30 (incl. H1 fail-closed), L72 | L9 table rows, L11 "Five skills", L34, L52, L70 (labels `agent-stop`, `hard`), L101-103 kill switch, L252 Stage 0 row, Stage 2 row ("unattended"), the PR-loop section, the L25 relation sentence; Phase B | on top |
| `.claude/CLAUDE.md` | ship pass: the `tools/` block (+brief, checkpoint, gh, git, merge_ready), `scripts/` line (agent/ keeps only qemu-lock), justfile line (+brief/checkpoint), the `/justin:brief` and `/justin:pause` table rows | Task 19 (`harness-tables`): the `.claude/skills/` layout line and the Skills table (+`/justin:review`, `/justin:merge`; `/review-pr-comments` row narrowed). Ship pass: the `tools/` block (+cmd/loop, inputs), `scripts/` (+prompts, evals, loop-config.json) | on the merged tree |
| `docs/project/developer-guide.md`, README | recipe rows (just-recipes check) + ship pass | ship pass | on top |
| Spec `2026-09-22-jl-rust-agent-tools.md` (W234 lines) | L41 + L194 Order; L71-72 rows; L79 (`merge_ready`, git policy); L84 → "Fake `gh` and `claude` are `#!/bin/sh` scripts on PATH (R4 precedent), not test binaries"; L151 example; L176 R3 row + an "R3 as built" paragraph after L181 (D1-D10, oracle shas, synthesised fixtures); §2 Freshness, Stale × Other-subcommands cell (`AIOS_SHIM_STALE`; checkpoint exit 3) | L70 row (`aios loop review\|merge\|eval`; sweep and retro later); L79 (`inputs`); §1 Protection L92 (Option A's list, still "the guarantee"); §2 Freshness: a new `aios loop` column; L175 R2 row; §4 Loop spec amendments L196-203 (L200 matches R3's L84); OQ L223-225 (Option A answered) and L243 (exit 3 answered); OQ8 recorded as open | Phase B; R2 re-reads the whole Freshness table |
| `.claude/hooks/tests/test_git_push_guard.py` | L1471-1487 (drops brief.sh) | none | — |
| `.claude/settings.json` | owner commit (§11) | owner commit, Phase B (§11) | sequenced; never merged by an agent |
| `.gitattributes` | none | `scripts/agent/evals/** -text` | — |

**`/review-pr-comments` after R2 (replaces R2 Task 19f's deletion).** Decision 6 keeps kernel-area fixes needs-human, so the loop never replies to or resolves their threads, and brief's merge-ready blocks on any unresolved thread. Deleting the skill would leave a team-fix kernel PR with no path to merge-ready. R2 therefore keeps it, narrowed to the threads the loop leaves:
- Its description and Step 1 cover only threads that `/justin:review` left needs-human (kernel-area findings, and any thread the loop did not resolve). Its Step 3 still spawns the writer for the file's area (kernel-dev with the lead's Fable review), and Steps 6-7 still reply and resolve after the fix sha is pushed.
- team/SKILL step 8 (W234 L143): `gh pr ready <n>`, then `/justin:review <n>`; for the threads it leaves needs-human, `/review-pr-comments`. The owner removes the `needs-human` label after deciding (§3).
- The other call sites in R2's 19f list (rule 03 L48, rule 04 L28, implement-phase L161, generate-phase-doc L208, write-arch-doc L340, developer-guide L2318) name `/justin:review <n>` first and `/review-pr-comments` for the threads it leaves. The research note's links stay valid, so it is not edited.
- R2's 19g greps that `review-pr-comments` vanishes are dropped. In their place: `just docs-check` exits 0, and `git -C <BW> grep -n 'review-pr-comments' -- .claude docs/project` shows only the narrowed call sites above.
- Retiring the skill waits for a loop path that verifies a lead-pushed kernel fix and then replies and resolves (with its own scenario): recorded with R2's OQ13 as an open owner-scope item (§11), not asked now.

**Rule 03 Guard Rails bullet (R3's text; R2 appends).** It replaces W234 L22:
- Compiled `aios` subcommands that commit, push or write to GitHub run git and gh where neither the guard nor the permission rules see. Today that is `aios checkpoint`; R2 adds `aios loop`.
- Their controls:
  - their code and tests;
  - the Option-A gate on their sources;
  - an owner settings ask on each invocation spelling, so every run prompts once and a `-p` or background run is refused.
- `/justin:pause` pushes through `just checkpoint`, refspec `refs/heads/<claude/*>:refs/heads/<claude/*>` only, never forced.
- An unattended runner needs the owner to remove that ask from its stage's settings.

R2 appends (A12):
- `aios loop` never merges;
- it pushes only `<sha>:refs/heads/<the PR's claude/* head>` (a fix commit or a two-parent merge of `origin/main`);
- its asks are §11's seven R2 rules, named exactly: `Bash(.claude/hooks/aios loop *)`, `Bash(*/.claude/hooks/aios loop *)`, `Bash(target/tools/installed/aios loop *)`, `Bash(*/target/tools/installed/aios loop *)`, `Bash(*/aios loop *)`, `Bash(cargo run *loop*)` and `Bash(cargo r *loop*)`, plus any rule R2's probe adds (R2 Task 21b). It extends R3's bullet; there is no second bullet.

## 9. The guard and direct shim calls (probed 2026-10-11, W234 guard @ b47c756, main thread and agent, same result)

| Command | Decision |
| --- | --- |
| `.claude/hooks/aios brief`, `… checkpoint --handoff saved`, `… loop review 1`, `… loop merge 1` | **ask**: "cannot tell whether this starts QEMU: part of the command comes from a shell expansion…" (the guard reads the shim and meets `exec "$bin" "$@"`) |
| `"$CLAUDE_PROJECT_DIR"/.claude/hooks/aios loop review 1`, `"$CLAUDE_PROJECT_DIR"/.claude/hooks/aios checkpoint` | **none** (passes; settings allow `Bash(.claude/hooks/aios *)`, W234 L127) |
| `just brief`, `just checkpoint --handoff saved` | **none** (`just_starts_qemu` checks QEMU recipes only, W234 L1719; settings allow `Bash(just *)`, W234 L120) |
| `bash scripts/agent/checkpoint.sh --handoff saved` | **ask** (unresolvable push refspec; for an agent, commit in an expanded dir) |
| `bash scripts/agent/brief.sh --no-fetch` | none |

**What each plan does and expects:**
- **R3:**
  - Skills and fallbacks use only `just` forms: `just brief`, `just checkpoint --handoff saved`, `just --justfile "<main>/justfile" --working-directory . brief|checkpoint …`, and the D5 hint. So the guard gives no decision, and the **owner's settings asks** are the once-per-pause control (decision 4; no guard deny).
  - The 0e headless probe judges each run by a marker file the probe recipe touches, plus `permission_denials` in `--output-format json`, never by grepping model output. It covers:
    - the `just` spellings (bare, env-prefixed, double space, `cd x &&`, subshell, `--justfile … --working-directory .`, quoted recipe, `env`/`command`/`timeout` wrappers, `just brief && just checkpoint`);
    - the shim spellings (`.claude/hooks/aios checkpoint`, `"$CLAUDE_PROJECT_DIR"/.claude/hooks/aios checkpoint`, `sh .claude/hooks/aios checkpoint`);
    - the D5 hint;
    - a Monitor-tool run.

    Any spelling that runs extends the owner's settings delta before Task 1 (§11).
  - R3's 10f re-runs this §9 table against R3's `<BW>` (`.claude/worktrees/tools-r3`) after the switch-over, with that tree's guard, shim and justfile, and diffs it against the expected rows.
  - The follow-up issue for R5 ("the guard asks on direct `aios <sub>` calls with a misleading QEMU reason") must say that removing that ask requires the checkpoint and loop settings asks to be on main first.
- **R2:**
  - R2 Task 1b's expectation ("a direct `aios loop` passes unasked") is false and is corrected: a direct `.claude/hooks/aios loop …` already asks, with the misleading QEMU reason, and the `"$CLAUDE_PROJECT_DIR"` spelling gets **no** decision.
  - So the owner's ask rules (§11) are the real control, and they must cover the `*/` spelling.
  - `/justin:review` and `/justin:merge` call exactly `.claude/hooks/aios loop review|merge <n>` from the main checkout. Expect one prompt per run, whose text may be the guard's QEMU reason until R5. D10 (no `just loop`) stands: `just loop` would get no guard decision.
  - R2 reuses R3's saved 0e probe harness (A7: `$R/probe/`, checked against its `SHA256SUMS`, run unchanged by R2 Task 20c, with no stub of R2's own) for the loop spellings: the shim spellings, the direct-exe spelling of §6, an unlisted build path, and the `cargo run`/`cargo r` spellings, with the stubs' `ran-shim-loop`, `ran-exe-loop` and `ran-cargo-loop` markers. Every non-control row must be `refused` under §11's seven R2 asks (A12).

## 10. Test-data hygiene (both plans)

The repository is public. Every `/justin:pause` on these branches runs checkpoint's scan over new paths and added lines not yet on origin: `SECRET_PATH_RE`/`SECRET_TEXT_RE`, checkpoint.sh L85/L87, case-insensitive for paths.

- **No token-shaped literal in any committed file.** That means nothing matching `SECRET_TEXT_RE` (PEM `BEGIN … PRIVATE KEY` lines, `gh[pousr]_` + 36, `github_pat_`, `sk-ant-`, `AKIA` + 16, `xox?-`).
  - Tests build such strings at runtime from fragments (`concat!("gh", "p_", …)`, `format!`).
  - Goldens record only `<path>\t<reason>`, never the matched text.
  - R2's seeded eval patches use non-matching placeholders.
- **No committed path whose basename matches `SECRET_PATH_RE`.** That rules out `secret`, `credential`, `.env*`, `.netrc`, `.npmrc`, `.pypirc`, `id_rsa*`, `*.pem`/`.key`/`.p12`/`.pfx`/`.jks`/`.keystore`. Examples:
  - `secret_named_new_doc.golden` → `sensitive_name_new_doc.golden`;
  - `modified_tracked_credentials_not_flagged` → `modified_tracked_creds_not_flagged`.

  Scenario names, test fn names and golden basenames all follow this. Paths created at runtime inside scratch repos are exempt.
- **No `/Users/`, `/home/` or `/private/` paths**, and no account emails, in fixtures or goldens.
- **Verify** (each plan's final gate, in its own `<BW>`, on `git -C <BW> diff --name-only origin/main`, so only new or changed files count):
  - `git -C <BW> grep -nIE -e '-----BEGIN ([A-Z0-9]+ )*PRIVATE KEY-----' -e 'gh[pousr]_[A-Za-z0-9]{36}' -e 'github_pat_[A-Za-z0-9_]{22,}' -e 'sk-ant-[A-Za-z0-9_-]{20,}' -e 'AKIA[0-9A-Z]{16}' -e 'xox[abposr]-[A-Za-z0-9-]{10,}' -- tools scripts/agent .claude` prints nothing. The regex sources in `scan.rs` do not match themselves.
  - `git -C <BW> diff --name-only --diff-filter=A origin/main | grep -iE '(^|/)(\.env(\.[^/]*)?|\.netrc|\.npmrc|\.pypirc|id_(rsa|dsa|ecdsa|ed25519)[^/]*|[^/]*\.(pem|key|p12|pfx|jks|keystore)|[^/]*(credential|secret)[^/]*)$'` prints nothing.
  - A real `/justin:pause` on the branch never needs `--allow` for test data.

## 11. Settings (owner edits; decision 4; R2 OQ11 and R3 10e)

- Applied by the owner, running a lead-written script under the session scratchpad (jq, no hand edits). Committed by the owner separately as "owner-approved settings" (R4 precedent 65b80cb). Agents never write `.claude/settings.json` (#244) and never resolve a conflict in it.
- **Each script first checks its base:** `git hash-object .claude/settings.json` must equal the blob named in the script. R3's script names the post-#234 main blob. R2's names the blob on main after R3 merges. A script applied out of order fails before writing.
- **R3 (on R3's branch, before `gh pr ready`):**
  - remove `"Bash(bash -n scripts/agent/brief.sh)"` and `"Bash(bash -n scripts/agent/checkpoint.sh)"` (W234 L125-126);
  - append to `ask`: `"Bash(just checkpoint*)"`, `"Bash(just --justfile * checkpoint*)"`, `"Bash(.claude/hooks/aios checkpoint*)"`, `"Bash(*/.claude/hooks/aios checkpoint*)"`, plus whatever the 0e probe adds;
  - no guard deny.
- **R2 (Phase B, after convergence, as a delta on the merged file; A12):** append seven rules:
  - the shim spellings `"Bash(.claude/hooks/aios loop *)"` and `"Bash(*/.claude/hooks/aios loop *)"`;
  - for the direct-exe spelling (§6) `"Bash(target/tools/installed/aios loop *)"` and `"Bash(*/target/tools/installed/aios loop *)"`;
  - `"Bash(*/aios loop *)"`, for any other build's binary (`target/debug/aios`, a scratchpad release build);
  - `"Bash(cargo run *loop*)"` and `"Bash(cargo r *loop*)"`: settings allow `Bash(cargo *)` (W234 L119), and a debug build honours R2's test escape, so a `cargo run … loop` would otherwise run a branch build with no prompt;
  - plus whatever R2's reuse of the 0e probe adds (R2 Task 20c).
- No label step: `agent-stop` and `hard` exist (decision 10). R2 Task 20b is dropped, and R2's label-exists kill-switch check keeps failing closed.
- **Still open, recorded and not asked now:**
  - R2 OQ8: gate widening (shim, guard, settings, workflows, member manifests, `.justfile`, `.gitattributes`, `scripts/agent/loop-config.json`, `scripts/agent/prompts/**`). Decide before Stage 5, together with the `loop)` arm's check-then-exec race (§6).
  - The exact final ask spellings for both branches, which the 0e probe results fix. Sequenced per §1.
  - R2 OQ13 (kernel-area fixes later) now also carries the `/review-pr-comments` retirement (§8).

## 12. Contract checks (each plan's Task 0, after its edits)

- Both plans link this file, and neither restates a signature that differs from §2-§5:
  - `grep -c 'GhOutcome' <both plans>` ≥ 1 each;
  - `grep -nE 'enum Gh \{|trait Gh \{|gate_refs|CliGh|GhEnv::inherit|fn loop_gh_env' <both plans>` prints nothing.
- R2's plan states:
  - Phase A and Phase B, and that Task 23 is the first task of Phase B;
  - Phase A creates no `.claude/skills/**/SKILL.md`;
  - Task 3 is in Phase B;
  - the Phase-B order of §1: Task 18 after Tasks 19, 21, 23 and 24a; Task 22 after 18; 24d after 22; the ship pass (24b with 24c) after 24d and last before `gh pr ready`;
  - `committed_scores_are_fresh` is `#[ignore]`, and `committed_scores_parse` lands with `scores.json` in 18b;
  - Task 15e's verify is `gh_write_argv_never_merges`;
  - Task 19f keeps and narrows `/review-pr-comments` (§8);
  - the scenario builder's git identity and the `commit`-mode assertion (§5);
  - no `metrics.jsonl`, no `escaped` disposition, no Task 20b.
  - Stale statements are gone: `grep -nE 'AIOS_FAKE_ROOT|/usr/bin:/bin|no shared .tools/src/git.rs|R2 owns the shim|OQ7|git rm -r .claude/skills/review-pr-comments' <R2 plan>` prints nothing.
- R3's plan states:
  - D9 = H1: a git failure inside the scan fails closed (restore the index, commit nothing, exit 1). Pinned by `scan_git_failure_fails_closed`, an aios-only golden with a jailed git wrapper that fails `log -p`.
  - D10 = the `labelled needs-human` blocker.
  - Task 8b is removed.
  - The recipes use `justfile_directory()`.
  - OQ1 = A and OQ2 = stale-rebuild-only, citing decisions 4 and 5 (not #204's "1. A 2. Yes", which answered Option A and loop exit 3).
  - The history-oracle differentials are kept (decision 7).
  - The freeze holds complete, tested implementations (§1).
  - The amended 10f grep and the `soak_interleave.rs` port item (§3).
  - `ORACLE_TOOLS`, `QEMU_LOCK_TOOLS`, `FAKE_TOOLS` and `jail_covers_every_script_command` (§5).
  - Stale statements are gone: `grep -nE 'gh::\{Gh,|R2.s helper|AIOS_FAKE_ROOT/gh' <R3 plan>` prints nothing.
- After filing the gate issue (§1): R2's brief line is `gated by #<gate>`, and R3's names no gate.

## 13. Rejected critic points (2026-10-11 revision)

Every finding of the critic round was checked against W234, main and both plans. Seventeen were confirmed; the two gate-title findings were the same defect and are fixed once (§1). Three suggested fixes were replaced with a different fix:
- **`GhEnv` as `fn loop_gh_env()` or a `LazyLock` static** (gh.rs API finding): rejected. Every caller's env is made of literals, so `GhEnv` now holds `&'static` slices. That makes `GhEnv::INHERIT`, checkpoint's env and `LOOP_GH_ENV` true `const`s with no runtime initialiser. The rest of that finding (`GhEnv::apply` as the only path from a `GhEnv` to a `Command`, and non-`GH_` names in the gh.rs env test) is adopted.
- **The binary re-verifying the provenance stamp for review and merge** (shim `loop)` arm finding): rejected. It would duplicate the shim's whole verdict (stale, the four dirty causes, origin/main ancestry) in Rust, and the two copies could drift, which is the kind of added mechanism the audit-loop lesson warns against. The race is accepted and recorded instead (§6), and the direct-exe spelling gets explicit asks (§11). Both are revisited with OQ8 before Stage 5.
- **Scoping R2 Task 15e's grep to argv builders** (one of two options): not taken. The deny-table consts and the `#[cfg(test)]` rows sit in the same files, so a scoped grep would need exclusions that rot. The exhaustive-match unit test (§2) was the critic's other option, and it is adopted.

Also not taken: the critic's worktree names `tools-r2-loop`/`tools-r3-scripts`. Both plans create `.claude/worktrees/tools-r2` and `.claude/worktrees/tools-r3`, and §0 uses those.

## 14. Amendments (2026-10-11)

One merged set, A1-A12, in one numbering, as both plans state it (R3's "Contract amendments A1-A12" list and R2's "merged amendments A1-A12" table). R3's revision-3 list (A1-A9) and R2's v2 list (A1-A7) were two sets under colliding numbers; this set is R3's A1-A9, with A1, A4 and A7 reconciled against R2's, plus R2's evals/cases exclusion (A10), R2's docs gate (A11) and R2's own rows (A12). Both plans already cite this numbering; neither is renumbered.

**Precedence.** The A12 rows of §2, §8, §9 and §11 already match. For the other sections an item names, the team-build lead edits that section to match before either branch commits this file (R3 0f, R2 0b); until then, where an earlier section and §14 differ, §14 wins. Each item was checked against the repo on 2026-10-11.

- **A1 (§5, fake tool lists and the extractor).** `jail.rs` defines:
  - `pub const FAKE_GH_TOOLS`: exactly the fake gh's external commands (expected `cat`, `cp`, `dirname`, `grep`, `mkdir`, `tr`);
  - `pub const FAKE_CLAUDE_TOOLS: [&str; 8] = ["cat", "dirname", "env", "git", "grep", "mkdir", "sleep", "sort"]`, R2's list: R2 owns the `FAKE_CLAUDE` script (R2 6a), which runs exactly these. (`cp` and `tr` come from the fake gh, not from `FAKE_CLAUDE`.)
  - `pub const FAKE_TOOLS`: `FAKE_GH_TOOLS ∪ FAKE_CLAUDE_TOOLS` (10 entries), all of which `jail::bin` links;
  - `pub const SH_WORDS: &[&str]` (sh/bash keywords and builtins) and `pub fn script_commands(src: &str, not_commands: &[&str]) -> BTreeSet<String>` (specified in R3 Task 1a). The keyword list has this one name in both plans; there is no `NOT_COMMANDS` const in `jail.rs`. Callers append their own script's function names, e.g. `&[jail::SH_WORDS, BRIEF_FUNCTIONS].concat()`. R2 6a's `FAKE_CLAUDE` spec defines no functions, so R2 passes `jail::SH_WORDS` alone (plus the names of any function it later defines, by the same rule).

  R3's self-test asserts `script_commands(fake_gh::SCRIPT, SH_WORDS)` == `FAKE_GH_TOOLS`. R2 adds `fake_claude_commands_equal_fake_claude_tools` to `tools/tests/loop_fakes.rs`: `jail::script_commands(FAKE_CLAUDE, jail::SH_WORDS)` == `jail::FAKE_CLAUDE_TOOLS`. R2 23b points at that test, not at `agent_support.rs`.

  **The one tool-change rule (both plans).** Before R3 merges, a `FAKE_CLAUDE` tool change is an amendment that R3 applies in `jail.rs`, and R2 re-vendors. After R3 merges, the amendment names the added `FAKE_CLAUDE_TOOLS` entry as an allowed additive R2 line in §1's convergence verify, and R2 adds exactly that line in its convergence commit. No other R2 line ever enters `jail.rs`.
- **A2 (§5, per-script tool lists).** `ORACLE_TOOLS` is redefined as `BRIEF_TOOLS ∪ CHECKPOINT_TOOLS ∪ QEMU_LOCK_TOOLS` (`paste` sits in `CHECKPOINT_TOOLS`), and the equality is asserted per script. The extractor's output loses these words, each with a line cite:
  - `gh`: a fake (brief L154, L166, L177, L197, L269, L283, L551-552; checkpoint L219);
  - `.claude/hooks/aios`: the scenario's committed stub (brief L103);
  - `sh`: always linked (brief L474).

  `bash` is not a jail tool: the oracle runs as the absolute bash path that `require_tools()` resolves before `hermetic`. No script line runs `bash`; checkpoint L271 names it only inside an echoed string.
- **A3 (§5, §1, vendorable files).** `fake_gh.rs` and `jail.rs` depend only on main's `tests/common/mod.rs` (`isolated()`, `unique_dir`) and on each other, never on `agent.rs` or `parallel_map`. R3 Task 5c proves that the five files build on `origin/main` alone.
- **A4 (§1, §12, gate).** R3 Task 1i is the only filer of the gate issue. Its pre-check, in both plans, is the jq `startswith` filter, never `--search` (GitHub search tokenises `#` and `:`):
  `gh issue list --label needs-human --state open --limit 1000 --json number,title --jq '[.[] | select(.title | startswith("Gate: merge PR #<R2> "))] | length'`.
  It files only when that prints 0. R2 0f records `GATE` from R3's Decisions Made and runs the same command, expecting 1. The verify matches the blocker anywhere in R2's `merge-ready:` line (both drafts read `merge-ready: no (draft; gated by #<gate>…)`, since the `draft` blocker comes first, W234 brief L219): the anchored `[(;] ?gated by #<GATE>([,;)]|$)` of R3 Task 1i.
- **A5 (§8 Cargo row, §10 Verify, three-dot).** Every "what did this branch change" check is a three-dot diff:
  - `git -C <BW> diff --exit-code origin/main...HEAD -- Cargo.toml Cargo.lock`;
  - `git -C <BW> diff -U0 origin/main...HEAD -- tools/Cargo.toml`;
  - `git -C <BW> diff --name-only --diff-filter=A origin/main...HEAD`.

  This covers R3 3d and 12d, and R2 24g.
- **A6 (§11, §8 rule 03 bullet, ask spellings).** R3's asks gain `Bash(cargo run *checkpoint*)`, `Bash(target/tools/installed/aios checkpoint*)` and `Bash(*/target/tools/installed/aios checkpoint*)`. Settings allow `Bash(cargo *)` (W234 settings L119), so `cargo run -p aios-tools -- checkpoint` would otherwise commit and push with no prompt. The 0e probe covers these spellings, a main-thread background Bash call (`bash-bg`) and a background-agent run, and the rule 03 bullet names the asked spellings.
- **A7 (§9, the saved probe harness; one marker scheme).** R3 0e saves the harness under `$R/probe/` (`$R` = `<git-common-dir>/aios-agent/port-inputs/r3`) and lists every file in `SHA256SUMS`, before any run that counts. R2 20c copies it and runs `run-probe.sh` unchanged; it never runs the real shim and adds no stub of its own.
  - **Stubs.** Each only runs `touch "$PROBE_DIR/ran-<stub>-<sub>"`: the justfile's `brief` and `checkpoint` recipes (`ran-just-<recipe>`), `.claude/hooks/aios` (`ran-shim-$1`), `target/tools/installed/aios` (`ran-exe-$1`), and `bin/cargo` (`ran-cargo-<w>` for each `<w>` in `brief checkpoint loop` that appears as a whole argument).
  - **Spellings file** `<id>\t<kind>\t<sub>\t<spelling>`, where `<sub>` names the marker that means "ran" (`checkpoint`, `loop`, or `brief` for the control). Kinds: `bash` (a main-thread Bash call), `bash-bg` (a main-thread Bash call with `run_in_background: true`), `monitor` (the Monitor tool) and `background` (an Agent subagent spawned with `run_in_background: true`).
  - **Verdict, computed by `run-probe.sh` itself** so both plans judge alike: `ran` when any `$PROBE_DIR/ran-*-<sub>` exists; `refused` when none exists and a permission denial in the stream-json events names the command; `inconclusive` otherwise. Output row: `<id>\t<spelling>\t<verdict>\t<denials>\t<markers>`. So R2's loop rows count `ran-shim-loop`, `ran-exe-loop` and `ran-cargo-loop` alike.
  - Both plans' verifies fail on any `inconclusive` row and on any non-control `ran` row, and require the control row to be `ran`.
- **A8 (§11, settings base and recovery).** A settings script's base is the branch tip's blob (`git -C <BW> rev-parse HEAD:.claude/settings.json`), which must also equal `origin/main:.claude/settings.json` after a fetch. If either check fails, a worker merges `origin/main` (a clean merge while the branch has no settings edit), the lead re-pins the base, and the script is re-run. If main changes `settings.json` after the owner commit, the owner (never an agent) reverts the owner commit, a worker merges main, and the owner re-runs the script on the new base. The order is still R3, then R2.
- **A9 (§0, anchors).** Record #234's move to 8a28365 with its two moved anchors (guard L1513 → L1522, lesson L102 → L105), and main's move to 8fa4c24 (`.claude/CLAUDE.md` gains 6 lines at L51; the developer guide changes only after L1600). R3's Task 0a (2026-10-11) then re-read every anchor on `origin/main` 2d4a9f7, #234's squash merge (final head 8253f0d) on 8fa4c24, and both plans and this file now cite 2d4a9f7 lines under the old labels: the guard's `just_starts_qemu` L1719, the lesson's count L105, the `.claude/CLAUDE.md` Skills rows L287/L289 (7 lines lower, not 6) and `tools/tests/shim.rs`'s brief.sh doc comment L623.
- **A10 (§3, the 10f grep).** Add `':!scripts/agent/evals/cases'` to its exclusions: R2's eval cases are historical patch data that must name the scripts, like `tools/tests/common/agent.rs`. Both plans quote the amended grep verbatim (R3 10f, R2 23b); on R3's tree the extra pathspec matches nothing and changes nothing.
- **A11 (§1, the docs gate DG).** docs-check's knowledge-hygiene check reports every file under `docs/knowledge/plans/` except `README.md` and `_template.md` as `plans-not-empty` (main `tools/src/cmd/docs_check/checks/knowledge.rs` L62-69), and these findings are not in `scripts/docs/baseline.json`. So from each branch's plan commit until its distil commit, "docs-check clean" means: `just docs-check --json` exits 1, and its new findings are exactly the two `knowledge-hygiene plans-not-empty` lines for that branch's plan copy and contract copy. From the distil commit on, `just docs-check` exits 0. The distil is R3 Task 12d and R2 Task 24c; each plan's DG gives the exact form.
- **A12 (§2, §8, §11; R2's own rows, R2 tasks 4d, 13b, 20a, 20c, 21b).** It changes nothing R3 builds. §2's R2 typed reads are `pr_view`, `viewer_login`, `pr_checks`, `review_threads`, `issues_labelled`, `label_exists`, `run_log_failed` and the dedup `issue list --search` (no `rate_remaining`: nothing consumes it), and the supervised long read is `gh pr checks <n> --watch` over **every** check, never `--required`, bounded by `ci_wait_minutes` (the merge verdict counts every check; R2 D21 and `checks_reads_never_pass_required`). §11's R2 asks are the four path spellings plus `Bash(*/aios loop *)`, `Bash(cargo run *loop*)` and `Bash(cargo r *loop*)`, seven in all (R2 D22); §8's rule-03 R2 append names them. §9's R2 probe runs A7's saved harness over those spellings.
