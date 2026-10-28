---
author: jl + claude
date: 2026-10-27
tags: [tooling, security, agent-loop]
status: draft
phase: tools
milestone: R5
---

# Plan: Tools R5 — Rust `aios guard` in shadow mode (toward R5b)

Draft for the owner. The Fable plan lens was applied by hand on 2026-10-28 (the cloud session has no plan mode, so the ExitPlanMode hook cannot fire); its four must-fix findings are fixed below. Once the owner approves, the plan is committed unchanged as `docs/knowledge/plans/2026-10-27-jl-tools-r5-guard-shadow.md`, the first commit on `claude/project-thread-ex0dhj` (the branch this session may push; it carries R5a, steps 1–8). The second PR (R5, steps 9–11) gets its own `claude/*` branch from main after R5a merges, and carries this plan file forward. The branches are team-build's (harness files). Facts are labelled [VERIFIED] (checked on `main` at `493894f`), [DECIDED] (an owner decision already on record) or [ASSUMED] (inferred, still to check).

## Approach

R5 ports `.claude/hooks/git-push-guard.py` to `aios guard`, then runs both on every PreToolUse call. Python still decides. Rust decides in parallel and logs each comparison, until R5b's bar is met: 1,000 real calls, 0 disagreements. After that R5b moves the hook to `aios guard` and deletes the Python guard and its tests (discussion §3, R5/R5b rows) [DECIDED].

Three choices shape the plan:

1. **R5 is an exact port.** Rust reproduces Python's decision (level, exit code and reason text) on every input, the known #241 gaps included. Those gaps are pinned as table rows at their *current* decision, tagged `gap-241-*`. The fixes come after the cutover (R5c), against the table, with Python gone. If a fix lands during shadow, every input it touches shows up as a disagreement, and "0 disagreements" stops measuring parity. See Decision D1, which is the one choice that changes the goal.
2. **The shadow is a hook program, not a second guard.** `aios hook guard-shadow` is registered `async: true` on the same matcher as the Python guard. It never prints or decides, and like `route-shadow` it fails open. Inside one process it runs the Rust decision in-process and then the Python guard as a child, with the same payload and environment, so both see the same repository state. It does not try to read the real hook's decision (hooks can't see each other's output). Instead it reruns the deciding guard file, the one at `$CLAUDE_PROJECT_DIR/.claude/hooks/git-push-guard.py`, which is the exact command the settings run.
3. **The count is per pair of builds.** Each record carries the Rust binary's git hash and the Python guard's blob hash. A call counts toward 1,000 only for the pair being certified: main's installed binary and main's guard blob. A change to either one starts a new count.

**Key gaps found during exploration** (the doc and issues compared with the code; full list in "Gaps between the doc and the code" below):

- `aios guard` does not exist. There is no `tools/src/cmd/guard/`, and `Command` in `tools/src/main.rs` has no `Guard` variant [VERIFIED]. The only R5 code that exists is the shim's `guard` branch (#203), which is tested in `tools/tests/shim.rs` but called by nothing. PreToolUse still runs `/usr/bin/python3 .../git-push-guard.py` directly [VERIFIED].
- The port is about twice the size the doc records. The guard is 3,523 lines (the doc says 1,817), and its tests are 2,470 lines with 160 test methods in 16 classes (the doc says 709 lines and "the 55 unit tests") [VERIFIED].
- No CI job or recipe runs the Python guard's tests. `.github/workflows/`, the `justfile` and `setup-dev-env.sh` never mention them [VERIFIED]. So the oracle the shadow trusts is checked by nothing. Step 1 fixes that first.

## Progress

