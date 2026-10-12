---
author: jl + claude
date: 2026-10-11
tags: [tooling]
status: in-progress
---

# Plan: Tools R3: `aios brief` and `aios checkpoint` (port of brief.sh and checkpoint.sh)

Branch `claude/tools-r3-scripts` (team-build), branch worktree `<BW>` = `.claude/worktrees/tools-r3`, created from `origin/main` **after PR #234 merges**. R2 (`claude/tools-r2-loop`, `.claude/worktrees/tools-r2`) is built in parallel against this plan's frozen API. **R3 merges first**; R2 merges `origin/main` after R3 lands (owner decision 3).

- Spec: `docs/knowledge/discussions/2026-09-22-jl-rust-agent-tools.md` §1 (subcommand rows W234 L71-72, Protection), §2 (Invocation, Freshness), §3 (R3 row W234 L176), §4 (Order W234 L194).
- Method: `docs/knowledge/lessons/2026-09-24-jl-rust-port-parity-gotchas.md` (fixture bundle → recorder → differential → switch-over and deletion in one PR) and `docs/knowledge/lessons/2026-09-29-jl-soak-port-awk-and-process-parity.md` (read the oracle from git, normalise only what must vary).
- Anchors: since Task 0a (2026-10-11), every `W234 L<n>` and "main L<n>" names a line at `origin/main` 2d4a9f7, #234's squash merge (final head 8253f0d) on 8fa4c24. The labels are kept so this plan and the contract still read alike; they first named `.claude/worktrees/harness-teams` @ b47c756 and main @ 030b3b7 (A9).
  - Between b47c756 and 2d4a9f7, brief.sh, the shim, the justfile, settings.json, qemu-lock.sh, the guard, its tests and `.claude/CLAUDE.md` changed, so most of their anchors moved. The guard's `just_starts_qemu` is at L1719 (it still matches `QEMU_RECIPES` only and never reads the justfile), the model-routing lesson's `brief.sh` count at L105, and the `.claude/CLAUDE.md` Skills rows at L287/L289. checkpoint.sh, rule 03 and the pause, start and merge-and-cleanup skills are byte-identical, and agent-loop.md L11/L16/L18/L25/L30/L72 and rule 11 L55 have the same text.
  - The `tools/` lines cited as "main L<n>" are unchanged from 030b3b7, except `tools/tests/shim.rs`, whose brief.sh doc comment is at L623.
  - Task 0a read every anchor on 2d4a9f7 before any code, and the plan and the contract cite those lines.

## Coordination with R2: see the contract

The full agreement is [2026-10-11-jl-tools-r2-r3-contract.md](2026-10-11-jl-tools-r2-r3-contract.md) ("the contract"), with the merged amendments A1-A12 below written into it. Task 0f commits it beside this plan, byte-identical to R2's copy. Where it and this plan disagree, the contract wins, and this plan does not restate its signatures. Summary of what binds R3:
- **Order (§1; owner decision 3).** R3 merges first. R2 is built in parallel against R3's frozen API and merges `origin/main` after R3 lands. R3's PR amends the spec Order to `R1 → R4 → R3 → R2 → R5 → R5b`. One `needs-human` gate issue enforces the order: it names only R2 with a `#`, and only R3 Task 1i files it (A4).
- **API freeze (§1).** R3's first code commits (Tasks 1 and 5) are complete, tested implementations of `tools/src/gh.rs` (§2), `tools/src/merge_ready.rs` (§3), `tools/src/git.rs` (§4) and the shared harness `tools/tests/common/{fake_gh,jail}.rs` (§5, A1-A3). Their tip is `R3_API_FREEZE`. R2 vendors those five files from it and takes main's copies verbatim at convergence. After the freeze, no listed public signature changes without an amendment.
- **R3 owns** those five files; the shim's `AIOS_SHIM_STALE` handling and checkpoint's stale-rebuild exit 3 (§6; decisions 5 and 9); the `brief`/`checkpoint` recipes (§6); the D10 `labelled needs-human` blocker (§3); the saved probe harness that R2 reuses (§9, A7); and the R3 rows of every shared doc (§8). **R2 owns** the shim's `loop)` arm and the loop's exit-3 strictness (decision 2), and edits the shim only after R3 is on main.
- **Shared files (§8).** R3 edits first, on its own branch. R2 edits them only in its Phase B, on the merged tree. The only merge conflicts left are the one-line unions in `lib.rs`, `main.rs`, `cmd/mod.rs` and `tests/common/mod.rs`.
- **Settings (§11, A6, A8).** Owner edits, applied by a lead-written jq script that first checks its base blob. R3's runs on R3's branch before the ship pass; R2's only after R2's convergence.
- **Docs gate (A11).** While this plan and the contract copy are committed, `just docs-check` exits 1 with exactly their two `plans-not-empty` findings (DG, Progress).
- **Amendments.** Both branches belong to team-build, so one lead amends the contract. It edits the contract and both plans, makes one worker commit per branch with the two contract copies byte-identical, and records the amendment in its handoff file. No HEADS-UP message is sent.

### Contract amendments A1-A12 (one merged set, 2026-10-11)

R3's revision-3 list (A1-A9) and R2's v2 list (A1-A7) were two different sets under colliding numbers, and the scratch contract held neither (`grep -c 'FAKE_GH_TOOLS\|FAKE_CLAUDE_TOOLS\|evals/cases'` on it printed 0). This is the one merged set: R3's A1-A9, with A1, A4 and A7 reconciled against R2's, plus R2's evals/cases exclusion as A10, R2's docs gate as A11 and R2's own rows as A12. Before either branch commits the contract (R3 0f, R2 0b), the lead:
1. writes the set into the scratch contract as a new §14 "Amendments (2026-10-11)", in this numbering, and edits the sections each item names (§1, §2, §3, §5, §8, §9, §11, §12) to match (the scratch contract already carries §14 and the A12 rows of §2, §8, §9 and §11; the lead checks them and edits the rest);
2. R2's plan: already applied in R2 v2; do not renumber.