- [ ] Step 0: Owner decisions and preconditions (no code)
  - [ ] 0a: D1–D6 below are decided and recorded in this plan
  - [ ] 0b: The Python guard is frozen for the shadow window. Any change to `git-push-guard.py` during shadow goes to Rust in the same PR, and the count restarts. It has changed twice since 2026-09-22, last in #234 on 2026-10-11 [VERIFIED], so a freeze is realistic
  - [x] 0d: The owner confirms the round-2/3 corpora and the replay tool exist in `<common>/aios-agent/port-inputs/` on the Mac (steps 2a(ii), 8b). Without them, those sources drop out and the plan says so
  - [ ] 0e: Writers for steps 1a, 3–7, 9 and 11b are spawned in the foreground (`run_in_background: false`): `tools/src/cmd/guard/**`, `tools/src/cmd/hook/**`, `.github/` and `.claude/**` edits sit behind permission asks that a background agent is refused (rule 11, Spawning agents)
  - [ ] 0c: Order against R2. #282 (R2a) is still a draft, so R2's changed-paths gate is not in place [VERIFIED]. The owner merges by hand anyway, so R5 does not have to wait for the gate (D5)
- [x] Step 1: Put the oracle under CI
  - [x] 1a: Add `/usr/bin/python3 -m unittest discover -s .claude/hooks/tests -v` (the form the suite's header gives; the path form `python3 -m unittest .claude/hooks/tests/test_git_push_guard.py` fails with `ValueError: Empty module name` [VERIFIED]) as a `just guard-test` recipe, and run that recipe in `.github/workflows/ci.yml`, job `tools` ("Tools (host)", ubuntu-26.04). Files: `justfile`, `.github/workflows/ci.yml`. It is deleted again in R5b, together with the guard
  - [x] 1b: Acceptance: `just guard-test` prints `Ran 160 tests` and `OK` locally [VERIFIED on Linux, Python 3.13, 2026-10-28], and the `Tools (host)` job on the branch's PR is green (`gh pr checks <n>`). Fix any test that fails on Linux at its root cause; never skip it (user rule)
- [x] Step 2: Fixture bundle and recorder (lesson 2026-09-24, steps 1–2)
  - [x] 2a: `tools/tests/fixtures/guard/cases.jsonl`, one case per line: payload, environment (`AIOS_TEAM`, `CLAUDE_PROJECT_DIR`, the `GIT_*` set), a repository-setup recipe id, and the expected `{level, exit, reason}`. Sources:
    - (i) every assertion in the 160 Python tests;
    - (ii) the round-2 and round-3 corpora from `<common>/aios-agent/port-inputs/` [ASSUMED present on the owner's Mac; not checked], with secrets scrubbed;
    - (iii) every repro in #241 and its five comments, tagged `gap-241-<section>` and pinned at today's decision.
  - [x] 2b: Repository setups as code in `tools/tests/guard_support/` (temporary repo, bare remote, worktrees `agent-<id>` / `wf_…`, the lock directory, symlinks, a stopped rebase). These reuse `tools/tests/common` and `hook_support`
  - [x] 2c: An ignored recorder test (`AIOS_BLESS_GOLDENS=1`, the docs_check_parity convention) that runs the Python guard on every case and writes `tools/tests/golden/guard/*.json`. `.gitattributes` already marks golden and fixtures `-text` [VERIFIED]
  - [x] 2d: Acceptance: `AIOS_BLESS_GOLDENS=1 cargo test -p aios-tools --test guard_parity -- --ignored record` writes the goldens, and `cargo test -p aios-tools --test guard_parity goldens_match_python_tests` prints `test result: ok` (the recorder output matches the Python tests' own expectations for every (i) case)
- [x] Step 3: Shell model (`tools/src/cmd/guard/shell.rs`)
  - [x] 3a: Port the parser: quotes, `$(…)`, backticks, heredocs, `case`, `sh -c`, `eval`, `read`, functions, `MAX_DEPTH = 8`. It keeps the per-word expansion state (bare or quoted) and passes nested commands as argument vectors (lesson 2026-10-11 guard-wrapped-commands)
  - [x] 3b (landed with the analyzer in step 4): Directory tracking exactly as Python does it (plain `cd`/`pushd` at the head, lexical `-C`). The #241 §1–2 gaps stay as pinned rows
  - [x] 3c (landed with the analyzer in step 4): The parse-failure fallback exactly as Python does it (keyword gate), pending D4
  - [x] 3d: Patterns compile through `pyre::compile`, strings use `pystr` semantics (lesson 2026-09-24)
  - [x] 3e: Acceptance: `cargo test -p aios-tools guard::shell` prints `test result: ok`, with the parser table tests ported from the Python shell-model classes
- [x] Step 4: Git and gh rules (`git_rules.rs`, `gh_rules.rs`)
  - [x] 4a: Git option abbreviation and cluster rules, aliases, script files, and push-destination resolution (`push.default`, upstream, `remote.<name>.push`/`.mirror`). Config reads go through `guard::repo` (not `tools/src/git.rs`; see Issues), run with the same environment as Python's `Repo` (`GIT_OPTIONAL_LOCKS=0`, `LC_ALL=C`; guard L1004 [VERIFIED])
  - [x] 4b: The deny and ask tables from the docstring, gh api/GraphQL writes, `HOME_REPO`
  - [x] 4c: Acceptance: `cargo test -p aios-tools --lib -- cmd::guard::git_rules cmd::guard::gh_rules cmd::guard::tests` prints `test result: ok`
- [x] Step 5: Protected-sed rule (`sed_rule.rs`), with the `ProtectedSed` class and the #241 sed comment's rows. Acceptance: `cargo test -p aios-tools guard::sed_rule` prints `test result: ok`
- [x] Step 6: Team rules 1–4 (`team_rules.rs`, and `spawn_rules.rs` for rule 3 and EnterWorktree; see Issues): the QEMU wrapper (with `run --` wrapped analysis), toolchain and kills, spawn shape (frontmatter YAML read), and placement. Acceptance: `cargo test -p aios-tools --lib -- cmd::guard::team_rules cmd::guard::spawn_rules cmd::guard::tests` prints `test result: ok`
- [ ] Step 7: `aios guard` subcommand (`tools/src/cmd/guard/mod.rs`; shared files: `tools/src/main.rs` gains `Command::Guard`, `tools/src/cmd/mod.rs` gains `pub mod guard`)
  - [ ] 7a: Hook protocol identical to `emit()`/`main()`. JSON goes on stdout, and a deny also prints its reason on stderr and exits 2. An unreadable payload or an internal error produces an ask with the same reason prefixes (`git-push-guard: `, `git-push-guard error: `). The `git-push-guard` name stays in the reason text until R5b, so the reasons match byte for byte
  - [ ] 7b: `decide()` returns a `Verdict` carrying rule tags (`push`, `gh`, `sed`, `r1`…`r4`, `parse-fallback`), which the shadow logs for coverage
  - [ ] 7c: Verify: `cargo fmt --check -p aios-tools && cargo clippy -p aios-tools --all-targets -- -D warnings && cargo test -p aios-tools`
- [ ] Step 8: Differential test (lesson step 3)
  - [ ] 8a: `tools/tests/guard_parity.rs` runs every case through both `aios guard` and the live Python guard at HEAD (it still exists during R5, so no history oracle is needed) and compares level, exit code and reason bytes. It also checks the goldens
  - [ ] 8b: A local-only replay of the 3,502-command history (and #234's 18,948-evaluation set if it is in port-inputs) through the same comparison. The results go in this plan, and the commands are never committed (secrets)
  - [ ] 8c: Acceptance: `cargo test -p aios-tools --test guard_parity` prints `test result: ok` (0 differences on the committed corpus), and the local replay's summary line in this plan reads `0 differences`, before shadow starts
- [ ] Step 9: Shadow hook (`tools/src/cmd/hook/guard_shadow.rs`; shared files: `tools/src/cmd/hook/mod.rs` gains `Cmd::GuardShadow` and its `name()` and `OnError::Open` arms; the shim `.claude/hooks/aios` is unchanged, since it passes any `hook <prog>` through)
  - [ ] 9a: Reads the payload and runs `guard::decide` in-process. It then runs the Python guard as `/usr/bin/python3 "$CLAUDE_PROJECT_DIR"/.claude/hooks/git-push-guard.py` (the exact command settings run; a PATH python would make false disagreements) through `proc` with the same stdin and environment, under a 20 s limit (the registered timeout). It compares `{level, exit, reason}`
  - [ ] 9b: Takes a state fingerprint before and after the pair (HEAD and branch of the cwd repository, whether the lock directory exists, the `worktrees/` names). A disagreement with a changed fingerprint is logged as `skew` and excluded from the count, but kept for triage. This is needed because the hook is async and the tool may run at the same time
  - [ ] 9c: Appends one record per call to `<common>/aios-agent/hooks/guard-shadow.jsonl` with `append_jsonl` (D3). Fields: `ts`, `rust_bin` (the binary's git hash), `py_blob`, `tool`, `ctx` (`main`/`agent:<type>`, team), `rules`, `rust`, `python`, `outcome` (`agree`/`disagree`/`skew`/`py-timeout`/`py-error`/`rust-error`), `ms_rust`, `ms_py`. The command text and both reasons are logged **only on `disagree`/`skew`** (secrets; the file sits under `.git`, untracked, and is never committed)
  - [ ] 9d: Never prints, always exits 0, and a broken log directory is a no-op, as in `route-shadow`. Tests in `tools/tests/hook_guard_shadow.rs` with a fake Python guard (the test points the interpreter path at it)
  - [ ] 9e: Acceptance: `cargo test -p aios-tools --test hook_guard_shadow` prints `test result: ok`
- [ ] Step 10: Progress report: `aios guard shadow-report [--pair current]`
  - [ ] 10a: For the certified pair (main's `installed/aios` hash + `git rev-parse origin/main:.claude/hooks/git-push-guard.py`): counted calls, the current streak since the last `disagree`, the excluded counts by outcome, coverage per rule tag and context, and the disagreements with their command and both reasons. It prints `R5b: ready` only when the D2 bar is met
  - [ ] 10b: A one-line `guard shadow: N/1000` in `aios brief`, with its goldens under `tools/tests/golden/brief/` updated (D6)
  - [ ] 10c: Acceptance: on an empty log, `aios guard shadow-report` prints `R5b: not ready` and `0/1000`; `cargo test -p aios-tools --test brief_parity` prints `test result: ok`
- [ ] Step 11: Wiring (owner-run, rule 11 Config)
  - [ ] 11a: The owner applies the exact settings change: one PreToolUse entry `{"matcher": "Bash|Monitor|Agent|EnterWorktree", "hooks": [{"type": "command", "command": "\"$CLAUDE_PROJECT_DIR\"/.claude/hooks/aios hook guard-shadow", "async": true}]}`. The Python entry is unchanged. The entry has `"timeout": 30`, which bounds the whole shadow run above the child's 20 s limit
  - [ ] 11b: Rule 11 Guard text (`.claude/rules/11-teams.md`) only: "the guard keeps no state files" becomes "the deciding guard keeps no state; the shadow writes a log it never reads". The docstring of `.claude/hooks/git-push-guard.py` is left unchanged, so the step 0b freeze and the certified blob hash hold; R5b deletes the file anyway
  - [ ] 11d: Acceptance (after the owner applies 11a): one Bash call in a fresh session adds a record with `"outcome":"agree"` to `<common>/aios-agent/hooks/guard-shadow.jsonl` (`tail -1` of the file), and `aios guard shadow-report` shows `1/1000`
  - [ ] 11c: Discussion doc corrections (Gaps section below). The ship pass covers the CLAUDE.md layout, tests and counts
- [ ] Step 12: Simplifier, `/audit-loop`, `gh pr ready`, hand-off (rules 02, 03, 11)

**R5b** (a separate PR, after `shadow-report` says ready): the settings entry moves from Python to `.claude/hooks/aios guard`, the shadow entry and `guard_shadow.rs` go, `git-push-guard.py`, its tests and step 1's CI line are deleted, the parity test becomes golden-only (R1 precedent), and the reason prefix is renamed in one deliberate golden bless. **R5c:** the #241 fixes, flipping the `gap-241-*` rows.

## Code Structure Decisions

- **The guard sources are under `tools/src/cmd/guard/`**, which is covered by the existing `Edit(**/tools/src/cmd/guard/**)` ask [VERIFIED]. The shadow hook sits under `cmd/hook/`, which is covered by `Edit(**/tools/src/cmd/hook/**)` [VERIFIED].
- **The shadow is evaluated in-process with the Python guard as a child, not by changing the Python guard.** The deciding guard stays byte-identical to what has been reviewed, and the shadow can't change a decision.
- **Async, not sync.** The Python guard already costs a Python start-up on every Bash call. A sync shadow would add the shim (about 68 ms idle, about 130 ms at load 138, per the discussion doc) plus a second Python run to every call. The price of async is state skew, which step 9b handles.
- **Shadow runs main's binary through the shim** (`aios hook`: a stale or dirty binary falls back to nothing). So only merged Rust code gets certified, and a stale binary just pauses the count.

## Dependencies & Risks

- **Depends on:** R3's shared modules (merged in #257) [VERIFIED]. `pyre`/`pystr` [VERIFIED]. The port-inputs corpora [ASSUMED].
- **Risk: one PR is very large** (probably 6–8k lines of Rust for a 3.5k-line parser and its rule set) [ASSUMED estimate]. See D6 for splitting it.
- **Risk: the 1,000 calls are mostly `none`** (builds, `ls`, reads). The count would then say little about deny and ask paths. D2 adds a coverage floor.
- **Risk: Python skew across checkouts.** A session started in an old worktree runs that worktree's Python guard (settings load from the session's checkout, rule 03) [VERIFIED rule text], while the shadow's Rust is main's. Records whose `py_blob` is not main's are logged but not counted.
- **Risk: an agent can write the log** (it is under `.git`). It is evidence for a cutover decision, not enforcement. `shadow-report` also cross-checks the count against the transcripts' Bash-call count for the same window [ASSUMED feasible].
- **Behaviour change at R5b, to record now:** the guard will come from main's build (shim) rather than from the session checkout's file. A worktree made before a guard change gets the new rules at once, where today it keeps the old ones until it merges main.

## Owner decisions needed

- **D1 — #241 timing. DECIDED 2026-10-27 by the owner: (a).** (a) exact port in R5, fixes in R5c after the cutover. (b) Fixes in R5, and the shadow classifies each Rust-stricter result as `intended`. That needs Rust to know which rule a fix changed, and #241 §3 and §7 *loosen* false denies, so it can't be a simple "Rust stricter is fine" rule.
- **D2 — the R5b bar. DECIDED 2026-10-27 by the owner: the recommendation.** Recommended: 1,000 *consecutive* counted calls on one certified pair, 0 `disagree`, with an exact match on level, exit code and reason bytes; at least 50 non-`none` decisions, at least 1 hit on each of `push`, `gh`, `sed`, `r1`…`r4`; and calls from at least one lead, one solo session and one agent type with `isolation`. Any `disagree` resets the streak, and `skew` must be under 1% of calls.
- **D3 — the log path. DECIDED 2026-10-27 by the owner: the recommendation.** The doc says `.git/aios-agent/guard-shadow.jsonl`. The hooks' convention is `<common>/aios-agent/hooks/` (`Ctx::state_dir`) [VERIFIED]. Recommended: the hooks directory, with the doc amended.
- **D4 — parse failure DECIDED 2026-10-27 by the owner: the recommendation.** (#241, 2026-10-11 comment): (a) full grammar, (b) always ask, (c) keep the keyword gate and pin it. Recommended: (c) for R5, since parity requires it, with (a) or (b) in R5c.
- **D5 — order against R2. DECIDED 2026-10-27 by the owner: the recommendation.** Recommended: start R5 now. The changed-paths gate guards the loop's auto-merge, which doesn't exist yet, and the owner merges R5 by hand.
- **D6 — PR shape. DECIDED 2026-10-27 by the owner: the recommendation.** The doc says one PR per port. Recommended: R5a (steps 1–8: port, parity, no wiring) and R5 (steps 9–11: shadow, report, wiring), so each audit stays a reviewable size. The shadow only starts once both are in.

## Gaps between the doc and the code

Doc = `docs/knowledge/discussions/2026-09-22-jl-rust-agent-tools.md`.

1. **The subcommand table lists `aios guard` as if it were a subcommand.** It does not exist (no module, no clap variant) [VERIFIED]. The table should mark it "R5, not yet built".
2. **Context table and R5 row: the size is stale.** The guard is 3,523 lines, not 1,817, and its tests are 2,470 lines with 160 methods, not 709 lines and 55 tests [VERIFIED]. "The 55 unit tests ported as table tests" understates the work threefold.
3. **The oracle is untested in CI.** The doc's parity story assumes the old tool is correct, but nothing runs `test_git_push_guard.py` [VERIFIED]. Step 1.
4. **§1 Protection and the R1 answer say there are `Edit` *and* `Write` asks on `tools/src/cmd/guard/**` and `loop/**`.** `.claude/settings.json` has only `Edit(...)` rules. There is no `Write(...)` rule anywhere, and the `target/tools/**` "deny for Edit and Write" is only `Edit(**/target/tools/**)` [VERIFIED]. If Claude Code's `Edit` rules now cover `Write` too, the doc only needs a wording fix; if not, it is a real gap. [ASSUMED either way: needs one headless probe like R1's.] The owner runs that probe on the Mac before R5a's audit; its result decides the doc wording in step 11c.
5. **Open Questions still list R2's changed-paths gate scope (Option A or B) as open.** Rule 03 records Option A as decided on #204 [VERIFIED rule text]. The doc should record the answer.
6. **The log path** differs from the hooks' state-dir convention (D3) [VERIFIED].
7. **"The guard keeps no state files"** (rule 11, guard docstring) has to be reworded once a shadow log exists. The doc doesn't mention the conflict.
8. **#241 says its gaps are "deferred to R5, the Rust port that rebuilds the parser"**, which means they get fixed in R5. The doc's R5 row requires decisions equal to Python's. The two conflict, and neither says which wins (D1).
9. **"1,000 real calls with 0 disagreements" is undefined.** It doesn't say whether the calls are consecutive, which build they count against, which fields have to match, or whether there is any coverage floor (D2).
10. **The doc doesn't say how the shadow gets Python's decision.** Hooks can't read each other's output, so the shadow has to rerun Python, and that brings the async state-skew question (step 9b).
11. **The guard grew new rules after the doc was written.** The protected-sed rule, `run --` wrapped analysis and `wf_` worktrees (#234) are not in the R5 row's list of rules to port. The guard docstring is the current spec [VERIFIED].
12. **Port inputs.** The doc says the round-2/3 corpora and the replay tool are in `port-inputs/`. I could not check that from here [ASSUMED].
13. **The R5b behaviour change** (main's build decides, not the session checkout's file) is not recorded (Dependencies & Risks).

## Issues Encountered

- 2026-10-28, step 1: the path form `python3 -m unittest .claude/hooks/tests/test_git_push_guard.py` fails with `ValueError: Empty module name`; the recipe uses the discover form (Fable plan gate finding 1).
- 2026-10-28, step 2, fixture format: `cases.jsonl` holds one *scenario* per line (one Python test method), not one case per line. A scenario is the test's own ordered steps, operations (`git`, `git-fail`, `write`, `copy`, `mkdir`, `chmod`, `symlink`, `remove`, `rmdir`) and checks (`bash`, `tool`, raw `stdin`), on a named setup (`repo` = `RepoCase.setUp`, `team` = `TeamCase.setUp`). Tests mutate the repository between assertions, so a check is only meaningful at its position in the test.
- 2026-10-28, step 2, source (i): the scenarios were extracted by running the 160 tests under instrumentation (git and filesystem calls, `decide`, `decide_tool` and hook runs recorded in order, temp paths as `{ROOT}`), not by hand. The tests' `HOME_REPO` mock (`example/aios`) is mapped to the real `juslee/aios`, so every check runs through the unmodified hook. `goldens_match_python_tests` proves the mapping: all 2,343 recorded decisions equal what the tests saw. The extractor, the compactor, the pinned-scenario generator and the fill script are not in the repository. They are in the project's shared files (`r5-step2/`, with a README and SHA256SUMS), and the owner copies them to `port-inputs/r5/` (remote tools cannot write under `.git`).
- 2026-10-28, step 2: in-process `decide()` reads the process environment, so the extractor records the variables a test patched (for example `CLAUDE_PROJECT_DIR` under `mock.patch.dict`) as the check's `env`. A first extraction missed this and gave 2 mismatches in `WrappedCommands.test_the_lock_is_the_genuine_wrappers_own`.
- 2026-10-28, step 2: 25 checks are not expressible as a hook call and are left out of the fixture; they become Rust unit tests in steps 3-7: `HookProtocol.test_internal_error_asks` and `TeamRules.test_hook_internal_error_in_a_team_rule_asks` (an injected exception), 21 direct `temp_worktree()` calls in `WorkflowWorktrees`, and 2 checks of `GhCommands.test_scratchpad_body_files_are_the_agents_own` under a `claude_tmp_roots` mock.
- 2026-10-28, step 2: scenario roots live under the system temp directory, never under the checkout. The guard treats paths containing `.claude` specially and rule 03 puts every checkout under `.claude/worktrees/`, so a root under `target/tmp` would change decisions.
- 2026-10-28, step 2, sources (ii) and (iii): `Corpus.adv_r2`, `Corpus.adv_r3` (73 adversarial commands each) and `Corpus.stage0_cases` (89) from port-inputs, and 27 `Gap241.*` scenarios from #241 and its five comments, are pinned at today's Python decision. Each #241 row keeps the issue's `issue_now` and `issue_expected` for R5c. 11 rows differ today from the issue's "now" column, because the repro here differs from the issue's setup or the guard changed after the issue; the pin is today's decision either way.
- 2026-10-28, step 2: `python_reproduces_goldens` runs the Python guard on all 2,694 checks (about 65 s) and found the goldens byte-stable across runs.

- 2026-10-28, step 3: the Python tests have no shell-model class: they test the parser only through decisions. So 3e's table is recorded from the guard itself. `tests/fixtures/guard/shell.jsonl` holds 1,727 parses (`{text, depth, commands | error}`) from the guard's own `parse_commands`: 84 edge cases written for the parser's branches, and every fixture command up to 8 KiB (the nesting tests' 90 KB wrapped commands are left out). `guard::shell::tests::parses_like_python` checks the Rust parser against it (all equal), and `guard_parity`'s `python_parses_like_the_shell_table` checks the table against the live Python guard; `record_shell` rewrites it.
- 2026-10-28, step 3: 3b (`cd`/`pushd` and `-C` tracking) and 3c (the keyword-gate fallback) are Python's `Analyzer.dispatch` and `decide`, which call into every rule. They land with the analyzer in step 4, so each commit compiles without placeholder rule functions.
- 2026-10-28, step 3: Python-semantics findings for the later steps [VERIFIED on CPython 3.13.17, Unicode 15.1]: Python's `\w` is exactly `[\p{L}\p{N}_]`, so `\b` is emulated with that class (the `regex` crate's `\b` also counts marks); under `re.I` only `i` (U+0130, U+0131), `k` (U+212A) and `s` (U+017F) have non-ASCII equivalents, and the `regex` crate's `(?i)` lacks the two `i` ones, so `re.I` patterns spell those classes out; `str.isdigit()` adds 128 code points to the decimal digits (`pystr::is_digit`). One divergence is accepted: `$'\uD800'` gives a lone surrogate in Python and U+FFFD in Rust (a `String` cannot hold a surrogate).
- 2026-10-28, step 4: the guard's git queries are `guard::repo`, not `tools/src/git.rs`. Python's `Repo.run` decodes git's output as strict UTF-8 and lets the `UnicodeDecodeError` escape to the hook's catch-all (an ask with the exception's text), removes five `GIT_*` variables, and kills git after 4 s; `git.rs` decodes lossily and has none of that, so reusing it would change decisions.
- 2026-10-28, step 4: the analyzer lands without its sed and team hooks (`check_sed` and its run positions, `check_team_command`, `check_qemu_script`, `check_placement`); steps 5 and 6 add them with their modules, so every commit compiles without placeholder functions. `fnmatch.fnmatchcase` (push destinations with `*`) is ported as CPython 3.13's `_translate` plus a matcher that reads each class body as `re` reads it, so empty ranges, unclosed `[` and escaped set operators behave as in Python.
- 2026-10-28, step 4: the acceptance line as first written (`cargo test -p aios-tools guard::git_rules guard::gh_rules`) is not a valid cargo command (cargo takes one filter; more go after `--`). It now names the three test paths after `--`; `cmd::guard::tests` holds 24 `decide()` rows recorded from the Python guard outside any repository.
- 2026-10-28, step 5: the sed rule's `ProtectedSed` and #241 sed rows are scenarios in `cases.jsonl`, so step 8 checks them end to end; step 5's unit tests pin the helpers (`run_positions`, the GNU/BSD option readings, the script readers, the protected-path tests) and 17 `decide()` rows against the Python guard. Two helpers are new and shared: `guard::fnmatch` (moved out of `git_rules`, now an O(pattern x name) matcher, since sed globs are the agent's own input) and `pystr::repr` (`{a!r}` in sed's reasons; `str.isprintable` from the `regex` crate's Unicode 16.0 categories).
- 2026-10-28, step 6: the tool-call rules (rule 3's spawn shape and rule 4 rows 1-3, Python's `decide_tool` with its frontmatter reader) are `guard::spawn_rules`, not part of `team_rules`. They take a JSON payload and the environment instead of a command, share nothing with the command rules, and step 7's hook calls them directly. The acceptance line names both modules and the `decide()` rows (105 team-rule rows for the owner, an agent and a team lead, all equal to Python). An `OSError` from reading an agent file is named from `io::ErrorKind`, which covers every errno CPython maps to a subclass that `open` and `read` can raise.
- 2026-10-28, step 6: `lock_message` formats the lock's `eta` with the system `date` (`date -d @N` on Linux, `date -r N` on macOS), because the crate has no time dependency and forbids `unsafe`, so it cannot call `localtime`. Both read `TZ` and the zone database through the C library [VERIFIED on Linux: equal to Python for 0, ±1e11, the largest year glibc formats, and an out-of-range value, which both reject]. If `date` cannot run, the raw `eta` is shown, which is Python's text for an unparsable value. A variable whose value is not UTF-8 is read with replacement characters (Python keeps the bytes); it only reaches `CLAUDE_PROJECT_DIR`, which Claude Code sets from the project's path.
- 2026-10-28, step 3: the oracle's Python version matters at the margins. The goldens and CI use the container's `/usr/bin/python3` (3.13); the owner's Mac runs `/usr/bin/python3` from the Command Line Tools [ASSUMED 3.9]. Unicode table versions (3.9 is Unicode 13.0, the `regex` crate 16.0) and exception names (`re.error` vs `PatternError`) differ only on inputs no real command uses; the shadow's disagreement log is where any real one would show.

## Decisions Made

- 2026-10-27, D2–D6 (owner): each recommendation as written above: the D2 bar, the hooks state directory for the log, the keyword-gate fallback pinned for R5, R5 starts before R2's gate, and the split into R5a and R5.
- 2026-10-27, D1 (owner): R5 is an exact port of the Python guard. The #241 gaps are pinned at today's decisions as `gap-241-*` table rows and fixed in R5c, after the R5b cutover.

## Lessons Learned

(to be filled during implementation)