Each item was checked against the repo on 2026-10-11.
- **A1 (§5, fake tool lists and the extractor).** `jail.rs` defines:
  - `pub const FAKE_GH_TOOLS`: exactly the fake gh's external commands (expected `cat`, `cp`, `dirname`, `grep`, `mkdir`, `tr`);
  - `pub const FAKE_CLAUDE_TOOLS: [&str; 8] = ["cat", "dirname", "env", "git", "grep", "mkdir", "sleep", "sort"]`, R2's list: R2 owns the `FAKE_CLAUDE` script (R2 6a), which runs exactly these. (`cp` and `tr` come from the fake gh, not from `FAKE_CLAUDE`.)
  - `pub const FAKE_TOOLS`: `FAKE_GH_TOOLS ∪ FAKE_CLAUDE_TOOLS` (10 entries), all of which `jail::bin` links;
  - `pub const SH_WORDS: &[&str]` (sh/bash keywords and builtins) and `pub fn script_commands(src: &str, not_commands: &[&str]) -> BTreeSet<String>` (specified in Task 1a). The keyword list has this one name in both plans; there is no `NOT_COMMANDS` const in `jail.rs`. Callers append their own script's function names, e.g. `&[jail::SH_WORDS, BRIEF_FUNCTIONS].concat()`. R2 6a's `FAKE_CLAUDE` spec defines no functions, so R2 passes `jail::SH_WORDS` alone (plus the names of any function it later defines, by the same rule).

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
  It files only when that prints 0. R2 0f records `GATE` from R3's Decisions Made and runs the same command, expecting 1. The verify matches the blocker anywhere in R2's `merge-ready:` line (both drafts read `merge-ready: no (draft; gated by #<gate>…)`, since the `draft` blocker comes first, W234 brief L219): the anchored `[(;] ?gated by #<GATE>([,;)]|$)` of Task 1i.
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
- **A11 (§1, the docs gate DG).** docs-check's knowledge-hygiene check reports every file under `docs/knowledge/plans/` except `README.md` and `_template.md` as `plans-not-empty` (main `tools/src/cmd/docs_check/checks/knowledge.rs` L62-69), and these findings are not in `scripts/docs/baseline.json`. So from each branch's plan commit until its distil commit, "docs-check clean" means: `just docs-check --json` exits 1, and its new findings are exactly the two `knowledge-hygiene plans-not-empty` lines for that branch's plan copy and contract copy. From the distil commit on, `just docs-check` exits 0. For R3 the distil is Task 12d; the exact form is DG (Progress).
- **A12 (§2, §8, §11; R2's own rows, R2 tasks 4d, 13b, 20a, 20c, 21b).** **R2's own rows** (changes nothing R3 builds): §2's R2 typed reads are `pr_view`, `viewer_login`, `pr_checks`, `review_threads`, `issues_labelled`, `label_exists`, `run_log_failed` and the dedup `issue list --search` (no `rate_remaining`: nothing consumes it), and the supervised long read is `gh pr checks <n> --watch` over **every** check, bounded by `ci_wait_minutes` (the merge verdict counts every check); §11's R2 asks are the four path spellings plus `Bash(*/aios loop *)`, `Bash(cargo run *loop*)` and `Bash(cargo r *loop*)`; §8's rule-03 R2 append names them.

## Approach

**Goal.** Replace `scripts/agent/brief.sh` (602 lines at W234) and `scripts/agent/checkpoint.sh` (285 lines) with `aios brief [--no-fetch]` and `aios checkpoint [--handoff saved|not-saved] [--allow PATH]...`.
- Stdout, exit status and side effects match the post-#234 scripts byte for byte in every scenario, except for the divergences D1-D10 (Code Structure Decisions).
- Callers switch to `just brief` / `just checkpoint`, and the same PR deletes the scripts.
- Precompact is out of scope: #234 retired it.

**Parity-first order.** Each port goes oracle → goldens → code → switch-over → deletion, in small commits, and every commit passes the full gate.
1. **Probes before code (Task 0).** The headless permission-precedence probe and the guard baseline run before any port code exists. A settings ask that does not hold therefore shows up in Task 0, not in Task 10.
2. **Shared API first (Tasks 1 and 5 = the freeze).** The harness, `gh.rs`, `git.rs` and `merge_ready.rs` land complete and tested, and are proved to build alone on `origin/main`, so R2 can vendor them.
3. **Oracles.** Each script is read from git history with `git cat-file blob <sha>:<path>`, the R4 pattern in `tools/tests/common/soak.rs` (`OnceLock`, `CARGO_TARGET_TMPDIR`, a "fetch-depth 0" panic hint).
   - `CHECKPOINT_ORACLE` = `faf6d20295136489d6ccdfa5a253480a9f757fcc` (#166). #234 does not change checkpoint.sh, and main's copy is identical.
   - `BRIEF_ORACLE` = the commit on `origin/main` that last touched `scripts/agent/brief.sh` once #234 has merged, pinned in Task 0b. It must hold both #234's sections (`handoff-`, `Routing log`) and #246's (`print_interleaved`, `PANIC-LOCK`). Today neither b47c756 nor a189f7a holds all four.
4. **Goldens recorded from the oracle on macOS**, in their own green commit (Tasks 2 and 4), before any port code. macOS is the named recording platform: this Mac's BSD awk/sed/date, Homebrew bash 5 and jq 1.7.1. The CI run of that commit must also pass `oracle_reproduces_*_goldens` on ubuntu-26.04 (mawk and GNU tools). A scenario whose oracle output depends on the host's tools is listed in `PLATFORM_SCENARIOS` (Code Structure Decisions).
5. **Port code**, landed with `*_goldens_match_aios` and a live differential. The differential runs the oracle and `aios` in twin temporary repos, and also compares the fake-gh call logs (argv, cwd, env) and the docs-check child's argv and cwd.
6. **Switch-over and deletion.** The recipes, skills, docs and guard test change, the owner applies the settings, and the scripts are deleted. The differentials keep running from history afterwards (owner decision 7, the R4 precedent).

**Execution order.** Tasks run in the order **0, 1, 5, 2, 3, 4, 6, 7, 8, 9, 10, 11, 12**. The task numbers are kept because the contract cites them. Task 5 (`merge_ready.rs`) runs second, so the freeze holds all of §2-§5. Inside Task 10 the commits run 10h → 10a, 10b, 10c → 10g → 10d, then the owner's 10e and the lead's 10f (Task 10 states why). Inside Task 12, the real-world checks (12a-12c) run before 12d, which files the follow-ups and makes the ship pass with the distil as one commit. That commit is the last before `gh pr ready` (contract §8).

**Key gaps found during exploration** (each fixed in the task named):
- **No `gh` caller exists in `tools/` yet.** R1 and R4 never call gh. This plan adds `gh.rs`, `git.rs` and `merge_ready.rs` (Tasks 1 and 5) exactly as the contract specifies.
- **A missed brief.sh consumer.** `tools/tests/soak_interleave.rs` (main L25, L739-749; from #246, so not in W234) runs `bash ../scripts/agent/brief.sh --no-fetch` from disk, so deleting the script turns `cargo test` red. Task 10h ports it (contract §3).
- **The guard sees nothing the binary does.**
  - Today `bash scripts/agent/checkpoint.sh` asks once per pause, because the guard reads the script and meets an unresolvable push refspec.
  - `just checkpoint` gets no guard decision: `just_starts_qemu` (W234 guard L1719) checks only QEMU recipes, and settings allow `Bash(just *)` (W234 settings L120) and `Bash(cargo *)` (L119).
  - Without owner settings asks, the port would remove rule 03's once-per-pause ask. See "Guard and pause" below.
- **Direct shim calls get a misleading ask.** The W234 guard asks on `.claude/hooks/aios brief|checkpoint` with the QEMU "shell expansion" reason (contract §9, probed). So skills and fallbacks use `just` forms only, and Task 12d files a follow-up.
- **The brief's stale note.** brief.sh (W234 L615) greps docs-check's stderr for the shim's stale warning (W234 shim L581). Inside `aios brief`, that warning goes to brief's own caller, so the shim exports `AIOS_SHIM_STALE=1` instead and clears it on every other path (owner decision 9; Task 8; D6).
- **The `checkpoint it:` hint** prints `bash '<SELF>'`. Its replacement must work in any worktree, including ones branched before R3 (D5).
- **Both scripts `cd "$ROOT"` first** (brief L52-56, checkpoint L75-79). checkpoint's `add -A -- .`, `status -- .` and excludes, brief's phase-doc glob, its `-- docs/knowledge` and its docs-check child are all relative to that cwd. The `--working-directory .` fallbacks and the D5 hint can start the binary in a subdirectory. So both ports chdir to the toplevel, and `run_from_subdirectory` pins it (Tasks 2a, 4b).
- **brief reads some things from the main worktree, not the toplevel.** Handoffs come from `$MAIN_WT/.remember` (W234 brief L492), the QEMU lock from `cd "$MAIN_WT" && sh scripts/agent/qemu-lock.sh status` (L473-474), and `rel_path` labels are relative to `MAIN_WT`, so `- checkout:` reads `(main checkout)` only when ROOT = MAIN_WT (L76-79, L123). Phase docs, knowledge notes and docs-check use ROOT. Every other scenario runs from the main worktree, so `run_from_linked_worktree` pins the split (Task 4b), with a 6e mutation.
- **Outside-repo scenarios must escape the aios checkout.** `common::unique_dir` lives under `CARGO_TARGET_TMPDIR` = `<repo>/target/tmp`, inside the aios work tree, and `isolated()` removes `GIT_CEILING_DIRECTORIES`. An `outside_repo` scenario built there would resolve the real checkout, where checkpoint runs `git add -A` and could commit. So it runs under `std::env::temp_dir()` with the ceiling set after `hermetic` (main `tools/tests/cli.rs` L187 and `soak_cli.rs` L311 are the precedents), and every scenario asserts its twin's toplevel (Task 1e).
- **Guard test coupling.** `.claude/hooks/tests/test_git_push_guard.py::test_main_thread_everyday_commands_stay_allowed` (W234 L1471-1487) opens the real `scripts/agent/brief.sh`. It is a protected file, so the edit is a foreground task (Task 10c). **CI does not run the Python guard suite** (`.github/workflows/ci.yml` has no python step). The edit is checked locally only, the PR test plan says so, and Task 12d files a follow-up.
- **Host tools.**
  - The brief oracle needs jq ≥ 1.7 and bash. Both oracles call `paste`, `ps`, `pgrep` (through qemu-lock.sh) and more.
  - Every scenario runs under the contract's PATH jail (§5: no `/usr/bin`, no `/bin`). Its per-script tool lists are derived from the scripts and checked by `jail_covers_every_script_command` (A2). The extractor scans command substitutions inside double quotes, because checkpoint runs `paste`, `head`, `dirname` and `basename` only inside `"$(...)"` (L74, L196, L205, L221, L223, L250); a regex that skips quoted text would drop `paste` from the jail again.
  - bash runs by absolute path.
  - Tests fail loudly when a tool is missing and never skip silently.

**Guard and pause (owner decision 4).**
- **Today.** `/justin:pause` runs `bash scripts/agent/checkpoint.sh --handoff saved` (W234 pause SKILL.md L38). The guard asks on the main thread (an unresolvable refspec) and for an agent (a commit in an expanded directory). Headless and background runs turn the ask into a refusal, so no unattended runner can pause.
- **After the port.** `just checkpoint` reaches a compiled binary whose commit and push neither the guard nor the permission rules see. The binary has its own bounds:
  - the refspec is built only from a `claude/?*` branch (Task 3a, a hard `bail!`), and the push is never forced and never `--no-verify`;
  - the secret scan, which fails closed on a git error (D9), and the wip hold;
  - its sources sit under the Option-A gate (decision 1), and the shim runs only main's stamped build.
- **The control is the owner's settings asks** (Task 10e), one per invocation spelling: `just`, the `--justfile` fallback, the shim, the installed binary and `cargo run`. Every pause therefore prompts once, and `-p` and background-agent runs are refused, as today. There is **no guard deny** (decision 4).
  - Task 0e proves the asks hold for `-p`, for a main-thread background Bash call and for a background agent before any port code exists. Task 10f re-proves them on the committed settings.
  - Unattended pause becomes a later owner settings change, at the stage that wants it.
  - Rule 03 L22 gets the contract's general bullet (§8, with A6's spellings).

**Review focus** (each pinned by named tests):
1. **Nothing reaches origin that should not.**
   - There is no push to `main`, no force, and no push from a non-`claude/*` branch, a detached HEAD, or an in-progress merge/rebase/cherry-pick/revert/bisect/unmerged state. Wip commits are held when a ready PR exists or when gh fails or is missing.
   - Tests: `on_main_dirty`, `non_claude_branch`, `detached_head`, `*_in_progress`, `ready_pr_holds_wip`, `gh_fails_holds_wip`, `gh_missing_holds_wip`, `tracking_origin_main_pushes_branch_only`, `run_from_subdirectory`.
   - `assert_origin_invariants` checks three things in **every** checkpoint scenario: origin's `refs/heads/main` is unchanged, the bare origin's reflog shows fast-forwards only, and the fake-gh calls pass `CHECKPOINT_GH_ALLOW`.
2. **The secret stop.** Path and text regexes; new paths only; staged changes plus unpushed commits; the `--allow` exact match and `./` strip; and the fail-closed scan (D9), which never prints `Safe to /clear` and always exits 1. On a stop, the index is restored byte-exactly (`git write-tree` before = after). Tests: the #166 scenarios, `index_restored_exactly`, `scan_git_failure_fails_closed`, `scan_git_failure_on_clean_tree`, `unborn_head`, and the `scan.rs` unit tables (one row per failing scan git call).
3. **Load-bearing line formats.**
   - `- #N … `, `  merge-ready: …` and the blocker texts, which merge-and-cleanup's awk parses;
   - `checkpoint: … | wip: … | pushed: … | handoff: …`, `Safe to /clear — run /justin:start after` and `checkpoint it:`.

   Tests: the brief PR scenarios, every checkpoint golden, and `skill_contract.rs` (Task 10g), which runs the skill's own awk.
4. **jq and awk semantics.** `//` falls through only on null/false (0 threads stays 0); `max_by` keeps the last maximum; `[[:cntrl:]]` → `?`; `scan("#([0-9]+)")|tonumber|unique`; the sed fall-through on the Boots row; and the tie order of `sort -u` and `ls -t`. Tests: the `merge_ready` unit tables, `main_ci_tie`, `prs_every_blocker`, `soak_boots_row_quirk`, `soak_equal_mtime_tie*`, `no_origin_main`.
5. **Degradation.** Every GitHub failure becomes one notice line (or a `?` count) and exit 0, and a failed call stops the calls that depend on it. Outside a repo, or on a usage error, the exit is 2. Tests: `offline`, `offline_empty_stderr`, `gh_missing`, `rate_zero`, `threads_unknown`, `pr_list_fails`, `main_ci_suites_fail`, `main_ci_runs_fail`, `queue_counts_unknown`, `cli_*`, `outside_repo`.
6. **No gh writes, no merge** (mechanical, every scenario).
   - `gh::is_read` refuses every write before spawning (contract §2).
   - Each scenario's fake-gh calls must match its allowlist:
     - brief, `BRIEF_GH_ALLOW`: `api rate_limit`, `api graphql` with `reviewThreads` or `checkSuites`, the three `issue list` calls, `pr list --state open --limit 30`, `run list --branch main`;
     - checkpoint, `CHECKPOINT_GH_ALLOW`: `pr list --head`.
   - `gh.rs` spawns through `Command::new(program)` (the `run_program` seam), so a grep for `Command::new("gh")` matches nothing. The check that means something is that no file other than `tools/src/gh.rs` runs `std::process::Command` with a gh program. `git grep -n 'GhCli\|run_program' -- tools/src` lists the expected callers only: `gh.rs`, `cmd/brief/mod.rs` (builds the `GhCli`), `cmd/brief/github.rs` (reads through `GhRunner`; it names `GhCli` in a doc comment) and `cmd/checkpoint/mod.rs`.

## Progress

Conventions for every task:
- **Placement (rule 11).** The writer is a `worker`, spawned with `isolation: "worktree"`. Its first command is `git reset --hard <tip>`, and it commits only on its temporary branch, with the message the lead gives. The lead stays in the main checkout and never edits files in `<BW>`. It places the work in this order:
  1. range check: `git -C <BW> rev-list --first-parent <tip>..worktree-<name>` = the reported commits, and the oldest one's first parent = `<tip>`;
  2. `git -C <BW> merge --ff-only worktree-<name>`;
  3. `git worktree remove <path>` (never `--force`);
  4. `git -C <BW> branch -d worktree-<name>`;
  5. `git -C <BW> push -u origin claude/tools-r3-scripts`.
- **Merging `origin/main`** (on `MAIN-MOVED`, or when a check needs a newer base). A worker runs `git merge origin/main` in its temporary worktree, reset to the tip; it is never a rebase. On a conflict in `.claude/settings.json`, it runs `git merge --abort` and reports, and the Task 10e recovery (A8) applies. Agents never resolve that file.
- Tasks that edit `.claude/**` are spawned with `run_in_background: false`, so the owner prompt reaches the owner. Only the owner changes `.claude/settings.json` (Task 10e).
- Commit subject: `Tools R3 Task N: <what>`, plus the session's Co-Authored-By trailer.
- **G**, the per-task gate before every commit: `cargo fmt --check -p aios-tools && cargo clippy -p aios-tools --all-targets -- -D warnings && cargo test -p aios-tools`, then DG.
- **DG**, the docs gate (A11), run in the writer's or verifier's worktree:
  - From the Task 0f commit until the Task 12d commit: `just docs-check --json > "$TMPDIR/dc.json"; echo "rc=$?"` prints `rc=1`, and `jq -r '.findings[] | select(.new) | "\(.check) \(.target) \(.file)"' "$TMPDIR/dc.json" | sort` prints exactly these two lines:
    - `knowledge-hygiene plans-not-empty docs/knowledge/plans/2026-10-11-jl-tools-r2-r3-contract.md`
    - `knowledge-hygiene plans-not-empty docs/knowledge/plans/2026-10-11-jl-tools-r3-scripts.md`
  - From the Task 12d commit on: `just docs-check` exits 0 and prints `No new drift since baseline.`
  - Any other new line is drift. If this branch made it, the task fixes it. If it came from `main` (a fresh worktree of `origin/main` shows it too), the lead stops and reports it, and never baselines it here.
- **T(args) = N** means that `cargo test -p aios-tools <args> 2>&1 | grep -E '^test result:'` prints exactly one line, and it starts `test result: ok. N passed` (`≥ N` where marked). A filter is one TESTNAME (before `--`) or one name after `-- --ignored`.
- **Writer rules.** No `git stash`, no working around a refused command, no credentials, no `CARGO_TARGET_DIR` pointing at `<BW>`, no real `gh`, no QEMU, and no `cargo run … checkpoint` (Task 10e asks on it). No verify starts with `just -` (settings ask `Bash(just -*)`, W234 L326, which a background worker cannot answer): verifies use `just <recipe>`, `grep` or a cargo test that runs `just` as a child. Tests use only the fake gh. Only the lead uses real, read-only gh (Tasks 0d, 1h, 1i, 4e, 12a, 12d, 12e).
- **Numeric greps.** `git grep -c` prints `<path>:<count>`, so wherever a verify expects a number it uses `git grep -hc` (or `git show HEAD:<path> | grep -c`). "Prints 0 or nothing" is the one plain `git grep -c` form, since git grep omits files with no match.
- **Builds.** The shim runs main's binary.
  - A writer exercises its own build with `just tools` in its temporary worktree, then sets `AIOS_TOOLS_BIN="$PWD/target/tools/installed/aios"` in the same command.
  - The lead builds `<BW>` without writing there: `cargo build --release --locked -p aios-tools --manifest-path <BW>/Cargo.toml --target-dir <scratch>/r3-target`, giving the binary `<scratch>/r3-target/release/aios`.
- **Test-data hygiene (contract §10).**
  - No token-shaped literal in any committed file: tests build them from fragments, e.g. `concat!("gh", "p_", …)`.
  - Goldens record only `<path>\t<reason>`.
  - No committed basename matches `SECRET_PATH_RE`; the scenario names below already follow this.
  - No `/Users/`, `/home/` or `/private/` paths, and no account emails, in fixtures or goldens.
  - The added-file list is the three-dot diff (A5).
- Every parity test calls `assert_not_vacuous(&Outcome)` (Task 1e).

- [ ] **Task 0: Branch, anchors, oracle pins, probes, plan commit** (lead, main checkout; owner-attended for 0e; worker for 0f)
  - [x] 0a: After #234 merges, run `git -C <main> fetch origin && git worktree add .claude/worktrees/tools-r3 -b claude/tools-r3-scripts origin/main`. Re-read on `origin/main`:
    - every `W234 L<n>` anchor of this plan and the contract (including the moved anchors of A9: the guard, the routing lesson, and the `.claude/CLAUDE.md` Skills rows, 7 lines lower at 2d4a9f7);
    - the main-only files: `tools/tests/soak_interleave.rs` L25 and L739-749, `tools/src/cmd/docs_check/mod.rs` L187, `tools/src/cmd/docs_check/checks/knowledge.rs` L62-69 (A11) and `tools/tests/shim.rs` L473.

    Write the corrections into the plan copy that 0f commits.
    - Verify: `git -C <BW> rev-parse --abbrev-ref HEAD` prints `claude/tools-r3-scripts`; `test -f <BW>/.claude/rules/11-teams.md`; `git -C <BW> grep -hcF 'sub=${1:-}' -- .claude/hooks/aios` prints `1`.
  - [x] 0b: Pin both oracles.
    - `BRIEF_ORACLE=$(git log -1 --format=%H origin/main -- scripts/agent/brief.sh)`.
    - `git merge-base --is-ancestor f3b039b4c9c1d9cdccb2fc383ea248616159b246 $BRIEF_ORACLE` (has #246).
    - For each of `handoff-`, `print_interleaved`, `PANIC-LOCK` and `Routing log`: `git show $BRIEF_ORACLE:scripts/agent/brief.sh | grep -c -- '<p>'` ≥ 1. Use one count per pattern: a single `grep -c -e … -e …` counts lines, and main's copy alone scores 4.
    - `CHECKPOINT_ORACLE=faf6d20295136489d6ccdfa5a253480a9f757fcc`.
    - Verify: the ancestor check exits 0; each of the four counts is ≥ 1; `git diff --quiet $CHECKPOINT_ORACLE origin/main -- scripts/agent/checkpoint.sh` exits 0.
  - [x] 0c: Port inputs and the guard baseline.
    - `R=$(git rev-parse --git-common-dir)/aios-agent/port-inputs/r3`. Write `ORACLES` (both shas) and the blobs of brief.sh, checkpoint.sh and qemu-lock.sh at `BRIEF_ORACLE`, each with `git cat-file blob <sha>:<path> > $R/<name>`.
    - Write `guard-baseline.tsv`: the post-#234 guard's decision and reason for each command below. Each command is fed as a PreToolUse payload (cwd = the main checkout) to `/usr/bin/python3 .claude/hooks/git-push-guard.py`, from a scratch script in the session scratchpad. Each command runs twice: once as the main thread (no `agent_id`), and once as an agent (`agent_id` + `agent_type: worker`).
      1. `bash scripts/agent/brief.sh --no-fetch`
      2. `bash scripts/agent/checkpoint.sh --handoff saved`
      3. `just brief`
      4. `just brief --no-fetch | awk -v n="- #1 " 'index($0, n) == 1 {p = 1} p && /merge-ready:/ {print; exit}'` (the merge-and-cleanup L52 pipe)
      5. `just checkpoint --handoff saved`
      6. `(cd /abs/wt && just checkpoint)`
      7. `just --justfile /abs/main/justfile --working-directory . brief` (the brief fallback, Task 10a)
      8. `just --justfile /abs/main/justfile --working-directory . checkpoint --handoff saved` (the pause fallback)
      9. `.claude/hooks/aios brief`
      10. `.claude/hooks/aios checkpoint --handoff saved`
      11. `"$CLAUDE_PROJECT_DIR"/.claude/hooks/aios checkpoint`
      12. `(cd '/abs/wt' && just --justfile '/abs/main/justfile' --working-directory . checkpoint)` (the D5 hint)
    - Expected for the rows contract §9 probed, the same for main thread and agent: rows 1, 3, 5 and 11 → none; rows 9 and 10 → ask, with the QEMU "shell expansion" reason; row 2 → ask.
    - Rows 4, 6, 7, 8 and 12 are expected to give none, because `just_starts_qemu` (W234 guard L1719) matches recipe names against `QEMU_RECIPES` and never reads the justfile. They are recorded as found. The guard is not the pause control (decision 4), so a row that asks only adds a prompt: it is noted, not fixed. The result, as 0f commits it into the plan, is `EXPECTED_GUARD`.
    - Write `guard-expected.tsv` (the contract rows above, as `<row>\t<main|agent>\t<decision>`) and `SHA256SUMS`.
    - Verify: `(cd "$R" && shasum -a 256 -c SHA256SUMS)` prints OK for every file; `wc -l < $R/guard-baseline.tsv` = 24; `wc -l < $R/guard-expected.tsv` = 14; `awk -F'\t' 'NR == FNR {e[$1 FS $2] = $3; next} ($1 FS $2) in e && e[$1 FS $2] != $3' $R/guard-expected.tsv $R/guard-baseline.tsv` prints nothing (baseline columns: row, main|agent, decision, reason).
  - [x] 0d: Real gh shapes (read-only, lead). Capture one response for each brief and checkpoint call into `$R/gh-capture/`, and add the files to `SHA256SUMS`: `gh api rate_limit`, the threads and suites GraphQL queries, the three `issue list` calls, `pr list --limit 30 --json …`, `run list --branch main …` and `pr list --head <b>`. Task 4a copies shapes, never values, from these.
    - Verify: `ls $R/gh-capture | wc -l` = 9; `(cd "$R" && shasum -a 256 -c SHA256SUMS)` prints OK.
  - [x] 0e: **Headless permission-precedence probe** (owner-attended). It proves decision 4's asks before any port code exists (contract §9). The harness is saved under `$R/probe/` and reused by Task 10f and by R2 20c, in the A7 scheme, which this sub-task builds before any counted run.
    - `$R/probe/` holds:
      - `run-probe.sh <settings.json> <spellings.tsv> <out.tsv>`, which runs every row and writes its verdict (A7);
      - `make-settings.sh <base settings.json> <asks.txt>`, the jq generator;
      - `asks.txt`;
      - `spellings-r3.tsv` (`<id>\t<kind>\t<sub>\t<spelling>`; kinds `bash`, `bash-bg`, `monitor`, `background`; `<sub>` = `checkpoint`, or `brief` for the control);
      - a template probe directory with the A7 stubs: a `justfile` whose `brief` and `checkpoint` recipes run only `touch "$PROBE_DIR/ran-just-<recipe>"`; a `.claude/hooks/aios` stub running only `touch "$PROBE_DIR/ran-shim-$1"`; a `target/tools/installed/aios` stub running only `touch "$PROBE_DIR/ran-exe-$1"`; and `bin/cargo`, which touches `$PROBE_DIR/ran-cargo-<w>` for each `<w>` in `brief checkpoint loop` that appears as a whole argument.
    - All of these are listed in `SHA256SUMS`. The stubs carry the `loop` marker names R2 needs (`ran-shim-loop`, `ran-exe-loop`, `ran-cargo-loop`), so R2 runs the saved files unchanged.
    - **Probe settings.** `make-settings.sh` emits `{permissions: (.permissions | .ask += <asks.txt>)}` from main's `.claude/settings.json`: the real allow, ask and deny lists, with no hooks, so the settings asks alone are tested. `asks.txt` holds `Bash(just checkpoint*)`, `Bash(just --justfile * checkpoint*)`, `Bash(.claude/hooks/aios checkpoint*)`, `Bash(*/.claude/hooks/aios checkpoint*)`, `Bash(cargo run *checkpoint*)`, `Bash(target/tools/installed/aios checkpoint*)` and `Bash(*/target/tools/installed/aios checkpoint*)`.
    - **Runs.** There is one run per spelling, each in a fresh copy of the template directory with its own `PROBE_DIR`: `cd <copy> && env -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT -u CLAUDE_CODE_EFFORT_LEVEL PROBE_DIR=<copy> PATH="<copy>/bin:$PATH" claude -p --settings <probe.json> --setting-sources user --output-format stream-json --verbose "<prompt>"`. The method is the one in lesson `docs/knowledge/lessons/2026-10-06-jl-model-routing-wiring-checks.md`.
      - `bash` prompt: "Run exactly this Bash command once and nothing else: <spelling>".
      - `bash-bg` prompt: "Run exactly this Bash command once with the Bash tool's run_in_background option set to true, and nothing else; wait for its completion notification, then stop: <spelling>".
      - `monitor` prompt: "Use the Monitor tool to run exactly: <spelling>".
      - `background` prompt: "Spawn one subagent with the Agent tool and run_in_background: true, whose only job is to run exactly this Bash command once: <spelling>; wait for its completion notification, then stop".
      - The markers are read after the claude process exits, plus a 60 s grace period for the `bash-bg` and `background` rows.
    - **Spellings** (24 rows):
      - just: `just checkpoint`; `FOO=1 just checkpoint`; `just  checkpoint` (double space); `cd <copy> && just checkpoint`; `(cd <copy> && just checkpoint)`; `just --justfile <copy>/justfile --working-directory . checkpoint`; `just 'checkpoint'`; `just "checkpoint"`; `env just checkpoint`; `command just checkpoint`; `timeout 60 just checkpoint`; `just brief && just checkpoint`;
      - shim: `.claude/hooks/aios checkpoint`; `"$CLAUDE_PROJECT_DIR"/.claude/hooks/aios checkpoint`; `sh .claude/hooks/aios checkpoint`;
      - binary and cargo: `cargo run -q -p aios-tools -- checkpoint`; `cargo r -q -p aios-tools -- checkpoint`; `target/tools/installed/aios checkpoint`; `<copy>/target/tools/installed/aios checkpoint`;
      - the D5 hint: `(cd '<copy>' && just --justfile '<copy>/justfile' --working-directory . checkpoint)`;
      - `bash-bg`: `just checkpoint`;
      - `monitor`: `just checkpoint`;
      - `background`: `just checkpoint`;
      - control (`<sub>` = `brief`): `just brief`.
    - **Judging** (A7). `run-probe.sh` judges each row by the `ran-*-<sub>` markers and by the permission denials in the stream-json events (the result's `permission_denials`, or a subagent's denied tool result for the background row), never by the model's text:
      - `ran`: some `ran-*-<sub>` marker exists (for `just brief && just checkpoint`, `ran-just-brief` alone is not `ran`);
      - `refused`: no such marker, and a denial names the command;
      - `inconclusive`: neither. For example, a stub was not reached; the harness is fixed, re-saved and re-checksummed, and every row re-run.

      Record `claude --version` in `$R/probe/claude-version.txt`, and one row per run (no header) in `$R/probes.tsv` (`<id>\t<spelling>\t<verdict>\t<denials>\t<markers>`), and add both to `SHA256SUMS`.
    - **Follow-through.** Any spelling that runs extends `asks.txt`, and so the owner's ask delta (Task 10e), with a rule that covers it, and the probe is repeated for that spelling. If no settings rule can cover a spelling (for example a Monitor, `bash-bg` or background run), stop and bring the result to the owner before Task 1. That is a new fact, not a re-ask of decision 4.
    - Verify: `wc -l < $R/probes.tsv` = 24; `awk -F'\t' '$3 == "inconclusive" || ($1 != "control" && $3 != "refused")' $R/probes.tsv | wc -l` = 0; `awk -F'\t' '$1 == "control" {print $3}' $R/probes.tsv` prints `ran`; `(cd "$R" && shasum -a 256 -c SHA256SUMS)` prints OK, with `probe/run-probe.sh` and `probe/spellings-r3.tsv` among the files.
  - [ ] 0f: The owner approves this plan (plan mode plus the Fable plan gate). Before the commit, the lead applies the merged amendments A1-A12 to the scratch contract (Coordination; R2's plan: already applied in R2 v2; do not renumber).
    - **Precondition, on the scratch contract**, one marker per amendment (A1 has three: the R2 list, the keyword const, the fake-gh list): `for m in 'FAKE_GH_TOOLS' 'FAKE_CLAUDE_TOOLS: [&str; 8]' 'SH_WORDS' 'CHECKPOINT_TOOLS' 'parallel_map' 'startswith("Gate: merge PR #' 'origin/main...HEAD' 'Bash(cargo run *checkpoint*)' 'ran-shim-' 'bash-bg' 'rev-parse HEAD:.claude/settings.json' '8a28365' 'evals/cases' 'plans-not-empty' 'Bash(cargo run *loop*)'; do grep -qF -- "$m" <scratch>/r2r3/2026-10-11-jl-tools-r2-r3-contract.md || echo "MISSING $m"; done` prints nothing. R2's 0a runs the same loop.
    - A worker commits this plan as `docs/knowledge/plans/2026-10-11-jl-tools-r3-scripts.md`, with the 0a corrections, both oracle shas, `EXPECTED_GUARD` and the 0e result under Decisions Made.
    - It commits the amended contract beside it as `docs/knowledge/plans/2026-10-11-jl-tools-r2-r3-contract.md`, byte-identical to R2's copy.
    - The lead records "owned branches: R3, R2; order R3 then R2; contract amendments A1-A12" in its handoff file.
    - Verify:
      - DG, in the worker's worktree, before and after the commit (docs-check reads tracked and untracked files, `tools/src/cmd/docs_check/repo.rs` L4, so the two `plans-not-empty` findings appear as soon as the files are written; 0f is the first commit at which DG's two-finding form applies).
      - The contract §12 checks for R3 pass on the committed plan: the `GhOutcome` count is ≥ 1, and both stale-pattern greps print nothing. The lead copies the three `grep` lines from contract §12 into a scratch script, so this plan never quotes the stale patterns it must not contain.
      - `cmp <(git -C <BW> show HEAD:docs/knowledge/plans/2026-10-11-jl-tools-r2-r3-contract.md) <scratch>/r2r3/2026-10-11-jl-tools-r2-r3-contract.md` exits 0, run after the precondition loop passes (so the cmp compares the merged set).
      - The precondition loop, re-run with `git -C <BW> show HEAD:docs/knowledge/plans/2026-10-11-jl-tools-r2-r3-contract.md` as its input (`grep -qF -- "$m" <(…)`), prints nothing.

- [ ] **Task 1: Shared test harness, `gh.rs`, `git.rs`, R3's oracle and twin-repo support; draft PR; gate issue** (worker, background; the lead runs 1h and 1i)
  - [x] 1a: `tools/tests/common/jail.rs` (new; contract §5 with A1-A3).
    - `jail::bin(dir, tools) -> PathBuf` symlinks each tool, plus `sh` and every entry of `FAKE_TOOLS`, resolved once from the host PATH. It panics on a missing tool, and refuses `gh` and `claude`.
    - `jail::hermetic(cmd, fake_bin, jail_bin)` applies `common::isolated()`, removes every `GH_*`, `GITHUB_*`, `AIOS_*` and `CLAUDE*` variable, and sets only `PATH=<fake_bin>:<jail_bin>`, `TZ=UTC` and `LC_ALL=C`. It sets no git identity: R3's runners add the identity env after it (Task 1e), and R2's scenario builder writes it into the clone's `.git/config` (contract §5).
    - `jail::assert_no_command_not_found(stderr)`.
    - `pub const SH_WORDS: &[&str]` (sh/bash keywords and builtins; the one keyword list, A1) and `pub fn script_commands(src: &str, not_commands: &[&str]) -> BTreeSet<String>`, a small lexer rather than one regex. It returns the first word of every simple command that is not in `not_commands`:
      - **Command starts:** line start, and after `|`, `;`, `&&`, `||`, `&`, `(`, `{`, `!`, `then`, `do`, `else`, `if`, `elif`, `while` and `until`, and after a case pattern (`<pattern>)` opening a `case` arm).
      - **Prefixes skipped:** leading `NAME=value` assignments (so `GH_PROMPT_DISABLED=1 gh …` yields `gh`, and `ALLOW_LIST="$ALLOW" awk …` yields `awk`) and redirections.
      - **Recursion:** the body of every command substitution, `$(...)` and backticks, is scanned as a script of its own, wherever it appears, including inside double-quoted strings and here-documents with an unquoted delimiter. So checkpoint L196's `"…$(printf … | paste -sd ',' - | sed …)"` yields `printf`, `paste` and `sed`.
      - **Skipped text:** comments; single-quoted text (awk and jq programs); the literal parts of double-quoted strings; escaped backticks (`` \` ``, Markdown in brief's echo lines, e.g. L123); arithmetic `$((…))`; and here-document bodies with a quoted delimiter.
      - Callers pass `&[SH_WORDS, <their script's function names>].concat()` (A1).
    - `pub const` tool lists:
      - `FAKE_GH_TOOLS`, `FAKE_CLAUDE_TOOLS` (A1: R2's 8 entries, `[&str; 8]`) and `FAKE_TOOLS` (their union);
      - `BRIEF_TOOLS`, `CHECKPOINT_TOOLS` (holds `paste`) and `QEMU_LOCK_TOOLS`;
      - `ORACLE_TOOLS`, their union, with no `flock`, `lockf`, `uniq`, `xargs` or `readlink`.

      Each entry is an external command some line runs, never a builtin.
    - `jail.rs` and `fake_gh.rs` use only `super::{isolated, unique_dir}` and each other (A3).
    - Verify: T(`--test agent_support jail_`) = 2 (`jail_has_no_gh_or_claude`, `jail_covers_every_script_command`); T(`--test agent_support script_commands_`) = 1 (the extractor's unit table, 1f).
  - [x] 1b: `tools/tests/common/fake_gh.rs` (new; contract §5).
    - `pub const SCRIPT`: the `#!/bin/sh` fake. It finds its root from `$0`, never from the environment, and uses the `<root>/gh/<NN>-<label>/{match,out[.K],err[.K],rc[.K]}` layout with ordinals.
    - The per-call log is `<root>/gh-calls/<SEQ>/{argv,cwd,env,fixture,body}`. The fake exits 99 on an unknown call and 98 on a `<SEQ>` race, and appends its own stderr to `<root>/fake-stderr`.
    - `fake_gh::render`; `FakeGh::install`, `.fixture(label, regex)` with `.out/.err/.rc/.nth`, `.calls()`, `.assert_all_matched()`, `.assert_calls_match(allow)` and `.assert_none_match(deny)`.
    - Verify: T(`--test agent_support fake_gh_`) = 3 (`fake_gh_unknown_call_exits_99`, `fake_gh_ordinals_advance`, `fake_gh_logs_cwd_env_and_body`).
  - [x] 1c: `tools/src/gh.rs` (new; `pub mod gh;` in `lib.rs` between `cmd` and `paths`).
    - Exactly contract §2: `GhOutcome`, `GhOutcome::first_stderr_line`, `GhEnv` with `INHERIT` and `apply`, `GhRunner`, `GhCli`, `run_program` and `is_read`.
    - No write helpers, no forced environment, and no `GH_` literal in the file. The module doc states the read-only rule.
    - Unit tests (contract §2): `run_program_missing_is_missing`, `nonzero_exit_is_failed_with_stderr`, `refused_argv_spawns_nothing`, `env_set_and_remove_applied` (with `AIOS_GHENV_TEST_SET` and `AIOS_GHENV_TEST_REMOVE`), and `is_read_table` (every brief and checkpoint argv passes; the contract's write list fails).
    - Verify: T(`--lib gh::`) ≥ 5; `git grep -c 'GH_' -- tools/src/gh.rs` prints 0 or nothing.
  - [x] 1d: `tools/src/git.rs` (new; `pub mod git;`).
    - Exactly contract §4: `command`, `out` → `Option<Vec<u8>>`, `text`, `ok`, `output`, `worktrees` and `branch`, all inheriting the environment unchanged.
    - Unit tests: `git_command_sets_no_env` (`Command::get_envs().count() == 0`), `out_chomps_trailing_newlines`, `out_is_none_on_failure`, `worktrees_reads_porcelain_bytes` (with a non-UTF-8 path) and `branch_is_none_when_detached`.
    - Verify: T(`--lib git::`) ≥ 5.
  - [x] 1e: `tools/tests/common/agent.rs` (new).
    - `tools/tests/common/mod.rs` gains `pub mod agent; pub mod fake_gh; pub mod jail;`, alphabetical with `fixture`, `soak` and `soak_fake`.
    - It also gains `pub fn parallel_map`, moved from `tools/tests/docs_check_parity.rs` L56, which then calls `common::parallel_map` (no copy).

    `agent.rs` provides:
    - The `BRIEF_ORACLE` and `CHECKPOINT_ORACLE` consts (full shas), and `oracle_script(Which::{Brief, Checkpoint, QemuLock}) -> PathBuf`. It runs `git -C <repo root> cat-file blob` under `isolated()`, caches in a `OnceLock`, writes under `CARGO_TARGET_TMPDIR`, and panics with "Tools (host) needs fetch-depth 0".
    - `require_tools() -> HostTools { bash: PathBuf }`. It resolves `bash`, `git`, `sh` and `jq` on the host PATH and panics, never skips, unless all four resolve and `jq --version` is ≥ 1.7.
    - `FAKE_DOCS`: a `#!/bin/sh` program, `aios-fake-docs`, installed in `<fake_bin>`.
      - It finds its root from `$0`, logs its argv (NUL-separated) and `pwd -P` under `<root>/docs-calls/<SEQ>/{argv,cwd}`, prints `<root>/docs/{out,err}` and exits with `rc`.
      - If `<root>/docs/stale` exists, it first prints the shim's exact stale warning (W234 shim L581) to stderr.
      - Its commands are a subset of `FAKE_GH_TOOLS`.
      - `docs_calls(root)` reads the log.
    - The `Scenario` builder, which makes deterministic twins:
      - a bare `origin` with `core.logAllRefUpdates=always`, a clone, optional linked worktrees and files, and an optional `run_cwd`: a subdirectory of the twin root to run from, defaulting to the root.
      - Every base commit contains, tracked and identical in both twins, `.claude/hooks/aios` = `#!/bin/sh\nexec aios-fake-docs "$@"` (mode 755), so brief.sh's `[ -x .claude/hooks/aios ]` runs FAKE_DOCS. Unless the scenario says otherwise, it also contains `scripts/agent/qemu-lock.sh` from `oracle_script(QemuLock)`.
      - Fixed identity for setup (`common::git()`, which passes `-c user.name=aios-test -c user.email=aios-test@example.invalid`) and for every oracle and aios run (the identity env below). Dates are **pinned for setup and for every oracle and aios run**: `GIT_AUTHOR_DATE` = `GIT_COMMITTER_DATE` = `2026-01-01T00:00:00Z` + n, with n per commit step. File mtimes are set with `File::set_modified`.
    - Each run gets its own materialisation:
      - `run_oracle(&Scenario, args)` runs `<HostTools.bash> <oracle> args` in twin A (the absolute path, A2), with cwd `run_cwd`.
      - `run_aios(&Scenario, sub, args)` runs `CARGO_BIN_EXE_aios` in twin B, with cwd `run_cwd`. For brief, it sets `AIOS_BRIEF_DOCS_CHECK=aios-fake-docs`.
      - Both go through `jail::hermetic` with `ORACLE_TOOLS`, after asserting that the twins' `git status --porcelain` output is identical. Both build their `Command` through one helper, `twin_command(&Scenario, Twin, program)`, which applies `hermetic`, then the identity env, then the pinned dates.
      - **The identity env (pinned committer for checkpoint's `wip` commit).** `hermetic` keeps `isolated()`'s `GIT_CONFIG_GLOBAL=/dev/null` and `GIT_CONFIG_NOSYSTEM=1`, so a run has no `user.email`: on the CI runner checkpoint's commit fails ("unable to auto-detect email address"), and on the Mac git falls back to `juslee@<host>` (machine-dependent shas, and an identity leak into goldens). So `twin_command`, **after** `hermetic`, sets `GIT_AUTHOR_NAME` = `GIT_COMMITTER_NAME` = `aios-test` and `GIT_AUTHOR_EMAIL` = `GIT_COMMITTER_EMAIL` = `aios-test@example.invalid`. Chosen over writing `user.name`, `user.email` and `commit.gpgsign=false` into each twin's `.git/config` (contract §5's form for R2's stage env, which `env_clear`s): the env form also covers linked-worktree runs and overrides any inherited `GIT_AUTHOR_*`/`GIT_COMMITTER_*` that `isolated()` does not remove, and `commit.gpgsign` cannot be set, because no global or system config is read.
      - **The toplevel assertion.** Before each run, for each twin, the runner asserts that `git -C <run_cwd> rev-parse --show-toplevel` (under the same `hermetic` env, canonicalised) equals that twin's root (or, for a linked-worktree run, that worktree's root). A scenario can then never run against the enclosing aios checkout: the twins live under `CARGO_TARGET_TMPDIR`, inside the aios work tree, and `isolated()` removes `GIT_CEILING_DIRECTORIES`.
      - **`Scenario::outside_repo()`**, the one exception. Its run directory is a fresh directory under `std::env::temp_dir()` (canonicalised, outside every checkout; not `unique_dir`), and the runner sets `GIT_CEILING_DIRECTORIES=<its parent>` **after** `hermetic`, as main `tools/tests/cli.rs` L187 and `soak_cli.rs` L311 do. The runner first asserts that `git -C <dir> rev-parse --show-toplevel` fails there. Both ports' `outside_repo` scenarios use it.
      - Both return `Outcome {code, stdout, stderr, gh_calls, docs_calls, repo_state}`.
    - `assert_not_vacuous(&Outcome)`:
      - `fake.assert_all_matched()`;
      - `gh_calls` is non-empty where the scenario expects gh;
      - `jail::assert_no_command_not_found` on the run's stderr and on `<root>/fake-stderr`;
      - for brief: exactly 1 docs call, except in `cli_*` and `outside_repo`; and the `\n## ` section count is ≥ 1 for the error scenarios and equal to the oracle's for the rest.
    - `assert_origin_invariants(&Outcome)`: origin's `refs/heads/main` equals the pre-run sha, and every origin reflog entry's old sha is zero or an ancestor of its new sha.
    - `BRIEF_GH_ALLOW` and `CHECKPOINT_GH_ALLOW` (regex tables, Review focus 6). The label → regex fixture tables `BRIEF_GH_FIXTURES` (`rate-limit`, `issues-needs-human`, `issues-agent-ready`, `issues-agent-working`, `graphql-threads`, `graphql-suites`, `pr-list`, `run-list`) and `CHECKPOINT_GH_FIXTURES` (`pr-list-head`). A scenario installs only the fixtures it expects to be called.
    - `normalize()` replaces exactly these named rules and compares everything else raw:
      - `root`: each twin's root, as given and canonicalised (`pwd -P` on macOS), → `<ROOT>`;
      - `origin` → `<ORIGIN>`;
      - `header_date`: `# AIOS brief - …`;
      - `git_cr`: `(%cr)`;
      - `since_7_days`: the date after `since 7 days ago (no handoff or marker yet):`.

      `oracle_renames()` applies the documented renames to oracle output (the D1 prefixes, the D5 hint) and nothing else.
    - `DIVERGENT_SCENARIOS` (scenario → D-number) lists the aios-only goldens, which are excluded from the oracle differential. `PLATFORM_SCENARIOS` (scenario → host tool and oracle line) lists scenarios whose oracle comparison runs on macOS only; elsewhere it prints `oracle skipped on <os>: <scenario> (<reason>)`.
    - Verify: T(`--test agent_support oracles_load_from_history`) = 1; T(`--test agent_support twin_repos_have_identical_heads`) = 1; T(`--test docs_check_parity`) ≥ 1 (the moved `parallel_map`).
  - [x] 1f: `tools/tests/agent_support.rs` self-tests. Each is written in the sub-task whose code it tests, so the 1a-1e verifies run them:
    - **The contract §5 set:**
      - `jail_has_no_gh_or_claude`.
      - `jail_covers_every_script_command`:
        - for brief.sh and checkpoint.sh at their oracle shas and qemu-lock.sh at `BRIEF_ORACLE`, `jail::script_commands(blob, &[jail::SH_WORDS, <that script's functions>].concat())` minus A2's removed words equals `BRIEF_TOOLS`, `CHECKPOINT_TOOLS` and `QEMU_LOCK_TOOLS` respectively. The function lists (`BRIEF_FUNCTIONS`, `CHECKPOINT_FUNCTIONS`, `QEMU_LOCK_FUNCTIONS`) are consts in `agent_support.rs`, not in `jail.rs`;
        - `script_commands(fake_gh::SCRIPT, jail::SH_WORDS)` equals `FAKE_GH_TOOLS`;
        - `script_commands(FAKE_DOCS, jail::SH_WORDS)` is a subset of `FAKE_GH_TOOLS`;
        - `command -v <tool>` succeeds under `hermetic` for every `ORACLE_TOOLS` and `FAKE_TOOLS` entry;
        - each function-list entry (a function the script defines, or a false positive) carries a line cite.
    - `script_commands_scans_substitutions_and_compound_starts`, a unit table over literal lines from the oracles (each row cites its line):
      - checkpoint L196 (`paste` and `sed` inside a double-quoted `$(...)`), L205 (`head`), L74 (`dirname`, `basename`, nested `$(...)`), L190 (`git` after `elif`), L198 (`git` after `if`) and L219 (`gh` after an assignment prefix);
      - brief L111 (`date` inside `"$(date …)"`) and L123 (escaped backticks yield nothing);
      - negative rows: awk's `print` inside a single-quoted program (checkpoint L108-113) and jq's `select` inside single quotes yield nothing.
      - `fake_gh_unknown_call_exits_99`, `fake_gh_ordinals_advance` and `fake_gh_logs_cwd_env_and_body`.
      - `fakes_are_posix_sh` (`sh -n` on the fake gh, FAKE_DOCS and the twin stub).
    - `fake_docs_logs_argv_and_cwd`: run from a subdirectory, it logs `docs-check --json` and that directory.
    - `oracles_load_from_history`: each blob is non-empty, and its first line is `#!/usr/bin/env bash`.
    - `twin_repos_have_identical_heads`.
    - `checkpoint_wip_identity_is_pinned`: a `claude/x` scenario with one dirty file and the `pr-list-head` fixture. The oracle's `checkpoint.sh --handoff not-saved` runs in each twin through `twin_command` (the one env path of `run_oracle` and `run_aios`; the binary has no `checkpoint` until Task 3). In both twins the first stdout line contains `wip: committed`, the two `rev-parse HEAD` shas are equal, and `git log -1 --format='%an <%ae>|%cn <%ce>'` prints `aios-test <aios-test@example.invalid>|aios-test <aios-test@example.invalid>`. It is not vacuous on either platform: without the identity env the commit fails on the CI runner and records `juslee@<host>` on the Mac.
    - `gh_allowlists_reject_writes`: `pr merge 1` and a `mutation` graphql fail both allowlists.
    - `oracle_smoke_brief_and_checkpoint`: the oracle's `brief.sh --no-fetch` and `checkpoint.sh` run in one scenario with the fake gh. brief exits 0 with the oracle's full `## ` header set, and checkpoint's first line starts `checkpoint: `. Both pass `assert_not_vacuous` and their allowlists.
    - `outside_repo_scenarios_escape_the_checkout`: `Scenario::outside_repo()`'s directory is not under `env!("CARGO_MANIFEST_DIR")/..`, and `git rev-parse --show-toplevel` fails there under the runner's env.
    - Verify: T(`--test agent_support script_commands_`) = 1; T(`--test agent_support outside_repo_`) = 1; T(`--test agent_support checkpoint_wip_identity_is_pinned`) = 1; T(`--test agent_support`) = 14; G.
  - [x] 1g: The worker commits Task 1 in one commit: the harness, `gh.rs`, `git.rs`, `agent.rs` and the `parallel_map` move.
    - Verify (in the writer's worktree): `git show --name-only --format= HEAD | grep -cE '^tools/(src/(gh|git)\.rs|tests/common/(agent|fake_gh|jail)\.rs)$'` = 5; G.
  - [x] 1h: **Prove the harness on CI.** The lead places the range, pushes, and opens a draft PR: `gh pr create --draft --label team-build --title "Tools R3: aios brief and aios checkpoint" --body "R3 in progress: shared API and harness first; plan in docs/knowledge/plans"`.
    - Verify: `gh pr checks <n>` shows "Tools (host)" passing, and `gh run view <run-id> --log | grep -cE 'test (oracle_smoke_brief_and_checkpoint|jail_covers_every_script_command) \.\.\. ok'` = 2, so both ran on ubuntu-26.04 and were not skipped. A tool missing on that image fails here. It is resolved by a contract amendment to the tool lists, never by a skip.
  - [ ] 1i: **The merge-order gate** (contract §1 with A4; R3 is the only filer). The lead runs this as soon as both draft PRs exist, right after whichever opens second (R3 1h or R2 0d).
    - First: `gh issue list --label needs-human --state open --limit 1000 --json number,title --jq '[.[] | select(.title | startswith("Gate: merge PR #<R2> "))] | length'`. File only when it prints 0; otherwise record the existing number.
    - File: `gh issue create --label needs-human --title 'Gate: merge PR #<R2> only after the R3 scripts PR (claude/tools-r3-scripts) merges' --body '<names R3 only by branch or https URL, never as #<n>>'`. Record `GATE=<n>` under Decisions Made of both plans, in the next worker commit on each branch.
    - Verify, from the main checkout (both PRs are drafts, so R2's line reads `merge-ready: no (draft; gated by #<GATE>…)`):
      - `bash scripts/agent/brief.sh --no-fetch | awk -v n='- #<R2> ' 'index($0, n) == 1 {p = 1} p && /merge-ready:/ {print; exit}' | grep -cE '[(;] ?gated by #<GATE>([,;)]|$)'` = 1;
      - the same awk with `n='- #<R3> '`, piped to `grep -cE '(gated by|named in needs-human) #<GATE>([,;)]|$)'`, prints 0;
      - the existing-gate check (the first command of this sub-task) now prints 1.

- [ ] **Task 5: `tools/src/merge_ready.rs`, the verdict brief and R2 share** (worker, background; runs second and completes the freeze)
  - [x] 5a: `tools/src/merge_ready.rs` (`pub mod merge_ready;` between `git` and `paths`).
    - Exactly contract §3: `PR_FIELDS`, `NEEDS_HUMAN_ISSUES_ARGS`, `clean`, `refs`, `alt`, `check_name`, `CheckState`, `check_state`, `gates`, `labelled_needs_human`, `blockers` and `pr_lines`.
    - The blocker order is brief.sh's 11 (W234 L201-241), with `labelled needs-human` (D10) in position 4.
    - The module doc is the contract's provenance sentence (`former scripts/agent/brief.sh (blob at <BRIEF_ORACLE>, L201-241)`).
    - Verify: `git grep -hcE 'former `?scripts/agent/brief\.sh`? \(blob at' -- tools/src/merge_ready.rs` prints `1`, the one allowed provenance mention (10f's grep exempts exactly this form).
  - [x] 5b: Unit tables:
    - every blocker alone and all together (12);
    - conclusion `""` is pending; `false` vs `null`; 0 threads printed as 0; `#0012` → 12;
    - a value beyond 2^53 parsed exactly (D4);
    - title gate vs body name; unknown gates; control characters;
    - the DRAFT state is not a blocker; a null `mergeable` prints `mergeable: null`;
    - `labelled_needs_human_blocks`;
    - the `draft` blocker comes first, so a draft gated PR reads `no (draft; gated by #N)` (the A4 verify's shape).

    `PR_FIELDS` and `NEEDS_HUMAN_ISSUES_ARGS` are spelled as brief.sh's argv (W234 L198, L166). Lib tests cannot read the oracle, so the Task 7 differential, which compares the gh argv logs, proves them.
    - Verify: T(`--lib merge_ready::`) ≥ 15; G.
  - [x] 5c: **Freeze.** After placement, the lead records `R3_API_FREEZE=<BW tip>` under Decisions Made of both plans, in the next worker commit on each branch.
    - Verify, all in worker worktrees, with no commit:
      1. Reset to the freeze sha: `cargo test -p aios-tools` passes, and T(`--test agent_support`) = 15 (14 at Task 1; the lead review fix added fake_gh_ordinals_follow_the_contract_rule).
      2. **Vendorability (A3)**, in a worker worktree reset to `origin/main`:
         - run `git checkout <R3_API_FREEZE> -- tools/src/gh.rs tools/src/git.rs tools/src/merge_ready.rs tools/tests/common/fake_gh.rs tools/tests/common/jail.rs`;
         - add `pub mod gh; pub mod git; pub mod merge_ready;` to `tools/src/lib.rs` and `pub mod fake_gh; pub mod jail;` to `tools/tests/common/mod.rs`;
         - T(`--lib gh::`) ≥ 5, T(`--lib git::`) ≥ 5 and T(`--lib merge_ready::`) ≥ 15 pass;
         - `cargo build -p aios-tools --tests` exits 0, and `cargo clippy -p aios-tools --all-targets -- -D warnings` exits 0;
         - then `git reset --hard origin/main`, and the lead removes the worktree (no commits to place).
      3. After the next commit: `git -C <BW> show HEAD:docs/knowledge/plans/2026-10-11-jl-tools-r3-scripts.md | grep -cE 'R3_API_FREEZE = [0-9a-f]{40}'` prints `1`.

- [ ] **Task 2: Checkpoint scenarios and goldens recorded from the oracle (no port code)** (worker, background)
  - [x] 2a: `tools/tests/checkpoint_parity.rs`, with a `scenario!` macro whose invocations each start at column 0 as `scenario!(<name>, …)` and declare one scenario-table entry. From Task 3 on, the macro also generates one `#[test]` named after the scenario, which runs `aios` on it and checks its golden. So `cargo test -p aios-tools --test checkpoint_parity <name>` runs one scenario. In this task it generates the table entry only. 52 scenarios:
    - the 15 from #166: `sensitive_name_new_doc`, `pem_line_in_added_text`, `key_file_in_earlier_local_commit`, `allow_matches_flagged_path`, `allow_matches_nothing`, `allow_dot_slash_prefix`, `other_worktree_dirty_from_claude_worktree`, `other_worktree_dirty_from_main`, `gh_fails_holds_wip`, `gh_missing_holds_wip`, `ready_pr_holds_wip`, `draft_pr_pushes_wip`, `non_wip_commits_push_without_gh`, `merge_in_progress`, `detached_head`;
    - #166's 5 round-1 extras: `env_in_new_untracked_dir`, `pem_in_keys_dir`, `modified_tracked_creds_not_flagged`, `clean_tree_unpushed_pushes`, `real_conflict_merge`;
    - 24 gaps:
      - `commit_hook_fails`, `push_rejected_non_ff`, `non_claude_branch`, `on_main_dirty`;
      - `rebase_in_progress`, `cherry_pick_in_progress`, `revert_in_progress`, `bisect_in_progress`, `unmerged_paths`;
      - `remember_and_pycache_excluded`, `index_restored_exactly`, `quoted_non_ascii_paths`, `missing_other_worktree_dir`, `allow_override_paragraph`, `nothing_to_do_safe_to_clear`, `handoff_saved`, `handoff_not_saved`;
      - `tracking_origin_main_pushes_branch_only`: a `claude/x` worktree whose upstream is `origin/main`;
      - `outside_repo`, built with `Scenario::outside_repo()` (Task 1e: a directory under `std::env::temp_dir()`, `GIT_CEILING_DIRECTORIES` set after `hermetic`), never under `unique_dir`, so it can never reach the aios checkout;
      - `write_tree_fails`: a `<fake_bin>/git` wrapper that execs the real git by absolute path, except for `write-tree`;
      - `unborn_head` (`@ none`): the scan's `log` calls fail in the oracle and are skipped in the port;
      - `allow_duplicates_kept_in_unused`, `allow_on_blocked_branch_all_unused`;
      - `run_from_subdirectory`: a dirty `notes-root.txt` at the root and a dirty `docs/notes-sub.txt`, with the run from `docs/`. It asserts that `git ls-tree -r --name-only HEAD` lists both and that `git status --porcelain` is empty after the run; it is differential.
    - 6 CLI: `cli_allow_empty`, `cli_allow_dash`, `cli_handoff_bad`, `cli_handoff_twice_last_wins`, `cli_unknown`, `cli_help` (D1, aios-only);
    - 2 divergences (D9, aios-only; each uses a `<fake_bin>/git` wrapper that execs the real git by absolute path except for the one failing call, which prints one stderr line and exits 128):
      - `scan_git_failure_fails_closed`: a dirty new file and one unpushed non-wip commit; the wrapper fails `log -p`. It pins: index unchanged (`write-tree` before = after), no new commit, origin refs unchanged (the unpushed commit is not pushed either), the D9 output (Code Structure D9), exit 1;
      - `scan_git_failure_on_clean_tree`: nothing to commit and nothing unpushed; the wrapper fails the first scan call, `diff --cached --name-only`. Here the script's own logic would print `Safe to /clear` and exit 0 (L276-278); it pins exit 1 and no `Safe to /clear` line.
    - Verify: `grep -c '^scenario!' tools/tests/checkpoint_parity.rs` = 52.
  - [x] 2b: Golden format, `tools/tests/golden/checkpoint/<name>.golden`, with these sections:
    - `exit`, `stdout`, `stderr`;
    - `origin refs` (`git -C origin for-each-ref --format='%(refname) %(objectname)'`);
    - `HEAD message` (`git log -1 --format=%B`);
    - `index` (`git write-tree` before / after);
    - `upstream` (`git config branch.<b>.merge`);
    - `gh calls` (rendered argv, cwd as `<ROOT>`, env).

    Every scenario also runs `assert_not_vacuous`, `assert_origin_invariants` and `fake.assert_calls_match(CHECKPOINT_GH_ALLOW)`.
    - Verify: `grep -c 'assert_origin_invariants' tools/tests/checkpoint_parity.rs` ≥ 1, inside the shared scenario runner (one call site covers all scenarios).
  - [x] 2c: `#[ignore] record_checkpoint_goldens` writes the 49 non-divergent goldens from the **oracle** (`AIOS_BLESS_GOLDENS=1`, through `oracle_renames()`). The only active test this task adds is `oracle_reproduces_checkpoint_goldens`.
    - Verify: `AIOS_BLESS_GOLDENS=1 cargo test -p aios-tools --test checkpoint_parity -- --ignored record_checkpoint_goldens` exits 0; then T(`--test checkpoint_parity`) = 1.
  - [x] 2d: Hygiene and counts.
    - Verify: `ls tools/tests/golden/checkpoint/*.golden | wc -l` = 49; `git grep -n -e /Users/ -e /home/ -e /private/ -- tools/tests/golden/checkpoint` prints nothing; the contract §10 token grep over `tools` prints nothing; G.
    - After placement and push, the lead checks `gh run view <run-id> --log | grep -c 'test oracle_reproduces_checkpoint_goldens ... ok'` = 1 (ubuntu-26.04). A failure there adds the scenario to `PLATFORM_SCENARIOS`, with its tool and oracle line, in a follow-up worker commit.
    - An identity or sha mismatch (a failed `wip` commit, an author other than `aios-test`, or a commit sha that differs between twins or between macOS and CI) is never a `PLATFORM_SCENARIOS` case: it is a harness bug in `twin_command`'s identity env or date pinning (Task 1e), fixed there in a follow-up worker commit.

- [x] **Task 3: `aios checkpoint` (port)** (worker, background)
  - [x] 3a: `tools/src/cmd/checkpoint/mod.rs`: `pub fn run(args: &[OsString], cwd: &Path, out: &mut dyn Write, err: &mut dyn Write) -> anyhow::Result<u8>`.
    - **Parser.** A hand parser takes the arguments in argv order, as the script does:
      - `--allow`: empty or `-*` → exit 2; strip one `./`;
      - `--handoff saved|not-saved`: the last one wins;
      - `-h`/`--help`: new help text naming `aios checkpoint`, the options, the exit statuses and the side effects, with no script path and no bash note (D1);
      - anything unknown → exit 2.

      Messages start `checkpoint: …` (D1).
    - **The toplevel.** It resolves `git rev-parse --show-toplevel` from `cwd`; outside a repo it prints `checkpoint: not inside a git repository` and exits 2. Every later git call, gh call and pathspec uses the toplevel as cwd, as the script's `cd "$ROOT"` does (L79).
    - **Blocking order.** An empty branch → detached; a `claude/?*` branch → `in_progress()` (MERGE_HEAD, CHERRY_PICK_HEAD, REVERT_HEAD, rebase-merge or rebase-apply, BISECT_LOG, `ls-files -u`); then `main`; then any other branch.
    - **Commit path**, as checkpoint.sh L176-209:
      - a `write-tree` snapshot;
      - `add -A -- . <EXCLUDES>`, with its exit status unchecked (kept; H2 is a follow-up);
      - the scan, then the split into allowed and blocked paths;
      - `read-tree` on a stop, a scan failure (D9) or a commit failure;
      - the commit message's 3-4 paragraphs, with hooks run.
    - **Push.**
      - The `wip:` hold goes through `GhCli { cwd: <toplevel>, env: CHECKPOINT_GH_ENV }`, with the identical `--jq`. `const CHECKPOINT_GH_ENV: GhEnv = GhEnv { set: &[("GH_PROMPT_DISABLED", "1")], remove: &[] }` is used only on `pr list --head`.
      - `GhOutcome::Missing` → `PR state unknown: gh is not installed`; `Failed` → `first_stderr_line`.
      - Then `git::command(<toplevel>).env("GIT_TERMINAL_PROMPT", "0")` runs `push -q -u origin refs/heads/<b>:refs/heads/<b>`.
      - The refspec comes from `fn push_refspec(b: &ClaudeBranch) -> String`, where a `ClaudeBranch` can be constructed only from a string matching `claude/?*`. The call site re-checks, and `bail!`s (exit 2, nothing pushed) if it does not hold: a hard check, not a `debug_assert!`.
      - The rejection-reason extraction is the script's.
    - **Output.** The recount, the line formats, the unused allows, the exit-3 block, and the other worktrees with the D5 hint (`<main>` = the first porcelain worktree), then the final lines.
    - Verify: T(`--lib checkpoint::tests::`) ≥ 1 (the parser table).
  - [x] 3b: `tools/src/cmd/checkpoint/scan.rs`.
    - `SECRET_PATH_RE` and `SECRET_TEXT_RE` as `regex::bytes`, copied verbatim from the oracle; the path regex uses `(?i-u)` (D8). `EXCLUDES`.
    - `new_paths()`, with `-c core.quotePath=false --no-renames --diff-filter=A` (staged changes plus `log HEAD --not --remotes=origin`).
    - `added_lines(patch: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)>`, a line-for-line port of the awk.
    - `scan(repo) -> Result<Vec<(Vec<u8>, &'static str)>, ScanError>`, sorted and unique bytewise (D3). It is `scan_with(&mut |args| git::output(repo, args))`, where `scan_with(run: &mut dyn FnMut(&[&str]) -> std::io::Result<std::process::Output>)` is the unit-test seam.
      - **D9 (owner decision 8, H1).** The four scan git calls are, in order: (1) `diff --cached --name-only --no-renames --diff-filter=A`, (2) `log --format= --name-only --no-renames --diff-filter=A HEAD --not --remotes=origin`, (3) `diff --cached --no-color --no-ext-diff --no-textconv -U0`, (4) `log -p --format= --no-color --no-ext-diff --no-textconv -U0 HEAD --not --remotes=origin` (each with `-c core.quotePath=false`, checkpoint.sh L118-123). Any of them failing to spawn or exiting non-zero returns `ScanError { call, first_stderr_line }`, failing closed. The exception: calls 2 and 4 are not issued when `rev-parse -q --verify HEAD` fails. An unborn HEAD has no local commits to scan, and skipping matches the oracle's empty result there.
      - On `ScanError`, the caller runs `read-tree <pre_index>`, commits nothing, pushes nothing, and prints exactly the D9 output (Code Structure Decisions, D9): never `Safe to /clear`, and always exit 1.
    - Unit tables: a hit and a near-miss for each regex alternative; `+++ /dev/null`; a quoted path; invalid-UTF-8 path bytes; `ſecret` and `K` (Kelvin) not folded; token-shaped inputs built from fragments.
    - `scan_fails_closed_on_each_git_call`: a table over a fake `run` (synthetic `Output`s, with `ExitStatusExt::from_raw`, no process spawned) in which call k (k = 1..4) exits 128 with one stderr line and the others succeed. Each row expects `Err(ScanError { call: k, .. })` with that line; a fifth row fails the spawn of call 1 (`io::Error`); a sixth row has `rev-parse` fail and asserts that calls 2 and 4 are never issued and the result is `Ok`.
    - Verify: T(`--lib checkpoint::scan::`) ≥ 7; T(`--lib scan_fails_closed_on_each_git_call`) = 1.
  - [x] 3c: `tools/src/cmd/checkpoint/others.rs`.
    - Other worktrees come from `git::worktrees` and are compared with `canonicalize`; missing directories are skipped.
    - Each is checked with `--no-optional-locks status --porcelain --untracked-files=all -- . <EXCLUDES>` and `rev-list --count HEAD --not --remotes=origin`.
    - Verify: T(`--lib checkpoint::others::`) ≥ 1 (a unit test: a missing worktree directory is skipped, and the current one is excluded).
  - [x] 3d: Wire it up.
    - `tools/src/main.rs`: `Command::Checkpoint`, before `DocsCheck` (variants are alphabetical; R2 adds `Loop` between `Hook` and `Soak`). It uses `disable_help_flag` and raw `std::env::args_os().skip(2)`, the soak pattern (main.rs L26 and L82). `Err` → `checkpoint: <err:#>`, exit 2.
    - `tools/src/cmd/mod.rs`: `pub mod checkpoint;`.
    - Update the `lib.rs` crate doc and the `tools/Cargo.toml` description (L7) to `… (checkpoint, docs-check, hook, soak; agent-loop commands in later PRs)`. Task 6f adds `brief, `, so the final line is the contract's `AIOS host tooling: the aios binary (brief, checkpoint, docs-check, hook, soak; agent-loop commands in later PRs)`.
    - Verify (run after 3e is written, in the same commit): T(`--test checkpoint_parity cli_unknown`) = 1; `git diff -U0 origin/main...HEAD -- tools/Cargo.toml | grep -cE '^[-+][^-+]'` = 2 (A5; run after the commit).
  - [x] 3e: Tests added in this commit:
    - the 52 per-scenario `#[test]`s: the `scenario!` arm of 2a, aios only, against the golden; they need no bash or jq.
    - The three aios-only goldens, blessed from aios one scenario at a time (`AIOS_BLESS_GOLDENS=1 cargo test -p aios-tools --test checkpoint_parity cli_help`, then the same with `scan_git_failure_fails_closed`, then with `scan_git_failure_on_clean_tree`), and reviewed by hand in the commit against the D9 output in Code Structure Decisions. Besides the golden, both D9 scenarios assert in Rust that stdout has no `Safe to /clear` line and that the exit is 1, so a re-bless cannot weaken them.
    - `checkpoint_differential_against_oracle`: twin repos, over the 49 non-divergent scenarios, comparing normalised stdout, exit, origin refs, HEAD message, HEAD sha and author/committer (`aios-test`, Task 1e's identity env), index, upstream and gh calls, run with `parallel_map`.
    - `checkpoint_differential_under_bin_bash_3_2`: the same differential with the oracle run by `/bin/bash`, only where `/bin/bash --version` reports 3.2 (macOS). Elsewhere it prints "no /bin/bash 3.2 here" and returns.
    - Verify: `ls tools/tests/golden/checkpoint/*.golden | wc -l` = 52; T(`--test checkpoint_parity scan_git_failure_`) = 2.
  - [x] 3f: Mutation checks. These are uncommitted, in the writer's worktree; afterwards run `git checkout -- tools/src`.
    - Remove `--diff-filter=A` → `modified_tracked_creds_not_flagged` fails.
    - Drop the `wip:` hold → `ready_pr_holds_wip`, `gh_fails_holds_wip` and `gh_missing_holds_wip` fail.
    - Push with no refspec (`git push -q -u origin`) → `tracking_origin_main_pushes_branch_only` fails. (`HEAD:refs/heads/<b>` is an equivalent mutant: git sets the same upstream and remote ref.)
    - Remove `read-tree` on a stop → `sensitive_name_new_doc` and `index_restored_exactly` fail.
    - Make a scan git failure return an empty scan → `scan_git_failure_fails_closed` fails.
    - Keep `cwd` instead of the toplevel → `run_from_subdirectory` fails.
    - On a scan failure, fall through to the script's final-line logic (print `Safe to /clear` and exit 0 when nothing is dirty or unpushed) → `scan_git_failure_on_clean_tree` fails.
    - Verify: under each mutation, `cargo test -p aios-tools --test checkpoint_parity <one named test>` exits non-zero; afterwards `git status --porcelain -- tools/src` prints nothing. The writer lists the seven results in its report.
  - [x] 3g: Verify: T(`--test checkpoint_parity`) = 55 (52 per-scenario tests + `oracle_reproduces_checkpoint_goldens` + 2 differentials); T(`--lib checkpoint::`) ≥ 9; `git grep -n 'GH_PROMPT_DISABLED' -- tools/src` lists only files under `tools/src/cmd/checkpoint/`; G.

- [x] **Task 4: Brief scenarios, gh fixtures and goldens recorded from the oracle (no port code)** (worker, background; 4e partly lead)
  - [x] 4a: Fixtures under `tools/tests/fixtures/brief/<scenario>/`:
    - `gh/<label>.{out,err,rc}`, with labels from `BRIEF_GH_FIXTURES`;
    - `docs/{out,err,rc,stale}`;
    - plus a `setup` described in Rust: repo states, worktrees, soak dirs, handoffs, phase docs and hook logs.

    A scenario that serves no fixture (`cli_help`, `cli_unknown_after_help`, `outside_repo`) has no directory: git cannot commit an empty one. JSON shapes come from the Task 0d capture, never its values: statusCheckRollup mixes CheckRun and StatusContext; `conclusion ""`; null `mergeable`; `labels[{name}]`; `author{login}`. Only `pr_labelled_needs_human` carries a PR label `needs-human`.
    - Verify: `comm -23 <(ls tools/tests/fixtures/brief | sort) <(grep -oE '^scenario!\([a-z0-9_]+' tools/tests/brief_parity.rs | cut -d'(' -f2 | sort)` prints nothing, so every fixture directory names a scenario; `git grep -l '"needs-human"' -- 'tools/tests/fixtures/brief/*/gh/pr-list.out'` lists only `pr_labelled_needs_human`. A scenario whose fixture file is missing panics in the runner.
  - [x] 4b: `tools/tests/brief_parity.rs`, using `scenario!` as in 2a (table entries now; the per-scenario `#[test]` arm arrives in Task 7). 76 scenarios; all run with `--no-fetch` except `fetch_fails` and `fetch_prunes`. Every failure scenario serves only the fixtures the oracle calls, so a port that issues a call the oracle skips hits the fake's exit 99, and the differential compares the call sequence:
    - GitHub reachability (6): `offline` (rate_limit fails with a stderr line), `offline_empty_stderr` (rate_limit fails with empty stderr: `GitHub unavailable: unknown error`, W234 brief L161), `gh_missing`, `rate_zero`, `fetch_fails` (origin URL removed), `fetch_prunes` (origin deleted a branch; the brief runs with fetch);
    - remote state (1): `no_origin_main`. Origin has only a `claude/x` branch, and the clone has no `refs/remotes/origin/main`. It has soak runs with recorded commits and a `run-list` fixture, and **no** `graphql-suites` fixture: a port that issues the suites query hits the fake's exit 99. It covers:
      - the Git section without the origin/main and local-main lines (L125-131);
      - Main CI with no suites query and no not-tip suffix (L265-298), so the gh call log has one call fewer;
      - Soak's `origin/main unknown` (L339-341);
    - working directory (2):
      - `run_from_subdirectory`. The run is from `docs/`, with a phase doc and knowledge notes. The phase and knowledge sections and the docs-check child's cwd (`<ROOT>`) must match the oracle's;
      - `run_from_linked_worktree`. The run is from a linked worktree `wt-x` (at `<main worktree>/wt-x`, so `rel_path` labels it `wt-x`) on `claude/x`, whose branch commit deletes `scripts/agent/qemu-lock.sh` and adds a phase doc. The main worktree (on `main`) keeps qemu-lock.sh and holds two handoff files in `.remember/`; the lock owner file (a dead pid) is in the shared common dir, where qemu-lock.sh keeps it (qemu-lock.sh L77-81). It pins what brief reads from `MAIN_WT` (handoffs, L492; the QEMU lock, L473-474: `state=dead`, not "no scripts/agent/qemu-lock.sh in the main checkout"), what it reads from ROOT (the phase doc only `wt-x` has; knowledge notes; docs-check's cwd), and the `rel_path` labels (`- checkout: wt-x on \`claude/x\` …`, the worktree list's `(main checkout)` line; L76-79, L123);
    - `worktrees` (1): detached, missing, not on origin, behind origin/main, local main behind;
    - PRs (9):
      - `prs_every_blocker`: all 11 oracle blockers; control characters in title, branch and login; author null; team labels in label order; no team;
      - `merge_ready_yes`, `threads_unknown`, `threads_zero_is_known`, `gates_unknown`, `pr_beyond_50_threads_unknown`, `no_prs`;
      - `pr_list_fails`: probe, issues and threads succeed and `pr list` fails, giving `- GitHub unavailable: <pr-list stderr line>` (L243); with a phase doc whose next step is in a milestone, so the Phase section prints `open PR for M<n>: unknown (GitHub unavailable)` (L601);
      - `pr_labelled_needs_human` (D10, aios-only);
    - Main CI (6): `main_ci` (runs and suites merged), `main_ci_tie` (equal `createdAt`; the suite wins), `main_ci_not_tip`, `main_ci_empty`, and the two distinct failures:
      - `main_ci_suites_fail`: the suites GraphQL fails, so `run list` is never issued (no `run-list` fixture) and the line carries the GraphQL stderr (L269-282, L301);
      - `main_ci_runs_fail`: the suites query succeeds and `run list` fails (L283-285, L301);
    - Needs human (1): `queue_counts_unknown`: the `agent-ready` and `agent-working` `issue list` calls fail, giving `- queue: ? agent-ready, ? agent-working` (L551-553);
    - Soak (17):
      - `soak_main_tip`, `soak_behind`, `soak_not_on_main`, `soak_dirty`, `soak_partial`, `soak_no_rows`, `soak_legacy_wedge`;
      - `soak_interleaved_clean_per_arm`, `soak_interleaved_no_arms`, `soak_interleaved_no_summary`;
      - `soak_interleaved_beside_main` (soak_interleave.rs's assertion);
      - `soak_interleaved_no_boots` (in `PLATFORM_SCENARIOS`: awk on a missing boots.tsv, main brief.sh L395-407);
      - `soak_boots_row_quirk`, `soak_none`, `soak_commit_is_branch_name`;
      - `soak_equal_mtime_tie` (within one worktree), `soak_equal_mtime_tie_across_worktrees`;
    - QEMU lock (3): `qemu_lock_free`, `qemu_lock_dead_holder` (an owner file naming a dead pid: `state=dead`), `qemu_lock_missing` (no qemu-lock.sh in the base commit);
    - Routing log (3): `routing_log` (state dir set, compact `"kind":"launched"`), `routing_log_empty_var` (an empty value counts as unset), `routing_log_missing`;
    - Handoffs (2): `handoffs` (two by mtime, a tie by name, no trailing newline, 61+ lines, `handoff-.md`), `handoffs_none`;
    - Knowledge (6):
      - `knowledge_since_handoff` and `knowledge_since_marker` list committed notes, at fixed epochs;
      - `knowledge_7_days` covers uncommitted notes only: a commit dated relative to now would change every sha in the golden;
      - `knowledge_over_25`, `knowledge_uncommitted_rename_and_space`, `knowledge_template_filtered`;
    - Phase (8): `phase_in_progress`, `phase_all_complete`, `phase_struck_task`, `phase_no_step_heading`, `phase_all_checked`, `phase_open_pr_for_m`, `phase_open_pr_none`, `phase_open_pr_unknown`;
    - Docs (8): `docs_clean`, `docs_new_drift_key_order`, `docs_prune_hint`, `docs_rc2`, `docs_panic101`, `docs_other_rc_last_line`, `docs_unreadable_json`, `docs_stale_note` (the oracle gets FAKE_DOCS's `stale` file; aios gets `AIOS_SHIM_STALE=1`);
    - CLI (3): `cli_help` (D1, aios-only), `cli_unknown_after_help`, `outside_repo` (built with `Scenario::outside_repo()`, Task 1e).
    - Plus the test `brief_fixtures_label_needs_human_only_in_d10`: every scenario outside `DIVERGENT_SCENARIOS` serves a `pr-list` with no `needs-human` label, so it stays differential.
    - Verify: `grep -c '^scenario!' tools/tests/brief_parity.rs` = 76.
  - [x] 4c: Goldens in `tools/tests/golden/brief/<name>.golden`, with these sections: `exit`, `stdout`, `stderr`, `gh calls`, `docs calls` (per call, one line with the argv space-joined, then `docs-cwd <ROOT>`; normalised), `marker touched`, and `remote refs` (`git for-each-ref refs/remotes`).
    - `#[ignore] record_brief_goldens` records the 74 non-divergent goldens from the oracle, through `oracle_renames()`.
    - The active tests are `oracle_reproduces_brief_goldens` and `brief_fixtures_label_needs_human_only_in_d10`.
    - Every scenario runs `assert_not_vacuous` and `fake.assert_calls_match(BRIEF_GH_ALLOW)`.
    - Verify: `AIOS_BLESS_GOLDENS=1 cargo test -p aios-tools --test brief_parity -- --ignored record_brief_goldens` exits 0; T(`--test brief_parity`) = 2; `ls tools/tests/golden/brief/*.golden | wc -l` = 74; `grep -c '^docs-check --json$' tools/tests/golden/brief/run_from_subdirectory.golden` = 1 and `grep -c '^docs-cwd <ROOT>$' tools/tests/golden/brief/run_from_subdirectory.golden` = 1; `grep -c 'run list' tools/tests/golden/brief/main_ci_suites_fail.golden` = 0; `grep -c '^- queue: ? agent-ready, ? agent-working$' tools/tests/golden/brief/queue_counts_unknown.golden` = 1; `grep -c '^- GitHub unavailable: unknown error$' tools/tests/golden/brief/offline_empty_stderr.golden` ≥ 1.
  - [x] 4d: `#[ignore] capture_real_gh_shapes`, run by the lead. It uses the network and read-only gh, outside the jail by design, and is pointed at the repository explicitly, so it works from any clone, whatever its `origin`: every repo-scoped call (`issue list`, `pr list`, `pr list --head`, `run list`) gets `-R juslee/aios`, and the two GraphQL calls get the literals `-F owner=juslee -F name=aios` in place of the `{owner}`/`{repo}` placeholders (which gh resolves only from a github.com remote). Every call goes through `aios_tools::gh::run_program(OsStr::new("gh"), …, &GhEnv::INHERIT)`, so `is_read` refuses anything but a read. It writes to `target/r3-gh-capture/` (uncommitted), and asserts, per call, that every committed fixture's JSON keys are a subset of the real response's keys.
    - Verify: `cargo test -p aios-tools --test brief_parity -- --list --ignored | grep -c capture_real_gh_shapes` = 1; `grep -cF '"juslee/aios"' tools/tests/brief_parity.rs` ≥ 1.
  - [x] 4e: Verify:
    - `git grep -n -e /Users/ -e /home/ -e /private/ -- tools/tests/golden/brief tools/tests/fixtures/brief` prints nothing; the contract §10 token grep over `tools` prints nothing; G.
    - The lead, in a worker-free scratch clone of `<BW>` (never in `<BW>`; its `origin` is a local path, which 4d's `-R` and literals make irrelevant): `cargo test -p aios-tools --test brief_parity -- --ignored capture_real_gh_shapes` passes.
    - The lead, after the push: `gh run view <run-id> --log | grep -c 'test oracle_reproduces_brief_goldens ... ok'` = 1 (ubuntu-26.04). A failure adds the scenario to `PLATFORM_SCENARIOS` (tool and oracle line) in a follow-up worker commit.
    - Result (2026-10-12): the lead ran `capture_real_gh_shapes` in a scratch clone at eaf4a64 and it passed (committed fixtures compared: pr-list 67, graphql-suites 66, run-list 66, the two queue counts 67 each; StatusContext fixtures not compared, because no real PR has a StatusContext check), and PR #257's CI on eaf4a64 logged `test oracle_reproduces_brief_goldens ... ok` on ubuntu-26.04.
    - An identity or sha mismatch (an author other than `aios-test`, or a commit sha that differs between twins or between macOS and CI) is never a `PLATFORM_SCENARIOS` case: it is a harness bug in `twin_command`'s identity env or date pinning (Task 1e), fixed there in a follow-up worker commit.

- [x] **Task 6: `aios brief`, the local sections** (worker, background)
  - [x] 6a: `tools/src/cmd/brief/mod.rs`: `run(args, cwd, out, err) -> anyhow::Result<u8>`.
    - Parse the arguments in order. `-h`/`--help` prints new help text naming `aios brief [--no-fetch]` and its side effects (D1). Outside a repo, print `brief: not inside a git repository` and exit 2; otherwise exit 0.
    - Find the toplevel (`git rev-parse --show-toplevel` from `cwd`), the common dir and the main worktree (`git::worktrees`). **Every later git, gh, docs-check and file call runs with the toplevel as cwd**, as brief.sh's `cd "$ROOT"` does (L56): the phase-doc glob, `-- docs/knowledge`, the soak dirs.
    - Start docs-check first, on a `std::thread`, as a child: program `current_exe()`, argv exactly `docs-check --json`, cwd = the toplevel. Only the program can be overridden, by `AIOS_BRIEF_DOCS_CHECK=<program>`, and only `if cfg!(debug_assertions)` (the `AIOS_SOAK_LOADAVG` precedent, `tools/src/cmd/soak/host.rs` L129-133). The argv and cwd stay the same, so the `docs calls` golden section pins them.
    - Touch the marker last.
    - Verify: T(`--lib brief::tests::`) ≥ 1 (the parser table).
  - [x] 6b: `clock.rs`:
    - `fmt_epoch(epoch, fmt)` tries `date -r`, then `date -d @`, and otherwise prints the raw epoch (kept bug-for-bug);
    - `header()` runs `date '+%Y-%m-%d %H:%M %Z'`;
    - epochs come from `SystemTime` and `metadata().modified()`, and are passed as `--since=@<epoch>`.
    - Verify: T(`--lib brief::clock::`) ≥ 2.
  - [x] 6c: Sections:
    - `git_section.rs`: Git, with the fetch (`GIT_TERMINAL_PROMPT=0 git fetch --quiet --prune origin`), minus the GitHub probe. The origin/main lines appear only when `refs/remotes/origin/main` resolves. `unpushed_desc` uses `--remotes` (all remotes), unlike checkpoint (kept).
    - `soak.rs`:
      - run discovery: bytewise-sorted dirs per worktree, worktrees in porcelain order, strict `>` (D7);
      - `soak_relation`, which prints `origin/main unknown` when there is no tip;
      - `print_soak` (with the Boots sed fall-through);
      - `print_interleaved`, matching the macOS oracle on a missing boots.tsv.

      Class names and columns come from `cmd::soak::report` (`SCRIPT_COLUMNS`, report.rs L32; column 3 = class, 2 = mode), not re-hardcoded. Widen visibility to `pub(crate)` only where needed.
    - `local.rs`: Host QEMU lock, Routing log, Handoffs and Knowledge notes, each reading from the worktree brief.sh reads it from:
      - the QEMU lock from the **main worktree**: if `<main>/scripts/agent/qemu-lock.sh` is a file, spawn `sh scripts/agent/qemu-lock.sh status` with cwd `<main>` and prefix `- `, else print `- no scripts/agent/qemu-lock.sh in the main checkout` (W234 L473-477);
      - handoffs from `<main>/.remember` (L492);
      - knowledge notes from the **toplevel** (`-- docs/knowledge`), like the phase doc and docs-check;
      - `rel_path` labels relative to `<main>` (L76-79), shared with `git_section.rs` and `soak.rs`.
      `run_from_linked_worktree` (Task 4b) pins every one of these.
    - `phase.rs`: the awk port. It does not reuse `docs_check::checks::milestones::is_unchecked_task` (`tools/src/cmd/docs_check/checks/milestones.rs` L87), whose semantics differ.
    - `docs.rs`: Docs drift.
      - The error-line preference is `docs-check: ` → the panic header → the last line. The oracle's `^aios: ` step is gone (D2).
      - The stale note comes from `AIOS_SHIM_STALE` (D6).
    - Verify: T(`--lib brief::`) ≥ 4.
  - [x] 6d: An interim test, `brief_local_sections_match_goldens`. It compares section by section (split on `\n## `), plus the `docs calls` section, for every section except `Open pull requests`, `Main CI` and `Needs human`. Those three are listed in `const NOT_YET_PORTED`, which Task 7 deletes. The `## Git` comparison stops before the `GitHub unavailable` probe line.
    - Verify: T(`--test brief_parity brief_local_sections`) = 1.
  - [x] 6e: Mutation checks (uncommitted):
    - soak tie `>=` instead of `>` → `soak_equal_mtime_tie`;
    - drop the sed fall-through → `soak_boots_row_quirk`;
    - touch the marker first → `knowledge_since_marker`;
    - skip `--prune` → `fetch_prunes`;
    - keep the caller's `cwd` for the phase glob and the docs-check child → `run_from_subdirectory`;
    - read handoffs from the toplevel instead of the main worktree → `run_from_linked_worktree`.
    - Verify: under each mutation, `cargo test -p aios-tools --test brief_parity brief_local_sections` exits non-zero and its output names that scenario; `git status --porcelain -- tools/src` prints nothing afterwards.
  - [x] 6f: Wire `Command::Brief` (the first variant; raw `args_os`, `disable_help_flag`) and `pub mod brief;`. Update the lib.rs crate doc, and add `brief, ` to the `tools/Cargo.toml` description.
    - Verify: `cargo run -q -p aios-tools -- brief --bogus; test $? = 2`; `grep -c 'the aios binary (brief, checkpoint, docs-check, hook, soak; agent-loop commands in later PRs)' tools/Cargo.toml` = 1; G.

- [x] **Task 7: `aios brief`, the GitHub sections, full goldens and the differential** (worker, background)
  - [x] 7a: `github.rs`. Every call goes through `GhCli { cwd: <toplevel>, env: GhEnv::INHERIT }`.
    - The probe is `gh api rate_limit --jq .resources.core.remaining`: `Failed` → `first_stderr_line`, or `unknown error` when that line is empty (L161); `0` → exhausted; `Missing` → `gh is not installed`.
    - The needs-human issues (`merge_ready::NEEDS_HUMAN_ISSUES_ARGS`), the threads GraphQL (the same query text) and the PR list (`--limit 30`, `--json merge_ready::PR_FIELDS`), rendered by `merge_ready::pr_lines`. A failed PR list prints `- GitHub unavailable: <its first stderr line>` (L243) and makes the Phase section's lookup `unknown (GitHub unavailable)` (L601).
    - Main CI: the suites GraphQL, issued only when `refs/remotes/origin/main` resolves, then `run list`, **issued only when the suites query did not fail** (L267-285); either failure prints `- GitHub unavailable: <that call's first stderr line>` (L301). Suites are appended after runs, grouped by workflow, with the last maximum by `createdAt`, then sorted by name. The not-tip suffix appears only when there is a tip.
    - Needs human and its queue counts (a failed count prints `?`, L551-552); the `open PR for M<n>` lookup.
    - The argv, cwd and env of every gh call equal the oracle's; the differential compares the call logs.
    - Verify: T(`--lib brief::github::`) ≥ 1.
  - [x] 7b: Delete `NOT_YET_PORTED` and the interim test.
    - Add the 76 per-scenario `#[test]`s, each checking the whole output against its golden, aios only.
    - Bless the aios-only goldens one scenario at a time (`AIOS_BLESS_GOLDENS=1 cargo test -p aios-tools --test brief_parity cli_help`, then `pr_labelled_needs_human`), and review them by hand.
    - Add `brief_differential_against_oracle`: twin repos over the 74 non-divergent scenarios, with `PLATFORM_SCENARIOS` handled as defined. It compares stdout, exit, gh calls (so the call sequence), docs calls, the marker and remote refs, using `parallel_map`.
    - Verify: `ls tools/tests/golden/brief/*.golden | wc -l` = 76; `git grep -c NOT_YET_PORTED -- tools` prints nothing.
  - [x] 7c: Mutation checks (uncommitted):
    - the call-site lookup of the open-thread count filters out 0 (`.filter(|n| *n > 0)` in `tools/src/cmd/brief/github.rs`; `pr_lines` takes an `Option<u64>`, so the count never passes through `merge_ready::alt`) → `threads_zero_is_known` fails;
    - `max_by` takes the first maximum → `main_ci_tie` fails;
    - the gate regex matches a body as a title → `prs_every_blocker` fails;
    - drop blocker 4 → `pr_labelled_needs_human` fails;
    - always issue the suites query → `no_origin_main` fails;
    - issue `run list` after a suites failure → `main_ci_suites_fail` fails;
    - print `0` instead of `?` for a failed queue count → `queue_counts_unknown` fails.
    - Run 2026-10-12 as call-site mutations in github.rs (all 7 caught); merge_ready.rs is frozen, and the classifier refused editing it.
    - Verify: each named test exits non-zero under its mutation; `git status --porcelain -- tools/src` prints nothing afterwards.
  - [x] 7d: Verify: T(`--test brief_parity`) = 79 (76 per-scenario tests + `oracle_reproduces_brief_goldens` + `brief_fixtures_label_needs_human_only_in_d10` + the differential); `cargo test -p aios-tools` passes in full; G.

- [x] **Task 8: Shim: `AIOS_SHIM_STALE`, and checkpoint's exit 3 on a failed stale rebuild** (worker, **foreground**: `.claude/hooks/aios` is protected; contract §6; R2 edits the shim only after R3 is on main)
  - [x] 8a: `.claude/hooks/aios`, exactly the contract §6 R3 rows:
    - `unset AIOS_SHIM_STALE`, with a one-line comment, right after `sub=${1:-}` (W234 L136).
    - The `<other>` tail (W234 L580-582) becomes `if ! build_fg; then`:
      - for `$sub` = `checkpoint`: `echo "aios: rebuilding $bin failed; aios checkpoint never commits or pushes with a stale binary (run: just tools)" >&2; exit 3`;
      - otherwise: the unchanged warning, then `AIOS_SHIM_STALE=1; export AIOS_SHIM_STALE`.
    - The header's `aios <other>` paragraph (W234 L87-91) documents both, and states that checkpoint still runs a dirty build and an `AIOS_TOOLS_BIN` override (owner decision 5).
    - There is no `case` arm and no exit-3 helper: checkpoint does not join the loop's strict arm. The former Task 8b (a shim helper) is removed, so the sub-tasks run 8a, 8c, 8d.
    - Verify: `sh -n .claude/hooks/aios`; `diff <(git show origin/main:.claude/hooks/aios | shellcheck -s sh -f json1 - | jq -r '.comments[].code' | sort) <(shellcheck -s sh -f json1 .claude/hooks/aios | jq -r '.comments[].code' | sort) | grep -c '^>'` = 0 (no new shellcheck finding).
  - [x] 8c: `tools/tests/shim.rs`, appended at the end of the file:
    - `a_stale_binary_whose_rebuild_fails_runs_it_with_aios_shim_stale` (the fake binary prints `${AIOS_SHIM_STALE-unset}`);
    - `a_fresh_binary_runs_with_aios_shim_stale_cleared` (with `AIOS_SHIM_STALE=1` inherited);
    - `checkpoint_on_a_stale_binary_whose_rebuild_fails_exits_3` (code 3, empty stdout, binary not run, stderr names `just tools`);
    - `checkpoint_runs_a_dirty_binary`, `checkpoint_honours_aios_tools_bin`;
    - `brief_on_a_stale_binary_whose_rebuild_fails_warns_and_runs_it`.

    Edit the doc comment at L623 to name `aios brief`. The existing `a_stale_binary_whose_rebuild_fails_warns_and_runs_it` stays.
    - Verify: T(`--test shim aios_shim_stale`) = 2; T(`--test shim checkpoint_`) = 3; T(`--test shim brief_on_a_stale`) = 1.
  - [x] 8d: Verify: `cargo test -p aios-tools --test shim` passes; `git grep -n 'brief\.sh' -- tools/tests/shim.rs` prints nothing; G.

- [ ] **Task 9: Recipes and build tables** (worker, background)
  - [ ] 9a: `justfile`: directly after `docs-check-all` (main L347-348), add `[positional-arguments]` recipes `brief *args:` and `checkpoint *args:`. Each runs `{{ quote(justfile_directory() / ".claude" / "hooks" / "aios") }} <sub> "$@"` (the soak recipe's idiom, justfile L153), with a one-line comment. So `just --justfile <main>/justfile --working-directory . …` from any worktree runs **main's** shim. This is the only justfile hunk.
    - The same commit appends to `tools/tests/shim.rs` the test `justfile_recipes_run_mains_shim_from_another_directory`. For each of `brief` and `checkpoint`, it runs `just --justfile <repo>/justfile --working-directory <dir> --dry-run <recipe>` as a child process (`<repo>` = `env!("CARGO_MANIFEST_DIR")/..`, canonicalised; `<dir>` a fresh directory under `std::env::temp_dir()`, outside every checkout). It asserts that the dry-run output (stderr) names `<repo>/.claude/hooks/aios` as an absolute path, quoted, followed by the subcommand. CI's "Tools (host)" job has `just` (`extractions/setup-just`), as the existing shim tests already need.
    - Verify (no `just -` command line, Progress writer rules): `grep -cE '^(brief|checkpoint) \*args:' justfile` = 2; T(`--test shim justfile_recipes_`) = 1.
  - [ ] 9b: README Build Commands (next to L127-128) and its tools line (L87), and developer-guide §5.1 (`docs/project/developer-guide.md` L1546, rows near L1573): both rows. The `just-recipes` check (`tools/src/cmd/docs_check/checks/just_recipes.rs`) requires them.
    - Verify: DG; `cargo test -p aios-tools --test shim` passes; G.

- [ ] **Task 10: Switch-over, contract test and deletion** (workers, background or foreground per commit; the owner applies settings; the lead verifies)
  - **Commit order and grouping.** Every commit passes G (with DG). The sub-tasks below are listed in this order:
    1. **10h** (worker, background), alone. On main, `tools/tests/soak_interleave.rs` (L739-749) runs `../scripts/agent/brief.sh` from disk, so no commit may delete the script before this port lands.
    2. **10a, 10b, 10c** (one worker, **foreground**: `.claude/**` is protected), one commit each, in that order. They remove every link and code-span path to the scripts (the CLAUDE.md rows, agent-loop.md L16/L18, the skills), so `md-links` and `repo-paths` stay clean when 10d deletes the files.
    3. **10g** (worker, background), alone, after 10a: two of its three tests read the pause SKILL.md text that 10a writes.
    4. **10d** (worker, background), last, alone: the deletion, with nothing that needs a prompt.
    5. **10e** (owner): the settings commit.
    6. **10f** (lead, read-only): the switch-over verify.
  - [ ] 10h: `tools/tests/soak_interleave.rs` (main L25, L739-749; contract §3). `brief_lists_an_interleaved_run_once_and_never_as_main_soak` runs `env!("CARGO_BIN_EXE_aios") brief --no-fetch` with the same fake gh and PATH. It also sets `AIOS_BRIEF_DOCS_CHECK` to a `#!/bin/sh` stub in its bin dir, so no real docs-check runs over the fixture. The module doc (L25) names `aios brief`.
    - Verify: T(`--test soak_interleave brief_lists`) = 1; `git grep -c 'brief\.sh' -- tools/tests/soak_interleave.rs` prints 0 or nothing; G.
  - [ ] 10a: Skills. Fallbacks use `just` forms only.
    - `.claude/skills/justin/skills/brief/SKILL.md` (W234 L4, L18-30): run `just brief`. When the current checkout's justfile has no `brief` recipe (a branch older than R3), the fallback is `just --justfile "<main checkout>/justfile" --working-directory . brief`. It still reports on the current checkout, since the binary resolves the toplevel itself, and it asks once (settings `Bash(just -*)`, W234 L326). Update the description and the side-effects text: it is the binary now, not a script.
    - `.claude/skills/justin/skills/pause/SKILL.md` (W234 L4-6, L35-62):
      - Run `just checkpoint --handoff saved`. For a pre-R3 branch, the fallback is `just --justfile "<main checkout>/justfile" --working-directory . checkpoint --handoff saved` (main's shim, contract §6).
      - "The binary does all git work."
      - The `checkpoint it:` wording follows D5, and the safeguards list gains the fail-closed scan (D9).
      - **Exit 3 has two meanings** (contract §6). Empty stdout with an `aios: ` stderr line means the shim refused a stale binary: report it and tell the user to run `just tools`, never `--allow`. A `Stopped before committing` block on stdout is the secret stop, which keeps the existing ask-the-user flow.
    - `.claude/skills/merge-and-cleanup/SKILL.md`:
      - L52: `just brief --no-fetch | awk …`, the same awk;
      - L55: `labelled needs-human` is a hard stop like `gated by`: report the loop's needs-human comment and stop; the owner removes the label after deciding;
      - L56: "`aios brief`'s `--limit 30`".
    - `.claude/skills/justin/skills/start/SKILL.md`, L19 only: "computed by `aios brief`"; never propose merging a PR "gated by or named in a `needs-human` issue, or labelled `needs-human`".
    - `.claude/CLAUDE.md` Skills table rows `/justin:brief` and `/justin:pause` (W234 L287, L289, re-read in 0a at 2d4a9f7) become `aios brief` / `aios checkpoint`. These rows name the scripts, so they cannot wait for the ship pass: 10f's `.claude` grep would fail.
    - Verify: `git grep -hc 'just checkpoint --handoff saved' -- .claude/skills/justin/skills/pause/SKILL.md` prints a number ≥ 1; `git grep -hcF 'just brief --no-fetch | awk' -- .claude/skills/merge-and-cleanup/SKILL.md` prints `1`; `git grep -hc 'labelled `needs-human`' -- .claude/skills/justin/skills/start/SKILL.md` prints `1`; DG; G.
  - [ ] 10b: Rules and docs.
    - `.claude/rules/03-git-workflow.md` L22 is replaced by the contract §8 general Guard Rails bullet:
      - compiled `aios` subcommands that commit, push or write to GitHub; today that is `aios checkpoint`;
      - their controls: code and tests, the Option-A gate, and an owner settings ask on each invocation spelling (A6: `just checkpoint`, the `--justfile` fallback, the shim, the installed binary, `cargo run … checkpoint`), so every run prompts once and `-p` and background runs are refused;
      - `/justin:pause` pushes through `just checkpoint`, refspec `refs/heads/<claude/*>:refs/heads/<claude/*>` only, never forced;
      - an unattended runner needs the owner to remove that ask from its stage's settings.
    - `.claude/rules/11-teams.md` L55: "refuses a PR the brief shows as gated **or labelled needs-human**".
    - `docs/project/agent-loop.md`:
      - L11 ("shared logic lives in the `aios` binary");
      - L16 and L18 (links → `tools/src/cmd/brief/`, `tools/src/cmd/checkpoint/`);
      - L25 (merge-ready gains the label);
      - L30 (`aios checkpoint`; the fail-closed scan, D9);
      - L72 (Holding a PR: the label also holds it);
      - re-read L23, L32, L64, L205 and L238, and change any that names a script.
    - Spec (contract §8, W234 lines):
      - L41 and L194: the Order becomes `R1 → R4 → R3 → R2 → R5 → R5b` (owner decision 3);
      - L71-72: the subcommand rows;
      - L79: `merge_ready` joins the shared-module list, with `git`'s inherit policy;
      - L84 → "Fake `gh` and `claude` are `#!/bin/sh` scripts on PATH (R4 precedent), not test binaries";
      - L151: the example becomes `aios brief`;
      - L176: the R3 row, plus an "R3 as built" paragraph after L181 (D1-D10, the oracle shas, the scenario counts, and synthesised fixtures that copy real shapes, not recorded responses);
      - §2 Freshness, the Stale × Other-subcommands cell (`AIOS_SHIM_STALE`; checkpoint exit 3).
    - `docs/knowledge/lessons/2026-10-06-jl-model-routing-wiring-checks.md` L105: "`aios brief`'s count".
    - Comments: `tools/src/cmd/docs_check/mod.rs` L187 ("every consumer (`docs.yml`, `aios brief`)") and `tools/src/cmd/docs_check/checks/milestones.rs` L173 ("as in `aios brief`'s phase section").
    - Verify: `git grep -hcF 'R1 → R4 → R3 → R2 → R5 → R5b' -- docs/knowledge/discussions/2026-09-22-jl-rust-agent-tools.md` prints `1`; `git grep -hc 'labelled needs-human' -- .claude/rules/11-teams.md` prints `1`; `git grep -hc 'cargo run' -- .claude/rules/03-git-workflow.md` prints a number ≥ 1; DG; G.
  - [ ] 10c: The guard test (protected). In `test_main_thread_everyday_commands_stay_allowed` (W234 L1471-1487), drop the brief.sh copy (L1472-1473) and its commands (L1482, L1487). Add `just brief`, `just brief --no-fetch` and `just checkpoint --handoff saved`, each expected "none": the guard gives no decision, and the settings ask is the control.
    - Verify: `/usr/bin/python3 -m unittest discover -s .claude/hooks/tests` passes (local only: not in CI); `grep -c 'brief\.sh' .claude/hooks/tests/test_git_push_guard.py` prints `0`; G.
  - [ ] 10g: `tools/tests/skill_contract.rs` (after 10a; see the commit order):
    - `merge_and_cleanup_awk_reads_every_pr_verdict`: extracts the awk program from `.claude/skills/merge-and-cleanup/SKILL.md` (its `awk -v n=` line), runs it with `sh -c` on `aios brief --no-fetch` output for `prs_every_blocker` and `pr_labelled_needs_human`, and asserts each PR's `merge-ready:` line.
    - `pause_skill_names_just_checkpoint_and_line_formats`: the pause SKILL.md names `just checkpoint --handoff saved`, the first-line format `checkpoint: <branch> @ <sha> | wip: ... | pushed: ... | handoff: ...` and both last-line forms, and each matches what `aios checkpoint` prints in `handoff_saved` and `other_worktree_dirty_from_main`.
    - `pause_skill_tells_the_two_exit_3s_apart`:
      - the skill text names both cues (`aios: ` on stderr with empty stdout; `Stopped before committing` on stdout);
      - `aios checkpoint`'s `sensitive_name_new_doc` output has the stdout cue and no `aios: ` stderr line;
      - the shim source contains the exact refusal text the skill quotes (`aios checkpoint never commits or pushes with a stale binary`), and `checkpoint_on_a_stale_binary_whose_rebuild_fails_exits_3` (Task 8c) pins its empty stdout.
    - Verify: T(`--test skill_contract`) = 3; G.
  - [ ] 10d: `git rm scripts/agent/brief.sh scripts/agent/checkpoint.sh`, the last Task-10 commit, alone.
    - Verify, in the worker's worktree:
      - before the change, on the parent commit (`<tip>`): `git grep -nE 'scripts/agent/(brief|checkpoint)\.sh' <tip> -- tools ':!tools/tests/common/agent.rs' | grep -vE 'former `?scripts/agent/(brief|checkpoint)\.sh`? \(blob at'` prints nothing, so no test reads a script from disk;
      - after `git rm` and before the commit: T(`--test soak_interleave`) passes with the scripts gone from the work tree (`test ! -e scripts/agent/brief.sh`), and G passes (so `cargo test -p aios-tools` and DG hold without the scripts);
      - after the commit: `git ls-files scripts/agent` lists only `scripts/agent/qemu-lock.sh`.
  - [ ] 10e: **Owner-run settings change** (contract §11 with A6 and A8; owner decision 4). The classifier refuses agent writes (#244).
    - **Base.** The lead fetches and pins `SETTINGS_BASE=$(git -C <BW> rev-parse HEAD:.claude/settings.json)`, and checks that it equals `git rev-parse origin/main:.claude/settings.json`. If it does not, a worker merges `origin/main` first (Progress conventions; a clean merge, since the branch has no settings edit), and the lead re-pins.
    - **Script.** The lead writes `<scratch>/r3-settings.sh`. It exits 1 unless `git hash-object .claude/settings.json` = `SETTINGS_BASE`. Then, with jq into a temp file and `mv`, it:
      - removes `"Bash(bash -n scripts/agent/brief.sh)"` and `"Bash(bash -n scripts/agent/checkpoint.sh)"` (W234 L125-126) from `allow`;
      - appends every line of `$R/probe/asks.txt` to `ask`: the seven rules of 0e, plus every rule the 0e probe added.

      No guard deny.
    - **Owner commit.** The owner runs the script in `<BW>` and commits the result separately as "owner-approved settings: aios checkpoint asks" (the R4 precedent, 65b80cb), before Task 11.
    - **Recovery (A8).** If `origin/main`'s `settings.json` changes after the owner commit (a worker's merge of main aborts on it), the owner reverts the owner commit. A worker then merges `origin/main`, the lead re-pins `SETTINGS_BASE`, and the owner re-runs the script and commits again.
    - R2's settings delta comes only after R2's convergence (contract §11).
    - Verify: `while read -r a; do jq -e --arg a "$a" '.permissions.ask | index($a)' <BW>/.claude/settings.json >/dev/null || echo "MISSING $a"; done < $R/probe/asks.txt` prints nothing; `jq -e '[.permissions.allow[] | select(test("scripts/agent/(brief|checkpoint)"))] | length == 0' <BW>/.claude/settings.json`; `git -C <BW> log -1 --format=%an -- .claude/settings.json` names the owner.
  - [ ] 10f: Verify the switch-over (lead, read-only on `<BW>`):
    - The contract §3 grep as amended by A10, verbatim: `git -C <BW> grep -n -e 'brief\.sh' -e 'checkpoint\.sh' -- . ':!docs/knowledge/plans' ':!docs/knowledge/lessons' ':!docs/knowledge/research' ':!docs/knowledge/decisions' ':!docs/knowledge/discussions/2026-09-22-jl-rust-agent-tools.md' ':!tools/tests/common/agent.rs' ':!scripts/agent/evals/cases' | grep -vE 'former `?scripts/agent/(brief|checkpoint)\.sh`? \(blob at `?[0-9a-f]{7,40}'` prints nothing.
    - `git -C <BW> grep -n -e 'brief\.sh' -e 'checkpoint\.sh' -- .claude` prints nothing.
    - DG, run by a verifier worker reset to `<BW>`'s tip (exactly the two `plans-not-empty` lines; so repo-paths, md-links, just-recipes and harness-tables add nothing).
    - The 0c payloads, re-run against `<BW>` (cwd `<BW>`, with its guard, shim and justfile), rows 3-12: the `diff` against `EXPECTED_GUARD` rows 3-12 is empty.
    - The saved 0e harness, re-run as `$R/probe/run-probe.sh <generated from <BW>/.claude/settings.json, permissions only, no extra asks> $R/probe/spellings-r3.tsv <scratch>/probes-10f.tsv`: `wc -l < <scratch>/probes-10f.tsv` = 24; `awk -F'\t' '$3 == "inconclusive" || ($1 != "control" && $3 != "refused")' <scratch>/probes-10f.tsv | wc -l` = 0; the control row is `ran`.

- [ ] **Task 11: Simplify and audit** (lead-run per rule 11)
  - [ ] 11a: Spawn the simplifier over `merge-base(origin/main)..HEAD`, then Placement.
    - Verify: G; `git -C <BW> diff --stat <before>..HEAD -- tools/tests/golden` prints nothing.
  - [ ] 11b: `/audit-loop` (tools mode) until a clean round. A dead or empty auditor never counts as clean. A fix found later, in 12a-12c, comes back through 11b before 12d.
    - Verify: the ledger at `<git-common-dir>/aios-agent/audit/claude-tools-r3-scripts/<head>.json` shows a clean round for the current head.

- [ ] **Task 12: Real verification, follow-ups, ship pass with distil, PR, hand-off** (lead; 12c owner-attended)
  - [ ] 12a: Real brief, read-only, from the main checkout, with the lead's scratch build (Progress conventions):
    ```bash
    diff <(bash <(git show $BRIEF_ORACLE:scripts/agent/brief.sh) --no-fetch | sed 1d) \
         <(<scratch>/r3-target/release/aios brief --no-fetch | sed 1d)
    ```
    The direct binary call prompts once (it is not allowlisted), and the lead is attended. Accepted differences are the D-listed lines (D10 for any PR labelled `needs-human`) and GitHub state that changed between the two runs (rerun once). Real gh, read-only. The result goes into the PR body.
    - Verify: every remaining diff hunk is named in the PR body by its D-number; R3's `merge-ready:` line names no `#<GATE>`.
  - [ ] 12b: Real checkpoint end to end, in a scratch dir under the session scratchpad, never against GitHub.
    - Setup: `git clone --bare <BW> <s>/origin.git`, `git clone <s>/origin.git <s>/clone` (the clone holds R3's justfile and shim), `git -C <s>/clone checkout -b claude/x`, a new file `deploy-secret.txt`, and a dirty linked worktree from `git -C <s>/clone worktree add <s>/wt -b claude/y`.
    - Every command runs with `AIOS_TOOLS_BIN=<scratch>/r3-target/release/aios` in the same command line.
    1. `just checkpoint` in `<s>/clone` → exit 3; the stop block names `deploy-secret.txt`; `git write-tree` is unchanged.
    2. `just checkpoint --allow deploy-secret.txt` → exit 1, with the first line `checkpoint: claude/x @ <sha> | wip: committed 1 file(s) | pushed: no (PR state unknown: <gh's first stderr line>; wip kept local) | handoff: not recorded`. Real gh cannot resolve a local-path remote.
    3. The same, with a scratch `gh` stub (`<s>/bin/gh`: prints nothing, exits 0) first on PATH → `pushed: yes`.
    4. Run the `checkpoint it:` hint printed under `<s>/wt` verbatim (D5), as `env AIOS_TOOLS_BIN=<scratch>/r3-target/release/aios PATH="<s>/bin:$PATH" sh -c '<hint>'` → `pushed: yes` there.
    5. From `<s>/clone/docs` (create it, with a dirty file in it and one at the root): `just --justfile <s>/clone/justfile --working-directory . checkpoint` → both files are in the new commit.
    - Verify: `git -C <s>/origin.git rev-parse refs/heads/main` equals its pre-run sha; `git -C <s>/origin.git for-each-ref --format='%(refname)' refs/heads/claude/x refs/heads/claude/y | wc -l` = 2; `git -C <BW> status --porcelain` prints nothing.
  - [ ] 12c: Owner-attended. The owner starts a separate solo session in `<BW>`: one that spawns no writers, while the R3 lead stays in the main checkout. `AIOS_TOOLS_BIN=<scratch>/r3-target/release/aios` goes in the session's launch environment, since Bash tool state does not persist and the variable cannot be exported from inside. The owner runs `/justin:brief` and `/justin:pause`. Pause prompts once (the settings ask) and pushes only `claude/tools-r3-scripts`, if anything. The handoff file lands in the gitignored `.remember/`.
    - Verify: `git ls-remote origin refs/heads/main` is unchanged across the session; the first output lines of both skills go into the PR body.
  - [ ] 12d: Follow-ups, then **the ship pass with the distil, as one commit**: the last commit before `gh pr ready` (contract §8). It is a worker commit, foreground, because `.claude/CLAUDE.md` is protected.
    - **First, the lead files the follow-up issues** (label `agent`, each titled `R3 follow-up: …`):
      1. Per contract §4: move `docs_check`'s inherited-env git calls onto `git.rs`. Give `git.rs` a `hardened(cwd)` constructor, then move `soak::host::git()` and `cmd/loop/git.rs` onto it after both PRs merge, never weakening their environment. `hook::path_guard`'s git stays.
      2. The guard's misleading QEMU-reason ask on direct `.claude/hooks/aios <sub>` calls (R5). Removing it requires the checkpoint and loop settings asks to be on main first.
      3. Run `.claude/hooks/tests` in CI.
      4. checkpoint hardening: H2 (the `git add -A` exit status) and H3 (a `sequencer` directory blocks the commit), plus the replace-refs gap in the secret scan (owner decision 8, contract §4).
    - **Then one commit**, which:
      - edits the `.claude/CLAUDE.md` Workspace Layout: `tools/` gains brief, checkpoint, `src/gh.rs`, `git.rs` and `merge_ready.rs`; the `scripts/` line's `agent/` keeps only qemu-lock; the justfile line gains brief/checkpoint;
      - updates the developer-guide test counts;
      - touches `scripts/docs/baseline.json` only if drift is resolved;
      - adds the lesson `docs/knowledge/lessons/<commit date>-jl-r3-brief-checkpoint-port.md` (frontmatter author/date/tags `[tooling]`/status final): jq-semantics gotchas, the PATH jail and the per-script derived tool lists (an extractor that skips quoted text misses `"$(… | paste …)"`), per-run materialisation, the toplevel-cwd rule and brief's MAIN_WT reads, test repos under `target/` sitting inside the checkout (outside-repo scenarios need `temp_dir()` plus a ceiling), host-tool-dependent oracle output, the guard's blindness to compiled git writes, the 0e precedence result, and the `plans-not-empty` docs gate. It cites the four follow-up issue numbers;
      - runs `git rm` on this plan and R3's copy of the contract (R2 keeps its own copy until its distil).
    - After this commit, only `/review-pr-comments` fixes follow. A fix that changes an inventory count updates it in the same commit.
    - Verify, all on the new `<BW>` tip:
      - DG's post-12d form: `just docs-check` exits 0 and prints `No new drift since baseline.`; G;
      - `gh issue list --label agent --state open --limit 1000 --json title --jq '[.[] | select(.title | startswith("R3 follow-up: "))] | length'` prints `4` (a title prefix, not `--search`, as in A4);
      - `git -C <BW> ls-files docs/knowledge/plans | grep -cE 'tools-r3-scripts|tools-r2-r3-contract'` = 0;
      - `git -C <BW> diff --exit-code origin/main...HEAD -- Cargo.toml Cargo.lock`;
      - `git -C <BW> diff -U0 origin/main...HEAD -- tools/Cargo.toml | grep -cE '^[-+][^-+]'` = 2;
      - `git -C <BW> grep -n 'GhCli\|run_program' -- tools/src` lists only `gh.rs`, `cmd/brief/mod.rs`, `cmd/brief/github.rs` and `cmd/checkpoint/mod.rs` (Review focus 6);
      - `git -C <BW> grep -n 'GH_PROMPT_DISABLED' -- tools/src` lists only `tools/src/cmd/checkpoint/` files;
      - every contract item has a non-test caller: `for f in GhCli GhEnv::INHERIT first_stderr_line git::command git::out git::text git::ok git::output git::worktrees git::branch merge_ready::pr_lines merge_ready::PR_FIELDS merge_ready::NEEDS_HUMAN_ISSUES_ARGS; do git -C <BW> grep -qF "$f" -- tools/src/cmd || echo "unused $f"; done` prints nothing;
      - the contract §10 token grep over `tools scripts/agent .claude` prints nothing;
      - `git -C <BW> diff --name-only --diff-filter=A origin/main...HEAD | grep -iE '<contract §10 SECRET_PATH basename regex>'` prints nothing (A5).
  - [ ] 12e: The PR (the draft from 1h).
    - `gh pr edit <n> --body-file …`, with:
      - a Summary;
      - Parity evidence: 52 + 76 scenarios (49 + 74 differential), the differentials including `/bin/bash` 3.2, `PLATFORM_SCENARIOS`, the mutation checks;
      - Divergences D1-D10, and what is kept bug-for-bug;
      - the Settings change (owner-applied) and the 0e probe;
      - the guard/pause answer;
      - the merge order and gate `#<GATE>`;
      - the Test plan, stating that the Python guard suite ran locally only (`/usr/bin/python3 -m unittest discover -s .claude/hooks/tests`), not in CI;
      - the trailer.
    - Option A puts this PR in needs-human, because it touches `tools/**` and `justfile`; at Stage 0 every PR is owner-merged anyway. R3 is never loop-reviewed, since R2 is not on main yet (contract §7).
    - Then `gh pr ready <n>`, then `/review-pr-comments`.
    - Hand off with the PR URL, `gh pr checks <n>`, "run `/merge-and-cleanup <n>`", and "after it merges, close gate #<GATE>; R2's convergence (its Task 23) can start". Never merge.
    - Verify: `gh pr view <n> --json isDraft --jq .isDraft` prints `false`; `gh pr checks <n>` shows every check passing.

## Code Structure Decisions

- **Layout.**
  - `tools/src/cmd/brief/{mod,clock,git_section,github,soak,local,phase,docs}.rs` and `tools/src/cmd/checkpoint/{mod,scan,others}.rs`.
  - Shared (contract §2-§4): `tools/src/gh.rs`, `tools/src/git.rs`, `tools/src/merge_ready.rs`.
  - Tests: `tools/tests/{agent_support,checkpoint_parity,brief_parity,skill_contract}.rs`; `tools/tests/common/{agent,fake_gh,jail}.rs`, where `fake_gh` and `jail` are shared with R2 (contract §5); `tools/tests/golden/{brief,checkpoint}/`; and `tools/tests/fixtures/brief/`. The `.gitattributes` rules `tools/tests/fixtures/** -text` and `tools/tests/golden/** -text` cover them.
- **Shared modules and harness.** The contract's §2-§5 (with A1-A3 and A7) fixes the signatures, the environment policy (gh.rs forces nothing; git.rs inherits), the cwd rule (brief and checkpoint run gh in the toplevel), the read-only rule (`is_read` before every spawn), the tool lists and `script_commands`. This plan does not restate them.
- **Toplevel cwd.** Both binaries resolve the toplevel from their cwd first and run every later child (git, gh, docs-check) and every pathspec from it, as both scripts' `cd "$ROOT"`. `run_from_subdirectory` pins it in both ports. The exceptions are brief.sh's: `sh qemu-lock.sh status` runs in the main worktree, handoffs come from `<main>/.remember`, and `rel_path` labels are relative to `<main>` (pinned by `run_from_linked_worktree`).
- **Test repos and the enclosing checkout.** Every scenario asserts that its twin's toplevel is the twin root; `outside_repo` alone runs under `std::env::temp_dir()` with a `GIT_CEILING_DIRECTORIES` ceiling set after `hermetic` (Task 1e).
- **External calls keep the oracle's argv, cwd and env**: gh including `--jq`; `GH_PROMPT_DISABLED` on checkpoint's `pr list --head` only; `GIT_TERMINAL_PROMPT` on fetch and push; docs-check's `docs-check --json` from the toplevel. So the differential compares the fake-gh and FAKE_DOCS call logs.
- **Sequential gh calls, docs-check on a thread.** brief.sh is sequential except for the background docs-check. Parallel gh calls would change the call order and buy little.
- **docs-check as a child of `current_exe()`**, not in-process. This keeps the exit 2/101 parity and isolates a panic. It is the binary the shim chose, so no shim message can appear on its stderr (D2).
- **No `time` crate.** Formatting shells out to `date` (`-r`, then `-d @`), as the script does and as R4 ruling 1 did. Epoch arithmetic is std.
- **No new dependencies.** regex (bytes), serde_json (`preserve_order`, already enabled, `tools/Cargo.toml` L22) and anyhow suffice. signal-hook is not used: the script handled no signals, so having no index restore on a signal is parity.
- **Debug-only test seam.** `AIOS_BRIEF_DOCS_CHECK` replaces the docs-check program only, never its argv or cwd, and is read only under `cfg!(debug_assertions)`. `just tools` builds release, so the shipped binary ignores it.
- **Recording platform.** Goldens are recorded from the oracle on macOS. `PLATFORM_SCENARIOS` lists each scenario whose oracle output depends on the host's tools, with the tool and the oracle line. Today that is `soak_interleaved_no_boots`: BSD awk stops at the missing boots.tsv, while gawk runs END. The port follows the macOS golden. The golden check runs everywhere; the oracle comparison runs on macOS only and prints the skip reason elsewhere.
- **Accepted divergences.** Each is in the module docs and the spec's "R3 as built" paragraph, and each is pinned by a named scenario. The aios-only goldens are listed in `DIVERGENT_SCENARIOS`.
  - **D1:** message prefixes become `brief:` and `checkpoint:` (the R4 ruling 5 precedent), and `--help` prints new text for `aios brief` / `aios checkpoint`, with no script path and no bash note. Pinned: `cli_*`; `cli_help` (both) is aios-only.
  - **D2:** unreachable branches are removed:
    - brief's "jq is not installed" (W234 L629-630);
    - "docs-check unavailable (needs .claude/hooks/aios …)" (L645);
    - the docs-error preference for the shim's `^aios: ` line (L626). The child is `current_exe()`, never the shim, and the binary prints no `aios: ` line.

    "gh is not installed" is kept. Pinned: `gh_missing`, `docs_other_rc_last_line`.
  - **D3:** collation is bytewise (= `LC_ALL=C`) for `sort -u`, the `ls -t` name tie and the soak dir order; `grep -c .` counts non-empty lines bytewise. Pinned: `handoffs`, `knowledge_uncommitted_rename_and_space`.
  - **D4:** a jq `tonumber` beyond 2^53 is parsed exactly as u64 (jq lost precision). Pinned: the `merge_ready` unit table.
  - **D5:** the hint becomes `checkpoint it: (cd '<wt>' && just --justfile '<main>/justfile' --working-directory . checkpoint)`. It works in any worktree, including one branched before R3, and runs main's shim. Pinned: `other_worktree_dirty_from_*` (oracle output through `oracle_renames()`), 12b step 4.
  - **D6:** the stale note comes from `AIOS_SHIM_STALE=1`, instead of grepping docs-check's stderr. The shim exports it on a failed stale rebuild and clears it on every other path (owner decision 9). The text is identical. Pinned: `docs_stale_note`, the Task 8c shim tests.
  - **D7** (a D3 corollary for the Soak section): equal-mtime runs resolve to the first in bytewise path order within a worktree, and to the earlier worktree in porcelain order across worktrees. The oracle's `sort -u` (W234 L420) follows the caller's locale, so the two agree under `LC_ALL=C` and differ only in other locales. Pinned: `soak_equal_mtime_tie`, `soak_equal_mtime_tie_across_worktrees`.
  - **D8:** the secret path regex folds case ASCII-only (`(?i-u)`), matching grep in the C locale and matching invalid UTF-8 path bytes; `ſ` and `K` are not folded. Pinned: the `scan.rs` unit table.
  - **D9** (owner decision 8, H1): a git failure inside the secret scan (any of its four calls, Task 3b) fails closed: the index is restored (`read-tree <pre_index>`), nothing is committed or pushed, and the exit is **always 1**. An unborn HEAD skips the two `log` calls instead of failing. The output is exactly, in order:
    1. the first line `checkpoint: <branch> @ <short sha or none> | wip: not committed (secret scan failed: <first stderr line of the failing call>) | pushed: no (secret scan failed) | handoff: <handoff>`;
    2. no `Allowed by --allow` block and no `--allow matched no flagged path` line (the scan gave no verdict on any path);
    3. the other-worktrees block, unchanged from the script (L258-275);
    4. the script's `Not fully saved: <dirty> uncommitted file(s) and <unpushed> commit(s) not on origin remain only in <ROOT>.` line when dirty or unpushed is non-zero, and its `Not fully saved: <n> other worktree(s) …` line when others is non-zero (L280-283);
    5. `/clear keeps all of it on disk; run /justin:start after.` (L284).

    `Safe to /clear — run /justin:start after` is **never** printed on a scan failure, even when nothing is dirty, unpushed or elsewhere (where the script's L276-278 would print it and exit 0). Pinned: `scan_git_failure_fails_closed` and `scan_git_failure_on_clean_tree` (aios-only), `scan_fails_closed_on_each_git_call` (the four calls), `unborn_head` (differential).
  - **D10** (contract §3): `merge_ready` adds the blocker `labelled needs-human` in position 4, from the `labels` field brief already requests, so no argv changes. Pinned: `pr_labelled_needs_human` (aios-only), `labelled_needs_human_blocks`, `skill_contract.rs`.
  - Internal errors (mktemp, cd) become anyhow → exit 2, as in the script. This is not a divergence.
- **Kept bug-for-bug** (parity; listed so reviewers do not "fix" them):
  - the `git add -A` exit status is unchecked (H2, follow-up);
  - a multi-commit cherry-pick/revert `sequencer` is not detected (H3, follow-up);
  - evil merges and binary files are not scanned, and the scan honours replace refs (follow-up);
  - the push is attempted after a commit failure;
  - the Boots sed fall-through prints the whole row;
  - `unpushed_desc` uses `--remotes` (all remotes) in brief but `--remotes=origin` in checkpoint;
  - `date -r` is tried before `date -d @` (GNU `date -r <epoch>` reads a file of that name);
  - there is no index restore on SIGINT/SIGTERM.

## Dependencies & Risks

- **Depends on:**
  - #234 merged into main, with its brief.sh conflict against #246 resolved (Task 0b checks each section).
  - CI "Tools (host)" (`.github/workflows/ci.yml` L63-86: ubuntu-26.04, `fetch-depth: 0`, no `GH_TOKEN`, `cargo test -p aios-tools`) providing bash, jq ≥ 1.7 and every `ORACLE_TOOLS` and `FAKE_TOOLS` entry (proved in 1h).
  - The lead applying the merged amendments A1-A12 to the scratch contract before 0f (R2's plan: already applied in R2 v2; do not renumber); the owner applying the Task 10e settings change; the 0e probe passing.
  - R2 vendoring from `R3_API_FREEZE` and not merging first (contract §1). R3 depends on nothing from R2 except the agreed `FAKE_CLAUDE_TOOLS` list (A1: R2's 8 entries), fixed at the freeze.
- **Shared files.** The contract §8 table is the ownership and merge rule for every file both branches edit. R3's only extra hunk outside that table is `pub fn parallel_map` in `tools/tests/common/mod.rs`, moved from `docs_check_parity.rs`. R2 adds only a `pub mod` line there, so the union stays trivial. `parallel_map` is not one of the five vendored files (A3).

**Risks:**
- **The guard sees nothing of the compiled push.** A branch-worktree agent on `claude/*` could commit and push, and the pushes go unattended to a public repo.
  - Mitigation: the settings asks per spelling (A6), proved in 0e for `-p` and for a background agent and re-checked in 10f; the Option-A gate on the sources; the binary's own bounds.
  - Residual: an ask rule is a speed bump for a spelling the probe did not cover (e.g. a new cargo alias). `git push origin claude/*` is already allowed for the pushes themselves.
- **The oracle pins the wrong brief.sh** (b47c756/a189f7a lack #246's soak). Mitigation: 0b's per-pattern greps and the ancestor check.
- **A vacuous differential**: jq or bash missing, a fake-gh 99, a missing tool behind `2>/dev/null`, or the oracle failing early identically on both sides.
  - Mitigation: `require_tools()` and the absolute bash; `assert_not_vacuous` (docs calls included); the per-script `jail_covers_every_script_command`; the 1h, 2d and 4e CI log checks; the mutation checks in 3f, 6e and 7c.
- **A port that keeps the caller's cwd, or reads MAIN_WT things from the toplevel.** Mitigation: `run_from_subdirectory` in both ports and `run_from_linked_worktree` in brief (differential), plus their mutation checks; 12b step 5.
- **A test that reaches the real checkout.** An outside-repo scenario built under `target/tmp` would resolve the aios worktree, and checkpoint could commit there. Mitigation: `Scenario::outside_repo()` under `temp_dir()` with a ceiling, the toplevel assertion on every run, and `outside_repo_scenarios_escape_the_checkout`.
- **A tool list that agrees with a blind extractor.** Mitigation: the extractor scans command substitutions inside double quotes, and `script_commands_scans_substitutions_and_compound_starts` pins `paste` (checkpoint L196) and `date` (brief L111).
- **A host tool leaking in** (`/usr/bin/gh` on GitHub's image). Mitigation: the jail (no `/usr/bin` and no `/bin` on PATH), the `GH_*`/`GITHUB_*` strip, `assert_all_matched`, the non-empty-calls check, and `jail_has_no_gh_or_claude`.
- **Host-tool-dependent oracle output** (BSD awk vs mawk/gawk; BSD vs GNU sed/date/stat). Mitigation: one recording platform; CI runs `oracle_reproduces_*` over the full sets (2d, 4e); `PLATFORM_SCENARIOS`, each with a cited cause.
- **A time-dependent golden** (`%cr`, the header, the 7-day window, commit shas). Mitigation: only the five named normalisation rules; dates pinned for setup and every run; mtimes set explicitly; `knowledge_7_days` covers uncommitted notes only.
- **A skill parses changed output.** Mitigation: `skill_contract.rs` runs the skill's own awk and checks the pause cues.
- **A stale main binary committing.** Mitigation: the shim's checkpoint exit 3 on a failed stale rebuild (owner decision 5). A pre-R3 main binary has no `checkpoint` subcommand, exits 2 and writes nothing.
- **main moves `settings.json` under the settings script.** Mitigation: the base check (tip blob = `origin/main` blob) and the A8 recovery, which only the owner performs.
- **CI does not run the Python guard suite.** Mitigation: 10c and 10f run it locally; the PR test plan says so; a follow-up issue.
- **Test runtime.** There are about 600 twin-repo runs (52 + 76 per-scenario runs, 49 + 74 oracle reproductions, 2 × (49 + 74) differential runs, and 2 × 49 for bash 3.2), counting the per-scenario tests, both differentials, the bash 3.2 pass and the oracle reproductions. Mitigation: `parallel_map`, with a budget under 90 s on this Mac.
- **Residual (not R3's):** #213 (the stamp is tied to local HEAD); #241 (guard rule 4 gaps, R5).

## Owner Questions

All of R3's earlier questions are decided (see Decisions Made). Two items are still open **for later**; they are recorded here and not asked now:
1. **Later: the final settings ask spellings** for both branches (R3 Task 10e, R2 OQ11). They are owner edits, applied through a lead-written script that checks its base blob (A8). The 0e probe fixes R3's set, and R2's run of the saved harness fixes R2's. The merge order sequences the two edits: R3's goes on R3's branch before the ship pass, and R2's only after R2's convergence. So they never conflict, and the A8 recovery covers a main that moves underneath.
2. **Later (R2's, before Stage 5): R2 OQ8, widening the Option-A gate** to the shim, guard, settings, workflows, member manifests, `.justfile`, `.gitattributes`, `scripts/agent/loop-config.json` and `scripts/agent/prompts/**`, together with the `loop)` arm's check-then-exec race (contract §6). R3 changes nothing for it.

## Issues Encountered

- **The first draft recorded answers it never received.** The relayed "1. A, 2. Yes" answered #204 (Option A; loop exit 3), not this plan's pause-confirmation and checkpoint-exit-3 questions. Owner decisions 4 and 5 (2026-10-11) now answer those, and the plan follows them.
- **The first draft and R2's plan contradicted each other** on `gh.rs`, `git.rs`, the fake gh, the shim and the merge-ready names. The contract resolves all five. This revision follows it: the former shim helper sub-task is removed, and the fixture keys are re-cast as label → regex tables.
- **Checked against the repo for revision 2 (2026-10-11):**
  - checkpoint.sh calls `paste` (W234 L196, L223, L250).
  - `tools/tests/soak_interleave.rs` runs brief.sh from disk (main L25, L739-749).
  - brief.sh sorts soak dirs with `sort -u` (W234 L420), so D7 is a D3 corollary.
  - `Bash(just -*)` is an **ask** rule (W234 settings L326, inside `ask` L241-341), so both `just --justfile …` fallbacks prompt.
  - Both scripts' `--help` prints their header comments (checkpoint L63, brief L42), which name script paths and bash 3.2.
  - brief.sh's `^aios: ` docs-error step (W234 L626) is unreachable in the port, since no `"aios: "` string exists in `tools/src`.
  - `.claude/CLAUDE.md` Skills rows L287/L289 name the scripts, so they move into Task 10a, because 10f's `.claude` grep runs before the ship pass.
  - The docs_check comment is at L187.
- **Checked against the repo for revision 3 (2026-10-11):**
  - **The `draft` blocker comes first** (W234 brief L219). At 1i both PRs are drafts, so R2's line is `merge-ready: no (draft; gated by #N)`, and a prefix check fails. 1i matches the blocker instead (A4).
  - **Both scripts `cd "$ROOT"`** (brief L52-56, checkpoint L75-79). The docs-check child runs `.claude/hooks/aios docs-check --json` from `$ROOT` (brief L101-105).
  - **No origin/main changes three brief sections:** Git (L125-131), Main CI (`[ -n "$tip" ]`, L265-298) and Soak (`origin/main unknown`, L339-341).
  - **Settings allow `Bash(cargo *)`** (W234 settings L119), with no `cargo run` ask, so `cargo run … checkpoint` would not prompt (A6).
  - **No script line runs `bash`.** checkpoint L271 names it only inside an echoed string. A jailed `Command::new("bash")` resolves from the jail PATH and fails with ENOENT, so the oracle runs by absolute path (A2).
  - **R2's plan (v2) contradicts the FAKE_TOOLS model.** 4a vendors `jail.rs` byte for byte, 23c requires it unchanged, 6a/6b need `env` and `sort` for FAKE_CLAUDE, and 23b points at R3's `agent_support.rs`, which never scans FAKE_CLAUDE. A1 resolves this.
  - **main's `tools/tests/common/mod.rs`** has `#![allow(dead_code)]`, `isolated()` and `unique_dir()`, so the five frozen files can build on it alone (A3, checked in 5c).
  - **#234's head moved** from b47c756 to 8a28365, then to 8253f0d, and merged as 2d4a9f7 on 8fa4c24. Task 0a re-read every anchor there: most brief.sh, shim, spec, guard and `.claude/CLAUDE.md` anchors moved, and this plan and the contract cite the 2d4a9f7 lines (A9).
  - **`.remember/` is gitignored on main and on 8a28365**, so 12c's handoff file leaves `<BW>` clean.
- **Found while revising (not in the critic results):**
  - Under D9, an unborn HEAD would fail every first checkpoint on a new branch (`git log HEAD` fails). So the port skips the two `log` calls when HEAD does not resolve, pinned by `unborn_head`.
  - The D5 hint names main's justfile, which has no `checkpoint` recipe until R3 merges, so the hint cannot be exercised in the real repository before the merge. 12b exercises it in a scratch clone of `<BW>`.
  - `jail::hermetic` strips `AIOS_*`, so FAKE_DOCS finds its root from `$0`, and the twins carry an identical committed `.claude/hooks/aios` that execs `aios-fake-docs` from PATH.
  - A lead-run `just tools` in `<BW>` would write in a branch worktree. So the lead builds into the session scratchpad, with `--manifest-path <BW>/Cargo.toml --locked`.
  - The contract asks for "one worker commit" per amendment, but the two contract copies live on two branches. An amendment is therefore one worker commit per branch, with byte-identical contract copies (checked with `cmp` in 0f).
  - **Revision 2 miscounted the brief scenarios.** Its 4b list holds 69 scenarios, not 61, and checkpoint's 50 was right. With `no_origin_main` and `run_from_subdirectory`, the brief has 71 (69 non-divergent goldens). Every brief count and T() total was corrected (4b, 4c, 7b, 7d, 12e, Risks).
  - **Revision 2's fixture-directory count could not hold:** git cannot commit an empty directory, and three scenarios serve no fixture. 4a now checks that every fixture directory names a scenario.
  - **`Bash(cargo run *checkpoint*)` would refuse Task 3d's old verify** (`cargo run … checkpoint --bogus`) for background workers once the asks are on main. 3d now uses the `cli_unknown` scenario test, and the writer rules forbid `cargo run … checkpoint`.
  - **The 0e probe needs a third verdict.** A stub that is never reached (for example a `cargo` the Bash tool's PATH skips) leaves no marker and no denial; counting that as `refused` would hide a hole. So `inconclusive` fails the verify.
- **Checked against the repo for revision 4 (2026-10-11; the second re-check):**
  - **Two amendment lists, colliding numbers.** R3's A1-A9 and R2 v2's A1-A7 differed in content and numbering (R2's A2 gate via `--search`, A3 three-dot, A4 evals/cases, A5 settings, A6 probe harness, A7 docs gate), and `grep -c 'FAKE_GH_TOOLS\|FAKE_CLAUDE_TOOLS\|evals/cases'` on the scratch contract printed 0. Fixed by the one merged set A1-A12 (Coordination; R2 v2 already uses its numbering), and 0f's per-amendment marker loop before the `cmp`.
  - **FAKE_CLAUDE_TOOLS.** R3 v2 listed the 10-entry contract §5 union under that name (`cp` and `tr` come from the fake gh). R2 6a's script runs 8 tools. A1 now fixes R2's 8 as `[&str; 8]`, and the keyword list is `jail::SH_WORDS` in both plans (R2 6c called a `jail::NOT_COMMANDS` that R3 never defined).
  - **The docs gate.** main `tools/src/cmd/docs_check/checks/knowledge.rs` L62-69 reports every file under `docs/knowledge/plans/` except `README.md` and `_template.md` as `knowledge-hygiene plans-not-empty`, with the file as `file` and `plans-not-empty` as `target` (`Finding::new(CHECK, f, "plans-not-empty", …)`), and they are not baselined. So every "reports 0 new" verify from 0f to 12d (G, 0f, 9b, 10a, 10b, 10f) would fail: DG (A11) replaces them. `docs.yml` fails only on an exit other than 0 or 1, so the PR's Docs check stays green meanwhile.
  - **The extractor.** checkpoint runs `paste` (L196, L223, L250), `head` (L205, L221), `dirname` and `basename` (L74) only inside double-quoted `$(...)`; brief runs `date` only in `"$(date …)"` (L111). Commands also follow `if`/`elif` (L190, L198). brief's echo lines carry escaped Markdown backticks (L123, L136, L365), which must stay literal. Task 1a's lexer spec and the new unit table cover these.
  - **Test repos sit inside the checkout.** `unique_dir` and `isolated()`'s HOME live under `CARGO_TARGET_TMPDIR` = `<repo>/target/tmp`, and `isolated()` removes `GIT_CEILING_DIRECTORIES` (main `tools/tests/common/mod.rs` L15-43). The precedents for escaping are `tools/tests/cli.rs` L187 (a ceiling) and `tools/tests/soak_cli.rs` L311 (`std::env::temp_dir()`).
  - **brief's MAIN_WT reads.** `MAIN_WT` is the first porcelain worktree (W234 brief L59); handoffs (L492), the QEMU lock (L473-477) and `rel_path` (L76-79) use it; the lock directory itself is in the common dir (qemu-lock.sh L77-81), so only the script's location distinguishes the two worktrees.
  - **brief's per-call failures.** L161 (`${GH_WHY:-unknown error}`), L243 (`pr list` stderr), L265-301 (a failed suites query sets `ci_ok=0`, so `run list` is never issued and `runs.err` holds the GraphQL stderr), L551-552 (`|| echo "?"`), L601 (`open PR for M<n>: unknown (GitHub unavailable)`).
  - **D9's open shape.** checkpoint.sh's final lines and exit come from `dirty`, `unpushed` and `others` (L276-285): with all three zero it prints `Safe to /clear` and exits 0, which a failed scan must never do.
  - **`git grep -c` output.** It prints `<path>:<count>` even for one pathspec, so the numeric verifies use `-h`.
  - **`just -*` is an ask** (W234 settings L326), so 9a's `just --list` and `just --justfile … --dry-run` verifies would be refused in a background worker; `just` is on CI's "Tools (host)" image (`extractions/setup-just@v4`, ci.yml), which 9a's shim.rs test relies on, as the existing shim tests do.
  - **gh in a clone of `<BW>`.** Its `origin` is a local path, so gh can resolve neither the default repo nor `{owner}`/`{repo}`; 4d now passes `-R juslee/aios` and literal GraphQL variables.
  - **main moved to 8fa4c24** (#251, #252). Among the files this plan cites, only `.claude/CLAUDE.md` (+6 lines at L51) and the developer guide (after L1600) changed (A9). At 2d4a9f7 the Skills rows are 7 lines lower (L287/L289), not 6.
- **Final fixes (plan frozen, 2026-10-11):**
  - **No committer identity for the oracle and aios runs.** `common::git()` pins the identity for setup only, and `hermetic` keeps `isolated()`'s `GIT_CONFIG_GLOBAL=/dev/null`, so checkpoint's `wip` commit would fail on the CI runner ("unable to auto-detect email address") and record `juslee@<host>` on the Mac (machine-dependent shas, an identity leak). Task 1e's `twin_command` now sets the `aios-test` identity env after `hermetic`, `checkpoint_wip_identity_is_pinned` (1f) and the 3e differential check it, and 2d/4e rule out `PLATFORM_SCENARIOS` for an identity or sha mismatch.
  - **The amendment set disagreed with R2 v2** (eleven amendments here, twelve there; no A12 marker in 0f; §2, §8 and §11 not in the edit list; a renumbering step that R2 v2 had already applied). A12 is now in the list and 0f's marker loop, the renumbering step is dropped, and the contract carries §14.
- **Found in Task 3 (2026-10-12): Task 10f's grep will hit more than the provenance form.** `git grep -n -e 'brief\.sh' -e 'checkpoint\.sh' -- tools` also matches mentions outside the exempt form. These are `tools/src/git.rs:1` (frozen: its fix waits for R2's convergence, or needs a contract amendment), `tools/tests/agent_support.rs` L17, L33 and L690, `tools/tests/common/jail.rs` L34 and L41 (frozen; their `former scripts/agent/...` form lacks `(blob at`), and `tools/tests/shim.rs:623`, besides the docs_check comments that 10b covers. Task 10 must reword these, or exempt them through the provenance form, before 10f's verify can pass.

### Rejected review points

Every R3 and "both" gap of the three critics (journal `wf_fc95b75d-ac8`) and of the 2026-10-11 re-check was checked against W234 (b47c756 and 8a28365), main and the contract. Adopted points are fixed in the task named. The following were rejected or replaced, with the reason.
- **"Drop the shim change, OQ6 = lean"** (cross-plan critic, shim ownership): rejected. Owner decision 9 keeps brief's stale note through `AIOS_SHIM_STALE`.
- **"Keep both fake gh scripts; don't fold"** (cross-plan critic): superseded by contract §5, which defines one shared fake that R3 owns. One fake must log cwd and env for both plans, and two `FAKE_GH` definitions would be legacy. R3's fixtures become label → regex tables instead of semantic argv keys.
- **"R3 stays parity; R2 adds the labelled needs-human blocker as second merger"** (cross-plan critic): rejected per contract §3. R3 owns `merge_ready.rs` and merges first, so the blocker lands once, as D10, with its own aios-only scenario.
- **"R2 owns start/SKILL.md, including L19"** (cross-plan critic): rejected per contract §8. L19 states the merge-ready rule that D10 changes, so R3 edits L19 only, and R2 edits the other lines in its Phase B.
- **"R2 owns all of §2 Freshness, including the checkpoint and AIOS_SHIM_STALE cells"** (cross-plan critic): rejected per contract §8. R3 makes those shim changes and merges first, so it writes the Stale × Other-subcommands cell. R2 adds its `aios loop` column on top and re-reads the table.
- **"Keep R3's spec Order edit conditional on merging first"** (cross-plan critic): no longer conditional, since owner decision 3 fixes the order.
- **"Note in R3 12e that /justin:review's eval-gate finding is expected"** (cross-plan critic, eval hash): not applicable. R3 merges before R2 exists on main, is never loop-reviewed, and uses `/review-pr-comments` (contract §7).
- **"Re-open OQ1/OQ2 and merge OQ2 with R2's OQ12"** (both critics): superseded by owner decisions 4 and 5.
- **"Drop D7"** (R3 critic): reworded instead of dropped. The behaviour is a real divergence in non-C locales (`sort -u` follows the caller's locale), and the contract numbers D9 and D10. So D7 stays as a stated D3 corollary, with a cross-worktree tie scenario added.
- **"Date one knowledge commit relative to now and normalise it"** (R3 critic, `knowledge_7_days`): replaced by the critic's alternative, uncommitted notes only. A now-relative commit date changes every downstream sha in the golden, and the two fixed-epoch scenarios cover committed notes.
- **"Rework 12c to run `<W>/target/tools/release/aios checkpoint` directly"** (R3 critic): not needed. Owner decision 5 keeps `AIOS_TOOLS_BIN` working for checkpoint, so 12c keeps it, set in the session's launch environment. The critic's point that Bash state does not persist is adopted.
- **"`cargo test -p aios-tools --lib -- checkpoint:: gh:: git::`"** (R3 critic, Verify syntax): replaced by one filter per command with an expected count (`T(…)`), so each Verify names one TESTNAME.
- **"A shim.rs test for the old-worktree fallback"** (R3 critic, recipes): adopted in revision 4 as `justfile_recipes_run_mains_shim_from_another_directory` (9a), which runs `just --dry-run` from a directory outside the checkout as a child process. A real pre-R3 worktree is not built: the behaviour lives in the recipe's `justfile_directory()`, which the dry run shows resolved.
- **"Open the draft PR early for the Option-A needs-human route"** (R3 critic, nit): adopted by dropping that clause; 1h's reason is the CI proof.
- **Re-check: the gate verify as `grep -A2 … | grep -c 'gated by #<GATE>'`** (1i point): replaced by the re-check's other form, the merge-and-cleanup awk plus an anchored `[(;] ?gated by #<GATE>([,;)]|$)`. One verify then serves the contract and both plans, and `#12` cannot match `#123`.
- **Re-check: the existing-gate check as `gh issue list --search 'Gate: merge PR #<R2>'`** (single-filer point): replaced by a jq `startswith` filter on the titles of the open needs-human issues. GitHub search tokenises `#` and `:`, so a search can miss or over-match, and the exact prefix cannot.
- **Re-check: "record `cargo run` as an accepted residual"** (ask-spellings point, its alternative): not taken. The asks are added (A6), and the residual left is only spellings the probe does not cover.
- **Re-check: "fold 12d's commit into 11c, or move 11c after 12d"**: done as one commit in 12d, after the real-world checks and the filed follow-ups. Task 11 keeps simplify and audit only.
- **Second re-check, Task 10 order "1. 10h and 10g, background"**: 10h goes first as proposed, but 10g moves after 10a. Two of `skill_contract.rs`'s three tests assert the pause SKILL.md text (`just checkpoint --handoff saved`, the two exit-3 cues) that 10a writes, so 10g before 10a would fail G. The rest is adopted: 10a-10c foreground, 10d last and alone, and 10d's verify proves the parent commit reads no script from disk.
- **Second re-check, D9 "`scan_git_failure_on_clean_tree` (no staged changes and one unpushed commit)"**: split instead. With one unpushed commit the script's own logic already skips `Safe to /clear` (L276, `unpushed != 0`), so that scenario would not pin the new rule. The clean-tree scenario therefore has nothing unpushed, where the script would print `Safe to /clear` and exit 0, and the unpushed commit moves into `scan_git_failure_fails_closed`, which then also pins that a scan failure pushes no pending commit.
- **Second re-check, 9a "alternatively, mark the 9a verify as lead-run"**: not taken. The lead must not run builds or tests in `<BW>` (rule 11), and the shim.rs test makes the check part of G and CI.
- **Second re-check, 4d "`git -C <clone> remote set-url origin git@github.com:juslee/aios.git`"** (one of two options): not taken. It changes the clone's config and makes the test depend on where it runs; `-R juslee/aios` with literal GraphQL variables works from any clone, and each call still passes `is_read` through `run_program`.
- **Second re-check, A7 "R2 judges by `ran-shim-loop` / `ran-exe-loop`"**: refined, not rejected. `run-probe.sh` computes the verdict from any `ran-*-<sub>` marker, so a loop spelling that reaches the fake `cargo` (`ran-cargo-loop`) counts as `ran` too; judging by two named stubs would miss it.
- **Second re-check, the FAKE_CLAUDE post-merge rule ("choose one")**: the chosen rule is the contract §5 one, stated in A1 for both plans: before R3 merges, R3 applies the amendment; after, the amendment lists the one additive R2 line that §1's convergence verify allows. The other option (only before R3 merges) would leave no path for a tool R2's Phase B needs.

## Decisions Made

- **Owner decisions, 2026-10-11** (final; implemented, never re-asked; numbered as the owner gave them):
  1. **#204 = Option A.** R2's changed-paths gate covers every input of the aios binary, which is the shim provenance stamp's list: `tools/**`, `shared/**`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` / `rust-toolchain`, `.cargo/**`, `justfile`. It applies to R2-R5 as well, so this PR goes to needs-human under the loop. The spec keeps calling it the guarantee.
  2. **Loop exit 3, full strictness.** `aios loop merge|review` exit 3 when a stale binary's foreground rebuild fails. A dirty build, a rebuilt-but-not-fresh binary and `AIOS_TOOLS_BIN` are refused with exit 3. A PR build is run directly only for eval or `--dry-run`. R2 owns this (the shim's `loop)` arm).
  3. **R3 owns `tools/src/gh.rs`, `tools/src/git.rs`, `tools/src/merge_ready.rs` and the shared fake-gh harness, and merges first.** R2 is built in parallel against R3's agreed API and merges main after R3 lands. R2 owns the shim's `loop)` arm and the loop's exit-3 strictness. The spec Order (L194 and L41 at 2d4a9f7) becomes R1 → R4 → R3 → R2 → R5 → R5b, and R3's PR records it (Task 10b).
  4. **Pause asks once per pause** (behaviour parity; the former OQ1 = A). The owner adds settings asks for the checkpoint spellings, an owner edit scripted by the lead (Task 10e). Background and `-p` runs are refused. **No guard deny.**
  5. **checkpoint's shim branch exits 3 on a failed stale rebuild only** (the former OQ2 = stale-rebuild-only; the earlier "1. A, 2. Yes" answered #204, not OQ1/OQ2). `AIOS_TOOLS_BIN` and dirty builds stay usable for testing branch builds, and checkpoint does not join the loop's strict arm (Task 8).
  6. **R2 Stage-0 scope.**
     - Attended only: the owner or an attended lead runs `/justin:review` and `/justin:merge`; nothing auto-merges; main stays user-only.
     - No `justin/review` commit statuses (`statuses/` is on the deny list).
     - No `aios loop sweep`: `/merge-and-cleanup` keeps cleanup and the soak copy.
     - The retro (`aios retro`, `/justin:retro`, the metrics line) is deferred until 5 loop-reviewed merges. So R2 has no `metrics.jsonl` writer and brief has no metrics line (R3 adds nothing for it).
     - Kernel-area fixes always go to needs-human.
     - R2's Task-1 probe budget is ≤ $0.50, with fable confirmed in `-p`.
     - R2's first eval run is ≤ $25, and the owner approves the case list before scores are recorded.
  7. **R3 keeps the history-oracle differential tests** after the scripts are deleted (R4 precedent).
  8. **R3 hardening is H1 only:** a git failure inside the checkpoint secret scan fails closed (restore the index, commit nothing, exit 1), as divergence D9 with its own pin scenario. H2 and H3 are filed as a follow-up issue (12d).
  9. **R3 keeps brief's stale note:** the shim exports `AIOS_SHIM_STALE=1` when it runs a stale binary, and clears it on the fresh path and every other path (Task 8, D6).
  10. **The labels `agent-stop` and `hard` exist on GitHub** (created 2026-10-11); R2's label-exists check relies on them.
- **Owner, 2026-10-11 (relayed):** "Do R2 and R3 together": the two branches run in parallel after #234, under the contract.
- **Owner, standing:**
  - Main is user-only (rule 03): agents never merge, and auto-merge is a later stage (agent-loop.md).
  - R2 keeps `/review-pr-comments`, narrowed (contract §8); R3 does not touch it, and uses it in 12e.
  - No legacy: carry only current, used things. The scripts, their `bash -n` allows, the brief.sh fallbacks in skills and the dead jq/shim branches all go, and every shared-module item has a caller (checked in 12d). `FAKE_CLAUDE_TOOLS` ships in R3's `jail.rs` only because `jail::bin` links it and R2, the next merge, tests it (A1).
  - Isolated writers and the lead's fast-forward (rule 11); leads stay in the main checkout.
  - The parity-port method: the oracle is the deleted script's blob from git history, then differential tests, then the script is deleted.
  - Tools crate rules (rule 01): the deps are clap, anyhow, serde, serde_json, regex and signal-hook; `time` only when first used; `#![forbid(unsafe_code)]`.
  - Precompact is retired by #234, so it is out of R3.
- **Revision 4 (2026-10-11):** one merged contract amendment set, A1-A12 (Coordination): R3's A1-A9 with A1 (`FAKE_CLAUDE_TOOLS` = R2's 8, `jail::SH_WORDS`, one tool-change rule), A4 (the `startswith` pre-check in both plans) and A7 (one marker scheme, `bash-bg`, the verdict in `run-probe.sh`) reconciled, plus R2's evals/cases exclusion (A10), docs gate (A11) and own rows (A12). The contract carries it as §14 before either branch commits the contract; R2 v2 already uses this numbering, so R2's plan is not renumbered. It supersedes revision 3's A1-A9 and R2 v2's A1-A7.
- **Oracle pins:** `CHECKPOINT_ORACLE = faf6d20295136489d6ccdfa5a253480a9f757fcc`; `BRIEF_ORACLE = <filled in at Task 0b>`.
- **Settings base:** `SETTINGS_BASE = <filled in at Task 10e>` (A8; re-pinned on recovery).
- **Guard expectation (0c):** `EXPECTED_GUARD = <table filled in at Task 0c>`.
- **Permission probe (0e):** `<CLI version, the 24 rows and any added ask rules, filled in at Task 0e; harness at $R/probe/, A7 scheme>`.
- **API freeze:** `R3_API_FREEZE = <filled in at Task 5c>`.
- **Merge-order gate:** `GATE = <issue number, filled in at Task 1i>`.

- **Task 0 results (2026-10-11, team-build lead, main checkout):**
  - 0a: `<BW>` created from `origin/main` 2d4a9f7; its three verifies pass. Every anchor of this plan and the contract was re-read on 2d4a9f7 and corrected (A9).
  - 0b: `BRIEF_ORACLE=2d4a9f72ebb8cd98a6659d8c75ebec91be23658f` (brief.sh last changed by #234; contains #246; `handoff-` 4, `print_interleaved` 2, `PANIC-LOCK` 2, `Routing log` 2). `CHECKPOINT_ORACLE=faf6d20295136489d6ccdfa5a253480a9f757fcc`, unchanged on `origin/main`.
  - 0c: `EXPECTED_GUARD` (the 2d4a9f7 guard, cwd the main checkout, the same for the main thread and a worker): rows 1, 3, 4, 5, 6, 7, 8, 11 and 12 give none; row 2 asks (a push refspec, or for an agent a commit directory, from a shell expansion); rows 9 and 10 ask with the QEMU "shell expansion" reason. All 14 contract rows match `guard-expected.tsv`.
  - 0d: nine raw gh responses in `$R/gh-capture/`, listed in `SHA256SUMS`.
  - 0e (2026-10-12, claude 2.1.296, the owner's auto-mode user settings plus the probe settings): **pass**. All 23 non-control rows are `refused`, the control is `ran`, and `SHA256SUMS` lists the harness, `probe/claude-version.txt` and `probes.tsv`.
    - The first counted run (kept as `probes-run1-inconclusive.tsv`) failed. Three spellings ran: `sh .claude/hooks/aios checkpoint` (r15), `cargo r -q -p aios-tools -- checkpoint` (r17) and, once the prompt was fixed, `just 'checkpoint'` (r07). r07 and r08 were inconclusive: inline in the prompt, `just 'checkpoint'` reads as English, and the model ran a bare `checkpoint`. The harness fix puts the command in a fenced block, to be run verbatim. The harness was re-saved and re-checksummed, and every row re-run.
    - `asks.txt`, and so Task 10e's owner ask delta, gains five rules: `Bash(just 'checkpoint'*)`, `Bash(just "checkpoint"*)`, `Bash(cargo r *checkpoint*)`, `Bash(sh .claude/hooks/aios checkpoint*)` and `Bash(sh */.claude/hooks/aios checkpoint*)`. It now holds 12 asks, among them the contract §11 four.
    - Known limit (no stop): settings patterns match the command text, so no rule set covers every shell quoting of the recipe name (`just che'ck'point`). Decision 4 makes the asks a control against accidents by cooperative agents, not a guard deny, so this is recorded, not fixed. The Monitor, background-Bash and background-agent rows are all refused, so every probed kind is covered by settings.
- **R3_API_FREEZE = 3296e1d2fb8f7538c2fe292ca6d80699cd001068** (2026-10-12): Task 1 (ecef063), its lead review fix (59afd73: fake gh ordinals follow contract §5) and Task 5 (5c24fab), merged with origin/main 5f97b57 (#256 Cargo.lock, #259 nightly-2026-10-12). Task 1h passed on PR #257's CI (Tools (host) on ubuntu-26.04; jail_covers_every_script_command and oracle_smoke_brief_and_checkpoint ran and passed). R2 vendors gh.rs, git.rs, merge_ready.rs, tests/common/fake_gh.rs and tests/common/jail.rs from this sha.

## Lessons Learned

(to be filled during implementation)
